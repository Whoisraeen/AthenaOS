//! Crisp grayscale-AA text rendering — the one wired text path.
//!
//! *"Built for people who care about how things feel."* — `LEGACY_GAMING_CONCEPT.md`
//! §AthUI ("a cohesive and visually stunning UI"). The 8×8 bitmap block font was
//! athena-visual-qa's single dominant "basic / not crisp" signal; this module is
//! the fix (`docs/design/typography-rendering.md`). It owns the two embedded
//! `athfont` faces (RaeSans = Inter, RaeMono = JetBrains Mono), a per-(family,
//! glyph,ppem) coverage cache, and the source-over compositing the §6 type ramp
//! feeds into via `Canvas::draw_text_aa`.
//!
//! Anti-aliasing is **grayscale, not sub-pixel** (spec decision: software
//! compositor + translucent glass backdrops → sub-pixel produces colour fringes).
//! The glyph coverage is treated as source alpha and composited SOURCE-OVER
//! against the real destination pixel via `Canvas::blend_pixel` — NOT against
//! black (the `athui::text::blit_glyph` bug this path deliberately avoids).

extern crate alloc;

use crate::Canvas;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use athfont::{FontHandle, RasterizedGlyph};

/// The two shipped system families (`docs/design/design-language.md` §6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontFamily {
    /// RaeSans — Inter (humanist UI face). Default for all chrome/body text.
    Sans,
    /// RaeMono — JetBrains Mono (terminal / code). Even monospace advance.
    Mono,
}

/// One cached, rasterized glyph keyed by family + codepoint + ppem.
struct CacheEntry {
    family: FontFamily,
    codepoint: u32,
    ppem: u16,
    /// `None` = a real glyph with no outline (space) — cache the miss so we do
    /// not re-rasterize blanks every frame.
    glyph: Option<RasterizedGlyph>,
}

/// The text engine: two parsed faces + a bounded LRU-ish coverage cache. Built
/// once on first use (`ensure_init`) from the `include_bytes!`-embedded faces —
/// no filesystem dependency, so it works at first boot before AthFS mounts.
pub struct TextEngine {
    sans: Option<FontHandle>,
    mono: Option<FontHandle>,
    cache: Vec<CacheEntry>,
    cache_cap: usize,
}

static ENGINE_INIT: AtomicBool = AtomicBool::new(false);
/// Set (release) AFTER `ENGINE` is fully written, so a concurrent `ensure_init`
/// loser can acquire-wait for a published engine instead of racing the transient
/// `ENGINE_INIT=true, ENGINE=None` window the gate opens.
static ENGINE_PUBLISHED: AtomicBool = AtomicBool::new(false);
static mut ENGINE: Option<TextEngine> = None;

impl TextEngine {
    fn new() -> Self {
        // The variable Inter `glyf` carries the default-instance (Regular)
        // outlines, which athfont rasterizes directly. JetBrains Mono is static.
        let sans = FontHandle::from_bytes(athfont::builtin::rae_sans().to_vec());
        let mono = FontHandle::from_bytes(athfont::builtin::rae_mono().to_vec());
        TextEngine {
            sans,
            mono,
            cache: Vec::new(),
            cache_cap: 1024,
        }
    }

    fn face(&self, family: FontFamily) -> Option<&FontHandle> {
        match family {
            FontFamily::Sans => self.sans.as_ref(),
            FontFamily::Mono => self.mono.as_ref(),
        }
    }

    /// Index of the cache slot for this glyph, rasterizing + inserting on miss.
    fn slot(&mut self, family: FontFamily, codepoint: u32, ppem: u16) -> usize {
        if let Some(pos) = self
            .cache
            .iter()
            .position(|e| e.family == family && e.codepoint == codepoint && e.ppem == ppem)
        {
            return pos;
        }
        let glyph = self
            .face(family)
            .and_then(|f| f.rasterize_codepoint_or_fallback(codepoint, ppem));
        if self.cache.len() >= self.cache_cap {
            // Cheap eviction: drop the oldest quarter so steady-state churn is
            // amortized (UI ASCII set is < cache_cap, so this rarely fires).
            self.cache.drain(0..self.cache_cap / 4);
        }
        self.cache.push(CacheEntry {
            family,
            codepoint,
            ppem,
            glyph,
        });
        self.cache.len() - 1
    }

