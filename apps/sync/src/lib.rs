//! AthenaOS Sync — a clickable, switcher-friendly front end for the LIVE
//! zero-knowledge `athsync` cross-device sync engine.
//!
//! The Concept names "AthSync: end-to-end-encrypted cross-device sync" as a
//! proprietary-stack pillar, sitting under the user-ownership promise ("The user
//! owns the machine" — sync is OPT-IN and the relay is ZERO-KNOWLEDGE). This app
//! puts the iCloud/OneDrive-style experience — see this device's identity, pair
//! another device, watch records sync — one click away, except the relay can
//! NEVER read a single value.
//!
//! ## Shape (mirrors apps/vpn, apps/mail, apps/browser, apps/calendar)
//! - The syscall-free heart is [`SyncApp`]: it owns the LIVE
//!   [`athsync::SyncState`] (the LWW-register CRDT + enrolled-device roster + the
//!   group key + this device's keys) and drives pair / encrypt / merge over an
//!   INJECTABLE [`SyncTransport`]. No `athsync` internals are reached for — only
//!   its public API (`DeviceKeys`, `GroupKey`, `wrap_group_key_for`,
//!   `unwrap_group_key`, `SyncState::{new, enroll_device, local_set,
//!   apply_remote, get, ...}`, `SyncBlob`).
//! - The host KAT (`cargo test -p sync --features host`) links the live engine
//!   and proves the real properties end-to-end against a MOCK relay + a MOCK peer
//!   device: (a) enroll + pair → both devices derive the SAME group key; (b)
//!   encrypt on A → B decrypts the relayed blob to the same plaintext while the
//!   relay sees only ciphertext; (c) CRDT convergence of concurrent writes on
//!   both devices; (d) a tampered/forged blob is rejected (fail-closed).
//! - The live transport (kernel net syscalls to a relay) is a `cfg(not(test))`
//!   wrapper. The relay datapath is NOT wired this session; the live app reports
//!   an HONEST "live sync transport not wired" status rather than faking a sync.
//!   The crypto + CRDT are the same real code in both paths.

// no_std for the real userspace ELF; std under `cargo test` so the host KAT can
// link. The live ELF entry point lives in the thin `src/main.rs` bin, which
// calls `run()` below. (`run` uses `Canvas::new`, which is `unsafe`, so the
// LIBRARY cannot `#![forbid(unsafe_code)]` — the unsafe sites are the
// surface-buffer Canvas, documented.)
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use athsync::{
    unwrap_group_key, wrap_group_key_for, DeviceId, DeviceIdentity, DeviceKeys, E2eError, GroupKey,
    SyncBlob, SyncState, WrappedGroupKey,
};

// The render/run path is live-ELF only; under `cargo test` only the SyncApp
// (over athsync) is exercised, so the graphics/syscall imports are gated out to
// keep the host test warning-clean.
#[cfg(not(test))]
use ath_tokens::DARK;
#[cfg(not(test))]
use athgfx::Canvas;
#[cfg(not(test))]
#[allow(unused_imports)]
use athkit;

// ===========================================================================
// Injectable transport — the seam the host KAT mocks with a zero-knowledge relay.
// ===========================================================================

/// The relay the app pushes/pulls `SyncBlob`s through. This is the ONLY thing the
/// server ever touches: opaque AEAD ciphertext + an ed25519 signature, indexed by
/// the plaintext (record key, lamport, device id). Swapping the transport swaps
/// the whole network without touching any crypto or CRDT logic.
///
/// The host KAT supplies a MOCK relay (a dumb blob store that genuinely cannot
/// read values); the live ELF would supply a kernel-socket wrapper. A relay that
/// tampers with a blob is caught by `SyncState::apply_remote` (signature +
/// AEAD), so an honest-but-curious or actively-hostile relay is equally powerless.
pub trait SyncTransport {
    /// Upload one signed+encrypted blob to the relay. Returns `false` if the
    /// transport failed to accept the bytes.
    fn push(&mut self, blob: &SyncBlob) -> bool;
    /// Pull every blob the relay holds that this device has not yet seen. An empty
    /// vec means "nothing new" — it is NOT an error.
    fn pull(&mut self) -> Vec<SyncBlob>;
}

// ===========================================================================
// PairingPackage — the bytes exchanged out-of-band to enroll a new device.
// ===========================================================================

/// What a NEW device advertises (over QR / numeric-pin / proximity) when asking
/// to join the sync group: its public identity. The account holder wraps the
/// group key for this identity and hands back a [`WrappedGroupKey`].
///
/// This is the public, shareable half — it carries NO secret. The whole pairing
/// security rests on the human verifying it (or the PIN) out of band.
#[derive(Clone, Debug)]
pub struct PairingRequest {
    pub identity: DeviceIdentity,
    /// Friendly name the new device chose (shown in the device list).
    pub name: String,
}

