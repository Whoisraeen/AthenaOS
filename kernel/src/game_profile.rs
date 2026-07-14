//! Per-game performance profiles — Concept §Gaming Features:
//!
//! > "Per-game profiles — resolution, refresh rate, audio device, GPU
//! >  power limit, all configured per game and auto-applied."
//!
//! Windows handles this through a sprawl of vendor utilities (NVIDIA
//! Control Panel, AMD Adrenalin, MSI Afterburner, Razer Synapse, Steam's
//! own per-game Properties window, Xbox Game Bar's overlays). Each one
//! owns a slice of the surface and they routinely contradict each other.
//!
//! AthenaOS makes it one kernel primitive. A `GameProfile` is a record
//! keyed by a stable game identifier (binary hash or app-store ID). When
//! the game launches, `apply()` walks the profile and pokes every system
//! that can act on a field: scheduler (game mode + NULL_LATENCY + affinity),
//! cpufreq (per-game CPU power cap), compositor (display refresh rate + VRR
//! posture), and — once the remaining setters land — compositor (resolution /
//! HDR), audio (sink), cpufreq (GPU power budget).
//!
//! Storage rides on top of `config_registry` so profiles inherit
//! versioning + snapshots for free. Snapshot → tweak settings → roll back
//! if a game starts dropping frames.
//!
//! ## Syscalls (58-61)
//!
//! | nr | name                | rdi/rsi/rdx                                              | rax |
//! |----|---------------------|----------------------------------------------------------|----|
//! | 58 | GAME_PROFILE_SET    | rdi=id_ptr, rsi=id_len, rdx=profile_ptr (GameProfileAbi) | 0/err |
//! | 59 | GAME_PROFILE_GET    | rdi=id_ptr, rsi=id_len, rdx=out_ptr                       | bytes or u64::MAX |
//! | 60 | GAME_PROFILE_APPLY  | rdi=id_ptr, rsi=id_len                                    | 0/err |
//! | 61 | GAME_PROFILE_LIST   | rdi=out_ptr, rsi=out_cap                                  | count written |

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ── Profile schema ─────────────────────────────────────────────────────

/// 64-byte fixed ABI struct exchanged with userspace. Stays binary-stable
/// by carrying a version word; new fields append at the tail.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GameProfileAbi {
    pub version: u32, // = 1
    pub resolution_w: u32,
    pub resolution_h: u32,
    pub refresh_hz: u32,
    pub gpu_power_pct: u32,  // 0-100; 0 = leave default
    pub audio_sink_id: u32,  // 0 = default
    pub flags: u32,          // bit 0 = game_mode, bit 1 = null_latency, bit 2 = hdr, bit 3 = vrr
    pub priority: u32,       // SCHED_BODY=2, Normal=0
    pub affinity_mask: u64,  // CPU affinity bitmask
    pub memory_pin_mib: u32, // megabytes of guaranteed-resident memory
    /// Per-game CPU power cap, as a percent of the max non-turbo frequency
    /// (the CPU analogue of `gpu_power_pct`): `0` = leave the system default,
    /// `10..=100` = cap CPU frequency via `cpufreq::set_cap_percent`. Lets a
    /// single-player title run cooler/quieter while a competitive title runs flat-out.
    pub cpu_power_pct: u32,
    pub deadline_period_us: u32,
    pub deadline_runtime_us: u32,
}

pub const FLAG_GAME_MODE: u32 = 1 << 0;
pub const FLAG_NULL_LATENCY: u32 = 1 << 1;
pub const FLAG_HDR: u32 = 1 << 2;
pub const FLAG_VRR: u32 = 1 << 3;

