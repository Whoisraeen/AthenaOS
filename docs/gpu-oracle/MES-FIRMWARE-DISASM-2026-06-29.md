# MES firmware disassembly — the set_hw_resources stall is a HALTED SCHED pipe (not fault/poll)

## How (reproducible recipe, no special toolchain on Athena)
1. Athena native Linux (default `arch-linux.efi` boot entry, NOT the vfio one) → amdgpu loaded.
2. `zstd -df -o /tmp/gc_11_0_1_mes.bin /lib/firmware/amdgpu/gc_11_0_1_mes.bin.zst`
3. ucode header: common_firmware_header + mes_firmware_header_v1_0. `mes_ucode_offset_bytes`@0x28 = **256**, size@0x24 = 156384. So **file_offset = 256 + PC**.
4. Athena's objdump has NO riscv arch. Use **capstone on the dev box**: `pip install capstone` (5.0.7, has CS_ARCH_RISCV). Disassemble **CS_MODE_RISCV64** (base, 4-byte — the ucode is RV64, proven by `srli rd,rs,0x20` shift-by-32 + the 0xFFFFFFFF mask idiom `addi a4,zero,-1; srli a4,a4,0x20`). Compressed (RISCVC) mode MISDECODES — don't use it.
5. umr register db on Athena `/usr/share/umr/database/ip/gc_11_0_0.reg` has the CP_MES trap-CSR offsets (NOT in the kernel header): MCAUSE_LO=0x281a, MEPC_LO=0x2818, MBADADDR_LO=0x281c, MTVEC_LO=0x2801 (GC seg1).

## The finding
- Working MES idles at INSTR_PNTR **0x7204** (4-aligned) — disassembles to the main dispatch loop (checks a local state==0xa, RMW's local mem at 0xf0100168 via LOCAL aperture).
- AthenaOS SCHED pipe stalls at INSTR_PNTR **0x7656**. The 0x7600 region is CLEAN 4-byte RV64, so 0x7656 (2-aligned) is NOT a real instruction boundary → real PC = **0x7654** = `andi a3, a3, 0` — straight-line arithmetic inside the function at 0x7600 (prologue addi sp,-0x50; bitfield-extract a 64-bit arg at 0x7684: srli 0xb&0xf, srli 5&3, srli 7&7).
- **mcause=mepc=mbadaddr=0 → NO fault.** And 0x7654 is arithmetic (no load/branch to hang on). So the SCHED pipe **HALTED** (stopped advancing) mid-function — it is NOT poll-looping and NOT faulting.
- KIQ pipe1 (same MES, same GFX block) works → not a global power-gate. **Conclusion: pipe0's (SCHED) own HQD/queue state is bad — it runs a few instrs out of idle then halts.** Next: audit the KIQ MAP_QUEUES → SCHED MQD fields (the SCHED ring isn't being cleanly scheduled), and the SCHED HQD activation, vs the working driver.

## Ruled out this session (do not re-chase)
ENG17 TLB invalidate (red herring: 0x2f80000 is a reset constant, present pre-MES). MES fault (mcause=0). GMC gart_enable completeness (program_invalidation/MMHUB-L2/setup_vmid_config all no-change). query_fence (uninit garbage: 0xc0000000→0x5f4c4f4f across boots).