// ===========================================================================
// PairedDevice — a row in this device's roster (UI view of the enrolled set).
// ===========================================================================

/// A device this account has paired with, as shown in the device list. The
/// authoritative trust state lives in [`athsync::SyncState`]'s enrolled roster;
/// this is the display/metadata mirror the UI renders.
#[derive(Clone, Debug)]
pub struct PairedDevice {
    pub identity: DeviceIdentity,
    pub name: String,
    /// This device (vs. a remote one) — rendered with a "This device" badge.
    pub is_self: bool,
}

// ===========================================================================
// SyncApp — the syscall-free heart (host-KAT'd against the live engine).
// ===========================================================================

/// Outcome of a "Sync now" round-trip, surfaced honestly in the status detail.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncOutcome {
    /// Never synced this session.
    Idle,
    /// A push/pull cycle completed; carries how many remote blobs merged.
    Merged(usize),
    /// The transport could not accept an upload (relay unreachable).
    PushFailed,
    /// One or more pulled blobs were rejected (forged/tampered/unenrolled). The
    /// count of REJECTED blobs — the sync did not silently swallow bad data.
    Rejected(usize),
}

/// Why a "Sync now" produced no merge — kept distinct from a clean empty pull so
/// the UI can tell "nothing to do" from "relay not wired".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncDetail {
    None,
    /// The live relay datapath is not wired this session (honest stub).
    TransportNotWired,
}

/// The whole app model: this device's LIVE sync state (CRDT + roster + group key
/// + keys), the display roster, and the last-sync bookkeeping. There is NO path
/// that fabricates a converged value — every record comes from the real engine.
pub struct SyncApp {
    /// The LIVE zero-knowledge engine. All record reads/writes/merges go through
    /// it; we never shadow its converged state with a guess.
    state: SyncState,
    /// This device's own keys (kept so we can answer pairing requests as the
    /// account holder and show our identity).
    device: DeviceKeys,
    /// The shared group key — needed to wrap it for new devices.
    group_key: GroupKey,
    /// Display roster: this device plus every paired one.
    devices: Vec<PairedDevice>,
    /// Last "Sync now" outcome and detail (UI status).
    last_outcome: SyncOutcome,
    last_detail: SyncDetail,
    /// Monotonic time (ns) of the last sync round-trip, 0 if never.
    last_sync_ns: u64,
    /// Blobs produced locally but not yet confirmed pushed (the "pending" badge).
    pending: usize,
    /// The set of record keys the app has observed (local writes + accepted
    /// remotes), sorted. The engine stores converged values in a BTreeMap but does
    /// not expose its key iterator, so we mirror the key set here for the UI list.
    /// This is display metadata only — every VALUE still comes from the engine.
    known_keys: Vec<String>,
}

impl SyncApp {
    /// Create the app for THIS device with a freshly-minted (or restored) group
    /// key. The device enrolls itself automatically (so its own blobs verify on
    /// round-trip — see `SyncState::new`).
    pub fn new(device: DeviceKeys, name: &str, group_key: GroupKey) -> Self {
        let identity = device.identity();
        let state = SyncState::new(device.clone(), group_key.clone());
        let devices = alloc::vec![PairedDevice {
            identity,
            name: String::from(name),
            is_self: true,
        }];
        Self {
            state,
            device,
            group_key,
            devices,
            last_outcome: SyncOutcome::Idle,
            last_detail: SyncDetail::None,
            last_sync_ns: 0,
            pending: 0,
            known_keys: Vec::new(),
        }
    }

    /// This device's public identity (id + ed/x public keys) — what we advertise.
    pub fn this_device(&self) -> DeviceIdentity {
        self.device.identity()
    }

    pub fn this_device_id(&self) -> DeviceId {
        self.device.device_id
    }

