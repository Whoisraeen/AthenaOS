//! raewasm — the sandboxed WebAssembly runtime for AthenaOS (Concept §AthGuard —
//! "WebAssembly sandboxed runtime for untrusted extensions/widgets/apps": any
//! language in, one safe runtime — the anti-Electron).
//!
//! This parses, decodes, AND EXECUTES a `.wasm` module: the LEB128 integer codec +
//! the module header + a bounds-checked section walker + per-section decoders
//! (type/function/export/code/memory) + a stack-machine interpreter for the i32 core
//! (arithmetic/comparison/bitwise/shift, `local.*`, `drop`/`select`, linear-memory
//! load/store/size/grow) with **structured control flow** (`block`/`loop`/`if`/`else`/
//! `br`/`br_if`/`return`) and **`call`** (defined functions + host imports). The
//! sandbox guardrails are load-bearing — this runtime exists precisely to run UNTRUSTED
//! code, so every step is bounded: memory growth is capped, an instruction budget
//! (`fuel`) traps infinite loops, a call-depth budget traps runaway recursion, and any
//! fault (OOB access, div-by-zero, stack underflow, unknown opcode) returns an error
//! instead of panicking. The capability gate is the [`HostEnv`] trait: raewasm stays
//! kernel-free and resolves an imported `call` to its index; the embedder (AthGuard)
//! maps that index to a `Cap` and grants or denies it (deny → trap). Still to wire:
//! the i64/f32/f64 value types (i32-only today).
//!
//! Pure logic — `#![no_std]` + alloc, zero deps — host-KAT'd against the canonical
//! Wasm binary format (WebAssembly Core Spec 1.0 §5).

#![cfg_attr(not(test), no_std)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

pub mod pkg;

/// The Wasm magic `\0asm` (WebAssembly Core Spec §5.1.1).
pub const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];
/// The only binary format version (the MVP/1.0 format).
pub const WASM_VERSION: u32 = 1;

/// Top-level section ids (Core Spec §5.5.2). Id 0 is the custom section.
pub mod section_id {
    pub const CUSTOM: u8 = 0;
    pub const TYPE: u8 = 1;
    pub const IMPORT: u8 = 2;
    pub const FUNCTION: u8 = 3;
    pub const TABLE: u8 = 4;
    pub const MEMORY: u8 = 5;
    pub const GLOBAL: u8 = 6;
    pub const EXPORT: u8 = 7;
    pub const START: u8 = 8;
    pub const ELEMENT: u8 = 9;
    pub const CODE: u8 = 10;
    pub const DATA: u8 = 11;
    pub const DATA_COUNT: u8 = 12;
    /// Highest id this decoder recognizes; anything above is rejected.
    pub const MAX: u8 = 12;
}

/// A decode error. Every variant means the module is rejected (never executed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WasmError {
    /// Fewer than the 8-byte header is present.
    TooShort,
    /// First 4 bytes are not [`WASM_MAGIC`].
    BadMagic,
    /// Binary version is not [`WASM_VERSION`].
    BadVersion,
    /// A LEB128 value or a declared section length runs past the end of the buffer.
    Truncated,
    /// A LEB128 value does not terminate within its type's bit width (overlong).
    BadLeb128,
    /// A section id is above [`section_id::MAX`].
    BadSectionId,
}

/// A decoded top-level section: its id and a borrowed slice of its body (zero-copy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    pub id: u8,
    pub body: &'a [u8],
}

/// A decoded module: the version + the ordered list of its top-level sections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module<'a> {
    pub version: u32,
    pub sections: Vec<Section<'a>>,
}

impl<'a> Module<'a> {
    /// The first section with id `id`, if present.
    pub fn section(&self, id: u8) -> Option<&Section<'a>> {
        self.sections.iter().find(|s| s.id == id)
    }
}

/// Read an unsigned LEB128 integer (max `bits` wide) at `*pos`, advancing `*pos`.
/// Returns `None` (→ `Truncated`/`BadLeb128` upstream) on a buffer overrun or an
/// overlong encoding.
pub fn read_uleb(bytes: &[u8], pos: &mut usize, bits: u32) -> Option<u64> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        // A byte that would push bits past `bits` is malformed.
        if shift >= bits {
            return None;
        }
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
    }
}

/// Read a signed LEB128 integer (max `bits` wide), sign-extended.
pub fn read_sleb(bytes: &[u8], pos: &mut usize, bits: u32) -> Option<i64> {
    let mut result: i64 = 0;
    let mut shift: u32 = 0;
    loop {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        if shift >= bits {
            return None;
        }
        result |= ((byte & 0x7F) as i64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 64 && (byte & 0x40) != 0 {
                result |= -1i64 << shift;
            }
            return Some(result);
        }
    }
}

/// Decode a `.wasm` module into its header + top-level sections. Bounds-checked end
/// to end: a malformed/hostile module returns `Err`, never panics or reads OOB.
pub fn parse(bytes: &[u8]) -> Result<Module<'_>, WasmError> {
    if bytes.len() < 8 {
        return Err(WasmError::TooShort);
    }
    if bytes[0..4] != WASM_MAGIC {
        return Err(WasmError::BadMagic);
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != WASM_VERSION {
        return Err(WasmError::BadVersion);
    }

    let mut pos = 8usize;
    let mut sections = Vec::new();
    while pos < bytes.len() {
        let id = bytes[pos];
        pos += 1;
        if id > section_id::MAX {
            return Err(WasmError::BadSectionId);
        }
        let size = read_uleb(bytes, &mut pos, 32).ok_or(WasmError::BadLeb128)? as usize;
        let end = pos.checked_add(size).ok_or(WasmError::Truncated)?;
        let body = bytes.get(pos..end).ok_or(WasmError::Truncated)?;
        sections.push(Section { id, body });
        pos = end;
    }
    Ok(Module { version, sections })
}

// ── per-section decode (Core Spec §5.3-5.5) ──────────────────────────────────

/// A Wasm value type (Core Spec §5.3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
}

impl ValType {
    fn from_byte(b: u8) -> Option<ValType> {
        match b {
            0x7F => Some(ValType::I32),
            0x7E => Some(ValType::I64),
            0x7D => Some(ValType::F32),
            0x7C => Some(ValType::F64),
            _ => None,
        }
    }
}

/// A function signature: parameter + result value types (Core Spec §5.3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

fn decode_valtypes(body: &[u8], pos: &mut usize) -> Option<Vec<ValType>> {
    let n = read_uleb(body, pos, 32)? as usize;
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        out.push(ValType::from_byte(*body.get(*pos)?)?);
        *pos += 1;
    }
    Some(out)
}

/// Decode a TYPE section body into the module's function signatures. `None` on any
/// malformed/truncated input (a sandbox decoder never trusts the bytes).
pub fn decode_type_section(body: &[u8]) -> Option<Vec<FuncType>> {
    let mut pos = 0usize;
    let n = read_uleb(body, &mut pos, 32)? as usize;
    let mut out = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        if *body.get(pos)? != 0x60 {
            return None; // the functype tag
        }
        pos += 1;
        let params = decode_valtypes(body, &mut pos)?;
        let results = decode_valtypes(body, &mut pos)?;
        out.push(FuncType { params, results });
    }
    Some(out)
}

