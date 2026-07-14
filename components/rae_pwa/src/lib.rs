//! # RaePWA — a never-panic W3C Web App Manifest parser (`no_std`).
//!
//! RaeenOS_Concept.md §"web apps via PWA support that actually feels native":
//! the web is the universal app runtime, and a Progressive Web App identifies
//! itself with a [W3C Web App Manifest](https://www.w3.org/TR/appmanifest/) — a
//! JSON `manifest.json` carrying the app's name, icons, display mode, and theme.
//! Turning that document into a typed, validated model is the *first* step of
//! "install to desktop": it is what lets a web app get a real desktop icon, a
//! standalone window (no browser chrome), and a theme color — so it "actually
//! feels native" rather than living as a bookmark.
//!
//! ## What it is
//! - [`WebAppManifest`]: the typed manifest (name/short_name/start_url/scope/
//!   [`DisplayMode`]/theme+background color as ARGB/[`Icon`]s/description/lang/
//!   dir/orientation), with sensible defaults for every optional field.
//! - [`parse_manifest`]: `&str` → [`WebAppManifest`], built on the never-panic
//!   [`rae_json`] core. A manifest that does not identify the app (no `name` and
//!   no `short_name`) is rejected; everything else degrades gracefully.
//! - Helpers: [`WebAppManifest::display_name`], [`WebAppManifest::best_icon`]
//!   (size-aware icon pick), [`WebAppManifest::effective_scope`], and
//!   [`WebAppManifest::to_app_entry`] (the minimal launch descriptor the OS's
//!   install-to-desktop path will later consume — data model only this slice).
//! - [`resolve_url`]: RFC-3986 reference resolution of a manifest-relative
//!   reference (an icon `src`, `start_url`, or `scope`) against the absolute URL
//!   the `manifest.json` was fetched from — the step that turns `/icons/192.png`
//!   into `https://app.example/icons/192.png` so the launcher can actually open
//!   it. [`InstallDescriptor`] + [`WebAppManifest::install`] bundle a parsed
//!   manifest + its fetch URL into a fully-resolved, installable app record
//!   (stable id, resolved `start_url`/`scope`/icon, origin, display, colors).
//! - [`parse_css_color`]: a never-panic CSS-hex (`#rgb`/`#rrggbb`/`#rrggbbaa`)
//!   → ARGB `u32` parser for `theme_color`/`background_color`.
//!
//! ## Hostile-input posture (CLAUDE §10: untrusted bytes are an RCE surface)
//! A manifest is fetched from the open web and is therefore hostile. There is no
//! `unwrap`/`expect`/`panic`/raw-index-panic path reachable from
//! [`parse_manifest`]: malformed JSON, a JSON value that is not an object,
//! wrong-typed fields, a missing name, an `icons` value that is not an array, a
//! garbage color, and empty input all return `Err(ManifestError)`. Color parsing
//! never panics — an unrecognised color falls back to a defined sentinel rather
//! than erroring, because a bad accent color must not stop an app from
//! installing. The host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p rae_pwa`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rae_json::Json;

/// The fallback ARGB value [`parse_css_color`] returns for an unrecognised or
/// malformed color: fully-transparent black. Distinct from any explicit opaque
/// color a manifest could specify, so a consumer can treat "transparent" as
/// "no usable theme color" and fall back to an OS default.
pub const COLOR_NONE: u32 = 0x0000_0000;

/// The W3C [`display`](https://www.w3.org/TR/appmanifest/#display-member) member:
/// how much browser UI the app wants when launched. The spec default is
/// [`Browser`](DisplayMode::Browser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// All available display area; no OS/browser chrome.
    Fullscreen,
    /// A standalone app window (own icon, no browser UI) — the "feels native"
    /// mode install-to-desktop prefers.
    Standalone,
    /// Standalone plus a minimal set of navigation controls.
    MinimalUi,
    /// Opens in a conventional browser tab (the spec default).
    Browser,
}

impl DisplayMode {
    /// Parse a `display` string (case-insensitive). Unknown values fall back to
    /// the spec default, [`Browser`](DisplayMode::Browser).
    pub fn parse(s: &str) -> DisplayMode {
        // ASCII-lowercase compare without allocating.
        if eq_ignore_ascii_case(s, "fullscreen") {
            DisplayMode::Fullscreen
        } else if eq_ignore_ascii_case(s, "standalone") {
            DisplayMode::Standalone
        } else if eq_ignore_ascii_case(s, "minimal-ui") {
            DisplayMode::MinimalUi
        } else {
            DisplayMode::Browser
        }
    }

    /// The canonical manifest spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            DisplayMode::Fullscreen => "fullscreen",
            DisplayMode::Standalone => "standalone",
            DisplayMode::MinimalUi => "minimal-ui",
            DisplayMode::Browser => "browser",
        }
    }
}

impl Default for DisplayMode {
    fn default() -> Self {
        DisplayMode::Browser
    }
}

/// One entry of the manifest [`icons`](https://www.w3.org/TR/appmanifest/#icons-member)
/// array: a source URL plus optional size/type/purpose hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    /// The image URL (`src`). Required for an icon to be usable.
    pub src: String,
    /// The raw `sizes` string, e.g. `"192x192"`, `"any"`, or `"16x16 32x32"`.
    pub sizes: String,
    /// The MIME `type`, e.g. `"image/png"` (empty if unspecified).
    pub mime_type: String,
    /// The `purpose`, e.g. `"any"`, `"maskable"`, `"monochrome"` (empty if
    /// unspecified).
    pub purpose: String,
}

impl Icon {
    /// The largest square pixel dimension this icon advertises in `sizes`, or
    /// `None` if it specifies only `"any"` / nothing parseable. `"any"` is
    /// treated as scalable (see [`is_scalable`](Icon::is_scalable)).
    pub fn max_dimension(&self) -> Option<u32> {
        let mut best: Option<u32> = None;
        for token in self.sizes.split_whitespace() {
            if eq_ignore_ascii_case(token, "any") {
                continue;
            }
            // token is "WxH"; take the width (icons are conventionally square).
            if let Some(px) = parse_size_token(token) {
                best = Some(best.map_or(px, |b| if px > b { px } else { b }));
            }
        }
        best
    }

    /// `true` if `sizes` contains the scalable keyword `"any"` (e.g. an SVG).
    pub fn is_scalable(&self) -> bool {
        self.sizes
            .split_whitespace()
            .any(|t| eq_ignore_ascii_case(t, "any"))
    }

    /// `true` if this icon advertises `target` among its space-separated
    /// `purpose` keywords. Per the W3C spec, an icon with no `purpose` defaults
    /// to [`IconPurpose::Any`], so an empty `purpose` matches
    /// [`IconPurpose::Any`] only. Comparison is ASCII-case-insensitive.
    pub fn has_purpose(&self, target: IconPurpose) -> bool {
        let trimmed = self.purpose.trim();
        if trimmed.is_empty() {
            return target == IconPurpose::Any;
        }
        self.purpose
            .split_whitespace()
            .any(|tok| IconPurpose::parse(tok) == target)
    }
}

/// The W3C [`purpose`](https://www.w3.org/TR/appmanifest/#purpose-member) of an
/// icon: which presentation context it is intended for. Unknown keywords parse
/// to [`Any`](IconPurpose::Any) (the spec default) so a novel/garbage purpose
/// never causes an icon to be silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconPurpose {
    /// General-purpose; safe to display anywhere (the default).
    Any,
    /// Designed to fill a maskable safe-zone (adaptive/round home-screen icons).
    Maskable,
    /// A single-color glyph the OS may recolor (e.g. monochrome tray icon).
    Monochrome,
}