    /// The display roster (this device first, then paired devices).
    pub fn devices(&self) -> &[PairedDevice] {
        &self.devices
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn last_outcome(&self) -> SyncOutcome {
        self.last_outcome
    }

    pub fn last_detail(&self) -> SyncDetail {
        self.last_detail
    }

    pub fn last_sync_ns(&self) -> u64 {
        self.last_sync_ns
    }

    pub fn pending(&self) -> usize {
        self.pending
    }

    pub fn record_count(&self) -> usize {
        self.state.record_count()
    }

    /// Read the current CONVERGED value for a record key, as decided by the live
    /// LWW-register CRDT. `None` if the key is unknown.
    pub fn get_record(&self, key: &str) -> Option<&[u8]> {
        self.state.get(key)
    }

    /// All record keys the app has observed, sorted. Mirrors the engine's
    /// converged key set (the engine does not expose a key iterator); the value
    /// for each is read back from the engine via [`SyncApp::get_record`].
    pub fn record_keys(&self) -> Vec<String> {
        self.known_keys.clone()
    }

    // -- Account holder side: enroll a new device into the group --------------

    /// As the account holder, accept a pairing request from a new device: enroll
    /// its public identity (so its future blobs are accepted) AND wrap the group
    /// key for it so it can decrypt everything. Returns the [`WrappedGroupKey`]
    /// to hand back out-of-band (the relay may carry it — it learns nothing).
    ///
    /// `wrap_nonce` is fresh AEAD nonce material; the kernel path supplies CSPRNG
    /// bytes, the host KAT injects a fixed value for determinism.
    pub fn pair_device(&mut self, req: &PairingRequest, wrap_nonce: [u8; 12]) -> WrappedGroupKey {
        // 1. Enroll the new device so its authored blobs verify in apply_remote.
        self.state.enroll_device(req.identity);
        // 2. Mirror it into the display roster (skip if already present).
        if !self
            .devices
            .iter()
            .any(|d| d.identity.device_id == req.identity.device_id)
        {
            self.devices.push(PairedDevice {
                identity: req.identity,
                name: req.name.clone(),
                is_self: false,
            });
        }
        // 3. Wrap the group key for the new device using ECDH(holder, device).
        wrap_group_key_for(
            &self.group_key,
            &self.device.x_secret,
            &self.device.x_public,
            req.identity.device_id,
            &req.identity.x_public,
            wrap_nonce,
        )
    }

    // -- New device side: join the group from a wrapped key -------------------

    /// As a NEW device, finish joining: unwrap the group key handed back by the
    /// account holder and enroll the holder so the holder's blobs are accepted.
    /// Fails closed (does NOT join) if the wrap is for a different device or
    /// tampered. On success, rebuilds local state under the now-known group key.
    pub fn join_group(
        &mut self,
        wrapped: &WrappedGroupKey,
        holder: DeviceIdentity,
        holder_name: &str,
    ) -> Result<(), E2eError> {
        let gk = unwrap_group_key(wrapped, &self.device)?;
        // Rebuild state under the real group key (we were created with a
        // placeholder before joining). Enroll the holder so its blobs verify.
        let mut state = SyncState::new(self.device.clone(), gk.clone());
        state.enroll_device(holder);
        self.state = state;
        self.group_key = gk;
        // Roster: keep ourselves, add the holder.
        self.devices.retain(|d| d.is_self);
        self.devices.push(PairedDevice {
            identity: holder,
            name: String::from(holder_name),
            is_self: false,
        });
        Ok(())
    }

    // -- Records: local writes + the sync round-trip --------------------------

    /// Set a record locally: the live engine bumps the Lamport clock, updates the
    /// LWW register, and produces a signed+encrypted [`SyncBlob`] to upload. The
    /// blob is tracked as pending until a `sync_now` confirms the push.
    pub fn set_record(&mut self, key: &str, value: &[u8], nonce: [u8; 12]) -> SyncBlob {
        let blob = self.state.local_set(key, value.to_vec(), nonce);
        self.remember_key(key);
        self.pending += 1;
        blob
    }

    /// Push all pending blobs and pull+merge everything new from the relay. This
    /// is the "Sync now" action. The caller supplies the pending blobs (the app
    /// re-derives nothing it cannot prove). Returns the outcome.
    ///
    /// Every pulled blob goes through `SyncState::apply_remote`, which verifies
    /// the ed25519 signature against the enrolled author and AEAD-decrypts under
    /// the group key BEFORE merging — so a forged/tampered/unenrolled blob is
    /// rejected and counted, never silently merged.
    pub fn sync_now<T: SyncTransport>(
        &mut self,
        transport: &mut T,
        outbox: &[SyncBlob],
        now_ns: u64,
    ) -> SyncOutcome {
        // Push pending uploads.
        for blob in outbox {
            if !transport.push(blob) {
                self.last_outcome = SyncOutcome::PushFailed;
                self.last_detail = SyncDetail::None;
                return self.last_outcome;
            }
        }
        self.pending = 0;

        // Pull + merge.
        let incoming = transport.pull();
        let mut merged = 0usize;
        let mut rejected = 0usize;
        for blob in &incoming {
            match self.state.apply_remote(blob) {
                Ok(()) => {
                    self.remember_key(&blob.record_key);
                    merged += 1;
                }
                Err(_) => rejected += 1,
            }
        }

        self.last_sync_ns = now_ns;
        self.last_detail = SyncDetail::None;
        self.last_outcome = if rejected > 0 {
            SyncOutcome::Rejected(rejected)
        } else {
            SyncOutcome::Merged(merged)
        };
        self.last_outcome
    }

    /// Apply a single remote blob directly (used by the host KAT to exchange
    /// blobs without a relay, and by the live path once decoded). Thin pass-through
    /// to the engine so the fail-closed guarantee is the engine's, not the app's.
    pub fn apply_remote(&mut self, blob: &SyncBlob) -> Result<(), E2eError> {
        let r = self.state.apply_remote(blob);
        if r.is_ok() {
            self.remember_key(&blob.record_key);
        }
        r
    }

    /// Record that the app has touched `key` (so `record_keys` can list it for the
    /// UI; the engine itself does not expose its key set).
    fn remember_key(&mut self, key: &str) {
        if !self.known_keys.iter().any(|k| k == key) {
            self.known_keys.push(String::from(key));
            self.known_keys.sort();
        }
    }
}

// ===========================================================================
// Live ELF: window geometry, draw path, event loop. (cfg(not(test)) only — the
// host KAT exercises only the SyncApp over athsync, so none of this links into
// the test.)
// ===========================================================================

#[cfg(not(test))]
const WIN_W: usize = 640;
#[cfg(not(test))]
const WIN_H: usize = 480;
#[cfg(not(test))]
const SURFACE_VIRT: u64 = 0x0000_7E00_0000;
#[cfg(not(test))]
const PRESENT_X: i32 = 150;
#[cfg(not(test))]
const PRESENT_Y: i32 = 70;
#[cfg(not(test))]
const TITLE_H: usize = 32;

/// Format the last-sync time as a coarse status word.
#[cfg(not(test))]
fn outcome_label(o: SyncOutcome) -> &'static str {
    match o {
        SyncOutcome::Idle => "Never synced",
        SyncOutcome::Merged(_) => "Synced",
        SyncOutcome::PushFailed => "Relay unreachable",
        SyncOutcome::Rejected(_) => "Rejected bad data",
    }
}