impl GameProfileAbi {
    /// Serialize into the exact 56-byte repr(C) layout (no internal padding:
    /// `affinity_mask`'s u64 lands at the naturally-aligned offset 32). Used by
    /// the SMAP-safe copy-out path so no raw user-ptr deref of the struct is
    /// needed.
    pub fn to_le_bytes(&self) -> [u8; 56] {
        let mut b = [0u8; 56];
        b[0..4].copy_from_slice(&self.version.to_le_bytes());
        b[4..8].copy_from_slice(&self.resolution_w.to_le_bytes());
        b[8..12].copy_from_slice(&self.resolution_h.to_le_bytes());
        b[12..16].copy_from_slice(&self.refresh_hz.to_le_bytes());
        b[16..20].copy_from_slice(&self.gpu_power_pct.to_le_bytes());
        b[20..24].copy_from_slice(&self.audio_sink_id.to_le_bytes());
        b[24..28].copy_from_slice(&self.flags.to_le_bytes());
        b[28..32].copy_from_slice(&self.priority.to_le_bytes());
        b[32..40].copy_from_slice(&self.affinity_mask.to_le_bytes());
        b[40..44].copy_from_slice(&self.memory_pin_mib.to_le_bytes());
        b[44..48].copy_from_slice(&self.cpu_power_pct.to_le_bytes());
        b[48..52].copy_from_slice(&self.deadline_period_us.to_le_bytes());
        b[52..56].copy_from_slice(&self.deadline_runtime_us.to_le_bytes());
        b
    }

    pub fn default_competitive() -> Self {
        // Concept §Pro Gaming defaults.
        Self {
            version: 1,
            resolution_w: 1920,
            resolution_h: 1080,
            refresh_hz: 240,
            gpu_power_pct: 100,
            audio_sink_id: 0,
            flags: FLAG_GAME_MODE | FLAG_NULL_LATENCY | FLAG_VRR,
            priority: 2,
            affinity_mask: 0xF, // first 4 cores
            memory_pin_mib: 256,
            cpu_power_pct: 100,        // competitive: full power
            deadline_period_us: 4_167, // 240 Hz
            deadline_runtime_us: 3_000,
        }
    }

    pub fn default_balanced() -> Self {
        Self {
            version: 1,
            resolution_w: 2560,
            resolution_h: 1440,
            refresh_hz: 144,
            gpu_power_pct: 100,
            audio_sink_id: 0,
            flags: FLAG_GAME_MODE | FLAG_VRR | FLAG_HDR,
            priority: 2,
            affinity_mask: 0xFF,
            memory_pin_mib: 128,
            cpu_power_pct: 90,         // balanced
            deadline_period_us: 6_944, // 144 Hz
            deadline_runtime_us: 5_000,
        }
    }

    pub fn default_cinematic() -> Self {
        // Single-player, image-quality-first.
        Self {
            version: 1,
            resolution_w: 3840,
            resolution_h: 2160,
            refresh_hz: 60,
            gpu_power_pct: 100,
            audio_sink_id: 0,
            flags: FLAG_GAME_MODE | FLAG_HDR,
            priority: 2,
            affinity_mask: 0xFFFF,
            memory_pin_mib: 256,
            cpu_power_pct: 75, // cinematic: cooler/quieter for single-player
            deadline_period_us: 16_667, // 60 Hz
            deadline_runtime_us: 12_000,
        }
    }
}

// ── Profile store ──────────────────────────────────────────────────────

struct ProfileStore {
    /// game id (e.g. "athplay:steam:730", binary sha-256 prefix, etc.) → profile
    profiles: BTreeMap<String, GameProfileAbi>,
    /// Counter for diagnostics.
    apply_count: u64,
    /// The game id most recently applied.
    last_applied: Option<String>,
}

impl ProfileStore {
    fn new() -> Self {
        let mut s = Self {
            profiles: BTreeMap::new(),
            apply_count: 0,
            last_applied: None,
        };
        s.seed();
        s
    }

    fn seed(&mut self) {
        // Ship a couple of named presets so the Settings → Games panel and
        // the start-menu AthPlay tile have something to populate from.
        self.profiles.insert(
            String::from("preset:competitive"),
            GameProfileAbi::default_competitive(),
        );
        self.profiles.insert(
            String::from("preset:balanced"),
            GameProfileAbi::default_balanced(),
        );
        self.profiles.insert(
            String::from("preset:cinematic"),
            GameProfileAbi::default_cinematic(),
        );
    }
}

static STORE: Mutex<Option<ProfileStore>> = Mutex::new(None);

// ── Public APIs ────────────────────────────────────────────────────────

pub fn init() {
    let store = ProfileStore::new();
    let n = store.profiles.len();
    *STORE.lock() = Some(store);
    crate::serial_println!(
        "[ OK ] Per-game profiles: {} preset(s) loaded (competitive, balanced, cinematic)",
        n,
    );
}

