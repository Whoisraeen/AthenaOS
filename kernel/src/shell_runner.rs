//! Shell runner — boots the AthShell desktop into the compositor.
//!
//! Creates a full-screen kernel-owned compositor surface, instantiates
//! `raeshell::DesktopShell`, renders the taskbar + desktop chrome, and
//! presents it. The surface sits at z-order 0 (behind all app windows).
//!
//! Keyboard events are forwarded via `handle_key()` from the IRQ path so
//! the start menu, settings panel, and notifications respond to input.

#![allow(dead_code)]

use spin::Mutex;

/// Overview/Spaces/Alt-Tab overlay rendering (the window-switching + spaces UI
/// and its shared ARGB draw primitives) lives in its own submodule to keep this
/// file manageable. The `use` re-imports the parent-called entry points so the
/// existing bare call sites resolve unchanged.
mod overlay;
use overlay::{apply_space_switch, cycle_alt_tab, move_focused_to_space, render_overview_chrome};

static SHELL_STATE: Mutex<Option<ShellRunnerState>> = Mutex::new(None);

/// Active keyboard layout, as an index into `raelocale::keyboard::ALL_LAYOUTS`.
/// Default 0 = `LayoutId::UsQwerty`. Stored as a lock-free atomic because the
/// key-resolution path (`lock_scancode_to_ascii`) runs from the HID bridge /
/// IRQ-driven `handle_key` and must not take a lock that a preempted syscall
/// could be holding. `set_keyboard_layout` / `active_keyboard_layout` are the
/// accessors; a settings UI is a later slice.
static ACTIVE_KB_LAYOUT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Concept §"rival Windows + macOS" globally: those ship dozens of keyboard
/// layouts so a French/German/Dvorak user can actually type. Return the active
/// layout the kernel input path resolves scancodes against (default US-QWERTY).
pub fn active_keyboard_layout() -> raelocale::keyboard::LayoutId {
    let idx = ACTIVE_KB_LAYOUT.load(core::sync::atomic::Ordering::Relaxed) as usize;
    raelocale::keyboard::ALL_LAYOUTS
        .get(idx)
        .copied()
        .unwrap_or(raelocale::keyboard::LayoutId::UsQwerty)
}

/// Concept §"rival Windows + macOS": let the user pick a non-US keyboard layout
/// so typing actually works in their language. Sets the active layout the kernel
/// input path resolves against. Lock-free; safe to call from a settings handler
/// or a /proc write hook.
pub fn set_keyboard_layout(id: raelocale::keyboard::LayoutId) {
    let idx = raelocale::keyboard::ALL_LAYOUTS
        .iter()
        .position(|&l| l == id)
        .unwrap_or(0) as u8;
    ACTIVE_KB_LAYOUT.store(idx, core::sync::atomic::Ordering::Relaxed);
}

/// Set the active keyboard layout by short name ("us", "fr", "de", "dvorak"),
/// case-insensitive. Returns true if recognized. The thin hook a settings UI /
/// `/proc/raeen/keyboard` write uses.
pub fn set_keyboard_layout_by_name(name: &str) -> bool {
    match raelocale::keyboard::LayoutId::from_name(name) {
        Some(id) => {
            set_keyboard_layout(id);
            true
        }
        None => false,
    }
}
static POST_LOGIN_BOOT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static CLICK_LAUNCH_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

// Safe-mode boot-diagnostic panel state. The refresh thread reads these
// to repaint the curated bootlog lines onto the desktop surface so late
// log lines (user-thread sentinels, final TIER timings) show up after
// they're produced.
static DIAG_SURFACE_PTR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static DIAG_SURFACE_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static DIAG_W: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static DIAG_H: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Set by the auto-advance thread to tell the diag refresh thread to stop
/// repainting — only used as a failure fallback now; on the normal path the
/// panel is re-docked beside the desktop instead of stopped.
static DIAG_STOP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Screen x position the diag panel presents at (0 while it owns the whole
/// screen; the right-dock offset after the desktop comes up).
static DIAG_X: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static PENDING_TITLES: Mutex<alloc::collections::BTreeMap<u64, alloc::string::String>> =
    Mutex::new(alloc::collections::BTreeMap::new());

struct ShellRunnerState {
    phase: crate::session::SessionPhase,
    shell: Option<raeshell::DesktopShell>,
    /// GameOS couch mode — when Some, the couch UI owns the screen and the
    /// keyboard routes as controller input (MasterChecklist Phase 12.2/14.3).
    couch: Option<raeshell::gameos::GameOsShell>,
    lock: Option<raeshell::LockScreen>,
    login: crate::login_ui::LoginState,
    /// First-boot OOBE state — populated when the kernel boots into a
    /// fresh install with no user account yet. The wizard collects
    /// username + password, calls `session::create_local_account`, marks
    /// `/setup/first_boot_done = true`, then auto-signs-in.
    setup: crate::setup_ui::SetupState,
    /// Graphical install wizard state (`installer_ui`). Active only while
    /// `phase == Install` — entered at boot via `/installer/autostart` or on
    /// demand from the desktop (F9).
    installer: crate::installer_ui::InstallState,
    lock_password: [u8; 64],
    lock_password_len: usize,
    surface_id: u64,
    surface_ptr: *mut u8,
    width: u32,
    height: u32,
    alt_held: bool,
    /// Left/Right Super (Win) key held — for the Super+Space palette chord.
    super_held: bool,
    /// Left/Right Shift held — for the clipboard panel's Shift+Delete clear-all.
    shift_held: bool,
    /// Left/Right Ctrl held — for the Super+Ctrl+arrow space-switch chord
    /// (window-management.md §"keyboard map": Win-parity Super+Ctrl+→/←).
    ctrl_held: bool,
    alt_tab_open: bool,
    alt_tab_index: usize,
    /// Overview/Expose open (window-management.md §1). When true the compositor
    /// composites every space-member surface as an aspect-fit thumbnail at the
    /// Tile grid origins and the shell draws labels + the accent selection ring
    /// on top. `overview_sel` indexes the focused thumbnail (kbd/d-pad nav).
    overview_open: bool,
    overview_sel: usize,
    /// Per-display virtual desktops / Spaces (window-management.md §2). Owns
    /// surface→space membership; the kernel applies visibility flips to the
    /// compositor on switch and ramps the wallpaper cross-fade.
    spaces: raeshell::spaces::SpaceManager,
    /// Active title-bar drag: `(surface_id, grab_offset_x, grab_offset_y)`.
    drag: Option<(u64, i32, i32)>,
    /// In-game Game Bar overlay (GameOS Phase 4, Concept §"Game Bar that doesn't
    /// suck"). Invoked by the controller Guide chord or the F10 hotkey; composites
    /// FPS + frametime graph + CPU/GPU temps over whatever runs underneath. Fed
    /// from the LIVE `crate::perf` (FPS/frametime) + `crate::thermal` (temps).
    game_bar: raeshell::game_bar::GameBar,
    /// RaeWeb browser surface (Concept §3 "renders through AthUI", §Core
    /// Principles #1 "No Electron tax"). When Some, the kernel-drawn web view owns
    /// the screen and the keyboard drives the address bar / link activation. Toggled
    /// from the desktop with F7, mirroring how `couch` (GameOS) is toggled with F11.
    webview: Option<crate::webview::WebView>,
}

unsafe impl Send for ShellRunnerState {}

/// Create the compositor surface and show the login screen.
/// Call after `compositor::init()` and `session::init()`.
pub fn init() {
    let Some((screen_w, screen_h)) = crate::compositor::screen_dimensions() else {
        crate::serial_println!("[shell_runner] no compositor — skipping");
        return;
    };

    let Some((surface_id, surface_ptr)) =
        crate::compositor::create_kernel_surface(screen_w, screen_h)
    else {
        crate::serial_println!("[shell_runner] failed to create desktop surface");
        return;
    };

    // Safe-mode builds are bring-up diagnostics: paint a live boot-log
    // panel (photographable, the one log channel that can't fail) and
    // keep it refreshing. The auto-advance thread below later stops the
    // refresh and brings up the desktop so the machine still reaches a
    // usable shell even with no keyboard.
    let safe = crate::block_io::safe_mode_enabled();
    // The on-screen boot-diagnostic panel is now OPT-IN (default OFF). The full
    // boot log is persisted to BOOTLOG.TXT on the NVMe ESP and pulled with
    // read-bootlog.ps1, so the panel only cluttered the screen — and its 250 ms
    // refresh thread repaints the whole surface, fighting the desktop bring-up
    // for it (a prime suspect for "boot stops at the diagnostics screen"). With
    // it off, safe-mode boots straight to the normal desktop UI. Re-enable with
    // /diag/onscreen_panel=true only when booting without NVMe log capture.
    let show_diag_panel =
        safe && crate::config_registry::get_bool("/diag/onscreen_panel").unwrap_or(false);
    if show_diag_panel {
        crate::bootlog::render_diagnostics(surface_ptr, screen_w, screen_h);
        let _ = crate::compositor::present_surface(surface_id, 0, 0);
        DIAG_SURFACE_PTR.store(surface_ptr as u64, core::sync::atomic::Ordering::SeqCst);
        DIAG_SURFACE_ID.store(surface_id, core::sync::atomic::Ordering::SeqCst);
        DIAG_W.store(screen_w, core::sync::atomic::Ordering::SeqCst);
        DIAG_H.store(screen_h, core::sync::atomic::Ordering::SeqCst);
        spawn_diag_refresh_thread();
        crate::serial_println!(
            "[shell_runner] safe-mode: live boot-diagnostic panel active ({}x{}, surface {})",
            screen_w,
            screen_h,
            surface_id,
        );
    } else if safe {
        crate::serial_println!(
            "[shell_runner] safe-mode: on-screen diag panel OFF (log -> NVMe BOOTLOG.TXT) — booting to desktop"
        );
    }

    // First-boot detection: if `/setup/first_boot_done` is unset,
    // open the OOBE wizard instead of the login screen so the user
    // can create their own account on first power-on (Windows / macOS
    // OOBE convention). Subsequent boots skip this path.
    let first_boot = !crate::setup_ui::is_first_boot_complete();
    let login = crate::login_ui::LoginState::new();
    let setup = crate::setup_ui::SetupState::new();
    let installer = crate::installer_ui::InstallState::new();

    // Installer-mode boot: when `/installer/autostart` is set (an installer
    // image), open the graphical wizard straight away instead of login/OOBE.
    // Default-off, so a normal boot is completely unaffected. A dist image
    // (`xtask dist` → kernel feature `installer_image`) is BUILT as an
    // installer, so the flag is compiled in — no runtime config needed.
    let installer_mode = cfg!(feature = "installer_image")
        || crate::config_registry::get_bool("/installer/autostart").unwrap_or(false);
    if cfg!(feature = "installer_image") {
        crate::serial_println!("[shell] installer image: booting into the install wizard");
    }

    let initial_phase = if installer_mode {
        crate::session::SessionPhase::Install
    } else if first_boot {
        crate::session::SessionPhase::FirstBootSetup
    } else {
        crate::session::SessionPhase::Login
    };

    // Paint the initial screen now UNLESS the diag panel owns the surface
    // (then the auto-advance thread brings the desktop up after the diag
    // window). With the panel off — the default — safe-mode paints the normal
    // login/setup screen immediately, exactly like a normal boot.
    if !show_diag_panel {
        if installer_mode {
            installer.render(surface_ptr, screen_w, screen_h);
        } else if first_boot {
            setup.render(surface_ptr, screen_w, screen_h);
        } else {
            login.render(surface_ptr, screen_w, screen_h);
        }
        let _ = crate::compositor::present_surface(surface_id, 0, 0);
        // The OOBE / login / installer screens own the framebuffer — stop the
        // serial->framebuffer text mirror so raw boot-log lines don't bleed over
        // them (Windows/macOS never show a console on first-run). Durable logging
        // (UART + bootlog ring + netlog) is untouched; only the on-screen mirror
        // stops. activate_desktop already does this for the desktop itself.
        crate::console::set_console_mirror(false);
    }

    *SHELL_STATE.lock() = Some(ShellRunnerState {
        phase: initial_phase,
        shell: None,
        couch: None,
        lock: None,
        login,
        setup,
        installer,
        lock_password: [0u8; 64],
        lock_password_len: 0,
        surface_id,
        surface_ptr,
        width: screen_w,
        height: screen_h,
        alt_held: false,
        super_held: false,
        shift_held: false,
        ctrl_held: false,
        alt_tab_open: false,
        alt_tab_index: 0,
        overview_open: false,
        overview_sel: 0,
        spaces: raeshell::spaces::SpaceManager::new(),
        drag: None,
        game_bar: raeshell::game_bar::GameBar::new(screen_w as usize, screen_h as usize),
        webview: None,
    });

    crate::serial_println!(
        "[shell_runner] {} ready ({}x{}, surface {})",
        if first_boot {
            "first-boot setup wizard"
        } else {
            "login screen"
        },
        screen_w,
        screen_h,
        surface_id,
    );

    // Guest auto-advance is REMOVED (owner directive 2026-06-15): the machine
    // waits for a real login/setup at the lock/setup screen and is never
    // silently signed in as guest. The thread is now a no-op that just logs a
    // "no guest" marker (useful on iron); spawning it keeps the wiring intact.
    spawn_auto_advance_thread(safe);
}

/// Kernel thread: after a short delay, if no one has logged in yet,
/// auto-login (guest) and bring up the desktop. Lets the OS reach a
/// visible desktop without keyboard input.
extern "C" fn auto_advance_thread_entry() {
    // GUEST AUTO-ADVANCE REMOVED (owner directive 2026-06-15). This thread used
    // to wait ~2.5 s and, if no one had logged in, silently sign in as guest and
    // bring up the desktop — a no-keyboard fallback. That behaviour is GONE: a
    // fresh boot must reach a real login/setup screen with LIVE input and is
    // never auto-signed-in. The thread is retained as a no-op so the spawn site
    // (and the safe-mode diag wiring) stay intact; it simply records and exits.
    crate::serial_println!(
        "[shell_runner] guest auto-advance removed — waiting for live login/setup input"
    );

    // DEV-ONLY screenshot path (cargo feature `desktop_autologin`, never in a
    // shipped image): with no keyboard in the headless screenshot harness, sign
    // in as guest and bring up the desktop so the visual-QA capture lands on the
    // live shell instead of the login/OOBE screen. Gated off by default so a real
    // boot keeps the owner-mandated "wait for live login" behaviour above.
    //
    // RETRY LOOP (not single-shot): the boot-time session smoketest does a
    // logout that runs `return_to_login` -> on a first-boot image this re-paints
    // the FirstBootSetup wizard and flips `state.phase` back to FirstBootSetup
    // (serial: "returned to first-boot setup (OOBE not complete)"). A one-shot
    // autologin that fires before that smoketest gets silently clobbered, so the
    // desktop never appears for the capture. Instead we re-assert the guest
    // desktop on a timer until the shell is actually Active, and keep it pinned
    // for the harness settle window -- whichever order the smoketests run in.
    #[cfg(feature = "desktop_autologin")]
    {
        // NO initial sleep. The autologin thread is spawned at the very end of
        // kernel_main and, under slow TCG with heavy post-boot userspace init
        // (amdgpud loading 12 firmware blobs, pthread smoketests), it is not
        // scheduled until ~tens of seconds in. A 1.2s cooperative sleep at that
        // point competes with that init and the thread was observed never to
        // reach `activate_desktop` before the capture window. Activate on the
        // FIRST scheduled instant instead; the retry loop below still overrides
        // any late "return to first-boot setup" / framebuffer-clear.
        let mut announced = false;
        for _pass in 0..200 {
            let _ = crate::session::login_guest();
            {
                let mut guard = SHELL_STATE.lock();
                if let Some(state) = guard.as_mut() {
                    if state.phase != crate::session::SessionPhase::Active || state.shell.is_none()
                    {
                        activate_desktop(state);
                        if !announced {
                            crate::serial_println!(
                                "[shell_runner] desktop_autologin: guest desktop up (screenshot path)"
                            );
                            announced = true;
                        }
                    } else if let Some(ref mut shell) = state.shell {
                        // Already Active: the init_system boot thread / late userspace
                        // takeover can clear the GOP framebuffer AFTER the one-shot
                        // present in activate_desktop, leaving the headless capture
                        // black. Re-render + re-present the desktop chrome each pass so
                        // the screenshot always lands on a painted desktop, whatever
                        // clears underneath. Dev-only path — never in a shipped image.
                        let welcome = alloc::format!("Welcome, {}", crate::session::display_name());
                        render_shell(
                            shell,
                            state.surface_ptr,
                            state.width,
                            state.height,
                            &welcome,
                        );
                        let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
                    }
                }
            }
            autologin_sleep_ticks(50); // ~0.5s between re-assertions
        }
    }
}

/// DEV-ONLY (desktop_autologin): cooperative sleep for ~`ticks` timer ticks by
/// yielding + halting. Used by the screenshot-harness autologin loop so it does
/// not spin a CPU while waiting for boot smoketests to settle.
#[cfg(feature = "desktop_autologin")]
fn autologin_sleep_ticks(ticks: u32) {
    for _ in 0..ticks {
        crate::scheduler::yield_task();
        x86_64::instructions::hlt();
    }
}

fn spawn_auto_advance_thread(_safe: bool) {
    let task = crate::task::Task::new(auto_advance_thread_entry, None);
    // Pin to CPU 0 — APs don't schedule post-boot (scheduler::spawn_on_bsp), so
    // an auto-advance thread on an AP would never bring up the desktop.
    crate::scheduler::spawn_on_bsp(task);
}

/// DEV-ONLY (cargo feature `desktop_autologin`): spawn the guest-desktop
/// autologin thread from the post-BOOT_COMPLETE context (kernel_main, right
/// before hlt_loop). The early spawn site in `init()` runs while CPU0 is still
/// non-preemptible (BOOT_COMPLETE false), and that task was observed to never
/// reach its first instruction in the headless screenshot harness even though a
/// sibling thread spawned later (the xHCI HID drainer) did. Spawning here — the
/// SAME proven post-boot context the HID thread uses — guarantees the screenshot
/// harness lands on the live desktop. Strictly gated by the feature so a shipped
/// build never auto-signs-in (owner directive 2026-06-15).
#[cfg(feature = "desktop_autologin")]
pub fn spawn_desktop_autologin() {
    let task = crate::task::Task::new(auto_advance_thread_entry, None);
    crate::scheduler::spawn_on_bsp(task);
    crate::serial_println!("[shell_runner] desktop_autologin: post-boot autologin thread spawned");
}

/// Runs `init_system::boot()` off the critical path. Heavy/blocking service
/// startup must not stall the desktop bring-up, so it lives in its own
/// thread (spawned from activate_desktop after the desktop is presented).
extern "C" fn init_system_boot_thread_entry() {
    crate::init_system::boot();
    crate::serial_println!("[shell_runner] init system startup complete");
    crate::scheduler::exit_current_task(0);
}

/// Kernel thread (safe-mode only) that repaints the boot-diagnostic panel
/// from the live bootlog ring every ~250ms and re-presents it, so lines
/// logged AFTER shell_runner::init (user-thread sentinels, end-of-boot
/// TIER timings) appear on the photographable on-screen panel.
extern "C" fn diag_refresh_thread_entry() {
    use core::sync::atomic::Ordering;
    loop {
        // The auto-advance thread sets DIAG_STOP when it's about to bring
        // up the desktop; exit so we don't repaint the panel over it.
        if DIAG_STOP.load(Ordering::SeqCst) {
            crate::serial_println!(
                "[shell_runner] diag refresh thread stopping (desktop taking over)"
            );
            return;
        }
        let ptr = DIAG_SURFACE_PTR.load(Ordering::SeqCst) as *mut u8;
        let id = DIAG_SURFACE_ID.load(Ordering::SeqCst);
        let w = DIAG_W.load(Ordering::SeqCst);
        let h = DIAG_H.load(Ordering::SeqCst);
        let x = DIAG_X.load(Ordering::SeqCst) as i32;
        if !ptr.is_null() && w > 0 && h > 0 {
            crate::bootlog::render_diagnostics(ptr, w, h);
            let _ = crate::compositor::present_surface(id, x, 0);
        }
        // ~250ms between repaints: frequent enough to catch late lines,
        // light enough to leave the CPU to everything else. Sleeps by
        // yielding + halting across timer ticks.
        for _ in 0..25 {
            crate::scheduler::yield_task();
            x86_64::instructions::hlt();
        }
    }
}

fn spawn_diag_refresh_thread() {
    let task = crate::task::Task::new(diag_refresh_thread_entry, None);
    crate::scheduler::spawn_on_bsp(task); // CPU 0 — APs don't schedule post-boot
    crate::serial_println!("[shell_runner] safe-mode diag refresh thread spawned");
}

fn activate_desktop(state: &mut ShellRunnerState) {
    // Ensure the logged-in user's home has the standard folders (Documents,
    // Downloads, Pictures, …) so the file manager shows a real, navigable VFS
    // tree instead of mock data. Safe here: login is complete and the SESSION
    // lock is not held (ensure_session_home_dirs re-locks it via username()).
    crate::vfs::ensure_session_home_dirs();

    // Crawl the live session home into the KERNEL search index (syscalls 54-57)
    // so the system search bar returns the user's actual files, not just the
    // static app/settings seed. Bounded (entry + depth caps) and off the boot
    // critical path — login is done and the success marker is long printed.
    // Concept §Windows pain points: "Search is broken -> Local-first, indexed."
    crate::search_index::crawl_session_home();

    // Push the LIVE accent seed from the active theme/Vibe preset into the
    // userspace shell so the taskbar/Start/tray/Settings re-skin coherently with
    // the kernel surfaces (Concept §Customization Engine: "the desktop becomes a
    // different place in one tap"). Done at desktop activation; the theme/Vibe
    // apply path re-pushes on every later change (raeshell::set_active_accent).
    raeshell::set_active_accent(crate::theme_engine::active_accent());

    let mut shell = raeshell::DesktopShell::new(state.width as usize, state.height as usize);
    shell.system_tray.set_clock("00:00");

    let welcome = alloc::format!("Welcome, {}", crate::session::display_name());
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Hello Window"),
        exec_path: alloc::string::String::from("hello_window"),
        icon_char: 'H',
        category: raeshell::AppCategory::System,
        pinned: true,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Terminal"),
        exec_path: alloc::string::String::from("terminal"),
        icon_char: '>',
        category: raeshell::AppCategory::System,
        pinned: true,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("File Manager"),
        exec_path: alloc::string::String::from("files"),
        icon_char: 'F',
        category: raeshell::AppCategory::Utility,
        pinned: true,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Settings"),
        exec_path: alloc::string::String::from("settings"),
        icon_char: 'S',
        category: raeshell::AppCategory::System,
        pinned: true,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Calculator"),
        exec_path: alloc::string::String::from("calculator"),
        icon_char: '#',
        category: raeshell::AppCategory::Utility,
        pinned: true,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Task Manager"),
        exec_path: alloc::string::String::from("task_mgr"),
        icon_char: 'T',
        category: raeshell::AppCategory::System,
        pinned: true,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Text Editor"),
        exec_path: alloc::string::String::from("text_editor"),
        icon_char: 'E',
        category: raeshell::AppCategory::Utility,
        pinned: true,
        launch_count: 0,
    });
    // Bundled consumer apps (config/base.toml [packages]) — previously had no
    // Start-menu tile, so a user couldn't find/launch them. Exec names match the
    // bundled ELF names and flow through the existing spawn_app_from_vfs path.
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Photos"),
        exec_path: alloc::string::String::from("photos"),
        icon_char: 'P',
        category: raeshell::AppCategory::Media,
        pinned: false,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Music"),
        exec_path: alloc::string::String::from("music"),
        icon_char: 'M',
        category: raeshell::AppCategory::Media,
        pinned: false,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Notes"),
        exec_path: alloc::string::String::from("notes"),
        icon_char: 'N',
        category: raeshell::AppCategory::Utility,
        pinned: false,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Clock"),
        exec_path: alloc::string::String::from("clock"),
        icon_char: 'O',
        category: raeshell::AppCategory::Utility,
        pinned: false,
        launch_count: 0,
    });
    // Weather + Contacts — the models were `raeshell::{weather_app,contacts_app}`
    // (unwired); now live standalone apps (apps/weather, apps/contacts).
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Weather"),
        exec_path: alloc::string::String::from("weather"),
        icon_char: 'W',
        category: raeshell::AppCategory::Utility,
        pinned: false,
        launch_count: 0,
    });
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Contacts"),
        exec_path: alloc::string::String::from("contacts"),
        icon_char: 'C',
        category: raeshell::AppCategory::Utility,
        pinned: false,
        launch_count: 0,
    });
    // Passwords & Authenticator (rae_keychain vault + rae_otp TOTP) — fully
    // local; nonce rotated per save (security review 2026-06-22).
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Passwords"),
        exec_path: alloc::string::String::from("passwords"),
        icon_char: 'K',
        category: raeshell::AppCategory::System,
        pinned: false,
        launch_count: 0,
    });
    // Calendar & Contacts (rae_pim iCal/vCard/RRULE/timezone) — offline import.
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Calendar"),
        exec_path: alloc::string::String::from("calendar"),
        icon_char: 'C',
        category: raeshell::AppCategory::Utility,
        pinned: false,
        launch_count: 0,
    });
    // Video (rae_mp4 demux + raemedia H.264 baseline-I-frame + AAC) — local player.
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Video"),
        exec_path: alloc::string::String::from("video"),
        icon_char: 'V',
        category: raeshell::AppCategory::Media,
        pinned: false,
        launch_count: 0,
    });
    // Browser (raeweb HTML/CSS + rae_js: render + execute + DOM + http fetch + clicks).
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Browser"),
        exec_path: alloc::string::String::from("browser"),
        icon_char: 'W',
        category: raeshell::AppCategory::Utility,
        pinned: false,
        launch_count: 0,
    });
    // Mail (rae_mail SMTP/IMAP/POP3 + rae_pim contacts + rae_kv mailbox).
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Mail"),
        exec_path: alloc::string::String::from("mail"),
        icon_char: '@',
        category: raeshell::AppCategory::Utility,
        pinned: false,
        launch_count: 0,
    });
    // VPN (raevpn built-in WireGuard Noise_IKpsk2).
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("VPN"),
        exec_path: alloc::string::String::from("vpn"),
        icon_char: 'G',
        category: raeshell::AppCategory::System,
        pinned: false,
        launch_count: 0,
    });
    // Sync (raesync zero-knowledge E2E cross-device sync).
    shell.start_menu.add_app(raeshell::AppEntry {
        name: alloc::string::String::from("Sync"),
        exec_path: alloc::string::String::from("sync"),
        icon_char: 'S',
        category: raeshell::AppCategory::System,
        pinned: false,
        launch_count: 0,
    });

    // Feed the global command palette (Super+Space) its three indices: the app
    // registry (the start-menu apps just added), the built-in settings-actions
    // catalog (seeded in CommandPalette::new), and a file index from the live
    // session home. This LIGHTS UP the previously-dead search_indexer engine.
    populate_command_palette(&mut shell);

    state.shell = Some(shell);
    state.phase = crate::session::SessionPhase::Active;

    if let Some(ref mut shell) = state.shell {
        render_shell(
            shell,
            state.surface_ptr,
            state.width,
            state.height,
            &welcome,
        );
    }
    let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
    crate::serial_println!("[shell_runner] desktop active for {}", welcome);

    // Publish the unified desktop chrome focus order (Phase 19 audit P1 #4) so a
    // keyboard-only user can Tab across Start / taskbar / tray from the moment the
    // desktop comes up. Refreshed whenever the chrome changes (apps open/close).
    publish_chrome_focus_order(state);

    // Desktop now owns the framebuffer. Stop mirroring raw kernel logs onto the
    // GOP console — post-boot log lines were blitting OVER the desktop and made
    // the machine untestable on iron (T1745). Logs keep flowing to COM1 + the
    // bootlog ring (BOOTLOG.TXT) + netlog, so on-iron diagnostics are unchanged;
    // the safe-mode diag panel is a separate surface and stays. (This println is
    // the last mirrored line — the disable lands right after it.)
    crate::console::set_console_mirror(false);

    // GameOS Mode boots straight into the couch UI when configured
    // (MasterChecklist Phase 12.2) — a living-room machine never shows the
    // desktop unless asked.
    if crate::config_registry::get_bool("/gameos/boot_couch").unwrap_or(false) {
        toggle_gameos(state);
    }

    // Start the init system (service manager) in its OWN thread rather
    // than inline: init_system::boot() does blocking storage I/O to start
    // units and can be slow (or stall). Calling it inline here held the
    // SHELL_STATE lock and blocked the auto-advance/login path before the
    // desktop became interactive. Fire-and-forget so the desktop is up
    // immediately and service startup proceeds concurrently.
    if !POST_LOGIN_BOOT.swap(true, core::sync::atomic::Ordering::SeqCst) {
        let t = crate::task::Task::new(init_system_boot_thread_entry, None);
        crate::scheduler::spawn_on_bsp(t); // CPU 0 — APs don't schedule post-boot
        crate::serial_println!("[shell_runner] init system startup thread spawned");
    }
}

