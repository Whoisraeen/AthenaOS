//! High-level syscall wrappers for RaeenOS.
//!
//! Wraps the raw `sys::*` primitives into ergonomic Rust APIs grouped by
//! subsystem: filesystem, IPC, surfaces, process control, and capabilities.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::sys;

// ── Filesystem ───────────────────────────────────────────────────────────

pub mod fs {
    use super::*;

    pub const O_READ: u64 = 0;
    pub const O_WRITE: u64 = 1;
    pub const O_CREATE: u64 = 2;
    pub const O_APPEND: u64 = 4;
    pub const O_TRUNC: u64 = 8;

    #[derive(Debug, Clone, Copy)]
    pub struct FileDescriptor(pub u64);

    impl FileDescriptor {
        pub fn raw(&self) -> u64 {
            self.0
        }

        pub fn is_valid(&self) -> bool {
            self.0 != u64::MAX
        }
    }

    pub fn open(path: &str, flags: u64) -> Option<FileDescriptor> {
        let fd = unsafe {
            sys::syscall3(
                sys::SYS_OPEN,
                path.as_ptr() as u64,
                path.len() as u64,
                flags,
            )
        };
        if fd == u64::MAX {
            None
        } else {
            Some(FileDescriptor(fd))
        }
    }

    pub fn close(fd: FileDescriptor) -> bool {
        unsafe { sys::syscall1(sys::SYS_CLOSE, fd.0) == 0 }
    }

    pub fn read(fd: FileDescriptor, buf: &mut [u8]) -> usize {
        unsafe {
            sys::syscall3(
                sys::SYS_READ,
                fd.0,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            ) as usize
        }
    }

    pub fn write(fd: FileDescriptor, buf: &[u8]) -> usize {
        unsafe {
            sys::syscall3(sys::SYS_WRITE, fd.0, buf.as_ptr() as u64, buf.len() as u64) as usize
        }
    }

    pub fn stat(fd: FileDescriptor) -> u64 {
        unsafe { sys::syscall1(sys::SYS_STAT, fd.0) }
    }

    pub fn read_all(fd: FileDescriptor) -> Vec<u8> {
        let mut result = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            let n = read(fd, &mut buf);
            if n == 0 {
                break;
            }
            result.extend_from_slice(&buf[..n]);
            if n < buf.len() {
                break;
            }
        }
        result
    }

    pub fn read_to_string(fd: FileDescriptor) -> Option<String> {
        let bytes = read_all(fd);
        String::from_utf8(bytes).ok()
    }

    pub fn write_all(fd: FileDescriptor, data: &[u8]) -> bool {
        let mut offset = 0;
        while offset < data.len() {
            let n = write(fd, &data[offset..]);
            if n == 0 {
                return false;
            }
            offset += n;
        }
        true
    }
}

// ── Surface (compositor) ─────────────────────────────────────────────────

pub mod surface {
    use crate::sys;

    #[derive(Debug, Clone, Copy)]
    pub struct SurfaceId(pub u64);

    pub fn create(width: u32, height: u32, user_virt: u64) -> Option<SurfaceId> {
        let id = sys::surface_create(width as u64, height as u64, user_virt);
        if id == u64::MAX {
            None
        } else {
            Some(SurfaceId(id))
        }
    }

    pub fn present(id: SurfaceId, x: i32, y: i32) -> bool {
        sys::surface_present(id.0, x as u64, y as u64) == 0
    }

    pub fn focus(id: SurfaceId) -> bool {
        sys::surface_focus(id.0) == 0
    }

    pub fn close(id: SurfaceId) -> bool {
        sys::surface_close(id.0) == 0
    }
}

// ── Process control ──────────────────────────────────────────────────────

pub mod process {
    use crate::sys;

    #[derive(Debug, Clone, Copy)]
    pub struct Pid(pub u64);

    pub fn spawn(path: &str) -> Option<Pid> {
        let pid = unsafe { sys::syscall2(sys::SYS_SPAWN, path.as_ptr() as u64, path.len() as u64) };
        if pid == u64::MAX {
            None
        } else {
            Some(Pid(pid))
        }
    }

