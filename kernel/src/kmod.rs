#![allow(dead_code)]

extern crate alloc;

use alloc::{boxed::Box, collections::BTreeMap, format, string::String, vec::Vec};
use spin::Mutex;

// ─── Core Module Structures ──────────────────────────────────────────────────

pub struct KernelModule {
    pub name: String,
    pub version: ModuleVersion,
    pub description: String,
    pub author: String,
    pub license: ModuleLicense,
    pub state: ModuleState,
    pub base_addr: u64,
    pub size: u64,
    pub init_fn: Option<u64>,
    pub exit_fn: Option<u64>,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
    pub symbols_exported: Vec<ExportedSymbol>,
    pub symbols_imported: Vec<ImportedSymbol>,
    pub parameters: Vec<ModuleParam>,
    pub refcount: u32,
    pub load_time: u64,
    pub taint: ModuleTaint,
    pub sections: Vec<ModuleSection>,
    pub info: ModuleInfo,
}

pub struct ModuleVersion {
    major: u16,
    minor: u16,
    patch: u16,
}

impl ModuleVersion {
    pub fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn compatible(&self, other: &Self) -> bool {
        self.major == other.major && self.minor >= other.minor
    }
}

impl core::fmt::Display for ModuleVersion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

pub enum ModuleLicense {
    Gpl,
    GplV2,
    GplAndAdditional,
    Mit,
    Bsd,
    Apache2,
    Proprietary,
    Dual(Box<ModuleLicense>, Box<ModuleLicense>),
}

impl ModuleLicense {
    pub fn is_gpl_compatible(&self) -> bool {
        match self {
            Self::Gpl | Self::GplV2 | Self::GplAndAdditional => true,
            Self::Dual(a, b) => a.is_gpl_compatible() || b.is_gpl_compatible(),
            _ => false,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Gpl => "GPL",
            Self::GplV2 => "GPL v2",
            Self::GplAndAdditional => "GPL and additional rights",
            Self::Mit => "MIT",
            Self::Bsd => "BSD",
            Self::Apache2 => "Apache 2.0",
            Self::Proprietary => "Proprietary",
            Self::Dual(_, _) => "Dual License",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleState {
    Live,
    Coming,
    Going,
    Unformed,
}

impl ModuleState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Coming => "Coming",
            Self::Going => "Going",
            Self::Unformed => "Unformed",
        }
    }
}

// ─── Symbol Management ───────────────────────────────────────────────────────

pub struct ExportedSymbol {
    pub name: String,
    pub address: u64,
    pub namespace: Option<String>,
    pub gpl_only: bool,
}

pub struct ImportedSymbol {
    pub name: String,
    pub resolved_address: Option<u64>,
    pub module: Option<String>,
}

// ─── Module Parameters ───────────────────────────────────────────────────────

pub struct ModuleParam {
    pub name: String,
    pub param_type: ParamType,
    pub description: String,
    pub value: ParamValue,
    pub permissions: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Bool,
    Int,
    UInt,
    Long,
    ULong,
    String,
    IntArray,
    UIntArray,
}

#[derive(Debug, Clone)]
pub enum ParamValue {
    Bool(bool),
    Int(i32),
    UInt(u32),
    Long(i64),
    ULong(u64),
    Str(String),
    IntArray(Vec<i32>),
    UIntArray(Vec<u32>),
}

impl ParamValue {
    pub fn parse(param_type: ParamType, input: &str) -> Result<Self, ModuleError> {
        match param_type {
            ParamType::Bool => match input {
                "1" | "y" | "Y" | "yes" | "true" => Ok(ParamValue::Bool(true)),
                "0" | "n" | "N" | "no" | "false" => Ok(ParamValue::Bool(false)),
                _ => Err(ModuleError::InvalidParameter),
            },
            ParamType::Int => input
                .parse::<i32>()
                .map(ParamValue::Int)
                .map_err(|_| ModuleError::InvalidParameter),
            ParamType::UInt => input
                .parse::<u32>()
                .map(ParamValue::UInt)
                .map_err(|_| ModuleError::InvalidParameter),
            ParamType::Long => input
                .parse::<i64>()
                .map(ParamValue::Long)
                .map_err(|_| ModuleError::InvalidParameter),
            ParamType::ULong => input
                .parse::<u64>()
                .map(ParamValue::ULong)
                .map_err(|_| ModuleError::InvalidParameter),
            ParamType::String => Ok(ParamValue::Str(String::from(input))),
            ParamType::IntArray => {
                let vals: Result<Vec<i32>, _> =
                    input.split(',').map(|s| s.trim().parse()).collect();
                vals.map(ParamValue::IntArray)
                    .map_err(|_| ModuleError::InvalidParameter)
            }
            ParamType::UIntArray => {
                let vals: Result<Vec<u32>, _> =
                    input.split(',').map(|s| s.trim().parse()).collect();
                vals.map(ParamValue::UIntArray)
                    .map_err(|_| ModuleError::InvalidParameter)
            }
        }
    }
}