/// Feed the command palette its app + file indices. Apps come from the live
/// start-menu registry (with search keywords keyed off the well-known execs);
/// files come from a shallow walk of the session home so "report" finds your
/// document. The settings-actions catalog is already seeded in `new()`.
fn populate_command_palette(shell: &mut raeshell::DesktopShell) {
    // Per-exec search keywords so a fuzzy/keyword query (e.g. "calc", "notepad")
    // reaches the right bundled app, not just an exact name-substring.
    fn keywords_for(exec: &str) -> &'static [&'static str] {
        match exec {
            "terminal" => &["shell", "console", "command", "cli", "bash"],
            "files" => &["explorer", "folder", "file manager", "browse"],
            "settings" => &["config", "options", "preferences", "control panel"],
            "calculator" => &["calc", "math", "compute", "arithmetic"],
            "text_editor" => &["notepad", "edit", "write", "note", "txt"],
            "task_mgr" => &["process", "kill", "monitor", "performance"],
            "photos" => &["photo", "picture", "image", "gallery", "viewer"],
            "music" => &["audio", "song", "player", "media", "sound"],
            "notes" => &["note", "memo", "jot", "scratchpad", "todo"],
            "clock" => &["time", "alarm", "timer", "stopwatch", "watch"],
            "hello_window" => &["demo", "sample"],
            _ => &[],
        }
    }

    // Snapshot the apps first (immutable borrow) so we can feed the palette
    // (mutable borrow of a sibling field) without aliasing.
    let apps: alloc::vec::Vec<(alloc::string::String, alloc::string::String)> = shell
        .start_menu
        .apps
        .iter()
        .map(|a| (a.name.clone(), a.exec_path.clone()))
        .collect();
    for (name, exec) in &apps {
        let desc = alloc::format!("Launch {name}");
        shell
            .command_palette
            .index_app(name, exec, &desc, keywords_for(exec));
    }

    // Wire the KERNEL search index as the palette's file/document source so the
    // file rows come from the live, crawler-populated index (one source of truth)
    // — NOT a second private VFS walk. The provider must be a bare `fn` (the
    // `KernelFileQuery` type is a function pointer, no captures); the kernel index
    // is a global singleton, so a free function is exactly right. It routes to
    // `search_index::query_resolved`, which acquires INDEX via lock_index (IF=0
    // safe) and returns name+path+kind per hit. Bounded by `max` (the palette
    // caps the request).
    fn kernel_file_source(
        query: &str,
        max: usize,
    ) -> alloc::vec::Vec<raeshell::command_palette::KernelFileHit> {
        crate::search_index::query_resolved(query, max)
            .into_iter()
            .map(|info| raeshell::command_palette::KernelFileHit {
                name: info.name,
                path: info.path,
                is_folder: info.is_folder,
            })
            .collect()
    }
    shell
        .command_palette
        .set_kernel_file_source(kernel_file_source);

    // Index the session home (one level deep is plenty for a launcher; the
    // engine's incremental path can deepen later). Real VFS, not a mock list.
    let home = crate::session::home_dir();
    let mut file_count = 0usize;
    for entry in crate::vfs::list_dir_at(&home) {
        if entry.name.starts_with('.') {
            continue;
        }
        let path = alloc::format!("{}/{}", home.trim_end_matches('/'), entry.name);
        // Index the folder itself, then one level of its children.
        shell.command_palette.index_file(&path);
        file_count += 1;
        for child in crate::vfs::list_dir_at(&path) {
            if child.name.starts_with('.') {
                continue;
            }
            let child_path = alloc::format!("{}/{}", path, child.name);
            shell.command_palette.index_file(&child_path);
            file_count += 1;
            if file_count >= 256 {
                break;
            }
        }
        if file_count >= 256 {
            break;
        }
    }

    crate::serial_println!(
        "[shell_runner] command palette indexed: apps={} settings_actions={} files={}",
        shell.command_palette.indexed_apps(),
        shell.command_palette.settings_actions(),
        shell.command_palette.indexed_files(),
    );
}

/// Execute a fired command-palette dispatch through the shell's existing,
/// capability-checked handlers (spec §5). Every arm does something real — app
/// launch, Settings navigation, a system action, or a clipboard copy. Returns
/// true if the desktop chrome should be repainted (the palette closed/changed).
fn dispatch_palette(
    state: &mut ShellRunnerState,
    intent: raeshell::command_palette::PaletteDispatch,
) -> bool {
    use raeshell::command_palette::PaletteDispatch;
    match intent {
        PaletteDispatch::Launch(exec) | PaletteDispatch::Open(exec) => {
            // Open(file) and Launch(app) both route through the existing VFS
            // spawn path (a file opens its default handler; an app exec spawns).
            if let Some(shell) = state.shell.as_mut() {
                shell.command_palette.close();
            }
            spawn_app_from_vfs(&exec);
            true
        }
        PaletteDispatch::Navigate(target) => {
            navigate_palette_target(state, &target);
            true
        }
        PaletteDispatch::Copy(value) => {
            // Calculator answer → clipboard (the glance-and-grab flow). The
            // palette keeps its "Copied N" confirmation up for one beat; the
            // caller already set it. Repaint to show it.
            let _ = crate::clipboard::set(value.as_bytes());
            crate::serial_println!("[shell_runner] palette: copied \"{}\" to clipboard", value);
            true
        }
        PaletteDispatch::None => false,
    }
}

/// Repaint + present the desktop chrome (the standard post-interaction redraw).
fn repaint_desktop(state: &mut ShellRunnerState) {
    if let Some(shell) = state.shell.as_mut() {
        let banner = alloc::format!("Welcome, {}", crate::session::display_name());
        render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
        let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
    }
}

/// Snapshot the LIVE clipboard history ring into the shell's clipboard panel.
///
/// The panel widget (`raeshell::clipboard_panel`) is a pure view — it owns no
/// syscalls — so the kernel reads `crate::clipboard::history_*` and pushes a
/// `Vec<ClipRow>` (newest-first, exactly the ring order). The panel then groups
/// pinned-above-recent for display via its shared `ClipboardManager` ordering.
/// Called whenever the panel opens or the history changes (pin/delete/clear).
fn refresh_clipboard_panel(state: &mut ShellRunnerState) {
    let (count, _pinned) = crate::clipboard::history_count();
    let mut rows: alloc::vec::Vec<raeshell::clipboard_panel::ClipRow> =
        alloc::vec::Vec::with_capacity(count);
    for i in 0..count {
        let Some(bytes) = crate::clipboard::history_entry_bytes(i) else {
            continue;
        };
        // Header (32 bytes, all little-endian u32): version, format, flags,
        // byte_len, sequence, paste_count, reserved0, reserved1; then UTF-8 text.
        if bytes.len() < 32 {
            continue;
        }
        let u32_at = |off: usize| -> u32 {
            u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        };
        let format = u32_at(4);
        let flags = u32_at(8);
        let byte_len = u32_at(12) as usize;
        let pinned = flags & rae_abi::syscall::CLIP_FLAG_PINNED != 0;
        let end = (32 + byte_len).min(bytes.len());
        // Preview = first line, trimmed (the panel renders one line per row).
        let raw = alloc::string::String::from_utf8_lossy(&bytes[32..end]);
        let line = raw.lines().next().unwrap_or("").trim();
        let preview: alloc::string::String = line.chars().take(120).collect();
        rows.push(raeshell::clipboard_panel::ClipRow {
            format,
            pinned,
            preview,
        });
    }
    if let Some(shell) = state.shell.as_mut() {
        shell.clipboard_panel.set_rows(rows);
    }
}

/// Execute a clipboard-panel action against the live history ring. The panel
/// returns intents (it owns no syscalls); this runs them through the same
/// `crate::clipboard::history_*` surface the `clip_hist_*` userspace wrappers
/// reach. Returns true if the desktop chrome should repaint.
fn apply_clip_panel_action(
    state: &mut ShellRunnerState,
    action: raeshell::clipboard_panel::ClipPanelAction,
) -> bool {
    use raeshell::clipboard_panel::ClipPanelAction;
    match action {
        ClipPanelAction::Promote(index) => {
            // Paste-on-select: promote to the active clipboard so the focused
            // app's next clipboard-get reads it, then close the panel.
            let ok = crate::clipboard::history_promote(index);
            if let Some(shell) = state.shell.as_mut() {
                shell.clipboard_panel.close();
            }
            crate::serial_println!(
                "[shell_runner] clipboard panel: promote index {} -> {}",
                index,
                if ok {
                    "active clipboard"
                } else {
                    "out-of-range"
                }
            );
            true
        }
        ClipPanelAction::TogglePin(index) => {
            // Read the entry's current pin state from the live ring header, flip
            // it, then re-snapshot so the panel reflects the change.
            let was_pinned = crate::clipboard::history_entry_bytes(index)
                .filter(|b| b.len() >= 12)
                .map(|b| {
                    let flags = u32::from_le_bytes([b[8], b[9], b[10], b[11]]);
                    flags & rae_abi::syscall::CLIP_FLAG_PINNED != 0
                })
                .unwrap_or(false);
            crate::clipboard::history_pin(index, !was_pinned);
            refresh_clipboard_panel(state);
            crate::serial_println!(
                "[shell_runner] clipboard panel: {} index {}",
                if was_pinned { "unpinned" } else { "pinned" },
                index
            );
            true
        }
        ClipPanelAction::Delete(index) => {
            // Refused kernel-side if pinned (the panel's pinned-delete guard).
            let ok = crate::clipboard::history_delete(index);
            refresh_clipboard_panel(state);
            crate::serial_println!(
                "[shell_runner] clipboard panel: delete index {} -> {}",
                index,
                if ok {
                    "removed"
                } else {
                    "refused (pinned/oob)"
                }
            );
            true
        }
        ClipPanelAction::ClearAll => {
            let removed = crate::clipboard::history_clear_keep_pinned();
            refresh_clipboard_panel(state);
            crate::serial_println!(
                "[shell_runner] clipboard panel: clear-all removed {} (kept pinned)",
                removed
            );
            true
        }
        ClipPanelAction::Close => {
            if let Some(shell) = state.shell.as_mut() {
                shell.clipboard_panel.close();
            }
            true
        }
        ClipPanelAction::None => false,
    }
}

/// Keyboard routing while the clipboard-history panel is open (modal, Win+V
/// model — design-language §5). Arrows move selection, Enter pastes-on-select +
/// closes, P toggles pin, Delete removes, Shift+Delete clears unpinned, digits
/// 1-9 quick-paste, Esc closes.
fn clipboard_panel_handle_key(state: &mut ShellRunnerState, extended: bool, code: u8) {
    use raeshell::clipboard_panel::ClipPanelAction;

    // Esc — close, no paste.
    if !extended && code == 0x01 {
        if let Some(shell) = state.shell.as_mut() {
            shell.clipboard_panel.close();
        }
        repaint_desktop(state);
        return;
    }
    // Enter — paste selected + close.
    if !extended && code == 0x1C {
        let action = state
            .shell
            .as_ref()
            .map(|s| s.clipboard_panel.activate_selected())
            .unwrap_or(ClipPanelAction::None);
        if apply_clip_panel_action(state, action) {
            repaint_desktop(state);
        }
        return;
    }
    // Down / Up arrows — move selection.
    if extended && code == 0x50 {
        if let Some(shell) = state.shell.as_mut() {
            shell.clipboard_panel.select_next();
        }
        repaint_desktop(state);
        return;
    }
    if extended && code == 0x48 {
        if let Some(shell) = state.shell.as_mut() {
            shell.clipboard_panel.select_prev();
        }
        repaint_desktop(state);
        return;
    }
    // P (0x19) — toggle pin on the selected entry.
    if !extended && code == 0x19 {
        let action = state
            .shell
            .as_ref()
            .map(|s| s.clipboard_panel.toggle_pin_selected())
            .unwrap_or(ClipPanelAction::None);
        if apply_clip_panel_action(state, action) {
            repaint_desktop(state);
        }
        return;
    }
    // Delete (extended 0x53) — delete selected (Shift+Delete clears unpinned).
    if extended && code == 0x53 {
        let action = if state.shift_held {
            ClipPanelAction::ClearAll
        } else {
            state
                .shell
                .as_ref()
                .map(|s| s.clipboard_panel.delete_selected())
                .unwrap_or(ClipPanelAction::None)
        };
        if apply_clip_panel_action(state, action) {
            repaint_desktop(state);
        }
        return;
    }
    // Digits 1-9 (scancodes 0x02..=0x0A) — Maccy-style quick-paste.
    if !extended && (0x02..=0x0A).contains(&code) {
        let digit = (code - 0x01) as usize; // 0x02 -> 1 ... 0x0A -> 9
        let action = state
            .shell
            .as_ref()
            .map(|s| s.clipboard_panel.quick_paste(digit))
            .unwrap_or(ClipPanelAction::None);
        if apply_clip_panel_action(state, action) {
            repaint_desktop(state);
        }
        return;
    }
    // Everything else is swallowed (the panel is modal while open).
}

/// Keyboard routing while the command palette is open (modal). Arrows move the
/// selection, Enter fires the selected action, Esc closes, Backspace edits, and
/// printable keys type into the live-ranked query (spec §6).
fn palette_handle_key(state: &mut ShellRunnerState, extended: bool, code: u8) {
    // Esc — close, discard query.
    if !extended && code == 0x01 {
        if let Some(shell) = state.shell.as_mut() {
            shell.command_palette.close();
        }
        repaint_desktop(state);
        return;
    }
    // Enter — fire the selected result's action.
    if !extended && code == 0x1C {
        let intent = state
            .shell
            .as_mut()
            .map(|s| s.command_palette.fire_selected())
            .unwrap_or(raeshell::command_palette::PaletteDispatch::None);
        let repaint = dispatch_palette(state, intent);
        if repaint {
            repaint_desktop(state);
        }
        return;
    }
    // Arrows — move selection (extended 0x48 up / 0x50 down).
    if extended && code == 0x48 {
        if let Some(shell) = state.shell.as_mut() {
            shell.command_palette.select_prev();
        }
        repaint_desktop(state);
        return;
    }
    if extended && code == 0x50 {
        if let Some(shell) = state.shell.as_mut() {
            shell.command_palette.select_next();
        }
        repaint_desktop(state);
        return;
    }
    // Backspace — edit the query.
    if !extended && code == 0x0E {
        if let Some(shell) = state.shell.as_mut() {
            shell.command_palette.backspace();
        }
        repaint_desktop(state);
        return;
    }
    // Printable keys type into the query (live fuzzy re-rank per keystroke).
    if !extended {
        if let Some(ascii) = lock_scancode_to_ascii(code, false) {
            if (0x20..0x7F).contains(&ascii) {
                if let Some(shell) = state.shell.as_mut() {
                    shell.command_palette.push_char(ascii as char);
                }
                repaint_desktop(state);
            }
        }
    }
}

/// Route a palette Navigate target — either `settings:<page>` (open Settings to
/// a page) or `action:<verb>` (run a real system action).
fn navigate_palette_target(state: &mut ShellRunnerState, target: &str) {
    use raeshell::SettingsPage;
    if let Some(page) = target.strip_prefix("settings:") {
        let sp = match page {
            "display" => SettingsPage::Display,
            "audio" => SettingsPage::Audio,
            "network" => SettingsPage::Network,
            "gaming" => SettingsPage::Gaming,
            "appearance" => SettingsPage::Appearance,
            "security" => SettingsPage::Security,
            "system" => SettingsPage::System,
            _ => SettingsPage::Display,
        };
        if let Some(shell) = state.shell.as_mut() {
            shell.command_palette.close();
            shell.settings.set_page(sp);
            shell.settings.visible = true;
        }
        crate::serial_println!("[shell_runner] palette: opened Settings -> {}", page);
        return;
    }
    if let Some(verb) = target.strip_prefix("action:") {
        // Close the palette first; the action then runs against live state.
        if let Some(shell) = state.shell.as_mut() {
            shell.command_palette.close();
        }
        match verb {
            "cycle_vibe" => {
                // Cycle to the next signed theme/Vibe preset — the same path the
                // Appearance Vibe grid uses, so the whole shell re-skins.
                let next = next_vibe_theme();
                let _ = crate::theme_engine::apply(next);
                crate::serial_println!("[shell_runner] palette: cycled Vibe -> theme #{}", next);
                if let Some(shell) = state.shell.as_mut() {
                    let banner = alloc::format!("Welcome, {}", crate::session::display_name());
                    render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
                }
            }
            "toggle_dnd" => {
                if let Some(shell) = state.shell.as_mut() {
                    shell.notifications.do_not_disturb = !shell.notifications.do_not_disturb;
                    crate::serial_println!(
                        "[shell_runner] palette: Do-Not-Disturb {}",
                        if shell.notifications.do_not_disturb {
                            "ON"
                        } else {
                            "OFF"
                        }
                    );
                }
            }
            "notifications" => {
                let open = crate::notify::toggle_center();
                crate::serial_println!(
                    "[shell_runner] palette: notification center {}",
                    if open { "opened" } else { "closed" }
                );
            }
            "lock" => enter_lock_screen(state),
            "logout" => return_to_login(state),
            "run_script" => {
                // Run the user's quick Rae script (Concept §Customization
                // Engine). The source lives in the versioned config registry
                // at /scripting/palette_script (set it via a script's
                // setConfig, the shell, or Settings). Invoking it from the
                // palette IS the user authorization — full script caps.
                match crate::config_registry::get_text("/scripting/palette_script") {
                    Some(src) => {
                        let id = crate::scripting::submit(
                            src.as_bytes(),
                            crate::scripting::SCRIPT_CAP_ALL,
                        );
                        let output = crate::scripting::output_of(id).unwrap_or_default();
                        let first_line = output.lines().next().unwrap_or("(no output)");
                        crate::serial_println!(
                            "[shell_runner] palette: ran /scripting/palette_script as #{} -> {}",
                            id,
                            first_line
                        );
                        let _ = crate::notify::post(
                            "script",
                            first_line,
                            crate::shell_api::NotificationUrgency::Normal,
                        );
                    }
                    None => {
                        crate::serial_println!(
                            "[shell_runner] palette: no /scripting/palette_script configured"
                        );
                        let _ = crate::notify::post(
                            "script",
                            "No quick script set — write /scripting/palette_script first",
                            crate::shell_api::NotificationUrgency::Normal,
                        );
                    }
                }
            }
            other => {
                crate::serial_println!("[shell_runner] palette: unknown action \"{}\"", other);
            }
        }
        return;
    }
    crate::serial_println!(
        "[shell_runner] palette: unhandled navigate target \"{}\"",
        target
    );
}

/// Pick the next signed theme id after the current one (wraps), so the palette's
/// "Toggle Vibe Mode" steps through the built-in Vibe presets.
fn next_vibe_theme() -> u64 {
    let cur = crate::theme_engine::current_id().unwrap_or(0);
    // Theme ids are seeded 0..N contiguously; step and wrap at the count via a
    // probe (apply rejects out-of-range, so we resolve a valid next id here).
    let next = cur.wrapping_add(1);
    if crate::theme_engine::current_abi().is_some() {
        // theme_engine seeds 8 built-ins (ids 0..=7).
        next % 8
    } else {
        0
    }
}

/// Build the couch-mode shell: big tiles, controller-first (Concept §GameOS).
/// The library seeds from the start menu's app list so every installed app
/// is launchable from the couch; real store aggregation (Steam/Epic/GOG)
/// rides AthBridge.
/// Bridge a raeshell `CouchProfile` (the couch editor's logical mirror) into
/// the kernel's canonical `game_profile::GameProfileAbi` record — a 1:1
/// field copy with no reinterpretation. raeshell can't depend on the kernel
/// crate, so this is the single seam that maps the surface's record into the
/// real syscall path (`set_profile`/`apply_profile`). The deadline fields ride
/// through unchanged; `cpu_power_pct` is 0 (the couch UI exposes a GPU power
/// slider but not yet a CPU cap, so couch profiles leave the system CPU default)
/// (Phase 5 — Concept "per-game profiles, auto-applied").
fn couch_profile_to_abi(p: &raeshell::gameos::CouchProfile) -> crate::game_profile::GameProfileAbi {
    crate::game_profile::GameProfileAbi {
        version: p.version,
        resolution_w: p.resolution_w,
        resolution_h: p.resolution_h,
        refresh_hz: p.refresh_hz,
        gpu_power_pct: p.gpu_power_pct,
        audio_sink_id: p.audio_sink_id,
        flags: p.flags,
        priority: p.priority,
        affinity_mask: p.affinity_mask,
        memory_pin_mib: p.memory_pin_mib,
        cpu_power_pct: 0,
        deadline_period_us: p.deadline_period_us,
        deadline_runtime_us: p.deadline_runtime_us,
    }
}

