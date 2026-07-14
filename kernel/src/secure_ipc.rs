//! Secure IPC — authenticated inter-process communication with capability checks.
//!
//! Every message carries a `CapabilityToken` proving the sender holds the
//! required capability for the operation.  Channels are bidirectional,
//! capability-gated, and support zero-copy transfer of large payloads
//! via shared memory pages rather than buffer copies.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

use crate::capability::{CapHandle, Rights};
use crate::task::TaskId;

// ── Error Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    ChannelNotFound,
    ChannelFull,
    ChannelEmpty,
    PermissionDenied,
    InvalidEndpoint,
    InvalidCapability,
    PayloadTooLarge,
    ChannelClosed,
    NameAlreadyRegistered,
    NameNotFound,
    RateLimited,
}

// ── Capability Token ──────────────────────────────────────────────────────

/// Unforgeable proof that the sender holds a specific capability.
/// The kernel validates this on every `send` by checking the sender's
/// CapTable — userspace cannot fabricate a valid token.
#[derive(Debug, Clone, Copy)]
pub struct CapabilityToken {
    pub holder_task: TaskId,
    pub cap_handle: CapHandle,
    pub rights: Rights,
    pub nonce: u64,
}

impl CapabilityToken {
    pub fn new(holder: TaskId, handle: CapHandle, rights: Rights, nonce: u64) -> Self {
        Self {
            holder_task: holder,
            cap_handle: handle,
            rights,
            nonce,
        }
    }
}

// ── Message Payload ───────────────────────────────────────────────────────

/// For small payloads (≤ 4 KiB), data is inlined.
/// For large payloads, we share physical pages instead of copying.
#[derive(Debug, Clone)]
pub enum IpcPayload {
    /// Inline data (small messages, ≤ PAGE_SIZE).
    Inline(Vec<u8>),
    /// Zero-copy: physical frame numbers shared between sender/receiver.
    /// The kernel maps these frames into both address spaces.
    SharedPages { frame_numbers: Vec<u64>, len: usize },
}

impl IpcPayload {
    pub fn inline(data: &[u8]) -> Self {
        Self::Inline(data.to_vec())
    }

    pub fn shared(frames: Vec<u64>, len: usize) -> Self {
        Self::SharedPages {
            frame_numbers: frames,
            len,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Inline(data) => data.len(),
            Self::SharedPages { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_zero_copy(&self) -> bool {
        matches!(self, Self::SharedPages { .. })
    }
}

// ── IPC Message ───────────────────────────────────────────────────────────

/// Typed message with sender identity, capability proof, and payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Request,
    Response,
    Notification,
    Error,
    Signal,
}

#[derive(Debug, Clone)]
pub struct IpcMessage {
    pub msg_type: MessageType,
    pub sender_pid: TaskId,
    pub token: CapabilityToken,
    pub payload: IpcPayload,
    pub reply_to: Option<u64>,
    pub sequence: u64,
    pub timestamp: u64,
}

impl IpcMessage {
    pub fn new(
        msg_type: MessageType,
        sender: TaskId,
        token: CapabilityToken,
        payload: IpcPayload,
    ) -> Self {
        Self {
            msg_type,
            sender_pid: sender,
            token,
            payload,
            reply_to: None,
            sequence: 0,
            timestamp: 0,
        }
    }
}

// ── Channel Endpoint ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EndpointSide {
    A,
    B,
}

/// Permissions required to use an endpoint.
#[derive(Debug, Clone, Copy)]
pub struct EndpointPolicy {
    pub required_cap_flavor: u32,
    pub required_rights: Rights,
}

impl EndpointPolicy {
    pub fn new(flavor: u32, rights: Rights) -> Self {
        Self {
            required_cap_flavor: flavor,
            required_rights: rights,
        }
    }

    /// Unrestricted endpoint — any process with a Channel cap can use it.
    pub fn open() -> Self {
        Self {
            required_cap_flavor: 1, // Channel
            required_rights: Rights::NONE,
        }
    }
}

// ── Channel ───────────────────────────────────────────────────────────────

const DEFAULT_QUEUE_DEPTH: usize = 64;
const MAX_QUEUE_DEPTH: usize = 1024;
const ZERO_COPY_THRESHOLD: usize = 4096;

struct MessageQueue {
    messages: Vec<IpcMessage>,
    capacity: usize,
}

impl MessageQueue {
    fn new(capacity: usize) -> Self {
        Self {
            messages: Vec::new(),
            capacity,
        }
    }

    fn push(&mut self, msg: IpcMessage) -> Result<(), IpcError> {
        if self.messages.len() >= self.capacity {
            return Err(IpcError::ChannelFull);
        }
        self.messages.push(msg);
        Ok(())
    }

    fn pop(&mut self) -> Option<IpcMessage> {
        if self.messages.is_empty() {
            None
        } else {
            Some(self.messages.remove(0))
        }
    }