    /// True once at least one face parsed (the AA path is usable).
    pub fn ready(&self) -> bool {
        self.sans.is_some() || self.mono.is_some()
    }

    /// Number of families that parsed (smoketest telemetry).
    pub fn families_loaded(&self) -> usize {
        self.sans.is_some() as usize + self.mono.is_some() as usize
    }
}

/// Build the engine on first use. Idempotent; safe to call from the boot path
/// before `activate_desktop` (per `iron-console-logging-tax`, off the serial
/// hot loop). Returns whether the engine is ready.
pub fn ensure_init() -> bool {
    if ENGINE_INIT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        // Winner of the init gate: build, then publish-with-release so a
        // concurrent reader that observes a *built* engine sees fully-written
        // fields (the `ENGINE_PUBLISHED` acquire-load below pairs with this).
        let eng = TextEngine::new();
        unsafe {
            ENGINE = Some(eng);
        }
        ENGINE_PUBLISHED.store(true, Ordering::Release);
    } else {
        // Loser of the gate: the winner has set ENGINE_INIT but may still be
        // inside TextEngine::new(); spin-wait (bounded) until it publishes the
        // engine so we never observe the transient `INIT=true, ENGINE=None`
        // window. This is the same boot path the compositor + a service thread
        // can both hit, and (on host) the default parallel test runner exposes.
        for _ in 0..1_000_000 {
            if ENGINE_PUBLISHED.load(Ordering::Acquire) {
                break;
            }
            core::hint::spin_loop();
        }
    }
    engine().map_or(false, |e| e.ready())
}

fn engine() -> Option<&'static mut TextEngine> {
    if ENGINE_INIT.load(Ordering::SeqCst) {
        // SAFETY: single-threaded compositor/boot access; the engine is built
        // once (ENGINE_INIT gate) and afterwards only mutated through its
        // bounded cache. Matches the `athfont::FontEngine` global discipline.
        #[allow(static_mut_refs)]
        unsafe {
            ENGINE.as_mut()
        }
    } else {
        None
    }
}

/// Whether the crisp AA path is available (engine built + a face parsed).
pub fn is_ready() -> bool {
    engine().map_or(false, |e| e.ready())
}

/// Families currently loaded (0, 1, or 2). For the boot smoketest.
pub fn families_loaded() -> usize {
    engine().map_or(0, |e| e.families_loaded())
}

/// Result of a single `draw_text_aa` call (for the smoketest proof).
pub struct DrawStats {
    pub advance: i32,
    pub total_coverage: u64,
    pub min_cov: u8,
    pub max_cov: u8,
}

impl Canvas {
    /// Draw `s` with the system font at the given §6 type style, fg colour, and
    /// pen origin. `(x, y)` is the **top-left** of the line box; the baseline is
    /// derived from `style.px` internally. Returns the x advance after the last
    /// glyph (kerned).
    ///
    /// Grayscale AA: each glyph's coverage is composited SOURCE-OVER against the
    /// real destination pixel (`blend_pixel`), so text reads correctly over the
    /// dark desktop, light surfaces, AND translucent glass — no dark halo (the
    /// `athui::text::blit_glyph`-against-black bug). Falls back to the 8×8 bitmap
    /// path when the font engine is not yet ready (early boot / unmapped glyph).
    pub fn draw_text_aa(
        &mut self,
        x: i32,
        y: i32,
        s: &str,
        style: ath_tokens::TypeStyle,
        fg: u32,
        family: FontFamily,
    ) -> i32 {
        let stats = self.draw_text_aa_stats(x, y, s, style, fg, family);
        stats.advance
    }

