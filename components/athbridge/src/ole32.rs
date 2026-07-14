//! OLE32 — COM/OLE runtime, Structured Storage, Monikers, winmm multimedia,
//! and version info APIs for the AthBridge compatibility layer.
#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ===========================================================================
// GUID / IID / CLSID — 128-bit identifiers
// ===========================================================================

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const ZERO: Self = Self {
        data1: 0,
        data2: 0,
        data3: 0,
        data4: [0; 8],
    };

    pub const fn new(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Self {
        Self {
            data1: d1,
            data2: d2,
            data3: d3,
            data4: d4,
        }
    }

    pub fn from_bytes(bytes: &[u8; 16]) -> Self {
        Self {
            data1: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            data2: u16::from_le_bytes([bytes[4], bytes[5]]),
            data3: u16::from_le_bytes([bytes[6], bytes[7]]),
            data4: [
                bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14],
                bytes[15],
            ],
        }
    }

    pub fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&self.data1.to_le_bytes());
        b[4..6].copy_from_slice(&self.data2.to_le_bytes());
        b[6..8].copy_from_slice(&self.data3.to_le_bytes());
        b[8..16].copy_from_slice(&self.data4);
        b
    }

    pub fn format(&self) -> String {
        alloc::format!(
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            self.data1,
            self.data2,
            self.data3,
            self.data4[0],
            self.data4[1],
            self.data4[2],
            self.data4[3],
            self.data4[4],
            self.data4[5],
            self.data4[6],
            self.data4[7],
        )
    }
}

impl core::fmt::Debug for Guid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Guid({})", self.format())
    }
}

pub type Iid = Guid;
pub type Clsid = Guid;

