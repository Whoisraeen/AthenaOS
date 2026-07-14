//! User sessions — wires `raeid::AccountManager` into the kernel boot path.
//!
//! Concept § AthID: passkeys first, optional, never required for local use.
//! Guest mode is full-featured; local accounts use password auth at login.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use core::sync::atomic::Ordering;
use spin::Mutex;

use raeid::{AccountManager, AuthResult, DeviceInfo, SessionToken, UserId, GUEST_USER_ID};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    /// First-boot OOBE — no user has completed setup yet. Renders
    /// `setup_ui` instead of `login_ui`. Transitions to `Active`
    /// (auto-sign-in) on successful account creation. Subsequent boots
    /// skip straight to `Login` because the config registry has
    /// `/setup/first_boot_done = true`.
    FirstBootSetup,
    /// Login screen — desktop not available.
    Login,
    /// Signed in — shell and apps run.
    Active,
    /// Workstation locked — must re-authenticate.
    Locked,
    /// The graphical install wizard (`installer_ui`) owns the screen. Entered
    /// at boot when `/installer/autostart` is set, or on demand from the
    /// desktop (F9). The keyboard routes to the wizard; on completion the
    /// machine reboots (real install) or returns to the prior phase (cancel).
    Install,
}

struct SessionState {
    manager: AccountManager,
    phase: SessionPhase,
    active_token: Option<SessionToken>,
    active_user: Option<UserId>,
    default_user: UserId,
    token_seq: u64,
}

static SESSION: Mutex<Option<SessionState>> = Mutex::new(None);

fn now_secs() -> u64 {
    crate::timers::jiffies_to_ns(crate::timers::JIFFIES.load(Ordering::Relaxed)) / 1_000_000_000
}

