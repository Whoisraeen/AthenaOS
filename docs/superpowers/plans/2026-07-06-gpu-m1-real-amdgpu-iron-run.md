# M1: Real-amdgpu Iron Run Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One `ATHENA_AMDGPU_REAL=1` iron boot on Athena that answers: does the REAL upstream amdgpu C (Strategy B), running under the LinuxKPI shim, clear the MES `set_hw_resources` wall (`0x7656` halt) that the native Rust reimplementation cannot?

**Architecture:** The real amdgpu object set (`~/m4-obj/amdgpu-bringup.o`, 88 objects, built by `linuxkpi-drm/m4c-link.sh`) links into the `amdgpud` userspace daemon via the `real_amdgpu_init` feature. The daemon probes the GPU, wires BARs into `ath_linuxkpi::device_map`, and calls `rae_amdgpu_device_init(...)` → `amdgpu_driver_load_kms` → `amdgpu_device_init` — the complete upstream init. Evidence comes back over the netlog UDP broadcast; Athena auto-returns to Arch via the ~480 s watchdog.

**Tech Stack:** Rust (`x86_64-unknown-none`), vendored GPL amdgpu C via clang (WSL), xtask, QEMU/KVM, ssh/efibootmgr iron loop.

**Spec:** `docs/superpowers/specs/2026-07-06-gpu-real-submit-design.md` (§8 M1, §8 fallback trigger).

## Global Constraints

- **Iron boots use the `--safe` image ONLY** (CLAUDE.md §9 — a non-safe image once wiped a Windows partition).
- **Build tree for this plan is the WSL checkout `~/athenaos`** (clang + `~/m4-obj` + `/dev/kvm` live there; the Windows git-bash has no clang). Its `origin` is the Windows checkout (`file:///mnt/c/Users/woisr/Documents/Projects/AthenaOS`).
- **Never discard the WSL tree's uncommitted WIP** — it is the 07-04 session's post-reloc-fix debug work (`rae_dbg_ptrs`, `kernel/src/linuxkpi_host.rs` +93 lines, new syscall plumbing). Commit it before anything else (multi-agent memory: commit slices immediately).
- **Gates before every commit:** `export ATHENA_AGENT=opus && bash scripts/ownership-lock.sh && bash scripts/architecture-gate.sh`. If the staged diff touches `components/ath_abi/`, the commit subject MUST carry `[interface]`.
- Stage **explicit paths** only — never `git add .` (OneDrive/multi-agent hazard).
- Netlog listener runs on the **Windows** side (`scripts\netlog-listen.ps1`) — WSL2 NAT does not receive LAN UDP broadcasts.
- Athena: `whoisathena@192.168.1.244`, passwordless sudo, GPU `c4:00.0` (`1002:15bf` Phoenix1). AthenaOS boot entry = `Boot0003` "AthenaOS-test", fired only via `efibootmgr --bootnext 0003` (never in BootOrder).
- **NEVER live unbind/rebind the APU** on Athena (wedges the iGPU; recovery = physical power button).
- Serial sentinels print as `[user-thread] msg: N` (9000 = amdgpud start, 9098 = real-init done, 9099 = no GPU/skip).

---

### Task 1: Reconcile and sync the WSL build tree

**Files:**
- Modify (commit, WSL tree): the 17 dirty files (`amdgpud/*`, `components/ath_linuxkpi/*`, `components/ath_abi/src/lib.rs`, `kernel/src/linuxkpi_host.rs`, `kernel/src/syscall.rs`, `kernel/src/interrupts.rs`, `linuxkpi-drm/m4-link.sh`, `linuxkpi-drm/m4c-link.sh`, `xtask/src/main.rs`, + rest of `git status`)

**Interfaces:**
- Produces: WSL `~/athenaos` at Windows-main HEAD (contains `c0e350d` reloc fix, `d4887f0` spec, `1453ee0` amendment) + the 07-04 debug WIP committed on top. All later tasks build from this tree.

- [ ] **Step 1: Inspect and commit the WIP in WSL**

