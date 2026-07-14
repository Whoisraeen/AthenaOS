# MES firmware-type fix — the PSP 0xffff0006 rejection cracked (2026-06-28)

**The MES wall was a wrong firmware-type number.** Captured by tracing the working
amdgpu's PSP command payloads on the Athena (not just the MMIO writes).

## Method
amdgpu's `LOAD_IP_FW` command lives in the GPCOM ring *memory*, not MMIO — so the
earlier MMIO trace couldn't see the fw_type. Kprobe on the command-submit instead:
```
echo 'p:pspcmd amdgpu:psp_cmd_submit_buf cmdid=+8(%dx):u32 alo=+28(%dx):x32 ahi=+32(%dx):x32 sz=+36(%dx):u32 ftype=+40(%dx):u32' > /sys/kernel/tracing/kprobe_events
```
(`cmd` = 3rd arg = `%dx`; `psp_gfx_cmd_resp`: cmd_id@+8, LOAD_IP_FW union@+28:
fw_phy_addr_lo@+28 / hi@+32 / size@+36 / **fw_type@+40**.) Then cold `modprobe amdgpu`.

## The working LOAD_IP_FW sequence (cmd 6) — full Athena capture
```
ftype=71  sz=17408    SDMA_UCODE_TH0
ftype=72  sz=16896    SDMA_UCODE_TH1
ftype=2   sz=263168   CP_PFP
ftype=1   sz=263168   CP_ME
ftype=4   sz=267008   CP_MEC
ftype=33  sz=127040   MES pipe0 ucode   <-- AthenaOS sent 76 (RS64_MES) -> REJECTED
ftype=34  sz=131072   MES pipe0 data    <-- AthenaOS sent 77 -> REJECTED
ftype=81  sz=104016   MES KIQ ucode     <-- AthenaOS sent 78 -> REJECTED
ftype=82  sz=131072   MES KIQ data      <-- AthenaOS sent 79 -> REJECTED
ftype=68  sz=66048    IMU_I
ftype=69  sz=66048    IMU_D
ftype=20  sz=2560     (RLC variant)
ftype=21  sz=21104    (RLC variant)
ftype=26  sz=66048    RLC_IRAM
ftype=48  sz=33280    RLC_DRAM_BOOT
ftype=25  sz=8704     RLC_P
ftype=8   sz=25088    RLC_G            (last gfx fw before AUTOLOAD)
```
Preceded by cmd 32 (ring init) + cmd 5 SETUP_TMR (TMR @ 0x80_78000000, 64 MiB), and the
gfx batch is followed by **cmd 33 = AUTOLOAD_RLC (0x21)**, then DMCUB(13)/VCN(51)/etc.
Boot brought up `ring gfx_0.0.0` AND `ring mes_kiq_3.1.0` — fully working MES.

## The fix
The Athena's PSP (Arch kernel 7.0.12) takes the MES under fw_type **33/34/81/82**:
- mes_2.bin (pipe0/scheduler): ucode = **33**, data = **34**
- mes1.bin (pipe1/KIQ):        ucode = **81**, data = **82**

AthenaOS used the `RS64_MES`/`RS64_KIQ` enum values 76/77/78/79 (correct for *some* gfx11
kernels, wrong for this PSP/firmware). Every other fw_type already matched. So:
1. Set the MES fw types to 33/34/81/82.
2. **Revert H1's MES deferral** — put the MES back in the PSP autoload batch (amdgpu loads
   it there, after CP, before IMU/RLC; RLC_G stays last). H1 deferred it because it was
   rejected — but the rejection was the wrong type, not wrong placement. With the right
   type the PSP loads + sets up the MES, so the direct-load workaround (and its entry+1
   stall, the whole H2/H3 chain) should fall away: the MES comes up PSP-loaded like amdgpu.
3. (Optional follow-up) re-add SDMA 71/72 — amdgpu loads them; AthenaOS excluded them after
   a rejection that was likely the same wrong-type/extraction class.

This is THE crack for the weeks-long MES-stuck-at-entry+1 wall: not addressing, not config,
not IC_BASE, not disassembly — a wrong firmware-type number the working driver handed us.

