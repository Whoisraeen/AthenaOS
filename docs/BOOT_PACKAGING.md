# Boot packaging (R01 — Redox extraction, no kernel swap)

AthenaOS keeps **rust-osdev/bootloader 0.11** + `bootloader_api` in the kernel. We adopt Redox **artifact layout ideas**, not the Redox bootloader source.

## What `xtask build` produces

| Artifact | Path | Use |
|----------|------|-----|
| Kernel ELF | `target/x86_64-unknown-none/release/kernel` | Debug / symbols |
| BIOS disk image | `target/x86_64-unknown-none/release/kernel.bios.img` | QEMU `-drive file=…` (default) |
| UEFI disk image | `target/x86_64-unknown-none/release/kernel.uefi.img` | QEMU + OVMF |
| Initramfs | `kernel/src/initramfs.tar` | Embedded in kernel; ELFs from `config/base.toml` |

Default **`cargo run -p xtask -- build --release`** creates **both** `.bios.img` and `.uefi.img` (`--boot=bios|uefi|all`).

## Future install layout (Phase 16)

Redox stages:

```text
/boot/kernel
/boot/initramfs.tar
/EFI/BOOT/BOOTX64.EFI   # on ESP
```

AthenaOS installer will lay down GPT + ESP + root; today smoketest embeds initramfs in the kernel ELF.

## QEMU profiles

| Profile | Command | Disks |
|---------|---------|-------|
| Default | `--disk=virtio` | Boot image + `target/virtio.img` |
| NVMe | `--disk=nvme` | Boot image on NVMe controller |
| Smoketest | `--disk=smoketest` | Boot virtio + `nvme.img` + `ahci.img` markers |

`target/boot.ps1` mirrors smoketest disks for Windows dev loops.

## OVMF

`xtask run --uefi` searches common paths (`OVMF.fd`, QEMU share dirs, Linux `/usr/share/OVMF/…`).

## Ventoy / USB

`deploy-ventoy` copies a **raw disk image** today. For Ventoy, prefer a **UEFI ISO** once `xtask iso` lands (mkisofs-rs per extraction map). Document Secure Boot off for Athena bring-up.

## Redox reference

- Cookbook: `redox_reference/recipes/core/bootloader/recipe.toml` (when cloned)
- Do **not** replace hybrid kernel or scheme IPC
