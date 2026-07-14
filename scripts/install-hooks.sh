#!/usr/bin/env bash
# install-hooks.sh — install the AthenaOS three-agent guardrail hooks into
# .git/hooks/. Run once per clone (the hook itself is not version-controlled,
# so each agent's checkout must install it). Idempotent.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"

for hook in pre-commit commit-msg; do
  cp "$REPO_ROOT/scripts/git-hooks/$hook" "$REPO_ROOT/.git/hooks/$hook"
  chmod +x "$REPO_ROOT/.git/hooks/$hook" 2>/dev/null || true
  echo "Installed $hook -> $REPO_ROOT/.git/hooks/$hook"
done
chmod +x "$REPO_ROOT/scripts/ownership-lock.sh" "$REPO_ROOT/scripts/architecture-gate.sh" 2>/dev/null || true
echo "Remember to set your agent identity, e.g.:  export RAEEN_AGENT=opus"