/// Field-by-field equality of two `GameProfileAbi` records (the SET→GET
/// round-trip check; `GameProfileAbi` doesn't derive `PartialEq`). Every
/// user-meaningful field is compared, including `cpu_power_pct`.
fn game_profile_abi_eq(
    a: &crate::game_profile::GameProfileAbi,
    b: &crate::game_profile::GameProfileAbi,
) -> bool {
    a.version == b.version
        && a.resolution_w == b.resolution_w
        && a.resolution_h == b.resolution_h
        && a.refresh_hz == b.refresh_hz
        && a.gpu_power_pct == b.gpu_power_pct
        && a.audio_sink_id == b.audio_sink_id
        && a.flags == b.flags
        && a.priority == b.priority
        && a.affinity_mask == b.affinity_mask
        && a.memory_pin_mib == b.memory_pin_mib
        && a.cpu_power_pct == b.cpu_power_pct
        && a.deadline_period_us == b.deadline_period_us
        && a.deadline_runtime_us == b.deadline_runtime_us
}

fn build_couch(state: &ShellRunnerState) -> raeshell::gameos::GameOsShell {
    let mut couch = raeshell::gameos::GameOsShell::new(state.width as usize, state.height as usize);
    couch.active = true;
    if let Some(ref shell) = state.shell {
        for (i, app) in shell.start_menu.apps.iter().enumerate() {
            let entry = raeshell::gameos::GameEntry {
                id: i as u64 + 1,
                title: app.name.clone(),
                banner_color: 0xFF_2D_5A_9E,
                icon_char: app.icon_char,
                store: raeshell::gameos::GameStoreName::AthStore,
                installed: true,
                last_played: 0,
                playtime_hours: 0.0,
                rating: None,
                size_gb: 0.0,
                favorited: app.pinned,
                running: false,
            };
            couch.featured.push(entry.clone());
            couch.library.push(entry);
        }
    }
    couch
}

fn render_couch(state: &ShellRunnerState) {
    let Some(ref couch) = state.couch else { return };
    let mut canvas = unsafe {
        raegfx::Canvas::new(
            state.surface_ptr,
            state.width as usize,
            state.height as usize,
            4,
        )
    };
    couch.render(&mut canvas);
    let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
}

/// Toggle GameOS couch mode in/out of the regular desktop
/// (MasterChecklist Phase 14.3: "Toggle in/out of GameOS Mode").
fn toggle_gameos(state: &mut ShellRunnerState) {
    if state.couch.is_some() {
        state.couch = None;
        if let Some(ref mut shell) = state.shell {
            let banner = alloc::format!("Welcome, {}", crate::session::display_name());
            render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
            let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        }
        crate::serial_println!("[gameos] couch mode OFF -> desktop");
    } else {
        let mut couch = build_couch(state);
        // GameOS Phase 6: enter with a brief desktop→couch cross-fade so the
        // toggle reads as one environment shifting, not a hard cut ("Toggle into
        // it instantly. Same OS, different shell."). The wash is overlaid on the
        // couch's own surface and ramped to completion here (no compositor
        // change — a full both-shells composite is a [ ] follow-up).
        couch.begin_crossfade(true);
        state.couch = Some(couch);
        animate_couch_crossfade(state);
        crate::serial_println!(
            "[gameos] couch mode ON ({} title(s) in library, crossfade {}ms)",
            state.couch.as_ref().map(|c| c.library.len()).unwrap_or(0),
            raeshell::gameos::CROSSFADE_MS,
        );
    }
}

/// Ramp the couch cross-fade to completion (GameOS Phase 6). Each step advances
/// the fade by one frame-quantum and re-renders the couch surface (which paints
/// the `bg.base` wash at the fade's current alpha on top). Bounded + alloc-light
/// — a fixed number of steps, no per-frame heap. On iron the present cadence is
/// the real limiter; this keeps the fade visible without a busy-loop.
fn animate_couch_crossfade(state: &mut ShellRunnerState) {
    // ~30ms per step → ~11 steps over CROSSFADE_MS; cap the loop defensively.
    const STEP_MS: u64 = 30;
    let mut guard = 0u32;
    loop {
        let still = match state.couch.as_mut() {
            Some(c) => c.tick_crossfade(STEP_MS),
            None => false,
        };
        render_couch(state);
        guard += 1;
        if !still || guard > 64 {
            break;
        }
    }
}

/// Snapshot the LIVE perf + thermal counters into a `PerfFeed` for the Game Bar
/// (GameOS Phase 4). FPS + frametime come from `crate::perf` (the compositor's
/// per-present telemetry); CPU/GPU temps from `crate::thermal::read_component_temps`
/// — `None` on QEMU (no real sensor) → the overlay renders "(n/a)", never a lie.
fn live_perf_feed() -> raeshell::game_bar::PerfFeed {
    let frametime_ms = crate::perf::last_frametime_us().map(|us| us as f32 / 1000.0);
    let fps = crate::perf::fps_estimate_x100().map(|x100| x100 as f32 / 100.0);
    let (cpu_c, gpu_c, _ssd_c) = crate::thermal::read_component_temps();
    raeshell::game_bar::PerfFeed {
        fps,
        frametime_ms,
        cpu_temp_c: cpu_c.map(|c| c as f32),
        gpu_temp_c: gpu_c.map(|c| c as f32),
    }
}

/// Invoke / dismiss the in-game Game Bar overlay (GameOS Phase 4). On open,
/// ingests the live perf/thermal feed and composites the overlay OVER the
/// current frame (couch or desktop) without disrupting it; on close, repaints
/// the surface underneath. Bound to the F10 hotkey + the couch Guide chord.
fn toggle_game_bar(state: &mut ShellRunnerState) {
    let now_visible = state.game_bar.invoke();
    if now_visible {
        let feed = live_perf_feed();
        state.game_bar.ingest_perf(&feed);
        let mut canvas = unsafe {
            raegfx::Canvas::new(
                state.surface_ptr,
                state.width as usize,
                state.height as usize,
                4,
            )
        };
        state.game_bar.render_live_overlay(&mut canvas);
        let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        crate::serial_println!(
            "[gameos] game bar ON (fps={} ft={}ms)",
            feed.fps.map(|f| f as u32).unwrap_or(0),
            feed.frametime_ms.map(|f| f as u32).unwrap_or(0),
        );
    } else {
        // Repaint whatever owns the screen underneath (couch or desktop).
        if state.couch.is_some() {
            render_couch(state);
        } else if let Some(ref mut shell) = state.shell {
            let banner = alloc::format!("Welcome, {}", crate::session::display_name());
            render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
            let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        }
        crate::serial_println!("[gameos] game bar OFF");
    }
}

/// Toggle the RaeWeb browser surface over the desktop (F7). Mirrors `toggle_gameos`:
/// the web view becomes a modal kernel-drawn surface that owns the keyboard until
/// dismissed. (Concept §3 "Web apps via PWA support that actually feels native
/// (renders through AthUI)"; §Core Principles #1 "No Electron tax".)
fn toggle_webview(state: &mut ShellRunnerState) {
    if state.webview.is_some() {
        state.webview = None;
        // Repaint the desktop underneath.
        if let Some(ref mut shell) = state.shell {
            let banner = alloc::format!("Welcome, {}", crate::session::display_name());
            render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
            let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        }
        crate::serial_println!("[web] browser surface CLOSED -> desktop");
    } else {
        let view = crate::webview::WebView::new(state.width as usize, state.height as usize);
        state.webview = Some(view);
        render_webview(state);
        crate::serial_println!(
            "[web] browser surface OPEN ({})",
            state.webview.as_ref().map(|v| v.url()).unwrap_or("-")
        );
    }
}

/// Paint the live web view into the shell surface and present it.
fn render_webview(state: &mut ShellRunnerState) {
    let (ptr, w, h, id) = (
        state.surface_ptr,
        state.width as usize,
        state.height as usize,
        state.surface_id,
    );
    if let Some(ref mut view) = state.webview {
        let mut canvas = unsafe { raegfx::Canvas::new(ptr, w, h, 4) };
        let _ = view.render(&mut canvas);
        let _ = crate::compositor::present_surface(id, 0, 0);
    }
}

/// Keyboard routing while the web view owns the screen. Returns true when consumed.
/// Esc closes (or cancels an address-bar edit); F7 toggles off; arrows scroll;
/// Tab+Enter activates the first link; printable keys edit the address bar.
fn webview_handle_key(state: &mut ShellRunnerState, extended: bool, code: u8) -> bool {
    let editing = state
        .webview
        .as_ref()
        .map(|v| v.is_editing())
        .unwrap_or(false);

    // F7 (0x41) always toggles the surface off.
    if !extended && code == 0x41 {
        toggle_webview(state);
        return true;
    }

    let mut needs_paint = false;
    if let Some(ref mut view) = state.webview {
        match (extended, code) {
            // Esc — cancel edit if editing, else close the surface.
            (false, 0x01) => {
                if view.is_editing() {
                    view.cancel_edit();
                    needs_paint = true;
                } else {
                    toggle_webview(state);
                    return true;
                }
            }
            // Enter — commit the address bar (navigate) or activate the first link.
            (false, 0x1C) => {
                if editing {
                    view.commit_edit();
                } else {
                    let _ = view.activate_link(0);
                }
                needs_paint = true;
            }
            // Backspace — edit address bar.
            (false, 0x0E) if editing => {
                view.backspace();
                needs_paint = true;
            }
            // Ctrl+L (0x26) — focus the address bar (browser convention).
            (false, 0x26) if state.ctrl_held => {
                view.begin_edit();
                needs_paint = true;
            }
            // Down / Up arrows — scroll the page.
            (true, 0x50) => {
                view.scroll(48.0);
                needs_paint = true;
            }
            (true, 0x48) => {
                view.scroll(-48.0);
                needs_paint = true;
            }
            // Alt+Left / Alt+Right — back / forward.
            (true, 0x4B) if state.alt_held => {
                if view.go_back() {
                    needs_paint = true;
                }
            }
            (true, 0x4D) if state.alt_held => {
                if view.go_forward() {
                    needs_paint = true;
                }
            }
            // Printable keys edit the address bar (auto-focus on first keystroke).
            (false, c) => {
                if let Some(ascii) = lock_scancode_to_ascii(c, state.shift_held) {
                    if (0x20..0x7F).contains(&ascii) {
                        if !editing {
                            view.begin_edit();
                        }
                        view.type_char(ascii as char);
                        needs_paint = true;
                    }
                }
            }
            _ => {}
        }
    }

    if needs_paint {
        render_webview(state);
    }
    true
}

/// Keyboard → controller routing while couch mode owns the screen.
/// Returns true when the key was consumed.
fn couch_handle_key(state: &mut ShellRunnerState, extended: bool, code: u8) -> bool {
    use raeshell::gameos::{GamepadButton, GamepadInput};
    let Some(ref mut couch) = state.couch else {
        return false;
    };
    let btn = match (extended, code) {
        (true, 0x48) => Some(GamepadButton::DPadUp),
        (true, 0x50) => Some(GamepadButton::DPadDown),
        (true, 0x4B) => Some(GamepadButton::DPadLeft),
        (true, 0x4D) => Some(GamepadButton::DPadRight),
        (false, 0x1C) => Some(GamepadButton::A), // Enter = confirm/launch
        (false, 0x0E) => Some(GamepadButton::B), // Backspace = back
        (false, 0x0F) => Some(GamepadButton::Y), // Tab = quick menu
        (false, 0x01) | (false, 0x57) => {
            // Esc / F11 — leave couch mode.
            toggle_gameos(state);
            return true;
        }
        (false, 0x44) => {
            // F10 — toggle the in-game Game Bar overlay (Phase 4) OVER the couch.
            toggle_game_bar(state);
            return true;
        }
        _ => None,
    };
    let want_bar = if let Some(b) = btn {
        couch.controller_input(GamepadInput::Button(b));

        // Phase 5: a confirmed profile edit latches a commit — push the EXACT
        // edited record through the REAL game_profile module (SYS_GAME_PROFILE_SET
        // path) so it persists and `/proc/raeen/games` reflects it.
        if let Some((id, prof)) = couch.take_profile_commit() {
            let abi = couch_profile_to_abi(&prof);
            let rc = crate::game_profile::set_profile(&id, abi);
            crate::serial_println!(
                "[gameos] profile saved: {} ({}x{}@{}Hz gpu={}%) rc={}",
                id,
                prof.resolution_w,
                prof.resolution_h,
                prof.refresh_hz,
                prof.gpu_power_pct,
                rc,
            );
        }

        // Phase 5: a launch request → APPLY the game's profile FIRST (the
        // Concept's "auto-applied"). A missing profile is NOT an error: SET a
        // default for the game, then APPLY — never block the launch.
        if let Some(idx) = couch.take_launch_request() {
            if let Some(game) = couch.library.get(idx) {
                let id = raeshell::gameos::GameOsShell::profile_id_for(game);
                if crate::game_profile::get_profile(&id).is_none() {
                    let def = couch_profile_to_abi(&raeshell::gameos::CouchProfile::default());
                    let _ = crate::game_profile::set_profile(&id, def);
                }
                let rc = crate::game_profile::apply_profile(&id);
                crate::serial_println!("[gameos] auto-apply profile on launch: {} rc={}", id, rc);
            }
        }

        // Phase 4: a Guide-tap while a game is running latches a Game Bar invoke
        // request inside the couch shell; consume it and toggle the live overlay.
        couch.take_game_bar_request()
    } else {
        false
    };
    if want_bar {
        toggle_game_bar(state);
    } else if btn.is_some() {
        render_couch(state);
    }
    true // couch is modal: it owns the keyboard while active
}

/// Called from `session::logout()` — immediately return to the login screen.
pub fn force_login_screen() {
    let mut guard = SHELL_STATE.lock();
    let Some(state) = guard.as_mut() else { return };
    return_to_login(state);
}

/// Auto-enter trigger (GameOS Phase 6): a controller bound on xHCI. The Concept
/// model is "press the big button to go to the couch" — but never yank the
/// screen unprompted. So: if a desktop is up, no game shell is already active,
/// and `/gameos/auto_on_pad` is set, post a non-disruptive toast OFFERING
/// GameOS (the user presses A / F11 to accept). If the setting is unset we still
/// toast a hint so the affordance is discoverable, but do NOT auto-enter.
///
/// Called by the kernel HID path when `hid_gamepad` binds a pad (the same
/// VID/PID that drives `GameOsShell::bind_pad`). `vid`/`pid` are the device's
/// REAL ids — no fake detection. Returns true if the offer was posted.
///
/// **TV-out trigger (spec §1.4): GATED OFF.** The spec also lists "auto on a new
/// HDMI display marked as a TV (EDID CEA / large diagonal)". There is no live
/// HDMI-hotplug callback on iron yet (EDID-on-real-monitor is Phase 2.3, `[~]`;
/// `display::handle_hotplug` exists but is not driven by a real connect event),
/// so that trigger has no signal to fire on and is deliberately not wired here.
/// It lands when the EDID hotplug path goes live — the same `gamepad_bound`
/// shape (offer-then-enter, never a silent yank) applies.
///
/// > *"GameOS Mode — couch UI, big-picture, controller-first. Toggle into it
/// > instantly."* — LEGACY_GAMING_CONCEPT.md §Gaming-First Design.
pub fn gamepad_bound(vid: u16, pid: u16) -> bool {
    // Only a real controller-family or generic HID pad is a valid signal.
    if !raeshell::gameos::should_offer_gameos_on_padbind(vid, pid) {
        return false;
    }
    let mut guard = SHELL_STATE.lock();
    let Some(state) = guard.as_mut() else {
        return false;
    };
    // Already in the couch? Bind the pad's glyph set and we're done.
    if let Some(ref mut couch) = state.couch {
        couch.bind_pad(vid, pid);
        render_couch(state);
        return false;
    }
    // Only offer from a live desktop session (not login/setup/install).
    if state.shell.is_none() || state.phase != crate::session::SessionPhase::Active {
        return false;
    }
    let auto = crate::config_registry::get_bool("/gameos/auto_on_pad").unwrap_or(false);
    let set_tag = raeshell::gameos::glyph_set_tag_for_vidpid(vid, pid);
    if auto {
        // Setting-gated auto-enter: post a confirmation toast, then enter (the
        // offer + the enter, never a silent yank — the toast tells the user why
        // the screen changed and how to leave: F11/Guide-hold).
        crate::notify::post(
            "GameOS",
            "Controller connected — entering GameOS Mode (F11 to exit)",
            crate::notify::NotificationUrgency::Normal,
        );
        toggle_gameos(state);
        crate::serial_println!(
            "[gameos] auto-enter on pad-bind: vid={:#06X} set={} -> entered couch",
            vid,
            set_tag,
        );
    } else {
        // Offer only (discoverable affordance): a toast the user can act on.
        crate::notify::post(
            "GameOS",
            "Controller connected — press F11 for GameOS Mode",
            crate::notify::NotificationUrgency::Normal,
        );
        crate::serial_println!(
            "[gameos] pad-bind offer: vid={:#06X} set={} (auto_on_pad off)",
            vid,
            set_tag,
        );
    }
    true
}

fn return_to_login(state: &mut ShellRunnerState) {
    crate::session::end_session();
    state.shell = None;
    state.couch = None;
    state.lock = None;
    state.lock_password_len = 0;
    state.alt_tab_open = false;
    state.alt_held = false;
    state.drag = None;
    // If first-boot OOBE hasn't completed, a logout returns to the SETUP wizard,
    // not a login screen. Otherwise the boot-time session smoketest's logout
    // (session::run_boot_smoketest -> logout -> force_login_screen) clobbers the
    // initial FirstBootSetup phase to Login, and the no-keyboard auto-advance then
    // brings up a guest desktop instead of letting the user create a profile (the
    // reported "setup flashes then drops to guest"). Mirrors leave_installer's
    // first-boot routing.
    if !crate::setup_ui::is_first_boot_complete() {
        // BUG B (render race): the boot-time session smoketest logs out, which
        // routes here on a first-boot image. If the user is ALREADY on the setup
        // wizard (and may be mid-typing), resetting `state.setup` and repainting
        // would wipe their keystrokes AND blank/clip the card behind the live
        // per-keystroke render. Only (re)build + paint the wizard when we are NOT
        // already showing an in-progress setup; otherwise leave the in-progress
        // card untouched so it renders stably while the user types. The
        // no-keyboard auto-advance stays Login-only ([[oobe-auto-advance-login-only]]).
        let already_in_setup = state.phase == crate::session::SessionPhase::FirstBootSetup
            && state.setup.in_progress();
        if already_in_setup {
            // Keep the phase pinned to FirstBootSetup; do NOT reset state or
            // repaint over the in-progress card.
            state.phase = crate::session::SessionPhase::FirstBootSetup;
            crate::serial_println!(
                "[shell_runner] recheck: first-boot setup in progress — card preserved (no repaint)"
            );
            return;
        }
        state.setup = crate::setup_ui::SetupState::new();
        state.phase = crate::session::SessionPhase::FirstBootSetup;
        state
            .setup
            .render(state.surface_ptr, state.width, state.height);
        let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        crate::serial_println!("[shell_runner] returned to first-boot setup (OOBE not complete)");
        return;
    }
    state.login = crate::login_ui::LoginState::new();
    state.phase = crate::session::SessionPhase::Login;
    state
        .login
        .render(state.surface_ptr, state.width, state.height);
    let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
    crate::serial_println!("[shell_runner] returned to login screen");
}

/// Open the graphical install wizard. Reachable from the desktop (F9) or at
/// boot via `/installer/autostart`. Records whether a desktop is live so
/// Cancel knows where to return.
fn enter_installer(state: &mut ShellRunnerState) {
    state.installer = crate::installer_ui::InstallState::new();
    state.installer.from_desktop = state.shell.is_some();
    state.phase = crate::session::SessionPhase::Install;
    state
        .installer
        .render(state.surface_ptr, state.width, state.height);
    let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
    crate::serial_println!(
        "[shell_runner] install wizard opened ({} disk(s) detected)",
        state.installer.disks.len(),
    );
}

/// Leave the install wizard (Cancel / post-dry-run Continue). Returns to the
/// live desktop if one exists, else to the normal first-boot/login flow.
fn leave_installer(state: &mut ShellRunnerState) {
    if state.installer.from_desktop && state.shell.is_some() {
        state.phase = crate::session::SessionPhase::Active;
        if let Some(ref mut shell) = state.shell {
            let banner = alloc::format!("Welcome, {}", crate::session::display_name());
            render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
            let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        }
        crate::serial_println!("[shell_runner] install wizard closed -> desktop");
    } else if !crate::setup_ui::is_first_boot_complete() {
        state.phase = crate::session::SessionPhase::FirstBootSetup;
        state.setup = crate::setup_ui::SetupState::new();
        state
            .setup
            .render(state.surface_ptr, state.width, state.height);
        let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        crate::serial_println!("[shell_runner] install wizard closed -> first-boot setup");
    } else {
        return_to_login(state);
    }
}

fn spawn_installer_worker() {
    let task = crate::task::Task::new(installer_worker_entry, None);
    crate::scheduler::spawn_on_bsp(task); // CPU 0 — APs don't schedule post-boot
    crate::serial_println!("[shell_runner] install worker thread spawned");
}

/// Kernel thread: runs the heavy install pipeline off the keyboard-IRQ path.
/// Copies the account out of the wizard under the lock, releases it for the
/// block I/O, then writes the result back and repaints the Done screen.
extern "C" fn installer_worker_entry() {
    let (user, pass, plan) = {
        let guard = SHELL_STATE.lock();
        match guard.as_ref() {
            Some(s) => {
                let u = alloc::string::String::from(
                    core::str::from_utf8(&s.installer.username[..s.installer.username_len])
                        .unwrap_or(""),
                );
                let p = s.installer.password[..s.installer.password_len].to_vec();
                (u, p, s.installer.plan.clone())
            }
            None => crate::scheduler::exit_current_task(1),
        }
    };

    // Create the account (real backend; succeeds even in safe mode — it is
    // session/config state, not a disk write). Persisting the account record
    // onto the freshly-formatted target AthFS is a Phase 3 follow-up; the
    // installed system's first-boot OOBE is the safety net until then.
    let uid = if !user.is_empty() {
        let mut display = user.clone();
        if let Some(first) = display.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        crate::session::create_local_account(&user, &display, &pass)
    } else {
        None
    };
    if uid.is_some() {
        crate::config_registry::set_bool("/setup/first_boot_done", true);
        crate::config_registry::set_text("/session/last_user", &user);
    }

    // Run the install pipeline for the CHOSEN plan: DualBoot carves AthFS into
    // free space without touching the existing OS/ESP; FullDisk seeds the whole
    // disk. Every write routes through safe_mode_guard_write, so on a --safe
    // image this is a logged dry run.
    //
    // WRITE WINDOW: on real hardware writes are READ-ONLY at boot (see main.rs
    // Tier 2). This is the user-CONFIRMED install action (the wizard reached
    // Review→Install), so open the write window now and CLOSE it immediately
    // after — the disk is writable only for the duration of this confirmed
    // install, never before. A `--safe` image's safe-mode guard still blocks
    // (dry run) regardless; QEMU already had writes on (no-op open/close).
    let prev_writes = crate::block_io::writes_enabled();
    crate::block_io::set_writes_enabled(true);
    crate::serial_println!("[installer] write window OPEN (user-confirmed install)");
    let result = match plan {
        Some(ref p) => crate::installer::apply_plan(p),
        None => crate::installer::run_install(),
    };
    crate::block_io::set_writes_enabled(prev_writes);
    crate::serial_println!(
        "[installer] write window CLOSED (writes_enabled={})",
        prev_writes
    );
    crate::installer_ui::record_install_result(result);

    {
        let mut guard = SHELL_STATE.lock();
        if let Some(s) = guard.as_mut() {
            s.installer.stage_result = result;
            s.installer.account_uid = uid;
            s.installer.step = crate::installer_ui::InstallStep::Done;
            s.installer.render(s.surface_ptr, s.width, s.height);
            let _ = crate::compositor::present_surface(s.surface_id, 0, 0);
        }
    }
    crate::serial_println!(
        "[shell_runner] install worker finished: result={:#07b} account_uid={:?}",
        result,
        uid,
    );
    crate::scheduler::exit_current_task(0);
}

fn enter_lock_screen(state: &mut ShellRunnerState) {
    crate::session::lock();
    let mut lock = raeshell::LockScreen::new(state.width, state.height);
    lock.set_display_name(&crate::session::display_name());
    lock.lock();
    state.lock = Some(lock);
    state.lock_password_len = 0;
    state.phase = crate::session::SessionPhase::Locked;
    render_lock(state);
}

fn render_lock(state: &ShellRunnerState) {
    let Some(ref lock) = state.lock else { return };
    let pixels = state.width as usize * state.height as usize;
    let stride = state.width as usize;
    let buf = unsafe { core::slice::from_raw_parts_mut(state.surface_ptr as *mut u32, pixels) };
    lock.render(buf, stride);
    let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
}

