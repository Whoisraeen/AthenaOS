#![allow(dead_code)]

extern crate alloc;

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use spin::Mutex;

// ─── Error types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum TtyError {
    DeviceNotFound,
    DeviceExists,
    DeviceBusy,
    InvalidArg,
    NoSpace,
    Io,
    WouldBlock,
    NotATty,
    PermissionDenied,
    Hangup,
    BadFileDescriptor,
    NotSupported,
    Again,
}

// ─── Poll flags ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct PollFlags {
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
    pub hangup: bool,
}

impl PollFlags {
    pub fn none() -> Self {
        Self {
            readable: false,
            writable: false,
            error: false,
            hangup: false,
        }
    }

    pub fn readable() -> Self {
        Self {
            readable: true,
            writable: false,
            error: false,
            hangup: false,
        }
    }

    pub fn writable() -> Self {
        Self {
            readable: false,
            writable: true,
            error: false,
            hangup: false,
        }
    }

    pub fn read_write() -> Self {
        Self {
            readable: true,
            writable: true,
            error: false,
            hangup: false,
        }
    }
}

// ─── Device ID ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeviceId {
    pub major: u16,
    pub minor: u32,
}

impl DeviceId {
    pub fn new(major: u16, minor: u32) -> Self {
        Self { major, minor }
    }

    pub fn dev_t(&self) -> u64 {
        ((self.major as u64) << 32) | (self.minor as u64)
    }

    pub fn from_dev_t(dev: u64) -> Self {
        Self {
            major: (dev >> 32) as u16,
            minor: dev as u32,
        }
    }
}

// ─── Major numbers ───────────────────────────────────────────────────────────

pub const MAJOR_MEM: u16 = 1;
pub const MAJOR_TTY: u16 = 4;
pub const MAJOR_CONSOLE: u16 = 5;
pub const MAJOR_MISC: u16 = 10;
pub const MAJOR_PTY_MASTER: u16 = 128;
pub const MAJOR_PTY_SLAVE: u16 = 136;

// ─── Character Device trait ──────────────────────────────────────────────────

pub trait CharDevice: Send {
    fn name(&self) -> &str;
    fn read(&mut self, buf: &mut [u8], offset: u64) -> Result<usize, TtyError>;
    fn write(&mut self, data: &[u8], offset: u64) -> Result<usize, TtyError>;
    fn ioctl(&mut self, cmd: u32, arg: u64) -> Result<u64, TtyError>;
    fn poll(&self) -> PollFlags;
    fn open(&mut self) -> Result<(), TtyError>;
    fn close(&mut self) -> Result<(), TtyError>;
}

// ─── Character Device Registry ───────────────────────────────────────────────

pub struct CharDeviceRegistry {
    devices: BTreeMap<DeviceId, Box<dyn CharDevice + Send>>,
    by_name: BTreeMap<String, DeviceId>,
    next_minor: u32,
}

impl CharDeviceRegistry {
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            by_name: BTreeMap::new(),
            next_minor: 0,
        }
    }

    pub fn register(
        &mut self,
        major: u16,
        minor: u32,
        device: Box<dyn CharDevice + Send>,
    ) -> Result<DeviceId, TtyError> {
        let id = DeviceId::new(major, minor);
        if self.devices.contains_key(&id) {
            return Err(TtyError::DeviceExists);
        }
        let name = String::from(device.name());
        self.by_name.insert(name, id);
        self.devices.insert(id, device);
        if minor >= self.next_minor {
            self.next_minor = minor + 1;
        }
        Ok(id)
    }

    pub fn unregister(&mut self, id: &DeviceId) -> Result<(), TtyError> {
        if let Some(dev) = self.devices.remove(id) {
            let name = String::from(dev.name());
            self.by_name.remove(&name);
            Ok(())
        } else {
            Err(TtyError::DeviceNotFound)
        }
    }

    pub fn get(&self, id: &DeviceId) -> Option<&dyn CharDevice> {
        self.devices.get(id).map(|d| d.as_ref() as &dyn CharDevice)
    }

    pub fn get_mut(&mut self, id: &DeviceId) -> Option<&mut Box<dyn CharDevice + Send>> {
        self.devices.get_mut(id)
    }

    pub fn lookup(&self, name: &str) -> Option<DeviceId> {
        self.by_name.get(name).copied()
    }

    pub fn list(&self) -> Vec<(&DeviceId, &str)> {
        self.devices
            .iter()
            .map(|(id, dev)| (id, dev.name()))
            .collect()
    }

    pub fn allocate_minor(&mut self) -> u32 {
        let m = self.next_minor;
        self.next_minor += 1;
        m
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }
}

// ─── Special devices ─────────────────────────────────────────────────────────

pub struct NullDevice;

impl CharDevice for NullDevice {
    fn name(&self) -> &str {
        "null"
    }

    fn read(&mut self, _buf: &mut [u8], _offset: u64) -> Result<usize, TtyError> {
        Ok(0)
    }

    fn write(&mut self, data: &[u8], _offset: u64) -> Result<usize, TtyError> {
        Ok(data.len())
    }

    fn ioctl(&mut self, _cmd: u32, _arg: u64) -> Result<u64, TtyError> {
        Err(TtyError::NotSupported)
    }

    fn poll(&self) -> PollFlags {
        PollFlags::read_write()
    }

    fn open(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
}

pub struct ZeroDevice;

impl CharDevice for ZeroDevice {
    fn name(&self) -> &str {
        "zero"
    }

    fn read(&mut self, buf: &mut [u8], _offset: u64) -> Result<usize, TtyError> {
        for b in buf.iter_mut() {
            *b = 0;
        }
        Ok(buf.len())
    }

    fn write(&mut self, data: &[u8], _offset: u64) -> Result<usize, TtyError> {
        Ok(data.len())
    }

    fn ioctl(&mut self, _cmd: u32, _arg: u64) -> Result<u64, TtyError> {
        Err(TtyError::NotSupported)
    }

    fn poll(&self) -> PollFlags {
        PollFlags::read_write()
    }

    fn open(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
}

pub struct FullDevice;

impl CharDevice for FullDevice {
    fn name(&self) -> &str {
        "full"
    }

    fn read(&mut self, buf: &mut [u8], _offset: u64) -> Result<usize, TtyError> {
        for b in buf.iter_mut() {
            *b = 0;
        }
        Ok(buf.len())
    }

    fn write(&mut self, _data: &[u8], _offset: u64) -> Result<usize, TtyError> {
        Err(TtyError::NoSpace)
    }

    fn ioctl(&mut self, _cmd: u32, _arg: u64) -> Result<u64, TtyError> {
        Err(TtyError::NotSupported)
    }

    fn poll(&self) -> PollFlags {
        PollFlags {
            readable: true,
            writable: false,
            error: false,
            hangup: false,
        }
    }

    fn open(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
}

pub struct RandomDevice {
    state: [u64; 4],
}

impl RandomDevice {
    pub fn new() -> Self {
        Self {
            state: [
                0xDEAD_BEEF_CAFE_BABE,
                0x1234_5678_9ABC_DEF0,
                0xFEDC_BA98_7654_3210,
                0x0123_4567_89AB_CDEF,
            ],
        }
    }

    fn xoshiro256ss(&mut self) -> u64 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.state[1] << 17;
        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(45);
        result
    }
}

impl CharDevice for RandomDevice {
    fn name(&self) -> &str {
        "urandom"
    }

    fn read(&mut self, buf: &mut [u8], _offset: u64) -> Result<usize, TtyError> {
        let mut i = 0;
        while i < buf.len() {
            let val = self.xoshiro256ss();
            let bytes = val.to_le_bytes();
            let remaining = buf.len() - i;
            let copy_len = if remaining < 8 { remaining } else { 8 };
            buf[i..i + copy_len].copy_from_slice(&bytes[..copy_len]);
            i += copy_len;
        }
        Ok(buf.len())
    }

