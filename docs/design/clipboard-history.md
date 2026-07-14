# Design Spec: Clipboard History (Win+V-class history + pin)

> *"Built for people who care about how things feel."* — LEGACY_GAMING_CONCEPT.md
>
> A keystroke opens a glass panel of everything you've recently copied — text,
> images, links — pin the keepers, paste the one you want. It must clear:
> **Windows 11's Win+V clipboard history + pin**, with macOS-grade materiality and
> Linux-grade ownership (clear/auto-clear under your control, no cloud by default).

**All tokens below are defined in [`design-language.md`](./design-language.md)**
and live in `rae_tokens` (ADR 0003). This spec only *assigns* them. No new magic
numbers; surface-specific layout dimensions are local constants from `space.*`.

---

## Concept promise + bar to clear

> "The user owns the machine — no forced telemetry." — LEGACY_GAMING_CONCEPT.md
> (§The user owns the machine) → clipboard history is **local-only by default**,
> with explicit auto-clear, no opt-out cloud sync.

- **Bar to clear:** Windows 11 **Win+V** — a flyout of recent clips, pin to keep
  across sessions, image thumbnails, per-entry delete, clear-all, and paste-on-
  select. AthenaOS matches the model and adds the glass material + accent cohesion
  + a privacy posture that's owned, not assumed.

---

## Already built (delta only — verify-before-spec)

Grounded in code. There are **two layers** that already exist; this surface is the
*panel* over the rich one plus a syscall to make it session-wide.

| Piece | Where | Today | This spec adds |
|---|---|---|---|
| Rich clipboard manager | `components/raeshell/src/clipboard.rs::ClipboardManager` | LIVE data model, **unwired** (`allow(unused)`): multi-format (`Text/RichText/Image/Files/Url/Color/Custom`), **history** (`Vec<ClipboardEntry>`), **pin/unpin**, `delete_entry`, `clear_history` (keeps pinned), `search`, `paste_at(index)`, per-entry `preview`, size/total caps + pinned-safe eviction (`enforce_limits`) | a glass panel that renders `history()` rows + a syscall so it's session-wide |
| Kernel clipboard (current) | `kernel/src/clipboard.rs` (syscalls 107 GET / 108 SET) | LIVE but **single 64 KiB UTF-8 buffer, no history** | the architect adds a **history-aware** kernel surface (or promotes the `ClipboardManager` to a session service) so the panel's history survives across apps — see Handoff |
| Entry preview | `ClipboardEntry.preview` (≤120 chars, format-aware: "PNG image 1920x1080", "3 files", "#FF8800") | LIVE | rendered as the row's preview line |
| Pinned-safe eviction | `enforce_limits` (evicts oldest **un**pinned first; `max_history`, `max_total_size`) | LIVE | the "Recent capped, Pinned never evicted" UX |
| Glass / shadow | `compositor::set_surface_blur`, `SurfaceEffect::DropShadow` | LIVE | `material.glass` + `elev.3` |

**This is a wire-up + one ABI addition, not a rebuild.** The history, pin model,
multi-format entries, preview generation, search, and eviction policy already
exist in `ClipboardManager`. Missing: (a) a session-wide history carrier (the
kernel side is a single buffer today), (b) the invocation hotkey, (c) the panel.

---

## Prior art distilled (current systems, 2024–2025)

- **Windows 11 (Win+V):** a flyout anchored near the caret/cursor; newest-first
  list; **Pinned** items float to a section that survives reboots; text + image +
  HTML entries; each row has a `…` menu (pin, delete); a top bar with "Clear all"
  and a sync toggle; **clicking a row pastes it into the focused field** and
  closes. **Take:** the whole interaction model — pin section, paste-on-select,
  clear-all, image thumbnails. **Avoid:** the cloud-sync default (AthenaOS = local
  by default, sync is an explicit opt-in); the cramped row height.
- **macOS (no native history; Maccy/Paste/Raycast fill it):** Maccy = a compact
  dropdown, fuzzy-searchable, keyboard-driven, pins; Paste = a horizontal full-
  width "shelf" of large cards. **Take:** keyboard-first search + number-key quick
  paste (Maccy); the legible large-preview card (Paste). **Avoid:** the full-
  screen shelf (too heavy for a glance-and-paste flow — ours is a flyout).
- **GNOME (Clipboard Indicator) / KDE (Klipper):** tray-attached history, private-
  mode toggle, configurable history length, "clear on lock". **Take:** the
  privacy toggles (incognito/clear-on-lock) and configurable length. **Avoid:**
  the tray-menu cramped presentation.

