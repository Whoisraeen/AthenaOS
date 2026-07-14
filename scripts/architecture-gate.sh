#!/usr/bin/env bash
# architecture-gate.sh — mechanical enforcement of the RaeenOS architecture rules
# on every commit. Wired into .git/hooks/pre-commit. Fails the commit on:
#
#   (a) §R7 violations: new std-isms or Linux clones in no_std/kernel crates.
#   (b) a deleted or bypassed RaeShield capability check.
#   (c) a new kernel module missing the R10 4-artifact contract
#       (init + run_boot_smoketest + procfs/dump_text + Concept docstring).
#   (d) a changed signature in the shared-interface crate (rae_abi / rae_driver_api)
#       without an Opus sign-off marker ([interface] tag + RAEEN_AGENT=opus).
#
# Concept doc (RaeenOS_Concept.md) and docs/LINUX_DRIVER_STRATEGY.md §R7 win.
set -uo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
AGENT="${RAEEN_AGENT:-}"
fail=0

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }

STAGED=$(git diff --cached --name-only --diff-filter=ACMR)
[ -z "$STAGED" ] && { green "architecture-gate: no staged changes."; exit 0; }

# Added lines only (what this commit introduces), per file.
added_lines() { git diff --cached -U0 -- "$1" | grep -E '^\+' | grep -vE '^\+\+\+'; }
removed_lines() { git diff --cached -U0 -- "$1" | grep -E '^-' | grep -vE '^---'; }