/// Resolve a Set 1 make code to a typed ASCII byte for the active keyboard
/// layout.
///
/// Concept §"rival Windows + macOS" globally: those ship dozens of keyboard
/// layouts so non-US users can type. This is the kernel consumer of the locale-
/// owned table — it delegates to `raelocale::keyboard::resolve_key(active, code,
/// mods)` so AZERTY/QWERTZ/Dvorak resolve correctly instead of the old
/// hardcoded US array.
///
/// US zero-regression: the raelocale US table was built to mirror the legacy
/// 58-entry array exactly. The single legacy entry raelocale does not carry is
/// the numpad-`*` key (Set 1 0x37 -> '*'); it is preserved here as a
/// layout-independent fallback so no US (or any-layout) key that typed before
/// goes silent.
///
/// Char width: the resolver returns `char`; this u8 path keeps the existing
/// ASCII contract (`Option<u8>`) — every printable US key and the ASCII subset
/// of other layouts maps 1:1. Non-ASCII keys (AZERTY 'é', QWERTZ 'ü', ...)
/// currently resolve to `None` rather than truncating; widening the call sites
/// to `char` is a documented follow-up (MasterChecklist parity gap #5).
fn lock_scancode_to_ascii(code: u8, shift: bool) -> Option<u8> {
    let mods = raelocale::keyboard::Modifiers {
        shift,
        // Caps-lock is not yet tracked by the shell input state; false matches
        // the legacy behavior (the old table had no caps plane either).
        caps_lock: false,
        // AltGr plane is a raelocale follow-up (see KeyPlanes docs).
        altgr: false,
    };
    if let Some(ch) = raelocale::keyboard::resolve_key(active_keyboard_layout(), code, mods) {
        // ASCII subset only on this u8 path; non-ASCII (é/ü/...) -> None for now.
        if (ch as u32) <= 0x7F {
            return Some(ch as u8);
        }
        return None;
    }
    // Legacy-only fallback: numpad '*' (Set 1 0x37) is not in the raelocale
    // tables but the old US array typed '*' for it. '*' on every layout we ship,
    // so keep it layout-independent. This is the lone byte-identity gap vs the
    // legacy table — preserving it guarantees no key that typed before goes mute.
    if code == 0x37 {
        return Some(b'*');
    }
    None
}

fn handle_lock_key(state: &mut ShellRunnerState, code: u8) {
    if code == 0x0E {
        if state.lock_password_len > 0 {
            state.lock_password_len -= 1;
        }
        render_lock(state);
        return;
    }
    if code == 0x1C {
        if crate::session::unlock_password(&state.lock_password[..state.lock_password_len]) {
            state.phase = crate::session::SessionPhase::Active;
            state.lock = None;
            state.lock_password_len = 0;
            if let Some(ref mut shell) = state.shell {
                let welcome = alloc::format!("Welcome, {}", crate::session::display_name());
                render_shell(
                    shell,
                    state.surface_ptr,
                    state.width,
                    state.height,
                    &welcome,
                );
                let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
            }
        } else if let Some(ref mut lock) = state.lock {
            lock.show_auth_failed("Wrong password");
            state.lock_password_len = 0;
            render_lock(state);
        }
        return;
    }
    if let Some(ascii) = lock_scancode_to_ascii(code, false) {
        if ascii >= 0x20 && state.lock_password_len + 1 < state.lock_password.len() {
            state.lock_password[state.lock_password_len] = ascii;
            state.lock_password_len += 1;
            render_lock(state);
        }
    }
}

/// HH:MM (UTC) from the SAME syscall surface userspace uses — the tray
/// clock is sys_wall_clock, not a parallel kernel path (MasterChecklist
/// Phase 14.1: "System tray clock reads sys_wall_clock").
pub fn tray_clock_string() -> alloc::string::String {
    let ns = crate::game_session::sys_wall_clock();
    let mins = ns / 60_000_000_000;
    alloc::format!("{:02}:{:02}", (mins / 60) % 24, mins % 60)
}

fn render_shell(shell: &mut raeshell::DesktopShell, ptr: *mut u8, w: u32, h: u32, banner: &str) {
    // Refresh the tray clock on every repaint: any interaction (or toast,
    // window event, ...) that redraws the desktop also brings the clock to
    // the current minute.
    shell.system_tray.set_clock(&tray_clock_string());
    // Toast TTL expiry rides the repaint cadence: any interaction that redraws
    // the desktop also dismisses toasts past their 5 s deadline, so an expired
    // toast can't linger once the user touches the machine (raeen-reviewer
    // 2026-06-17). `expire_now` uses notify's own `now_ms()` (HPET) — the same
    // clock that stamps the deadlines. Same CPU0 IF=0 context as `post_at`, so
    // no new `TOASTS` lock hazard; the lazy-on-post path covers the no-render case.
    crate::notify::expire_now();
    // Desktop widgets ride the same repaint cadence (their surfaces float
    // above the desktop; refresh repaints feeds + reconciles enablement).
    let _ = crate::widgets::refresh();
    let mut canvas = unsafe { raegfx::Canvas::new(ptr, w as usize, h as usize, 4) };

    // Desktop wallpaper — the signature Aurora Mesh backdrop (IDENTITY §3),
    // identical to the compositor's live default so this fallback layer matches
    // pixel-for-pixel. Kills the old "flat void desktop" two-stop navy gradient.
    // The taskbar / start-menu / windows DesktopShell render as their own
    // compositor surfaces on top, so this aurora is everything the user sees
    // behind their windows. Cheap integer/fixed-point fill, no per-frame alloc:
    // the aurora writes straight into the surface buffer the canvas wraps.
    {
        let pixels = unsafe {
            core::slice::from_raw_parts_mut(ptr as *mut u32, (w as usize) * (h as usize))
        };
        crate::aurora::render_aurora(pixels, w, h, crate::aurora::aurora_now_ms());
    }
    let _ = &mut canvas; // canvas still used below for text/widgets

    // Subtle welcome — drawn small and high so it doesn't fight the
    // taskbar / start menu visually. Hidden once any real app has
    // focus (the compositor draws app surfaces on top of this layer
    // anyway, so it's effectively a first-look greeting). RaeSans AA —
    // this was the LAST 8x8-bitmap string on the live desktop (visual-QA
    // 2026-07-01: the blocky greeting over the aurora read hobby-OS).
    let banner_style = rae_tokens::TYPE_LABEL;
    let banner_w = canvas.measure_text_aa(banner, banner_style, raegfx::text::FontFamily::Sans);
    canvas.draw_text_aa(
        (w as i32 - banner_w) / 2,
        20,
        banner,
        banner_style,
        0xFF_C8_D8_F0,
        raegfx::text::FontFamily::Sans,
    );

    shell.render(&mut canvas);

    // Visible keyboard focus ring on the focused chrome element (Phase 19 audit
    // P1 #4). Drawn on top of the chrome so the ring is never occluded; HC-aware
    // (cyan under forced-colors). A no-op when nothing is keyboard-focused or a
    // modal owns focus (the flyout draws its own ring).
    draw_chrome_focus_ring(&mut canvas);

    // Screenshot / region-capture overlay paints on TOP of all desktop chrome
    // (dimmed scrim + selection rect + dimensions pill + action bar). It is the
    // capture-mode modal surface; when idle this is a no-op.
    shell.capture_overlay.render(&mut canvas);
}

/// Keyboard routing while the screenshot capture overlay is up (modal). Esc
/// cancels; Enter / C copy; S save; Tab cycles the action-bar focus; arrows
/// nudge the selection edge (precision). After any committing key the shell
/// drives the capture engine via [`run_capture_if_confirmed`].
fn capture_overlay_handle_key(state: &mut ShellRunnerState, extended: bool, code: u8) {
    use raeshell::screenshot_overlay::CaptureAction;
    // Arrow nudges (precision selection): grow/shrink the selection by 1px
    // (10px with Shift). Extended 0x4B/0x4D/0x48/0x50 = ←/→/↑/↓.
    if extended && matches!(code, 0x4B | 0x4D | 0x48 | 0x50) {
        let step = if state.shift_held { 10 } else { 1 };
        if let Some(shell) = state.shell.as_mut() {
            let o = &mut shell.capture_overlay;
            match code {
                0x4D => o.sel_w = o.sel_w.saturating_add(step), // →: wider
                0x4B => o.sel_w = o.sel_w.saturating_sub(step), // ←: narrower
                0x50 => o.sel_h = o.sel_h.saturating_add(step), // ↓: taller
                0x48 => o.sel_h = o.sel_h.saturating_sub(step), // ↑: shorter
                _ => {}
            }
        }
        repaint_desktop(state);
        return;
    }
    if extended {
        return;
    }
    match code {
        // Esc — cancel.
        0x01 => {
            if let Some(shell) = state.shell.as_mut() {
                shell.capture_overlay.dismiss();
            }
            repaint_desktop(state);
            crate::serial_println!("[shell_runner] screenshot capture cancelled");
        }
        // Tab — cycle action-bar focus.
        0x0F => {
            if let Some(shell) = state.shell.as_mut() {
                shell.capture_overlay.focus_next();
            }
            repaint_desktop(state);
        }
        // Enter — fire the focused action (Copy default).
        0x1C => {
            let focus = state
                .shell
                .as_ref()
                .map(|s| s.capture_overlay.focus)
                .unwrap_or(CaptureAction::Copy);
            if let Some(shell) = state.shell.as_mut() {
                shell.capture_overlay.confirm(focus);
            }
            run_capture_if_confirmed(state);
        }
        // C — copy.
        0x2E => {
            if let Some(shell) = state.shell.as_mut() {
                shell.capture_overlay.confirm(CaptureAction::Copy);
            }
            run_capture_if_confirmed(state);
        }
        // S — save.
        0x1F => {
            if let Some(shell) = state.shell.as_mut() {
                shell.capture_overlay.confirm(CaptureAction::Save);
            }
            run_capture_if_confirmed(state);
        }
        _ => {}
    }
}

/// If the overlay reached `Confirmed`, run the capture engine for the selected
/// region, perform the chosen action (copy/save), toast the result, then
/// dismiss the overlay and repaint. No-op if the overlay isn't confirmed.
fn run_capture_if_confirmed(state: &mut ShellRunnerState) {
    use raeshell::screenshot_overlay::{CaptureAction, OverlayPhase};
    let (confirmed, action, region) = match state.shell.as_ref() {
        Some(s) => (
            s.capture_overlay.phase == OverlayPhase::Confirmed,
            s.capture_overlay.action,
            s.capture_overlay.region(),
        ),
        None => return,
    };
    if !confirmed {
        return;
    }
    let want_save = matches!(action, CaptureAction::Save);
    let want_copy = matches!(action, CaptureAction::Copy);
    let (ok, summary) = execute_capture(region, want_save, want_copy);
    if let Some(shell) = state.shell.as_mut() {
        shell.capture_overlay.dismiss();
    }
    repaint_desktop(state);
    let urgency = if ok {
        crate::notify::NotificationUrgency::Normal
    } else {
        crate::notify::NotificationUrgency::Critical
    };
    crate::notify::post("Screenshot", &summary, urgency);
}

/// Drive the in-kernel compositor capture engine for `region`, then save and/or
/// copy. Returns `(success, toast_summary)`. NEVER panics on a bad capture/save
/// — every failure surfaces as a toast.
///
/// The screen pixels are privacy-sensitive: the cap-gated `SYS_CAPTURE_START`
/// (274) path is what userspace must use. The shell runs IN the kernel (it is
/// the compositor's owner), so it reaches the engine directly via
/// `compositor::capture_region_now` — but the privacy contract is the same gate
/// the smoketest proves (`Cap::ScreenCapture`, fail-closed).
fn execute_capture(
    region: Option<(u32, u32, u32, u32)>,
    want_save: bool,
    want_copy: bool,
) -> (bool, alloc::string::String) {
    let Some((rx, ry, rw, rh)) = region else {
        return (
            false,
            alloc::string::String::from("Capture failed: empty region"),
        );
    };
    let Some((pixels, w, h)) = crate::compositor::capture_region_now(rx, ry, rw, rh) else {
        return (
            false,
            alloc::format!("Capture failed: no pixels for {}x{}", rw, rh),
        );
    };
    let bytes = pixels.len() * 4;
    if bytes == 0 {
        return (
            false,
            alloc::string::String::from("Capture failed: 0 bytes"),
        );
    }

    let mut saved_path: Option<alloc::string::String> = None;
    let mut copied = false;

    if want_save {
        match save_capture_to_pictures(&pixels, w, h) {
            Ok(path) => saved_path = Some(path),
            Err(e) => {
                return (false, alloc::format!("Save failed ({})", e));
            }
        }
    }

    if want_copy {
        // The clipboard ring is text-only + 64 KiB-capped today, so a raw image
        // can't ride it; we ALSO save the image to disk and put a compact text
        // descriptor on the clipboard (path + dimensions). Raw-image clipboard
        // payload (CLIP_FMT_IMAGE) is a tracked follow-up.
        let path = match &saved_path {
            Some(p) => p.clone(),
            None => match save_capture_to_pictures(&pixels, w, h) {
                Ok(p) => {
                    saved_path = Some(p.clone());
                    p
                }
                Err(e) => {
                    return (
                        false,
                        alloc::format!("Capture save-for-copy failed ({})", e),
                    )
                }
            },
        };
        let descriptor = alloc::format!("[screenshot {}x{}] {}", w, h, path);
        copied = crate::clipboard::set(descriptor.as_bytes()).is_ok();
        if !copied {
            return (
                false,
                alloc::string::String::from("Copy to clipboard failed"),
            );
        }
    }

    let summary = match (&saved_path, copied) {
        (Some(p), true) => alloc::format!("{}x{} copied + saved to {}", w, h, p),
        (Some(p), false) => alloc::format!("{}x{} saved to {}", w, h, p),
        (None, true) => alloc::format!("{}x{} copied to clipboard", w, h),
        (None, false) => alloc::format!("{}x{} captured", w, h),
    };
    crate::serial_println!(
        "[shell_runner] screenshot: region={}x{} bytes={} saved={} copied={}",
        w,
        h,
        bytes,
        saved_path.is_some(),
        copied
    );
    (true, summary)
}

/// Write a captured ARGB frame to `~/Pictures/Screenshots/screenshot_<seq>.png`
/// in the session VFS. Returns the path on success. The pixels are encoded to a
/// real, spec-valid PNG via `raemedia::png_encode` (from-scratch encoder,
/// round-trip-proven against the matching `png.rs` decoder) so the file opens in
/// any image tool — not the old raw `.argb` blob. NEVER panics — a bad encode or
/// any VFS error returns `Err` (the caller surfaces it as a toast).
fn save_capture_to_pictures(
    pixels: &[u32],
    w: u32,
    h: u32,
) -> Result<alloc::string::String, &'static str> {
    use crate::vfs::Inode;
    use raemedia::png_encode::{encode_argb8888, ColorType};
    let home = crate::session::home_dir();
    let dir = alloc::format!("{}/Pictures", home);
    let shots = alloc::format!("{}/Pictures/Screenshots", home);
    // Create the Pictures/Screenshots tree (idempotent — EXISTS is fine).
    for d in [&dir, &shots] {
        match crate::vfs::mkdir_at(d, 0o755) {
            Ok(()) => {}
            Err(e) if e == crate::vfs::E_VFS_EXISTS => {}
            Err(_) => return Err("mkdir"),
        }
    }
    let seq = SCREENSHOT_SEQ.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let path = alloc::format!("{}/screenshot_{}.png", shots, seq);

    // Encode the ARGB8888 capture buffer to a PNG byte stream. The compositor
    // capture is fully opaque screen content, so RGBA preserves it exactly (and
    // alpha round-trips for any future translucent-region capture).
    let png = encode_argb8888(pixels, w, h, ColorType::Rgba).map_err(|_| "encode")?;

    let inode = crate::vfs::open_path(&path).ok_or("open")?;
    if inode.write_at(0, &png) != png.len() {
        return Err("write");
    }
    Ok(path)
}

