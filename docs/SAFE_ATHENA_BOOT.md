# Safe Athena Boot — test RaeenOS on real hardware WITHOUT touching the disk

The last bare-metal **install** wiped a Windows partition. This procedure boots
RaeenOS in a **guaranteed read-only** mode so it *physically cannot write to any
disk* — you can exercise the kernel, drivers, USB, networking, audio, and the
desktop on real Athena hardware with zero risk to whatever is on the SSD.

## Why it's safe (two independent gates)

Every block-device write in the kernel funnels through one guard
(`block_io::safe_mode_guard_write`, called by the NVMe, AHCI, virtio, **and
USB-MSC** `write_sector` impls). A `--safe` image trips **both** gates:

1. **Writes never enabled** — `set_writes_enabled(true)` is skipped for the whole
   boot, so `writes_enabled()` stays `false`.
2. **Safe-mode flag** — the `safe_mode` build feature sets `SAFE_MODE = true`.

Either gate blocks a write; a `--safe` build has both on, all boot. The only
permitted write is a pre-created `BOOTLOG.TXT` file's *own already-allocated data
clusters* (never FAT, never the root dir, never anything Windows boots from), and
only if you pre-create that file — otherwise **every** sector write is refused.

## 1. Build the safe image

```
cargo run -p xtask --release -- build --release --safe --uefi
```

`--safe` = the read-only kernel; `--uefi` = the image Athena's firmware boots.
Output: `target/x86_64-unknown-none/release/kernel.uefi.img`.

## 2. Flash to a USB stick

Write `kernel.uefi.img` to a USB stick as a **raw image** (Rufus "DD mode",
balenaEtcher, or `dd`). This overwrites the *stick*, not your SSD.

## 3. Boot Athena from the stick

1. Insert the stick, power on, enter firmware setup (**Del** or **F2**).
2. **Disable Secure Boot** (Security → Secure Boot → Disabled; OS Type → Other OS).
3. Select the USB stick as the boot device.

## 4. Confirm read-only on screen / serial

You should see, early in boot:

```
[safe-mode] ENABLED — sector writes will be refused at the BlockDevice trait
[storage] *** SAFE IMAGE: storage is READ-ONLY for this entire boot — ...
          RaeenOS cannot write any real disk ... Safe to run on real hardware. ***
```

and, whenever anything attempts a write:

```
[safe-mode] BLOCKED <device> write lba=... (storage read-only, reject #N)
```

If you see those lines, **your disk is untouched.** If you ever see
`[storage] disk writes ENABLED`, you booted the wrong (non-safe) image — power
off and reflash the `--safe` image.

## 5. Retrieve diagnostics (no serial cable needed)

- On-screen: the serial log mirrors to the GOP framebuffer; the safe-mode diag
  panel docks on the right of the desktop.
- USB-C UART (if cabled): COM @ 0x3F8.
- Pre-created `BOOTLOG.TXT`: extract with `scripts/read-bootlog.ps1` after boot
  (the only thing a safe image may write).

## ⚠️ Do NOT use the standard image to "just test"

A standard build (no `--safe`) **enables writes** so the installer can format and
write the disk — that is what wiped Windows. For *testing*, only ever flash the
`--safe` image. Install deliberately, later, with the standard image once you've
decided to commit the disk.

> Follow-up (stronger default, tracked in `block_io.rs`): gate the standard
> write-enable on the QEMU hardware profile so even a *non-safe* build is
> read-only on real hardware until a user-confirmed installer flips it.
