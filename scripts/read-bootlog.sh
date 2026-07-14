#!/usr/bin/env bash
# read-bootlog.sh - read (or pre-create) RaeenOS's BOOTLOG.TXT from an ESP, on Linux.
#
# The kernel persists the boot-log ring to a PRE-CREATED 1 MiB BOOTLOG.TXT in the
# ESP root (kernel/src/bootlog_persist.rs). On Linux, unlike Windows, you just
# mount the FAT partition - this wraps that: mount the ESP, strip the null
# padding, print/grep. (The big read-bootlog.ps1 only walks the raw disk by hand
# because Windows refuses to mount a removable ESP; Linux has no such problem.)
#
# IMPORTANT (the "it only worked when we pre-created it" gotcha): in --safe mode
# the kernel can ONLY overwrite an EXISTING BOOTLOG.TXT - its clusters are already
# allocated, and creating a new file would be a forbidden sector write. xtask
# bakes the file into the USB image, but any OTHER target ESP (e.g. the internal
# NVMe ESP) needs it pre-created. Use `--create` for that.
#
# Usage:
#   scripts/read-bootlog.sh /dev/sda1               # read from a FAT/ESP partition
#   scripts/read-bootlog.sh /mnt/usb                # read from an already-mounted ESP
#   scripts/read-bootlog.sh /dev/sda1 -g amdgpu     # read + grep (case-insensitive)
#   scripts/read-bootlog.sh /dev/sda1 --create      # pre-create the 1 MiB BOOTLOG.TXT
#   scripts/read-bootlog.sh --list                  # list candidate FAT/ESP partitions
set -euo pipefail

GREP=""
CREATE=0
TARGET=""
while [ $# -gt 0 ]; do
    case "$1" in
        -g | --grep) GREP="${2:-}"; shift 2 ;;
        --create) CREATE=1; shift ;;
        --list)
            echo "candidate FAT/ESP partitions (RM=1 is removable):"
            lsblk -o NAME,FSTYPE,LABEL,SIZE,RM,MOUNTPOINT | grep -iE "vfat|fat|NAME" || true
            exit 0 ;;
        -h | --help) sed -n '2,30p' "$0"; exit 0 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) TARGET="$1"; shift ;;
    esac
done

if [ -z "$TARGET" ]; then
    echo "usage: $0 <partition|mountpoint> [--create] [-g PATTERN]   (or --list)" >&2
    echo "candidate FAT partitions:" >&2
    lsblk -o NAME,FSTYPE,LABEL,SIZE,RM,MOUNTPOINT | grep -iE "vfat|fat" >&2 || true
    exit 2
fi

# Resolve to a mountpoint. TARGET is either an already-mounted directory or a
# block device we mount ourselves (and unmount on exit).
MNT=""
OWN_MOUNT=0
cleanup() {
    if [ "$OWN_MOUNT" = 1 ] && mountpoint -q "$MNT" 2>/dev/null; then
        sudo umount "$MNT" && rmdir "$MNT" 2>/dev/null || true
    fi
}
trap cleanup EXIT

if [ -d "$TARGET" ]; then
    MNT="$TARGET"
elif [ -b "$TARGET" ]; then
    MNT="$(mktemp -d)"
    OWN_MOUNT=1
    if [ "$CREATE" = 1 ]; then
        sudo mount -t vfat "$TARGET" "$MNT"
    else
        sudo mount -t vfat -o ro "$TARGET" "$MNT"
    fi
else
    echo "[read-bootlog] '$TARGET' is neither a directory nor a block device" >&2
    exit 2
fi

if [ "$CREATE" = 1 ]; then
    echo "[read-bootlog] pre-creating 1 MiB $MNT/BOOTLOG.TXT (clusters allocated so --safe can overwrite it in place)"
    sudo dd if=/dev/zero of="$MNT/BOOTLOG.TXT" bs=1M count=1 status=none
    sudo sync
    echo "[read-bootlog] done - boot RaeenOS (--safe) on this target and it'll fill the file."
    exit 0
fi

F="$MNT/BOOTLOG.TXT"
if [ ! -f "$F" ]; then
    echo "[read-bootlog] no BOOTLOG.TXT in $MNT" >&2
    echo "[read-bootlog]   -> flash a current image, or pre-create it: $0 $TARGET --create" >&2
    exit 1
fi

# Strip the null padding (the gap between the locked early-boot region and the
# latest ring tail) before printing - mirrors read-bootlog.ps1's zero-collapse.
if [ -n "$GREP" ]; then
    tr -d '\0' < "$F" | grep -iE "$GREP" || { echo "[read-bootlog] no match for '$GREP'"; exit 0; }
else
    OUT="BOOTLOG.dump.txt"
    tr -d '\0' < "$F" > "$OUT"
    echo "[read-bootlog] saved $(wc -c < "$OUT") bytes (nulls stripped) -> $OUT"
    echo "[read-bootlog] -- first 25 lines --"
    head -25 "$OUT" | sed 's/^/  /'
    echo "[read-bootlog] -- (full log in $OUT) --"
fi