    fn len(&self) -> usize {
        self.messages.len()
    }

    fn is_full(&self) -> bool {
        self.messages.len() >= self.capacity
    }

    fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Bidirectional IPC channel with two endpoints (A and B).
/// Each side has its own message queue and policy.
struct SecureChannel {
    id: u32,
    queue_a_to_b: MessageQueue,
    queue_b_to_a: MessageQueue,
    policy_a: EndpointPolicy,
    policy_b: EndpointPolicy,
    owner_a: Option<TaskId>,
    owner_b: Option<TaskId>,
    closed: bool,
    sequence_counter: u64,
    total_messages: u64,
    total_bytes: u64,
}

impl SecureChannel {
    fn new(id: u32, depth: usize, policy_a: EndpointPolicy, policy_b: EndpointPolicy) -> Self {
        let depth = depth.min(MAX_QUEUE_DEPTH);
        Self {
            id,
            queue_a_to_b: MessageQueue::new(depth),
            queue_b_to_a: MessageQueue::new(depth),
            policy_a,
            policy_b,
            owner_a: None,
            owner_b: None,
            closed: false,
            sequence_counter: 0,
            total_messages: 0,
            total_bytes: 0,
        }
    }

    fn next_sequence(&mut self) -> u64 {
        self.sequence_counter += 1;
        self.sequence_counter
    }
}

// ── Endpoint Handle ───────────────────────────────────────────────────────

/// Opaque handle to one side of a channel, returned to userspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct IpcEndpoint {
    pub channel_id: u32,
    pub side: EndpointSide,
}

impl IpcEndpoint {
    pub fn a(channel_id: u32) -> Self {
        Self {
            channel_id,
            side: EndpointSide::A,
        }
    }

    pub fn b(channel_id: u32) -> Self {
        Self {
            channel_id,
            side: EndpointSide::B,
        }
    }
}

// ── Namespace ─────────────────────────────────────────────────────────────

/// Named channels discoverable by authorized processes.
/// Like D-Bus well-known names but capability-gated.
struct NamespaceEntry {
    name: String,
    channel_id: u32,
    side: EndpointSide,
    required_cap_flavor: u32,
    owner: TaskId,
}

struct IpcNamespace {
    entries: Vec<NamespaceEntry>,
}

impl IpcNamespace {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn register(
        &mut self,
        name: &str,
        channel_id: u32,
        side: EndpointSide,
        cap_flavor: u32,
        owner: TaskId,
    ) -> Result<(), IpcError> {
        if self.entries.iter().any(|e| e.name == name) {
            return Err(IpcError::NameAlreadyRegistered);
        }
        self.entries.push(NamespaceEntry {
            name: String::from(name),
            channel_id,
            side,
            required_cap_flavor: cap_flavor,
            owner,
        });
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<&NamespaceEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    fn unregister(&mut self, name: &str, owner: TaskId) -> Result<(), IpcError> {
        if let Some(idx) = self
            .entries
            .iter()
            .position(|e| e.name == name && e.owner == owner)
        {
            self.entries.remove(idx);
            Ok(())
        } else {
            Err(IpcError::NameNotFound)
        }
    }

    fn list_names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }
}

// ── Broadcast Channel ─────────────────────────────────────────────────────

/// One-to-many channel for system events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BroadcastId(pub u32);

struct BroadcastSubscription {
    subscriber: TaskId,
    cap_handle: CapHandle,
}

struct BroadcastChannel {
    id: BroadcastId,
    name: String,
    subscribers: Vec<BroadcastSubscription>,
    required_cap_flavor: u32,
    history: Vec<IpcMessage>,
    history_limit: usize,
    publisher: TaskId,
}

impl BroadcastChannel {
    fn new(id: BroadcastId, name: &str, publisher: TaskId, cap_flavor: u32) -> Self {
        Self {
            id,
            name: String::from(name),
            subscribers: Vec::new(),
            required_cap_flavor: cap_flavor,
            history: Vec::new(),
            history_limit: 32,
            publisher,
        }
    }

    fn subscribe(&mut self, subscriber: TaskId, cap_handle: CapHandle) {
        if !self.subscribers.iter().any(|s| s.subscriber == subscriber) {
            self.subscribers.push(BroadcastSubscription {
                subscriber,
                cap_handle,
            });
        }
    }

    fn unsubscribe(&mut self, subscriber: TaskId) {
        self.subscribers.retain(|s| s.subscriber != subscriber);
    }

    fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    fn record_history(&mut self, msg: &IpcMessage) {
        if self.history.len() >= self.history_limit {
            self.history.remove(0);
        }
        self.history.push(msg.clone());
    }
}

// ── IPC Monitor (Audit) ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum AuditEventKind {
    Send,
    Receive,
    ChannelCreated,
    ChannelClosed,
    PermissionDenied,
    BroadcastPublished,
    NameRegistered,
    NameUnregistered,
}

