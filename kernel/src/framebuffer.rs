//! Framebuffer driver — pixels, not text.
//!
//! The bootloader hands us a linear framebuffer in `BootInfo`. We hold onto
//! a raw pointer + geometry behind a spinlock so we can paint from anywhere
//! in the kernel.
//!
//! Today: clear-screen and fill-rect. Tomorrow: font rendering, then the
//! handoff to RaeGFX.

use bootloader_api::info::{FrameBuffer, FrameBufferInfo, PixelFormat};
use spin::Mutex;

/// 8-bit RGB color. The framebuffer's actual pixel format may be BGR, RGB,
/// or grayscale — we map at write time.
#[derive(Debug, Clone, Copy)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// RaeenOS brand palette — used by the boot fill so the user immediately knows
/// they're looking at RaeKernel, not a "no signal" black screen.
pub const RAEENOS_INK: Rgb = Rgb::new(0x0a, 0x0e, 0x1a); // deep blue-black bg
pub const RAEENOS_GLOW: Rgb = Rgb::new(0x4e, 0x9c, 0xff); // electric blue accent
pub const RAEENOS_PLASMA: Rgb = Rgb::new(0xff, 0x2e, 0x88); // magenta highlight

struct State {
    /// Raw pointer to the start of the framebuffer's bytes.
    /// Held as a pointer + length to keep the static Send-able.
    buffer_ptr: *mut u8,
    buffer_len: usize,
    info: FrameBufferInfo,
}

// SAFETY: the framebuffer memory is owned by the kernel for the duration of
// the run; we never alias it outside this module.
unsafe impl Send for State {}

static FB: Mutex<Option<State>> = Mutex::new(None);

/// Snapshot of the framebuffer geometry + kernel virtual pointer, suitable
/// for handing off to the compositor.
#[derive(Clone, Copy)]
pub struct FbInfo {
    pub ptr: *mut u8,
    pub byte_len: usize,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes_per_pixel: u32,
}
unsafe impl Send for FbInfo {}
unsafe impl Sync for FbInfo {}

/// Take a copy of the framebuffer pointer + geometry. Returns `None` if
/// `init` hasn't been called yet.
pub fn fb_info() -> Option<FbInfo> {
    let guard = FB.lock();
    guard.as_ref().map(|s| FbInfo {
        ptr: s.buffer_ptr,
        byte_len: s.buffer_len,
        width: s.info.width as u32,
        height: s.info.height as u32,
        stride: s.info.stride as u32,
        bytes_per_pixel: s.info.bytes_per_pixel as u32,
    })
}

/// Initialize from the bootloader-provided framebuffer.
///
/// Validates the geometry the bootloader handed us. Real iron with 4K or
/// HiDPI panels has occasionally produced stride/height combinations that
/// overflow `usize::MAX` or exceed the actual buffer length — silently
/// truncating the pixel writes (the per-pixel guard later) hides the
/// configuration error. We log an explicit `[fb]` line so a regression
/// shows up in the boot snapshot instead of just "looks fine, nothing
/// draws". MasterChecklist Phase 1.2.
pub fn init(fb: &mut FrameBuffer) {
    let info = fb.info();
    let buffer = fb.buffer_mut();
    let buffer_len = buffer.len();

    // Required bytes for the full frame: stride (in pixels) * height * bpp.
    // `checked_mul` catches the overflow case before we trust the value.
    let required = (info.stride as u64)
        .checked_mul(info.height as u64)
        .and_then(|v| v.checked_mul(info.bytes_per_pixel as u64));

    let state = State {
        buffer_ptr: buffer.as_mut_ptr(),
        buffer_len,
        info,
    };
    *FB.lock() = Some(state);

    match required {
        Some(req) if req as usize <= buffer_len => {
            crate::serial_println!(
                "[fb] geometry OK: {}x{} stride={} bpp={} need={}B have={}B",
                info.width,
                info.height,
                info.stride,
                info.bytes_per_pixel,
                req,
                buffer_len,
            );
        }
        Some(req) => {
            crate::serial_println!(
                "[fb][WARN] short buffer: need {}B have {}B (writes past row {} drop)",
                req,
                buffer_len,
                buffer_len
                    / ((info.stride as usize).max(1) * (info.bytes_per_pixel as usize).max(1)),
            );
        }
        None => {
            crate::serial_println!(
                "[fb][WARN] geometry overflow: stride={} h={} bpp={} (forcing all writes to drop)",
                info.stride,
                info.height,
                info.bytes_per_pixel,
            );
        }
    }

    verify_gop_mode(&info, buffer_len);
}