pub const IID_IUNKNOWN: Iid = Guid::new(
    0x00000000,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_ICLASS_FACTORY: Iid = Guid::new(
    0x00000001,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_IDISPATCH: Iid = Guid::new(
    0x00020400,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_ISTREAM: Iid = Guid::new(
    0x0000000C,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_ISTORAGE: Iid = Guid::new(
    0x0000000B,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_IPERSIST: Iid = Guid::new(
    0x0000010C,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_IPERSIST_STREAM: Iid = Guid::new(
    0x00000109,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_IPERSIST_FILE: Iid = Guid::new(
    0x0000010B,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_IMONIKER: Iid = Guid::new(
    0x0000000F,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);

// ===========================================================================
// HRESULT codes
// ===========================================================================

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_NOINTERFACE: i32 = 0x80004002_u32 as i32;
pub const E_POINTER: i32 = 0x80004003_u32 as i32;
pub const E_FAIL: i32 = 0x80004005_u32 as i32;
pub const E_OUTOFMEMORY: i32 = 0x8007000E_u32 as i32;
pub const E_INVALIDARG: i32 = 0x80070057_u32 as i32;
pub const E_NOTIMPL: i32 = 0x80004001_u32 as i32;
pub const E_UNEXPECTED: i32 = 0x8000FFFF_u32 as i32;
pub const E_ACCESSDENIED: i32 = 0x80070005_u32 as i32;
pub const CLASS_E_NOAGGREGATION: i32 = 0x80040110_u32 as i32;
pub const CLASS_E_CLASSNOTAVAILABLE: i32 = 0x80040111_u32 as i32;
pub const REGDB_E_CLASSNOTREG: i32 = 0x80040154_u32 as i32;
pub const CO_E_NOTINITIALIZED: i32 = 0x800401F0_u32 as i32;
pub const STG_E_FILENOTFOUND: i32 = 0x80030002_u32 as i32;
pub const STG_E_ACCESSDENIED: i32 = 0x80030005_u32 as i32;
pub const STG_E_INVALIDNAME: i32 = 0x800300FC_u32 as i32;
pub const DISP_E_MEMBERNOTFOUND: i32 = 0x80020003_u32 as i32;
pub const DISP_E_UNKNOWNNAME: i32 = 0x80020006_u32 as i32;

pub fn succeeded(hr: i32) -> bool {
    hr >= 0
}
pub fn failed(hr: i32) -> bool {
    hr < 0
}

// ===========================================================================
// IUnknown — base COM interface
// ===========================================================================

pub trait IUnknown {
    fn query_interface(&self, iid: &Iid) -> Result<usize, i32>;
    fn add_ref(&self) -> u32;
    fn release(&self) -> u32;
}

// ===========================================================================
// COM Class Factory
// ===========================================================================

pub trait IClassFactory: IUnknown {
    fn create_instance(&self, outer: Option<&dyn IUnknown>, iid: &Iid) -> Result<usize, i32>;
    fn lock_server(&self, lock: bool) -> i32;
}

// ===========================================================================
// COM Initialization and Apartment Model
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApartmentType {
    SingleThreaded,  // STA
    MultiThreaded,   // MTA
    NeutralThreaded, // NTA (COM+)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoInitFlag {
    ApartmentThreaded = 0x2,
    MultiThreaded = 0x0,
    DisableOle1Dde = 0x4,
    SpeedOverMemory = 0x8,
}

struct ComThreadState {
    initialized: bool,
    apartment: ApartmentType,
    init_count: u32,
}

pub struct ComRuntime {
    initialized: AtomicBool,
    global_lock_count: AtomicU32,
    class_registry: BTreeMap<Clsid, ClassRegistration>,
    interface_registry: BTreeMap<Iid, String>,
    progid_map: BTreeMap<String, Clsid>,
    thread_states: BTreeMap<u64, ComThreadState>,
    objects_alive: AtomicU32,
    next_cookie: AtomicU32,
}

#[derive(Debug, Clone)]
pub struct ClassRegistration {
    pub clsid: Clsid,
    pub progid: Option<String>,
    pub description: String,
    pub threading_model: ApartmentType,
    pub in_proc: bool,
}

impl ComRuntime {
    pub fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            global_lock_count: AtomicU32::new(0),
            class_registry: BTreeMap::new(),
            interface_registry: BTreeMap::new(),
            progid_map: BTreeMap::new(),
            thread_states: BTreeMap::new(),
            objects_alive: AtomicU32::new(0),
            next_cookie: AtomicU32::new(1),
        }
    }

    pub fn co_initialize(&mut self, thread_id: u64) -> i32 {
        self.co_initialize_ex(thread_id, ApartmentType::SingleThreaded)
    }

    pub fn co_initialize_ex(&mut self, thread_id: u64, apartment: ApartmentType) -> i32 {
        if let Some(state) = self.thread_states.get_mut(&thread_id) {
            if state.apartment != apartment {
                return E_INVALIDARG;
            }
            state.init_count += 1;
            return S_FALSE;
        }

        self.thread_states.insert(
            thread_id,
            ComThreadState {
                initialized: true,
                apartment,
                init_count: 1,
            },
        );
        self.initialized.store(true, Ordering::SeqCst);
        S_OK
    }

    pub fn co_uninitialize(&mut self, thread_id: u64) -> i32 {
        if let Some(state) = self.thread_states.get_mut(&thread_id) {
            state.init_count -= 1;
            if state.init_count == 0 {
                state.initialized = false;
                self.thread_states.remove(&thread_id);
            }
            S_OK
        } else {
            CO_E_NOTINITIALIZED
        }
    }

    pub fn is_initialized(&self, thread_id: u64) -> bool {
        self.thread_states
            .get(&thread_id)
            .map_or(false, |s| s.initialized)
    }

    pub fn register_class(&mut self, reg: ClassRegistration) {
        if let Some(ref progid) = reg.progid {
            self.progid_map.insert(progid.clone(), reg.clsid);
        }
        self.class_registry.insert(reg.clsid, reg);
    }

    pub fn co_get_class_object(&self, clsid: &Clsid) -> Result<&ClassRegistration, i32> {
        self.class_registry.get(clsid).ok_or(REGDB_E_CLASSNOTREG)
    }

    pub fn co_create_instance(&self, clsid: &Clsid) -> Result<&ClassRegistration, i32> {
        if !self.initialized.load(Ordering::Relaxed) {
            return Err(CO_E_NOTINITIALIZED);
        }
        self.class_registry.get(clsid).ok_or(REGDB_E_CLASSNOTREG)
    }

    pub fn register_interface(&mut self, iid: Iid, name: String) {
        self.interface_registry.insert(iid, name);
    }

    pub fn clsid_from_progid(&self, progid: &str) -> Option<&Clsid> {
        self.progid_map.get(progid)
    }

    pub fn dll_can_unload_now(&self) -> bool {
        self.objects_alive.load(Ordering::Relaxed) == 0
            && self.global_lock_count.load(Ordering::Relaxed) == 0
    }

    pub fn lock_server(&self, lock: bool) {
        if lock {
            self.global_lock_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.global_lock_count.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

// ===========================================================================
// IDispatch — OLE Automation
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchFlags {
    Method = 0x1,
    PropertyGet = 0x2,
    PropertyPut = 0x4,
    PropertyPutRef = 0x8,
}

pub struct DispatchMethod {
    pub dispid: i32,
    pub name: String,
    pub flags: u16,
    pub param_count: u32,
}

pub struct DispatchTable {
    methods: BTreeMap<i32, DispatchMethod>,
    name_to_id: BTreeMap<String, i32>,
    next_dispid: i32,
}

impl DispatchTable {
    pub fn new() -> Self {
        Self {
            methods: BTreeMap::new(),
            name_to_id: BTreeMap::new(),
            next_dispid: 1,
        }
    }

    pub fn add_method(&mut self, name: String, flags: u16, param_count: u32) -> i32 {
        let dispid = self.next_dispid;
        self.next_dispid += 1;
        self.name_to_id.insert(name.clone(), dispid);
        self.methods.insert(
            dispid,
            DispatchMethod {
                dispid,
                name,
                flags,
                param_count,
            },
        );
        dispid
    }

    pub fn get_ids_of_names(&self, names: &[&str]) -> Vec<Result<i32, i32>> {
        names
            .iter()
            .map(|n| self.name_to_id.get(*n).copied().ok_or(DISP_E_UNKNOWNNAME))
            .collect()
    }

    pub fn get_method(&self, dispid: i32) -> Option<&DispatchMethod> {
        self.methods.get(&dispid)
    }

    pub fn invoke(&self, dispid: i32, _flags: u16, _args: &[Variant]) -> Result<Variant, i32> {
        if self.methods.contains_key(&dispid) {
            Ok(Variant::Empty)
        } else {
            Err(DISP_E_MEMBERNOTFOUND)
        }
    }
}

// ===========================================================================
// VARIANT — type-safe variant for OLE Automation
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum VarType {
    VtEmpty = 0,
    VtNull = 1,
    VtI2 = 2,
    VtI4 = 3,
    VtR4 = 4,
    VtR8 = 5,
    VtCy = 6,
    VtDate = 7,
    VtBstr = 8,
    VtDispatch = 9,
    VtError = 10,
    VtBool = 11,
    VtVariant = 12,
    VtUnknown = 13,
    VtDecimal = 14,
    VtI1 = 16,
    VtUi1 = 17,
    VtUi2 = 18,
    VtUi4 = 19,
    VtI8 = 20,
    VtUi8 = 21,
    VtInt = 22,
    VtUint = 23,
    VtHresult = 25,
    VtArray = 0x2000,
}

#[derive(Debug, Clone)]
pub enum Variant {
    Empty,
    Null,
    I2(i16),
    I4(i32),
    R4(f32),
    R8(f64),
    Cy(i64),
    Date(f64),
    Bstr(Bstr),
    Dispatch(u64),
    Error(i32),
    Bool(bool),
    Unknown(u64),
    Decimal {
        lo: u64,
        mid: u32,
        hi: u32,
        sign: u8,
        scale: u8,
    },
    I1(i8),
    Ui1(u8),
    Ui2(u16),
    Ui4(u32),
    I8(i64),
    Ui8(u64),
    Int(i32),
    Uint(u32),
    Hresult(i32),
    Array(SafeArray),
}

impl Variant {
    pub fn vt(&self) -> VarType {
        match self {
            Self::Empty => VarType::VtEmpty,
            Self::Null => VarType::VtNull,
            Self::I2(_) => VarType::VtI2,
            Self::I4(_) => VarType::VtI4,
            Self::R4(_) => VarType::VtR4,
            Self::R8(_) => VarType::VtR8,
            Self::Cy(_) => VarType::VtCy,
            Self::Date(_) => VarType::VtDate,
            Self::Bstr(_) => VarType::VtBstr,
            Self::Dispatch(_) => VarType::VtDispatch,
            Self::Error(_) => VarType::VtError,
            Self::Bool(_) => VarType::VtBool,
            Self::Unknown(_) => VarType::VtUnknown,
            Self::Decimal { .. } => VarType::VtDecimal,
            Self::I1(_) => VarType::VtI1,
            Self::Ui1(_) => VarType::VtUi1,
            Self::Ui2(_) => VarType::VtUi2,
            Self::Ui4(_) => VarType::VtUi4,
            Self::I8(_) => VarType::VtI8,
            Self::Ui8(_) => VarType::VtUi8,
            Self::Int(_) => VarType::VtInt,
            Self::Uint(_) => VarType::VtUint,
            Self::Hresult(_) => VarType::VtHresult,
            Self::Array(_) => VarType::VtArray,
        }
    }

    pub fn to_i4(&self) -> Option<i32> {
        match self {
            Self::I4(v) => Some(*v),
            Self::I2(v) => Some(*v as i32),
            Self::I1(v) => Some(*v as i32),
            Self::Ui1(v) => Some(*v as i32),
            Self::Ui2(v) => Some(*v as i32),
            Self::Bool(v) => Some(if *v { -1 } else { 0 }),
            _ => None,
        }
    }
}

// ===========================================================================
// BSTR — OLE string type
// ===========================================================================

#[derive(Debug, Clone)]
pub struct Bstr {
    data: Vec<u16>,
}

impl Bstr {
    pub fn sys_alloc_string(s: &str) -> Self {
        let data: Vec<u16> = s.encode_utf16().collect();
        Self { data }
    }

    pub fn sys_alloc_string_len(s: &[u16], len: usize) -> Self {
        let mut data = Vec::with_capacity(len);
        let copy_len = core::cmp::min(s.len(), len);
        data.extend_from_slice(&s[..copy_len]);
        while data.len() < len {
            data.push(0);
        }
        Self { data }
    }

    pub fn sys_string_len(&self) -> usize {
        self.data.len()
    }

    pub fn sys_realloc_string_len(&mut self, new_data: &[u16], new_len: usize) {
        self.data.clear();
        let copy_len = core::cmp::min(new_data.len(), new_len);
        self.data.extend_from_slice(&new_data[..copy_len]);
        while self.data.len() < new_len {
            self.data.push(0);
        }
    }

    pub fn sys_free_string(&mut self) {
        self.data.clear();
    }

    pub fn as_slice(&self) -> &[u16] {
        &self.data
    }

    pub fn to_string_lossy(&self) -> String {
        let mut s = String::new();
        for &code in &self.data {
            if code < 0x80 {
                s.push(code as u8 as char);
            } else {
                s.push(char::REPLACEMENT_CHARACTER);
            }
        }
        s
    }
}

// ===========================================================================
// SAFEARRAY — OLE array type
// ===========================================================================

#[derive(Debug, Clone)]
pub struct SafeArrayBound {
    pub elements: u32,
    pub lower_bound: i32,
}

#[derive(Debug, Clone)]
pub struct SafeArray {
    pub dimensions: u16,
    pub features: u16,
    pub element_size: u32,
    pub bounds: Vec<SafeArrayBound>,
    pub data: Vec<u8>,
    pub locked: bool,
    pub lock_count: u32,
}

impl SafeArray {
    pub fn create(vt: VarType, bounds: &[SafeArrayBound]) -> Self {
        let element_size = match vt {
            VarType::VtI1 | VarType::VtUi1 => 1,
            VarType::VtI2 | VarType::VtUi2 | VarType::VtBool => 2,
            VarType::VtI4
            | VarType::VtUi4
            | VarType::VtR4
            | VarType::VtInt
            | VarType::VtUint
            | VarType::VtError
            | VarType::VtHresult => 4,
            VarType::VtI8 | VarType::VtUi8 | VarType::VtR8 | VarType::VtCy | VarType::VtDate => 8,
            VarType::VtBstr | VarType::VtDispatch | VarType::VtUnknown => 8,
            _ => 16,
        };

        let total_elements: u32 = bounds.iter().map(|b| b.elements).product();
        let data_size = (total_elements * element_size) as usize;

        Self {
            dimensions: bounds.len() as u16,
            features: 0,
            element_size,
            bounds: bounds.to_vec(),
            data: alloc::vec![0u8; data_size],
            locked: false,
            lock_count: 0,
        }
    }

    pub fn destroy(&mut self) {
        self.data.clear();
        self.bounds.clear();
        self.dimensions = 0;
    }

    pub fn access_data(&mut self) -> Result<&mut [u8], i32> {
        self.lock_count += 1;
        self.locked = true;
        Ok(&mut self.data)
    }

    pub fn unaccess_data(&mut self) -> i32 {
        if self.lock_count > 0 {
            self.lock_count -= 1;
            if self.lock_count == 0 {
                self.locked = false;
            }
            S_OK
        } else {
            E_UNEXPECTED
        }
    }

    pub fn get_element(&self, indices: &[i32]) -> Result<&[u8], i32> {
        let offset = self.calc_offset(indices)?;
        let size = self.element_size as usize;
        if offset + size <= self.data.len() {
            Ok(&self.data[offset..offset + size])
        } else {
            Err(E_INVALIDARG)
        }
    }

    pub fn put_element(&mut self, indices: &[i32], data: &[u8]) -> i32 {
        let offset = match self.calc_offset(indices) {
            Ok(o) => o,
            Err(e) => return e,
        };
        let size = self.element_size as usize;
        if offset + size <= self.data.len() && data.len() >= size {
            self.data[offset..offset + size].copy_from_slice(&data[..size]);
            S_OK
        } else {
            E_INVALIDARG
        }
    }

    fn calc_offset(&self, indices: &[i32]) -> Result<usize, i32> {
        if indices.len() != self.dimensions as usize {
            return Err(E_INVALIDARG);
        }
        let mut offset: usize = 0;
        let mut multiplier: usize = 1;
        for i in (0..self.dimensions as usize).rev() {
            let idx = (indices[i] - self.bounds[i].lower_bound) as usize;
            if idx >= self.bounds[i].elements as usize {
                return Err(E_INVALIDARG);
            }
            offset += idx * multiplier;
            multiplier *= self.bounds[i].elements as usize;
        }
        Ok(offset * self.element_size as usize)
    }

    pub fn total_elements(&self) -> u32 {
        self.bounds.iter().map(|b| b.elements).product()
    }
}

// ===========================================================================
// IStream — sequential/random access byte stream
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamSeek {
    Set = 0,
    Cur = 1,
    End = 2,
}

#[derive(Debug, Clone)]
pub struct StreamStat {
    pub name: String,
    pub size: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub atime: u64,
    pub mode: u32,
    pub locks_supported: u32,
    pub clsid: Guid,
}

pub struct ComStream {
    pub name: String,
    pub data: Vec<u8>,
    pub position: u64,
    pub clsid: Guid,
    pub committed: bool,
}

impl ComStream {
    pub fn new(name: String) -> Self {
        Self {
            name,
            data: Vec::new(),
            position: 0,
            clsid: Guid::ZERO,
            committed: true,
        }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, i32> {
        let pos = self.position as usize;
        if pos >= self.data.len() {
            return Ok(0);
        }
        let available = self.data.len() - pos;
        let to_read = core::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&self.data[pos..pos + to_read]);
        self.position += to_read as u64;
        Ok(to_read)
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize, i32> {
        let pos = self.position as usize;
        if pos > self.data.len() {
            self.data.resize(pos, 0);
        }
        let end = pos + buf.len();
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[pos..end].copy_from_slice(buf);
        self.position = end as u64;
        self.committed = false;
        Ok(buf.len())
    }

    pub fn seek(&mut self, offset: i64, origin: StreamSeek) -> Result<u64, i32> {
        let new_pos = match origin {
            StreamSeek::Set => offset as u64,
            StreamSeek::Cur => (self.position as i64 + offset) as u64,
            StreamSeek::End => (self.data.len() as i64 + offset) as u64,
        };
        self.position = new_pos;
        Ok(new_pos)
    }

    pub fn set_size(&mut self, new_size: u64) -> i32 {
        self.data.resize(new_size as usize, 0);
        if self.position > new_size {
            self.position = new_size;
        }
        S_OK
    }

    pub fn copy_to(&self, dest: &mut ComStream, count: u64) -> Result<u64, i32> {
        let pos = self.position as usize;
        let available = self.data.len().saturating_sub(pos);
        let to_copy = core::cmp::min(count as usize, available);
        let src = &self.data[pos..pos + to_copy];
        dest.write(src)?;
        Ok(to_copy as u64)
    }

    pub fn commit(&mut self) -> i32 {
        self.committed = true;
        S_OK
    }

    pub fn revert(&mut self) -> i32 {
        S_OK
    }

    pub fn stat(&self) -> StreamStat {
        StreamStat {
            name: self.name.clone(),
            size: self.data.len() as u64,
            mtime: 0,
            ctime: 0,
            atime: 0,
            mode: 0,
            locks_supported: 0,
            clsid: self.clsid,
        }
    }

    pub fn clone_stream(&self) -> Self {
        Self {
            name: self.name.clone(),
            data: self.data.clone(),
            position: self.position,
            clsid: self.clsid,
            committed: self.committed,
        }
    }
}

// ===========================================================================
// IStorage — structured storage (compound file)
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageElementType {
    Storage = 1,
    Stream = 2,
    LockBytes = 3,
    Property = 4,
}

#[derive(Debug, Clone)]
pub struct StorageElement {
    pub name: String,
    pub element_type: StorageElementType,
    pub size: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub mode: u32,
    pub clsid: Guid,
    pub state_bits: u32,
}

pub struct CompoundStorage {
    pub name: String,
    pub clsid: Guid,
    pub state_bits: u32,
    pub mode: u32,
    pub streams: BTreeMap<String, ComStream>,
    pub substorages: BTreeMap<String, CompoundStorage>,
    pub committed: bool,
}

impl CompoundStorage {
    pub fn new(name: String) -> Self {
        Self {
            name,
            clsid: Guid::ZERO,
            state_bits: 0,
            mode: 0,
            streams: BTreeMap::new(),
            substorages: BTreeMap::new(),
            committed: true,
        }
    }

    pub fn create_stream(&mut self, name: &str) -> Result<&mut ComStream, i32> {
        if self.streams.contains_key(name) {
            return Err(STG_E_ACCESSDENIED);
        }
        self.streams
            .insert(String::from(name), ComStream::new(String::from(name)));
        self.streams.get_mut(name).ok_or(E_FAIL)
    }

    pub fn open_stream(&mut self, name: &str) -> Result<&mut ComStream, i32> {
        self.streams.get_mut(name).ok_or(STG_E_FILENOTFOUND)
    }

    pub fn create_storage(&mut self, name: &str) -> Result<&mut CompoundStorage, i32> {
        if self.substorages.contains_key(name) {
            return Err(STG_E_ACCESSDENIED);
        }
        self.substorages
            .insert(String::from(name), CompoundStorage::new(String::from(name)));
        self.substorages.get_mut(name).ok_or(E_FAIL)
    }

    pub fn open_storage(&mut self, name: &str) -> Result<&mut CompoundStorage, i32> {
        self.substorages.get_mut(name).ok_or(STG_E_FILENOTFOUND)
    }

    pub fn destroy_element(&mut self, name: &str) -> i32 {
        if self.streams.remove(name).is_some() {
            return S_OK;
        }
        if self.substorages.remove(name).is_some() {
            return S_OK;
        }
        STG_E_FILENOTFOUND
    }

    pub fn rename_element(&mut self, old_name: &str, new_name: &str) -> i32 {
        if let Some(stream) = self.streams.remove(old_name) {
            self.streams.insert(String::from(new_name), stream);
            return S_OK;
        }
        if let Some(storage) = self.substorages.remove(old_name) {
            self.substorages.insert(String::from(new_name), storage);
            return S_OK;
        }
        STG_E_FILENOTFOUND
    }

    pub fn enum_elements(&self) -> Vec<StorageElement> {
        let mut elements = Vec::new();
        for (name, stream) in &self.streams {
            elements.push(StorageElement {
                name: name.clone(),
                element_type: StorageElementType::Stream,
                size: stream.data.len() as u64,
                mtime: 0,
                ctime: 0,
                mode: 0,
                clsid: Guid::ZERO,
                state_bits: 0,
            });
        }
        for (name, _) in &self.substorages {
            elements.push(StorageElement {
                name: name.clone(),
                element_type: StorageElementType::Storage,
                size: 0,
                mtime: 0,
                ctime: 0,
                mode: 0,
                clsid: Guid::ZERO,
                state_bits: 0,
            });
        }
        elements
    }

    pub fn commit(&mut self) -> i32 {
        self.committed = true;
        S_OK
    }

    pub fn revert(&mut self) -> i32 {
        S_OK
    }

    pub fn set_class(&mut self, clsid: Guid) -> i32 {
        self.clsid = clsid;
        S_OK
    }

    pub fn set_state_bits(&mut self, bits: u32, mask: u32) -> i32 {
        self.state_bits = (self.state_bits & !mask) | (bits & mask);
        S_OK
    }

    pub fn stat(&self) -> StorageElement {
        StorageElement {
            name: self.name.clone(),
            element_type: StorageElementType::Storage,
            size: 0,
            mtime: 0,
            ctime: 0,
            mode: self.mode,
            clsid: self.clsid,
            state_bits: self.state_bits,
        }
    }
}

// ===========================================================================
// IPersist, IPersistStream, IPersistFile
// ===========================================================================

pub trait IPersist {
    fn get_class_id(&self) -> Guid;
}

pub trait IPersistStream: IPersist {
    fn is_dirty(&self) -> bool;
    fn load(&mut self, stream: &mut ComStream) -> i32;
    fn save(&self, stream: &mut ComStream, clear_dirty: bool) -> i32;
    fn get_size_max(&self) -> u64;
}

pub trait IPersistFile: IPersist {
    fn is_dirty(&self) -> bool;
    fn load(&mut self, filename: &str, mode: u32) -> i32;
    fn save(&self, filename: &str, remember: bool) -> i32;
    fn save_completed(&mut self, filename: &str) -> i32;
    fn get_cur_file(&self) -> Option<String>;
}

// ===========================================================================
// Moniker — IMoniker, binding, display names
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonikerType {
    File,
    Item,
    Composite,
    AntiMoniker,
    Pointer,
    Generic,
}

#[derive(Debug, Clone)]
pub struct Moniker {
    pub moniker_type: MonikerType,
    pub display_name: String,
    pub clsid: Guid,
    pub components: Vec<Moniker>,
}

impl Moniker {
    pub fn file_moniker(path: &str) -> Self {
        Self {
            moniker_type: MonikerType::File,
            display_name: String::from(path),
            clsid: Guid::ZERO,
            components: Vec::new(),
        }
    }

    pub fn item_moniker(delimiter: &str, item: &str) -> Self {
        let display = alloc::format!("{}{}", delimiter, item);
        Self {
            moniker_type: MonikerType::Item,
            display_name: display,
            clsid: Guid::ZERO,
            components: Vec::new(),
        }
    }

    pub fn composite_moniker(left: Moniker, right: Moniker) -> Self {
        let display = alloc::format!("{}{}", left.display_name, right.display_name);
        Self {
            moniker_type: MonikerType::Composite,
            display_name: display,
            clsid: Guid::ZERO,
            components: alloc::vec![left, right],
        }
    }

    pub fn get_display_name(&self) -> &str {
        &self.display_name
    }

    pub fn bind_to_object(&self, _iid: &Iid) -> Result<u64, i32> {
        match self.moniker_type {
            MonikerType::File => Ok(0),
            MonikerType::Item => Ok(0),
            MonikerType::Composite => Ok(0),
            _ => Err(E_NOTIMPL),
        }
    }

    pub fn is_equal(&self, other: &Moniker) -> bool {
        self.moniker_type == other.moniker_type && self.display_name == other.display_name
    }

    pub fn hash_value(&self) -> u32 {
        let mut h: u32 = 0;
        for b in self.display_name.bytes() {
            h = h.wrapping_mul(31).wrapping_add(b as u32);
        }
        h
    }
}

// ===========================================================================
// winmm — Multimedia API equivalents
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundFlag {
    Sync = 0x0000,
    Async = 0x0001,
    NoDefault = 0x0002,
    Memory = 0x0004,
    Loop = 0x0008,
    NoStop = 0x0010,
    Filename = 0x00020000,
    Resource = 0x00040004,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveFormat {
    Pcm8Mono8000,
    Pcm8Stereo8000,
    Pcm16Mono44100,
    Pcm16Stereo44100,
    Pcm16Mono48000,
    Pcm16Stereo48000,
    Pcm24Stereo48000,
    Float32Stereo44100,
    Float32Stereo48000,
}

#[derive(Debug, Clone)]
pub struct WaveFormatEx {
    pub format_tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub cb_size: u16,
}

impl WaveFormatEx {
    pub fn pcm(channels: u16, sample_rate: u32, bits: u16) -> Self {
        let block_align = channels * (bits / 8);
        Self {
            format_tag: 1, // WAVE_FORMAT_PCM
            channels,
            samples_per_sec: sample_rate,
            avg_bytes_per_sec: sample_rate * block_align as u32,
            block_align,
            bits_per_sample: bits,
            cb_size: 0,
        }
    }
}

pub struct WaveOutDevice {
    pub device_id: u32,
    pub format: WaveFormatEx,
    pub opened: bool,
    pub playing: bool,
    pub volume: u32,
    pub buffers_queued: u32,
}

impl WaveOutDevice {
    pub fn open(device_id: u32, format: WaveFormatEx) -> Self {
        Self {
            device_id,
            format,
            opened: true,
            playing: false,
            volume: 0xFFFFFFFF,
            buffers_queued: 0,
        }
    }

    pub fn write(&mut self, _data: &[u8], _size: u32) -> i32 {
        self.buffers_queued += 1;
        self.playing = true;
        S_OK
    }

    pub fn close(&mut self) -> i32 {
        self.opened = false;
        self.playing = false;
        S_OK
    }

    pub fn pause(&mut self) -> i32 {
        self.playing = false;
        S_OK
    }

    pub fn restart(&mut self) -> i32 {
        self.playing = true;
        S_OK
    }

    pub fn reset(&mut self) -> i32 {
        self.buffers_queued = 0;
        self.playing = false;
        S_OK
    }

    pub fn set_volume(&mut self, vol: u32) -> i32 {
        self.volume = vol;
        S_OK
    }
}

pub struct MidiOutDevice {
    pub device_id: u32,
    pub opened: bool,
}

impl MidiOutDevice {
    pub fn open(device_id: u32) -> Self {
        Self {
            device_id,
            opened: true,
        }
    }

    pub fn short_msg(&self, _msg: u32) -> i32 {
        if self.opened {
            S_OK
        } else {
            E_FAIL
        }
    }

    pub fn long_msg(&self, _data: &[u8]) -> i32 {
        if self.opened {
            S_OK
        } else {
            E_FAIL
        }
    }

    pub fn close(&mut self) -> i32 {
        self.opened = false;
        S_OK
    }
}

#[derive(Debug, Clone, Copy)]
pub struct JoyInfo {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub buttons: u32,
}

pub struct MultimediaTimerEvent {
    pub id: u32,
    pub delay_ms: u32,
    pub resolution_ms: u32,
    pub periodic: bool,
    pub active: bool,
}

pub struct WinmmState {
    pub wave_devices: Vec<WaveOutDevice>,
    pub midi_devices: Vec<MidiOutDevice>,
    pub timer_events: Vec<MultimediaTimerEvent>,
    pub joystick_count: u32,
    pub next_timer_id: AtomicU32,
}

impl WinmmState {
    pub fn new() -> Self {
        Self {
            wave_devices: Vec::new(),
            midi_devices: Vec::new(),
            timer_events: Vec::new(),
            joystick_count: 0,
            next_timer_id: AtomicU32::new(1),
        }
    }

    pub fn play_sound(&self, _name: &str, _flags: u32) -> bool {
        true
    }

    pub fn wave_out_open(&mut self, device_id: u32, format: WaveFormatEx) -> Result<usize, i32> {
        let dev = WaveOutDevice::open(device_id, format);
        self.wave_devices.push(dev);
        Ok(self.wave_devices.len() - 1)
    }

    pub fn wave_out_write(&mut self, handle: usize, data: &[u8]) -> i32 {
        if handle < self.wave_devices.len() {
            self.wave_devices[handle].write(data, data.len() as u32)
        } else {
            E_INVALIDARG
        }
    }

    pub fn wave_out_close(&mut self, handle: usize) -> i32 {
        if handle < self.wave_devices.len() {
            self.wave_devices[handle].close()
        } else {
            E_INVALIDARG
        }
    }

    pub fn midi_out_open(&mut self, device_id: u32) -> Result<usize, i32> {
        let dev = MidiOutDevice::open(device_id);
        self.midi_devices.push(dev);
        Ok(self.midi_devices.len() - 1)
    }

    pub fn midi_out_short_msg(&self, handle: usize, msg: u32) -> i32 {
        if handle < self.midi_devices.len() {
            self.midi_devices[handle].short_msg(msg)
        } else {
            E_INVALIDARG
        }
    }

    pub fn midi_out_close(&mut self, handle: usize) -> i32 {
        if handle < self.midi_devices.len() {
            self.midi_devices[handle].close()
        } else {
            E_INVALIDARG
        }
    }

    pub fn time_set_event(&mut self, delay: u32, resolution: u32, periodic: bool) -> u32 {
        let id = self.next_timer_id.fetch_add(1, Ordering::Relaxed);
        self.timer_events.push(MultimediaTimerEvent {
            id,
            delay_ms: delay,
            resolution_ms: resolution,
            periodic,
            active: true,
        });
        id
    }

    pub fn time_kill_event(&mut self, id: u32) -> i32 {
        if let Some(evt) = self.timer_events.iter_mut().find(|e| e.id == id) {
            evt.active = false;
            S_OK
        } else {
            E_INVALIDARG
        }
    }

    pub fn joy_get_num_devs(&self) -> u32 {
        self.joystick_count
    }

    pub fn joy_get_pos(&self, _joy_id: u32) -> Result<JoyInfo, i32> {
        Ok(JoyInfo {
            x: 32768,
            y: 32768,
            z: 0,
            buttons: 0,
        })
    }
}

// ===========================================================================
// Version Info — GetFileVersionInfo equivalents
// ===========================================================================

#[derive(Debug, Clone, Copy)]
pub struct VsFixedFileInfo {
    pub signature: u32,
    pub struct_version: u32,
    pub file_version_ms: u32,
    pub file_version_ls: u32,
    pub product_version_ms: u32,
    pub product_version_ls: u32,
    pub file_flags_mask: u32,
    pub file_flags: u32,
    pub file_os: u32,
    pub file_type: u32,
    pub file_subtype: u32,
    pub file_date_ms: u32,
    pub file_date_ls: u32,
}

impl VsFixedFileInfo {
    pub const SIGNATURE: u32 = 0xFEEF04BD;

    pub fn new(major: u16, minor: u16, patch: u16, build: u16) -> Self {
        Self {
            signature: Self::SIGNATURE,
            struct_version: 0x00010000,
            file_version_ms: ((major as u32) << 16) | minor as u32,
            file_version_ls: ((patch as u32) << 16) | build as u32,
            product_version_ms: ((major as u32) << 16) | minor as u32,
            product_version_ls: ((patch as u32) << 16) | build as u32,
            file_flags_mask: 0x3F,
            file_flags: 0,
            file_os: 0x00040004, // VOS_NT_WINDOWS32
            file_type: 1,        // VFT_APP
            file_subtype: 0,
            file_date_ms: 0,
            file_date_ls: 0,
        }
    }

    pub fn file_version(&self) -> (u16, u16, u16, u16) {
        (
            (self.file_version_ms >> 16) as u16,
            (self.file_version_ms & 0xFFFF) as u16,
            (self.file_version_ls >> 16) as u16,
            (self.file_version_ls & 0xFFFF) as u16,
        )
    }

    pub fn product_version(&self) -> (u16, u16, u16, u16) {
        (
            (self.product_version_ms >> 16) as u16,
            (self.product_version_ms & 0xFFFF) as u16,
            (self.product_version_ls >> 16) as u16,
            (self.product_version_ls & 0xFFFF) as u16,
        )
    }
}

#[derive(Debug, Clone)]
pub struct VersionInfoBlock {
    pub fixed_info: VsFixedFileInfo,
    pub string_table: BTreeMap<String, String>,
    pub translations: Vec<(u16, u16)>,
}

impl VersionInfoBlock {
    pub fn new(major: u16, minor: u16, patch: u16, build: u16) -> Self {
        let mut string_table = BTreeMap::new();
        let ver_string = alloc::format!("{}.{}.{}.{}", major, minor, patch, build);
        string_table.insert(String::from("FileVersion"), ver_string.clone());
        string_table.insert(String::from("ProductVersion"), ver_string);
        string_table.insert(String::from("CompanyName"), String::from("AthenaOS"));
        string_table.insert(String::from("FileDescription"), String::new());
        string_table.insert(String::from("InternalName"), String::new());
        string_table.insert(String::from("LegalCopyright"), String::new());
        string_table.insert(String::from("OriginalFilename"), String::new());
        string_table.insert(String::from("ProductName"), String::new());

        Self {
            fixed_info: VsFixedFileInfo::new(major, minor, patch, build),
            string_table,
            translations: alloc::vec![(0x0409, 0x04B0)], // en-US, Unicode
        }
    }

    pub fn get_file_version_info_size(&self) -> u32 {
        let mut size = core::mem::size_of::<VsFixedFileInfo>() as u32;
        for (k, v) in &self.string_table {
            size += (k.len() + v.len() + 8) as u32;
        }
        size += (self.translations.len() * 4) as u32;
        size + 128
    }

    pub fn ver_query_value(&self, sub_block: &str) -> Option<VersionQueryResult> {
        if sub_block == "\\" || sub_block == "/" {
            return Some(VersionQueryResult::FixedInfo(self.fixed_info));
        }
        if sub_block.contains("\\VarFileInfo\\Translation") {
            return Some(VersionQueryResult::Translations(self.translations.clone()));
        }
        if let Some(key) = sub_block.rsplit('\\').next() {
            if let Some(val) = self.string_table.get(key) {
                return Some(VersionQueryResult::StringValue(val.clone()));
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
pub enum VersionQueryResult {
    FixedInfo(VsFixedFileInfo),
    StringValue(String),
    Translations(Vec<(u16, u16)>),
}

// ===========================================================================
// MCI (Media Control Interface) — simplified command interface
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MciCommand {
    Open = 0x0803,
    Close = 0x0804,
    Play = 0x0806,
    Stop = 0x0808,
    Pause = 0x0809,
    Resume = 0x0855,
    Seek = 0x0807,
    Status = 0x0814,
    Set = 0x080D,
    GetDevCaps = 0x080B,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MciDeviceType {
    CdAudio,
    WaveAudio,
    Sequencer,
    VideoDisc,
    Digital,
    Animation,
}

pub struct MciDevice {
    pub id: u32,
    pub device_type: MciDeviceType,
    pub filename: Option<String>,
    pub playing: bool,
    pub paused: bool,
    pub position_ms: u64,
    pub length_ms: u64,
}

impl MciDevice {
    pub fn new(id: u32, device_type: MciDeviceType) -> Self {
        Self {
            id,
            device_type,
            filename: None,
            playing: false,
            paused: false,
            position_ms: 0,
            length_ms: 0,
        }
    }

    pub fn play(&mut self) -> i32 {
        self.playing = true;
        self.paused = false;
        S_OK
    }

    pub fn stop(&mut self) -> i32 {
        self.playing = false;
        self.paused = false;
        self.position_ms = 0;
        S_OK
    }

    pub fn pause(&mut self) -> i32 {
        if self.playing {
            self.paused = true;
            self.playing = false;
        }
        S_OK
    }

    pub fn resume(&mut self) -> i32 {
        if self.paused {
            self.paused = false;
            self.playing = true;
        }
        S_OK
    }

    pub fn seek(&mut self, position_ms: u64) -> i32 {
        if position_ms <= self.length_ms {
            self.position_ms = position_ms;
            S_OK
        } else {
            E_INVALIDARG
        }
    }
}

// ===========================================================================
// Global OLE32 State
// ===========================================================================

pub struct Ole32State {
    pub com_runtime: ComRuntime,
    pub winmm: WinmmState,
    pub storages: BTreeMap<String, CompoundStorage>,
    pub mci_devices: Vec<MciDevice>,
    pub version_blocks: BTreeMap<String, VersionInfoBlock>,
    pub next_mci_id: AtomicU32,
    pub initialized: bool,
}

impl Ole32State {
    pub fn new() -> Self {
        Self {
            com_runtime: ComRuntime::new(),
            winmm: WinmmState::new(),
            storages: BTreeMap::new(),
            mci_devices: Vec::new(),
            version_blocks: BTreeMap::new(),
            next_mci_id: AtomicU32::new(1),
            initialized: false,
        }
    }

    pub fn stg_create_docfile(&mut self, name: &str) -> Result<&mut CompoundStorage, i32> {
        self.storages
            .insert(String::from(name), CompoundStorage::new(String::from(name)));
        self.storages.get_mut(name).ok_or(E_FAIL)
    }

    pub fn stg_open_storage(&mut self, name: &str) -> Result<&mut CompoundStorage, i32> {
        self.storages.get_mut(name).ok_or(STG_E_FILENOTFOUND)
    }

    pub fn mci_send_command(&mut self, device_id: u32, command: MciCommand) -> i32 {
        if let Some(dev) = self.mci_devices.iter_mut().find(|d| d.id == device_id) {
            match command {
                MciCommand::Play => dev.play(),
                MciCommand::Stop => dev.stop(),
                MciCommand::Pause => dev.pause(),
                MciCommand::Resume => dev.resume(),
                MciCommand::Close => {
                    dev.stop();
                    S_OK
                }
                _ => S_OK,
            }
        } else if command == MciCommand::Open {
            let id = self.next_mci_id.fetch_add(1, Ordering::Relaxed);
            self.mci_devices
                .push(MciDevice::new(id, MciDeviceType::WaveAudio));
            S_OK
        } else {
            E_INVALIDARG
        }
    }

    pub fn register_version_info(&mut self, filename: String, info: VersionInfoBlock) {
        self.version_blocks.insert(filename, info);
    }

    pub fn get_file_version_info_size(&self, filename: &str) -> u32 {
        self.version_blocks
            .get(filename)
            .map(|v| v.get_file_version_info_size())
            .unwrap_or(0)
    }

    pub fn get_file_version_info(&self, filename: &str) -> Option<&VersionInfoBlock> {
        self.version_blocks.get(filename)
    }
}

struct SpinMutex<T> {
    locked: core::sync::atomic::AtomicBool,
    data: core::cell::UnsafeCell<T>,
}
unsafe impl<T: Send> Sync for SpinMutex<T> {}
unsafe impl<T: Send> Send for SpinMutex<T> {}
impl<T> SpinMutex<T> {
    const fn new(data: T) -> Self {
        Self {
            locked: core::sync::atomic::AtomicBool::new(false),
            data: core::cell::UnsafeCell::new(data),
        }
    }
    fn lock(&self) -> SpinMutexGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinMutexGuard { mutex: self }
    }
}
struct SpinMutexGuard<'a, T> {
    mutex: &'a SpinMutex<T>,
}
impl<T> core::ops::Deref for SpinMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}
impl<T> core::ops::DerefMut for SpinMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}
impl<T> Drop for SpinMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}

pub static OLE32: SpinMutex<Option<Ole32State>> = SpinMutex::new(None);

pub fn init() {
    let mut state = Ole32State::new();
    state.initialized = true;
    *OLE32.lock() = Some(state);
}

// ===========================================================================
// CoTaskMemAlloc / CoTaskMemFree — COM task memory allocator
// ===========================================================================

static COTASKMEM_NEXT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0x6000_0000);

pub fn co_task_mem_alloc(size: usize) -> u64 {
    if size == 0 {
        return 0;
    }
    COTASKMEM_NEXT.fetch_add(((size + 15) & !15) as u64, Ordering::Relaxed)
}

pub fn co_task_mem_realloc(ptr: u64, size: usize) -> u64 {
    if ptr == 0 {
        return co_task_mem_alloc(size);
    }
    if size == 0 {
        return 0;
    }
    co_task_mem_alloc(size)
}

pub fn co_task_mem_free(_ptr: u64) {
    // In a real implementation this would return memory to the COM allocator.
}

// ===========================================================================
// OleInitialize / OleUninitialize — OLE compound document support
// ===========================================================================

pub fn ole_initialize(reserved: u64) -> i32 {
    let _ = reserved;
    let mut guard = OLE32.lock();
    match guard.as_mut() {
        Some(state) => {
            state
                .com_runtime
                .co_initialize_ex(0, ApartmentType::SingleThreaded);
            state.initialized = true;
            S_OK
        }
        None => {
            let mut state = Ole32State::new();
            state.initialized = true;
            state
                .com_runtime
                .co_initialize_ex(0, ApartmentType::SingleThreaded);
            *guard = Some(state);
            S_OK
        }
    }
}

pub fn ole_uninitialize() {
    let mut guard = OLE32.lock();
    if let Some(state) = guard.as_mut() {
        state.com_runtime.co_uninitialize(0);
    }
}

// ===========================================================================
// CLSIDFromString / StringFromCLSID / StringFromGUID2
// ===========================================================================

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    Some((hex_nibble(hi)? << 4) | hex_nibble(lo)?)
}

fn parse_hex_u32(s: &[u8]) -> Option<u32> {
    if s.len() != 8 {
        return None;
    }
    let mut val = 0u32;
    for &b in s {
        val = (val << 4) | hex_nibble(b)? as u32;
    }
    Some(val)
}

fn parse_hex_u16(s: &[u8]) -> Option<u16> {
    if s.len() != 4 {
        return None;
    }
    let mut val = 0u16;
    for &b in s {
        val = (val << 4) | hex_nibble(b)? as u16;
    }
    Some(val)
}

/// Parse a CLSID/GUID from string form: `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`
pub fn clsid_from_string(s: &str) -> Result<Guid, i32> {
    let bytes = s.as_bytes();

    let start = if bytes.first() == Some(&b'{') { 1 } else { 0 };
    let end = if bytes.last() == Some(&b'}') {
        bytes.len() - 1
    } else {
        bytes.len()
    };
    let inner = &bytes[start..end];

    // Expected: XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX (36 chars)
    if inner.len() != 36 {
        return Err(E_INVALIDARG);
    }
    if inner[8] != b'-' || inner[13] != b'-' || inner[18] != b'-' || inner[23] != b'-' {
        return Err(E_INVALIDARG);
    }

    let d1 = parse_hex_u32(&inner[0..8]).ok_or(E_INVALIDARG)?;
    let d2 = parse_hex_u16(&inner[9..13]).ok_or(E_INVALIDARG)?;
    let d3 = parse_hex_u16(&inner[14..18]).ok_or(E_INVALIDARG)?;

    let mut d4 = [0u8; 8];
    d4[0] = hex_byte(inner[19], inner[20]).ok_or(E_INVALIDARG)?;
    d4[1] = hex_byte(inner[21], inner[22]).ok_or(E_INVALIDARG)?;
    d4[2] = hex_byte(inner[24], inner[25]).ok_or(E_INVALIDARG)?;
    d4[3] = hex_byte(inner[26], inner[27]).ok_or(E_INVALIDARG)?;
    d4[4] = hex_byte(inner[28], inner[29]).ok_or(E_INVALIDARG)?;
    d4[5] = hex_byte(inner[30], inner[31]).ok_or(E_INVALIDARG)?;
    d4[6] = hex_byte(inner[32], inner[33]).ok_or(E_INVALIDARG)?;
    d4[7] = hex_byte(inner[34], inner[35]).ok_or(E_INVALIDARG)?;

    Ok(Guid::new(d1, d2, d3, d4))
}

pub fn string_from_clsid(clsid: &Guid) -> String {
    clsid.format()
}

pub fn string_from_guid2(guid: &Guid, buf: &mut [u16]) -> i32 {
    let s = guid.format();
    let wide: Vec<u16> = s.encode_utf16().chain(core::iter::once(0)).collect();

    if buf.len() < wide.len() {
        return 0;
    }

    let copy = wide.len().min(buf.len());
    buf[..copy].copy_from_slice(&wide[..copy]);
    wide.len() as i32
}

pub fn clsid_from_prog_id(prog_id: &str) -> Result<Guid, i32> {
    let guard = OLE32.lock();
    match guard.as_ref() {
        Some(state) => state
            .com_runtime
            .clsid_from_progid(prog_id)
            .copied()
            .ok_or(REGDB_E_CLASSNOTREG),
        None => Err(CO_E_NOTINITIALIZED),
    }
}

pub fn prog_id_from_clsid(clsid: &Guid) -> Result<String, i32> {
    let guard = OLE32.lock();
    match guard.as_ref() {
        Some(state) => {
            for (progid, cls) in &state.com_runtime.progid_map {
                if cls == clsid {
                    return Ok(progid.clone());
                }
            }
            Err(REGDB_E_CLASSNOTREG)
        }
        None => Err(CO_E_NOTINITIALIZED),
    }
}

// ===========================================================================
// StubClassFactory — concrete IClassFactory returning E_NOTIMPL for unknown
// ===========================================================================

pub struct StubClassFactory {
    clsid: Guid,
    ref_count: core::sync::atomic::AtomicU32,
}

impl StubClassFactory {
    pub fn new(clsid: Guid) -> Self {
        Self {
            clsid,
            ref_count: core::sync::atomic::AtomicU32::new(1),
        }
    }

    pub fn clsid(&self) -> &Guid {
        &self.clsid
    }
}

impl IUnknown for StubClassFactory {
    fn query_interface(&self, iid: &Iid) -> Result<usize, i32> {
        if *iid == IID_IUNKNOWN || *iid == IID_ICLASS_FACTORY {
            self.add_ref();
            Ok(0)
        } else {
            Err(E_NOINTERFACE)
        }
    }

    fn add_ref(&self) -> u32 {
        self.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn release(&self) -> u32 {
        let prev = self.ref_count.fetch_sub(1, Ordering::Relaxed);
        prev - 1
    }
}

impl IClassFactory for StubClassFactory {
    fn create_instance(&self, _outer: Option<&dyn IUnknown>, _iid: &Iid) -> Result<usize, i32> {
        Err(CLASS_E_CLASSNOTAVAILABLE)
    }

    fn lock_server(&self, lock: bool) -> i32 {
        let guard = OLE32.lock();
        if let Some(state) = guard.as_ref() {
            state.com_runtime.lock_server(lock);
        }
        S_OK
    }
}

// ===========================================================================
// StubUnknown — minimal IUnknown implementation for lightweight COM objects
// ===========================================================================

pub struct StubUnknown {
    ref_count: core::sync::atomic::AtomicU32,
}

impl StubUnknown {
    pub fn new() -> Self {
        Self {
            ref_count: core::sync::atomic::AtomicU32::new(1),
        }
    }

    pub fn ref_count(&self) -> u32 {
        self.ref_count.load(Ordering::Relaxed)
    }
}

impl IUnknown for StubUnknown {
    fn query_interface(&self, iid: &Iid) -> Result<usize, i32> {
        if *iid == IID_IUNKNOWN {
            self.add_ref();
            Ok(0)
        } else {
            Err(E_NOINTERFACE)
        }
    }

    fn add_ref(&self) -> u32 {
        self.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn release(&self) -> u32 {
        let prev = self.ref_count.fetch_sub(1, Ordering::Relaxed);
        prev - 1
    }
}

// ===========================================================================
// COM helper — CoCreateInstance wrapper with graceful failure
// ===========================================================================

pub fn co_create_instance_checked(
    clsid: &Clsid,
    _outer: Option<&dyn IUnknown>,
    _cls_context: u32,
    iid: &Iid,
) -> Result<usize, i32> {
    let guard = OLE32.lock();
    let state = guard.as_ref().ok_or(CO_E_NOTINITIALIZED)?;

    match state.com_runtime.co_create_instance(clsid) {
        Ok(_reg) => {
            if *iid == IID_IUNKNOWN || *iid == IID_ICLASS_FACTORY {
                Ok(0)
            } else {
                Err(E_NOINTERFACE)
            }
        }
        Err(e) => Err(e),
    }
}

// ===========================================================================
// COM context flags
// ===========================================================================

pub const CLSCTX_INPROC_SERVER: u32 = 0x1;
pub const CLSCTX_INPROC_HANDLER: u32 = 0x2;
pub const CLSCTX_LOCAL_SERVER: u32 = 0x4;
pub const CLSCTX_REMOTE_SERVER: u32 = 0x10;
pub const CLSCTX_ALL: u32 = CLSCTX_INPROC_SERVER | CLSCTX_INPROC_HANDLER | CLSCTX_LOCAL_SERVER;

// ===========================================================================
// STGM flags (Structured Storage mode)
// ===========================================================================

pub const STGM_READ: u32 = 0x00000000;
pub const STGM_WRITE: u32 = 0x00000001;
pub const STGM_READWRITE: u32 = 0x00000002;
pub const STGM_SHARE_DENY_NONE: u32 = 0x00000040;
pub const STGM_SHARE_DENY_READ: u32 = 0x00000030;
pub const STGM_SHARE_DENY_WRITE: u32 = 0x00000020;
pub const STGM_SHARE_EXCLUSIVE: u32 = 0x00000010;
pub const STGM_CREATE: u32 = 0x00001000;
pub const STGM_CONVERT: u32 = 0x00020000;
pub const STGM_FAILIFTHERE: u32 = 0x00000000;
pub const STGM_DIRECT: u32 = 0x00000000;
pub const STGM_TRANSACTED: u32 = 0x00010000;

// ===========================================================================
// COM threading model constants
// ===========================================================================

pub const COINIT_APARTMENTTHREADED: u32 = 0x2;
pub const COINIT_MULTITHREADED: u32 = 0x0;
pub const COINIT_DISABLE_OLE1DDE: u32 = 0x4;
pub const COINIT_SPEED_OVER_MEMORY: u32 = 0x8;

// ===========================================================================
// GUID comparison and hashing
// ===========================================================================

pub fn is_equal_guid(g1: &Guid, g2: &Guid) -> bool {
    g1 == g2
}

pub fn is_equal_iid(i1: &Iid, i2: &Iid) -> bool {
    i1 == i2
}

pub fn is_equal_clsid(c1: &Clsid, c2: &Clsid) -> bool {
    c1 == c2
}