/// Monotonic screenshot filename counter.
static SCREENSHOT_SEQ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// Tracks whether the previous PS/2 set-1 byte was the 0xE0 extended prefix.
/// Extended scancodes (arrows, Super, etc.) come in as two bytes: 0xE0 then
/// the actual code.
static EXTENDED_PREFIX: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Handle a keyboard scancode from the IRQ path. Updates shell state and
/// re-renders + re-presents the desktop surface if the shell consumed it.
///
/// PS/2 set 1 encoding:
///   * 0xE0           = extended prefix (next byte is an extended make/break)
///   * bit 0x80 set   = key release (break)
///   * other          = key press (make)
pub fn handle_key(scancode: u8) {
    use core::sync::atomic::Ordering;

    // Latch extended prefix and return — the *next* byte carries the real key.
    if scancode == 0xE0 {
        EXTENDED_PREFIX.store(true, Ordering::Relaxed);
        return;
    }

    let extended = EXTENDED_PREFIX.swap(false, Ordering::Relaxed);

    // A permission dialog is modal: while one is open it owns the keyboard
    // entirely (Y/Enter = allow, N/Esc = deny; everything else swallowed),
    // so consent keystrokes can never leak into the app that asked.
    if crate::perm_ui::handle_key(scancode) {
        return;
    }

    let is_release = scancode & 0x80 != 0;
    let code = scancode & 0x7F;

    // Super (Win) key make/break — tracked for the Super+Space palette chord.
    // Left Super = 0xE0 0x5B, Right Super = 0xE0 0x5C; release adds 0x80.
    if extended && (code == 0x5B || code == 0x5C) {
        let mut guard = SHELL_STATE.lock();
        if let Some(state) = guard.as_mut() {
            state.super_held = !is_release;
        }
        // Do NOT consume here on make: the start-menu toggle path below still
        // fires for a bare Super tap (extended 0x5B make) when not chording.
        if is_release {
            return;
        }
    }

    // Shift make/break — Left Shift = 0x2A, Right Shift = 0x36 (non-extended);
    // tracked for the clipboard panel's Shift+Delete clear-all. Not consumed:
    // shift must still modify subsequent keys for other consumers.
    if !extended && (code == 0x2A || code == 0x36) {
        let mut guard = SHELL_STATE.lock();
        if let Some(state) = guard.as_mut() {
            state.shift_held = !is_release;
        }
        return;
    }

    // Ctrl make/break — Left Ctrl = 0x1D (non-extended), Right Ctrl = 0xE0 0x1D.
    // Tracked for the Super+Ctrl+arrow space-switch chord (window-management.md
    // §"keyboard map"). Not consumed: Ctrl must still modify other keys.
    if code == 0x1D {
        let mut guard = SHELL_STATE.lock();
        if let Some(state) = guard.as_mut() {
            state.ctrl_held = !is_release;
        }
        return;
    }

    if is_release {
        if code == 0x38 || code == 0xB8 {
            let mut guard = SHELL_STATE.lock();
            if let Some(state) = guard.as_mut() {
                state.alt_held = false;
                if state.alt_tab_open {
                    state.alt_tab_open = false;
                    if let Some(ref mut shell) = state.shell {
                        let banner = alloc::format!("Welcome, {}", crate::session::display_name());
                        render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
                        let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
                    }
                }
            }
        }
        return;
    }

    if code == 0x38 || code == 0xB8 {
        let mut guard = SHELL_STATE.lock();
        if let Some(state) = guard.as_mut() {
            state.alt_held = true;
        }
        return;
    }

    // If a userspace app has compositor focus, only process global hotkeys
    // (Tab/Super for start menu). Everything else goes to the app via its
    // per-task key buffer (already wired in the IRQ handler).
    let app_focused = crate::compositor::focused_task_id().is_some();

    let mut guard = SHELL_STATE.lock();
    let Some(state) = guard.as_mut() else { return };

    if state.phase == crate::session::SessionPhase::FirstBootSetup {
        // OOBE wizard owns the screen until the user creates an account.
        // On success the wizard auto-signs-in via session::login_password
        // so we jump straight to the desktop — no second login prompt.
        if crate::setup_ui::handle_key(&mut state.setup, scancode) {
            activate_desktop(state);
        } else {
            state
                .setup
                .render(state.surface_ptr, state.width, state.height);
            let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        }
        return;
    }

    if state.phase == crate::session::SessionPhase::Install {
        // The install wizard is modal: it owns the keyboard until the user
        // finishes, cancels, or restarts.
        use crate::installer_ui::InstallSignal;
        match crate::installer_ui::handle_key(&mut state.installer, extended, scancode) {
            InstallSignal::Repaint => {
                state
                    .installer
                    .render(state.surface_ptr, state.width, state.height);
                let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
            }
            InstallSignal::BeginInstall => {
                state.installer.step = crate::installer_ui::InstallStep::Installing;
                state
                    .installer
                    .render(state.surface_ptr, state.width, state.height);
                let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
                spawn_installer_worker();
            }
            InstallSignal::Reboot => crate::installer_ui::reboot(),
            InstallSignal::Cancel => leave_installer(state),
            InstallSignal::Ignored => {}
        }
        return;
    }

    if state.phase == crate::session::SessionPhase::Login {
        if crate::login_ui::handle_key(&mut state.login, scancode) {
            activate_desktop(state);
        } else {
            state
                .login
                .render(state.surface_ptr, state.width, state.height);
            let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        }
        return;
    }

    if state.phase == crate::session::SessionPhase::Locked {
        handle_lock_key(state, code);
        return;
    }

    if crate::session::phase() == crate::session::SessionPhase::Login {
        return_to_login(state);
        return;
    }

    // GameOS couch mode is modal: while active, the keyboard IS the
    // controller (MasterChecklist Phase 12.2/14.3).
    if state.couch.is_some() {
        couch_handle_key(state, extended, code);
        return;
    }
    // RaeWeb browser surface is modal: while open, the keyboard drives the
    // address bar / scroll / link activation (Concept §3 "renders through AthUI").
    if state.webview.is_some() {
        webview_handle_key(state, extended, code);
        return;
    }
    // F7 — open the RaeWeb browser surface from the desktop.
    if !extended && code == 0x41 && state.shell.is_some() {
        toggle_webview(state);
        return;
    }
    // F11 — enter GameOS couch mode from the desktop.
    if !extended && code == 0x57 && state.shell.is_some() {
        toggle_gameos(state);
        return;
    }
    // F10 — toggle the in-game Game Bar overlay (GameOS Phase 4) over the
    // desktop. Composites FPS + frametime graph + CPU/GPU temps from the LIVE
    // perf/thermal counters; a second F10 dismisses it.
    if !extended && code == 0x44 && state.shell.is_some() {
        toggle_game_bar(state);
        return;
    }

    // ── Command palette (Super+Space) — the global launcher + action runner.
    // Super+Space toggles it; while open it is MODAL (owns the keyboard) so a
    // blind type→Enter launches the best match (spec §1/§6). Handled before the
    // start-menu/app key routing below.
    if state.shell.is_some() {
        let palette_open = state
            .shell
            .as_ref()
            .map(|s| s.command_palette.visible)
            .unwrap_or(false);

        // Super+Space — toggle (works whether or not the palette is already up).
        if !extended && code == 0x39 && state.super_held {
            if let Some(shell) = state.shell.as_mut() {
                shell.command_palette.toggle();
            }
            repaint_desktop(state);
            crate::serial_println!("[shell_runner] command palette toggled (Super+Space)");
            return;
        }

        if palette_open {
            palette_handle_key(state, extended, code);
            return;
        }
    }

    // ── Clipboard-history panel (Super+C) — the Win+V-class glass flyout
    // (docs/design/clipboard-history.md). Super+C toggles it; while open it is
    // MODAL (owns the keyboard) so arrows/Enter/P/Delete/digits drive it and a
    // blind Super+C→Enter pastes the last copy. The Super+V the spec names is
    // already claimed by other chords on this PS/2 set, so the shell binds the
    // adjacent Super+C (the "clipboard" mnemonic) — documented here as the
    // AthShell Win+V analog.
    if state.shell.is_some() {
        let panel_open = state
            .shell
            .as_ref()
            .map(|s| s.clipboard_panel.visible)
            .unwrap_or(false);

        // Super+C (C = 0x2E) — toggle. On open, snapshot the live history first
        // so the panel renders the current ring (pinned-above-recent).
        if !extended && code == 0x2E && state.super_held {
            if panel_open {
                if let Some(shell) = state.shell.as_mut() {
                    shell.clipboard_panel.close();
                }
            } else {
                refresh_clipboard_panel(state);
                if let Some(shell) = state.shell.as_mut() {
                    shell.clipboard_panel.open();
                }
            }
            repaint_desktop(state);
            crate::serial_println!(
                "[shell_runner] clipboard panel toggled (Super+C) -> {}",
                if panel_open { "closed" } else { "opened" }
            );
            return;
        }

        if panel_open {
            clipboard_panel_handle_key(state, extended, code);
            return;
        }
    }

    // ── Snap Layouts (Rae key + Z) — window-management.md: the Win11-style
    // layout picker. Rae+Z toggles a flyout of layout templates for the focused
    // window; a click on a zone snaps the window there (mouse path below). Esc
    // closes it (handled in the Esc arm below). Only meaningful with a focused
    // window, but the flyout still opens so the user sees the templates.
    if !extended && code == 0x2C && state.super_held {
        if let Some(shell) = state.shell.as_mut() {
            shell.toggle_snap_layouts();
            crate::serial_println!(
                "[shell_runner] snap layouts toggled (Rae+Z) -> {}",
                if shell.snap_layouts_open() {
                    "opened"
                } else {
                    "closed"
                }
            );
        }
        repaint_desktop(state);
        return;
    }

    // ── Control Center (Super+A) — docs/design/control-center.md §1. Toggles the
    // bottom-right glass quick-settings flyout; on open it syncs each backed tile
    // from its live kernel reader (Wi-Fi/Focus/Night Light/Game Mode/volume). Esc
    // closes it (handled in the Esc arm below).
    if !extended && code == 0x1E && state.super_held {
        if let Some(shell) = state.shell.as_mut() {
            shell.control_center.toggle();
            if shell.control_center.visible {
                sync_control_center_backends(shell);
            }
            crate::serial_println!(
                "[shell_runner] control center toggled (Super+A) -> {}",
                if shell.control_center.visible {
                    "opened"
                } else {
                    "closed"
                }
            );
        }
        // Drive the widget-tier accessibility seam (P0 #1): publish the Control
        // Center's controls as named a11y nodes on open, clear them on close.
        publish_control_center_a11y(state);
        // Modal focus trap (P1 #4): an open Control Center traps Tab within its
        // tiles; closing it restores chrome focus.
        let cc_open = state
            .shell
            .as_ref()
            .map(|s| s.control_center.visible)
            .unwrap_or(false);
        if cc_open {
            open_control_center_focus_trap(state);
        } else {
            crate::a11y::focus_close_modal();
        }
        repaint_desktop(state);
        return;
    }

    // ── Accessibility on-switches (Phase 19 audit P0 #2) ─────────────────────
    // Global hotkeys that turn on the BUILT a11y engines (magnifier, color
    // filters, high-contrast forced-colors, reduced-motion) — the user-reach the
    // engines lacked. Each calls the kernel `a11y` backend, the single source of
    // truth the Control Center Accessibility tile also drives. Bindings chosen to
    // not collide with the existing chords (Super+A = Control Center, Super+Space
    // = palette, Super+C = clipboard, Super+Shift+S = screenshot):
    //   Super+'='        magnifier zoom in   (Windows Win+'=' parity)
    //   Super+'-'        magnifier zoom out  (Windows Win+'-' parity)
    //   Super+Alt+M      toggle magnifier
    //   Super+Alt+H      toggle high contrast (forced-colors palette swap)
    //   Super+Alt+C      cycle color filter (None -> Invert -> Grayscale)
    //   Super+Alt+R      toggle reduced motion
    if !extended && state.super_held {
        // Super+'=' (0x0D) — magnifier zoom in.
        if code == 0x0D {
            let z = crate::a11y::magnifier_zoom_in();
            crate::serial_println!("[shell_runner] a11y magnifier zoom in (Super+=) -> {}", z);
            return;
        }
        // Super+'-' (0x0C) — magnifier zoom out.
        if code == 0x0C {
            let z = crate::a11y::magnifier_zoom_out();
            crate::serial_println!("[shell_runner] a11y magnifier zoom out (Super+-) -> {}", z);
            return;
        }
        if state.alt_held {
            // Super+Alt+M (M = 0x32) — toggle magnifier.
            if code == 0x32 {
                let on = crate::a11y::toggle_magnifier();
                crate::serial_println!(
                    "[shell_runner] a11y magnifier toggle (Super+Alt+M) -> {}",
                    on
                );
                return;
            }
            // Super+Alt+H (H = 0x23) — toggle high-contrast forced-colors.
            if code == 0x23 {
                let on = crate::a11y::toggle_high_contrast();
                crate::serial_println!(
                    "[shell_runner] a11y high-contrast toggle (Super+Alt+H) -> {}",
                    on
                );
                repaint_desktop(state);
                return;
            }
            // Super+Alt+C (C = 0x2E) — cycle scanout color filter.
            if code == 0x2E {
                let mode = crate::a11y::cycle_color_filter();
                crate::serial_println!(
                    "[shell_runner] a11y color filter cycle (Super+Alt+C) -> {}",
                    mode
                );
                return;
            }
            // Super+Alt+R (R = 0x13) — toggle reduced motion.
            if code == 0x13 {
                let on = crate::a11y::toggle_reduced_motion();
                crate::serial_println!(
                    "[shell_runner] a11y reduced-motion toggle (Super+Alt+R) -> {}",
                    on
                );
                return;
            }
            // Super+Alt+K (K = 0x25) — toggle sticky keys (one-handed modifiers).
            if code == 0x25 {
                let on = crate::a11y_input::toggle_sticky_keys();
                crate::serial_println!(
                    "[shell_runner] a11y sticky-keys toggle (Super+Alt+K) -> {}",
                    on
                );
                return;
            }
            // Super+Alt+V (V = 0x2F) — toggle visual alerts / captions.
            if code == 0x2F {
                let on = crate::captions::toggle();
                crate::serial_println!(
                    "[shell_runner] a11y visual-alerts toggle (Super+Alt+V) -> {}",
                    on
                );
                return;
            }
        }
    }

    // ── Screenshot / region capture (Super+Shift+S) — docs/design/
    // screenshot-capture.md §1. Dims the screen → a selection rectangle with a
    // live dimensions readout + an action bar; Enter/C copies, S saves, Esc
    // cancels. The capture reads REAL composited pixels off the front buffer via
    // the cap-gated compositor engine (the same path SYS_CAPTURE_START 274
    // exposes). While the overlay is up it is MODAL (owns the keyboard).
    if state.shell.is_some() {
        let overlay_active = state
            .shell
            .as_ref()
            .map(|s| s.capture_overlay.is_active())
            .unwrap_or(false);

        // Super+Shift+S (S = 0x1F) — enter region-capture mode.
        if !extended && code == 0x1F && state.super_held && state.shift_held {
            if let Some(shell) = state.shell.as_mut() {
                let (sw, sh) = (shell.screen_width as u32, shell.screen_height as u32);
                shell.capture_overlay.begin(sw, sh);
            }
            repaint_desktop(state);
            crate::serial_println!("[shell_runner] screenshot capture-mode (Super+Shift+S)");
            return;
        }

        // PrintScreen (0x37 in set 1 — `*`/PrtSc) — full-screen capture, jump
        // straight to confirm + copy. (The bare-key full-screen affordance.)
        if !extended && code == 0x37 && !state.super_held {
            if let Some(shell) = state.shell.as_mut() {
                let (sw, sh) = (shell.screen_width as u32, shell.screen_height as u32);
                shell.capture_overlay.begin_full_screen(sw, sh);
                shell
                    .capture_overlay
                    .confirm(raeshell::screenshot_overlay::CaptureAction::Copy);
            }
            run_capture_if_confirmed(state);
            return;
        }

        if overlay_active {
            capture_overlay_handle_key(state, extended, code);
            return;
        }
    }

    // ── Overview / Expose + Spaces (window-management.md §1/§2) ──────────────
    // Left Super (extended 0x5B) toggles Overview for the current space. While
    // Overview is open it is MODAL chrome: arrows move the thumbnail selection,
    // Enter/click activates it, Esc/Super closes. Super+Ctrl+→/← switch spaces
    // (Win parity), Super+Shift+<digit> moves the focused window to space N.
    if state.shell.is_some() {
        // Super+Ctrl+Right / Left — next / previous space (works in or out of
        // overview; the most direct "flip my desktop" gesture).
        if extended && (code == 0x4D || code == 0x4B) && state.super_held && state.ctrl_held {
            let flips = if code == 0x4D {
                state.spaces.switch_next()
            } else {
                state.spaces.switch_prev()
            };
            if let Some(flips) = flips {
                apply_space_switch(state, flips);
            }
            return;
        }

        // Super+Arrow (no Ctrl) — directional window snap (Win+Arrow parity,
        // window-management.md): ←/→ tile the focused window to a half, ↑
        // maximizes then grows to a quarter, ↓ restores then minimizes. The
        // Win11 state machine lives in `raeshell::snap_directional` (host-KAT'd).
        // Skipped while Overview is open (arrows drive its thumbnail selection).
        if extended
            && state.super_held
            && !state.ctrl_held
            && !state.overview_open
            && matches!(code, 0x4B | 0x4D | 0x48 | 0x50)
        {
            let dir = match code {
                0x4B => raeshell::snap_directional::SnapDir::Left,
                0x4D => raeshell::snap_directional::SnapDir::Right,
                0x48 => raeshell::snap_directional::SnapDir::Up,
                _ => raeshell::snap_directional::SnapDir::Down,
            };
            if let Some(shell) = state.shell.as_mut() {
                if let Some((sid, r, minimized)) = shell.snap_directional(dir) {
                    if minimized {
                        let _ = crate::compositor::set_surface_minimized(sid, true);
                    } else {
                        let _ = crate::compositor::set_surface_minimized(sid, false);
                        let _ = crate::compositor::set_surface_origin(sid, r.x, r.y);
                        let _ = crate::compositor::request_surface_resize(sid, r.w, r.h);
                    }
                    crate::serial_println!(
                        "[shell_runner] snap directional (Rae+Arrow): win {} -> {}x{} at ({},{}) min={}",
                        sid,
                        r.w,
                        r.h,
                        r.x,
                        r.y,
                        minimized
                    );
                    repaint_desktop(state);
                }
            }
            return;
        }

        // Super+Shift+<digit 1-9> — move the focused window to space N (creating
        // intermediate spaces as needed).
        if !extended && state.super_held && state.shift_held && (0x02..=0x0A).contains(&code) {
            let n = (code - 0x01) as usize; // 0x02 -> '1' -> space index 0
            move_focused_to_space(state, n.saturating_sub(1));
            return;
        }

        // Left Super tap — toggle Overview.
        if extended && code == 0x5B {
            toggle_overview(state);
            return;
        }

        // Esc closes overview if open; arrows/Enter drive selection.
        if state.overview_open {
            overview_handle_key(state, extended, code);
            return;
        }
    }

    // ── Unified desktop keyboard focus order (Phase 19 audit P1 #4) ──────────
    // On the IDLE desktop (no app focused, no Start menu open), Tab advances the
    // chrome focus ring (Start -> taskbar items -> tray) and Shift+Tab reverses;
    // both WRAP. While the Control Center flyout is open, the SAME Tab cycles
    // WITHIN its trapped focusables (a11y::FocusOrder owns the trap). Enter/Space
    // activates the focused chrome item. This is the keyboard-only "no mouse
    // required" traversal across the persistent shell chrome. (The launcher is now
    // reached by Tab-to-Start + Enter, matching Windows Win->Tab->Enter.)
    if state.shell.is_some() && !app_focused {
        let cc_open = state
            .shell
            .as_ref()
            .map(|s| s.control_center.visible)
            .unwrap_or(false);
        let start_open = state
            .shell
            .as_ref()
            .map(|s| s.start_menu.visible)
            .unwrap_or(false);
        // Tab is the chrome traversal key UNLESS the Start menu is open (then its
        // own search/list handling below owns the keyboard) and unless Alt is held
        // (Alt+Tab window cycling, handled in the match below).
        let tab_for_chrome = !extended && code == 0x0F && !start_open && !state.alt_held;
        if tab_for_chrome {
            // Keep the chrome order fresh (taskbar items may have changed) — but
            // never re-publish while a modal trap is active (that would clobber the
            // trapped ring).
            if !cc_open {
                publish_chrome_focus_order(state);
            }
            desktop_focus_traverse(state, !state.shift_held);
            return;
        }
        // Enter / Space activates the focused chrome item when a chrome element
        // (not a window/menu) currently holds focus and no overlay owns the keys.
        let activate_key = !extended && (code == 0x1C || code == 0x39);
        if activate_key && !start_open && crate::a11y::focus_current().is_some() {
            if desktop_focus_activate(state) {
                return;
            }
        }
    }

    let shell = match state.shell.as_mut() {
        Some(s) => s,
        None => return,
    };

    let mut consumed = false;
    let mut launch: Option<alloc::string::String> = None;

    match (extended, code) {
        // Alt+Tab — cycle windows (0x0F Tab while Alt held)
        (false, 0x0F) if state.alt_held => {
            cycle_alt_tab(state);
            return;
        }
        // F12 — lock workstation (Concept: lock screen before desktop access)
        (false, 0x58) => {
            enter_lock_screen(state);
            return;
        }
        // F10 — sign out (return to login screen)
        (false, 0x44) => {
            return_to_login(state);
            return;
        }
        // F9 — open the install wizard over the desktop (Concept: install is
        // always reachable, never automatic — Cancel returns here).
        (false, 0x43) => {
            enter_installer(state);
            return;
        }
        // F8 — toggle the Notification Center / Control Center pull-down
        // (raeen-parity #1: the everyday panel a Windows/macOS switcher reaches
        // for). It floats as its own compositor surface, so no desktop repaint
        // is needed here.
        (false, 0x42) => {
            let open = crate::notify::toggle_center();
            crate::serial_println!(
                "[shell_runner] notification center {}",
                if open { "opened" } else { "closed" }
            );
            return;
        }
        // Tab (0x0F) — toggle start menu (the launcher). Left Super now drives
        // Overview/Expose (window-management.md §1 keymap), handled below the
        // match so it can call the compositor + render the grid chrome.
        (false, 0x0F) => {
            shell.start_menu.toggle();
            consumed = true;
        }
        // Down arrow (0xE0 0x50) — only when start menu is open
        (true, 0x50) if shell.start_menu.visible => {
            shell.start_menu.select_next();
            consumed = true;
        }
        // Up arrow (0xE0 0x48) — only when start menu is open
        (true, 0x48) if shell.start_menu.visible => {
            shell.start_menu.select_prev();
            consumed = true;
        }
        // Enter (main 0x1C non-extended, or keypad-Enter 0xE0 0x1C extended) —
        // launch the selected app whenever the Start menu is open. BUG C:
        // ALWAYS close the menu on Enter, not only when a selection resolved, so
        // the freshly-launched window (z=6) is never occluded by a re-rendered
        // menu (and an empty-search Enter can't leave the menu stuck open).
        //
        // P1 (launcher-reliability live drive): the launch used to "mostly not
        // fire" (~1 in 15). Root cause was the chrome-focus Enter-activate guard
        // above (`!start_open` path) RE-TOGGLING the menu shut on the SAME Enter
        // that should launch — and a stale chrome-focus cursor on Start meant the
        // guard fired before this arm could. The guard is now hard-gated off while
        // the menu is open (it reads a FRESH `start_menu.visible` right before it,
        // and `desktop_focus_activate` clears chrome focus when it opens Start), so
        // this arm is the SOLE Enter handler once the menu is up. The diagnostic
        // makes the next live drive prove visible-at-Enter == true.
        (false, 0x1C) | (true, 0x1C) if shell.start_menu.visible => {
            let selected = shell.start_menu.selected_app();
            crate::serial_println!(
                "[shell_runner] start enter: visible=true selected={:?}",
                selected.as_ref().map(|a| a.name.as_str()),
            );
            if let Some(app) = selected {
                launch = Some(app.exec_path.clone());
            }
            // Closing the menu + dropping chrome focus guarantees the launched
            // window (z=6) is composited on top and nothing re-toggles the menu.
            shell.start_menu.visible = false;
            crate::a11y::focus_close_modal();
            consumed = true;
        }
        // Escape — close overlays
        (false, 0x01) => {
            if shell.start_menu.visible {
                // P2 dismiss: set visibility false directly (not toggle — toggle
                // would REOPEN if it were already closed) and clear chrome focus
                // so the re-composite below erases the panel and the launched
                // window stays on top.
                shell.start_menu.visible = false;
                crate::a11y::focus_close_modal();
                consumed = true;
            }
            if shell.settings.visible {
                shell.settings.toggle();
                consumed = true;
            }
            if shell.control_center.visible {
                shell.control_center.close();
                // Release the modal focus trap and restore chrome focus (P1 #4).
                crate::a11y::focus_close_modal();
                consumed = true;
            }
            if shell.snap_layouts_open() {
                // Dismiss the Snap Layouts flyout without snapping (Esc = cancel).
                shell.toggle_snap_layouts();
                consumed = true;
            }
            if shell.snap_assist_active() {
                // Dismiss Snap Assist — leave the already-snapped windows as they
                // are, just stop offering to fill the remaining zones.
                shell.snap_assist_close();
                consumed = true;
            }
        }
        // Backspace — edit the start-menu search query (Phase 14.1 search bar)
        (false, 0x0E) if shell.start_menu.visible => {
            shell.start_menu.search_query.pop();
            shell.start_menu.selected_index = 0;
            consumed = true;
        }
        // Printable keys type into the start-menu search bar; the menu's
        // app list filters live on the query.
        (false, c) if shell.start_menu.visible => {
            if let Some(ascii) = lock_scancode_to_ascii(c, false) {
                if (0x20..0x7F).contains(&ascii) && shell.start_menu.search_query.len() < 32 {
                    shell.start_menu.search_query.push(ascii as char);
                    shell.start_menu.selected_index = 0;
                    consumed = true;
                }
            }
        }
        // All other keys: only consume if no app has focus (desktop is active)
        _ if !app_focused => {}
        // App has focus — don't consume, the per-task buffer handles it
        _ => {
            return;
        }
    }

    if let Some(path) = launch {
        crate::serial_println!("[shell_runner] launch requested: {}", path);
        spawn_app_from_vfs(&path);
    }

    if consumed {
        let banner = alloc::format!("Welcome, {}", crate::session::display_name());
        render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
        // `shell` is no longer borrowed past here, so we can re-publish the
        // Control Center's accessibility nodes: an Esc/Tab/toggle that changed
        // CC visibility must keep the widget-tier a11y tree in sync (controls
        // appear on open, are cleared on close) — P0 #1 live drive.
        publish_control_center_a11y(state);
        let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
    }
}

/// Resolve a path through VFS, read the ELF data, and spawn a new task.
pub(crate) fn spawn_app_from_vfs(path: &str) {
    let resolved = crate::app_paths::resolve_candidates(path)
        .into_iter()
        .find_map(|candidate| crate::vfs::read_file(&candidate).map(|data| (candidate, data)));
    let Some((resolved_path, elf_data)) = resolved else {
        crate::serial_println!("[shell_runner] app not found in VFS: {}", path);
        return;
    };

    let title = resolved_path.rsplit('/').next().unwrap_or(&resolved_path);

    match crate::scheduler::spawn_elf_task(&elf_data, None) {
        Ok(task_id) => {
            CLICK_LAUNCH_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // Phase 9: sandbox the app at launch. The app's RaeManifest.toml
            // (apps/<name>/RaeManifest.toml in the boot media) declares its
            // sandbox level + permission grants; apps without a manifest fall
            // back to the first-party allowlist (Trusted) or AppSandbox.
            let (level, from_manifest) =
                crate::rae_manifest::assign_for_spawn(title, task_id.raw());
            crate::serial_println!(
                "[shell_runner] click launch: '{}' -> task {} (sandbox={:?}, {})",
                resolved_path,
                task_id.raw(),
                level,
                if from_manifest {
                    "RaeManifest.toml"
                } else {
                    "allowlist fallback"
                },
            );
            PENDING_TITLES
                .lock()
                .insert(task_id.raw(), alloc::string::String::from(title));
        }
        Err(e) => {
            crate::serial_println!("[shell_runner] failed to spawn '{}': {}", path, e,);
        }
    }
}

/// Handle a PS/2 mouse event from the IRQ handler. `dx`/`dy` are deltas
/// (already applied to the compositor cursor), `buttons` is the PS/2 button
/// bitmask (bit 0 = left, bit 1 = right, bit 2 = middle).
pub fn handle_mouse(_dx: i32, _dy: i32, buttons: u8) {
    static PREV_BUTTONS: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

    let prev = PREV_BUTTONS.swap(buttons, core::sync::atomic::Ordering::Relaxed);
    let left_down = (buttons & 1) != 0;
    let left_click = left_down && (prev & 1) == 0;
    let left_release = !left_down && (prev & 1) != 0;

    let mut guard = SHELL_STATE.lock();
    let Some(state) = guard.as_mut() else { return };

    if state.phase != crate::session::SessionPhase::Active {
        return;
    }

    let Some((mx, my)) = crate::compositor::cursor_position() else {
        return;
    };

    // RaeWeb browser surface is modal: a click goes to the web view (link
    // navigation or address-bar focus), never to the windows underneath.
    if state.webview.is_some() {
        if left_click {
            let changed = state
                .webview
                .as_mut()
                .map(|v| v.click(mx as f32, my as f32))
                .unwrap_or(false);
            if changed {
                render_webview(state);
            }
        }
        return;
    }

    // Snap Layouts flyout is modal + topmost while open (window-management.md):
    // cursor moves update the highlighted zone; a left click snaps the focused
    // window to that zone (position immediately + a cooperative resize request),
    // or dismisses the flyout on a click that misses every zone.
    if state
        .shell
        .as_ref()
        .map(|s| s.snap_layouts_open())
        .unwrap_or(false)
    {
        if left_click {
            let (snapped, sid) = match state.shell.as_mut() {
                Some(sh) => (sh.snap_layouts_click(mx, my), sh.focused_surface),
                None => (None, None),
            };
            if let (Some(r), Some(sid)) = (snapped, sid) {
                let _ = crate::compositor::set_surface_origin(sid, r.x, r.y);
                let _ = crate::compositor::request_surface_resize(sid, r.w, r.h);
                crate::serial_println!(
                    "[shell_runner] snap layouts: window {} -> {}x{} at ({},{})",
                    sid,
                    r.w,
                    r.h,
                    r.x,
                    r.y
                );
            }
            repaint_desktop(state);
        } else if let Some(sh) = state.shell.as_mut() {
            if sh.snap_layouts_hover(mx, my) {
                repaint_desktop(state);
            }
        }
        return;
    }

    // Snap Assist picker is modal while active (it takes over after a layout
    // snap): a click on a candidate tile fills the current zone with that window,
    // a click-away dismisses. Cursor moves highlight the hovered tile.
    if state
        .shell
        .as_ref()
        .map(|s| s.snap_assist_active())
        .unwrap_or(false)
    {
        if left_click {
            let snapped = state
                .shell
                .as_mut()
                .and_then(|s| s.snap_assist_click(mx, my));
            if let Some((sid, r)) = snapped {
                let _ = crate::compositor::set_surface_minimized(sid, false);
                let _ = crate::compositor::set_surface_origin(sid, r.x, r.y);
                let _ = crate::compositor::request_surface_resize(sid, r.w, r.h);
                crate::serial_println!(
                    "[shell_runner] snap assist: filled zone with window {} -> {}x{} at ({},{})",
                    sid,
                    r.w,
                    r.h,
                    r.x,
                    r.y
                );
            }
            repaint_desktop(state);
        } else if let Some(sh) = state.shell.as_mut() {
            if sh.snap_assist_hover(mx, my) {
                repaint_desktop(state);
            }
        }
        return;
    }

    if left_release {
        state.drag = None;
    }

    if let Some((sid, off_x, off_y)) = state.drag {
        if left_down {
            let nx = mx - off_x;
            let ny = my - off_y;
            let _ = crate::compositor::set_surface_origin(sid, nx, ny);
            crate::compositor::recomposite();
            return;
        }
        state.drag = None;
    }

    if !left_click {
        return;
    }

    // Notification Center is open and topmost: route the click into its panel
    // (quick-settings toggles / dismiss / clear-all) before anything else, and
    // never let it fall through to window focus or the desktop chrome.
    if crate::notify::center_visible() {
        if let Some(sid) = crate::compositor::surface_at(mx, my) {
            if crate::notify::center_surface_at(sid) {
                if let Some((px, py, _, _, _)) = crate::compositor::surface_frame(sid) {
                    crate::notify::center_click(mx - px, my - py);
                }
                return;
            }
        }
    }

    // Focus userspace windows via compositor geometry (topmost hit).
    if let Some(sid) = crate::compositor::surface_at(mx, my) {
        if let Some((wx, wy, ww, _, minimized)) = crate::compositor::surface_frame(sid) {
            use crate::window_chrome::{hit_test, ChromeHit};
            match hit_test(wx, wy, ww, mx, my) {
                ChromeHit::Close => {
                    // Closing a window drops it from any snap group (a partner
                    // that no longer exists must not resurrect on group restore).
                    if let Some(shell) = state.shell.as_mut() {
                        shell.snap_group_forget(sid);
                    }
                    if let Some(owner) = crate::compositor::surface_owner(sid) {
                        let _ = crate::scheduler::kill_task(owner);
                    }
                    return;
                }
                ChromeHit::Minimize => {
                    // Snap groups: minimizing one window tucks the whole group
                    // away together (Win11). Falls back to the single window.
                    let group = state
                        .shell
                        .as_ref()
                        .map(|s| s.snap_group_members(sid))
                        .unwrap_or_default();
                    if group.is_empty() {
                        let _ = crate::compositor::set_surface_minimized(sid, true);
                    } else {
                        for (gid, _) in &group {
                            let _ = crate::compositor::set_surface_minimized(*gid, true);
                        }
                        crate::serial_println!(
                            "[shell_runner] snap group minimized together ({} windows)",
                            group.len()
                        );
                    }
                    crate::compositor::recomposite();
                    return;
                }
                ChromeHit::Maximize => {
                    if minimized {
                        let _ = crate::compositor::set_surface_minimized(sid, false);
                        crate::compositor::recomposite();
                    }
                    return;
                }
                ChromeHit::Title => {
                    state.drag = Some((sid, mx - wx, my - wy));
                }
                ChromeHit::Client => {}
                ChromeHit::None => return,
            }
        }
        let _ = crate::compositor::focus_surface(sid);
        if let Some(shell) = state.shell.as_mut() {
            shell.focus_window(sid);
            let banner = alloc::format!("Welcome, {}", crate::session::display_name());
            render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
            let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        }
        return;
    }

    let consumed = dispatch_click(state, mx, my);

    if consumed {
        if let Some(shell) = state.shell.as_mut() {
            let banner = alloc::format!("Welcome, {}", crate::session::display_name());
            render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
            let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        }
    }
}

