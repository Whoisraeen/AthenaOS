//! AthenaOS Driver Supervisor (userspace).
//!
//! Spawned by `user_init` to prove the capability-gated driver model:
//!   1. Poll for an `SYS_CAP_GRANT` deposit from the parent (handle 1).
//!   2. Redeem it via `SYS_MMIO_MAP` and touch the mapped page.
//!   3. Call `SYS_DRIVER_REGISTER` (109) with the frozen ABI to enroll a PCI device.
//!
//! Serial sentinels: 10001 start · 55555 cap ready · 10010 MMIO write · 10002 register OK · 10003 fail.

#![no_std]
#![no_main]

use ath_abi::cap;
use ath_abi::syscall as abi;
use core::panic::PanicInfo;

const SYS_CAP_QUERY: u64 = 6;
const SYS_MMIO_MAP: u64 = 7;
const SYS_GETPID: u64 = 29;

const CAP_CHILD_MMIO: u64 = 1;
const MMIO_USER_BASE: u64 = 0x4000_0000;
const MMIO_LEN: u64 = 4096;

const _: () = assert!(abi::SYS_DRIVER_REGISTER == 109);
const _: () = assert!(ath_abi::ABI_VERSION == 4);

#[inline(always)]
unsafe fn sys_print(value: u64) {
    core::arch::asm!(
        "syscall",
        in("rax") abi::SYS_PRINT,
        in("rdi") value,
        out("rcx") _, out("r11") _,
    );
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") abi::SYS_EXIT,
        in("rdi") code,
        options(noreturn),
    );
}

#[inline(always)]
unsafe fn sys_cap_query(handle: u64) -> (u64, u64, u64) {
    let (status, flavor, rights);
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_CAP_QUERY => status,
        in("rdi") handle,
        lateout("rsi") flavor,
        lateout("rdx") rights,
        out("rcx") _, out("r11") _,
    );
    (status, flavor, rights)
}

#[inline(always)]
unsafe fn sys_mmio_map(handle: u64, user_virt: u64, length: u64) -> u64 {
    let result: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_MMIO_MAP => result,
        in("rdi") handle,
        in("rsi") user_virt,
        in("rdx") length,
        out("rcx") _, out("r11") _,
    );
    result
}

#[inline(always)]
unsafe fn sys_getpid() -> u64 {
    let pid: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_GETPID => pid,
        out("rcx") _, out("r11") _,
    );
    pid
}

#[inline(always)]
unsafe fn sys_driver_register(
    vendor_id: u16,
    device_id: u16,
    pci_bus: u8,
    pci_dev: u8,
    pci_func: u8,
    target_thread: u64,
    signature_valid: bool,
) -> u64 {
    let arg1 = ((vendor_id as u64) << 16) | (device_id as u64);
    let arg2 = ((pci_bus as u64) << 16) | ((pci_dev as u64) << 8) | (pci_func as u64);
    let result: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") abi::SYS_DRIVER_REGISTER => result,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") target_thread,
        in("r10") signature_valid as u64,
        out("rcx") _, out("r11") _,
    );
    result
}

#[inline(always)]
fn spin_yield(iter: u32) {
    for _ in 0..iter {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        sys_print(10001);

        // Parent grants handle 1 after spawn; poll until it lands.
        let mut granted = false;
        for _ in 0..500_000 {
            let (status, _flavor, _rights) = sys_cap_query(CAP_CHILD_MMIO);
            if status == 0 {
                granted = true;
                break;
            }
            if status != cap::E_NO_HANDLE {
                break;
            }
            spin_yield(64);
        }

        if !granted {
            sys_print(55555);
            sys_exit(42);
        }

        sys_print(55555);

        let map_rc = sys_mmio_map(CAP_CHILD_MMIO, MMIO_USER_BASE, MMIO_LEN);
        if map_rc != 0 {
            sys_print(10003);
            sys_exit(3);
        }

        core::ptr::write_volatile(MMIO_USER_BASE as *mut u32, 0x4452_4956); // 'DRIV'
        sys_print(10010);

        // QEMU e1000 @ 00:04.0 — same BDF LinuxKPI Phase 2 probes first.
        let self_pid = sys_getpid();
        let reg = sys_driver_register(0x8086, 0x100E, 0, 4, 0, self_pid, true);
        if reg == 0 {
            sys_print(10002);
            sys_exit(0);
        }

        sys_print(10003);
        sys_exit(4);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        sys_print(10999);
        sys_exit(99);
    }
}