// ─── Module Metadata ─────────────────────────────────────────────────────────

pub struct ModuleTaint {
    pub proprietary: bool,
    pub forced_load: bool,
    pub out_of_tree: bool,
    pub unsigned: bool,
    pub staging: bool,
}

impl ModuleTaint {
    pub fn clean() -> Self {
        Self {
            proprietary: false,
            forced_load: false,
            out_of_tree: false,
            unsigned: false,
            staging: false,
        }
    }

    pub fn flags(&self) -> u32 {
        let mut f = 0u32;
        if self.proprietary {
            f |= 1 << 0;
        }
        if self.forced_load {
            f |= 1 << 1;
        }
        if self.out_of_tree {
            f |= 1 << 2;
        }
        if self.unsigned {
            f |= 1 << 3;
        }
        if self.staging {
            f |= 1 << 4;
        }
        f
    }
}

pub struct ModuleSection {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub flags: SectionFlags,
}

pub struct SectionFlags {
    pub alloc: bool,
    pub write: bool,
    pub exec: bool,
}

impl SectionFlags {
    pub fn from_elf(flags: u64) -> Self {
        Self {
            alloc: flags & 0x2 != 0,
            write: flags & 0x1 != 0,
            exec: flags & 0x4 != 0,
        }
    }

    pub fn to_page_flags(&self) -> u64 {
        let mut pf = 0u64;
        if !self.exec {
            pf |= 1 << 63;
        } // NX bit
        if self.write {
            pf |= 1 << 1;
        } // writable
        pf | 1 // present
    }
}

pub struct ModuleInfo {
    pub src_version: Option<String>,
    pub build_id: Option<[u8; 20]>,
    pub vermagic: String,
    pub intree: bool,
    pub retpoline: bool,
}

// ─── Error Types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleError {
    NotFound,
    AlreadyLoaded,
    DependencyMissing(String),
    CircularDependency,
    InUse(u32),
    InvalidFormat,
    InvalidParameter,
    VermagicMismatch,
    LicenseViolation,
    SymbolConflict(String),
    MemoryAllocationFailed,
    RelocationFailed,
    InitFailed(i64),
    PermissionDenied,
    InvalidState(ModuleState),
}

impl core::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotFound => write!(f, "module not found"),
            Self::AlreadyLoaded => write!(f, "module already loaded"),
            Self::DependencyMissing(dep) => write!(f, "missing dependency: {}", dep),
            Self::CircularDependency => write!(f, "circular dependency detected"),
            Self::InUse(count) => write!(f, "module in use (refcount={})", count),
            Self::InvalidFormat => write!(f, "invalid module format"),
            Self::InvalidParameter => write!(f, "invalid parameter value"),
            Self::VermagicMismatch => write!(f, "version magic mismatch"),
            Self::LicenseViolation => write!(f, "GPL-only symbol used by non-GPL module"),
            Self::SymbolConflict(sym) => write!(f, "symbol conflict: {}", sym),
            Self::MemoryAllocationFailed => write!(f, "module memory allocation failed"),
            Self::RelocationFailed => write!(f, "relocation processing failed"),
            Self::InitFailed(code) => write!(f, "module init returned error {}", code),
            Self::PermissionDenied => write!(f, "permission denied"),
            Self::InvalidState(state) => write!(f, "invalid module state: {}", state.as_str()),
        }
    }
}

// ─── Relocation Types ────────────────────────────────────────────────────────