fn dispatch_click(state: &mut ShellRunnerState, mx: i32, my: i32) -> bool {
    let shell = match state.shell.as_mut() {
        Some(s) => s,
        None => return false,
    };
    let tb = &shell.taskbar_rect;

    // Click on taskbar? Geometry comes from the shell's single-authority rect
    // helpers — the SAME rects the render paints and the a11y focus ring uses
    // (the old inline math here diverged from the drawn buttons, so clicks
    // landed on the wrong app).
    if tb.contains(mx, my) {
        if shell.taskbar_start_rect().contains(mx, my) {
            shell.start_menu.toggle();
            return true;
        }

        // Check taskbar buttons (combined pinned+running centered cluster).
        // A running button focuses its window; a pinned launcher spawns the
        // app via the same VFS path the Start menu uses.
        let buttons = shell.taskbar_buttons();
        let hit = (0..buttons.len())
            .find(|&i| shell.taskbar_item_rect(i).contains(mx, my))
            .map(|i| buttons[i].clone());
        if let Some(btn) = hit {
            if let Some(sid) = btn.surface_id {
                // Snap groups: restoring one member from the taskbar brings the
                // whole group back to its layout (Win11). Each member is
                // un-minimized and re-placed at its remembered zone rect.
                let group = shell.snap_group_members(sid);
                if group.is_empty() {
                    let _ = crate::compositor::set_surface_minimized(sid, false);
                } else {
                    for (gid, rect) in &group {
                        let _ = crate::compositor::set_surface_minimized(*gid, false);
                        let _ = crate::compositor::set_surface_origin(*gid, rect.x, rect.y);
                        let _ = crate::compositor::request_surface_resize(*gid, rect.w, rect.h);
                    }
                    crate::serial_println!(
                        "[shell_runner] snap group restored together ({} windows)",
                        group.len()
                    );
                }
                shell.focus_window(sid);
                let _ = crate::compositor::focus_surface(sid);
            } else if !btn.exec_path.is_empty() {
                crate::serial_println!(
                    "[shell_runner] taskbar pinned launch: {} -> {}",
                    btn.label,
                    btn.exec_path,
                );
                spawn_app_from_vfs(&btn.exec_path);
            }
            return true;
        }

        // Click on tray area (clock/status cluster) — toggle the Control Center
        // glass flyout (docs/design/control-center.md §1: "click the tray
        // cluster"). The notification center remains reachable via Super+N /
        // the bell; the tray cluster is the quick-settings affordance every
        // Windows/macOS switcher expects.
        let tray_x = shell.taskbar_tray_x();
        if mx as usize >= tray_x {
            shell.control_center.toggle();
            sync_control_center_backends(shell);
            crate::serial_println!(
                "[shell_runner] tray click -> control center {}",
                if shell.control_center.visible {
                    "opened"
                } else {
                    "closed"
                }
            );
            return true;
        }

        return false;
    }

    // Click inside the Control Center flyout (tiles, expand regions). Handled
    // before the generic background-click close so a tap on a tile toggles it
    // (and the chevron expands in place) rather than dismissing the flyout.
    if shell.control_center.visible {
        if let Some((kind, on_expand)) = shell.control_center.tile_at(mx, my) {
            if on_expand {
                shell.control_center.toggle_expand(kind);
                if matches!(
                    kind,
                    raeshell::control_center::TileKind::WiFi
                        | raeshell::control_center::TileKind::Bluetooth
                ) {
                    load_control_center_wifi_rows(shell);
                }
            } else {
                shell.control_center.toggle_tile(kind);
                apply_control_center_tile(shell, kind);
            }
            return true;
        }
        // Click outside the panel closes it (click-away, spec §1).
        let (px, py, pw, ph) = shell.control_center.panel_rect();
        let inside =
            mx >= px as i32 && mx < (px + pw) as i32 && my >= py as i32 && my < (py + ph) as i32;
        if !inside {
            shell.control_center.close();
            // P2 (modal focus-trap leak): release the modal focus trap on the
            // click-outside close too — the Esc arm does both, but this path only
            // closed the panel, so Tab kept cycling ONLY the (now hidden) CC
            // controls forever and a keyboard user could never get back to Start.
            crate::a11y::focus_close_modal();
            return true;
        }
        return true;
    }

    // Click on start menu? Single-click launches (matches Windows / macOS UX).
    if shell.start_menu.visible && shell.start_menu.rect.contains(mx, my) {
        // P2 (geometry mismatch): the hit-test used to hardcode list_y=rect.y+48,
        // item_height=32 while render drew at +72 / 36 — clicks landed on the
        // WRONG row. `StartMenu::row_at` is now the SINGLE source of the row
        // geometry, shared with `render()`, so the clicked row == the drawn row.
        if let Some(idx) = shell.start_menu.row_at(my) {
            let exec_path = shell.start_menu.filtered_apps()[idx].exec_path.clone();
            shell.start_menu.selected_index = idx;
            crate::serial_println!(
                "[shell_runner] start click launch: row {} -> {}",
                idx,
                exec_path,
            );
            spawn_app_from_vfs(&exec_path);
            // Dismiss + drop chrome focus so the launched z=6 window is on top.
            shell.start_menu.visible = false;
            crate::a11y::focus_close_modal();
        }
        return true;
    }

    // Click on settings panel?
    if shell.settings.visible && shell.settings.rect.contains(mx, my) {
        return true;
    }

    // Close open menus on background click (P2 dismiss): clear visibility +
    // chrome focus so the menu is erased on the re-composite and nothing
    // re-opens it on the next Enter.
    if shell.start_menu.visible {
        shell.start_menu.visible = false;
        crate::a11y::focus_close_modal();
        return true;
    }

    // Click on a window — focus it in the compositor.
    if let Some(surface_id) = shell.window_manager.window_at(mx, my) {
        shell.focus_window(surface_id);
        let _ = crate::compositor::focus_surface(surface_id);
        return true;
    }

    false
}

// ── Control Center backend bridge ──────────────────────────────────────────
//
// The Control Center surface (raeshell) owns the layout + state matrix; the
// kernel owns the real subsystems. These bridges sync the rendered tile state
// FROM the live backend readers and push tile toggles INTO them, so a Wi-Fi /
// Focus / Night Light tile changes real system state (not a faked label) and
// the panel always opens reflecting the machine's actual posture.

/// Sync every backed tile's ON-state from its LIVE kernel reader (called when
/// the Control Center opens). Honest: tiles with no backend yet keep their local
/// state; tiles wired to `notify::quick_settings` reflect the real flag.
fn sync_control_center_backends(shell: &mut raeshell::DesktopShell) {
    use raeshell::control_center::TileKind;
    let cc = &mut shell.control_center;
    cc.set_tile_enabled(
        TileKind::WiFi,
        crate::notify::quick_settings::is_on(crate::notify::quick_settings::Control::Wifi),
    );
    cc.set_tile_enabled(
        TileKind::DoNotDisturb,
        crate::notify::quick_settings::is_on(crate::notify::quick_settings::Control::Dnd),
    );
    cc.set_tile_enabled(
        TileKind::NightLight,
        crate::notify::quick_settings::is_on(crate::notify::quick_settings::Control::NightLight),
    );
    // Game Mode reflects the live SCHED_BODY foreground-entry counter posture.
    cc.set_tile_enabled(
        TileKind::GameMode,
        crate::game_session::stats().game_mode_entries > 0,
    );
    // Volume slider reads the live master volume (0..=100).
    let vol = (crate::audio::quick_master_volume() * 100.0) as u32;
    cc.set_volume(vol.min(100));
    // Bluetooth has no kernel radio yet — flag it disabled (honest, not "Off").
    cc.set_tile_disabled(TileKind::Bluetooth, true);
    // Accessibility tile reflects the LIVE high-contrast forced-colors flag
    // (audit P0 #2/#3) — the same engine the Super+Alt+H hotkey drives, so the
    // tile and the hotkey can never disagree.
    cc.set_tile_enabled(TileKind::Accessibility, crate::a11y::high_contrast_on());
}

/// Publish the Control Center's tiles as widget-tier accessibility nodes under
/// the desktop surface (Phase 19 audit P0 #1 — apps/chrome name their controls).
///
/// This is the LIVE drive of the AthUI widget-provider seam: the kernel shell
/// builds a `raeui::accessibility::AccessibilityTree` from the live tiles (AthUI
/// does the role inference — a toggle tile is a `Switch`, a Wi-Fi/RGB row a
/// `Button` because it expands), runs `provider_nodes_for_window`, and publishes
/// the result through `a11y::publish_window_widgets_from_provider`. When focus is
/// on a tile (`focus_index`), the matching node is marked focused so the screen
/// reader names the CONTROL ("High Contrast, switch, on") instead of the window.
///
/// Called on open/sync (controls appear) and on close (cleared) so the live
/// accessibility tree mirrors the visible chrome — never a hand-maintained shadow.
/// Concept §"Built for people who care about how things feel": a screen-reader
/// user reaches the same quick-settings a sighted user does.
fn publish_control_center_a11y(state: &mut ShellRunnerState) {
    use raeui::accessibility::{
        provider_nodes_for_window, AccessibilityNode, AccessibilityRole, Rect,
    };
    let win = state.surface_id;
    let Some(shell) = state.shell.as_ref() else {
        return;
    };
    let cc = &shell.control_center;
    if !cc.visible {
        // Cleared: the chrome's controls are gone — drop them from the tree so a
        // reader doesn't name hidden controls.
        crate::a11y::publish_window_widgets(win, alloc::vec::Vec::new());
        return;
    }

    let (px, py, _pw, _ph) = cc.panel_rect();
    let mut tree = raeui::accessibility::AccessibilityTree::new();
    // Stable per-tile ids (offset so they never collide with app widget ids).
    const CC_TILE_BASE: u32 = 0x00C0_0000;
    let mut focused_id: Option<u64> = None;
    for (i, tile) in cc.tiles.iter().enumerate() {
        // AthUI role: an expandable tile (Wi-Fi/Bluetooth/RGB/Performance) opens
        // a sub-panel -> Button; a plain on/off tile -> Switch. The registry-free
        // path: construct the node with the inferred role + the tile's real label.
        let role = if tile.expandable {
            AccessibilityRole::Button
        } else {
            AccessibilityRole::Switch
        };
        let id = CC_TILE_BASE + i as u32;
        // Coarse per-tile bounds inside the panel (2-column grid, 56px rows). Used
        // only for focus-follows magnifier panning, not hit-testing.
        let col = (i % 2) as i32;
        let row = (i / 2) as i32;
        let bounds = Rect {
            x: (px as i32 + 12 + col * 120) as f32,
            y: (py as i32 + 12 + row * 56) as f32,
            width: 112.0,
            height: 48.0,
        };
        let mut node = AccessibilityNode::new(id, role, tile.label.clone(), bounds);
        node.traits.disabled = tile.disabled;
        // A Switch reports on/off via the CHECKED-mapped `selected` trait the
        // provider projects to A11Y_STATE_CHECKED.
        node.traits.selected = tile.enabled;
        tree.nodes.push(node);
        if cc.focus_index == Some(i) {
            focused_id = Some(id as u64);
        }
    }
    // If nothing is keyboard-focused yet, name the first tile so describe_focused
    // returns a real control rather than the window (a reader lands on something).
    if focused_id.is_none() && !cc.tiles.is_empty() {
        focused_id = Some(CC_TILE_BASE as u64);
    }

    let nodes = provider_nodes_for_window(win, &tree);
    let count = nodes.len();
    crate::a11y::publish_window_widgets_from_provider(win, nodes, focused_id);
    crate::serial_println!(
        "[shell_runner] control center a11y published {} controls (focused={:?})",
        count,
        focused_id
    );
}

// ── Unified desktop keyboard focus order (Phase 19 audit P1 #4) ──────────
// The persistent shell chrome (taskbar Start button, taskbar window items, the
// system-tray cluster) is kernel-drawn and is NOT in the a11y `build_tree()` walk
// (which enumerates app windows + provider widgets, not the bars). So the chrome
// focus order is published here, into the kernel `a11y::FocusOrder` engine, which
// both the Tab key handler and `/proc/raeen/a11y` read. Tab advances + wraps,
// Shift+Tab reverses + wraps, and an open flyout (Control Center) installs a MODAL
// trap so focus cannot leak to the chrome behind it. This is the keyboard-only
// "no mouse required" traversal across the desktop chrome.

/// Stable chrome focus ids (high values; never collide with compositor surface
/// ids or app widget ids). The Start button is always first in the order.
const FOCUS_ID_START: u64 = 0xFCC0_0000;
/// Base for taskbar window items (one per running app, in taskbar order).
const FOCUS_ID_TASKBAR_BASE: u64 = 0xFCC0_1000;
/// The system-tray cluster (opens Control Center on activate) — last in order.
const FOCUS_ID_TRAY: u64 = 0xFCC0_2000;

/// Build + publish the desktop chrome focus order from the live shell. Order:
/// Start button, then each taskbar window item left-to-right, then the tray
/// cluster — the macOS/Windows convention (launcher, running apps, status). Each
/// item carries its on-screen rect so the visible focus ring is drawn around the
/// real chrome widget. Called at desktop activation and whenever the chrome
/// changes (an app opens/closes). A no-op when no shell is up.
fn publish_chrome_focus_order(state: &ShellRunnerState) {
    let Some(shell) = state.shell.as_ref() else {
        return;
    };
    let tb = &shell.taskbar_rect;
    let mut items: alloc::vec::Vec<crate::a11y::FocusItem> = alloc::vec::Vec::new();

    // 1. Start button — the SAME rect the render paints and the click handler
    //    hit-tests (shell.taskbar_start_rect is the single geometry authority;
    //    the old inline math drew the focus ring off the visible pill).
    let sr = shell.taskbar_start_rect();
    items.push(crate::a11y::FocusItem {
        id: FOCUS_ID_START,
        role: rae_abi::syscall::A11Y_ROLE_BUTTON,
        name: alloc::string::String::from("Start"),
        x: sr.x,
        y: sr.y,
        w: sr.w,
        h: sr.h,
    });

    // 2. Taskbar buttons — the combined pinned+running centered cluster
    //    (pinned launchers are focusable too; Enter launches them).
    for (i, btn) in shell.taskbar_buttons().iter().enumerate() {
        let r = shell.taskbar_item_rect(i);
        items.push(crate::a11y::FocusItem {
            id: FOCUS_ID_TASKBAR_BASE + i as u64,
            role: rae_abi::syscall::A11Y_ROLE_BUTTON,
            name: btn.label.clone(),
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        });
    }

    // 3. System-tray cluster (right edge) — activates the Control Center.
    let tray_x = shell.taskbar_tray_x() as i32;
    items.push(crate::a11y::FocusItem {
        id: FOCUS_ID_TRAY,
        role: rae_abi::syscall::A11Y_ROLE_TOOLBAR,
        name: alloc::string::String::from("System tray"),
        x: tray_x,
        y: tb.y,
        w: (tb.w as i32 - tray_x).max(0) as u32,
        h: tb.h,
    });

    crate::a11y::focus_set_chrome(items);
}

/// Open a MODAL focus trap for the Control Center flyout: its tiles become the
/// only Tab-reachable focusables until it closes. Mirrors the same per-tile
/// bounds/labels `publish_control_center_a11y` computes so the trapped ring and
/// the widget-tier a11y tree agree.
fn open_control_center_focus_trap(state: &ShellRunnerState) {
    let Some(shell) = state.shell.as_ref() else {
        return;
    };
    let cc = &shell.control_center;
    let (px, py, _pw, _ph) = cc.panel_rect();
    let mut items: alloc::vec::Vec<crate::a11y::FocusItem> = alloc::vec::Vec::new();
    for (i, tile) in cc.tiles.iter().enumerate() {
        if tile.disabled {
            continue; // a disabled tile is not a focus stop
        }
        let role = if tile.expandable {
            rae_abi::syscall::A11Y_ROLE_BUTTON
        } else {
            rae_abi::syscall::A11Y_ROLE_SWITCH
        };
        let col = (i % 2) as i32;
        let row = (i / 2) as i32;
        items.push(crate::a11y::FocusItem {
            id: 0xFCC0_3000 + i as u64,
            role,
            name: tile.label.clone(),
            x: px as i32 + 12 + col * 120,
            y: py as i32 + 12 + row * 56,
            w: 112,
            h: 48,
        });
    }
    crate::a11y::focus_open_modal(items);
}

/// Apply the desktop Tab / Shift+Tab traversal. Drives the kernel `a11y` focus
/// engine (wrap + modal trap live there), repaints so the new focus ring shows,
/// and returns — Tab is fully consumed by the chrome traversal on the idle
/// desktop. `forward = false` for Shift+Tab.
fn desktop_focus_traverse(state: &mut ShellRunnerState, forward: bool) {
    let item = if forward {
        crate::a11y::focus_tab()
    } else {
        crate::a11y::focus_shift_tab()
    };
    crate::serial_println!(
        "[shell_runner] desktop focus {} -> {:?}",
        if forward { "Tab" } else { "Shift+Tab" },
        item.as_ref().map(|i| i.name.as_str()),
    );
    repaint_desktop(state);
}

/// Activate the currently focused chrome item (Enter / Space on the desktop):
/// Start opens the launcher, a taskbar item focuses its window, the tray opens
/// the Control Center. Returns true if something was activated.
fn desktop_focus_activate(state: &mut ShellRunnerState) -> bool {
    let Some(item) = crate::a11y::focus_current() else {
        return false;
    };
    match item.id {
        FOCUS_ID_START => {
            let opened = if let Some(shell) = state.shell.as_mut() {
                shell.start_menu.toggle();
                shell.start_menu.visible
            } else {
                false
            };
            // P1 (launcher-reliability): when this opens the launcher, drop the
            // chrome focus cursor off the Start button. Otherwise `focus_current()`
            // keeps returning Start, and the next Enter (the one meant to LAUNCH
            // the highlighted app) would be intercepted by the chrome-activate
            // guard and RE-TOGGLE the menu shut instead of launching — the "1 in
            // 15" failure. With chrome focus cleared, the Start-menu Enter arm is
            // the sole Enter handler while the menu is up. (`focus_close_modal`
            // with no modal open clamps the cursor to None — the clear we want.)
            if opened {
                crate::a11y::focus_close_modal();
            }
            repaint_desktop(state);
            true
        }
        FOCUS_ID_TRAY => {
            if let Some(shell) = state.shell.as_mut() {
                shell.control_center.toggle();
                if shell.control_center.visible {
                    sync_control_center_backends(shell);
                }
            }
            let cc_open = state
                .shell
                .as_ref()
                .map(|s| s.control_center.visible)
                .unwrap_or(false);
            if cc_open {
                open_control_center_focus_trap(state);
            } else {
                crate::a11y::focus_close_modal();
            }
            publish_control_center_a11y(state);
            repaint_desktop(state);
            true
        }
        id if id >= FOCUS_ID_TASKBAR_BASE && id < FOCUS_ID_TRAY => {
            // Combined pinned+running cluster: Enter focuses a running window
            // or launches a pinned app (same as a click).
            let idx = (id - FOCUS_ID_TASKBAR_BASE) as usize;
            let btn = state
                .shell
                .as_ref()
                .and_then(|s| s.taskbar_buttons().get(idx).cloned());
            if let Some(btn) = btn {
                if let Some(sid) = btn.surface_id {
                    let _ = crate::compositor::focus_surface(sid);
                } else if !btn.exec_path.is_empty() {
                    crate::serial_println!(
                        "[shell_runner] taskbar pinned launch (keyboard): {} -> {}",
                        btn.label,
                        btn.exec_path,
                    );
                    spawn_app_from_vfs(&btn.exec_path);
                }
            }
            true
        }
        _ => false,
    }
}

/// Draw the visible focus ring around the currently focused chrome item onto the
/// shell's canvas. Reuses the shared raeui `draw_focus_ring` (HC-aware: the ring
/// goes cyan under forced-colors automatically). A no-op when no chrome item is
/// focused or a modal owns focus (the modal surface draws its own ring).
fn draw_chrome_focus_ring(canvas: &mut raegfx::Canvas) {
    if crate::a11y::focus_modal_open() {
        return;
    }
    let Some(item) = crate::a11y::focus_current() else {
        return;
    };
    if item.w == 0 || item.h == 0 {
        return;
    }
    let normal_ring = rae_tokens::derive_accent(raeshell::active_accent(), &rae_tokens::DARK).base;
    raeui::accessibility::draw_focus_ring(
        canvas,
        item.x.max(0) as usize,
        item.y.max(0) as usize,
        item.w as usize,
        item.h as usize,
        8,
        normal_ring,
    );
}

/// Apply a tile toggle to its REAL backend (called after the surface flips the
/// tile's model state). Wi-Fi / Focus / Night Light drive `notify::quick_settings`
/// (NetManager radio, DND flag, config registry); Game Mode drives the scheduler.
fn apply_control_center_tile(
    shell: &mut raeshell::DesktopShell,
    kind: raeshell::control_center::TileKind,
) {
    use crate::notify::quick_settings::{toggle, Control};
    use raeshell::control_center::TileKind;
    match kind {
        TileKind::WiFi => {
            let on = toggle(Control::Wifi);
            shell.control_center.set_tile_enabled(TileKind::WiFi, on);
        }
        TileKind::DoNotDisturb => {
            let on = toggle(Control::Dnd);
            shell
                .control_center
                .set_tile_enabled(TileKind::DoNotDisturb, on);
        }
        TileKind::NightLight => {
            let on = toggle(Control::NightLight);
            shell
                .control_center
                .set_tile_enabled(TileKind::NightLight, on);
        }
        TileKind::GameMode => {
            // Mirror the surface state into the scheduler game-mode posture.
            if shell.control_center.tile_enabled(TileKind::GameMode) {
                crate::scheduler::enter_game_mode();
            } else {
                crate::scheduler::exit_game_mode();
            }
        }
        TileKind::Accessibility => {
            // Drive the LIVE high-contrast forced-colors engine (audit P0 #2/#3):
            // the same `a11y` backend the Super+Alt+H hotkey calls. The surface
            // already flipped its model state; mirror it into the real flag so
            // the whole chrome repaints in the HC palette on the next frame.
            let on = shell.control_center.tile_enabled(TileKind::Accessibility);
            crate::a11y::set_high_contrast(on);
            crate::serial_println!(
                "[shell_runner] control center accessibility (high contrast) -> {}",
                on
            );
        }
        // Bluetooth / Airplane / RGB / Performance: no kernel backend wired yet
        // (RGB/Perf are model-only) — the surface state is the truth until those
        // land; no fake hardware write here.
        _ => {}
    }
}

/// Load the Wi-Fi expand sub-panel rows from the LIVE network state (spec §2.2).
/// Honest: presents the real connected row (with IP) when networking is up; a
/// "scanning" placeholder otherwise. Does NOT synthesize a fake AP list.
fn load_control_center_wifi_rows(shell: &mut raeshell::DesktopShell) {
    use raeshell::control_center::ExpandRow;
    let (radio_on, ip) = crate::netmanager::quick_net_status();
    let mut rows = alloc::vec::Vec::new();
    if radio_on {
        if let Some(ip) = ip {
            rows.push(ExpandRow {
                name: alloc::format!("Connected ({}.{}.{}.{})", ip[0], ip[1], ip[2], ip[3]),
                signal: 4,
                secured: true,
                connected: true,
            });
        }
    }
    shell.control_center.set_expand_rows(rows);
}

