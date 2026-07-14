//! LinuxKPI Phase 1 smoketest — kmalloc, jiffies, msleep, printk via `ath_linuxkpi`.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

const SYS_PRINT: u64 = 1;
const SYS_EXIT: u64 = 12;

#[inline(always)]
unsafe fn sys_print(value: u64) {
    core::arch::asm!(
        "syscall",
        in("rax") SYS_PRINT,
        in("rdi") value,
        out("rcx") _, out("r11") _,
    );
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") SYS_EXIT,
        in("rdi") code,
        options(noreturn),
    );
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        sys_print(7000);
        let pass = ath_linuxkpi::self_test();
        sys_print(7100 + pass as u64);

        let ptr = ath_linuxkpi::kmalloc(64, 0);
        if ptr.is_null() {
            sys_print(7001);
        } else {
            core::ptr::write_volatile(ptr, 0xAB);
            ath_linuxkpi::kfree(ptr);
            sys_print(7002);
        }

        let j = ath_linuxkpi::get_jiffies_64();
        sys_print(7200 + (j & 0xFF));

        ath_linuxkpi::msleep(3);
        let j2 = ath_linuxkpi::get_jiffies_64();
        sys_print(7300 + ((j2.saturating_sub(j)) & 0xFF));

        let _ = ath_linuxkpi::athena_printk(b"[hello_linuxkpi] athena_printk OK\0".as_ptr());

        // Phase 2: PCI claim + user ioremap + IRQ cap + irq_wait (sentinels 8000–8209).
        sys_print(8000);
        let (p2, dev) = ath_linuxkpi::self_test_phase2_with_handle();
        sys_print(8100 + p2 as u64);
        if p2 >= 4 {
            sys_print(8200);
        }

        sys_print(8300);
        let p3 = ath_linuxkpi::self_test_phase3_on(dev);
        sys_print(8400 + p3 as u64);
        if p3 >= 2 {
            sys_print(8500);
        }

        sys_print(8600);
        let p4 = ath_linuxkpi::self_test_phase4_on(dev);
        sys_print(8600 + p4 as u64);
        if p4 >= 2 {
            sys_print(8700);
        }

        sys_print(9200);
        let intel = ath_linuxkpi::self_test_intel_gpu();
        sys_print(9200 + intel as u64);
        if intel == 0 {
            sys_print(9299);
        } else {
            sys_print(9290);
        }

        // request_firmware end-to-end (syscall 142). Loads the test blob the
        // build packs into the initramfs firmware/ tree and reads its first byte
        // through the kernel mapping. 9301 = loaded+readable, 9399 = absent.
        sys_print(9300);
        let fw = ath_linuxkpi::self_test_firmware("athena-selftest.bin");
        sys_print(9300 + fw as u64);
        if fw == 0 {
            sys_print(9399);
        }

        sys_print(7900);
        sys_exit(0);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        sys_print(7999);
        sys_exit(99);
    }
}