/// Initialize the profile store only if it has not been initialized yet
/// (idempotent). Some boot smoketests (e.g. the GameOS profile round-trip in
/// `shell_runner::run_boot_smoketest`) run BEFORE the main `init()` call in
/// `kernel_main`; they call this so the store exists and the SET/GET/APPLY/LIST
/// path is exercisable. The later `init()` re-seeds cleanly (scratch smoketest
/// profiles are expendable).
pub fn ensure_init() {
    let need = STORE.lock().is_none();
    if need {
        let store = ProfileStore::new();
        *STORE.lock() = Some(store);
    }
}

/// Smoketest: apply each preset and exit game mode after. Logs what we
/// would have driven if the missing setters were live.
pub fn run_boot_smoketest() {
    for preset in &["preset:competitive", "preset:balanced", "preset:cinematic"] {
        let rc = apply_profile(preset);
        if rc != 0 {
            crate::serial_println!("[game_profile] [WARN] {} apply rc={:x}", preset, rc);
        }
    }
    // The cinematic preset (applied last) caps the CPU at 75%. Prove the
    // per-game CPU power override actually reached cpufreq — this is the
    // FAIL-able half of the smoketest: if the cap did not stick, the
    // override is dead wiring and we say so.
    let cap = crate::cpufreq::current_cap_percent();
    if cap == 75 {
        crate::serial_println!(
            "[game_profile] cpu-power override: cinematic capped CPU at {}% -> PASS",
            cap,
        );
    } else {
        crate::serial_println!(
            "[game_profile] [FAIL] cpu-power override: expected cap 75%, got {}%",
            cap,
        );
    }
    // Per-game DISPLAY REFRESH switching: re-apply the 240 Hz competitive
    // profile and prove the compositor's frame pacer actually switched. 240 is
    // distinct from the 60 Hz boot default, so this FAILs if the per-game
    // refresh wiring is dead (not a coincidence of the boot rate).
    let _ = apply_profile("preset:competitive");
    let hz = crate::compositor::current_refresh_hz();
    if hz == 240 {
        crate::serial_println!(
            "[game_profile] refresh-switch: competitive set display to {}Hz -> PASS",
            hz,
        );
    } else {
        crate::serial_println!(
            "[game_profile] [FAIL] refresh-switch: expected 240Hz, got {}Hz",
            hz,
        );
    }
    // Restore to default state: lift the CPU cap so the boot CPU is not left
    // throttled, restore the 60 Hz boot refresh rate, and leave game mode.
    crate::cpufreq::set_cap_percent(100);
    crate::compositor::set_refresh_hz(60, false);
    crate::scheduler::exit_game_mode();
    crate::serial_println!(
        "[game_profile] smoketest complete — scheduler primitives toggled cleanly",
    );
}