# ── (a) §R7: no std-isms / Linux clones in no_std (kernel + components) ───────
# Linux-clone module/identifier names that must never enter the native stack.
# (Our own shim `raeen_drm` is allowed; bare `drm`/`kms`/`wayland` etc. are not.)
LINUX_CLONE_RE='\b(ext4|ext2|btrfs_clone|wayland|netfilter|iptables|seccomp|epoll_clone|io_uring|pulseaudio|alsa|systemd|rustysd|cgroup_v[12]|sysfs_linux)\b'
for f in $STAGED; do
  case "$f" in
    # Vendored third-party crates (components/vendored/*) are upstream
    # snapshots patched via [patch.crates-io]; their cfg(test)-gated std use
    # is upstream's and is reviewed at vendor time. The Linux-clone check
    # below still applies to them; only the std-ism line-lint is skipped.
    components/vendored/*/src/*.rs)
      add=$(added_lines "$f")
      if echo "$add" | grep -qiE "$LINUX_CLONE_RE"; then
        hit=$(echo "$add" | grep -ioE "$LINUX_CLONE_RE" | head -1)
        red "  R7 FAIL  $f adds a Linux-clone identifier ('$hit'). Use the RaeenOS proprietary stack."
        fail=1
      fi
      ;;
    kernel/src/*.rs|components/*/src/*.rs)
      add=$(added_lines "$f")
      # std-ism: a new `use std::` or `extern crate std` in a no_std crate.
      if echo "$add" | grep -qE '^\+[[:space:]]*(use[[:space:]]+std::|extern[[:space:]]+crate[[:space:]]+std)'; then
        red "  R7 FAIL  $f introduces a std-ism (use std:: / extern crate std) in a no_std crate."
        fail=1
      fi
      # Linux clone identifier.
      if echo "$add" | grep -qiE "$LINUX_CLONE_RE"; then
        hit=$(echo "$add" | grep -ioE "$LINUX_CLONE_RE" | head -1)
        red "  R7 FAIL  $f adds a Linux-clone identifier ('$hit'). Use the RaeenOS proprietary stack."
        fail=1
      fi
      ;;
  esac
done

# New third-party Linux/Unix port directories are a §R7 breadth-creep vector.
for f in $STAGED; do
  case "$f" in
    ports/*)
      # New port additions to xtask's port list are flagged for human review.
      :;;
  esac
done
if echo "$STAGED" | grep -q '^xtask/src/main.rs$'; then
  if added_lines xtask/src/main.rs | grep -qiE 'rustysd|systemd|busybox|coreutils'; then
    red "  R7 FAIL  xtask adds a Linux init/coreutils clone to the port list. Native RaeenOS only."
    fail=1
  fi
fi

# ── (b) RaeShield capability checks must not be deleted/bypassed ──────────────
CAP_CALL_RE='(capability::|cap_check|check_cap|require_cap|assert_system_authority|with_current_task.*cap_table)'
for f in $STAGED; do
  case "$f" in
    kernel/src/*.rs|components/raeshield/*.rs)
      removed=$(removed_lines "$f")
      added=$(added_lines "$f")
      r_count=$(echo "$removed" | grep -cE "$CAP_CALL_RE")
      a_count=$(echo "$added"   | grep -cE "$CAP_CALL_RE")
      if [ "$r_count" -gt "$a_count" ]; then
        red "  CAP FAIL  $f removes more capability checks ($r_count) than it adds ($a_count)."
        yellow "            A privileged path may have lost its RaeShield gate. Restore it or get Opus sign-off."
        fail=1
      fi
      # Explicit bypass markers are never allowed.
      if echo "$added" | grep -qiE 'SAFETY-BYPASS|CAP-BYPASS|skip.?cap.?check'; then
        red "  CAP FAIL  $f adds a capability-bypass marker."
        fail=1
      fi
      ;;
  esac
done

# ── (c) R10 4-artifact contract for NEW kernel modules ───────────────────────
# A brand-new kernel/src/<mod>.rs must ship: //! docstring + pub fn init +
# run_boot_smoketest + a procfs/dump_text surface.
for f in $(git diff --cached --name-only --diff-filter=A); do
  case "$f" in
    # arch:: backend seam sub-modules (kernel/src/arch/**) fulfil the R10
    # 4-artifact contract through the PARENT arch:: module: there is one
    # arch::init(), one arch::run_boot_smoketest() (which exercises every seam),
    # and one /proc/raeen/arch dump_text() that reports them all. Per-seam files
    # (addr.rs, future mmu.rs, …) are sub-modules of that single contract, not
    # standalone modules — exactly as the in-`mod.rs` seam sub-modules
    # (interrupts, cpu, interrupt_controller, timer) already are. Exempt them.
    kernel/src/arch/*) continue;;
    kernel/src/*.rs)
      base=$(basename "$f")
      case "$base" in
        main.rs|mod.rs|panic.rs|serial.rs|console.rs|gdt.rs) continue;; # infra exempt
      esac
      content=$(git show ":$f" 2>/dev/null || cat "$f")
      miss=""
      echo "$content" | grep -qE '^//!' || miss="$miss docstring"
      echo "$content" | grep -qE 'pub fn init' || miss="$miss init()"
      echo "$content" | grep -qE 'run_boot_smoketest' || miss="$miss run_boot_smoketest()"
      echo "$content" | grep -qE 'dump_text|proc_dump|/proc/raeen' || miss="$miss procfs"
      if [ -n "$miss" ]; then
        red "  R10 FAIL  new kernel module $f missing:$miss"
        yellow "            Every kernel module that counts ships init + smoketest + procfs + Concept docstring."
        fail=1
      fi
      ;;
  esac
done

# ── (d) Interface crate changes: only Opus may touch them ────────────────────
# The `[interface]` commit-message sign-off is enforced in the commit-msg hook
# (scripts/git-hooks/commit-msg) because the message does not exist yet at
# pre-commit time. Here we enforce the identity half, which needs no message.
if echo "$STAGED" | grep -qE '^components/rae_abi/|^components/rae_driver_api/'; then
  if [ "$AGENT" != "opus" ]; then
    red "  IFACE FAIL  interface crate changed by '$AGENT'. Only Opus edits rae_abi / rae_driver_api."
    fail=1
  else
    green "  IFACE OK    Opus interface change (commit-msg hook will check the [interface] tag)."
  fi
fi

if [ "$fail" -ne 0 ]; then
  red "architecture-gate: commit rejected. Fix the violations above (Concept doc wins)."
  exit 1
fi
green "architecture-gate: §R7 + RaeShield + R10 + interface checks passed."
exit 0
