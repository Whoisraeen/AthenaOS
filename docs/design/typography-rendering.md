# Design Spec: typography-rendering (crisp anti-aliased UI text)

> Cross-references `docs/design/design-language.md` §6 (type ramp). Token names
> here (`type.body`, `text.primary`, `TYPE_*`, `TypeStyle`) are that file's,
> verbatim. This spec decides **how glyphs become crisp pixels**; §6 decides
> **which glyph at which size**.

## Concept promise + bar to clear

> "Built for people who care about how things feel." — `LEGACY_GAMING_CONCEPT.md`
>
> "A cohesive and visually stunning UI." — §AthUI

**The bar:** the *first thing the eye reads as "premium" is the text.* macOS (SF
Pro + Core Text grayscale AA + light hinting) and Windows 11 (Segoe UI Variable +
DirectWrite, ClearType/grayscale) both ship sub-pixel-clean, kerned, hinted UI
text at every size. AthenaOS today ships an **8×8 bitmap block font**
(`font8x8::BASIC_FONTS`) — the same 64-pixel face nearest-neighbour/bilinear-
upscaled for "large" text. raeen-visual-qa judged this the **single dominant
"basic / not crisp" signal** in the first OS screenshot. Until this is fixed, no
amount of glass, shadow, or accent work makes the desktop rival macOS/Win11,
because the type undercuts all of it. **This is THE highest-leverage UI fix.**

---

## Prior art distilled

- **macOS (Core Text / CoreGraphics):** real vector outlines, **grayscale AA**
  (Apple dropped sub-pixel/LCD AA in 10.14 — Retina density made it
  unnecessary), light autohinting, generous tracking at large sizes, optical
  sizing on SF. Glyphs alpha-composite over live content. *Take:* grayscale AA
  + gamma-correct blend is the modern, display-agnostic choice. *Avoid:*
  assuming Retina DPI — our software raster is often 1× on a 1080p/1440p panel,
  where AA quality matters *more*, not less.
- **Windows 11 (DirectWrite):** Segoe UI **Variable** (optical axis), ClearType
  sub-pixel by default but grayscale in many WinUI surfaces; strong TrueType
  hinting (vertical stem snapping to the pixel grid) keeps small text legible.
  *Take:* hinting/grid-fitting at body sizes (11–14px) is where bitmap fonts
  "win" on crispness and naive vector AA "loses" (fuzzy stems). *Avoid:*
  sub-pixel AA on a software compositor — it's RGB-stripe-order- and
  rotation-dependent and fights our translucent glass backdrops (colour fringes
  over a blurred gradient). **Grayscale only.**
- **GNOME (Cairo + FreeType + HarfBuzz, fontconfig):** FreeType autohinter +
  grayscale; slight-hinting default. *Take:* the FreeType pipeline shape
  (parse → hint → scanline-rasterize → coverage AA → cache) is exactly what
  `raefont` already implements. *Avoid:* fontconfig's complexity — we ship a
  fixed 2-family set, not a system font picker (yet).
- **KDE (FreeType, same stack), SteamOS (Big Picture, large sizes only):**
  confirm the same pipeline; SteamOS never renders body-size text so it dodges
  the hinting problem — we cannot, the desktop is dense.

**Distilled principle:** *real outlines + grayscale coverage AA + grid-fit
hinting at small ppem + gamma-correct compositing + a per-(glyph,ppem) cache.*
That is a FreeType-class pipeline. The relevant finding (below) is that AthenaOS
**already has one** — it is unwired, untested, and starved of a font file.

---

## Verify-before-spec: what already exists (this reframes the whole task)

The original framing was "atlas vs. runtime rasterizer, build one." The codebase
audit changes that — a runtime rasterizer is **already built**:

