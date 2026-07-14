//! RaeenOS Passwords & Authenticator — *"import & keep my stuff"*
//! (RaeenOS_Concept.md §Compatibility Strategy criterion #5).
//!
//! A first-party, fully-local password vault + software authenticator — the
//! macOS Passwords / Windows Credential Manager half of the OS auth story, with
//! the Google-Authenticator TOTP half folded in. A switcher arrives with a
//! browser full of saved logins and an authenticator app full of TOTP secrets;
//! this is where both live, encrypted at rest behind one master passphrase.
//!
//! Standalone userspace ELF launched from the start menu (`exec_path =
//! "passwords"`). The two engines are already host-KAT'd and do ALL the work:
//!   * `rae_keychain` — Argon2id master-key KDF + ChaCha20-Poly1305 AEAD vault,
//!     fail-closed on a wrong passphrase (the AEAD tag won't verify → Err, no
//!     oracle).
//!   * `rae_otp` — HOTP/TOTP (RFC 4226 / 6238), base32 + `otpauth://` import,
//!     skew-tolerant verify.
//!
//! This crate is the clickable shell over them: an unlock screen, a credential
//! list that NEVER renders a secret by default, an authenticator view that
//! computes the live 6-digit code + a seconds-remaining ring, and real
//! add-credential / import-otpauth flows.
//!
//! The vault DECISION logic lives in the syscall-free [`VaultModel`] so the host
//! KAT (`cargo test -p passwords --features host`) links the LIVE engines with no
//! kernel: create a vault, store a credential carrying a known TOTP secret,
//! re-open with the right passphrase (succeeds), re-open with a wrong passphrase
//! (fails closed), and compute a TOTP at a FIXED injected timestamp that matches
//! an RFC 6238 vector. The on-disk blob round-trips through `to_bytes` / `open`.
//!
//! TIME SOURCE: the live app reads `SYS_WALL_CLOCK` (syscall 40 — unix-epoch ns,
//! UTC), the SAME source the tray clock + Clock app read. The TOTP code is a pure
//! function of (secret, unix_time), so the host test injects a fixed timestamp.

// no_std for the real userspace ELF; std under `cargo test` so the host KAT can
// link. The live ELF entry point lives in the thin `src/main.rs` bin, which calls
// `run()` below. (`run` uses `Canvas::new`, which is `unsafe`, so the LIBRARY
// cannot `#![forbid(unsafe_code)]` — only the bin can; the one unsafe site is the
// surface-buffer Canvas + the raw `SYS_WALL_CLOCK` syscall, both documented.)
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use rae_keychain::{Keychain, KeychainError};
use rae_otp::OtpAuth;

// The render/run path is live-ELF only; under `cargo test` only the VaultModel
// (over rae_keychain + rae_otp) is exercised, so the graphics/syscall imports are
// gated out to keep the host test warning-clean.
#[cfg(not(test))]
#[allow(unused_imports)]
use raekit;

#[cfg(not(test))]
use rae_tokens::DARK;
#[cfg(not(test))]
use raegfx::text::FontFamily;
#[cfg(not(test))]
use raegfx::Canvas;

// ── Window geometry (live ELF only) ──────────────────────────────────────

#[cfg(not(test))]
const WIN_W: usize = 520;
#[cfg(not(test))]
const WIN_H: usize = 560;
#[cfg(not(test))]
const SURFACE_VIRT: u64 = 0x0000_7B00_0000;

#[cfg(not(test))]
const TITLE_H: usize = 28;
#[cfg(not(test))]
const TABBAR_H: usize = 34;
#[cfg(not(test))]
const ROW_H: usize = 48;
#[cfg(not(test))]
const FOOTER_H: usize = 30;

/// On-screen present origin (`surface_present(sid, PRESENT_X, PRESENT_Y)`).
/// Absolute cursor coords are converted to surface-local space by subtracting the
/// LIVE window origin (`surface_origin`), falling back to this.
#[cfg(not(test))]
const PRESENT_X: i32 = 200;
#[cfg(not(test))]
const PRESENT_Y: i32 = 70;

/// The default TOTP step (RFC 6238 §4 — 30 s) for the seconds-remaining ring on
/// entries whose `otpauth://` URI did not override `period`. Used by the
/// VaultModel TOTP path, so it stays available to the host KAT.
const DEFAULT_STEP_SECS: u64 = 30;

/// Where the encrypted vault blob lives on disk: `<home>/.config/vault.raekeyc`.
/// (The keychain blob is one opaque AEAD payload; the `.raekeyc` magic is checked
/// by `Keychain::open`.)
#[cfg(not(test))]
const VAULT_FILE: &str = "vault.raekeyc";

// ── Theme (live ELF only — the host KAT exercises only the VaultModel) ────

#[cfg(not(test))]
const BG: u32 = DARK.bg_base;
#[cfg(not(test))]
const PANEL: u32 = DARK.bg_raised;
#[cfg(not(test))]
const ROW_BG: u32 = DARK.bg_overlay;
#[cfg(not(test))]
const ROW_SEL: u32 = DARK.bg_elevated;
#[cfg(not(test))]
const STROKE: u32 = DARK.stroke_subtle;
#[cfg(not(test))]
const TEXT_PRIMARY: u32 = DARK.text_primary;
#[cfg(not(test))]
const TEXT_SECONDARY: u32 = DARK.text_secondary;
#[cfg(not(test))]
const TEXT_TERTIARY: u32 = DARK.text_tertiary;
#[cfg(not(test))]
const DANGER: u32 = DARK.state_danger;
#[cfg(not(test))]
const OK_COLOR: u32 = DARK.state_ok;

#[cfg(not(test))]
fn accent() -> u32 {
    rae_tokens::derive_accent(raekit::sys::theme_accent(), &DARK).base
}

// ── Wall clock (SYS_WALL_CLOCK = 40, unix-epoch ns) ───────────────────────
//
// raekit has no wrapper for SYS_WALL_CLOCK, so we issue the raw syscall through
// the public `raekit::sys::syscall0` — the EXACT pattern the Clock app uses
// (no new ABI surface for this slice). 0 means "unavailable" → TOTP shows
// `------`. Only compiled into the live ELF; the host test injects a fixed time.
#[cfg(not(test))]
const SYS_WALL_CLOCK: u64 = 40;

#[cfg(not(test))]
fn wall_secs() -> u64 {
    let ns = unsafe { raekit::sys::syscall0(SYS_WALL_CLOCK) };
    ns / 1_000_000_000
}

// ===========================================================================
// VaultModel — the syscall-free heart (host-KAT'd against the live engines).
// ===========================================================================

/// One row a user can see: the safe-to-display `(service, account)` plus a flag
/// for whether the stored credential carries an `otpauth://` TOTP secret (in its
/// metadata) so the Authenticator tab can list it. NEVER carries the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct VaultRow {
    /// Service / realm (e.g. `"github.com"`).
    pub service: String,
    /// Account / identity within the service.
    pub account: String,
    /// True iff this credential's secret is a parseable `otpauth://` TOTP URI.
    pub has_totp: bool,
}

/// What an unlock attempt produced — distinguishes "no vault yet" (first run, a
/// fresh vault is created) from "wrong passphrase" (fail-closed) without leaking
/// which one to a UI oracle beyond the user's own attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockOutcome {
    /// The blob decrypted: the vault is now open.
    Opened,
    /// No blob existed: a fresh empty vault was created and is open.
    Created,
    /// A blob existed but the passphrase (or a tampered header/tag) failed the
    /// AEAD — fail-closed, no plaintext released.
    WrongPassphrase,
    /// The blob is corrupt / unsupported (not a passphrase problem).
    Corrupt,
}

/// The in-memory vault: an open `Keychain` (or `None` when locked) over the
/// host-KAT'd engines. All policy (TOTP detection, code computation, add /
/// import) is here and syscall-free, so the host KAT drives it directly.
pub struct VaultModel {
    kc: Option<Keychain>,
}

impl Default for VaultModel {
    fn default() -> Self {
        Self::new()
    }
}