## ✅✅ IRON-CONFIRMED (KVM-VFIO cold boot, 2026-06-28) — THE MES EXECUTES
With the correct fw types the boot shows `LOAD_IP_FW x13` (MES back in the batch, **accepted**,
no `0xffff0006`), `RLC_BOOTLOAD_STATUS=0xc000001f` (first light), and decisively:
```
MES fw-version heartbeat: sched(pipe0)=0x1025088 kiq(pipe1)=0x6038109   (NON-ZERO = EXECUTING)
MES exec probe: INSTR_PNTR 0x7204->0x7204                                (was stuck @0x1401 for weeks)
```
`0x1025088` is the exact fw-version `umr` read off the live working amdgpu, and `0x7204` is its
running INSTR_PNTR. **The MES microengine is now executing its scheduler loop on AthenaOS.**
0 panics.

## ⛔ The NEXT gate (now tractable) — MES command-ring servicing
The MES runs but doesn't yet drain the SCHED command ring:
```
MES set_hw_resources submitted -> NO ack (MES not draining its ring)
MES map_legacy_queue(gfx) submitted -> NO ack
SCHED diag (pipe0): ACTIVE=0x0  DOORBELL=0x00000000 (off 0x58)  rptr=0x0
```
So the MES is alive + executing, but the pipe0 SCHED ring's HQD/doorbell isn't set up so the MES
services `set_hw_resources`/`map_queues`. CP RPTR=0 (gfx ring not fetched yet) follows from that.

### Update 2026-06-28 (later): MAP_QUEUES opcode fixed (0x26 -> 0xA2), gate now isolated to the KIQ
Hexdumped the working amdgpu's KIQ ring on iron — `xxd -e -g4` of
`/sys/kernel/debug/dri/1/amdgpu_ring_mes_kiq_3.1.0` (toggle to amdgpu auto-bind, no kprobe needed,
the ring memory retains the packets it pushed):
```
header 0xc005a200  -> PACKET3 type3, op=(>>8)&0xff = 0xA2 (MAP_QUEUES), count=5
  0x34080000  NUM_QUEUES=1 ENGINE_SEL=5(MES) ME=2 QUEUE_TYPE=0
  0x00000058  doorbell offset 0x58 -> index 0x16 (the SCHED ring)
  0x0033a000 / 0x00000080   MQD @ 0x80_0033a000
  0x00401900 / 0x00007fff   WPTR @ 0x7fff_00401900
then 0xc0033700 = WRITE_DATA(0x37) fence of 0xdeadbeef.   (NO SET_RESOURCES on this ring.)
```
AthenaOS emitted op **0x26** for MAP_QUEUES — the wrong PACKET3 opcode (commit dde2ff0 fixes it to
0xA2). With it, AthenaOS's header becomes 0xc005a200 and its sel word 0x34080000 — BYTE-IDENTICAL to
the working ring (KAT now asserts both exact values). Same bug class as the fw-type: one wrong
constant, everything else correct. (The brief SET_RESOURCES experiment, commit 27332c3, was
disproven by this ring dump — the working KIQ ring carries only MAP_QUEUES — and reverted.)

**But the opcode fix alone did NOT activate the SCHED ring.** Iron after dde2ff0:
```
KIQ  diag (pipe1): ACTIVE=0x1, DOORBELL=0xc0000060 (HIT bit31 set, off 0x18), rptr=0x0
SCHED diag (pipe0): ACTIVE=0x0, rptr=0x0
```
So the gate is now ISOLATED to the KIQ itself: the KIQ HQD is **active**, its doorbell **HIT**
(bit31), AthenaOS pushed a now-byte-correct MAP_QUEUES, the KIQ wptr-poll addr (q.fence+1040) matches
where the push writes the wptr, and `mes_ring_push` rings the doorbell with the dword wptr (7) — yet
**rptr stays 0: the KIQ microengine never fetches its ring.** "Active queue + hit doorbell + correct
wptr, but rptr=0" is not a packet-content bug (we proved the packet) and not a wptr-visibility bug by
code inspection — it's the KIQ's *fetch config*.