fn local_device() -> DeviceInfo {
    DeviceInfo {
        device_id: [
            0x52, 0xAE, 0xE0, 0x5, 0x00, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
        device_name: String::from("This PC"),
        os_version: String::from("AthenaOS"),
        last_ip_hash: None,
    }
}

fn fresh_token(st: &mut SessionState) -> SessionToken {
    st.token_seq = st.token_seq.wrapping_add(1);
    let mut bytes = [0u8; 32];
    let t = st.token_seq;
    let now = now_secs();
    bytes[0..8].copy_from_slice(&t.to_le_bytes());
    bytes[8..16].copy_from_slice(&now.to_le_bytes());
    bytes[16..24].copy_from_slice(b"AthenaOS-");
    SessionToken(bytes)
}

/// Seed AthID with the default local account (`raeen` / `raeen`).
pub fn init() {
    let now = now_secs();
    let mut manager = AccountManager::new(now);

    let profile = manager.create_user(String::from("raeen"), String::from("Raeen"), now);
    let user_id = profile.id;

    let salt = [
        0x52, 0xAE, 0xE0, 0x5, 0x4C, 0x4F, 0x47, 0x49, 0x4E, 0x2D, 0x53, 0x41, 0x4C, 0x54, 0x00,
        0x01,
    ];
    let _ = manager.add_password(user_id, b"raeen", salt, now);

    *SESSION.lock() = Some(SessionState {
        manager,
        phase: SessionPhase::Login,
        active_token: None,
        active_user: None,
        default_user: user_id,
        token_seq: 1,
    });

    crate::serial_println!("[session] AthID ready — default user 'raeen' (password: raeen)");
    run_persistence_smoketest();
}

/// Create a local user account during install (MasterChecklist Phase 16.1).
/// Registers the account in AthID with a real Argon2id (RFC 9106) password hash
/// (via `rae_crypto`, the shared memory-hard KDF) and persists the profile to
/// the config registry so it survives reboot. Returns the new user id on
/// success, or `None` if the username is taken/invalid.
pub fn create_local_account(username: &str, display_name: &str, password: &[u8]) -> Option<u64> {
    if username.is_empty() || username.len() > 32 || password.is_empty() {
        return None;
    }
    let now = now_secs();
    let mut guard = SESSION.lock();
    let st = guard.as_mut()?;

    // Reject duplicate usernames.
    if st.manager.find_user_by_username(username).is_some() {
        crate::serial_println!(
            "[session] create_local_account: '{}' already exists",
            username
        );
        return None;
    }

    let profile = st
        .manager
        .create_user(String::from(username), String::from(display_name), now);
    let user_id = profile.id;

    // Per-account salt from the kernel CSPRNG. A salt MUST be unpredictable to
    // do its job (resist cross-account/rainbow precomputation); the old
    // `username ^ 0x5A` derivation carried ~8 bits of entropy and was otherwise
    // computable from the public username, defeating the point. Fall back to a
    // username+TSC mix only if the CSPRNG is somehow unavailable.
    let mut salt = [0u8; 16];
    if crate::crypto::getrandom(&mut salt).is_err() || salt == [0u8; 16] {
        let tsc = unsafe { core::arch::x86_64::_rdtsc() };
        for (i, b) in username.bytes().enumerate().take(16) {
            salt[i] = b ^ 0x5A;
        }
        for (i, b) in salt.iter_mut().enumerate() {
            *b ^= (tsc.rotate_left((i as u32) * 5) as u8) ^ (now as u8);
        }
    }
    let _ = st.manager.add_password(user_id, password, salt, now);

    // Account-creation password-strength feedback (Win11/macOS parity). Advisory
    // only — we do NOT block weak passwords here (the owner may set a simple
    // local password), but a warning is logged so the UI/OOBE can surface it.
    let strength = raeid::estimate_password_strength(password);
    if strength <= raeid::PasswordStrength::Weak {
        crate::serial_println!(
            "[session] account '{}' password strength: {:?} (weak — consider a longer, more varied password)",
            username,
            strength,
        );
    }

    // Persist profile so the installed system has this account on next boot.
    let prefix = alloc::format!("/users/{}/", username);
    crate::config_registry::set_text(&alloc::format!("{prefix}profile/username"), username);
    crate::config_registry::set_text(
        &alloc::format!("{prefix}profile/display_name"),
        display_name,
    );
    crate::config_registry::set_text(
        &alloc::format!("{prefix}profile/created"),
        &alloc::format!("{now}"),
    );

    // Durably persist the account (incl. its Argon2id credential) to the AthFS
    // root so the next boot has a working login. No-op when AthFS isn't mounted.
    let persisted = persist_accounts(st);

    crate::serial_println!(
        "[session] create_local_account: '{}' ({}) id={} -> OK (password set, raefs_persisted={})",
        username,
        display_name,
        user_id.0,
        persisted,
    );
    Some(user_id.0)
}

/// Flat file in the AthFS root holding all local accounts (identity + Argon2id
/// credential). See `raeid::{serialize,deserialize}_accounts`.
const ACCOUNTS_FILE: &str = "accounts.dat";

/// Write all non-guest local accounts to the AthFS root. Returns `false` (and
/// no-ops) when AthFS isn't mounted — durable persistence only happens on an
/// installed system with a AthFS root. Holds the caller's `SESSION` lock; only
/// the `RAEFS` lock is taken here (no nesting with `SESSION`).
fn persist_accounts(st: &SessionState) -> bool {
    let mut records = alloc::vec::Vec::new();
    for profile in st.manager.list_users() {
        if profile.id == GUEST_USER_ID {
            continue;
        }
        if let Some(cred) = st.manager.password_credential(profile.id) {
            records.push(raeid::AccountRecord {
                username: profile.username.clone(),
                display_name: profile.display_name.clone(),
                credential: cred.clone(),
            });
        }
    }
    let bytes = raeid::serialize_accounts(&records);
    crate::raefs::RAEFS
        .lock()
        .as_mut()
        .map(|fs| fs.write_file_bytes_on(ACCOUNTS_FILE, &bytes))
        .unwrap_or(false)
}

/// Load persisted accounts from the AthFS root into the session manager. Call
/// AFTER `storage_mount` has mounted the AthFS root (which is after
/// `session::init`). Accounts whose username already exists (e.g. the default
/// `raeen`) are skipped. Safe no-op when AthFS is absent or has no accounts.
pub fn load_persisted_accounts() {
    let bytes = match crate::raefs::RAEFS
        .lock()
        .as_ref()
        .and_then(|fs| fs.read_file_bytes_on(ACCOUNTS_FILE))
    {
        Some(b) if !b.is_empty() => b,
        _ => {
            crate::serial_println!(
                "[session] load_persisted_accounts: no accounts.dat on AthFS (fresh system)"
            );
            return;
        }
    };
    let records = raeid::deserialize_accounts(&bytes);
    let now = now_secs();
    let mut guard = SESSION.lock();
    let Some(st) = guard.as_mut() else {
        return;
    };
    let mut loaded = 0usize;
    for rec in &records {
        if st.manager.find_user_by_username(&rec.username).is_some() {
            continue;
        }
        let id = st
            .manager
            .create_user(rec.username.clone(), rec.display_name.clone(), now)
            .id;
        if st
            .manager
            .set_password_credential(id, rec.credential.clone())
            .is_ok()
        {
            loaded += 1;
        }
    }
    crate::serial_println!(
        "[session] load_persisted_accounts: {} record(s) on disk, {} loaded into session",
        records.len(),
        loaded,
    );
}

/// R10 smoketest: prove the persistence round-trip end-to-end WITHOUT requiring
/// a mounted AthFS — build an account credential, serialize it, deserialize into
/// a record, and confirm the reconstructed credential still authenticates the
/// original password (and rejects a wrong one). The actual AthFS write/read is
/// exercised by `persist_accounts` / `load_persisted_accounts` on an installed
/// system. Uses a low Argon2id cost so the boot stays fast.
pub fn run_persistence_smoketest() {
    let params = raeid::PasswordParams {
        algorithm: raeid::PasswordAlgorithm::Argon2id,
        salt: *b"raeen-persist-ck",
        iterations: 1,
        memory_cost_kb: 64,
        parallelism: 1,
    };
    let cred = raeid::PasswordCredential {
        hash: raeid::hash_password(b"s3cret-pw", &params),
        params,
        created_at: 1,
        last_changed: 1,
        requires_change: false,
    };
    let records = alloc::vec![raeid::AccountRecord {
        username: String::from("persisttest"),
        display_name: String::from("Persist Test"),
        credential: cred.clone(),
    }];
    let bytes = raeid::serialize_accounts(&records);
    let back = raeid::deserialize_accounts(&bytes);
    let hash_match = back.len() == 1 && back[0].credential.hash == cred.hash;
    let reload_auth = back.len() == 1
        && raeid::verify_password(b"s3cret-pw", &back[0].credential)
        && !raeid::verify_password(b"wrong", &back[0].credential);
    let pass = hash_match && reload_auth;
    crate::serial_println!(
        "[session] persistence smoketest: records={} hash_match={} reload_auth={} -> {}",
        back.len(),
        hash_match,
        reload_auth,
        if pass { "PASS" } else { "FAIL" },
    );
}

pub fn phase() -> SessionPhase {
    SESSION
        .lock()
        .as_ref()
        .map(|s| s.phase)
        .unwrap_or(SessionPhase::Login)
}

pub fn is_desktop_active() -> bool {
    phase() == SessionPhase::Active
}

pub fn display_name() -> alloc::string::String {
    let guard = SESSION.lock();
    let Some(st) = guard.as_ref() else {
        return String::from("Raeen");
    };
    let uid = st.active_user.unwrap_or(st.default_user);
    st.manager
        .get_user(uid)
        .map(|u| u.display_name.clone())
        .unwrap_or_else(|| String::from("User"))
}

pub fn active_user_id() -> Option<u64> {
    SESSION
        .lock()
        .as_ref()
        .and_then(|s| s.active_user.map(|u| u.0))
}

/// Login name for the active session (`raeen`, `guest`, …).
pub fn username() -> alloc::string::String {
    let guard = SESSION.lock();
    let Some(st) = guard.as_ref() else {
        return String::from("guest");
    };
    let uid = st.active_user.unwrap_or(st.default_user);
    if uid == GUEST_USER_ID {
        return String::from("guest");
    }
    st.manager
        .get_user(uid)
        .map(|u| u.username.clone())
        .unwrap_or_else(|| String::from("user"))
}

/// Per-user home directory (`/home/<username>`).
pub fn home_dir() -> alloc::string::String {
    alloc::format!("/home/{}", username())
}

/// Prefix for per-user config keys (`/users/<username>/`).
pub fn user_config_prefix() -> alloc::string::String {
    alloc::format!("/users/{}/", username())
}

fn apply_success(st: &mut SessionState, user_id: UserId, token: SessionToken) {
    st.active_user = Some(user_id);
    st.active_token = Some(token);
    st.phase = SessionPhase::Active;
    seed_user_config_for(st);
}

fn seed_user_config_for(st: &SessionState) {
    let uid = st.active_user.unwrap_or(st.default_user);
    let (user, display) = if uid == GUEST_USER_ID {
        (String::from("guest"), String::from("Guest"))
    } else {
        st.manager
            .get_user(uid)
            .map(|u| (u.username.clone(), u.display_name.clone()))
            .unwrap_or_else(|| (String::from("user"), String::from("User")))
    };
    let prefix = alloc::format!("/users/{}/", user);
    crate::config_registry::set_text(&alloc::format!("{prefix}profile/username"), &user);
    crate::config_registry::set_text(&alloc::format!("{prefix}profile/display_name"), &display);
    crate::config_registry::set_text(&alloc::format!("{prefix}desktop/wallpaper"), "RaeBlue");
    let home = alloc::format!("/home/{}", user);
    crate::config_registry::set_text(&alloc::format!("{prefix}profile/home"), &home);
    crate::serial_println!("[session] user config seeded at {}", prefix);
}

/// Non-destructive password check: verify `password` for `username` without
/// starting a session. Used by the installer to confirm the account's password
/// hash round-trips after creation. Returns true if the password is correct.
pub fn verify_local_password(username: &str, password: &[u8]) -> bool {
    let mut guard = SESSION.lock();
    let Some(st) = guard.as_mut() else {
        return false;
    };
    let user_id = match st.manager.find_user_by_username(username) {
        Some(u) => u.id,
        None => return false,
    };
    let token = fresh_token(st);
    let now = now_secs();
    matches!(
        st.manager
            .authenticate_password(user_id, password, token, local_device(), now),
        AuthResult::Success { .. }
    )
}

/// Password login for `username`. Returns true on success.
pub fn login_password(username: &str, password: &[u8]) -> bool {
    let mut guard = SESSION.lock();
    let Some(st) = guard.as_mut() else {
        return false;
    };

    let user_id = if let Some(u) = st.manager.find_user_by_username(username) {
        u.id
    } else if username.eq_ignore_ascii_case("raeen") {
        st.default_user
    } else {
        return false;
    };

    let token = fresh_token(st);
    let now = now_secs();
    match st
        .manager
        .authenticate_password(user_id, password, token, local_device(), now)
    {
        AuthResult::Success { .. } => {
            apply_success(st, user_id, token);
            crate::serial_println!("[session] user '{}' logged in", username);
            true
        }
        AuthResult::LockedOut { until } => {
            crate::serial_println!("[session] account locked until {}", until);
            false
        }
        _ => false,
    }
}

/// Guest session — no password (Concept: guest mode is full-featured).
pub fn login_guest() -> bool {
    let mut guard = SESSION.lock();
    let Some(st) = guard.as_mut() else {
        return false;
    };

    let token = fresh_token(st);
    let now = now_secs();
    match st.manager.login_as_guest(token, local_device(), now) {
        AuthResult::Success { .. } => {
            apply_success(st, GUEST_USER_ID, token);
            crate::serial_println!("[session] guest session started");
            true
        }
        _ => false,
    }
}

pub fn lock() {
    let mut guard = SESSION.lock();
    let Some(st) = guard.as_mut() else { return };
    if st.phase == SessionPhase::Active {
        st.phase = SessionPhase::Locked;
        crate::serial_println!("[session] workstation locked");
    }
}

pub fn unlock_password(password: &[u8]) -> bool {
    let mut guard = SESSION.lock();
    let Some(st) = guard.as_mut() else {
        return false;
    };
    if st.phase != SessionPhase::Locked {
        return false;
    }

    let user_id = st.active_user.unwrap_or(st.default_user);
    let token = fresh_token(st);
    let now = now_secs();
    match st
        .manager
        .authenticate_password(user_id, password, token, local_device(), now)
    {
        AuthResult::Success { .. } => {
            st.active_token = Some(token);
            st.phase = SessionPhase::Active;
            crate::serial_println!("[session] workstation unlocked");
            true
        }
        _ => false,
    }
}

/// Clear the active session without touching shell UI (used by the shell runner).
pub fn end_session() {
    let mut guard = SESSION.lock();
    let Some(st) = guard.as_mut() else { return };
    if let Some(token) = st.active_token.take() {
        st.manager.revoke_session(&token);
    }
    st.active_user = None;
    st.phase = SessionPhase::Login;
    crate::serial_println!("[session] logged out");
}

/// End session and return to the login screen (syscall / settings sign-out).
pub fn logout() {
    end_session();
    crate::shell_runner::force_login_screen();
}

/// Syscall helper: pack session info into user buffer.
pub fn write_info(buf: &mut [u8]) -> u64 {
    // Compute home_dir() FIRST: it calls username() which locks SESSION, so
    // calling it while holding the guard below would re-enter the (non-
    // reentrant) spin::Mutex and deadlock the core. This is the SAME hazard
    // dump_text() was fixed for; write_info had it uncorrected and it
    // deadlocked CPU0 on the SYS_SESSION_INFO syscall every time a first-party
    // app (Files etc.) called session_info() at launch — the SEV-1
    // "triple-fault on app launch" the beta-test reported.
    let home = home_dir();
    let home_bytes = home.as_bytes();

    let guard = SESSION.lock();
    let Some(st) = guard.as_ref() else {
        return u64::MAX;
    };

    let uid = st.active_user.map(|u| u.0).unwrap_or(0);
    let name = st
        .active_user
        .and_then(|id| st.manager.get_user(id))
        .map(|u| u.username.as_str())
        .unwrap_or("");

    let name_bytes = name.as_bytes();
    let need = 8 + 2 + name_bytes.len() + 1 + 2 + home_bytes.len();
    if buf.len() < need {
        return u64::MAX;
    }

    buf[0..8].copy_from_slice(&uid.to_le_bytes());
    buf[8..10].copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    buf[10..10 + name_bytes.len()].copy_from_slice(name_bytes);

    let phase_byte = match st.phase {
        SessionPhase::Login => 0u8,
        SessionPhase::Active => 1,
        SessionPhase::Locked => 2,
        SessionPhase::FirstBootSetup => 3,
        SessionPhase::Install => 4,
    };
    let phase_off = 10 + name_bytes.len();
    buf[phase_off] = phase_byte;
    buf[phase_off + 1..phase_off + 3].copy_from_slice(&(home_bytes.len() as u16).to_le_bytes());
    buf[phase_off + 3..phase_off + 3 + home_bytes.len()].copy_from_slice(home_bytes);

    need as u64
}

/// /proc/raeen/session — current login/session state snapshot.
pub fn dump_text() -> String {
    // Compute home_dir() FIRST: it calls username() which locks SESSION, so
    // calling it while holding the guard below would re-enter the spin::Mutex
    // and deadlock the core (froze the /proc/raeen/session boot dump).
    let home = home_dir();
    let guard = SESSION.lock();
    let Some(st) = guard.as_ref() else {
        return String::from("# AthenaOS session\nstatus: not initialized\n");
    };

    let phase = match st.phase {
        SessionPhase::Login => "login",
        SessionPhase::Active => "active",
        SessionPhase::Locked => "locked",
        SessionPhase::FirstBootSetup => "first_boot_setup",
        SessionPhase::Install => "install",
    };
    let active_uid = st.active_user.map(|u| u.0).unwrap_or(0);
    let active_name = if let Some(uid) = st.active_user {
        if uid == GUEST_USER_ID {
            String::from("guest")
        } else {
            st.manager
                .get_user(uid)
                .map(|u| u.username.clone())
                .unwrap_or_else(|| String::from("unknown"))
        }
    } else {
        String::from("none")
    };

    let mut out = String::from("# AthenaOS session\n");
    out.push_str(&alloc::format!("phase: {phase}\n"));
    out.push_str(&alloc::format!("active_uid: {active_uid}\n"));
    out.push_str(&alloc::format!("active_user: {active_name}\n"));
    out.push_str(&alloc::format!("home_dir: {}\n", home));
    out.push_str("default_account: raeen\n");
    out
}

/// Boot smoketest for session login/lock/unlock/logout flow.
pub fn run_boot_smoketest() {
    let login_ok = login_password("raeen", b"raeen");
    let mut lock_ok = false;
    let mut unlock_ok = false;
    let mut logout_ok = false;

    if login_ok {
        lock();
        lock_ok = phase() == SessionPhase::Locked;
        unlock_ok = unlock_password(b"raeen");
        logout();
        logout_ok = phase() == SessionPhase::Login;
    }

    if login_ok && lock_ok && unlock_ok && logout_ok {
        crate::serial_println!("[session] smoketest PASS: login->lock->unlock->logout");
    } else {
        crate::serial_println!(
            "[session] smoketest FAIL: login={} lock={} unlock={} logout={}",
            login_ok,
            lock_ok,
            unlock_ok,
            logout_ok
        );
    }
}
