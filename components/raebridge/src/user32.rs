//! user32.dll — Window management, message dispatch, input, and UI APIs.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    wide_to_string, Atom, CompatContext, Msg, PaintStruct, Point, Rect, WinBool, WinHandle,
    WindowObject, WndClassExW, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_INVALID_HANDLE,
    ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_SUCCESS, FALSE, IDOK, MB_OKCANCEL,
    MB_YESNO, NULL_HANDLE, SW_HIDE, TRUE, WM_CLOSE, WM_PAINT, WM_QUIT,
};

fn set_last_error(ctx: &mut CompatContext, code: u32) {
    ctx.last_error = code;
}

// =========================================================================
// Window class registration
// =========================================================================

pub fn register_class_ex_w(ctx: &mut CompatContext, wc: &WndClassExW) -> Atom {
    if wc.class_name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return Atom(0);
    }

    if ctx.registered_classes.contains_key(&wc.class_name) {
        set_last_error(ctx, ERROR_ALREADY_EXISTS);
        return Atom(0);
    }

    let atom = (ctx.registered_classes.len() as u16) + 0xC000;
    ctx.registered_classes
        .insert(wc.class_name.clone(), wc.clone());
    set_last_error(ctx, ERROR_SUCCESS);
    Atom(atom)
}

pub fn unregister_class_w(ctx: &mut CompatContext, class_name: &[u16]) -> WinBool {
    let name = wide_to_string(class_name);
    if ctx.registered_classes.remove(&name).is_some() {
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        FALSE
    }
}

// =========================================================================
// Window management
// =========================================================================

pub fn create_window_ex_w(
    ctx: &mut CompatContext,
    ex_style: u32,
    class_name: &[u16],
    window_name: &[u16],
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: WinHandle,
    _menu: WinHandle,
    _instance: WinHandle,
    _param: u64,
) -> WinHandle {
    let cls = wide_to_string(class_name);
    let title = wide_to_string(window_name);

    // A guest-registered class OR a USER32-provided system control (EDIT/…). A
    // real Notepad creates an "EDIT" child it never registered, so requiring
    // registration would reject it.
    if !ctx.registered_classes.contains_key(&cls) && !is_system_class(&cls) {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return NULL_HANDLE;
    }

    let hwnd_val = ctx.next_window_id;
    ctx.next_window_id += 1;

    let adjusted_x = if x == 0x80000000_u32 as i32 { 0 } else { x };
    let adjusted_y = if y == 0x80000000_u32 as i32 { 0 } else { y };
    let w = if width == 0x80000000_u32 as i32 {
        800
    } else {
        width
    };
    let h = if height == 0x80000000_u32 as i32 {
        600
    } else {
        height
    };

    let uvirt = 0x6000_0000u64 + (ctx.windows.len() as u64) * 0x100_0000;
    let mut surface_id = None;
    let mut surface_vaddr = None;

    let native_id = unsafe { crate::syscalls::sys_surface_create(w as u32, h as u32, uvirt) };
    if native_id != u64::MAX {
        surface_id = Some(native_id);
        surface_vaddr = Some(uvirt);
    }

    let win = WindowObject {
        handle: WinHandle(hwnd_val),
        class_name: cls,
        title,
        style,
        ex_style,
        rect: Rect {
            left: adjusted_x,
            top: adjusted_y,
            right: adjusted_x + w,
            bottom: adjusted_y + h,
        },
        client_rect: Rect {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        },
        parent,
        visible: false,
        enabled: true,
        user_data: 0,
        surface_id,
        surface_vaddr,
    };

    ctx.windows.insert(hwnd_val, win);
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(hwnd_val)
}

