//! Global Descriptor Table (GDT) and Task State Segment (TSS).
//!
//! Supports per-CPU GDT+TSS for SMP. The BSP uses `init()` which sets up the
//! shared GDT with the BSP's TSS. Each AP gets its own TSS + GDT via
//! `init_ap_percpu(cpu_id)` so every core has a private RSP0 for syscall/IRQ
//! stack switching, and a private IST for double-fault recovery.

use crate::arch::VirtAddr;
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

pub const MAX_CPUS: usize = 64;

/// Per-CPU data: each core owns a TSS (with its own RSP0 and IST stacks)
/// and a GDT that references that TSS. The GDT+TSS must be 'static because
/// LGDT stores a pointer the CPU dereferences on every interrupt/syscall.
struct PerCpuGdt {
    gdt: GlobalDescriptorTable,
    selectors: Selectors,
    tss: &'static TaskStateSegment,
}

/// Per-CPU storage. Index 0 = BSP, 1..N = APs in boot order.
static PER_CPU: Mutex<[Option<&'static PerCpuGdt>; MAX_CPUS]> = Mutex::new([None; MAX_CPUS]);

/// Per-CPU mutable TSS pointers for RSP0 updates during context switches.
/// Indexed by cpu_id. We store raw pointers because the TSS must be mutable
/// from `set_rsp0` but the GDT holds an immutable reference.
static PER_CPU_TSS_PTR: Mutex<[Option<TssPtrWrapper>; MAX_CPUS]> =
    Mutex::new([const { None }; MAX_CPUS]);

// SAFETY: TSS pointers are page-aligned heap allocations never freed.
unsafe impl Send for TssPtrWrapper {}
#[derive(Clone, Copy)]
struct TssPtrWrapper(pub *mut TaskStateSegment);

// ── BSP (CPU 0) uses a static TSS just like before ─────────────────────

lazy_static! {
    static ref BSP_TSS: Mutex<TaskStateSegment> = {
        let mut tss = TaskStateSegment::new();

        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            let stack_start = VirtAddr::from_ptr(core::ptr::addr_of!(STACK));
            stack_start + STACK_SIZE as u64
        };

        tss.privilege_stack_table[0] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            let stack_start = VirtAddr::from_ptr(core::ptr::addr_of!(STACK));
            stack_start + STACK_SIZE as u64
        };

        Mutex::new(tss)
    };
}

lazy_static! {
    pub static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code_selector = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data_selector = gdt.append(Descriptor::kernel_data_segment());

        let user32_code_idx = gdt.append(Descriptor::user_code_segment()).index();
        let user_data_idx = gdt.append(Descriptor::user_data_segment()).index();
        let user_code_idx = gdt.append(Descriptor::user_code_segment()).index();

        let user32_code_selector =
            SegmentSelector::new(user32_code_idx, x86_64::PrivilegeLevel::Ring3);
        let user_data_selector = SegmentSelector::new(user_data_idx, x86_64::PrivilegeLevel::Ring3);
        let user_code_selector = SegmentSelector::new(user_code_idx, x86_64::PrivilegeLevel::Ring3);

        let tss_ptr = &*BSP_TSS.lock() as *const TaskStateSegment;
        let tss_selector = gdt.append(Descriptor::tss_segment(unsafe { &*tss_ptr }));
        (
            gdt,
            Selectors {
                kernel_code_selector,
                kernel_data_selector,
                user32_code_selector,
                user_code_selector,
                user_data_selector,
                tss_selector,
            },
        )
    };
}

#[derive(Debug, Clone, Copy)]
pub struct Selectors {
    pub kernel_code_selector: SegmentSelector,
    pub kernel_data_selector: SegmentSelector,
    pub user32_code_selector: SegmentSelector,
    pub user_code_selector: SegmentSelector,
    pub user_data_selector: SegmentSelector,
    pub tss_selector: SegmentSelector,
}

/// Load the GDT and set CS + SS + TSS on the BSP (CPU 0).
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, FS, GS, SS};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.kernel_code_selector);
        SS::set_reg(GDT.1.kernel_data_selector);
        DS::set_reg(GDT.1.kernel_data_selector);
        ES::set_reg(GDT.1.kernel_data_selector);
        FS::set_reg(GDT.1.kernel_data_selector);
        GS::set_reg(GDT.1.kernel_data_selector);
        load_tss(GDT.1.tss_selector);
    }

    // Record BSP's TSS pointer for set_rsp0.
    let tss_ptr = &*BSP_TSS.lock() as *const TaskStateSegment as *mut TaskStateSegment;
    PER_CPU_TSS_PTR.lock()[0] = Some(TssPtrWrapper(tss_ptr));

    x86_64::registers::model_specific::GsBase::write(VirtAddr::new(0)); // BSP is CPU 0
}

