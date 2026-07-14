//! Windows loader data structures: TEB, PEB, process parameters, module list —
//! Concept §Compatibility (MasterChecklist Phase 11: LDR/PEB/TEB).
//!
//! A real MSVC-compiled PE does not call our thunks blind: its CRT entry reads
//! the **Thread Environment Block** through `GS` the instant it runs —
//! `GS:[0x30]` (TEB self), `GS:[0x58]` (TLS array), `GS:[0x60]` (PEB),
//! `GS:[0x68]` (LastError) — and the PEB for the image base, command line,
//! process heap, and OS version. Without these at the right offsets the process
//! faults before `main`. This module lays them out to the exact Windows x64 ABI
//! offsets and builds a linked TEB→PEB→ProcessParameters set.
//!
//! Wiring status: the structures + builder + the GetLastError/TlsGetValue
//! accessors are here and offset-verified. Pointing the guest's `GS` base at the
//! built TEB before its entry point runs is the next slice (needs kernel
//! GS-base support); until then the thunk model's `CompatContext` still serves
//! the simulated path.

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

/// `NT_TIB` + TEB head. Field offsets match the Windows x64 TEB exactly; the
/// reserved arrays pad to the next meaningful field so `GS:[off]` accesses from
/// real code land correctly.
#[repr(C)]
pub struct Teb {
    // ── NT_TIB ──
    pub exception_list: u64,         // 0x000 SEH chain head (Phase 11 SEH)
    pub stack_base: u64,             // 0x008
    pub stack_limit: u64,            // 0x010
    pub sub_system_tib: u64,         // 0x018
    pub fiber_data: u64,             // 0x020
    pub arbitrary_user_pointer: u64, // 0x028
    pub self_ptr: u64,               // 0x030 -> &Teb (GS:[0x30])
    // ── TEB proper ──
    pub environment_pointer: u64,              // 0x038
    pub client_id_process: u64,                // 0x040 PID
    pub client_id_thread: u64,                 // 0x048 TID
    pub active_rpc_handle: u64,                // 0x050
    pub thread_local_storage_pointer: u64,     // 0x058 -> *mut *mut (TLS array)
    pub process_environment_block: u64,        // 0x060 -> &Peb (GS:[0x60])
    pub last_error_value: u32,                 // 0x068
    pub count_of_owned_critical_sections: u32, // 0x06C
    // pad 0x070 .. 0x1480
    _reserved_to_tls_slots: [u8; 0x1480 - 0x070],
    pub tls_slots: [u64; 64], // 0x1480 (TlsGetValue/TlsSetValue)
    // pad to the Win10 x64 TEB size (0x1838)
    _reserved_tail: [u8; 0x1838 - (0x1480 + 64 * 8)],
}

/// `PEB` head. Only the fields a CRT/loader actually reads are named; the rest
/// is padding to keep the named fields at their ABI offsets.
#[repr(C)]
pub struct Peb {
    pub inherited_address_space: u8,      // 0x000
    pub read_image_file_exec_options: u8, // 0x001
    pub being_debugged: u8,               // 0x002
    pub bit_field: u8,                    // 0x003
    _pad0: [u8; 4],                       // 0x004
    pub mutant: u64,                      // 0x008
    pub image_base_address: u64,          // 0x010
    pub ldr: u64,                         // 0x018 -> &PebLdrData
    pub process_parameters: u64,          // 0x020 -> &RtlUserProcessParameters
    pub sub_system_data: u64,             // 0x028
    pub process_heap: u64,                // 0x030
    // pad 0x038 .. 0x118 (OS version block)
    _reserved_to_osver: [u8; 0x118 - 0x038],
    pub os_major_version: u32, // 0x118
    pub os_minor_version: u32, // 0x11C
    pub os_build_number: u16,  // 0x120
    pub os_csd_version: u16,   // 0x122
    pub os_platform_id: u32,   // 0x124
    _reserved_tail: [u8; 0x140 - 0x128],
}

/// Doubly-linked list head/node, as Windows `LIST_ENTRY`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ListEntry {
    pub flink: u64,
    pub blink: u64,
}

/// `PEB_LDR_DATA` — the module lists the loader/GetModuleHandle walk.
#[repr(C)]
pub struct PebLdrData {
    pub length: u32,                                    // 0x000
    pub initialized: u32,                               // 0x004
    pub ss_handle: u64,                                 // 0x008
    pub in_load_order_module_list: ListEntry,           // 0x010
    pub in_memory_order_module_list: ListEntry,         // 0x020
    pub in_initialization_order_module_list: ListEntry, // 0x030
}