pub fn destroy_window(ctx: &mut CompatContext, hwnd: WinHandle) -> WinBool {
    if let Some(win) = ctx.windows.remove(&hwnd.0) {
        if let Some(sid) = win.surface_id {
            unsafe { crate::syscalls::sys_surface_close(sid) };
        }
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn show_window(ctx: &mut CompatContext, hwnd: WinHandle, cmd_show: i32) -> WinBool {
    let was_visible = if let Some(win) = ctx.windows.get_mut(&hwnd.0) {
        let prev = win.visible;
        win.visible = cmd_show != SW_HIDE;
        if win.visible {
            if let Some(sid) = win.surface_id {
                unsafe { crate::syscalls::sys_surface_present(sid, win.rect.left, win.rect.top) };
                unsafe { crate::syscalls::sys_surface_focus(sid) };
            }
        }
        prev
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    };
    set_last_error(ctx, ERROR_SUCCESS);
    WinBool::from_bool(was_visible)
}

pub fn move_window(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    repaint: WinBool,
) -> WinBool {
    if let Some(win) = ctx.windows.get_mut(&hwnd.0) {
        win.rect = Rect {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
        };
        win.client_rect = Rect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        if win.visible {
            if let Some(sid) = win.surface_id {
                unsafe { crate::syscalls::sys_surface_present(sid, win.rect.left, win.rect.top) };
            }
        }
        let _ = repaint;
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn set_window_pos(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    _hwnd_insert_after: WinHandle,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> WinBool {
    if let Some(win) = ctx.windows.get_mut(&hwnd.0) {
        let no_move = flags & 0x0002 != 0; // SWP_NOMOVE
        let no_size = flags & 0x0001 != 0; // SWP_NOSIZE
        if !no_move {
            win.rect.left = x;
            win.rect.top = y;
        }
        if !no_size {
            win.rect.right = win.rect.left + cx;
            win.rect.bottom = win.rect.top + cy;
            win.client_rect.right = cx;
            win.client_rect.bottom = cy;
        }
        if win.visible {
            if let Some(sid) = win.surface_id {
                unsafe { crate::syscalls::sys_surface_present(sid, win.rect.left, win.rect.top) };
            }
        }
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn get_window_rect(ctx: &mut CompatContext, hwnd: WinHandle, rect: &mut Rect) -> WinBool {
    if let Some(win) = ctx.windows.get(&hwnd.0) {
        *rect = win.rect;
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn get_client_rect(ctx: &mut CompatContext, hwnd: WinHandle, rect: &mut Rect) -> WinBool {
    if let Some(win) = ctx.windows.get(&hwnd.0) {
        *rect = win.client_rect;
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn set_window_text_w(ctx: &mut CompatContext, hwnd: WinHandle, text: &[u16]) -> WinBool {
    if let Some(win) = ctx.windows.get_mut(&hwnd.0) {
        win.title = wide_to_string(text);
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn get_window_text_w(ctx: &mut CompatContext, hwnd: WinHandle, buffer: &mut [u16]) -> i32 {
    if let Some(win) = ctx.windows.get(&hwnd.0) {
        let wide: Vec<u16> = win
            .title
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();
        let copy_len = core::cmp::min(wide.len(), buffer.len());
        for i in 0..copy_len {
            buffer[i] = wide[i];
        }
        if copy_len < buffer.len() {
            buffer[copy_len] = 0;
        }
        set_last_error(ctx, ERROR_SUCCESS);
        (copy_len.saturating_sub(1)) as i32
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        0
    }
}

pub fn get_window_text_length_w(ctx: &mut CompatContext, hwnd: WinHandle) -> i32 {
    let title_len = ctx
        .windows
        .get(&hwnd.0)
        .map(|w| w.title.encode_utf16().count() as i32);
    if let Some(len) = title_len {
        set_last_error(ctx, ERROR_SUCCESS);
        len
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        0
    }
}

pub fn is_window(ctx: &CompatContext, hwnd: WinHandle) -> WinBool {
    WinBool::from_bool(ctx.windows.contains_key(&hwnd.0))
}

pub fn is_window_visible(ctx: &CompatContext, hwnd: WinHandle) -> WinBool {
    match ctx.windows.get(&hwnd.0) {
        Some(w) => WinBool::from_bool(w.visible),
        None => FALSE,
    }
}

pub fn enable_window(ctx: &mut CompatContext, hwnd: WinHandle, enable: WinBool) -> WinBool {
    if let Some(win) = ctx.windows.get_mut(&hwnd.0) {
        let was_disabled = !win.enabled;
        win.enabled = enable.is_true();
        set_last_error(ctx, ERROR_SUCCESS);
        WinBool::from_bool(was_disabled)
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn get_foreground_window(ctx: &CompatContext) -> WinHandle {
    ctx.windows
        .values()
        .find(|w| w.visible)
        .map(|w| w.handle)
        .unwrap_or(NULL_HANDLE)
}

pub fn set_foreground_window(ctx: &mut CompatContext, hwnd: WinHandle) -> WinBool {
    if ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn get_desktop_window(_ctx: &CompatContext) -> WinHandle {
    WinHandle(0x00010001) // fixed desktop window handle
}

pub fn find_window_w(
    ctx: &CompatContext,
    class_name: Option<&[u16]>,
    window_name: Option<&[u16]>,
) -> WinHandle {
    let cls = class_name.map(wide_to_string);
    let title = window_name.map(wide_to_string);

    for win in ctx.windows.values() {
        let cls_match = cls.as_ref().map_or(true, |c| *c == win.class_name);
        let title_match = title.as_ref().map_or(true, |t| *t == win.title);
        if cls_match && title_match {
            return win.handle;
        }
    }
    NULL_HANDLE
}

pub fn bring_window_to_top(ctx: &mut CompatContext, hwnd: WinHandle) -> WinBool {
    if ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn set_window_long_w(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    index: i32,
    new_long: i64,
) -> i64 {
    if let Some(win) = ctx.windows.get_mut(&hwnd.0) {
        let prev = match index {
            -21 => {
                // GWL_USERDATA
                let old = win.user_data;
                win.user_data = new_long;
                old
            }
            -16 => {
                // GWL_STYLE
                let old = win.style as i64;
                win.style = new_long as u32;
                old
            }
            -20 => {
                // GWL_EXSTYLE
                let old = win.ex_style as i64;
                win.ex_style = new_long as u32;
                old
            }
            _ => 0,
        };
        set_last_error(ctx, ERROR_SUCCESS);
        prev
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        0
    }
}

pub fn get_window_long_w(ctx: &mut CompatContext, hwnd: WinHandle, index: i32) -> i64 {
    if let Some(win) = ctx.windows.get(&hwnd.0) {
        let val = match index {
            -21 => win.user_data,       // GWL_USERDATA
            -16 => win.style as i64,    // GWL_STYLE
            -20 => win.ex_style as i64, // GWL_EXSTYLE
            _ => 0,
        };
        set_last_error(ctx, ERROR_SUCCESS);
        val
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        0
    }
}

// =========================================================================
// Message loop
// =========================================================================

pub fn get_message_w(
    ctx: &mut CompatContext,
    msg: &mut Msg,
    hwnd: WinHandle,
    _msg_filter_min: u32,
    _msg_filter_max: u32,
) -> WinBool {
    if ctx.quit_posted {
        msg.hwnd = NULL_HANDLE;
        msg.message = WM_QUIT;
        msg.wparam = 0;
        msg.lparam = 0;
        msg.time = 0;
        msg.pt = Point { x: 0, y: 0 };
        return FALSE;
    }

    let idx = if hwnd.0 == 0 {
        ctx.message_queue.iter().position(|_| true)
    } else {
        ctx.message_queue.iter().position(|m| m.hwnd == hwnd)
    };

    match idx {
        Some(i) => {
            *msg = ctx.message_queue.remove(i);
            if msg.message == WM_QUIT {
                return FALSE;
            }
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        None => {
            msg.hwnd = NULL_HANDLE;
            msg.message = 0;
            msg.wparam = 0;
            msg.lparam = 0;
            msg.time = 0;
            msg.pt = Point { x: 0, y: 0 };
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
    }
}

pub fn peek_message_w(
    ctx: &mut CompatContext,
    msg: &mut Msg,
    hwnd: WinHandle,
    msg_filter_min: u32,
    msg_filter_max: u32,
    remove_msg: u32,
) -> WinBool {
    let _ = msg_filter_min;
    let _ = msg_filter_max;

    let idx = if hwnd.0 == 0 {
        ctx.message_queue.iter().position(|_| true)
    } else {
        ctx.message_queue.iter().position(|m| m.hwnd == hwnd)
    };

    match idx {
        Some(i) => {
            if remove_msg & 0x0001 != 0 {
                // PM_REMOVE
                *msg = ctx.message_queue.remove(i);
            } else {
                *msg = ctx.message_queue[i].clone();
            }
            TRUE
        }
        None => FALSE,
    }
}

pub fn translate_message(ctx: &mut CompatContext, msg: &Msg) -> WinBool {
    // Synthesize WM_CHAR from a WM_KEYDOWN for printable virtual-keys and post it
    // to the queue (the loop dequeues it next), so a WndProc receives typed
    // characters. Basic ASCII passthrough — VK letter/digit codes ARE their ASCII
    // value; full keyboard-layout / shift / dead-key / IME translation is a
    // follow-up. Returns TRUE iff a character message was produced.
    if msg.message == crate::WM_KEYDOWN {
        let vk = msg.wparam as u32;
        if (0x20..=0x7E).contains(&vk) {
            ctx.message_queue.push(Msg {
                hwnd: msg.hwnd,
                message: crate::WM_CHAR,
                wparam: vk as u64,
                lparam: msg.lparam,
                time: msg.time,
                pt: msg.pt,
            });
            return TRUE;
        }
    }
    FALSE
}

/// Resolve a window's registered `lpfnWndProc`: hwnd → its `WindowObject` → that
/// window's class → the class's `wnd_proc`. Returns 0 for a thread message
/// (`hwnd == 0`), an unknown window, or a class with no proc. Pure (no guest
/// call), so the routing is host-KAT'able. `pub` so the shim layer can resolve
/// under the ctx lock and then release it BEFORE invoking the WndProc.
pub fn resolve_wndproc(ctx: &CompatContext, hwnd: WinHandle) -> u64 {
    if hwnd.0 == 0 {
        return 0;
    }
    let win = match ctx.windows.get(&hwnd.0) {
        Some(w) => w,
        None => return 0,
    };
    ctx.registered_classes
        .get(&win.class_name)
        .map(|c| c.wnd_proc)
        .unwrap_or(0)
}

/// Invoke a guest `lpfnWndProc` with the Win64 ABI. This is the one
/// GUEST-EXECUTION seam in the message pump — it calls real guest machine code,
/// so it is proven in QEMU with a live guest, not host-KAT'able (a host test has
/// no guest WndProc to jump to). `resolve_wndproc` gates it to a non-zero addr.
/// MUST be called with NO ctx lock held — the WndProc calls back into the Win32
/// API (BeginPaint/FillRect/...), which re-enters `with_ctx`; the shim layer
/// resolves under the lock, releases, then calls this.
pub fn invoke_wndproc(proc_addr: u64, hwnd: u64, msg: u32, wparam: u64, lparam: i64) -> i64 {
    type WndProc = unsafe extern "win64" fn(u64, u32, u64, i64) -> i64;
    // SAFETY: `proc_addr` is the guest's registered lpfnWndProc, mapped executable
    // in this (shared guest/host) address space; only ever called with the
    // non-zero address `resolve_wndproc` returned for a live window's class.
    let f: WndProc = unsafe { core::mem::transmute::<u64, WndProc>(proc_addr) };
    unsafe { f(hwnd, msg, wparam, lparam) }
}

/// True if `class` is a USER32-provided system control class — one the guest can
/// `CreateWindowEx` WITHOUT registering, because USER32 (here, AthBridge)
/// supplies the window proc. A real Notepad's text lives in an "EDIT" child it
/// never registers; BUTTON/STATIC are accepted as system classes too (creation
/// succeeds) though only EDIT has interactive built-in behavior so far.
pub fn is_system_class(class: &str) -> bool {
    is_edit_class(class)
        || class.eq_ignore_ascii_case("BUTTON")
        || class.eq_ignore_ascii_case("STATIC")
}

/// The system EDIT control class (case-insensitive, like Win32 class matching).
pub fn is_edit_class(class: &str) -> bool {
    class.eq_ignore_ascii_case("EDIT")
}

/// How a dispatched message must be handled, decided under the ctx lock so the
/// shim can drop the lock before the one case that re-enters the API.
pub enum Dispatch {
    /// Invoke this guest `lpfnWndProc` — real guest code that re-enters the
    /// Win32 API, so the caller MUST release the ctx lock first.
    Guest(u64),
    /// A system control (EDIT/…): run [`run_builtin_proc`] — host Rust that does
    /// NOT re-enter `with_ctx`, so it runs while the lock is held.
    Builtin,
    /// Thread message / unknown window / classless window → LRESULT 0.
    None,
}

/// Decide how `hwnd`'s message should be dispatched. A guest-registered class
/// with a WndProc takes precedence; otherwise a system class routes to the
/// built-in proc; otherwise nothing handles it.
pub fn classify_dispatch(ctx: &CompatContext, hwnd: WinHandle) -> Dispatch {
    let proc_addr = resolve_wndproc(ctx, hwnd);
    if proc_addr != 0 {
        return Dispatch::Guest(proc_addr);
    }
    match ctx.windows.get(&hwnd.0) {
        Some(w) if is_system_class(&w.class_name) => Dispatch::Builtin,
        _ => Dispatch::None,
    }
}

/// The built-in (system-provided) window proc for a control class. Only the EDIT
/// control is interactive today: WM_CHAR accumulates the typed text into the
/// window's text buffer (`WindowObject.title`) — the storage `GetWindowTextW`
/// reads back, i.e. the mechanism a real Notepad relies on instead of a custom
/// WM_CHAR buffer. Pure for the message it handles (WM_CHAR's wParam is the
/// integer char code — no guest pointer), so it is host-KAT'able; the WM_*TEXT
/// messages carry guest buffers and stay in the shim's DefWindowProc.
pub fn run_builtin_proc(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    msg: u32,
    wparam: u64,
    lparam: i64,
) -> i64 {
    let is_edit = ctx
        .windows
        .get(&hwnd.0)
        .map(|w| is_edit_class(&w.class_name))
        .unwrap_or(false);
    if is_edit {
        match msg {
            crate::WM_CHAR => {
                if let Some(w) = ctx.windows.get_mut(&hwnd.0) {
                    match wparam as u32 {
                        0x08 => {
                            w.title.pop(); // Backspace deletes the last char.
                        }
                        0x0D => {
                            w.title.push('\n'); // Enter inserts a newline (multiline EDIT).
                        }
                        c @ 0x20..=0x7E => {
                            w.title.push(c as u8 as char); // Printable ASCII.
                        }
                        _ => {} // Other control chars: ignored.
                    }
                }
                return 0;
            }
            crate::WM_PAINT => {
                // The system EDIT proc paints its own text — clear to white, blit
                // the buffer black — so a real Notepad's edit child shows what was
                // typed (the SW-render half of the EDIT control).
                crate::gdi32::paint_control_text(ctx, hwnd);
                return 0;
            }
            _ => {}
        }
    }
    def_window_proc_w(ctx, hwnd, msg, wparam, lparam)
}

/// `DispatchMessageW`: route a dequeued message to its window's WndProc and
/// return the `LRESULT`. A guest WndProc is invoked via the Win64 ABI; a system
/// control (EDIT/…) runs its built-in proc; a thread message / no-proc window
/// returns 0.
pub fn dispatch_message_w(ctx: &mut CompatContext, msg: &Msg) -> i64 {
    match classify_dispatch(ctx, msg.hwnd) {
        Dispatch::Guest(proc_addr) => {
            invoke_wndproc(proc_addr, msg.hwnd.0, msg.message, msg.wparam, msg.lparam)
        }
        Dispatch::Builtin => run_builtin_proc(ctx, msg.hwnd, msg.message, msg.wparam, msg.lparam),
        Dispatch::None => 0,
    }
}

pub fn post_message_w(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    msg_id: u32,
    wparam: u64,
    lparam: i64,
) -> WinBool {
    if hwnd.0 != 0 && !ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }

    ctx.message_queue.push(Msg {
        hwnd,
        message: msg_id,
        wparam,
        lparam,
        time: 0,
        pt: Point { x: 0, y: 0 },
    });
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

/// `SendMessageW`: SYNCHRONOUSLY invoke the target window's WndProc and return
/// its `LRESULT`. (Was wrong — it enqueued the message, which is `PostMessage`
/// semantics; SendMessage bypasses the queue entirely.) An unknown hwnd is a
/// handle error; a window with no resolved proc returns 0 (DefWindowProc-like).
pub fn send_message_w(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    msg_id: u32,
    wparam: u64,
    lparam: i64,
) -> i64 {
    if hwnd.0 != 0 && !ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    match classify_dispatch(ctx, hwnd) {
        Dispatch::Guest(proc_addr) => invoke_wndproc(proc_addr, hwnd.0, msg_id, wparam, lparam),
        Dispatch::Builtin => run_builtin_proc(ctx, hwnd, msg_id, wparam, lparam),
        Dispatch::None => 0,
    }
}

pub fn post_quit_message(ctx: &mut CompatContext, exit_code: i32) {
    ctx.quit_posted = true;
    ctx.message_queue.push(Msg {
        hwnd: NULL_HANDLE,
        message: WM_QUIT,
        wparam: exit_code as u64,
        lparam: 0,
        time: 0,
        pt: Point { x: 0, y: 0 },
    });
}

pub fn def_window_proc_w(
    _ctx: &mut CompatContext,
    hwnd: WinHandle,
    msg: u32,
    wparam: u64,
    lparam: i64,
) -> i64 {
    let _ = hwnd;
    let _ = wparam;
    let _ = lparam;
    match msg {
        WM_CLOSE => 0,
        WM_PAINT => 0,
        _ => 0,
    }
}

// =========================================================================
// Dialogs
// =========================================================================

pub fn message_box_w(
    ctx: &mut CompatContext,
    _hwnd: WinHandle,
    _text: &[u16],
    _caption: &[u16],
    flags: u32,
) -> i32 {
    set_last_error(ctx, ERROR_SUCCESS);
    if flags & MB_YESNO != 0 {
        crate::IDYES
    } else if flags & MB_OKCANCEL != 0 {
        IDOK
    } else {
        IDOK
    }
}

// =========================================================================
// Window text (caption / EDIT-control buffer)
//
// A window's "text" is its caption for a top-level window and its content
// buffer for an EDIT control — the same storage in both cases. A notepad-class
// app stores typed text in an EDIT child and its File->Save handler reads it
// back with `GetWindowTextW(hEdit, buf, len)` before writing the file, so this
// is the load-bearing storage for the "types, saves" half of the gate. We back
// it with `WindowObject.title`, exactly as DefWindowProcW(WM_SETTEXT/WM_GETTEXT)
// does on Windows. These helpers are pure (no guest pointers) and host-KAT'able;
// the shim layer marshals the guest buffers around them.
// =========================================================================

/// `SetWindowTextW` storage half: replace the window's text. Returns FALSE for
/// an unknown hwnd (sets ERROR_INVALID_HANDLE), TRUE otherwise.
pub fn set_window_text(ctx: &mut CompatContext, hwnd: WinHandle, text: &str) -> WinBool {
    let ok = match ctx.windows.get_mut(&hwnd.0) {
        Some(w) => {
            w.title = String::from(text);
            true
        }
        None => false,
    };
    set_last_error(
        ctx,
        if ok {
            ERROR_SUCCESS
        } else {
            ERROR_INVALID_HANDLE
        },
    );
    if ok {
        TRUE
    } else {
        FALSE
    }
}

/// `GetWindowTextW` storage half: the window's current text, or `None` for an
/// unknown hwnd. The shim truncates it into the guest buffer via
/// [`copy_text_truncated`].
pub fn get_window_text(ctx: &CompatContext, hwnd: WinHandle) -> Option<String> {
    ctx.windows.get(&hwnd.0).map(|w| w.title.clone())
}

/// `GetWindowTextLengthW`: the window's text length in UTF-16 code units
/// (excluding the NUL), or 0 for an unknown hwnd — matching Win32, which cannot
/// distinguish an empty caption from a bad hwnd here.
pub fn get_window_text_length(ctx: &CompatContext, hwnd: WinHandle) -> i32 {
    ctx.windows
        .get(&hwnd.0)
        .map(|w| w.title.encode_utf16().count() as i32)
        .unwrap_or(0)
}

/// Copy `text` into a `GetWindowTextW`-style buffer of `max` UTF-16 units:
/// at most `max - 1` code units followed by a NUL. Returns
/// `(buffer_including_nul, chars_copied_excluding_nul)`. `max == 0` writes
/// nothing and returns count 0 (Win32 writes nothing and returns 0). Pure +
/// FAIL-able: the truncation boundary and the always-present terminator are the
/// classic off-by-one source, so they get tested without any raw pointers.
pub fn copy_text_truncated(text: &str, max: usize) -> (Vec<u16>, usize) {
    if max == 0 {
        return (Vec::new(), 0);
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let n = core::cmp::min(units.len(), max - 1);
    let mut buf = Vec::with_capacity(n + 1);
    buf.extend_from_slice(&units[..n]);
    buf.push(0);
    (buf, n)
}

pub fn dialog_box_param_w(
    ctx: &mut CompatContext,
    _instance: WinHandle,
    _template_name: &[u16],
    _parent: WinHandle,
    _dialog_func: u64,
    _init_param: i64,
) -> isize {
    set_last_error(ctx, ERROR_NOT_SUPPORTED);
    -1
}

pub fn end_dialog(ctx: &mut CompatContext, _dlg: WinHandle, result: isize) -> WinBool {
    let _ = result;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_dlg_item(ctx: &mut CompatContext, _dlg: WinHandle, _id: i32) -> WinHandle {
    set_last_error(ctx, ERROR_NOT_SUPPORTED);
    NULL_HANDLE
}

pub fn set_dlg_item_text_w(
    ctx: &mut CompatContext,
    _dlg: WinHandle,
    _id: i32,
    _text: &[u16],
) -> WinBool {
    set_last_error(ctx, ERROR_NOT_SUPPORTED);
    FALSE
}

// =========================================================================
// Painting
// =========================================================================

pub fn begin_paint(ctx: &mut CompatContext, hwnd: WinHandle, ps: &mut PaintStruct) -> WinHandle {
    if let Some(win) = ctx.windows.get(&hwnd.0) {
        let dc_handle = WinHandle(hwnd.0 | 0x80000000);
        ps.hdc = dc_handle;
        ps.erase = FALSE;
        ps.rc_paint = win.client_rect;
        set_last_error(ctx, ERROR_SUCCESS);
        dc_handle
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        NULL_HANDLE
    }
}

pub fn end_paint(ctx: &mut CompatContext, _hwnd: WinHandle, _ps: &PaintStruct) -> WinBool {
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn invalidate_rect(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    _rect: Option<&Rect>,
    _erase: WinBool,
) -> WinBool {
    if hwnd.0 != 0 && !ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

/// `UpdateWindow(hwnd)` — drive an immediate, SYNCHRONOUS `WM_PAINT` to the
/// window's WndProc (we treat every call as a needed repaint). This is what lets
/// a no-input app render deterministically: `CreateWindow` → `ShowWindow` →
/// `UpdateWindow` paints now (via the real `SendMessage` → WndProc dispatch),
/// before the message loop. Unknown hwnd is a handle error.
pub fn update_window(ctx: &mut CompatContext, hwnd: WinHandle) -> WinBool {
    if !ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = send_message_w(ctx, hwnd, WM_PAINT, 0, 0);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

/// `GetDC(hwnd)` — delegate to the gdi32 device-context model so the returned
/// HDC is stored (`gdi_objects`) and resolvable by `FillRect`/`TextOut` back to
/// this window's surface (the older tagged-handle stub wasn't paintable).
pub fn get_dc(ctx: &mut CompatContext, hwnd: WinHandle) -> WinHandle {
    if hwnd.0 != 0 && !ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return NULL_HANDLE;
    }
    crate::gdi32::get_dc(ctx, hwnd)
}

pub fn release_dc(ctx: &mut CompatContext, hwnd: WinHandle, hdc: WinHandle) -> i32 {
    crate::gdi32::release_dc(ctx, hwnd, hdc)
}

// =========================================================================
// Input
// =========================================================================

pub fn get_key_state(_ctx: &CompatContext, _vkey: i32) -> i16 {
    0
}

pub fn get_async_key_state(_ctx: &CompatContext, _vkey: i32) -> i16 {
    0
}

pub fn get_cursor_pos(_ctx: &mut CompatContext, point: &mut Point) -> WinBool {
    point.x = 0;
    point.y = 0;
    TRUE
}

pub fn set_cursor_pos(_ctx: &mut CompatContext, _x: i32, _y: i32) -> WinBool {
    TRUE
}

pub fn show_cursor(_ctx: &mut CompatContext, show: WinBool) -> i32 {
    if show.is_true() {
        0
    } else {
        -1
    }
}

pub fn set_capture(ctx: &mut CompatContext, hwnd: WinHandle) -> WinHandle {
    if ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_SUCCESS);
        hwnd
    } else {
        NULL_HANDLE
    }
}

pub fn release_capture(_ctx: &mut CompatContext) -> WinBool {
    TRUE
}

pub fn get_keyboard_state(_ctx: &CompatContext, key_state: &mut [u8; 256]) {
    for b in key_state.iter_mut() {
        *b = 0;
    }
}

pub fn map_virtual_key_w(_ctx: &CompatContext, code: u32, map_type: u32) -> u32 {
    let _ = map_type;
    code
}

// =========================================================================
// Clipboard
// =========================================================================

pub fn open_clipboard(ctx: &mut CompatContext, _hwnd: WinHandle) -> WinBool {
    if ctx.clipboard_open {
        set_last_error(ctx, ERROR_ACCESS_DENIED);
        return FALSE;
    }
    ctx.clipboard_open = true;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn close_clipboard(ctx: &mut CompatContext) -> WinBool {
    ctx.clipboard_open = false;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_clipboard_data(ctx: &mut CompatContext, format: u32) -> Option<&[u8]> {
    if !ctx.clipboard_open {
        set_last_error(ctx, ERROR_ACCESS_DENIED);
        return None;
    }
    ctx.clipboard.get(&format).map(|v| v.as_slice())
}

pub fn set_clipboard_data(ctx: &mut CompatContext, format: u32, data: &[u8]) -> WinHandle {
    if !ctx.clipboard_open {
        set_last_error(ctx, ERROR_ACCESS_DENIED);
        return NULL_HANDLE;
    }
    ctx.clipboard.insert(format, data.to_vec());
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(format as u64)
}

pub fn empty_clipboard(ctx: &mut CompatContext) -> WinBool {
    if !ctx.clipboard_open {
        set_last_error(ctx, ERROR_ACCESS_DENIED);
        return FALSE;
    }
    ctx.clipboard.clear();
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// System metrics and parameters
// =========================================================================

pub fn get_system_metrics(_ctx: &CompatContext, index: i32) -> i32 {
    match index {
        0 => 1920, // SM_CXSCREEN
        1 => 1080, // SM_CYSCREEN
        2 => 20,   // SM_CXVSCROLL
        3 => 20,   // SM_CYHSCROLL
        4 => 30,   // SM_CYCAPTION
        5 => 1,    // SM_CXBORDER
        6 => 1,    // SM_CYBORDER
        43 => 1,   // SM_CMOUSEBUTTONS (but really SM_CMETRICS proxy)
        80 => 1,   // SM_CMONITORS
        _ => 0,
    }
}

pub fn system_parameters_info_w(
    ctx: &mut CompatContext,
    action: u32,
    _ui_param: u32,
    _pv_param: u64,
    _update_flags: u32,
) -> WinBool {
    let _ = action;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_dpi_for_window(ctx: &CompatContext, hwnd: WinHandle) -> u32 {
    if ctx.windows.contains_key(&hwnd.0) {
        96
    } else {
        0
    }
}

// =========================================================================
// ANSI string helper
// =========================================================================

fn cstr_to_string(ptr: &[u8]) -> String {
    let end = ptr.iter().position(|&b| b == 0).unwrap_or(ptr.len());
    let mut s = String::new();
    for &b in &ptr[..end] {
        s.push(b as char);
    }
    s
}

fn str_to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

// =========================================================================
// ANSI window class registration
// =========================================================================

pub fn register_class_ex_a(ctx: &mut CompatContext, wc: &WndClassExW) -> Atom {
    register_class_ex_w(ctx, wc)
}

pub fn unregister_class_a(ctx: &mut CompatContext, class_name: &[u8]) -> WinBool {
    let wide = str_to_wide(&cstr_to_string(class_name));
    unregister_class_w(ctx, &wide)
}

// =========================================================================
// ANSI window creation
// =========================================================================

pub fn create_window_ex_a(
    ctx: &mut CompatContext,
    ex_style: u32,
    class_name: &[u8],
    window_name: &[u8],
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: WinHandle,
    menu: WinHandle,
    instance: WinHandle,
    param: u64,
) -> WinHandle {
    let cls_w = str_to_wide(&cstr_to_string(class_name));
    let title_w = str_to_wide(&cstr_to_string(window_name));
    create_window_ex_w(
        ctx, ex_style, &cls_w, &title_w, style, x, y, width, height, parent, menu, instance, param,
    )
}

// =========================================================================
// ANSI default window procedure
// =========================================================================

pub fn def_window_proc_a(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    msg: u32,
    wparam: u64,
    lparam: i64,
) -> i64 {
    def_window_proc_w(ctx, hwnd, msg, wparam, lparam)
}

// =========================================================================
// ANSI message loop
// =========================================================================

pub fn get_message_a(
    ctx: &mut CompatContext,
    msg: &mut Msg,
    hwnd: WinHandle,
    msg_filter_min: u32,
    msg_filter_max: u32,
) -> WinBool {
    get_message_w(ctx, msg, hwnd, msg_filter_min, msg_filter_max)
}

pub fn dispatch_message_a(ctx: &mut CompatContext, msg: &Msg) -> i64 {
    dispatch_message_w(ctx, msg)
}

pub fn peek_message_a(
    ctx: &mut CompatContext,
    msg: &mut Msg,
    hwnd: WinHandle,
    msg_filter_min: u32,
    msg_filter_max: u32,
    remove_msg: u32,
) -> WinBool {
    peek_message_w(ctx, msg, hwnd, msg_filter_min, msg_filter_max, remove_msg)
}

pub fn post_message_a(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    msg_id: u32,
    wparam: u64,
    lparam: i64,
) -> WinBool {
    post_message_w(ctx, hwnd, msg_id, wparam, lparam)
}

pub fn send_message_a(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    msg_id: u32,
    wparam: u64,
    lparam: i64,
) -> i64 {
    send_message_w(ctx, hwnd, msg_id, wparam, lparam)
}

// =========================================================================
// ANSI window search
// =========================================================================

pub fn find_window_a(
    ctx: &CompatContext,
    class_name: Option<&[u8]>,
    window_name: Option<&[u8]>,
) -> WinHandle {
    let cls_w: Option<Vec<u16>> = class_name.map(|c| str_to_wide(&cstr_to_string(c)));
    let title_w: Option<Vec<u16>> = window_name.map(|t| str_to_wide(&cstr_to_string(t)));
    find_window_w(ctx, cls_w.as_deref(), title_w.as_deref())
}

// =========================================================================
// ANSI dialog
// =========================================================================

pub fn message_box_a(
    ctx: &mut CompatContext,
    _hwnd: WinHandle,
    _text: &[u8],
    _caption: &[u8],
    flags: u32,
) -> i32 {
    set_last_error(ctx, ERROR_SUCCESS);
    if flags & MB_YESNO != 0 {
        crate::IDYES
    } else if flags & MB_OKCANCEL != 0 {
        IDOK
    } else {
        IDOK
    }
}

// =========================================================================
// Resource loading (return dummy handles)
// =========================================================================

pub fn load_icon_w(ctx: &mut CompatContext, _instance: WinHandle, _icon_name: u64) -> WinHandle {
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(0xC100_0001)
}

pub fn load_icon_a(ctx: &mut CompatContext, instance: WinHandle, icon_name: u64) -> WinHandle {
    load_icon_w(ctx, instance, icon_name)
}

pub fn load_cursor_w(
    ctx: &mut CompatContext,
    _instance: WinHandle,
    _cursor_name: u64,
) -> WinHandle {
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(0xC200_0001)
}

pub fn load_cursor_a(ctx: &mut CompatContext, instance: WinHandle, cursor_name: u64) -> WinHandle {
    load_cursor_w(ctx, instance, cursor_name)
}

pub fn load_image_w(
    ctx: &mut CompatContext,
    _instance: WinHandle,
    _name: u64,
    image_type: u32,
    _cx: i32,
    _cy: i32,
    _flags: u32,
) -> WinHandle {
    let _ = image_type;
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(0xC300_0001)
}

// =========================================================================
// Timer
// =========================================================================

static NEXT_TIMER_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

pub fn set_timer(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    id_event: u64,
    _elapse: u32,
    _timer_func: u64,
) -> u64 {
    if hwnd.0 != 0 && !ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    if id_event != 0 {
        id_event
    } else {
        NEXT_TIMER_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
    }
}

pub fn kill_timer(ctx: &mut CompatContext, hwnd: WinHandle, _id_event: u64) -> WinBool {
    if hwnd.0 != 0 && !ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// ANSI window text
// =========================================================================

pub fn set_window_text_a(ctx: &mut CompatContext, hwnd: WinHandle, text: &[u8]) -> WinBool {
    let wide = str_to_wide(&cstr_to_string(text));
    set_window_text_w(ctx, hwnd, &wide)
}

pub fn get_window_text_a(ctx: &mut CompatContext, hwnd: WinHandle, buffer: &mut [u8]) -> i32 {
    if let Some(win) = ctx.windows.get(&hwnd.0) {
        let bytes = win.title.as_bytes();
        let copy_len = core::cmp::min(bytes.len(), buffer.len().saturating_sub(1));
        buffer[..copy_len].copy_from_slice(&bytes[..copy_len]);
        if copy_len < buffer.len() {
            buffer[copy_len] = 0;
        }
        set_last_error(ctx, ERROR_SUCCESS);
        copy_len as i32
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        0
    }
}

pub fn get_window_text_length_a(ctx: &mut CompatContext, hwnd: WinHandle) -> i32 {
    let len = ctx.windows.get(&hwnd.0).map(|w| w.title.len() as i32);
    if let Some(l) = len {
        set_last_error(ctx, ERROR_SUCCESS);
        l
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        0
    }
}

// =========================================================================
// Convenience constants
// =========================================================================

pub const CW_USEDEFAULT: i32 = 0x80000000_u32 as i32;
pub const GWL_STYLE: i32 = -16;
pub const GWL_EXSTYLE: i32 = -20;
pub const GWL_USERDATA: i32 = -21;
pub const PM_NOREMOVE: u32 = 0x0000;
pub const PM_REMOVE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOACTIVATE: u32 = 0x0010;

pub const IDC_ARROW: u64 = 32512;
pub const IDC_IBEAM: u64 = 32513;
pub const IDC_WAIT: u64 = 32514;
pub const IDC_HAND: u64 = 32649;
pub const IDI_APPLICATION: u64 = 32512;
pub const IDI_ERROR: u64 = 32513;
pub const IDI_WARNING: u64 = 32515;

pub const IMAGE_BITMAP: u32 = 0;
pub const IMAGE_ICON: u32 = 1;
pub const IMAGE_CURSOR: u32 = 2;

// =========================================================================
// Monitor APIs
// =========================================================================

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub cb_size: u32,
    pub monitor: Rect,
    pub work: Rect,
    pub flags: u32,
}

pub fn monitor_from_window(ctx: &CompatContext, _hwnd: WinHandle, _flags: u32) -> WinHandle {
    let _ = ctx;
    WinHandle(0xDEAD_0001) // virtual primary monitor handle
}

pub fn monitor_from_point(_ctx: &CompatContext, _x: i32, _y: i32, _flags: u32) -> WinHandle {
    WinHandle(0xDEAD_0001)
}

pub fn get_monitor_info_w(
    _ctx: &CompatContext,
    monitor: WinHandle,
    info: &mut MonitorInfo,
) -> WinBool {
    if monitor.0 == 0 {
        return FALSE;
    }
    info.cb_size = core::mem::size_of::<MonitorInfo>() as u32;
    info.monitor = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    info.work = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };
    info.flags = 1; // MONITORINFOF_PRIMARY
    TRUE
}

pub fn enum_display_monitors(
    _ctx: &CompatContext,
    _hdc: WinHandle,
    _clip: Option<&Rect>,
    callback: u64,
    _data: isize,
) -> WinBool {
    let _ = callback;
    TRUE
}

pub fn enum_display_settings_w(
    _ctx: &CompatContext,
    _device_name: Option<&[u16]>,
    mode_num: u32,
    dev_mode: &mut DevMode,
) -> WinBool {
    if mode_num > 0 {
        return FALSE;
    }
    dev_mode.pels_width = 1920;
    dev_mode.pels_height = 1080;
    dev_mode.bits_per_pel = 32;
    dev_mode.display_frequency = 60;
    dev_mode.display_flags = 0;
    TRUE
}

#[derive(Debug, Clone)]
pub struct DevMode {
    pub pels_width: u32,
    pub pels_height: u32,
    pub bits_per_pel: u32,
    pub display_frequency: u32,
    pub display_flags: u32,
}

pub fn change_display_settings_w(
    _ctx: &mut CompatContext,
    _dev_mode: Option<&DevMode>,
    _flags: u32,
) -> i32 {
    0 // DISP_CHANGE_SUCCESSFUL
}

// =========================================================================
// Window enumeration
// =========================================================================

pub fn enum_windows(ctx: &CompatContext, _callback: u64, _lparam: isize) -> WinBool {
    let _ = ctx;
    TRUE
}

pub fn enum_child_windows(
    ctx: &CompatContext,
    _parent: WinHandle,
    _callback: u64,
    _lparam: isize,
) -> WinBool {
    let _ = ctx;
    TRUE
}

pub fn enum_thread_windows(
    _ctx: &CompatContext,
    _thread_id: u32,
    _callback: u64,
    _lparam: isize,
) -> WinBool {
    TRUE
}

pub fn get_parent(ctx: &CompatContext, hwnd: WinHandle) -> WinHandle {
    if let Some(win) = ctx.windows.get(&hwnd.0) {
        win.parent
    } else {
        NULL_HANDLE
    }
}

pub fn get_window(ctx: &CompatContext, hwnd: WinHandle, cmd: u32) -> WinHandle {
    let _ = cmd;
    if ctx.windows.contains_key(&hwnd.0) {
        NULL_HANDLE // no sibling navigation
    } else {
        NULL_HANDLE
    }
}

pub fn get_top_window(ctx: &CompatContext, _hwnd: WinHandle) -> WinHandle {
    let _ = ctx;
    NULL_HANDLE
}

pub fn get_ancestor(ctx: &CompatContext, hwnd: WinHandle, flags: u32) -> WinHandle {
    let _ = flags;
    if ctx.windows.contains_key(&hwnd.0) {
        hwnd
    } else {
        NULL_HANDLE
    }
}

pub fn is_iconic(_ctx: &CompatContext, _hwnd: WinHandle) -> WinBool {
    FALSE
}

pub fn is_zoomed(_ctx: &CompatContext, _hwnd: WinHandle) -> WinBool {
    FALSE
}

// =========================================================================
// Coordinate mapping
// =========================================================================

pub fn screen_to_client(_ctx: &CompatContext, _hwnd: WinHandle, point: &mut Point) -> WinBool {
    let _ = point;
    TRUE
}

pub fn client_to_screen(_ctx: &CompatContext, _hwnd: WinHandle, point: &mut Point) -> WinBool {
    let _ = point;
    TRUE
}

pub fn map_window_points(
    _ctx: &CompatContext,
    _from: WinHandle,
    _to: WinHandle,
    _points: &mut [Point],
) -> i32 {
    0
}

pub fn window_from_point(ctx: &CompatContext, _x: i32, _y: i32) -> WinHandle {
    let _ = ctx;
    NULL_HANDLE
}

pub fn child_window_from_point(
    ctx: &CompatContext,
    _parent: WinHandle,
    _x: i32,
    _y: i32,
) -> WinHandle {
    let _ = ctx;
    NULL_HANDLE
}

// =========================================================================
// Raw input
// =========================================================================

#[derive(Debug)]
pub struct RawInputDevice {
    pub usage_page: u16,
    pub usage: u16,
    pub flags: u32,
    pub target: WinHandle,
}

pub fn register_raw_input_devices(
    _ctx: &mut CompatContext,
    devices: &[RawInputDevice],
    _size: u32,
) -> WinBool {
    let _ = devices;
    TRUE
}

pub fn get_raw_input_data(
    _ctx: &CompatContext,
    _raw_input: WinHandle,
    _command: u32,
    _data: u64,
    size: &mut u32,
    _header_size: u32,
) -> u32 {
    *size = 0;
    0
}

pub fn get_registered_raw_input_devices(
    _ctx: &CompatContext,
    _devices: u64,
    num_devices: &mut u32,
    _size: u32,
) -> u32 {
    *num_devices = 0;
    0
}

// =========================================================================
// Hooks
// =========================================================================

pub fn set_windows_hook_ex_w(
    _ctx: &mut CompatContext,
    _id_hook: i32,
    _lpfn: u64,
    _hmod: WinHandle,
    _thread_id: u32,
) -> WinHandle {
    WinHandle(0x00C0_0001_u64.wrapping_add(1))
}

pub fn unhook_windows_hook_ex(_ctx: &mut CompatContext, _hook: WinHandle) -> WinBool {
    TRUE
}

pub fn call_next_hook_ex(
    _ctx: &CompatContext,
    _hook: WinHandle,
    _code: i32,
    _wparam: u64,
    _lparam: i64,
) -> i64 {
    0
}

// =========================================================================
// Painting / region helpers
// =========================================================================

pub fn get_update_rect(
    _ctx: &CompatContext,
    _hwnd: WinHandle,
    rect: &mut Rect,
    _erase: WinBool,
) -> WinBool {
    rect.left = 0;
    rect.top = 0;
    rect.right = 0;
    rect.bottom = 0;
    FALSE
}

pub fn validate_rect(_ctx: &mut CompatContext, _hwnd: WinHandle, _rect: Option<&Rect>) -> WinBool {
    TRUE
}

pub fn redraw_window(
    _ctx: &mut CompatContext,
    _hwnd: WinHandle,
    _rect: Option<&Rect>,
    _rgn: WinHandle,
    _flags: u32,
) -> WinBool {
    TRUE
}

pub fn scroll_window(
    _ctx: &mut CompatContext,
    _hwnd: WinHandle,
    _dx: i32,
    _dy: i32,
    _scroll: Option<&Rect>,
    _clip: Option<&Rect>,
) -> WinBool {
    TRUE
}

// =========================================================================
// Window property / class info
// =========================================================================

pub fn set_prop_w(
    _ctx: &mut CompatContext,
    _hwnd: WinHandle,
    _name: &[u16],
    _data: u64,
) -> WinBool {
    TRUE
}

pub fn get_prop_w(_ctx: &CompatContext, _hwnd: WinHandle, _name: &[u16]) -> u64 {
    0
}

pub fn remove_prop_w(_ctx: &mut CompatContext, _hwnd: WinHandle, _name: &[u16]) -> u64 {
    0
}

pub fn get_class_name_w(ctx: &mut CompatContext, hwnd: WinHandle, buf: &mut [u16]) -> i32 {
    if let Some(win) = ctx.windows.get(&hwnd.0) {
        let name = &win.class_name;
        let copy_len = core::cmp::min(name.len(), buf.len().saturating_sub(1));
        for (i, ch) in name.bytes().take(copy_len).enumerate() {
            buf[i] = ch as u16;
        }
        if copy_len < buf.len() {
            buf[copy_len] = 0;
        }
        set_last_error(ctx, ERROR_SUCCESS);
        copy_len as i32
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        0
    }
}

pub fn get_class_long_w(_ctx: &CompatContext, _hwnd: WinHandle, _index: i32) -> u64 {
    0
}

pub fn set_class_long_w(
    _ctx: &mut CompatContext,
    _hwnd: WinHandle,
    _index: i32,
    _new_long: i64,
) -> u64 {
    0
}

pub fn adjust_window_rect(
    _ctx: &CompatContext,
    rect: &mut Rect,
    _style: u32,
    _menu: WinBool,
) -> WinBool {
    rect.left -= 8;
    rect.top -= 31;
    rect.right += 8;
    rect.bottom += 8;
    TRUE
}

pub fn adjust_window_rect_ex(
    ctx: &CompatContext,
    rect: &mut Rect,
    style: u32,
    menu: WinBool,
    _ex_style: u32,
) -> WinBool {
    adjust_window_rect(ctx, rect, style, menu)
}

// =========================================================================
// Focus / activation
// =========================================================================

pub fn get_focus(_ctx: &CompatContext) -> WinHandle {
    NULL_HANDLE
}

pub fn set_focus(ctx: &mut CompatContext, hwnd: WinHandle) -> WinHandle {
    let _ = ctx;
    hwnd
}

pub fn get_active_window(_ctx: &CompatContext) -> WinHandle {
    NULL_HANDLE
}

pub fn set_active_window(_ctx: &mut CompatContext, hwnd: WinHandle) -> WinHandle {
    hwnd
}

// =========================================================================
// Misc user32
// =========================================================================

pub fn set_cursor(_ctx: &mut CompatContext, cursor: WinHandle) -> WinHandle {
    cursor
}

pub fn get_double_click_time(_ctx: &CompatContext) -> u32 {
    500
}

pub fn get_window_dc(ctx: &mut CompatContext, hwnd: WinHandle) -> WinHandle {
    get_dc(ctx, hwnd)
}

pub fn get_keyboard_layout(_ctx: &CompatContext, _thread_id: u32) -> u64 {
    0x0409_0409 // en-US
}

pub fn get_keyboard_layout_name_w(_ctx: &CompatContext, buf: &mut [u16]) -> WinBool {
    let name = "00000409";
    for (i, ch) in name.bytes().enumerate() {
        if i >= buf.len() {
            break;
        }
        buf[i] = ch as u16;
    }
    if name.len() < buf.len() {
        buf[name.len()] = 0;
    }
    TRUE
}

pub fn to_unicode_ex(
    _ctx: &CompatContext,
    _vkey: u32,
    _scan_code: u32,
    _key_state: &[u8; 256],
    buf: &mut [u16],
    _flags: u32,
    _layout: u64,
) -> i32 {
    if !buf.is_empty() {
        buf[0] = 0;
    }
    0
}

pub fn char_upper_w(_ctx: &CompatContext, ch: u16) -> u16 {
    if ch >= b'a' as u16 && ch <= b'z' as u16 {
        ch - 32
    } else {
        ch
    }
}

pub fn char_lower_w(_ctx: &CompatContext, ch: u16) -> u16 {
    if ch >= b'A' as u16 && ch <= b'Z' as u16 {
        ch + 32
    } else {
        ch
    }
}

pub fn wsprintfw(_ctx: &CompatContext, _buf: &mut [u16], _fmt: &[u16]) -> i32 {
    0
}

pub fn get_window_long_ptr_w(ctx: &mut CompatContext, hwnd: WinHandle, index: i32) -> i64 {
    get_window_long_w(ctx, hwnd, index)
}

pub fn set_window_long_ptr_w(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    index: i32,
    new_long: i64,
) -> i64 {
    set_window_long_w(ctx, hwnd, index, new_long)
}

pub fn set_layered_window_attributes(
    _ctx: &mut CompatContext,
    _hwnd: WinHandle,
    _cr_key: u32,
    _alpha: u8,
    _flags: u32,
) -> WinBool {
    TRUE
}

pub fn track_mouse_event(_ctx: &mut CompatContext, _event: u64) -> WinBool {
    TRUE
}

// =========================================================================
// Menus (menu bar / popup -> WM_COMMAND)
//
// A real app builds a File/Edit menu and, when an item is chosen, USER32 posts
// WM_COMMAND(LOWORD=item id, HIWORD=0, lParam=0) to the owner window — which a
// Notepad's WndProc handles to run Save/Open/etc. The menu model is pure data
// (host-KAT'able); `post_menu_command` is the selection->WM_COMMAND path that
// the existing message pump already delivers to the WndProc.
// =========================================================================

/// MF_* AppendMenu flags we model.
pub const MF_STRING: u32 = 0x0000;
pub const MF_POPUP: u32 = 0x0010;
pub const MF_SEPARATOR: u32 = 0x0800;

/// A menu: a flat ordered list of items. A submenu is an item whose `submenu`
/// is a child HMENU (MF_POPUP). Pure data.
#[derive(Clone, Default)]
pub struct Menu {
    pub items: Vec<MenuItem>,
}

/// One menu entry.
#[derive(Clone)]
pub struct MenuItem {
    /// Command id — the WPARAM low word a click posts as WM_COMMAND. 0 for a
    /// separator or a popup/submenu item.
    pub id: u32,
    pub text: String,
    pub flags: u32,
    /// Child HMENU for an MF_POPUP item, else None.
    pub submenu: Option<u64>,
}

fn alloc_menu(ctx: &mut CompatContext) -> WinHandle {
    let h = ctx.next_menu_id;
    ctx.next_menu_id += 1;
    ctx.menus.insert(h, Menu::default());
    WinHandle(h)
}

/// `CreateMenu()` -> HMENU. A new empty menu (used as a menu bar).
pub fn create_menu(ctx: &mut CompatContext) -> WinHandle {
    set_last_error(ctx, ERROR_SUCCESS);
    alloc_menu(ctx)
}

/// `CreatePopupMenu()` -> HMENU. Same model as a menu bar (a flat item list).
pub fn create_popup_menu(ctx: &mut CompatContext) -> WinHandle {
    set_last_error(ctx, ERROR_SUCCESS);
    alloc_menu(ctx)
}

/// `AppendMenuW(hMenu, uFlags, uIDNewItem, lpNewItem)` -> BOOL. MF_POPUP →
/// `id_or_submenu` is the child HMENU; MF_SEPARATOR → a separator; else
/// MF_STRING → `id_or_submenu` is the command id and `text` the label. FALSE for
/// an unknown hMenu.
pub fn append_menu(
    ctx: &mut CompatContext,
    hmenu: WinHandle,
    flags: u32,
    id_or_submenu: u64,
    text: &str,
) -> WinBool {
    let item = if flags & MF_POPUP != 0 {
        MenuItem {
            id: 0,
            text: String::from(text),
            flags,
            submenu: Some(id_or_submenu),
        }
    } else if flags & MF_SEPARATOR != 0 {
        MenuItem {
            id: 0,
            text: String::new(),
            flags,
            submenu: None,
        }
    } else {
        MenuItem {
            id: id_or_submenu as u32,
            text: String::from(text),
            flags,
            submenu: None,
        }
    };
    match ctx.menus.get_mut(&hmenu.0) {
        Some(m) => {
            m.items.push(item);
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            FALSE
        }
    }
}

/// `SetMenu(hWnd, hMenu)` -> BOOL. Attach (or, with hMenu==0, detach) a window's
/// menu bar. FALSE for an unknown window.
pub fn set_menu(ctx: &mut CompatContext, hwnd: WinHandle, hmenu: WinHandle) -> WinBool {
    if !ctx.windows.contains_key(&hwnd.0) {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    if hmenu.0 == 0 {
        ctx.window_menus.remove(&hwnd.0);
    } else {
        ctx.window_menus.insert(hwnd.0, hmenu.0);
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

/// `GetMenu(hWnd)` -> HMENU (0 if the window has no menu / is unknown).
pub fn get_menu(ctx: &CompatContext, hwnd: WinHandle) -> WinHandle {
    WinHandle(ctx.window_menus.get(&hwnd.0).copied().unwrap_or(0))
}

/// `GetMenuItemCount(hMenu)` -> count, or -1 for an unknown menu (Win32 returns
/// -1 on error).
pub fn get_menu_item_count(ctx: &CompatContext, hmenu: WinHandle) -> i32 {
    ctx.menus
        .get(&hmenu.0)
        .map(|m| m.items.len() as i32)
        .unwrap_or(-1)
}

/// `GetMenuItemID(hMenu, nPos)` -> the item's command id, or 0xFFFFFFFF for a
/// bad position / submenu item (matching Win32's `(UINT)-1`).
pub fn get_menu_item_id(ctx: &CompatContext, hmenu: WinHandle, pos: i32) -> u32 {
    if pos < 0 {
        return u32::MAX;
    }
    match ctx
        .menus
        .get(&hmenu.0)
        .and_then(|m| m.items.get(pos as usize))
    {
        Some(it) if it.submenu.is_none() && it.flags & MF_SEPARATOR == 0 => it.id,
        _ => u32::MAX,
    }
}

/// `DestroyMenu(hMenu)` -> BOOL. Frees the menu object.
pub fn destroy_menu(ctx: &mut CompatContext, hmenu: WinHandle) -> WinBool {
    if ctx.menus.remove(&hmenu.0).is_some() {
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

/// Select a menu command: post `WM_COMMAND` to `hwnd` (wParam low word = `id`,
/// high word = 0 for a menu source; lParam = 0) — the exact message a menu click
/// delivers, which the pump routes to the window's WndProc. The load-bearing
/// "a menu item triggers the app's action" path.
pub fn post_menu_command(ctx: &mut CompatContext, hwnd: WinHandle, id: u32) -> WinBool {
    post_message_w(ctx, hwnd, crate::WM_COMMAND, id as u64, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{testpe, FullCompatSession, SessionId};

    /// Build a session (which derefs to `CompatContext`) with one registered
    /// class (carrying `wnd_proc`) and one window of that class, so the
    /// message-dispatch routing is exercised without the surface syscall that
    /// `create_window_ex_w` would issue.
    fn ctx_with_window(wnd_proc: u64) -> (FullCompatSession, WinHandle) {
        let exe = testpe::build_exit_process_exe();
        let mut ctx =
            FullCompatSession::new(SessionId(31), "ui.exe".into(), exe, "ui.exe".into()).unwrap();
        let cls = String::from("RaeTestClass");
        ctx.registered_classes.insert(
            cls.clone(),
            WndClassExW {
                style: 0,
                wnd_proc,
                class_name: cls.clone(),
                icon: NULL_HANDLE,
                cursor: NULL_HANDLE,
                background: NULL_HANDLE,
                menu_name: None,
                icon_sm: NULL_HANDLE,
            },
        );
        let hwnd = WinHandle(0x0001_0000);
        ctx.windows.insert(
            hwnd.0,
            WindowObject {
                handle: hwnd,
                class_name: cls,
                title: String::new(),
                style: 0,
                ex_style: 0,
                rect: Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                client_rect: Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                parent: NULL_HANDLE,
                visible: false,
                enabled: true,
                user_data: 0,
                surface_id: None,
                surface_vaddr: None,
            },
        );
        (ctx, hwnd)
    }

    #[test]
    fn resolve_wndproc_maps_hwnd_to_class_proc() {
        let (ctx, hwnd) = ctx_with_window(0xDEAD_BEEF);
        assert_eq!(
            resolve_wndproc(&ctx, hwnd),
            0xDEAD_BEEF,
            "hwnd -> class -> proc"
        );
        assert_eq!(
            resolve_wndproc(&ctx, WinHandle(0)),
            0,
            "thread message has no proc"
        );
        assert_eq!(
            resolve_wndproc(&ctx, WinHandle(0x9999)),
            0,
            "unknown hwnd has no proc"
        );
    }

    #[test]
    fn set_then_get_window_text_round_trips() {
        // The notepad save path: SetWindowText stores, GetWindowText reads back
        // identically, and the length matches the UTF-16 code-unit count.
        let (mut ctx, hwnd) = ctx_with_window(0);
        assert!(set_window_text(&mut ctx, hwnd, "Hello, AthBridge").is_true());
        assert_eq!(ctx.last_error, ERROR_SUCCESS);
        assert_eq!(
            get_window_text(&ctx, hwnd).as_deref(),
            Some("Hello, AthBridge")
        );
        assert_eq!(get_window_text_length(&ctx, hwnd), 16);
        // Overwrite (SetWindowText replaces, never appends).
        assert!(set_window_text(&mut ctx, hwnd, "x").is_true());
        assert_eq!(get_window_text(&ctx, hwnd).as_deref(), Some("x"));
        assert_eq!(get_window_text_length(&ctx, hwnd), 1);
    }

    #[test]
    fn window_text_unknown_hwnd() {
        let (mut ctx, _hwnd) = ctx_with_window(0);
        assert!(!set_window_text(&mut ctx, WinHandle(0x4242), "nope").is_true());
        assert_eq!(ctx.last_error, ERROR_INVALID_HANDLE);
        assert_eq!(get_window_text(&ctx, WinHandle(0x4242)), None);
        assert_eq!(get_window_text_length(&ctx, WinHandle(0x4242)), 0);
    }

    #[test]
    fn copy_text_truncated_truncates_and_always_terminates() {
        // Fits with room to spare: full text + NUL, count excludes NUL.
        let (buf, n) = copy_text_truncated("AB", 10);
        assert_eq!(buf, alloc::vec![b'A' as u16, b'B' as u16, 0]);
        assert_eq!(n, 2);
        // Exact-fit boundary: max=N keeps N-1 chars + NUL (the classic off-by-one).
        let (buf, n) = copy_text_truncated("ABCDE", 3);
        assert_eq!(buf, alloc::vec![b'A' as u16, b'B' as u16, 0]);
        assert_eq!(n, 2);
        // max == 1 fits only the terminator.
        let (buf, n) = copy_text_truncated("ABCDE", 1);
        assert_eq!(buf, alloc::vec![0u16]);
        assert_eq!(n, 0);
        // max == 0 writes nothing at all (Win32 returns 0, touches no buffer).
        let (buf, n) = copy_text_truncated("ABCDE", 0);
        assert!(buf.is_empty());
        assert_eq!(n, 0);
        // Empty text still terminates.
        let (buf, n) = copy_text_truncated("", 4);
        assert_eq!(buf, alloc::vec![0u16]);
        assert_eq!(n, 0);
    }

    /// A system "EDIT" child window (never registered) at `hwnd`, empty text.
    fn edit_window(hwnd: WinHandle) -> WindowObject {
        WindowObject {
            handle: hwnd,
            class_name: String::from("EDIT"),
            title: String::new(),
            style: 0,
            ex_style: 0,
            rect: Rect {
                left: 0,
                top: 0,
                right: 120,
                bottom: 24,
            },
            client_rect: Rect {
                left: 0,
                top: 0,
                right: 120,
                bottom: 24,
            },
            parent: NULL_HANDLE,
            visible: true,
            enabled: true,
            user_data: 0,
            surface_id: None,
            surface_vaddr: None,
        }
    }

    #[test]
    fn system_classes_recognized_case_insensitively() {
        assert!(is_system_class("EDIT"));
        assert!(is_system_class("edit")); // Win32 class matching is case-insensitive
        assert!(is_system_class("BUTTON"));
        assert!(is_system_class("STATIC"));
        assert!(is_edit_class("Edit"));
        assert!(!is_edit_class("BUTTON"));
        assert!(!is_system_class("MyAppWindowClass"));
    }

    #[test]
    fn classify_dispatch_routes_guest_builtin_none() {
        // A guest-registered class with a WndProc -> Guest(proc).
        let (mut ctx, guest_hwnd) = ctx_with_window(0xCAFE);
        assert!(matches!(
            classify_dispatch(&ctx, guest_hwnd),
            Dispatch::Guest(0xCAFE)
        ));
        // A system EDIT class (never registered) -> Builtin.
        let edit = WinHandle(0x0002_0000);
        ctx.windows.insert(edit.0, edit_window(edit));
        assert!(matches!(classify_dispatch(&ctx, edit), Dispatch::Builtin));
        // Unknown hwnd -> None.
        assert!(matches!(
            classify_dispatch(&ctx, WinHandle(0x9999)),
            Dispatch::None
        ));
    }

    #[test]
    fn edit_control_accumulates_typed_chars_via_dispatch() {
        // The real Notepad text mechanism: WM_CHAR dispatched to an EDIT control
        // accumulates into its text buffer (readable via GetWindowTextW).
        let (mut ctx, _hwnd) = ctx_with_window(0);
        let edit = WinHandle(0x0002_0000);
        ctx.windows.insert(edit.0, edit_window(edit));
        let mut send = |ctx: &mut CompatContext, ch: u64| {
            let msg = Msg {
                hwnd: edit,
                message: crate::WM_CHAR,
                wparam: ch,
                lparam: 0,
                time: 0,
                pt: Point { x: 0, y: 0 },
            };
            dispatch_message_w(ctx, &msg);
        };
        send(&mut ctx, b'H' as u64);
        send(&mut ctx, b'I' as u64);
        assert_eq!(get_window_text(&ctx, edit).as_deref(), Some("HI"));
        // Backspace deletes the last char.
        send(&mut ctx, 0x08);
        assert_eq!(get_window_text(&ctx, edit).as_deref(), Some("H"));
        // A non-printable control char is ignored (no corruption).
        send(&mut ctx, 0x01);
        assert_eq!(get_window_text(&ctx, edit).as_deref(), Some("H"));
        // Enter inserts a newline (multiline EDIT).
        send(&mut ctx, 0x0D);
        assert_eq!(get_window_text(&ctx, edit).as_deref(), Some("H\n"));
    }

    #[test]
    fn edit_control_wm_paint_renders_text() {
        // WM_PAINT to an EDIT control renders its text into its surface: a white
        // background fill + black 8x8 glyphs. Pixel-exact + FAIL-able.
        let (mut ctx, _hwnd) = ctx_with_window(0);
        let edit = WinHandle(0x0004_0000);
        let mut surf = alloc::vec![0u32; 64 * 16];
        let surf_ptr = surf.as_mut_ptr() as u64;
        let mut w = edit_window(edit);
        w.rect = Rect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 16,
        };
        w.client_rect = Rect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 16,
        };
        w.surface_vaddr = Some(surf_ptr);
        w.title = String::from("HI");
        ctx.windows.insert(edit.0, w);
        let msg = Msg {
            hwnd: edit,
            message: crate::WM_PAINT,
            wparam: 0,
            lparam: 0,
            time: 0,
            pt: Point { x: 0, y: 0 },
        };
        dispatch_message_w(&mut ctx, &msg);
        let white = surf.iter().filter(|&&p| p == 0xFFFF_FFFF).count();
        let black = surf.iter().filter(|&&p| p == 0xFF00_0000).count();
        assert!(
            white > (64 * 16) / 2,
            "EDIT WM_PAINT fills the bg white (got {white})"
        );
        assert!(black > 0, "EDIT WM_PAINT blits text glyphs (got {black})");
    }

    #[test]
    fn menu_build_set_and_enumerate() {
        let (mut ctx, hwnd) = ctx_with_window(0);
        let bar = create_menu(&mut ctx);
        let file = create_popup_menu(&mut ctx);
        assert!(append_menu(&mut ctx, file, MF_STRING, 100, "Save").is_true());
        assert!(append_menu(&mut ctx, file, MF_SEPARATOR, 0, "").is_true());
        assert!(append_menu(&mut ctx, file, MF_STRING, 101, "Exit").is_true());
        assert!(append_menu(&mut ctx, bar, MF_POPUP, file.0, "File").is_true());
        // The File submenu has 3 items; string ids resolve, the separator is -1.
        assert_eq!(get_menu_item_count(&ctx, file), 3);
        assert_eq!(get_menu_item_id(&ctx, file, 0), 100);
        assert_eq!(get_menu_item_id(&ctx, file, 1), u32::MAX); // separator
        assert_eq!(get_menu_item_id(&ctx, file, 2), 101);
        // The bar has one popup item — a submenu, so its "id" is (UINT)-1.
        assert_eq!(get_menu_item_count(&ctx, bar), 1);
        assert_eq!(get_menu_item_id(&ctx, bar, 0), u32::MAX);
        // SetMenu/GetMenu round-trip; an unknown menu -> count -1.
        assert!(set_menu(&mut ctx, hwnd, bar).is_true());
        assert_eq!(get_menu(&ctx, hwnd).0, bar.0);
        assert_eq!(get_menu_item_count(&ctx, WinHandle(0xDEAD)), -1);
        // AppendMenu to an unknown menu is a handle error.
        assert!(!append_menu(&mut ctx, WinHandle(0xBEEF), MF_STRING, 1, "x").is_true());
        // Detach.
        assert!(set_menu(&mut ctx, hwnd, NULL_HANDLE).is_true());
        assert_eq!(get_menu(&ctx, hwnd).0, 0);
    }

    #[test]
    fn post_menu_command_enqueues_wm_command() {
        // Selecting a menu item posts WM_COMMAND(id) to the owner window — the
        // path the pump then delivers to the WndProc (delivery is proven by the
        // typing pump KAT, which uses the same dispatch).
        let (mut ctx, hwnd) = ctx_with_window(0);
        let before = ctx.message_queue.len();
        assert!(post_menu_command(&mut ctx, hwnd, 100).is_true());
        assert_eq!(ctx.message_queue.len(), before + 1);
        let m = ctx.message_queue.last().unwrap();
        assert_eq!(m.message, crate::WM_COMMAND);
        assert_eq!(m.wparam, 100, "WM_COMMAND low word = the menu item id");
        assert_eq!(m.hwnd, hwnd);
    }

    #[test]
    fn send_message_is_synchronous_not_enqueued() {
        // SendMessage must NOT push to the queue (that is PostMessage). With no
        // resolvable proc it returns 0 without touching the queue.
        let (mut ctx, hwnd) = ctx_with_window(0);
        let before = ctx.message_queue.len();
        let r = send_message_w(&mut ctx, hwnd, WM_CLOSE, 0, 0);
        assert_eq!(r, 0);
        assert_eq!(
            ctx.message_queue.len(),
            before,
            "SendMessage must not enqueue"
        );
    }

    #[test]
    fn send_message_unknown_hwnd_is_handle_error() {
        let (mut ctx, _hwnd) = ctx_with_window(0);
        let r = send_message_w(&mut ctx, WinHandle(0x4242), WM_CLOSE, 0, 0);
        assert_eq!(r, 0);
        assert_eq!(ctx.last_error, ERROR_INVALID_HANDLE);
    }

    #[test]
    fn post_message_enqueues_one() {
        let (mut ctx, hwnd) = ctx_with_window(0);
        let before = ctx.message_queue.len();
        assert!(post_message_w(&mut ctx, hwnd, WM_CLOSE, 0, 0).is_true());
        assert_eq!(
            ctx.message_queue.len(),
            before + 1,
            "PostMessage must enqueue exactly one"
        );
    }

    #[test]
    fn dispatch_with_no_proc_returns_zero() {
        // DispatchMessage on a window whose class has no proc returns 0 (and must
        // not panic / call a null pointer). The guest-call path is QEMU-proven.
        let (mut ctx, hwnd) = ctx_with_window(0);
        let msg = Msg {
            hwnd,
            message: WM_PAINT,
            wparam: 0,
            lparam: 0,
            time: 0,
            pt: Point { x: 0, y: 0 },
        };
        assert_eq!(dispatch_message_w(&mut ctx, &msg), 0);
    }

    #[test]
    fn update_window_paints_known_and_errors_unknown() {
        // UpdateWindow drives a synchronous WM_PAINT via SendMessage; with a
        // no-proc window the dispatch is a clean no-op (returns success), and an
        // unknown hwnd is a handle error.
        let (mut ctx, hwnd) = ctx_with_window(0);
        assert_eq!(
            update_window(&mut ctx, hwnd).0,
            1,
            "UpdateWindow on a live window succeeds"
        );
        assert_eq!(
            update_window(&mut ctx, WinHandle(0x4242)).0,
            0,
            "UpdateWindow on an unknown hwnd fails"
        );
        assert_eq!(ctx.last_error, ERROR_INVALID_HANDLE);
    }
}