/// Decode a FUNCTION section body: the type index of each defined function.
pub fn decode_function_section(body: &[u8]) -> Option<Vec<u32>> {
    let mut pos = 0usize;
    let n = read_uleb(body, &mut pos, 32)? as usize;
    let mut out = Vec::with_capacity(n.min(65536));
    for _ in 0..n {
        out.push(read_uleb(body, &mut pos, 32)? as u32);
    }
    Some(out)
}

/// What an export refers to (Core Spec §5.5.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Func,
    Table,
    Memory,
    Global,
}

/// One export: its name + what it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    pub name: alloc::string::String,
    pub kind: ExportKind,
    pub index: u32,
}

/// Decode an EXPORT section body.
pub fn decode_export_section(body: &[u8]) -> Option<Vec<Export>> {
    let mut pos = 0usize;
    let n = read_uleb(body, &mut pos, 32)? as usize;
    let mut out = Vec::with_capacity(n.min(65536));
    for _ in 0..n {
        let name_len = read_uleb(body, &mut pos, 32)? as usize;
        let name_end = pos.checked_add(name_len)?;
        let name = core::str::from_utf8(body.get(pos..name_end)?).ok()?.into();
        pos = name_end;
        let kind = match *body.get(pos)? {
            0x00 => ExportKind::Func,
            0x01 => ExportKind::Table,
            0x02 => ExportKind::Memory,
            0x03 => ExportKind::Global,
            _ => return None,
        };
        pos += 1;
        let index = read_uleb(body, &mut pos, 32)? as u32;
        out.push(Export { name, kind, index });
    }
    Some(out)
}

/// A function body from the CODE section: its local declarations + the raw
/// instruction bytes (the interpreter slice decodes these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncBody<'a> {
    /// `(count, type)` local-variable groups (run-length encoded, per the spec).
    pub locals: Vec<(u32, ValType)>,
    /// Instruction bytes, up to and including the function's terminating `end`.
    pub code: &'a [u8],
}

/// Decode a CODE section body into per-function bodies.
pub fn decode_code_section(body: &[u8]) -> Option<Vec<FuncBody<'_>>> {
    let mut pos = 0usize;
    let n = read_uleb(body, &mut pos, 32)? as usize;
    let mut out = Vec::with_capacity(n.min(65536));
    for _ in 0..n {
        let size = read_uleb(body, &mut pos, 32)? as usize;
        let entry_end = pos.checked_add(size)?;
        let entry = body.get(pos..entry_end)?;
        let mut lp = 0usize;
        let lgroups = read_uleb(entry, &mut lp, 32)? as usize;
        let mut locals = Vec::with_capacity(lgroups.min(1024));
        for _ in 0..lgroups {
            let count = read_uleb(entry, &mut lp, 32)? as u32;
            let vt = ValType::from_byte(*entry.get(lp)?)?;
            lp += 1;
            locals.push((count, vt));
        }
        let code = entry.get(lp..)?;
        out.push(FuncBody { locals, code });
        pos = entry_end;
    }
    Some(out)
}

// ── interpreter — i32 core: arithmetic + comparison, linear memory, and
//    structured control flow (block/loop/if/else/br/br_if/return) (Core Spec §4) ─

/// A Wasm linear-memory page is 64 KiB (Core Spec §4.2.8).
pub const PAGE_SIZE: usize = 65536;
/// Hard ceiling on linear-memory growth — a sandbox never lets untrusted code
/// balloon memory without bound (64 MiB).
pub const MAX_PAGES_CAP: u32 = 1024;
/// Default instruction budget: untrusted code that loops forever must TRAP (return
/// `None`), not hang the host. Every executed instruction costs one unit.
pub const DEFAULT_FUEL: u64 = 50_000_000;

/// The decode of a MEMORY section: each memory's `(min, max)` page limits.
pub fn decode_memory_section(body: &[u8]) -> Option<Vec<(u32, Option<u32>)>> {
    let mut pos = 0usize;
    let n = read_uleb(body, &mut pos, 32)? as usize;
    let mut out = Vec::with_capacity(n.min(16));
    for _ in 0..n {
        let flags = *body.get(pos)?;
        pos += 1;
        let min = read_uleb(body, &mut pos, 32)? as u32;
        let max = if flags & 1 != 0 {
            Some(read_uleb(body, &mut pos, 32)? as u32)
        } else {
            None
        };
        out.push((min, max));
    }
    Some(out)
}

/// Maximum dynamic call depth — a guest that recurses without a base case must TRAP,
/// not overflow the host stack. Conservative for a kernel stack; tunable.
pub const MAX_CALL_DEPTH: u32 = 256;

/// A function import: which module/field it names + the index of its signature. The
/// embedder binds each import (by `module`/`name`) to a capability at instantiate
/// time — this is the seam where a `call` to host code becomes a `Cap`-gated request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportFunc {
    pub module: String,
    pub name: String,
    pub type_index: u32,
}

/// Decode an IMPORT section, returning the imported FUNCTIONS in declaration order.
/// The function index space starts with imports, then the module's own functions.
/// Table/memory/global imports are parsed-through (to advance) but not surfaced.
pub fn decode_import_section(body: &[u8]) -> Option<Vec<ImportFunc>> {
    let mut pos = 0usize;
    let n = read_uleb(body, &mut pos, 32)? as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let mlen = read_uleb(body, &mut pos, 32)? as usize;
        let mend = pos.checked_add(mlen)?;
        let module = core::str::from_utf8(body.get(pos..mend)?).ok()?.into();
        pos = mend;
        let flen = read_uleb(body, &mut pos, 32)? as usize;
        let fend = pos.checked_add(flen)?;
        let name = core::str::from_utf8(body.get(pos..fend)?).ok()?.into();
        pos = fend;
        let kind = *body.get(pos)?;
        pos += 1;
        match kind {
            0x00 => {
                let type_index = read_uleb(body, &mut pos, 32)? as u32;
                out.push(ImportFunc {
                    module,
                    name,
                    type_index,
                });
            }
            0x01 => {
                // table: elemtype byte + limits
                let _elem = *body.get(pos)?;
                pos += 1;
                let flags = *body.get(pos)?;
                pos += 1;
                read_uleb(body, &mut pos, 32)?; // min
                if flags & 1 != 0 {
                    read_uleb(body, &mut pos, 32)?; // max
                }
            }
            0x02 => {
                // memory: limits
                let flags = *body.get(pos)?;
                pos += 1;
                read_uleb(body, &mut pos, 32)?;
                if flags & 1 != 0 {
                    read_uleb(body, &mut pos, 32)?;
                }
            }
            0x03 => {
                // global: valtype + mutability byte
                let _ = *body.get(pos.checked_add(1)?)?;
                pos += 2;
            }
            _ => return None,
        }
    }
    Some(out)
}

