//! Runtime permission prompts — interactive capability grant requests.
//!
//! When a sandboxed task attempts an operation that requires a capability it
//! doesn't hold, the kernel can queue a `PermRequest` instead of returning
//! EPERM immediately.  A trusted UI agent (the compositor shell) polls the
//! queue, presents the prompt to the user, and posts back an `Approve` or
//! `Deny` verdict.  The requesting task blocks until the verdict arrives.
//!
//! Design constraints:
//!   - Only the shell with the `PERM_PROMPT` capability may read/resolve requests.
//!   - Requests time out after `PROMPT_TIMEOUT_MS` to prevent indefinite hangs.
//!   - Each request carries a human-readable description so the UI can show
//!     "App X wants to access your Camera" etc.
//!   - The queue is bounded: if it fills up, new requests are auto-denied.

use alloc::collections::VecDeque;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::capability::{Cap, Rights};
use crate::task::TaskId;

const MAX_PENDING: usize = 32;
const PROMPT_TIMEOUT_MS: u64 = 30_000;

static REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(pub u64);

impl RequestId {
    fn next() -> Self {
        RequestId(REQUEST_SEQ.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pending,
    Approved,
    Denied,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapFlavor {
    Filesystem,
    Network,
    Camera,
    Audio,
    Gpu,
    Process,
    CryptoKey,
    Debug,
    Mmio,
    Irq,
    Port,
    System,
    ScreenCapture,
    Accessibility,
}

impl CapFlavor {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Filesystem => "File System",
            Self::Network => "Network",
            Self::Camera => "Camera",
            Self::Audio => "Audio",
            Self::Gpu => "GPU",
            Self::Process => "Process Management",
            Self::CryptoKey => "Cryptographic Keys",
            Self::Debug => "Debug / Profiling",
            Self::Mmio => "Hardware MMIO",
            Self::Irq => "Interrupt Vector",
            Self::Port => "I/O Port",
            Self::System => "System Control",
            Self::ScreenCapture => "Screen Capture",
            Self::Accessibility => "Accessibility",
        }
    }

    pub fn from_cap(cap: &Cap) -> Self {
        match cap {
            Cap::Filesystem { .. } => Self::Filesystem,
            Cap::Network { .. } => Self::Network,
            Cap::Camera { .. } => Self::Camera,
            Cap::Audio { .. } => Self::Audio,
            Cap::Gpu { .. } => Self::Gpu,
            Cap::Process { .. } => Self::Process,
            Cap::CryptoKey { .. } => Self::CryptoKey,
            Cap::Debug { .. } => Self::Debug,
            Cap::Mmio { .. } => Self::Mmio,
            Cap::Irq { .. } => Self::Irq,
            Cap::Port { .. } => Self::Port,
            Cap::Channel { .. } => Self::Network,
            Cap::Hypervisor { .. } => Self::Process,
            Cap::Attestation { .. } => Self::Debug,
            Cap::System { .. } => Self::System,
            Cap::ScreenCapture { .. } => Self::ScreenCapture,
            Cap::Accessibility { .. } => Self::Accessibility,
        }
    }
}

/// A single pending permission request.
#[derive(Debug, Clone)]
pub struct PermRequest {
    pub id: RequestId,
    pub requester: TaskId,
    pub app_name: String,
    pub flavor: CapFlavor,
    pub rights: Rights,
    pub description: String,
    pub verdict: Verdict,
    pub created_tick: u64,
    /// The full requested capability. `flavor`/`rights` above are the lossy
    /// UI projection; THIS is what an approval actually grants. Without it
    /// the queue could only log approvals (the pre-2026-06-11 hole: resolve()
    /// claimed to grant but had nothing to grant).
    pub cap: Cap,
    /// Cap handle in the requester's table after an approved grant (0 = none).
    /// Returned to the requester by `poll_verdict` so it can use the new cap.
    pub granted_handle: u64,
}

struct PromptQueue {
    pending: VecDeque<PermRequest>,
    resolved: VecDeque<PermRequest>,
}

impl PromptQueue {
    const fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            resolved: VecDeque::new(),
        }
    }
}