impl VaultModel {
    /// A locked, empty model.
    pub fn new() -> VaultModel {
        VaultModel { kc: None }
    }

    /// Whether the vault is currently unlocked.
    pub fn is_open(&self) -> bool {
        self.kc.is_some()
    }

    /// Lock the vault: drop the open keychain (its master key + secrets zeroize).
    pub fn lock(&mut self) {
        self.kc = None;
    }

    /// Open an EXISTING serialized blob with `passphrase`, or — if `existing` is
    /// `None` (no vault file yet) — create a fresh empty vault from `seed`
    /// entropy. Fail-closed: a wrong passphrase yields [`UnlockOutcome::
    /// WrongPassphrase`] and the model stays locked.
    ///
    /// `seed` MUST be real CSPRNG entropy for a fresh vault (the salt/nonce are
    /// derived from it); the live app mixes kernel time + pid, the test passes a
    /// fixed seed (fine for a deterministic KAT).
    pub fn unlock(
        &mut self,
        existing: Option<&[u8]>,
        passphrase: &str,
        seed: &[u8],
    ) -> UnlockOutcome {
        match existing {
            Some(blob) => match Keychain::open(blob, passphrase) {
                Ok(kc) => {
                    self.kc = Some(kc);
                    UnlockOutcome::Opened
                }
                Err(KeychainError::BadPassphrase) => UnlockOutcome::WrongPassphrase,
                Err(_) => UnlockOutcome::Corrupt,
            },
            None => {
                self.kc = Some(Keychain::create(passphrase, seed));
                UnlockOutcome::Created
            }
        }
    }

    /// Open with explicit cheap KDF cost (host KATs — keeps Argon2id fast). Only
    /// affects the FRESH-vault path; an existing blob carries its own params.
    pub fn unlock_with_params(
        &mut self,
        existing: Option<&[u8]>,
        passphrase: &str,
        seed: &[u8],
        kdf: rae_keychain::KdfParams,
    ) -> UnlockOutcome {
        match existing {
            Some(blob) => match Keychain::open(blob, passphrase) {
                Ok(kc) => {
                    self.kc = Some(kc);
                    UnlockOutcome::Opened
                }
                Err(KeychainError::BadPassphrase) => UnlockOutcome::WrongPassphrase,
                Err(_) => UnlockOutcome::Corrupt,
            },
            None => {
                let (salt, nonce) = derive_salt_nonce(seed);
                self.kc = Some(Keychain::with_entropy(passphrase, salt, nonce, kdf));
                UnlockOutcome::Created
            }
        }
    }

    /// The safe-to-display rows (`(service, account)` + a TOTP flag), sorted as
    /// the keychain sorts. NEVER includes a secret. Empty when locked.
    pub fn rows(&self) -> Vec<VaultRow> {
        let kc = match &self.kc {
            Some(k) => k,
            None => return Vec::new(),
        };
        kc.list()
            .into_iter()
            .map(|(service, account)| {
                let has_totp = kc
                    .entry(&service, &account)
                    .map(|c| credential_is_totp(c.secret()))
                    .unwrap_or(false);
                VaultRow {
                    service,
                    account,
                    has_totp,
                }
            })
            .collect()
    }

    /// The rows that carry a TOTP secret — the Authenticator tab's contents.
    pub fn totp_rows(&self) -> Vec<VaultRow> {
        self.rows().into_iter().filter(|r| r.has_totp).collect()
    }

    /// The cleartext secret for `(service, account)` — used ONLY for the explicit
    /// "reveal" action (a password row) or to derive a TOTP. Borrowed; the caller
    /// must not log or persist it.
    pub fn secret(&self, service: &str, account: &str) -> Option<&[u8]> {
        self.kc.as_ref().and_then(|kc| kc.get(service, account))
    }

    /// The current TOTP code + seconds-remaining for `(service, account)` at
    /// `unix_time`, if the stored secret is a parseable TOTP URI. The seconds
    /// remaining counts down within the entry's own period (default 30 s).
    pub fn totp_code_at(
        &self,
        service: &str,
        account: &str,
        unix_time: u64,
    ) -> Option<(String, u64)> {
        let secret = self.secret(service, account)?;
        let uri = core::str::from_utf8(secret).ok()?;
        let auth = OtpAuth::parse(uri)?;
        let period = if auth.period == 0 {
            DEFAULT_STEP_SECS
        } else {
            auth.period
        };
        let code = auth.code_at(unix_time);
        let remaining = period - (unix_time % period);
        Some((code, remaining))
    }

    /// Add (or overwrite) a plain password credential. Fails (unchanged) if the
    /// vault is locked or a field is over the engine's bounds.
    pub fn add_password(
        &mut self,
        service: &str,
        account: &str,
        password: &str,
    ) -> Result<(), KeychainError> {
        let kc = self.kc.as_mut().ok_or(KeychainError::Corrupt)?;
        kc.add(service, account, password.as_bytes())
    }

    /// Import an `otpauth://` TOTP/HOTP URI as a credential whose SECRET is the
    /// URI itself (so the authenticator view can re-parse parameters: digits,
    /// period, algorithm). The service/account default to the URI's issuer/label.
    /// Returns the `(service, account)` it was stored under, or `None` if the URI
    /// does not parse.
    pub fn import_otpauth(&mut self, uri: &str) -> Option<(String, String)> {
        let auth = OtpAuth::parse(uri)?;
        let kc = self.kc.as_mut()?;
        let service = auth
            .issuer
            .clone()
            .unwrap_or_else(|| issuer_from_label(&auth.label));
        let account = account_from_label(&auth.label, &service);
        kc.add(&service, &account, uri.as_bytes()).ok()?;
        Some((service, account))
    }

    /// Delete the credential for `(service, account)`. Returns whether it existed.
    pub fn delete(&mut self, service: &str, account: &str) -> bool {
        match self.kc.as_mut() {
            Some(kc) => kc.delete(service, account),
            None => false,
        }
    }

    /// Serialize + encrypt the open vault to a blob, rotating the AEAD nonce from
    /// `seed` so EVERY save writes under a FRESH (key, nonce) pair. `None` when
    /// locked.
    ///
    /// This is the ONE catastrophic ChaCha20-Poly1305 footgun: re-sealing the same
    /// master key under the same nonce across two saves leaks the keystream XOR of
    /// the two plaintexts and opens a Poly1305 forgery path. `Keychain::to_bytes`
    /// (the stored-nonce path) does exactly that on every save after the first, so
    /// the vault MUST persist through here instead — the keychain already exposes
    /// `to_bytes_with_nonce` for the rotation; this draws the per-write nonce.
    ///
    /// `seed` MUST carry real CSPRNG / entropy-mixed bytes (the live app feeds it
    /// kernel-time ^ wall-clock ^ pid ^ a per-save counter, the SAME mix used to
    /// seed a fresh vault); a constant seed would defeat the rotation. The nonce is
    /// spread from `seed` via the same FNV-1a expansion the create path uses for its
    /// salt/nonce split, so two saves with two distinct seeds cannot collide.
    pub fn to_bytes_with_fresh_nonce(&self, seed: &[u8]) -> Option<Vec<u8>> {
        self.kc.as_ref().map(|kc| {
            let (_salt, nonce) = derive_salt_nonce(seed);
            kc.to_bytes_with_nonce(nonce)
        })
    }

    /// Serialize + encrypt the open vault using the keychain's STORED nonce.
    ///
    /// DANGER: this reuses the same nonce across saves under the same master key —
    /// a catastrophic ChaCha20-Poly1305 mistake. It exists ONLY for the host KAT's
    /// fail-ability demonstration (proving the differ-nonce test catches reuse).
    /// The live save path MUST use
    /// [`to_bytes_with_fresh_nonce`](Self::to_bytes_with_fresh_nonce). `None` when
    /// locked.
    #[cfg(test)]
    pub fn to_bytes_stored_nonce(&self) -> Option<Vec<u8>> {
        self.kc.as_ref().map(|kc| kc.to_bytes())
    }

    /// Number of stored credentials (0 when locked).
    pub fn len(&self) -> usize {
        self.kc.as_ref().map(|kc| kc.len()).unwrap_or(0)
    }