/// Initialize a per-CPU GDT+TSS for an Application Processor and load it.
///
/// Each AP gets a freshly heap-allocated TSS (with its own IST double-fault
/// stack and RSP0) and a GDT that references it. This avoids the "TSS busy"
/// GP fault that occurs when two cores share a single TSS descriptor.
pub fn init_ap_percpu(cpu_id: usize) {
    use alloc::alloc::{alloc_zeroed, Layout};
    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, FS, GS, SS};
    use x86_64::instructions::tables::load_tss;

    assert!(cpu_id > 0 && cpu_id < MAX_CPUS, "cpu_id out of range");

    // Allocate a TSS on the heap. It must be 'static — the CPU holds a live
    // pointer to it for the rest of the system's lifetime.
    let tss_layout = Layout::new::<TaskStateSegment>();
    let tss_ptr = unsafe { alloc_zeroed(tss_layout) as *mut TaskStateSegment };
    assert!(!tss_ptr.is_null(), "TSS allocation failed for AP");

    let tss = unsafe { &mut *tss_ptr };
    *tss = TaskStateSegment::new();

    // Allocate per-AP double-fault IST stack (20 KiB).
    const IST_STACK_SIZE: usize = 4096 * 5;
    let ist_layout = Layout::from_size_align(IST_STACK_SIZE, 16).unwrap();
    let ist_stack = unsafe { alloc_zeroed(ist_layout) };
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
        VirtAddr::new((ist_stack as u64) + IST_STACK_SIZE as u64);

    // Allocate per-AP RSP0 (kernel stack for syscalls/interrupts from Ring 3).
    const RSP0_STACK_SIZE: usize = 4096 * 5;
    let rsp0_layout = Layout::from_size_align(RSP0_STACK_SIZE, 16).unwrap();
    let rsp0_stack = unsafe { alloc_zeroed(rsp0_layout) };
    tss.privilege_stack_table[0] = VirtAddr::new((rsp0_stack as u64) + RSP0_STACK_SIZE as u64);

    // Build a per-AP GDT that references this TSS.
    let tss_ref: &'static TaskStateSegment = unsafe { &*tss_ptr };
    let gdt_layout = Layout::new::<PerCpuGdt>();
    let percpu_ptr = unsafe { alloc_zeroed(gdt_layout) as *mut PerCpuGdt };
    let percpu = unsafe { &mut *percpu_ptr };

    let mut gdt = GlobalDescriptorTable::new();
    let kcs = gdt.append(Descriptor::kernel_code_segment());
    let kds = gdt.append(Descriptor::kernel_data_segment());
    let u32cs_idx = gdt.append(Descriptor::user_code_segment()).index();
    let uds_idx = gdt.append(Descriptor::user_data_segment()).index();
    let ucs_idx = gdt.append(Descriptor::user_code_segment()).index();
    let u32cs = SegmentSelector::new(u32cs_idx, x86_64::PrivilegeLevel::Ring3);
    let uds = SegmentSelector::new(uds_idx, x86_64::PrivilegeLevel::Ring3);
    let ucs = SegmentSelector::new(ucs_idx, x86_64::PrivilegeLevel::Ring3);
    let tss_sel = gdt.append(Descriptor::tss_segment(tss_ref));

    percpu.gdt = gdt;
    percpu.selectors = Selectors {
        kernel_code_selector: kcs,
        kernel_data_selector: kds,
        user32_code_selector: u32cs,
        user_code_selector: ucs,
        user_data_selector: uds,
        tss_selector: tss_sel,
    };
    percpu.tss = tss_ref;

    let percpu_ref: &'static PerCpuGdt = unsafe { &*percpu_ptr };
    PER_CPU.lock()[cpu_id] = Some(percpu_ref);
    PER_CPU_TSS_PTR.lock()[cpu_id] = Some(TssPtrWrapper(tss_ptr));

    // Load this CPU's GDT and TSS.
    percpu_ref.gdt.load();
    unsafe {
        CS::set_reg(percpu_ref.selectors.kernel_code_selector);
        SS::set_reg(percpu_ref.selectors.kernel_data_selector);
        DS::set_reg(percpu_ref.selectors.kernel_data_selector);
        ES::set_reg(percpu_ref.selectors.kernel_data_selector);
        FS::set_reg(percpu_ref.selectors.kernel_data_selector);
        GS::set_reg(percpu_ref.selectors.kernel_data_selector);
        load_tss(percpu_ref.selectors.tss_selector);
    }

    x86_64::registers::model_specific::GsBase::write(VirtAddr::new(cpu_id as u64));
}

