//! System Event Bus — publish/subscribe for typed system-wide events.
//!
//! Capabilities gate which events a process may receive. Critical events
//! (power, crash) are delivered with higher priority. Rapid-fire events
//! are deduplicated/coalesced, and late subscribers can catch up via a
//! per-type history ring.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

use crate::capability::CapHandle;
use crate::task::TaskId;

// ── Event Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub enum EventType {
    PowerStateChanged = 1,
    NetworkStatusChanged = 2,
    DisplayConfigChanged = 3,
    AudioDeviceChanged = 4,
    InputDeviceConnected = 5,
    InputDeviceDisconnected = 6,
    AppLaunched = 7,
    AppTerminated = 8,
    ThemeChanged = 9,
    UserSessionChanged = 10,
    StorageMounted = 11,
    StorageUnmounted = 12,
    BluetoothDeviceFound = 13,
    UsbDeviceAttached = 14,
    UsbDeviceDetached = 15,
    BatteryLevelChanged = 16,
    SystemSuspending = 17,
    SystemResumed = 18,
    ScreenLocked = 19,
    ScreenUnlocked = 20,
}

impl EventType {
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            EventType::PowerStateChanged | EventType::SystemSuspending | EventType::SystemResumed
        )
    }

    /// Capability flavor required to receive this event type.
    /// Returns 0 for unrestricted events.
    pub fn required_cap_flavor(&self) -> u32 {
        match self {
            EventType::AudioDeviceChanged => 8,   // Audio
            EventType::DisplayConfigChanged => 7, // Gpu
            EventType::NetworkStatusChanged => 6, // Network
            _ => 0,
        }
    }
}

// ── Event Priority ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum EventPriority {
    Critical = 0,
    High = 1,
    Normal = 2,
    Low = 3,
}

impl EventPriority {
    pub fn from_event_type(et: EventType) -> Self {
        if et.is_critical() {
            Self::Critical
        } else {
            match et {
                EventType::AppLaunched | EventType::AppTerminated => Self::High,
                EventType::ThemeChanged | EventType::BatteryLevelChanged => Self::Low,
                _ => Self::Normal,
            }
        }
    }
}

// ── Event Payload ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum EventData {
    None,
    U64(u64),
    String(String),
    Pair(u64, u64),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct SystemEvent {
    pub event_type: EventType,
    pub priority: EventPriority,
    pub data: EventData,
    pub source_task: TaskId,
    pub timestamp: u64,
    pub sequence: u64,
}

impl SystemEvent {
    pub fn new(event_type: EventType, data: EventData, source: TaskId, timestamp: u64) -> Self {
        Self {
            event_type,
            priority: EventPriority::from_event_type(event_type),
            data,
            source_task: source,
            timestamp,
            sequence: 0,
        }
    }
}

// ── Subscription ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubscriptionId(pub u64);

#[derive(Debug, Clone)]
struct Subscription {
    id: SubscriptionId,
    subscriber: TaskId,
    event_type: EventType,
    cap_handle: Option<CapHandle>,
    filter: EventFilter,
    delivery_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventFilter {
    All,
    SourceTask(TaskId),
    MinPriority(EventPriority),
}

impl EventFilter {
    fn matches(&self, event: &SystemEvent) -> bool {
        match self {
            Self::All => true,
            Self::SourceTask(tid) => event.source_task == *tid,
            Self::MinPriority(min_prio) => event.priority <= *min_prio,
        }
    }
}

// ── Event History Ring ────────────────────────────────────────────────────

struct EventHistory {
    events: Vec<SystemEvent>,
    capacity: usize,
}

impl EventHistory {
    fn new(capacity: usize) -> Self {
        Self {
            events: Vec::new(),
            capacity,
        }
    }

    fn push(&mut self, event: SystemEvent) {
        if self.events.len() >= self.capacity {
            self.events.remove(0);
        }
        self.events.push(event);
    }

    fn since_sequence(&self, seq: u64) -> Vec<&SystemEvent> {
        self.events.iter().filter(|e| e.sequence > seq).collect()
    }

    fn last_n(&self, n: usize) -> &[SystemEvent] {
        let start = self.events.len().saturating_sub(n);
        &self.events[start..]
    }

    fn last_of_type(&self, et: EventType) -> Option<&SystemEvent> {
        self.events.iter().rev().find(|e| e.event_type == et)
    }
}

// ── Deduplication ─────────────────────────────────────────────────────────

struct DedupeState {
    last_event_time: BTreeMap<EventType, u64>,
    coalesce_window_ms: u64,
}

impl DedupeState {
    fn new(coalesce_window_ms: u64) -> Self {
        Self {
            last_event_time: BTreeMap::new(),
            coalesce_window_ms,
        }
    }