    fn write(&mut self, data: &[u8], _offset: u64) -> Result<usize, TtyError> {
        for chunk in data.chunks(8) {
            let mut val: u64 = 0;
            for (j, &b) in chunk.iter().enumerate() {
                val |= (b as u64) << (j * 8);
            }
            self.state[0] ^= val;
        }
        Ok(data.len())
    }

    fn ioctl(&mut self, _cmd: u32, _arg: u64) -> Result<u64, TtyError> {
        Err(TtyError::NotSupported)
    }

    fn poll(&self) -> PollFlags {
        PollFlags::read_write()
    }

    fn open(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
}

pub struct KmsgDevice {
    ring: Vec<u8>,
    ring_capacity: usize,
    write_pos: usize,
}

impl KmsgDevice {
    pub fn new() -> Self {
        let cap = 65536;
        Self {
            ring: vec![0u8; cap],
            ring_capacity: cap,
            write_pos: 0,
        }
    }
}

impl CharDevice for KmsgDevice {
    fn name(&self) -> &str {
        "kmsg"
    }

    fn read(&mut self, buf: &mut [u8], offset: u64) -> Result<usize, TtyError> {
        let start = offset as usize;
        if start >= self.write_pos {
            return Ok(0);
        }
        let available = self.write_pos - start;
        let to_copy = if available < buf.len() {
            available
        } else {
            buf.len()
        };
        let ring_start = start % self.ring_capacity;
        if ring_start + to_copy <= self.ring_capacity {
            buf[..to_copy].copy_from_slice(&self.ring[ring_start..ring_start + to_copy]);
        } else {
            let first = self.ring_capacity - ring_start;
            buf[..first].copy_from_slice(&self.ring[ring_start..]);
            buf[first..to_copy].copy_from_slice(&self.ring[..to_copy - first]);
        }
        Ok(to_copy)
    }

    fn write(&mut self, data: &[u8], _offset: u64) -> Result<usize, TtyError> {
        for &b in data {
            let pos = self.write_pos % self.ring_capacity;
            self.ring[pos] = b;
            self.write_pos += 1;
        }
        Ok(data.len())
    }

    fn ioctl(&mut self, _cmd: u32, _arg: u64) -> Result<u64, TtyError> {
        Err(TtyError::NotSupported)
    }

    fn poll(&self) -> PollFlags {
        PollFlags::read_write()
    }

    fn open(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
}

// ─── Termios — POSIX terminal attributes ─────────────────────────────────────

// Input flags
pub const IGNBRK: u32 = 0o000001;
pub const BRKINT: u32 = 0o000002;
pub const IGNPAR: u32 = 0o000004;
pub const PARMRK: u32 = 0o000010;
pub const INPCK: u32 = 0o000020;
pub const ISTRIP: u32 = 0o000040;
pub const INLCR: u32 = 0o000100;
pub const IGNCR: u32 = 0o000200;
pub const ICRNL: u32 = 0o000400;
pub const IXON: u32 = 0o002000;
pub const IXOFF: u32 = 0o010000;
pub const IXANY: u32 = 0o020000;
pub const IMAXBEL: u32 = 0o020000;
pub const IUTF8: u32 = 0o040000;

// Output flags
pub const OPOST: u32 = 0o000001;
pub const ONLCR: u32 = 0o000004;
pub const OCRNL: u32 = 0o000010;
pub const ONOCR: u32 = 0o000020;
pub const ONLRET: u32 = 0o000040;
pub const OFILL: u32 = 0o000100;
pub const OFDEL: u32 = 0o000200;

// Control flags
pub const CSIZE: u32 = 0o000060;
pub const CS5: u32 = 0o000000;
pub const CS6: u32 = 0o000020;
pub const CS7: u32 = 0o000040;
pub const CS8: u32 = 0o000060;
pub const CSTOPB: u32 = 0o000100;
pub const CREAD: u32 = 0o000200;
pub const PARENB: u32 = 0o000400;
pub const PARODD: u32 = 0o001000;
pub const HUPCL: u32 = 0o002000;
pub const CLOCAL: u32 = 0o004000;

// Local flags
pub const ISIG: u32 = 0o000001;
pub const ICANON: u32 = 0o000002;
pub const ECHO: u32 = 0o000010;
pub const ECHOE: u32 = 0o000020;
pub const ECHOK: u32 = 0o000040;
pub const ECHONL: u32 = 0o000100;
pub const NOFLSH: u32 = 0o000200;
pub const TOSTOP: u32 = 0o000400;
pub const ECHOCTL: u32 = 0o001000;
pub const ECHOPRT: u32 = 0o002000;
pub const ECHOKE: u32 = 0o004000;
pub const IEXTEN: u32 = 0o100000;

// Control character indices
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSTART: usize = 8;
pub const VSTOP: usize = 9;
pub const VSUSP: usize = 10;
pub const VEOL: usize = 11;
pub const VREPRINT: usize = 12;
pub const VDISCARD: usize = 13;
pub const VWERASE: usize = 14;
pub const VLNEXT: usize = 15;
pub const VEOL2: usize = 16;

// ioctl commands
pub const TCGETS: u32 = 0x5401;
pub const TCSETS: u32 = 0x5402;
pub const TCSETSW: u32 = 0x5403;
pub const TCSETSF: u32 = 0x5404;
pub const TIOCGWINSZ: u32 = 0x5413;
pub const TIOCSWINSZ: u32 = 0x5414;
pub const TIOCGPGRP: u32 = 0x540F;
pub const TIOCSPGRP: u32 = 0x5410;
pub const TIOCGSID: u32 = 0x5429;
pub const TIOCSCTTY: u32 = 0x540E;
pub const TIOCNOTTY: u32 = 0x5422;
pub const FIONREAD: u32 = 0x541B;

#[derive(Debug, Clone)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_cc: [u8; 32],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

impl Termios {
    pub fn default_cooked() -> Self {
        let mut cc = [0u8; 32];
        cc[VINTR] = 3; // ^C
        cc[VQUIT] = 28; // ^\
        cc[VERASE] = 127; // DEL
        cc[VKILL] = 21; // ^U
        cc[VEOF] = 4; // ^D
        cc[VTIME] = 0;
        cc[VMIN] = 1;
        cc[VSTART] = 17; // ^Q
        cc[VSTOP] = 19; // ^S
        cc[VSUSP] = 26; // ^Z
        cc[VEOL] = 0;
        cc[VREPRINT] = 18; // ^R
        cc[VDISCARD] = 15; // ^O
        cc[VWERASE] = 23; // ^W
        cc[VLNEXT] = 22; // ^V
        cc[VEOL2] = 0;

        Self {
            c_iflag: ICRNL | IXON | IMAXBEL | IUTF8,
            c_oflag: OPOST | ONLCR,
            c_cflag: CS8 | CREAD | CLOCAL,
            c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN,
            c_cc: cc,
            c_ispeed: 38400,
            c_ospeed: 38400,
        }
    }

    pub fn raw() -> Self {
        let mut cc = [0u8; 32];
        cc[VMIN] = 1;
        cc[VTIME] = 0;

        Self {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: CS8 | CREAD | CLOCAL,
            c_lflag: 0,
            c_cc: cc,
            c_ispeed: 38400,
            c_ospeed: 38400,
        }
    }

    pub fn is_canonical(&self) -> bool {
        self.c_lflag & ICANON != 0
    }

    pub fn is_echo(&self) -> bool {
        self.c_lflag & ECHO != 0
    }

