//! Panic handler — print the panic message to serial, then halt forever.

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::crash_dump::write_crash_dump(info);

    // The boot splash gates the serial→GOP text mirror off for a clean boot
    // face; a panic MUST bring the on-screen log back so a user with no
    // serial cable still sees what died (CLAUDE.md §9 diagnosability).
    crate::console::set_console_mirror(true);

    crate::serial_println!();
    crate::serial_println!("[PANIC] {}", info);

    // PIE load base, computed at runtime — QEMU and real UEFI load us at
    // different bases, so a hardcoded value decodes real-hardware backtraces to
    // garbage. Subtract it from a code address to get the ELF file offset that
    // scripts/resolve-panic.ps1 resolves.
    let kimg_base = crate::kernel_image_base();

    // Frame-pointer backtrace (requires -C force-frame-pointers=yes). Kernel
    // stacks are higher-half; only walk plausible canonical kernel addresses.
    {
        let mut rbp: u64;
        unsafe {
            core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack, preserves_flags));
        }
        crate::serial_println!("[PANIC] backtrace (ret_addr / +file_off):");
        for _ in 0..24 {
            if rbp < 0xffff_8000_0000_0000 || (rbp & 0x7) != 0 {
                break;
            }
            let ret = unsafe { core::ptr::read_volatile((rbp as *const u64).wrapping_add(1)) };
            let next = unsafe { core::ptr::read_volatile(rbp as *const u64) };
            if ret == 0 {
                break;
            }
            crate::serial_println!("[PANIC]   {:#x}  +{:#x}", ret, ret.wrapping_sub(kimg_base));
            if next <= rbp {
                break;
            }
            rbp = next;
        }
    }

    // Stack-scan fallback. The RBP chain breaks as soon as it crosses an
    // alloc/core frame (those are precompiled without frame pointers), so a
    // panic inside std collections — e.g. BTreeMap navigation — yields an empty
    // RBP backtrace. Scanning the stack upward for words that land inside the
    // kernel image recovers the caller return addresses regardless. Noisy, but
    // every printed +file_off is resolvable via scripts/resolve-panic.ps1.
    {
        // .text lives high in the image (file_off ~0x1.6M+); .rodata is
        // everything below it. The panic formatter's own frames sit nearest RSP
        // and are full of rodata pointers (format strings), so a shallow scan
        // returns only noise. So: scan deep — kernel stacks are 64 KiB and
        // heap-allocated with no guard page, so overscan is harmless noise, not
        // a fault — and drop the low rodata bulk by only printing offsets above
        // 16 MiB. Real return addresses are >= ~0x1613000.
        let kimg_base = crate::kernel_image_base();
        const KIMG_SIZE: u64 = 0x0400_0000; // 64 MiB
        const TEXT_FLOOR: u64 = 0x0100_0000; // 16 MiB — below all .text, cuts rodata
        const SCAN_WORDS: u64 = 2048; // 16 KiB of stack (safe vs. a small BSP stack)

        let mut rsp: u64;
        unsafe {
            core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
        }
        crate::serial_println!(
            "[PANIC] stack-scan (+file_off; send the 7-digit ones starting 16/17/18):"
        );
        let mut printed = 0u32;
        let mut last = 0u64;
        let mut p = rsp & !0x7;
        let end = p.saturating_add(SCAN_WORDS * 8);
        while p < end && printed < 64 {
            let v = unsafe { core::ptr::read_volatile(p as *const u64) };
            if v >= kimg_base + TEXT_FLOOR && v < kimg_base + KIMG_SIZE && v != last {
                crate::serial_println!("[PANIC]   +{:#x}", v - kimg_base);
                last = v;
                printed += 1;
            }
            p += 8;
        }
        if printed == 0 {
            crate::serial_println!("[PANIC]   (no kernel return addresses found on stack)");
        }
    }

    crate::serial_println!("[PANIC] system halted.");

    // Flush the bootlog RAM ring to the persisted ESP file BEFORE
    // halting. Best-effort: no-op if bootlog_persist wasn't initialized
    // (early-boot panic before storage came up). The flush itself prints
    // a single status line which is also captured in the ring just
    // before the disk write, so the persisted log includes the flush
    // result. Lets the user pull `B:\BOOTLOG.TXT` after a Windows boot
    // to see exactly where the kernel died — invaluable on bare-metal
    // where serial cables aren't attached.
    crate::bootlog_persist::flush();

    crate::kprintln!();
    crate::kprintln!("[PANIC] {}", info);
    crate::kprintln!("[PANIC] system halted.");

    crate::hlt_loop();
}
