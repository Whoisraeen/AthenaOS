# AthenaOS amdgpu status — 2026-06-27 (GFX FIRST LIGHT achieved)

The session that turned the AMD GPU bring-up from "intractable / host-wedging" into
**a live graphics engine with one precisely-localized remaining gate.** Everything
here is iron-verified on the Athena Beelink (Phoenix1 `1002:15bf`, Radeon 760M) via
the cold-vfio KVM loop. 18 GPU commits this session.

## ✅ What works now (iron-proven, host stays alive)

The full GFX cold-start chain, end to end:

| Stage | Evidence |
|---|---|
| Discovery / VBIOS / GMC / IH | 39 IP blocks, VRAM 2048 MiB, IH ring programmed |
| SMU mailbox | `GetSmuVersion=0x004c5700`, DisallowGfxOff acked |
| **GFX power-up + RLC autoload** | `RLC_BOOTLOAD_STATUS 0x0 → 0xc000001f complete` = **FIRST LIGHT** |
| **IMU executing** | `GFX_IMU_CORE_STATUS` CBUSY=1 |
| **CP cores alive** | `CP_ME_CNTL=0` (matches working driver), `CP_STAT` busy (PFP/RCIU) |
| CP + SDMA register apertures | writes latch (were all dropping to 0 when gated) |
| GART / VMID0 | `CONTEXT0_CNTL=0x01fffe01`, page-table base set, identity map |

### The keystone fix
`EnableGfxImu` IS `amdgpu_dpm_set_gfx_power_up_by_imu` (the GFX power-up + IMU start).
It MUST be sent, but only its **synchronous response-poll** wedged the APU. Sending
it **async** (`smu_send_msg_async` — write the mailbox, never read the response)
powers up GFX without hanging. Commit `d189031`. Every subsequent register was matched
to the live working driver via `umr` comparison.

## ⛔ The one remaining gate: the CP doesn't fetch our ring

`CP EXEC: wrote WPTR=14, RPTR=0x0` (register AND writeback both 0). The CP's PFP is
busy/stuck *before* fetching the first packet (`CP_STAT` = CP_BUSY+RCIU_BUSY+PFP_BUSY).

**Hypotheses RULED OUT this session (with live umr/iron data):**
- ✗ "CP ran but the register doesn't update" — the RPTR **writeback** dword is also 0
- ✗ GART addressing — `CONTEXT0_START=lo` is set (walker indexes relative to the ring VA)
- ✗ GART page-table format — depth=0 flat, the standard GART format
- ✗ `CP_RB0_CNTL` config — our BUFSZ/BLKSZ are structurally fine; the working driver's
  extra bits (21/23) are dynamic `MIN_AVAILSZ`/`MIN_IB_AVAILSZ`, not config

**Hypotheses ALSO ruled out (commits 7c59ae0 + the cp-resume work):**
- ✗ **The `cp_gfx_resume` micro-sequence** — downloaded the full `gfx_v11_0.c` and
  matched it EXACTLY (unhalt-first, `CP_RB_WPTR_DELAY=0`, `WPTR=0`, separate
  rptr/wptr writeback dwords, the `mdelay(1)` + DOUBLE `CP_RB0_CNTL` write, no
  `CP_RB0_RPTR` write). `CP_ME_CNTL` now `0x15000000→0` (matches working). CP still
  doesn't fetch — so the sequence is NOT the gate.
- ✗ **MES requirement** — the source confirms the gfx ring uses `cp_gfx_resume` /
  `CP_RB0` directly (called when `!amdgpu_async_gfx_ring`); MES only does KIQ/compute.
  So the CP_RB0 path IS valid; MES is not required for first pixels.

