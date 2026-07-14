#!/usr/bin/env bash
# C-link smoke test for ath_linuxkpi (Path C).
#
# Proves the shim is genuinely linkable from C: builds libath_linuxkpi.a (the
# `--features clib --crate-type staticlib` artifact) for x86_64-unknown-none and
# links tools/linuxkpi_clink/probe.c (a stand-in driver TU) against it. Zero
# unresolved symbols == a real Linux .ko can link the same exported surface.
#
# Run from a Linux host with the rust nightly toolchain + the bare-metal target
# (see the WSL2 dev env). Not run on Windows.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "[clink] building staticlib (ath_linuxkpi, --features clib) ..."
cargo rustc -p ath_linuxkpi --target x86_64-unknown-none --release \
    --features clib --crate-type staticlib

A="$(realpath "$(find target/x86_64-unknown-none/release -name 'libath_linuxkpi.a' | head -1)")"
[ -f "$A" ] || { echo "[clink] FAIL: staticlib not produced"; exit 1; }
echo "[clink] archive: $A ($(nm "$A" 2>/dev/null | grep -cE ' T ') exported T symbols)"

OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT
cc -ffreestanding -fno-stack-protector -fno-builtin \
    -c tools/linuxkpi_clink/probe.c -o "$OUT/probe.o"

echo "[clink] linking the C probe against the shim ..."
cc -nostdlib -nostartfiles -static -Wl,-e,probe_main \
    -o "$OUT/probe.elf" "$OUT/probe.o" -L"$(dirname "$A")" -lath_linuxkpi

UNRESOLVED="$(nm -u "$OUT/probe.elf" 2>/dev/null || true)"
if [ -n "$UNRESOLVED" ]; then
    echo "[clink] FAIL: unresolved symbols after linking the C driver:"
    echo "$UNRESOLVED"
    exit 1
fi
echo "[clink] PASS: C driver TU links clean against libath_linuxkpi.a (0 unresolved)."