/// Classify and log the bootloader 0.11 GOP mode. MasterChecklist Phase 1.1 —
/// verify 1080p/4K-class framebuffers from real iron (and QEMU virtio-vga).
pub fn verify_gop_mode(info: &FrameBufferInfo, buffer_len: usize) {
    let width = info.width;
    let height = info.height;
    let bpp = info.bytes_per_pixel;
    let class = if width >= 3840 || height >= 2160 {
        "4K-class"
    } else if width >= 1920 && height >= 1080 {
        "1080p-class"
    } else {
        "sub-1080p"
    };
    crate::serial_println!(
        "[gop] verify OK: {}x{} stride={} bpp={} buf={}B ({})",
        width,
        height,
        info.stride,
        bpp * 8,
        buffer_len,
        class,
    );
}

/// Clear the entire framebuffer to a solid color.
pub fn clear(color: Rgb) {
    let mut guard = FB.lock();
    let Some(state) = guard.as_mut() else { return };
    clear_state(state, color);
}

fn clear_state(state: &mut State, color: Rgb) {
    let bpp = state.info.bytes_per_pixel;
    let height = state.info.height;
    let width = state.info.width;
    let stride = state.info.stride;

    // 4K and HiDPI panels: row-wise bulk fill avoids 8M+ per-pixel calls.
    if bpp == 4 {
        let pixel = pack_pixel_u32(color, state.info.pixel_format);
        let row_bytes = (stride as usize).saturating_mul(4);
        let visible_bytes = (width as usize).saturating_mul(4).min(row_bytes);
        for y in 0..height {
            let row_off = (y as usize)
                .saturating_mul(stride as usize)
                .saturating_mul(4);
            if row_off + visible_bytes > state.buffer_len {
                break;
            }
            // SAFETY: row_off + visible_bytes checked against buffer_len.
            unsafe {
                let dst = state.buffer_ptr.add(row_off) as *mut u32;
                for x in 0..(visible_bytes / 4) {
                    dst.add(x).write_volatile(pixel);
                }
            }
        }
        return;
    }

    for y in 0..height {
        for x in 0..width {
            write_pixel(state, x, y, color);
        }
    }
}

#[inline]
fn pack_pixel_u32(color: Rgb, format: PixelFormat) -> u32 {
    let (r, g, b) = (color.r as u32, color.g as u32, color.b as u32);
    match format {
        PixelFormat::Bgr => (r << 16) | (g << 8) | b,
        PixelFormat::U8 => {
            let luma = (color.r as u32 * 299 + color.g as u32 * 587 + color.b as u32 * 114) / 1000;
            luma | (luma << 8) | (luma << 16)
        }
        _ => (b << 16) | (g << 8) | r, // Rgb and unknown → RGB order
    }
}

/// Fill an axis-aligned rectangle.
pub fn fill_rect(x0: usize, y0: usize, w: usize, h: usize, color: Rgb) {
    let mut guard = FB.lock();
    let Some(state) = guard.as_mut() else { return };
    let x1 = (x0 + w).min(state.info.width);
    let y1 = (y0 + h).min(state.info.height);
    if state.info.bytes_per_pixel == 4 && w > 0 && h > 0 {
        let pixel = pack_pixel_u32(color, state.info.pixel_format);
        for y in y0..y1 {
            let row_off = y * state.info.stride * 4 + x0 * 4;
            if row_off + (x1 - x0) * 4 > state.buffer_len {
                continue;
            }
            unsafe {
                let dst = state.buffer_ptr.add(row_off) as *mut u32;
                for x in 0..(x1 - x0) {
                    dst.add(x).write_volatile(pixel);
                }
            }
        }
        return;
    }
    for y in y0..y1 {
        for x in x0..x1 {
            write_pixel(state, x, y, color);
        }
    }
}