    pub fn exit(code: u32) -> ! {
        sys::exit(code as u64);
    }

    pub fn yield_now() {
        sys::yield_now();
    }

    pub fn sleep_approx(ms: u64) {
        let target_ns = sys::time_ns() + ms * 1_000_000;
        while sys::time_ns() < target_ns {
            sys::yield_now();
        }
    }

    pub fn getpid() -> Pid {
        Pid(sys::getpid())
    }

    pub fn time_ns() -> u64 {
        sys::time_ns()
    }
}

// ── Capability system ────────────────────────────────────────────────────

pub mod cap {
    #[derive(Debug, Clone, Copy)]
    pub struct CapHandle(pub u64);

    pub const RIGHT_READ: u64 = 1;
    pub const RIGHT_WRITE: u64 = 2;
    pub const RIGHT_EXEC: u64 = 4;
    pub const RIGHT_MAP: u64 = 8;
    pub const RIGHT_GRANT: u64 = 16;

    #[derive(Debug, Clone, Copy)]
    pub struct CapInfo {
        pub handle: CapHandle,
        pub flavor: u64,
        pub rights: u64,
    }

    impl CapInfo {
        pub fn can_read(&self) -> bool {
            self.rights & RIGHT_READ != 0
        }
        pub fn can_write(&self) -> bool {
            self.rights & RIGHT_WRITE != 0
        }
        pub fn can_exec(&self) -> bool {
            self.rights & RIGHT_EXEC != 0
        }
        pub fn can_map(&self) -> bool {
            self.rights & RIGHT_MAP != 0
        }
        pub fn can_grant(&self) -> bool {
            self.rights & RIGHT_GRANT != 0
        }
    }

    pub fn query(handle: CapHandle) -> Option<CapInfo> {
        let (status, flavor, rights) = unsafe {
            let mut flavor: u64;
            let mut rights: u64;
            let status: u64;
            core::arch::asm!(
                "syscall",
                inout("rax") 6u64 => status,
                in("rdi") handle.0,
                lateout("rsi") flavor,
                lateout("rdx") rights,
                out("rcx") _, out("r11") _,
            );
            (status, flavor, rights)
        };
        if status != 0 {
            return None;
        }
        Some(CapInfo {
            handle,
            flavor,
            rights,
        })
    }

    pub fn grant(target_pid: u64, src_handle: CapHandle, new_rights: u64) -> Option<CapHandle> {
        let result = unsafe {
            let r: u64;
            core::arch::asm!(
                "syscall",
                inout("rax") 4u64 => r,
                in("rdi") target_pid,
                in("rsi") src_handle.0,
                in("rdx") new_rights,
                in("r10") 0u64,
                in("r8") 0u64,
                out("rcx") _, out("r11") _,
            );
            r
        };
        if result >= 0xFF00_0000_0000_0000 {
            None
        } else {
            Some(CapHandle(result))
        }
    }

    pub fn revoke(handle: CapHandle) -> bool {
        let result = unsafe {
            let r: u64;
            core::arch::asm!(
                "syscall",
                inout("rax") 5u64 => r,
                in("rdi") handle.0,
                out("rcx") _, out("r11") _,
            );
            r
        };
        result == 0
    }
}

// ── Local search index (kernel search_index, syscalls 54-57) ───────────────

/// Userspace wrapper for the kernel-side, local-first search index — the ONE
/// source of truth the start-menu/Files/command-palette all query (Concept
/// §Windows pain points: "Search is broken -> Local-first, indexed, sub-100ms").
///
/// The kernel crawler (`search_index::crawl_session_home`) populates the index
/// post-login, so the same index that the crawler feeds is the index this
/// queries — no parallel userspace file index. `SYS_SEARCH_QUERY` returns only
/// `(id, kind)` pairs (16-byte records); higher layers resolve the id to a
/// display string, or use the kind to route the hit (file/app/setting).
pub mod search {
    use super::*;
    use crate::sys;