static QUEUE: Mutex<PromptQueue> = Mutex::new(PromptQueue::new());

fn current_tick() -> u64 {
    crate::hpet::read_millis().unwrap_or(0) as u64
}

/// Submit a permission request. Returns a `RequestId` the caller can poll,
/// or `None` if the queue is full (auto-deny).
pub fn request_permission(
    requester: TaskId,
    app_name: &str,
    cap: &Cap,
    description: &str,
) -> Option<RequestId> {
    let id = {
        let mut q = QUEUE.lock();
        if q.pending.len() >= MAX_PENDING {
            return None;
        }
        let req = PermRequest {
            id: RequestId::next(),
            requester,
            app_name: String::from(app_name),
            flavor: CapFlavor::from_cap(cap),
            rights: cap.rights(),
            description: String::from(description),
            verdict: Verdict::Pending,
            created_tick: current_tick(),
            cap: *cap,
            granted_handle: 0,
        };
        let id = req.id;
        q.pending.push_back(req);
        id
    }; // QUEUE released — perm_ui::pump re-locks it via drain_pending.

    // Event-driven consent: a new request immediately opens the compositor
    // dialog (no polling loop). No-op when one is already showing or the
    // compositor isn't up yet — the request stays queued either way.
    crate::perm_ui::pump();
    Some(id)
}

/// Poll the verdict for a previously submitted request. Returns the verdict
/// and — when `Approved` — the cap handle the grant landed at in the
/// requester's table (0 otherwise). Also garbage-collects timed-out requests.
pub fn poll_verdict(id: RequestId) -> (Verdict, u64) {
    let mut q = QUEUE.lock();
    let now = current_tick();

    // Check resolved queue first.
    if let Some(pos) = q.resolved.iter().position(|r| r.id == id) {
        let req = q.resolved.remove(pos).unwrap();
        return (req.verdict, req.granted_handle);
    }

    // Check pending — enforce timeout.
    if let Some(pos) = q.pending.iter().position(|r| r.id == id) {
        if now.saturating_sub(q.pending[pos].created_tick) > PROMPT_TIMEOUT_MS {
            let mut req = q.pending.remove(pos).unwrap();
            req.verdict = Verdict::TimedOut;
            crate::serial_println!(
                "[perm] Request {:?} timed out for task {:?}",
                id,
                req.requester
            );
            return (Verdict::TimedOut, 0);
        }
        return (Verdict::Pending, 0);
    }

    (Verdict::Denied, 0)
}

/// Called by the trusted UI shell to retrieve pending requests.
/// Returns up to `max` pending requests (oldest first).
pub fn drain_pending(max: usize) -> alloc::vec::Vec<PermRequest> {
    let q = QUEUE.lock();
    q.pending.iter().take(max).cloned().collect()
}