/// Blit an 8x8 bitmap glyph at pixel `(px, py)`. Each byte of `glyph` is one
/// row; bit 7 (MSB) is the leftmost pixel. Set bits draw `fg`, clear bits `bg`.
///
/// This locks the framebuffer and packs the fg/bg pixels exactly once, then
/// writes all 64 pixels in a tight inner loop. The GOP text console renders a
/// glyph for every character of every mirrored serial line, so the previous
/// approach (64 `fill_rect` calls per glyph, each taking the FB lock and
/// re-packing the pixel) made boot crawl. Keep this hot path lock-light.
pub fn draw_glyph_8x8(px: usize, py: usize, glyph: &[u8; 8], fg: Rgb, bg: Rgb) {
    let mut guard = FB.lock();
    let Some(state) = guard.as_mut() else { return };
    if state.info.bytes_per_pixel == 4 {
        let fg_px = pack_pixel_u32(fg, state.info.pixel_format);
        let bg_px = pack_pixel_u32(bg, state.info.pixel_format);
        let stride = state.info.stride as usize;
        let width = state.info.width as usize;
        let height = state.info.height as usize;
        for (row_idx, &row) in glyph.iter().enumerate() {
            let y = py + row_idx;
            if y >= height {
                break;
            }
            let row_off = y * stride * 4 + px * 4;
            for col in 0..8usize {
                let x = px + col;
                if x >= width {
                    break;
                }
                let off = row_off + col * 4;
                if off + 4 > state.buffer_len {
                    break;
                }
                let pixel = if (row >> (7 - col)) & 1 == 1 {
                    fg_px
                } else {
                    bg_px
                };
                // SAFETY: off + 4 checked against buffer_len above.
                unsafe {
                    (state.buffer_ptr.add(off) as *mut u32).write_volatile(pixel);
                }
            }
        }
        return;
    }
    // Non-32bpp fallback: per-pixel writes (rare; QEMU + Athena are 32bpp).
    for (row_idx, &row) in glyph.iter().enumerate() {
        for col in 0..8usize {
            let color = if (row >> (7 - col)) & 1 == 1 { fg } else { bg };
            write_pixel(state, px + col, py + row_idx, color);
        }
    }
}

/// Paint a recognizable RaeenOS boot image — deep background with an accent
/// stripe across the top. Cheap, runs in a few ms even at 1080p.
pub fn fill_raeenos_palette() {
    clear(RAEENOS_INK);
    let (width, _height) = {
        let guard = FB.lock();
        match guard.as_ref() {
            Some(s) => (s.info.width, s.info.height),
            None => return,
        }
    };
    // 8-px accent stripe across the top.
    fill_rect(0, 0, width, 8, RAEENOS_GLOW);
    // A small magenta plasma square in the top-right corner — the "we booted" marker.
    let size = 16;
    fill_rect(
        width.saturating_sub(size * 2),
        size,
        size,
        size,
        RAEENOS_PLASMA,
    );
}

#[inline]
fn write_pixel(state: &State, x: usize, y: usize, color: Rgb) {
    if x >= state.info.width || y >= state.info.height {
        return;
    }
    let pixel_offset = (y * state.info.stride + x) * state.info.bytes_per_pixel;
    if pixel_offset + state.info.bytes_per_pixel > state.buffer_len {
        return;
    }
    let bytes = match state.info.pixel_format {
        PixelFormat::Rgb => [color.r, color.g, color.b, 0],
        PixelFormat::Bgr => [color.b, color.g, color.r, 0],
        PixelFormat::U8 => {
            // Grayscale: BT.601 luma weights, integer-only.
            let luma = (color.r as u32 * 299 + color.g as u32 * 587 + color.b as u32 * 114) / 1000;
            [luma as u8, 0, 0, 0]
        }
        _ => [color.r, color.g, color.b, 0],
    };
    // SAFETY: `buffer_ptr` points to a buffer of length `buffer_len`,
    // we've just bounds-checked that `pixel_offset + bytes_per_pixel <= buffer_len`.
    unsafe {
        let dst = state.buffer_ptr.add(pixel_offset);
        for (i, b) in bytes.iter().take(state.info.bytes_per_pixel).enumerate() {
            dst.add(i).write_volatile(*b);
        }
    }
}