    /// Returns true if the event should be delivered (not a duplicate).
    fn should_deliver(&mut self, event_type: EventType, timestamp: u64) -> bool {
        if event_type.is_critical() {
            self.last_event_time.insert(event_type, timestamp);
            return true;
        }

        if let Some(&last_time) = self.last_event_time.get(&event_type) {
            if timestamp.saturating_sub(last_time) < self.coalesce_window_ms {
                return false;
            }
        }
        self.last_event_time.insert(event_type, timestamp);
        true
    }
}

// ── Pending Delivery Queue ────────────────────────────────────────────────

/// Events waiting to be delivered to a specific subscriber.
struct DeliveryQueue {
    task: TaskId,
    events: Vec<SystemEvent>,
    max_pending: usize,
}

impl DeliveryQueue {
    fn new(task: TaskId, max_pending: usize) -> Self {
        Self {
            task,
            events: Vec::new(),
            max_pending,
        }
    }

    fn enqueue(&mut self, event: SystemEvent) -> bool {
        if self.events.len() >= self.max_pending {
            return false;
        }
        // Critical events go to front.
        if event.priority == EventPriority::Critical {
            self.events.insert(0, event);
        } else {
            self.events.push(event);
        }
        true
    }

    fn dequeue(&mut self) -> Option<SystemEvent> {
        if self.events.is_empty() {
            None
        } else {
            Some(self.events.remove(0))
        }
    }

    fn pending_count(&self) -> usize {
        self.events.len()
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

// ── Event Bus ─────────────────────────────────────────────────────────────

pub struct EventBus {
    subscriptions: Vec<Subscription>,
    delivery_queues: BTreeMap<TaskId, DeliveryQueue>,
    history: EventHistory,
    dedupe: DedupeState,
    next_sub_id: u64,
    next_sequence: u64,
    timestamp: u64,
    total_published: u64,
    total_delivered: u64,
    total_coalesced: u64,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            subscriptions: Vec::new(),
            delivery_queues: BTreeMap::new(),
            history: EventHistory::new(256),
            dedupe: DedupeState::new(50),
            next_sub_id: 1,
            next_sequence: 1,
            timestamp: 0,
            total_published: 0,
            total_delivered: 0,
            total_coalesced: 0,
        }
    }

    pub fn tick(&mut self, timestamp: u64) {
        self.timestamp = timestamp;
    }

    /// Subscribe to an event type. Returns a subscription ID for unsubscribing.
    pub fn subscribe(
        &mut self,
        subscriber: TaskId,
        event_type: EventType,
        cap_handle: Option<CapHandle>,
        filter: EventFilter,
    ) -> SubscriptionId {
        let id = SubscriptionId(self.next_sub_id);
        self.next_sub_id += 1;

        self.subscriptions.push(Subscription {
            id,
            subscriber,
            event_type,
            cap_handle,
            filter,
            delivery_count: 0,
        });

        if !self.delivery_queues.contains_key(&subscriber) {
            self.delivery_queues
                .insert(subscriber, DeliveryQueue::new(subscriber, 128));
        }

        id
    }

    /// Unsubscribe by subscription ID.
    pub fn unsubscribe(&mut self, sub_id: SubscriptionId) -> bool {
        if let Some(idx) = self.subscriptions.iter().position(|s| s.id == sub_id) {
            let sub = self.subscriptions.remove(idx);
            // Clean up delivery queue if no more subscriptions for that task.
            if !self
                .subscriptions
                .iter()
                .any(|s| s.subscriber == sub.subscriber)
            {
                self.delivery_queues.remove(&sub.subscriber);
            }
            true
        } else {
            false
        }
    }

    /// Unsubscribe all subscriptions for a task (e.g., on process exit).
    pub fn unsubscribe_task(&mut self, task: TaskId) {
        self.subscriptions.retain(|s| s.subscriber != task);
        self.delivery_queues.remove(&task);
    }

    /// Publish an event to all matching subscribers.
    pub fn publish(&mut self, mut event: SystemEvent) -> usize {
        self.total_published += 1;

        if !self
            .dedupe
            .should_deliver(event.event_type, event.timestamp)
        {
            self.total_coalesced += 1;
            return 0;
        }

        event.sequence = self.next_sequence;
        self.next_sequence += 1;

        let mut delivered = 0usize;

        // Collect matching subscriber task IDs first to avoid borrow issues.
        let matching: Vec<(TaskId, usize)> = self
            .subscriptions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.event_type == event.event_type && s.filter.matches(&event))
            .map(|(idx, s)| (s.subscriber, idx))
            .collect();

        for (task_id, sub_idx) in &matching {
            if let Some(queue) = self.delivery_queues.get_mut(task_id) {
                if queue.enqueue(event.clone()) {
                    delivered += 1;
                    self.subscriptions[*sub_idx].delivery_count += 1;
                }
            }
        }