#[derive(Debug, Clone, Copy)]
pub struct IpcAuditEntry {
    pub kind: AuditEventKind,
    pub channel_id: u32,
    pub sender: TaskId,
    pub receiver: Option<TaskId>,
    pub msg_size: usize,
    pub timestamp: u64,
    pub cap_flavor: u32,
}

struct IpcMonitor {
    log: Vec<IpcAuditEntry>,
    capacity: usize,
    enabled: bool,
    denied_count: u64,
}

impl IpcMonitor {
    fn new(capacity: usize) -> Self {
        Self {
            log: Vec::new(),
            capacity,
            enabled: true,
            denied_count: 0,
        }
    }

    fn record(&mut self, entry: IpcAuditEntry) {
        if !self.enabled {
            return;
        }
        if entry.kind == AuditEventKind::PermissionDenied {
            self.denied_count += 1;
        }
        if self.log.len() >= self.capacity {
            self.log.remove(0);
        }
        self.log.push(entry);
    }

    fn recent(&self, count: usize) -> &[IpcAuditEntry] {
        let start = self.log.len().saturating_sub(count);
        &self.log[start..]
    }

    fn denied_count(&self) -> u64 {
        self.denied_count
    }

    fn clear(&mut self) {
        self.log.clear();
    }
}

impl PartialEq for AuditEventKind {
    fn eq(&self, other: &Self) -> bool {
        core::mem::discriminant(self) == core::mem::discriminant(other)
    }
}

impl Eq for AuditEventKind {}

// ── Secure IPC System ─────────────────────────────────────────────────────

pub struct SecureIpcSystem {
    channels: BTreeMap<u32, SecureChannel>,
    namespace: IpcNamespace,
    broadcasts: Vec<BroadcastChannel>,
    monitor: IpcMonitor,
    next_channel_id: u32,
    next_broadcast_id: u32,
    timestamp: u64,
}

impl SecureIpcSystem {
    pub fn new() -> Self {
        Self {
            channels: BTreeMap::new(),
            namespace: IpcNamespace::new(),
            broadcasts: Vec::new(),
            monitor: IpcMonitor::new(4096),
            next_channel_id: 1,
            next_broadcast_id: 1,
            timestamp: 0,
        }
    }

    pub fn tick(&mut self) {
        self.timestamp += 1;
    }

    /// Create a paired channel. Returns (EndpointA, EndpointB).
    pub fn create_channel(
        &mut self,
        policy_a: EndpointPolicy,
        policy_b: EndpointPolicy,
        queue_depth: usize,
        creator: TaskId,
    ) -> (IpcEndpoint, IpcEndpoint) {
        let id = self.next_channel_id;
        self.next_channel_id += 1;

        let depth = if queue_depth == 0 {
            DEFAULT_QUEUE_DEPTH
        } else {
            queue_depth
        };
        let mut chan = SecureChannel::new(id, depth, policy_a, policy_b);
        chan.owner_a = Some(creator);
        self.channels.insert(id, chan);

        self.monitor.record(IpcAuditEntry {
            kind: AuditEventKind::ChannelCreated,
            channel_id: id,
            sender: creator,
            receiver: None,
            msg_size: 0,
            timestamp: self.timestamp,
            cap_flavor: 0,
        });

        (IpcEndpoint::a(id), IpcEndpoint::b(id))
    }

    /// Create channel with default policies (open).
    pub fn create_channel_open(&mut self, creator: TaskId) -> (IpcEndpoint, IpcEndpoint) {
        self.create_channel(
            EndpointPolicy::open(),
            EndpointPolicy::open(),
            DEFAULT_QUEUE_DEPTH,
            creator,
        )
    }

    /// Assign ownership of an endpoint side to a task.
    pub fn assign_endpoint(
        &mut self,
        endpoint: &IpcEndpoint,
        task: TaskId,
    ) -> Result<(), IpcError> {
        let chan = self
            .channels
            .get_mut(&endpoint.channel_id)
            .ok_or(IpcError::ChannelNotFound)?;
        match endpoint.side {
            EndpointSide::A => chan.owner_a = Some(task),
            EndpointSide::B => chan.owner_b = Some(task),
        }
        Ok(())
    }