impl IconPurpose {
    /// Parse one `purpose` keyword (case-insensitive); unknown -> [`Any`](IconPurpose::Any).
    pub fn parse(s: &str) -> IconPurpose {
        if eq_ignore_ascii_case(s, "maskable") {
            IconPurpose::Maskable
        } else if eq_ignore_ascii_case(s, "monochrome") {
            IconPurpose::Monochrome
        } else {
            IconPurpose::Any
        }
    }
}

impl Default for IconPurpose {
    fn default() -> Self {
        IconPurpose::Any
    }
}

/// Why parsing a manifest failed. A manifest that does not *identify* the app is
/// the only "well-formed but rejected" case; the rest are structurally invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    /// The bytes were not valid JSON.
    BadJson,
    /// Valid JSON, but the top-level value was not an object.
    NotAnObject,
    /// Neither `name` nor `short_name` was present and non-empty — the app has
    /// no identity, so it cannot be installed.
    NoIdentity,
    /// A field that must be an array (`icons`) was present but not an array.
    BadIconsType,
}

/// A parsed, validated W3C Web App Manifest.
///
/// Optional fields carry sensible defaults: an absent string field is an empty
/// `String`, an absent `display` is [`DisplayMode::Browser`], and an absent
/// color is [`COLOR_NONE`]. At least one of `name` / `short_name` is guaranteed
/// non-empty (enforced by [`parse_manifest`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebAppManifest {
    /// The full human-readable name.
    pub name: String,
    /// A short name for constrained surfaces (taskbar, home screen).
    pub short_name: String,
    /// The URL the app loads at launch.
    pub start_url: String,
    /// The navigation scope; empty means "derive from `start_url`'s directory"
    /// (see [`effective_scope`](WebAppManifest::effective_scope)).
    pub scope: String,
    /// The preferred display mode.
    pub display: DisplayMode,
    /// `theme_color` as ARGB `u32` ([`COLOR_NONE`] if unspecified/invalid).
    pub theme_color: u32,
    /// `background_color` as ARGB `u32` ([`COLOR_NONE`] if unspecified/invalid).
    pub background_color: u32,
    /// The declared icons (may be empty).
    pub icons: Vec<Icon>,
    /// Free-text description.
    pub description: String,
    /// BCP 47 language tag, e.g. `"en-US"`.
    pub lang: String,
    /// Base text direction: `"ltr"`, `"rtl"`, or `"auto"`.
    pub dir: String,
    /// Default orientation, e.g. `"portrait"`, `"landscape"`.
    pub orientation: String,
}

/// The minimal launch descriptor the OS "install to desktop" path will consume:
/// just enough to register a desktop entry and open the app's window. (The
/// launcher wiring lives in raeshell/start_menu and is intentionally NOT done
/// here — this is the data model only.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    /// The label to show under the desktop/menu icon.
    pub name: String,
    /// The URL the window opens at.
    pub start_url: String,
    /// The window style to launch with.
    pub display: DisplayMode,
    /// The accent/theme color as ARGB ([`COLOR_NONE`] if none).
    pub theme_color: u32,
    /// The chosen icon's `src` URL, or `None` if the manifest shipped no usable
    /// icon (the installer would then synthesize a placeholder).
    pub icon_src: Option<String>,
}

/// A fully-resolved, installable web-app record: the output of
/// [`WebAppManifest::install`] and the unit an "Install to desktop" action
/// persists. Unlike [`AppEntry`], every URL here is **absolute** (resolved
/// against the manifest's fetch URL), so the launcher can open the app window
/// without any further context.
///
/// The `origin` + `scope` are the basis of the per-origin capability sandbox the
/// security layer applies to the installed app (RaeenOS_Concept.md: "every web
/// origin runs sandboxed") — this crate only *describes* them; enforcement is
/// the security layer's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallDescriptor {
    /// Stable identity for de-duping re-installs: the resolved `start_url`'s
    /// origin + path (no query/fragment).
    pub id: String,
    /// The label shown under the launcher icon.
    pub name: String,
    /// The security origin (`scheme://host[:port]`) the app belongs to. Empty
    /// if the manifest URL had no determinable origin (e.g. a relative test
    /// input) — the install path treats an empty origin as untrusted.
    pub origin: String,
    /// Absolute URL the app window opens at.
    pub start_url: String,
    /// Absolute navigation scope; in-scope navigations stay in the app window,
    /// out-of-scope ones hand off to the browser.
    pub scope: String,
    /// The window style to launch with.
    pub display: DisplayMode,
    /// Accent/theme color as ARGB ([`COLOR_NONE`] if none).
    pub theme_color: u32,
    /// Window background color as ARGB ([`COLOR_NONE`] if none).
    pub background_color: u32,
    /// Absolute URL of the chosen launcher icon, or `None` if the manifest
    /// shipped none (the installer synthesizes a placeholder).
    pub icon_url: Option<String>,
}

impl WebAppManifest {
    /// The name to display: `short_name` if non-empty, else `name`. One of the
    /// two is always non-empty (guaranteed by [`parse_manifest`]).
    pub fn display_name(&self) -> &str {
        if !self.short_name.is_empty() {
            &self.short_name
        } else {
            &self.name
        }
    }

    /// Pick the icon that best fits a `target_px` square.
    ///
    /// Selection order:
    /// 1. The smallest icon whose advertised size is `>= target_px` (so we scale
    ///    *down*, never blurrily up).
    /// 2. Failing that, the largest fixed-size icon (closest from below).
    /// 3. Failing that, any scalable (`"any"`) icon.
    /// 4. Failing that, the first icon present.
    ///
    /// Returns `None` only when the manifest declared no icons.
    pub fn best_icon(&self, target_px: u32) -> Option<&Icon> {
        if self.icons.is_empty() {
            return None;
        }

        let mut best_upscale: Option<(&Icon, u32)> = None; // smallest >= target
        let mut best_downscale: Option<(&Icon, u32)> = None; // largest < target
        let mut scalable: Option<&Icon> = None;

        for icon in &self.icons {
            if icon.src.is_empty() {
                continue;
            }
            match icon.max_dimension() {
                Some(px) if px >= target_px => {
                    best_upscale = Some(match best_upscale {
                        Some((bi, bp)) if bp <= px => (bi, bp),
                        _ => (icon, px),
                    });
                }
                Some(px) => {
                    best_downscale = Some(match best_downscale {
                        Some((bi, bp)) if bp >= px => (bi, bp),
                        _ => (icon, px),
                    });
                }
                None => {
                    if icon.is_scalable() && scalable.is_none() {
                        scalable = Some(icon);
                    }
                }
            }
        }

        if let Some((icon, _)) = best_upscale {
            return Some(icon);
        }
        if let Some((icon, _)) = best_downscale {
            return Some(icon);
        }
        if let Some(icon) = scalable {
            return Some(icon);
        }
        // Last resort: the first icon that at least has a src.
        self.icons.iter().find(|i| !i.src.is_empty())
    }

    /// The effective navigation scope: `scope` if set, otherwise the directory
    /// portion of `start_url` (everything up to and including the last `/`).
    /// Never panics; returns `"/"` when nothing usable is available.
    pub fn effective_scope(&self) -> String {
        if !self.scope.is_empty() {
            return self.scope.clone();
        }
        directory_of(&self.start_url)
    }

    /// `true` if `path` (a `start_url` or `scope`) is a plausible same-origin
    /// relative or absolute reference. This is a *basic* sanity check, not a URL
    /// parser: it rejects nothing-but-whitespace and obvious cross-origin
    /// absolute URLs while accepting relative paths and same-document refs.
    /// Never panics.
    pub fn is_same_origin_ish(path: &str) -> bool {
        let p = path.trim();
        if p.is_empty() {
            return false;
        }
        // A scheme-relative or absolute URL means a different origin is possible;
        // we only accept it if it is clearly not pointing off-site via a scheme.
        if p.starts_with("//") {
            return false; // scheme-relative -> arbitrary host
        }
        if let Some(idx) = find_scheme_colon(p) {
            // Has a scheme like "https:" — treat as cross-origin-capable; reject
            // for the same-origin check (the install path resolves these against
            // the manifest's own origin separately).
            let _ = idx;
            return false;
        }
        // Relative path, root-relative path, query, or fragment: same origin.
        true
    }

