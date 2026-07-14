use crate::arch::VirtAddr;
use core::sync::atomic::{AtomicUsize, Ordering};

// 64 KiB. The synchronous RaeFS path stacks multiple BLOCK_SIZE (4 KiB)
// buffers per frame (write_at -> insert_extent -> insert_into_node (recursive)
// -> allocate_block), which overflowed a 16 KiB stack and silently corrupted
// saved registers (there is no guard page on heap-backed kernel stacks).
const KERNEL_STACK_SIZE: usize = 4096 * 16; // 64 KiB

// ---- Native user-stack layout (new_elf_with_pty) -------------------------
// TOP is one page below the 47-bit canonical user ceiling; the mapped region
// grows DOWN from there. 256 pages (1 MiB) so real raekit apps' startup frames
// fit. The old 5-page (20 KiB) map crashed Files at 0x7FFF_FFFF_9F68.
/// Number of 4 KiB pages mapped for a native task's initial user stack.
pub const NATIVE_USER_STACK_PAGES: usize = 256; // 1 MiB
/// Highest mapped user-stack address (exclusive); initial RSP descends from here.
pub const NATIVE_USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
/// Lowest mapped user-stack address (inclusive).
pub const NATIVE_USER_STACK_BASE: u64 =
    NATIVE_USER_STACK_TOP - (NATIVE_USER_STACK_PAGES as u64 * 4096);

// Compile-time invariants. Break any of these and the build fails — this is the
// FAIL-able guard for the "Files crashes on launch / 20 KiB stack" regression.
const _: () = {
    // 47-bit canonical: top must stay below the user-half ceiling.
    assert!(NATIVE_USER_STACK_TOP < 0x0000_8000_0000_0000);
    // Base must remain a valid canonical user address (bit 47 clear).
    assert!(NATIVE_USER_STACK_BASE < 0x0000_8000_0000_0000);
    assert!(NATIVE_USER_STACK_BASE < NATIVE_USER_STACK_TOP);
    // At least 1 MiB of stack for real app frames (regression floor).
    assert!(NATIVE_USER_STACK_TOP - NATIVE_USER_STACK_BASE >= 1024 * 1024);
    // Page-aligned span.
    assert!(NATIVE_USER_STACK_TOP & 0xFFF == 0);
    assert!(NATIVE_USER_STACK_BASE & 0xFFF == 0);
};

/// FAIL-able runtime check of the native user-stack layout, callable from a boot
/// smoketest. Verifies the mapped span size and that the initial RSP handed to a
/// fresh relibc task lands strictly INSIDE the mapped region (not above the top,
/// not below the base). Returns `Err` with a reason on any violation.
pub fn native_stack_layout_selfcheck() -> Result<(), &'static str> {
    let span = NATIVE_USER_STACK_TOP - NATIVE_USER_STACK_BASE;
    if span < 1024 * 1024 {
        return Err("native user stack < 1 MiB");
    }
    let rsp = crate::native_stack::setup_relibc_stack(NATIVE_USER_STACK_TOP);
    if rsp >= NATIVE_USER_STACK_TOP {
        return Err("initial RSP at/above stack top");
    }
    if rsp < NATIVE_USER_STACK_BASE {
        return Err("initial RSP below stack base (unmapped)");
    }
    Ok(())
}

/// 512-byte FXSAVE area for x87 + SSE state, 16-byte aligned.
#[repr(C, align(16))]
pub struct FxSaveArea {
    pub data: [u8; 512],
}

impl FxSaveArea {
    pub const fn new() -> Self {
        FxSaveArea { data: [0u8; 512] }
    }
}