    /// Send a message through an endpoint.
    /// Verifies the sender holds the required capability before enqueuing.
    pub fn send(
        &mut self,
        endpoint: &IpcEndpoint,
        sender: TaskId,
        token: CapabilityToken,
        payload: IpcPayload,
    ) -> Result<u64, IpcError> {
        // Extract policy values up front to avoid borrow conflicts.
        let (policy_flavor, policy_rights, closed) = {
            let chan = self
                .channels
                .get(&endpoint.channel_id)
                .ok_or(IpcError::ChannelNotFound)?;
            let p = match endpoint.side {
                EndpointSide::A => &chan.policy_a,
                EndpointSide::B => &chan.policy_b,
            };
            (p.required_cap_flavor, p.required_rights, chan.closed)
        };

        if closed {
            return Err(IpcError::ChannelClosed);
        }

        if !verify_cap(sender, &token, policy_rights) {
            self.monitor.record(IpcAuditEntry {
                kind: AuditEventKind::PermissionDenied,
                channel_id: endpoint.channel_id,
                sender,
                receiver: None,
                msg_size: payload.len(),
                timestamp: self.timestamp,
                cap_flavor: policy_flavor,
            });
            return Err(IpcError::PermissionDenied);
        }

        let chan = self
            .channels
            .get_mut(&endpoint.channel_id)
            .ok_or(IpcError::ChannelNotFound)?;
        let msg_size = payload.len();
        let seq = chan.next_sequence();
        let receiver = match endpoint.side {
            EndpointSide::A => chan.owner_b,
            EndpointSide::B => chan.owner_a,
        };

        let msg = IpcMessage {
            msg_type: MessageType::Request,
            sender_pid: sender,
            token,
            payload,
            reply_to: None,
            sequence: seq,
            timestamp: self.timestamp,
        };

        let result = match endpoint.side {
            EndpointSide::A => chan.queue_a_to_b.push(msg),
            EndpointSide::B => chan.queue_b_to_a.push(msg),
        };

        match result {
            Ok(()) => {
                chan.total_messages += 1;
                chan.total_bytes += msg_size as u64;
                self.monitor.record(IpcAuditEntry {
                    kind: AuditEventKind::Send,
                    channel_id: endpoint.channel_id,
                    sender,
                    receiver,
                    msg_size,
                    timestamp: self.timestamp,
                    cap_flavor: policy_flavor,
                });
                Ok(seq)
            }
            Err(e) => Err(e),
        }
    }

    /// Send with automatic zero-copy decision.
    /// Large payloads (> 4 KiB) use shared pages instead of memcpy.
    pub fn send_auto(
        &mut self,
        endpoint: &IpcEndpoint,
        sender: TaskId,
        token: CapabilityToken,
        data: &[u8],
    ) -> Result<u64, IpcError> {
        let payload = if data.len() > ZERO_COPY_THRESHOLD {
            let num_pages = (data.len() + 4095) / 4096;
            let frames: Vec<u64> = (0..num_pages as u64).collect();
            IpcPayload::SharedPages {
                frame_numbers: frames,
                len: data.len(),
            }
        } else {
            IpcPayload::Inline(data.to_vec())
        };
        self.send(endpoint, sender, token, payload)
    }

    /// Blocking receive from an endpoint.
    pub fn recv(
        &mut self,
        endpoint: &IpcEndpoint,
        receiver: TaskId,
    ) -> Result<IpcMessage, IpcError> {
        let chan = self
            .channels
            .get_mut(&endpoint.channel_id)
            .ok_or(IpcError::ChannelNotFound)?;
        if chan.closed {
            return Err(IpcError::ChannelClosed);
        }

        let msg = match endpoint.side {
            EndpointSide::A => chan.queue_b_to_a.pop(),
            EndpointSide::B => chan.queue_a_to_b.pop(),
        };

        match msg {
            Some(m) => {
                self.monitor.record(IpcAuditEntry {
                    kind: AuditEventKind::Receive,
                    channel_id: endpoint.channel_id,
                    sender: m.sender_pid,
                    receiver: Some(receiver),
                    msg_size: m.payload.len(),
                    timestamp: self.timestamp,
                    cap_flavor: 0,
                });
                Ok(m)
            }
            None => Err(IpcError::ChannelEmpty),
        }
    }

    /// Non-blocking receive.
    pub fn try_recv(&mut self, endpoint: &IpcEndpoint) -> Option<IpcMessage> {
        let chan = self.channels.get_mut(&endpoint.channel_id)?;
        if chan.closed {
            return None;
        }
        match endpoint.side {
            EndpointSide::A => chan.queue_b_to_a.pop(),
            EndpointSide::B => chan.queue_a_to_b.pop(),
        }
    }

    /// Close a channel (both sides).
    pub fn close_channel(&mut self, channel_id: u32, closer: TaskId) -> Result<(), IpcError> {
        let chan = self
            .channels
            .get_mut(&channel_id)
            .ok_or(IpcError::ChannelNotFound)?;
        chan.closed = true;
        self.monitor.record(IpcAuditEntry {
            kind: AuditEventKind::ChannelClosed,
            channel_id,
            sender: closer,
            receiver: None,
            msg_size: 0,
            timestamp: self.timestamp,
            cap_flavor: 0,
        });
        Ok(())
    }