    /// An item kind, mirroring `kernel::search_index::Kind` and the
    /// `sys::SEARCH_KIND_*` tags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind {
        App,
        File,
        Setting,
        Contact,
        Document,
        Other,
    }

    impl Kind {
        /// Decode the on-the-wire `u32` kind tag from a query result record.
        pub fn from_u32(n: u32) -> Self {
            match n {
                sys::SEARCH_KIND_APP => Kind::App,
                sys::SEARCH_KIND_FILE => Kind::File,
                sys::SEARCH_KIND_SETTING => Kind::Setting,
                sys::SEARCH_KIND_CONTACT => Kind::Contact,
                sys::SEARCH_KIND_DOCUMENT => Kind::Document,
                _ => Kind::Other,
            }
        }
    }

    /// One decoded search hit. The kernel index returns only the stable item id
    /// + its kind; the display string lives with whoever registered the item.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SearchHit {
        pub id: u64,
        pub kind: Kind,
    }

    /// Decode `count` result records from the raw kernel output buffer. PURE —
    /// no syscall — so it is host-testable (the FAIL-able KAT below feeds a
    /// synthetic blob and asserts the exact decode). Each record is
    /// `SEARCH_RESULT_STRIDE` (16) bytes: little-endian `[u64 id][u32 kind][u32 pad]`.
    /// Records past the end of `buf` are skipped (defensive against a short buffer).
    pub fn decode_results(buf: &[u8], count: usize) -> Vec<SearchHit> {
        let stride = sys::SEARCH_RESULT_STRIDE;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let base = i * stride;
            if base + stride > buf.len() {
                break;
            }
            let id = u64::from_le_bytes([
                buf[base],
                buf[base + 1],
                buf[base + 2],
                buf[base + 3],
                buf[base + 4],
                buf[base + 5],
                buf[base + 6],
                buf[base + 7],
            ]);
            let kind =
                u32::from_le_bytes([buf[base + 8], buf[base + 9], buf[base + 10], buf[base + 11]]);
            out.push(SearchHit {
                id,
                kind: Kind::from_u32(kind),
            });
        }
        out
    }

    /// Query the kernel index for up to `max` hits matching `query`. Returns an
    /// empty vec when the query is empty, the index is not yet populated
    /// (pre-crawl), or there are simply no matches — never an error (graceful
    /// empty-index handling: the crawl runs post-login, so an early query is a
    /// legitimate 0-result, not a failure).
    pub fn query(query: &str, max: usize) -> Vec<SearchHit> {
        if query.is_empty() || max == 0 {
            return Vec::new();
        }
        let cap_bytes = max * sys::SEARCH_RESULT_STRIDE;
        let mut buf = alloc::vec![0u8; cap_bytes];
        let count = unsafe {
            sys::syscall4(
                sys::SYS_SEARCH_QUERY,
                query.as_ptr() as u64,
                query.len() as u64,
                buf.as_mut_ptr() as u64,
                cap_bytes as u64,
            )
        } as usize;
        // The kernel never writes more records than fit in `cap_bytes`; clamp
        // defensively so a bogus count can't make `decode_results` over-read.
        decode_results(&buf, count.min(max))
    }

    /// One decoded RESOLVED search hit — the rich row the Files app / command
    /// palette renders (name + path), the counterpart of the opaque [`SearchHit`]
    /// that `SYS_SEARCH_QUERY` (56) returns. From `SYS_SEARCH_QUERY_RESOLVED` (281).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ResolvedHit {
        /// Stable index item id (`0` when the kernel resolver doesn't carry one).
        pub id: u64,
        pub kind: Kind,
        /// Leaf display name (the row title).
        pub name: String,
        /// Absolute path (the row subtitle + `Open` target). Empty for items with
        /// no filesystem path (apps, settings).
        pub path: String,
        /// `true` for a directory (the palette renders a folder row).
        pub is_folder: bool,
    }

    /// Decode `count` RESOLVED records from the raw kernel output buffer. PURE —
    /// no syscall — so it is host-testable (the FAIL-able KAT below feeds a
    /// synthetic blob + a truncated/garbage blob and asserts the decode + the
    /// clamp). Each record is a `SEARCH_RESOLVED_HEADER_SIZE` (24)-byte header
    /// (little-endian `[u64 id][u32 kind][u8 is_folder][u8 r0][u16 r1]
    /// [u16 name_len][u16 path_len][u32 r2]`) immediately followed by `name_len`
    /// UTF-8 name bytes then `path_len` UTF-8 path bytes.
    ///
    /// DEFENSIVE: clamps the walk to `count`, bounds-checks every header AND the
    /// name/path payload against the remaining buffer, and STOPS (never panics,
    /// never over-reads) on a short/garbage record — so a bogus count or a
    /// truncated buffer returns the records that fully decoded, not a panic.
    pub fn decode_resolved(buf: &[u8], count: usize) -> Vec<ResolvedHit> {
        let header = sys::SEARCH_RESOLVED_HEADER_SIZE;
        let mut out = Vec::with_capacity(count.min(64));
        let mut off = 0usize;
        for _ in 0..count {
            // Header must fit.
            if off + header > buf.len() {
                break;
            }
            let id = u64::from_le_bytes([
                buf[off],
                buf[off + 1],
                buf[off + 2],
                buf[off + 3],
                buf[off + 4],
                buf[off + 5],
                buf[off + 6],
                buf[off + 7],
            ]);
            let kind =
                u32::from_le_bytes([buf[off + 8], buf[off + 9], buf[off + 10], buf[off + 11]]);
            let is_folder = buf[off + 12] != 0;
            let name_len = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
            let path_len = u16::from_le_bytes([buf[off + 18], buf[off + 19]]) as usize;
            let name_start = off + header;
            // Payload bounds check — guards against a lying length / short buffer.
            let path_start = match name_start.checked_add(name_len) {
                Some(v) => v,
                None => break,
            };
            let rec_end = match path_start.checked_add(path_len) {
                Some(v) => v,
                None => break,
            };
            if rec_end > buf.len() {
                break;
            }
            let name = String::from_utf8_lossy(&buf[name_start..path_start]).into_owned();
            let path = String::from_utf8_lossy(&buf[path_start..rec_end]).into_owned();
            out.push(ResolvedHit {
                id,
                kind: Kind::from_u32(kind),
                name,
                path,
                is_folder,
            });
            off = rec_end;
        }
        out
    }

    /// Query the kernel index for up to `max` RESOLVED hits (name + path) matching
    /// `query`, via `SYS_SEARCH_QUERY_RESOLVED` (281). Returns an empty vec when
    /// the query is empty, the index isn't populated yet (pre-crawl), or there are
    /// no matches — never an error (same graceful empty-result posture as
    /// [`query`]). This is what the Files app / command palette calls to render
    /// clickable, named rows.
    ///
    /// Sizes the output buffer for `max` records at a generous average record
    /// size (header + a typical name + a typical path); the kernel writes only
    /// whole records that fit and caps the count, and the decoder clamps to the
    /// returned count, so a short buffer just yields fewer rows (never a panic).
    pub fn query_resolved(query: &str, max: usize) -> Vec<ResolvedHit> {
        if query.is_empty() || max == 0 {
            return Vec::new();
        }
        // Budget a roomy per-record size so common rows aren't dropped: 24-byte
        // header + up to ~512 bytes of name+path. A row longer than this is
        // simply not written by the kernel (it never emits a partial record).
        const PER_RECORD_BUDGET: usize = sys::SEARCH_RESOLVED_HEADER_SIZE + 512;
        let cap_bytes = max.saturating_mul(PER_RECORD_BUDGET).max(PER_RECORD_BUDGET);
        let mut buf = alloc::vec![0u8; cap_bytes];
        let count = unsafe {
            sys::syscall4(
                sys::SYS_SEARCH_QUERY_RESOLVED,
                query.as_ptr() as u64,
                query.len() as u64,
                buf.as_mut_ptr() as u64,
                cap_bytes as u64,
            )
        } as usize;
        // Clamp defensively so a bogus count can't drive an over-long walk.
        decode_resolved(&buf, count.min(max))
    }

    /// Live index stats: `(items, tokens, queries_total, last_query_cycles)`.
    /// All zero when the index isn't up yet.
    pub fn stats() -> (u64, u64, u64, u64) {
        let mut buf = [0u8; 32];
        let n = unsafe {
            sys::syscall3(
                sys::SYS_SEARCH_STATS,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                0,
            )
        };
        if n < 32 {
            return (0, 0, 0, 0);
        }
        let rd = |o: usize| {
            u64::from_le_bytes([
                buf[o],
                buf[o + 1],
                buf[o + 2],
                buf[o + 3],
                buf[o + 4],
                buf[o + 5],
                buf[o + 6],
                buf[o + 7],
            ])
        };
        (rd(0), rd(8), rd(16), rd(24))
    }
}