| Asset | Where | State |
|---|---|---|
| 8×8 bitmap path (the blocky text on screen) | `raegfx::Canvas::draw_glyph` / `draw_text` / `draw_glyph_scaled` (`components/raegfx/src/lib.rs:133–230`) | LIVE, used by **every kernel surface** (window_chrome, login_ui, setup_ui, notify, widgets, control_panel) |
| **Full no_std TrueType/OpenType engine** | `components/raefont/src/lib.rs` (4055 lines) | BUILT, **unwired, ZERO host KATs**. Has: sfnt/TTC container, cmap (fmt 0/4/6/12/14), glyf (simple+composite), loca/hmtx/hhea/maxp/kern/OS-2/gasp/GDEF/COLR/CPAL/fvar, a **TrueType hinting bytecode interpreter**, a **scanline `Rasterizer`** with grayscale AA + gamma + oversample + (unused) subpixel, `TextShaper`, `FontDatabase` (matching+fallback), `GlyphCache`, global `FontEngine`. |
| raefont → Canvas glue | `components/raeui/src/text.rs` (`TextRenderer::render_text`, `measure_text`, `break_lines`) | BUILT, **has a compositing bug** (see Integration §3.4). Userspace-side only; not reachable from kernel surfaces. |
| **Duplicate twin** | `components/raegfx/src/font.rs` — a *second* TrueType `FontEngine` | DEAD (`#![allow(dead_code)]`). Violates CLAUDE.md rule 7 (no parallel twins). **Must be quarantined, not extended.** |
| Type ramp tokens | `rae_tokens::{TypeStyle, TYPE_DISPLAY..TYPE_CAPTION}` (`components/rae_tokens/src/lib.rs:413–459`) | LIVE — exactly the §6 ramp, each carries `ppem` + `weight` + `line_height`. The natural `TypeStyle` selector callers pass. |
| **The actual missing piece** | *no `.ttf`/`.otf`/`.ttc` exists anywhere in the tree* (`**/*.ttf` → 0 files) | **Nothing supplies font bytes.** `raefont` parses bytes it is never given. |

**So the delta is not "build a rasterizer." It is: embed a font, host-KAT the
existing rasterizer, fix the blend bug, kill the twin, and wire ONE crisp path
to BOTH the kernel surfaces and the userspace apps.**

---

## Decision 1 — rendering approach: **runtime vector rasterizer (raefont)**, not a baked atlas

The brief offered (a) a pre-baked grayscale-AA bitmap atlas vs (b) a runtime
vector rasterizer. With a complete rasterizer already in-tree, the trade-off
table tips decisively to (b), but with the atlas idea **retained as the cache
shape**:

| Factor | (a) Baked AA atlas | (b) Runtime rasterizer (`raefont`) | Decision |
|---|---|---|---|
| Build cost from today | New: an offline baker tool + per-size PNG/raw blobs + a blit path. | **Already written** (4055 lines). Cost = wire + test + 1 font file. | **(b)** — less *net* work because (b) exists |
| no_std / soft-float | Trivial (just blits bytes). | `raefont` is `#![no_std]`; its floats are the compositor-style GPR f32 math (memory `kernel-soft-float` — no FPU save needed, same as compositor HDR). Verified: only deps = `alloc`. | both fine |
| Binary size | One atlas **per size per weight** (6 sizes × 3 weights × 2 families ≈ 36 sheets) — each a full coverage bitmap. Bloats fast. | One compressed/subset `.ttf` per family (~100–300 KB subset) covers **all** sizes/weights via outlines. | **(b)** smaller for full ramp |
| Arbitrary sizes (zoom, OOBE 32px clock, future HiDPI) | Locked to baked sizes; off-ramp sizes re-blur. | Any ppem, crisp. | **(b)** |
| Kerning / hinting / ligatures | None (atlas is pre-spaced; no pair kerning unless baked per-pair — impractical). | `kern` table + hinting interpreter + shaper already present. | **(b)** |
| First-pixel latency / determinism | Instant (no rasterize). | First glyph at a ppem rasterizes once, then **cached** (`GlyphCache`/`GlyphCacheMap`) → steady-state is an atlas anyway. | tie (cache = runtime atlas) |
| Boot-path risk (CLAUDE.md rule 12/15) | None. | Rasterizing in the boot critical path could cost ms. **Mitigation:** keep the 8×8 path for *pre-FontEngine-init* boot text; AA path activates after `FontEngine::init()` + font load, off the critical boot smoketest. | manageable |

**Tie-breaker (Concept/feel > simplest-correct > reversibility):** *feel* demands
crisp text at every size including the 32px OOBE clock and future HiDPI — the
atlas cannot do arbitrary sizes or kerning, so it caps the ceiling below
macOS/Win11. *Simplest-correct* also favours (b) here precisely because the
rasterizer is already written; building an atlas baker is the *new* code.
**Chosen: runtime vector rasterizer (`raefont`), with its per-(glyph,ppem)
`GlyphCache` serving as the de-facto runtime atlas** — we get the atlas's
steady-state speed and the vector path's unbounded crispness.

