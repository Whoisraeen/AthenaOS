# ADR 0003 — Home for the shared design tokens + `derive_accent`

- Status: accepted
- Date: 2026-06-17
- Owner: raeen-lead (autonomous) / raeen-architect to land

## Context
`docs/design/design-language.md` defines the single canonical token set (spacing,
radius, type ramp, motion, color palettes, elevation) and the accent-derivation
rule (`derive_accent(seed) -> AccentRamp`, six derived tokens). It proposes
`raeui::tokens` as the constant home and `theme_engine::derive_accent` (kernel) as
the derivation home.

The cohesion problem to solve: ~30 files each redefine `const ACCENT` + a private
palette. The tokens must be consumed by BOTH:
- the `#![no_std]` kernel — `kernel/src/window_chrome.rs`, `theme_engine.rs`,
  compositor titlebar/taskbar draw; and
- userspace — `raeui`, `components/raeshell` (`desktop.rs`, `notify.rs` is kernel
  actually — mixed), and the bundled apps.

`raeui` cannot be the shared home: it depends on `raegfx`/`raefont` (and optional
`wgpu`/`skia`), so the `#![no_std]` kernel cannot import it. Splitting the tokens
between a userspace copy and a kernel copy just re-creates the duplication we are
removing.

## Options
1. **Constants in `raeui::tokens`, duplicated in the kernel.** Rejected —
   re-creates the per-surface duplication; two sources drift.
2. **Constants in `rae_abi`.** Rejected — `rae_abi` is the FROZEN ABI contract
   (syscall numbers, capability/IPC surfaces, `[interface]`-gated). Design tokens
   are not ABI; churning that crate on every palette tweak is wrong, and it would
   bump `ABI_VERSION` for visual changes.
3. **New `#![no_std]`, zero-dependency crate `rae_tokens`** — the single source of
   truth for the const tokens AND `derive_accent` (pure integer/`libm` math, no
   allocation). Depended on by the kernel, `raeui` (re-exported as
   `pub mod tokens`), `raeshell`, and apps. `theme_engine::derive_accent` becomes
   a thin call into `rae_tokens::derive_accent`. **Chosen.**

## Decision
Create `components/rae_tokens` (`#![no_std]`, no deps beyond optional `libm`).
It holds every token in `design-language.md` as typed constants/`const fn`s plus
`derive_accent(seed: u32) -> AccentRamp`. `raeui` re-exports it as `raeui::tokens`
so the design-language handoff name still resolves. Kernel surfaces and apps
depend on `rae_tokens` directly and migrate off their private `const ACCENT`
incrementally (per-crate slices, merge-safe).

This is NOT an ABI change — `rae_abi` is untouched, no `[interface]` commit
needed. It is a normal additive workspace crate; raeen-architect lands it because
it edits workspace membership and is consumed cross-crate.

## Rationale
Tie-breaker: (1) the Concept's Vibe-Mode "one tap re-skins the whole desktop"
requires a single seed flowing everywhere — one token home is the literal
mechanism; (3) a zero-dep no_std crate is the simplest thing that satisfies both
the kernel and userspace; (4) fully reversible — additive crate, consumers migrate
one at a time, and reverting is inlining the constants back (where they already
lived).

## How to reverse
Delete `components/rae_tokens`, drop the dependency lines, and inline the
constants back into each consumer (their prior state). Mechanical; no data or ABI
migration.