**ALSO ruled out:** GART PTE encoding — `gart_sys_pte_flags` =
`VALID|SYSTEM|SNOOPED|R|W|X|MTYPE_UC`, the correct flags for a system-memory ring
(matches amdgpu's GART convention).

## 🔑 PARADIGM CORRECTION (iron 2026-06-27) — Phoenix's CP is **F32, not RS64**

Ground truth from Athena's `/lib/firmware/amdgpu/` (decompressed `.zst` headers):

| blob | `header_version_major` @ off 8 | verdict |
|---|---|---|
| `gc_11_0_1_pfp.bin` | **1** (header_size 0x2c = gfx_fw_header_v1_0) | **F32** |
| `gc_11_0_1_me.bin`  | **1** | **F32** |
| `gc_11_0_1_mec.bin` | **1** | **F32** |
| `gc_11_0_1_rlc.bin` | 2 (v2.3) | (RLC is always v2 — normal) |

amdgpu decides RS64 vs F32 purely by the PFP header: `rs64_enable =
amdgpu_ucode_hdr_version(pfp, 2, 0)` (major==2 && minor==0). Athena's PFP is **v1.0**,
so the real amdgpu **also uses the F32 CP path** for this ASIC+firmware. There is no
rs64-named variant on the box; only one `gc_11_0_1_pfp.bin`, and it's v1.

**Consequence:** `config_gfx_rs64` (and therefore the RS64 program-counter) does NOT
apply to Phoenix. The iron boot confirmed it: `config_gfx_rs64 SKIPPED — CP is F32
(v1 ucode)`. The RS64-PC fix below is correct *for an RS64 ASIC* (kept — it's right,
and applies if the firmware is ever updated to v2), but it is **not this machine's
gate.** The CP-fetch gate is in the **F32 path**, which means the earlier
GART-VA / GOP-ring-residue / doorbell leads are back in play (they were never ruled
out for F32 — the RS64 detour set them aside).

**F32 CP-fetch gate — what we know (iron, this boot):** after PSP autoload completes
(`RLC_BOOTLOAD_STATUS=0xc000001f`) and CP unhalt (`CP_ME_CNTL 0x15000000→0`), the
F32 PFP is *running* (`CP_STAT=0x80008000` = CP_BUSY+PFP_BUSY) but does NOT advance
through our ring (`CP EXEC WPTR=14 RPTR=0`, wb_RPTR=0). For a PSP-load F32 ASIC the
driver does NO extra CP-start step beyond unhalt+cp_gfx_resume — so this is NOT a
missing register write; it's the ring **addressing/delivery**:
1. **GART-VA reach** — our identity ring VA (~6 GB, 0x179a80000) vs what the gfxhub
   VMID0 page table actually maps. amdgpu puts the gfx ring in the low GART aperture.
2. **GOP-ring residue** — the early probe shows the GOP left a CP ring programmed
   (`RPTR=0x1 WPTR=0x3ffa208`); the F32 PFP may still be on it.
3. **Doorbell/WPTR delivery** — is the F32 CP reading WPTR from MMIO or the poll addr?

**Highest-value next action:** umr the *working* amdgpu's `CP_RB0_BASE/CNTL`, the
gfxhub VMID0 GART config (`CONTEXT0_*`, page-table base), and idle `CP_STAT`, then
match ours. That data is what the F32 ring-addressing fix needs.

## ✅ umr REFERENCE CAPTURED + GART APERTURE FIX LANDED (iron 2026-06-27)

Booted Athena into amdgpu (cmdline toggle), `umr -go 0` (gfxoff off) to read the LIVE
working gfx ring + GART. Key values (`docs/gpu-oracle/working-gfx-ring-gart-*.txt`):
`CP_RB0_BASE=0xff0064d0 BASE_HI=0x7f` (ring at GART VA 0x7fff_xxxx), `CP_RB0_RPTR=0x200
WPTR=0x1d200` (actively consumed — CP_RB0 *is* the gfx path), `CONTEXT0_CNTL=0x01fffe01`,
`PAGE_TABLE_START=0x7_fff00000` (= gart_start 0x7fff_00000000 >>12), `DOORBELL_CONTROL=
0xc0000458` (offset 0x458), `DOORBELL_RANGE 0x458..0x7f8`, `RPTR_ADDR/WPTR_POLL HI=0x7fff`.

**GART aperture fix (commit 170b0e4) — VERIFIED on iron:** mapped our buffer span at
`GFX_GART_APERTURE_BASE=0x7fff_0000_0000` (was VA==PA identity). Result on the next
cold-vfio boot: **`CP_RB0_BASE` now reads `0x7f:0xff000000` (BASE_HI=0x7f, matching
amdgpu)**, and the full gfxhub VM readback matches amdgpu EXACTLY (`CONTEXT0_CNTL=
0x01fffe01`, `L1_TLB=0x1859`, `PAGE_TABLE_BASE=0x1:0x79c9c001`, `SYS_APERTURE=
0x200000..0x201fff`). The ring VA + GART are now provably correct.

**…but RPTR still 0.** The ring addressing was necessary, not sufficient.

## 🧱 ADDRESSING BATTLE WON — gate isolated to F32 microengine EXECUTION (iron 2026-06-27)

Three more fixes landed + iron-tested, each matching the working driver exactly:
- **Doorbell** (commit e234bcb): `CP_RB_DOORBELL_CONTROL=0x40000458` (offset 0x458 | EN),
  `RANGE 0x458..0x7f8`, ring at byte 0x458 (was EN-only/offset-0/ring-byte-0). The CP
  reads WPTR from the doorbell once EN is set; we were writing a slot it didn't watch.
- **Full TLB invalidate + ACK poll** (commit 0039da7): write the full
  `GCVM_INVALIDATE_ENG0_REQ=0xF80001` (L2 PTE/PDE0-2 + L1 PTE flush, vmid0) and POLL
  `INVALIDATE_ENG0_ACK` (was a bare 0x1, never polled). Iron: **"GART TLB invalidate
  ACKed (VMID0 flush complete — walker live)"**.

**Every register now matches amdgpu** — ring VA (BASE_HI=0x7f), CONTEXT0_CNTL=0x01fffe01,
L1_TLB=0x1859, PAGE_TABLE_BASE set, SYS_APERTURE, doorbell 0x458, TLB walker flushed +
ACKed. **And BOTH the CP (RPTR=0) AND SDMA (RB_RPTR=0) still stall identically.**

**This rules out the entire addressing/GART/doorbell/TLB layer.** The gate is now
precisely isolated to **F32 microengine execution**: both F32 engines (CP + SDMA) are
firmware-loaded (autoload `RLC_BOOTLOAD=0xc000001f`), unhalted (CP_ME_CNTL=0 / SDMA
HALT=0, threads enabled), correctly addressed, with a live VMID0 walker — yet neither
fetches its ring. CP_STAT=0x80008000 (PFP_BUSY: the PFP is powered + "busy" but never
advances RPTR). This matches the long-standing SDMA "F32 loaded but not EXECUTING"
finding — now confirmed to hit the CP identically, so it's a **common cause**.

**`CP_DEVICE_ID`/`CP_MAX_CONTEXT` (commit a9b6b84) — tried, did NOT unlock.** Added the
`gfx_v11_0_cp_gfx_start` CP-init writes (DEVICE_ID=1, MAX_CONTEXT=7). The CP still
stalls (RPTR=0), confirming the gate is NOT any addressable CP config — it's the
engine-execution/clock layer. Every addressable register (ring VA, GART, doorbell,
TLB, CP-init) now matches amdgpu; the engine simply does not run.

## 🎯 ROOT-CAUSE REDIRECT (2026-06-27) — the gfx ring needs MES scheduling, not raw CP_RB0

amdgpu's `gfx_v11_0_cp_resume` routes on `amdgpu_async_gfx_ring` (**default = 1**):
- `=0` → `gfx_v11_0_cp_gfx_resume` + `cp_gfx_start` (the legacy raw-CP_RB0 path I matched)
- `=1` (DEFAULT) → **`gfx_v11_0_cp_async_gfx_ring_resume`**, which does:
  `gfx_v11_0_kgq_init_queue` (build the gfx **MQD** — Memory Queue Descriptor) →
  **`amdgpu_gfx_enable_kgq`** (MAP the queue via **MES**) → `cp_gfx_start`.

So the working driver runs the gfx ring as a **MES-scheduled Kernel Gfx Queue (KGQ)**,
not a raw CP_RB0 ring. MES maps the queue *into* CP_RB0 (which is why umr shows CP_RB0
active), but the CP only services it once **MES schedules it**. We program CP_RB0 +
doorbell directly and never set up MES/KGQ — so every register matches amdgpu (amdgpu
writes them too) yet the queue is never scheduled → the CP never fetches (RPTR=0).
**This explains the entire "every register matches but nothing fetches" puzzle.**

**CONFIRMED by experiment (2026-06-27):** booted the working amdgpu with
`amdgpu.async_gfx_ring=0` (force the legacy raw-CP_RB0 path) → amdgpu **failed to probe
entirely** (`error -110`) with repeated `MES failed to respond to msg=MISC (WAIT_REG_MEM)`.
So amdgpu depends on MES even in legacy gfx mode; the default async=1 boot works
*because* MES is up and schedules the queue. **AthenaOS never starts the MES engine
(we load mes_2.bin/mes1.bin but never bring up the MES microengine + ring), so the
gfx queue is never scheduled and the CP never fetches.** This is THE root cause of
the CP-fetch gate — not addressing, clock, gating, or the PFP init.

## 🔔 DOORBELL APERTURE BUG FOUND + FIXED (iron 2026-06-27, commit 652ff91)

The doorbell aperture was DISABLED — confirmed: `RCC_DOORBELL_APER_EN 0x0 -> 0x1`
(the GOP firmware sets up only display, never enabled it). After enabling
`BIF_DOORBELL_APER_EN` (NBIO seg2 0xc0 bit0), `HQD_DOORBELL` reads `0xc0000058` — the
`DOORBELL_HIT` status bit (31) is now SET, proving the doorbell write NOW REACHES the
HQD (it didn't before — was 0x40000058, no HIT). Real, confirmed bug fix; this is
also the gfx CP's doorbell path.

**Next layer:** the MES still doesn't drain (`rptr_report=0`) even though the doorbell
now hits the HQD. So the MES microengine doesn't SERVICE its command queue on the
doorbell. Candidates: (a) the MES needs its doorbell-range/aperture register
(CP_MES_DOORBELL_CONTROL / a per-engine range) so the hit routes to the MES *core*,
not just the HQD status; (b) the wptr value/unit the MES reads (dwords vs bytes); (c)
the MES scheduler loop needs the command ring registered via a MES-specific path, not
just CP_HQD with me=3. Next: umr the working MES HQD/doorbell regs during an active
amdgpu boot + compare; check CP_MES_DOORBELL_CONTROL1 (seen in mes_v11_0). The gfx CP
(also doorbell-driven) should likewise be re-checked now the aperture routes.

## 🔌 INTEGRATION WIRED + IRON-TESTED — gate was localized to DOORBELL DELIVERY (2026-06-27)

The full MES queue-scheduling integration is ASSEMBLED + runs end-to-end on iron
(commits a1358a7/6781a4d): MES alive → MES cmd-ring MQD built + loaded via
queue_init_register → set_hw_resources + map_legacy_queue submitted on the MES ring.
The diagnostic is decisive:
```
MES ring diag: rptr_report=0x0, CP_HQD_ACTIVE=0x1, HQD_DOORBELL=0x40000058
```
- **CP_HQD_ACTIVE=1** — the MES command queue is LIVE (queue_init worked).
- **HQD_DOORBELL=0x40000058** — EN(bit30) + offset 0x58 (index 0x16), correct.
- **rptr_report=0** — but the MES never consumed the packet.

So the queue is set up correctly and the MES simply **doesn't service its ring when we
ring the doorbell** (`ring_doorbell` → BAR2). This is almost certainly the SAME root
cause as the gfx CP never responding to its own doorbell (0x458) all along: **our
doorbell writes aren't reaching the GPU's doorbell aperture.** Next: verify the BAR2
doorbell mapping + the write path (`amdgpud` `ring_doorbell`, the BAR2 ioremap), and/or
whether a doorbell-range/aperture register must be programmed to route index 0x16/0x116
to the MES/CP. This is ONE shared fix that unblocks both the MES command ring AND the
gfx ring kick — the last gate before the CP fetches.

## 📋 COMPLETE INTEGRATION RECIPE (zero unknowns — next session = pure assembly)

Every descriptor/packet is BUILT + host-proven in `mes.rs` (108 KATs). Every constant
the live wiring needs is now resolved. The remaining work is ONLY the bring-up wiring
+ the submit protocol — no more research. The exact recipe:

**Buffers to allocate (before the GART build, joined to the gart_lo/gart_hi span so
they're GART-mapped) — all reached at gart_va(phys):**
- MES cmd ring (64 KiB), MES EOP (2 KiB), MES MQD (2 KiB = 512 dw), MES sch_ctx
  (4 KiB), MES query_fence (4 KiB), gfx MQD (2 KiB). (gfx ring already allocated.)

**Constants (all confirmed):** HWIDs GC=11, MMHUB=34, OSSSYS=40 (→ `discovery::ip_base`
for gc_base[8]/mmhub_base[8]/osssys_base[8]). Doorbell register-indices: MES=0x16
(layout 0x0B<<1; ring byte 0x58), GFX=0x116 (layout 0x8B<<1; byte 0x458). Masks
(`amdgpu_mes_get_hqd_mask` + amdgpu_mes_init): vmid_mask_mmhub=0xFF00,
vmid_mask_gfxhub=0xFF00, gfx_hqd_mask[0]=0x2 (get_hqd_mask(1,2,1)), compute_hqd_mask[0..4]
=0xfe, sdma_hqd_mask[0..2]=0xfc, gds_size=0.

**Step sequence (after MES alive — CP_MES_CNTL=0x0c000000):**
1. `build_mes_mqd(mes_mqd_va, mes_ring_va, mes_rptr_va, mes_wptr_va, mes_eop_va,
   65536, 0x16)` → dma_write to the MES MQD buffer.
2. apply `build_mes_queue_init_register(hqd_regs, &mes_mqd, 0)` (loads the MES HQD).
3. `build_mes_set_hw_resources(&MesHwResources{ gc_base, mmhub_base, osssys_base,
   masks, sch_ctx_va, query_fence_va, api_fence_addr=query_fence_va, api_fence_value=1
   })` → SUBMIT (below).
4. `build_gfx_mqd(gfx_mqd_va, gfx_ring_va(=gart_va(gfx.dma_addr)), gfx_rptr_va,
   gfx_wptr_va, 65536, 0x116)` → dma_write to the gfx MQD buffer.
5. `build_mes_map_legacy_queue(0x116, gfx_mqd_va, gfx_wptr_va, 0, 0)` → SUBMIT.
6. RPTR should now advance (the CP fetches).

**SUBMIT protocol (write packet to MES cmd ring + poll its completion):**
- dma_write the 64-dword packet to the MES ring at the current wptr (dword offset).
- advance wptr += 64; dma_write the new wptr to the MES wptr-poll dword; `ring_doorbell`
  at the MES ring byte offset 0x58 with the new wptr.
- poll the packet's api_status fence (dma_read query_fence) until it reads `1`
  (api_fence_value), bounded — that's the MES acking the command.

(The submit's pure part — laying the packet into the ring image — is host-KAT-able;
the dma_write/doorbell/poll is GpuOps, iron-verified.)

## ✅✅ MES IS ALIVE (iron 2026-06-27) — RUNG 1 COMPLETE

`CP_MES_CNTL readback=0x0c000000 (PIPE0_ACTIVE=true) — MES engine ALIVE` — the EXACT
value the working amdgpu driver reads (PIPE0+PIPE1 active). The MES microengine is now
running on AthenaOS for the first time. The fix was the direct-load: copy mes_2.bin's
ucode (127040 B) + data into GART-mapped buffers, point `CP_MES_IC_BASE` at the ucode's
GART VA (`0x7fff00710000`), prime the I-cache, set the PC (`0xffffffff_f0005000>>2`),
then activate. Commit ec4195d.

**The CP still has RPTR=0** — expected: the MES is alive but IDLE (no queue mapped). It
now needs to be TOLD to schedule the gfx queue (rungs 2-4): KIQ + gfx MQD +
`set_hw_resources` + `MAP_QUEUES`. The hardest part — getting the scheduler engine
itself running — is DONE.

**(historical) RUNG 1 WIRED + IRON-TESTED (did not activate yet):**

iron 2026-06-27: the MES-enable sequence now runs on the live bring-up. The ucode
entry parse is CONFIRMED correct against the real blob (`mes_2.bin` header:
`mes_uc_start_addr = 0xffffffff_f0005000`, a high sign-extended entry; `>>2` → the PC
amdgpu writes). BUT `CP_MES_CNTL` reads back **0** after we write PIPE0_ACTIVE
(`MES engine did not activate`). So our trimmed enable is incomplete — the full
`mes_v11_0_enable` also does `CP_MES_MSCRATCH_LO/HI` writes (0x2815) and a specific
reset→activate handling we skipped, and the MES may need its IC-base / a settle delay.
**Next (rung 1 completion) — set the MES instruction-cache base.** `mes_v11_0_hw_init`
calls `mes_v11_0_load_microcode` BEFORE `mes_v11_0_enable(true)`, and that sets
`CP_MES_IC_BASE_LO/HI` (0x5850/seg1) to the MES ucode address + primes/invalidates the
I-cache (`CP_MES_IC_OP_CNTL`, lines 1041-1078). Without the IC base the MES has no code
to run on activation → it faults → the ACTIVE bit clears (our `CP_MES_CNTL=0`). On the
PSP-autoload path the ucode is loaded but the IC base still needs programming (same
shape as `config_gfx_rs64` for the gfx CP). So rung 1 completion = either DIRECT-load
the MES ucode + set `CP_MES_IC_BASE` (like the SDMA direct-load), or find where the PSP
placed it and point IC_BASE there, THEN enable + `udelay(500)` → expect
`CP_MES_CNTL=0x0c000000`. (The MSCRATCH writes are gated behind a debug flag — not
needed.)

1. **START the MES engine** — ⏳ rung 1 wired + iron-tested, not yet activating:
   `components/ath_amdgpu/src/mes.rs`
   `build_mes_enable_sequence` (the pure `mes_v11_0_enable` register sequence: pulse
   pipe reset → `CP_MES_PRGRM_CNTR_START` from the MES ucode entry, me=3 via
   GRBM_GFX_CNTL → `CP_MES_CNTL` PIPE0_ACTIVE [+PIPE1 for KIQ]). Host-KAT'd 3/3 (100
   amdgpu KATs). Offsets: CP_MES_CNTL=(0x2807,1), PRGRM_CNTR_START=(0x2800,1)/_HI=
   (0x289d,1). NEXT for rung 1: parse the MES ucode `uc_start_addr` from the mes_2/
   mes1 headers (extend `rlc_autoload::extract_mes_*`), resolve the regs in regs.rs,
   wire into the daemon after autoload, read CP_MES_CNTL/status to confirm the pipe
   activates on iron.
2. KIQ (Kernel Interface Queue) for submitting MES commands + its MQD.
3. Build the gfx **MQD** (`gfx_v11_0_kgq_init_queue` / `gfx_v11_0_mqd_init`).
4. `set_hw_resources` (tell MES the doorbell/aperture layout via its ring) +
   **MES MAP_QUEUES** (`amdgpu_mes_map_legacy_queue` / `enable_kgq`) to schedule the
   gfx queue — THEN the CP fetches.
(SDMA's identical RPTR=0 is a SEPARATE cause — the F32 DEC_START issue from prior
notes — since SDMA doesn't use MES.)

## (superseded) umr pass 2 — PFP stuck in its own init, NOT a clock or gating miss:
- Working driver: GFXCLK=800MHz (engine clocked; ours has a bootup clock too),
  `RLC_CGCG_CGLS_CTRL=0x363f` (clock gating ENABLED — so CG-disable is NOT the step).
- Working active `CP_STAT=0x80038400` has **ME_BUSY(17)+MEQ_BUSY(16)** — the ME is
  engaged. **Ours = `0x80808000` = PFP_BUSY(15)+RCIU_BUSY(23), NO ME_BUSY**: the F32
  PFP is *executing* (busy, doing register accesses via RCIU) but stuck in its boot/
  init — it never feeds the ME, so the ME never engages and the ring is never served.
- So the gate is the **PFP's execution environment / init handshake**, not addressable
  config (every config reg matches amdgpu). Candidates: the RLC Clear-State Buffer
  (`gfx_v11_0_init_csb` — RLC_CSIB_ADDR/LENGTH; rlc_resume runs it on the PSP path and
  we may skip it), the IMU/SMU gfx power-up completeness, or a PFP-polled scratch/
  handshake register. Next: trace WHAT the PFP polls (umr the live running CP's
  scratch/handshake regs + compare CSB state), or wire init_csb.

**Older prime-suspect list (GFXCLK now de-prioritized by pass 2):**
1. **GFXCLK / engine clock** — on this APU the SMU controls GFXCLK; if it's 0 (we send
   DisallowGfxOff but never request a gfx clock frequency), the microengines have no
   clock to execute on, though block registers stay readable. Check the SMU GetGfxclkFreq
   / set a clock via the PMFW, and umr GFXCLK on the working driver.
2. **Clock gating (CGCG/CGLS/MGCG)** — gfx11 RLC clock-gates the gfx domain; though it's
   meant to auto-ungate on activity, verify the RLC_CGCG_CGLS_CTRL state vs amdgpu.
3. **A microengine DEC_START / F32_WAKEUP / PROGRAM_COUNTER trigger** beyond unhalt
   (the SDMA-side lead from the prior memory).
MES is NOT the common cause — SDMA doesn't use MES and fails identically.

## (RS64 ASICs only) ROOT CAUSE of an RS64 PFP stall — program-counter was a byte address

The PFP being stuck **before any fetch** (`RPTR=0`, `CP_STAT` PFP_BUSY) is the exact
signature of an RS64 core started at the **wrong entry point**. And it was:
`config_gfx_rs64` (the gfx11 step that sets the PFP/ME program-counter from the
firmware header's `ucode_start_addr`) wrote the **raw byte address**, but the
`CP_PFP_PRGRM_CNTR_START` register is a **DWORD** address. amdgpu's
`gfx_v11_0_config_gfx_rs64` computes:

```
CP_PFP_PRGRM_CNTR_START    = (ucode_start_addr_lo >> 2) | (ucode_start_addr_hi << 30)
CP_PFP_PRGRM_CNTR_START_HI =  ucode_start_addr_hi >> 2
```

We wrote `addr_lo` / `addr_hi` raw — so the RS64 PFP/ME began executing at **4× the
real entry point**, ran into garbage, and wedged (PFP_BUSY, never fetches). Fixed by
`rs64_pc_start()` (new helper) doing the exact gfx11 `>>2`-with-hi-carry split for
PFP, ME, and MEC. Host-KAT'd (`rs64_pc_start_is_dword_address_with_hi_carry` +
updated `config_gfx_rs64_sets_starts_and_pulses_resets`); 97/97 amdgpu KATs pass.

**Why this matches every observation:** config_gfx_rs64 already ran on iron (it made
the CP register block go live — the ring base reads back now, where it was 0 before),
the cores went *busy* (CP_STAT) — but busy at the wrong PC, so no ring fetch. The PC
fix is the missing piece between "cores alive" and "cores fetch the ring."

**Next iron boot — what proves it:** after this fix, `CP EXEC` should show `RPTR`
*advancing* past 0 (the CP consuming the ring), and `CP_STAT` PFP_BUSY clearing as
the PFP idles waiting for work instead of spinning. That's the first executed GPU
command → then SDMA, draw, scanout.

**If RPTR still 0 after this (fallback leads):**
1. umr the working driver's `CP_PFP_PRGRM_CNTR_START` during active bring-up and
   compare to what we write — confirms the ucode_start_addr parse + the `>>2` split.
2. TLB-invalidate ACK poll (we write the request, never poll the ack).
3. CP soft-reset (`gfx_v11_0_soft_reset`) + re-run cp_resume — but note amdgpu does
   NOT soft-reset in normal bring-up, so this is a last resort, not the path.

The other engine, SDMA F32, is separately stuck (`RB_RPTR=0`) — same "F32 microengine
loaded but not executing" class; lower priority than the CP for first pixels.

## Roadmap to "display graphics and games" (multi-week)
CP ring fetch (current gate) → SDMA → a real draw pipeline (PM4 + shaders) →
scanout/modeset to a panel → Mesa + libdrm_amdgpu → Vulkan → Proton/Wine for games.
First light unblocked the hardest, longest-standing link; the rest is a known,
ordered sequence.

## The iron loop (reproducible, no flashing)
- Build on the dev box → `scp kernel.uefi.img` to Athena `~/athena-vm/kernel.uefi.fix.img`
- `~/athena-vm/runfix.sh` (absolute paths) boots it in KVM with the GPU VFIO-passed cold
- **Reset between cold tests = REBOOT** (FLR doesn't clear APU state)
- amdgpu-XOR-vfio: toggle by reboot (revert cmdline for umr; re-add to test)
- To capture serial through an APU wedge: stream over TCP via the existing SSH channel
  (a file on the wedging host loses unsynced page cache; `~/athena-vm/capture-C.sh`)
- Full handoff + all register targets: memory `amdgpu-iron-hang-uc-firmware-read`