```bash
wsl -e bash -lc 'cd ~/athenaos && git status --short && git diff --stat | tail -3'
```

Expected: ~17 modified files, ~481 insertions. Then commit it (subject carries `[interface]` because `components/ath_abi/src/lib.rs` is in the diff — verify with `git diff --stat components/ath_abi/` first; if ath_abi shows 0 lines changed, drop the tag):

```bash
wsl -e bash -lc 'cd ~/athenaos && export ATHENA_AGENT=opus && bash scripts/ownership-lock.sh && git add -u && bash scripts/architecture-gate.sh && git commit -m "[interface] amdgpu: 07-04 real-init debug WIP — rae_dbg_ptrs hook + linuxkpi_host C-debug syscalls

Why:
- MasterChecklist Phase 6.1 — real amdgpu M5 run: diagnostics for the soc21 nbio.funcs wall
- Preserves the 07-04 session WIP before the M1 iron run builds this tree

What changed:
- amdgpud/src/main.rs: rae_dbg_ptrs debug hook (C vscnprintf faults on %lx)
- kernel/src/linuxkpi_host.rs + syscall.rs + ath_abi: host debug plumbing
- linuxkpi-drm/m4-link.sh + m4c-link.sh: build tweaks from the 07-04 session

Verify:
- Task 2 of docs/superpowers/plans/2026-07-06-gpu-m1-real-amdgpu-iron-run.md (full build)
"'
```

Expected: gates green, commit created. NOTE: `git add -u` is acceptable here ONLY because the goal is preserving the entire WIP set verbatim; list the staged files with `git status --short` in the same invocation and confirm nothing unexpected (no `logs/`, no `target/`).

- [ ] **Step 2: Fetch Windows main and rebase the WIP onto it**

```bash
wsl -e bash -lc 'cd ~/athenaos && git fetch origin && git log --oneline origin/main -3 && git rebase origin/main'
```

Expected: `origin/main` shows `1453ee0` (spec amendment) at tip; rebase applies the nvidia-skeleton commit + the WIP commit cleanly.
**If the rebase conflicts:** `git rebase --abort`, then build from a clean detached checkout instead — `git switch -c m1-clean origin/main` — and note in the run log that the 07-04 debug hooks are NOT in the image (the WIP branch stays preserved as `main@{1}`; do NOT delete it). The run is still valid — the debug hooks are diagnostics, not init-path behavior.

- [ ] **Step 3: Verify the tree state**

```bash
wsl -e bash -lc 'cd ~/athenaos && git log --oneline -4 && git status --short | wc -l'
```

Expected: WIP commit on top, `1453ee0` in ancestry, 0 dirty files.

---

### Task 2: Build the M1 iron image (`--safe`, real amdgpu) in WSL

**Files:**
- Create (build artifacts, WSL): `~/athenaos/target/kernel.uefi.img`, `~/athenaos/target/x86_64-unknown-none/release/amdgpud`

**Interfaces:**
- Consumes: Task 1's synced tree; `/home/athena/m4-obj/` (m4c rebuilds it).
- Produces: `~/athenaos/target/kernel.uefi.img` — the ONLY image Task 5 deploys.

- [ ] **Step 1: Full safe UEFI build with the real amdgpu set**

```bash
wsl -e bash -lc 'cd ~/athenaos && ATHENA_AMDGPU_REAL=1 cargo run -p xtask --release -- build --release --safe --uefi 2>&1 | tail -30; echo EXIT=$?'
```

Expected:
- `[xtask] ATHENA_AMDGPU_REAL: building the real amdgpu object set (m4c-link.sh)...`
- m4c output: `[m4c] real objects: 88   external symbols to stub: <N>` and a final zero-unresolved confirmation
- `EXIT=0`

If m4c fails: read `/tmp/m4-link.log` (its step-0 compile log) — clang or the vendored GPL tree broke; fix per the log before proceeding. Do NOT fall back to the Rust-reimpl daemon (that is not M1).

- [ ] **Step 2: Verify the image actually carries the real-init daemon**