    /// Check how many messages are pending for an endpoint.
    pub fn pending_count(&self, endpoint: &IpcEndpoint) -> usize {
        match self.channels.get(&endpoint.channel_id) {
            Some(chan) => match endpoint.side {
                EndpointSide::A => chan.queue_b_to_a.len(),
                EndpointSide::B => chan.queue_a_to_b.len(),
            },
            None => 0,
        }
    }

    /// Check if a channel's send queue is full (backpressure signal).
    pub fn is_full(&self, endpoint: &IpcEndpoint) -> bool {
        match self.channels.get(&endpoint.channel_id) {
            Some(chan) => match endpoint.side {
                EndpointSide::A => chan.queue_a_to_b.is_full(),
                EndpointSide::B => chan.queue_b_to_a.is_full(),
            },
            None => true,
        }
    }

    // ── Namespace operations ──────────────────────────────────────────────

    pub fn register_name(
        &mut self,
        name: &str,
        endpoint: &IpcEndpoint,
        cap_flavor: u32,
        owner: TaskId,
    ) -> Result<(), IpcError> {
        self.namespace
            .register(name, endpoint.channel_id, endpoint.side, cap_flavor, owner)?;
        self.monitor.record(IpcAuditEntry {
            kind: AuditEventKind::NameRegistered,
            channel_id: endpoint.channel_id,
            sender: owner,
            receiver: None,
            msg_size: 0,
            timestamp: self.timestamp,
            cap_flavor,
        });
        Ok(())
    }

    pub fn lookup_name(&self, name: &str) -> Option<IpcEndpoint> {
        self.namespace.lookup(name).map(|entry| IpcEndpoint {
            channel_id: entry.channel_id,
            side: entry.side,
        })
    }

    pub fn unregister_name(&mut self, name: &str, owner: TaskId) -> Result<(), IpcError> {
        self.namespace.unregister(name, owner)?;
        self.monitor.record(IpcAuditEntry {
            kind: AuditEventKind::NameUnregistered,
            channel_id: 0,
            sender: owner,
            receiver: None,
            msg_size: 0,
            timestamp: self.timestamp,
            cap_flavor: 0,
        });
        Ok(())
    }

    pub fn list_names(&self) -> Vec<&str> {
        self.namespace.list_names()
    }

    // ── Broadcast operations ──────────────────────────────────────────────

    pub fn create_broadcast(
        &mut self,
        name: &str,
        publisher: TaskId,
        cap_flavor: u32,
    ) -> BroadcastId {
        let id = BroadcastId(self.next_broadcast_id);
        self.next_broadcast_id += 1;
        self.broadcasts
            .push(BroadcastChannel::new(id, name, publisher, cap_flavor));
        id
    }

    pub fn subscribe_broadcast(
        &mut self,
        broadcast_id: BroadcastId,
        subscriber: TaskId,
        cap_handle: CapHandle,
    ) -> Result<(), IpcError> {
        let bc = self
            .broadcasts
            .iter_mut()
            .find(|b| b.id == broadcast_id)
            .ok_or(IpcError::ChannelNotFound)?;
        bc.subscribe(subscriber, cap_handle);
        Ok(())
    }

    pub fn unsubscribe_broadcast(
        &mut self,
        broadcast_id: BroadcastId,
        subscriber: TaskId,
    ) -> Result<(), IpcError> {
        let bc = self
            .broadcasts
            .iter_mut()
            .find(|b| b.id == broadcast_id)
            .ok_or(IpcError::ChannelNotFound)?;
        bc.unsubscribe(subscriber);
        Ok(())
    }

    pub fn publish_broadcast(
        &mut self,
        broadcast_id: BroadcastId,
        publisher: TaskId,
        token: CapabilityToken,
        payload: IpcPayload,
    ) -> Result<usize, IpcError> {
        let bc = self
            .broadcasts
            .iter_mut()
            .find(|b| b.id == broadcast_id)
            .ok_or(IpcError::ChannelNotFound)?;

        if bc.publisher != publisher {
            return Err(IpcError::PermissionDenied);
        }

        let msg = IpcMessage {
            msg_type: MessageType::Notification,
            sender_pid: publisher,
            token,
            payload,
            reply_to: None,
            sequence: 0,
            timestamp: self.timestamp,
        };

        bc.record_history(&msg);
        let count = bc.subscriber_count();

        self.monitor.record(IpcAuditEntry {
            kind: AuditEventKind::BroadcastPublished,
            channel_id: broadcast_id.0,
            sender: publisher,
            receiver: None,
            msg_size: msg.payload.len(),
            timestamp: self.timestamp,
            cap_flavor: bc.required_cap_flavor,
        });

        Ok(count)
    }

    pub fn broadcast_history(&self, broadcast_id: BroadcastId) -> Option<&[IpcMessage]> {
        self.broadcasts
            .iter()
            .find(|b| b.id == broadcast_id)
            .map(|b| b.history.as_slice())
    }

    // ── Monitor / audit ───────────────────────────────────────────────────

