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

run -p ath_crypto
run -p athnet     --features tls13
run -p athssh
run -p athfs      --features ntfs_ro
run -p athaudio   --features "dsp hq_resample"
run -p athgfx     --features tessellate
run -p athshell   --features terminal_vt
run -p athbridge
# Browser pillar (web apps / PWA) — pure-logic HTML/CSS/JS, lib crates that test
# cleanly on host. Added to the gate after both rotted unnoticed (a non-compiling
# athweb test + an invisible <hr>) precisely because they were NOT gated here.
run -p athweb
run -p ath_js
# Pure-logic component libs (UI / HID / fonts / locale / document engines / media)
# — all host-KAT clean; gated here so they can't silently rot the way the browser
# pillar did (a crate not in this gate WILL eventually rot).
run -p athui
run -p athkit
run -p athfont
run -p athhid
run -p athlocale
run -p ath_pdf
run -p ath_docx
run -p ath_xlsx
run -p athmedia
run -p athpkg
run -p athwasm
run -p athvpn
# Services + media/data/document engines + filesystem + security — the full
# anti-rot sweep, so every host-testable non-GPU crate turns CI red on regression.
run -p athstore
run -p athsync
run -p athid
run -p ath_image
run -p ath_mp4
run -p ath_gif
run -p ath_mime
run -p ath_formats
run -p ath_kv
run -p ath_pim
run -p ath_otp
run -p ath_keychain
run -p ath_mail
run -p athfat
run -p athshield
# Data/text formats, image codecs, and remaining services — found tested-but-ungated
# (2026-06-25): ~650 host KATs that were rotting unguarded. All pass cleanly on host;
# gated here so a regression turns CI red (a crate not in this gate WILL rot).
run -p ath_json
run -p ath_toml
run -p ath_csv
run -p ath_markdown
run -p ath_diff
run -p ath_tar
run -p ath_zip
run -p ath_deflate
run -p ath_encode
run -p ath_hash
run -p ath_time
run -p ath_tokens
run -p ath_calc
run -p ath_files
run -p ath_pwa
run -p ath_regex
run -p ath_jpeg
run -p ath_png
run -p ath_bmp
run -p ath_webp
run -p athprint
run -p athupdate
run -p athlang
run -p athbackup
run -p athpackage
run -p athcloud
run -p athcontainer
run -p athaccessibility
run -p athsettings
run -p athai
run -p athplay
run -p aarch64_logic
# User-facing apps — the LIB target host-KATs the live draw/state path (the no_main
# ELF bin is `test = false`); `--features host` pulls the std athkit shim so it links.
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
echo "=== athfs_filekey_kat ==="
cargo run --release --manifest-path tools/athfs_filekey_kat/Cargo.toml
echo ""
echo "=== linuxkpi_harness ==="
cargo run --release --manifest-path tools/linuxkpi_harness/Cargo.toml

echo ""
echo "ALL HOST KATS PASSED"
