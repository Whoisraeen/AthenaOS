//! ELF dynamic linker for Linux binary compatibility.
//!
//! Provides `ld-linux.so`-equivalent functionality: shared library loading,
//! symbol resolution via GNU hash tables, relocation processing, lazy PLT
//! binding, and dlopen/dlsym/dlclose runtime loading API.
//!
//! This is part of RaeenOS's Linux compatibility layer — it runs *inside*
//! the kernel to link Linux ELF binaries against their shared libraries,
//! not a userspace ld.so clone.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::elf_loader::{
    self, DynEntry, Elf64Dyn, Elf64Header, Elf64Phdr, Elf64Rela, Elf64Sym, ElfError, ElfRelocation,
    ElfSymbol, LoadedObject, LoadedSegment, TlsImage, DT_FINI, DT_FINI_ARRAY, DT_FINI_ARRAYSZ,
    DT_FLAGS, DT_FLAGS_1, DT_GNU_HASH, DT_HASH, DT_INIT, DT_INIT_ARRAY, DT_INIT_ARRAYSZ, DT_JMPREL,
    DT_NEEDED, DT_NULL, DT_PLTGOT, DT_PLTREL, DT_PLTRELSZ, DT_RELA, DT_RELAENT, DT_RELASZ,
    DT_RPATH, DT_SONAME, DT_STRSZ, DT_STRTAB, DT_SYMENT, DT_SYMTAB, ELFCLASS64, ELFDATA2LSB,
    ELF_MAGIC, EM_X86_64, PT_DYNAMIC, PT_GNU_RELRO, PT_INTERP, PT_LOAD, PT_TLS, R_X86_64_64,
    R_X86_64_COPY, R_X86_64_DTPMOD64, R_X86_64_DTPOFF64, R_X86_64_GLOB_DAT, R_X86_64_IRELATIVE,
    R_X86_64_JUMP_SLOT, R_X86_64_NONE, R_X86_64_PC32, R_X86_64_RELATIVE, R_X86_64_TPOFF64,
    STB_GLOBAL, STB_WEAK,
};

// ═══════════════════════════════════════════════════════════════════════════════
// Dynamic tag extensions
// ═══════════════════════════════════════════════════════════════════════════════

const DT_RUNPATH: u64 = 29;
const DT_REL: u64 = 17;
const DT_RELSZ: u64 = 18;
const DT_RELENT: u64 = 19;
const DT_DEBUG: u64 = 21;
const DT_SYMBOLIC: u64 = 16;

// ═══════════════════════════════════════════════════════════════════════════════
// Library search paths
// ═══════════════════════════════════════════════════════════════════════════════

const DEFAULT_SEARCH_PATHS: &[&str] = &[
    "/lib",
    "/lib/x86_64-linux-gnu",
    "/usr/lib",
    "/usr/lib/x86_64-linux-gnu",
    "/usr/local/lib",
];

// ═══════════════════════════════════════════════════════════════════════════════
// GNU Hash table
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct GnuHashTable {
    pub nbuckets: u32,
    pub symoffset: u32,
    pub bloom_size: u32,
    pub bloom_shift: u32,
    pub bloom: Vec<u64>,
    pub buckets: Vec<u32>,
    pub chains: Vec<u32>,
}

