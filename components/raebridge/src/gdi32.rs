//! gdi32.dll — Graphics Device Interface API stubs for AthBridge.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    CompatContext, HandleType, Point, Rect, Size, WinBool, WinHandle, ERROR_INVALID_HANDLE,
    ERROR_INVALID_PARAMETER, ERROR_SUCCESS, FALSE, NULL_HANDLE, TRUE,
};

fn set_last_error(ctx: &mut CompatContext, code: u32) {
    ctx.last_error = code;
}

// =========================================================================
// GDI object tracking
// =========================================================================

static NEXT_GDI_HANDLE: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0xA000_0001);

fn alloc_gdi_handle() -> WinHandle {
    let v = NEXT_GDI_HANDLE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    WinHandle(v)
}

#[derive(Debug, Clone)]
pub enum GdiObject {
    Brush { color: u32 },
    Pen { style: i32, width: i32, color: u32 },
    Font { height: i32, face_name: String },
    Bitmap { width: i32, height: i32, bpp: u16 },
    Dc { target: WinHandle },
    Region,
}

// =========================================================================
// Device context operations
// =========================================================================

pub fn create_dc_w(
    ctx: &mut CompatContext,
    _driver: Option<&[u16]>,
    _device: Option<&[u16]>,
    _output: Option<&[u16]>,
    _init_data: u64,
) -> WinHandle {
    let h = alloc_gdi_handle();
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

pub fn delete_dc(ctx: &mut CompatContext, hdc: WinHandle) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn create_compatible_dc(ctx: &mut CompatContext, hdc: WinHandle) -> WinHandle {
    let _ = hdc;
    let h = alloc_gdi_handle();
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

pub fn select_object(ctx: &mut CompatContext, hdc: WinHandle, obj: WinHandle) -> WinHandle {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return NULL_HANDLE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    obj // return previous object (simplified)
}

pub fn delete_object(ctx: &mut CompatContext, obj: WinHandle) -> WinBool {
    if obj.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Brush, pen, font creation
// =========================================================================

pub fn create_solid_brush(ctx: &mut CompatContext, color: u32) -> WinHandle {
    let h = alloc_gdi_handle();
    ctx.gdi_objects.insert(h.0, GdiObject::Brush { color });
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

// =========================================================================
// Software rasterization into a window surface (WM_PAINT path)
// =========================================================================

/// Convert a Win32 `COLORREF` (0x00BBGGRR) to a surface ARGB pixel (0xAARRGGBB,
/// opaque). The compositor surface created by `CreateWindowEx` is 32-bit ARGB.
pub fn colorref_to_argb(color: u32) -> u32 {
    let r = color & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = (color >> 16) & 0xFF;
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// Pure software fill: set every pixel inside `rect` (clipped to the
/// `width`×`height` buffer) to `argb`. Host-KAT'able — no surface, no syscall.
/// Bounds-checked so a hostile rect can never write out of `buf`.
pub fn fill_rect_pixels(buf: &mut [u32], width: i32, height: i32, rect: &Rect, argb: u32) {
    if width <= 0 || height <= 0 {
        return;
    }
    let x0 = rect.left.max(0);
    let y0 = rect.top.max(0);
    let x1 = rect.right.min(width);
    let y1 = rect.bottom.min(height);
    for y in y0..y1 {
        let row = (y as usize).wrapping_mul(width as usize);
        for x in x0..x1 {
            let idx = row + x as usize;
            if idx < buf.len() {
                buf[idx] = argb;
            }
        }
    }
}

/// Minimal 8x8 bitmap font (top row first; bit `0x80` = leftmost pixel).
/// Covers the subset needed to render basic strings; unsupported chars render
/// blank. The blit is verified pixel-exact against this table by the host KAT,
/// so adding glyphs is safe and incremental.
pub fn glyph8x8(c: u8) -> [u8; 8] {
    match c {
        b' ' => [0, 0, 0, 0, 0, 0, 0, 0],
        b'A' => [0x18, 0x24, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x00],
        b'D' => [0x7C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7C, 0x00],
        b'E' => [0x7E, 0x40, 0x40, 0x7C, 0x40, 0x40, 0x7E, 0x00],
        b'G' => [0x3C, 0x42, 0x40, 0x4E, 0x42, 0x42, 0x3C, 0x00],
        b'H' => [0x42, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x00],
        b'I' => [0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00],
        b'L' => [0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x7E, 0x00],
        b'N' => [0x42, 0x62, 0x52, 0x4A, 0x46, 0x42, 0x42, 0x00],
        b'O' => [0x3C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x3C, 0x00],
        b'R' => [0x7C, 0x42, 0x42, 0x7C, 0x48, 0x44, 0x42, 0x00],
        b'S' => [0x3C, 0x42, 0x40, 0x3C, 0x02, 0x42, 0x3C, 0x00],
        b'T' => [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00],
        b'W' => [0x42, 0x42, 0x42, 0x5A, 0x5A, 0x66, 0x42, 0x00],
        b'0' => [0x3C, 0x46, 0x4A, 0x52, 0x62, 0x42, 0x3C, 0x00],
        b'1' => [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00],
        b'2' => [0x3C, 0x42, 0x02, 0x0C, 0x30, 0x40, 0x7E, 0x00],
        b'3' => [0x3C, 0x42, 0x02, 0x1C, 0x02, 0x42, 0x3C, 0x00],
        _ => [0, 0, 0, 0, 0, 0, 0, 0],
    }
}

/// Pure 8x8 text blit: render `text` (ASCII bytes) from `(x, y)`, one glyph per
/// 8px column, in `argb`, clipped to the `width`×`height` buffer. Host-KAT'able
/// — no surface, no syscall; bounds-checked so a long string or off-screen
/// origin can never write out of `buf`.
pub fn blit_text(buf: &mut [u32], width: i32, height: i32, x: i32, y: i32, text: &[u8], argb: u32) {
    if width <= 0 || height <= 0 {
        return;
    }
    for (i, &ch) in text.iter().enumerate() {
        let gx = x + (i as i32) * 8;
        let glyph = glyph8x8(ch);
        for (row, bits) in glyph.iter().enumerate() {
            let py = y + row as i32;
            if py < 0 || py >= height {
                continue;
            }
            for col in 0..8i32 {
                if bits & (0x80u8 >> col) != 0 {
                    let px = gx + col;
                    if px < 0 || px >= width {
                        continue;
                    }
                    let idx = (py as usize) * (width as usize) + px as usize;
                    if idx < buf.len() {
                        buf[idx] = argb;
                    }
                }
            }
        }
    }
}

/// The window a DC targets (`GetDC`/`BeginPaint` bind it), if any.
fn dc_target(ctx: &CompatContext, hdc: WinHandle) -> Option<WinHandle> {
    match ctx.gdi_objects.get(&hdc.0) {
        Some(GdiObject::Dc { target }) => Some(*target),
        _ => None,
    }
}

/// A brush's COLORREF, if `brush` is a stored solid brush.
fn brush_color(ctx: &CompatContext, brush: WinHandle) -> Option<u32> {
    match ctx.gdi_objects.get(&brush.0) {
        Some(GdiObject::Brush { color }) => Some(*color),
        _ => None,
    }
}

/// `GetDC(hwnd)` — a device context bound to a window's client area, stored so
/// later GDI ops reach that window's surface buffer.
pub fn get_dc(ctx: &mut CompatContext, hwnd: WinHandle) -> WinHandle {
    let h = alloc_gdi_handle();
    ctx.gdi_objects.insert(h.0, GdiObject::Dc { target: hwnd });
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

/// `ReleaseDC(hwnd, hdc)` — drop a window DC.
pub fn release_dc(ctx: &mut CompatContext, _hwnd: WinHandle, hdc: WinHandle) -> i32 {
    ctx.gdi_objects.remove(&hdc.0);
    set_last_error(ctx, ERROR_SUCCESS);
    1
}

/// `BeginPaint(hwnd, &mut PAINTSTRUCT)` -> HDC. Returns a window-bound DC and
/// fills the paint struct's `hdc` + `rcPaint` (the client rect).
pub fn begin_paint(
    ctx: &mut CompatContext,
    hwnd: WinHandle,
    ps: &mut crate::PaintStruct,
) -> WinHandle {
    let rc = ctx
        .windows
        .get(&hwnd.0)
        .map(|w| w.client_rect)
        .unwrap_or_default();
    let hdc = get_dc(ctx, hwnd);
    ps.hdc = hdc;
    ps.erase = TRUE;
    ps.rc_paint = rc;
    hdc
}

/// `EndPaint(hwnd)` -> presents the window's surface to the compositor so the
/// pixels rastered between Begin/EndPaint become visible.
pub fn end_paint(ctx: &mut CompatContext, hwnd: WinHandle) -> WinBool {
    if let Some(win) = ctx.windows.get(&hwnd.0) {
        if let Some(sid) = win.surface_id {
            unsafe { crate::syscalls::sys_surface_present(sid, win.rect.left, win.rect.top) };
        }
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn create_pen(ctx: &mut CompatContext, style: i32, width: i32, color: u32) -> WinHandle {
    let _ = (style, width, color);
    let h = alloc_gdi_handle();
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

pub fn create_font_w(
    ctx: &mut CompatContext,
    height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    weight: i32,
    italic: u32,
    underline: u32,
    strike_out: u32,
    char_set: u32,
    out_precision: u32,
    clip_precision: u32,
    quality: u32,
    pitch_and_family: u32,
    face_name: &[u16],
) -> WinHandle {
    let _ = (
        width,
        escapement,
        orientation,
        weight,
        italic,
        underline,
        strike_out,
        char_set,
        out_precision,
        clip_precision,
        quality,
        pitch_and_family,
    );
    let _name = crate::wide_to_string(face_name);
    let _ = height;
    let h = alloc_gdi_handle();
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

// =========================================================================
// Text output
// =========================================================================

pub fn text_out_w(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    x: i32,
    y: i32,
    text: &[u16],
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    // Render into the target window's surface (window-bound DC only). Default
    // opaque-black text (correct for the common white-background case); honoring
    // SetTextColor is a follow-up. Non-ASCII chars fall back to a blank glyph.
    let Some(hwnd) = dc_target(ctx, hdc) else {
        return TRUE;
    };
    let Some(win) = ctx.windows.get(&hwnd.0) else {
        return TRUE;
    };
    let w = win.client_rect.right - win.client_rect.left;
    let h = win.client_rect.bottom - win.client_rect.top;
    let Some(vaddr) = win.surface_vaddr else {
        return TRUE;
    };
    if w <= 0 || h <= 0 {
        return TRUE;
    }
    let bytes: Vec<u8> = text
        .iter()
        .map(|&u| if u < 0x80 { u as u8 } else { b'?' })
        .collect();
    // SAFETY: `vaddr` is the window's `w*h` ARGB32 compositor surface (guest VA ==
    // host VA in-process); `blit_text` clips every pixel to `w*h`.
    let buf = unsafe { core::slice::from_raw_parts_mut(vaddr as *mut u32, (w * h) as usize) };
    blit_text(buf, w, h, x, y, &bytes, 0xFF00_0000);
    TRUE
}

pub fn ext_text_out_w(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    x: i32,
    y: i32,
    options: u32,
    rect: Option<&Rect>,
    text: &[u16],
    dx: Option<&[i32]>,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = (x, y, options, rect, text, dx);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_text_extent_point32_w(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    text: &[u16],
    size: &mut Size,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    // Approximate character dimensions: 8 pixels wide, 16 pixels tall
    size.cx = text.len() as i32 * 8;
    size.cy = 16;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Shape drawing
// =========================================================================

pub fn rectangle(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = (left, top, right, bottom);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn ellipse(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = (left, top, right, bottom);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn move_to_ex(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    x: i32,
    y: i32,
    prev_point: Option<&mut Point>,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    if let Some(pt) = prev_point {
        pt.x = 0;
        pt.y = 0;
    }
    let _ = (x, y);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn line_to(ctx: &mut CompatContext, hdc: WinHandle, x: i32, y: i32) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = (x, y);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn polygon(ctx: &mut CompatContext, hdc: WinHandle, points: &[Point]) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    if points.len() < 2 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn polyline(ctx: &mut CompatContext, hdc: WinHandle, points: &[Point]) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    if points.len() < 2 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Bit-block transfer
// =========================================================================

pub fn bit_blt(
    ctx: &mut CompatContext,
    hdc_dest: WinHandle,
    x_dest: i32,
    y_dest: i32,
    width: i32,
    height: i32,
    hdc_src: WinHandle,
    x_src: i32,
    y_src: i32,
    rop: u32,
) -> WinBool {
    if hdc_dest.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = (x_dest, y_dest, width, height, hdc_src, x_src, y_src, rop);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn stretch_blt(
    ctx: &mut CompatContext,
    hdc_dest: WinHandle,
    x_dest: i32,
    y_dest: i32,
    w_dest: i32,
    h_dest: i32,
    hdc_src: WinHandle,
    x_src: i32,
    y_src: i32,
    w_src: i32,
    h_src: i32,
    rop: u32,
) -> WinBool {
    if hdc_dest.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = (
        x_dest, y_dest, w_dest, h_dest, hdc_src, x_src, y_src, w_src, h_src, rop,
    );
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn stretch_di_bits(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    x_dest: i32,
    y_dest: i32,
    dest_width: i32,
    dest_height: i32,
    x_src: i32,
    y_src: i32,
    src_width: i32,
    src_height: i32,
    bits: &[u8],
    _bmi: u64,
    usage: u32,
    rop: u32,
) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    let _ = (
        x_dest,
        y_dest,
        dest_width,
        dest_height,
        x_src,
        y_src,
        src_width,
        src_height,
        bits,
        usage,
        rop,
    );
    set_last_error(ctx, ERROR_SUCCESS);
    src_height
}

// =========================================================================
// Bitmap operations
// =========================================================================

pub fn create_bitmap(
    ctx: &mut CompatContext,
    width: i32,
    height: i32,
    planes: u32,
    bit_count: u32,
    _bits: u64,
) -> WinHandle {
    if width <= 0 || height <= 0 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return NULL_HANDLE;
    }
    let _ = (planes, bit_count);
    let h = alloc_gdi_handle();
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

pub fn create_dib_section(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    _bmi: u64,
    usage: u32,
    bits_out: &mut u64,
    _section: WinHandle,
    _offset: u32,
) -> WinHandle {
    let _ = (hdc, usage);
    let h = alloc_gdi_handle();
    *bits_out = h.0 + 0x1000;
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

pub fn get_di_bits(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    hbm: WinHandle,
    start: u32,
    lines: u32,
    buffer: &mut [u8],
    _bmi: u64,
    usage: u32,
) -> i32 {
    if hdc.0 == 0 || hbm.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    let _ = (start, usage);
    for b in buffer.iter_mut() {
        *b = 0;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    lines as i32
}

pub fn set_di_bits(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    hbm: WinHandle,
    start: u32,
    lines: u32,
    bits: &[u8],
    _bmi: u64,
    usage: u32,
) -> i32 {
    if hdc.0 == 0 || hbm.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    let _ = (start, bits, usage);
    set_last_error(ctx, ERROR_SUCCESS);
    lines as i32
}

// =========================================================================
// Pixel operations
// =========================================================================

pub fn set_pixel(ctx: &mut CompatContext, hdc: WinHandle, x: i32, y: i32, color: u32) -> u32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0xFFFFFFFF;
    }
    let _ = (x, y);
    set_last_error(ctx, ERROR_SUCCESS);
    color
}

pub fn get_pixel(ctx: &mut CompatContext, hdc: WinHandle, x: i32, y: i32) -> u32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0xFFFFFFFF;
    }
    let _ = (x, y);
    set_last_error(ctx, ERROR_SUCCESS);
    0x00000000 // black
}

pub fn fill_rect(ctx: &mut CompatContext, hdc: WinHandle, rect: &Rect, brush: WinHandle) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    // Resolve the brush color (default opaque black if the brush isn't a stored
    // solid brush — e.g. a stock-brush handle we don't model yet).
    let argb = colorref_to_argb(brush_color(ctx, brush).unwrap_or(0));
    set_last_error(ctx, ERROR_SUCCESS);
    // Only window-bound DCs have a surface to paint into; a memory/other DC is a
    // success no-op (nothing to show).
    let Some(hwnd) = dc_target(ctx, hdc) else {
        return 1;
    };
    // Copy the surface params out so the (immutable) window borrow ends before we
    // bump the (mutable) paint counter on ctx below.
    let (w, h, vaddr) = match ctx.windows.get(&hwnd.0) {
        Some(win) => (
            win.client_rect.right - win.client_rect.left,
            win.client_rect.bottom - win.client_rect.top,
            win.surface_vaddr,
        ),
        None => return 1,
    };
    let Some(vaddr) = vaddr else {
        return 1; // no backing surface (e.g. QEMU surface_create failed) -> no-op
    };
    if w <= 0 || h <= 0 {
        return 1;
    }
    // SAFETY: `vaddr` is the window's compositor surface buffer (created by
    // `CreateWindowEx` via `sys_surface_create`, a `w*h` ARGB32 region); guest
    // VA == host VA in the in-process model. `fill_rect_pixels` clips to w*h.
    let buf = unsafe { core::slice::from_raw_parts_mut(vaddr as *mut u32, (w * h) as usize) };
    fill_rect_pixels(buf, w, h, rect, argb);
    // Account the clamped filled area (observable "a guest painted" proof).
    let cw = (rect.right.min(w) - rect.left.max(0)).max(0);
    let ch = (rect.bottom.min(h) - rect.top.max(0)).max(0);
    ctx.gui_paint_pixels = ctx
        .gui_paint_pixels
        .saturating_add((cw as u64) * (ch as u64));
    1
}

/// Paint a control's own text into its surface: clear the client area to white,
/// then blit the window text (ASCII, 8x8 font, black) at a small inset. The
/// built-in EDIT control calls this on WM_PAINT so a real Notepad's edit child
/// actually shows the typed text. Returns the count of pixels cleared (the
/// observable "painted" measure, also folded into `gui_paint_pixels`). A window
/// with no backing surface (QEMU `sys_surface_create` failed) is a no-op → 0.
/// (Multiline `\n` layout renders as a blank glyph for now — a follow-up.)
pub fn paint_control_text(ctx: &mut CompatContext, hwnd: WinHandle) -> u64 {
    // Copy the surface params + text out so the immutable window borrow ends
    // before the mutable `gui_paint_pixels` bump below.
    let (w, h, vaddr, text) = match ctx.windows.get(&hwnd.0) {
        Some(win) => (
            win.client_rect.right - win.client_rect.left,
            win.client_rect.bottom - win.client_rect.top,
            win.surface_vaddr,
            win.title.clone(),
        ),
        None => return 0,
    };
    let Some(vaddr) = vaddr else {
        return 0; // no backing surface -> nothing to show
    };
    if w <= 0 || h <= 0 {
        return 0;
    }
    // SAFETY: `vaddr` is the window's `w*h` ARGB32 compositor surface (guest VA ==
    // host VA in-process); both `fill_rect_pixels` and `blit_text` clip to `w*h`.
    let buf = unsafe { core::slice::from_raw_parts_mut(vaddr as *mut u32, (w * h) as usize) };
    let full = Rect {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    fill_rect_pixels(buf, w, h, &full, 0xFFFF_FFFF); // white background
    let bytes: Vec<u8> = text
        .bytes()
        .map(|b| if b < 0x80 { b } else { b'?' })
        .collect();
    blit_text(buf, w, h, 2, 2, &bytes, 0xFF00_0000); // opaque-black text
    let painted = (w as u64) * (h as u64);
    ctx.gui_paint_pixels = ctx.gui_paint_pixels.saturating_add(painted);
    painted
}

pub fn frame_rect(ctx: &mut CompatContext, hdc: WinHandle, rect: &Rect, brush: WinHandle) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    let _ = (rect, brush);
    set_last_error(ctx, ERROR_SUCCESS);
    1
}

// =========================================================================
// Text and background attributes
// =========================================================================

pub fn set_bk_mode(ctx: &mut CompatContext, hdc: WinHandle, mode: i32) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    let _ = mode;
    set_last_error(ctx, ERROR_SUCCESS);
    crate::OPAQUE
}

pub fn set_text_color(ctx: &mut CompatContext, hdc: WinHandle, color: u32) -> u32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0xFFFFFFFF;
    }
    let _ = color;
    set_last_error(ctx, ERROR_SUCCESS);
    0x00000000 // previous: black
}

pub fn set_bk_color(ctx: &mut CompatContext, hdc: WinHandle, color: u32) -> u32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0xFFFFFFFF;
    }
    let _ = color;
    set_last_error(ctx, ERROR_SUCCESS);
    0x00FFFFFF // previous: white
}

pub fn get_text_color(ctx: &mut CompatContext, hdc: WinHandle) -> u32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0xFFFFFFFF;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    0x00000000
}

// =========================================================================
// DC state save / restore
// =========================================================================

pub fn save_dc(ctx: &mut CompatContext, hdc: WinHandle) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    1
}

pub fn restore_dc(ctx: &mut CompatContext, hdc: WinHandle, saved_dc: i32) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = saved_dc;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Device capabilities and stock objects
// =========================================================================

pub fn get_device_caps(ctx: &mut CompatContext, hdc: WinHandle, index: i32) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    match index {
        8 => 1920,   // HORZRES
        10 => 1080,  // VERTRES
        12 => 32,    // BITSPIXEL
        88 => 96,    // LOGPIXELSX
        90 => 96,    // LOGPIXELSY
        118 => 1920, // DESKTOPHORZRES
        117 => 1080, // DESKTOPVERTRES
        _ => 0,
    }
}

pub fn get_stock_object(_ctx: &mut CompatContext, index: i32) -> WinHandle {
    // Stock objects have fixed pseudo-handles in the 0x8000_xxxx range
    WinHandle(0x8000_0000 | (index as u64 & 0xFF))
}

// =========================================================================
// Raster operation codes (used by BitBlt/StretchBlt)
// =========================================================================

pub const SRCCOPY: u32 = 0x00CC0020;
pub const SRCPAINT: u32 = 0x00EE0086;
pub const SRCAND: u32 = 0x008800C6;
pub const SRCINVERT: u32 = 0x00660046;
pub const SRCERASE: u32 = 0x00440328;
pub const PATCOPY: u32 = 0x00F00021;
pub const PATPAINT: u32 = 0x00FB0A09;
pub const PATINVERT: u32 = 0x005A0049;
pub const BLACKNESS: u32 = 0x00000042;
pub const WHITENESS: u32 = 0x00FF0062;

// Pen styles
pub const PS_SOLID: i32 = 0;
pub const PS_DASH: i32 = 1;
pub const PS_DOT: i32 = 2;
pub const PS_DASHDOT: i32 = 3;
pub const PS_NULL: i32 = 5;

// Font weights
pub const FW_NORMAL: i32 = 400;
pub const FW_BOLD: i32 = 700;

// DIB color usage
pub const DIB_RGB_COLORS: u32 = 0;
pub const DIB_PAL_COLORS: u32 = 1;

// Region combine modes
pub const RGN_AND: i32 = 1;
pub const RGN_OR: i32 = 2;
pub const RGN_XOR: i32 = 3;
pub const RGN_DIFF: i32 = 4;
pub const RGN_COPY: i32 = 5;

// Region return values
pub const ERROR_RGN: i32 = 0;
pub const NULLREGION: i32 = 1;
pub const SIMPLEREGION: i32 = 2;
pub const COMPLEXREGION: i32 = 3;

// =========================================================================
// Compatible bitmap
// =========================================================================

pub fn create_compatible_bitmap(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    width: i32,
    height: i32,
) -> WinHandle {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return NULL_HANDLE;
    }
    if width <= 0 || height <= 0 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return NULL_HANDLE;
    }
    let h = alloc_gdi_handle();
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

// =========================================================================
// ANSI text output
// =========================================================================

fn cstr_to_wide(ptr: &[u8]) -> Vec<u16> {
    let end = ptr.iter().position(|&b| b == 0).unwrap_or(ptr.len());
    let mut wide = Vec::with_capacity(end + 1);
    for &b in &ptr[..end] {
        wide.push(b as u16);
    }
    wide.push(0);
    wide
}

pub fn text_out_a(ctx: &mut CompatContext, hdc: WinHandle, x: i32, y: i32, text: &[u8]) -> WinBool {
    let wide = cstr_to_wide(text);
    text_out_w(ctx, hdc, x, y, &wide)
}

pub fn get_text_extent_point32_a(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    text: &[u8],
    size: &mut Size,
) -> WinBool {
    let wide = cstr_to_wide(text);
    get_text_extent_point32_w(ctx, hdc, &wide, size)
}

pub fn ext_text_out_a(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    x: i32,
    y: i32,
    options: u32,
    rect: Option<&Rect>,
    text: &[u8],
    dx: Option<&[i32]>,
) -> WinBool {
    let wide = cstr_to_wide(text);
    ext_text_out_w(ctx, hdc, x, y, options, rect, &wide, dx)
}

// =========================================================================
// Region and clipping
// =========================================================================

pub fn create_rect_rgn(
    ctx: &mut CompatContext,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> WinHandle {
    let _ = (left, top, right, bottom);
    let h = alloc_gdi_handle();
    set_last_error(ctx, ERROR_SUCCESS);
    h
}

pub fn create_rect_rgn_indirect(ctx: &mut CompatContext, rect: &Rect) -> WinHandle {
    create_rect_rgn(ctx, rect.left, rect.top, rect.right, rect.bottom)
}

pub fn select_clip_rgn(ctx: &mut CompatContext, hdc: WinHandle, rgn: WinHandle) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return ERROR_RGN;
    }
    let _ = rgn;
    set_last_error(ctx, ERROR_SUCCESS);
    SIMPLEREGION
}

pub fn combine_rgn(
    ctx: &mut CompatContext,
    dest: WinHandle,
    src1: WinHandle,
    src2: WinHandle,
    mode: i32,
) -> i32 {
    if dest.0 == 0 || src1.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return ERROR_RGN;
    }
    let _ = (src2, mode);
    set_last_error(ctx, ERROR_SUCCESS);
    SIMPLEREGION
}

// =========================================================================
// Additional drawing operations
// =========================================================================

pub fn round_rect(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    width: i32,
    height: i32,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = (left, top, right, bottom, width, height);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn arc(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    x_start: i32,
    y_start: i32,
    x_end: i32,
    y_end: i32,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = (left, top, right, bottom, x_start, y_start, x_end, y_end);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn set_rop2(ctx: &mut CompatContext, hdc: WinHandle, mode: i32) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    let _ = mode;
    set_last_error(ctx, ERROR_SUCCESS);
    13 // R2_COPYPEN (default)
}

pub fn get_current_position_ex(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    point: &mut Point,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    point.x = 0;
    point.y = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Mapping and coordinate transforms
// =========================================================================

pub fn set_map_mode(ctx: &mut CompatContext, hdc: WinHandle, mode: i32) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    let _ = mode;
    set_last_error(ctx, ERROR_SUCCESS);
    1 // MM_TEXT (previous)
}

pub fn set_viewport_org_ex(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    x: i32,
    y: i32,
    prev: Option<&mut Point>,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    if let Some(p) = prev {
        p.x = 0;
        p.y = 0;
    }
    let _ = (x, y);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn set_window_org_ex(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    x: i32,
    y: i32,
    prev: Option<&mut Point>,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    if let Some(p) = prev {
        p.x = 0;
        p.y = 0;
    }
    let _ = (x, y);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Text metrics
// =========================================================================

#[derive(Debug, Clone, Copy, Default)]
pub struct TextMetric {
    pub height: i32,
    pub ascent: i32,
    pub descent: i32,
    pub internal_leading: i32,
    pub external_leading: i32,
    pub ave_char_width: i32,
    pub max_char_width: i32,
    pub weight: i32,
    pub overhang: i32,
    pub digitized_aspect_x: i32,
    pub digitized_aspect_y: i32,
    pub first_char: u16,
    pub last_char: u16,
    pub default_char: u16,
    pub break_char: u16,
    pub italic: u8,
    pub underlined: u8,
    pub struck_out: u8,
    pub pitch_and_family: u8,
    pub char_set: u8,
}

pub fn get_text_metrics_w(ctx: &mut CompatContext, hdc: WinHandle, tm: &mut TextMetric) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    tm.height = 16;
    tm.ascent = 13;
    tm.descent = 3;
    tm.internal_leading = 1;
    tm.external_leading = 0;
    tm.ave_char_width = 8;
    tm.max_char_width = 16;
    tm.weight = 400;
    tm.first_char = 0x20;
    tm.last_char = 0xFF;
    tm.default_char = 0x3F;
    tm.break_char = 0x20;
    tm.char_set = 0; // ANSI_CHARSET
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_text_metrics_a(ctx: &mut CompatContext, hdc: WinHandle, tm: &mut TextMetric) -> WinBool {
    get_text_metrics_w(ctx, hdc, tm)
}

// =========================================================================
// Mapping modes
// =========================================================================

pub const MM_TEXT: i32 = 1;
pub const MM_LOMETRIC: i32 = 2;
pub const MM_HIMETRIC: i32 = 3;
pub const MM_LOENGLISH: i32 = 4;
pub const MM_HIENGLISH: i32 = 5;
pub const MM_TWIPS: i32 = 6;
pub const MM_ISOTROPIC: i32 = 7;
pub const MM_ANISOTROPIC: i32 = 8;

// =========================================================================
// Paths
// =========================================================================

pub fn begin_path(ctx: &mut CompatContext, hdc: WinHandle) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn end_path(ctx: &mut CompatContext, hdc: WinHandle) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn stroke_path(ctx: &mut CompatContext, hdc: WinHandle) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn fill_path(ctx: &mut CompatContext, hdc: WinHandle) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn stroke_and_fill_path(ctx: &mut CompatContext, hdc: WinHandle) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn close_figure(ctx: &mut CompatContext, hdc: WinHandle) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    TRUE
}

// =========================================================================
// Regions (extended)
// =========================================================================

pub fn get_rgn_box(_ctx: &CompatContext, _rgn: WinHandle, rect: &mut Rect) -> i32 {
    rect.left = 0;
    rect.top = 0;
    rect.right = 0;
    rect.bottom = 0;
    1 // NULLREGION
}

pub fn offset_rgn(_ctx: &mut CompatContext, _rgn: WinHandle, _x: i32, _y: i32) -> i32 {
    1 // SIMPLEREGION
}

pub fn pt_in_region(_ctx: &CompatContext, _rgn: WinHandle, _x: i32, _y: i32) -> WinBool {
    FALSE
}

pub fn rect_in_region(_ctx: &CompatContext, _rgn: WinHandle, _rect: &Rect) -> WinBool {
    FALSE
}

pub fn equal_rgn(_ctx: &CompatContext, _rgn1: WinHandle, _rgn2: WinHandle) -> WinBool {
    FALSE
}

pub fn set_rect_rgn(
    _ctx: &mut CompatContext,
    _rgn: WinHandle,
    _left: i32,
    _top: i32,
    _right: i32,
    _bottom: i32,
) -> WinBool {
    TRUE
}

pub fn create_elliptic_rgn(
    ctx: &mut CompatContext,
    _left: i32,
    _top: i32,
    _right: i32,
    _bottom: i32,
) -> WinHandle {
    let h = ctx.handle_table.allocate(HandleType::GdiObj, 0, None);
    WinHandle(h)
}

pub fn create_polygon_rgn(
    ctx: &mut CompatContext,
    _points: &[Point],
    _fill_mode: i32,
) -> WinHandle {
    let h = ctx.handle_table.allocate(HandleType::GdiObj, 0, None);
    WinHandle(h)
}

// =========================================================================
// Palette
// =========================================================================

pub fn create_palette(ctx: &mut CompatContext, _log_palette: u64) -> WinHandle {
    let h = ctx.handle_table.allocate(HandleType::GdiObj, 0, None);
    WinHandle(h)
}

pub fn select_palette(
    _ctx: &mut CompatContext,
    _hdc: WinHandle,
    palette: WinHandle,
    _force_background: WinBool,
) -> WinHandle {
    palette
}

pub fn realize_palette(_ctx: &mut CompatContext, _hdc: WinHandle) -> u32 {
    0
}

pub fn get_system_palette_entries(
    _ctx: &CompatContext,
    _hdc: WinHandle,
    _start: u32,
    _entries: u32,
    _palette_entries: u64,
) -> u32 {
    0
}

pub fn get_nearest_color(_ctx: &CompatContext, _hdc: WinHandle, color: u32) -> u32 {
    color
}

pub fn get_nearest_palette_index(_ctx: &CompatContext, _palette: WinHandle, _color: u32) -> u32 {
    0
}

// =========================================================================
// Extended drawing
// =========================================================================

pub fn pie(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    _left: i32,
    _top: i32,
    _right: i32,
    _bottom: i32,
    _xr1: i32,
    _yr1: i32,
    _xr2: i32,
    _yr2: i32,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn chord(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    _left: i32,
    _top: i32,
    _right: i32,
    _bottom: i32,
    _xr1: i32,
    _yr1: i32,
    _xr2: i32,
    _yr2: i32,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    TRUE
}

pub fn poly_bezier(ctx: &mut CompatContext, hdc: WinHandle, _points: &[Point]) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    TRUE
}

pub fn poly_bezier_to(ctx: &mut CompatContext, hdc: WinHandle, _points: &[Point]) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    TRUE
}

pub fn polyline_to(ctx: &mut CompatContext, hdc: WinHandle, _points: &[Point]) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    TRUE
}

pub fn set_stretch_blt_mode(ctx: &mut CompatContext, hdc: WinHandle, _mode: i32) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    1 // previous mode
}

pub fn set_brush_org_ex(
    _ctx: &mut CompatContext,
    _hdc: WinHandle,
    _x: i32,
    _y: i32,
    _prev_pt: Option<&mut Point>,
) -> WinBool {
    TRUE
}

pub fn gradient_fill(
    ctx: &mut CompatContext,
    hdc: WinHandle,
    _vertices: u64,
    _num_vertex: u32,
    _mesh: u64,
    _num_mesh: u32,
    _mode: u32,
) -> WinBool {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn alpha_blend(
    ctx: &mut CompatContext,
    _dest_dc: WinHandle,
    _dest_x: i32,
    _dest_y: i32,
    _dest_w: i32,
    _dest_h: i32,
    _src_dc: WinHandle,
    _src_x: i32,
    _src_y: i32,
    _src_w: i32,
    _src_h: i32,
    _blend_func: u32,
) -> WinBool {
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn transparent_blt(
    ctx: &mut CompatContext,
    _dest_dc: WinHandle,
    _dest_x: i32,
    _dest_y: i32,
    _dest_w: i32,
    _dest_h: i32,
    _src_dc: WinHandle,
    _src_x: i32,
    _src_y: i32,
    _src_w: i32,
    _src_h: i32,
    _transparent_color: u32,
) -> WinBool {
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Font enumeration
// =========================================================================

pub fn enum_font_families_ex_w(
    _ctx: &mut CompatContext,
    _hdc: WinHandle,
    _log_font: u64,
    _callback: u64,
    _lparam: isize,
    _flags: u32,
) -> i32 {
    1
}

pub fn get_text_face_w(ctx: &mut CompatContext, hdc: WinHandle, buf: &mut [u16]) -> i32 {
    if hdc.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    let face = "Segoe UI";
    let needed = face.len();
    for (i, ch) in face.bytes().enumerate() {
        if i >= buf.len() {
            break;
        }
        buf[i] = ch as u16;
    }
    if needed < buf.len() {
        buf[needed] = 0;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    needed as i32
}

pub fn add_font_resource_ex_w(
    _ctx: &mut CompatContext,
    _name: &[u16],
    _flags: u32,
    _reserved: u64,
) -> i32 {
    1
}

pub fn remove_font_resource_ex_w(
    _ctx: &mut CompatContext,
    _name: &[u16],
    _flags: u32,
    _reserved: u64,
) -> WinBool {
    TRUE
}

// =========================================================================
// Miscellaneous GDI
// =========================================================================

pub fn get_object_w(_ctx: &CompatContext, _obj: WinHandle, _buf_size: i32, _buf: u64) -> i32 {
    0
}

pub fn set_world_transform(_ctx: &mut CompatContext, _hdc: WinHandle, _xform: u64) -> WinBool {
    TRUE
}

pub fn get_world_transform(_ctx: &CompatContext, _hdc: WinHandle, _xform: u64) -> WinBool {
    TRUE
}

pub fn set_graphics_mode(_ctx: &mut CompatContext, _hdc: WinHandle, _mode: i32) -> i32 {
    1 // previous mode (GM_COMPATIBLE)
}

pub fn get_clip_box(_ctx: &CompatContext, _hdc: WinHandle, rect: &mut Rect) -> i32 {
    rect.left = 0;
    rect.top = 0;
    rect.right = 1920;
    rect.bottom = 1080;
    1 // SIMPLEREGION
}

pub fn intersect_clip_rect(
    _ctx: &mut CompatContext,
    _hdc: WinHandle,
    _left: i32,
    _top: i32,
    _right: i32,
    _bottom: i32,
) -> i32 {
    1 // SIMPLEREGION
}

pub fn exclude_clip_rect(
    _ctx: &mut CompatContext,
    _hdc: WinHandle,
    _left: i32,
    _top: i32,
    _right: i32,
    _bottom: i32,
) -> i32 {
    1 // SIMPLEREGION
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{testpe, FullCompatSession, SessionId, WindowObject};
    use alloc::vec;

    #[test]
    fn fill_rect_pixels_clips_to_buffer() {
        // A rect exceeding the buffer on every side clips to a full fill, never
        // overruns.
        let mut buf = vec![0u32; 4 * 3];
        fill_rect_pixels(
            &mut buf,
            4,
            3,
            &Rect {
                left: -2,
                top: -1,
                right: 100,
                bottom: 100,
            },
            0xFFFF_FFFF,
        );
        assert!(
            buf.iter().all(|&p| p == 0xFFFF_FFFF),
            "clip-fill covers whole buffer"
        );
        // An empty/inverted rect writes nothing.
        let mut b2 = vec![0u32; 12];
        fill_rect_pixels(
            &mut b2,
            4,
            3,
            &Rect {
                left: 2,
                top: 2,
                right: 1,
                bottom: 1,
            },
            0xFF,
        );
        assert!(b2.iter().all(|&p| p == 0), "inverted rect writes nothing");
    }

    #[test]
    fn blit_text_renders_glyph_bits_and_advances() {
        // Render "HI" into a 16x8 buffer; each lit glyph bit must become `argb`,
        // the second char must start at column 8, and unlit pixels stay 0.
        let w = 16i32;
        let h = 8i32;
        let mut buf = vec![0u32; (w * h) as usize];
        let argb = 0xFFAB_CDEF;
        blit_text(&mut buf, w, h, 0, 0, b"HI", argb);
        for (ci, &ch) in b"HI".iter().enumerate() {
            let g = glyph8x8(ch);
            let gx = ci as i32 * 8;
            for (row, bits) in g.iter().enumerate() {
                for col in 0..8i32 {
                    let lit = bits & (0x80u8 >> col) != 0;
                    let px = buf[(row as i32 * w + gx + col) as usize];
                    if lit {
                        assert_eq!(px, argb, "lit bit {ch} r{row} c{col}");
                    } else {
                        assert_eq!(px, 0, "unlit bit {ch} r{row} c{col}");
                    }
                }
            }
        }
    }

    #[test]
    fn blit_text_clips_off_screen_origin() {
        // A negative origin and a string longer than the buffer must clip, never
        // panic or write out of bounds.
        let mut buf = vec![0u32; 8 * 8];
        blit_text(&mut buf, 8, 8, -4, -3, b"HELLO RAEEN OS", 0xFFFF_FFFF);
        // No assertion on contents beyond "did not panic / overrun"; the bounds
        // checks in blit_text guarantee safety. Touch the buffer to keep it live.
        let _ = buf[0];
    }

    #[test]
    fn colorref_to_argb_swaps_channels() {
        // COLORREF 0x00BBGGRR -> opaque ARGB 0xFFRRGGBB.
        assert_eq!(colorref_to_argb(0x0000_00FF), 0xFFFF_0000, "red");
        assert_eq!(colorref_to_argb(0x00FF_0000), 0xFF00_00FF, "blue");
        assert_eq!(colorref_to_argb(0x0000_FF00), 0xFF00_FF00, "green");
    }

    #[test]
    fn fill_rect_renders_into_window_surface() {
        let exe = testpe::build_exit_process_exe();
        let mut ctx =
            FullCompatSession::new(SessionId(41), "paint.exe".into(), exe, "paint.exe".into())
                .unwrap();
        // An 8x4 ARGB surface backed by a host buffer (the in-process model lets
        // the real fill_rect path render into it).
        let mut surf = vec![0u32; 8 * 4];
        let hwnd = WinHandle(0x0001_0000);
        ctx.windows.insert(
            hwnd.0,
            WindowObject {
                handle: hwnd,
                class_name: String::new(),
                title: String::new(),
                style: 0,
                ex_style: 0,
                rect: Rect {
                    left: 0,
                    top: 0,
                    right: 8,
                    bottom: 4,
                },
                client_rect: Rect {
                    left: 0,
                    top: 0,
                    right: 8,
                    bottom: 4,
                },
                parent: NULL_HANDLE,
                visible: true,
                enabled: true,
                user_data: 0,
                surface_id: None,
                surface_vaddr: Some(surf.as_mut_ptr() as u64),
            },
        );
        let brush = create_solid_brush(&mut ctx, 0x0000_00FF); // red COLORREF
        let hdc = get_dc(&mut ctx, hwnd);
        let r = fill_rect(
            &mut ctx,
            hdc,
            &Rect {
                left: 1,
                top: 1,
                right: 4,
                bottom: 3,
            },
            brush,
        );
        assert_eq!(r, 1, "FillRect into a window DC succeeds");
        // Inside [1,4)x[1,3) -> red; outside -> untouched.
        for y in 0..4i32 {
            for x in 0..8i32 {
                let px = surf[(y * 8 + x) as usize];
                let inside = (1..4).contains(&x) && (1..3).contains(&y);
                if inside {
                    assert_eq!(px, 0xFFFF_0000, "pixel ({x},{y}) should be red");
                } else {
                    assert_eq!(px, 0, "pixel ({x},{y}) should be untouched");
                }
            }
        }
    }
}