    /// Like [`draw_text_aa`] but returns coverage stats — used by the boot
    /// smoketest to prove real, non-uniform glyph ink (not blank/tofu).
    pub fn draw_text_aa_stats(
        &mut self,
        x: i32,
        y: i32,
        s: &str,
        style: ath_tokens::TypeStyle,
        fg: u32,
        family: FontFamily,
    ) -> DrawStats {
        let ppem = style.px.clamp(6, 256) as u16;
        let fg_a = ((fg >> 24) & 0xFF) as u32;
        let fg_a = if fg_a == 0 { 255 } else { fg_a }; // 0x00RRGGBB => opaque
        let fg_rgb = fg & 0x00FF_FFFF;

        // Baseline = top + ascent. Use the face ascender if available; fall back
        // to ~0.8·ppem (a sane default that keeps text inside the line box).
        let eng = match engine() {
            Some(e) if e.ready() => e,
            _ => {
                // 8×8 fallback (early boot / no font). Scale the bitmap to ~ppem.
                let scale = (ppem as usize / 8).max(1);
                let adv = self.draw_text_scaled(x.max(0) as usize, y.max(0) as usize, s, fg, scale)
                    as i32
                    - x;
                return DrawStats {
                    advance: adv.max(0),
                    total_coverage: 0,
                    min_cov: 0,
                    max_cov: 0,
                };
            }
        };

        let baseline = {
            let asc = eng
                .face(family)
                .map(|f| {
                    let m = f.metrics();
                    (m.ascender as f32 * f.px_scale(ppem)) as i32
                })
                .filter(|&a| a > 0)
                .unwrap_or((ppem as i32 * 8) / 10);
            y + asc
        };

        let mut pen_x = x;
        let mut prev_gid: Option<u16> = None;
        let mut total_coverage: u64 = 0;
        let mut min_cov: u8 = 255;
        let mut max_cov: u8 = 0;

        for ch in s.chars() {
            let cp = ch as u32;
            // Kerning against the previous glyph.
            if let (Some(face), Some(pg)) = (eng.face(family), prev_gid) {
                if let Some(gid) = face.cmap.lookup(cp) {
                    let k = face.kerning(pg, gid);
                    if k != 0 {
                        pen_x += (k as f32 * face.px_scale(ppem)) as i32;
                    }
                }
            }

            let slot = eng.slot(family, cp, ppem);
            // Resolve the cur-glyph id once for kerning + advance fallback.
            let cur_gid = eng.face(family).and_then(|f| f.cmap.lookup(cp));
            // Metric-width fallback advance for a glyph with no outline (space).
            let metric_adv = cur_gid
                .and_then(|gid| eng.face(family).map(|f| f.advance_px(gid, ppem)))
                .filter(|&a| a > 0)
                .unwrap_or((ppem as i32) / 3);

            // Borrow the cached entry immutably ONLY for the blit; scope it so it
            // is dropped before we mutate pen/prev again.
            let advance = {
                let entry = &eng.cache[slot];
                match &entry.glyph {
                    Some(g) => {
                        let gx0 = pen_x + g.bearing_x;
                        let gy0 = baseline - g.bearing_y;
                        let gw = g.width as i32;
                        let gh = g.height as i32;
                        for ry in 0..gh {
                            let py = gy0 + ry;
                            if py < 0 {
                                continue;
                            }
                            for rx in 0..gw {
                                let px = gx0 + rx;
                                if px < 0 {
                                    continue;
                                }
                                let cov = g.pixels[(ry as u32 * g.width + rx as u32) as usize];
                                if cov == 0 {
                                    continue;
                                }
                                total_coverage += cov as u64;
                                if cov < min_cov {
                                    min_cov = cov;
                                }
                                if cov > max_cov {
                                    max_cov = cov;
                                }
                                // alpha = coverage * fg_alpha (grayscale; no gamma
                                // in the hot loop — the spec's cheaper
                                // approximation; blend_pixel does source-over
                                // against the real dst, NOT against black).
                                let a = (cov as u32 * fg_a) / 255;
                                if a == 0 {
                                    continue;
                                }
                                self.blend_pixel(px as usize, py as usize, (a << 24) | fg_rgb);
                            }
                        }
                        g.advance.max(1)
                    }
                    // No outline (space / unmapped) → advance by the metric.
                    None => metric_adv,
                }
            };
            prev_gid = cur_gid;
            pen_x += advance;
        }

        DrawStats {
            advance: pen_x,
            total_coverage,
            min_cov: if max_cov == 0 { 0 } else { min_cov },
            max_cov,
        }
    }

