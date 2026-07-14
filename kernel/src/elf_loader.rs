#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ─── Error Types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    InvalidMagic,
    InvalidClass,
    InvalidEndian,
    UnsupportedType,
    UnsupportedMachine,
    InvalidProgramHeader,
    InvalidSectionHeader,
    InvalidDynamic,
    SymbolNotFound,
    RelocationFailed,
    UnsupportedRelocation,
    LoadFailed,
    NoMemory,
    InvalidAlignment,
    TlsError,
    LibraryNotFound,
    CircularDependency,
    InvalidGot,
    InvalidPlt,
}

// ─── ELF Constants ───────────────────────────────────────────────────────────

pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const EM_X86_64: u16 = 62;

pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_NOTE: u32 = 4;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_EH_FRAME: u32 = 0x6474e550;
pub const PT_GNU_STACK: u32 = 0x6474e551;
pub const PT_GNU_RELRO: u32 = 0x6474e552;

pub const PF_X: u32 = 0x1;
pub const PF_W: u32 = 0x2;
pub const PF_R: u32 = 0x4;

pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_HASH: u32 = 5;
pub const SHT_DYNAMIC: u32 = 6;
pub const SHT_NOTE: u32 = 7;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;
pub const SHT_DYNSYM: u32 = 11;
pub const SHT_INIT_ARRAY: u32 = 14;
pub const SHT_FINI_ARRAY: u32 = 15;
pub const SHT_GNU_HASH: u32 = 0x6ffffff6;

pub const DT_NULL: u64 = 0;
pub const DT_NEEDED: u64 = 1;
pub const DT_PLTRELSZ: u64 = 2;
pub const DT_PLTGOT: u64 = 3;
pub const DT_HASH: u64 = 4;
pub const DT_STRTAB: u64 = 5;
pub const DT_SYMTAB: u64 = 6;
pub const DT_RELA: u64 = 7;
pub const DT_RELASZ: u64 = 8;
pub const DT_RELAENT: u64 = 9;
pub const DT_STRSZ: u64 = 10;
pub const DT_SYMENT: u64 = 11;
pub const DT_INIT: u64 = 12;
pub const DT_FINI: u64 = 13;
pub const DT_SONAME: u64 = 14;
pub const DT_RPATH: u64 = 15;
pub const DT_SYMBOLIC: u64 = 16;
pub const DT_REL: u64 = 17;
pub const DT_RELSZ: u64 = 18;
pub const DT_RELENT: u64 = 19;
pub const DT_PLTREL: u64 = 20;
pub const DT_DEBUG: u64 = 21;
pub const DT_JMPREL: u64 = 23;
pub const DT_INIT_ARRAY: u64 = 25;
pub const DT_FINI_ARRAY: u64 = 26;
pub const DT_INIT_ARRAYSZ: u64 = 27;
pub const DT_FINI_ARRAYSZ: u64 = 28;
pub const DT_FLAGS: u64 = 30;
pub const DT_FLAGS_1: u64 = 0x6ffffffb;
pub const DT_GNU_HASH: u64 = 0x6ffffef5;

pub const R_X86_64_NONE: u32 = 0;
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_PC32: u32 = 2;
pub const R_X86_64_GOT32: u32 = 3;
pub const R_X86_64_PLT32: u32 = 4;
pub const R_X86_64_COPY: u32 = 5;
pub const R_X86_64_GLOB_DAT: u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;
pub const R_X86_64_RELATIVE: u32 = 8;
pub const R_X86_64_GOTPCREL: u32 = 9;
pub const R_X86_64_32: u32 = 10;
pub const R_X86_64_32S: u32 = 11;
pub const R_X86_64_TPOFF64: u32 = 18;
pub const R_X86_64_DTPMOD64: u32 = 16;
pub const R_X86_64_DTPOFF64: u32 = 17;
pub const R_X86_64_IRELATIVE: u32 = 37;

pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;
pub const STT_NOTYPE: u8 = 0;
pub const STT_OBJECT: u8 = 1;
pub const STT_FUNC: u8 = 2;
pub const STT_SECTION: u8 = 3;
pub const STT_TLS: u8 = 6;

// ─── ELF Structures ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfType {
    None = 0,
    Rel = 1,
    Exec = 2,
    Dyn = 3,
    Core = 4,
}