    /// Pick the best icon for `target_px` that *also* advertises `purpose`,
    /// using the same size logic as [`best_icon`](WebAppManifest::best_icon) but
    /// restricted to icons matching the purpose.
    ///
    /// If no icon matches the requested purpose, falls back to
    /// [`best_icon`](WebAppManifest::best_icon) over all icons (a maskable tile
    /// is nice-to-have, not a hard requirement for installing). Returns `None`
    /// only when the manifest declared no usable icon at all.
    pub fn best_icon_for_purpose(&self, target_px: u32, purpose: IconPurpose) -> Option<&Icon> {
        let matching: Vec<&Icon> = self
            .icons
            .iter()
            .filter(|i| !i.src.is_empty() && i.has_purpose(purpose))
            .collect();
        if let Some(icon) = pick_by_size(&matching, target_px) {
            return Some(icon);
        }
        self.best_icon(target_px)
    }

    /// Build the minimal [`AppEntry`] launch descriptor for install-to-desktop,
    /// choosing an icon sized for a typical desktop tile (`target_px`).
    pub fn to_app_entry(&self, target_px: u32) -> AppEntry {
        AppEntry {
            name: self.display_name().to_string(),
            start_url: self.start_url.clone(),
            display: self.display,
            theme_color: self.theme_color,
            icon_src: self.best_icon(target_px).map(|i| i.src.clone()),
        }
    }

    /// Turn this parsed manifest + the absolute URL it was *fetched from* into a
    /// fully-resolved, installable [`InstallDescriptor`] — the record an "Install
    /// to desktop" action persists and the launcher later opens.
    ///
    /// All references are resolved against `manifest_url` via [`resolve_url`], so
    /// the resulting `start_url`, `scope`, and `icon_url` are absolute and
    /// directly openable. The display-tile icon is chosen with a preference for
    /// a maskable purpose at `icon_px`. The app `id` is the resolved
    /// `start_url`'s origin + path (a stable identity for de-duping re-installs).
    ///
    /// Never panics: a relative/garbage `manifest_url` degrades to best-effort
    /// resolution and the descriptor is still produced.
    pub fn install(&self, manifest_url: &str, icon_px: u32) -> InstallDescriptor {
        // start_url default per spec: when absent, the manifest URL itself.
        let raw_start = if self.start_url.trim().is_empty() {
            manifest_url
        } else {
            self.start_url.as_str()
        };
        let start_url = resolve_url(manifest_url, raw_start);

        let scope = if self.scope.trim().is_empty() {
            directory_of(&start_url)
        } else {
            resolve_url(manifest_url, &self.scope)
        };

        let icon = self.best_icon_for_purpose(icon_px, IconPurpose::Maskable);
        let icon_url = icon.map(|i| resolve_url(manifest_url, &i.src));

        let origin = origin_of(&start_url);
        let id = app_id_of(&start_url);

        InstallDescriptor {
            id,
            name: self.display_name().to_string(),
            origin,
            start_url,
            scope,
            display: self.display,
            theme_color: self.theme_color,
            background_color: self.background_color,
            icon_url,
        }
    }
}

/// Apply the size-selection policy of
/// [`best_icon`](WebAppManifest::best_icon) to an arbitrary icon slice.
/// Shared by [`best_icon`](WebAppManifest::best_icon) and the purpose-aware
/// picker so the two never diverge. `None` if the slice is empty / all `src`-less.
fn pick_by_size<'a>(icons: &[&'a Icon], target_px: u32) -> Option<&'a Icon> {
    let mut best_upscale: Option<(&Icon, u32)> = None; // smallest >= target
    let mut best_downscale: Option<(&Icon, u32)> = None; // largest < target
    let mut scalable: Option<&Icon> = None;
    let mut first: Option<&Icon> = None;

    for &icon in icons {
        if icon.src.is_empty() {
            continue;
        }
        if first.is_none() {
            first = Some(icon);
        }
        match icon.max_dimension() {
            Some(px) if px >= target_px => {
                best_upscale = Some(match best_upscale {
                    Some((bi, bp)) if bp <= px => (bi, bp),
                    _ => (icon, px),
                });
            }
            Some(px) => {
                best_downscale = Some(match best_downscale {
                    Some((bi, bp)) if bp >= px => (bi, bp),
                    _ => (icon, px),
                });
            }
            None => {
                if icon.is_scalable() && scalable.is_none() {
                    scalable = Some(icon);
                }
            }
        }
    }

    best_upscale
        .map(|(i, _)| i)
        .or(best_downscale.map(|(i, _)| i))
        .or(scalable)
        .or(first)
}

/// Parse a W3C Web App Manifest from its JSON bytes.
///
/// Returns `Err` only for: invalid JSON ([`ManifestError::BadJson`]), a non-object
/// top level ([`ManifestError::NotAnObject`]), an `icons` value that is present
/// but not an array ([`ManifestError::BadIconsType`]), or a manifest with neither
/// `name` nor `short_name` ([`ManifestError::NoIdentity`]). Everything else
/// degrades to a default. Never panics on any input.
///
/// ```
/// use rae_pwa::{parse_manifest, DisplayMode};
/// let m = parse_manifest(r#"{"name":"Demo","display":"standalone"}"#).unwrap();
/// assert_eq!(m.display, DisplayMode::Standalone);
/// assert_eq!(m.display_name(), "Demo");
/// ```
pub fn parse_manifest(input: &str) -> Result<WebAppManifest, ManifestError> {
    let json = rae_json::parse(input).map_err(|_| ManifestError::BadJson)?;
    if json.as_object().is_none() {
        return Err(ManifestError::NotAnObject);
    }

    let name = string_field(&json, "name");
    let short_name = string_field(&json, "short_name");
    if name.is_empty() && short_name.is_empty() {
        return Err(ManifestError::NoIdentity);
    }

    // `icons`: if the key is present it MUST be an array; absent is fine (empty).
    let icons = match json.get("icons") {
        None => Vec::new(),
        Some(Json::Array(arr)) => parse_icons(arr),
        Some(_) => return Err(ManifestError::BadIconsType),
    };

    let display = json
        .get("display")
        .and_then(Json::as_str)
        .map(DisplayMode::parse)
        .unwrap_or_default();

    let theme_color = color_field(&json, "theme_color");
    let background_color = color_field(&json, "background_color");

    Ok(WebAppManifest {
        name,
        short_name,
        start_url: string_field(&json, "start_url"),
        scope: string_field(&json, "scope"),
        display,
        theme_color,
        background_color,
        icons,
        description: string_field(&json, "description"),
        lang: string_field(&json, "lang"),
        dir: string_field(&json, "dir"),
        orientation: string_field(&json, "orientation"),
    })
}

// ── Field extraction helpers (all defaulting, never-panic) ───────────────────

/// A string member's value, or `""` if absent or not a string.
fn string_field(obj: &Json, key: &str) -> String {
    obj.get(key)
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string()
}

/// A color member parsed to ARGB, or [`COLOR_NONE`] if absent/not a string/bad.
fn color_field(obj: &Json, key: &str) -> u32 {
    match obj.get(key).and_then(Json::as_str) {
        Some(s) => parse_css_color(s),
        None => COLOR_NONE,
    }
}