### The exact next data needed (do NOT guess more pokes)
Dump the WORKING MES-KIQ HQD registers (me=3, pipe=1) and diff against what AthenaOS programs in
`build_mes_queue_init_register(pipe=1)` — specifically `CP_HQD_PQ_CONTROL` (fetch/rptr-report/
wptr-source bits), `CP_HQD_PQ_DOORBELL_CONTROL`, `CP_HQD_PQ_WPTR_*`, and whether RS64 MES takes the
KIQ wptr from the doorbell vs the poll addr. umr reads me=0 by default, so this needs an explicit
GRBM-context select (or a kprobe on amdgpu's `mes_v11_0_kiq_init_register` / the HQD write path) to
capture the pipe=1 HQD. That register diff is the crack — then: KIQ drains -> SCHED ACTIVE=1 ->
set_hw_resources ACKs -> gfx queue maps -> CP fetches -> SDMA -> draw -> scanout.

### Update 2026-06-28 (3rd pass): full source diff vs Linux — the whole KIQ stage MATCHES
Per owner ("verify the entire pipeline by how Linux does it"). Pulled mes_v11_0.c (torvalds master)
and diffed AthenaOS's MES bring-up function-by-function:
- `mes_v11_0_queue_init_register` (the KIQ HQD write sequence): AthenaOS's `build_mes_queue_init_register`
  is FAITHFUL — CP_HQD_VMID, CP_HQD_PQ_DOORBELL_CONTROL=0 (disable first), CP_MQD_BASE_ADDR/_HI,
  CP_MQD_CONTROL, CP_HQD_PQ_BASE/_HI, RPTR_REPORT_ADDR, PQ_CONTROL, WPTR_POLL_ADDR, real DOORBELL,
  PERSISTENT_STATE, ACTIVE last, GRBM restore. Same regs, same order.
- `mes_v11_0_mqd_init` (the MQD VALUES): AthenaOS's `build_mes_mqd` matches — header 0xC0310800,
  thread_mgmt 0xffffffff×4, misc 0x7, EOP, MQD self-ptr, PQ base>>8, rptr/wptr writeback masks, and
  PQ_CONTROL sets exactly Linux's 6 fields (QUEUE_SIZE, RPTR_BLOCK_SIZE-quirk→0, UNORD_DISPATCH,
  PRIV_STATE, KMD_QUEUE, NO_UPDATE_RPTR), doorbell OFFSET<<2|EN, PERSISTENT_STATE PRELOAD_SIZE=0x55.