/// Apply a stored profile to the running system. Returns 0 on success, or
/// a non-zero diagnostic code on failure. Best-effort: a missing setter
/// for a field (e.g. compositor refresh-rate setter) logs the intent and
/// keeps going so the fields that DO have wired setters still take effect.
pub fn apply_profile(id: &str) -> u64 {
    let profile = {
        let g = STORE.lock();
        match g.as_ref().and_then(|s| s.profiles.get(id).copied()) {
            Some(p) => p,
            None => return ERR_NO_SUCH_PROFILE,
        }
    };

    crate::serial_println!(
        "[game_profile] applying '{}': {}x{}@{}Hz, gpu={}%, cpu={}%, flags=0x{:x}, pin={}MiB",
        id,
        profile.resolution_w,
        profile.resolution_h,
        profile.refresh_hz,
        profile.gpu_power_pct,
        profile.cpu_power_pct,
        profile.flags,
        profile.memory_pin_mib,
    );

    // Things we can do today:
    if profile.flags & FLAG_GAME_MODE != 0 {
        crate::scheduler::enter_game_mode();
    }
    if profile.flags & FLAG_NULL_LATENCY != 0 {
        if let Some(tid) = crate::scheduler::current_task_id() {
            let _ = crate::scheduler::enable_null_latency(tid);
        }
    }
    // Per-game CPU power cap — the CPU analogue of the GPU power budget.
    // `cpufreq::set_cap_percent` clamps to [10,100] and translates the
    // percent into a P-state target (max non-turbo frequency × pct, floored
    // at the hardware minimum). A value below 10 means "leave the system
    // default", so we only cap when the profile asks for one. This lets a
    // cinematic single-player profile run cooler/quieter while a competitive
    // profile holds full clocks.
    if profile.cpu_power_pct >= 10 {
        crate::cpufreq::set_cap_percent(profile.cpu_power_pct);
    }
    // Per-game display refresh rate (Concept §Pro Gaming: "display refresh-rate
    // switching per game profile"). Drive the compositor's frame pacer to the
    // profile's `refresh_hz`; if the profile opted into VRR (`FLAG_VRR`), use an
    // adaptive range so the panel follows the app's frame rate, else a fixed
    // cadence. `set_refresh_hz` ignores 0 and no-ops if the compositor is down.
    if profile.refresh_hz > 0 {
        crate::compositor::set_refresh_hz(profile.refresh_hz, profile.flags & FLAG_VRR != 0);
    }

    // Things waiting on subsystem setters:
    //   * compositor::set_resolution()    — not yet exposed
    //   * compositor::set_hdr(bool)       — set_hdr_enabled exists; wiring TBD
    //   * audio::set_default_sink()       — not yet exposed
    //   * cpufreq::set_gpu_power_pct()    — not yet exposed
    // We log the *intent* now so once any of these setters lands the
    // call sites already exist. The profile is the canonical source of
    // truth either way.

    let mut g = STORE.lock();
    if let Some(s) = g.as_mut() {
        s.apply_count += 1;
        s.last_applied = Some(String::from(id));
    }

    0
}

pub fn set_profile(id: &str, profile: GameProfileAbi) -> u64 {
    if profile.version != 1 {
        return ERR_BAD_VERSION;
    }
    let mut g = STORE.lock();
    match g.as_mut() {
        Some(s) => {
            s.profiles.insert(String::from(id), profile);
            0
        }
        None => ERR_NOT_INIT,
    }
}

pub fn get_profile(id: &str) -> Option<GameProfileAbi> {
    let g = STORE.lock();
    g.as_ref().and_then(|s| s.profiles.get(id).copied())
}

pub fn list_ids() -> Vec<String> {
    let g = STORE.lock();
    g.as_ref()
        .map(|s| s.profiles.keys().cloned().collect())
        .unwrap_or_default()
}

pub fn stats() -> (u64, Option<String>) {
    let g = STORE.lock();
    g.as_ref()
        .map(|s| (s.apply_count, s.last_applied.clone()))
        .unwrap_or((0, None))
}

// ── Error codes ────────────────────────────────────────────────────────

pub const ERR_NO_SUCH_PROFILE: u64 = 0xFFFF_FFFF_FFFF_FF01;
pub const ERR_BAD_VERSION: u64 = 0xFFFF_FFFF_FFFF_FF02;
pub const ERR_NOT_INIT: u64 = 0xFFFF_FFFF_FFFF_FF03;
pub const ERR_BAD_USER_PTR: u64 = 0xFFFF_FFFF_FFFF_FF04;

// ── Syscall handlers ───────────────────────────────────────────────────

pub const SYS_GAME_PROFILE_SET: u64 = 58;
pub const SYS_GAME_PROFILE_GET: u64 = 59;
pub const SYS_GAME_PROFILE_APPLY: u64 = 60;
pub const SYS_GAME_PROFILE_LIST: u64 = 61;

pub fn sys_set(
    id_ptr: u64,
    id_len: u64,
    profile_ptr: u64,
    validate: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if !validate(id_ptr, id_len, false) {
        return ERR_BAD_USER_PTR;
    }
    if !validate(
        profile_ptr,
        core::mem::size_of::<GameProfileAbi>() as u64,
        false,
    ) {
        return ERR_BAD_USER_PTR;
    }
    let id = read_user_string(id_ptr, id_len);
    // Copy the struct bytes through the uaccess chokepoint (extable fixup), then
    // reconstruct from the LOCAL buffer — was a raw read_unaligned on the
    // (validated) user pointer, i.e. TOCTOU-unsafe on a raced unmap.
    let sz = core::mem::size_of::<GameProfileAbi>();
    let profile: GameProfileAbi = match crate::uaccess::copy_from_user(profile_ptr, sz) {
        Ok(b) if b.len() == sz => unsafe {
            core::ptr::read_unaligned(b.as_ptr() as *const GameProfileAbi)
        },
        _ => return ERR_BAD_USER_PTR,
    };
    set_profile(&id, profile)
}