/// Parse the `icons` array. Each element that is an object with a non-empty
/// `src` becomes an [`Icon`]; malformed/`src`-less entries are skipped (a bad
/// icon must not sink the whole manifest).
fn parse_icons(arr: &[Json]) -> Vec<Icon> {
    let mut out = Vec::new();
    for entry in arr {
        if entry.as_object().is_none() {
            continue;
        }
        let src = string_field(entry, "src");
        if src.is_empty() {
            continue;
        }
        out.push(Icon {
            src,
            sizes: string_field(entry, "sizes"),
            mime_type: string_field(entry, "type"),
            purpose: string_field(entry, "purpose"),
        });
    }
    out
}

// ── CSS color parsing (#rgb / #rrggbb / #rrggbbaa → ARGB) ─────────────────────

/// Parse a CSS color string into an ARGB `u32` (`0xAARRGGBB`).
///
/// Supports the hex forms a manifest realistically uses:
/// `#rgb`, `#rgba`, `#rrggbb`, and `#rrggbbaa` (with or without the leading `#`).
/// Forms without an alpha channel are returned fully opaque (`0xFF` alpha).
/// Any unrecognised input (named colors like `"rebeccapurple"`, `rgb(...)`
/// functional notation, empty, or garbage) yields [`COLOR_NONE`] rather than
/// panicking — a manifest's accent color is cosmetic and must never block an
/// install.
///
/// ```
/// use rae_pwa::parse_css_color;
/// assert_eq!(parse_css_color("#f00"), 0xFFFF_0000);
/// assert_eq!(parse_css_color("#00ff00"), 0xFF00_FF00);
/// assert_eq!(parse_css_color("not-a-color"), rae_pwa::COLOR_NONE);
/// ```
pub fn parse_css_color(input: &str) -> u32 {
    let s = input.trim();
    let hex = s.strip_prefix('#').unwrap_or(s);
    let bytes = hex.as_bytes();

    match bytes.len() {
        3 => {
            // #rgb -> expand each nibble to a byte (r -> rr).
            let r = hex_nibble(bytes[0]);
            let g = hex_nibble(bytes[1]);
            let b = hex_nibble(bytes[2]);
            match (r, g, b) {
                (Some(r), Some(g), Some(b)) => argb(0xFF, r * 17, g * 17, b * 17),
                _ => COLOR_NONE,
            }
        }
        4 => {
            // #rgba
            let r = hex_nibble(bytes[0]);
            let g = hex_nibble(bytes[1]);
            let b = hex_nibble(bytes[2]);
            let a = hex_nibble(bytes[3]);
            match (r, g, b, a) {
                (Some(r), Some(g), Some(b), Some(a)) => argb(a * 17, r * 17, g * 17, b * 17),
                _ => COLOR_NONE,
            }
        }
        6 => {
            // #rrggbb
            match (hex_byte(bytes, 0), hex_byte(bytes, 2), hex_byte(bytes, 4)) {
                (Some(r), Some(g), Some(b)) => argb(0xFF, r, g, b),
                _ => COLOR_NONE,
            }
        }
        8 => {
            // #rrggbbaa
            match (
                hex_byte(bytes, 0),
                hex_byte(bytes, 2),
                hex_byte(bytes, 4),
                hex_byte(bytes, 6),
            ) {
                (Some(r), Some(g), Some(b), Some(a)) => argb(a, r, g, b),
                _ => COLOR_NONE,
            }
        }
        _ => COLOR_NONE,
    }
}

/// Pack ARGB components into `0xAARRGGBB`.
#[inline]
fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// One hex digit `[0-9a-fA-F]` → 0..=15, else `None`.
#[inline]
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Two hex digits at `bytes[i..i+2]` → a byte, or `None` if either is non-hex.
/// Bounds-checked: returns `None` rather than panicking past the slice.
#[inline]
fn hex_byte(bytes: &[u8], i: usize) -> Option<u8> {
    let hi = bytes.get(i).copied().and_then(hex_nibble)?;
    let lo = bytes.get(i + 1).copied().and_then(hex_nibble)?;
    Some((hi << 4) | lo)
}

// ── Small string utilities (no_std, alloc-free) ──────────────────────────────

/// ASCII-case-insensitive equality without allocating.
fn eq_ignore_ascii_case(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if a[i].to_ascii_lowercase() != b[i].to_ascii_lowercase() {
            return false;
        }
    }
    true
}

/// Parse a `"WxH"` size token, returning its width (the conventional square
/// dimension). `None` if it is not `digits 'x' digits`. Never panics.
fn parse_size_token(token: &str) -> Option<u32> {
    let lower = token.as_bytes();
    // find the 'x' / 'X' separator
    let mut sep = None;
    for (i, &c) in lower.iter().enumerate() {
        if c == b'x' || c == b'X' {
            sep = Some(i);
            break;
        }
    }
    let sep = sep?;
    let w = &token[..sep];
    let h = &token[sep + 1..];
    let wv = parse_u32(w)?;
    // height must also be a number for the token to be a valid "WxH".
    parse_u32(h)?;
    Some(wv)
}

/// Parse a non-empty run of ASCII digits to `u32` (saturating). `None` if empty
/// or any non-digit is present. Never panics.
fn parse_u32(s: &str) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &c in s.as_bytes() {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.saturating_mul(10).saturating_add((c - b'0') as u32);
    }
    Some(v)
}

/// The directory portion of a URL/path: everything up to and including the last
/// `/`. Returns `"/"` if there is no `/`. Never panics.
fn directory_of(url: &str) -> String {
    match url.rfind('/') {
        Some(idx) => url[..=idx].to_string(),
        None => "/".to_string(),
    }
}

/// Find the byte index of a URL scheme's `:` (as in `https://`), if the prefix
/// before it is a plausible scheme (`[A-Za-z][A-Za-z0-9+.-]*`). `None` otherwise.
fn find_scheme_colon(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    for (i, &c) in bytes.iter().enumerate() {
        if c == b':' {
            return if i > 0 { Some(i) } else { None };
        }
        let ok = c.is_ascii_alphanumeric() || c == b'+' || c == b'.' || c == b'-';
        if !ok {
            return None;
        }
    }
    None
}

// ── URL reference resolution (RFC 3986 §5, the PWA-relevant subset) ──────────

/// Resolve a (possibly relative) `reference` against an absolute `base` URL,
/// following RFC 3986 §5.3 reference resolution for the forms a Web App Manifest
/// realistically uses (`https`/`http` hierarchical URLs):
///
/// - An absolute reference (has its own scheme) is returned as-is.
/// - A scheme-relative reference (`//host/path`) inherits `base`'s scheme.
/// - A root-relative path (`/path`) replaces `base`'s path, keeping its origin.
/// - A relative path (`a/b`) is merged onto `base`'s directory.
/// - A query-only (`?q`) or fragment-only (`#f`) reference replaces only that
///   component.
/// - The merged path is normalized (`.`/`..` segments removed) per §5.2.4.
///
/// If `base` is not an absolute hierarchical URL (e.g. a bare relative test
/// input), resolution degrades to best-effort: a root-relative or relative
/// reference is returned reasonably rather than panicking. **Never panics** on
/// any pair of inputs — this runs on untrusted manifest content.
pub fn resolve_url(base: &str, reference: &str) -> String {
    let r = reference.trim();
    if r.is_empty() {
        return base.trim().to_string();
    }

    // Reference has its own scheme -> absolute, return as-is (normalize path).
    if let Some((scheme, rest)) = split_scheme(r) {
        // rest begins after "scheme:". For a hierarchical URL it is "//auth/path".
        if let Some(after_slashes) = rest.strip_prefix("//") {
            let (auth, path, tail) = split_authority(after_slashes);
            return assemble(scheme, auth, &normalize_path(path), tail);
        }
        // Opaque scheme (e.g. mailto:) — leave untouched.
        return r.to_string();
    }

    // Decompose the base into (scheme, authority, path, query/fragment).
    let base_parts = decompose(base);

    // Scheme-relative: "//host/path" inherits base scheme.
    if let Some(after) = r.strip_prefix("//") {
        let (auth, path, tail) = split_authority(after);
        return assemble(&base_parts.scheme, auth, &normalize_path(path), tail);
    }

    // Fragment-only.
    if let Some(frag) = r.strip_prefix('#') {
        return assemble_from(
            &base_parts,
            Some(base_parts.path.as_str()),
            None,
            Some(frag),
        );
    }

    // Query-only (and optional fragment).
    if let Some(rest) = r.strip_prefix('?') {
        let (query, frag) = split_fragment(rest);
        return assemble_from(
            &base_parts,
            Some(base_parts.path.as_str()),
            Some(query),
            frag,
        );
    }

    // Path reference (root-relative or relative), possibly with ?query/#frag.
    let (path_part, after_path) = split_path_tail(r);
    let (query, frag) = match after_path {
        Some(t) => {
            if let Some(q) = t.strip_prefix('?') {
                let (qq, ff) = split_fragment(q);
                (Some(qq), ff)
            } else if let Some(f) = t.strip_prefix('#') {
                (None, Some(f))
            } else {
                (None, None)
            }
        }
        None => (None, None),
    };

    let merged = if path_part.starts_with('/') {
        normalize_path(path_part)
    } else {
        normalize_path(&merge_path(&base_parts.path, path_part))
    };

    assemble_from(&base_parts, Some(&merged), query, frag)
}