/// The host-import surface. The Wasm sandbox cannot touch the system directly — every
/// outside effect goes through an embedder-supplied `HostEnv`, which is where the
/// capability check lives. raewasm stays kernel-free: it resolves an imported `call`
/// to its import index and hands it here; the embedder (the kernel/AthGuard) maps
/// that index to a `Cap` and either services the call or denies it (returns `None`,
/// which traps the guest). This is the "one safe runtime" gate — untrusted code can
/// only do what its manifest's capabilities permit.
pub trait HostEnv {
    /// Service host import `import_index` with `args`; return its results, or `None`
    /// to TRAP (e.g. the capability is not granted). A wrong result arity also traps.
    fn call_import(&mut self, import_index: u32, args: &[i32]) -> Option<Vec<i32>>;
}

/// A `HostEnv` that denies every import — the default for pure-Wasm execution (no host
/// surface granted). A module that `call`s an import under this env traps.
struct DenyHost;
impl HostEnv for DenyHost {
    fn call_import(&mut self, _import_index: u32, _args: &[i32]) -> Option<Vec<i32>> {
        None
    }
}

/// An instantiated module: its decoded type/import/function/code/export tables plus
/// its linear-memory limits — everything needed to call an export (and to resolve a
/// `call` to a defined function or a host import).
pub struct Instance<'a> {
    types: Vec<FuncType>,
    imports: Vec<ImportFunc>,
    func_types: Vec<u32>,
    codes: Vec<FuncBody<'a>>,
    exports: Vec<Export>,
    init_pages: u32,
    max_pages: u32,
}

impl<'a> Instance<'a> {
    /// An empty instance (no functions) — used for raw `Vm::run`/`execute_i32`, where
    /// a stray `call` has no target and therefore traps.
    fn empty() -> Instance<'a> {
        Instance {
            types: Vec::new(),
            imports: Vec::new(),
            func_types: Vec::new(),
            codes: Vec::new(),
            exports: Vec::new(),
            init_pages: 0,
            max_pages: MAX_PAGES_CAP,
        }
    }

    /// The imports this module declares, in function-index order (so the embedder can
    /// bind each to a capability before calling an export).
    pub fn imports(&self) -> &[ImportFunc] {
        &self.imports
    }

    /// Call exported function `name` with i32 `args`, routing host imports through
    /// `host` (the capability gate) and bounding execution by `fuel`. Returns the
    /// result values, or `None` on any decode/lookup/trap (incl. a denied import).
    pub fn call_export(
        &self,
        name: &str,
        args: &[i32],
        host: &mut dyn HostEnv,
        fuel: u64,
    ) -> Option<Vec<i32>> {
        let export = self
            .exports
            .iter()
            .find(|e| e.kind == ExportKind::Func && e.name == name)?;
        let fi = export.index as usize;
        let num_imports = self.imports.len();
        let type_index = if fi < num_imports {
            self.imports[fi].type_index
        } else {
            *self.func_types.get(fi - num_imports)?
        };
        let ftype = self.types.get(type_index as usize)?;
        if args.len() != ftype.params.len() {
            return None;
        }
        let nresults = ftype.results.len();
        let mut vm = Vm::new(self.init_pages, self.max_pages, fuel);
        let mut stack: Vec<i32> = args.to_vec();
        vm.invoke(self, host, export.index, &mut stack)?;
        if stack.len() < nresults {
            return None;
        }
        Some(stack.split_off(stack.len() - nresults))
    }
}

/// Decode + instantiate a module: parse the header/sections and decode the type,
/// import, function, export, code, and memory tables. Validates that the FUNCTION and
/// CODE sections agree (one body per defined function). `None` on any malformed input.
pub fn instantiate(module_bytes: &[u8]) -> Option<Instance<'_>> {
    let module = parse(module_bytes).ok()?;
    let types = match module.section(section_id::TYPE) {
        Some(s) => decode_type_section(s.body)?,
        None => Vec::new(),
    };
    let imports = match module.section(section_id::IMPORT) {
        Some(s) => decode_import_section(s.body)?,
        None => Vec::new(),
    };
    let func_types = match module.section(section_id::FUNCTION) {
        Some(s) => decode_function_section(s.body)?,
        None => Vec::new(),
    };
    let exports = match module.section(section_id::EXPORT) {
        Some(s) => decode_export_section(s.body)?,
        None => Vec::new(),
    };
    let codes = match module.section(section_id::CODE) {
        Some(s) => decode_code_section(s.body)?,
        None => Vec::new(),
    };
    if func_types.len() != codes.len() {
        return None; // every defined function needs exactly one body
    }
    let (init_pages, max_pages) = match module.section(section_id::MEMORY) {
        Some(s) => {
            let (min, max) = *decode_memory_section(s.body)?.first()?;
            (min, max.unwrap_or(MAX_PAGES_CAP))
        }
        None => (0, MAX_PAGES_CAP),
    };
    Some(Instance {
        types,
        imports,
        func_types,
        codes,
        exports,
        init_pages,
        max_pages,
    })
}

/// How a block body terminated — the structured-control-flow signal threaded up the
/// recursive interpreter. `Branch(n)` is already relative to the PARENT block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// Consumed this block's `end`; the `usize` is the position just past it.
    End(usize),
    /// Consumed an `else` (only meaningful inside an `if` then-body); pos past it.
    Else(usize),
    /// A `br`/`br_if` still targeting `n` blocks above the parent.
    Branch(u32),
    /// A `return` — unwind the whole function.
    Return,
}

/// Advance past one instruction (opcode + its immediates) without executing it.
/// Used to skip untaken branches; an opcode this slice doesn't recognize returns
/// `None`, so a module using an unsupported instruction is rejected, not mis-skipped.
fn skip_instr(code: &[u8], mut pos: usize) -> Option<usize> {
    let op = *code.get(pos)?;
    pos += 1;
    match op {
        // No immediate (control delimiters, drop/select, and every numeric op).
        0x00 | 0x01 | 0x05 | 0x0B | 0x0F | 0x1A | 0x1B | 0x45..=0xC4 => Some(pos),
        // One blocktype byte.
        0x02 | 0x03 | 0x04 => {
            code.get(pos)?;
            Some(pos + 1)
        }
        // One uleb index (br, br_if, call, local/global get/set/tee).
        0x0C | 0x0D | 0x10 | 0x20..=0x24 => {
            read_uleb(code, &mut pos, 32)?;
            Some(pos)
        }
        // br_table: a vector of label indices plus the default.
        0x0E => {
            let n = read_uleb(code, &mut pos, 32)? as usize;
            for _ in 0..=n {
                read_uleb(code, &mut pos, 32)?;
            }
            Some(pos)
        }
        // call_indirect: type index + table index.
        0x11 => {
            read_uleb(code, &mut pos, 32)?;
            read_uleb(code, &mut pos, 32)?;
            Some(pos)
        }
        // Memory load/store: align + offset memarg.
        0x28..=0x3E => {
            read_uleb(code, &mut pos, 32)?;
            read_uleb(code, &mut pos, 32)?;
            Some(pos)
        }
        // memory.size / memory.grow: one memory-index byte.
        0x3F | 0x40 => Some(pos + 1),
        // const immediates.
        0x41 | 0x42 => {
            read_sleb(code, &mut pos, 64)?;
            Some(pos)
        }
        0x43 => Some(pos + 4), // f32.const
        0x44 => Some(pos + 8), // f64.const
        _ => None,
    }
}