    /// Whether the open vault holds no credentials (true when locked).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Whether a stored secret is an `otpauth://` TOTP/HOTP URI (vs a plain
/// password). Detection is parse-based, not just a prefix check, so a malformed
/// URI is treated as a password (and never rendered as a code).
fn credential_is_totp(secret: &[u8]) -> bool {
    match core::str::from_utf8(secret) {
        Ok(s) => s.starts_with("otpauth://") && OtpAuth::parse(s).is_some(),
        Err(_) => false,
    }
}

/// Derive the service from an `Issuer:account` label when no `issuer=` param is
/// present (`"ACME Co:alice@acme.com"` → `"ACME Co"`). Falls back to the whole
/// label.
fn issuer_from_label(label: &str) -> String {
    match label.split_once(':') {
        Some((issuer, _)) if !issuer.is_empty() => String::from(issuer.trim()),
        _ => String::from(label),
    }
}

/// Derive the account from an `Issuer:account` label, dropping the issuer prefix
/// if it matches `service`. Falls back to the whole label.
fn account_from_label(label: &str, service: &str) -> String {
    match label.split_once(':') {
        Some((issuer, account)) if issuer.trim() == service => String::from(account.trim()),
        _ => String::from(label.trim()),
    }
}

/// Split a `seed` into a 16-byte salt + 12-byte nonce by hashing (BLAKE2b via
/// the keychain's own derivation would need access to a private fn; here we use a
/// tiny FNV-1a expansion that is deterministic for the test and only ever fed
/// real CSPRNG bytes in the live app). This mirrors what `Keychain::create`
/// does internally; we replicate only for the `unlock_with_params` cheap-KDF
/// path. NOT a KDF — it just spreads caller entropy into two fixed-size fields.
fn derive_salt_nonce(seed: &[u8]) -> ([u8; 16], [u8; 12]) {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    // FNV-1a streamed with the field index folded in, so salt != nonce even for
    // a short/empty seed.
    for (i, slot) in salt.iter_mut().enumerate() {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ (i as u64).wrapping_mul(0x100_0000_01b3);
        for &b in seed {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        *slot = (h >> 24) as u8;
    }
    for (i, slot) in nonce.iter_mut().enumerate() {
        let mut h: u64 = 0x84222325_cbf2_9ce4 ^ ((i as u64 + 64).wrapping_mul(0x100_0000_01b3));
        for &b in seed {
            h ^= (b as u64).rotate_left(7);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        *slot = (h >> 32) as u8;
    }
    (salt, nonce)
}

// ===========================================================================
// App state + render (live ELF only — syscall-touching).
// ===========================================================================

/// Which top-level view is showing.
#[cfg(not(test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// Saved logins (service + account; secret hidden by default).
    Passwords,
    /// 2FA codes (entries carrying an otpauth:// TOTP secret).
    Authenticator,
}

/// A transient modal for entering text — add-credential (three fields) or
/// import-otpauth (one field).
#[cfg(not(test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Modal {
    None,
    /// Add a password: cycle Service → Account → Password with Tab/Enter.
    AddPassword,
    /// Paste an otpauth:// URI.
    ImportOtp,
}

/// Which add-password field is focused.
#[cfg(not(test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum AddField {
    Service,
    Account,
    Password,
}

/// A short status line shown in the footer (e.g. "Saved", "Wrong passphrase").
#[cfg(not(test))]
struct Toast {
    text: String,
}

#[cfg(not(test))]
impl Toast {
    fn new() -> Toast {
        Toast {
            text: String::new(),
        }
    }
    fn set(&mut self, s: &str) {
        self.text.clear();
        self.text.push_str(s);
    }
    fn clear(&mut self) {
        self.text.clear();
    }
}

/// The whole live app.
#[cfg(not(test))]
struct App {
    vault: VaultModel,
    /// The serialized blob loaded at launch (`None` = no vault file yet).
    on_disk: Option<Vec<u8>>,
    /// Locked → showing the unlock screen; else the tab views.
    unlocked: bool,
    tab: Tab,
    /// Passphrase being typed on the unlock screen (never persisted; cleared on
    /// unlock).
    passphrase: String,
    /// Selected row index in the active tab's list.
    selected: usize,
    /// The `(service, account)` whose password is currently revealed (explicit
    /// action only); `None` = all hidden. Cleared on any navigation.
    revealed: Option<(String, String)>,
    /// Active modal + its buffers.
    modal: Modal,
    add_field: AddField,
    add_service: String,
    add_account: String,
    add_password: String,
    import_uri: String,
    toast: Toast,
    shift: bool,
    /// Seed entropy for a fresh vault (kernel time ^ pid, mixed at launch).
    seed: [u8; 32],
    /// Monotonic per-save counter, folded into the FRESH save-nonce entropy so two
    /// saves within the same wall-clock second still rotate the nonce. A counter
    /// alone is NOT the entropy source — it is mixed with re-sampled kernel time ^
    /// wall clock ^ pid each save (see [`fresh_save_seed`](App::fresh_save_seed)).
    save_counter: u64,
}

#[cfg(not(test))]
impl App {
    fn new() -> App {
        let on_disk = load_vault_blob();
        let mut seed = [0u8; 32];
        // Best-effort entropy: kernel monotonic time ^ wall clock ^ pid, spread
        // across the seed. Only used when CREATING a fresh vault.
        let t = raekit::sys::time_ns();
        let w = wall_secs();
        let pid = raekit::sys::getpid();
        for (i, b) in seed.iter_mut().enumerate() {
            let mix = t
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(w.rotate_left(i as u32 & 63))
                .wrapping_add(pid.wrapping_mul(i as u64 + 1));
            *b = (mix >> ((i % 8) * 8)) as u8;
        }
        App {
            vault: VaultModel::new(),
            on_disk,
            unlocked: false,
            tab: Tab::Passwords,
            passphrase: String::new(),
            selected: 0,
            revealed: None,
            modal: Modal::None,
            add_field: AddField::Service,
            add_service: String::new(),
            add_account: String::new(),
            add_password: String::new(),
            import_uri: String::new(),
            toast: Toast::new(),
            shift: false,
            seed,
            save_counter: 0,
        }
    }

    /// Draw a FRESH 32-byte seed for a save's AEAD nonce. Re-samples real entropy
    /// every call — kernel monotonic time, wall clock, and pid — and folds in a
    /// monotonic per-save counter so two saves in the same wall-clock second still
    /// differ. The counter is a tiebreaker, NOT the sole source: the live entropy
    /// (`time_ns` advances every nanosecond) dominates, mirroring the create-path
    /// mix. Two consecutive saves therefore cannot produce the same nonce.
    #[cfg(not(test))]
    fn fresh_save_seed(&mut self) -> [u8; 32] {
        self.save_counter = self.save_counter.wrapping_add(1);
        let t = raekit::sys::time_ns();
        let w = wall_secs();
        let pid = raekit::sys::getpid();
        let ctr = self.save_counter;
        let mut seed = [0u8; 32];
        for (i, b) in seed.iter_mut().enumerate() {
            let mix = t
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(w.rotate_left(i as u32 & 63))
                .wrapping_add(pid.wrapping_mul(i as u64 + 1))
                .wrapping_add(ctr.rotate_left((i as u32).wrapping_mul(11) & 63))
                // Fold the launch-time seed in too, so each byte carries the
                // boot-entropy pool as well as the per-save sample.
                .wrapping_add(self.seed[i] as u64);
            *b = (mix >> ((i % 8) * 8)) as u8;
        }
        seed
    }