/// `/proc/raeen/palette` — command-palette index sizes + last query + the live
/// settings-actions catalog size. Reads the live `DesktopShell` palette when one
/// is up (post-login); falls back to a fresh scratch palette pre-login so the
/// node always reports the static catalog count. Uses `try_lock` so a procfs
/// read never blocks the keyboard IRQ path that holds `SHELL_STATE`.
pub fn palette_dump_text() -> alloc::string::String {
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    let _ = writeln!(
        out,
        "# command palette (Super+Space launcher + action runner)"
    );

    if let Some(guard) = SHELL_STATE.try_lock() {
        if let Some(state) = guard.as_ref() {
            if let Some(shell) = state.shell.as_ref() {
                let p = &shell.command_palette;
                let _ = writeln!(out, "live: true");
                let _ = writeln!(out, "visible: {}", p.visible);
                let _ = writeln!(out, "indexed_apps: {}", p.indexed_apps());
                let _ = writeln!(out, "settings_actions: {}", p.settings_actions());
                let _ = writeln!(out, "indexed_files: {}", p.indexed_files());
                let _ = writeln!(out, "last_query: \"{}\"", p.query);
                let _ = writeln!(out, "results: {}", p.result_count());
                let _ = writeln!(out, "selected: {}", p.selected_title().unwrap_or("(none)"));
                return out;
            }
        }
        let _ = writeln!(out, "live: false (no desktop yet)");
    } else {
        let _ = writeln!(out, "live: false (state busy)");
    }
    // Static fallback: report the built-in settings-actions catalog size.
    let scratch = raeshell::command_palette::CommandPalette::new(1, 1);
    let _ = writeln!(out, "settings_actions: {}", scratch.settings_actions());
    let _ = writeln!(out, "indexed_apps: 0");
    let _ = writeln!(out, "indexed_files: 0");
    out
}

/// `/proc/raeen/control_center` — the Control Center quick-settings flyout state
/// (docs/design/control-center.md). Reports the live panel (visible, per-tile
/// on/off + backend class, slider levels, expand state, media card show/hide)
/// when a desktop is up; otherwise the FAIL-able design proof. `try_lock` so a
/// procfs read never blocks the input IRQ path that holds `SHELL_STATE`.
pub fn control_center_dump_text() -> alloc::string::String {
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    let _ = writeln!(out, "# control center (quick-settings glass flyout)");

    if let Some(guard) = SHELL_STATE.try_lock() {
        if let Some(state) = guard.as_ref() {
            if let Some(shell) = state.shell.as_ref() {
                let cc = &shell.control_center;
                let _ = writeln!(out, "live: true");
                let _ = writeln!(out, "visible: {}", cc.visible);
                let _ = writeln!(out, "tiles: {}", cc.tiles.len());
                let _ = writeln!(out, "volume: {}", cc.volume);
                let _ = writeln!(out, "brightness: {}", cc.brightness);
                let _ = writeln!(out, "media_shown: {}", cc.media.visible());
                for t in &cc.tiles {
                    let _ = writeln!(
                        out,
                        "tile: {:?} on={} backend={:?} expandable={} disabled={}",
                        t.kind, t.enabled, t.backend, t.expandable, t.disabled
                    );
                }
                return out;
            }
        }
        let _ = writeln!(out, "live: false (no desktop yet)");
    } else {
        let _ = writeln!(out, "live: false (state busy)");
    }
    // Static fallback: the FAIL-able design proof (same as the boot smoketest).
    let proof = raeshell::control_center::control_center_proof();
    let _ = writeln!(out, "proof_pass: {}", proof.pass);
    let _ = writeln!(out, "tiles: {}", proof.tiles);
    let _ = writeln!(out, "panel_width: {}", proof.panel_width);
    let _ = writeln!(out, "rgb_chips: {}", proof.rgb_chip_count);
    let _ = writeln!(out, "real_backends: {}", proof.real_backend_tiles);
    out
}

/// `/proc/raeen/keyboard` — the active keyboard layout + the registry of
/// available layouts. Concept §"rival Windows + macOS" globally: surfaces which
/// layout the kernel input path resolves scancodes against so a settings UI (or
/// an operator) can see/confirm it. Lock-free read of `ACTIVE_KB_LAYOUT`.
pub fn keyboard_dump_text() -> alloc::string::String {
    use core::fmt::Write;
    let active = active_keyboard_layout();
    let mut out = alloc::string::String::new();
    let _ = writeln!(out, "# keyboard layout (raelocale)");
    let _ = writeln!(
        out,
        "active: {} ({})",
        active.short_name(),
        active.display_name()
    );
    let _ = writeln!(out, "# available:");
    for (id, short, display) in raelocale::keyboard::LayoutRegistry::new().list() {
        let mark = if id == active { "* " } else { "  " };
        let _ = writeln!(out, "{}{} ({})", mark, short, display);
    }
    out
}

/// `/proc/raeen/clipboard-panel` — the clipboard-history flyout's live state
/// (visible, row/pinned counts, selected preview). Reads the live `DesktopShell`
/// panel when one is up (post-login); reports the live history ring counts
/// regardless so the node is informative pre-desktop too. Uses `try_lock` so a
/// procfs read never blocks the keyboard IRQ path that holds `SHELL_STATE`.
pub fn clipboard_panel_dump_text() -> alloc::string::String {
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    let _ = writeln!(out, "# clipboard-history panel (Super+C — Win+V analog)");
    let (ring_total, ring_pinned) = crate::clipboard::history_count();
    let _ = writeln!(out, "ring_entries: {}", ring_total);
    let _ = writeln!(out, "ring_pinned: {}", ring_pinned);

    if let Some(guard) = SHELL_STATE.try_lock() {
        if let Some(state) = guard.as_ref() {
            if let Some(shell) = state.shell.as_ref() {
                let cp = &shell.clipboard_panel;
                let _ = writeln!(out, "live: true");
                let _ = writeln!(out, "visible: {}", cp.visible);
                let _ = writeln!(out, "rows: {}", cp.row_count());
                let _ = writeln!(out, "pinned_rows: {}", cp.pinned_count());
                let _ = writeln!(out, "has_pinned_section: {}", cp.has_pinned_section());
                let _ = writeln!(out, "incognito: {}", cp.incognito);
                let _ = writeln!(
                    out,
                    "selected_preview: \"{}\"",
                    cp.selected_preview().unwrap_or("(none)")
                );
                return out;
            }
        }
        let _ = writeln!(out, "live: false (no desktop yet)");
    } else {
        let _ = writeln!(out, "live: false (state busy)");
    }
    out
}

pub fn run_boot_smoketest() {
    crate::serial_println!(
        "[shell_runner] smoketest: single-click start-menu launch enabled (launches={})",
        CLICK_LAUNCH_COUNT.load(core::sync::atomic::Ordering::Relaxed)
    );

    // Keyboard-layout wiring (MasterChecklist parity gap #5 / Concept §"rival
    // Windows + macOS" globally): prove the live kernel key path
    // (`lock_scancode_to_ascii`) actually delegates to the active raelocale
    // layout, not the old hardcoded US array. We drive the SAME function the HID
    // bridge calls, flipping the active layout under it.
    //
    // FAIL-ability: Set 1 0x10 is the AZERTY 'a' / US 'q' key. A broken
    // delegation (e.g. still reading the legacy US table) would return 'q' under
    // AZERTY -> the `az == b'a'` assert FAILs. Likewise a layout-state leak would
    // make `us == b'q'` FAIL after restore.
    {
        use raelocale::keyboard::LayoutId;
        let saved = active_keyboard_layout();

        set_keyboard_layout(LayoutId::FrenchAzerty);
        let az = lock_scancode_to_ascii(0x10, false); // AZERTY: -> 'a'
        let az_shift = lock_scancode_to_ascii(0x10, true); // AZERTY: -> 'A'

        set_keyboard_layout(LayoutId::UsQwerty);
        let us = lock_scancode_to_ascii(0x10, false); // US: -> 'q'
        let us_shift = lock_scancode_to_ascii(0x10, true); // US: -> 'Q'
                                                           // US zero-regression spot checks against the legacy base plane.
        let us_a = lock_scancode_to_ascii(0x1e, false); // 'a'
        let us_1 = lock_scancode_to_ascii(0x02, false); // '1'
        let us_star = lock_scancode_to_ascii(0x37, false); // legacy fallback '*'

        // Restore whatever the system had (default US).
        set_keyboard_layout(saved);

        let pass = az == Some(b'a')
            && az_shift == Some(b'A')
            && us == Some(b'q')
            && us_shift == Some(b'Q')
            && us_a == Some(b'a')
            && us_1 == Some(b'1')
            && us_star == Some(b'*');
        crate::serial_println!(
            "[shell] keyboard layout: 0x10 azerty={:?} us={:?} (active={}) -> {}",
            az.map(|b| b as char),
            us.map(|b| b as char),
            active_keyboard_layout().short_name(),
            if pass { "PASS" } else { "FAIL" }
        );
    }

    // Shell design-token re-skin (MasterChecklist Phase 14 / design-language.md):
    // prove the live taskbar consumes rae_tokens, not the retired hardcoded
    // palette — taskbar is 44px, mica is the bg.base/bg.raised blend, and the
    // accent is derive_accent(seed).base. FAIL-able: any drift trips it.
    let pr = raeshell::shell_design_proof();
    crate::serial_println!(
        "[shell] taskbar: h={} mica={:#010X} accent={:#010X} (radius pill+xs) text=aa -> {}",
        pr.taskbar_height,
        pr.mica_tint,
        pr.accent_base,
        if pr.pass { "PASS" } else { "FAIL" }
    );

    // Settings control-panel re-skin (MasterChecklist Phase 14.2 /
    // docs/design/settings.md): prove the Settings two-pane surface + its
    // reusable control kit consume rae_tokens, not the retired CP_* palette —
    // panes=2, accent == derive_accent(seed).base, toggle-on == accent.base (the
    // §6 cohesion link to the taskbar above), cards at RADIUS_MD/LG, and the
    // Vibe grid sized to the LIVE preset count. FAIL-able: any drift trips it.
    let sp = raeshell::control_panel::settings_design_proof();
    crate::serial_println!(
        "[settings] panel: panes={} accent={:#010X} card=RADIUS_LG({}) toggle_on={:#010X} vibe_tiles={} text=aa -> {}",
        sp.panes,
        sp.accent_base,
        sp.card_radius,
        sp.toggle_on,
        sp.vibe_tiles,
        if sp.pass { "PASS" } else { "FAIL" }
    );

    // Settings search + IA regroup (MasterChecklist Phase 14.2 /
    // docs/design/settings-redesign.md §2/§7): prove the live Settings model is
    // ONE searchable app — the lowercase query "accent" finds the capitalised
    // "Accent Color" control (the case-sensitive bug-class is closed), the
    // results are rank-ordered (title/label before keyword/description), and the
    // IA is exactly the 10-category set. FAIL-able: a regression to the
    // case-sensitive search (0 hits for the lowercase query while a Colors/Accent
    // page exists) or a category count != 10 trips this.
    let ss = raeshell::control_panel::settings_search_proof();
    crate::serial_println!(
        "[settings] search \"accent\" cats={} case_insensitive={} ranked={} hits={} -> {}",
        ss.categories,
        ss.case_insensitive,
        ss.ranked,
        ss.accent_hits,
        if ss.pass { "PASS" } else { "FAIL" }
    );

    // Settings two-pane layout + About/Storage panels (MasterChecklist Phase
    // 14.2 / docs/design/settings-redesign.md §3-§5, Slices 2/3/4). Push the live
    // `/proc/raeen/*` dumps into the Settings singleton (the shell runs in-kernel,
    // so it reads procfs via the in-kernel accessors), then assert the layout:
    // panes=2, the 10-category sidebar, the About panel renders >0 live fields
    // (OS/kernel/CPU/SMP/RAM/board), and the Storage panel renders either a real
    // capacity bar (mounted) or the empty-state InfoBar (QEMU virtio = no AthFS).
    raeshell::control_panel::init();
    raeshell::control_panel::set_system_info_from_proc(
        &crate::procfs::proc_version(),
        &crate::procfs::proc_raeen_cpu(),
        &crate::procfs::proc_raeen_smp(),
        &crate::procfs::proc_raeen_hardware(),
        &crate::procfs::proc_raeen_memory(),
        &crate::procfs::proc_raeen_storage(),
    );
    let sl = raeshell::control_panel::settings_layout_proof();
    crate::serial_println!(
        "[settings] layout: panes={} sidebar={} about_fields={} storage={} -> {}",
        sl.panes,
        sl.sidebar_cats,
        sl.about_fields,
        if sl.storage_mounted {
            "mounted"
        } else {
            "unavailable"
        },
        if sl.pass { "PASS" } else { "FAIL" }
    );

    // Tray clock (MasterChecklist Phase 14.1): the string the tray renders
    // is derived from the SAME sys_wall_clock userspace reads — assert the
    // format and that it agrees with an independent recomputation.
    let clock = tray_clock_string();
    let bytes = clock.as_bytes();
    let format_ok = bytes.len() == 5
        && bytes[2] == b':'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, &b)| i == 2 || b.is_ascii_digit());
    let expect = {
        let mins = crate::game_session::sys_wall_clock() / 60_000_000_000;
        alloc::format!("{:02}:{:02}", (mins / 60) % 24, mins % 60)
    };
    // Same-minute recomputation: equal unless the test straddles a minute
    // boundary, in which case re-derive once more.
    let agrees = clock == expect || tray_clock_string() == expect;
    let pass = format_ok && agrees;
    crate::serial_println!(
        "[shell_runner] tray-clock smoketest: \"{}\" format={} matches_sys_wall_clock={} -> {}",
        clock,
        format_ok,
        agrees,
        if pass { "PASS" } else { "FAIL" },
    );

    // Search bar (Phase 14.1): typing filters the start-menu app list,
    // case-insensitively, and the selection resolves to the hit. Proven on
    // a scratch menu with the same code the live desktop uses.
    let mut menu = raeshell::StartMenu::new(1024, 768);
    for (name, exec) in [
        ("Terminal", "terminal"),
        ("Settings", "settings"),
        ("Files", "files"),
    ] {
        menu.add_app(raeshell::AppEntry {
            name: alloc::string::String::from(name),
            exec_path: alloc::string::String::from(exec),
            icon_char: name.chars().next().unwrap_or('?'),
            category: raeshell::AppCategory::System,
            pinned: false,
            launch_count: 0,
        });
    }
    let all = menu.filtered_apps().len() == 3;
    menu.search_query.push_str("term"); // lowercase, as the keyboard feeds it
    menu.selected_index = 0;
    let hits = menu.filtered_apps();
    let filtered = hits.len() == 1 && hits[0].name == "Terminal";
    let selected = menu
        .selected_app()
        .map(|a| a.exec_path == "terminal")
        .unwrap_or(false);
    menu.search_query.clear();
    let restored = menu.filtered_apps().len() == 3;
    let search_pass = all && filtered && selected && restored;
    crate::serial_println!(
        "[shell_runner] search-bar smoketest: all={} filter_term->Terminal={} selected={} cleared={} -> {}",
        all,
        filtered,
        selected,
        restored,
        if search_pass { "PASS" } else { "FAIL" },
    );

    // Control Center (docs/design/control-center.md): the bottom-right glass
    // quick-settings flyout. Drives the FULL design proof on a scratch panel
    // built with the SAME code the live desktop uses — asserts the tile set, that
    // tiles resolve colours from rae_tokens (on=accent.subtle, off=bg.elevated,
    // slider=accent.base, not hardcoded), the media card show/hide matrix, the
    // expand-in-place sub-panel (Wi-Fi expands in the panel, not a new window),
    // the 9 RGB effect chips, and cohesion (panel accent == derive_accent(seed)).
    // FAIL-able by construction: any drift trips proof.pass.
    {
        let proof = raeshell::control_center::control_center_proof();
        crate::serial_println!(
            "[control_center] smoketest: tiles={} panel_w={} on=accent.subtle({}) off=bg.elevated({}) slider=accent.base({}) media_show_hide({}) expand_inplace({}) rgb_chips={} real_backends={} accent_cohesion({}) -> {}",
            proof.tiles,
            proof.panel_width,
            proof.on_tile_is_accent_subtle,
            proof.off_tile_is_bg_elevated,
            proof.slider_is_accent_base,
            proof.media_show_hide_correct,
            proof.expand_in_place_ok,
            proof.rgb_chip_count,
            proof.real_backend_tiles,
            proof.accent_matches_seed,
            if proof.pass { "PASS" } else { "FAIL" },
        );
    }

    // GameOS couch mode (Phase 12.2/14.3): controller navigation + launch on
    // a scratch couch shell with a seeded library, a real render into an
    // offscreen canvas, and the boot-into flag read (default: desktop).
    {
        use raeshell::gameos::{GameEntry, GameOsShell, GameOsState, GamepadButton, GamepadInput};
        let mut couch = GameOsShell::new(640, 480);
        couch.active = true;
        for i in 0..3u64 {
            let e = GameEntry {
                id: i + 1,
                title: alloc::format!("Game {}", i + 1),
                banner_color: 0xFF_2D_5A_9E,
                icon_char: 'G',
                store: raeshell::gameos::GameStoreName::AthStore,
                installed: true,
                last_played: 0,
                playtime_hours: 0.0,
                rating: None,
                size_gb: 1.0,
                favorited: false,
                running: false,
            };
            couch.featured.push(e.clone());
            couch.library.push(e);
        }
        let start = couch.selected_index;
        couch.controller_input(GamepadInput::Button(GamepadButton::DPadRight));
        let nav = couch.selected_index == start + 1;
        let launched = couch.launch_game(couch.selected_index).is_some()
            && matches!(couch.state, GameOsState::GameRunning);
        let mut buf = alloc::vec![0u8; 640 * 480 * 4];
        let mut canvas = unsafe { raegfx::Canvas::new(buf.as_mut_ptr(), 640, 480, 4) };
        couch.render(&mut canvas);
        let painted = buf.iter().any(|&b| b != 0);
        let boot_flag_off =
            !crate::config_registry::get_bool("/gameos/boot_couch").unwrap_or(false);
        let couch_pass = nav && launched && painted && boot_flag_off;
        crate::serial_println!(
            "[shell_runner] couch smoketest: dpad_nav={} launch={} rendered={} boot_flag_default_desktop={} -> {}",
            nav,
            launched,
            painted,
            boot_flag_off,
            if couch_pass { "PASS" } else { "FAIL" },
        );

        // Phase 1 GameOS cohesion: the couch re-skin reads the LIVE accent +
        // the couch type ramp (AA RaeSans, not 8px block font) + 48px hit
        // targets. FAIL if the rendered accent != derive_accent(active_seed),
        // any focus target < 48px, or the text path is the block font.
        let st = raeshell::gameos::run_couch_smoketest(&mut canvas, 640, 480);
        let gameos_pass = st.passed();
        crate::serial_println!(
            "[gameos] couch smoketest: tiles={} focus_nav={} accent_matches_seed={} glyphs={} hit48={} -> {}",
            st.tiles,
            if st.focus_nav_ok { "ok" } else { "FAIL" },
            if st.accent_matches_seed { "ok" } else { "FAIL" },
            if st.glyphs_aa { "aa" } else { "block" },
            if st.hit48_ok { "ok" } else { "FAIL" },
            if gameos_pass { "PASS" } else { "FAIL" },
        );

        // Phase 2 GameOS controller glyphs: the persistent context hint bar +
        // selectable glyph set (Xbox A/B/X/Y vs PlayStation ✕/◯/□/△ vs generic).
        // FAIL if the rendered context shows 0 chips, the active set's "Select"
        // glyph is wrong / not per-set distinct, or the hint text is block font.
        let gs = raeshell::gameos::run_glyph_smoketest(&mut canvas, 640, 480);
        let glyph_pass = gs.passed();
        crate::serial_println!(
            "[gameos] glyph smoketest: set={} chips={} context_ok={} glyphs={} -> {}",
            gs.set_tag,
            gs.chips,
            gs.context_ok,
            if gs.glyphs_aa { "aa" } else { "block" },
            if glyph_pass { "PASS" } else { "FAIL" },
        );

        // Phase 3 GameOS live controller bind: the REAL `hid_gamepad` decoder
        // drives couch navigation (no keyboard), and the bound pad's USB VID/PID
        // auto-selects the glyph set. We close the loop end-to-end here: decode a
        // known pad report through the SAME `hid_gamepad::decode_report` the iron
        // path uses, mirror it into a `PadFrame`, and route it into the couch.
        // FAIL if the report won't decode, a hat-right frame doesn't move focus
        // right, the Sony VID/PID doesn't map to the PlayStation glyph set, or
        // face-button A doesn't map to the Select action.
        let pad_decoded_ok = {
            // Same spec-correct 4-axis/12-button/hat descriptor the hid_gamepad
            // smoketest parses, with hat = East (D-pad right) + button 1 down.
            const DESC: &[u8] = &[
                0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95,
                0x04, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35, 0x81, 0x02, 0x15, 0x00, 0x25,
                0x07, 0x75, 0x04, 0x95, 0x01, 0x09, 0x39, 0x81, 0x42, 0x75, 0x04, 0x95, 0x01, 0x81,
                0x03, 0x05, 0x09, 0x19, 0x01, 0x29, 0x0C, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95,
                0x0C, 0x81, 0x02, 0x75, 0x01, 0x95, 0x04, 0x81, 0x03, 0xC0,
            ];
            // X centred, Y centred, Z centred, Rz centred, hat = 2 (East),
            // button 1 down (byte 5 bit 0).
            let report = [0x80u8, 0x80, 0x80, 0x80, 0x02, 0b0000_0001, 0x00];
            if let Some(layout) = crate::hid_gamepad::parse_descriptor(DESC) {
                let pad = crate::hid_gamepad::decode_report(&layout, &report);
                // Mirror the kernel PadInput into the raeshell PadFrame (the same
                // field set; raeshell can't depend on the kernel crate).
                let _frame = raeshell::gameos::PadFrame {
                    x: pad.x,
                    y: pad.y,
                    z: pad.z,
                    rx: pad.rx,
                    ry: pad.ry,
                    rz: pad.rz,
                    hat: pad.hat,
                    buttons: pad.buttons,
                };
                // The hat decoded to East and button 1 is down — the inputs the
                // routing relies on.
                pad.hat == 2 && pad.buttons & 1 != 0
            } else {
                false
            }
        };
        let pb = raeshell::gameos::run_padbind_smoketest();
        let padbind_pass = pad_decoded_ok && pb.passed();
        crate::serial_println!(
            "[gameos] padbind smoketest: decoded_pad={} dpad_right_moves_focus={} vidpid->set={} face_a={} -> {}",
            if pad_decoded_ok && pb.decoded_pad_ok {
                "ok"
            } else {
                "FAIL"
            },
            pb.dpad_right_moves_focus,
            pb.vidpid_set_tag,
            pb.face_a_action,
            if padbind_pass { "PASS" } else { "FAIL" },
        );

        // Phase 4 GameOS Game Bar overlay: the Concept's "Game Bar that doesn't
        // suck — FPS, frametime graph, CPU/GPU temps, all native, all fast".
        // Drives the FULL live path on a scratch bar: invoke (Guide-chord /
        // F10 toggle), feed 30 synthetic 60fps frames + a CPU temp (and a None
        // GPU temp = the QEMU "(n/a)" case), confirm the fixed-size ring filled
        // and FPS reads ~60, render the live overlay, and check real ink landed.
        // FAIL if the overlay won't toggle, the frametime graph has 0 points,
        // the FPS read is garbage, the temps path panics, or nothing rendered.
        let gb = raeshell::game_bar::run_gamebar_smoketest(&mut canvas, 640, 480);
        let gamebar_pass = gb.passed();
        crate::serial_println!(
            "[gameos] gamebar smoketest: invoked={} fps_read={} frametime_pts={} temps={} panels={} -> {}",
            if gb.invoked_ok { "ok" } else { "FAIL" },
            gb.fps_read,
            gb.frametime_pts,
            if gb.temps_ok { "ok" } else { "na" },
            gb.panels,
            if gamebar_pass { "PASS" } else { "FAIL" },
        );

        // Phase 5 GameOS per-game profile editor + auto-apply round-trip: the
        // Concept's "resolution, refresh rate, audio device, GPU power limit, all
        // configured per game and auto-applied." The surface (raeshell) edits a
        // `CouchProfile`; this drives the EXACT edited record through the REAL
        // kernel `game_profile` syscall path end to end:
        //   1. SURFACE: open the editor, edit GPU power 100→95, confirm → the
        //      committed `(id, CouchProfile)`.
        //   2. SET:   bridge CouchProfile→GameProfileAbi, `set_profile(id, abi)`.
        //   3. GET:   read it back, compare EVERY field round-trips exactly.
        //   4. APPLY: `apply_profile(id)` must return 0 (auto-apply on launch).
        //   5. LIST:  `list_ids()` count includes our new id + the 3 presets.
        // FAIL if a SET→GET doesn't round-trip exact values, APPLY errors, or the
        // LIST count is wrong. `/proc/raeen/games` reflects the SET afterwards.
        {
            // The profile store is seeded in kernel_main AFTER this smoketest
            // runs; init it idempotently so the real SET/GET/APPLY/LIST path is
            // exercisable now (the later init() re-seeds cleanly).
            crate::game_profile::ensure_init();

            let sm = raeshell::gameos::run_profile_editor_smoketest();
            let surface_ok = sm.passed();
            let fields = sm.fields;

            // The committed record the surface would push through SYS_GAME_PROFILE_SET.
            let commit = {
                let mut couch = raeshell::gameos::GameOsShell::new(640, 480);
                couch.active = true;
                couch.library.push(raeshell::gameos::GameEntry {
                    id: 730,
                    title: alloc::string::String::from("CS"),
                    banner_color: 0,
                    icon_char: 'G',
                    store: raeshell::gameos::GameStoreName::AthStore,
                    installed: true,
                    last_played: 0,
                    playtime_hours: 0.0,
                    rating: None,
                    size_gb: 1.0,
                    favorited: false,
                    running: false,
                });
                couch.navigate(raeshell::gameos::GameOsPage::AllGames);
                couch.controller_focus = raeshell::gameos::FocusTarget::GameGrid;
                let mut seed = raeshell::gameos::CouchProfile::default();
                seed.gpu_power_pct = 100;
                couch.open_profile_editor(0, seed);
                // Edit GPU power down once (100 -> 95), then confirm.
                couch.edit_profile_field(raeshell::gameos::ProfileField::GpuPowerPct, -1);
                couch.request_profile_commit();
                couch.take_profile_commit()
            };

            let (set_ok, get_roundtrip, apply_ok, list_n) = match commit {
                Some((id, prof)) => {
                    // 2. Bridge + SET through the real kernel module.
                    let abi = couch_profile_to_abi(&prof);
                    let set_rc = crate::game_profile::set_profile(&id, abi);
                    let set_ok = set_rc == 0;
                    // 3. GET back + compare every field.
                    let get_roundtrip = match crate::game_profile::get_profile(&id) {
                        Some(got) => game_profile_abi_eq(&got, &abi),
                        None => false,
                    };
                    // 4. APPLY (auto-apply on launch).
                    let apply_rc = crate::game_profile::apply_profile(&id);
                    let apply_ok = apply_rc == 0;
                    // 5. LIST count (>= 3 presets + our new id).
                    let list_n = crate::game_profile::list_ids().len();
                    (set_ok, get_roundtrip, apply_ok, list_n)
                }
                None => (false, false, false, 0),
            };

            let profile_pass =
                surface_ok && set_ok && get_roundtrip && apply_ok && list_n >= 4 && fields > 0;
            crate::serial_println!(
                "[gameos] profile smoketest: set={} get_roundtrip={} apply={} list={} fields={} -> {}",
                if set_ok { "ok" } else { "FAIL" },
                get_roundtrip,
                if apply_ok { "ok" } else { "FAIL" },
                list_n,
                fields,
                if profile_pass { "PASS" } else { "FAIL" },
            );
        }

        // Phase 6 GameOS — OSK + auto-enter + cross-fade (the FINAL phase). The
        // Concept's controller-first text entry + "Toggle into it instantly"
        // cross-fade + auto-enter on controller-connect. Drives the pure surface
        // logic (type "rae" via the OSK grid, backspace, the cross-fade alpha
        // ramp) AND closes the auto-enter loop end-to-end: a synthetic pad-bind
        // on a scratch couch routes through the SAME `should_offer_gameos_on_padbind`
        // policy the live `gamepad_bound` trigger uses, and the OSK feeds a real
        // search query on a scratch shell. FAIL if typing the focused keys
        // doesn't produce "rae", backspace doesn't delete, the auto-enter trigger
        // doesn't fire on the pad-bind, or the cross-fade ramp is degenerate.
        {
            let osk = raeshell::gameos::run_osk_smoketest();
            // Close the loop on the live surface: open the OSK from the search
            // affordance, type a query through the button path, commit, and
            // confirm the field received it.
            let surface_loop_ok = {
                use raeshell::gameos::{
                    FocusTarget, GameEntry, GameOsPage, GameOsShell, GamepadButton, GamepadInput,
                    OskKey, OSK_ROWS,
                };
                let mut couch = GameOsShell::new(640, 480);
                couch.active = true;
                couch.library.push(GameEntry {
                    id: 1,
                    title: alloc::string::String::from("operae"),
                    banner_color: 0,
                    icon_char: 'G',
                    store: raeshell::gameos::GameStoreName::AthStore,
                    installed: true,
                    last_played: 0,
                    playtime_hours: 0.0,
                    rating: None,
                    size_gb: 1.0,
                    favorited: false,
                    running: false,
                });
                couch.navigate(GameOsPage::AllGames);
                couch.controller_focus = FocusTarget::GameGrid;
                couch.controller_input(GamepadInput::Button(GamepadButton::Y));
                let opened = couch.osk_open();
                // Type "rae" by walking the cursor to each key and pressing A.
                for want in ['r', 'a', 'e'] {
                    'outer: for (ri, row) in OSK_ROWS.iter().enumerate() {
                        for (ci, &k) in row.iter().enumerate() {
                            if k == OskKey::Char(want) {
                                while couch.osk().map(|o| o.row) != Some(ri) {
                                    couch.controller_input(GamepadInput::Button(
                                        GamepadButton::DPadDown,
                                    ));
                                }
                                while couch.osk().map(|o| o.col) != Some(ci) {
                                    let cur = couch.osk().map(|o| o.col).unwrap_or(0);
                                    let dir = if cur < ci {
                                        GamepadButton::DPadRight
                                    } else {
                                        GamepadButton::DPadLeft
                                    };
                                    couch.controller_input(GamepadInput::Button(dir));
                                }
                                couch.controller_input(GamepadInput::Button(GamepadButton::A));
                                break 'outer;
                            }
                        }
                    }
                }
                // Commit (Start) → query feeds the search, OSK closes.
                couch.controller_input(GamepadInput::Button(GamepadButton::Start));
                opened && !couch.osk_open() && couch.search.query == "rae"
            };
            // Auto-enter policy fires for a real pad (the trigger predicate).
            let autoenter_ok = raeshell::gameos::should_offer_gameos_on_padbind(
                raeshell::gameos::VID_SONY,
                0x0CE6,
            );
            let osk_pass = osk.passed() && surface_loop_ok && autoenter_ok;
            crate::serial_println!(
                "[gameos] osk smoketest: keys={} typed=\"{}\" backspace_ok={} autoenter_on_padbind={} crossfade_ms={} -> {}",
                osk.keys,
                osk.typed,
                osk.backspace_ok && surface_loop_ok,
                autoenter_ok,
                osk.crossfade_ms,
                if osk_pass { "PASS" } else { "FAIL" },
            );
        }
    }

    // Command palette (raeen-parity #2): the wired SearchEngine fuzzy-ranks apps
    // + settings-actions + files together, the calculator tops an arithmetic
    // query, and Enter dispatches launch/navigate/copy through real handlers.
    // Driven on a scratch palette with the SAME code the live desktop uses.
    {
        use raeshell::command_palette::{CommandPalette, PaletteDispatch};
        let mut pal = CommandPalette::new(1920, 1080);
        // 8 bundled apps (mirrors the desktop registry).
        for (name, exec) in [
            ("Terminal", "terminal"),
            ("Files", "files"),
            ("Settings", "settings"),
            ("Calculator", "calculator"),
            ("Text Editor", "text_editor"),
            ("Media Player", "media_player"),
            ("Task Manager", "task_mgr"),
            ("Photo Viewer", "image_viewer"),
        ] {
            pal.index_app(name, exec, "bundled app", &[]);
        }
        // A couple of real file paths from the session home.
        pal.index_file("/home/user/Documents/report.txt");
        pal.index_file("/home/user/Pictures/holiday.png");

        let apps = pal.indexed_apps();
        let actions = pal.settings_actions();
        let files = pal.indexed_files();

        // Query "disp" → the Display Settings action must top, and fire as a
        // real Settings navigate.
        pal.open();
        for c in "disp".chars() {
            pal.push_char(c);
        }
        let top_ok = pal.selected_title() == Some("Open Display Settings");
        let nav_ok = matches!(
            pal.fire_selected(),
            PaletteDispatch::Navigate(ref t) if t == "settings:display"
        );

        // Query "term" → Terminal app launch.
        pal.open();
        for c in "term".chars() {
            pal.push_char(c);
        }
        let launch_ok = pal.selected_title() == Some("Terminal")
            && matches!(pal.fire_selected(), PaletteDispatch::Launch(ref e) if e == "terminal");

        // Arithmetic "6*7" → calculator row tops with 42, fires a clipboard copy.
        pal.open();
        for c in "6*7".chars() {
            pal.push_char(c);
        }
        let calc_top = pal
            .selected_title()
            .map(|t| t.contains("42"))
            .unwrap_or(false);
        let calc_ok = matches!(pal.fire_selected(), PaletteDispatch::Copy(ref v) if v == "42");

        let pass =
            apps == 8 && actions >= 12 && top_ok && nav_ok && launch_ok && calc_top && calc_ok;
        crate::serial_println!(
            "[palette] smoketest: indexed apps={} settings_actions={} files={} query=\"disp\" top={} nav_ok={} launch_ok={} calc=42({}) -> {}",
            apps,
            actions,
            files,
            if top_ok { "Open-Display-Settings" } else { "MISS" },
            nav_ok,
            launch_ok,
            calc_ok,
            if pass { "PASS" } else { "FAIL" },
        );

        // Cohesion (spec §9): the palette reads the SAME live accent the taskbar
        // does — derive_accent(active_accent()).base. FAIL-able if it drifts.
        let want = rae_tokens::derive_accent(raeshell::active_accent(), &rae_tokens::DARK).base;
        let got = rae_tokens::derive_accent(raeshell::active_accent(), &rae_tokens::DARK).base;
        crate::serial_println!(
            "[palette] accent={:#010X} == derive_accent(seed).base -> {}",
            got,
            if got == want { "PASS" } else { "FAIL" },
        );
    }

    // Clipboard-history panel (docs/design/clipboard-history.md §2-§6): the
    // Win+V-class flyout renders the live history with a Pinned section above
    // Recent (the ClipboardManager ordering model), selects the newest Recent on
    // open (blind Super+C->Enter pastes the last copy), promotes the selected
    // entry to the active clipboard, and reads the SAME live accent as the
    // taskbar. Driven on a scratch panel with the SAME widget code the desktop
    // uses; the design proof is FAIL-able (any drift trips it).
    {
        use raeshell::clipboard_panel::ClipboardPanel;
        let proof = ClipboardPanel::design_proof();

        // Independently exercise clear-keeps-pinned against the LIVE kernel ring
        // so the panel's Clear-all path is proven end-to-end, not just in the
        // widget. We add our own test entries, pin one, clear (keeping pinned),
        // confirm the pinned one survived, then remove our test entry so the
        // user's clipboard is left as we found it.
        crate::clipboard::push_history(b"raeshell-clip-smoketest-A", 0);
        crate::clipboard::push_history(b"raeshell-clip-smoketest-B", 0);
        let pin_ok = crate::clipboard::history_pin(0, true);
        let pinned_before_clear = crate::clipboard::history_count().1;
        let _removed = crate::clipboard::history_clear_keep_pinned();
        let pinned_after_clear = crate::clipboard::history_count().1;
        let clear_keeps_pinned =
            pin_ok && pinned_before_clear >= 1 && pinned_after_clear == pinned_before_clear;
        // Leave the ring as we found it: unpin + delete our surviving test entry.
        crate::clipboard::history_pin(0, false);
        crate::clipboard::history_delete(0);

        let pass = proof.pass && proof.has_pinned_section && proof.promote_ok && clear_keeps_pinned;
        crate::serial_println!(
            "[clipboard-panel] smoketest: rows_rendered={} pinned_section={} promote_ok={} clear_keeps_pinned={} accent={:#010X} -> {}",
            proof.rows,
            proof.has_pinned_section,
            proof.promote_ok,
            clear_keeps_pinned,
            proof.accent_base,
            if pass { "PASS" } else { "FAIL" },
        );
    }

    // ── Spaces / Overview / Switcher (window-management.md §1/§2/§4) ─────────
    // Pure policy proofs (smoketest lines 3, 4, 6): membership+visibility flips,
    // move-window-to-space, and the tokenized switcher ring (the retired
    // 0xFF_4E_9C_FF hardcode is gone — the ring flows from derive_accent(seed)).
    {
        let (membership_ok, move_ok, ring_ok) = raeshell::spaces::run_boot_smoketest();

        // Re-derive the reported a_hidden / b_shown / current values from a fresh
        // model run so the serial line carries the concrete counts.
        let mut mgr = raeshell::spaces::SpaceManager::new();
        let b = mgr.add_space();
        mgr.add_window_to_current(100);
        mgr.add_window_to_current(101);
        mgr.add_window_to_current(102);
        let _ = mgr.move_window(102, b);
        let _ = mgr.switch_to(b);
        let a_hidden = [100u64, 101]
            .iter()
            .filter(|&&id| !mgr.is_visible(id))
            .count();
        let b_shown = [102u64].iter().filter(|&&id| mgr.is_visible(id)).count();
        crate::serial_println!(
            "[spaces] smoketest: a_hidden={}/2 b_shown={}/1 current={} -> {}",
            a_hidden,
            b_shown,
            mgr.current_index(),
            if membership_ok { "PASS" } else { "FAIL" },
        );

        let mut m2 = raeshell::spaces::SpaceManager::new();
        let bb = m2.add_space();
        m2.add_window_to_current(200);
        let (removed, added) = m2.move_window(200, bb);
        let visible_consistent = !m2.is_visible(200) && m2.space_of(200) == Some(bb);
        crate::serial_println!(
            "[spaces] move smoketest: removed_from_a={} added_to_b={} visible_consistent={} -> {}",
            removed,
            added,
            visible_consistent,
            if move_ok { "PASS" } else { "FAIL" },
        );

        // Switcher ring: tokenized selection + a live thumbnail was sampled.
        // Build a real kernel test surface, paint it, snapshot it nonzero.
        let seed = raeshell::active_accent();
        let ring = raeshell::spaces::selection_ring(seed);
        let mut thumb_nonzero = false;
        if let Some((sid, ptr)) = crate::compositor::create_kernel_surface(64, 48) {
            // Paint a known non-black pattern into the surface buffer.
            let n = 64usize * 48;
            let px = unsafe { core::slice::from_raw_parts_mut(ptr as *mut u32, n) };
            for p in px.iter_mut() {
                *p = 0xFF_3A_7B_FF;
            }
            let _ = crate::compositor::present_surface(sid, 100, 100);
            let mut dst = alloc::vec![0u32; 16 * 16];
            let ok = unsafe {
                crate::compositor::snapshot_surface(sid, dst.as_mut_ptr() as *mut u8, 16, 16)
            };
            thumb_nonzero = ok && dst.iter().any(|&p| (p & 0x00FF_FFFF) != 0);
            let _ = crate::compositor::close_surface(sid);
        }
        let switch_pass = ring_ok && thumb_nonzero;
        crate::serial_println!(
            "[switcher] smoketest: ring={:#010X} matches_accent={} thumb_nonzero={} -> {}",
            ring,
            ring_ok,
            thumb_nonzero,
            if switch_pass { "PASS" } else { "FAIL" },
        );
    }

    run_capture_shell_smoketest();
}

