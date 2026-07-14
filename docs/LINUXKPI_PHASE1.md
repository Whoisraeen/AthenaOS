# LinuxKPI Phase 1 — userspace host foundation

**Status:** QEMU boot proof (`[~]`). Athena hardware proof pending.

## Goal

Provide a **userspace** Linux driver–compatible C ABI (`kmalloc`, `kfree`, `kzalloc`,
`get_jiffies_64`, `msleep`, `athena_printk`, spinlock stubs) backed by a small kernel
**host** (`kernel/src/linuxkpi_host.rs`), without loading GPL `.ko` modules into the
MPL-2.0 kernel. This is Path C scaffolding per `docs/LINUX_DRIVER_STRATEGY.md`.

Phase 2 adds `ioremap`, `SYS_DRIVER_CLAIM` / DMA / IRQ doorbells via capabilities.
Phase 3–4 cover zero-copy DMA and IOMMU sandboxing.

## Layout

| Path | Role |
|------|------|
| `components/ath_linuxkpi/` | `#![no_std]` shim: bump heap (256 KiB), syscall stubs |
| `components/ath_linuxkpi/include/ath_linuxkpi.h` | C header for future driver ports |
| `kernel/src/linuxkpi_host.rs` | Syscalls 127–131 implementation |
| `hello_linuxkpi/` | Boot smoketest ELF (initramfs) |

## Syscalls

Documented in `docs/SYSCALL_TABLE.md` block 23. Magic version: `0x524B5049_0001`.

## Boot proof (QEMU)

After `cargo run -p xtask -- build --release` and `powershell -File target\boot.ps1`:

- `[linuxkpi] host ready: …`
- `[linuxkpi] host smoketest: version=0x524b50490001 jiffies_ok=true …`
- Serial sentinels from `hello_linuxkpi`: `msg: 7000`, `msg: 7103` (self_test pass=3),
  `msg: 7002` (kmalloc), `msg: 7900` (done)
- `[linuxkpi] [hello_linuxkpi] athena_printk OK`
- `/proc/athena/linuxkpi` — ABI + jiffies line in procfs dump

## Verify

```powershell
cargo run -p xtask -- build --release
powershell -File target\boot.ps1
Select-String -Path target\serial-input.log -Pattern "linuxkpi","msg: 7103","msg: 7900","System successfully booted"
Select-String -Path target\serial-input.log -Pattern "PANIC"
```

## Not in Phase 1

- Kernel-side `kmalloc` for arbitrary driver blobs (`linux_compat.rs` remains separate)
- `ioremap` / PCI BAR mapping / IRQ registration
- Mesa or Wi-Fi firmware loads