    pub fn is_signal(&self) -> bool {
        self.c_lflag & ISIG != 0
    }
}

// ─── Winsize ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Winsize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

impl Winsize {
    pub fn default_console() -> Self {
        Self {
            ws_row: 25,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }

    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

// ─── Line Discipline ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LdiscMode {
    N_TTY,
    N_SLIP,
    N_MOUSE,
    N_PPP,
    N_STRIP,
    N_AX25,
}

#[derive(Debug, Clone)]
pub enum LdiscAction {
    None,
    Echo(Vec<u8>),
    Signal(u8),
    DataReady,
    Output(Vec<u8>),
    Flush,
}

const LDISC_BUF_SIZE: usize = 4096;

pub struct LineDiscipline {
    mode: LdiscMode,
    column: u32,
    canon_buf: Vec<u8>,
    echo_buf: Vec<u8>,
    read_buf: Vec<u8>,
    read_head: usize,
    read_tail: usize,
    read_cnt: usize,
    canon_head: usize,
    erasing: bool,
    lnext: bool,
}

impl LineDiscipline {
    pub fn new() -> Self {
        Self {
            mode: LdiscMode::N_TTY,
            column: 0,
            canon_buf: Vec::with_capacity(LDISC_BUF_SIZE),
            echo_buf: Vec::new(),
            read_buf: vec![0u8; LDISC_BUF_SIZE],
            read_head: 0,
            read_tail: 0,
            read_cnt: 0,
            canon_head: 0,
            erasing: false,
            lnext: false,
        }
    }

    pub fn receive_char(&mut self, c: u8, termios: &Termios) -> LdiscAction {
        if self.mode != LdiscMode::N_TTY {
            return self.raw_receive(c);
        }
        self.process_input_char(c, termios)
    }

    pub fn receive_buf(&mut self, data: &[u8], termios: &Termios) -> Vec<LdiscAction> {
        let mut actions = Vec::with_capacity(data.len());
        for &c in data {
            let action = self.receive_char(c, termios);
            match &action {
                LdiscAction::None => {}
                _ => actions.push(action),
            }
        }
        actions
    }

    pub fn read(&mut self, buf: &mut [u8], termios: &Termios) -> usize {
        if termios.is_canonical() {
            self.canon_copy_to_read();
        }
        let available = self.read_cnt;
        if available == 0 {
            return 0;
        }
        let to_copy = if available < buf.len() {
            available
        } else {
            buf.len()
        };
        for i in 0..to_copy {
            buf[i] = self.read_buf[self.read_tail % LDISC_BUF_SIZE];
            self.read_tail += 1;
        }
        self.read_cnt -= to_copy;
        to_copy
    }

    pub fn write_output(&self, data: &[u8], termios: &Termios) -> Vec<u8> {
        if termios.c_oflag & OPOST == 0 {
            return data.to_vec();
        }
        let mut out = Vec::with_capacity(data.len() * 2);
        for &c in data {
            let processed = self.output_process(c, termios);
            out.extend_from_slice(&processed);
        }
        out
    }

    pub fn chars_in_buffer(&self) -> usize {
        self.read_cnt
    }

    pub fn flush_read(&mut self) {
        self.read_head = 0;
        self.read_tail = 0;
        self.read_cnt = 0;
        self.canon_buf.clear();
        self.canon_head = 0;
    }

    pub fn flush_write(&mut self) {
        self.echo_buf.clear();
    }

    fn raw_receive(&mut self, c: u8) -> LdiscAction {
        if self.read_cnt < LDISC_BUF_SIZE {
            self.read_buf[self.read_head % LDISC_BUF_SIZE] = c;
            self.read_head += 1;
            self.read_cnt += 1;
            LdiscAction::DataReady
        } else {
            LdiscAction::None
        }
    }

    fn process_input_char(&mut self, c: u8, termios: &Termios) -> LdiscAction {
        if self.lnext {
            self.lnext = false;
            return self.insert_char(c, termios);
        }

        if termios.is_signal() && self.is_special_char(c, termios) {
            if c == termios.c_cc[VINTR] {
                if termios.c_lflag & NOFLSH == 0 {
                    self.flush_read();
                }
                return LdiscAction::Signal(2); // SIGINT
            }
            if c == termios.c_cc[VQUIT] {
                if termios.c_lflag & NOFLSH == 0 {
                    self.flush_read();
                }
                return LdiscAction::Signal(3); // SIGQUIT
            }
            if c == termios.c_cc[VSUSP] {
                if termios.c_lflag & NOFLSH == 0 {
                    self.flush_read();
                }
                return LdiscAction::Signal(20); // SIGTSTP
            }
        }

        // VLNEXT — literal next, escape the following character
        if termios.c_lflag & IEXTEN != 0 && c == termios.c_cc[VLNEXT] {
            self.lnext = true;
            if termios.is_echo() {
                return LdiscAction::Echo(vec![b'^', 8]); // ^<backspace> visual
            }
            return LdiscAction::None;
        }

        // Input translations
        let c = self.translate_input(c, termios);

        if termios.is_canonical() {
            return self.canon_receive(c, termios);
        }

        self.insert_char(c, termios)
    }

    fn translate_input(&self, c: u8, termios: &Termios) -> u8 {
        let mut ch = c;
        if termios.c_iflag & ISTRIP != 0 {
            ch &= 0x7F;
        }
        if ch == b'\r' {
            if termios.c_iflag & IGNCR != 0 {
                return 0xFF; // sentinel: drop this char
            }
            if termios.c_iflag & ICRNL != 0 {
                return b'\n';
            }
        }
        if ch == b'\n' && termios.c_iflag & INLCR != 0 {
            return b'\r';
        }
        ch
    }

    fn canon_receive(&mut self, c: u8, termios: &Termios) -> LdiscAction {
        if c == 0xFF {
            return LdiscAction::None;
        }

        // VERASE — delete one character
        if c == termios.c_cc[VERASE] {
            let echo = self.erase_char(termios);
            if !echo.is_empty() {
                return LdiscAction::Echo(echo);
            }
            return LdiscAction::None;
        }

        // VWERASE — delete one word
        if termios.c_lflag & IEXTEN != 0 && c == termios.c_cc[VWERASE] {
            let echo = self.erase_word(termios);
            if !echo.is_empty() {
                return LdiscAction::Echo(echo);
            }
            return LdiscAction::None;
        }

        // VKILL — delete entire line
        if c == termios.c_cc[VKILL] {
            let echo = self.kill_line(termios);
            if !echo.is_empty() {
                return LdiscAction::Echo(echo);
            }
            return LdiscAction::None;
        }

        // VREPRINT — reprint the current line
        if termios.c_lflag & IEXTEN != 0 && c == termios.c_cc[VREPRINT] {
            let mut echo = vec![b'^', b'R', b'\r', b'\n'];
            echo.extend_from_slice(&self.canon_buf);
            return LdiscAction::Echo(echo);
        }

        // VEOF — end of file
        if c == termios.c_cc[VEOF] {
            self.canon_copy_to_read();
            return LdiscAction::DataReady;
        }

        // Line terminator: newline or VEOL
        if c == b'\n'
            || c == termios.c_cc[VEOL]
            || (termios.c_cc[VEOL2] != 0 && c == termios.c_cc[VEOL2])
        {
            self.canon_buf.push(c);
            self.canon_copy_to_read();
            let echo = if termios.is_echo() || (c == b'\n' && termios.c_lflag & ECHONL != 0) {
                self.echo_char(c, termios)
            } else {
                Vec::new()
            };
            if echo.is_empty() {
                return LdiscAction::DataReady;
            }
            // Both echo and data
            return LdiscAction::Echo(echo);
        }

        self.insert_char(c, termios)
    }