#[cfg(not(test))]
fn outcome_color(o: SyncOutcome) -> u32 {
    match o {
        SyncOutcome::Merged(_) => DARK.state_ok,
        SyncOutcome::Idle => DARK.text_tertiary,
        SyncOutcome::PushFailed => DARK.state_warn,
        SyncOutcome::Rejected(_) => DARK.state_danger,
    }
}

/// Render a 16-byte device id as a short hex fingerprint (first 4 bytes).
#[cfg(not(test))]
fn short_id(id: DeviceId) -> String {
    let mut s = String::new();
    for b in &id.0[..4] {
        push_hex_byte(&mut s, *b);
    }
    s
}

#[cfg(not(test))]
fn push_hex_byte(out: &mut String, b: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(b >> 4) as usize] as char);
    out.push(HEX[(b & 0x0F) as usize] as char);
}

/// Render the whole window: a glass card with this device's identity, the paired
/// device list, the synced-record list (converged CRDT state), and the actions.
#[cfg(not(test))]
fn render(app: &SyncApp, canvas: &mut Canvas) {
    use ath_tokens::{RADIUS_LG, RADIUS_MD, SPACE_3, SPACE_4};

    // Liquid-Glass background: deep base, a raised glass card on top.
    canvas.clear(DARK.bg_base);
    canvas.fill_rounded_rect(
        SPACE_3 as usize,
        SPACE_3 as usize,
        WIN_W - 2 * SPACE_3 as usize,
        WIN_H - 2 * SPACE_3 as usize,
        RADIUS_LG as usize,
        DARK.bg_raised,
    );
    canvas.draw_rounded_rect_outline(
        SPACE_3 as usize,
        SPACE_3 as usize,
        WIN_W - 2 * SPACE_3 as usize,
        WIN_H - 2 * SPACE_3 as usize,
        RADIUS_LG as usize,
        DARK.stroke_strong,
    );

    // Title.
    canvas.draw_text_scaled(
        (SPACE_4 + 8) as usize,
        (SPACE_4 + 4) as usize,
        "AthSync  -  end-to-end encrypted",
        DARK.text_primary,
        2,
    );

    let x = SPACE_4 as usize + 8;
    let mut y = TITLE_H + SPACE_4 as usize + 8;

    // This device identity.
    canvas.draw_text(x, y, "This device:", DARK.text_tertiary, None);
    y += 16;
    let mut id_line = String::from("id ");
    id_line.push_str(&short_id(app.this_device_id()));
    canvas.draw_text(x, y, &id_line, DARK.text_secondary, None);
    y += 24;

    // Paired devices.
    canvas.draw_text(x, y, "Devices:", DARK.text_tertiary, None);
    y += 18;
    for d in app.devices() {
        canvas.fill_circle(x + 6, y + 8, 5, DARK.state_ok);
        let label = if d.is_self {
            let mut s = d.name.clone();
            s.push_str("  (this device)");
            s
        } else {
            d.name.clone()
        };
        canvas.draw_text(x + 20, y, &label, DARK.text_primary, None);
        y += 24;
    }
    let _ = y; // device list is the last item in the left column

    // Synced records — the visible converged CRDT state.
    let dx = x + 300;
    let mut dy = TITLE_H + SPACE_4 as usize + 8;
    canvas.draw_text(dx, dy, "Synced records:", DARK.text_tertiary, None);
    dy += 18;
    if app.record_count() == 0 {
        canvas.draw_text(dx, dy, "(nothing synced yet)", DARK.text_tertiary, None);
        dy += 20;
    } else {
        for key in app.record_keys() {
            let mut line = key.clone();
            line.push_str(" = ");
            if let Some(val) = app.get_record(&key) {
                // Values are byte payloads; show printable bytes only.
                for &b in val.iter().take(24) {
                    if (0x20..0x7F).contains(&b) {
                        line.push(b as char);
                    } else {
                        line.push('.');
                    }
                }
            }
            canvas.draw_text(dx, dy, &line, DARK.text_secondary, None);
            dy += 18;
        }
    }
    dy += 12;

    // Sync status.
    canvas.draw_text(dx, dy, "Status:", DARK.text_tertiary, None);
    dy += 16;
    canvas.draw_text(
        dx,
        dy,
        outcome_label(app.last_outcome()),
        outcome_color(app.last_outcome()),
        None,
    );
    dy += 24;
    if app.last_detail() == SyncDetail::TransportNotWired {
        canvas.draw_text(
            dx,
            dy,
            "live relay not wired (crypto only)",
            DARK.state_warn,
            None,
        );
        dy += 18;
    }
    let mut pend = String::from("pending changes: ");
    push_u64(&mut pend, app.pending() as u64);
    canvas.draw_text(dx, dy, &pend, DARK.text_tertiary, None);

    // Action buttons (bottom): "Sync now" and "Pair device".
    let by = WIN_H - 56;
    canvas.fill_rounded_rect(x, by, 150, 36, RADIUS_MD as usize, DARK.state_ok);
    canvas.draw_text_scaled(x + 16, by + 10, "Sync now", 0xFF_FF_FF_FF, 2);
    canvas.fill_rounded_rect(x + 170, by, 150, 36, RADIUS_MD as usize, DARK.bg_elevated);
    canvas.draw_text_scaled(x + 186, by + 10, "Pair device", DARK.text_primary, 2);
}