/// `UNICODE_STRING` (UTF-16, length in bytes).
#[repr(C)]
pub struct UnicodeString {
    pub length: u16,
    pub maximum_length: u16,
    _pad: u32,
    pub buffer: u64, // -> UTF-16 code units
}

/// Minimal `RTL_USER_PROCESS_PARAMETERS` — enough for ImagePathName +
/// CommandLine (GetCommandLineW) at their ABI offsets.
#[repr(C)]
pub struct RtlUserProcessParameters {
    _reserved_head: [u8; 0x060],
    pub image_path_name: UnicodeString, // 0x060
    pub command_line: UnicodeString,    // 0x070
}

/// The owned, linked process environment. Holding the boxes keeps the pointers
/// the TEB/PEB embed valid for the process lifetime.
pub struct ProcessEnv {
    pub teb: Box<Teb>,
    pub peb: Box<Peb>,
    pub params: Box<RtlUserProcessParameters>,
    pub ldr: Box<PebLdrData>,
    pub cmdline_utf16: Vec<u16>,
}

impl ProcessEnv {
    /// Build a fully-linked TEB/PEB/params set for a process whose image is at
    /// `image_base`, with the given stack range and command line.
    pub fn build(image_base: u64, stack_base: u64, stack_limit: u64, cmdline: &str) -> Box<Self> {
        let mut cmdline_utf16: Vec<u16> = cmdline.encode_utf16().collect();
        cmdline_utf16.push(0); // NUL-terminate (Win32 strings are NUL-terminated)

        let mut ldr = Box::new(PebLdrData {
            length: core::mem::size_of::<PebLdrData>() as u32,
            initialized: 1,
            ss_handle: 0,
            in_load_order_module_list: ListEntry { flink: 0, blink: 0 },
            in_memory_order_module_list: ListEntry { flink: 0, blink: 0 },
            in_initialization_order_module_list: ListEntry { flink: 0, blink: 0 },
        });
        // Empty-but-valid circular lists point at themselves.
        let ldr_addr = &*ldr as *const PebLdrData as u64;
        let self_link = |off: u64| ListEntry {
            flink: ldr_addr + off,
            blink: ldr_addr + off,
        };
        ldr.in_load_order_module_list = self_link(0x010);
        ldr.in_memory_order_module_list = self_link(0x020);
        ldr.in_initialization_order_module_list = self_link(0x030);

        let mut params = Box::new(RtlUserProcessParameters {
            _reserved_head: [0u8; 0x060],
            image_path_name: UnicodeString {
                length: 0,
                maximum_length: 0,
                _pad: 0,
                buffer: 0,
            },
            command_line: UnicodeString {
                length: ((cmdline_utf16.len() - 1) * 2) as u16,
                maximum_length: (cmdline_utf16.len() * 2) as u16,
                _pad: 0,
                buffer: cmdline_utf16.as_ptr() as u64,
            },
        });
        let _ = &mut params;

        let mut peb = Box::new(Peb {
            inherited_address_space: 0,
            read_image_file_exec_options: 0,
            being_debugged: 0,
            bit_field: 0,
            _pad0: [0; 4],
            mutant: 0,
            image_base_address: image_base,
            ldr: &*ldr as *const PebLdrData as u64,
            process_parameters: &*params as *const RtlUserProcessParameters as u64,
            sub_system_data: 0,
            process_heap: 0,
            _reserved_to_osver: [0; 0x118 - 0x038],
            os_major_version: 10, // Windows 10 (matches the registry shim defaults)
            os_minor_version: 0,
            os_build_number: 19045,
            os_csd_version: 0,
            os_platform_id: 2, // VER_PLATFORM_WIN32_NT
            _reserved_tail: [0; 0x140 - 0x128],
        });

        let mut teb = Box::new(Teb {
            exception_list: !0, // 0xFFFF... = end of SEH chain
            stack_base,
            stack_limit,
            sub_system_tib: 0,
            fiber_data: 0,
            arbitrary_user_pointer: 0,
            self_ptr: 0,
            environment_pointer: 0,
            client_id_process: 0,
            client_id_thread: 0,
            active_rpc_handle: 0,
            thread_local_storage_pointer: 0,
            process_environment_block: &*peb as *const Peb as u64,
            last_error_value: 0,
            count_of_owned_critical_sections: 0,
            _reserved_to_tls_slots: [0; 0x1480 - 0x070],
            tls_slots: [0; 64],
            _reserved_tail: [0; 0x1838 - (0x1480 + 64 * 8)],
        });
        teb.self_ptr = &*teb as *const Teb as u64;
        let _ = &mut peb;

        Box::new(ProcessEnv {
            teb,
            peb,
            params,
            ldr,
            cmdline_utf16,
        })
    }

