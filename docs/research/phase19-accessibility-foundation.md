# Phase 19.1 — Accessibility Foundation (design spec)

> **SUPERSEDED (status only) — 2026-06-21.** The *design* below is sound and was implemented
> largely as written, but its **status claims are STALE**. This doc describes the foundation
> tier as "spec -> ready for implementation" / `[ ]`; in reality the kernel a11y tree, the
> cap-gated AT ABI, and most of 19.2/19.3 have since LANDED. **The authoritative current state
> is `docs/research/accessibility-audit-2026-06-21.md`** (verified against live source). Read
> this doc as the *design rationale + seam map* (still accurate), NOT as a status tracker. Do
> NOT trust any `[ ]` / "ready for implementation" / "NEW" / "do not add yourself" line here as
> current — most of it is built. Live status ladder lives in `MasterChecklist.md` and
> `docs/PARITY_MATRIX.md §J`; this doc's `[ ]`s are not resurrected.
>
> What shipped since this spec (verified in source 2026-06-21, file evidence in the audit):
> - `kernel/src/a11y.rs` (1236 lines) — tree built from the live compositor surface list, R10
>   complete (init line, FAIL-able smoketests, `/proc/raeen/a11y`, Concept docstring), wired in
>   `kernel_main`. (This whole spec is built.)
> - Cap-gated AT ABI `SYS_A11Y_SNAPSHOT` (277) + `SYS_A11Y_ACTION` (278), `Cap::Accessibility`
>   READ/WRITE, fail-closed — i.e. the "NEEDS-INTERFACE" block (sec 7) LANDED.
> - 19.2/19.3 followups: magnifier (compositor upscale + focus-follows), color filters,
>   screen-reader announce core + `SpeechSink`/`LogSpeechSink`, contrast math + audit.
> - User-facing on-switches (hotkeys Super+Alt+M/H/C/R + Super+=/- and the Control Center
>   Accessibility tile) AND the high-contrast LIVE forced-colors palette swap
>   (`rae_tokens::active_palette() == HIGH_CONTRAST`) shipped AFTER the audit — proven by
>   `a11y::run_onswitch_smoketest` (boot) + `rae_tokens` `active_palette_swaps_under_high_contrast`
>   (host KAT).

**Status:** SUPERSEDED design spec (built; status above) — kept for design rationale + seam map.
**Implementer:** raeen-accessibility
**Interface owner (must land first):** raeen-architect — see the NEEDS-INTERFACE block (sec 7).
**Scope:** the foundation tier only — the accessibility tree, its /proc/raeen/a11y snapshot,
and a capability-gated read/action API. Screen reader / magnifier / high-contrast (19.2) and
keyboard navigation (19.3) are separate later items; this spec only names them as follow-ups.
(All of these are now BUILT — see the SUPERSEDED banner; this section describes the original plan.)

Concept-doc lines this serves (module docstring quote):
- Security: "Capability-based permissions — apps request capabilities ... OS enforces at the
  syscall layer."
- Security: a system "where you can run untrusted software without fear" — an AT client must
  not read another apps UI tree unprompted.
- Phase 19 mandate: parity with Windows Narrator / macOS VoiceOver (both shipped non-optional).

## 1. Prior art distilled
- AccessKit (Rust, MIT/Apache): Tree = arena of Nodes by NodeId with one root; each Node has
  Role, name, Bounds, actions, value, state flags; updates as TreeUpdate diffs. Roles: Window,
  Button, Label/StaticText, TextInput, CheckBox, Slider, List, MenuItem.
- macOS AXUIElement/NSAccessibility: parent/child element tree; client needs the Accessibility
  TCC permission.
- Windows UI Automation: provider tree of AutomationElement; client reads via the UIA core.