impl GnuHashTable {
    pub fn parse(data: &[u8], offset: usize) -> Option<Self> {
        if offset + 16 > data.len() {
            return None;
        }

        let nbuckets = u32::from_le_bytes(data[offset..offset + 4].try_into().ok()?);
        let symoffset = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().ok()?);
        let bloom_size = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().ok()?);
        let bloom_shift = u32::from_le_bytes(data[offset + 12..offset + 16].try_into().ok()?);

        let mut pos = offset + 16;

        let mut bloom = Vec::with_capacity(bloom_size as usize);
        for _ in 0..bloom_size {
            if pos + 8 > data.len() {
                return None;
            }
            bloom.push(u64::from_le_bytes(data[pos..pos + 8].try_into().ok()?));
            pos += 8;
        }

        let mut buckets = Vec::with_capacity(nbuckets as usize);
        for _ in 0..nbuckets {
            if pos + 4 > data.len() {
                return None;
            }
            buckets.push(u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?));
            pos += 4;
        }

        let max_sym = buckets.iter().copied().max().unwrap_or(0);
        let mut chains = Vec::new();
        if max_sym >= symoffset {
            let chain_start = max_sym - symoffset;
            let mut i = 0u32;
            loop {
                if pos + 4 > data.len() {
                    break;
                }
                let val = u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?);
                chains.push(val);
                pos += 4;
                i += 1;
                if val & 1 != 0 || i > chain_start + 1024 {
                    break;
                }
            }
        }

        Some(Self {
            nbuckets,
            symoffset,
            bloom_size,
            bloom_shift,
            bloom,
            buckets,
            chains,
        })
    }

    fn gnu_hash(name: &[u8]) -> u32 {
        let mut h: u32 = 5381;
        for &b in name {
            h = h.wrapping_mul(33).wrapping_add(b as u32);
        }
        h
    }

    pub fn lookup(&self, name: &str, symbols: &[ElfSymbol]) -> Option<usize> {
        let name_bytes = name.as_bytes();
        let hash = Self::gnu_hash(name_bytes);

        if self.bloom_size > 0 {
            let word_idx = ((hash / 64) % self.bloom_size) as usize;
            if word_idx >= self.bloom.len() {
                return None;
            }
            let bloom_word = self.bloom[word_idx];
            let bit1 = 1u64 << (hash % 64);
            let bit2 = 1u64 << ((hash >> self.bloom_shift) % 64);
            if bloom_word & bit1 == 0 || bloom_word & bit2 == 0 {
                return None;
            }
        }

        let bucket_idx = (hash % self.nbuckets) as usize;
        if bucket_idx >= self.buckets.len() {
            return None;
        }
        let mut sym_idx = self.buckets[bucket_idx] as usize;
        if sym_idx == 0 {
            return None;
        }

        loop {
            if sym_idx >= symbols.len() {
                return None;
            }
            let chain_idx = sym_idx.checked_sub(self.symoffset as usize)?;
            if chain_idx >= self.chains.len() {
                return None;
            }

            let chain_val = self.chains[chain_idx];
            if (chain_val | 1) == (hash | 1) {
                if symbols[sym_idx].name == name {
                    return Some(sym_idx);
                }
            }

            if chain_val & 1 != 0 {
                break;
            }
            sym_idx += 1;
        }

        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shared Library representation
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct SharedLibrary {
    pub name: String,
    pub soname: Option<String>,
    pub base_addr: u64,
    pub load_size: u64,
    pub entry_point: u64,
    pub segments: Vec<LoadedSegment>,
    pub symbols: Vec<ElfSymbol>,
    pub relocations: Vec<ElfRelocation>,
    pub plt_relocations: Vec<ElfRelocation>,
    pub dynamic: Vec<DynEntry>,
    pub needed: Vec<String>,
    pub rpath: Option<String>,
    pub runpath: Option<String>,
    pub gnu_hash: Option<GnuHashTable>,
    pub got_addr: Option<u64>,
    pub plt_got_addr: Option<u64>,
    pub init_func: Option<u64>,
    pub fini_func: Option<u64>,
    pub init_array: Option<(u64, u64)>,
    pub fini_array: Option<(u64, u64)>,
    pub tls: Option<TlsImage>,
    pub relro_start: Option<u64>,
    pub relro_size: Option<u64>,
    pub ref_count: u32,
    pub strtab_offset: u64,
    pub strtab_size: u64,
    pub symtab_offset: u64,
    pub symtab_entsize: u64,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shared Library Cache
// ═══════════════════════════════════════════════════════════════════════════════

pub struct SharedLibraryCache {
    libraries: BTreeMap<String, SharedLibrary>,
    soname_map: BTreeMap<String, String>,
}

impl SharedLibraryCache {
    pub fn new() -> Self {
        Self {
            libraries: BTreeMap::new(),
            soname_map: BTreeMap::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&SharedLibrary> {
        self.libraries.get(name).or_else(|| {
            self.soname_map
                .get(name)
                .and_then(|real_name| self.libraries.get(real_name))
        })
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut SharedLibrary> {
        if self.libraries.contains_key(name) {
            self.libraries.get_mut(name)
        } else if let Some(real_name) = self.soname_map.get(name).cloned() {
            self.libraries.get_mut(&real_name)
        } else {
            None
        }
    }

    pub fn insert(&mut self, lib: SharedLibrary) {
        let name = lib.name.clone();
        if let Some(ref soname) = lib.soname {
            self.soname_map.insert(soname.clone(), name.clone());
        }
        self.libraries.insert(name, lib);
    }

    pub fn remove(&mut self, name: &str) -> Option<SharedLibrary> {
        let lib = self.libraries.remove(name);
        if let Some(ref l) = lib {
            if let Some(ref soname) = l.soname {
                self.soname_map.remove(soname);
            }
        }
        lib
    }

    pub fn contains(&self, name: &str) -> bool {
        self.libraries.contains_key(name) || self.soname_map.contains_key(name)
    }

    pub fn loaded_count(&self) -> usize {
        self.libraries.len()
    }

    pub fn library_names(&self) -> Vec<String> {
        self.libraries.keys().cloned().collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lazy binding / PLT resolver
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct PltEntry {
    got_offset: u64,
    symbol_name: String,
    library: String,
    resolved: bool,
    resolved_addr: u64,
}

struct LazyResolver {
    entries: Vec<PltEntry>,
}

impl LazyResolver {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn add_entry(&mut self, got_offset: u64, sym_name: &str, library: &str) {
        self.entries.push(PltEntry {
            got_offset,
            symbol_name: String::from(sym_name),
            library: String::from(library),
            resolved: false,
            resolved_addr: 0,
        });
    }

    fn resolve(&mut self, index: usize, addr: u64) -> bool {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.resolved = true;
            entry.resolved_addr = addr;
            true
        } else {
            false
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// DynLinker — main dynamic linker
// ═══════════════════════════════════════════════════════════════════════════════

pub struct DynLinker {
    cache: SharedLibraryCache,
    global_symbols: BTreeMap<String, SymbolResolution>,
    search_paths: Vec<String>,
    ld_library_path: Vec<String>,
    next_load_addr: u64,
    aslr_seed: u64,
    aslr_enabled: bool,
    loading_stack: Vec<String>,
    lazy_resolver: LazyResolver,
    tls_offset: u64,
    init_order: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SymbolResolution {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub binding: u8,
    pub sym_type: u8,
    pub library: String,
}

#[derive(Debug, Clone)]
pub struct LinkResult {
    pub entry_point: u64,
    pub base_addr: u64,
    pub interp: Option<String>,
    pub libraries_loaded: Vec<String>,
    pub init_functions: Vec<u64>,
}

impl DynLinker {
    pub fn new(aslr_enabled: bool) -> Self {
        let mut search_paths = Vec::new();
        for p in DEFAULT_SEARCH_PATHS {
            search_paths.push(String::from(*p));
        }

        Self {
            cache: SharedLibraryCache::new(),
            global_symbols: BTreeMap::new(),
            search_paths,
            ld_library_path: Vec::new(),
            next_load_addr: 0x7000_0000,
            aslr_seed: 0xABCD_EF01_2345_6789,
            aslr_enabled,
            loading_stack: Vec::new(),
            lazy_resolver: LazyResolver::new(),
            tls_offset: 0,
            init_order: Vec::new(),
        }
    }

    fn aslr_offset(&mut self) -> u64 {
        if !self.aslr_enabled {
            return 0;
        }
        self.aslr_seed ^= self.aslr_seed << 13;
        self.aslr_seed ^= self.aslr_seed >> 7;
        self.aslr_seed ^= self.aslr_seed << 17;
        self.aslr_seed & 0x1FFF_F000
    }

    fn allocate_base(&mut self, size: u64) -> u64 {
        let base = self.next_load_addr + self.aslr_offset();
        let aligned = (base + 0xFFF) & !0xFFF;
        self.next_load_addr = aligned + ((size + 0xFFF) & !0xFFF);
        aligned
    }

    pub fn set_ld_library_path(&mut self, paths: &str) {
        self.ld_library_path = paths
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|s| String::from(s))
            .collect();
    }

    pub fn add_search_path(&mut self, path: &str) {
        self.search_paths.push(String::from(path));
    }

    fn build_search_order(&self, rpath: Option<&str>, runpath: Option<&str>) -> Vec<String> {
        let mut order = Vec::new();
        if let Some(rp) = rpath {
            for p in rp.split(':').filter(|s| !s.is_empty()) {
                order.push(String::from(p));
            }
        }
        for p in &self.ld_library_path {
            order.push(p.clone());
        }
        if let Some(rp) = runpath {
            for p in rp.split(':').filter(|s| !s.is_empty()) {
                order.push(String::from(p));
            }
        }
        for p in &self.search_paths {
            order.push(p.clone());
        }
        order
    }

    pub fn find_library(
        &self,
        name: &str,
        rpath: Option<&str>,
        runpath: Option<&str>,
    ) -> Option<String> {
        if name.contains('/') {
            return Some(String::from(name));
        }

        let search = self.build_search_order(rpath, runpath);
        for dir in &search {
            let path = format!("{}/{}", dir, name);
            if crate::vfs::open_path(&path).is_some() {
                return Some(path);
            }
        }

        None
    }

    fn read_strtab_entry(data: &[u8], strtab_off: usize, strtab_size: usize, idx: usize) -> String {
        if idx >= strtab_size {
            return String::new();
        }
        let start = strtab_off + idx;
        let mut end = start;
        while end < data.len() && end < strtab_off + strtab_size && data[end] != 0 {
            end += 1;
        }
        String::from_utf8_lossy(&data[start..end]).into_owned()
    }

    fn parse_dynamic_section(data: &[u8], phdr: &Elf64Phdr) -> Vec<DynEntry> {
        let mut entries = Vec::new();
        let offset = phdr.offset as usize;
        let size = phdr.filesz as usize;
        let mut pos = 0;
        while pos + 16 <= size {
            let off = offset + pos;
            if off + 16 > data.len() {
                break;
            }
            let tag = u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
            let val = u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap());
            if tag == DT_NULL {
                break;
            }
            entries.push(DynEntry { tag, val });
            pos += 16;
        }
        entries
    }

    fn find_dyn_val(dynamic: &[DynEntry], tag: u64) -> Option<u64> {
        dynamic.iter().find(|e| e.tag == tag).map(|e| e.val)
    }

    fn parse_symbols_from_dynamic(
        data: &[u8],
        symtab_file_off: usize,
        strtab_file_off: usize,
        strtab_size: usize,
        gnu_hash: Option<&GnuHashTable>,
        max_syms: usize,
    ) -> Vec<ElfSymbol> {
        let mut symbols = Vec::new();
        let count = if let Some(gh) = gnu_hash {
            let max_bucket = gh.buckets.iter().copied().max().unwrap_or(0) as usize;
            let chain_end = max_bucket + gh.chains.len();
            chain_end.max(gh.symoffset as usize + gh.chains.len())
        } else {
            max_syms
        };

        for i in 0..count {
            let off = symtab_file_off + i * 24;
            if off + 24 > data.len() {
                break;
            }
            let name_idx = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
            let info = data[off + 4];
            let shndx = u16::from_le_bytes(data[off + 6..off + 8].try_into().unwrap());
            let value = u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap());
            let size = u64::from_le_bytes(data[off + 16..off + 24].try_into().unwrap());

            let name = Self::read_strtab_entry(data, strtab_file_off, strtab_size, name_idx);
            symbols.push(ElfSymbol {
                name,
                value,
                size,
                binding: info >> 4,
                sym_type: info & 0xf,
                section_index: shndx,
            });
        }
        symbols
    }

    fn parse_rela_table(data: &[u8], offset: usize, size: usize) -> Vec<ElfRelocation> {
        let mut relocs = Vec::new();
        let count = size / 24;
        for i in 0..count {
            let off = offset + i * 24;
            if off + 24 > data.len() {
                break;
            }
            let r_offset = u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
            let r_info = u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap());
            let r_addend = i64::from_le_bytes(data[off + 16..off + 24].try_into().unwrap());
            relocs.push(ElfRelocation {
                offset: r_offset,
                rel_type: (r_info & 0xffffffff) as u32,
                symbol_index: (r_info >> 32) as u32,
                addend: r_addend,
            });
        }
        relocs
    }

    pub fn load_shared_library(
        &mut self,
        name: &str,
        data: &[u8],
    ) -> Result<SharedLibrary, ElfError> {
        if let Some(lib) = self.cache.get(name) {
            return Ok(lib.clone());
        }

        if self.loading_stack.contains(&String::from(name)) {
            return Err(ElfError::CircularDependency);
        }
        self.loading_stack.push(String::from(name));

        let header = elf_loader::ElfLoader::parse_header(data)?;
        let phdrs = elf_loader::ElfLoader::parse_program_headers(data, &header)?;

        let vaddr_min = phdrs
            .iter()
            .filter(|p| p.p_type == PT_LOAD)
            .map(|p| p.vaddr)
            .min()
            .unwrap_or(0);
        let vaddr_max = phdrs
            .iter()
            .filter(|p| p.p_type == PT_LOAD)
            .map(|p| p.vaddr + p.memsz)
            .max()
            .unwrap_or(0);
        let load_size = vaddr_max - vaddr_min;
        let base_addr = self.allocate_base(load_size) - vaddr_min;

        let mut segments = Vec::new();
        for phdr in &phdrs {
            if phdr.p_type != PT_LOAD {
                continue;
            }
            let file_offset = phdr.offset as usize;
            let file_size = phdr.filesz as usize;
            let mem_size = phdr.memsz as usize;
            if file_offset + file_size > data.len() {
                self.loading_stack.retain(|n| n != name);
                return Err(ElfError::LoadFailed);
            }
            let mut seg_data = alloc::vec![0u8; mem_size];
            seg_data[..file_size].copy_from_slice(&data[file_offset..file_offset + file_size]);
            segments.push(LoadedSegment {
                vaddr: base_addr + phdr.vaddr,
                memsz: phdr.memsz,
                filesz: phdr.filesz,
                flags: phdr.flags,
                data: seg_data,
            });
        }

        let mut dynamic = Vec::new();
        for phdr in &phdrs {
            if phdr.p_type == PT_DYNAMIC {
                dynamic = Self::parse_dynamic_section(data, phdr);
                break;
            }
        }

        let strtab_offset_virt = Self::find_dyn_val(&dynamic, DT_STRTAB).unwrap_or(0);
        let strtab_size = Self::find_dyn_val(&dynamic, DT_STRSZ).unwrap_or(0) as usize;
        let symtab_offset_virt = Self::find_dyn_val(&dynamic, DT_SYMTAB).unwrap_or(0);
        let _syment = Self::find_dyn_val(&dynamic, DT_SYMENT).unwrap_or(24);

        let strtab_file_off = strtab_offset_virt as usize;
        let symtab_file_off = symtab_offset_virt as usize;

        let gnu_hash_virt = Self::find_dyn_val(&dynamic, DT_GNU_HASH);
        let gnu_hash_table =
            gnu_hash_virt.and_then(|addr| GnuHashTable::parse(data, addr as usize));

        let max_syms = if strtab_file_off > symtab_file_off && symtab_file_off > 0 {
            (strtab_file_off - symtab_file_off) / 24
        } else {
            512
        };

        let symbols = Self::parse_symbols_from_dynamic(
            data,
            symtab_file_off,
            strtab_file_off,
            strtab_size,
            gnu_hash_table.as_ref(),
            max_syms,
        );

        let rela_offset = Self::find_dyn_val(&dynamic, DT_RELA).unwrap_or(0) as usize;
        let rela_size = Self::find_dyn_val(&dynamic, DT_RELASZ).unwrap_or(0) as usize;
        let relocations = if rela_size > 0 {
            Self::parse_rela_table(data, rela_offset, rela_size)
        } else {
            Vec::new()
        };

        let jmprel_offset = Self::find_dyn_val(&dynamic, DT_JMPREL).unwrap_or(0) as usize;
        let jmprel_size = Self::find_dyn_val(&dynamic, DT_PLTRELSZ).unwrap_or(0) as usize;
        let plt_relocations = if jmprel_size > 0 {
            Self::parse_rela_table(data, jmprel_offset, jmprel_size)
        } else {
            Vec::new()
        };

        let mut needed = Vec::new();
        let mut soname = None;
        let mut rpath = None;
        let mut runpath = None;
        let mut got_addr = None;
        let mut init_func = None;
        let mut fini_func = None;
        let mut init_array = None;
        let mut fini_array = None;

        for entry in &dynamic {
            match entry.tag {
                DT_NEEDED => {
                    let lib_name = Self::read_strtab_entry(
                        data,
                        strtab_file_off,
                        strtab_size,
                        entry.val as usize,
                    );
                    if !lib_name.is_empty() {
                        needed.push(lib_name);
                    }
                }
                DT_SONAME => {
                    soname = Some(Self::read_strtab_entry(
                        data,
                        strtab_file_off,
                        strtab_size,
                        entry.val as usize,
                    ));
                }
                DT_RPATH => {
                    rpath = Some(Self::read_strtab_entry(
                        data,
                        strtab_file_off,
                        strtab_size,
                        entry.val as usize,
                    ));
                }
                DT_RUNPATH => {
                    runpath = Some(Self::read_strtab_entry(
                        data,
                        strtab_file_off,
                        strtab_size,
                        entry.val as usize,
                    ));
                }
                DT_PLTGOT => {
                    got_addr = Some(base_addr + entry.val);
                }
                DT_INIT => {
                    init_func = Some(base_addr + entry.val);
                }
                DT_FINI => {
                    fini_func = Some(base_addr + entry.val);
                }
                DT_INIT_ARRAY => {
                    init_array = Some((base_addr + entry.val, 0));
                }
                DT_INIT_ARRAYSZ => {
                    if let Some((addr, _)) = init_array {
                        init_array = Some((addr, entry.val));
                    }
                }
                DT_FINI_ARRAY => {
                    fini_array = Some((base_addr + entry.val, 0));
                }
                DT_FINI_ARRAYSZ => {
                    if let Some((addr, _)) = fini_array {
                        fini_array = Some((addr, entry.val));
                    }
                }
                _ => {}
            }
        }

        let mut relro_start = None;
        let mut relro_size = None;
        let mut tls = None;
        for phdr in &phdrs {
            match phdr.p_type {
                PT_GNU_RELRO => {
                    relro_start = Some(base_addr + phdr.vaddr);
                    relro_size = Some(phdr.memsz);
                }
                PT_TLS => {
                    tls = Some(TlsImage {
                        image_addr: base_addr + phdr.vaddr,
                        image_size: phdr.filesz,
                        mem_size: phdr.memsz,
                        alignment: phdr.align,
                        dtv_offset: self.tls_offset,
                    });
                    self.tls_offset += phdr.memsz;
                }
                _ => {}
            }
        }

        let lib = SharedLibrary {
            name: String::from(name),
            soname,
            base_addr,
            load_size,
            entry_point: base_addr + header.entry,
            segments,
            symbols,
            relocations,
            plt_relocations,
            dynamic,
            needed,
            rpath,
            runpath,
            gnu_hash: gnu_hash_table,
            got_addr,
            plt_got_addr: got_addr,
            init_func,
            fini_func,
            init_array,
            fini_array,
            tls,
            relro_start,
            relro_size,
            ref_count: 1,
            strtab_offset: strtab_offset_virt,
            strtab_size: strtab_size as u64,
            symtab_offset: symtab_offset_virt,
            symtab_entsize: 24,
        };

        self.cache.insert(lib.clone());
        self.loading_stack.retain(|n| n != name);
        Ok(lib)
    }

    pub fn resolve_symbol_bfs(
        &self,
        name: &str,
        exclude_lib: Option<&str>,
    ) -> Option<SymbolResolution> {
        if let Some(sym) = self.global_symbols.get(name) {
            if exclude_lib.map_or(true, |ex| sym.library != ex) {
                return Some(sym.clone());
            }
        }

        for lib_name in self.cache.library_names() {
            if exclude_lib.map_or(false, |ex| lib_name == ex) {
                continue;
            }
            if let Some(lib) = self.cache.get(&lib_name) {
                let found = if let Some(ref gh) = lib.gnu_hash {
                    gh.lookup(name, &lib.symbols)
                } else {
                    lib.symbols.iter().position(|s| {
                        s.name == name
                            && !s.is_undefined()
                            && (s.binding == STB_GLOBAL || s.binding == STB_WEAK)
                    })
                };

                if let Some(idx) = found {
                    let sym = &lib.symbols[idx];
                    if !sym.is_undefined() {
                        return Some(SymbolResolution {
                            name: sym.name.clone(),
                            address: lib.base_addr + sym.value,
                            size: sym.size,
                            binding: sym.binding,
                            sym_type: sym.sym_type,
                            library: lib_name.clone(),
                        });
                    }
                }
            }
        }
        None
    }

    pub fn register_global_symbols(&mut self, lib_name: &str) {
        let lib = match self.cache.get(lib_name) {
            Some(l) => l.clone(),
            None => return,
        };

        for sym in &lib.symbols {
            if (sym.binding == STB_GLOBAL || sym.binding == STB_WEAK)
                && !sym.name.is_empty()
                && !sym.is_undefined()
            {
                let existing = self.global_symbols.get(&sym.name);
                let should_insert = match existing {
                    None => true,
                    Some(e) => e.binding == STB_WEAK && sym.binding == STB_GLOBAL,
                };
                if should_insert {
                    self.global_symbols.insert(
                        sym.name.clone(),
                        SymbolResolution {
                            name: sym.name.clone(),
                            address: lib.base_addr + sym.value,
                            size: sym.size,
                            binding: sym.binding,
                            sym_type: sym.sym_type,
                            library: String::from(lib_name),
                        },
                    );
                }
            }
        }
    }

    pub fn apply_relocations_for(&mut self, lib_name: &str) -> Result<(), ElfError> {
        let lib = self
            .cache
            .get(lib_name)
            .ok_or(ElfError::LibraryNotFound)?
            .clone();
        let mut segments = lib.segments.clone();

        for reloc in &lib.relocations {
            self.apply_single_relocation(&mut segments, reloc, &lib.symbols, lib.base_addr)?;
        }

        for reloc in &lib.plt_relocations {
            self.apply_single_relocation(&mut segments, reloc, &lib.symbols, lib.base_addr)?;
        }

        if let Some(lib_mut) = self.cache.get_mut(lib_name) {
            lib_mut.segments = segments;
        }
        Ok(())
    }

    fn apply_single_relocation(
        &self,
        segments: &mut [LoadedSegment],
        reloc: &ElfRelocation,
        symbols: &[ElfSymbol],
        base_addr: u64,
    ) -> Result<(), ElfError> {
        let target_addr = base_addr + reloc.offset;

        let segment = segments
            .iter_mut()
            .find(|s| target_addr >= s.vaddr && target_addr < s.vaddr + s.memsz);
        let segment = match segment {
            Some(s) => s,
            None => return Ok(()),
        };
        let seg_offset = (target_addr - segment.vaddr) as usize;

        match reloc.rel_type {
            R_X86_64_RELATIVE => {
                let value = (base_addr as i64 + reloc.addend) as u64;
                self.write_u64(segment, seg_offset, value);
            }
            R_X86_64_64 => {
                let sym_value = self.resolve_reloc_symbol(symbols, reloc, base_addr)?;
                let value = (sym_value as i64 + reloc.addend) as u64;
                self.write_u64(segment, seg_offset, value);
            }
            R_X86_64_GLOB_DAT => {
                let sym_value = self.resolve_reloc_symbol(symbols, reloc, base_addr)?;
                self.write_u64(segment, seg_offset, sym_value);
            }
            R_X86_64_JUMP_SLOT => {
                let sym_value = self.resolve_reloc_symbol(symbols, reloc, base_addr)?;
                self.write_u64(segment, seg_offset, sym_value);
            }
            R_X86_64_COPY => {
                // R_X86_64_COPY: zero-fill; the real data is in the defining lib
                let sym = symbols
                    .get(reloc.symbol_index as usize)
                    .ok_or(ElfError::SymbolNotFound)?;
                let copy_size = sym.size as usize;
                if seg_offset + copy_size <= segment.data.len() {
                    for i in 0..copy_size {
                        segment.data[seg_offset + i] = 0;
                    }
                }
            }
            R_X86_64_PC32 => {
                let sym_value = self.resolve_reloc_symbol(symbols, reloc, base_addr)?;
                let value = (sym_value as i64 + reloc.addend - target_addr as i64) as u32;
                if seg_offset + 4 <= segment.data.len() {
                    segment.data[seg_offset..seg_offset + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
            R_X86_64_TPOFF64 => {
                let sym = symbols
                    .get(reloc.symbol_index as usize)
                    .ok_or(ElfError::SymbolNotFound)?;
                let value = (sym.value.wrapping_add(self.tls_offset) as i64 + reloc.addend) as u64;
                self.write_u64(segment, seg_offset, value);
            }
            R_X86_64_DTPMOD64 => {
                self.write_u64(segment, seg_offset, 1);
            }
            R_X86_64_DTPOFF64 => {
                let sym = symbols
                    .get(reloc.symbol_index as usize)
                    .ok_or(ElfError::SymbolNotFound)?;
                let value = (sym.value as i64 + reloc.addend) as u64;
                self.write_u64(segment, seg_offset, value);
            }
            R_X86_64_IRELATIVE => {
                let value = (base_addr as i64 + reloc.addend) as u64;
                self.write_u64(segment, seg_offset, value);
            }
            R_X86_64_NONE => {}
            _ => {
                return Err(ElfError::UnsupportedRelocation);
            }
        }
        Ok(())
    }

    fn resolve_reloc_symbol(
        &self,
        symbols: &[ElfSymbol],
        reloc: &ElfRelocation,
        base_addr: u64,
    ) -> Result<u64, ElfError> {
        let sym = symbols
            .get(reloc.symbol_index as usize)
            .ok_or(ElfError::SymbolNotFound)?;

        if sym.is_undefined() || sym.name.is_empty() {
            if !sym.name.is_empty() {
                if let Some(resolved) = self.resolve_symbol_bfs(&sym.name, None) {
                    return Ok(resolved.address);
                }
            }
            if sym.binding == STB_WEAK {
                return Ok(0);
            }
            return Err(ElfError::SymbolNotFound);
        }
        Ok(base_addr + sym.value)
    }

    fn write_u64(&self, segment: &mut LoadedSegment, offset: usize, value: u64) {
        if offset + 8 <= segment.data.len() {
            segment.data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
    }

    pub fn setup_lazy_plt(&mut self, lib_name: &str) {
        let lib = match self.cache.get(lib_name) {
            Some(l) => l.clone(),
            None => return,
        };

        for (i, reloc) in lib.plt_relocations.iter().enumerate() {
            if reloc.rel_type == R_X86_64_JUMP_SLOT {
                if let Some(sym) = lib.symbols.get(reloc.symbol_index as usize) {
                    let got_entry = lib.base_addr + reloc.offset;
                    self.lazy_resolver.add_entry(got_entry, &sym.name, lib_name);
                }
            }
        }
    }

    pub fn resolve_lazy(&mut self, plt_index: usize) -> Option<u64> {
        let entry = self.lazy_resolver.entries.get(plt_index)?.clone();
        if entry.resolved {
            return Some(entry.resolved_addr);
        }

        let resolved = self.resolve_symbol_bfs(&entry.symbol_name, None)?;
        self.lazy_resolver.resolve(plt_index, resolved.address);
        Some(resolved.address)
    }

    pub fn collect_init_functions(&self, lib_name: &str) -> Vec<u64> {
        let mut inits = Vec::new();
        let lib = match self.cache.get(lib_name) {
            Some(l) => l,
            None => return inits,
        };
        if let Some(addr) = lib.init_func {
            inits.push(addr);
        }
        if let Some((array_addr, array_size)) = lib.init_array {
            let count = array_size / 8;
            for i in 0..count {
                inits.push(array_addr + i * 8);
            }
        }
        inits
    }

    pub fn collect_fini_functions(&self, lib_name: &str) -> Vec<u64> {
        let mut finis = Vec::new();
        let lib = match self.cache.get(lib_name) {
            Some(l) => l,
            None => return finis,
        };
        if let Some((array_addr, array_size)) = lib.fini_array {
            let count = array_size / 8;
            for i in (0..count).rev() {
                finis.push(array_addr + i * 8);
            }
        }
        if let Some(addr) = lib.fini_func {
            finis.push(addr);
        }
        finis
    }

    pub fn link_executable(&mut self, name: &str, data: &[u8]) -> Result<LinkResult, ElfError> {
        let lib = self.load_shared_library(name, data)?;
        self.register_global_symbols(name);

        let needed = lib.needed.clone();
        let rpath = lib.rpath.clone();
        let runpath = lib.runpath.clone();

        let mut loaded = Vec::new();
        loaded.push(String::from(name));

        let mut queue = needed.clone();
        while let Some(dep_name) = queue.pop() {
            if self.cache.contains(&dep_name) {
                if let Some(dep) = self.cache.get_mut(&dep_name) {
                    dep.ref_count += 1;
                }
                continue;
            }

            let dep_path = self.find_library(&dep_name, rpath.as_deref(), runpath.as_deref());

            if let Some(path) = dep_path {
                let dep_data = self.read_library_data(&path);
                if let Some(ref dd) = dep_data {
                    if let Ok(dep_lib) = self.load_shared_library(&dep_name, dd) {
                        self.register_global_symbols(&dep_name);
                        for sub_dep in &dep_lib.needed {
                            if !self.cache.contains(sub_dep) {
                                queue.push(sub_dep.clone());
                            }
                        }
                        loaded.push(dep_name.clone());
                    }
                }
            }
        }

        for lib_name in &loaded {
            let _ = self.apply_relocations_for(lib_name);
        }

        let mut init_functions = Vec::new();
        for lib_name in loaded.iter().rev() {
            let inits = self.collect_init_functions(lib_name);
            init_functions.extend(inits);
        }

        let entry = self.cache.get(name).map(|l| l.entry_point).unwrap_or(0);
        let base = self.cache.get(name).map(|l| l.base_addr).unwrap_or(0);

        Ok(LinkResult {
            entry_point: entry,
            base_addr: base,
            interp: None,
            libraries_loaded: loaded,
            init_functions,
        })
    }

    fn read_library_data(&self, path: &str) -> Option<Vec<u8>> {
        let inode = crate::vfs::open_path(path)?;
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        let mut offset = 0;
        loop {
            let n = inode.read_at(offset, &mut buf);
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            offset += n;
        }
        if data.is_empty() {
            None
        } else {
            Some(data)
        }
    }

    // ── dlopen / dlsym / dlclose API ────────────────────────────────────

    pub fn dlopen(&mut self, name: &str, data: &[u8]) -> Result<u64, ElfError> {
        if let Some(lib) = self.cache.get_mut(name) {
            lib.ref_count += 1;
            return Ok(lib.base_addr);
        }

        let lib = self.load_shared_library(name, data)?;
        let base = lib.base_addr;
        self.register_global_symbols(name);
        self.apply_relocations_for(name)?;
        Ok(base)
    }

    pub fn dlsym(&self, handle: u64, symbol: &str) -> Result<u64, ElfError> {
        if handle == 0 {
            return self
                .resolve_symbol_bfs(symbol, None)
                .map(|s| s.address)
                .ok_or(ElfError::SymbolNotFound);
        }

        for name in self.cache.library_names() {
            if let Some(lib) = self.cache.get(&name) {
                if lib.base_addr == handle {
                    if let Some(ref gh) = lib.gnu_hash {
                        if let Some(idx) = gh.lookup(symbol, &lib.symbols) {
                            let sym = &lib.symbols[idx];
                            if !sym.is_undefined() {
                                return Ok(lib.base_addr + sym.value);
                            }
                        }
                    }
                    for sym in &lib.symbols {
                        if sym.name == symbol && !sym.is_undefined() {
                            return Ok(lib.base_addr + sym.value);
                        }
                    }
                    return Err(ElfError::SymbolNotFound);
                }
            }
        }
        Err(ElfError::LibraryNotFound)
    }

    pub fn dlclose(&mut self, name: &str) -> Result<(), ElfError> {
        let should_unload = if let Some(lib) = self.cache.get_mut(name) {
            lib.ref_count = lib.ref_count.saturating_sub(1);
            lib.ref_count == 0
        } else {
            return Err(ElfError::LibraryNotFound);
        };

        if should_unload {
            let finis = self.collect_fini_functions(name);
            // In a full implementation, we'd call each fini function.
            let _ = finis;

            self.global_symbols.retain(|_, v| v.library != name);
            self.cache.remove(name);
        }
        Ok(())
    }

    pub fn loaded_library_count(&self) -> usize {
        self.cache.loaded_count()
    }

    pub fn global_symbol_count(&self) -> usize {
        self.global_symbols.len()
    }

    pub fn get_library_info(&self, name: &str) -> Option<LibraryInfo> {
        self.cache.get(name).map(|lib| LibraryInfo {
            name: lib.name.clone(),
            soname: lib.soname.clone(),
            base_addr: lib.base_addr,
            load_size: lib.load_size,
            ref_count: lib.ref_count,
            symbol_count: lib.symbols.len(),
            needed: lib.needed.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct LibraryInfo {
    pub name: String,
    pub soname: Option<String>,
    pub base_addr: u64,
    pub load_size: u64,
    pub ref_count: u32,
    pub symbol_count: usize,
    pub needed: Vec<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Global instance
// ═══════════════════════════════════════════════════════════════════════════════

pub static DYNAMIC_LINKER: Mutex<Option<DynLinker>> = Mutex::new(None);

pub fn init() {
    *DYNAMIC_LINKER.lock() = Some(DynLinker::new(true));
    crate::serial_println!("[ OK ] Dynamic linker initialized");
}

pub fn link_executable(name: &str, data: &[u8]) -> Result<LinkResult, ElfError> {
    let mut guard = DYNAMIC_LINKER.lock();
    let linker = guard.as_mut().ok_or(ElfError::LoadFailed)?;
    linker.link_executable(name, data)
}

pub fn dlopen(name: &str, data: &[u8]) -> Result<u64, ElfError> {
    let mut guard = DYNAMIC_LINKER.lock();
    let linker = guard.as_mut().ok_or(ElfError::LoadFailed)?;
    linker.dlopen(name, data)
}

pub fn dlsym(handle: u64, symbol: &str) -> Result<u64, ElfError> {
    let guard = DYNAMIC_LINKER.lock();
    let linker = guard.as_ref().ok_or(ElfError::LoadFailed)?;
    linker.dlsym(handle, symbol)
}

pub fn dlclose(name: &str) -> Result<(), ElfError> {
    let mut guard = DYNAMIC_LINKER.lock();
    let linker = guard.as_mut().ok_or(ElfError::LoadFailed)?;
    linker.dlclose(name)
}
