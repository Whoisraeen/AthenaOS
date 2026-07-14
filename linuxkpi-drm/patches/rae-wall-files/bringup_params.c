// SPDX-License-Identifier: MPL-2.0
/*
 * bringup_params.c — amdgpu module-parameter storage (MPL-2.0, original work).
 *
 * amdgpu reads ~84 module parameters (amdgpu_dpm, amdgpu_dc, amdgpu_vm_size, ...)
 * as plain int/uint globals throughout its init. They are DEFINED in amdgpu_drv.c
 * (the modprobe glue we do not compile — the userspace daemon calls
 * amdgpu_device_init directly), so without these the link auto-stubs them as
 * FUNCTIONS and the code reads a function address as an int, mis-gating behaviour.
 *
 * This provides them as real data with amdgpu upstream's documented defaults
 * (-1 = auto, 0xffffffff = all, etc. — the config contract, not GPL expression),
 * so a headless GFX bring-up gets the same defaults a normal modprobe would. The
 * daemon may override any of these before calling amdgpu_device_init.
 */
#include <linux/types.h>

/* STRING params (module_param_string / charp). These are WORSE than the int
 * params when missing: the auto-stub makes them functions, and amdgpu reads the
 * function address as a char* -> garbage string. amdgpu_device_check_arguments
 * parses amdgpu_lockup_timeout early, so a stub there is FATAL ("invalid
 * lockup_timeout parameter syntax"). Empty/NULL = amdgpu's documented defaults
 * (lockup_timeout empty -> default per-ring timeouts; the others -> feature off). */
char amdgpu_lockup_timeout[256] = { 0 }; /* AMDGPU_MAX_TIMEOUT_PARAM_LENGTH */
char *amdgpu_disable_cu = 0;
char *amdgpu_virtual_display = 0;

/* cg_mask: clock-gating enable mask — u64, all CG blocks enabled by default. A
 * function stub read as a u64 gives a garbage mask (wrong CG blocks toggled). */
u64 amdgpu_cg_mask = 0xffffffffffffffffULL;

/* watchdog_timer is a STRUCT param (not a scalar). The layout MUST match amdgpu.h
 * exactly (bool + uint32_t) so amdgpu reads .period / .timeout_fatal_disable at the
 * right offsets. This TU does not include amdgpu.h, so the local decl is safe. */
struct amdgpu_watchdog_timer {
	bool timeout_fatal_disable;
	uint32_t period;
};
struct amdgpu_watchdog_timer amdgpu_watchdog_timer = { 0, 0x0 };

unsigned int amdgpu_vram_limit = UINT_MAX;
int amdgpu_gart_size = -1;
int amdgpu_gtt_size = -1;
int amdgpu_moverate = -1;
int amdgpu_audio = -1;
int amdgpu_pcie_gen2 = -1;
int amdgpu_msi = -1;
int amdgpu_dpm = -1;
/* Keep upstream's automatic firmware loader choice.  On Athena's Phoenix APU
 * this selects PSP loading, which is the path used by the working Arch amdgpu
 * driver.  Do not force RLC-backdoor autoload here: that bypasses the PSP and
 * SMU IP blocks altogether and makes the resulting device state unlike the
 * production driver we must interoperate with. */