impl core::fmt::Debug for FxSaveArea {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("FxSaveArea([..512])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(usize);

impl TaskId {
    pub fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        TaskId(NEXT_ID.fetch_add(1, Ordering::SeqCst))
    }

    /// Raw u64 form used by the syscall ABI (target task in rdi, etc.).
    pub const fn raw(self) -> u64 {
        self.0 as u64
    }
    /// Reconstruct a TaskId from a userspace-supplied u64. Validity (whether
    /// the task actually exists) is checked by the scheduler when this is used.
    pub const fn from_raw(n: u64) -> Self {
        TaskId(n as usize)
    }
    /// Sentinel value for kernel-owned resources (e.g. compositor desktop surface).
    pub const fn kernel_sentinel() -> Self {
        TaskId(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPriority {
    Normal,
    Game,
}

/// Deadline scheduling parameters for SCHED_GAME hard real-time tasks.
/// Attached to Game-priority tasks that need guaranteed timing (compositor, audio, game threads).
#[derive(Debug, Clone, Copy)]
pub struct DeadlineTask {
    pub period_us: u64,
    pub deadline_us: u64,
    pub runtime_us: u64,
    pub last_wake: u64,
    pub last_finish: u64,
    pub deadline_misses: u64,
    pub total_invocations: u64,
    pub absolute_deadline: u64,
}

impl DeadlineTask {
    pub fn new(period_us: u64, deadline_us: u64, runtime_us: u64) -> Self {
        Self {
            period_us,
            deadline_us,
            runtime_us,
            last_wake: 0,
            last_finish: 0,
            deadline_misses: 0,
            total_invocations: 0,
            absolute_deadline: 0,
        }
    }

    /// Start a new deadline period at `now_us`.
    pub fn wake(&mut self, now_us: u64) {
        self.last_wake = now_us;
        self.absolute_deadline = now_us + self.deadline_us;
        self.total_invocations += 1;
    }

    /// Record that the task finished its work for this period.
    pub fn finish(&mut self, now_us: u64) {
        self.last_finish = now_us;
    }

    /// Returns true if the current period's deadline was missed.
    pub fn check_miss(&mut self, now_us: u64) -> bool {
        if self.last_wake > 0
            && self.last_finish < self.last_wake
            && now_us > self.absolute_deadline
        {
            self.deadline_misses += 1;
            true
        } else {
            false
        }
    }

    /// True if this task should start a new period (enough time has passed since last wake).
    pub fn needs_new_period(&self, now_us: u64) -> bool {
        self.last_wake == 0 || now_us.saturating_sub(self.last_wake) >= self.period_us
    }
}

/// CPU affinity bitmask for per-task core pinning.
#[derive(Debug, Clone, Copy)]
pub struct CpuAffinity {
    pub mask: u64,
}

impl CpuAffinity {
    /// Allow all CPUs.
    pub const fn all() -> Self {
        CpuAffinity { mask: u64::MAX }
    }

    pub fn from_mask(mask: u64) -> Self {
        CpuAffinity { mask }
    }

    pub fn is_allowed(&self, cpu: u32) -> bool {
        cpu < 64 && (self.mask & (1u64 << cpu)) != 0
    }

    /// Performance cores (0-3) — default for game threads.
    pub fn performance_cores() -> Self {
        CpuAffinity { mask: 0x0F }
    }

    /// Efficiency cores (4-7) — default for background in game mode.
    pub fn efficiency_cores() -> Self {
        CpuAffinity { mask: 0xF0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    BlockedOnReceive(usize), // Blocked waiting for a message on chan_id
    BlockedOnSend(usize),    // Blocked waiting for space in chan_id
    BlockedOnIrq(u8),        // Blocked waiting for a hardware IRQ vector
    BlockedOnWait(TaskId),   // Blocked waiting for a child process to exit
    BlockedOnVirtio(u16),    // Blocked waiting for a virtio block request
    BlockedOnFutex(u64),     // Blocked on a futex, keyed by shared-frame phys addr
    BlockedOnDrm(u64),       // Blocked waiting for amdgpud request id
    Zombie(u64),             // Process terminated, exit code retained for wait()
}

impl TaskState {
    /// True for every `BlockedOn*` variant — the set of states that
    /// `block_current_task_with` puts a task into and `unblock_*` /
    /// `wake_thread*` wake up. Routes `finish_task_switch`:
    /// `is_blocked() == true`  → stashed task → `blocked_tasks` (still asleep)
    /// `is_blocked() == false` → stashed task → runqueue (yielded or wake-raced)
    #[inline]
    pub fn is_blocked(self) -> bool {
        matches!(
            self,
            TaskState::BlockedOnReceive(_)
                | TaskState::BlockedOnSend(_)
                | TaskState::BlockedOnIrq(_)
                | TaskState::BlockedOnWait(_)
                | TaskState::BlockedOnVirtio(_)
                | TaskState::BlockedOnFutex(_)
                | TaskState::BlockedOnDrm(_)
        )
    }
}

#[derive(Debug)]
pub struct Task {
    pub id: TaskId,
    pub stack_ptr: VirtAddr,
    /// Per-task capability table for MMIO/IRQ/Port + new Channel caps.
    pub cap_table: crate::capability::CapTable,
    pub state: TaskState,
    pub priority: TaskPriority,
    stack_base: *mut u8,
    pub pml4: Option<x86_64::structures::paging::PhysFrame>,
    pub is_idle: bool,
    pub parent_id: Option<TaskId>,
    pub exit_code: Option<u64>,
    pub fds: [Option<alloc::sync::Arc<spin::Mutex<crate::vfs::File>>>; 32],
    /// CFS virtual runtime in nanoseconds. Lower = runs sooner.
    /// Game-priority tasks ignore this field.
    pub vruntime: u64,
    /// Hard deadline parameters (for Game-priority tasks with real-time constraints).
    pub deadline: Option<DeadlineTask>,
    /// CPU affinity mask — which cores this task is allowed to run on.
    pub affinity: CpuAffinity,
    /// Set by game-mode throttling: limits background tasks to 5% CPU.
    pub throttled: bool,
    /// Per-task FPU/SSE state, saved/restored on context switch via FXSAVE64/FXRSTOR64.
    pub fpu_state: FxSaveArea,
    /// Ring buffer of PS/2 scancodes delivered by the keyboard IRQ handler.
    pub key_buf: KeyRingBuf,
    /// Ring buffer of mouse events delivered by the mouse IRQ handler.
    pub mouse_buf: MouseRingBuf,
    /// PTY slave id when this task was spawned attached to a pseudo-terminal.
    pub pty_slave_id: Option<u32>,
    /// User-mode FS segment base (x86_64 TLS pointer). INHERITED from the
    /// live IA32_FS_BASE at task creation (Unix-style), then overwritten by
    /// Linux-ABI `arch_prctl(ARCH_SET_FS)` or native syscall 126; restored
    /// into the MSR whenever this task is switched in. The kernel itself
    /// never uses FS and user code cannot write FS base directly
    /// (CR4.FSGSBASE stays clear), so this field tracks the truth — every
    /// switch site compares outgoing vs incoming FIELDS and skips the MSR
    /// write when equal (zero writes until some task actually sets a TLS).
    /// Inheriting instead of zeroing matters: pre-TLS boot residue in the
    /// MSR is load-bearing for early userspace (raebridge stall, 2026-06-10).
    /// GS is virtualized per-task too now (see `gs_base`): the CPU id moved off
    /// the user-visible GS base into the kernel-GS per-CPU block, so a Win32
    /// guest can own its GS base. `arch_prctl(ARCH_SET_GS)` stays refused for
    /// the Linux ABI (Linux libcs use FS); Win32 uses the native SYS_SET_GS_BASE.
    pub fs_base: u64,
    /// Win32 TEB pointer for RaeBridge guests (the user-visible GS base).
    /// 0 for native/Linux tasks. Saved/restored across context switches like
    /// `fs_base` (see scheduler.rs) so a guest's `gs:[0x30]` TEB self-pointer
    /// survives reschedules. `SYS_SET_GS_BASE` (282) writes it. Unlike
    /// `fs_base` this does NOT inherit the live GS MSR — the live GS base is
    /// the kernel per-CPU pointer mid-init, never a TEB, so inheriting would
    /// be wrong. Default 0 = "this task has no Win32 TEB".
    pub gs_base: u64,
    /// Scheduler tick (ms) at which this task was last picked to run. The
    /// work-stealing scan only steals a "cold" task (one not run for the last
    /// few ticks); a "hot" task that just ran is left on its CPU. This breaks
    /// the steal-thrash livelock where two bursty service threads (e.g. the
    /// xHCI HID servicer) ping-pong between cores every tick. 0 = never run
    /// (eligible to steal as fresh backlog). MasterChecklist 4.8.
    pub last_ran_tick: u64,
    /// Core this task last ran on (cache affinity). `select_cpu` sends a waking
    /// task back HERE rather than defaulting to CPU0, so a periodic thread keeps
    /// one home core across sleep/wake cycles instead of being re-homed to CPU0
    /// and re-stolen by an idle AP every wake — the dominant steal-thrash cause
    /// (491 steals / 491 picks in one boot). `u32::MAX` = no home yet.
    pub last_cpu: u32,
    /// True for a `CLONE_THREAD` thread that SHARES another task's address space
    /// (`pml4`). The shared PML4 is reference-counted (see `memory::as_incref` /
    /// `as_decref`), so it is freed only when the last sharer exits.
    pub shares_address_space: bool,
    /// `clear_child_tid`: the user-space address (in this task's AS) the kernel
    /// must zero + futex-wake when this task exits, set by
    /// `CLONE_CHILD_CLEARTID` at clone or `set_tid_address(2)`. This is what
    /// `pthread_join` blocks on — the joiner futex-waits on the joinee's TID
    /// slot, and the joinee's exit clears it and wakes the joiner. 0 = unset.
    pub clear_child_tid: u64,
}

impl Task {
    /// Record that this task was just picked to run on `cpu` at scheduler
    /// `tick`. `last_ran_tick` gates work-stealing (only COLD tasks are stolen);
    /// `last_cpu` drives cache-affinity placement in `select_cpu`.
    pub fn mark_scheduled(&mut self, cpu: usize, tick: u64) {
        self.last_ran_tick = tick;
        self.last_cpu = cpu as u32;
    }
}

/// The live IA32_FS_BASE — new tasks inherit the creator's FS base.
fn inherited_fs_base() -> u64 {
    x86_64::registers::model_specific::FsBase::read().as_u64()
}

/// Fixed-capacity ring buffer for keyboard scancodes (lock-free single-producer append).
#[derive(Debug)]
pub struct KeyRingBuf {
    buf: [u8; 64],
    head: usize,
    tail: usize,
}

impl KeyRingBuf {
    pub const fn new() -> Self {
        Self {
            buf: [0; 64],
            head: 0,
            tail: 0,
        }
    }
    pub fn push(&mut self, scancode: u8) {
        let next = (self.head + 1) % self.buf.len();
        if next == self.tail {
            return;
        }
        self.buf[self.head] = scancode;
        self.head = next;
    }
    pub fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail {
            return None;
        }
        let v = self.buf[self.tail];
        self.tail = (self.tail + 1) % self.buf.len();
        Some(v)
    }
}

/// Packed mouse event: dx (i16) + dy (i16) + buttons (u8).
#[derive(Debug, Clone, Copy)]
pub struct PackedMouseEvent {
    pub dx: i16,
    pub dy: i16,
    pub buttons: u8,
}

impl PackedMouseEvent {
    pub fn to_u64(self) -> u64 {
        (self.buttons as u64) | ((self.dx as u16 as u64) << 8) | ((self.dy as u16 as u64) << 24)
    }
}

/// Fixed-capacity ring buffer for mouse events.
#[derive(Debug)]
pub struct MouseRingBuf {
    buf: [PackedMouseEvent; 32],
    head: usize,
    tail: usize,
}

impl MouseRingBuf {
    pub const fn new() -> Self {
        Self {
            buf: [PackedMouseEvent {
                dx: 0,
                dy: 0,
                buttons: 0,
            }; 32],
            head: 0,
            tail: 0,
        }
    }
    pub fn push(&mut self, ev: PackedMouseEvent) {
        let next = (self.head + 1) % self.buf.len();
        if next == self.tail {
            return;
        }
        self.buf[self.head] = ev;
        self.head = next;
    }
    pub fn pop(&mut self) -> Option<PackedMouseEvent> {
        if self.head == self.tail {
            return None;
        }
        let v = self.buf[self.tail];
        self.tail = (self.tail + 1) % self.buf.len();
        Some(v)
    }
}

unsafe impl Send for Task {}

impl Task {
    pub fn kernel_stack_end(&self) -> VirtAddr {
        VirtAddr::new((self.stack_base as usize + KERNEL_STACK_SIZE) as u64)
    }

    /// True iff `stack_ptr` (the saved kernel RSP this task will resume on)
    /// points strictly inside this task's OWN kernel stack — the precondition
    /// for context-switching INTO it. A task whose saved SP is 0 or out of
    /// range would resume on a garbage stack: `switch_context` does
    /// `mov rsp, <stack_ptr>` then pops/`ret`, so an `rsp=0` resume faults at
    /// ~`-8` (`cr2=0xff..f8`) — the intermittent SMP work-stealing #DF
    /// (MasterChecklist 4.8). The scheduler calls this on EVERY `pick_next`
    /// result and refuses to switch to a task that fails it, turning a silent
    /// double fault into a loud, recoverable log line. For a live, correctly
    /// descheduled task this is always true (the SP was just saved by
    /// `switch_context` or built in-range at creation), so it is a no-op on the
    /// happy path and only fires on a corrupt/duplicate Task.
    pub fn saved_stack_is_sane(&self) -> bool {
        let sp = self.stack_ptr.as_u64();
        let end = self.kernel_stack_end().as_u64();
        let base = end.saturating_sub(KERNEL_STACK_SIZE as u64);
        // Explicit `sp != 0` first. The range test already rejects 0 for a
        // well-formed task (`0 > base` is false for a high-half kstack), but a
        // task with a transiently-zeroed/uninitialized `stack_base` would make
        // `base == 0` and let a tiny non-zero `sp` slip — and rsp=0 is the exact
        // signature of the intermittent SMP steal-resume #DF (push to NULL →
        // cr2=0xff..f8). Be unmissable about it. (MasterChecklist 4.8.)
        sp != 0 && sp > base && sp < end
    }

    pub fn pop_key(&mut self) -> Option<u8> {
        self.key_buf.pop()
    }

    pub fn push_key(&mut self, scancode: u8) {
        self.key_buf.push(scancode);
    }

    pub fn push_mouse(&mut self, dx: i16, dy: i16, buttons: u8) {
        self.mouse_buf.push(PackedMouseEvent { dx, dy, buttons });
    }

    pub fn pop_mouse_packed(&mut self) -> Option<u64> {
        self.mouse_buf.pop().map(|ev| ev.to_u64())
    }

    /// Creates a new kernel thread.
    pub fn new(entry_point: extern "C" fn(), parent_id: Option<TaskId>) -> Self {
        let (stack_base, stack_end) = crate::memory::alloc_kernel_stack(KERNEL_STACK_SIZE);
        let mut rsp = stack_end.as_u64();

        unsafe {
            // Stack frame expected by switch_context (top to bottom):
            //   RIP, rbx, rbp, r12, r13, r14, r15
            //
            // We use kernel_thread_entry as the return address.  It does
            // `sti; call r12; exit_current_task(0)` so the real entry runs
            // with interrupts enabled and the task exits cleanly if it returns.
            rsp -= 8;
            core::ptr::write(
                rsp as *mut u64,
                crate::context::kernel_thread_entry as usize as u64,
            );
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, 0); // rbx
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, 0); // rbp
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, entry_point as usize as u64); // r12 = real entry
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, 0); // r13
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, 0); // r14
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, 0); // r15
        }