Adopted model: AccessKit vocabulary, RaeenOS-native types. We do NOT vendor the AccessKit
crate into the kernel (std/alloc-heavy, adapter-oriented). We mirror its data shape
(Node/NodeId/Role/Bounds/Action) as small no_std kernel types so the tree maps 1:1 to a future
userspace AccessKit adapter. The RaeenOS boundary is the capability + syscall edge (the
analogue of macOS TCC / UIA core).

## 1b. Verify-before-implement — what already exists (do NOT rebuild)
> **STALE as written (2026-06-21):** the two bullets below described the pre-implementation
> state. Both are now OUT OF DATE — the kernel DOES have an a11y tree (`kernel/src/a11y.rs`),
> and the raeui `infer_role`/`infer_label` stubs were replaced by real role inference
> (`role_from_widget_kind`, `provider_nodes_for_window`). See the audit. Kept for design context.
- components/raeui/src/accessibility.rs ALREADY has a full a11y-tree model in userspace:
  AccessibilityRole (23 roles incl. Window/Button/Slider/...), AccessibilityTraits,
  AccessibilityAction, AccessibilityNode {id,role,label,value,bounds,children,actions},
  AccessibilityTree (build_from_widget_tree, focus_next/prev, describe_focused, high-contrast
  palette). ~~STATUS [ ]: it builds from the RaeUI WidgetNode tree and is NOT wired to anything
  on-screen~~ (STALE — role inference now real: `role_from_widget_kind`, NOT a `Group` stub).
  The remaining live gap is the PROVIDER WIRING (`provider_nodes_for_window` has no caller yet),
  not the role stubs — see audit Top-5 #1.
- ~~The KERNEL has NO a11y tree at all today.~~ (STALE — `kernel/src/a11y.rs` is live, 1236
  lines, R10-complete.) The delta described here was: build the tree in the kernel from
  the compositor surface list (sec 2), expose it (procfs + cap-gated syscalls), and leave the
  raeui types as the userspace AccessKit-adapter shape they map onto. **This is DONE.** Do NOT duplicate the raeui
  Role/Action enums in a new userspace crate; the kernel a11y.rs Role mirrors AccessKit and the
  existing raeui enum, and the widget provider (sec 6) is where raeui's per-widget data later
  flows in. raeui's high-contrast palette + focus traversal feed 19.2/19.3, not this tier.

## 2. Where the tree comes from (the live surface tree already exists)
kernel/src/compositor.rs already owns the authoritative window tree: Surface { id, owner_task,
width, height, x, y, visible, z_order (higher = closer), title[48]/title_len, minimized }.
CompositorState.surfaces: Vec<Surface> + focused_surface_id() + exclusive_surface_id() give
role+name+state+bounds+z for every window today. Foundation tier builds the a11y tree from this
— no new compositor state required.

Foundation = window-level tree. Each Surface -> one Node with Role::Window. Widget-level child
nodes (Button/Label/TextInput inside a window) require RaeUI/RaeShell to report per-widget
role+name+bounds; that is a declared extension point (sec 6), not built in this tier. The
smoketest and procfs prove the window tier; the widget hook is wired but empty until raeen-ui
populates it.

Lock discipline (mandatory — CLAUDE.md 10.6): the compositor mutex is acquired
interrupts-disabled via lock_compositor() because syscalls run with RFLAGS.IF=0 on the single
scheduling CPU. The a11y snapshot path runs inside a syscall, so it MUST acquire the surface
list through lock_compositor(), copy out a plain Vec<AccessNode> (no references held across the
unlock), drop the guard, then serialize. Never hold COMPOSITOR across the copy-to-user.