/// The security origin (`scheme://authority`) of an absolute URL, or `""` if the
/// URL has no scheme+authority (e.g. a relative reference). Lower-cased scheme;
/// no path/query/fragment. Never panics.
pub fn origin_of(url: &str) -> String {
    let parts = decompose(url);
    if parts.scheme.is_empty() || parts.authority.is_none() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&parts.scheme);
    out.push_str("://");
    if let Some(a) = parts.authority {
        out.push_str(&a);
    }
    out
}

/// A stable install id: the origin + path of an absolute URL (no query/fragment).
/// Falls back to the trimmed URL when no origin is determinable. Never panics.
fn app_id_of(url: &str) -> String {
    let parts = decompose(url);
    if parts.scheme.is_empty() || parts.authority.is_none() {
        return url.trim().to_string();
    }
    let mut out = origin_of(url);
    out.push_str(&parts.path);
    out
}

/// The decomposed pieces of a URL we care about.
struct UrlParts {
    scheme: String,
    /// `Some` for `scheme://authority/...`; `None` for path-only inputs.
    authority: Option<String>,
    path: String,
}

/// Break a URL into scheme/authority/path. Tolerant of relative inputs (empty
/// scheme, no authority). Query/fragment are folded into `path` only when the
/// caller does not need them split (decompose drops them for origin/merge use).
fn decompose(url: &str) -> UrlParts {
    let u = url.trim();
    // Strip any query/fragment for the structural decomposition.
    let core = {
        let end = u.find(['?', '#']).unwrap_or(u.len());
        &u[..end]
    };

    if let Some((scheme, rest)) = split_scheme(core) {
        if let Some(after) = rest.strip_prefix("//") {
            let (auth, path, _tail) = split_authority(after);
            return UrlParts {
                scheme: scheme.to_ascii_lowercase(),
                authority: Some(auth.to_string()),
                path: if path.is_empty() {
                    String::from("/")
                } else {
                    path.to_string()
                },
            };
        }
        // Opaque (e.g. mailto:) — treat the remainder as the path.
        return UrlParts {
            scheme: scheme.to_ascii_lowercase(),
            authority: None,
            path: rest.to_string(),
        };
    }

    if let Some(after) = core.strip_prefix("//") {
        let (auth, path, _tail) = split_authority(after);
        return UrlParts {
            scheme: String::new(),
            authority: Some(auth.to_string()),
            path: if path.is_empty() {
                String::from("/")
            } else {
                path.to_string()
            },
        };
    }

    UrlParts {
        scheme: String::new(),
        authority: None,
        path: core.to_string(),
    }
}

/// Split `"scheme:rest"` into `(scheme, rest)` when a valid scheme prefix is
/// present. `rest` includes everything after the `:`. `None` for relative refs.
fn split_scheme(s: &str) -> Option<(&str, &str)> {
    let idx = find_scheme_colon(s)?;
    Some((&s[..idx], &s[idx + 1..]))
}

/// Given the text right after `"//"`, split off the authority from the path.
/// Returns `(authority, path, tail)` where `tail` is any `?query`/`#fragment`
/// that followed the path. The path begins at the first `/` (and includes it),
/// or is empty if the authority runs to a `?`/`#`/end.
fn split_authority(after: &str) -> (&str, &str, Option<&str>) {
    // Authority ends at the first of '/', '?', '#'.
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..end];
    let remainder = &after[end..];
    // Now split the remainder into path and ?/# tail.
    let (path, tail) = split_path_tail(remainder);
    (authority, path, tail)
}

/// Split a string into its path component and any `?query`/`#fragment` tail.
/// The path is everything up to the first `?` or `#`.
fn split_path_tail(s: &str) -> (&str, Option<&str>) {
    match s.find(['?', '#']) {
        Some(idx) => (&s[..idx], Some(&s[idx..])),
        None => (s, None),
    }
}

/// Split a post-`?` string into `(query, Some(fragment))` at the first `#`.
fn split_fragment(s: &str) -> (&str, Option<&str>) {
    match s.find('#') {
        Some(idx) => (&s[..idx], Some(&s[idx + 1..])),
        None => (s, None),
    }
}

/// Merge a relative path onto a base path per RFC 3986 §5.2.3: drop the base's
/// last segment (the filename) and append the relative path.
fn merge_path(base_path: &str, rel: &str) -> String {
    let dir = match base_path.rfind('/') {
        Some(idx) => &base_path[..=idx],
        None => "/",
    };
    let mut out = String::from(dir);
    out.push_str(rel);
    out
}

/// Remove `.` and `..` segments from a path per RFC 3986 §5.2.4. Preserves a
/// leading `/` and a trailing `/`. Never panics; `..` past the root is dropped.
fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        return String::from("/");
    }
    let leading_slash = path.starts_with('/');
    let trailing_slash = path.ends_with('/') && path.len() > 1;

    let mut segs: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {} // skip empty (from //) and current-dir
            ".." => {
                segs.pop();
            }
            other => segs.push(other),
        }
    }

    let mut out = String::new();
    if leading_slash {
        out.push('/');
    }
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(seg);
    }
    if trailing_slash && !out.ends_with('/') {
        out.push('/');
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// Assemble `scheme://authority<path><tail>` (tail = the raw `?…`/`#…`).
fn assemble(scheme: &str, authority: &str, path: &str, tail: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&scheme.to_ascii_lowercase());
    out.push_str("://");
    out.push_str(authority);
    if !path.starts_with('/') && !path.is_empty() {
        out.push('/');
    }
    out.push_str(path);
    if let Some(t) = tail {
        out.push_str(t);
    }
    out
}

/// Assemble a resolved URL from `base`'s scheme+authority and explicit
/// path/query/fragment components.
fn assemble_from(
    base: &UrlParts,
    path: Option<&str>,
    query: Option<&str>,
    fragment: Option<&str>,
) -> String {
    let mut out = String::new();
    if !base.scheme.is_empty() {
        out.push_str(&base.scheme);
        out.push(':');
    }
    if let Some(auth) = &base.authority {
        out.push_str("//");
        out.push_str(auth);
    }
    if let Some(p) = path {
        if base.authority.is_some() && !p.starts_with('/') && !p.is_empty() {
            out.push('/');
        }
        out.push_str(p);
    }
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    out
}