/// Append a `u64` as decimal (no_std-safe).
#[cfg(not(test))]
fn push_u64(out: &mut String, mut v: u64) {
    if v == 0 {
        out.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] {
        out.push(b as char);
    }
}

/// The clickable regions the event loop hit-tests.
#[cfg(not(test))]
#[derive(PartialEq, Eq)]
enum Hit {
    None,
    SyncNow,
    PairDevice,
}

#[cfg(not(test))]
fn hit_test(lx: i32, ly: i32) -> Hit {
    use ath_tokens::SPACE_4;
    let x = (SPACE_4 as usize + 8) as i32;
    let by = (WIN_H - 56) as i32;
    if ly >= by && ly < by + 36 {
        if lx >= x && lx < x + 150 {
            return Hit::SyncNow;
        }
        if lx >= x + 170 && lx < x + 320 {
            return Hit::PairDevice;
        }
    }
    Hit::None
}

/// The honest stand-in for "live sync relay not wired": a transport that accepts
/// pushes but never returns blobs. A "Sync now" over it confirms the local push
/// path and reports an honest TransportNotWired detail — it NEVER fabricates a
/// merged remote record.
#[cfg(not(test))]
struct NullTransport;

#[cfg(not(test))]
impl SyncTransport for NullTransport {
    fn push(&mut self, _blob: &SyncBlob) -> bool {
        true
    }
    fn pull(&mut self) -> Vec<SyncBlob> {
        Vec::new()
    }
}

