# amdgpu Phoenix1 (Radeon 760M/780M, GC 11.0.1) blobs — see docs/FIRMWARE.md
#
# Source: linux-firmware repo, amdgpu/ directory (gitlab.com/kernel-firmware/
# linux-firmware). Phoenix1 is PSP 13.0.4 (NOT 13.0.8 — that's Mendocino).
# The canonical list the daemon preflights is ath_amdgpu::bringup::FW_PHOENIX:
#
#   psp_13_0_4_toc.bin   psp_13_0_4_ta.bin   gc_11_0_1_imu.bin
#   gc_11_0_1_me.bin     gc_11_0_1_pfp.bin   gc_11_0_1_mec.bin
#   gc_11_0_1_rlc.bin    gc_11_0_1_mes_2.bin gc_11_0_1_mes1.bin
#   sdma_6_0_1.bin       dcn_3_1_4_dmcub.bin vcn_4_0_2.bin
#
# NOTE: there is no smu_13_0_4.bin — on APUs the SMU/PMFW image lives in the
# system BIOS and the PSP bootloader loads it from there.
#
# License: LICENSE.amdgpu (redistributable, from linux-firmware).