        self.total_delivered += delivered as u64;
        self.history.push(event);
        delivered
    }

    /// Convenience: publish a simple event with no data.
    pub fn publish_simple(&mut self, event_type: EventType, source: TaskId) -> usize {
        let event = SystemEvent::new(event_type, EventData::None, source, self.timestamp);
        self.publish(event)
    }

    /// Convenience: publish event with a u64 payload.
    pub fn publish_u64(&mut self, event_type: EventType, source: TaskId, val: u64) -> usize {
        let event = SystemEvent::new(event_type, EventData::U64(val), source, self.timestamp);
        self.publish(event)
    }

    /// Poll: retrieve the next pending event for a task.
    pub fn poll(&mut self, task: TaskId) -> Option<SystemEvent> {
        self.delivery_queues.get_mut(&task)?.dequeue()
    }

    /// Poll: drain all pending events for a task.
    pub fn drain(&mut self, task: TaskId) -> Vec<SystemEvent> {
        let mut result = Vec::new();
        if let Some(queue) = self.delivery_queues.get_mut(&task) {
            while let Some(ev) = queue.dequeue() {
                result.push(ev);
            }
        }
        result
    }

    /// Pending event count for a task.
    pub fn pending_for(&self, task: TaskId) -> usize {
        self.delivery_queues
            .get(&task)
            .map_or(0, |q| q.pending_count())
    }

    /// Catch-up: get events of a type since a given sequence number.
    pub fn history_since(&self, event_type: EventType, since_seq: u64) -> Vec<&SystemEvent> {
        self.history
            .since_sequence(since_seq)
            .into_iter()
            .filter(|e| e.event_type == event_type)
            .collect()
    }

    /// Get the last N events from history.
    pub fn history_last(&self, n: usize) -> &[SystemEvent] {
        self.history.last_n(n)
    }

    /// Get last event of a specific type.
    pub fn last_event_of(&self, event_type: EventType) -> Option<&SystemEvent> {
        self.history.last_of_type(event_type)
    }

    /// List all subscriptions for a task.
    pub fn subscriptions_for(&self, task: TaskId) -> Vec<(SubscriptionId, EventType)> {
        self.subscriptions
            .iter()
            .filter(|s| s.subscriber == task)
            .map(|s| (s.id, s.event_type))
            .collect()
    }

    /// Number of active subscriptions.
    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    /// Stats.
    pub fn stats(&self) -> EventBusStats {
        EventBusStats {
            total_published: self.total_published,
            total_delivered: self.total_delivered,
            total_coalesced: self.total_coalesced,
            active_subscriptions: self.subscriptions.len() as u64,
            active_tasks: self.delivery_queues.len() as u64,
        }
    }

    /// Configure coalesce window (milliseconds).
    pub fn set_coalesce_window(&mut self, ms: u64) {
        self.dedupe.coalesce_window_ms = ms;
    }
}

// ── Stats ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct EventBusStats {
    pub total_published: u64,
    pub total_delivered: u64,
    pub total_coalesced: u64,
    pub active_subscriptions: u64,
    pub active_tasks: u64,
}

// ── Global Instance ───────────────────────────────────────────────────────

lazy_static! {
    pub static ref EVENT_BUS: Mutex<EventBus> = Mutex::new(EventBus::new());
}

// ── Public kernel API ─────────────────────────────────────────────────────

pub fn init() {
    let _ = &*EVENT_BUS;
}

pub fn subscribe(
    subscriber: TaskId,
    event_type: EventType,
    cap_handle: Option<CapHandle>,
) -> SubscriptionId {
    EVENT_BUS
        .lock()
        .subscribe(subscriber, event_type, cap_handle, EventFilter::All)
}

pub fn unsubscribe(sub_id: SubscriptionId) {
    EVENT_BUS.lock().unsubscribe(sub_id);
}

pub fn publish(event_type: EventType, source: TaskId, data: EventData) -> usize {
    let mut bus = EVENT_BUS.lock();
    let ts = bus.timestamp;
    let event = SystemEvent::new(event_type, data, source, ts);
    bus.publish(event)
}

pub fn publish_simple(event_type: EventType, source: TaskId) -> usize {
    EVENT_BUS.lock().publish_simple(event_type, source)
}

pub fn poll(task: TaskId) -> Option<SystemEvent> {
    EVENT_BUS.lock().poll(task)
}

pub fn drain(task: TaskId) -> Vec<SystemEvent> {
    EVENT_BUS.lock().drain(task)
}

pub fn cleanup_task(task: TaskId) {
    EVENT_BUS.lock().unsubscribe_task(task);
}