/// Load the BSP's GDT on an AP without loading a TSS. Used as a fallback
/// if `init_ap_percpu` hasn't been called yet (e.g. during early trampoline
/// bringup before the heap is accessible on the AP).
pub fn init_ap() {
    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, FS, GS, SS};

    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.kernel_code_selector);
        SS::set_reg(GDT.1.kernel_data_selector);
        DS::set_reg(GDT.1.kernel_data_selector);
        ES::set_reg(GDT.1.kernel_data_selector);
        FS::set_reg(GDT.1.kernel_data_selector);
        GS::set_reg(GDT.1.kernel_data_selector);
    }
}

/// Reload the BSP's TSS after S3 resume. The CPU reset cleared TR, but the
/// GDT in memory still marks the TSS descriptor BUSY from the boot-time
/// `ltr` — reloading it as-is would #GP. Clear the busy bit (64-bit TSS type
/// 0xB → available 0x9, i.e. descriptor bit 41) directly in the live GDT
/// (located via SGDT so this follows whatever GDTR the resume path restored),
/// then `ltr`. Interrupts must be disabled by the caller (they are — the
/// resume path runs fully masked); without TR, the first IRQ/exception would
/// have no RSP0/IST and double-fault.
pub fn reload_bsp_tss_after_resume() {
    use x86_64::instructions::tables::{load_tss, sgdt};
    let sel = GDT.1.tss_selector;
    unsafe {
        let gdtr = sgdt();
        let entry = (gdtr.base.as_u64() + (sel.index() as u64) * 8) as *mut u64;
        let lo = entry.read_volatile();
        entry.write_volatile(lo & !(1u64 << 41)); // busy → available
        load_tss(sel);
    }
}

/// Update RSP0 in the current CPU's TSS. Called during context switches to
/// point at the new task's kernel stack so interrupts/syscalls land safely.
pub fn set_rsp0(stack_end: VirtAddr) {
    set_rsp0_for_cpu(current_cpu_id(), stack_end);
}

/// Update RSP0 for a specific CPU. The caller must hold the scheduler lock
/// or have interrupts disabled.
pub fn set_rsp0_for_cpu(cpu_id: usize, stack_end: VirtAddr) {
    let guard = PER_CPU_TSS_PTR.lock();
    if let Some(wrapper) = guard[cpu_id] {
        unsafe {
            (*wrapper.0).privilege_stack_table[0] = stack_end;
        }
    } else {
        BSP_TSS.lock().privilege_stack_table[0] = stack_end;
    }
}

/// Returns the logical CPU id for the current core.
///
/// Two sources, in order, to stay correct now that AthBridge guests own their
/// user-visible GS base (the Win32 TEB, via `SYS_SET_GS_BASE`):
///
/// 1. **Active GS base, if it is a small integer (`< MAX_CPUS`).** This is the
///    common case: kernel code (kernel threads, syscall Rust between the
///    handler's swapgs pair, and the post-context-switch return window) runs
///    with the small-integer CPU id parked in the active GS base by
///    `gdt::init`/`init_ap_percpu`. No swapgs has moved it, so it is the id.
///    It is also the only valid source during early boot before
///    `init_percpu_syscall` has programmed `IA32_KERNEL_GS_BASE`.
///
/// 2. **Kernel-GS per-CPU block (`syscall::current_cpu_id_from_kernel_gs`).**
///    Reached only when the active GS base is a large value — i.e. we are in an
///    interrupt handler that preempted a AthBridge guest whose active GS base
///    is the TEB. The x86-interrupt handlers don't swapgs, so the per-CPU
///    pointer is parked in `IA32_KERNEL_GS_BASE`; we read the authoritative
///    `cpu_id` field from it. This is the path that keeps SMP correct under
///    guest-controlled user GS bases.
///
/// Final clamp to 0 if neither source yields a sane id (stray non-canonical GS
/// base) — same defense-in-depth as before.
pub fn current_cpu_id() -> usize {
    let active = x86_64::registers::model_specific::GsBase::read().as_u64() as usize;
    if active < MAX_CPUS {
        return active;
    }
    if let Some(id) = crate::syscall::current_cpu_id_from_kernel_gs() {
        return id;
    }
    0
}