/// Scan from the start of a block body to the position just past its matching `end`,
/// tracking nesting (a nested block/loop/if raises depth; `end` lowers it).
fn skip_block_body(code: &[u8], mut pos: usize) -> Option<usize> {
    let mut depth = 0i32;
    loop {
        match *code.get(pos)? {
            0x02 | 0x03 | 0x04 => depth += 1,
            0x0B => {
                if depth == 0 {
                    return Some(pos + 1);
                }
                depth -= 1;
            }
            _ => {}
        }
        pos = skip_instr(code, pos)?;
    }
}

/// Scan an `if` then-body to its closing `else` or `end`. Returns `(found_else, pos)`
/// where `pos` is just past the delimiter — used to skip the untaken then-branch.
fn skip_then(code: &[u8], mut pos: usize) -> Option<(bool, usize)> {
    let mut depth = 0i32;
    loop {
        match *code.get(pos)? {
            0x02 | 0x03 | 0x04 => depth += 1,
            0x05 if depth == 0 => return Some((true, pos + 1)),
            0x0B => {
                if depth == 0 {
                    return Some((false, pos + 1));
                }
                depth -= 1;
            }
            _ => {}
        }
        pos = skip_instr(code, pos)?;
    }
}

/// A bounded Wasm execution context: linear memory + a fuel budget. Both are the
/// sandbox's guardrails — memory growth is capped at [`MAX_PAGES_CAP`] and every
/// instruction costs fuel, so untrusted code can neither balloon RAM nor spin
/// forever. The interpreter is fail-safe: any trap (OOB access, div-by-zero, stack
/// underflow, unknown opcode, fuel exhaustion) returns `None`, never panics.
pub struct Vm {
    /// Linear memory (page-granular, zero-initialized, bounds-checked on every access).
    pub mem: Vec<u8>,
    max_pages: u32,
    fuel: u64,
    call_depth: u32,
}

impl Vm {
    /// A VM with `initial_pages` of zeroed linear memory, growable to `max_pages`
    /// (clamped to [`MAX_PAGES_CAP`]), and `fuel` instructions of budget.
    pub fn new(initial_pages: u32, max_pages: u32, fuel: u64) -> Vm {
        let init = initial_pages.min(MAX_PAGES_CAP);
        let max = max_pages.min(MAX_PAGES_CAP).max(init);
        Vm {
            mem: alloc::vec![0u8; init as usize * PAGE_SIZE],
            max_pages: max,
            fuel,
            call_depth: MAX_CALL_DEPTH,
        }
    }

    fn mem_pages(&self) -> u32 {
        (self.mem.len() / PAGE_SIZE) as u32
    }

    /// Grow linear memory by `delta` pages. Returns the previous page count, or -1 if
    /// it would exceed the max (Core Spec §4.4.7 — the trap-free failure value).
    fn mem_grow(&mut self, delta: u32) -> i32 {
        let old = self.mem_pages();
        let new = match old.checked_add(delta) {
            Some(n) if n <= self.max_pages => n,
            _ => return -1,
        };
        self.mem.resize(new as usize * PAGE_SIZE, 0);
        old as i32
    }