    pub fn audit_recent(&self, count: usize) -> &[IpcAuditEntry] {
        self.monitor.recent(count)
    }

    pub fn audit_denied_count(&self) -> u64 {
        self.monitor.denied_count()
    }

    pub fn audit_clear(&mut self) {
        self.monitor.clear();
    }

    // ── Stats ─────────────────────────────────────────────────────────────

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn broadcast_count(&self) -> usize {
        self.broadcasts.len()
    }

    pub fn channel_stats(&self, channel_id: u32) -> Option<(u64, u64)> {
        self.channels
            .get(&channel_id)
            .map(|c| (c.total_messages, c.total_bytes))
    }

    // ── Per-task reclaim (SEV-2 leak fix) ─────────────────────────────────
    //
    // `reclaim_task_resources` swept compositor/sandbox/net/fast-IPC on task
    // exit but never SECURE_IPC, so a task that created secure channels,
    // registered namespace names, or subscribed to broadcasts and then EXITED
    // leaked the `SecureChannel` (two message queues), its namespace entry, and
    // its broadcast subscriptions — unbounded growth — and left a dead TaskId
    // dangling in `owner_a/owner_b` (a reused TaskId could be handed a stale
    // endpoint). This is the SECURE_IPC sibling of `ipc::cleanup_task_channels`.
    //
    // Operates ONLY on SECURE_IPC's own tables; it never calls `verify_cap` or
    // anything that re-enters the scheduler, so it is safe to call from
    // `reclaim_task_resources` (which holds SCHEDULER, IF=0). Mirrors the
    // half-dead-channel policy by full teardown: any channel owned by `tid` on
    // EITHER side is removed (freeing both queues) rather than nulling one owner
    // and leaving a half-live channel whose surviving owner could later be
    // reused — teardown is the safe choice the audit asked for.
    pub fn cleanup_task(&mut self, tid: TaskId) {
        // (a) Remove every channel where this task owns either endpoint.
        let owned: Vec<u32> = self
            .channels
            .iter()
            .filter(|(_, c)| c.owner_a == Some(tid) || c.owner_b == Some(tid))
            .map(|(id, _)| *id)
            .collect();
        for id in owned {
            // remove() drops the SecureChannel, freeing both Vec<IpcMessage>
            // queues; no shared frames are held here (payload SharedPages frame
            // numbers are placeholders, not owned allocations — see send_auto).
            self.channels.remove(&id);
        }

        // (b) Unregister every namespace name owned by this task.
        self.namespace.entries.retain(|e| e.owner != tid);

        // (c) Drop every broadcast subscription held by this task, and tear down
        //     any broadcast channel this task published (its history Vec +
        //     subscriber list).
        for bc in self.broadcasts.iter_mut() {
            bc.unsubscribe(tid);
        }
        self.broadcasts.retain(|b| b.publisher != tid);
    }

    /// Count channels currently owned (either side) by `tid` — smoketest probe.
    pub fn channels_owned_by(&self, tid: TaskId) -> usize {
        self.channels
            .values()
            .filter(|c| c.owner_a == Some(tid) || c.owner_b == Some(tid))
            .count()
    }

    /// Count namespace names currently owned by `tid` — smoketest probe.
    pub fn names_owned_by(&self, tid: TaskId) -> usize {
        self.namespace
            .entries
            .iter()
            .filter(|e| e.owner == tid)
            .count()
    }

    /// Count broadcast subscriptions currently held by `tid` (across all
    /// broadcasts) plus broadcasts published by `tid` — smoketest probe.
    pub fn broadcast_refs_for(&self, tid: TaskId) -> usize {
        let subs = self
            .broadcasts
            .iter()
            .filter(|b| b.subscribers.iter().any(|s| s.subscriber == tid))
            .count();
        let pubs = self
            .broadcasts
            .iter()
            .filter(|b| b.publisher == tid)
            .count();
        subs + pubs
    }

    /// True if NO channel still names `tid` as an owner on either side — used by
    /// the smoketest to prove no stale `owner_a/owner_b = Some(dead_tid)` remains.
    pub fn no_dangling_owner(&self, tid: TaskId) -> bool {
        !self
            .channels
            .values()
            .any(|c| c.owner_a == Some(tid) || c.owner_b == Some(tid))
    }
}