/// Creates the window surface and runs the event loop. The live relay datapath is
/// not wired this session, so "Sync now" reports an honest "live relay not wired"
/// status (and only confirms the local push path); it does NOT fake a remote
/// merge. The same real `SyncApp` crypto + CRDT run once the kernel-socket
/// `SyncTransport` lands.
#[cfg(not(test))]
pub fn run() -> ! {
    let sid = athkit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        athkit::sys::exit(1);
    }
    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    // Build THIS device. The kernel path would derive these seeds from a
    // per-device secret in the keychain; here we derive deterministic-but-app-
    // local seeds from the boot time so the first-run window is populated and the
    // identity is stable for the session. Real secret provisioning is a keychain
    // integration follow-up (labeled, not faked: the crypto is real either way).
    let t = athkit::sys::time_ns();
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&t.to_le_bytes());
    id[8] = 0xD1; // device tag
    let ed_seed = derive_seed(t, 0xA1);
    let x_secret = derive_seed(t, 0x5C);
    let device = DeviceKeys::from_seeds(DeviceId(id), ed_seed, x_secret);

    // A fresh group key for this account (the holder mints it; new devices get it
    // wrapped). Derived locally for the session.
    let group_key = GroupKey(derive_seed(t, 0x6B));

    let mut app = SyncApp::new(device, "This Mac", group_key);

    // Seed a couple of local records so the converged-record list isn't empty on
    // first run (real local settings would feed these). These are genuine local
    // writes through the live engine — the values shown are the engine's.
    let _ = app.set_record("settings/theme", b"glass-dark", nonce_from(t, 1));
    let _ = app.set_record("settings/wallpaper", b"aurora", nonce_from(t, 2));

    render(&app, &mut canvas);
    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    let mut left_was_down = false;
    loop {
        let mut dirty = false;

        let mut edge = false;
        loop {
            let ev = athkit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            let now_down = (ev & 0x01) != 0;
            if now_down && !left_was_down {
                edge = true;
            }
            left_was_down = now_down;
        }
        if edge {
            let (cx, cy, _btn) = athkit::sys::cursor_pos();
            let (ox, oy) =
                athkit::sys::surface_origin(sid).unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
            let lx = (cx as i32).saturating_sub(ox as i32);
            let ly = (cy as i32).saturating_sub(oy as i32);
            match hit_test(lx, ly) {
                Hit::SyncNow => {
                    // No live relay this session: drive the REAL sync path over a
                    // null transport. The local push is confirmed; the pull is
                    // honestly empty. We mark the detail so the UI says so — we do
                    // NOT synthesize a merged remote record.
                    let mut t = NullTransport;
                    app.sync_now(&mut t, &[], athkit::sys::time_ns());
                    app.last_detail = SyncDetail::TransportNotWired;
                    dirty = true;
                }
                Hit::PairDevice => {
                    // Pairing needs a second device's identity (scanned out of
                    // band). Without a live pairing channel this session, this is
                    // a no-op placeholder in the live ELF; the real pair_device
                    // path is host-KAT'd. (No fake device is added.)
                    dirty = false;
                }
                Hit::None => {}
            }
        }

        let key = athkit::sys::read_key();
        if key != 0 {
            let code = (key & 0xFF) as u8;
            let pressed = (key & 0x8000_0000) == 0;
            if pressed && code == 0x01 {
                athkit::sys::exit(0);
            }
        }

        if dirty {
            render(&app, &mut canvas);
            athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }
        athkit::sys::yield_now();
    }
}

/// Derive a 32-byte seed from a time value + a domain tag (live-ELF only). This is
/// session-local key material, NOT a real keychain secret — but the crypto it
/// feeds (ed25519/x25519/AEAD) is the genuine engine.
#[cfg(not(test))]
fn derive_seed(t: u64, tag: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    let tb = t.to_le_bytes();
    for i in 0..32 {
        s[i] = tb[i % 8] ^ tag ^ (i as u8).wrapping_mul(31);
    }
    s
}

/// Derive a 12-byte AEAD nonce from a time value + a counter (live-ELF only).
#[cfg(not(test))]
fn nonce_from(t: u64, ctr: u8) -> [u8; 12] {
    let mut n = [0u8; 12];
    let tb = t.to_le_bytes();
    n[..8].copy_from_slice(&tb);
    n[8] = ctr;
    n
}