pub struct ModuleRelocation {
    pub section: usize,
    pub offset: u64,
    pub reloc_type: RelocType,
    pub symbol: String,
    pub addend: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocType {
    R64,
    Pc32,
    GotPcRel,
    PltOff64,
    Relative,
    GlobDat,
    JumpSlot,
    TpOff64,
    DtpMod64,
    DtpOff64,
}

impl RelocType {
    pub fn from_elf(r_type: u32) -> Option<Self> {
        match r_type {
            1 => Some(Self::R64),
            2 => Some(Self::Pc32),
            9 => Some(Self::GotPcRel),
            11 => Some(Self::PltOff64),
            8 => Some(Self::Relative),
            6 => Some(Self::GlobDat),
            7 => Some(Self::JumpSlot),
            18 => Some(Self::TpOff64),
            16 => Some(Self::DtpMod64),
            17 => Some(Self::DtpOff64),
            _ => None,
        }
    }
}

// ─── Parse Result ────────────────────────────────────────────────────────────

pub struct ModuleParseResult {
    pub name: String,
    pub info: ModuleInfo,
    pub sections: Vec<ModuleSection>,
    pub symbols: Vec<ExportedSymbol>,
    pub imports: Vec<ImportedSymbol>,
    pub relocations: Vec<ModuleRelocation>,
    pub params: Vec<ModuleParam>,
    pub init_fn: Option<u64>,
    pub exit_fn: Option<u64>,
}

// ─── Memory Allocator ────────────────────────────────────────────────────────

pub struct ModuleMemoryRegion {
    pub base: u64,
    pub size: u64,
    pub used: u64,
    pub module: String,
}

pub struct ModuleMemoryAllocator {
    regions: Vec<ModuleMemoryRegion>,
    total_allocated: u64,
    max_memory: u64,
}

impl ModuleMemoryAllocator {
    pub fn new(max_memory: u64) -> Self {
        Self {
            regions: Vec::new(),
            total_allocated: 0,
            max_memory,
        }
    }

    pub fn allocate(&mut self, size: u64, module_name: &str) -> Result<u64, ModuleError> {
        if self.total_allocated + size > self.max_memory {
            return Err(ModuleError::MemoryAllocationFailed);
        }

        let base = self.next_free_base();
        let region = ModuleMemoryRegion {
            base,
            size,
            used: size,
            module: String::from(module_name),
        };
        self.regions.push(region);
        self.total_allocated += size;
        Ok(base)
    }

    pub fn free(&mut self, base: u64) {
        if let Some(idx) = self.regions.iter().position(|r| r.base == base) {
            self.total_allocated -= self.regions[idx].size;
            self.regions.remove(idx);
        }
    }

    fn next_free_base(&self) -> u64 {
        const MODULE_REGION_START: u64 = 0xFFFF_FF00_0000_0000;
        const ALIGNMENT: u64 = 0x1000;

        if self.regions.is_empty() {
            return MODULE_REGION_START;
        }

        let last = self.regions.last().unwrap();
        let next = last.base + last.size;
        (next + ALIGNMENT - 1) & !(ALIGNMENT - 1)
    }

    pub fn total_used(&self) -> u64 {
        self.total_allocated
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }
}

// ─── Module Loader ───────────────────────────────────────────────────────────

pub struct ModuleLoader {
    modules: BTreeMap<String, KernelModule>,
    symbol_table: BTreeMap<String, (u64, String)>,
    load_order: Vec<String>,
    module_memory: ModuleMemoryAllocator,
    vermagic: String,
}

pub static MODULE_LOADER: Mutex<Option<ModuleLoader>> = Mutex::new(None);

impl ModuleLoader {
    pub fn new() -> Self {
        Self {
            modules: BTreeMap::new(),
            symbol_table: BTreeMap::new(),
            load_order: Vec::new(),
            module_memory: ModuleMemoryAllocator::new(256 * 1024 * 1024), // 256 MiB max
            vermagic: String::from("RaeenOS 0.1.0 SMP preempt mod_unload"),
        }
    }

