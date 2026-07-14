#!/usr/bin/env bash
set -u
echo "HOME=$HOME"
if [ -d ~/athenaos ]; then
  echo "SRC:yes"
  cd ~/athenaos
  echo "HEAD: $(git rev-parse --short HEAD 2>/dev/null)"
  echo "branch: $(git branch --show-current 2>/dev/null)"
  echo "status-short:"; git status --short | head
  git log --oneline -3
  echo "remote: $(git remote -v | head -1)"
else
  echo "SRC:no"
fi
echo "rustc: $(rustc --version 2>/dev/null || echo none)"
echo "cargo: $(cargo --version 2>/dev/null || echo none)"
echo "gcc: $(gcc --version 2>/dev/null | head -1 || echo none)"
echo "m4-obj dir: $(ls -d ~/m4-obj 2>/dev/null && ls ~/m4-obj 2>/dev/null | wc -l || echo none)"