```bash
wsl -e bash -lc 'cd ~/athenaos && nm target/x86_64-unknown-none/release/amdgpud | grep -cE " T (rae_amdgpu_device_init|amdgpu_device_init)" && readelf -r target/x86_64-unknown-none/release/amdgpud | grep -c "R_X86_64_64" || true'
```

Expected: first count = `2` (both symbols defined as text); second count = `0` (the `c0e350d` -Bsymbolic guarantee — any nonzero means the 0x77 reloc fault class is back: STOP, re-check `amdgpud/build.rs` has `cargo:rustc-link-arg=-Bsymbolic` and m4-link.sh has `-fvisibility=hidden`).

- [ ] **Step 3: Confirm safe mode is baked in**

```bash
wsl -e bash -lc 'cd ~/athenaos && ls -la target/kernel.uefi.img && strings target/x86_64-unknown-none/release/kernel 2>/dev/null | grep -m1 "safe-mode" || echo "strings-check skipped (marker optional)"'
```

Expected: image exists, dated now. (The authoritative safe-mode proof is the boot banner in Task 5's capture: `[safe-mode]` lines + the write-guard messages.)

---

### Task 3: QEMU/KVM regression gate on the real image

**Files:**
- Read: `/tmp/athena-serial.log` (WSL xtask writes the serial log to `$TMPDIR` or `/tmp` on Linux)

**Interfaces:**
- Consumes: Task 2's build (xtask `run` reuses the same env: keep `ATHENA_AMDGPU_REAL=1` so it does not rebuild the default daemon over it).
- Produces: a green CI verdict authorizing the iron deploy.

- [ ] **Step 1: Boot the exact real-amdgpu image in QEMU CI mode**

```bash
wsl -e bash -lc 'cd ~/athenaos && ATHENA_AMDGPU_REAL=1 cargo run -p xtask --release -- run --release --ci 2>&1 | tail -5; echo EXIT=$?'
```

Expected: `EXIT=0` (booted). Runtime note: KVM in WSL makes this fast; if it runs under TCG expect minutes, not seconds.

- [ ] **Step 2: Verify the boot log — green + the daemon self-skips**

```bash
wsl -e bash -lc 'L=$(ls -t /tmp/athena-serial.log ${TMPDIR:-/tmp}/athena-serial.log 2>/dev/null | head -1); grep -c "PANIC" "$L"; grep -m1 "System successfully booted" "$L"; grep -m1 "no AMD GPU found" "$L"; grep -m1 "msg: 9099" "$L"'
```

Expected: `0` panics, `[ OS ] System successfully booted.`, `[amdgpu] no AMD GPU found — amdgpud exiting (expected on QEMU)`, sentinel `9099`. This proves shipping the real daemon does not regress the OS. If ANY line is missing: STOP, diagnose before iron (CLAUDE.md rule 21).

---

### Task 4: Preflight Athena + the capture path

**Files:**
- Run: `scripts/netlog-listen.ps1` (Windows side)

**Interfaces:**
- Produces: a live netlog listener writing to a capture file; verified Boot0003; healthy `/boot` FAT on Athena.

- [ ] **Step 1: Athena reachability + boot entry + ESP health**

```powershell
ssh whoisathena@192.168.1.244 "sudo efibootmgr | grep -i 0003; mount | grep ' /boot '; ls -la /boot/kernel-x86_64 /boot/EFI/RAEEN/ 2>/dev/null"
```

Expected: `Boot0003* AthenaOS-test`, `/boot` mounted `rw` (NOT `ro` — `ro` means the FAT went errors=remount-ro: run `sudo umount /boot && sudo fsck.fat -a /dev/nvme0n1p1 && sudo mount /boot` first), and the existing deployed kernel files listed.

- [ ] **Step 2: Start the netlog listener on Windows (background, ≥20 min)**

```powershell
# From the repo root on Windows, in its own terminal (leave running):
powershell -ExecutionPolicy Bypass -File scripts\netlog-listen.ps1
```

Expected: listener binds UDP 51514 and prints a waiting banner. Confirm the output file path it announces (historically `BOOTLOG.netlog.txt` in the working directory) — Task 6 reads it.

---

### Task 5: Deploy + iron boot + capture

**Files:**
- Modify (Athena): `/boot/kernel-x86_64`, `/boot/EFI/RAEEN/kernel-x86_64`

**Interfaces:**
- Consumes: Task 2's `~/athenaos/target/kernel.uefi.img`, Task 4's running listener.
- Produces: the M1 netlog capture (Task 6's evidence).

- [ ] **Step 1: Ship the image to Athena (from WSL — it built there)**

```bash
wsl -e bash -lc 'scp ~/athenaos/target/kernel.uefi.img whoisathena@192.168.1.244:/tmp/athena-m1.img && ssh whoisathena@192.168.1.244 "ls -la /tmp/athena-m1.img"'
```

Expected: file lands, size matches the local image.

- [ ] **Step 2: Extract the kernel from the image onto the ESP (loop-mount, both paths)**

```bash
wsl -e bash -lc 'ssh whoisathena@192.168.1.244 "LOOP=\$(sudo losetup -fP --show /tmp/athena-m1.img) && sudo mount \${LOOP}p1 /mnt && sudo cp /mnt/kernel-x86_64 /boot/kernel-x86_64 && sudo cp /mnt/kernel-x86_64 /boot/EFI/RAEEN/kernel-x86_64 && sudo umount /mnt && sudo losetup -d \$LOOP && sync && ls -la /boot/kernel-x86_64"'
```

Expected: new timestamp/size on `/boot/kernel-x86_64` (ESP ROOT — the bootloader loads from root, NOT only `/boot/EFI/RAEEN/`).

- [ ] **Step 3: Fire the one-shot boot**

```bash
wsl -e bash -lc 'ssh whoisathena@192.168.1.244 "sudo efibootmgr --bootnext 0003 && sudo reboot" || true'
```

Expected: ssh drops as the box reboots. `BootNext: 0003` printed before the drop.

- [ ] **Step 4: Wait and watch the listener**

Watch the Windows netlog terminal. Timeline: firmware+AthenaOS boot ~30 s → amdgpud starts (sentinel 9000, SCHED_BODY self-promote) → checkpoints stream → either the init completes/fails fast, or the watchdog fires at ~480 s and Athena reboots back to Arch. **Do nothing for 10 minutes.** If NO netlog packets arrive by +4 min: the boot may have wedged pre-network — wait for the watchdog return anyway; if Athena is not back on ssh by +15 min, it needs the physical power button (owner) — report and stop.

- [ ] **Step 5: Confirm Athena returned to Arch + post-run ESP health check**

```bash
wsl -e bash -lc 'ssh -o ConnectTimeout=10 whoisathena@192.168.1.244 "uptime && sudo fsck.fat -n /dev/nvme0n1p1 2>&1 | tail -3"'
```

Expected: fresh uptime (minutes); fsck read-only check reports clean (the shared-ESP corruption class from memory — if dirty: `sudo umount /boot && sudo fsck.fat -a /dev/nvme0n1p1 && sudo mount /boot`, then flag it in the run report).

---

### Task 6: Read the verdict, classify, and land the evidence

**Files:**
- Create: `docs/gpu-oracle/netlog-M1-REAL-AMDGPU-20260706.txt` (the capture, committed)
- Modify: `MasterChecklist.md` (the 6.1 real-amdgpu row), the memory topic file

**Interfaces:**
- Consumes: the netlog capture file from Task 4/5.
- Produces: the M1 verdict + the next-step decision per spec §8.

- [ ] **Step 1: Extract the marker ladder from the capture**

```powershell
Select-String -Path .\BOOTLOG.netlog.txt -Pattern "amdgpud starting","CKPT 0","CKPT 1","REAL-INIT","RELOC-CHK","DBG ","RETURNED 0","FAILED","msg: 9098","MES","set_hw_resources","HALT-DUMP" | Select-Object -First 60
```

Expected ladder (happy path): `9000` → `CKPT 0` → `CKPT 1: probe OK + DISCOVERY ACTIVE` → `REAL-INIT: wiring device_map + calling amdgpu_device_init` → `RELOC-CHK nbio_v4_3_funcs@… slot35(set_reg_remap)=0x…2c (want ~0x12bc2c)` → upstream amdgpu printk stream (early_init → sw_init → hw_init per IP block) → `REAL-INIT: amdgpu_device_init RETURNED 0` → `msg: 9098`.

- [ ] **Step 2: Classify into exactly one verdict branch**

| Branch | Evidence | Next action |
|---|---|---|
| **A. M1 PASS** | `RETURNED 0` (or the log shows MES `set_hw_resources` acked / MES hw_init passed even if a later IP block fails) | The wall is DOWN. Update MasterChecklist 6.1 real-amdgpu row to `[~]→` iron-noted; write the M2 plan (DRM seam). |
| **B. Reloc regression** | `RELOC-CHK slot35=0x77` or garbage | The loader/link fix didn't hold on iron: re-verify `-Bsymbolic` made it into THIS image (Task 2 Step 2 output), check `kernel/src/elf.rs` R_X86_64_RELATIVE handling; fix, rebuild, redeploy (repeat Tasks 2–6). NOT a §8 fix-arc (it's pre-MES plumbing). |
| **C. MES halt, same signature** | init reaches gfx/mes hw_init then silence/halt; any `HALT-DUMP` shows `INSTR frozen 0x7656` | **§8 fix-arc counter = 1.** Diff the captured init stream against `docs/gpu-oracle/stock-init-20260706.txt` + the mmiotrace oracle; fix the shim behavior gap; ONE more arc allowed before the Route-B pivot. |
| **D. New pre-MES wall** | init fails/hangs in an earlier facade (TTM page/pfn is the predicted next per `linuxkpi-drm/M5-ONPATH-AUDIT.md`) | Fix the named facade in `ath_linuxkpi` (host-KAT first per rule 15), rebuild, redeploy. Not a §8 arc — the run hasn't reached MES yet. |

- [ ] **Step 3: Land the evidence**

```powershell
Copy-Item .\BOOTLOG.netlog.txt "docs\gpu-oracle\netlog-M1-REAL-AMDGPU-20260706.txt"
```

Then update `MasterChecklist.md`'s Phase 6.1 AMDGPU row with the verdict (honest ladder: iron evidence only), append the verdict to the memory topic file (`amdgpu-iron-hang-uc-firmware-read.md`), and commit from the **Windows** tree:

```powershell
cd C:\Users\woisr\Documents\Projects\AthenaOS
$env:ATHENA_AGENT="opus"; bash scripts/ownership-lock.sh; bash scripts/architecture-gate.sh
git add docs/gpu-oracle/netlog-M1-REAL-AMDGPU-20260706.txt MasterChecklist.md
git commit -m "gpu: M1 real-amdgpu iron run — <VERDICT BRANCH + one-line evidence>"
```

(Also `wsl -e bash -lc 'cd ~/athenaos && git fetch origin'` afterward so the WSL tree sees the verdict commit.)

- [ ] **Step 4: Report to owner**

State: the branch taken, the exact marker lines, the §8 fix-arc counter value, and the single next step. No hedging — the plan's decision table already made the call.

---

## Self-review notes (done at write time)

- **Spec coverage:** M1 (spec §8 row 1) fully covered; §8 fallback counter wired into Task 6 branch C; §2 oracle used in branch C; safe-mode + capture constraints from CLAUDE.md §9 embedded.
- **No placeholders:** every step has exact commands + expected output; verdict branches have named next actions.
- **Type/name consistency:** marker strings copied verbatim from `amdgpud/src/main.rs` (`CKPT 0/1`, `REAL-INIT`, `RELOC-CHK`, sentinels 9000/9098/9099) and `m4c-link.sh` (`[m4c] real objects:`); paths cross-checked against memory + live box state (Boot0003, `/boot` layout, `~/m4-obj`).
- **Known risk accepted:** Task 1's `git add -u` (whole-WIP preserve) is a deliberate exception to explicit-paths, justified + guarded inline.