    fn insert_char(&mut self, c: u8, termios: &Termios) -> LdiscAction {
        if c == 0xFF {
            return LdiscAction::None;
        }

        if termios.is_canonical() {
            if self.canon_buf.len() >= LDISC_BUF_SIZE - 1 {
                return LdiscAction::None;
            }
            self.canon_buf.push(c);
        } else {
            if self.read_cnt >= LDISC_BUF_SIZE {
                return LdiscAction::None;
            }
            self.read_buf[self.read_head % LDISC_BUF_SIZE] = c;
            self.read_head += 1;
            self.read_cnt += 1;
        }

        self.erasing = false;
        if termios.is_echo() {
            let echo = self.echo_char(c, termios);
            if echo.is_empty() {
                if !termios.is_canonical() {
                    return LdiscAction::DataReady;
                }
                return LdiscAction::None;
            }
            return LdiscAction::Echo(echo);
        }

        if !termios.is_canonical() {
            LdiscAction::DataReady
        } else {
            LdiscAction::None
        }
    }

    fn echo_char(&mut self, c: u8, termios: &Termios) -> Vec<u8> {
        if c == b'\n' {
            self.column = 0;
            return vec![b'\r', b'\n'];
        }
        if c == b'\t' {
            let spaces = 8 - (self.column % 8);
            self.column += spaces;
            return vec![b'\t'];
        }
        if c < 32 && termios.c_lflag & ECHOCTL != 0 {
            self.column += 2;
            return vec![b'^', c + 64];
        }
        if c == 127 && termios.c_lflag & ECHOCTL != 0 {
            self.column += 2;
            return vec![b'^', b'?'];
        }
        if c >= 32 {
            self.column += 1;
        }
        vec![c]
    }

    fn erase_char(&mut self, termios: &Termios) -> Vec<u8> {
        if self.canon_buf.is_empty() {
            return Vec::new();
        }
        let removed = self.canon_buf.pop().unwrap();
        if !termios.is_echo() {
            return Vec::new();
        }
        if termios.c_lflag & ECHOPRT != 0 {
            if !self.erasing {
                self.erasing = true;
                return vec![b'\\', removed];
            }
            return vec![removed];
        }
        if termios.c_lflag & ECHOE != 0 {
            if removed < 32 && termios.c_lflag & ECHOCTL != 0 {
                // Control char was echoed as ^X, erase two columns
                return vec![8, b' ', 8, 8, b' ', 8];
            }
            if removed == b'\t' {
                // Tab erasing: just emit a single backspace-space-backspace
                return vec![8, b' ', 8];
            }
            if self.column > 0 {
                self.column -= 1;
            }
            return vec![8, b' ', 8]; // \b SPACE \b
        }
        Vec::new()
    }

    fn erase_word(&mut self, termios: &Termios) -> Vec<u8> {
        let mut echo = Vec::new();
        // Skip trailing whitespace
        while !self.canon_buf.is_empty() {
            if let Some(&last) = self.canon_buf.last() {
                if last != b' ' && last != b'\t' {
                    break;
                }
                echo.extend_from_slice(&self.erase_char(termios));
            }
        }
        // Erase word characters
        while !self.canon_buf.is_empty() {
            if let Some(&last) = self.canon_buf.last() {
                if last == b' ' || last == b'\t' {
                    break;
                }
                echo.extend_from_slice(&self.erase_char(termios));
            }
        }
        echo
    }

    fn kill_line(&mut self, termios: &Termios) -> Vec<u8> {
        if self.canon_buf.is_empty() {
            return Vec::new();
        }

        if !termios.is_echo() {
            self.canon_buf.clear();
            return Vec::new();
        }

        if termios.c_lflag & ECHOKE != 0 && termios.c_lflag & ECHOE != 0 {
            let mut echo = Vec::new();
            while !self.canon_buf.is_empty() {
                echo.extend_from_slice(&self.erase_char(termios));
            }
            return echo;
        }

        if termios.c_lflag & ECHOK != 0 {
            self.canon_buf.clear();
            self.column = 0;
            return vec![b'\r', b'\n'];
        }

        let mut echo = Vec::new();
        if termios.c_lflag & ECHOCTL != 0 {
            echo.extend_from_slice(&[b'^', b'U']);
        }
        self.canon_buf.clear();
        echo
    }

    fn is_special_char(&self, c: u8, termios: &Termios) -> bool {
        c == termios.c_cc[VINTR] || c == termios.c_cc[VQUIT] || c == termios.c_cc[VSUSP]
    }

    fn output_process(&self, c: u8, termios: &Termios) -> Vec<u8> {
        if c == b'\n' && termios.c_oflag & ONLCR != 0 {
            return vec![b'\r', b'\n'];
        }
        if c == b'\r' {
            if termios.c_oflag & OCRNL != 0 {
                return vec![b'\n'];
            }
            if termios.c_oflag & ONOCR != 0 {
                return Vec::new();
            }
        }
        if c == b'\n' && termios.c_oflag & ONLRET != 0 {
            return vec![b'\n'];
        }
        vec![c]
    }

