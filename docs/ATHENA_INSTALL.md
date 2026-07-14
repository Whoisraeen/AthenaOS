# Installing RaeenOS on the Beelink "Athena" (bare-metal)

Two images are built and QEMU-verified (2026-06-11):

| File | Purpose |
|---|---|
| `target/install-usb-safe.img` | **SAFE dry-run.** Boots RaeenOS and *attempts* the install but every disk write to the internal NVMe is refused (`--safe` build). Proves the stick boots on Athena and that nothing destructive happens. **Run this FIRST.** |
| `target/install-usb.img` | **REAL install.** Boots RaeenOS and writes a fresh GPT + bootable ESP + RaeFS root onto Athena's internal NVMe. |

Both are 40 MiB raw disk images: protective MBR + GPT + a FAT32 ESP holding
`EFI/BOOT/BOOTX64.EFI` + `kernel-x86_64` + `INSTALL.NOW`. The `INSTALL.NOW`
marker is what triggers the installer — it is already inside the image, nothing
to create by hand.

---

## ⚠️ READ THIS FIRST

**The real install ERASES Athena's internal NVMe.** It writes a new partition
table over whatever is there now (Windows, data, anything). Back up first, or be
certain the internal drive is expendable. The install targets the **active block
device** = Athena's internal NVMe; the USB stick is the *source*, never the
target.

The SAFE stick exists precisely so you can confirm "this boots and the install
logic runs" **without** any write landing. In QEMU the safe run produced
`[safe-mode] BLOCKED nvme write lba=0 …`, `stages=0/5`, and the target disk's
sentinel byte was untouched.

---

## 1. Flash a USB stick (on this Windows box)

Use any **raw / "DD" image** writer. The image is a full disk image, not a file
to copy.

**Rufus** (simplest):
1. Insert the USB stick (≥ 256 MiB; the image is 40 MiB).
2. Rufus → **Device** = your USB stick (double-check the size/letter — this
   erases it).
3. **Boot selection** → SELECT → pick `target\install-usb-safe.img`
   (for pass 1) — change the file filter to "All files" if the .img is hidden.
4. If Rufus asks **ISO vs DD** → choose **DD Image mode**.
5. START. Repeat later with `target\install-usb.img` for the real install.

**balenaEtcher** also works (always writes raw) — Flash from file → pick the
`.img` → select the USB → Flash.

> Do **not** "format then copy files" — that breaks the GPT/boot layout. It must
> be a raw image write.

---

## 2. Boot Athena from the stick

1. Insert the stick, power on, and spam the **one-time boot-menu key** (Beelink
   mini-PCs: usually **F7**; BIOS setup is **Del** or **F2** — confirm yours).
2. In BIOS/UEFI settings, make sure:
   - **Secure Boot = Disabled** (RaeenOS's `BOOTX64.EFI` is not Microsoft-signed —
     Secure Boot will refuse it).
   - Boot mode = **UEFI** (not Legacy/CSM).
3. From the boot menu pick the **UEFI: <USB stick name>** entry.

Output appears **on Athena's monitor** — every kernel log line is mirrored to the
screen (GOP framebuffer), so you can watch the whole boot/install live.

---

## 3. Pass 1 — SAFE dry-run (`install-usb-safe.img`)

Boot it and watch the screen for, in order:

```
[safe-mode] ENABLED — sector writes will be refused at the BlockDevice trait
... (normal boot) ...
[install] INSTALL.NOW marker present on the boot stick — running automated install
[safe-mode] BLOCKED nvme write lba=0 ...          ← writes are being refused (good)
[install] automated install finished: stages=0b00000 (0/5) — boot tree NOT written
[ OS ] System successfully booted.
```

Seeing this = **the stick boots on Athena AND the install touched nothing.** Your
internal drive is exactly as it was. If it instead drops to a `Shell>` prompt or
hangs, see Troubleshooting.

Then pull the stick, bring it back here, and dump the on-stick boot log:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\read-bootlog.ps1
# auto-detects the USB; writes BOOTLOG.dump.txt. (Pass -Disk N if it can't pick one.)
```

Skim `BOOTLOG.dump.txt` for the same lines (and any `[PANIC]`). No panic + the
safe-mode blocks = you're clear to do the real install.

---

## 4. Pass 2 — REAL install (`install-usb.img`)

Flash `target\install-usb.img` to the stick (step 1 again), boot Athena from it,
and watch for:

```
[install] INSTALL.NOW marker present on the boot stick — running automated install
[install] stage 1 GPT: ESP at LBA 2048, RaeFS root at LBA 264192
[install] clone ESP: NNNNN sectors (38 MiB) source ESP@2048 -> target ESP@2048 -> bootable
[install] stage 4 RaeFS: on-disk format at LBA 264192 ... superblock_readback=true -> PASS
[install] ===== install complete: stages=0b11111 (5/5) =====
[install] automated install finished: stages=0b11111 (5/5) — bootable ESP written; remove the stick and power-cycle
```

**`stages=5/5`** = success. (If you see `clone ESP` *fail* and a
`NOT firmware-bootable` fallback line, the installer couldn't read the stick's
ESP — capture the log and stop.)

---

## 5. Pass 3 — boot the installed system (no stick)

1. **Power off. Remove the USB stick.** (Leaving it in re-triggers the installer.)
2. Power on. In the boot menu / BIOS boot order, pick the **internal NVMe**
   (UEFI). You may need to set it first in the boot order.

Watch for:

```
BdsDxe: starting Boot... NVMe...        ← firmware boots the installed disk
... RaeenOS bootloader + kernel ...
[storage] RaeFS mounted from partition 2 @ LBA 264192
[ OS ] System successfully booted.
```

That's RaeenOS booting **from its own NVMe** — the glassmorphic login/desktop
should come up. This is the milestone the QEMU runs proved; Athena is the iron
confirmation.

---

## Troubleshooting

- **Drops to `Shell>` / "Not Found" / "no bootable device":** firmware couldn't
  load `\EFI\BOOT\BOOTX64.EFI`. Check Secure Boot is **off** and boot mode is
  **UEFI**. From the `Shell>` you can type `fs0:` then `ls` to see if the ESP
  mounted (`EFI\BOOT\BOOTX64.EFI` should be listed).
- **Black screen / no serial mirror:** the display may have come up on a
  different output (HDMI vs USB-C/DP). Try the other port.
- **`[PANIC]` on screen:** photograph it (or grab `BOOTLOG.dump.txt` from the
  stick) and send it — that's a real bug to fix, not a flashing problem.
- **Install reports `clone ESP` failed:** the kernel couldn't enumerate the stick
  as a USB-MSC device, or couldn't find its ESP. Note the exact `[usb-msc]` and
  `[install]` lines from the log.
- **Re-flashing the stick in Windows later:** Windows may show the stick as
  "unformatted" or only show a tiny partition — that's expected (it's a Linux/UEFI
  layout). Just re-flash the raw image over it; don't let Windows "format" it.

---

## What this does NOT cover yet (known gaps)

- Real GPU acceleration (software-rendered until the AMD driver lands), real NIC /
  audio / USB-flash drivers proven on Athena, and a polished installer UI. This is
  a *bootable persistent install*, not yet a feature-complete daily driver.
- A post-boot userspace `RELIBC PANIC` (syscall 141) may appear *after*
  `System successfully booted` — it doesn't block boot; it's a separate userspace
  gap.