    /// Measure the pixel advance of `s` at `style` without drawing — for caret /
    /// hit-test positioning and centering. Mirrors `draw_text_aa`'s advance.
    pub fn measure_text_aa(
        &self,
        s: &str,
        style: ath_tokens::TypeStyle,
        family: FontFamily,
    ) -> i32 {
        let ppem = style.px.clamp(6, 256) as u16;
        let eng = match engine() {
            Some(e) if e.ready() => e,
            _ => return (s.chars().count() as i32) * ((ppem as i32 / 8).max(1)) * 8,
        };
        let face = match eng.face(family) {
            Some(f) => f,
            None => return (s.chars().count() as i32) * (ppem as i32) / 2,
        };
        let mut pen = 0i32;
        let mut prev: Option<u16> = None;
        for ch in s.chars() {
            let cp = ch as u32;
            if let Some(gid) = face.cmap.lookup(cp) {
                if let Some(pg) = prev {
                    let k = face.kerning(pg, gid);
                    if k != 0 {
                        pen += (k as f32 * face.px_scale(ppem)) as i32;
                    }
                }
                let adv = face.advance_px(gid, ppem);
                pen += if adv > 0 { adv } else { (ppem as i32) / 3 };
                prev = Some(gid);
            } else {
                pen += (ppem as i32) / 3;
                prev = None;
            }
        }
        pen
    }
}

/// Outcome of the AA-text boot smoketest — the kernel formats the serial line
/// (athgfx is `no_std` with no serial of its own).
pub struct SmoketestResult {
    pub families: usize,
    pub total_coverage: u64,
    pub min_cov: u8,
    pub max_cov: u8,
    pub pass: bool,
}

/// Boot smoketest: prove the AA path renders real, non-uniform glyph ink for a
/// known string (catches blank/tofu/missing-font regressions). FAIL-able —
/// zero coverage or a single flat coverage value reds `pass`.
///
/// Renders to a small offscreen ARGB buffer so the test is side-effect-free
/// (no framebuffer touch) yet exercises the exact compositing path the OOBE
/// uses. The kernel prints:
///   `[gfx] draw_text_aa smoketest: face=Inter glyph_coverage=true -> PASS`
pub fn run_boot_smoketest() -> SmoketestResult {
    ensure_init();
    let ready = is_ready();
    let families = families_loaded();

    const W: usize = 96;
    const H: usize = 32;
    let mut buf = alloc::vec![0u32; W * H];
    let stats = {
        let ptr = buf.as_mut_ptr() as *mut u8;
        let mut canvas = unsafe { Canvas::new(ptr, W, H, 4) };
        canvas.draw_text_aa_stats(
            2,
            4,
            "Rae",
            ath_tokens::TYPE_TITLE,
            0xFF_FFFFFF,
            FontFamily::Sans,
        )
    };

    // Real glyphs => non-zero coverage AND a range of values (grayscale AA, not
    // a flat block). tofu/blank/missing-font => total_coverage == 0.
    let non_uniform = stats.max_cov > stats.min_cov && stats.total_coverage > 0;
    let pass = ready && families >= 1 && stats.total_coverage > 0 && non_uniform;

    SmoketestResult {
        families,
        total_coverage: stats.total_coverage,
        min_cov: stats.min_cov,
        max_cov: stats.max_cov,
        pass,
    }
}