The 8×8 bitmap path is **retained as the early-boot fallback only** (before the
font engine initialises), and as the `'?'`-tofu fallback for unmapped glyphs.

---

## Decision 2 — font faces & licensing

| Role | Face | License | Embeddable? | Subset shipped |
|---|---|---|---|---|
| **RaeSans** (UI) | **Inter** (humanist grotesque; large x-height, designed for screens — matches §6 "Inter / SF Pro / Segoe family"; `ThemeAbi.font_family` already defaults to `"Inter"`) | **SIL OFL 1.1** | **Yes** — OFL explicitly permits bundling in commercial/closed products | Weights **400 / 500 / 600** (Regular/Medium/SemiBold), Latin + punctuation + arrows/box-drawing for chrome. Variable `.ttf` acceptable (raefont parses `fvar`) but ship **static instances** for the 3 weights to keep the rasterizer path simple. |
| **RaeMono** (terminal/code) | **JetBrains Mono** | **SIL OFL 1.1** | **Yes** | Weights 400 / 500; Latin + box-drawing + common programming ligature glyphs (ligatures optional — shaper supports, can defer). |

**OFL compliance obligations (carry into the commit that adds the files):**
- Keep the upstream `OFL.txt` alongside each font.
- Do **not** rename the font files to a reserved name; "RaeSans"/"RaeMono" are
  *our family aliases in `FontPattern`/`ThemeAbi`*, mapping to the embedded
  Inter/JetBrains Mono — we do **not** rebrand the font's internal `name` table
  (avoids the OFL Reserved Font Name clause).
- License text + attribution belong in `docs/THIRD_PARTY_LICENSES` (or repo
  equivalent).

**Where the files live:** `components/raefont/assets/Inter-{Regular,Medium,
SemiBold}.ttf` + `JetBrainsMono-{Regular,Medium}.ttf` + `OFL.txt`, embedded into
the binary via `include_bytes!` behind a `raefont::builtin` module that returns
`&'static [u8]` (no filesystem dependency — must work at first boot before AthFS
mounts). Subset with a tool like `fonttools subset` *offline* to control binary
size; record the exact subset command in the assets README so the blob is
reproducible (CLAUDE.md off-target/reproducibility discipline).

> **DESIGN_LANGUAGE.md §6 proposed addendum** (a metric §6 lacks): add a line
> noting the embedded face files + that `raefont::builtin::rae_sans(weight)`
> resolves the `ThemeAbi.font_family="Inter"` default to bytes, and that
> grayscale AA (not sub-pixel) is the system AA mode. (Docs-only note; I did not
> edit §6 in this pass — see Handoff.)

---

## AthenaOS design tokens this surface uses

This surface consumes §6 wholesale and adds **rendering-mode** tokens (proposed
for DESIGN_LANGUAGE.md §6 if accepted):

- **type ramp:** `TYPE_DISPLAY` 32/600, `TYPE_TITLE` 22/600, `TYPE_SUBTITLE`
  17/500, `TYPE_BODY` 14/400, `TYPE_LABEL` 13/500, `TYPE_CAPTION` 11/400
  (`rae_tokens::TypeStyle`, already live). `ppem` = the px field.
- **colour:** glyph fg = `text.primary` / `text.secondary` / `text.tertiary`
  (§4); coverage AA modulates the fg's alpha — fg RGB is constant, alpha = glyph
  coverage × fg alpha.
- **`aa.mode` = grayscale** (proposed token). Not sub-pixel. Rationale: software
  compositor + translucent glass backdrops.
- **`aa.gamma` = 2.2** (proposed). Gamma-correct coverage blend (see §5). The
  rasterizer currently hardcodes `1.8`; standardise to 2.2 (sRGB-ish) and make
  it a token so Vibe Mode / a11y "increase text contrast" can tune it.
- **`text.hint_ppem_floor` = 16** (proposed): below 16 ppem, enable grid-fit
  hinting (the interpreter exists); at/above, hinting optional. Keeps 11/13/14px
  body text crisp instead of fuzzy.
- **motion:** none — text does not animate per-glyph; it fades with its surface
  (§7 `motion.*`). Caret blink is a surface concern, not here.

---

## Integration — how `Canvas::draw_text` gains an AA path