    /// GS base value for this process: the address of its TEB.
    pub fn gs_base(&self) -> u64 {
        &*self.teb as *const Teb as u64
    }

    pub fn set_last_error(&mut self, code: u32) {
        self.teb.last_error_value = code;
    }
    pub fn last_error(&self) -> u32 {
        self.teb.last_error_value
    }

    /// `TlsSetValue` / `TlsGetValue` against the TEB's static slot array.
    pub fn tls_set(&mut self, index: usize, value: u64) -> bool {
        if index < self.teb.tls_slots.len() {
            self.teb.tls_slots[index] = value;
            true
        } else {
            false
        }
    }
    pub fn tls_get(&self, index: usize) -> Option<u64> {
        self.teb.tls_slots.get(index).copied()
    }
}

/// `LDR_DATA_TABLE_ENTRY` — one node of the PEB module lists. The named fields
/// sit at the Windows x64 ABI offsets so an app that walks `PEB->Ldr` directly
/// (packers, some anti-cheat, GetModuleHandle's own walk) reads them correctly.
#[repr(C)]
pub struct LdrDataTableEntry {
    pub in_load_order_links: ListEntry,           // 0x00
    pub in_memory_order_links: ListEntry,         // 0x10
    pub in_initialization_order_links: ListEntry, // 0x20
    pub dll_base: u64,                            // 0x30
    pub entry_point: u64,                         // 0x38
    pub size_of_image: u64,                       // 0x40 (u32 + pad in real layout)
    pub full_dll_name: UnicodeString,             // 0x48
    pub base_dll_name: UnicodeString,             // 0x58
}

/// Registry backing GetModuleHandle / GetProcAddress. Maps a loaded module's
/// name to its base (its HMODULE) — the loader registers kernel32/ntdll/the app
/// itself here. Resolution is ASCII case-insensitive (Win32 module names are).
pub struct ModuleRegistry {
    // (lowercase dll name, base/HMODULE)
    modules: Vec<(alloc::string::String, u64)>,
}

impl ModuleRegistry {
    pub const fn new() -> Self {
        Self {
            modules: Vec::new(),
        }
    }

    /// Register a loaded module. Re-registering a name updates its base.
    pub fn register(&mut self, name: &str, base: u64) {
        let key = name.to_ascii_lowercase();
        if let Some(e) = self.modules.iter_mut().find(|(n, _)| *n == key) {
            e.1 = base;
        } else {
            self.modules.push((key, base));
        }
    }

    /// `GetModuleHandleW(name)` — the module's base, or `None` if not loaded.
    /// Case-insensitive; a missing ".dll" suffix is tolerated (Win32 behavior).
    pub fn get_module_handle(&self, name: &str) -> Option<u64> {
        let q = name.to_ascii_lowercase();
        self.modules
            .iter()
            .find(|(n, _)| *n == q || *n == alloc::format!("{}.dll", q))
            .map(|(_, b)| *b)
    }

    /// Reverse lookup: the dll name for an HMODULE, so `GetProcAddress(hmod, fn)`
    /// can resolve `fn` against the right DLL's shim table.
    pub fn name_for_handle(&self, handle: u64) -> Option<&str> {
        self.modules
            .iter()
            .find(|(_, b)| *b == handle)
            .map(|(n, _)| n.as_str())
    }

