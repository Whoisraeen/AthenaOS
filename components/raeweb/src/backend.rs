//! RaeWeb paint backend — the engine→pixels bridge.
//!
//! > "Native everywhere. No Electron tax. No web wrappers. Native rendering,
//! > native input, native audio — sub-frame latency end to end."
//! > — RaeenOS Concept, §Design Principles #1
//!
//! This is the single highest-leverage piece of Phase 1 (docs/research/web-engine.md
//! §"Paint bridge"): it converts the engine's private [`DisplayList`]/[`PaintCommand`]
//! IR into [`raegfx::Canvas`] calls, turning the inert 4.9k-line renderer into pixels
//! that draw through the SAME crisp-AA path (Inter / `draw_text_aa`) as every other
//! RaeUI surface — the literal meaning of "renders through RaeUI".
//!
//! `no_std + alloc`, never panics on malformed input: every coordinate is clamped to
//! `usize` and to the active clip, and an empty/degenerate command is skipped, so a
//! hostile document can produce garbage geometry but never a crash.

use crate::{CssColor, DisplayList, PaintCommand, Rect};
use rae_tokens::TypeStyle;
use raegfx::text::FontFamily;
use raegfx::Canvas;

/// What the bridge actually emitted — proof material for the R10 smoketest.
///
/// The smoketest asserts `text_draws >= 1` and `total_commands >= 1`; a bridge that
/// silently issues zero text calls is a false green, so this struct is the FAIL hook.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Total [`PaintCommand`]s consumed from the display list.
    pub total_commands: usize,
    /// `draw_text_aa` calls issued (the "did any text actually render?" signal).
    pub text_draws: usize,
    /// `fill_rect` / `fill_rect`-class solid fills issued.
    pub rect_fills: usize,
    /// `fill_rounded_rect` calls issued.
    pub rounded_fills: usize,
    /// `fill_rounded_rect_shadow` calls issued.
    pub shadow_draws: usize,
    /// Border edge fills issued.
    pub border_draws: usize,
    /// Commands skipped because they fell entirely outside the canvas/clip or were
    /// degenerate (zero size) — counted so the smoketest can distinguish
    /// "nothing to draw" from "drew nothing".
    pub skipped: usize,
}

/// An axis-aligned clip in integer device space (an intersection of every active
/// `SetClip`). All draws are clamped to it before hitting the canvas.
#[derive(Clone, Copy)]
struct ClipRect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl ClipRect {
    fn full(w: usize, h: usize) -> Self {
        Self {
            x0: 0,
            y0: 0,
            x1: w as i64,
            y1: h as i64,
        }
    }

    /// Intersect with a viewport-space [`Rect`] (after scroll has been applied by
    /// the caller). Empty/inverted results are kept (they simply clip everything).
    fn intersect(self, r: &Rect) -> Self {
        let rx0 = r.x as i64;
        let ry0 = r.y as i64;
        let rx1 = (r.x + r.width) as i64;
        let ry1 = (r.y + r.height) as i64;
        Self {
            x0: self.x0.max(rx0),
            y0: self.y0.max(ry0),
            x1: self.x1.min(rx1),
            y1: self.y1.min(ry1),
        }
    }

    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    /// Does `(px, py)` (device space) fall inside the clip?
    fn contains(&self, px: i64, py: i64) -> bool {
        px >= self.x0 && px < self.x1 && py >= self.y0 && py < self.y1
    }
}