**AthenaOS synthesis:** Windows' **pin-section + paste-on-select + thumbnails**
model, Maccy's **keyboard-first search + number-key paste**, KDE's **privacy
controls (incognito, clear-on-lock, auto-clear)** — rendered as one glass flyout
that reads the live accent.

---

## AthenaOS design tokens this surface uses

Pulled verbatim from `design-language.md` / `rae_tokens`. No new magic numbers.

- **spacing:** `space.2` (intra-row gap, badge inset), `space.3` (row inset, row
  vertical pad), `space.4` (panel padding, section gap), `space.1` (row-to-row gap).
- **radius:** `radius.lg` (the panel), `radius.md` (image-thumbnail cards),
  `radius.sm` (the search field), `radius.xs` (text rows, format badges, the
  per-row action buttons).
- **elevation:** `elev.3` (the floating panel — transient flyout), `elev.2`
  (a row's hover lift, optional), `elev.focus` (selected-row glow).
- **type:** `type.subtitle` (panel section headers "Pinned" / "Recent"),
  `type.body` (text-entry preview), `type.caption` (source app + timestamp +
  format badge + the keyboard-hint footer).
- **accent model:** seed `ThemeAbi.accent_argb` → `derive_accent`. Pinned indicator
  + selected-row fill use `accent.subtle`/`accent.base`; the pin glyph when active
  is `accent.text`. **No private `const ACCENT`.**
- **material:** `material.glass` (live blur — small transient flyout).
- **motion:** `motion.fast` (panel open/close — smaller than Start, so faster than
  `motion.standard`), `motion.micro` (row hover/selection, pin toggle), `motion.exit`
  (paste-and-dismiss), `motion.instant` (reduced-motion).

---

## 1. Invocation

- **Primary hotkey:** `Super+V` (mirrors Windows `Win+V`; `Super` is the shell's
  modifier per `desktop-shell`).
- **Behavior:** opens the panel **anchored to the focused text caret if one
  exists** (Windows behavior — paste lands where you're typing), else floating
  **centered-bottom** above the taskbar. Pressing `Super+V` again closes it.
- The panel is a **single global instance**. It opens with the search field
  focused-but-empty and the **first Recent entry selected** so a blind
  `Super+V → Enter` pastes the last copy (the 90% case).

---

## 2. Panel surface (anchored flyout)

**Bar to clear:** the Win+V flyout that feels native, not a popup window.

### Geometry
- **Width:** **360px** (local constant = `space.6` × 11.25; matches the quick-
  settings flyout width in `desktop-shell` for family consistency). 
- **Max height:** **480px** (clamped to screen − taskbar − `space.4`); the list
  scrolls beyond that. 
- **Anchoring:** if caret-anchored, the panel's top-left sits `space.2` below-right
  of the caret, flipping to stay on-screen; else bottom-centered, `space.4` above
  the taskbar.
- **Material:** `material.glass` — `radius.lg`, blur 16 (`GLASS_BLUR_RADIUS`),
  tint per palette, 1px `stroke.strong` top highlight + `stroke.subtle` border.
  Shadow `elev.3`.
- **Content padding:** `space.4`.

### Layout (top → bottom)
1. **Header bar** — a title "Clipboard" (`type.subtitle`, `text.primary`) on the
   left; on the right, two icon buttons (32px targets, `radius.xs` hover):
   **incognito toggle** (privacy, §6) and **Clear all** (`state.danger` on hover).
2. **Search field** (optional, shows when history > 8 entries) — full width minus
   `space.4`, height 32px, `radius.sm`, `bg.elevated`, search glyph + placeholder
   "Search clipboard" (`text.tertiary`, `type.body`). Wired to `ClipboardManager::search`.
3. **Pinned section** — header "Pinned" (`type.subtitle`, `text.secondary`), shown
   only when ≥1 pinned entry. Rows render the pinned subset (`enforce_limits`
   never evicts these).
4. **Recent section** — header "Recent" (`type.subtitle`, `text.secondary`); the
   newest-first un-pinned entries.

---

## 3. Entry rows

Each row renders one `ClipboardEntry`. **Bar to clear:** Win+V's text rows + image
thumbnails + the per-row action affordances.

### Text / link / files / color entry (compact row)
- Height **44px** (≥32px floor). 
- **Format badge** (left, `space.3` inset) — a `radius.xs` chip, `type.caption`,
  `bg.elevated` fill, labeling the `ClipboardFormat`: "TXT" / "HTML" / "URL" /
  "FILES" / a color swatch for `Color`. Drives instant scanning of mixed history.
- **Preview** (`type.body`, `text.primary`) — `entry.preview` (already truncated
  to 120 chars by `generate_preview`); URLs/colors render their canonical form.
- **Meta line** (`type.caption`, `text.tertiary`) — `source_app` (if known) +
  relative timestamp ("2m ago"), right-aligned or as a subrow.
- **Pin glyph** (right, on hover/selection) — outline when unpinned, filled
  `accent.text` when pinned.

### Image entry (thumbnail card)
- Taller row: a **`radius.md` thumbnail card** (up to 320×120px, aspect-preserved)
  showing the actual `Image` pixels, with a `type.caption` badge overlay
  ("PNG 1920×1080" from the preview). This is the Win+V/Paste large-preview
  treatment — images deserve to be *seen*, not labeled.

### Per-row actions (revealed on hover/selection)
A right-aligned cluster of 28px `radius.xs` buttons (≥32px effective hit area with
padding): **Pin/Unpin** (`pin`/`unpin`), **Delete** (`delete_entry`,
`state.danger` on hover; **disabled with a lock glyph if the entry is pinned** —
mirrors `delete_entry`'s pinned guard). Keyboard equivalents in §5.

### Pin vs recent ordering
- **Pinned always render above Recent**, in pin order; **Recent renders newest-
  first** (`history.insert(0, …)` already prepends). Pinned entries are **exempt
  from eviction** (`enforce_limits` evicts oldest un-pinned only) — the spec's
  visual promise ("pin = keep") is already the engine's policy.

### States (per row)
- **default:** transparent over glass.
- **hover:** `bg.elevated` fill @ `radius.xs`, action cluster fades in (`motion.micro`).
- **selected (keyboard):** `accent.subtle` fill + 2px `accent.base` left bar +
  `elev.focus` glow; action cluster visible. Distinct from hover.
- **active (paste fired):** `accent.active` flash → panel dismisses (`motion.exit`).
- **pinned:** a persistent `accent.text` pin glyph + a faint `accent.subtle` left
  edge so pinned rows read as "kept" even when not selected.
- **disabled delete (pinned):** the delete button is `text.tertiary` + lock glyph.
- **dark/light:** glass tint + tokens per palette.
- **reduced-motion:** no fades; selection jumps; panel opacity-only.

---

## 4. Paste-on-select interaction

- **Click a row** or **Enter on the selected row** → the entry's content is set as
  the active clipboard and **pasted into the previously-focused field**
  (`ClipboardManager::paste_at(index)` provides the content; the shell injects the
  paste into the focused app), then the panel closes on `motion.exit`. This is the
  Win+V core loop.
- **Plain copy without paste:** `Ctrl+Enter` (or a modifier-click) just promotes
  the entry to the active clipboard *without* pasting — for when you want to queue
  it. The panel closes.
- Selecting a row bumps its `paste_count` (already tracked) feeding future ordering.

---

## 5. Keyboard + controller navigation map

- `Super+V` — open/close (toggle).
- type — filter (`ClipboardManager::search`).
- `↓` / `↑` — move selection across Pinned then Recent (`motion.micro`).
- `1`–`9` — quick-paste the Nth visible entry (Maccy-style number paste).
- `Enter` — paste selected + close (§4); `Ctrl+Enter` — promote without paste.
- `P` (or the pin button) — toggle pin on the selected entry (`pin`/`unpin`).
- `Delete` / `Backspace` — delete selected (no-op + shake on a pinned entry).
- `Shift+Delete` — Clear all un-pinned (`clear_history`).
- `Esc` — close, no paste.
- **Controller (couch):** D-pad = selection, A = paste, X = pin, Y = delete, B =
  close; rows use the 48px couch floor, `elev.focus` glow reads at 3m.

---

## 6. Privacy / auto-clear behavior (ownership posture)

The Concept's "user owns the machine" makes privacy a *first-class* control, not a
buried setting:

- **Incognito toggle** (header button) — while active, new copies are **not**
  recorded to history (the active clipboard still works for paste). A clear
  `accent.subtle` "Incognito — not saving" strip shows in the panel.
- **Clear-on-lock** — history (un-pinned) is wiped when the session locks
  (default **on**; a Settings toggle). Pinned entries persist (the user chose to
  keep them).
- **Auto-clear timer** — un-pinned entries older than a configurable window
  (default **24h**) are evicted. Pinned never expire.
- **Bounded memory** — `max_history` (count) and `max_total_size` (default 100 MiB
  in `ClipboardManager`) already cap growth with pinned-safe eviction; surface a
  small "N items • X MB" footer (`type.caption`, `text.tertiary`).
- **Local-only by default** — `sync_enabled` is **off**; any future cross-device
  sync is an explicit opt-in (no silent cloud, per Concept).
- **Max entries shown:** the list virtualizes; render the first ~8 then scroll
  (the panel never grows unbounded on screen).

**Flag to raeen-accessibility + AthGuard:** password-manager / secret fields
should be excludable from history (a `Custom` MIME or a "sensitive" hint that the
manager honors by skipping the copy). Coordinate the exclusion mechanism.

---

## 7. Accessibility (in scope from the start)

| Concern | Rule | Owner |
|---|---|---|
| Contrast | preview `type.body` ≥4.5:1; format badge text ≥4.5:1 on its `bg.elevated` chip; image-overlay badge needs a scrim behind it | raeen-accessibility |
| Focus visibility | selected row = `accent.subtle` + 2px `accent.base` bar + `elev.focus`, never color-only | raeen-accessibility |
| Reduced-motion | open/close opacity-only; no row fades | raeen-accessibility |
| Hit targets | 44px rows (pointer) / 48px (couch); per-row action buttons ≥32px effective | raeen-visual-qa |
| Destructive guard | Clear-all and delete confirm intent (the pinned-delete lock is already modeled); destructive affordances use `state.danger` | raeen-accessibility |

---

## 8. Cohesion acceptance

Ships only when:
1. The panel reads the **same `accent.base`** as the shell — Vibe-switch and the
   pinned indicator + selected fill change with the taskbar/Start.
2. Glass material, `radius.lg`, `elev.3` match the quick-settings flyout (same
   transient-glass family, same 360px width).
3. Selected ≠ hover visually; pinned rows are distinct from un-pinned.
4. Dark + light both pass contrast.

---

## Handoff

### Implementers (two-part — the ABI then the panel)
- **opus / raeen-architect** — the **session-wide history syscall**. The kernel
  clipboard (`kernel/src/clipboard.rs`, syscalls 107/108) is a single 64 KiB
  buffer with no history; the panel needs history that survives across apps.
  Decide between (a) promoting `raeshell::ClipboardManager` to a userspace session
  service that the panel and apps talk to, or (b) a small history-aware kernel
  surface (new syscalls for `history_count` / `history_entry(i)` / `pin(i)` /
  `delete(i)` / `clear`). Whichever: it is an **[interface]** change — bump
  `ABI_VERSION`, update `docs/SYSCALL_TABLE.md` in the same commit, batch the
  syscall numbers (no one-off magic numbers). Keep the 64 KiB-per-entry / total
  caps as the kernel-side guard.
- **raeen-shell-apps / raeen-ui** — the **panel**. Wire `ClipboardManager`
  (remove `allow(unused)`) to the §2 glass flyout: `Super+V` hotkey + caret
  anchoring, Pinned/Recent sections rendering `history()`, the §3 row renderer
  (text rows + image thumbnail cards + per-row pin/delete), §4 paste-on-select,
  §6 privacy controls. The entry-row and format-badge widgets consume
  `rae_tokens` (no private palette) and are reusable.
- **raeen-accessibility / AthGuard** (flagged) — sensitive-field exclusion;
  contrast + focus audit.

### FAIL-able boot-log proof line
From a `run_boot_smoketest` that exercises the history model (must print FAIL):

```
[clipboard-history] smoketest: copied=3 pinned=1 evict_unpinned=1 pin_survives_evict=1 paste_at(0)_ok=1 -> PASS
```

(FAIL if a pinned entry is evicted under `enforce_limits`, if `clear_history`
drops a pinned entry, if `paste_at` returns the wrong content, or if the count/
size caps are not enforced.) Plus cohesion:
`[clipboard-history] accent=0x.. == derive_accent(seed).base -> PASS`.

### Visual-QA verification list (raeen-visual-qa)
- QEMU screenshot: panel open with a **Pinned** section (≥1 pinned, accent left
  edge + filled pin glyph) and a **Recent** section, glass blur visible, accent
  cohesive with the taskbar.
- Screenshot: a mixed history showing a text row (TXT badge), a URL row, and an
  **image thumbnail card** rendering real pixels with the overlay badge.
- Screenshot: a row **selected** (accent fill + left bar + glow) with the per-row
  pin/delete cluster visible; and a pinned row showing the **delete button
  disabled with a lock glyph**.
- Screenshot: the **incognito** state strip ("not saving").
- Cohesion: before/after Vibe switch — pin indicator + selection accent change.

### Unblocks (MasterChecklist)
- Phase 8 (AthUI/AthKit): the entry-row + thumbnail-card widgets.
- Phase 14 (AthShell + apps): the clipboard-history surface; activates the dead
  `raeshell::clipboard` manager and gives the kernel clipboard a history.