    /// `GetProcAddress(hmod, name)` — resolves via the caller-supplied shim
    /// resolver (winapi_shims::resolve_shim), so ldr stays decoupled from the
    /// shim tables. Returns the export's address, or `None`.
    pub fn get_proc_address<F>(&self, handle: u64, func: &str, resolve: F) -> Option<u64>
    where
        F: Fn(&str, &str) -> Option<u64>,
    {
        let dll = self.name_for_handle(handle)?;
        resolve(dll, func)
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    #[test]
    fn teb_field_offsets_match_win_x64_abi() {
        assert_eq!(offset_of!(Teb, exception_list), 0x000);
        assert_eq!(offset_of!(Teb, stack_base), 0x008);
        assert_eq!(offset_of!(Teb, stack_limit), 0x010);
        assert_eq!(offset_of!(Teb, self_ptr), 0x030);
        assert_eq!(offset_of!(Teb, thread_local_storage_pointer), 0x058);
        assert_eq!(offset_of!(Teb, process_environment_block), 0x060);
        assert_eq!(offset_of!(Teb, last_error_value), 0x068);
        assert_eq!(offset_of!(Teb, tls_slots), 0x1480);
    }

    #[test]
    fn peb_field_offsets_match_win_x64_abi() {
        assert_eq!(offset_of!(Peb, image_base_address), 0x010);
        assert_eq!(offset_of!(Peb, ldr), 0x018);
        assert_eq!(offset_of!(Peb, process_parameters), 0x020);
        assert_eq!(offset_of!(Peb, process_heap), 0x030);
        assert_eq!(offset_of!(Peb, os_major_version), 0x118);
        assert_eq!(offset_of!(Peb, os_build_number), 0x120);
    }

    #[test]
    fn params_command_line_at_abi_offset() {
        assert_eq!(offset_of!(RtlUserProcessParameters, image_path_name), 0x060);
        assert_eq!(offset_of!(RtlUserProcessParameters, command_line), 0x070);
    }

    #[test]
    fn build_links_teb_peb_params() {
        let env = ProcessEnv::build(0x1_4000_0000, 0x20_0000, 0x10_0000, "game.exe -foo");
        // TEB self-pointer + PEB link are consistent.
        assert_eq!(env.teb.self_ptr, env.gs_base());
        assert_eq!(
            env.teb.process_environment_block,
            &*env.peb as *const Peb as u64
        );
        assert_eq!(env.peb.image_base_address, 0x1_4000_0000);
        assert_eq!(env.peb.os_major_version, 10);
        // PEB -> ProcessParameters -> CommandLine round-trip.
        assert_eq!(
            env.peb.process_parameters,
            &*env.params as *const RtlUserProcessParameters as u64
        );
        // CommandLine length excludes the NUL (Win32 convention).
        assert_eq!(
            env.params.command_line.length as usize,
            "game.exe -foo".len() * 2
        );
        // SEH chain head is the end-of-list sentinel.
        assert_eq!(env.teb.exception_list, !0);
    }

    #[test]
    fn last_error_and_tls_round_trip() {
        let mut env = ProcessEnv::build(0x1000, 0x2000, 0x1000, "x");
        assert_eq!(env.last_error(), 0);
        env.set_last_error(2); // ERROR_FILE_NOT_FOUND
        assert_eq!(env.last_error(), 2);
        assert!(env.tls_set(5, 0xCAFE));
        assert_eq!(env.tls_get(5), Some(0xCAFE));
        assert!(!env.tls_set(999, 0)); // out of range
    }

    #[test]
    fn ldr_entry_offsets_match_win_x64_abi() {
        assert_eq!(offset_of!(LdrDataTableEntry, in_load_order_links), 0x00);
        assert_eq!(offset_of!(LdrDataTableEntry, in_memory_order_links), 0x10);
        assert_eq!(offset_of!(LdrDataTableEntry, dll_base), 0x30);
        assert_eq!(offset_of!(LdrDataTableEntry, entry_point), 0x38);
        assert_eq!(offset_of!(LdrDataTableEntry, full_dll_name), 0x48);
        assert_eq!(offset_of!(LdrDataTableEntry, base_dll_name), 0x58);
    }

    #[test]
    fn module_registry_get_module_handle_and_proc() {
        let mut reg = ModuleRegistry::new();
        reg.register("kernel32.dll", 0x7FFF_0001_0000);
        reg.register("ntdll.dll", 0x7FFF_0002_0000);
        reg.register("game.exe", 0x1_4000_0000);

        // Case-insensitive, suffix-tolerant (GetModuleHandle behavior).
        assert_eq!(
            reg.get_module_handle("KERNEL32.DLL"),
            Some(0x7FFF_0001_0000)
        );
        assert_eq!(reg.get_module_handle("kernel32"), Some(0x7FFF_0001_0000));
        assert_eq!(reg.get_module_handle("missing.dll"), None);
        // Re-register updates the base.
        reg.register("ntdll.dll", 0x7FFF_0099_0000);
        assert_eq!(reg.get_module_handle("ntdll.dll"), Some(0x7FFF_0099_0000));

        // GetProcAddress: resolve through a stand-in shim resolver that knows
        // kernel32!ExitProcess at a fixed address.
        let resolver = |dll: &str, func: &str| -> Option<u64> {
            if dll.eq_ignore_ascii_case("kernel32.dll") && func == "ExitProcess" {
                Some(0xABCD_1234)
            } else {
                None
            }
        };
        let hk32 = reg.get_module_handle("kernel32.dll").unwrap();
        assert_eq!(
            reg.get_proc_address(hk32, "ExitProcess", resolver),
            Some(0xABCD_1234)
        );
        assert_eq!(reg.get_proc_address(hk32, "NoSuchExport", resolver), None);
        // Unknown handle -> None.
        assert_eq!(reg.get_proc_address(0xDEAD, "ExitProcess", resolver), None);
    }
}