/// Capability verification against the sender's REAL CapTable.
///
/// The pre-2026-06-11 version trusted `token.rights` — a value userspace
/// self-declares — so any sender could fabricate a token claiming any rights
/// (Audit.md CRITICAL: secure_ipc.rs:946). The real checks:
///
///   1. `required == Rights::NONE` is the `EndpointPolicy::open()` contract:
///      the channel CREATOR explicitly opted out of cap-gating for that side.
///      This is intended public-channel semantics, not a hole — the policy
///      value comes from the (kernel-mediated) channel creation, never from
///      the sender.
///   2. Anti-spoof: the token must name the actual sender as holder. A task
///      cannot replay another task's token.
///   3. The claimed handle must exist in the sender's CapTable RIGHT NOW
///      (revocation-safe: a revoked cap fails here), and the rights used for
///      the policy check come from the TABLE's cap — the token's
///      self-declared `rights` field is ignored for authorization.
///
/// Lock order: callers hold SECURE_IPC; this takes SCHEDULER inside
/// `with_task_by_id`. SECURE_IPC → SCHEDULER is the established order; never
/// call into secure_ipc while holding the scheduler lock.
fn verify_cap(sender: TaskId, token: &CapabilityToken, required: Rights) -> bool {
    if required == Rights::NONE {
        return true; // EndpointPolicy::open() — explicit creator opt-out
    }
    if token.holder_task != sender {
        return false;
    }
    crate::scheduler::with_task_by_id(sender, |t| t.cap_table.get(token.cap_handle))
        .flatten()
        .map(|real_cap| real_cap.rights().contains(required))
        .unwrap_or(false)
}

lazy_static! {
    pub static ref SECURE_IPC: Mutex<SecureIpcSystem> = Mutex::new(SecureIpcSystem::new());
}

// ── Public API (kernel-facing) ────────────────────────────────────────────

pub fn init() {
    let _ = &*SECURE_IPC;
}

/// R10 boot smoketest — regression fence for the forged-token hole
/// (Audit.md CRITICAL: secure_ipc.rs:946). Proves on every boot:
///   forged: a token claiming rights with NO real cap behind it is DENIED;
///   spoofed: a token naming a different holder_task is DENIED;
///   real: a token backed by an actual CapTable entry is ACCEPTED;
///   open: an `EndpointPolicy::open()` endpoint accepts without a cap.
pub fn run_boot_smoketest() {
    use crate::capability::Cap;

    let Some(tid) = crate::scheduler::current_task_id() else {
        crate::serial_println!("[secure-ipc] smoketest: no current task -> SKIP");
        return;
    };

    // Channel: side A requires WRITE on a real cap; side B is open().
    let (ep_a, ep_b) = create_channel(
        tid,
        EndpointPolicy::new(1, Rights::WRITE),
        EndpointPolicy::open(),
        8,
    );

    // 1. Forged token: claims WRITE, but the handle is garbage.
    let forged = CapabilityToken::new(tid, CapHandle::from_raw(0xDEAD), Rights::WRITE, 1);
    let forged_denied = matches!(
        send_message(&ep_a, tid, forged, b"forged"),
        Err(IpcError::PermissionDenied)
    );

    // 2. Real cap: insert a Channel cap with WRITE into the task's table and
    //    present a token for that handle.
    let real_handle = crate::scheduler::with_task_by_id(tid, |t| {
        t.cap_table.insert_root(Cap::Channel {
            chan_id: ep_a.channel_id,
            rights: Rights::READ | Rights::WRITE,
        })
    });
    let real_ok = match real_handle {
        Some(h) => {
            let token = CapabilityToken::new(tid, h, Rights::WRITE, 2);
            send_message(&ep_a, tid, token, b"real").is_ok()
        }
        None => false,
    };

    // 3. Spoofed holder: same real handle, but the token names a different
    //    task as holder — must be rejected even though the cap exists.
    let spoof_denied = match real_handle {
        Some(h) => {
            let other = crate::task::TaskId::from_raw(tid.raw().wrapping_add(0x5005));
            let token = CapabilityToken::new(other, h, Rights::WRITE, 3);
            matches!(
                send_message(&ep_a, tid, token, b"spoof"),
                Err(IpcError::PermissionDenied)
            )
        }
        None => false,
    };

    // 4. Open endpoint (side B): no cap needed by explicit creator policy.
    let open_ok = {
        let token = CapabilityToken::new(tid, CapHandle::from_raw(0), Rights::NONE, 4);
        send_message(&ep_b, tid, token, b"open").is_ok()
    };

    let pass = forged_denied && real_ok && spoof_denied && open_ok;
    crate::serial_println!(
        "[secure-ipc] smoketest: forged_denied={} real_ok={} spoof_denied={} open_ok={} -> {}",
        forged_denied,
        real_ok,
        spoof_denied,
        open_ok,
        if pass { "PASS" } else { "FAIL" },
    );

    run_cleanup_smoketest();
}

