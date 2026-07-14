# One-Shot Iron Flash — Verdict Runbook (2026-06-22)

Everything on the software side of the three "on-iron" blockers is committed and
QEMU-proven. This is the exact procedure to turn ONE Athena flash into a
definitive yes/no on all of them at once (per CLAUDE.md §9 "maximize what one
boot teaches"). Pairs with `docs/SAFE_ATHENA_BOOT.md` (full safety procedure).

## PREREQUISITE — clear the boot blocker (one revert, ~30s)
The main working tree currently livelocks the boot. **PROVEN** (clean-worktree
raeen-verifier PASS, 2026-06-22) that the sole cause is a concurrent session's
UNCOMMITTED `sched_proof` WIP — committed `HEAD` boots clean 7/7 HEALTHY.

To get a clean buildable tree, EITHER:
- have the `sched_proof` author finish/commit a non-livelocking version, OR
- build from committed HEAD without the uncommitted WIP:
  ```sh
  git stash -u              # sets aside ALL uncommitted work — coordinate first!
  # ...build+flash...       # (or: build from a detached worktree at HEAD)
  git stash pop             # restore the concurrent WIP afterward
  ```
  Safer (no stash, no disturbing the concurrent agent):
  ```sh
  git worktree add --detach /c/raeen-flash <current-main-sha>   # outside the repo
  cd /c/raeen-flash
  ```
  (committed HEAD has 0 `sched_proof` refs → boots clean.)

## BUILD the safe iron image (the only image ever flashed for testing)
```sh
cargo run -p xtask --release -- build --release --safe --uefi
```
Produces `target/kernel.uefi.img`. `--safe` = `block_io::safe_mode_guard_write`
refuses every sector write except the pre-created BOOTLOG.TXT (no disk wipe risk).

## FLASH (refuses internal drives)
```powershell
scripts/flash-usb.ps1            # pick the USB stick
```
Boot the Athena from the stick. Then pull the bootlog:
```powershell
scripts/read-bootlog.ps1 -Disk 0     # or get-bootlog.bat
scripts/netlog-listen.ps1            # (alternative) end-of-boot UDP broadcast
```

## THE THREE VERDICTS — grep the bootlog

### 1. Networking RX → real internet on hardware (blocker #3)
```
grep "[net] iron DHCP:" BOOTLOG.TXT
```
- `-> BOUND ip=...`        => RX works, DHCP bound on the real RTL8125. ✅ internet.
- `-> NO-RX (no frames...)` => RX still dead on iron (frames never arrive). Check
  the adjacent `[rtl] RX received: pkts=N` (N=0 confirms no RX) + `[rtl] RX armed:
  ... rx_desc_own=A/64`. The 3 posted-write fences (commit `bf4d7cf`) target the
  most likely cause; if still NO-RX, the next localizer is PHY-RXDV / 8125 extra init.
- `-> RX-OK-but-unbound`   => frames arrive but DHCP didn't complete (server/lease).

### 2. Apps clickable on iron (blocker #2)
All 9 cars are bundled + registered + QEMU-PASS. On iron:
- Boot to desktop, open the Start menu — confirm tiles: Files, Photos, Music,
  Video, Notes, Clock, Passwords, Calendar, **Browser, Mail, VPN, Sync** + Settings.
- Click each; confirm it launches (spawn_app_from_vfs) and renders its window.
- Quick functional checks: Files→open a PDF/DOCX/image; Passwords→unlock+TOTP;
  Calendar→agenda; Video→open a baseline .mp4 (real keyframe); Browser→about:home
  + a local .html with a clickable button; Mail/VPN/Sync→the UI + host-proven flows.
- (raeen-beta-tester can drive this via QMP once the desktop is up.)

### 3. GPU (blocker #1 — owner's lane)
While flashed, capture the amdgpu bring-up state for the owner's debug:
```
grep -E "amdgpu|DISC [0-9]|CKPT|ip_discovery" BOOTLOG.TXT
```
(See [[amdgpu-iron-hang-uc-firmware-read]]: the stall is the first UC-mapped
firmware read after ip_discovery.bin; the DISC 1-5 checkpoints localize it.)

## What's proven WITHOUT this flash (so the flash only needs to confirm)
- 9 cars build for the bare target + bundle into the initramfs + boot clean
  (clean-worktree verifier PASS: 7/7 HEALTHY, 9/9 smoketests, 0 panics).
- Networking stack end-to-end on virtio (`[dhcp] rx -> event=Bound`,
  `[DHCP] Configured: IP/GW/DNS`). The iron gap is RTL8125-hardware-RX only.
- Every car's logic host-KAT'd (FAIL-able), security review clean, fail-closed.

The flash converts "QEMU-proven" → "iron-proven" for #2 and #3 in a single trip.