    pub fn load_module(
        &mut self,
        name: &str,
        data: &[u8],
        params: &[(&str, &str)],
    ) -> Result<(), ModuleError> {
        if self.modules.contains_key(name) {
            return Err(ModuleError::AlreadyLoaded);
        }

        let parsed = self.parse_module_elf(data)?;

        if !self.check_vermagic(&parsed.info.vermagic) {
            return Err(ModuleError::VermagicMismatch);
        }

        let mut module = KernelModule {
            name: String::from(name),
            version: ModuleVersion::new(0, 1, 0),
            description: String::new(),
            author: String::new(),
            license: ModuleLicense::Gpl,
            state: ModuleState::Unformed,
            base_addr: 0,
            size: 0,
            init_fn: parsed.init_fn,
            exit_fn: parsed.exit_fn,
            dependencies: Vec::new(),
            dependents: Vec::new(),
            symbols_exported: parsed.symbols,
            symbols_imported: parsed.imports,
            parameters: parsed.params,
            refcount: 0,
            load_time: 0,
            taint: ModuleTaint::clean(),
            sections: parsed.sections,
            info: parsed.info,
        };

        self.check_dependencies(&module)?;

        if !self.check_license(&module.license) {
            for imp in &module.symbols_imported {
                if let Some((_, owner)) = self.symbol_table.get(&imp.name) {
                    if let Some(owner_mod) = self.modules.get(owner) {
                        for exp in &owner_mod.symbols_exported {
                            if exp.name == imp.name && exp.gpl_only {
                                return Err(ModuleError::LicenseViolation);
                            }
                        }
                    }
                }
            }
        }

        let total_size: u64 = module.sections.iter().map(|s| s.size).sum();
        let base = self.allocate_module_memory(total_size)?;
        module.base_addr = base;
        module.size = total_size;

        self.apply_relocations(&mut module, &parsed.relocations)?;

        for (pname, pval) in params {
            if let Some(param) = module.parameters.iter_mut().find(|p| p.name == *pname) {
                param.value = ParamValue::parse(param.param_type, pval)?;
            }
        }

        for sym in &module.symbols_exported {
            if self.symbol_table.contains_key(&sym.name) {
                return Err(ModuleError::SymbolConflict(sym.name.clone()));
            }
            self.symbol_table
                .insert(sym.name.clone(), (sym.address, String::from(name)));
        }

        module.state = ModuleState::Coming;
        self.call_init(&module)?;
        module.state = ModuleState::Live;

        self.load_order.push(String::from(name));
        self.modules.insert(String::from(name), module);
        Ok(())
    }

    pub fn unload_module(&mut self, name: &str) -> Result<(), ModuleError> {
        let module = self.modules.get(name).ok_or(ModuleError::NotFound)?;

        if module.refcount > 0 {
            return Err(ModuleError::InUse(module.refcount));
        }

        if !module.dependents.is_empty() {
            return Err(ModuleError::InUse(module.dependents.len() as u32));
        }

        if module.state != ModuleState::Live {
            return Err(ModuleError::InvalidState(module.state));
        }

        let base_addr = module.base_addr;
        let exported_names: Vec<String> = module
            .symbols_exported
            .iter()
            .map(|s| s.name.clone())
            .collect();

        self.call_exit(module);

        for sym_name in &exported_names {
            self.symbol_table.remove(sym_name);
        }

        self.free_module_memory(base_addr);
        self.load_order.retain(|n| n != name);
        self.modules.remove(name);
        Ok(())
    }

    pub fn get_module(&self, name: &str) -> Option<&KernelModule> {
        self.modules.get(name)
    }

    pub fn list_modules(&self) -> Vec<&KernelModule> {
        self.modules.values().collect()
    }