// ===========================================================================
// Host KAT — links the LIVE athsync zero-knowledge engine, no kernel, no network.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(tag: u8) -> DeviceKeys {
        let mut id = [0u8; 16];
        id[0] = tag;
        DeviceKeys::from_seeds(DeviceId(id), [tag ^ 0xA1; 32], [tag ^ 0x5C; 32])
    }

    fn nonce(n: u8) -> [u8; 12] {
        [n; 12]
    }

    /// A zero-knowledge MOCK relay: a dumb blob store. It can hold and hand back
    /// blobs but CANNOT read any value (it never touches the group key). Models
    /// the server the Concept promises learns nothing.
    struct MockRelay {
        store: Vec<SyncBlob>,
    }

    impl MockRelay {
        fn new() -> Self {
            Self { store: Vec::new() }
        }
        /// Everything currently stored — what a device pulls. (A real relay would
        /// track per-device cursors; for the KAT we hand the whole store.)
        fn snapshot(&self) -> Vec<SyncBlob> {
            self.store.clone()
        }
    }

    impl SyncTransport for MockRelay {
        fn push(&mut self, blob: &SyncBlob) -> bool {
            self.store.push(blob.clone());
            true
        }
        fn pull(&mut self) -> Vec<SyncBlob> {
            self.store.clone()
        }
    }

    /// (a) Enroll + pair a second device -> both derive the SAME group key.
    #[test]
    fn pairing_yields_identical_group_key_on_both_devices() {
        let holder_keys = keys(1);
        let gk = GroupKey([0x42; 32]);
        let mut holder = SyncApp::new(holder_keys, "Holder", gk.clone());

        // New device asks to join with its public identity.
        let new_keys = keys(2);
        let req = PairingRequest {
            identity: new_keys.identity(),
            name: String::from("Phone"),
        };
        let wrapped = holder.pair_device(&req, nonce(20));

        // New device joins from the wrapped key.
        let placeholder = GroupKey([0u8; 32]);
        let mut joiner = SyncApp::new(new_keys, "Phone", placeholder);
        joiner
            .join_group(&wrapped, holder.this_device(), "Holder")
            .expect("join must succeed for the intended recipient");

        // Both devices now hold the SAME group key. We prove it by a cross-device
        // record round-trip (only possible with equal group keys): the holder
        // encrypts, the joiner decrypts to the same plaintext.
        let holder2 = &mut holder;
        let blob = holder2.set_record("k", b"shared-secret-value", nonce(1));
        joiner
            .apply_remote(&blob)
            .expect("joiner decrypts holder's blob -> keys match");
        assert_eq!(joiner.get_record("k"), Some(&b"shared-secret-value"[..]));

        // And directly: pair_device returned a wrap the joiner unwrapped to gk.
        let direct = unwrap_group_key(&wrapped, &keys(2)).expect("unwrap");
        assert_eq!(direct.0, gk.0, "paired device derives the SAME group key");
    }

    /// (b) Encrypt a record on A -> B decrypts the relayed blob to the same
    /// plaintext, and the relay only ever sees ciphertext (NOT the plaintext).
    #[test]
    fn record_syncs_a_to_b_relay_sees_only_ciphertext() {
        let a_keys = keys(1);
        let b_keys = keys(2);
        let gk = GroupKey([0x42; 32]);

        let mut a = SyncApp::new(a_keys.clone(), "A", gk.clone());
        let mut b = SyncApp::new(b_keys.clone(), "B", gk.clone());
        // Mutual enrollment (pairing already happened out of band).
        a.state.enroll_device(b_keys.identity());
        b.state.enroll_device(a_keys.identity());

        let mut relay = MockRelay::new();

        // A writes a record and syncs (pushes the blob to the relay).
        let plaintext = b"wifi-password-hunter2";
        let blob = a.set_record("creds/wifi", plaintext, nonce(7));
        let out = a.sync_now(&mut relay, core::slice::from_ref(&blob), 1_000);
        assert_eq!(out, SyncOutcome::Merged(1)); // pulls back its own (idempotent)

        // The relay's stored blob value is NOT the plaintext — it's AEAD ct+tag.
        let stored = relay.snapshot();
        assert_eq!(stored.len(), 1);
        assert_ne!(
            stored[0].ciphertext.as_slice(),
            &plaintext[..],
            "the relay must NEVER hold the plaintext"
        );
        // The relay does see the (plaintext) record key for indexing — by design.
        assert_eq!(stored[0].record_key, "creds/wifi");

        // B syncs: pulls the blob and decrypts it to the SAME plaintext.
        let out_b = b.sync_now(&mut relay, &[], 2_000);
        assert_eq!(out_b, SyncOutcome::Merged(1));
        assert_eq!(b.get_record("creds/wifi"), Some(&plaintext[..]));
    }

    /// (c) CRDT convergence: two devices make CONCURRENT writes to the same key,
    /// then exchange blobs -> both converge to the identical winning value.
    #[test]
    fn concurrent_writes_converge_on_both_devices() {
        let a_keys = keys(1);
        let b_keys = keys(2); // larger device id -> wins ties
        let gk = GroupKey([0x42; 32]);

        let mut a = SyncApp::new(a_keys.clone(), "A", gk.clone());
        let mut b = SyncApp::new(b_keys.clone(), "B", gk.clone());
        a.state.enroll_device(b_keys.identity());
        b.state.enroll_device(a_keys.identity());

        // Independent, concurrent writes to the SAME key (both lamport=1 locally).
        let blob_a = a.set_record("settings/wallpaper", b"aurora", nonce(10));
        let blob_b = b.set_record("settings/wallpaper", b"nebula", nonce(11));

        // Exchange.
        a.apply_remote(&blob_b).unwrap();
        b.apply_remote(&blob_a).unwrap();

        // Convergence: identical value on BOTH devices (b's id wins the tie).
        assert_eq!(
            a.get_record("settings/wallpaper"),
            b.get_record("settings/wallpaper"),
            "both devices must converge to the SAME value"
        );
        assert_eq!(a.get_record("settings/wallpaper"), Some(&b"nebula"[..]));
    }

    /// (d) A tampered/forged blob is rejected (fail-closed) and counted as
    /// rejected, never merged into the converged state.
    #[test]
    fn tampered_blob_is_rejected_fail_closed() {
        let a_keys = keys(1);
        let b_keys = keys(2);
        let gk = GroupKey([0x42; 32]);

        let mut a = SyncApp::new(a_keys.clone(), "A", gk.clone());
        let mut b = SyncApp::new(b_keys.clone(), "B", gk.clone());
        a.state.enroll_device(b_keys.identity());
        b.state.enroll_device(a_keys.identity());

        // A authors a genuine blob, then a hostile relay flips a ciphertext byte.
        let mut tampered = a.set_record("settings/theme", b"glass-dark", nonce(3));
        tampered.ciphertext[0] ^= 0x01;

        struct HostileRelay {
            blob: SyncBlob,
        }
        impl SyncTransport for HostileRelay {
            fn push(&mut self, _b: &SyncBlob) -> bool {
                true
            }
            fn pull(&mut self) -> Vec<SyncBlob> {
                alloc::vec![self.blob.clone()]
            }
        }

        let mut relay = HostileRelay { blob: tampered };
        let out = b.sync_now(&mut relay, &[], 5_000);
        assert_eq!(
            out,
            SyncOutcome::Rejected(1),
            "a tampered blob must be rejected, not merged"
        );
        assert_eq!(
            b.get_record("settings/theme"),
            None,
            "rejected data must NEVER enter the converged state"
        );

        // A blob from an UNENROLLED device is also rejected.
        let stranger = keys(9);
        let forged = athsync::encrypt_record("k", b"v", 1, &stranger, &gk, nonce(4));
        assert_eq!(b.apply_remote(&forged), Err(E2eError::UnknownDevice));

        // A forged signature on an enrolled author is rejected too.
        let mut bad_sig = a.set_record("k2", b"v2", nonce(5));
        bad_sig.signature[0] ^= 0xFF;
        assert_eq!(b.apply_remote(&bad_sig), Err(E2eError::BadSignature));
    }

    /// A wrong-device join attempt fails closed (the wrap is not for it).
    #[test]
    fn wrong_device_cannot_join_group() {
        let holder = keys(1);
        let gk = GroupKey([0x42; 32]);
        let mut app = SyncApp::new(holder.clone(), "Holder", gk);

        let intended = keys(2);
        let req = PairingRequest {
            identity: intended.identity(),
            name: String::from("Intended"),
        };
        let wrapped = app.pair_device(&req, nonce(20));

        // A different device tries to join with the intended device's wrap.
        let attacker = keys(3);
        let mut attacker_app = SyncApp::new(attacker, "Attacker", GroupKey([0u8; 32]));
        let r = attacker_app.join_group(&wrapped, app.this_device(), "Holder");
        assert!(r.is_err(), "a non-recipient device must NOT join the group");
    }

    /// Record-key tracking surfaces every touched key for the UI list.
    #[test]
    fn record_keys_lists_converged_keys() {
        let mut app = SyncApp::new(keys(1), "A", GroupKey([0x42; 32]));
        app.set_record("settings/theme", b"dark", nonce(1));
        app.set_record("settings/wallpaper", b"aurora", nonce(2));
        let keys_list = app.record_keys();
        assert_eq!(keys_list.len(), 2);
        assert!(keys_list.iter().any(|k| k == "settings/theme"));
        assert!(keys_list.iter().any(|k| k == "settings/wallpaper"));
        assert_eq!(app.record_count(), 2);
    }
}
