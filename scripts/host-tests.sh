#!/usr/bin/env bash
# Host KAT gate — runs the pure-logic tests for every testable workspace crate
# WITHOUT QEMU or the bare-metal kernel target. The kernel itself can't be
# `cargo test`ed (no_std custom target), so these are the cheapest real proof
# (CLAUDE.md SS15 layer 1) and they caught real bugs this cycle.
#
# Each component is `#![cfg_attr(not(test), no_std)]`: no_std for the real build,
# std under `cargo test` so the harness links. std-leaning deps (TLS, rubato,
# naga, vte, ntfs) sit behind features enabled here so the bare-metal build never
# pulls them.
#
# Used by .github/workflows/host-tests.yml and runnable locally:
#   bash scripts/host-tests.sh
set -euo pipefail
cd "$(dirname "$0")/.."

run() {
    echo ""
    echo "=== cargo test $* ==="
    cargo test "$@"
}

run -p rae_crypto
run -p raenet     --features tls13
run -p raessh
run -p raefs      --features ntfs_ro
run -p raeaudio   --features "dsp hq_resample"
run -p raegfx     --features tessellate
run -p raeshell   --features terminal_vt
run -p raebridge
# Browser pillar (web apps / PWA) — pure-logic HTML/CSS/JS, lib crates that test
# cleanly on host. Added to the gate after both rotted unnoticed (a non-compiling
# raeweb test + an invisible <hr>) precisely because they were NOT gated here.
run -p raeweb
run -p rae_js
# Pure-logic component libs (UI / HID / fonts / locale / document engines / media)
# — all host-KAT clean; gated here so they can't silently rot the way the browser
# pillar did (a crate not in this gate WILL eventually rot).
run -p raeui
run -p raekit
run -p raefont
run -p raehid
run -p raelocale
run -p rae_pdf
run -p rae_docx
run -p rae_xlsx
run -p raemedia
run -p raepkg
run -p raewasm
run -p raevpn
# Services + media/data/document engines + filesystem + security — the full
# anti-rot sweep, so every host-testable non-GPU crate turns CI red on regression.
run -p raestore
run -p raesync
run -p raeid
run -p rae_image
run -p rae_mp4
run -p rae_gif
run -p rae_mime
run -p rae_formats
run -p rae_kv
run -p rae_pim
run -p rae_otp
run -p rae_keychain
run -p rae_mail
run -p raefat
run -p raeshield
# Data/text formats, image codecs, and remaining services — found tested-but-ungated
# (2026-06-25): ~650 host KATs that were rotting unguarded. All pass cleanly on host;
# gated here so a regression turns CI red (a crate not in this gate WILL rot).
run -p rae_json
run -p rae_toml
run -p rae_csv
run -p rae_markdown
run -p rae_diff
run -p rae_tar
run -p rae_zip
run -p rae_deflate
run -p rae_encode
run -p rae_hash
run -p rae_time
run -p rae_tokens
run -p rae_calc
run -p rae_files
run -p rae_pwa
run -p rae_regex
run -p rae_jpeg
run -p rae_png
run -p rae_bmp
run -p rae_webp
run -p raeprint
run -p raeupdate
run -p raelang
run -p raebackup
run -p raepackage
run -p raecloud
run -p raecontainer
run -p raeaccessibility
run -p raesettings
run -p raeai
run -p raeplay
run -p aarch64_logic
# User-facing apps — the LIB target host-KATs the live draw/state path (the no_main
# ELF bin is `test = false`); `--features host` pulls the std raekit shim so it links.
run -p files     --features host
run -p passwords --features host
run -p calendar  --features host
run -p photos    --features host
run -p video     --features host
run -p mail      --features host

# Standalone host harnesses (own main(), print PASS/FAIL + exit nonzero on fail).
echo ""
echo "=== argon2_kat ==="
cargo run --release --manifest-path tools/argon2_kat/Cargo.toml
echo ""
echo "=== raefs_filekey_kat ==="
cargo run --release --manifest-path tools/raefs_filekey_kat/Cargo.toml
echo ""
echo "=== linuxkpi_harness ==="
cargo run --release --manifest-path tools/linuxkpi_harness/Cargo.toml

echo ""
echo "ALL HOST KATS PASSED"