int amdgpu_fw_load_type = -1;
int amdgpu_aspm = -1;
int amdgpu_runtime_pm = -1;
uint amdgpu_ip_block_mask = 0xffffffff;
int amdgpu_bapm = -1;
int amdgpu_vm_size = -1;
int amdgpu_vm_fragment_size = -1;
int amdgpu_vm_block_size = -1;
int amdgpu_vm_update_mode = -1;
int amdgpu_dc = -1;
int amdgpu_sched_jobs = 32;
int amdgpu_sched_hw_submission = 2;
uint amdgpu_pg_mask = 0xffffffff;
uint amdgpu_sdma_phase_quantum = 32;
int amdgpu_enforce_isolation = -1;
int amdgpu_modeset = -1;
uint amdgpu_svm_default_granularity = 9;
uint amdgpu_pp_feature_mask = 0xfff7bfff;
int amdgpu_lbpw = -1;
int amdgpu_compute_multipipe = -1;
int amdgpu_gpu_recovery = -1;
int amdgpu_smu_pptable_id = -1;
uint amdgpu_dc_feature_mask = 2;
int amdgpu_async_gfx_ring = 1;
int amdgpu_mcbp = -1;
int amdgpu_discovery = -1;
int amdgpu_mes_log_enable = 0;
int amdgpu_uni_mes = 1;
int amdgpu_noretry = -1;
int amdgpu_force_asic_type = -1;
int amdgpu_tmz = -1;
int amdgpu_reset_method = -1;
int amdgpu_num_kcq = -1;
int amdgpu_use_xgmi_p2p = 1;
int amdgpu_sg_display = -1;
int amdgpu_user_partt_mode = 0 /* AMDGPU_AUTO_COMPUTE_PARTITION_MODE */;
int amdgpu_seamless = -1;
int amdgpu_agp = -1;
int amdgpu_wbrf = -1;
int amdgpu_damage_clips = -1;
/* rebar DISABLED for AthenaOS bring-up: resizable-BAR reprograms BAR0, which a
 * VFIO-passthrough host (QEMU/KVM) cannot remap (KVM_SET_USER_MEMORY_REGION at a
 * torn-down all-ones BAR base -> abort). Small-BAR mode is a fully-supported
 * amdgpu configuration (systems without rebar run this way) and is fine for the
 * MES/GFX bring-up path; the Phoenix APU uses GTT/system RAM anyway. */
int amdgpu_rebar = 0;
int amdgpu_user_queue = -1;
int amdgpu_ras_enable = -1;
uint amdgpu_ras_mask = 0xffffffff;
int amdgpu_bad_page_threshold = -1;
int amdgpu_si_support = -1;
int amdgpu_cik_support = -1;
int amdgpu_dm_abm_level = -1;
int amdgpu_backlight = -1;

/* zero-initialised params (no explicit default in amdgpu_drv.c) */
int amdgpu_vis_vram_limit;
int amdgpu_disp_priority;
int amdgpu_hw_i2c;
int amdgpu_deep_color;
int amdgpu_vm_fault_stop;
int amdgpu_exp_hw_support;
uint amdgpu_pcie_gen_cap;
uint amdgpu_pcie_lane_cap;
uint amdgpu_force_long_training;
int amdgpu_emu_mode;
uint amdgpu_smu_memory_pool_size;
uint amdgpu_dc_debug_mask;
uint amdgpu_dc_visual_confirm;
int amdgpu_mes;
int amdgpu_mes_kiq;
uint amdgpu_freesync_vid_mode;
int amdgpu_smartshift_bias;
int amdgpu_vcnfw_log;
int amdgpu_umsch_mm;
uint amdgpu_debug_mask;
int amdgpu_umsch_mm_fwlog;
uint amdgpu_hdmi_hpd_debounce_delay_ms;
int amdgpu_no_queue_eviction_on_vm_fault;
int amdgpu_mtype_local;

/* mgpu_info — the global multi-GPU registry DEFINED in amdgpu_drv.c (the modprobe
 * glue we do not compile). amdgpu_register_gpu_instance / amdgpu_unregister_gpu_instance
 * do mutex_lock on &mgpu_info.mutex; the auto-stub emitted it into a read-only
 * region so the lock's cmpxchg WRITE faulted (a page fault in the teardown path).
 * Provide real WRITABLE zeroed storage: a zeroed struct mutex reads as unlocked,
 * which mutex_lock accepts. Sized/aligned generously (>= sizeof the real struct);
 * the compiled callers compute the real field offsets from amdgpu.h. */
__attribute__((aligned(64))) char mgpu_info[8192] = { 0 };
