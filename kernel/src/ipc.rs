use alloc::collections::BTreeMap;
use lazy_static::lazy_static;
use spin::Mutex;

#[derive(Debug, Clone, Copy, Default)]
pub struct Message {
    pub msg_type: u64,
    pub arg1: u64,
    pub arg2: u64,
    pub arg3: u64,
}

pub const IPC_BUFFER_SIZE: usize = 64;

/// Well-known channel ids created at boot (main.rs) in this order. BUG-18: the
/// keyboard/mouse IRQ paths used to scatter the literals `1` and `2`; these
/// constants make the binding explicit and survive a reorder of channel setup.
pub const KEYBOARD_CHANNEL: usize = 1;
pub const MOUSE_CHANNEL: usize = 2;

/// Sentinel owner for channels created by the kernel itself (keyboard, mouse,
/// internal services) — never swept by per-task cleanup.
pub const SYSTEM_OWNER: u64 = 0;

pub struct Channel {
    pub buffer: [Message; IPC_BUFFER_SIZE],
    pub head: usize,
    pub tail: usize,
    pub len: usize,
    /// Physical address of a 4KB frame shared between sender and receiver.
    /// Used for zero-copy bulk data transfer.
    pub shared_frame: Option<u64>,
    /// Task that created the channel, or `SYSTEM_OWNER` for kernel channels.
    /// BUG-23: used to reclaim a task's channels when it exits.
    pub owner_pid: u64,
}

lazy_static! {
    pub static ref IPC: Mutex<IpcSystem> = Mutex::new(IpcSystem::new());
}

pub struct IpcSystem {
    channels: BTreeMap<usize, Channel>,
    next_chan_id: usize,
}

impl IpcSystem {
    pub fn new() -> Self {
        IpcSystem {
            channels: BTreeMap::new(),
            next_chan_id: 1,
        }
    }

    pub fn create_channel(&mut self, with_shmem: bool) -> usize {
        self.create_channel_owned(with_shmem, SYSTEM_OWNER)
    }

    /// Create a channel owned by `owner_pid` so it is reclaimed when that task
    /// exits (BUG-23). `SYSTEM_OWNER` channels are never auto-swept.
    pub fn create_channel_owned(&mut self, with_shmem: bool, owner_pid: u64) -> usize {
        let id = self.next_chan_id;
        self.next_chan_id += 1;

        let shared_frame = if with_shmem {
            crate::memory::allocate_frame().map(|f| f.start_address().as_u64())
        } else {
            None
        };

        self.channels.insert(
            id,
            Channel {
                buffer: [Message::default(); IPC_BUFFER_SIZE],
                head: 0,
                tail: 0,
                len: 0,
                shared_frame,
                owner_pid,
            },
        );
        id
    }

    /// Destroy every channel owned by `pid` (BUG-23: channels were never freed
    /// on task exit — an unbounded leak of the 64-message buffer + any shared
    /// frame). System channels (`SYSTEM_OWNER`) are left alone.
    pub fn cleanup_task_channels(&mut self, pid: u64) {
        if pid == SYSTEM_OWNER {
            return;
        }
        let owned: alloc::vec::Vec<usize> = self
            .channels
            .iter()
            .filter(|(_, c)| c.owner_pid == pid)
            .map(|(id, _)| *id)
            .collect();
        for id in owned {
            self.destroy_channel(id);
        }
    }

    pub fn destroy_channel(&mut self, chan_id: usize) -> bool {
        if let Some(chan) = self.channels.remove(&chan_id) {
            if let Some(frame_addr) = chan.shared_frame {
                crate::memory::deallocate_frame(
                    x86_64::structures::paging::PhysFrame::from_start_address(
                        crate::arch::PhysAddr::new(frame_addr),
                    )
                    .unwrap(),
                );
            }
            true
        } else {
            false
        }
    }

    pub fn send(&mut self, chan_id: usize, msg: Message) -> Result<(), &'static str> {
        if let Some(chan) = self.channels.get_mut(&chan_id) {
            if chan.len == IPC_BUFFER_SIZE {
                return Err("Channel full");
            }
            chan.buffer[chan.tail] = msg;
            chan.tail = (chan.tail + 1) % IPC_BUFFER_SIZE;
            chan.len += 1;
            Ok(())
        } else {
            Err("Invalid channel")
        }
    }

    pub fn try_recv(&mut self, chan_id: usize) -> Option<Message> {
        if let Some(chan) = self.channels.get_mut(&chan_id) {
            if chan.len == 0 {
                return None;
            }
            let msg = chan.buffer[chan.head];
            chan.head = (chan.head + 1) % IPC_BUFFER_SIZE;
            chan.len -= 1;
            Some(msg)
        } else {
            None
        }
    }

    /// Number of queued (unread) messages on `chan_id`, or `None` if the channel
    /// doesn't exist. Used by diagnostics/smoketests to confirm a send landed.
    pub fn channel_len(&self, chan_id: usize) -> Option<usize> {
        self.channels.get(&chan_id).map(|c| c.len)
    }

    pub fn get_channel_shared_frame(&self, chan_id: usize) -> Option<u64> {
        self.channels.get(&chan_id).and_then(|c| c.shared_frame)
    }
}

// IPC capability checks use `crate::capability` (single authority source).