    pub fn module_loaded(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    pub fn resolve_symbol(&self, name: &str) -> Option<u64> {
        self.symbol_table.get(name).map(|(addr, _)| *addr)
    }

    pub fn export_symbol(&mut self, name: &str, address: u64, module: &str, gpl_only: bool) {
        self.symbol_table
            .insert(String::from(name), (address, String::from(module)));

        if let Some(m) = self.modules.get_mut(module) {
            m.symbols_exported.push(ExportedSymbol {
                name: String::from(name),
                address,
                namespace: None,
                gpl_only,
            });
        }
    }

    pub fn get_module_param(&self, module: &str, param: &str) -> Option<&ParamValue> {
        self.modules
            .get(module)?
            .parameters
            .iter()
            .find(|p| p.name == param)
            .map(|p| &p.value)
    }

    pub fn set_module_param(
        &mut self,
        module: &str,
        param: &str,
        value: &str,
    ) -> Result<(), ModuleError> {
        let m = self.modules.get_mut(module).ok_or(ModuleError::NotFound)?;
        let p = m
            .parameters
            .iter_mut()
            .find(|p| p.name == param)
            .ok_or(ModuleError::InvalidParameter)?;

        p.value = ParamValue::parse(p.param_type, value)?;
        Ok(())
    }

    pub fn check_dependencies(&self, module: &KernelModule) -> Result<(), ModuleError> {
        for dep in &module.dependencies {
            if !self.modules.contains_key(dep) {
                return Err(ModuleError::DependencyMissing(dep.clone()));
            }
            if self.modules.get(dep).unwrap().state != ModuleState::Live {
                return Err(ModuleError::DependencyMissing(dep.clone()));
            }
        }
        Ok(())
    }

    pub fn resolve_dependencies(&self, name: &str) -> Result<Vec<String>, ModuleError> {
        let module = self.modules.get(name).ok_or(ModuleError::NotFound)?;
        let mut resolved = Vec::new();
        let mut visiting = Vec::new();
        self.resolve_deps_recursive(&module.dependencies, &mut resolved, &mut visiting)?;
        Ok(resolved)
    }

    fn resolve_deps_recursive(
        &self,
        deps: &[String],
        resolved: &mut Vec<String>,
        visiting: &mut Vec<String>,
    ) -> Result<(), ModuleError> {
        for dep in deps {
            if visiting.contains(dep) {
                return Err(ModuleError::CircularDependency);
            }
            if resolved.contains(dep) {
                continue;
            }
            visiting.push(dep.clone());
            if let Some(m) = self.modules.get(dep) {
                self.resolve_deps_recursive(&m.dependencies, resolved, visiting)?;
            }
            visiting.retain(|v| v != dep);
            resolved.push(dep.clone());
        }
        Ok(())
    }

    pub fn force_unload(&mut self, name: &str) -> Result<(), ModuleError> {
        let module = self.modules.get_mut(name).ok_or(ModuleError::NotFound)?;
        module.state = ModuleState::Going;
        module.refcount = 0;

        let base_addr = module.base_addr;
        let exported_names: Vec<String> = module
            .symbols_exported
            .iter()
            .map(|s| s.name.clone())
            .collect();
        let dependents: Vec<String> = module.dependents.clone();

        self.call_exit(self.modules.get(name).unwrap());

        for dep_name in &dependents {
            if let Some(dep) = self.modules.get_mut(dep_name) {
                dep.dependencies.retain(|d| d != name);
            }
        }

        for sym_name in &exported_names {
            self.symbol_table.remove(sym_name);
        }

        self.free_module_memory(base_addr);
        self.load_order.retain(|n| n != name);
        self.modules.remove(name);
        Ok(())
    }

    pub fn modinfo(&self, name: &str) -> Option<String> {
        let m = self.modules.get(name)?;
        Some(format!(
            "name:        {}\n\
             version:     {}\n\
             description: {}\n\
             author:      {}\n\
             license:     {}\n\
             state:       {}\n\
             base_addr:   0x{:016x}\n\
             size:        {} bytes\n\
             refcount:    {}\n\
             depends:     {}\n\
             vermagic:    {}\n\
             intree:      {}\n\
             retpoline:   {}\n\
             taint:       0x{:x}",
            m.name,
            m.version,
            m.description,
            m.author,
            m.license.name(),
            m.state.as_str(),
            m.base_addr,
            m.size,
            m.refcount,
            if m.dependencies.is_empty() {
                String::from("(none)")
            } else {
                m.dependencies.join(", ")
            },
            m.info.vermagic,
            m.info.intree,
            m.info.retpoline,
            m.taint.flags(),
        ))
    }

    pub fn module_refcount(&self, name: &str) -> Option<u32> {
        self.modules.get(name).map(|m| m.refcount)
    }

    pub fn try_module_get(&mut self, name: &str) -> bool {
        if let Some(m) = self.modules.get_mut(name) {
            if m.state == ModuleState::Live {
                m.refcount += 1;
                return true;
            }
        }
        false
    }

    pub fn module_put(&mut self, name: &str) {
        if let Some(m) = self.modules.get_mut(name) {
            if m.refcount > 0 {
                m.refcount -= 1;
            }
        }
    }

    fn parse_module_elf(&self, data: &[u8]) -> Result<ModuleParseResult, ModuleError> {
        if data.len() < 64 {
            return Err(ModuleError::InvalidFormat);
        }

        // Validate ELF magic
        if &data[0..4] != b"\x7fELF" {
            return Err(ModuleError::InvalidFormat);
        }

        // Must be 64-bit (class 2)
        if data[4] != 2 {
            return Err(ModuleError::InvalidFormat);
        }

        // Must be little-endian
        if data[5] != 1 {
            return Err(ModuleError::InvalidFormat);
        }

        // Must be relocatable (ET_REL = 1)
        let e_type = u16::from_le_bytes([data[16], data[17]]);
        if e_type != 1 {
            return Err(ModuleError::InvalidFormat);
        }

        let e_shoff = u64::from_le_bytes([
            data[40], data[41], data[42], data[43], data[44], data[45], data[46], data[47],
        ]);
        let e_shentsize = u16::from_le_bytes([data[58], data[59]]) as u64;
        let e_shnum = u16::from_le_bytes([data[60], data[61]]) as u64;

        let mut sections = Vec::new();
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut relocations = Vec::new();
        let mut init_fn = None;
        let mut exit_fn = None;

        // Parse section headers
        for i in 0..e_shnum {
            let sh_offset = e_shoff + i * e_shentsize;
            if (sh_offset + e_shentsize) as usize > data.len() {
                break;
            }

            let off = sh_offset as usize;
            let sh_type =
                u32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]]);
            let sh_flags = u64::from_le_bytes([
                data[off + 8],
                data[off + 9],
                data[off + 10],
                data[off + 11],
                data[off + 12],
                data[off + 13],
                data[off + 14],
                data[off + 15],
            ]);
            let sh_addr = u64::from_le_bytes([
                data[off + 16],
                data[off + 17],
                data[off + 18],
                data[off + 19],
                data[off + 20],
                data[off + 21],
                data[off + 22],
                data[off + 23],
            ]);
            let sh_size = u64::from_le_bytes([
                data[off + 32],
                data[off + 33],
                data[off + 34],
                data[off + 35],
                data[off + 36],
                data[off + 37],
                data[off + 38],
                data[off + 39],
            ]);