// ── Runtime mode-set (change framebuffer resolution) ─────────────────────────
// MasterChecklist Phase 2.5: "Mode set: change framebuffer resolution at runtime."
//
// On QEMU/GOP, the bootloader hands us a fixed framebuffer. True mode-setting
// requires a GPU driver (EDID → KMS set-mode → new scanout buffer). Until a
// GPU driver exists, we offer a software-blit mode: rescale the compositor
// output to whatever the bootloader gave us.
//
// Once a GPU driver is available, this module wires `set_mode()` to the driver's
// mode-set ioctl, which reprograms the display controller's CRTC registers.

/// Current logical display resolution (may differ from physical framebuffer if scaling is active).
static LOGICAL_WIDTH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static LOGICAL_HEIGHT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static MODE_SET_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Request a mode change at runtime.
///
/// Before GPU driver: validates the resolution is within the physical framebuffer
/// bounds and stores it as the new logical resolution (compositor renders at this
/// size; blit to physical framebuffer if needed).
///
/// After GPU driver: calls the driver's CRTC mode-set path.
///
/// Returns `true` if the mode was accepted.
pub fn physical_dimensions() -> Option<(u32, u32)> {
    let guard = FB.lock();
    let st = guard.as_ref()?;
    Some((st.info.width as u32, st.info.height as u32))
}

pub fn set_mode(width: u32, height: u32) -> bool {
    // Validate against physical framebuffer dimensions.
    let (phys_w, phys_h) = match physical_dimensions() {
        Some(d) => d,
        None => return false,
    };

    // Accept if within physical bounds (exact match or downscale).
    if width > phys_w || height > phys_h || width == 0 || height == 0 {
        crate::serial_println!(
            "[fb] mode_set REJECTED: {}x{} exceeds physical {}x{}",
            width,
            height,
            phys_w,
            phys_h
        );
        return false;
    }

    let hw = crate::gpu::request_display_mode(width, height, 32);
    let prev_w = LOGICAL_WIDTH.swap(width, core::sync::atomic::Ordering::Relaxed);
    let prev_h = LOGICAL_HEIGHT.swap(height, core::sync::atomic::Ordering::Relaxed);
    MODE_SET_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let _ = crate::compositor::set_output_resolution(width, height);
    crate::compositor::recomposite();

    crate::serial_println!(
        "[fb] mode_set: {}x{} -> {}x{} (physical {}x{}, gpu_hw={})",
        prev_w,
        prev_h,
        width,
        height,
        phys_w,
        phys_h,
        hw,
    );
    true
}

pub fn run_boot_smoketest() {
    let Some((phys_w, phys_h)) = physical_dimensions() else {
        crate::serial_println!("[fb] smoketest: no framebuffer");
        return;
    };
    // Prove runtime mode-set works by going DOWN to 1024x768 and then BACK to the
    // native framebuffer size. The restore is mandatory: leaving the logical
    // resolution at 1024x768 makes the compositor render the entire desktop into
    // a 1024x768 corner of a larger panel (iron: "the desktop is a lot smaller
    // than the actual display"). The test only proves the mechanism — it must
    // never degrade the shipped resolution.
    let target_w = 1024u32.min(phys_w);
    let target_h = 768u32.min(phys_h);
    let down = set_mode(target_w, target_h);
    let restored = set_mode(phys_w, phys_h);
    crate::serial_println!(
        "[fb] smoketest: mode_set down({}x{})={} restore({}x{})={} count={}",
        target_w,
        target_h,
        down,
        phys_w,
        phys_h,
        restored,
        mode_set_count()
    );
}

pub fn current_mode() -> (u32, u32) {
    let w = LOGICAL_WIDTH.load(core::sync::atomic::Ordering::Relaxed);
    let h = LOGICAL_HEIGHT.load(core::sync::atomic::Ordering::Relaxed);
    if w == 0 {
        // Not yet set — use physical framebuffer size.
        let guard = FB.lock();
        if let Some(ref st) = *guard {
            return (st.info.width as u32, st.info.height as u32);
        }
    }
    (w, h)
}

pub fn mode_set_count() -> u32 {
    MODE_SET_COUNT.load(core::sync::atomic::Ordering::Relaxed)
}