    fn load32(&self, ea: usize) -> Option<i32> {
        let b = self.mem.get(ea..ea.checked_add(4)?)?;
        Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn store32(&mut self, ea: usize, v: i32) -> Option<()> {
        let b = self.mem.get_mut(ea..ea.checked_add(4)?)?;
        b.copy_from_slice(&v.to_le_bytes());
        Some(())
    }

    /// Run a function body to completion and return its operand stack. The body is
    /// the function's implicit block (a `br`/`return` at its top level exits it). No
    /// host surface: a `call` traps (use [`Instance::call_export`] for module calls).
    pub fn run(&mut self, code: &[u8], locals: &mut Vec<i32>) -> Option<Vec<i32>> {
        let inst = Instance::empty();
        let mut host = DenyHost;
        let mut stack: Vec<i32> = Vec::new();
        match self.run_body(&inst, &mut host, code, 0, false, &mut stack, locals)? {
            Stop::End(_) | Stop::Return => Some(stack),
            _ => None, // a stray `else` or an out-of-range branch is malformed
        }
    }

    /// Resolve and execute a `call` to function `func_index`: a host import (serviced
    /// through `host`, the capability gate) or a defined function (run recursively,
    /// bounded by the call-depth budget). Pops the callee's args off `stack` and pushes
    /// its results. Any trap (denied import, bad arity, depth/fuel exhausted) → `None`.
    fn invoke(
        &mut self,
        inst: &Instance,
        host: &mut dyn HostEnv,
        func_index: u32,
        stack: &mut Vec<i32>,
    ) -> Option<()> {
        let fi = func_index as usize;
        let num_imports = inst.imports.len();
        let type_index = if fi < num_imports {
            inst.imports[fi].type_index
        } else {
            *inst.func_types.get(fi - num_imports)?
        };
        let ftype = inst.types.get(type_index as usize)?;
        let nparams = ftype.params.len();
        let nresults = ftype.results.len();
        if stack.len() < nparams {
            return None;
        }
        let args = stack.split_off(stack.len() - nparams);

        if fi < num_imports {
            // Host import — the embedder enforces the capability for this import.
            let results = host.call_import(fi as u32, &args)?;
            if results.len() != nresults {
                return None;
            }
            stack.extend(results);
            return Some(());
        }

        // Defined function: set up locals (params + zeroed declared locals) and recurse.
        self.call_depth = self.call_depth.checked_sub(1)?; // depth budget → trap if 0
        let (code_ref, mut locals) = {
            let body = inst.codes.get(fi - num_imports)?;
            let mut locals = args;
            for (count, _vt) in &body.locals {
                for _ in 0..*count {
                    locals.push(0);
                }
            }
            (body.code, locals)
        };
        let mut inner: Vec<i32> = Vec::new();
        let stop = self.run_body(inst, host, code_ref, 0, false, &mut inner, &mut locals)?;
        self.call_depth += 1;
        match stop {
            Stop::End(_) | Stop::Return => {}
            _ => return None,
        }
        if inner.len() < nresults {
            return None;
        }
        let rv = inner.split_off(inner.len() - nresults);
        stack.extend(rv);
        Some(())
    }

    /// Execute the instruction stream that is the body of a block starting at `start`.
    /// `is_loop` distinguishes a `loop` (a `br` to it jumps to the start) from a
    /// `block`/`if` (a `br` to it jumps past the end). Recurses for nested blocks and
    /// for `call`. `inst`/`host` carry the module's functions + the host-import gate.
    fn run_body(
        &mut self,
        inst: &Instance,
        host: &mut dyn HostEnv,
        code: &[u8],
        start: usize,
        is_loop: bool,
        stack: &mut Vec<i32>,
        locals: &mut Vec<i32>,
    ) -> Option<Stop> {
        'restart: loop {
            let mut pos = start;
            loop {
                self.fuel = self.fuel.checked_sub(1)?; // fuel exhausted → trap
                let op = *code.get(pos)?;
                pos += 1;
                match op {
                    0x00 => return None,                  // unreachable → trap
                    0x01 => {}                            // nop
                    0x0B => return Some(Stop::End(pos)),  // end
                    0x05 => return Some(Stop::Else(pos)), // else
                    0x0F => return Some(Stop::Return),    // return
                    0x0C | 0x0D => {
                        // br <l> / br_if <l>
                        let l = read_uleb(code, &mut pos, 32)? as u32;
                        let take = if op == 0x0D { stack.pop()? != 0 } else { true };
                        if take {
                            if l == 0 {
                                if is_loop {
                                    continue 'restart; // loop back to the start
                                }
                                return Some(Stop::End(skip_block_body(code, start)?));
                            }
                            return Some(Stop::Branch(l - 1));
                        }
                    }
                    0x02 | 0x03 => {
                        // block / loop
                        let bt = *code.get(pos)?;
                        pos += 1;
                        if bt != 0x40 && ValType::from_byte(bt).is_none() {
                            return None;
                        }
                        match self.run_body(inst, host, code, pos, op == 0x03, stack, locals)? {
                            Stop::End(next) => pos = next,
                            Stop::Else(_) => return None, // `else` outside an `if`
                            Stop::Return => return Some(Stop::Return),
                            Stop::Branch(0) => {
                                if is_loop {
                                    continue 'restart;
                                }
                                return Some(Stop::End(skip_block_body(code, start)?));
                            }
                            Stop::Branch(n) => return Some(Stop::Branch(n - 1)),
                        }
                    }
                    0x04 => {
                        // if <blocktype>
                        let bt = *code.get(pos)?;
                        pos += 1;
                        if bt != 0x40 && ValType::from_byte(bt).is_none() {
                            return None;
                        }
                        let cond = stack.pop()?;
                        let then_start = pos;
                        let after = if cond != 0 {
                            match self
                                .run_body(inst, host, code, then_start, false, stack, locals)?
                            {
                                Stop::End(p) => p,
                                Stop::Else(p) => skip_block_body(code, p)?, // skip else-arm
                                Stop::Return => return Some(Stop::Return),
                                Stop::Branch(0) => {
                                    if is_loop {
                                        continue 'restart;
                                    }
                                    return Some(Stop::End(skip_block_body(code, start)?));
                                }
                                Stop::Branch(n) => return Some(Stop::Branch(n - 1)),
                            }
                        } else {
                            let (found_else, p) = skip_then(code, then_start)?;
                            if found_else {
                                match self.run_body(inst, host, code, p, false, stack, locals)? {
                                    Stop::End(q) => q,
                                    Stop::Else(_) => return None,
                                    Stop::Return => return Some(Stop::Return),
                                    Stop::Branch(0) => {
                                        if is_loop {
                                            continue 'restart;
                                        }
                                        return Some(Stop::End(skip_block_body(code, start)?));
                                    }
                                    Stop::Branch(n) => return Some(Stop::Branch(n - 1)),
                                }
                            } else {
                                p
                            }
                        };
                        pos = after;
                    }
                    0x10 => {
                        // call <funcidx> — dispatch to a defined function or host import
                        let f = read_uleb(code, &mut pos, 32)? as u32;
                        self.invoke(inst, host, f, stack)?;
                    }
                    0x1A => {
                        stack.pop()?; // drop
                    }
                    0x1B => {
                        // select
                        let c = stack.pop()?;
                        let b = stack.pop()?;
                        let a = stack.pop()?;
                        stack.push(if c != 0 { a } else { b });
                    }
                    0x20 => {
                        let i = read_uleb(code, &mut pos, 32)? as usize;
                        stack.push(*locals.get(i)?);
                    }
                    0x21 => {
                        let i = read_uleb(code, &mut pos, 32)? as usize;
                        let v = stack.pop()?;
                        *locals.get_mut(i)? = v;
                    }
                    0x22 => {
                        let i = read_uleb(code, &mut pos, 32)? as usize;
                        let v = *stack.last()?;
                        *locals.get_mut(i)? = v;
                    }
                    0x28 | 0x2D => {
                        // i32.load / i32.load8_u
                        let _align = read_uleb(code, &mut pos, 32)?;
                        let off = read_uleb(code, &mut pos, 32)? as usize;
                        let ea = (stack.pop()? as u32 as usize).checked_add(off)?;
                        stack.push(if op == 0x28 {
                            self.load32(ea)?
                        } else {
                            *self.mem.get(ea)? as i32
                        });
                    }
                    0x36 | 0x3A => {
                        // i32.store / i32.store8
                        let _align = read_uleb(code, &mut pos, 32)?;
                        let off = read_uleb(code, &mut pos, 32)? as usize;
                        let v = stack.pop()?;
                        let ea = (stack.pop()? as u32 as usize).checked_add(off)?;
                        if op == 0x36 {
                            self.store32(ea, v)?;
                        } else {
                            *self.mem.get_mut(ea)? = v as u8;
                        }
                    }
                    0x3F => {
                        if *code.get(pos)? != 0 {
                            return None;
                        }
                        pos += 1;
                        stack.push(self.mem_pages() as i32);
                    }
                    0x40 => {
                        if *code.get(pos)? != 0 {
                            return None;
                        }
                        pos += 1;
                        let delta = stack.pop()? as u32;
                        let r = self.mem_grow(delta);
                        stack.push(r);
                    }
                    0x41 => {
                        stack.push(read_sleb(code, &mut pos, 32)? as i32);
                    }
                    0x45 => {
                        let a = stack.pop()?;
                        stack.push((a == 0) as i32);
                    }
                    0x67..=0x69 => {
                        let a = stack.pop()? as u32;
                        stack.push(match op {
                            0x67 => a.leading_zeros() as i32,
                            0x68 => a.trailing_zeros() as i32,
                            _ => a.count_ones() as i32, // 0x69 popcnt
                        });
                    }
                    0x46..=0x4F => {
                        let b = stack.pop()?;
                        let a = stack.pop()?;
                        let r = match op {
                            0x46 => a == b,
                            0x47 => a != b,
                            0x48 => a < b,
                            0x49 => (a as u32) < (b as u32),
                            0x4A => a > b,
                            0x4B => (a as u32) > (b as u32),
                            0x4C => a <= b,
                            0x4D => (a as u32) <= (b as u32),
                            0x4E => a >= b,
                            _ => (a as u32) >= (b as u32), // 0x4F ge_u
                        };
                        stack.push(r as i32);
                    }
                    0x6A..=0x78 => {
                        let b = stack.pop()?;
                        let a = stack.pop()?;
                        let r = match op {
                            0x6A => a.wrapping_add(b),
                            0x6B => a.wrapping_sub(b),
                            0x6C => a.wrapping_mul(b),
                            0x6D => {
                                if b == 0 || (a == i32::MIN && b == -1) {
                                    return None; // div_s trap
                                }
                                a / b
                            }
                            0x6E => {
                                if b == 0 {
                                    return None; // div_u trap
                                }
                                ((a as u32) / (b as u32)) as i32
                            }
                            0x6F => {
                                if b == 0 {
                                    return None; // rem_s trap
                                }
                                a.wrapping_rem(b)
                            }
                            0x70 => {
                                if b == 0 {
                                    return None; // rem_u trap
                                }
                                ((a as u32) % (b as u32)) as i32
                            }
                            0x71 => a & b,
                            0x72 => a | b,
                            0x73 => a ^ b,
                            0x74 => a.wrapping_shl(b as u32),
                            0x75 => a.wrapping_shr(b as u32),
                            0x76 => (a as u32).wrapping_shr(b as u32) as i32,
                            0x77 => a.rotate_left((b as u32) & 31),
                            _ => a.rotate_right((b as u32) & 31), // 0x78 rotr
                        };
                        stack.push(r);
                    }
                    _ => return None, // unsupported opcode — reject, don't guess
                }
            }
        }
    }
}