            if sh_type == 1 || sh_type == 8 {
                // SHT_PROGBITS or SHT_NOBITS
                sections.push(ModuleSection {
                    name: format!(".section{}", i),
                    address: sh_addr,
                    size: sh_size,
                    flags: SectionFlags::from_elf(sh_flags),
                });
            }
        }

        // Stub: in a real implementation we'd fully parse symtab/rela sections
        let _ = (
            &mut symbols,
            &mut imports,
            &mut relocations,
            &mut init_fn,
            &mut exit_fn,
        );

        Ok(ModuleParseResult {
            name: String::from("parsed_module"),
            info: ModuleInfo {
                src_version: None,
                build_id: None,
                vermagic: self.vermagic.clone(),
                intree: false,
                retpoline: true,
            },
            sections,
            symbols,
            imports,
            relocations,
            params: Vec::new(),
            init_fn,
            exit_fn,
        })
    }

    fn allocate_module_memory(&mut self, size: u64) -> Result<u64, ModuleError> {
        self.module_memory.allocate(size, "pending")
    }

    fn free_module_memory(&mut self, base: u64) {
        self.module_memory.free(base);
    }

    fn apply_relocations(
        &self,
        module: &mut KernelModule,
        relocs: &[ModuleRelocation],
    ) -> Result<(), ModuleError> {
        for reloc in relocs {
            let sym_addr = self
                .resolve_symbol(&reloc.symbol)
                .or_else(|| {
                    module
                        .symbols_exported
                        .iter()
                        .find(|s| s.name == reloc.symbol)
                        .map(|s| s.address)
                })
                .ok_or(ModuleError::RelocationFailed)?;

            if reloc.section >= module.sections.len() {
                return Err(ModuleError::RelocationFailed);
            }

            let section_base = module.sections[reloc.section].address + module.base_addr;
            let target = section_base + reloc.offset;

            match reloc.reloc_type {
                RelocType::R64 => {
                    let value = (sym_addr as i64 + reloc.addend) as u64;
                    unsafe {
                        core::ptr::write(target as *mut u64, value);
                    }
                }
                RelocType::Pc32 => {
                    let value = ((sym_addr as i64 + reloc.addend) - target as i64) as i32;
                    unsafe {
                        core::ptr::write(target as *mut i32, value);
                    }
                }
                RelocType::Relative => {
                    let value = (module.base_addr as i64 + reloc.addend) as u64;
                    unsafe {
                        core::ptr::write(target as *mut u64, value);
                    }
                }
                RelocType::GlobDat | RelocType::JumpSlot => unsafe {
                    core::ptr::write(target as *mut u64, sym_addr);
                },
                _ => {
                    // GotPcRel, PltOff64, TpOff64, DtpMod64, DtpOff64 handled similarly
                    let value = (sym_addr as i64 + reloc.addend) as u64;
                    unsafe {
                        core::ptr::write(target as *mut u64, value);
                    }
                }
            }
        }
        Ok(())
    }

    fn check_vermagic(&self, module_vermagic: &str) -> bool {
        self.vermagic == module_vermagic
    }

    fn check_license(&self, license: &ModuleLicense) -> bool {
        license.is_gpl_compatible()
    }

    fn call_init(&self, module: &KernelModule) -> Result<(), ModuleError> {
        if let Some(init_addr) = module.init_fn {
            let init: fn() -> i64 = unsafe { core::mem::transmute(init_addr + module.base_addr) };
            let ret = init();
            if ret != 0 {
                return Err(ModuleError::InitFailed(ret));
            }
        }
        Ok(())
    }

    fn call_exit(&self, module: &KernelModule) {
        if let Some(exit_addr) = module.exit_fn {
            let exit: fn() = unsafe { core::mem::transmute(exit_addr + module.base_addr) };
            exit();
        }
    }
}