The non-negotiable cohesion requirement: **one crisp path feeds BOTH the kernel
surfaces and the userspace apps**, or we re-create the incoherence DESIGN_
LANGUAGE.md exists to prevent. The path:

### 3.1 New API on `raegfx::Canvas` (the shared chokepoint)

`raegfx` is the one crate both the kernel (compositor/window_chrome) and
userspace apps already depend on for `Canvas`. The AA entry point lands here so
there is exactly one text API:

```
// proposed signature shape (raeen-gfx implements):
impl Canvas {
    /// Draw `s` with the system font at the given type style, fg colour, and
    /// origin (baseline-relative y handled internally from font metrics).
    /// Returns the x advance after the last glyph (kerned).
    /// Falls back to the 8x8 bitmap path if the font engine is not yet ready.
    pub fn draw_text_aa(
        &mut self,
        x: i32, y: i32,
        s: &str,
        style: rae_tokens::TypeStyle,   // selects ppem + weight
        fg: u32,                         // ARGB; alpha respected
        family: FontFamily,             // Sans | Mono
    ) -> i32;
}
```

- Internally calls a `raegfx::text` module that owns a `&'static mut FontEngine`
  (raefont's global) + the per-(glyph,ppem,weight) `GlyphCache`.
- `style.weight` → picks the Inter 400/500/600 instance in the `FontDatabase`.
- Per-glyph: `cmap.lookup` → `loca`/`glyf` → `Rasterizer::rasterize` (cached) →
  `blend_glyph` (the corrected blit, §3.4) → advance by `hmtx` + `kern`.
- The **8×8 `draw_text` stays** as `draw_text` (unchanged) for early boot and as
  the internal fallback when `FontEngine::get()` is `None`.

### 3.2 Resolve the duplicate-engine violation FIRST

`components/raegfx/src/font.rs` is a second TrueType engine and must **not** be
the one wired (CLAUDE.md rule 7 — extend the wired module, never a twin). Decide
explicitly: the **standalone `raefont` crate is the wired engine** (it is more
complete — hinting interpreter, COLR/CPAL, shaper). `raegfx/src/font.rs` is
quarantined per `docs/QUARANTINED_MODULES.md` (or deleted), and `raegfx` depends
on `raefont`. This is a prerequisite, not optional — shipping two TrueType
engines is exactly the incoherence risk in OS form.

### 3.3 How callers pick a `TypeStyle`

Callers already have the ramp. Migration is mechanical: every
`canvas.draw_text(x, y, s, fg, None)` becomes
`canvas.draw_text_aa(x, y, s, TYPE_BODY, fg, FontFamily::Sans)` (or the surface's
ramp entry). Terminal uses `FontFamily::Mono`. Surfaces to migrate (all current
8×8 callers): `kernel/src/window_chrome.rs`, `login_ui`, `setup_ui` (OOBE),
`notify`, `widgets`, `control_panel`; userspace `raeshell/desktop`, the 6 app
ELF crates, `raekit` views. The accent/colour tokens they pass are unchanged.

### 3.4 Fix the compositing bug in the existing blit (load-bearing)

`raeui/src/text.rs::blit_glyph` (line ~387) does **not** composite over the
destination — it computes `(b + inv * 0 / 255)` and writes opaque
`0xFF000000 | premultiplied_fg`, i.e. it blends the glyph against **black**, not
against the actual glass/gradient pixel underneath. On the dark desktop this
looks *almost* right and is exactly why it was never caught; over light surfaces
or glass it produces dark halos. The corrected blend (§5) must:
`out = src_over(fg·coverage, dst)` using the **destination pixel read back**
(`Canvas::read_pixel`/`blend_pixel` already exist — the 8×8 scaled path at
lib.rs:207 already uses `blend_pixel` correctly; reuse that). The new
`draw_text_aa` should route through `Canvas::blend_pixel`, not hand-roll it.

### 3.5 Coexistence / fallback ladder

1. **Before `FontEngine::init()`** (early boot, panic screen, pre-AthFS): 8×8
   bitmap. Always available, zero deps.
2. **After init + builtin font load:** `draw_text_aa` → raefont. This is the
   desktop, OOBE, login, every app.
3. **Unmapped glyph (no cmap entry, no fallback hit):** raefont returns tofu;
   draw the 8×8 `'?'` so nothing is invisible.

`FontEngine::init()` + `database.add_font(builtin bytes)` is called once, after
the framebuffer is up and before `activate_desktop` (so the first composited
desktop frame is already crisp). Per memory `iron-console-logging-tax`, do this
*off* the serial-logging hot path.

---

## States & interaction

Text rendering is mostly stateless, but the **a11y / mode** matrix still applies:

- **default / dark:** fg = `text.primary` (`0xFF_F0_F2_F8`), grayscale AA,
  gamma 2.2.
- **light:** identical pipeline; fg = `text.primary` (`0xFF_14_18_22`). Gamma-
  correct blend matters MORE here (dark text on light has the classic
  "AA looks too thin" problem — stem darkening token available, default off).
- **disabled:** fg = `text.tertiary`; no separate render path.
- **focus:** text colour unchanged; focus is the surface's ring/glow (§8), never
  a text-colour-only change (a11y rule).
- **reduced-motion:** no effect on glyph rendering (text never animates per-
  glyph); surface fade still respects `motion.instant`.
- **high-contrast / "larger text" a11y:** ppem scales via the `TypeStyle` the
  surface passes (a global scale factor multiplies `style.ppem`); `aa.gamma` and
  optional stem-darkening tune weight. **Flag to raeen-accessibility:** define
  the global text-scale factor (e.g. 100/125/150/200%) and its token home; the
  rasterizer already supports arbitrary ppem so this is a token+plumbing task,
  not a render-engine task.

**Keyboard/controller:** N/A to glyph rendering. Caret position/measurement uses
`measure_text` / `break_lines` (already in `raeui/text.rs`) — these must be
re-pointed at the corrected pipeline so hit-testing and cursor placement match
what is drawn.

---

## Anti-aliasing on the software compositor (the correctness core)

- **Grayscale, not sub-pixel.** Sub-pixel (LCD-stripe) AA assumes a known RGB
  sub-pixel order and no rotation, and produces colour fringes over our blurred
  glass/gradient backdrops. The rasterizer's `SubpixelMode::None` is the shipped
  mode; the `Rgb/Bgr/Vrgb/Vbgr` modes stay built but **off**.
- **Coverage = alpha.** The rasterizer emits an 8-bit coverage per pixel (0–255)
  via scanline area coverage. The blit treats coverage as the source alpha and
  does **source-over** against the real destination pixel:
  `out_c = fg_c · α + dst_c · (1 − α)` per channel, `α = coverage/255 · fg_alpha`.
- **Gamma-correct blending.** Compositing coverage in sRGB space makes text look
  too thin (light-on-dark) or too heavy (dark-on-light) — the classic "fuzzy /
  wrong-weight" tell that separates amateur from system text. The fix:
  linearise dst and fg (sRGB→linear, `aa.gamma` 2.2), blend in linear, convert
  back. The compositor **already has an sRGB↔linear path** (`HdrPipeline`,
  DESIGN_LANGUAGE.md §0) — reuse its LUT/Taylor approximation rather than adding
  floats to the glyph hot loop. raefont's `gamma` field currently applies gamma
  to the *coverage mask* at rasterize time (an approximation); standardise to
  **blend-time gamma** for correctness, or document the coverage-gamma as the
  cheaper approximation if blend-time proves too costly on the software raster.
  (raeen-gfx owns this perf/correctness call; host-KAT both.)
- **Hinting at small ppem.** Below `text.hint_ppem_floor` (16), run the TrueType
  hinting interpreter (present in raefont) or vertical-stem grid-fit so 11/13/14
  px body text snaps to the pixel grid. This is the difference between "crisp
  small text" and "soft grey mush" — it is *why* the 8×8 bitmap currently looks
  sharper than a naive vector path would at body size, and why we must hint.

---

## Already built (delta only)

Restating the delta crisply, because most of this exists:

- **Built, keep:** `raefont` parser + hinting interpreter + grayscale `Rasterizer`
  + shaper + `FontDatabase` + `GlyphCache`; `rae_tokens::TypeStyle` ramp;
  `raeui/text.rs` measure/break-lines; `Canvas::blend_pixel`.
- **Build (the actual new work):**
  1. **Embed an Inter + JetBrains Mono subset** (`include_bytes!`) — *nothing
     supplies font bytes today; this unblocks everything.*
  2. **Host-KAT the rasterizer** — it has **zero** tests; rasterize a known
     glyph at a known ppem, assert the coverage bitmap (FAIL-able). CLAUDE.md
     rule 15 (pure logic → host KAT first) is mandatory and unmet.
  3. **Fix `blit_glyph` source-over + gamma** (§3.4/§5) — current code blends
     against black.
  4. **Add `Canvas::draw_text_aa`** in `raegfx` (the shared API).
  5. **Quarantine the `raegfx::font` twin**; wire `raefont` as the one engine.
  6. **Migrate the kernel + app surfaces** off 8×8 to `draw_text_aa`.
- **Do NOT rebuild:** a rasterizer, an atlas baker, a parser, a cache.

---

## Staging — smallest-provable increments + proof per step

| Stage | Scope | Proof (cheapest layer that can FAIL) |
|---|---|---|
| **S0** | Quarantine `raegfx::font.rs` twin; make `raegfx` depend on `raefont`. | `cargo run -p xtask --release -- build --release` exit 0; `docs/QUARANTINED_MODULES.md` row added; no `raegfx::font` references remain (grep). |
| **S1** *(first increment — see below)* | Embed **Inter SemiBold** subset; **host-KAT** `raefont::Rasterizer` on one glyph ('A') at `TYPE_TITLE.ppem` (22): assert non-zero coverage, expected width/height bounds, and that lowering coverage→0 outside the glyph bbox. | `cargo test -p raefont` (per-crate, per memory `no-std-workspace-host-test`): KAT prints `raefont rasterize 'A'@22: WxH=.. cov_sum=..` and asserts bounds; flips to FAIL if coverage is all-zero (catches "no font bytes"). |
| **S2** | Fix `blit_glyph` source-over + gamma; host-KAT the **blend** (fg over a known dst → expected ARGB, incl. a light-bg case that the old black-blend would fail). | `cargo test -p raeui` (or wherever blend lands): `blend AA glyph over 0xFF101418 and over 0xFFEFF2F8` asserts no dark halo on light. FAIL-able. |
| **S3** | `Canvas::draw_text_aa` + full ramp (all 6 `TYPE_*`, weights 400/500/600) + JetBrains Mono for Mono family; `FontEngine::init()` wired before `activate_desktop`. | QEMU boot: serial marker `[raefont] engine ready: families=2 weights=3` + smoketest renders one AA string to an offscreen canvas and asserts cached glyph count > 0. No `[PANIC]`; `System successfully booted`. |
| **S4** | Migrate OOBE/login/window-chrome/notify + the desktop + the 6 apps to `draw_text_aa`. | **Re-screenshot for raeen-visual-qa** (host-render + QEMU-window + iron — per memory `ui-glass-design-system`: headless QEMU screendump striping is a capture artifact, verify on the clean surfaces). Boot-time guard: no new `[boot] WARN`, `[BOOT-BENCH]` not regressed (rasterize-on-first-frame must not blow the 6 s budget). |

**Boot-time watch (CLAUDE.md rule 12/15):** S3/S4 add per-glyph rasterization to
the first desktop frame. The `GlyphCache` makes steady state cheap, but the first
paint of OOBE/login could add ms. If `[BOOT-BENCH]` regresses, pre-warm the cache
for the ASCII set at `TYPE_BODY`/`TYPE_TITLE` during `FontEngine::init()` (off the
critical smoketest path), or defer non-visible-surface text.

---

## Handoff

- **Implementer — `raefont` crate owner (raeen-gfx-adjacent):** owns S0–S2 — embed
  the subset + `builtin` module, add the host KATs (the rasterizer's first
  tests), fix/standardise gamma. *This crate is the engine; do not extend the
  `raegfx::font` twin.*
- **Implementer — raeen-gfx (the Canvas path):** owns S3 — `Canvas::draw_text_aa`,
  the `raegfx::text` module owning the `FontEngine` + cache, the source-over
  blend through `Canvas::blend_pixel`, and the gamma-correct blend reusing
  `HdrPipeline`'s sRGB↔linear. Decides coverage-gamma vs blend-gamma on perf.
- **Implementer — raeen-ui / raeen-shell-apps (adoption):** owns S4 — migrate
  every 8×8 caller (kernel surfaces + the 6 app ELFs + raekit views) to
  `draw_text_aa(.., TYPE_*, .., family)`; re-point `measure_text`/`break_lines`
  caret math at the wired pipeline.
- **raeen-accessibility — flagged items:** (1) define the global **text-scale
  factor** (100/125/150/200%) and its token home — engine already supports
  arbitrary ppem; (2) confirm `aa.gamma`/stem-darkening defaults meet contrast
  with §4.2 ratios at body size; (3) verify hinting actually keeps 11px
  `TYPE_CAPTION` above the legibility floor on a 1× panel.
- **DESIGN_LANGUAGE.md §6:** propose adding the rendering-mode lines (embedded
  faces, `aa.mode = grayscale`, `aa.gamma = 2.2`, `text.hint_ppem_floor = 16`).
  *Not edited in this pass — flagged for the next DESIGN_LANGUAGE update so the
  token addition is reviewed, not silently inlined.*

### Visual-QA verification list (hand to raeen-visual-qa after S4)

Screenshot and verify on **host-render + iron** (not headless QEMU screendump —
striping artifact):
1. The OOBE 32px clock/heading (`TYPE_DISPLAY`) — crisp curved stems, no
   stair-stepping, no dark halo on the gradient.
2. Window titlebar text (`TYPE_TITLE`/`TYPE_LABEL`) at 1× — legible, kerned
   ("AV"/"To" pairs tightened), not blocky.
3. Body text (`TYPE_BODY` 14px, `TYPE_CAPTION` 11px) on `bg.raised` AND on light
   palette — small text crisp (hinting working), no fuzzy grey stems, no fringe.
4. Terminal text (JetBrains Mono, `FontFamily::Mono`) — even monospace advance,
   box-drawing glyphs align.
5. Text over **glass** (Start menu / quick-settings) — AA composites over the
   blurred backdrop with no black/colour halo (proves the source-over + gamma
   fix and grayscale-not-subpixel choice).
6. Side-by-side before/after of the *same* surface vs the original 8×8
   screenshot — the "basic/not crisp" verdict should flip.

### Unblocks checklist lines

- Phase 8 (AthUI/AthKit) — real text rendering is the precondition for "cohesive
  and visually stunning" UI; every surface spec assumes crisp type.
- Phase 14 (AthShell + apps) — the 6 apps + terminal need readable text to be
  shippable.
- The `ui-glass-design-system` memory's open follow-up: "raefont = crisp-text
  follow-up" — this spec is its plan.

---

## THE single first increment to build (S1)

**Embed an Inter SemiBold subset into `raefont` via `include_bytes!` and add the
rasterizer's first host KAT.**

Concretely:
1. Add `components/raefont/assets/Inter-SemiBold.ttf` (offline-subset Latin +
   punctuation) + `OFL.txt`; add `raefont::builtin::rae_sans_semibold() ->
   &'static [u8]`.
2. Add `#[cfg(test)] mod tests` to `raefont`: load the builtin bytes via
   `FontHandle::from_bytes`, look up `'A'`, parse its `glyf`, rasterize at
   `ppem = 22` (`TYPE_TITLE`), and assert:
   - `FontHandle::from_bytes(..).is_some()` (font parsed),
   - rasterized `width > 0 && height > 0`,
   - `coverage.iter().map(|&p| p as u32).sum() > 0` (glyph actually inked —
     **this is the FAIL line**: if no bytes are embedded or parsing breaks,
     coverage is all-zero and the test fails),
   - a corner pixel outside the 'A' bbox has coverage `0` (no overflow ink).

**Exact proof:**
```
cargo test -p raefont
# expect: test rasterize_inter_A_at_title ... ok
# stdout: [raefont-kat] 'A'@22 W=.. H=.. cov_sum=.. (>0)
```
This is the cheapest layer that proves the whole thesis viable (a real face
parses and rasterizes to non-trivial AA coverage in our no_std crate) and is the
prerequisite every later stage builds on — without embedded bytes, raefont
rasterizes nothing. It touches no kernel code, no boot path, and cannot produce a
false green (the `cov_sum > 0` assertion fails loudly if the font is missing or
the parser regresses).

---

**Sources (font licensing):**
- [Inter Font: License — SIL Open Font License](https://madegooddesigns.com/inter-font/)
- [JetBrains Mono — free and open-source typeface (OFL)](https://www.jetbrains.com/lp/mono/)
- [SIL Open Font License — permits embedding/bundling](https://en.wikipedia.org/wiki/SIL_Open_Font_License)