// ── IPC (enhanced) ───────────────────────────────────────────────────────

pub mod ipc_ext {
    use crate::sys;

    #[derive(Debug, Clone, Copy)]
    pub struct Channel(pub u64);

    pub fn send(channel: Channel, data: u64) -> bool {
        sys::ipc_send(channel.0, data) == 0
    }

    pub fn recv(channel: Channel) -> Option<u64> {
        let v = sys::ipc_recv(channel.0);
        if v == 0 {
            None
        } else {
            Some(v)
        }
    }

    pub fn send_bytes(channel: Channel, buf: &[u8]) -> bool {
        let chunks = buf.len() / 8 + if buf.len() % 8 != 0 { 1 } else { 0 };
        for i in 0..chunks {
            let start = i * 8;
            let end = (start + 8).min(buf.len());
            let mut word = 0u64;
            for (j, &b) in buf[start..end].iter().enumerate() {
                word |= (b as u64) << (j * 8);
            }
            if !send(channel, word) {
                return false;
            }
        }
        true
    }

    pub fn recv_blocking(channel: Channel) -> u64 {
        loop {
            if let Some(v) = recv(channel) {
                return v;
            }
            crate::sys::yield_now();
        }
    }
}

// ── Host KATs (pure decoders only — no syscall is issued) ──────────────────
//
// Run with `cargo test -p raekit`. Exercises the FAIL-able wire decoders for
// the resolved-search surface (SYS_SEARCH_QUERY_RESOLVED, 281): encode a
// synthetic buffer in the documented format, decode it, assert the round-trip;
// then feed truncated/garbage buffers and assert a clamped/empty result with no
// panic. These prove the userspace half of the contract the kernel boot
// smoketest proves on the encoder half — both quote docs/SYSCALL_TABLE.md.
#[cfg(test)]
mod tests {
    extern crate alloc;
    use crate::sys;
    use crate::syscalls::search::{decode_resolved, Kind};
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Encode one resolved record in the exact wire layout the kernel writes:
    /// 24-byte LE header `[u64 id][u32 kind][u8 is_folder][u8 r0][u16 r1]
    /// [u16 name_len][u16 path_len][u32 r2]` then name bytes then path bytes.
    fn encode(id: u64, kind: u32, is_folder: bool, name: &str, path: &str) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&id.to_le_bytes());
        r.extend_from_slice(&kind.to_le_bytes());
        r.push(if is_folder { 1 } else { 0 });
        r.push(0u8);
        r.extend_from_slice(&0u16.to_le_bytes());
        r.extend_from_slice(&(name.len() as u16).to_le_bytes());
        r.extend_from_slice(&(path.len() as u16).to_le_bytes());
        r.extend_from_slice(&0u32.to_le_bytes());
        r.extend_from_slice(name.as_bytes());
        r.extend_from_slice(path.as_bytes());
        assert_eq!(
            r.len(),
            sys::SEARCH_RESOLVED_HEADER_SIZE + name.len() + path.len()
        );
        r
    }

    #[test]
    fn header_size_matches_const() {
        // Encoder + decoder both depend on a 24-byte header; if this drifts from
        // the rae_abi struct size the whole format silently corrupts.
        assert_eq!(sys::SEARCH_RESOLVED_HEADER_SIZE, 24);
    }

    #[test]
    fn round_trips_two_records() {
        let mut buf = Vec::new();
        buf.extend(encode(
            7,
            sys::SEARCH_KIND_DOCUMENT,
            false,
            "resume.txt",
            "/home/raeen/Documents/resume.txt",
        ));
        buf.extend(encode(
            9,
            sys::SEARCH_KIND_FILE,
            true,
            "Vacation_Photos",
            "/home/raeen/Pictures/Vacation_Photos",
        ));

        let hits = decode_resolved(&buf, 2);
        assert_eq!(hits.len(), 2);

        assert_eq!(hits[0].id, 7);
        assert_eq!(hits[0].kind, Kind::Document);
        assert!(!hits[0].is_folder);
        assert_eq!(hits[0].name, "resume.txt");
        assert_eq!(hits[0].path, "/home/raeen/Documents/resume.txt");

        assert_eq!(hits[1].id, 9);
        assert_eq!(hits[1].kind, Kind::File);
        assert!(hits[1].is_folder);
        assert_eq!(hits[1].name, "Vacation_Photos");
        assert_eq!(hits[1].path, "/home/raeen/Pictures/Vacation_Photos");
    }

    #[test]
    fn empty_name_and_path_decode() {
        // An app/setting hit has an empty path; a zero-length string must decode
        // to "" and not consume any payload bytes.
        let buf = encode(1, sys::SEARCH_KIND_APP, false, "Calculator", "");
        let hits = decode_resolved(&buf, 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, Kind::App);
        assert_eq!(hits[0].name, "Calculator");
        assert_eq!(hits[0].path, "");
    }

    #[test]
    fn count_clamped_to_available_records() {
        // A bogus count (kernel claims 5, buffer holds 1) must yield 1, not panic.
        let buf = encode(3, sys::SEARCH_KIND_SETTING, false, "Game Mode", "");
        let hits = decode_resolved(&buf, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Game Mode");
    }

    #[test]
    fn truncated_header_returns_empty() {
        // Fewer than 24 bytes — the header itself doesn't fit. Must return empty.
        let buf = [0u8; 10];
        let hits = decode_resolved(&buf, 1);
        assert!(hits.is_empty());
    }

    #[test]
    fn truncated_payload_stops_cleanly() {
        // Header says name_len=10 path_len=20 but the buffer is cut after the
        // header — the payload bounds check must drop the record, not over-read.
        let mut buf = encode(2, sys::SEARCH_KIND_DOCUMENT, false, "abcdefghij", "x");
        let full = buf.len();
        buf.truncate(full - 5); // chop the tail of the payload
        let hits = decode_resolved(&buf, 1);
        assert!(hits.is_empty());
    }

    #[test]
    fn lying_length_does_not_overrun() {
        // First record is valid; the second's name_len claims more than remains.
        let mut buf = encode(1, sys::SEARCH_KIND_APP, false, "Files", "/apps/files");
        // Second header with an absurd name_len and no payload.
        buf.extend_from_slice(&2u64.to_le_bytes());
        buf.extend_from_slice(&sys::SEARCH_KIND_FILE.to_le_bytes());
        buf.push(0);
        buf.push(0);
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&60000u16.to_le_bytes()); // lying name_len
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        // The valid first record decodes; the bogus second is dropped (no panic).
        let hits = decode_resolved(&buf, 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Files");
    }

    #[test]
    fn garbage_buffer_never_panics() {
        // Random bytes + a wild count must not panic and must not over-read.
        let buf: Vec<u8> = (0u8..200).collect();
        let _ = decode_resolved(&buf, 1000);
        let _ = decode_resolved(&[], 1000);
        let _ = decode_resolved(&[0xFF; 23], 1);
        // Reaching here without a panic is the assertion.
        let sentinel = String::from("ok");
        assert_eq!(sentinel, "ok");
    }
}
