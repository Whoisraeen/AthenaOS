#!/usr/bin/env bash
# ownership-lock.sh — reject any staged diff touching a crate the committing
# agent does not own. Reads agents/OWNERSHIP.toml. Part of the three-agent
# parallel-development guardrail. Wired into .git/hooks/pre-commit.
#
# Identity comes from $RAEEN_AGENT: one of opus | gemini | composer.
# OWNERLESS crates (e.g. components/raebridge) reject EVERYONE.
#
# Usage (manual):  RAEEN_AGENT=gemini scripts/ownership-lock.sh
set -uo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
MANIFEST="$REPO_ROOT/agents/OWNERSHIP.toml"
AGENT="${RAEEN_AGENT:-}"

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }

if [ ! -f "$MANIFEST" ]; then
  red "ownership-lock: agents/OWNERSHIP.toml not found — cannot verify ownership."
  exit 1
fi

if [ -z "$AGENT" ]; then
  red "ownership-lock: RAEEN_AGENT is not set."
  yellow "  Set it to your slice before committing, e.g.:"
  yellow "    export RAEEN_AGENT=gemini   # or opus / composer"
  exit 1
fi

case "$AGENT" in
  opus|gemini|composer) ;;
  *) red "ownership-lock: RAEEN_AGENT='$AGENT' is not a known agent (opus|gemini|composer)."; exit 1;;
esac

# ── Lead-developer full access ───────────────────────────────────────────────
# Owner directive (2026-06): development is consolidated under Opus, the lead
# developer, who has full read/write access to every crate. The per-crate map
# in OWNERSHIP.toml is retained as a SUBSYSTEM AREA map (navigation + intent),
# not an access boundary, while Opus drives AthenaOS to Concept-doc completion.
# The architecture-gate (no-Linux-clone / AthGuard-cap / R10 / interface-tag)
# still runs for Opus — full access, same quality bar.
if [ "$AGENT" = "opus" ]; then
  green "ownership-lock: agent 'opus' (lead developer) has full repository access. OK."
  exit 0
fi

# ── Parse the manifest ───────────────────────────────────────────────────────
# Opus-only meta paths (interface + build contract + governance).
OPUS_ONLY=$(awk '
  /^\[meta\.opus_only_paths\]/ {grab=1; next}
  /^\[/ {grab=0}
  grab && /"/ {
    while (match($0, /"[^"]+"/)) {
      s=substr($0, RSTART+1, RLENGTH-2); print s;
      $0=substr($0, RSTART+RLENGTH);
    }
  }
' "$MANIFEST")

# Shared path prefixes (any agent may edit — its own checklist/docs sections).
SHARED=$(awk '
  /^\[meta\.shared_paths\]/ {grab=1; next}
  /^\[/ {grab=0}
  grab && /"/ {
    while (match($0, /"[^"]+"/)) {
      s=substr($0, RSTART+1, RLENGTH-2); print s;
      $0=substr($0, RSTART+RLENGTH);
    }
  }
' "$MANIFEST")

# Crate -> owner table, from the [crates] section: lines of  "path" = "owner"
CRATE_LINES=$(awk '
  /^\[crates\]/ {grab=1; next}
  /^\[/ {grab=0}
  grab && /^"/ {print}
' "$MANIFEST")

# ── Helper: who owns this file? Longest matching crate-path prefix wins. ──────
owner_of() {
  local f="$1"
  # 1) Opus-only meta paths (exact or prefix).
  while IFS= read -r p; do
    [ -z "$p" ] && continue
    case "$f" in
      "$p"*) echo "opus"; return 0;;
    esac
  done <<< "$OPUS_ONLY"

  # 2) Crate prefix match — keep the longest matching path.
  local best_owner="" best_len=0
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    local path owner
    path=$(echo "$line"  | sed -E 's/^"([^"]+)".*/\1/')
    owner=$(echo "$line" | sed -E 's/^"[^"]+"[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')
    case "$f" in
      "$path"/*|"$path")
        if [ "${#path}" -gt "$best_len" ]; then best_len=${#path}; best_owner="$owner"; fi
        ;;
    esac
  done <<< "$CRATE_LINES"
  if [ -n "$best_owner" ]; then echo "$best_owner"; return 0; fi

  # 3) Shared prefixes — allowed for everyone.
  while IFS= read -r p; do
    [ -z "$p" ] && continue
    case "$f" in
      "$p"*) echo "shared"; return 0;;
    esac
  done <<< "$SHARED"

  # 4) Unclassified root file — treat as shared but warn (e.g. a new top-level file).
  echo "unclassified"
}

# ── Walk the staged diff ─────────────────────────────────────────────────────
STAGED=$(git diff --cached --name-only --diff-filter=ACMRD)
if [ -z "$STAGED" ]; then
  green "ownership-lock: no staged changes."
  exit 0
fi

violations=0
while IFS= read -r f; do
  [ -z "$f" ] && continue
  owner=$(owner_of "$f")
  case "$owner" in
    "$AGENT"|shared)
      ;; # allowed
    opus)
      if [ "$AGENT" != "opus" ]; then
        red "  DENY  $f"
        yellow "        → interface/build/governance path: opus only. Route the change through Opus."
        violations=$((violations+1))
      fi
      ;;
    OWNERLESS)
      red "  DENY  $f"
      yellow "        → crate is OWNERLESS (e.g. raebridge): requires human assignment."
      violations=$((violations+1))
      ;;
    unclassified)
      yellow "  WARN  $f (unclassified path — add it to agents/OWNERSHIP.toml). Allowing."
      ;;
    *)
      red "  DENY  $f"
      yellow "        → owned by '$owner'; you are '$AGENT'. Edit only your own crates."
      violations=$((violations+1))
      ;;
  esac
done <<< "$STAGED"

if [ "$violations" -gt 0 ]; then
  red "ownership-lock: $violations file(s) outside your slice ($AGENT). Commit rejected."
  yellow "Interface changes must route through Opus. See agents/OWNERSHIP.toml."
  exit 1
fi

green "ownership-lock: all staged files are within the '$AGENT' slice. OK."
exit 0
