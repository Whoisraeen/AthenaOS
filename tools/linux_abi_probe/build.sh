#!/usr/bin/env bash
# Build the AthenaOS Linux-ABI probe (see probe.c).
#
# This produces `linux_abi_probe.elf`, a tiny static x86_64 Linux binary that
# the kernel embeds (kernel/src/linux_exec.rs) and spawns at boot to PROVE the
# Linux syscall-translation layer works on a real Linux ELF. It MUST be built on
# a Linux box with gcc (e.g. the Athena Arch box over SSH) — there is no
# cross-toolchain on the Windows dev box. Commit the resulting .elf.
#
# Usage (on a Linux host, from this directory):
#   ./build.sh
# Or remotely from the dev box:
#   scp probe.c whoisathena@192.168.1.244:/tmp/ && \
#   ssh whoisathena@192.168.1.244 'cd /tmp && bash -s' < build.sh && \
#   scp whoisathena@192.168.1.244:/tmp/linux_abi_probe.elf .
set -euo pipefail

SRC="${1:-probe.c}"
OUT="${2:-linux_abi_probe.elf}"

# -nostdlib + raw syscalls: nothing runs before our checks, so a PASS is
# unambiguously our translation layer. -fno-stack-protector is REQUIRED — with
# no TLS the canary read from %fs:0x28 faults before main().
gcc -static -no-pie -nostdlib -ffreestanding \
    -fno-builtin -fno-stack-protector -fno-tree-loop-distribute-patterns \
    -O2 -o "$OUT" "$SRC"
strip "$OUT"

echo "[build] $OUT = $(stat -c %s "$OUT") bytes"
echo "[build] self-test on this Linux host (must print PASS, exit 0):"
"./$OUT"; echo "[build] exit=$?"