impl ElfType {
    pub fn from_u16(val: u16) -> Result<Self, ElfError> {
        match val {
            0 => Ok(Self::None),
            1 => Ok(Self::Rel),
            2 => Ok(Self::Exec),
            3 => Ok(Self::Dyn),
            4 => Ok(Self::Core),
            _ => Err(ElfError::UnsupportedType),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Header {
    pub magic: [u8; 4],
    pub class: u8,
    pub data: u8,
    pub version: u8,
    pub osabi: u8,
    pub abiversion: u8,
    pub pad: [u8; 7],
    pub elf_type: u16,
    pub machine: u16,
    pub version2: u32,
    pub entry: u64,
    pub phoff: u64,
    pub shoff: u64,
    pub flags: u32,
    pub ehsize: u16,
    pub phentsize: u16,
    pub phnum: u16,
    pub shentsize: u16,
    pub shnum: u16,
    pub shstrndx: u16,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Phdr {
    pub p_type: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Shdr {
    pub name: u32,
    pub sh_type: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub addralign: u64,
    pub entsize: u64,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Sym {
    pub name: u32,
    pub info: u8,
    pub other: u8,
    pub shndx: u16,
    pub value: u64,
    pub size: u64,
}

impl Elf64Sym {
    pub fn binding(&self) -> u8 {
        self.info >> 4
    }
    pub fn sym_type(&self) -> u8 {
        self.info & 0xf
    }
    pub fn is_undefined(&self) -> bool {
        self.shndx == 0
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Rela {
    pub offset: u64,
    pub info: u64,
    pub addend: i64,
}

impl Elf64Rela {
    pub fn sym_index(&self) -> u32 {
        (self.info >> 32) as u32
    }
    pub fn rel_type(&self) -> u32 {
        (self.info & 0xffffffff) as u32
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Dyn {
    pub tag: u64,
    pub val: u64,
}

// ─── Loader Structures ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    pub value: u64,
    pub size: u64,
    pub binding: u8,
    pub sym_type: u8,
    pub object: String,
}

#[derive(Debug, Clone)]
pub struct LoadedSegment {
    pub vaddr: u64,
    pub memsz: u64,
    pub filesz: u64,
    pub flags: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct DynEntry {
    pub tag: u64,
    pub val: u64,
}

#[derive(Debug, Clone)]
pub struct ElfSymbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
    pub binding: u8,
    pub sym_type: u8,
    pub section_index: u16,
}

impl ElfSymbol {
    pub fn is_undefined(&self) -> bool {
        self.section_index == 0
    }
}

#[derive(Debug, Clone)]
pub struct ElfRelocation {
    pub offset: u64,
    pub rel_type: u32,
    pub symbol_index: u32,
    pub addend: i64,
}

#[derive(Debug, Clone)]
pub struct TlsImage {
    pub image_addr: u64,
    pub image_size: u64,
    pub mem_size: u64,
    pub alignment: u64,
    pub dtv_offset: u64,
}

#[derive(Debug, Clone)]
pub struct LoadedObject {
    pub name: String,
    pub base_addr: u64,
    pub entry_point: u64,
    pub segments: Vec<LoadedSegment>,
    pub dynamic: Vec<DynEntry>,
    pub symbols: Vec<ElfSymbol>,
    pub relocations: Vec<ElfRelocation>,
    pub got_addr: Option<u64>,
    pub plt_addr: Option<u64>,
    pub init_func: Option<u64>,
    pub fini_func: Option<u64>,
    pub init_array: Option<(u64, u64)>,
    pub fini_array: Option<(u64, u64)>,
    pub needed: Vec<String>,
    pub tls: Option<TlsImage>,
    pub relro_start: Option<u64>,
    pub relro_size: Option<u64>,
    pub gnu_hash: Option<u64>,
}

// ─── ELF Loader ──────────────────────────────────────────────────────────────

pub struct ElfLoader {
    loaded_objects: BTreeMap<String, LoadedObject>,
    symbol_table: BTreeMap<String, SymbolInfo>,
    tls_images: Vec<TlsImage>,
    aslr_enabled: bool,
    aslr_seed: u64,
    next_load_addr: u64,
    search_paths: Vec<String>,
    loading_stack: Vec<String>,
}

impl ElfLoader {
    pub fn new(aslr_enabled: bool) -> Self {
        Self {
            loaded_objects: BTreeMap::new(),
            symbol_table: BTreeMap::new(),
            tls_images: Vec::new(),
            aslr_enabled,
            aslr_seed: 0x12345678_9abcdef0,
            next_load_addr: 0x4000_0000,
            search_paths: Vec::new(),
            loading_stack: Vec::new(),
        }
    }

    fn aslr_offset(&mut self) -> u64 {
        if !self.aslr_enabled {
            return 0;
        }
        // Simple xorshift64 PRNG for ASLR slide
        self.aslr_seed ^= self.aslr_seed << 13;
        self.aslr_seed ^= self.aslr_seed >> 7;
        self.aslr_seed ^= self.aslr_seed << 17;
        (self.aslr_seed & 0x3FFF_F000) // 256 MiB range, page-aligned
    }

    fn allocate_base(&mut self, size: u64) -> u64 {
        let base = self.next_load_addr + self.aslr_offset();
        let aligned = (base + 0xFFF) & !0xFFF;
        self.next_load_addr = aligned + ((size + 0xFFF) & !0xFFF);
        aligned
    }

    pub fn parse_header(data: &[u8]) -> Result<Elf64Header, ElfError> {
        if data.len() < 64 {
            return Err(ElfError::InvalidMagic);
        }
        if data[0..4] != ELF_MAGIC {
            return Err(ElfError::InvalidMagic);
        }
        if data[4] != ELFCLASS64 {
            return Err(ElfError::InvalidClass);
        }
        if data[5] != ELFDATA2LSB {
            return Err(ElfError::InvalidEndian);
        }

        let header = Elf64Header {
            magic: [data[0], data[1], data[2], data[3]],
            class: data[4],
            data: data[5],
            version: data[6],
            osabi: data[7],
            abiversion: data[8],
            pad: [0; 7],
            elf_type: u16::from_le_bytes([data[16], data[17]]),
            machine: u16::from_le_bytes([data[18], data[19]]),
            version2: u32::from_le_bytes([data[20], data[21], data[22], data[23]]),
            entry: u64::from_le_bytes(data[24..32].try_into().unwrap()),
            phoff: u64::from_le_bytes(data[32..40].try_into().unwrap()),
            shoff: u64::from_le_bytes(data[40..48].try_into().unwrap()),
            flags: u32::from_le_bytes([data[48], data[49], data[50], data[51]]),
            ehsize: u16::from_le_bytes([data[52], data[53]]),
            phentsize: u16::from_le_bytes([data[54], data[55]]),
            phnum: u16::from_le_bytes([data[56], data[57]]),
            shentsize: u16::from_le_bytes([data[58], data[59]]),
            shnum: u16::from_le_bytes([data[60], data[61]]),
            shstrndx: u16::from_le_bytes([data[62], data[63]]),
        };

        if header.machine != EM_X86_64 {
            return Err(ElfError::UnsupportedMachine);
        }
        Ok(header)
    }

    pub fn parse_program_headers(
        data: &[u8],
        header: &Elf64Header,
    ) -> Result<Vec<Elf64Phdr>, ElfError> {
        let mut phdrs = Vec::new();
        let phoff = header.phoff as usize;
        let phentsize = header.phentsize as usize;

        for i in 0..header.phnum as usize {
            let offset = phoff + i * phentsize;
            if offset + phentsize > data.len() {
                return Err(ElfError::InvalidProgramHeader);
            }
            let ph = &data[offset..offset + phentsize];
            let phdr = Elf64Phdr {
                p_type: u32::from_le_bytes(ph[0..4].try_into().unwrap()),
                flags: u32::from_le_bytes(ph[4..8].try_into().unwrap()),
                offset: u64::from_le_bytes(ph[8..16].try_into().unwrap()),
                vaddr: u64::from_le_bytes(ph[16..24].try_into().unwrap()),
                paddr: u64::from_le_bytes(ph[24..32].try_into().unwrap()),
                filesz: u64::from_le_bytes(ph[32..40].try_into().unwrap()),
                memsz: u64::from_le_bytes(ph[40..48].try_into().unwrap()),
                align: u64::from_le_bytes(ph[48..56].try_into().unwrap()),
            };
            phdrs.push(phdr);
        }
        Ok(phdrs)
    }

    pub fn parse_section_headers(
        data: &[u8],
        header: &Elf64Header,
    ) -> Result<Vec<Elf64Shdr>, ElfError> {
        let mut shdrs = Vec::new();
        let shoff = header.shoff as usize;
        let shentsize = header.shentsize as usize;

        if shoff == 0 {
            return Ok(shdrs);
        }

        for i in 0..header.shnum as usize {
            let offset = shoff + i * shentsize;
            if offset + shentsize > data.len() {
                return Err(ElfError::InvalidSectionHeader);
            }
            let sh = &data[offset..offset + shentsize];
            let shdr = Elf64Shdr {
                name: u32::from_le_bytes(sh[0..4].try_into().unwrap()),
                sh_type: u32::from_le_bytes(sh[4..8].try_into().unwrap()),
                flags: u64::from_le_bytes(sh[8..16].try_into().unwrap()),
                addr: u64::from_le_bytes(sh[16..24].try_into().unwrap()),
                offset: u64::from_le_bytes(sh[24..32].try_into().unwrap()),
                size: u64::from_le_bytes(sh[32..40].try_into().unwrap()),
                link: u32::from_le_bytes(sh[40..44].try_into().unwrap()),
                info: u32::from_le_bytes(sh[44..48].try_into().unwrap()),
                addralign: u64::from_le_bytes(sh[48..56].try_into().unwrap()),
                entsize: u64::from_le_bytes(sh[56..64].try_into().unwrap()),
            };
            shdrs.push(shdr);
        }
        Ok(shdrs)
    }

    fn parse_symbols(data: &[u8], symtab: &Elf64Shdr, strtab: &Elf64Shdr) -> Vec<ElfSymbol> {
        let mut symbols = Vec::new();
        let sym_offset = symtab.offset as usize;
        let sym_count = if symtab.entsize > 0 {
            symtab.size / symtab.entsize
        } else {
            0
        };
        let str_offset = strtab.offset as usize;
        let str_size = strtab.size as usize;

        for i in 0..sym_count as usize {
            let off = sym_offset + i * 24;
            if off + 24 > data.len() {
                break;
            }

            let name_idx = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
            let info = data[off + 4];
            let _other = data[off + 5];
            let shndx = u16::from_le_bytes(data[off + 6..off + 8].try_into().unwrap());
            let value = u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap());
            let size = u64::from_le_bytes(data[off + 16..off + 24].try_into().unwrap());

            let name = Self::read_string(data, str_offset, str_size, name_idx);
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

    fn parse_relocations(data: &[u8], rela_shdr: &Elf64Shdr) -> Vec<ElfRelocation> {
        let mut relocs = Vec::new();
        let offset = rela_shdr.offset as usize;
        let count = if rela_shdr.entsize > 0 {
            rela_shdr.size / rela_shdr.entsize
        } else {
            0
        };

        for i in 0..count as usize {
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

    fn parse_dynamic(data: &[u8], phdr: &Elf64Phdr) -> Vec<DynEntry> {
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

    fn read_string(
        data: &[u8],
        strtab_offset: usize,
        strtab_size: usize,
        name_idx: usize,
    ) -> String {
        if name_idx >= strtab_size {
            return String::new();
        }
        let start = strtab_offset + name_idx;
        let mut end = start;
        while end < data.len() && end < strtab_offset + strtab_size && data[end] != 0 {
            end += 1;
        }
        String::from_utf8_lossy(&data[start..end]).into_owned()
    }

    pub fn load_segments(
        &mut self,
        data: &[u8],
        phdrs: &[Elf64Phdr],
        base_addr: u64,
    ) -> Result<Vec<LoadedSegment>, ElfError> {
        let mut segments = Vec::new();
        for phdr in phdrs {
            if phdr.p_type != PT_LOAD {
                continue;
            }

            let file_offset = phdr.offset as usize;
            let file_size = phdr.filesz as usize;
            let mem_size = phdr.memsz as usize;

            if file_offset + file_size > data.len() {
                return Err(ElfError::LoadFailed);
            }

            let mut segment_data = alloc::vec![0u8; mem_size];
            segment_data[..file_size].copy_from_slice(&data[file_offset..file_offset + file_size]);

            segments.push(LoadedSegment {
                vaddr: base_addr + phdr.vaddr,
                memsz: phdr.memsz,
                filesz: phdr.filesz,
                flags: phdr.flags,
                data: segment_data,
            });
        }
        Ok(segments)
    }

    pub fn resolve_symbol(&self, name: &str) -> Option<&SymbolInfo> {
        self.symbol_table.get(name)
    }

    pub fn apply_relocations(
        segments: &mut [LoadedSegment],
        relocations: &[ElfRelocation],
        symbols: &[ElfSymbol],
        base_addr: u64,
        global_symbols: &BTreeMap<String, SymbolInfo>,
        tls_offset: u64,
    ) -> Result<(), ElfError> {
        // Write a relocation result at `seg_offset`, or fail LOUDLY. BUG-36: a
        // relocation whose target is out of the segment (or outside any segment)
        // must NOT be silently skipped — that leaves an unresolved pointer the
        // program later dereferences/jumps through (crash or control-flow
        // hijack). The serial line makes a real out-of-bounds reloc on iron
        // pinpointable instead of a mystery fault later.
        macro_rules! write_reloc {
            ($seg:expr, $off:expr, $bytes:expr) => {{
                let off = $off;
                let b = $bytes;
                if off + b.len() <= $seg.data.len() {
                    $seg.data[off..off + b.len()].copy_from_slice(&b);
                } else {
                    crate::serial_println!(
                        "[elf_loader] reloc out of bounds: off={} width={} seg_len={}",
                        off,
                        b.len(),
                        $seg.data.len()
                    );
                    return Err(ElfError::RelocationFailed);
                }
            }};
        }

        for reloc in relocations {
            let target_addr = base_addr + reloc.offset;

            let segment = segments
                .iter_mut()
                .find(|s| target_addr >= s.vaddr && target_addr < s.vaddr + s.memsz);
            let segment = match segment {
                Some(s) => s,
                None => {
                    crate::serial_println!(
                        "[elf_loader] reloc type={} targets {:#x} outside every loaded segment",
                        reloc.rel_type,
                        target_addr
                    );
                    return Err(ElfError::RelocationFailed);
                }
            };
            let seg_offset = (target_addr - segment.vaddr) as usize;

            match reloc.rel_type {
                R_X86_64_RELATIVE => {
                    let value = (base_addr as i64 + reloc.addend) as u64;
                    write_reloc!(segment, seg_offset, value.to_le_bytes());
                }
                R_X86_64_64 => {
                    let sym = symbols
                        .get(reloc.symbol_index as usize)
                        .ok_or(ElfError::SymbolNotFound)?;
                    let sym_value = if sym.is_undefined() {
                        global_symbols
                            .get(&sym.name)
                            .map(|s| s.value)
                            .ok_or(ElfError::SymbolNotFound)?
                    } else {
                        base_addr + sym.value
                    };
                    let value = (sym_value as i64 + reloc.addend) as u64;
                    write_reloc!(segment, seg_offset, value.to_le_bytes());
                }
                R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                    let sym = symbols
                        .get(reloc.symbol_index as usize)
                        .ok_or(ElfError::SymbolNotFound)?;
                    let sym_value = if sym.is_undefined() {
                        global_symbols.get(&sym.name).map(|s| s.value).unwrap_or(0)
                    } else {
                        base_addr + sym.value
                    };
                    write_reloc!(segment, seg_offset, sym_value.to_le_bytes());
                }
                R_X86_64_COPY => {
                    let sym = symbols
                        .get(reloc.symbol_index as usize)
                        .ok_or(ElfError::SymbolNotFound)?;
                    if let Some(src_info) = global_symbols.get(&sym.name) {
                        let copy_size = sym.size.min(src_info.size) as usize;
                        write_reloc!(segment, seg_offset, alloc::vec![0u8; copy_size]);
                    }
                }
                R_X86_64_TPOFF64 => {
                    let sym = symbols
                        .get(reloc.symbol_index as usize)
                        .ok_or(ElfError::SymbolNotFound)?;
                    let tpoff = sym.value.wrapping_add(tls_offset);
                    let value = (tpoff as i64 + reloc.addend) as u64;
                    write_reloc!(segment, seg_offset, value.to_le_bytes());
                }
                R_X86_64_DTPMOD64 => {
                    let value: u64 = 1;
                    write_reloc!(segment, seg_offset, value.to_le_bytes());
                }
                R_X86_64_DTPOFF64 => {
                    let sym = symbols
                        .get(reloc.symbol_index as usize)
                        .ok_or(ElfError::SymbolNotFound)?;
                    let value = (sym.value as i64 + reloc.addend) as u64;
                    write_reloc!(segment, seg_offset, value.to_le_bytes());
                }
                R_X86_64_PC32 | R_X86_64_PLT32 => {
                    let sym = symbols
                        .get(reloc.symbol_index as usize)
                        .ok_or(ElfError::SymbolNotFound)?;
                    let sym_value = if sym.is_undefined() {
                        global_symbols
                            .get(&sym.name)
                            .map(|s| s.value)
                            .ok_or(ElfError::SymbolNotFound)?
                    } else {
                        base_addr + sym.value
                    };
                    let value = (sym_value as i64 + reloc.addend - target_addr as i64) as u32;
                    write_reloc!(segment, seg_offset, value.to_le_bytes());
                }
                R_X86_64_IRELATIVE => {
                    let resolver_addr = (base_addr as i64 + reloc.addend) as u64;
                    write_reloc!(segment, seg_offset, resolver_addr.to_le_bytes());
                }
                R_X86_64_NONE => {}
                _ => return Err(ElfError::UnsupportedRelocation),
            }
        }
        Ok(())
    }

    pub fn setup_got(
        segments: &mut [LoadedSegment],
        got_addr: u64,
        base_addr: u64,
    ) -> Result<(), ElfError> {
        let segment = segments
            .iter_mut()
            .find(|s| got_addr >= s.vaddr && got_addr < s.vaddr + s.memsz);
        if let Some(seg) = segment {
            let offset = (got_addr - seg.vaddr) as usize;
            if offset + 24 <= seg.data.len() {
                // GOT[0] = address of _DYNAMIC
                seg.data[offset..offset + 8].copy_from_slice(&base_addr.to_le_bytes());
                // GOT[1] = link_map (filled by dynamic linker)
                seg.data[offset + 8..offset + 16].copy_from_slice(&0u64.to_le_bytes());
                // GOT[2] = _dl_runtime_resolve address
                seg.data[offset + 16..offset + 24].copy_from_slice(&0u64.to_le_bytes());
            }
        }
        Ok(())
    }

    pub fn setup_plt(
        segments: &mut [LoadedSegment],
        plt_addr: u64,
        got_addr: u64,
        num_entries: usize,
    ) -> Result<(), ElfError> {
        let segment = segments
            .iter_mut()
            .find(|s| plt_addr >= s.vaddr && plt_addr < s.vaddr + s.memsz);
        if let Some(seg) = segment {
            let base_offset = (plt_addr - seg.vaddr) as usize;
            // PLT[0]: push GOT[1]; jmp GOT[2]
            if base_offset + 16 <= seg.data.len() {
                let plt0: [u8; 16] = [
                    0xff, 0x35, 0x00, 0x00, 0x00, 0x00, // push [GOT+8]
                    0xff, 0x25, 0x00, 0x00, 0x00, 0x00, // jmp [GOT+16]
                    0x0f, 0x1f, 0x40, 0x00, // nop
                ];
                seg.data[base_offset..base_offset + 16].copy_from_slice(&plt0);
                let got_1_offset = (got_addr + 8).wrapping_sub(plt_addr + 6) as u32;
                seg.data[base_offset + 2..base_offset + 6]
                    .copy_from_slice(&got_1_offset.to_le_bytes());
                let got_2_offset = (got_addr + 16).wrapping_sub(plt_addr + 12) as u32;
                seg.data[base_offset + 8..base_offset + 12]
                    .copy_from_slice(&got_2_offset.to_le_bytes());
            }
            // PLT entries
            for i in 0..num_entries {
                let entry_offset = base_offset + 16 + i * 16;
                if entry_offset + 16 > seg.data.len() {
                    break;
                }
                let got_entry = got_addr + 24 + (i as u64) * 8;
                let plt_entry_addr = plt_addr + 16 + (i as u64) * 16;
                let got_rel = (got_entry).wrapping_sub(plt_entry_addr + 6) as u32;
                let plt_entry: [u8; 16] = [
                    0xff, 0x25, 0x00, 0x00, 0x00, 0x00, // jmp [GOT+n]
                    0x68, i as u8, 0x00, 0x00, 0x00, // push index
                    0xe9, 0x00, 0x00, 0x00, 0x00, // jmp PLT[0]
                ];
                seg.data[entry_offset..entry_offset + 16].copy_from_slice(&plt_entry);
                seg.data[entry_offset + 2..entry_offset + 6]
                    .copy_from_slice(&got_rel.to_le_bytes());
                let plt0_rel = (plt_addr).wrapping_sub(plt_entry_addr + 16) as u32;
                seg.data[entry_offset + 12..entry_offset + 16]
                    .copy_from_slice(&plt0_rel.to_le_bytes());
            }
        }
        Ok(())
    }

    pub fn setup_tls(
        &mut self,
        data: &[u8],
        phdrs: &[Elf64Phdr],
        base_addr: u64,
    ) -> Option<TlsImage> {
        for phdr in phdrs {
            if phdr.p_type == PT_TLS {
                let image = TlsImage {
                    image_addr: base_addr + phdr.vaddr,
                    image_size: phdr.filesz,
                    mem_size: phdr.memsz,
                    alignment: phdr.align,
                    dtv_offset: self.tls_images.iter().map(|t| t.mem_size).sum(),
                };
                let _ = data;
                self.tls_images.push(image.clone());
                return Some(image);
            }
        }
        None
    }

    pub fn load_elf(&mut self, name: &str, data: &[u8]) -> Result<LoadedObject, ElfError> {
        if self.loaded_objects.contains_key(name) {
            return Ok(self.loaded_objects[name].clone());
        }

        if self.loading_stack.contains(&String::from(name)) {
            return Err(ElfError::CircularDependency);
        }
        self.loading_stack.push(String::from(name));

        let header = Self::parse_header(data)?;
        let elf_type = ElfType::from_u16(header.elf_type)?;
        let phdrs = Self::parse_program_headers(data, &header)?;
        let shdrs = Self::parse_section_headers(data, &header)?;

        let is_pie = elf_type == ElfType::Dyn;

        // A binary with PT_INTERP is dynamically linked: its interpreter (ld.so)
        // performs ALL relocation, GOT/PLT setup, TLS, and symbol resolution at
        // runtime — exactly as the Linux kernel hands off. If the kernel also
        // relocates/sets up the GOT, the two collide: the kernel pre-filled GOT
        // slots that ld.so then jumps through, sending control into dh's NX ELF
        // header page (#PF on instruction fetch at the load base). So for these
        // objects we ONLY map raw segments and let ld.so own the rest.
        let has_interp = phdrs.iter().any(|p| p.p_type == PT_INTERP);

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
        let total_size = vaddr_max - vaddr_min;

        let base_addr = if is_pie {
            self.allocate_base(total_size) - vaddr_min
        } else {
            0
        };

        let mut segments = self.load_segments(data, &phdrs, base_addr)?;

        // Parse dynamic section
        let mut dynamic = Vec::new();
        let mut needed = Vec::new();
        let mut got_addr = None;
        let mut init_func = None;
        let mut fini_func = None;
        let mut init_array = None;
        let mut fini_array = None;
        let mut strtab_addr: u64 = 0;
        let mut gnu_hash_addr = None;
        // Dynamic relocation table (DT_RELA) — the canonical relocation source
        // for PIE/dynamic objects (section headers may be absent/unscanned).
        let mut dt_rela_vaddr: Option<u64> = None;
        let mut dt_relasz: u64 = 0;
        let mut dt_relaent: u64 = 24;

        for phdr in &phdrs {
            if phdr.p_type == PT_DYNAMIC {
                dynamic = Self::parse_dynamic(data, phdr);
                break;
            }
        }

        // Find strtab in file for NEEDED entries
        let dyn_strtab_shdr = shdrs
            .iter()
            .find(|s| s.sh_type == SHT_STRTAB && s.offset > 0);

        for entry in &dynamic {
            match entry.tag {
                DT_NEEDED => {
                    if let Some(strtab) = dyn_strtab_shdr {
                        let lib_name = Self::read_string(
                            data,
                            strtab.offset as usize,
                            strtab.size as usize,
                            entry.val as usize,
                        );
                        needed.push(lib_name);
                    }
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
                DT_STRTAB => {
                    strtab_addr = entry.val;
                }
                DT_GNU_HASH => {
                    gnu_hash_addr = Some(base_addr + entry.val);
                }
                DT_RELA => {
                    dt_rela_vaddr = Some(entry.val);
                }
                DT_RELASZ => {
                    dt_relasz = entry.val;
                }
                DT_RELAENT => {
                    dt_relaent = entry.val;
                }
                _ => {}
            }
        }
        let _ = strtab_addr;

        // Parse symbols
        let mut symbols = Vec::new();
        let dynsym_shdr = shdrs.iter().find(|s| s.sh_type == SHT_DYNSYM);
        if let Some(sym_shdr) = dynsym_shdr {
            let strtab_idx = sym_shdr.link as usize;
            if strtab_idx < shdrs.len() {
                symbols = Self::parse_symbols(data, sym_shdr, &shdrs[strtab_idx]);
            }
        }

        // Parse relocations — prefer section headers, but fall back to the
        // dynamic segment's DT_RELA table (PIE objects like relibc may not have
        // their `.rela.dyn` picked up via section scan; DT_RELA is canonical).
        let mut relocations = Vec::new();
        for shdr in &shdrs {
            if shdr.sh_type == SHT_RELA {
                let mut relocs = Self::parse_relocations(data, shdr);
                relocations.append(&mut relocs);
            }
        }
        if relocations.is_empty() {
            if let Some(rela_vaddr) = dt_rela_vaddr {
                // Map the RELA table's virtual address to a file offset using PT_LOAD.
                let file_off = phdrs
                    .iter()
                    .filter(|p| p.p_type == PT_LOAD)
                    .find(|p| rela_vaddr >= p.vaddr && rela_vaddr < p.vaddr + p.filesz)
                    .map(|p| (rela_vaddr - p.vaddr + p.offset) as usize);
                let ent = if dt_relaent > 0 { dt_relaent } else { 24 };
                let count = if ent > 0 { dt_relasz / ent } else { 0 };
                if let Some(base_off) = file_off {
                    for i in 0..count as usize {
                        let off = base_off + i * ent as usize;
                        if off + 24 > data.len() {
                            break;
                        }
                        let r_offset = u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
                        let r_info =
                            u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap());
                        let r_addend =
                            i64::from_le_bytes(data[off + 16..off + 24].try_into().unwrap());
                        relocations.push(ElfRelocation {
                            offset: r_offset,
                            rel_type: (r_info & 0xffffffff) as u32,
                            symbol_index: (r_info >> 32) as u32,
                            addend: r_addend,
                        });
                    }
                }
            }
        }

        // Register global symbols
        for sym in &symbols {
            if (sym.binding == STB_GLOBAL || sym.binding == STB_WEAK)
                && !sym.name.is_empty()
                && sym.section_index != 0
            {
                self.symbol_table.insert(
                    sym.name.clone(),
                    SymbolInfo {
                        name: sym.name.clone(),
                        value: base_addr + sym.value,
                        size: sym.size,
                        binding: sym.binding,
                        sym_type: sym.sym_type,
                        object: String::from(name),
                    },
                );
            }
        }

        // Apply relocations + GOT setup ONLY for objects the kernel fully owns
        // (the interpreter itself, and native/static binaries). A dynamically
        // linked exe (has_interp) is relocated entirely by ld.so at runtime;
        // doing it here corrupts the GOT/PLT slots ld.so jumps through.
        if !has_interp {
            Self::apply_relocations(
                &mut segments,
                &relocations,
                &symbols,
                base_addr,
                &self.symbol_table,
                0,
            )?;

            // Setup GOT
            if let Some(got) = got_addr {
                Self::setup_got(&mut segments, got, base_addr)?;
            }
        }

        // Setup TLS
        let tls = self.setup_tls(data, &phdrs, base_addr);

        // RELRO
        let mut relro_start = None;
        let mut relro_size = None;
        for phdr in &phdrs {
            if phdr.p_type == PT_GNU_RELRO {
                relro_start = Some(base_addr + phdr.vaddr);
                relro_size = Some(phdr.memsz);
            }
        }

        let entry_point = base_addr + header.entry;

        let loaded = LoadedObject {
            name: String::from(name),
            base_addr,
            entry_point,
            segments,
            dynamic,
            symbols,
            relocations,
            got_addr,
            plt_addr: None,
            init_func,
            fini_func,
            init_array,
            fini_array,
            needed,
            tls,
            relro_start,
            relro_size,
            gnu_hash: gnu_hash_addr,
        };

        self.loaded_objects
            .insert(String::from(name), loaded.clone());
        self.loading_stack.retain(|n| n != name);
        Ok(loaded)
    }

    pub fn run_init(obj: &LoadedObject) -> Vec<u64> {
        let mut init_addrs = Vec::new();
        if let Some(init) = obj.init_func {
            init_addrs.push(init);
        }
        if let Some((array_addr, array_size)) = obj.init_array {
            let count = array_size / 8;
            for i in 0..count {
                init_addrs.push(array_addr + i * 8);
            }
        }
        init_addrs
    }

    pub fn run_fini(obj: &LoadedObject) -> Vec<u64> {
        let mut fini_addrs = Vec::new();
        if let Some((array_addr, array_size)) = obj.fini_array {
            let count = array_size / 8;
            for i in (0..count).rev() {
                fini_addrs.push(array_addr + i * 8);
            }
        }
        if let Some(fini) = obj.fini_func {
            fini_addrs.push(fini);
        }
        fini_addrs
    }

    pub fn dlopen(&mut self, name: &str, data: &[u8]) -> Result<u64, ElfError> {
        let obj = self.load_elf(name, data)?;
        Ok(obj.base_addr)
    }

    pub fn dlsym(&self, name: &str) -> Result<u64, ElfError> {
        self.symbol_table
            .get(name)
            .map(|s| s.value)
            .ok_or(ElfError::SymbolNotFound)
    }

    pub fn dlclose(&mut self, name: &str) -> Result<(), ElfError> {
        if self.loaded_objects.remove(name).is_none() {
            return Err(ElfError::LibraryNotFound);
        }
        self.symbol_table.retain(|_, v| v.object != name);
        Ok(())
    }

    pub fn loaded_object(&self, name: &str) -> Option<&LoadedObject> {
        self.loaded_objects.get(name)
    }

    pub fn add_search_path(&mut self, path: &str) {
        self.search_paths.push(String::from(path));
    }

    pub fn loaded_count(&self) -> usize {
        self.loaded_objects.len()
    }

    pub fn symbol_count(&self) -> usize {
        self.symbol_table.len()
    }
}

// ─── Global Instance ─────────────────────────────────────────────────────────

pub static ELF_LOADER: Mutex<Option<ElfLoader>> = Mutex::new(None);

/// Lock the global loader, constructing it on first use. Lazy because the
/// loader needs the heap; every entry point below shares this path (an
/// explicit `init()` existed before but was never called from kernel_main,
/// which left `load_linux_elf` returning `LoadFailed` for every Linux ELF —
/// found 2026-06-10 via the linux_hello spawn failure).
fn with_loader<R>(f: impl FnOnce(&mut ElfLoader) -> R) -> R {
    let mut guard = ELF_LOADER.lock();
    let loader = guard.get_or_insert_with(|| ElfLoader::new(true));
    f(loader)
}

pub fn load_elf(name: &str, data: &[u8]) -> Result<LoadedObject, ElfError> {
    with_loader(|l| l.load_elf(name, data))
}

pub fn dlopen(name: &str, data: &[u8]) -> Result<u64, ElfError> {
    with_loader(|l| l.dlopen(name, data))
}

pub fn dlsym(name: &str) -> Result<u64, ElfError> {
    with_loader(|l| l.dlsym(name))
}

pub fn dlclose(name: &str) -> Result<(), ElfError> {
    with_loader(|l| l.dlclose(name))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Linux ELF Detection & Auxiliary Vector
// ═══════════════════════════════════════════════════════════════════════════════

pub const ELFOSABI_NONE: u8 = 0; // UNIX System V ABI (Linux uses this)
pub const ELFOSABI_LINUX: u8 = 3; // Linux
pub const ELFOSABI_ATHENAOS: u8 = 0xAE; // AthenaOS native

pub const EI_OSABI: usize = 7;

/// Auxiliary vector entry types (Linux ABI)
pub const AT_NULL: u64 = 0;
pub const AT_IGNORE: u64 = 1;
pub const AT_EXECFD: u64 = 2;
pub const AT_PHDR: u64 = 3;
pub const AT_PHENT: u64 = 4;
pub const AT_PHNUM: u64 = 5;
pub const AT_PAGESZ: u64 = 6;
pub const AT_BASE: u64 = 7;
pub const AT_FLAGS: u64 = 8;
pub const AT_ENTRY: u64 = 9;
pub const AT_UID: u64 = 11;
pub const AT_EUID: u64 = 12;
pub const AT_GID: u64 = 13;
pub const AT_EGID: u64 = 14;
pub const AT_PLATFORM: u64 = 15;
pub const AT_HWCAP: u64 = 16;
pub const AT_CLKTCK: u64 = 17;
pub const AT_SECURE: u64 = 23;
pub const AT_RANDOM: u64 = 25;
pub const AT_HWCAP2: u64 = 26;
pub const AT_EXECFN: u64 = 31;
pub const AT_SYSINFO_EHDR: u64 = 33;

/// Detect whether an ELF binary is a Linux ELF or a AthenaOS native ELF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfOrigin {
    AthenaOS,
    Linux,
    Unknown(u8),
}

pub fn detect_elf_origin(data: &[u8]) -> Result<ElfOrigin, ElfError> {
    if data.len() < 64 {
        return Err(ElfError::InvalidMagic);
    }
    if data[0..4] != ELF_MAGIC {
        return Err(ElfError::InvalidMagic);
    }

    let osabi = data[EI_OSABI];
    match osabi {
        ELFOSABI_ATHENAOS => Ok(ElfOrigin::AthenaOS),
        ELFOSABI_NONE | ELFOSABI_LINUX => Ok(ElfOrigin::Linux),
        other => Ok(ElfOrigin::Unknown(other)),
    }
}

/// Information about a loaded Linux ELF needed to build the aux vector.
#[derive(Debug, Clone)]
pub struct LinuxElfInfo {
    pub entry_point: u64,
    pub phdr_addr: u64,
    pub phent_size: u16,
    pub phnum: u16,
    pub base_addr: u64,
    pub page_size: u64,
    pub uid: u32,
    pub gid: u32,
    pub random_bytes: [u8; 16],
    pub vdso_addr: u64,
}

impl LinuxElfInfo {
    pub fn from_header(data: &[u8], header: &Elf64Header, base_addr: u64) -> Self {
        let mut random = [0u8; 16];
        // Simple PRNG seed for AT_RANDOM
        let seed = base_addr ^ 0xDEAD_BEEF_1234_5678;
        for (i, b) in random.iter_mut().enumerate() {
            *b = ((seed >> (i * 4)) & 0xFF) as u8;
        }

        Self {
            entry_point: base_addr + header.entry,
            phdr_addr: base_addr + header.phoff,
            phent_size: header.phentsize,
            phnum: header.phnum,
            base_addr,
            page_size: 4096,
            uid: 1000,
            gid: 1000,
            random_bytes: random,
            vdso_addr: 0,
        }
    }
}

/// If `data` is a dynamically-linked ELF (has a PT_INTERP program header),
/// return the interpreter path it names (e.g. "/lib64/ld-linux-x86-64.so.2").
/// Static binaries have no PT_INTERP and return None. The kernel loads this
/// interpreter (ld.so) for dynamic executables; ld.so then relocates the main
/// exe and loads its shared libraries at runtime. Elf64_Phdr is 56 bytes:
/// p_type@0, p_offset@8, p_filesz@32.
pub fn read_interp_path(data: &[u8]) -> Option<alloc::string::String> {
    let header = ElfLoader::parse_header(data).ok()?;
    let phoff = header.phoff as usize;
    let phentsize = header.phentsize as usize;
    let phnum = header.phnum as usize;
    for i in 0..phnum {
        let off = phoff.checked_add(i.checked_mul(phentsize)?)?;
        if off + 40 > data.len() {
            break;
        }
        let p_type = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
        if p_type == PT_INTERP {
            let p_offset = u64::from_le_bytes(data[off + 8..off + 16].try_into().ok()?) as usize;
            let p_filesz = u64::from_le_bytes(data[off + 32..off + 40].try_into().ok()?) as usize;
            if p_filesz == 0 {
                return None;
            }
            let end = p_offset.checked_add(p_filesz)?;
            if end > data.len() {
                return None;
            }
            let raw = &data[p_offset..end];
            // The path is NUL-terminated; take bytes up to the first NUL.
            let s = raw.split(|&b| b == 0).next().unwrap_or(raw);
            return core::str::from_utf8(s)
                .ok()
                .map(alloc::string::String::from);
        }
    }
    None
}

/// Build the auxiliary vector for a Linux ELF process.
/// Returns a Vec of (type, value) pairs to be placed on the stack.
pub fn build_aux_vector(info: &LinuxElfInfo) -> Vec<(u64, u64)> {
    let mut auxv = Vec::new();

    auxv.push((AT_PHDR, info.phdr_addr));
    auxv.push((AT_PHENT, info.phent_size as u64));
    auxv.push((AT_PHNUM, info.phnum as u64));
    auxv.push((AT_PAGESZ, info.page_size));
    auxv.push((AT_ENTRY, info.entry_point));
    auxv.push((AT_BASE, info.base_addr));
    auxv.push((AT_FLAGS, 0));
    auxv.push((AT_UID, info.uid as u64));
    auxv.push((AT_EUID, info.uid as u64));
    auxv.push((AT_GID, info.gid as u64));
    auxv.push((AT_EGID, info.gid as u64));
    auxv.push((AT_CLKTCK, 100)); // 100 Hz
    auxv.push((AT_SECURE, 0));
    auxv.push((AT_HWCAP, 0)); // CPU feature bitmask (simplified)
    auxv.push((AT_HWCAP2, 0));

    if info.vdso_addr != 0 {
        auxv.push((AT_SYSINFO_EHDR, info.vdso_addr));
    }

    // AT_RANDOM points to 16 bytes of randomness on the stack.
    // The caller must place the random bytes and patch this entry.
    auxv.push((AT_RANDOM, 0)); // Placeholder — patched after stack setup

    auxv.push((AT_NULL, 0)); // Terminator
    auxv
}

/// Set up a Linux-compatible user stack layout:
///   [high address]
///     environment strings (null-terminated)
///     argument strings (null-terminated)
///     padding to 16-byte alignment
///     AT_RANDOM 16 random bytes
///     auxiliary vector (pairs of u64)
///     NULL (envp terminator)
///     envp[n-1] ... envp[0]
///     NULL (argv terminator)
///     argv[n-1] ... argv[0]
///     argc
///   [low address = initial RSP]
pub fn setup_linux_stack(
    stack_top: u64,
    argv: &[&str],
    envp: &[&str],
    info: &LinuxElfInfo,
) -> (u64, Vec<u8>) {
    let mut stack_data: Vec<u8> = Vec::new();

    // Phase 1: Write string data (args + env) at the top of the stack area
    let mut arg_offsets: Vec<u64> = Vec::new();
    let mut env_offsets: Vec<u64> = Vec::new();

    let mut string_pos = stack_top;

    // Argument strings
    for arg in argv {
        let bytes = arg.as_bytes();
        string_pos -= (bytes.len() + 1) as u64; // +1 for null terminator
        arg_offsets.push(string_pos);
    }

    // Environment strings
    for env in envp {
        let bytes = env.as_bytes();
        string_pos -= (bytes.len() + 1) as u64;
        env_offsets.push(string_pos);
    }

    // AT_RANDOM data (16 bytes)
    string_pos -= 16;
    let at_random_addr = string_pos;

    // Align to 16 bytes
    string_pos &= !0xF;

    // Phase 2: Build the auxv
    let mut auxv = build_aux_vector(info);
    // Patch AT_RANDOM
    for entry in auxv.iter_mut() {
        if entry.0 == AT_RANDOM {
            entry.1 = at_random_addr;
        }
    }

    // Phase 3: Calculate stack frame size
    let auxv_size = auxv.len() * 16; // Each entry is 2x u64
    let envp_ptrs = (env_offsets.len() + 1) * 8; // envp[] + NULL
    let argv_ptrs = (arg_offsets.len() + 1) * 8; // argv[] + NULL
    let argc_size = 8; // argc is u64

    let frame_size = argc_size + argv_ptrs + envp_ptrs + auxv_size;
    let frame_start = (string_pos - frame_size as u64) & !0xF;

    // Phase 4: Build the binary stack image
    let total_size = (stack_top - frame_start) as usize;
    stack_data.resize(total_size, 0);

    let base = frame_start;
    let mut pos = 0usize;

    // argc
    let argc = argv.len() as u64;
    stack_data[pos..pos + 8].copy_from_slice(&argc.to_le_bytes());
    pos += 8;

    // argv pointers
    for &off in &arg_offsets {
        stack_data[pos..pos + 8].copy_from_slice(&off.to_le_bytes());
        pos += 8;
    }
    // argv NULL terminator
    stack_data[pos..pos + 8].copy_from_slice(&0u64.to_le_bytes());
    pos += 8;

    // envp pointers
    for &off in &env_offsets {
        stack_data[pos..pos + 8].copy_from_slice(&off.to_le_bytes());
        pos += 8;
    }
    // envp NULL terminator
    stack_data[pos..pos + 8].copy_from_slice(&0u64.to_le_bytes());
    pos += 8;

    // Auxiliary vector
    for (atype, aval) in &auxv {
        stack_data[pos..pos + 8].copy_from_slice(&atype.to_le_bytes());
        pos += 8;
        stack_data[pos..pos + 8].copy_from_slice(&aval.to_le_bytes());
        pos += 8;
    }

    // Write AT_RANDOM bytes
    let random_offset = (at_random_addr - base) as usize;
    if random_offset + 16 <= stack_data.len() {
        stack_data[random_offset..random_offset + 16].copy_from_slice(&info.random_bytes);
    }

    // Write argument strings
    for (i, arg) in argv.iter().enumerate() {
        let str_offset = (arg_offsets[i] - base) as usize;
        let bytes = arg.as_bytes();
        if str_offset + bytes.len() < stack_data.len() {
            stack_data[str_offset..str_offset + bytes.len()].copy_from_slice(bytes);
            // null terminator is already 0 from resize
        }
    }

    // Write environment strings
    for (i, env) in envp.iter().enumerate() {
        let str_offset = (env_offsets[i] - base) as usize;
        let bytes = env.as_bytes();
        if str_offset + bytes.len() < stack_data.len() {
            stack_data[str_offset..str_offset + bytes.len()].copy_from_slice(bytes);
        }
    }

    (frame_start, stack_data)
}

// ═══════════════════════════════════════════════════════════════════════════════
// vDSO: Minimal virtual dynamic shared object
// ═══════════════════════════════════════════════════════════════════════════════

/// Minimal vDSO page for Linux compatibility. Contains a tiny ELF with
/// a `clock_gettime` fast path that reads the kernel's monotonic clock
/// directly (in a full implementation, via a shared memory page).
///
/// For now this is a placeholder — the vDSO page is allocated but
/// contains a minimal valid ELF header that glibc will accept without
/// crashing. The actual fast-path functions would read a shared
/// vsyscall page updated by the timer interrupt.
pub struct VdsoPage {
    pub base_addr: u64,
    pub size: usize,
}

impl VdsoPage {
    pub const VDSO_SIZE: usize = 4096;

    /// Create a minimal vDSO ELF image in the provided buffer.
    /// Returns the number of bytes written.
    pub fn build_minimal_image(buf: &mut [u8]) -> usize {
        if buf.len() < Self::VDSO_SIZE {
            return 0;
        }

        // Minimal ELF header for vDSO
        // Magic
        buf[0] = 0x7f;
        buf[1] = b'E';
        buf[2] = b'L';
        buf[3] = b'F';
        buf[4] = 2; // ELFCLASS64
        buf[5] = 1; // ELFDATA2LSB
        buf[6] = 1; // EV_CURRENT
        buf[7] = 0; // ELFOSABI_NONE
                    // e_type = ET_DYN (3)
        buf[16] = 3;
        buf[17] = 0;
        // e_machine = EM_X86_64 (62)
        buf[18] = 62;
        buf[19] = 0;
        // e_version = 1
        buf[20] = 1;
        // e_ehsize = 64
        buf[52] = 64;
        // e_phentsize = 56
        buf[54] = 56;
        // e_phnum = 0 (no program headers — glibc handles this gracefully)
        buf[56] = 0;

        Self::VDSO_SIZE
    }
}

/// Load a Linux ELF binary with full Linux-compatible setup:
/// - Detect OS/ABI
/// - Build auxiliary vector
/// - Set up stack layout
/// - Register as a Linux task for syscall routing
pub fn load_linux_elf(name: &str, data: &[u8]) -> Result<LoadedObject, ElfError> {
    let origin = detect_elf_origin(data)?;

    let obj = with_loader(|l| l.load_elf(name, data))?;

    // If this is a Linux ELF, mark the task for Linux syscall routing
    if origin == ElfOrigin::Linux {
        let header = ElfLoader::parse_header(data)?;
        let _info = LinuxElfInfo::from_header(data, &header, obj.base_addr);

        // The caller should use this info to set up the stack via
        // setup_linux_stack() and mark the task with
        // crate::linux_syscall::mark_task_as_linux()
    }

    Ok(obj)
}