// ── Host KATs (the FAIL-able proof: `cargo test -p rae_pwa`) ─────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A full, real-world-shaped manifest (the kind a production PWA ships).
    const FULL_MANIFEST: &str = r##"{
        "name": "Rae Mail",
        "short_name": "Mail",
        "start_url": "/app/index.html",
        "scope": "/app/",
        "display": "standalone",
        "theme_color": "#4285f4",
        "background_color": "#ffffff",
        "description": "Fast, native-feeling web mail.",
        "lang": "en-US",
        "dir": "ltr",
        "orientation": "portrait",
        "icons": [
            { "src": "/icons/16.png",  "sizes": "16x16",   "type": "image/png", "purpose": "any" },
            { "src": "/icons/192.png", "sizes": "192x192", "type": "image/png" },
            { "src": "/icons/512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" },
            { "src": "/icons/scalable.svg", "sizes": "any", "type": "image/svg+xml" }
        ]
    }"##;

    #[test]
    fn full_manifest_every_field_exact() {
        let m = parse_manifest(FULL_MANIFEST).expect("full manifest must parse");
        assert_eq!(m.name, "Rae Mail");
        assert_eq!(m.short_name, "Mail");
        assert_eq!(m.start_url, "/app/index.html");
        assert_eq!(m.scope, "/app/");
        assert_eq!(m.display, DisplayMode::Standalone);
        // FAIL-guard: if the display-enum mapping breaks, Standalone != Browser
        // here flips and this assert fails.
        assert_ne!(m.display, DisplayMode::Browser);
        // theme_color "#4285f4" -> opaque ARGB.
        assert_eq!(m.theme_color, 0xFF42_85F4);
        assert_eq!(m.background_color, 0xFFFF_FFFF);
        assert_eq!(m.description, "Fast, native-feeling web mail.");
        assert_eq!(m.lang, "en-US");
        assert_eq!(m.dir, "ltr");
        assert_eq!(m.orientation, "portrait");

        assert_eq!(m.icons.len(), 4);
        // A specific icon's src + sizes, exact.
        assert_eq!(m.icons[1].src, "/icons/192.png");
        assert_eq!(m.icons[1].sizes, "192x192");
        assert_eq!(m.icons[2].purpose, "maskable");
        // FAIL-guard: a wrong parse (e.g. fields shifted) trips this.
        assert_ne!(m.icons[1].src, "/icons/16.png");

        assert_eq!(m.display_name(), "Mail");
    }

    #[test]
    fn best_icon_picks_right_size() {
        let m = parse_manifest(FULL_MANIFEST).unwrap();
        // Target 192 -> exact 192 icon (smallest >= target).
        assert_eq!(m.best_icon(192).unwrap().src, "/icons/192.png");
        // Target 200 -> next size up is 512 (scale down, never up).
        assert_eq!(m.best_icon(200).unwrap().src, "/icons/512.png");
        // Target 10 -> smallest >= 10 is the 16px icon.
        assert_eq!(m.best_icon(10).unwrap().src, "/icons/16.png");
        // Target 1024 (bigger than every fixed icon) -> largest fixed = 512.
        assert_eq!(m.best_icon(1024).unwrap().src, "/icons/512.png");
        // FAIL-guard: best_icon(192) must NOT return the 16px icon.
        assert_ne!(m.best_icon(192).unwrap().src, "/icons/16.png");
    }

    #[test]
    fn best_icon_handles_any_and_missing_sizes() {
        // Only a scalable "any" icon and one with no sizes at all.
        let src = r#"{
            "name": "Scal",
            "icons": [
                { "src": "/a.svg", "sizes": "any" },
                { "src": "/nosize.png" }
            ]
        }"#;
        let m = parse_manifest(src).unwrap();
        // No fixed-size icon matches; the scalable one is chosen over the
        // size-less raster.
        assert_eq!(m.best_icon(128).unwrap().src, "/a.svg");

        // A manifest with zero icons -> None (and never panics).
        let m2 = parse_manifest(r#"{"name":"NoIcons"}"#).unwrap();
        assert!(m2.best_icon(64).is_none());
    }

    #[test]
    fn color_parse_forms_and_fallback() {
        // 3-digit expands each nibble (f -> ff).
        assert_eq!(parse_css_color("#fff"), 0xFFFF_FFFF);
        assert_eq!(parse_css_color("#f00"), 0xFFFF_0000);
        // 6-digit opaque.
        assert_eq!(parse_css_color("#ffffff"), 0xFFFF_FFFF);
        assert_eq!(parse_css_color("#aabbcc"), 0xFFAA_BBCC);
        // 8-digit carries alpha (aa = 0xAA).
        assert_eq!(parse_css_color("#aabbccdd"), 0xDDAA_BBCC);
        // 4-digit rgba.
        assert_eq!(parse_css_color("#f008"), 0x88FF_0000);
        // No leading '#'.
        assert_eq!(parse_css_color("00ff00"), 0xFF00_FF00);
        // Whitespace tolerated.
        assert_eq!(parse_css_color("  #123456  "), 0xFF12_3456);

        // Named / functional / garbage / empty -> defined fallback, no panic.
        assert_eq!(parse_css_color("rebeccapurple"), COLOR_NONE);
        assert_eq!(parse_css_color("rgb(1,2,3)"), COLOR_NONE);
        assert_eq!(parse_css_color("#xyz"), COLOR_NONE);
        assert_eq!(parse_css_color("#12"), COLOR_NONE);
        assert_eq!(parse_css_color(""), COLOR_NONE);
        assert_eq!(parse_css_color("#"), COLOR_NONE);

        // FAIL-guard: if the hex parser is broken, #f00 won't equal opaque red.
        assert_ne!(parse_css_color("#f00"), parse_css_color("#00f"));
    }

    #[test]
    fn display_mode_default_and_each_value() {
        // Missing display -> Browser (spec default).
        let m = parse_manifest(r#"{"name":"D"}"#).unwrap();
        assert_eq!(m.display, DisplayMode::Browser);

        for (s, want) in [
            ("fullscreen", DisplayMode::Fullscreen),
            ("standalone", DisplayMode::Standalone),
            ("minimal-ui", DisplayMode::MinimalUi),
            ("browser", DisplayMode::Browser),
            ("STANDALONE", DisplayMode::Standalone), // case-insensitive
            ("nonsense", DisplayMode::Browser),      // unknown -> default
        ] {
            assert_eq!(DisplayMode::parse(s), want, "display {:?}", s);
        }
        // FAIL-guard: if the mapping is broken, "fullscreen" would not map to
        // Fullscreen.
        assert_ne!(DisplayMode::parse("fullscreen"), DisplayMode::Browser);
    }

    #[test]
    fn identity_rules() {
        // name alone is enough.
        assert!(parse_manifest(r#"{"name":"OnlyName"}"#).is_ok());
        // short_name alone is enough; display_name falls back to it.
        let m = parse_manifest(r#"{"short_name":"SN"}"#).unwrap();
        assert_eq!(m.display_name(), "SN");
        // neither -> NoIdentity.
        assert_eq!(
            parse_manifest(r#"{"start_url":"/"}"#),
            Err(ManifestError::NoIdentity)
        );
        // present-but-empty strings still count as no identity.
        assert_eq!(
            parse_manifest(r#"{"name":"","short_name":""}"#),
            Err(ManifestError::NoIdentity)
        );
    }

    #[test]
    fn effective_scope_derivation() {
        let m = parse_manifest(r#"{"name":"A","start_url":"/app/index.html"}"#).unwrap();
        // No explicit scope -> directory of start_url.
        assert_eq!(m.effective_scope(), "/app/");

        let m2 = parse_manifest(r#"{"name":"A","start_url":"/app/x","scope":"/app/"}"#).unwrap();
        assert_eq!(m2.effective_scope(), "/app/");

        // start_url with no slash -> "/".
        let m3 = parse_manifest(r#"{"name":"A","start_url":"index"}"#).unwrap();
        assert_eq!(m3.effective_scope(), "/");
    }

    #[test]
    fn same_origin_ish_basic() {
        assert!(WebAppManifest::is_same_origin_ish("/app/"));
        assert!(WebAppManifest::is_same_origin_ish("index.html"));
        assert!(WebAppManifest::is_same_origin_ish("?q=1"));
        assert!(WebAppManifest::is_same_origin_ish("#frag"));
        // Cross-origin-capable forms rejected (no panic).
        assert!(!WebAppManifest::is_same_origin_ish("https://evil.example/"));
        assert!(!WebAppManifest::is_same_origin_ish("//evil.example/"));
        assert!(!WebAppManifest::is_same_origin_ish("   "));
        assert!(!WebAppManifest::is_same_origin_ish(""));
    }

    #[test]
    fn to_app_entry_shape() {
        let m = parse_manifest(FULL_MANIFEST).unwrap();
        let entry = m.to_app_entry(192);
        assert_eq!(entry.name, "Mail");
        assert_eq!(entry.start_url, "/app/index.html");
        assert_eq!(entry.display, DisplayMode::Standalone);
        assert_eq!(entry.theme_color, 0xFF42_85F4);
        assert_eq!(entry.icon_src.as_deref(), Some("/icons/192.png"));

        // A manifest with no icons -> icon_src None (installer synthesizes one).
        let m2 = parse_manifest(r#"{"name":"NoIcon"}"#).unwrap();
        assert_eq!(m2.to_app_entry(192).icon_src, None);
    }

    #[test]
    fn malformed_battery_all_err_zero_panics() {
        // (input, expected error) — every one must Err, none may panic.
        let not_json = "this is not json";
        assert_eq!(parse_manifest(not_json), Err(ManifestError::BadJson));
        assert_eq!(parse_manifest(""), Err(ManifestError::BadJson)); // empty -> json Empty -> BadJson
        assert_eq!(parse_manifest("   "), Err(ManifestError::BadJson));
        assert_eq!(parse_manifest("{"), Err(ManifestError::BadJson)); // unterminated

        // Valid JSON, wrong top-level type.
        assert_eq!(parse_manifest("[1,2,3]"), Err(ManifestError::NotAnObject));
        assert_eq!(parse_manifest("42"), Err(ManifestError::NotAnObject));
        assert_eq!(parse_manifest("\"str\""), Err(ManifestError::NotAnObject));
        assert_eq!(parse_manifest("true"), Err(ManifestError::NotAnObject));
        assert_eq!(parse_manifest("null"), Err(ManifestError::NotAnObject));

        // Object but no identity.
        assert_eq!(parse_manifest("{}"), Err(ManifestError::NoIdentity));

        // icons present but not an array.
        assert_eq!(
            parse_manifest(r#"{"name":"X","icons":"oops"}"#),
            Err(ManifestError::BadIconsType)
        );
        assert_eq!(
            parse_manifest(r#"{"name":"X","icons":{"src":"/a"}}"#),
            Err(ManifestError::BadIconsType)
        );
    }

    #[test]
    fn hostile_fields_degrade_not_panic() {
        // Wrong-typed optional fields are ignored (defaulted), never panic.
        let src = r#"{
            "name": "Robust",
            "start_url": 12345,
            "display": 7,
            "theme_color": ["not","a","string"],
            "icons": [
                "junk-string-not-object",
                { "no_src": true },
                { "src": "" },
                { "src": "/ok.png", "sizes": "64x64" }
            ]
        }"#;
        let m = parse_manifest(src).unwrap();
        assert_eq!(m.name, "Robust");
        // start_url wasn't a string -> default empty.
        assert_eq!(m.start_url, "");
        // display wasn't a string -> default Browser.
        assert_eq!(m.display, DisplayMode::Browser);
        // theme_color wasn't a string -> COLOR_NONE.
        assert_eq!(m.theme_color, COLOR_NONE);
        // Only the one well-formed icon survives the filter.
        assert_eq!(m.icons.len(), 1);
        assert_eq!(m.icons[0].src, "/ok.png");
    }

    #[test]
    fn icon_size_parsing_edges() {
        // Multi-size token: "16x16 32x32" -> max 32.
        let icon = Icon {
            src: "/a.png".to_string(),
            sizes: "16x16 32x32".to_string(),
            mime_type: String::new(),
            purpose: String::new(),
        };
        assert_eq!(icon.max_dimension(), Some(32));
        assert!(!icon.is_scalable());

        // Garbage sizes -> None, never panic.
        let bad = Icon {
            src: "/b.png".to_string(),
            sizes: "huge x nope".to_string(),
            mime_type: String::new(),
            purpose: String::new(),
        };
        assert_eq!(bad.max_dimension(), None);

        // "any" -> scalable, no fixed dimension.
        let any = Icon {
            src: "/c.svg".to_string(),
            sizes: "any".to_string(),
            mime_type: String::new(),
            purpose: String::new(),
        };
        assert_eq!(any.max_dimension(), None);
        assert!(any.is_scalable());
    }

    #[test]
    fn icons_order_and_count_with_dupes() {
        // FAIL-guard on count: exactly 4 valid icons in FULL_MANIFEST.
        let m = parse_manifest(FULL_MANIFEST).unwrap();
        let srcs: vec::Vec<&str> = m.icons.iter().map(|i| i.src.as_str()).collect();
        assert_eq!(
            srcs,
            vec![
                "/icons/16.png",
                "/icons/192.png",
                "/icons/512.png",
                "/icons/scalable.svg"
            ]
        );
    }

    // ── URL resolution KATs ──────────────────────────────────────────────────

    #[test]
    fn resolve_root_relative_and_relative() {
        let base = "https://app.example/manifest.json";
        // Root-relative replaces the path, keeps origin.
        assert_eq!(
            resolve_url(base, "/icons/192.png"),
            "https://app.example/icons/192.png"
        );
        // Relative merges onto the manifest's directory (root here).
        assert_eq!(
            resolve_url(base, "icons/192.png"),
            "https://app.example/icons/192.png"
        );
        // FAIL-guard: a relative ref must NOT keep the manifest filename.
        assert_ne!(
            resolve_url(base, "icons/192.png"),
            "https://app.example/manifest.json"
        );

        // Manifest in a subdirectory: relative merges onto THAT directory.
        let sub = "https://app.example/pwa/manifest.webmanifest";
        assert_eq!(
            resolve_url(sub, "icons/a.png"),
            "https://app.example/pwa/icons/a.png"
        );
        assert_eq!(
            resolve_url(sub, "/root.png"),
            "https://app.example/root.png"
        );
    }

    #[test]
    fn resolve_dot_segments() {
        let base = "https://app.example/a/b/manifest.json";
        // ".." climbs one directory from /a/b/.
        assert_eq!(
            resolve_url(base, "../up.png"),
            "https://app.example/a/up.png"
        );
        // "./" current dir.
        assert_eq!(
            resolve_url(base, "./here.png"),
            "https://app.example/a/b/here.png"
        );
        // Multiple "..", and one past root is clamped (never panics / underflows).
        assert_eq!(
            resolve_url(base, "../../../../etc"),
            "https://app.example/etc"
        );
    }

    #[test]
    fn resolve_absolute_scheme_relative_and_components() {
        let base = "https://app.example/app/manifest.json";
        // Absolute reference returned as-is.
        assert_eq!(
            resolve_url(base, "https://cdn.other/x.png"),
            "https://cdn.other/x.png"
        );
        // Scheme-relative inherits the base scheme.
        assert_eq!(
            resolve_url(base, "//cdn.other/x.png"),
            "https://cdn.other/x.png"
        );
        // Query-only replaces the query, keeps path.
        assert_eq!(
            resolve_url(base, "?v=2"),
            "https://app.example/app/manifest.json?v=2"
        );
        // Fragment-only.
        assert_eq!(
            resolve_url(base, "#top"),
            "https://app.example/app/manifest.json#top"
        );
        // Path + query + fragment together.
        assert_eq!(
            resolve_url(base, "/go?x=1#y"),
            "https://app.example/go?x=1#y"
        );
    }

    #[test]
    fn resolve_never_panics_on_garbage() {
        // None of these may panic; values are best-effort.
        let cases = [
            ("", ""),
            ("not a url", "../../.."),
            ("https://", "/x"),
            ("mailto:a@b.com", "x"),
            ("https://h/", ""),
            ("///", "//"),
            (":::", "::::"),
        ];
        for (b, r) in cases {
            let _ = resolve_url(b, r); // must return, not panic
        }
        // Empty reference -> base unchanged.
        assert_eq!(resolve_url("https://h/p", ""), "https://h/p");
    }

    #[test]
    fn origin_extraction() {
        assert_eq!(
            origin_of("https://app.example/a/b?c#d"),
            "https://app.example"
        );
        assert_eq!(origin_of("http://h:8080/x"), "http://h:8080");
        // Scheme is lower-cased.
        assert_eq!(origin_of("HTTPS://Host/x"), "https://Host");
        // No determinable origin -> empty (relative input).
        assert_eq!(origin_of("/just/a/path"), "");
        assert_eq!(origin_of("relative"), "");
    }

    // ── Purpose-aware icon selection KATs ────────────────────────────────────

    #[test]
    fn icon_purpose_matching() {
        let any = Icon {
            src: "/a.png".to_string(),
            sizes: "192x192".to_string(),
            mime_type: String::new(),
            purpose: String::new(), // empty -> defaults to Any
        };
        assert!(any.has_purpose(IconPurpose::Any));
        assert!(!any.has_purpose(IconPurpose::Maskable));

        let mask = Icon {
            src: "/m.png".to_string(),
            sizes: "512x512".to_string(),
            mime_type: String::new(),
            purpose: "maskable".to_string(),
        };
        assert!(mask.has_purpose(IconPurpose::Maskable));
        assert!(!mask.has_purpose(IconPurpose::Any));

        // Multiple purposes in one icon.
        let both = Icon {
            src: "/b.png".to_string(),
            sizes: "192x192".to_string(),
            mime_type: String::new(),
            purpose: "any maskable".to_string(),
        };
        assert!(both.has_purpose(IconPurpose::Any));
        assert!(both.has_purpose(IconPurpose::Maskable));
        assert!(!both.has_purpose(IconPurpose::Monochrome));

        // Unknown keyword parses to Any (never silently dropped).
        assert_eq!(IconPurpose::parse("badge"), IconPurpose::Any);
    }

    #[test]
    fn best_icon_for_purpose_prefers_then_falls_back() {
        let src = r#"{
            "name": "P",
            "icons": [
                { "src": "/any-192.png",  "sizes": "192x192", "purpose": "any" },
                { "src": "/mask-512.png", "sizes": "512x512", "purpose": "maskable" },
                { "src": "/mask-96.png",  "sizes": "96x96",   "purpose": "maskable" }
            ]
        }"#;
        let m = parse_manifest(src).unwrap();
        // Maskable @128 -> smallest maskable >= 128 is the 512 (96 is < 128).
        assert_eq!(
            m.best_icon_for_purpose(128, IconPurpose::Maskable)
                .unwrap()
                .src,
            "/mask-512.png"
        );
        // Maskable @64 -> smallest maskable >= 64 is the 96.
        assert_eq!(
            m.best_icon_for_purpose(64, IconPurpose::Maskable)
                .unwrap()
                .src,
            "/mask-96.png"
        );
        // Monochrome: none match -> fall back to best_icon overall (any-192 @192).
        assert_eq!(
            m.best_icon_for_purpose(192, IconPurpose::Monochrome)
                .unwrap()
                .src,
            "/any-192.png"
        );
        // FAIL-guard: maskable@128 must not return an "any"-purpose icon.
        assert_ne!(
            m.best_icon_for_purpose(128, IconPurpose::Maskable)
                .unwrap()
                .src,
            "/any-192.png"
        );
    }

    // ── Install descriptor KATs (the end-to-end "install this web app") ──────

    /// A Twitter/Maps-shaped real-world manifest fetched from a sub-path.
    const REALWORLD: &str = r##"{
        "name": "RaeMaps",
        "short_name": "Maps",
        "start_url": "./?source=pwa",
        "scope": "/maps/",
        "display": "standalone",
        "theme_color": "#1a73e8",
        "background_color": "#ffffff",
        "icons": [
            { "src": "icons/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any" },
            { "src": "icons/maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
        ]
    }"##;

    #[test]
    fn install_resolves_everything_absolute() {
        let manifest_url = "https://maps.example/maps/manifest.json";
        let d = parse_manifest(REALWORLD)
            .unwrap()
            .install(manifest_url, 192);

        assert_eq!(d.name, "Maps");
        assert_eq!(d.origin, "https://maps.example");
        // start_url "./?source=pwa" merged onto /maps/ directory + query.
        assert_eq!(d.start_url, "https://maps.example/maps/?source=pwa");
        // scope "/maps/" resolved (already root-relative).
        assert_eq!(d.scope, "https://maps.example/maps/");
        assert_eq!(d.display, DisplayMode::Standalone);
        assert_eq!(d.theme_color, 0xFF1A_73E8);
        assert_eq!(d.background_color, 0xFFFF_FFFF);
        // Maskable preference picks the maskable icon, resolved absolute.
        assert_eq!(
            d.icon_url.as_deref(),
            Some("https://maps.example/maps/icons/maskable-512.png")
        );
        // id = origin + path (no query).
        assert_eq!(d.id, "https://maps.example/maps/");

        // FAIL-guard: a broken resolver would leave the icon relative.
        assert_ne!(d.icon_url.as_deref(), Some("icons/maskable-512.png"));
    }

    #[test]
    fn install_defaults_start_url_to_manifest_url() {
        // No start_url -> spec default is the manifest URL itself.
        let src = r#"{"name":"Defaulty","display":"standalone"}"#;
        let d = parse_manifest(src)
            .unwrap()
            .install("https://x.example/sub/app.webmanifest", 192);
        assert_eq!(d.start_url, "https://x.example/sub/app.webmanifest");
        // No explicit scope -> directory of start_url.
        assert_eq!(d.scope, "https://x.example/sub/");
        assert_eq!(d.icon_url, None);
        assert_eq!(d.origin, "https://x.example");
    }

    #[test]
    fn install_never_panics_on_hostile_inputs() {
        // Garbage manifest URL + manifest with weird fields: must produce a
        // descriptor, never panic.
        let src = r#"{
            "name": "Hostile",
            "start_url": "//evil.example/steal",
            "icons": [ { "src": "../../../../../../etc/passwd", "sizes": "1x1" } ]
        }"#;
        let m = parse_manifest(src).unwrap();
        for base in ["", "not a url", "https://", ":::", "https://h/p/m.json"] {
            let d = m.install(base, 64);
            // The struct is always produced with a name.
            assert_eq!(d.name, "Hostile");
            // dot-segment climbing is clamped, not panicking; we just assert it
            // returned a string (best-effort) for each base.
            let _ = d.icon_url;
            let _ = d.start_url;
            let _ = d.scope;
            let _ = d.origin;
        }
        // With a real base, the scheme-relative start_url inherits https (which
        // a same-origin policy in the security layer can then reject — but we
        // must RESOLVE it, not panic).
        let d = m.install("https://app.example/m.json", 64);
        assert_eq!(d.start_url, "https://evil.example/steal");
        // The icon's dot-segments are clamped to the origin root.
        assert_eq!(
            d.icon_url.as_deref(),
            Some("https://app.example/etc/passwd")
        );
    }
}