    fn canon_copy_to_read(&mut self) {
        for &b in &self.canon_buf {
            if self.read_cnt < LDISC_BUF_SIZE {
                self.read_buf[self.read_head % LDISC_BUF_SIZE] = b;
                self.read_head += 1;
                self.read_cnt += 1;
            }
        }
        self.canon_buf.clear();
        self.canon_head = 0;
    }
}

// ─── TTY Core ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TtyFlags {
    pub stopped: bool,
    pub hw_stopped: bool,
    pub flow_stopped: bool,
    pub closing: bool,
    pub exclusive: bool,
    pub no_carrier: bool,
}

impl TtyFlags {
    pub fn new() -> Self {
        Self {
            stopped: false,
            hw_stopped: false,
            flow_stopped: false,
            closing: false,
            exclusive: false,
            no_carrier: false,
        }
    }
}

pub struct TtyCtrl {
    pub canon_data: Vec<u8>,
    pub canon_head: usize,
    pub canon_column: u32,
    pub echo_buf: Vec<u8>,
}

impl TtyCtrl {
    pub fn new() -> Self {
        Self {
            canon_data: Vec::new(),
            canon_head: 0,
            canon_column: 0,
            echo_buf: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PtyPair {
    pub master_id: u32,
    pub slave_id: u32,
}

pub enum TtyDriver {
    Console(u32),
    Serial(u32),
    Pty(PtyPair),
    Virtual(u32),
}

pub struct Tty {
    index: u32,
    driver: TtyDriver,
    ldisc: LineDiscipline,
    termios: Termios,
    winsize: Winsize,
    session: Option<u64>,
    pgrp: Option<u64>,
    open_count: u32,
    flags: TtyFlags,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    ctrl: TtyCtrl,
    name: String,
}

impl Tty {
    pub fn new(index: u32, driver: TtyDriver) -> Self {
        let name = match &driver {
            TtyDriver::Console(n) => alloc::format!("tty{}", n),
            TtyDriver::Serial(n) => alloc::format!("ttyS{}", n),
            TtyDriver::Pty(pair) => alloc::format!("pts/{}", pair.slave_id),
            TtyDriver::Virtual(n) => alloc::format!("ttyV{}", n),
        };

        Self {
            index,
            driver,
            ldisc: LineDiscipline::new(),
            termios: Termios::default_cooked(),
            winsize: Winsize::default_console(),
            session: None,
            pgrp: None,
            open_count: 0,
            flags: TtyFlags::new(),
            read_buf: Vec::with_capacity(4096),
            write_buf: Vec::with_capacity(4096),
            ctrl: TtyCtrl::new(),
            name,
        }
    }

    pub fn open(&mut self) -> Result<(), TtyError> {
        if self.flags.exclusive && self.open_count > 0 {
            return Err(TtyError::DeviceBusy);
        }
        if self.flags.closing {
            return Err(TtyError::Hangup);
        }
        self.open_count += 1;
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), TtyError> {
        if self.open_count == 0 {
            return Err(TtyError::BadFileDescriptor);
        }
        self.open_count -= 1;
        if self.open_count == 0 {
            self.flags.closing = true;
            self.flush_input();
            self.flush_output();
            self.session = None;
            self.pgrp = None;
            self.flags.closing = false;
        }
        Ok(())
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, TtyError> {
        if self.flags.no_carrier {
            return Err(TtyError::Hangup);
        }
        let n = self.ldisc.read(buf, &self.termios);
        if n == 0 && self.termios.is_canonical() {
            return Err(TtyError::WouldBlock);
        }
        Ok(n)
    }

    pub fn write(&mut self, data: &[u8]) -> Result<usize, TtyError> {
        if self.flags.no_carrier {
            return Err(TtyError::Hangup);
        }
        if self.flags.stopped || self.flags.flow_stopped {
            return Err(TtyError::WouldBlock);
        }
        let output = self.ldisc.write_output(data, &self.termios);
        self.write_buf.extend_from_slice(&output);
        Ok(data.len())
    }

    pub fn ioctl(&mut self, cmd: u32, arg: u64) -> Result<u64, TtyError> {
        match cmd {
            TCGETS => Ok(0),
            TCSETS | TCSETSW | TCSETSF => {
                if cmd == TCSETSF {
                    self.flush_input();
                }
                Ok(0)
            }
            TIOCGWINSZ => Ok(((self.winsize.ws_row as u64) << 48)
                | ((self.winsize.ws_col as u64) << 32)
                | ((self.winsize.ws_xpixel as u64) << 16)
                | (self.winsize.ws_ypixel as u64)),
            TIOCSWINSZ => {
                self.winsize.ws_row = (arg >> 48) as u16;
                self.winsize.ws_col = (arg >> 32) as u16;
                self.winsize.ws_xpixel = (arg >> 16) as u16;
                self.winsize.ws_ypixel = arg as u16;
                Ok(0)
            }
            TIOCGPGRP => Ok(self.pgrp.unwrap_or(0)),
            TIOCSPGRP => {
                self.pgrp = Some(arg);
                Ok(0)
            }
            TIOCGSID => self.session.map(|s| s).ok_or(TtyError::NotATty),
            TIOCSCTTY => {
                self.session = Some(arg);
                self.pgrp = Some(arg);
                Ok(0)
            }
            TIOCNOTTY => {
                self.session = None;
                self.pgrp = None;
                Ok(0)
            }
            FIONREAD => Ok(self.ldisc.chars_in_buffer() as u64),
            _ => Err(TtyError::InvalidArg),
        }
    }

    pub fn poll(&self) -> PollFlags {
        let readable = self.ldisc.chars_in_buffer() > 0;
        let writable = !self.flags.stopped && !self.flags.flow_stopped;
        PollFlags {
            readable,
            writable,
            error: false,
            hangup: self.flags.no_carrier,
        }
    }

    pub fn set_termios(&mut self, termios: &Termios) {
        self.termios = termios.clone();
    }

    pub fn get_termios(&self) -> &Termios {
        &self.termios
    }

    pub fn set_winsize(&mut self, ws: Winsize) {
        self.winsize = ws;
    }

    pub fn flush_input(&mut self) {
        self.ldisc.flush_read();
        self.read_buf.clear();
    }

    pub fn flush_output(&mut self) {
        self.ldisc.flush_write();
        self.write_buf.clear();
    }

    pub fn send_signal(&self, _sig: u8) {
        // In a full kernel we'd look up the foreground pgrp and send the signal.
        // Stub: signal delivery will be wired when the process table integrates
        // with the TTY subsystem.
    }

    pub fn hangup(&mut self) {
        self.flags.no_carrier = true;
        self.flush_input();
        self.flush_output();
        self.session = None;
        self.pgrp = None;
    }

    pub fn receive_input(&mut self, data: &[u8]) {
        let actions = self.ldisc.receive_buf(data, &self.termios);
        for action in actions {
            match action {
                LdiscAction::Echo(bytes) => {
                    self.write_buf.extend_from_slice(&bytes);
                }
                LdiscAction::Signal(sig) => {
                    self.send_signal(sig);
                }
                LdiscAction::DataReady => {}
                LdiscAction::Output(bytes) => {
                    self.write_buf.extend_from_slice(&bytes);
                }
                LdiscAction::Flush => {
                    self.flush_input();
                }
                LdiscAction::None => {}
            }
        }
    }

    pub fn drain_output(&mut self) -> Vec<u8> {
        let data = self.write_buf.clone();
        self.write_buf.clear();
        data
    }
}

impl CharDevice for Tty {
    fn name(&self) -> &str {
        &self.name
    }

    fn read(&mut self, buf: &mut [u8], _offset: u64) -> Result<usize, TtyError> {
        Tty::read(self, buf)
    }

    fn write(&mut self, data: &[u8], _offset: u64) -> Result<usize, TtyError> {
        Tty::write(self, data)
    }

    fn ioctl(&mut self, cmd: u32, arg: u64) -> Result<u64, TtyError> {
        Tty::ioctl(self, cmd, arg)
    }

    fn poll(&self) -> PollFlags {
        Tty::poll(self)
    }

    fn open(&mut self) -> Result<(), TtyError> {
        Tty::open(self)
    }

    fn close(&mut self) -> Result<(), TtyError> {
        Tty::close(self)
    }
}

// ─── PTY (Pseudo-Terminal) ───────────────────────────────────────────────────

pub struct PtyMaster {
    id: u32,
    slave: u32,
    input_buf: Vec<u8>,
    output_buf: Vec<u8>,
    packet_mode: bool,
    locked: bool,
}

impl PtyMaster {
    pub fn new(id: u32, slave: u32) -> Self {
        Self {
            id,
            slave,
            input_buf: Vec::with_capacity(4096),
            output_buf: Vec::with_capacity(4096),
            packet_mode: false,
            locked: true,
        }
    }
}

impl CharDevice for PtyMaster {
    fn name(&self) -> &str {
        "ptmx"
    }

    fn read(&mut self, buf: &mut [u8], _offset: u64) -> Result<usize, TtyError> {
        if self.output_buf.is_empty() {
            return Err(TtyError::WouldBlock);
        }
        let to_copy = if self.output_buf.len() < buf.len() {
            self.output_buf.len()
        } else {
            buf.len()
        };
        buf[..to_copy].copy_from_slice(&self.output_buf[..to_copy]);
        self.output_buf.drain(..to_copy);
        Ok(to_copy)
    }

    fn write(&mut self, data: &[u8], _offset: u64) -> Result<usize, TtyError> {
        self.input_buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn ioctl(&mut self, cmd: u32, _arg: u64) -> Result<u64, TtyError> {
        match cmd {
            TIOCGWINSZ => Ok(0),
            _ => Err(TtyError::InvalidArg),
        }
    }

    fn poll(&self) -> PollFlags {
        PollFlags {
            readable: !self.output_buf.is_empty(),
            writable: true,
            error: false,
            hangup: false,
        }
    }

    fn open(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
}

pub struct PtySlave {
    id: u32,
    master: u32,
}

impl PtySlave {
    pub fn new(id: u32, master: u32) -> Self {
        Self { id, master }
    }
}

impl CharDevice for PtySlave {
    fn name(&self) -> &str {
        "pts"
    }

    fn read(&mut self, _buf: &mut [u8], _offset: u64) -> Result<usize, TtyError> {
        Err(TtyError::WouldBlock)
    }

    fn write(&mut self, _data: &[u8], _offset: u64) -> Result<usize, TtyError> {
        Ok(0)
    }

    fn ioctl(&mut self, _cmd: u32, _arg: u64) -> Result<u64, TtyError> {
        Err(TtyError::InvalidArg)
    }

    fn poll(&self) -> PollFlags {
        PollFlags::none()
    }

    fn open(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), TtyError> {
        Ok(())
    }
}

pub struct PtyManager {
    pairs: BTreeMap<u32, (PtyMaster, PtySlave)>,
    next_id: u32,
    winsize_map: BTreeMap<u32, Winsize>,
    termios_map: BTreeMap<u32, Termios>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            pairs: BTreeMap::new(),
            next_id: 0,
            winsize_map: BTreeMap::new(),
            termios_map: BTreeMap::new(),
        }
    }

    pub fn allocate(&mut self) -> Result<(u32, u32), TtyError> {
        let id = self.next_id;
        self.next_id += 1;

        let master_id = id;
        let slave_id = id;

        let master = PtyMaster::new(master_id, slave_id);
        let slave = PtySlave::new(slave_id, master_id);

        self.pairs.insert(id, (master, slave));
        self.winsize_map.insert(id, Winsize::default_console());
        self.termios_map.insert(id, Termios::default_cooked());

        Ok((master_id, slave_id))
    }

    pub fn deallocate(&mut self, id: u32) -> Result<(), TtyError> {
        if self.pairs.remove(&id).is_some() {
            self.winsize_map.remove(&id);
            self.termios_map.remove(&id);
            Ok(())
        } else {
            Err(TtyError::DeviceNotFound)
        }
    }

    pub fn master_write(&mut self, id: u32, data: &[u8]) -> Result<usize, TtyError> {
        let (master, _) = self.pairs.get_mut(&id).ok_or(TtyError::DeviceNotFound)?;
        master.input_buf.extend_from_slice(data);
        Ok(data.len())
    }

    pub fn master_read(&mut self, id: u32, buf: &mut [u8]) -> Result<usize, TtyError> {
        let (master, _) = self.pairs.get_mut(&id).ok_or(TtyError::DeviceNotFound)?;
        if master.output_buf.is_empty() {
            return Err(TtyError::WouldBlock);
        }
        let to_copy = core::cmp::min(master.output_buf.len(), buf.len());
        buf[..to_copy].copy_from_slice(&master.output_buf[..to_copy]);
        master.output_buf.drain(..to_copy);
        Ok(to_copy)
    }

    pub fn slave_write(&mut self, id: u32, data: &[u8]) -> Result<usize, TtyError> {
        let (master, _) = self.pairs.get_mut(&id).ok_or(TtyError::DeviceNotFound)?;
        master.output_buf.extend_from_slice(data);
        Ok(data.len())
    }

    pub fn slave_read(&mut self, id: u32, buf: &mut [u8]) -> Result<usize, TtyError> {
        let (master, _) = self.pairs.get_mut(&id).ok_or(TtyError::DeviceNotFound)?;
        if master.input_buf.is_empty() {
            return Err(TtyError::WouldBlock);
        }
        let to_copy = core::cmp::min(master.input_buf.len(), buf.len());
        buf[..to_copy].copy_from_slice(&master.input_buf[..to_copy]);
        master.input_buf.drain(..to_copy);
        Ok(to_copy)
    }

    pub fn resize(&mut self, id: u32, ws: Winsize) {
        self.winsize_map.insert(id, ws);
    }

    pub fn get_slave_name(&self, id: u32) -> String {
        alloc::format!("/dev/pts/{}", id)
    }

    pub fn unlock(&mut self, id: u32) -> Result<(), TtyError> {
        let (master, _) = self.pairs.get_mut(&id).ok_or(TtyError::DeviceNotFound)?;
        master.locked = false;
        Ok(())
    }

    pub fn is_locked(&self, id: u32) -> Result<bool, TtyError> {
        let (master, _) = self.pairs.get(&id).ok_or(TtyError::DeviceNotFound)?;
        Ok(master.locked)
    }

    pub fn set_packet_mode(&mut self, id: u32, enabled: bool) -> Result<(), TtyError> {
        let (master, _) = self.pairs.get_mut(&id).ok_or(TtyError::DeviceNotFound)?;
        master.packet_mode = enabled;
        Ok(())
    }

    pub fn pair_count(&self) -> usize {
        self.pairs.len()
    }

    pub fn master_pending(&self, id: u32) -> Result<usize, TtyError> {
        let (master, _) = self.pairs.get(&id).ok_or(TtyError::DeviceNotFound)?;
        Ok(master.output_buf.len())
    }
}

// PTY syscall wrappers

pub fn sys_openpt(_flags: u32) -> Result<u32, TtyError> {
    let mut sub = TTY_SUBSYSTEM.lock();
    let subsys = sub.as_mut().ok_or(TtyError::DeviceNotFound)?;
    let (master_id, _slave_id) = subsys.pty_manager.allocate()?;
    Ok(master_id)
}

pub fn sys_grantpt(fd: u32) -> Result<(), TtyError> {
    let sub = TTY_SUBSYSTEM.lock();
    let subsys = sub.as_ref().ok_or(TtyError::DeviceNotFound)?;
    if subsys.pty_manager.pairs.contains_key(&fd) {
        Ok(())
    } else {
        Err(TtyError::BadFileDescriptor)
    }
}

pub fn sys_unlockpt(fd: u32) -> Result<(), TtyError> {
    let mut sub = TTY_SUBSYSTEM.lock();
    let subsys = sub.as_mut().ok_or(TtyError::DeviceNotFound)?;
    subsys.pty_manager.unlock(fd)
}

pub fn sys_ptsname(fd: u32) -> Result<String, TtyError> {
    let sub = TTY_SUBSYSTEM.lock();
    let subsys = sub.as_ref().ok_or(TtyError::DeviceNotFound)?;
    if subsys.pty_manager.pairs.contains_key(&fd) {
        Ok(subsys.pty_manager.get_slave_name(fd))
    } else {
        Err(TtyError::BadFileDescriptor)
    }
}

// ─── Virtual Console ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ConsoleAttrs {
    pub bold: bool,
    pub dim: bool,
    pub underline: bool,
    pub blink: bool,
    pub reverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}

impl ConsoleAttrs {
    pub fn none() -> Self {
        Self {
            bold: false,
            dim: false,
            underline: false,
            blink: false,
            reverse: false,
            hidden: false,
            strikethrough: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConsoleChar {
    pub ch: char,
    pub fg: u8,
    pub bg: u8,
    pub attrs: ConsoleAttrs,
}

impl ConsoleChar {
    pub fn blank() -> Self {
        Self {
            ch: ' ',
            fg: 7,
            bg: 0,
            attrs: ConsoleAttrs::none(),
        }
    }

    pub fn new(ch: char, fg: u8, bg: u8) -> Self {
        Self {
            ch,
            fg,
            bg,
            attrs: ConsoleAttrs::none(),
        }
    }
}

pub struct ConsoleScreen {
    buffer: Vec<ConsoleChar>,
    width: u32,
    height: u32,
    cursor_x: u32,
    cursor_y: u32,
    cursor_visible: bool,
    fg_color: u8,
    bg_color: u8,
    attrs: ConsoleAttrs,
    scroll_top: u32,
    scroll_bottom: u32,
    saved_x: u32,
    saved_y: u32,
    charset: u8,
    tab_stops: Vec<u32>,
}

impl ConsoleScreen {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height) as usize;
        let mut tab_stops = Vec::new();
        let mut col = 0;
        while col < width {
            tab_stops.push(col);
            col += 8;
        }

        Self {
            buffer: vec![ConsoleChar::blank(); size],
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: true,
            fg_color: 7,
            bg_color: 0,
            attrs: ConsoleAttrs::none(),
            scroll_top: 0,
            scroll_bottom: height - 1,
            saved_x: 0,
            saved_y: 0,
            charset: 0,
            tab_stops,
        }
    }

    pub fn put_char(&mut self, x: u32, y: u32, c: ConsoleChar) {
        if x < self.width && y < self.height {
            let idx = (y * self.width + x) as usize;
            self.buffer[idx] = c;
        }
    }

    pub fn get_char(&self, x: u32, y: u32) -> ConsoleChar {
        if x < self.width && y < self.height {
            self.buffer[(y * self.width + x) as usize]
        } else {
            ConsoleChar::blank()
        }
    }

    pub fn scroll_up(&mut self, lines: u32) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let lines = core::cmp::min(lines, bottom - top + 1);

        for y in top..(bottom + 1 - lines) {
            for x in 0..self.width {
                let src = ((y + lines) * self.width + x) as usize;
                let dst = (y * self.width + x) as usize;
                self.buffer[dst] = self.buffer[src];
            }
        }

        for y in (bottom + 1 - lines)..=bottom {
            for x in 0..self.width {
                let idx = (y * self.width + x) as usize;
                self.buffer[idx] = ConsoleChar::blank();
            }
        }
    }

    pub fn scroll_down(&mut self, lines: u32) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let lines = core::cmp::min(lines, bottom - top + 1);

        for y in (top + lines..=bottom).rev() {
            for x in 0..self.width {
                let src = ((y - lines) * self.width + x) as usize;
                let dst = (y * self.width + x) as usize;
                self.buffer[dst] = self.buffer[src];
            }
        }

        for y in top..top + lines {
            for x in 0..self.width {
                let idx = (y * self.width + x) as usize;
                self.buffer[idx] = ConsoleChar::blank();
            }
        }
    }

    pub fn clear(&mut self) {
        for cell in self.buffer.iter_mut() {
            *cell = ConsoleChar::blank();
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    pub fn clear_line(&mut self, y: u32) {
        if y < self.height {
            for x in 0..self.width {
                self.buffer[(y * self.width + x) as usize] = ConsoleChar::blank();
            }
        }
    }

    pub fn clear_to_eol(&mut self) {
        for x in self.cursor_x..self.width {
            self.buffer[(self.cursor_y * self.width + x) as usize] = ConsoleChar::blank();
        }
    }

    pub fn clear_to_bol(&mut self) {
        for x in 0..=self.cursor_x {
            if x < self.width {
                self.buffer[(self.cursor_y * self.width + x) as usize] = ConsoleChar::blank();
            }
        }
    }

    pub fn newline(&mut self) {
        self.cursor_x = 0;
        if self.cursor_y >= self.scroll_bottom {
            self.scroll_up(1);
        } else {
            self.cursor_y += 1;
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor_x = 0;
    }

    pub fn backspace(&mut self) {
        if self.cursor_x > 0 {
            self.cursor_x -= 1;
        }
    }

    pub fn tab(&mut self) {
        let next = ((self.cursor_x / 8) + 1) * 8;
        self.cursor_x = core::cmp::min(next, self.width - 1);
    }

    pub fn save_cursor(&mut self) {
        self.saved_x = self.cursor_x;
        self.saved_y = self.cursor_y;
    }

    pub fn restore_cursor(&mut self) {
        self.cursor_x = self.saved_x;
        self.cursor_y = self.saved_y;
    }

    pub fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => self.newline(),
            '\r' => self.carriage_return(),
            '\x08' => self.backspace(),
            '\t' => self.tab(),
            _ => {
                if self.cursor_x >= self.width {
                    self.newline();
                }
                let c = ConsoleChar {
                    ch,
                    fg: self.fg_color,
                    bg: self.bg_color,
                    attrs: self.attrs,
                };
                self.put_char(self.cursor_x, self.cursor_y, c);
                self.cursor_x += 1;
            }
        }
    }

    pub fn write_bytes(&mut self, data: &[u8]) {
        for &b in data {
            self.write_char(b as char);
        }
    }

    pub fn set_colors(&mut self, fg: u8, bg: u8) {
        self.fg_color = fg;
        self.bg_color = bg;
    }

    pub fn reset_attrs(&mut self) {
        self.attrs = ConsoleAttrs::none();
        self.fg_color = 7;
        self.bg_color = 0;
    }
}

pub struct VirtualConsole {
    id: u32,
    tty: Tty,
    screen: ConsoleScreen,
    active: bool,
    blanked: bool,
}

impl VirtualConsole {
    pub fn new(id: u32, width: u32, height: u32) -> Self {
        let tty = Tty::new(id, TtyDriver::Console(id));
        Self {
            id,
            tty,
            screen: ConsoleScreen::new(width, height),
            active: false,
            blanked: false,
        }
    }

    pub fn write_data(&mut self, data: &[u8]) {
        if self.blanked {
            return;
        }
        self.screen.write_bytes(data);
    }

    pub fn read_data(&mut self, buf: &mut [u8]) -> usize {
        self.tty.read(buf).unwrap_or(0)
    }
}

pub struct ConsoleManager {
    consoles: Vec<VirtualConsole>,
    active: usize,
    max_consoles: usize,
}

impl ConsoleManager {
    pub fn new(count: usize, width: u32, height: u32) -> Self {
        let mut consoles = Vec::with_capacity(count);
        for i in 0..count {
            let mut vc = VirtualConsole::new(i as u32, width, height);
            if i == 0 {
                vc.active = true;
            }
            consoles.push(vc);
        }
        Self {
            consoles,
            active: 0,
            max_consoles: count,
        }
    }

    pub fn switch_console(&mut self, id: usize) -> Result<(), TtyError> {
        if id >= self.consoles.len() {
            return Err(TtyError::InvalidArg);
        }
        self.consoles[self.active].active = false;
        self.active = id;
        self.consoles[self.active].active = true;
        Ok(())
    }

    pub fn active_console(&self) -> &VirtualConsole {
        &self.consoles[self.active]
    }

    pub fn active_console_mut(&mut self) -> &mut VirtualConsole {
        &mut self.consoles[self.active]
    }

    pub fn write_to_console(&mut self, id: usize, data: &[u8]) {
        if id < self.consoles.len() {
            self.consoles[id].write_data(data);
        }
    }

    pub fn read_from_console(&mut self, id: usize, buf: &mut [u8]) -> usize {
        if id < self.consoles.len() {
            self.consoles[id].read_data(buf)
        } else {
            0
        }
    }

    pub fn blank_screen(&mut self) {
        for vc in &mut self.consoles {
            vc.blanked = true;
        }
    }

    pub fn unblank_screen(&mut self) {
        for vc in &mut self.consoles {
            vc.blanked = false;
        }
    }

    pub fn console_count(&self) -> usize {
        self.consoles.len()
    }
}

// ─── DevFS ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DevFsEntry {
    CharDev { major: u16, minor: u32, mode: u16 },
    BlockDev { major: u16, minor: u32, mode: u16 },
    Directory,
    Symlink(String),
}

pub struct DevFs {
    entries: BTreeMap<String, DevFsEntry>,
}

impl DevFs {
    pub fn new() -> Self {
        let mut fs = Self {
            entries: BTreeMap::new(),
        };
        fs.populate_defaults();
        fs
    }

    pub fn mknod(&mut self, path: &str, entry: DevFsEntry) -> Result<(), TtyError> {
        if self.entries.contains_key(path) {
            return Err(TtyError::DeviceExists);
        }
        self.entries.insert(String::from(path), entry);
        Ok(())
    }

    pub fn remove(&mut self, path: &str) -> Result<(), TtyError> {
        if self.entries.remove(path).is_some() {
            Ok(())
        } else {
            Err(TtyError::DeviceNotFound)
        }
    }

    pub fn lookup(&self, path: &str) -> Option<&DevFsEntry> {
        self.entries.get(path)
    }

    pub fn readdir(&self, path: &str) -> Vec<(String, &DevFsEntry)> {
        let prefix = if path.ends_with('/') {
            String::from(path)
        } else {
            alloc::format!("{}/", path)
        };

        self.entries
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .filter(|(k, _)| {
                let rest = &k[prefix.len()..];
                !rest.contains('/')
            })
            .map(|(k, v)| {
                let name = k[prefix.len()..].to_string();
                (name, v)
            })
            .collect()
    }

    fn populate_defaults(&mut self) {
        // Root directory
        let _ = self.mknod("/dev", DevFsEntry::Directory);
        let _ = self.mknod("/dev/pts", DevFsEntry::Directory);

        // Memory devices (major 1)
        let _ = self.mknod(
            "/dev/null",
            DevFsEntry::CharDev {
                major: MAJOR_MEM,
                minor: 3,
                mode: 0o666,
            },
        );
        let _ = self.mknod(
            "/dev/zero",
            DevFsEntry::CharDev {
                major: MAJOR_MEM,
                minor: 5,
                mode: 0o666,
            },
        );
        let _ = self.mknod(
            "/dev/full",
            DevFsEntry::CharDev {
                major: MAJOR_MEM,
                minor: 7,
                mode: 0o666,
            },
        );
        let _ = self.mknod(
            "/dev/random",
            DevFsEntry::CharDev {
                major: MAJOR_MEM,
                minor: 8,
                mode: 0o666,
            },
        );
        let _ = self.mknod(
            "/dev/urandom",
            DevFsEntry::CharDev {
                major: MAJOR_MEM,
                minor: 9,
                mode: 0o666,
            },
        );
        let _ = self.mknod(
            "/dev/kmsg",
            DevFsEntry::CharDev {
                major: MAJOR_MEM,
                minor: 11,
                mode: 0o644,
            },
        );

        // TTY devices (major 4)
        for i in 0..8 {
            let path = alloc::format!("/dev/tty{}", i);
            let _ = self.mknod(
                &path,
                DevFsEntry::CharDev {
                    major: MAJOR_TTY,
                    minor: i,
                    mode: 0o620,
                },
            );
        }

        // Console & controlling TTY (major 5)
        let _ = self.mknod(
            "/dev/tty",
            DevFsEntry::CharDev {
                major: MAJOR_CONSOLE,
                minor: 0,
                mode: 0o666,
            },
        );
        let _ = self.mknod(
            "/dev/console",
            DevFsEntry::CharDev {
                major: MAJOR_CONSOLE,
                minor: 1,
                mode: 0o600,
            },
        );

        // PTY master (major 128)
        let _ = self.mknod(
            "/dev/ptmx",
            DevFsEntry::CharDev {
                major: MAJOR_PTY_MASTER,
                minor: 0,
                mode: 0o666,
            },
        );

        // Symlinks
        let _ = self.mknod(
            "/dev/stdin",
            DevFsEntry::Symlink(String::from("/proc/self/fd/0")),
        );
        let _ = self.mknod(
            "/dev/stdout",
            DevFsEntry::Symlink(String::from("/proc/self/fd/1")),
        );
        let _ = self.mknod(
            "/dev/stderr",
            DevFsEntry::Symlink(String::from("/proc/self/fd/2")),
        );
        let _ = self.mknod(
            "/dev/fd",
            DevFsEntry::Symlink(String::from("/proc/self/fd")),
        );
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn create_pty_node(&mut self, id: u32) -> Result<(), TtyError> {
        let path = alloc::format!("/dev/pts/{}", id);
        self.mknod(
            &path,
            DevFsEntry::CharDev {
                major: MAJOR_PTY_SLAVE,
                minor: id,
                mode: 0o620,
            },
        )
    }

    pub fn remove_pty_node(&mut self, id: u32) -> Result<(), TtyError> {
        let path = alloc::format!("/dev/pts/{}", id);
        self.remove(&path)
    }
}

// ─── TTY Subsystem (global state) ────────────────────────────────────────────

pub struct TtySubsystem {
    pub registry: CharDeviceRegistry,
    pub pty_manager: PtyManager,
    pub console_manager: ConsoleManager,
    pub devfs: DevFs,
}

impl TtySubsystem {
    pub fn new() -> Self {
        Self {
            registry: CharDeviceRegistry::new(),
            pty_manager: PtyManager::new(),
            console_manager: ConsoleManager::new(8, 80, 25),
            devfs: DevFs::new(),
        }
    }

    pub fn register_special_devices(&mut self) {
        let _ = self.registry.register(MAJOR_MEM, 3, Box::new(NullDevice));
        let _ = self.registry.register(MAJOR_MEM, 5, Box::new(ZeroDevice));
        let _ = self.registry.register(MAJOR_MEM, 7, Box::new(FullDevice));
        let _ = self
            .registry
            .register(MAJOR_MEM, 9, Box::new(RandomDevice::new()));
        let _ = self
            .registry
            .register(MAJOR_MEM, 11, Box::new(KmsgDevice::new()));
    }

    pub fn open_device(&mut self, path: &str) -> Result<DeviceId, TtyError> {
        let entry = self
            .devfs
            .lookup(path)
            .ok_or(TtyError::DeviceNotFound)?
            .clone();

        match entry {
            DevFsEntry::CharDev { major, minor, .. } => {
                let id = DeviceId::new(major, minor);
                if let Some(dev) = self.registry.get_mut(&id) {
                    dev.open()?;
                    Ok(id)
                } else {
                    Err(TtyError::DeviceNotFound)
                }
            }
            DevFsEntry::Symlink(target) => self.open_device(&target),
            _ => Err(TtyError::NotATty),
        }
    }

    pub fn close_device(&mut self, id: &DeviceId) -> Result<(), TtyError> {
        if let Some(dev) = self.registry.get_mut(id) {
            dev.close()
        } else {
            Err(TtyError::DeviceNotFound)
        }
    }

    pub fn read_device(&mut self, id: &DeviceId, buf: &mut [u8]) -> Result<usize, TtyError> {
        if let Some(dev) = self.registry.get_mut(id) {
            dev.read(buf, 0)
        } else {
            Err(TtyError::DeviceNotFound)
        }
    }

    pub fn write_device(&mut self, id: &DeviceId, data: &[u8]) -> Result<usize, TtyError> {
        if let Some(dev) = self.registry.get_mut(id) {
            dev.write(data, 0)
        } else {
            Err(TtyError::DeviceNotFound)
        }
    }

    pub fn ioctl_device(&mut self, id: &DeviceId, cmd: u32, arg: u64) -> Result<u64, TtyError> {
        if let Some(dev) = self.registry.get_mut(id) {
            dev.ioctl(cmd, arg)
        } else {
            Err(TtyError::DeviceNotFound)
        }
    }

    pub fn poll_device(&self, id: &DeviceId) -> Result<PollFlags, TtyError> {
        if let Some(dev) = self.registry.get(id) {
            Ok(dev.poll())
        } else {
            Err(TtyError::DeviceNotFound)
        }
    }
}

pub static TTY_SUBSYSTEM: Mutex<Option<TtySubsystem>> = Mutex::new(None);

// ─── Initialization ──────────────────────────────────────────────────────────

pub fn init() {
    let mut subsys = TtySubsystem::new();

    subsys.register_special_devices();

    // Register VT console TTYs
    for i in 0..8 {
        let tty = Tty::new(i, TtyDriver::Console(i));
        let _ = subsys.registry.register(MAJOR_TTY, i, Box::new(tty));
    }

    *TTY_SUBSYSTEM.lock() = Some(subsys);
}