/// Pack a [`CssColor`] (+ an inherited opacity multiplier) into the canvas's
/// `0xAARRGGBB` word. Alpha is the product of the color's own alpha and the
/// `PushOpacity` stack, clamped to `[0,255]`.
fn argb(color: CssColor, opacity: f32) -> u32 {
    let a = (color.a * opacity).clamp(0.0, 1.0);
    let a8 = (a * 255.0 + 0.5) as u32;
    (a8 << 24) | ((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32)
}

/// Map an engine font-family string + weight to the two shipped system faces.
/// Anything monospace-flavored → `Mono`; everything else → `Sans` (Inter), so the
/// web text composites through the exact crisp-AA path the rest of the OS uses.
fn resolve_family(family: &str) -> FontFamily {
    let f = family.trim().to_ascii_lowercase();
    if f.contains("mono") || f.contains("courier") || f.contains("consol") {
        FontFamily::Mono
    } else {
        FontFamily::Sans
    }
}

/// Map an engine `font-weight` string to a numeric weight for [`TypeStyle`].
fn resolve_weight(weight: &str) -> u32 {
    match weight.trim() {
        "bold" | "bolder" => 600,
        "lighter" | "100" | "200" | "300" => 400,
        "medium" | "500" => 500,
        "semibold" | "600" => 600,
        "700" | "800" | "900" => 600,
        _ => 400,
    }
}

/// Clamp a viewport-space rect to the canvas and the active clip, returning the
/// integer `(x, y, w, h)` to draw, or `None` if nothing is visible.
fn clamp_rect(rect: &Rect, clip: &ClipRect) -> Option<(usize, usize, usize, usize)> {
    if clip.is_empty() {
        return None;
    }
    let x0 = (rect.x as i64).max(clip.x0);
    let y0 = (rect.y as i64).max(clip.y0);
    let x1 = ((rect.x + rect.width) as i64).min(clip.x1);
    let y1 = ((rect.y + rect.height) as i64).min(clip.y1);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((
        x0 as usize,
        y0 as usize,
        (x1 - x0) as usize,
        (y1 - y0) as usize,
    ))
}

/// Paint a [`DisplayList`] into a [`Canvas`], returning [`PaintStats`] for proof.
///
/// The display list is in **viewport space**; `scroll_x/scroll_y` are subtracted so
/// the page scrolls under a fixed canvas. Opacity and clip are tracked with small
/// stacks so nested `PushOpacity`/`SetClip` nest correctly; an unbalanced `Pop`/
/// `ClearClip` (malformed engine output) is tolerated rather than panicking.
pub fn paint_displaylist_to_canvas(
    list: &DisplayList,
    canvas: &mut Canvas,
    scroll_x: f32,
    scroll_y: f32,
) -> PaintStats {
    let mut stats = PaintStats::default();
    let cw = canvas.width();
    let ch = canvas.height();

    // Stacks. The base clip is the whole canvas; base opacity is 1.0.
    let root_clip = ClipRect::full(cw, ch);
    let mut clip_stack: alloc::vec::Vec<ClipRect> = alloc::vec::Vec::new();
    clip_stack.push(root_clip);
    let mut opacity_stack: alloc::vec::Vec<f32> = alloc::vec::Vec::new();
    opacity_stack.push(1.0);

    // Apply scroll once per rect by translating into device space.
    let shift = |r: &Rect| -> Rect {
        Rect {
            x: r.x - scroll_x,
            y: r.y - scroll_y,
            width: r.width,
            height: r.height,
        }
    };

    for cmd in &list.commands {
        stats.total_commands += 1;
        let clip = *clip_stack.last().unwrap_or(&root_clip);
        let opacity = *opacity_stack.last().unwrap_or(&1.0);

        match cmd {
            PaintCommand::FillRect { rect, color } => {
                let r = shift(rect);
                match clamp_rect(&r, &clip) {
                    Some((x, y, w, h)) => {
                        canvas.fill_rect(x, y, w, h, argb(*color, opacity));
                        stats.rect_fills += 1;
                    }
                    None => stats.skipped += 1,
                }
            }
            PaintCommand::FillGradient { rect, top, bottom } => {
                let r = shift(rect);
                match clamp_rect(&r, &clip) {
                    Some((x, y, w, h)) => {
                        canvas.fill_rect_gradient(
                            x,
                            y,
                            w,
                            h,
                            argb(*top, opacity),
                            argb(*bottom, opacity),
                        );
                        stats.rect_fills += 1;
                    }
                    None => stats.skipped += 1,
                }
            }

            PaintCommand::FillRoundedRect {
                rect,
                color,
                radius,
            } => {
                let r = shift(rect);
                // Round-rect coverage handles its own AA; clamp the bounds to the
                // canvas (the rasterizer already bounds-checks each pixel) but skip
                // if the box lies fully outside the clip.
                if clip.intersect(&r).is_empty() || r.width <= 0.0 || r.height <= 0.0 {
                    stats.skipped += 1;
                } else {
                    canvas.fill_rounded_rect(
                        r.x.max(0.0) as usize,
                        r.y.max(0.0) as usize,
                        r.width as usize,
                        r.height as usize,
                        (*radius).max(0.0) as usize,
                        argb(*color, opacity),
                    );
                    stats.rounded_fills += 1;
                }
            }

            PaintCommand::DrawShadow {
                rect,
                color,
                offset_x: _,
                offset_y,
                blur,
                spread: _,
            } => {
                let r = shift(rect);
                if r.width <= 0.0 || r.height <= 0.0 || *blur <= 0.0 {
                    stats.skipped += 1;
                } else {
                    canvas.fill_rounded_rect_shadow(
                        r.x.max(0.0) as usize,
                        r.y.max(0.0) as usize,
                        r.width as usize,
                        r.height as usize,
                        0,
                        argb(*color, opacity) & 0x00FF_FFFF,
                        (*blur).max(1.0) as usize,
                        *offset_y as i32,
                    );
                    stats.shadow_draws += 1;
                }
            }

            PaintCommand::FillText {
                text,
                x,
                y,
                font_size,
                color,
                font_family,
                font_weight,
                underline,
                strikethrough,
            } => {
                if text.trim().is_empty() {
                    stats.skipped += 1;
                    continue;
                }
                let dx = (*x - scroll_x) as i64;
                let dy = (*y - scroll_y) as i64;
                // Cheap top-left clip test: if the pen origin is outside the clip,
                // skip. (Per-glyph clipping is a Phase-3 scissor concern; the
                // canvas itself bounds-checks every pixel so this can never write
                // out of bounds — it only avoids painting fully-clipped text.)
                if !clip.contains(dx, dy) && !clip.contains(dx, dy + *font_size as i64) {
                    stats.skipped += 1;
                    continue;
                }
                let style = TypeStyle {
                    px: (*font_size).max(1.0) as u32,
                    weight: resolve_weight(font_weight),
                    line_height: ((*font_size).max(1.0) * 1.25) as u32,
                };
                canvas.draw_text_aa(
                    dx.max(0) as i32,
                    dy.max(0) as i32,
                    text,
                    style,
                    argb(*color, opacity),
                    resolve_family(font_family),
                );
                stats.text_draws += 1;
                // text-decoration: underline -> a thin rule just below the baseline,
                // spanning the text run (width matches the layout char advance).
                if *underline {
                    let uw = (text.chars().count() as f32 * *font_size * 0.6) as usize;
                    if uw > 0 {
                        let ux = dx.max(0) as usize;
                        let uy = (dy + (*font_size * 0.92) as i64).max(0) as usize;
                        let uh = ((*font_size / 14.0) as usize).max(1);
                        canvas.fill_rect(ux, uy, uw, uh, argb(*color, opacity));
                        stats.rect_fills += 1;
                    }
                }
                // text-decoration: line-through -> a rule through the text middle.
                if *strikethrough {
                    let sw = (text.chars().count() as f32 * *font_size * 0.6) as usize;
                    if sw > 0 {
                        let sx = dx.max(0) as usize;
                        let sy = (dy + (*font_size * 0.5) as i64).max(0) as usize;
                        let sh = ((*font_size / 14.0) as usize).max(1);
                        canvas.fill_rect(sx, sy, sw, sh, argb(*color, opacity));
                        stats.rect_fills += 1;
                    }
                }
            }

            PaintCommand::DrawBorder {
                rect,
                color,
                widths,
                radius: _,
            } => {
                let r = shift(rect);
                let c = argb(*color, opacity);
                let mut drew = false;
                // Four edge fills (rounded-corner stroke is Phase 3). Each edge is
                // clamped to the clip independently.
                if widths.top > 0.0 {
                    if let Some((x, y, w, h)) = clamp_rect(
                        &Rect {
                            x: r.x,
                            y: r.y,
                            width: r.width,
                            height: widths.top,
                        },
                        &clip,
                    ) {
                        canvas.fill_rect(x, y, w, h, c);
                        drew = true;
                    }
                }
                if widths.bottom > 0.0 {
                    if let Some((x, y, w, h)) = clamp_rect(
                        &Rect {
                            x: r.x,
                            y: r.y + r.height - widths.bottom,
                            width: r.width,
                            height: widths.bottom,
                        },
                        &clip,
                    ) {
                        canvas.fill_rect(x, y, w, h, c);
                        drew = true;
                    }
                }
                if widths.left > 0.0 {
                    if let Some((x, y, w, h)) = clamp_rect(
                        &Rect {
                            x: r.x,
                            y: r.y,
                            width: widths.left,
                            height: r.height,
                        },
                        &clip,
                    ) {
                        canvas.fill_rect(x, y, w, h, c);
                        drew = true;
                    }
                }
                if widths.right > 0.0 {
                    if let Some((x, y, w, h)) = clamp_rect(
                        &Rect {
                            x: r.x + r.width - widths.right,
                            y: r.y,
                            width: widths.right,
                            height: r.height,
                        },
                        &clip,
                    ) {
                        canvas.fill_rect(x, y, w, h, c);
                        drew = true;
                    }
                }
                if drew {
                    stats.border_draws += 1;
                } else {
                    stats.skipped += 1;
                }
            }

            PaintCommand::StrokeRect { rect, color, width } => {
                let r = shift(rect);
                let c = argb(*color, opacity);
                let w = (*width).max(1.0);
                // Four clamped edge fills (the rectangle outline).
                let mut drew = false;
                for edge in [
                    Rect {
                        x: r.x,
                        y: r.y,
                        width: r.width,
                        height: w,
                    },
                    Rect {
                        x: r.x,
                        y: r.y + r.height - w,
                        width: r.width,
                        height: w,
                    },
                    Rect {
                        x: r.x,
                        y: r.y,
                        width: w,
                        height: r.height,
                    },
                    Rect {
                        x: r.x + r.width - w,
                        y: r.y,
                        width: w,
                        height: r.height,
                    },
                ] {
                    if let Some((x, y, ww, hh)) = clamp_rect(&edge, &clip) {
                        canvas.fill_rect(x, y, ww, hh, c);
                        drew = true;
                    }
                }
                if drew {
                    stats.border_draws += 1;
                } else {
                    stats.skipped += 1;
                }
            }

            PaintCommand::DrawImage { .. } => {
                // Image decode/blit is Phase 3 (needs a no_std PNG/JPEG decoder).
                // Counted as skipped so the smoketest sees it was deliberately not
                // painted rather than silently lost.
                stats.skipped += 1;
            }

            PaintCommand::SetClip { rect } => {
                let r = shift(rect);
                let new = clip.intersect(&r);
                clip_stack.push(new);
            }

            PaintCommand::ClearClip => {
                // Never pop the root clip — tolerate an unbalanced ClearClip.
                if clip_stack.len() > 1 {
                    clip_stack.pop();
                }
            }

            PaintCommand::PushOpacity(o) => {
                let combined = (opacity * o.clamp(0.0, 1.0)).clamp(0.0, 1.0);
                opacity_stack.push(combined);
            }

            PaintCommand::PopOpacity => {
                if opacity_stack.len() > 1 {
                    opacity_stack.pop();
                }
            }
        }
    }

    stats
}