        Task {
            id: TaskId::new(),
            stack_ptr: VirtAddr::new(rsp),
            cap_table: crate::capability::CapTable::new(),
            state: TaskState::Ready,
            priority: TaskPriority::Normal,
            stack_base,
            pml4: None,
            is_idle: false,
            parent_id,
            exit_code: None,
            fds: Default::default(),
            vruntime: 0,
            deadline: None,
            affinity: CpuAffinity::all(),
            throttled: false,
            fpu_state: FxSaveArea::new(),
            key_buf: KeyRingBuf::new(),
            mouse_buf: MouseRingBuf::new(),
            pty_slave_id: None,
            fs_base: inherited_fs_base(),
            gs_base: 0,
            last_ran_tick: 0,
            last_cpu: u32::MAX,
            shares_address_space: false,
            clear_child_tid: 0,
        }
    }

    /// Creates a new user-mode thread.
    pub fn new_user(entry_point: extern "C" fn(), parent_id: Option<TaskId>) -> Self {
        use x86_64::structures::paging::{FrameAllocator, Page, PageTableFlags, Size4KiB};

        let (kernel_stack_base, kernel_stack_end) =
            crate::memory::alloc_kernel_stack(KERNEL_STACK_SIZE);
        // §10.3: kernel stack allocated ABOVE, BEFORE the new user space, so the
        // kernel-half clone captures the stack mapping. 1.5f: create_new_pml4 ->
        // arch::mmu::new_user (delegating; same Root, order preserved).
        let pml4 = crate::arch::mmu::new_user();
        crate::memory::as_incref(pml4); // refcount this address space (see Drop)
        let mut rsp = kernel_stack_end.as_u64();

        // 2. Allocate and map User Stack in the new PML4
        let user_stack_virt_base = VirtAddr::new(0x0000_7FFF_FFFF_A000);
        let mut global_alloc = crate::memory::GlobalFrameAllocator;
        // W^X: the stack is data — never legitimately executed. NO_EXECUTE
        // defeats stack-buffer-overflow → shellcode. (Audit 2026-07-06 #4.)
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE;

        for i in 0..5 {
            let frame = global_alloc
                .allocate_frame()
                .expect("Failed to allocate user stack frame");
            let page =
                Page::<Size4KiB>::containing_address(user_stack_virt_base + (i * 4096) as u64);
            unsafe {
                let _ = crate::memory::map_page_in_pml4_fallible(pml4, page, frame, flags);
            }
        }

        let user_stack_end = user_stack_virt_base + (5 * 4096) as u64;

        unsafe {
            // Stack frame: RIP, rbx(=user RIP), rbp(=user RSP), r12..r15
            rsp -= 8;
            core::ptr::write(
                rsp as *mut u64,
                crate::context::thread_entry_user as *const () as usize as u64,
            );
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, entry_point as usize as u64); // rbx = user RIP
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, user_stack_end.as_u64()); // rbp = user RSP
            for _ in 0..4 {
                rsp -= 8;
                core::ptr::write(rsp as *mut u64, 0);
            }
        }

        Task {
            id: TaskId::new(),
            stack_ptr: VirtAddr::new(rsp),
            cap_table: crate::capability::CapTable::new(),
            state: TaskState::Ready,
            priority: TaskPriority::Normal,
            stack_base: kernel_stack_base,
            pml4: Some(pml4),
            is_idle: false,
            parent_id,
            exit_code: None,
            fds: Default::default(),
            vruntime: 0,
            deadline: None,
            affinity: CpuAffinity::all(),
            throttled: false,
            fpu_state: FxSaveArea::new(),
            key_buf: KeyRingBuf::new(),
            mouse_buf: MouseRingBuf::new(),
            pty_slave_id: None,
            fs_base: inherited_fs_base(),
            gs_base: 0,
            last_ran_tick: 0,
            last_cpu: u32::MAX,
            shares_address_space: false,
            clear_child_tid: 0,
        }
    }

    /// Creates a new user-mode thread from an ELF binary.
    pub fn new_elf(elf_data: &[u8], parent_id: Option<TaskId>) -> Result<Self, &'static str> {
        Self::new_elf_with_pty(elf_data, parent_id, None)
    }

    /// Creates a new ELF task optionally bound to a PTY slave endpoint.
    pub fn new_elf_with_pty(
        elf_data: &[u8],
        parent_id: Option<TaskId>,
        pty_slave_id: Option<u32>,
    ) -> Result<Self, &'static str> {
        use x86_64::structures::paging::{FrameAllocator, Page, PageTableFlags, Size4KiB};

        let (kernel_stack_base, kernel_stack_end) =
            crate::memory::alloc_kernel_stack(KERNEL_STACK_SIZE);
        // §10.3: kernel stack first, THEN the user space (the clone captures it).
        // 1.5f: create_new_pml4 -> arch::mmu::new_user (delegating; same Root).
        let pml4 = crate::arch::mmu::new_user();
        crate::memory::as_incref(pml4); // refcount this address space (see Drop)
        let elf = crate::elf::ElfBinary::new(elf_data)?;
        let (entry_point, phoff, phentsize, phnum) = elf.load_into_pml4(pml4)?;
        let mut rsp = kernel_stack_end.as_u64();

        // Native user stack. The TOP is anchored at 0x7FFF_FFFF_F000 (one page
        // below the 47-bit canonical user ceiling) and the mapped region grows
        // DOWN from there. The old layout mapped only 5 pages (20 KiB), which is
        // enough for the minimal `hello_window` control app but NOT for a real
        // raekit app: Files' crt0/relibc/raekit startup frames overflowed the
        // 20 KiB span and faulted at 0x7FFF_FFFF_9F68 (~152 B below the old base
        // 0x7FFF_FFFF_A000) → "Killing faulting task". There is no demand stack
        // growth (the page-fault handler kills user faults), so the initial map
        // must be generous. 256 pages = 1 MiB covers real app startup.
        // Per-page frames are correct here: a stack is N independent VA→PA
        // mappings, not a single contiguous physical span (pitfall #7 is about
        // DMA buffers treated as one frame, which this is not).
        const STACK_PAGES: usize = NATIVE_USER_STACK_PAGES; // 1 MiB
        let user_stack_top = VirtAddr::new(NATIVE_USER_STACK_TOP);
        let user_stack_virt_base = VirtAddr::new(NATIVE_USER_STACK_BASE);
        let mut global_alloc = crate::memory::GlobalFrameAllocator;
        // W^X: the stack is data — never legitimately executed. NO_EXECUTE
        // defeats stack-buffer-overflow → shellcode. (Audit 2026-07-06 #4.)
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE;

        let mut stack_frames = alloc::vec::Vec::new();
        for i in 0..STACK_PAGES {
            let page =
                Page::<Size4KiB>::containing_address(user_stack_virt_base + (i * 4096) as u64);
            let frame = global_alloc
                .allocate_frame()
                .expect("Failed to allocate user stack frame");
            let mapped =
                unsafe { crate::memory::map_page_in_pml4_fallible(pml4, page, frame, flags) };
            let backing = if mapped {
                frame
            } else {
                crate::memory::deallocate_frame(frame);
                crate::memory::pml4_page_frame(pml4, page)
                    .expect("user stack page already mapped but untranslatable")
            };
            stack_frames.push((page.start_address().as_u64(), backing));
        }

        let user_rsp = crate::native_stack::write_relibc_stack_image(
            user_stack_top.as_u64(),
            phoff,
            phentsize,
            phnum,
            &stack_frames,
        );

        unsafe {
            // Stack frame: RIP, rbx(=user RIP), rbp(=user RSP), r12..r15
            rsp -= 8;
            core::ptr::write(
                rsp as *mut u64,
                crate::context::thread_entry_user as *const () as usize as u64,
            );
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, entry_point as u64); // rbx = user RIP
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, user_rsp); // rbp = user RSP (argc on stack)
            for _ in 0..4 {
                rsp -= 8;
                core::ptr::write(rsp as *mut u64, 0);
            }
        }

        Ok(Task {
            id: TaskId::new(),
            stack_ptr: VirtAddr::new(rsp),
            cap_table: crate::capability::CapTable::new(),
            state: TaskState::Ready,
            priority: TaskPriority::Normal,
            stack_base: kernel_stack_base,
            pml4: Some(pml4),
            is_idle: false,
            parent_id,
            exit_code: None,
            fds: Default::default(),
            vruntime: 0,
            deadline: None,
            affinity: CpuAffinity::all(),
            throttled: false,
            fpu_state: FxSaveArea::new(),
            key_buf: KeyRingBuf::new(),
            mouse_buf: MouseRingBuf::new(),
            pty_slave_id,
            fs_base: inherited_fs_base(),
            gs_base: 0,
            last_ran_tick: 0,
            last_cpu: u32::MAX,
            shares_address_space: false,
            clear_child_tid: 0,
        })
    }

    /// Creates a new user-mode task from a Linux ELF, setting up a Linux-compatible
    /// initial stack (argc/argv/envp/auxv) and marking it for Linux syscall routing.
    pub fn new_linux_elf(
        elf_data: &[u8],
        parent_id: Option<TaskId>,
        argv: &[&str],
        envp: &[&str],
    ) -> Result<Self, &'static str> {
        use crate::arch::VirtAddr;
        use alloc::vec::Vec;
        use x86_64::structures::paging::{FrameAllocator, Page, PageTableFlags, Size4KiB};

        // 1) Kernel stack in kernel PML4, then clone address space (includes stack).
        //    §10.3: stack BEFORE the user space so the clone captures it.
        //    1.5f: create_new_pml4 -> arch::mmu::new_user (delegating; same Root).
        let (kernel_stack_base, kernel_stack_end) =
            crate::memory::alloc_kernel_stack(KERNEL_STACK_SIZE);
        let pml4 = crate::arch::mmu::new_user();
        crate::memory::as_incref(pml4); // refcount this address space (see Drop)

        // 2) Load Linux ELF (apply relocations, TLS metadata, etc).
        // UNIQUE name per spawn: load_elf caches LoadedObjects by name, so the
        // old shared "linux_exec" made the 2nd+ Linux binary spawned return the
        // FIRST one's cached object (e.g. dh got the static probe's base-0
        // object -> AT_PHDR=0x40 -> ld.so faulted). A per-spawn id forces a
        // fresh load each time.
        let load_id = {
            static SPAWN_SEQ: AtomicUsize = AtomicUsize::new(0);
            SPAWN_SEQ.fetch_add(1, Ordering::Relaxed)
        };
        let load_name = alloc::format!("linux_exec#{}", load_id);
        let obj = crate::elf_loader::load_linux_elf(&load_name, elf_data)
            .map_err(|_| "Linux ELF load failed")?;
        let mut entry_point = obj.entry_point;

        // 2b) PT_INTERP: a dynamically-linked binary names an ELF interpreter
        // (ld.so). Load it into the SAME address space (allocate_base gives it a
        // non-overlapping base) and run IT first — ld.so relocates the main exe
        // and loads its shared libraries (libc.so.6, ...) at runtime. Static
        // binaries have no PT_INTERP and skip this. The interpreter is
        // self-contained (only R_X86_64_RELATIVE), so the existing loader
        // relocates it correctly.
        let mut interp_segments: alloc::vec::Vec<crate::elf_loader::LoadedSegment> =
            alloc::vec::Vec::new();
        let mut interp_base: Option<u64> = None;
        if let Some(interp_path) = crate::elf_loader::read_interp_path(elf_data) {
            crate::serial_println!("[linux-exec] dynamic ELF: interpreter = {}", interp_path);
            let interp_data = crate::vfs::read_file(&interp_path)
                .ok_or("ELF interpreter (ld.so) not found in VFS")?;
            let interp_obj = crate::elf_loader::load_linux_elf("ld.so", &interp_data)
                .map_err(|_| "ELF interpreter load failed")?;
            entry_point = interp_obj.entry_point; // run ld.so first, not the main exe
            interp_base = Some(interp_obj.base_addr);
            interp_segments = interp_obj.segments;
            crate::serial_println!(
                "[linux-exec] ld.so loaded: base={:#x} entry={:#x}",
                interp_obj.base_addr,
                entry_point
            );
        }

        // 3) Build Linux stack image auxv based on the loaded base address.
        let header =
            crate::elf_loader::ElfLoader::parse_header(elf_data).map_err(|_| "Invalid ELF")?;
        let mut info =
            crate::elf_loader::LinuxElfInfo::from_header(elf_data, &header, obj.base_addr);
        // For a dynamic exe, AT_BASE must be the INTERPRETER's load base (ld.so
        // reads it to relocate itself); AT_PHDR/AT_ENTRY stay the MAIN exe's so
        // ld.so can find + jump to it.
        if let Some(ib) = interp_base {
            info.base_addr = ib;
        }

        // 4) Allocate + map user stack pages.
        // Keep it simple: fixed high address stack, 256 KiB. The base sits
        // 64 pages BELOW the canonical top (0x7FFF_FFFF_F000) — anchoring at
        // the native path's 0x7FFF_FFFF_A000 base put the computed stack TOP
        // at 0x8000_0003_A000, past the 47-bit canonical boundary, and
        // VirtAddr's checked add panicked on every Linux ELF spawn.
        const STACK_PAGES: usize = 64;
        let user_stack_virt_base = VirtAddr::new(0x0000_7FFF_FFFB_F000);
        let user_stack_top = user_stack_virt_base + (STACK_PAGES as u64 * 4096);

        let mut global_alloc = crate::memory::GlobalFrameAllocator;
        // W^X: the Linux-ELF stack is data — never legitimately executed.
        // NO_EXECUTE defeats stack-overflow → shellcode. The Linux *segment*
        // loop already sets NX; its stack did not. (Audit 2026-07-06 #4.)
        let stack_flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE;

        // Map ELF segments (copy relocated segment data into user pages) — the
        // main exe's segments PLUS the interpreter's (ld.so), both already
        // rebased to their own non-overlapping bases by the loader.
        for seg in obj.segments.iter().chain(interp_segments.iter()) {
            if seg.memsz == 0 {
                continue;
            }

            let vstart = seg.vaddr;
            let vend = seg.vaddr + seg.memsz;
            if vend <= vstart {
                return Err("Invalid segment bounds");
            }

            let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(vstart));
            let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(vend - 1));

            for page in Page::range_inclusive(start_page, end_page) {
                let mut page_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
                if (seg.flags & crate::elf_loader::PF_W) != 0 {
                    page_flags |= PageTableFlags::WRITABLE;
                }
                if (seg.flags & crate::elf_loader::PF_X) == 0 {
                    page_flags |= PageTableFlags::NO_EXECUTE;
                }

                let frame = global_alloc
                    .allocate_frame()
                    .ok_or("OOM allocating ELF segment frame")?;
                unsafe {
                    let _ = crate::memory::map_page_in_pml4_fallible(pml4, page, frame, page_flags);
                }

                // ELF requires every byte in `[filesz, memsz)` to read as
                // zero.  In particular, a pure `.bss` page has no file range
                // at all, so zero it BEFORE the no-copy fast path below.  The
                // old ordering skipped this write for such pages and exposed
                // stale physical-frame contents to Linux daemons (including
                // the LinuxKPI allocator's heap/freelist state).
                let frame_virt = *crate::memory::PHYS_MEM_OFFSET
                    .get()
                    .ok_or("Missing PHYS_MEM_OFFSET")?
                    + frame.start_address().as_u64();
                let frame_ptr = frame_virt.as_mut_ptr::<u8>();
                unsafe {
                    core::ptr::write_bytes(frame_ptr, 0, 4096);
                }

                let page_start = page.start_address().as_u64();
                let page_end = page_start + 4096;
                let copy_start = core::cmp::max(page_start, vstart);
                let copy_end = core::cmp::min(page_end, vend);
                if copy_start >= copy_end {
                    continue;
                }

                let src_off = (copy_start - vstart) as usize;
                let dst_off = (copy_start - page_start) as usize;
                let len = (copy_end - copy_start) as usize;

                unsafe {
                    core::ptr::copy_nonoverlapping(
                        seg.data.as_ptr().add(src_off),
                        frame_ptr.add(dst_off),
                        len,
                    );
                }
            }
        }

        // Track frames so we can populate the stack via phys-mem window.
        let mut stack_frames: Vec<(u64, x86_64::structures::paging::PhysFrame)> = Vec::new();
        for i in 0..STACK_PAGES {
            let frame = global_alloc
                .allocate_frame()
                .ok_or("OOM allocating user stack")?;
            let page =
                Page::<Size4KiB>::containing_address(user_stack_virt_base + (i as u64 * 4096));
            unsafe {
                let _ = crate::memory::map_page_in_pml4_fallible(pml4, page, frame, stack_flags);
            }
            stack_frames.push((page.start_address().as_u64(), frame));
        }

        let (initial_rsp, stack_image) =
            crate::elf_loader::setup_linux_stack(user_stack_top.as_u64(), argv, envp, &info);

        // 5) Write stack image into the mapped stack frames.
        let image_start = initial_rsp;
        let image_end = user_stack_top.as_u64();
        if image_end < image_start {
            return Err("Invalid stack image");
        }
        let total_len = (image_end - image_start) as usize;
        if total_len != stack_image.len() {
            return Err("Stack image length mismatch");
        }

        for (page_start, frame) in &stack_frames {
            let page_start = *page_start;
            let page_end = page_start + 4096;
            let copy_start = core::cmp::max(page_start, image_start);
            let copy_end = core::cmp::min(page_end, image_end);
            if copy_start >= copy_end {
                continue;
            }
            let src_off = (copy_start - image_start) as usize;
            let dst_off = (copy_start - page_start) as usize;
            let len = (copy_end - copy_start) as usize;

            let frame_virt =
                *crate::memory::PHYS_MEM_OFFSET.get().unwrap() + frame.start_address().as_u64();
            let frame_ptr = frame_virt.as_mut_ptr::<u8>();
            unsafe {
                // Zero the page first (defensive).
                core::ptr::write_bytes(frame_ptr, 0, 4096);
                core::ptr::copy_nonoverlapping(
                    stack_image.as_ptr().add(src_off),
                    frame_ptr.add(dst_off),
                    len,
                );
            }
        }

        // 6) Seed initial user entry state on the kernel stack.
        let mut rsp = kernel_stack_end.as_u64();

        unsafe {
            // Stack frame: RIP, rbx(=user RIP), rbp(=user RSP), r12..r15
            rsp -= 8;
            core::ptr::write(
                rsp as *mut u64,
                crate::context::thread_entry_user as *const () as usize as u64,
            );
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, entry_point as u64); // rbx = user RIP
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, initial_rsp); // rbp = user RSP
            for _ in 0..4 {
                rsp -= 8;
                core::ptr::write(rsp as *mut u64, 0);
            }
        }

        Ok(Task {
            id: TaskId::new(),
            stack_ptr: VirtAddr::new(rsp),
            cap_table: crate::capability::CapTable::new(),
            state: TaskState::Ready,
            priority: TaskPriority::Normal,
            stack_base: kernel_stack_base,
            pml4: Some(pml4),
            is_idle: false,
            parent_id,
            exit_code: None,
            fds: Default::default(),
            vruntime: 0,
            deadline: None,
            affinity: CpuAffinity::all(),
            throttled: false,
            fpu_state: FxSaveArea::new(),
            key_buf: KeyRingBuf::new(),
            mouse_buf: MouseRingBuf::new(),
            pty_slave_id: None,
            fs_base: inherited_fs_base(),
            gs_base: 0,
            last_ran_tick: 0,
            last_cpu: u32::MAX,
            shares_address_space: false,
            clear_child_tid: 0,
        })
    }

    /// Create a `CLONE_THREAD` thread that SHARES `parent_pml4` (the caller's
    /// address space — NOT a fresh one). Mirrors `new_linux_elf` step 6: its OWN
    /// kernel stack, a `thread_entry_user` switch frame that iret's to user at
    /// `user_rip` (the parent's clone-return site) with RSP=`child_stack` and all
    /// GPRs zeroed (so the child's `clone()` returns 0). `fs_base` is the TLS
    /// pointer (CLONE_SETTLS) or the parent's. `shares_address_space` makes Drop
    /// route through the AS refcount — the shared PML4 is freed only when the last
    /// group member exits (memory: linux-clone-threads-scoping). The caller marks
    /// the task Linux + attaches POSIX state/fds + spawns it.
    pub fn new_linux_thread(
        user_rip: u64,
        child_stack: u64,
        fs_base: u64,
        parent_pml4: x86_64::structures::paging::PhysFrame,
        parent_id: Option<TaskId>,
    ) -> Result<Self, &'static str> {
        // Own kernel stack (NEVER shared — the kernel-stack-sharing bug was the
        // tree's worst, §10.3). The address space is the parent's, refcounted.
        let (kernel_stack_base, kernel_stack_end) =
            crate::memory::alloc_kernel_stack(KERNEL_STACK_SIZE);
        crate::memory::as_incref(parent_pml4);

        let mut rsp = kernel_stack_end.as_u64();
        unsafe {
            // Stack frame: RIP, rbx(=user RIP), rbp(=user RSP), r12..r15
            rsp -= 8;
            core::ptr::write(
                rsp as *mut u64,
                crate::context::thread_entry_user as *const () as usize as u64,
            );
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, user_rip); // rbx = clone-return RIP
            rsp -= 8;
            core::ptr::write(rsp as *mut u64, child_stack); // rbp = child user RSP
            for _ in 0..4 {
                rsp -= 8;
                core::ptr::write(rsp as *mut u64, 0);
            }
        }

        Ok(Task {
            id: TaskId::new(),
            stack_ptr: VirtAddr::new(rsp),
            cap_table: crate::capability::CapTable::new(),
            state: TaskState::Ready,
            priority: TaskPriority::Normal,
            stack_base: kernel_stack_base,
            pml4: Some(parent_pml4), // SHARED with the parent (refcounted)
            is_idle: false,
            parent_id,
            exit_code: None,
            fds: Default::default(),
            vruntime: 0,
            deadline: None,
            affinity: CpuAffinity::all(),
            throttled: false,
            fpu_state: FxSaveArea::new(),
            key_buf: KeyRingBuf::new(),
            mouse_buf: MouseRingBuf::new(),
            pty_slave_id: None,
            fs_base,
            gs_base: 0,
            last_ran_tick: 0,
            last_cpu: u32::MAX,
            shares_address_space: true,
            clear_child_tid: 0,
        })
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        // Free Kernel Stack
        crate::memory::free_kernel_stack(self.stack_base, KERNEL_STACK_SIZE);

        // Free PML4 frame and User Stack pages — but only when the LAST task
        // using this address space exits. A CLONE_THREAD thread shares its
        // parent's PML4; freeing it on the first exit (e.g. the parent being
        // reaped) while another group member still runs corrupts the survivor's
        // resume (the iron double fault). as_decref returns true only at zero.
        if let Some(pml4_frame) = self.pml4 {
            if crate::memory::as_decref(pml4_frame) {
                unsafe {
                    crate::memory::free_user_page_tables(pml4_frame);
                }
            }
        }

        // Clean up root IPC channels created by this task
        for (handle, cap) in self.cap_table.iter() {
            if let crate::capability::Cap::Channel { chan_id, .. } = cap {
                if self.cap_table.parent_of(handle).is_none() {
                    crate::ipc::IPC.lock().destroy_channel(*chan_id as usize);
                }
            }
        }
    }
}
