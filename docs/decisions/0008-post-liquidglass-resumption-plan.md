# ADR 0008 — Post-Liquid-Glass-merge resumption plan & concurrent-session coordination

- Status: accepted
- Date: 2026-06-21
- Owner: athena-lead (autonomous)

## Context
Mid-session a SECOND active session began a major "Liquid Glass identity / Aurora" UI
overhaul in the same OneDrive-shared tree (commits 7d31891, df48a91, bb3895c, 48677e7,
dcb4ee3, 3b5321d, 89bdb49, 4ac0f20, 868bfe4, 00a052b + a workspace `cargo fmt`), touching
ath_tokens, athgfx, kernel (compositor/aurora/main/shell_runner), apps/files,
notifications_daemon, and the screenshot harness. Two agents on overlapping crates is the #1
documented failure mode (multi-agent shared worktree).

Response taken: yielded the UI/kernel lanes to the concurrent session; worked only
collision-free (specs + isolated leaf/component-crate host-KATs) with explicit-path commits;
did NOT install the pre-commit hook (it would force the concurrent session's commits through
the gate and could block them); verified the merged tree GREEN (both streams integrate — athgfx
icon KATs + glass/aurora KATs coexist 19/19, 3/3 boots, full a11y/CC/notify/search regression
intact). My corrective process notes: never pipe the architecture-gate through `tail` (masks the
exit code — caused one bad commit, since reverted); always read its pass/reject verdict.

## Decision — what to resume once the concurrent UI overhaul SETTLES
"Settled" = the concurrent session's contested files (ath_tokens/athgfx/kernel-compositor/
apps-files/notifications_daemon) show committed with no fresh uncommitted churn for a stable
window. Until then, stay on isolated host-KAT / doc lanes only.

Prioritized resumption queue (build ON the committed Liquid Glass foundation):
1. **Visual-QA Round-5 on the settled identity** — the concurrent session exposed
   `apps/files::render_preview` (3b5321d) and `athkit` host feature (868bfe4), so the harness can
   now render the LIVE Files + the Aurora/glass surfaces. Re-screenshot all surfaces, critique vs
   macOS/Win11, file the remaining polish gaps. (athena-visual-qa.)
2. **Notification Center dead-twin resolution** — the concurrent session is polishing
   `notifications_daemon` (the unpolished twin the harness rendered). Once it lands, confirm one
   canonical notification renderer (no twin) and that visual-QA sees the real one.
3. **Multi-arch Slice 0** (ADR 0007) — introduce the `arch::` boundary, move x86_64 behind it
   with ZERO behavior change; needs a quiet kernel + >=5-boot verify. Kernel-contested now → wait.
4. **Multi-arch Slice 1** (arch-neutral MM newtypes) → then the aarch64 bring-up (spec
   docs/research/aarch64-boot-bringup.md): milestones a-f.
5. **Boot-time** — measurements are inflated by concurrent-build dev-box load (6.1→7.7s during the
   marathon; not a code regression). Re-measure on a quiet box; the next lever is gating the
   pre-marker procfs serial dump (ADR 0006) — kernel-contested now → wait.
6. **Non-UI subsystem depth** (collision-free even now, host-KAT-able): AthNet protocol
   completeness (TLS 1.3 handshake, sockets API), AthAudio engine, AthFS hardening (criterion #2,
   but kernel-contested for boot proof), services/installer.
7. **Library follow-ups surfaced this session**: ath_json shortest-float is done; athshell
   inline-vector glyphs → the 13 new athgfx canonical icons (CC swap, kernel-contested);
   websocket RFC-6455 control-frame ≤125 enforcement (non-safety); ath_diff byte_delta → athupdate
   wiring behind safe_mode_guard_write.

## Rationale
Merge-safety (#1 failure mode) outranks raw throughput. Yielding the contested lanes and verifying
the merge GREEN preserved both sessions' work with zero collisions. The queue lets the next phase
resume at the highest-leverage point on a stable tree.

## How to reverse
Advisory roadmap; re-prioritize freely once the tree stabilizes.
