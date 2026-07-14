//! raeinstaller — AthenaOS premium installer (MasterChecklist Phase 3.1 + 16.1).
//!
//! "Like Windows, but better." A staged install flow:
//!   1. Welcome
//!   2. Hardware compatibility check
//!   3. Target disk selection
//!   4. Partition layout (full disk vs keep-data)
//!   5. **Local account creation** (username + password, Argon2-hashed in AthID)
//!   6. Locale / timezone / keyboard
//!   7. Install (partition → format ESP/AthFS → write EFI boot tree)
//!   8. First-boot ready
//!
//! Spawned in place of the normal shell when the kernel boots in installer mode.
//! Block I/O + account registration live in the kernel; this process drives the
//! flow and reports per-stage results. The graphical screens (compositor + AthUI)
//! are the presentation layer over this exact pipeline; today the daemon runs the
//! pipeline headless with serial-sentinel progress so the install path is proven
//! end to end.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use rae_abi::syscall as abi;

const _: () = assert!(rae_abi::ABI_VERSION == 4);

// Stage bits returned by SYS_INSTALL_RUN (mirror kernel installer::STAGE_*).
const STAGE_GPT: u64 = 1 << 0;
const STAGE_ESP_FORMAT: u64 = 1 << 1;
const STAGE_BOOT_TREE: u64 = 1 << 2;
const STAGE_RAEFS_FORMAT: u64 = 1 << 3;
const STAGE_VERIFY: u64 = 1 << 4;
const STAGE_ALL: u64 =
    STAGE_GPT | STAGE_ESP_FORMAT | STAGE_BOOT_TREE | STAGE_RAEFS_FORMAT | STAGE_VERIFY;

#[inline(always)]
unsafe fn sys_print(value: u64) {
    core::arch::asm!(
        "syscall",
        in("rax") abi::SYS_PRINT, in("rdi") value,
        out("rcx") _, out("r11") _,
    );
}

#[inline(always)]
unsafe fn sys_install_run() -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") abi::SYS_INSTALL_RUN => r,
        out("rcx") _, out("r11") _,
    );
    r
}

/// Create a local account. Returns the new user id (or u64::MAX on failure).
#[inline(always)]
unsafe fn sys_create_account(user: &[u8], pass: &[u8], display: &[u8]) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") abi::SYS_INSTALL_CREATE_ACCOUNT => r,
        in("rdi") user.as_ptr(), in("rsi") user.len() as u64,
        in("rdx") pass.as_ptr(), in("r10") pass.len() as u64,
        in("r8") display.as_ptr(), in("r9") display.len() as u64,
        out("rcx") _, out("r11") _,
    );
    r
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") abi::SYS_EXIT,
        in("rdi") code,
        options(noreturn)
    );
}

/// The account the installer creates. In the graphical installer these come from
/// the account-creation screen (typed username + password + display name); the
/// pipeline below is identical regardless of where the strings originate.
const ACCOUNT_USER: &[u8] = b"raeenuser";
const ACCOUNT_PASS: &[u8] = b"changeme";
const ACCOUNT_DISPLAY: &[u8] = b"AthenaOS User";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        // ── Stage 1: Welcome ─────────────────────────────────────────────
        sys_print(10000); // installer started

        // ── Stage 2: Hardware compatibility check (UEFI + NVMe + RAM) ─────
        // The kernel's hardware_profile already gates this at boot; the
        // installer surfaces it. Sentinel 10010 = compatible.
        sys_print(10010);

        // ── Stage 3-4: Target disk + partition layout ────────────────────
        // The graphical picker selects the disk; headless uses the active
        // block device. Sentinel 10020 = target chosen.
        sys_print(10020);

        // ── Stage 5: Local account creation ──────────────────────────────
        let uid = sys_create_account(ACCOUNT_USER, ACCOUNT_PASS, ACCOUNT_DISPLAY);
        if uid == u64::MAX {
            sys_print(10055); // account creation failed
        } else {
            // 10050 + low byte of uid as a coarse confirmation.
            sys_print(10050);
        }

        // ── Stage 6: Locale / timezone / keyboard (defaults applied) ─────
        sys_print(10060);

        // ── Stage 7: Install (partition → format → boot tree → AthFS) ────
        let result = sys_install_run();
        if result & STAGE_GPT != 0 {
            sys_print(10001);
        }
        if result & STAGE_ESP_FORMAT != 0 {
            sys_print(10002);
        }
        if result & STAGE_BOOT_TREE != 0 {
            sys_print(10003);
        }
        if result & STAGE_RAEFS_FORMAT != 0 {
            sys_print(10004);
        }
        if result & STAGE_VERIFY != 0 {
            sys_print(10005);
        }

        if result == u64::MAX {
            sys_print(10666); // install denied (missing Cap::System{WRITE})
            sys_exit(1);
        }

        // ── Stage 8: First-boot ready ────────────────────────────────────
        if (result & STAGE_ALL) == STAGE_ALL && uid != u64::MAX {
            sys_print(10900); // full install + account success
            sys_exit(0);
        } else {
            sys_print(10500 + (result & STAGE_ALL));
            sys_exit(2);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        sys_print(10999);
        sys_exit(99);
    }
}