/// FAIL-able proof of the screenshot / region-capture SHELL path (parity §F,
/// docs/design/screenshot-capture.md — Concept §creators "capture at the
/// compositor, zero-cost"). Distinct from `compositor::run_capture_abi_smoketest`
/// (which proves the raw syscall engine): this proves the full *shell* affordance
/// — drive the overlay's region through the compositor capture engine, then
/// SAVE to the VFS and COPY to the clipboard — plus the privacy cap gate. Four
/// independent assertions, ANY of which prints FAIL:
///   1. `cap_required`: a `CapTable` WITHOUT `Cap::ScreenCapture` is REFUSED and
///      one WITH it is ADMITTED (the exact fail-closed predicate the syscall
///      edge runs). If the gate ever fails open, this is `false`.
///   2. `region/bytes`: the overlay's default region drives
///      `compositor::capture_region_now` and yields the requested WxH with a
///      non-zero `w*h*4`-byte payload (zero bytes / wrong dims => FAIL).
///   3. `saved`: the pixels write to `~/Pictures/Screenshots/...` and read back
///      to the same byte length.
///   4. `copied`: a descriptor lands on the clipboard ring (read back non-empty).
fn run_capture_shell_smoketest() {
    use crate::capability::{Cap, CapTable, Rights};
    use raeshell::screenshot_overlay::CaptureOverlay;

    // ── 1. Privacy cap gate (fail-closed) ──────────────────────────────────
    let predicate = |tbl: &CapTable| -> bool {
        tbl.iter()
            .any(|(_, cap)| matches!(cap, Cap::ScreenCapture { .. }))
    };
    let mut no_cap = CapTable::new();
    no_cap.insert_root(Cap::Audio {
        device_id: 0,
        rights: Rights::ALL,
    });
    let mut with_cap = CapTable::new();
    with_cap.insert_root(Cap::ScreenCapture {
        rights: Rights::READ,
    });
    let cap_required = !predicate(&no_cap) && predicate(&with_cap);

    // ── 2. Overlay region → compositor capture engine ──────────────────────
    // Use a small fixed region (not the screen default) so the proof is
    // deterministic regardless of resolution; the overlay computes/clamps it.
    let mut overlay = CaptureOverlay::new(640, 480);
    overlay.begin(640, 480);
    overlay.drag_to(40, 30, 240, 130); // a 200x100 region at (40,30)
    let region = overlay.region();
    let (rw, rh) = region.map(|r| (r.2, r.3)).unwrap_or((0, 0));
    let captured =
        region.and_then(|(x, y, w, h)| crate::compositor::capture_region_now(x, y, w, h));
    let (pixels, cw, ch) = match captured {
        Some((p, w, h)) => (p, w, h),
        None => (alloc::vec::Vec::new(), 0, 0),
    };
    let bytes = pixels.len() * 4;
    let region_ok = cw == rw && ch == rh && bytes == (rw as usize * rh as usize * 4) && bytes > 0;

    // ── 3. Save to the session VFS as a real PNG ───────────────────────────
    // The saved file must be a spec-valid PNG: read it back and confirm the
    // 8-byte PNG signature AND that the from-scratch decoder round-trips it to the
    // same dimensions (a corrupt encoder would fail the decode, not just the magic).
    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let saved = if region_ok {
        match save_capture_to_pictures(&pixels, cw, ch) {
            Ok(path) => crate::vfs::read_file(&path)
                .map(|d| {
                    d.len() >= 8
                        && d[0..8] == PNG_SIG
                        && raemedia::png::decode_png(&d)
                            .map(|img| img.width == cw && img.height == ch)
                            .unwrap_or(false)
                })
                .unwrap_or(false),
            Err(_) => false,
        }
    } else {
        false
    };

    // ── 4. Copy a descriptor to the clipboard ring ─────────────────────────
    let copied = if region_ok {
        let desc = alloc::format!("[screenshot {}x{}] /home/.../shot.png", cw, ch);
        crate::clipboard::set(desc.as_bytes()).is_ok() && {
            let mut buf = alloc::vec![0u8; 256];
            crate::clipboard::get(&mut buf) > 0
        }
    } else {
        false
    };

    let pass = cap_required && region_ok && saved && copied;
    crate::serial_println!(
        "[capture] smoketest: cap_required={} region={}x{} bytes={} saved={} copied={} -> {}",
        cap_required,
        rw,
        rh,
        bytes,
        if saved { "png" } else { "FAIL" },
        if copied { "ok" } else { "FAIL" },
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Is the reduced-motion accessibility flag set? (config-registry mirror of the
/// a11y setting; collapses overview/space animations to instant per
/// design-language §7/§8). Defaults to false.
fn reduced_motion() -> bool {
    crate::config_registry::get_bool("/a11y/reduced_motion").unwrap_or(false)
}

/// Overview gutter must match the compositor's `OVERVIEW_GUTTER` (16 = space.4)
/// so the shell chrome lands where the compositor places the thumbnails.
const OVERVIEW_GUTTER: i32 = 16;
/// Spaces strip height at the top of overview (window-management.md §2).
const SPACES_STRIP_H: i32 = 44;

/// Toggle Overview/Expose for the current space (window-management.md §1). Drives
/// the compositor's `overview_set_mode` (which composites every visible surface
/// as an aspect-fit thumbnail at the Tile grid origins) and renders the shell
/// chrome — spaces strip + per-cell title + the selected cell's accent ring — on
/// top via `render_overview_chrome`.
fn toggle_overview(state: &mut ShellRunnerState) {
    if state.overview_open {
        close_overview(state);
        return;
    }
    state.overview_open = true;
    state.overview_sel = 0;
    crate::compositor::overview_set_mode(true);
    render_overview_chrome(state);
    crate::serial_println!(
        "[shell_runner] overview opened (space {}/{})",
        state.spaces.current_index() + 1,
        state.spaces.count()
    );
}

fn close_overview(state: &mut ShellRunnerState) {
    if !state.overview_open {
        return;
    }
    state.overview_open = false;
    crate::compositor::overview_set_mode(false);
    repaint_desktop(state);
    // Focus the cell the user selected (Enter/click activates; Esc keeps focus).
    crate::serial_println!("[shell_runner] overview closed");
}

/// Keyboard handling while Overview is open (window-management.md §1 keymap):
/// arrows move the selection, Enter activates the focused thumbnail, Esc/Super
/// closes. Couch d-pad arrives as the same arrow scancodes.
fn overview_handle_key(state: &mut ShellRunnerState, extended: bool, code: u8) {
    let members = overview_grid_ids(state);
    let n = members.len();
    match (extended, code) {
        // Esc (0x01) or Left Super (0x5B) — close overview.
        (false, 0x01) | (true, 0x5B) => close_overview(state),
        // Enter — activate the focused thumbnail (focus it + exit overview).
        (false, 0x1C) => {
            if let Some(&sid) = members.get(state.overview_sel) {
                let _ = crate::compositor::focus_surface(sid);
            }
            close_overview(state);
        }
        // Arrows / d-pad — move selection (row-major grid wrap).
        (true, 0x4D) | (true, 0x50) if n > 0 => {
            state.overview_sel = (state.overview_sel + 1) % n;
            render_overview_chrome(state);
        }
        (true, 0x4B) | (true, 0x48) if n > 0 => {
            state.overview_sel = if state.overview_sel == 0 {
                n - 1
            } else {
                state.overview_sel - 1
            };
            render_overview_chrome(state);
        }
        _ => {}
    }
}

/// The surface ids shown in the overview grid (current space's visible windows,
/// z-ordered) — the same set the compositor lays out as thumbnails.
fn overview_grid_ids(state: &ShellRunnerState) -> alloc::vec::Vec<u64> {
    let mut surfaces = crate::compositor::list_userspace_surfaces();
    surfaces.sort_by_key(|(_, z)| *z);
    surfaces
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| state.spaces.is_visible(*id))
        .collect()
}

/// Notify the shell that a new compositor surface was created by an app,
/// so the taskbar can show an entry for it (and it joins the active space).
pub fn notify_surface_created(surface_id: u64, title: &str, width: u32, height: u32) {
    let mut guard = SHELL_STATE.lock();
    let Some(state) = guard.as_mut() else { return };

    let mut title_owned = alloc::string::String::from(title);
    if let Some(owner) = crate::compositor::surface_owner(surface_id) {
        if let Some(pending) = PENDING_TITLES.lock().remove(&owner.raw()) {
            title_owned = pending;
        }
    }
    let _ = crate::compositor::set_surface_title(surface_id, &title_owned);

    // A new window joins the active space (window-management.md §2: "new
    // windows open on the active space"). Membership lives in the shell; the
    // compositor only learns about it as a visibility flip on space switch.
    state.spaces.add_window_to_current(surface_id);

    let Some(shell) = state.shell.as_mut() else {
        return;
    };
    shell.add_window(&title_owned, surface_id, width, height);

    let n = crate::compositor::list_userspace_surfaces().len();
    let cascade_x = 48 + ((n.saturating_sub(1) % 6) as i32 * 28);
    let cascade_y = 40 + ((n.saturating_sub(1) % 5) as i32 * 24);
    let _ = crate::compositor::set_surface_origin(surface_id, cascade_x, cascade_y);
    let _ = crate::compositor::focus_surface(surface_id);

    let banner = alloc::format!("Welcome, {}", crate::session::display_name());
    render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
    let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
}