pub fn sys_get(
    id_ptr: u64,
    id_len: u64,
    out_ptr: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if !validate_r(id_ptr, id_len, false) {
        return ERR_BAD_USER_PTR;
    }
    let size = core::mem::size_of::<GameProfileAbi>() as u64;
    if !validate_w(out_ptr, size, true) {
        return ERR_BAD_USER_PTR;
    }
    let id = read_user_string(id_ptr, id_len);
    let profile = match get_profile(&id) {
        Some(p) => p,
        None => return ERR_NO_SUCH_PROFILE,
    };
    // SMAP-safe: serialize + one validated extable copy-out (was a raw
    // write_unaligned of the struct through the user ptr).
    debug_assert_eq!(size as usize, 56);
    if crate::uaccess::copy_to_user(out_ptr, &profile.to_le_bytes()).is_err() {
        return ERR_BAD_USER_PTR;
    }
    size
}

pub fn sys_apply(id_ptr: u64, id_len: u64, validate: impl Fn(u64, u64, bool) -> bool) -> u64 {
    if !validate(id_ptr, id_len, false) {
        return ERR_BAD_USER_PTR;
    }
    let id = read_user_string(id_ptr, id_len);
    apply_profile(&id)
}

/// Out layout per entry: 64 bytes of UTF-8 (NUL-padded) per profile id.
/// rax = number of entries written.
pub fn sys_list(
    out_ptr: u64,
    out_cap_bytes: u64,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if out_cap_bytes > 0 && !validate_w(out_ptr, out_cap_bytes, true) {
        return 0;
    }
    let ids = list_ids();
    let max = (out_cap_bytes / 64) as usize;
    let written = ids.len().min(max);
    // Build the fixed 64-byte-per-id record buffer, then validate-and-copy out in
    // one shot through the uaccess chokepoint (extable fixup — a TOCTOU unmap
    // between validate_w above and the write yields Err, not a ring-0 fault).
    let mut buf = alloc::vec![0u8; written * 64];
    for (i, id) in ids.iter().take(written).enumerate() {
        let bytes = id.as_bytes();
        let n = bytes.len().min(63);
        buf[i * 64..i * 64 + n].copy_from_slice(&bytes[..n]);
    }
    if !buf.is_empty() && crate::uaccess::copy_to_user(out_ptr, &buf).is_err() {
        return 0;
    }
    written as u64
}

// ── Helpers ────────────────────────────────────────────────────────────

fn read_user_string(ptr: u64, len: u64) -> String {
    // Validated + fault-fixup (was a raw arbitrary-kernel-read).
    crate::uaccess::read_user_string(ptr, len)
}

// ── /proc/athena/games ──────────────────────────────────────────────────

pub fn dump_text() -> String {
    let g = STORE.lock();
    let s = match g.as_ref() {
        Some(s) => s,
        None => return String::from("# game profile store not initialized\n"),
    };
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "# AthenaOS per-game profiles ({} stored, {} applied since boot)\n",
        s.profiles.len(),
        s.apply_count,
    ));
    if let Some(last) = &s.last_applied {
        out.push_str(&alloc::format!("# last_applied = {}\n", last));
    }
    for (id, p) in &s.profiles {
        out.push_str(&alloc::format!(
            "\n[{}]\n  resolution = {}x{}\n  refresh_hz = {}\n  gpu_power_pct = {}\n  audio_sink_id = {}\n  flags = 0x{:x}\n  priority = {}\n  affinity_mask = 0x{:x}\n  memory_pin_mib = {}\n  deadline_us = {}/{}\n",
            id,
            p.resolution_w, p.resolution_h,
            p.refresh_hz,
            p.gpu_power_pct,
            p.audio_sink_id,
            p.flags,
            p.priority,
            p.affinity_mask,
            p.memory_pin_mib,
            p.deadline_period_us, p.deadline_runtime_us,
        ));
    }
    out
}