// ─── Notification System ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleEvent {
    Loading,
    Loaded,
    Unloading,
    Unloaded,
    StateChange(ModuleState),
}

pub type ModuleNotifyFn = fn(&str, ModuleEvent);

static MODULE_NOTIFIERS: Mutex<Vec<ModuleNotifyFn>> = Mutex::new(Vec::new());

pub fn register_module_notifier(callback: ModuleNotifyFn) {
    MODULE_NOTIFIERS.lock().push(callback);
}

pub fn unregister_module_notifier(callback: ModuleNotifyFn) {
    // `fn` items are zero-sized; direct `f != callback` is not guaranteed to
    // compare addresses (rustc emits a warning that the result is meaningless).
    // Cast through `usize` so we actually compare instruction-pointer values.
    let target = callback as usize;
    MODULE_NOTIFIERS.lock().retain(|&f| (f as usize) != target);
}

fn notify_module_event(name: &str, event: ModuleEvent) {
    let notifiers = MODULE_NOTIFIERS.lock();
    for notifier in notifiers.iter() {
        notifier(name, event);
    }
}

// ─── Public API ──────────────────────────────────────────────────────────────

pub fn init() {
    let loader = ModuleLoader::new();
    *MODULE_LOADER.lock() = Some(loader);
}

pub fn load_module(name: &str, data: &[u8], params: &[(&str, &str)]) -> Result<(), ModuleError> {
    notify_module_event(name, ModuleEvent::Loading);
    let mut loader = MODULE_LOADER.lock();
    let loader = loader.as_mut().ok_or(ModuleError::PermissionDenied)?;
    let result = loader.load_module(name, data, params);
    if result.is_ok() {
        notify_module_event(name, ModuleEvent::Loaded);
    }
    result
}

pub fn unload_module(name: &str) -> Result<(), ModuleError> {
    notify_module_event(name, ModuleEvent::Unloading);
    let mut loader = MODULE_LOADER.lock();
    let loader = loader.as_mut().ok_or(ModuleError::PermissionDenied)?;
    let result = loader.unload_module(name);
    if result.is_ok() {
        notify_module_event(name, ModuleEvent::Unloaded);
    }
    result
}

pub fn module_loaded(name: &str) -> bool {
    let loader = MODULE_LOADER.lock();
    loader.as_ref().map_or(false, |l| l.module_loaded(name))
}

pub fn resolve_symbol(name: &str) -> Option<u64> {
    let loader = MODULE_LOADER.lock();
    loader.as_ref().and_then(|l| l.resolve_symbol(name))
}

pub fn module_count() -> usize {
    let loader = MODULE_LOADER.lock();
    loader.as_ref().map_or(0, |l| l.modules.len())
}