/// Called by the trusted UI shell to approve or deny a request.
/// On approval, the kernel grants the requested capability to the task:
/// the user's explicit consent IS the granting authority, so the cap is
/// inserted as a root cap (kernel-rooted, no userspace granter chain).
/// If the requester exited while the prompt was pending, the approval
/// degrades to Denied — a grant into a dead task's table would dangle.
pub fn resolve(id: RequestId, approved: bool) {
    // Take the request out of the queue first, WITHOUT holding the queue
    // lock across the scheduler call below (lock-order hygiene: QUEUE is a
    // leaf lock; scheduler::with_task_by_id takes SCHEDULER internally).
    let req_opt = {
        let mut q = QUEUE.lock();
        q.pending
            .iter()
            .position(|r| r.id == id)
            .map(|pos| q.pending.remove(pos).unwrap())
    };
    let Some(mut req) = req_opt else { return };

    if approved {
        // Perform the REAL grant — the entire point of the prompt system.
        match crate::scheduler::with_task_by_id(req.requester, |t| t.cap_table.insert_root(req.cap))
        {
            Some(handle) => {
                req.verdict = Verdict::Approved;
                req.granted_handle = handle.raw();
            }
            None => {
                // Requester died while the prompt was pending.
                req.verdict = Verdict::Denied;
                crate::serial_println!(
                    "[perm] approval for {:?} dropped: task {:?} no longer exists",
                    id,
                    req.requester
                );
            }
        }
    } else {
        req.verdict = Verdict::Denied;
    }

    crate::serial_println!(
        "[perm] {} request {:?} from {:?} ({}): {}{}",
        if req.verdict == Verdict::Approved {
            "APPROVED"
        } else {
            "DENIED"
        },
        id,
        req.requester,
        req.app_name,
        req.flavor.label(),
        if req.granted_handle != 0 {
            alloc::format!(" -> granted cap handle {}", req.granted_handle)
        } else {
            String::new()
        },
    );

    QUEUE.lock().resolved.push_back(req);
}

/// Expire all timed-out pending requests. Called periodically by the
/// watchdog or timer subsystem.
pub fn expire_stale() {
    let mut q = QUEUE.lock();
    let now = current_tick();
    let mut i = 0;
    while i < q.pending.len() {
        if now.saturating_sub(q.pending[i].created_tick) > PROMPT_TIMEOUT_MS {
            let mut req = q.pending.remove(i).unwrap();
            req.verdict = Verdict::TimedOut;
            q.resolved.push_back(req);
        } else {
            i += 1;
        }
    }
}

pub fn pending_count() -> usize {
    QUEUE.lock().pending.len()
}

pub fn init() {
    crate::serial_println!("[ OK ] Runtime permission prompt system initialized");
}

/// R10 boot smoketest: prove the request → approve → GRANT → poll cycle
/// lands a real capability in the requester's CapTable. This is the
/// regression fence for the pre-2026-06-11 hole where `resolve()` logged
/// "APPROVED" but granted nothing (Audit.md CRITICAL: perm_prompt.rs:200).
pub fn run_boot_smoketest() {
    let Some(tid) = crate::scheduler::current_task_id() else {
        crate::serial_println!("[perm] smoketest: no current task -> SKIP");
        return;
    };

    // Request a narrowly-scoped Network cap (ports 41000-41001, READ).
    let want = Cap::Network {
        port_range_start: 41000,
        port_range_end: 41001,
        rights: Rights::READ,
    };
    let Some(id) = request_permission(tid, "perm-smoketest", &want, "boot self-test") else {
        crate::serial_println!("[perm] smoketest: queue full -> FAIL");
        return;
    };

    // Deny path first: a denied request must NOT grant.
    let Some(id_deny) = request_permission(tid, "perm-smoketest", &want, "deny path") else {
        crate::serial_println!("[perm] smoketest: queue full -> FAIL");
        return;
    };
    resolve(id_deny, false);
    let (v_deny, h_deny) = poll_verdict(id_deny);
    let deny_ok = v_deny == Verdict::Denied && h_deny == 0;

    // Approve path: the grant must land in the requester's table.
    resolve(id, true);
    let (v, handle) = poll_verdict(id);
    let granted_in_table = handle != 0
        && crate::scheduler::with_task_by_id(tid, |t| {
            matches!(
                t.cap_table
                    .get(crate::capability::CapHandle::from_raw(handle)),
                Some(Cap::Network {
                    port_range_start: 41000,
                    port_range_end: 41001,
                    ..
                })
            )
        })
        .unwrap_or(false);

    let pass = v == Verdict::Approved && granted_in_table && deny_ok;
    crate::serial_println!(
        "[perm] smoketest: approve={:?} handle={} in_table={} deny_clean={} -> {}",
        v,
        handle,
        granted_in_table,
        deny_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}