## 3. New module: kernel/src/a11y.rs
Module doc (//!): "Accessibility tree (AccessKit-compatible) — Phase 19.1. Concept Security:
OS enforces capabilities at the syscall layer; no app reads anothers UI tree unprompted."

Types (no_std):
- Role (repr u16): Unknown=0, Window=1, Button=2, Label=3, TextInput=4, CheckBox=5, Slider=6,
  List=7, MenuItem=8, plus Desktop for the root.
- NodeState bitflags u32: VISIBLE, FOCUSED, MINIMIZED, DISABLED, OFFSCREEN.
- Actions bitflags u32: FOCUS (raise+focus window), DEFAULT (activate; foundation = focus
  only), CLOSE.
- AccessNode { id: u64 (== Surface.id at window tier), parent: u64 (0 = root desktop),
  role: Role, name: String (from Surface.title[..title_len]), state: NodeState,
  bounds: (i32,i32,u32,u32) = x,y,w,h, actions: Actions, z_order: u32 }.

Functions:
- init() — registers /proc/raeen/a11y, logs the init line. Called from kernel_main AFTER
  compositor::init so the surface list exists.
- build_tree() -> Vec<AccessNode> — lock_compositor(), walk surfaces, map each to an AccessNode
  (root id=0 desktop; windows parent=0). State from visible + focused_surface_id() + minimized.
  Drop the guard before returning.
- snapshot_for_client(cap_ok: bool) -> Result<Vec<AccessNode>, A11yError> — returns the tree
  only if cap_ok (caller already passed the Cap::Accessibility{READ} check at the syscall edge).
- dispatch_action(node_id, action, cap_ok) -> bool — FOCUS/DEFAULT ->
  compositor::focus_surface(node_id); CLOSE -> compositor::close_surface(node_id). Returns false
  if no cap or no such node. NO stub arms — every action routes to a real compositor call.
- dump_text() -> String — the procfs renderer (sec 5).
- run_boot_smoketest() — the FAIL-able proof (sec 4).

## 4. R10 4-artifact contract
1. init log line: [a11y] accessibility tree online (AccessKit-compatible, window tier)
2. FAIL-able boot smoketest (a11y::run_boot_smoketest, from the boot smoketest runner): seed a
   synthetic kernel surface with a known title, build the tree, assert the node appears with the
   right role/name/bounds AND that the cap gate denies an un-capped client. Any false -> FAIL.
   Pass line:
   [a11y] tree smoketest: seeded_window_found=true role=Window name_ok=true bounds_ok=true
   focused_state_ok=true cap_denies_uncapped=true action_focus_ok=true -> PASS
   Note: create the probe via compositor::create_kernel_surface(...), title = "a11y-probe";
   build_tree(); find name == a11y-probe; assert role==Window; bounds match seeded x/y/w/h;
   snapshot_for_client(false) returns Err. Tear the probe down at the end (Surface::drop frees
   its frames).
3. procfs: /proc/raeen/a11y registered in procfs.rs alongside the table: ("a11y",
   crate::a11y::dump_text) next to ("compositor", proc_raeen_compositor).
4. Concept docstring quote: the Security lines at the top of this spec, in the //! module doc.

## 5. /proc/raeen/a11y shape
Header: nodes/focused/cap_gated. One line per node, e.g.:
  node id=17 parent=0 role=Window name="Settings" state=visible,focused bounds=200,120,900,640
  z=2 actions=focus,default,close
Minimized windows show state=minimized; widget child nodes indent under their window once sec 6
is populated.

## 6. Extension point for widget-level nodes (declared, not built here)
a11y.rs exposes set_widget_provider(f: fn(window_id: u64) -> Vec<AccessNode>). When raeen-ui
later reports per-widget role+name+bounds (a RaeUI item, Phase 8/19 follow-up), build_tree()
calls the provider per window and parents the returned widget nodes under that windows
Surface.id. Until then the provider is None and the tree is window-tier only. This is the seam
for the screen reader (19.2) and keyboard nav (19.3).

## 7. NEEDS-INTERFACE — for raeen-architect (land in an [interface] commit FIRST)
The cap-gated AT API requires ABI surface only raeen-architect may add. raeen-accessibility
builds against these AFTER they land; do not add them yourself.
1. New capability variant in components/rae_abi + kernel/src/capability.rs:
   Cap::Accessibility { rights: Rights }  (READ = read the tree; WRITE = dispatch actions).
   Rationale: assistive tech is its own privilege class (matches macOS TCC Accessibility and
   UIA); reusing Cap::Debug/Cap::System would over-grant. Additive enum variant -> bump
   ABI_VERSION (new Cap flavor is a contract change) + update docs/SYSCALL_TABLE.md same commit.
2. Two syscalls (propose next free numbers; last allocated was 267 SYS_AUDIO_SUBMIT — architect
   confirms/assigns):
   - SYS_A11Y_SNAPSHOT (268?): rdi=user buf ptr, rsi=buf len; returns node count written.
     Gated on a held Cap::Accessibility{READ}. Serializes into a repr(C) A11yNode array via
     validated copy_to_user (no raw deref — match the net/theme syscall fix pattern).
   - SYS_A11Y_ACTION (269?): rdi=node_id, rsi=action bits. Gated on Cap::Accessibility{WRITE}.
     Routes to a11y::dispatch_action.
   - Define repr(C) A11yNode { id, parent, role, state, x, y, w, h, actions, name[48] } in
     rae_abi (fixed ~80 bytes; name inline like ThemeInfo/SurfaceHdr).
3. The syscall.rs dispatch arms land WITH the numbers (number in rae_abi + dispatch arm in the
   same commit, per house rules).

## 8. Boot-log lines that prove each Phase 19.1 item (QEMU, headless, no iron/input)
- Item (1) tree from the surface tree with role/name/state/bounds/actions:
  [a11y] accessibility tree online (AccessKit-compatible, window tier) then the tree smoketest
  PASS line (sec 4).
- Item (2) /proc/raeen/a11y snapshot + FAIL-able smoketest: the smoketest line asserts the
  seeded window appears with the correct role; /proc/raeen/a11y dumps the live tree.
- Item (3) capability-gated AT API: cap_denies_uncapped=true (un-capped snapshot returns Err) +
  action_focus_ok=true (a capped action reaches the compositor), both inside the smoketest line.
Mark [~] on QEMU pass; [x] only on iron (paused).

## 9. Hand-off and follow-ups
> **STALE ordering (2026-06-21):** every "First / Then" step below has ALREADY HAPPENED. The
> interface (sec 7) landed (ABI 277/278, `Cap::Accessibility`), `kernel/src/a11y.rs` is built
> with the smoketest + host KAT, and all five followups (screen reader, magnifier, high-contrast,
> keyboard nav, widget provider) are at least partially built — see the audit for per-item state.
> The ONE remaining big dependency called out here ("RaeUI widget provider — the biggest
> dependency for real screen-reader value") is still the live #1 gap (provider has no caller).
- First: raeen-architect lands the NEEDS-INTERFACE block (sec 7) in an [interface] commit (Cap
  variant + 2 syscalls + A11yNode repr + ABI_VERSION bump + SYSCALL_TABLE).
- Then: raeen-accessibility builds kernel/src/a11y.rs (sec 3-5), wires init() into kernel_main
  after compositor::init, registers procfs, lands the FAIL-able smoketest (sec 4), and adds a
  host KAT (build a tree from synthetic Surface-like inputs; assert roles/states/cap-deny) per
  house rule 15 (pure logic gets a host KAT first).
- Follow-ups (separate items, do NOT build here):
  - 19.2 screen reader: focus-follows event stream + a text/braille/log sink (set_widget_provider
    seam + a focus-change event from the compositor).
  - 19.2 magnifier: compositor-level region scale.
  - 19.2 high-contrast / color-filter themes (ties to the Phase 13 theme engine).
  - 19.3 full keyboard navigation; sticky/slow keys in input.rs.
  - RaeUI widget provider (sec 6) — the biggest dependency for real screen-reader value.