    /// Attempt to unlock with the typed passphrase. Fail-closed: a wrong
    /// passphrase shows an error and stays locked, no oracle beyond the attempt.
    fn try_unlock(&mut self) {
        if self.passphrase.is_empty() {
            self.toast.set("Enter your master passphrase");
            return;
        }
        let existing = self.on_disk.as_deref();
        match self.vault.unlock(existing, &self.passphrase, &self.seed) {
            UnlockOutcome::Opened => {
                self.unlocked = true;
                self.toast.set("Unlocked");
            }
            UnlockOutcome::Created => {
                self.unlocked = true;
                self.toast.set("New vault created");
                #[cfg(not(test))]
                self.save();
            }
            UnlockOutcome::WrongPassphrase => {
                self.toast.set("Wrong passphrase");
            }
            UnlockOutcome::Corrupt => {
                self.toast.set("Vault file is corrupt");
            }
        }
        // The passphrase has done its job — clear the buffer either way.
        self.passphrase.clear();
        self.selected = 0;
        self.revealed = None;
    }

    /// Lock: drop the open keychain and return to the unlock screen.
    fn lock(&mut self) {
        self.vault.lock();
        self.unlocked = false;
        self.revealed = None;
        self.selected = 0;
        self.toast.set("Locked");
    }

    /// The rows for the active tab.
    fn visible_rows(&self) -> Vec<VaultRow> {
        match self.tab {
            Tab::Passwords => self.vault.rows(),
            Tab::Authenticator => self.vault.totp_rows(),
        }
    }

    /// Persist the open vault to `<home>/.config/vault.raekeyc` (best effort).
    ///
    /// Every save rotates the AEAD nonce from FRESH entropy
    /// ([`fresh_save_seed`](App::fresh_save_seed) → [`VaultModel::
    /// to_bytes_with_fresh_nonce`]), so no two writes ever re-use a (key, nonce)
    /// pair — the catastrophic ChaCha20-Poly1305 footgun the stored-nonce path had.
    #[cfg(not(test))]
    fn save(&mut self) {
        let seed = self.fresh_save_seed();
        if let Some(blob) = self.vault.to_bytes_with_fresh_nonce(&seed) {
            if save_vault_blob(&blob) {
                self.on_disk = Some(blob);
            } else {
                self.toast.set("Could not save vault");
            }
        }
    }

    /// Commit the add-password modal.
    #[cfg(not(test))]
    fn commit_add_password(&mut self) {
        if self.add_service.is_empty() || self.add_account.is_empty() {
            self.toast.set("Service and account are required");
            return;
        }
        match self
            .vault
            .add_password(&self.add_service, &self.add_account, &self.add_password)
        {
            Ok(()) => {
                self.save();
                self.toast.set("Saved");
                self.modal = Modal::None;
                self.add_service.clear();
                self.add_account.clear();
                self.add_password.clear();
                self.add_field = AddField::Service;
            }
            Err(_) => self.toast.set("Could not add credential"),
        }
    }

    /// Commit the import-otpauth modal.
    #[cfg(not(test))]
    fn commit_import(&mut self) {
        match self.vault.import_otpauth(&self.import_uri) {
            Some(_) => {
                self.save();
                self.toast.set("Authenticator entry added");
                self.modal = Modal::None;
                self.import_uri.clear();
                self.tab = Tab::Authenticator;
                self.selected = 0;
            }
            None => self.toast.set("Not a valid otpauth:// URI"),
        }
    }
}

// ── Persistence (live ELF only) ───────────────────────────────────────────

#[cfg(not(test))]
fn vault_path() -> String {
    let mut p = String::new();
    let mut info = [0u8; 96];
    if raekit::sys::session_info(&mut info).is_some() {
        if let Some(home) = raekit::sys::session_home_from(&info) {
            p.push_str(home);
            p.push_str("/.config/");
            p.push_str(VAULT_FILE);
            return p;
        }
    }
    p.push_str("/home/user/.config/");
    p.push_str(VAULT_FILE);
    p
}

#[cfg(not(test))]
fn config_dir() -> String {
    let mut p = String::new();
    let mut info = [0u8; 96];
    if raekit::sys::session_info(&mut info).is_some() {
        if let Some(home) = raekit::sys::session_home_from(&info) {
            p.push_str(home);
            p.push_str("/.config");
            return p;
        }
    }
    p.push_str("/home/user/.config");
    p
}