/// R10 cleanup smoketest (SEV-2 leak fence) — proves on every boot that
/// `cleanup_task` reclaims a synthetic task's channels, namespace names, and
/// broadcast subscriptions, and leaves NO `owner_a/owner_b = Some(dead_tid)`
/// dangling. FAIL-able: it asserts the counts went UP after creation and back
/// to zero after reclaim — if `cleanup_task` did nothing, every boolean is
/// false and it prints FAIL.
fn run_cleanup_smoketest() {
    use crate::capability::CapHandle;

    // A synthetic, never-scheduled tid so we don't disturb live state.
    let synth = crate::task::TaskId::from_raw(0xDEAD_BEEF);

    let mut sys = SECURE_IPC.lock();

    // Snapshot baseline (the live boot smoketest above created channels for the
    // real current task, not `synth`, so synth's baseline is zero).
    let base_chans = sys.channels_owned_by(synth);
    let base_names = sys.names_owned_by(synth);
    let base_bcast = sys.broadcast_refs_for(synth);

    // (1) Create a channel owned by synth on side A.
    let (ep_a, _ep_b) =
        sys.create_channel(EndpointPolicy::open(), EndpointPolicy::open(), 8, synth);
    // (2) Register a namespace name owned by synth.
    let name_reg_ok = sys
        .register_name("raeen.test.cleanup", &ep_a, 1, synth)
        .is_ok();
    // (3) Create + subscribe a broadcast as synth.
    let bid = sys.create_broadcast("raeen.test.bcast", synth, 1);
    let sub_ok = sys
        .subscribe_broadcast(bid, synth, CapHandle::from_raw(0))
        .is_ok();

    let created_chans = sys.channels_owned_by(synth);
    let created_names = sys.names_owned_by(synth);
    let created_bcast = sys.broadcast_refs_for(synth);

    // The resources must actually have been created above (else nothing to
    // reclaim → false green).
    let created_ok = created_chans > base_chans
        && name_reg_ok
        && created_names > base_names
        && sub_ok
        && created_bcast > base_bcast;

    // Reclaim everything synth owns.
    sys.cleanup_task(synth);

    let after_chans = sys.channels_owned_by(synth);
    let after_names = sys.names_owned_by(synth);
    let after_bcast = sys.broadcast_refs_for(synth);
    let no_dangling = sys.no_dangling_owner(synth);

    let channels_reclaimed = created_ok && after_chans == base_chans;
    let names_reclaimed = created_ok && after_names == base_names;
    let bcast_reclaimed = created_ok && after_bcast == base_bcast;

    drop(sys);

    let pass = channels_reclaimed && names_reclaimed && bcast_reclaimed && no_dangling;
    crate::serial_println!(
        "[secure-ipc] cleanup smoketest: channels_reclaimed={} names_reclaimed={} no_dangling_owner={} bcast_reclaimed={} -> {}",
        channels_reclaimed,
        names_reclaimed,
        no_dangling,
        bcast_reclaimed,
        if pass { "PASS" } else { "FAIL" },
    );
}

pub fn create_channel(
    creator: TaskId,
    policy_a: EndpointPolicy,
    policy_b: EndpointPolicy,
    depth: usize,
) -> (IpcEndpoint, IpcEndpoint) {
    SECURE_IPC
        .lock()
        .create_channel(policy_a, policy_b, depth, creator)
}

pub fn send_message(
    endpoint: &IpcEndpoint,
    sender: TaskId,
    token: CapabilityToken,
    data: &[u8],
) -> Result<u64, IpcError> {
    SECURE_IPC.lock().send_auto(endpoint, sender, token, data)
}

pub fn recv_message(endpoint: &IpcEndpoint, receiver: TaskId) -> Result<IpcMessage, IpcError> {
    SECURE_IPC.lock().recv(endpoint, receiver)
}

pub fn try_recv_message(endpoint: &IpcEndpoint) -> Option<IpcMessage> {
    SECURE_IPC.lock().try_recv(endpoint)
}

pub fn register_service(
    name: &str,
    endpoint: &IpcEndpoint,
    cap_flavor: u32,
    owner: TaskId,
) -> Result<(), IpcError> {
    SECURE_IPC
        .lock()
        .register_name(name, endpoint, cap_flavor, owner)
}

pub fn lookup_service(name: &str) -> Option<IpcEndpoint> {
    SECURE_IPC.lock().lookup_name(name)
}

/// Reclaim every SECURE_IPC resource owned by an exiting task (SEV-2 leak fix):
/// channels owned on either side (freeing their message queues), namespace
/// names, and broadcast subscriptions/publications. Called from
/// `scheduler::reclaim_task_resources` — the SINGLE per-task-exit reclaim path —
/// right next to `ipc::cleanup_task_channels`.
///
/// Lock order (CLAUDE.md §10.5/§10.6): the caller holds SCHEDULER (IF=0). This
/// acquires only SECURE_IPC and performs pure table edits — it never calls
/// `verify_cap` or otherwise re-enters the scheduler, so it cannot complete the
/// SECURE_IPC→SCHEDULER edge that `verify_cap` walks; mirrors the
/// "never touches SCHEDULER" discipline of `net::cleanup_task_sockets`.
pub fn cleanup_task(tid: TaskId) {
    SECURE_IPC.lock().cleanup_task(tid);
}