/// Execute a function body's instructions on a fresh operand stack with `locals`,
/// using a default-bounded VM (no preallocated linear memory). Supports the i32 core
/// — arithmetic/comparison/bitwise/shift, `local.*`, `drop`/`select`, and structured
/// control flow (`block`/`loop`/`if`/`else`/`br`/`br_if`/`return`). A trap (unknown
/// opcode, stack/local underflow, div-by-zero, fuel exhaustion) returns `None` —
/// untrusted code MUST fail safe, never panic or read OOB.
pub fn execute_i32(code: &[u8], locals: &mut Vec<i32>) -> Option<Vec<i32>> {
    Vm::new(0, MAX_PAGES_CAP, DEFAULT_FUEL).run(code, locals)
}

/// Instantiate a module and run its exported function `name` with i32 `args` under a
/// **deny-all** host (no host imports granted). Returns its result values. For a module
/// that uses host imports, instantiate it and call [`Instance::call_export`] with a
/// real [`HostEnv`] that enforces the capabilities. `None` on any decode/lookup/trap.
pub fn run_export_i32(module_bytes: &[u8], name: &str, args: &[i32]) -> Option<Vec<i32>> {
    instantiate(module_bytes)?.call_export(name, args, &mut DenyHost, DEFAULT_FUEL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn uleb(v: u64, bits: u32) -> Option<u64> {
        let bytes = leb_ubytes(v);
        let mut p = 0;
        let r = read_uleb(&bytes, &mut p, bits);
        if r.is_some() {
            assert_eq!(p, bytes.len(), "consumed all bytes");
        }
        r
    }
    fn leb_ubytes(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut b = (v & 0x7F) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
        out
    }

    #[test]
    fn uleb_canonical_vectors() {
        // The canonical Core-Spec example: 624485 -> E5 8E 26.
        let mut p = 0;
        assert_eq!(read_uleb(&[0xE5, 0x8E, 0x26], &mut p, 32), Some(624485));
        assert_eq!(p, 3);
        assert_eq!(uleb(0, 32), Some(0));
        assert_eq!(uleb(127, 32), Some(127)); // [0x7F]
        assert_eq!(uleb(128, 32), Some(128)); // [0x80,0x01]
        assert_eq!(uleb(u32::MAX as u64, 32), Some(u32::MAX as u64));
        // Overlong (never terminates within 32 bits) is rejected, not looped forever.
        assert_eq!(
            read_uleb(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80], &mut 0, 32),
            None
        );
        // Truncated (continuation bit but no next byte) is rejected.
        assert_eq!(read_uleb(&[0x80], &mut 0, 32), None);
    }

    #[test]
    fn sleb_sign_extends() {
        assert_eq!(read_sleb(&[0x7F], &mut 0, 32), Some(-1));
        assert_eq!(read_sleb(&[0x00], &mut 0, 32), Some(0));
        assert_eq!(read_sleb(&[0x40], &mut 0, 32), Some(-64));
        assert_eq!(read_sleb(&[0xC0, 0x00], &mut 0, 32), Some(64));
    }

    #[test]
    fn parses_empty_module() {
        let m = parse(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).expect("empty module");
        assert_eq!(m.version, 1);
        assert!(m.sections.is_empty());
    }

    #[test]
    fn walks_sections_zero_copy() {
        // header + a TYPE section (id 1, body [0xAA]) + a custom section (id 0, body [0x42,0x43]).
        let mut b = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        b.extend_from_slice(&[section_id::TYPE, 0x01, 0xAA]);
        b.extend_from_slice(&[section_id::CUSTOM, 0x02, 0x42, 0x43]);
        let m = parse(&b).expect("two-section module");
        assert_eq!(m.sections.len(), 2);
        assert_eq!(m.section(section_id::TYPE).unwrap().body, &[0xAA]);
        assert_eq!(m.section(section_id::CUSTOM).unwrap().body, &[0x42, 0x43]);
        assert!(m.section(section_id::CODE).is_none());
    }

    #[test]
    fn rejects_hostile_modules() {
        let good = vec![
            0x00,
            0x61,
            0x73,
            0x6d,
            0x01,
            0x00,
            0x00,
            0x00,
            section_id::TYPE,
            0x01,
            0xAA,
        ];
        assert_eq!(parse(&[]), Err(WasmError::TooShort));
        assert_eq!(
            parse(&[0x00, 0x61, 0x73, 0x6d, 0x01]),
            Err(WasmError::TooShort)
        );
        // Bad magic.
        let mut bad_magic = good.clone();
        bad_magic[1] = 0xFF;
        assert_eq!(parse(&bad_magic), Err(WasmError::BadMagic));
        // Unsupported version.
        let mut bad_ver = good.clone();
        bad_ver[4] = 2;
        assert_eq!(parse(&bad_ver), Err(WasmError::BadVersion));
        // A section claiming more bytes than remain must not read OOB.
        let mut overrun = good.clone();
        overrun[9] = 0x7F; // TYPE section size = 127, only 1 byte present
        assert_eq!(parse(&overrun), Err(WasmError::Truncated));
        // An unknown high section id is rejected (not silently skipped).
        let bad_id = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 99, 0x00];
        assert_eq!(parse(&bad_id), Err(WasmError::BadSectionId));
    }

    #[test]
    fn decodes_type_function_export_code() {
        // TYPE: (func (param i32 i32) (result i32)).
        let types = decode_type_section(&[0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F]).unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].params, vec![ValType::I32, ValType::I32]);
        assert_eq!(types[0].results, vec![ValType::I32]);
        // FUNCTION: one function of type index 0.
        assert_eq!(decode_function_section(&[0x01, 0x00]).unwrap(), vec![0u32]);
        // EXPORT: export "add" (func 0).
        let exports = decode_export_section(&[0x01, 0x03, b'a', b'd', b'd', 0x00, 0x00]).unwrap();
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].name, "add");
        assert_eq!(exports[0].kind, ExportKind::Func);
        assert_eq!(exports[0].index, 0);
        // CODE: one body, 0 locals, instrs = local.get 0 / local.get 1 / i32.add / end.
        let codes =
            decode_code_section(&[0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6A, 0x0B]).unwrap();
        assert_eq!(codes.len(), 1);
        assert!(codes[0].locals.is_empty());
        assert_eq!(codes[0].code, &[0x20, 0x00, 0x20, 0x01, 0x6A, 0x0B]);
    }

    #[test]
    fn section_decoders_reject_hostile_input() {
        assert_eq!(decode_type_section(&[0x01]), None); // count=1, no body
        assert_eq!(decode_type_section(&[0x01, 0x60, 0x01, 0xFF, 0x00]), None); // bad valtype
        assert_eq!(decode_export_section(&[0x01, 0x01, 0xFF, 0x00, 0x00]), None); // name not utf8
        assert_eq!(decode_code_section(&[0x01, 0x7F, 0x00]), None); // entry size overruns
    }

    /// The canonical `add` module: (module
    ///   (type (func (param i32 i32) (result i32)))
    ///   (func (type 0) local.get 0 local.get 1 i32.add)
    ///   (export "add" (func 0))).
    fn add_module() -> Vec<u8> {
        let mut m = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        for (id, body) in [
            (0x01u8, &[0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F][..]), // TYPE
            (0x03, &[0x01, 0x00][..]),                                 // FUNCTION
            (0x07, &[0x01, 0x03, b'a', b'd', b'd', 0x00, 0x00][..]),   // EXPORT "add"
            (
                0x0A,
                &[0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6A, 0x0B][..],
            ), // CODE
        ] {
            m.push(id);
            m.push(body.len() as u8); // all bodies < 128, so the uleb size is one byte
            m.extend_from_slice(body);
        }
        m
    }

    #[test]
    fn executes_straight_line_i32() {
        // i32.const 7, i32.const 5, i32.sub, end -> [2].
        assert_eq!(
            execute_i32(&[0x41, 0x07, 0x41, 0x05, 0x6B, 0x0B], &mut vec![]),
            Some(vec![2])
        );
        // local.get 0, i32.const 3, i32.mul, end with locals=[4] -> [12].
        assert_eq!(
            execute_i32(&[0x20, 0x00, 0x41, 0x03, 0x6C, 0x0B], &mut vec![4]),
            Some(vec![12])
        );
        // Fail-safe: stack underflow, unknown opcode, local out of range -> None.
        assert_eq!(execute_i32(&[0x6A, 0x0B], &mut vec![]), None);
        assert_eq!(execute_i32(&[0xFE, 0x0B], &mut vec![]), None);
        assert_eq!(execute_i32(&[0x20, 0x05, 0x0B], &mut vec![]), None);
    }

    #[test]
    fn runs_exported_add_function() {
        let m = add_module();
        assert_eq!(run_export_i32(&m, "add", &[2, 3]), Some(vec![5]));
        assert_eq!(run_export_i32(&m, "add", &[10, -4]), Some(vec![6]));
        // Wrong arg count / unknown export -> None (no panic on bad calls).
        assert_eq!(run_export_i32(&m, "add", &[1]), None);
        assert_eq!(run_export_i32(&m, "nope", &[1, 2]), None);
    }

    #[test]
    fn comparison_division_and_shift() {
        // 10 /u 3 -> 3.
        assert_eq!(
            execute_i32(&[0x41, 0x0A, 0x41, 0x03, 0x6E, 0x0B], &mut vec![]),
            Some(vec![3])
        );
        // div by zero traps -> None (untrusted code must not panic).
        assert_eq!(
            execute_i32(&[0x41, 0x05, 0x41, 0x00, 0x6D, 0x0B], &mut vec![]),
            None
        );
        // 7 == 7 -> 1 ; eqz 5 -> 0.
        assert_eq!(
            execute_i32(&[0x41, 0x07, 0x41, 0x07, 0x46, 0x0B], &mut vec![]),
            Some(vec![1])
        );
        assert_eq!(
            execute_i32(&[0x41, 0x05, 0x45, 0x0B], &mut vec![]),
            Some(vec![0])
        );
        // 1 << 4 -> 16 ; popcnt(7) -> 3.
        assert_eq!(
            execute_i32(&[0x41, 0x01, 0x41, 0x04, 0x74, 0x0B], &mut vec![]),
            Some(vec![16])
        );
        assert_eq!(
            execute_i32(&[0x41, 0x07, 0x69, 0x0B], &mut vec![]),
            Some(vec![3])
        );
        // select: c=0 picks the second operand.
        assert_eq!(
            execute_i32(
                &[0x41, 0x0B, 0x41, 0x16, 0x41, 0x00, 0x1B, 0x0B],
                &mut vec![]
            ),
            Some(vec![22])
        );
    }

    #[test]
    fn runs_if_else_branches() {
        // (func (param i32) (result i32)
        //   local.get 0  (if (result i32) (i32.const 10) (else (i32.const 20))))
        let code = [
            0x20, 0x00, // local.get 0
            0x04, 0x7F, // if (result i32)
            0x41, 0x0A, // i32.const 10
            0x05, // else
            0x41, 0x14, // i32.const 20
            0x0B, // end if
            0x0B, // end func
        ];
        let mut m = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        for (id, body) in [
            (0x01u8, vec![0x01, 0x60, 0x01, 0x7F, 0x01, 0x7F]), // TYPE (i32)->i32
            (0x03, vec![0x01, 0x00]),                           // FUNCTION
            (0x07, vec![0x01, 0x04, b'p', b'i', b'c', b'k', 0x00, 0x00]), // EXPORT "pick"
            (0x0A, {
                let mut c = vec![0x01, (1 + code.len()) as u8, 0x00];
                c.extend_from_slice(&code);
                c
            }), // CODE
        ] {
            m.push(id);
            m.push(body.len() as u8);
            m.extend_from_slice(&body);
        }
        assert_eq!(run_export_i32(&m, "pick", &[1]), Some(vec![10]));
        assert_eq!(run_export_i32(&m, "pick", &[0]), Some(vec![20]));
    }

    #[test]
    fn runs_loop_with_br_if() {
        // block { loop { if n==0 br 1; acc += n; n -= 1; br 0 } } — sums 1..=n into local 1.
        let code = [
            0x02, 0x40, // block
            0x03, 0x40, // loop
            0x20, 0x00, 0x45, 0x0D, 0x01, // local.get 0; i32.eqz; br_if 1 (exit block)
            0x20, 0x01, 0x20, 0x00, 0x6A, 0x21, 0x01, // acc = acc + n
            0x20, 0x00, 0x41, 0x01, 0x6B, 0x21, 0x00, // n = n - 1
            0x0C, 0x00, // br 0 (loop)
            0x0B, // end loop
            0x0B, // end block
            0x0B, // end func body
        ];
        let mut locals = vec![5, 0];
        assert!(execute_i32(&code, &mut locals).is_some());
        assert_eq!(locals[1], 15); // 5+4+3+2+1
                                   // A guard against runaway loops: zero never decrements past, so n=0 -> acc stays 0.
        let mut z = vec![0, 7];
        assert!(execute_i32(&code, &mut z).is_some());
        assert_eq!(z[1], 7);
    }

    #[test]
    fn linear_memory_store_load_grow() {
        // store 51966 at addr 16, load it back.
        let mut vm = Vm::new(1, 2, 10_000);
        let code = [
            0x41, 0x10, // i32.const 16 (addr)
            0x41, 0xFE, 0x95, 0x03, // i32.const 51966
            0x36, 0x02, 0x00, // i32.store align=2 off=0
            0x41, 0x10, // i32.const 16
            0x28, 0x02, 0x00, // i32.load align=2 off=0
            0x0B,
        ];
        assert_eq!(vm.run(&code, &mut vec![]), Some(vec![51966]));
        // memory.grow by 1 returns the old size (1); memory.size is then 2.
        let mut vm2 = Vm::new(1, 4, 10_000);
        let grow = [0x41, 0x01, 0x40, 0x00, 0x3F, 0x00, 0x0B];
        assert_eq!(vm2.run(&grow, &mut vec![]), Some(vec![1, 2]));
        // OOB store on a zero-page memory traps (the sandbox property), no panic.
        let mut empty = Vm::new(0, 1, 10_000);
        let oob = [0x41, 0x00, 0x41, 0x00, 0x36, 0x02, 0x00, 0x0B];
        assert_eq!(empty.run(&oob, &mut vec![]), None);
    }

    #[test]
    fn fuel_traps_infinite_loop() {
        // loop { br 0 } — would spin forever; the fuel budget must trap it to None.
        let mut vm = Vm::new(0, 0, 100);
        assert_eq!(vm.run(&[0x03, 0x40, 0x0C, 0x00, 0x0B], &mut vec![]), None);
    }

    /// Assemble a module from its section bodies (header + each `(id, body)`).
    fn module(sections: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut m = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        for (id, body) in sections {
            m.push(*id);
            // section sizes here are all < 128 (single-byte uleb).
            m.push(body.len() as u8);
            m.extend_from_slice(body);
        }
        m
    }

    /// A CODE section body for a list of `(locals_byte_stream, instr_bytes)` functions.
    fn code_section(funcs: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![funcs.len() as u8];
        for f in funcs {
            out.push(f.len() as u8); // entry size (each < 128)
            out.extend_from_slice(f);
        }
        out
    }

    #[test]
    fn calls_a_defined_function() {
        // double(x) = x + x ; quad(x) = double(double(x)). quad calls double twice.
        let m = module(&[
            (0x01, vec![0x01, 0x60, 0x01, 0x7F, 0x01, 0x7F]), // TYPE: (i32)->i32
            (0x03, vec![0x02, 0x00, 0x00]),                   // FUNCTION: f0,f1 of type 0
            (0x07, vec![0x01, 0x04, b'q', b'u', b'a', b'd', 0x00, 0x01]), // EXPORT quad=func1
            (
                0x0A,
                code_section(&[
                    vec![0x00, 0x20, 0x00, 0x20, 0x00, 0x6A, 0x0B], // double: lg0 lg0 add
                    vec![0x00, 0x20, 0x00, 0x10, 0x00, 0x10, 0x00, 0x0B], // quad: lg0 call0 call0
                ]),
            ),
        ]);
        assert_eq!(run_export_i32(&m, "quad", &[5]), Some(vec![20])); // double(double(5))
        assert_eq!(run_export_i32(&m, "quad", &[-3]), Some(vec![-12]));
    }

    /// A host env that adds its two args — but ONLY when the capability is granted.
    struct AddHost {
        granted: bool,
    }
    impl HostEnv for AddHost {
        fn call_import(&mut self, index: u32, args: &[i32]) -> Option<Vec<i32>> {
            if self.granted && index == 0 && args.len() == 2 {
                Some(vec![args[0].wrapping_add(args[1])])
            } else {
                None // capability denied / unknown import → trap the guest
            }
        }
    }

    #[test]
    fn host_import_is_capability_gated() {
        // (import "env" "host_add" (func (i32 i32)->i32))
        // (func use_host (a b) (result i32) (i32.add (call $host_add a b) (i32.const 1)))
        let m = module(&[
            (0x01, vec![0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F]), // TYPE: (i32,i32)->i32
            (
                0x02,
                vec![
                    0x01, // one import
                    0x03, b'e', b'n', b'v', // module "env"
                    0x08, b'h', b'o', b's', b't', b'_', b'a', b'd', b'd', // "host_add"
                    0x00, 0x00, // kind func, type 0
                ],
            ),
            (0x03, vec![0x01, 0x00]), // FUNCTION: one defined func (use_host) of type 0
            (
                0x07,
                vec![
                    0x01, 0x08, b'u', b's', b'e', b'_', b'h', b'o', b's', b't', 0x00, 0x01,
                ],
            ), // EXPORT use_host = func index 1 (0 is the import)
            (
                0x0A,
                code_section(&[vec![
                    0x00, // no locals
                    0x20, 0x00, 0x20, 0x01, // local.get 0, local.get 1
                    0x10, 0x00, // call import 0 (host_add)
                    0x41, 0x01, 0x6A, // i32.const 1, i32.add
                    0x0B,
                ]]),
            ),
        ]);
        let inst = instantiate(&m).expect("instantiate");
        assert_eq!(inst.imports().len(), 1);
        assert_eq!(inst.imports()[0].name, "host_add");
        // Capability GRANTED: host_add(3,4)=7, +1 -> 8.
        let mut grant = AddHost { granted: true };
        assert_eq!(
            inst.call_export("use_host", &[3, 4], &mut grant, DEFAULT_FUEL),
            Some(vec![8])
        );
        // Capability DENIED: the host refuses the import, so the call traps -> None.
        let mut deny = AddHost { granted: false };
        assert_eq!(
            inst.call_export("use_host", &[3, 4], &mut deny, DEFAULT_FUEL),
            None
        );
    }

    #[test]
    fn unbounded_recursion_traps() {
        // (func rec () (call $rec)) — infinite recursion must hit the call-depth budget
        // and trap, not overflow the host stack.
        let m = module(&[
            (0x01, vec![0x01, 0x60, 0x00, 0x00]), // TYPE: ()->()
            (0x03, vec![0x01, 0x00]),             // FUNCTION
            (0x07, vec![0x01, 0x03, b'r', b'e', b'c', 0x00, 0x00]), // EXPORT rec=func0
            (0x0A, code_section(&[vec![0x00, 0x10, 0x00, 0x0B]])), // rec: call 0
        ]);
        assert_eq!(run_export_i32(&m, "rec", &[]), None);
    }
}