/// Read the encrypted vault blob, or `None` if absent/unreadable. The blob is
/// opaque ciphertext — nothing is decrypted here.
#[cfg(not(test))]
fn load_vault_blob() -> Option<Vec<u8>> {
    let path = vault_path();
    let fd = raekit::sys::open(path.as_str(), 0);
    if fd == u64::MAX {
        return None;
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Cap: a vault is credentials, not files — refuse a giant blob.
        if data.len() > 16 * 1024 * 1024 {
            break;
        }
        let n = raekit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = raekit::sys::close(fd);
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

/// Write the encrypted vault blob (O_WRONLY|O_CREAT|O_TRUNC). Returns success.
#[cfg(not(test))]
fn save_vault_blob(blob: &[u8]) -> bool {
    let _ = raekit::sys::mkdir(config_dir().as_str());
    let path = vault_path();
    let fd = raekit::sys::open(path.as_str(), 0x0241);
    if fd == u64::MAX {
        return false;
    }
    let mut off = 0usize;
    while off < blob.len() {
        let end = (off + 4096).min(blob.len());
        let n = raekit::sys::write(fd, &blob[off..end]) as usize;
        if n == 0 {
            let _ = raekit::sys::close(fd);
            return false;
        }
        off += n;
    }
    let _ = raekit::sys::close(fd);
    true
}

// ── Render ─────────────────────────────────────────────────────────────────

#[cfg(not(test))]
fn render(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // Title bar.
    canvas.fill_rect(0, 0, WIN_W, TITLE_H, PANEL);
    canvas.draw_text_aa(
        10,
        ((TITLE_H - rae_tokens::TYPE_SUBTITLE.line_height as usize) / 2) as i32,
        "Passwords & Authenticator",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_SECONDARY,
        FontFamily::Sans,
    );

    if !app.unlocked {
        render_unlock(app, canvas);
    } else {
        render_tabs(app, canvas);
        render_list(app, canvas);
        if app.modal != Modal::None {
            render_modal(app, canvas);
        }
    }

    render_footer(app, canvas);
}

#[cfg(not(test))]
fn render_unlock(app: &App, canvas: &mut Canvas) {
    let cx = WIN_W / 2;
    let cy = WIN_H / 2 - 40;

    // Lock glyph (a simple shackle + body, drawn from primitives).
    let lock_w = 56;
    let lock_h = 44;
    let lx = cx - lock_w / 2;
    let ly = cy - 70;
    canvas.fill_rounded_rect(lx, ly + 18, lock_w, lock_h, 8, accent());
    // Shackle.
    canvas.fill_circle(cx, ly + 14, 16, DARK.text_tertiary);
    canvas.fill_circle(cx, ly + 14, 9, PANEL);
    canvas.fill_rect(lx, ly + 14, lock_w, 8, PANEL);

    let prompt = if app.on_disk.is_some() {
        "Enter your master passphrase to unlock"
    } else {
        "Create a master passphrase for your new vault"
    };
    let pw = canvas.measure_text_aa(prompt, rae_tokens::TYPE_BODY, FontFamily::Sans);
    canvas.draw_text_aa(
        (cx as i32) - pw / 2,
        cy as i32,
        prompt,
        rae_tokens::TYPE_BODY,
        TEXT_SECONDARY,
        FontFamily::Sans,
    );

    // Passphrase field — masked.
    let field_w = 320;
    let field_h = 36;
    let fx = cx - field_w / 2;
    let fy = cy + 30;
    canvas.fill_rounded_rect(
        fx,
        fy,
        field_w,
        field_h,
        rae_tokens::RADIUS_SM as usize,
        PANEL,
    );
    let masked: String = core::iter::repeat('•')
        .take(app.passphrase.len().min(32))
        .collect();
    let shown = if masked.is_empty() {
        "type passphrase, Enter to unlock"
    } else {
        masked.as_str()
    };
    let fg = if masked.is_empty() {
        TEXT_TERTIARY
    } else {
        TEXT_PRIMARY
    };
    canvas.draw_text_aa(
        fx as i32 + 12,
        fy as i32 + ((field_h - rae_tokens::TYPE_BODY.line_height as usize) / 2) as i32,
        shown,
        rae_tokens::TYPE_BODY,
        fg,
        FontFamily::Sans,
    );
}

#[cfg(not(test))]
fn render_tabs(app: &App, canvas: &mut Canvas) {
    let y = TITLE_H;
    canvas.fill_rect(0, y, WIN_W, TABBAR_H, PANEL);
    let half = WIN_W / 2;
    for (i, (label, tab)) in [
        ("Passwords", Tab::Passwords),
        ("Authenticator", Tab::Authenticator),
    ]
    .iter()
    .enumerate()
    {
        let tx = i * half;
        let active = app.tab == *tab;
        if active {
            canvas.fill_rect(tx, y, half, TABBAR_H, ROW_SEL);
            canvas.fill_rect(tx, y + TABBAR_H - 3, half, 3, accent());
        }
        let fg = if active { TEXT_PRIMARY } else { TEXT_SECONDARY };
        let lw = canvas.measure_text_aa(label, rae_tokens::TYPE_LABEL, FontFamily::Sans);
        canvas.draw_text_aa(
            (tx + half / 2) as i32 - lw / 2,
            (y + (TABBAR_H - rae_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
            label,
            rae_tokens::TYPE_LABEL,
            fg,
            FontFamily::Sans,
        );
    }
}

#[cfg(not(test))]
fn render_list(app: &App, canvas: &mut Canvas) {
    let top = TITLE_H + TABBAR_H + 6;
    let rows = app.visible_rows();
    if rows.is_empty() {
        let msg = match app.tab {
            Tab::Passwords => "No saved passwords yet.  Press A to add one.",
            Tab::Authenticator => "No 2FA codes yet.  Press I to import an otpauth:// URI.",
        };
        let lw = canvas.measure_text_aa(msg, rae_tokens::TYPE_BODY, FontFamily::Sans);
        canvas.draw_text_aa(
            (WIN_W as i32 - lw) / 2,
            (top + 40) as i32,
            msg,
            rae_tokens::TYPE_BODY,
            TEXT_TERTIARY,
            FontFamily::Sans,
        );
        return;
    }

    let unix = wall_secs();
    let max_rows = (WIN_H - top - FOOTER_H) / ROW_H;
    for (vis, row) in rows.iter().take(max_rows).enumerate() {
        let ry = top + vis * ROW_H;
        let selected = vis == app.selected.min(rows.len().saturating_sub(1));
        let bg = if selected { ROW_SEL } else { ROW_BG };
        canvas.fill_rounded_rect(
            8,
            ry,
            WIN_W - 16,
            ROW_H - 6,
            rae_tokens::RADIUS_SM as usize,
            bg,
        );

        // Service (primary) + account (secondary), stacked.
        canvas.draw_text_aa(
            18,
            ry as i32 + 6,
            &row.service,
            rae_tokens::TYPE_BODY,
            TEXT_PRIMARY,
            FontFamily::Sans,
        );
        canvas.draw_text_aa(
            18,
            ry as i32 + 24,
            &row.account,
            rae_tokens::TYPE_CAPTION,
            TEXT_SECONDARY,
            FontFamily::Sans,
        );

        match app.tab {
            Tab::Authenticator => {
                if let Some((code, remaining)) =
                    app.vault.totp_code_at(&row.service, &row.account, unix)
                {
                    // The big monospace code.
                    let code_w =
                        canvas.measure_text_aa(&code, rae_tokens::TYPE_TITLE, FontFamily::Mono);
                    let code_x = (WIN_W as i32) - 16 - 44 - code_w;
                    canvas.draw_text_aa(
                        code_x,
                        ry as i32 + 8,
                        &code,
                        rae_tokens::TYPE_TITLE,
                        accent(),
                        FontFamily::Mono,
                    );
                    // Seconds-remaining ring (a shrinking arc approximated by a
                    // filled circle whose radius tracks the countdown).
                    let ring_cx = (WIN_W - 16 - 20) as usize;
                    let ring_cy = ry + (ROW_H - 6) / 2;
                    let period = total_period(&app.vault, &row.service, &row.account);
                    let frac = remaining as usize * 14 / period.max(1) as usize;
                    let ring_color = if remaining <= 5 { DANGER } else { OK_COLOR };
                    canvas.fill_circle(ring_cx, ring_cy, 14, DARK.bg_base);
                    canvas.fill_circle(ring_cx, ring_cy, frac.max(2), ring_color);
                    canvas.fill_circle(ring_cx, ring_cy, frac.saturating_sub(4), bg);
                    let secs = num_str(remaining);
                    let sw =
                        canvas.measure_text_aa(&secs, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
                    canvas.draw_text_aa(
                        ring_cx as i32 - sw / 2,
                        ring_cy as i32 - (rae_tokens::TYPE_CAPTION.line_height as i32) / 2,
                        &secs,
                        rae_tokens::TYPE_CAPTION,
                        TEXT_PRIMARY,
                        FontFamily::Sans,
                    );
                }
            }
            Tab::Passwords => {
                let revealed = app
                    .revealed
                    .as_ref()
                    .map(|(s, a)| s == &row.service && a == &row.account)
                    .unwrap_or(false);
                let (text, fg) = if revealed {
                    match app.vault.secret(&row.service, &row.account) {
                        Some(bytes) => match core::str::from_utf8(bytes) {
                            Ok(s) => (String::from(s), TEXT_PRIMARY),
                            Err(_) => (String::from("<binary secret>"), TEXT_TERTIARY),
                        },
                        None => (String::from("••••••••"), TEXT_TERTIARY),
                    }
                } else {
                    (String::from("••••••••"), TEXT_TERTIARY)
                };
                let tw = canvas.measure_text_aa(&text, rae_tokens::TYPE_BODY, FontFamily::Mono);
                canvas.draw_text_aa(
                    (WIN_W as i32) - 18 - tw,
                    ry as i32 + 14,
                    &text,
                    rae_tokens::TYPE_BODY,
                    fg,
                    FontFamily::Mono,
                );
            }
        }
    }
}

/// The TOTP period for an entry (for the countdown ring), default 30 s.
#[cfg(not(test))]
fn total_period(vault: &VaultModel, service: &str, account: &str) -> u64 {
    vault
        .secret(service, account)
        .and_then(|s| core::str::from_utf8(s).ok())
        .and_then(OtpAuth::parse)
        .map(|a| {
            if a.period == 0 {
                DEFAULT_STEP_SECS
            } else {
                a.period
            }
        })
        .unwrap_or(DEFAULT_STEP_SECS)
}

#[cfg(not(test))]
fn render_modal(app: &App, canvas: &mut Canvas) {
    // Scrim.
    canvas.fill_rect(0, 0, WIN_W, WIN_H, rae_tokens::SCRIM_CAPTURE);
    let mw = 400;
    let mh = if app.modal == Modal::AddPassword {
        230
    } else {
        170
    };
    let mx = (WIN_W - mw) / 2;
    let my = (WIN_H - mh) / 2;
    canvas.fill_rounded_rect(mx, my, mw, mh, rae_tokens::RADIUS_MD as usize, PANEL);

    let title = match app.modal {
        Modal::AddPassword => "Add password",
        Modal::ImportOtp => "Import authenticator (otpauth://)",
        Modal::None => "",
    };
    canvas.draw_text_aa(
        mx as i32 + 16,
        my as i32 + 14,
        title,
        rae_tokens::TYPE_SUBTITLE,
        TEXT_PRIMARY,
        FontFamily::Sans,
    );

    let field =
        |canvas: &mut Canvas, y: usize, label: &str, value: &str, focused: bool, mask: bool| {
            canvas.draw_text_aa(
                mx as i32 + 16,
                y as i32,
                label,
                rae_tokens::TYPE_CAPTION,
                TEXT_TERTIARY,
                FontFamily::Sans,
            );
            let fh = 30;
            let fw = mw - 32;
            let stroke = if focused { accent() } else { STROKE };
            canvas.fill_rounded_rect(mx + 16, y + 16, fw, fh, rae_tokens::RADIUS_SM as usize, BG);
            canvas.fill_rect(mx + 16, y + 16 + fh - 2, fw, 2, stroke);
            let shown: String = if mask {
                core::iter::repeat('•')
                    .take(value.chars().count().min(40))
                    .collect()
            } else {
                String::from(value)
            };
            canvas.draw_text_aa(
                mx as i32 + 26,
                (y + 16 + (fh - rae_tokens::TYPE_BODY.line_height as usize) / 2) as i32,
                &shown,
                rae_tokens::TYPE_BODY,
                TEXT_PRIMARY,
                FontFamily::Mono,
            );
        };

    match app.modal {
        Modal::AddPassword => {
            field(
                canvas,
                my + 44,
                "Service",
                &app.add_service,
                app.add_field == AddField::Service,
                false,
            );
            field(
                canvas,
                my + 100,
                "Account",
                &app.add_account,
                app.add_field == AddField::Account,
                false,
            );
            field(
                canvas,
                my + 156,
                "Password",
                &app.add_password,
                app.add_field == AddField::Password,
                true,
            );
            canvas.draw_text_aa(
                mx as i32 + 16,
                my as i32 + mh as i32 - 22,
                "Tab: next field   Enter: save   Esc: cancel",
                rae_tokens::TYPE_CAPTION,
                TEXT_TERTIARY,
                FontFamily::Sans,
            );
        }
        Modal::ImportOtp => {
            field(
                canvas,
                my + 44,
                "otpauth:// URI",
                &app.import_uri,
                true,
                false,
            );
            canvas.draw_text_aa(
                mx as i32 + 16,
                my as i32 + mh as i32 - 22,
                "Enter: import   Esc: cancel",
                rae_tokens::TYPE_CAPTION,
                TEXT_TERTIARY,
                FontFamily::Sans,
            );
        }
        Modal::None => {}
    }
}

#[cfg(not(test))]
fn render_footer(app: &App, canvas: &mut Canvas) {
    let fy = WIN_H - FOOTER_H;
    canvas.fill_rect(0, fy, WIN_W, FOOTER_H, PANEL);
    let hint = if !app.unlocked {
        "Enter: unlock   Esc: quit"
    } else {
        match app.tab {
            Tab::Passwords => "A: add   R: reveal   D: delete   1/2: tabs   L: lock   Esc: quit",
            Tab::Authenticator => "I: import   D: delete   1/2: tabs   L: lock   Esc: quit",
        }
    };
    canvas.draw_text_aa(
        10,
        fy as i32 + ((FOOTER_H - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32,
        if app.toast.text.is_empty() {
            hint
        } else {
            app.toast.text.as_str()
        },
        rae_tokens::TYPE_CAPTION,
        if app.toast.text.is_empty() {
            TEXT_TERTIARY
        } else {
            accent()
        },
        FontFamily::Sans,
    );
}

/// Decimal string for a small number (countdown / seconds) — no alloc-heavy
/// formatting machinery, `no_std`-safe.
#[cfg(not(test))]
fn num_str(mut n: u64) -> String {
    if n == 0 {
        return String::from("0");
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    String::from(core::str::from_utf8(&buf[i..]).unwrap_or("0"))
}

// ── Input: US-QWERTY scancode → ASCII (live ELF only) ─────────────────────

/// Scancode → ASCII covering everything a passphrase, a credential field, and an
/// `otpauth://` URI need (letters, digits, and the URI punctuation `: / ? = & @
/// % . _ -`). Returns `None` for keys we do not type.
#[cfg(not(test))]
fn scancode_to_ascii(code: u8, shift: bool) -> Option<u8> {
    let base: u8 = match code {
        0x10 => b'q',
        0x11 => b'w',
        0x12 => b'e',
        0x13 => b'r',
        0x14 => b't',
        0x15 => b'y',
        0x16 => b'u',
        0x17 => b'i',
        0x18 => b'o',
        0x19 => b'p',
        0x1E => b'a',
        0x1F => b's',
        0x20 => b'd',
        0x21 => b'f',
        0x22 => b'g',
        0x23 => b'h',
        0x24 => b'j',
        0x25 => b'k',
        0x26 => b'l',
        0x2C => b'z',
        0x2D => b'x',
        0x2E => b'c',
        0x2F => b'v',
        0x30 => b'b',
        0x31 => b'n',
        0x32 => b'm',
        0x02 => return Some(if shift { b'!' } else { b'1' }),
        0x03 => return Some(if shift { b'@' } else { b'2' }),
        0x04 => return Some(if shift { b'#' } else { b'3' }),
        0x05 => return Some(if shift { b'$' } else { b'4' }),
        0x06 => return Some(if shift { b'%' } else { b'5' }),
        0x07 => return Some(if shift { b'^' } else { b'6' }),
        0x08 => return Some(if shift { b'&' } else { b'7' }),
        0x09 => return Some(if shift { b'*' } else { b'8' }),
        0x0A => return Some(if shift { b'(' } else { b'9' }),
        0x0B => return Some(if shift { b')' } else { b'0' }),
        0x39 => return Some(b' '),
        0x0C => return Some(if shift { b'_' } else { b'-' }),
        0x0D => return Some(if shift { b'+' } else { b'=' }),
        0x27 => return Some(if shift { b':' } else { b';' }),
        0x34 => return Some(if shift { b'>' } else { b'.' }),
        0x35 => return Some(if shift { b'?' } else { b'/' }),
        _ => return None,
    };
    if shift {
        Some(base.to_ascii_uppercase())
    } else {
        Some(base)
    }
}

/// Push a typed char into the focused text buffer for the active modal/screen.
#[cfg(not(test))]
fn type_into(app: &mut App, ch: u8) {
    let c = ch as char;
    match app.modal {
        Modal::AddPassword => match app.add_field {
            AddField::Service => app.add_service.push(c),
            AddField::Account => app.add_account.push(c),
            AddField::Password => app.add_password.push(c),
        },
        Modal::ImportOtp => app.import_uri.push(c),
        Modal::None => {
            if !app.unlocked {
                if app.passphrase.len() < 128 {
                    app.passphrase.push(c);
                }
            }
        }
    }
}

/// Backspace the focused buffer.
#[cfg(not(test))]
fn backspace(app: &mut App) {
    match app.modal {
        Modal::AddPassword => match app.add_field {
            AddField::Service => {
                app.add_service.pop();
            }
            AddField::Account => {
                app.add_account.pop();
            }
            AddField::Password => {
                app.add_password.pop();
            }
        },
        Modal::ImportOtp => {
            app.import_uri.pop();
        }
        Modal::None => {
            if !app.unlocked {
                app.passphrase.pop();
            }
        }
    }
}

// ===========================================================================
// Live entry point.
// ===========================================================================

/// The freestanding userspace entry (called by the `_start` shim in `main.rs`).
/// Creates the window surface, runs the event loop, redraws on change. A locked
/// vault drops to the unlock screen; an unlocked vault shows the tabs.
#[cfg(not(test))]
pub fn run() -> ! {
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }
    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    let mut app = App::new();
    render(&app, &mut canvas);
    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    let mut extended = false;
    // Re-present the TOTP view once a second so codes/countdowns stay live even
    // with no input. `last_tick` is the last wall-second we redrew on.
    let mut last_tick = wall_secs();

    loop {
        // ── Mouse: a click on the tab bar switches tabs (the primary couch/
        // pointer affordance); the keyboard drives everything else.
        let mut left_down = false;
        let mut mouse_edge = false;
        loop {
            let ev = raekit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            let now_down = (ev & 0x01) != 0;
            if now_down && !left_down {
                mouse_edge = true;
            }
            left_down = now_down;
        }
        if mouse_edge && app.unlocked && app.modal == Modal::None {
            let (cx, cy, _btn) = raekit::sys::cursor_pos();
            let (ox, oy) =
                raekit::sys::surface_origin(sid).unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
            let lx = (cx as i32).saturating_sub(ox as i32);
            let ly = (cy as i32).saturating_sub(oy as i32);
            let tab_top = TITLE_H as i32;
            if ly >= tab_top && ly < tab_top + TABBAR_H as i32 {
                app.tab = if lx < (WIN_W / 2) as i32 {
                    Tab::Passwords
                } else {
                    Tab::Authenticator
                };
                app.selected = 0;
                app.revealed = None;
                app.toast.clear();
                render(&app, &mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            }
        }

        let key = raekit::sys::read_key();
        if key == 0 {
            // Idle: keep the authenticator's live codes ticking.
            let now = wall_secs();
            if app.unlocked && app.tab == Tab::Authenticator && now != last_tick {
                last_tick = now;
                render(&app, &mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            }
            raekit::sys::yield_now();
            continue;
        }

        let sc = key as u8;
        if sc == 0xE0 {
            extended = true;
            continue;
        }
        let ext = core::mem::replace(&mut extended, false);
        let release = sc & 0x80 != 0;
        let code = sc & 0x7F;

        if code == 0x2A || code == 0x36 {
            app.shift = !release;
            continue;
        }
        if release {
            continue;
        }

        let mut changed = true;

        // Esc (0x01): close modal, else lock-or-quit.
        if code == 0x01 {
            if app.modal != Modal::None {
                app.modal = Modal::None;
                app.toast.clear();
            } else if app.unlocked {
                app.lock();
            } else {
                raekit::sys::exit(0);
            }
        }
        // Enter (0x1C): unlock / commit modal.
        else if code == 0x1C {
            #[cfg(not(test))]
            {
                if !app.unlocked {
                    app.try_unlock();
                } else {
                    match app.modal {
                        Modal::AddPassword => app.commit_add_password(),
                        Modal::ImportOtp => app.commit_import(),
                        Modal::None => changed = false,
                    }
                }
            }
        }
        // Backspace (0x0E).
        else if code == 0x0E {
            backspace(&mut app);
        }
        // Tab (0x0F): cycle add-password fields.
        else if code == 0x0F && app.modal == Modal::AddPassword {
            app.add_field = match app.add_field {
                AddField::Service => AddField::Account,
                AddField::Account => AddField::Password,
                AddField::Password => AddField::Service,
            };
        }
        // Arrow up/down (extended) — move the list selection.
        else if ext && code == 0x48 && app.unlocked && app.modal == Modal::None {
            if app.selected > 0 {
                app.selected -= 1;
                app.revealed = None;
            }
        } else if ext && code == 0x50 && app.unlocked && app.modal == Modal::None {
            let n = app.visible_rows().len();
            if app.selected + 1 < n {
                app.selected += 1;
                app.revealed = None;
            }
        }
        // Unlocked, no modal: command keys.
        else if app.unlocked && app.modal == Modal::None {
            match scancode_to_ascii(code, false) {
                Some(b'1') => {
                    app.tab = Tab::Passwords;
                    app.selected = 0;
                    app.revealed = None;
                    app.toast.clear();
                }
                Some(b'2') => {
                    app.tab = Tab::Authenticator;
                    app.selected = 0;
                    app.revealed = None;
                    app.toast.clear();
                }
                Some(b'a') if app.tab == Tab::Passwords => {
                    app.modal = Modal::AddPassword;
                    app.add_field = AddField::Service;
                    app.toast.clear();
                }
                Some(b'i') if app.tab == Tab::Authenticator => {
                    app.modal = Modal::ImportOtp;
                    app.toast.clear();
                }
                Some(b'r') if app.tab == Tab::Passwords => {
                    // Toggle reveal for the selected row (explicit action only).
                    let rows = app.visible_rows();
                    if let Some(row) = rows.get(app.selected) {
                        let key = (row.service.clone(), row.account.clone());
                        app.revealed = if app.revealed.as_ref() == Some(&key) {
                            None
                        } else {
                            Some(key)
                        };
                    }
                }
                Some(b'd') => {
                    let rows = app.visible_rows();
                    if let Some(row) = rows.get(app.selected) {
                        let (s, a) = (row.service.clone(), row.account.clone());
                        if app.vault.delete(&s, &a) {
                            #[cfg(not(test))]
                            app.save();
                            app.toast.set("Deleted");
                            if app.selected > 0 {
                                app.selected -= 1;
                            }
                            app.revealed = None;
                        }
                    }
                }
                Some(b'l') => app.lock(),
                _ => changed = false,
            }
        }
        // Locked OR inside a text modal: type printable chars.
        else if let Some(ascii) = scancode_to_ascii(code, app.shift) {
            type_into(&mut app, ascii);
        } else {
            changed = false;
        }

        if changed {
            render(&app, &mut canvas);
            raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }
    }
}

// ===========================================================================
// Host KAT — links the LIVE engines, no kernel. `cargo test -p passwords
// --features host`.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rae_keychain::KdfParams;

    // RFC 6238 Appendix B SHA-1 vector secret (ASCII "12345678901234567890"),
    // base32-encoded for the otpauth:// URI. At unix_time = 59, the 6-digit TOTP
    // is "287082" (the last six of the 8-digit 94287082). This is the FAIL-able
    // anchor: a wrong code or a leaked plaintext-on-wrong-passphrase fails it.
    const OTP_URI: &str = "otpauth://totp/ACME%20Co:alice@acme.com?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=ACME%20Co&algorithm=SHA1&digits=6&period=30";
    const RFC_FIXED_TIME: u64 = 59;
    const RFC_EXPECTED_CODE: &str = "287082";

    const SEED: &[u8] = b"raeenos-passwords-host-kat-seed-32by";
    const PASS: &str = "correct horse battery staple";
    const WRONG: &str = "Tr0ub4dor&3";

    fn cheap() -> KdfParams {
        KdfParams::test_cheap()
    }

    #[test]
    fn create_store_reopen_right_wrong_and_totp() {
        // 1. Create a fresh vault (no existing blob) with a master passphrase.
        let mut v = VaultModel::new();
        let outcome = v.unlock_with_params(None, PASS, SEED, cheap());
        assert_eq!(
            outcome,
            UnlockOutcome::Created,
            "fresh vault must be created"
        );
        assert!(v.is_open());
        assert!(v.is_empty());

        // 2. Store a plain password AND import a known-TOTP otpauth:// URI.
        v.add_password("github.com", "octocat", "hunter2")
            .expect("add password");
        let (svc, acct) = v.import_otpauth(OTP_URI).expect("import otpauth URI");
        assert_eq!(svc, "ACME Co");
        assert_eq!(acct, "alice@acme.com");
        assert_eq!(v.len(), 2);

        // The list NEVER includes a secret; the TOTP flag is set only on the
        // imported entry.
        let rows = v.rows();
        assert_eq!(rows.len(), 2);
        let totp_rows = v.totp_rows();
        assert_eq!(
            totp_rows.len(),
            1,
            "exactly one entry carries a TOTP secret"
        );
        assert_eq!(totp_rows[0].service, "ACME Co");

        // 3. Compute the TOTP at the FIXED RFC timestamp — must match the vector.
        let (code, remaining) = v
            .totp_code_at("ACME Co", "alice@acme.com", RFC_FIXED_TIME)
            .expect("entry has a TOTP code");
        assert_eq!(code, RFC_EXPECTED_CODE, "TOTP must match RFC 6238 vector");
        assert_eq!(code.len(), 6);
        // At t=59 within a 30s step, 59 % 30 = 29 → 1 second remains.
        assert_eq!(remaining, 1, "seconds-remaining within the 30s step");

        // 4. Serialize the encrypted blob (FRESH-nonce save path — the live path).
        let blob = v
            .to_bytes_with_fresh_nonce(SEED)
            .expect("serialize open vault");
        assert!(blob.len() > 16);

        // 5. Re-open with the RIGHT passphrase → succeeds, data intact, code same.
        let mut v2 = VaultModel::new();
        assert_eq!(
            v2.unlock_with_params(Some(&blob), PASS, SEED, cheap()),
            UnlockOutcome::Opened
        );
        assert_eq!(v2.len(), 2);
        assert_eq!(
            v2.totp_code_at("ACME Co", "alice@acme.com", RFC_FIXED_TIME)
                .map(|(c, _)| c),
            Some(String::from(RFC_EXPECTED_CODE))
        );
        // The plain password round-trips and is revealable only by explicit fetch.
        assert_eq!(v2.secret("github.com", "octocat"), Some(&b"hunter2"[..]));

        // 6. Re-open with the WRONG passphrase → FAIL CLOSED (no plaintext, no
        // oracle). The model stays locked.
        let mut v3 = VaultModel::new();
        assert_eq!(
            v3.unlock_with_params(Some(&blob), WRONG, SEED, cheap()),
            UnlockOutcome::WrongPassphrase
        );
        assert!(!v3.is_open(), "wrong passphrase must NOT open the vault");
        assert_eq!(v3.len(), 0);
        assert_eq!(v3.secret("github.com", "octocat"), None);
    }

    #[test]
    fn corrupt_blob_is_not_a_passphrase_error() {
        // A blob that fails magic/structure (not the AEAD tag) is Corrupt, not
        // WrongPassphrase — the UI distinguishes "bad password" from "bad file".
        let mut v = VaultModel::new();
        let junk = [0xABu8; 64];
        assert_eq!(
            v.unlock_with_params(Some(&junk), PASS, SEED, cheap()),
            UnlockOutcome::Corrupt
        );
        assert!(!v.is_open());
    }

    #[test]
    fn non_totp_credential_is_not_listed_as_authenticator() {
        let mut v = VaultModel::new();
        v.unlock_with_params(None, PASS, SEED, cheap());
        v.add_password("example.com", "bob", "s3cr3t").unwrap();
        // A plain password is never a TOTP row, and produces no code.
        assert!(v.totp_rows().is_empty());
        assert!(v.totp_code_at("example.com", "bob", 59).is_none());
    }

    #[test]
    fn locked_vault_yields_nothing() {
        let v = VaultModel::new();
        assert!(!v.is_open());
        assert!(v.rows().is_empty());
        assert!(v.totp_rows().is_empty());
        assert_eq!(v.secret("x", "y"), None);
        assert!(v.to_bytes_with_fresh_nonce(SEED).is_none());
        assert!(v.to_bytes_stored_nonce().is_none());
    }

    /// Pull the 12-byte ChaCha20-Poly1305 nonce out of a serialized vault header.
    ///
    /// RaeKeychain on-disk layout (little-endian):
    /// `magic[8] | version[2] | t_cost[4] | m_kib[4] | par[1] | salt_len[1] |
    ///  salt[16] | nonce[12] | ct+tag[..]` — so the nonce begins at offset
    /// `8+2+4+4+1+1 + 16 = 36`. Asserts the magic + salt_len first so a layout
    /// drift in rae_keychain trips this helper instead of silently reading the
    /// wrong bytes (keeps the differ-nonce test honest).
    fn nonce_of(blob: &[u8]) -> [u8; rae_keychain::NONCE_LEN] {
        assert!(blob.len() >= 36 + rae_keychain::NONCE_LEN, "blob too short");
        assert_eq!(
            &blob[..8],
            &rae_keychain::MAGIC,
            "unexpected keychain magic"
        );
        assert_eq!(
            blob[19] as usize,
            rae_keychain::SALT_LEN,
            "unexpected salt_len — header layout drifted"
        );
        let off = 8 + 2 + 4 + 4 + 1 + 1 + rae_keychain::SALT_LEN; // = 36
        let mut n = [0u8; rae_keychain::NONCE_LEN];
        n.copy_from_slice(&blob[off..off + rae_keychain::NONCE_LEN]);
        n
    }

    /// THE security regression gate for the nonce-reuse defect: two successive
    /// saves of a mutated vault MUST use DIFFERENT AEAD nonces, and the final blob
    /// MUST still open + round-trip every credential.
    ///
    /// FAIL-ABILITY: the same body run through the OLD stored-nonce path
    /// (`to_bytes_stored_nonce`) is asserted to REUSE the nonce — proving this test
    /// catches the catastrophic reuse it guards against (see
    /// `stored_nonce_path_reuses_nonce_proving_test_failable`).
    #[test]
    fn successive_saves_rotate_nonce_and_still_open() {
        let mut v = VaultModel::new();
        assert_eq!(
            v.unlock_with_params(None, PASS, SEED, cheap()),
            UnlockOutcome::Created
        );

        // Save #1: add a credential, persist with a FRESH nonce seed.
        v.add_password("github.com", "octocat", "hunter2")
            .expect("add #1");
        let seed1: &[u8] = b"raeenos-save-seed-number-one-32bytes!!";
        let blob1 = v.to_bytes_with_fresh_nonce(seed1).expect("save #1");

        // Save #2: mutate (add another) and persist with a DIFFERENT fresh seed —
        // exactly the live app's per-save `fresh_save_seed()` behavior.
        v.add_password("aws.com", "root", "s3cr3t").expect("add #2");
        let seed2: &[u8] = b"raeenos-save-seed-number-two-32bytes!!";
        let blob2 = v.to_bytes_with_fresh_nonce(seed2).expect("save #2");

        // THE assert: the two saves did NOT reuse the (key, nonce) pair. Same
        // master key (same passphrase/salt), so the nonce MUST differ.
        let n1 = nonce_of(&blob1);
        let n2 = nonce_of(&blob2);
        assert_ne!(
            n1, n2,
            "nonce REUSE across saves — catastrophic ChaCha20-Poly1305 defect"
        );

        // The final blob still opens with the right passphrase and round-trips
        // BOTH credentials.
        let mut reopened = VaultModel::new();
        assert_eq!(
            reopened.unlock_with_params(Some(&blob2), PASS, SEED, cheap()),
            UnlockOutcome::Opened
        );
        assert_eq!(reopened.len(), 2);
        assert_eq!(
            reopened.secret("github.com", "octocat"),
            Some(&b"hunter2"[..])
        );
        assert_eq!(reopened.secret("aws.com", "root"), Some(&b"s3cr3t"[..]));

        // The wrong passphrase still fails closed after rotation.
        let mut wrong = VaultModel::new();
        assert_eq!(
            wrong.unlock_with_params(Some(&blob2), WRONG, SEED, cheap()),
            UnlockOutcome::WrongPassphrase
        );
        assert!(!wrong.is_open());
    }

    /// Demonstrates the FAIL-ability of the rotation gate above: the OLD
    /// stored-nonce save path REUSES the nonce across two saves under one key.
    /// `to_bytes_stored_nonce` is the pre-fix behavior — if a future refactor
    /// silently re-pointed the live save at it, the assertion in
    /// `successive_saves_rotate_nonce_and_still_open` (n1 != n2) would FAIL, which
    /// is exactly the bug this work fixed. Here we prove that path reuses.
    #[test]
    fn stored_nonce_path_reuses_nonce_proving_test_failable() {
        let mut v = VaultModel::new();
        v.unlock_with_params(None, PASS, SEED, cheap());
        v.add_password("github.com", "octocat", "hunter2").unwrap();
        let blob1 = v.to_bytes_stored_nonce().expect("stored-nonce save #1");
        v.add_password("aws.com", "root", "s3cr3t").unwrap();
        let blob2 = v.to_bytes_stored_nonce().expect("stored-nonce save #2");
        // The stored-nonce path REUSES the nonce — the defect. (If the rotation
        // gate were run against THIS path it would fail its n1 != n2 assert.)
        assert_eq!(
            nonce_of(&blob1),
            nonce_of(&blob2),
            "the stored-nonce path is expected to reuse the nonce (the bug)"
        );
    }

    /// Two distinct save seeds must yield distinct nonces (the spreading function
    /// does not collapse different entropy to the same nonce). Belt-and-suspenders
    /// for the per-save counter + entropy mix.
    #[test]
    fn distinct_seeds_yield_distinct_nonces() {
        let mut v = VaultModel::new();
        v.unlock_with_params(None, PASS, SEED, cheap());
        v.add_password("svc", "acc", "pw").unwrap();
        let a = v.to_bytes_with_fresh_nonce(b"seed-alpha-distinct").unwrap();
        let b = v.to_bytes_with_fresh_nonce(b"seed-bravo-distinct").unwrap();
        assert_ne!(nonce_of(&a), nonce_of(&b));
    }
}