- `_DEFAULT` base constants verified: CP_HQD_PQ_CONTROL_DEFAULT=0x00308509 (contributes the working
  value's bits 15/20/21), PERSISTENT_STATE_DEFAULT=0x0be05501 (PRELOAD_SIZE 0x55 baked), MQD_CONTROL,
  EOP, IB — all consistent with the live SCHED PQ_CONTROL readback 0xd830800c (only QUEUE_SIZE differs:
  AthenaOS uses a 64KB SCHED ring -> 13, Linux 32KB -> 12; benign).
- `mes_v11_0_kiq_setting` RLC_CP_SCHEDULERS = 0xE8 — matches.
- Architecture (KIQ direct-init -> KIQ maps SCHED via MAP_QUEUES), legacy-queue-map path
  (sched_version&0xfff = 0x088 >= 0x47 -> KIQ map path applies) — matches.
- Live finding: the working CP_HQD bank for the MES (me=3) reads MOSTLY ZERO (KIQ pipe1 all 0; SCHED
  pipe0 only PQ_CONTROL survives) -> **the MES does NOT keep its queues in the CP_HQD register bank**;
  it manages them internally. So CP_HQD readback is the WRONG lens for the MES, and register-poking is
  a dead end — confirms the source/trace approach.

**So the KIQ stage is byte/sequence-faithful to Linux, yet `KIQ rptr=0` (won't fetch).** The bug is
NOT in the static config. Remaining suspects (need RUNTIME data, not source): (a) the `mes_v11_0_enable`
sequence for the now-PSP-LOADED MES — H2's `let psp_loaded=false` force-direct-load may conflict now
that the fw-type fix makes PSP genuinely load+start the MES (this doc predicted the H2 chain should
"fall away"); (b) doorbell-range/aperture routing for KIQ index 0x18 into the MES; (c) ordering — KIQ
doorbell rung before the engine was ready. NEXT DATA: **ftrace amdgpu's mes_v11_0_* call sequence
during a cold modprobe** (`echo 'mes_v11_0_*' > set_ftrace_filter; function_graph`) to catch any
runtime call/order AthenaOS omits — the static source can't show what the static source doesn't list.

### Update 2026-06-28 (4th pass): ftrace of the cold modprobe — runtime structure MATCHES
Soft-blacklisted amdgpu, armed `function_graph` with `mes_v11_0_*:mod:amdgpu` (+ amdgpu_mes_*),
single cold modprobe. The cold-init MES call tree:
```
mes_v11_0_kiq_hw_init:
  mes_v11_0_enable                    (504us — includes the MES-ready wait)
  mes_v11_0_mqd_init (KIQ)            -> [queue_init_register + kiq_enable_queue: INLINED]
  mes_v11_0_hw_init:                  (entered => sched_version>=0x47, legacy-map path)
    mes_v11_0_mqd_init (SCHED)
    mes_v11_0_set_hw_resources        -> submit_pkt_and_poll_completion -> ring_set_wptr  [COMPLETES]
    mes_v11_0_set_hw_resources_1      -> submit_pkt  (a SECOND hw-resources packet)
    mes_v11_0_query_sched_status      -> submit_pkt
    amdgpu_mes_update_enforce_isolation -> mes_v11_0_misc_op
  amdgpu_mes_map_legacy_queue x N     (gfx/sdma/compute, on the SCHED ring)
  amdgpu_mes_reg_write_reg_wait x N
```
Confirms: (a) structure == AthenaOS's order (enable -> KIQ mqd -> [KIQ maps SCHED] -> hw_init does
set_hw_resources on the now-active SCHED ring -> map_legacy_queue). (b) set_hw_resources COMPLETES in
amdgpu (submitted on the SCHED ring + polled), proving the SCHED ring is live by then — i.e. the
inlined KIQ map WORKED. (c) The KIQ map (queue_init_register + kiq_enable_queue) is INLINED, so the
function-graph can't show the doorbell/wptr write that actually makes the KIQ fetch.

Gaps spotted (downstream of the gate, but real): AthenaOS sends ONE set_hw_resources; amdgpu sends
**set_hw_resources** AND **set_hw_resources_1** then **query_sched_status** then enforce_isolation,
before any map_legacy_queue. Add these once the SCHED ring is live.

THE next data (still the KIQ-fetch gate): kprobe amdgpu's KIQ doorbell/wptr write on a cold modprobe
to capture the EXACT value it rings the KIQ doorbell with (AthenaOS rings dword-count=7 via
mes_ring_push; amdgpu may encode the wptr differently — unit/wrap-bit). Candidates for the gate:
(1) doorbell value/encoding mismatch, (2) a coherency/flush of the KIQ ring or wptr memory amdgpu does
that AthenaOS omits (no HDP/cache flush exists anywhere in raeen_amdgpu; check if the daemon's KIQ-ring
DmaBuf is WB-cached vs UC — if WB, the MES reads a stale/empty ring => rptr=0 fits exactly).

### Update 2026-06-28 (5th pass): doorbell + HDP RULED OUT — bug is a runtime precondition
- HDP/coherency: RULED OUT. Linux `amdgpu_device_flush_hdp` early-returns on `AMD_IS_APU`; Phoenix IS
  an APU, so amdgpu does NOT flush HDP either (mqd_init's flush_hdp call is a no-op here). The APU is
  coherent — the KIQ ring/wptr writes ARE visible to the MES. Not the gate.
- Doorbell encoding: RULED OUT. Linux `mes_v11_0_ring_set_wptr` does `WDOORBELL64(doorbell_index,
  ring->wptr)` — a 64-bit write of the dword wptr. AthenaOS's daemon `ring_doorbell` (amdgpud/src/main.rs:508)
  does exactly that: `writeq(value, doorbell_mmio + byte_offset)`, byte_offset 0x18*4=0x60, value=wptr(7).
  Identical. The KIQ HQD even shows DOORBELL HIT (bit31) — the 64-bit write reached the HQD.

**EXHAUSTIVE verdict: every static + structural thing AthenaOS does matches amdgpu** (fw types, MAP_QUEUES
opcode 0xA2, KIQ HQD register write sequence, MQD field values, _DEFAULT constants, RLC_CP_SCHEDULERS
0xE8, KIQ-maps-SCHED architecture, 64-bit doorbell + offset, HDP n/a, runtime call structure). Yet KIQ
rptr=0. So the gate is a RUNTIME PRECONDITION, not config. Narrowed suspects (need MES-pipe1 runtime
introspection, not more source diffing):
  1. **MES pipe1 (KIQ) not in its service loop.** Heartbeat kiq(pipe1)=0x6038109 proves pipe1 booted
     its ucode + wrote its fw version (an EARLY ucode step), but NOT that it's looping/servicing queues.
     NEXT: read pipe1's INSTR_PNTR (GRBM-select me=3/pipe=1) on AthenaOS — running PC vs stuck — and
     verify CP_MES_CNTL has PIPE1_ACTIVE set (does build_mes_enable_sequence pass pipe1_start=Some?).
  2. **Doorbell-aperture routing of index 0x18 to the MES** in the VFIO-passthrough env (HIT is set, but
     does the MES pipe1 get the wptr update vs just the HQD latching a hit?).
  3. The mes_v11_0_enable 504us **MES-ready WAIT** — does AthenaOS wait for the MES to report ready
     after enable, before KIQ setup? If it proceeds before the MES is looping, the KIQ map is issued to
     a not-yet-servicing engine. (The KIQ HQD/doorbell persist, but the engine never picked it up.)
This is the strongest lead: #1/#3 — the MES pipe1 engine-ready state. The config is PROVEN correct.

### Update 2026-06-28 (6th pass): pipe1/KIQ INSTR_PNTR read — KIQ STALLS EARLY (new distinct point)
Added a pipe1 INSTR_PNTR read to the MES exec probe (commit after 6f2dea0 — first try used a RAW
offset 0x2813<<2 which was WRONG: gc_base[1]!=0 so raw!=resolved, read 0x45f identical for both pipes;
fixed by reading in the exec-probe scope where the RESOLVED offset is derived from mr.cp_mes_gp3_lo).
Cold-boot result (reboot first — FLR won't clear the APU between cold tests; a non-cold boot gave a
short 3866-line log):
```
heartbeat: sched(pipe0)=0x1025088  kiq(pipe1)=0x6038109   (both booted, wrote fw version)
exec probe: pipe0 INSTR_PNTR 0x7204->0x7204,  pipe1/KIQ 0x15be->0x15be   (entry+1 = 0x1401)
```
**The KIQ engine (pipe1) is at 0x15be** — ~445 instructions PAST entry+1 (0x1401), stable, heartbeat
non-zero. So pipe1 booted + ran its early init; it is NOT crash-stuck at entry (the original wall).
But 0x15be is LOW in the ucode vs pipe0's deep service-loop PC 0x7204 — i.e. the KIQ **parks early and
never reaches its queue-service loop**, so it never fetches its ring (rptr=0) even though its HQD is
ACTIVE and the doorbell HIT. This is a NEW, distinct stall point (0x15be), one rung past the fw-type
wall. (Both PCs read non-advancing across 100us, but pipe0 is the known-good engine, so "non-advancing"
just means the sampler catches a parked engine — the KIQ is parked at a much earlier PC than pipe0.)

NEXT (concrete, narrowed): the KIQ stalls ~445 instr into mes1.bin. Two ways to crack it:
(a) DISASSEMBLE mes1.bin (KIQ ucode) around dword 0x15be — what hardware state/register does it poll
    there? (the H3 note at bringup.rs:2777 saw the MES poll a GFX power/clock state). That register is
    the missing precondition. mes1.bin is on the dev box (amdgpu firmware); the entry is uc_start.
(b) Read the WORKING KIQ's INSTR_PNTR to see if 0x15be is ALSO where amdgpu's idle KIQ parks (then it's
    fine and the gate is doorbell-wake) vs deeper (then AthenaOS's KIQ is genuinely stalled). Blocked by
    GFXOFF on the idle working GPU (reads 0) — needs sustained GPU load or a kprobe-timed read.
The load/enable is now PROVEN good enough to run the KIQ past init; the remaining gate is one early
poll-dependency in the KIQ ucode (same CLASS as the fw-type wall, one layer in).

### Update 2026-06-28 (7th pass): 0x15be IS the KIQ's normal idle PC — KIQ is NOT stalled
Booted the WORKING amdgpu with `amdgpu.gfxoff=0` (keeps GFX powered so the MES regs are readable) and
umr-read CP_MES_INSTR_PNTR for pipe1: it reads **0x15be** — the SAME PC as AthenaOS's KIQ. (Reads are
flaky: GFXOFF still intermittently gates even with the param, so most samples are 0, but 0x15be came
through on pipe1.) So **0x15be is the KIQ's NORMAL idle wait-loop PC**, not a stall. AthenaOS's KIQ is
parked exactly where the working KIQ parks when idle. The KIQ engine is HEALTHY.

**Conclusion flip:** the KIQ is not stalled — it idles correctly, but the rung doorbell never WAKES it
to fetch the MAP_QUEUES. So the gate is WORK/WPTR DELIVERY to the idling KIQ, not the engine. The MES
(software scheduler) reads queue wptrs from MEMORY writeback; the HQD doorbell HIT is latched but the
KIQ never acts on it. NEXT (concrete):
  1. After ringing the KIQ doorbell on AthenaOS, read CP_HQD_PQ_WPTR (me=3/pipe=1) — did the doorbell
     write latch the wptr (=7)? If it stays 0, the doorbell isn't delivering the wptr to the HQD.
  2. Check CP_PQ_WPTR_POLL_CNTL.EN (or the per-queue wptr-poll enable): the MES KIQ likely POLLS its
     wptr from the wptr_poll memory (q.fence dw260, where mes_ring_push writes it). amdgpu's mqd_init
     notes the wptr_poll_addr is "only used if CP_PQ_WPTR_POLL_CNTL.EN=1". If AthenaOS never enables
     wptr-poll, the KIQ never sees the memory wptr => never fetches. THIS is the leading candidate —
     enabling wptr-poll (or confirming the doorbell-latch path) is the likely fix.
  3. Trace amdgpu's wptr-poll-cntl / doorbell-monitor setup on a cold modprobe to confirm.
The KIQ ucode disassembly is NO LONGER needed (0x15be proven to be the normal idle PC).

### Update 2026-06-28 (11th pass): DECISIVE — the MES never reads the KIQ ring (scratch unchanged)
Replicated amdgpu's KIQ ring-test (commit fcf2d56): prime CP_MES scratch reg 0xc040 = 0xcafedead via
MMIO, append the byte-identical WRITE_DATA(reg 0xc040 = 0xdeadbeef) to the KIQ ring after MAP_QUEUES,
ring the doorbell, then read 0xc040 back. Iron result: **scratch = 0xcafedead (UNCHANGED)**. So the MES
NEVER processed AthenaOS's KIQ ring — not the MAP_QUEUES, not the WRITE_DATA. This DEFINITIVELY rules
out: MAP_QUEUES content, SCHED MQD, WRITE_DATA format, ring-size, the SCHED-mapping logic. The gate is
PURELY: **the doorbell ring does not trigger the MES pipe1 microengine to fetch/process its ring**,
despite HQD ACTIVE=1 + DOORBELL HIT (0xc0000060) + a healthy idle engine (0x15be) + the doorbell value
now matching (0x100) + the MEC doorbell range set + the ring GART-mapped.

Everything that can be configured is configured identically to the working driver, and the MES still
won't fetch. The doorbell physically reaches the HQD (HIT recorded) but the microengine isn't woken.
**Prime suspect: VFIO/KVM passthrough.** The whole debug loop runs AthenaOS in a KVM-VFIO VM on the
Athena; the bare-metal mmiotrace we compare against is native. The doorbell HIT being set proves the
doorbell-BAR write reaches the GPU's HQD register, BUT the hardware WAKE of the MES microengine on a
doorbell may not fire if KVM traps/emulates the doorbell BAR instead of true-passthrough (HIT set by
the register write, but no doorbell-aperture hardware event to wake the engine). On bare metal the
doorbell aperture path wakes the MES directly.

**THE decisive next test: a `--safe` bare-metal flash of AthenaOS on the Athena (boot natively, not in
the VM).** If the KIQ ring-test scratch flips to 0xdeadbeef on bare metal, the MES DOES drain on real
hardware and the entire stuck-KIQ symptom is a VFIO-VM artifact — meaning the GPU bring-up is far
closer than the VM suggests. If it stays 0xcafedead on bare metal too, it's a real AthenaOS bug and the
doorbell-wake/IH-interrupt arming is the remaining gate. This is the ONE experiment the VFIO loop
cannot run, and the scratch probe (commit fcf2d56) makes its result unambiguous in one boot. It needs
a human flash (the expensive round-trip), but it's now the highest-value next step by far.

### Update 2026-06-28 (10th pass): the mmiotrace IS queryable — KIQ submit decoded
KEY: `docs/gpu-oracle/cold_mmio.txt.gz` (5.9M kernel mmiotrace entries) is a COMPLETE record of every
register write amdgpu made on cold init. BAR5(registers)=0xdc500000, BAR0(VRAM)=0x3e0000000, doorbell
BAR=0xdc000000. Query: `zcat | grep '^W ' | awk '$3>=T0 && $3<=T1 {p=strtonum($5); if(p>=0xdc500000 &&
p<0xdc580000) printf "%s d=0x%x v=%s\n",$3,(p-0xdc500000)/4,$6}'`. dword = (phys-0xdc500000)/4. Verified
the math against CP_MEC_DOORBELL_RANGE (d=0x305c/0x305d = 0x0/0x450, exact).
Decoded the KIQ window (ts ~43.90):
- GRBM_GFX_CNTL = dword 0xa900 (0xc=me3/pipe0, 0xd=me3/pipe1, 0=restore). CP_MES PC = dword 0xc800.
  gc_base[1]=0xa000 (PC absolute 0xc800 = 0xa000+raw 0x2800). The H3 0x28xx writes ARE at absolute
  0x28xx (a different block, written me=0) — AthenaOS matches.
- KIQ HQD setup (GRBM=0xd, ts 43.9075): CP_HQD writes at d=0x320b..0x321a — DOORBELL_CONTROL(0x3218)=
  0x40000060, PQ_CONTROL(0x321a)=0xd8308011 (QUEUE_SIZE=0x11=1MB ring! AthenaOS uses 4KB=9),
  PERSISTENT_STATE(0x320d)=0xbe05501, ACTIVE(0x320b)=1, MQD_BASE(0x3209/a)=0x80_7fca3000. Mostly
  matches AthenaOS; the 1MB-vs-4KB KIQ ring size is the one notable diff.
- DOORBELL VALUES (doorbell BAR 0xdc000000): KIQ doorbell @byte 0x60 rung with **0x100** (256), then
  0x200 (+0x100). SCHED @0x58 in +0x80 (128) steps. So amdgpu pads every KIQ submission to 256 dwords
  and rings the PADDED wptr. AthenaOS rang the raw 7-dword count.
- The ONLY MMIO write between the KIQ HQD setup and the KIQ doorbell: **d=0xc040 v=0xcafedead** (a
  sentinel — amdgpu's ring-test fence, primed right before the doorbell).

FIX TRIED (commit 20f551a): pad kiq_map to 256 dwords (type-2 NOPs) so AthenaOS rings the KIQ doorbell
with 0x100. Iron: APPLIED but KIQ rptr STILL 0. So the doorbell value alone is not the gate either.
NEXT (trace-grounded, NOT guessing): (1) the 0xcafedead@0xc040 scratch — identify the register (umr
dword 0xc040) and whether amdgpu's KIQ ring-test (write sentinel -> doorbell -> poll) is the
interaction the MES needs; (2) the 1MB KIQ ring (QUEUE_SIZE 0x11) vs AthenaOS's 4KB — try matching it;
(3) decode the KIQ ring CONTENT amdgpu writes (the MAP_QUEUES packet body in memory) — but that's a
GTT/VRAM write, check BAR0 region 0x3e0... in the trace. The mmiotrace is the tool; mine it.

### Update 2026-06-28 (8th pass): CP_MEC_DOORBELL_RANGE was missing — fixed, but NOT the wake
Found a real gap: AthenaOS set only the gfx CP_RB_DOORBELL_RANGE [0x458,0x7f8]; it had NO
CP_MEC_DOORBELL_RANGE (the compute/MES-class range). umr on the working amdgpu (gfxoff=0, hammered
past gating) = **CP_MEC_DOORBELL_RANGE [0x0, 0x450]** (offsets 0x1dfc/0x1dfd, umr 0x305c/0x305d - GC
base 0x1260) — covers MES SCHED (0x58) + KIQ (0x60). AthenaOS omitted it entirely. Added it (commit
after d4d0358: regs.rs resolver + GpuOps::cp_mec_doorbell_range_regs + write [0x0,0x450] before the KIQ
doorbell). Iron: the write APPLIED ("CP_MEC_DOORBELL_RANGE = [0x0, 0x450]" logged, not SKIPPED) but the
symptom is UNCHANGED — KIQ rptr=0, SCHED ACTIVE=0, set_hw_resources NO ack. So the MEC doorbell range
is a real correctness fix (keep it) but NOT the wake mechanism for the MES KIQ.

Where this leaves the wake question: the KIQ (MES pipe1) engine is healthy + idle at 0x15be, the
doorbell aperture is enabled, the doorbell HIT is latched in the HQD (0xc0000060), the MEC range now
covers it — yet pipe1 never wakes to drain its ring. Remaining wake candidates (NOT yet tried):
  1. IH (interrupt handler) ring routing — if the MES wakes via a doorbell INTERRUPT through the IH
     ring, and AthenaOS's IH ring isn't routing the MES/CP doorbell interrupt, pipe1 never gets the
     wake event. (AthenaOS programs the IH ring base/rptr/wptr early — verify the doorbell-monitor IRQ
     source is enabled + routed to the MES.)
  2. A per-pipe doorbell/EOP-interrupt enable on the KIQ HQD that AthenaOS's mqd/queue_init omits.
  3. set_hw_resources dependency: maybe the MES only arms its doorbell monitor AFTER set_hw_resources
     — but that's on the SCHED ring (needs the KIQ first). amdgpu breaks this via the KIQ being a
     direct CP_HQD queue serviced by CP/MEC hardware; re-examine whether AthenaOS's KIQ is actually
     a CP/MEC-serviced queue vs a MES-software queue (the ring-fetch trigger differs).
NEXT: trace amdgpu's IH/interrupt + doorbell-monitor setup on a cold modprobe (ftrace amdgpu_irq_* +
the CP/MES interrupt enable), or kprobe what amdgpu does between writing the KIQ HQD and the KIQ
actually draining (the inlined mes_v11_0_kiq_enable_queue → ring_test_helper success path).

### Update 2026-06-28 (9th pass): wake-path candidates RULED OUT against ground truth
Hammered umr on the working amdgpu (gfxoff=0) and checked AthenaOS for each:
- CP_PQ_WPTR_POLL_CNTL = 0 on the working driver → wptr-poll is OFF; the MES is doorbell-driven, not
  poll-driven. So "enable wptr-poll" is NOT the fix (would diverge from amdgpu).
- CP_MES_DOORBELL_CONTROL1-6 EXIST (the MES's own per-doorbell monitor, offsets 0xc83c-0xc841,
  format = DOORBELL_OFFSET[2:27]/EN[30]/HIT[31]) but read 0 on the working driver even hammered (other
  regs gave non-zero past the gating, so these are likely genuinely 0) → not set at bring-up, not it.
- GART/VMID0 ring mapping: SELF-CONSISTENT. gart_va(phys)=phys+gart_delta, gart_delta=
  GFX_GART_APERTURE_BASE(0x7fff_0000_0000) - lo; init_gart_identity maps [APERTURE_BASE,+(hi-lo)] ->
  [lo,hi). Any ring with phys in [lo,hi) is mapped at exactly the VA gart_va emits. VMID0 is live
  (CONTEXT0_CNTL=0x01fffe01, page-table base set). So the KIQ CAN read its ring. Not a GART fault.
So: the KIQ engine is healthy + idle (0x15be), HQD ACTIVE + doorbell HIT, doorbell aperture + MEC range
set, GART maps the ring, wptr written — and it STILL won't drain. Every static/config/mapping cause is
ruled out. The gate is in the dynamic doorbell->MES-pipe1 TRIGGER that I can't see from registers.
THE decisive next step (no more register guessing): on a cold modprobe, kprobe amdgpu right at
mes_v11_0_kiq_enable_queue / amdgpu_ring_test_helper and capture the KIQ ring rptr going 0->N (the MES
draining) + dump the exact register/doorbell writes in that window — the ONE action that triggers the
drain. That's the only thing left that differs and it's invisible to a static/umr comparison.
