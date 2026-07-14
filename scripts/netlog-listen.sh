#!/usr/bin/env bash
# netlog-listen.sh - receive AthenaOS's end-of-boot UDP bootlog broadcast (Linux).
#
# The kernel broadcasts the in-RAM boot-log ring to 255.255.255.255:51514 as
# chunked datagrams (kernel/src/netlog.rs). This binds that port, reassembles
# each snapshot, and prints the full log per boot - no stick juggling, just run it
# on the same LAN segment before/while Athena boots. Ctrl-C to stop.
#
# Payload framing (little-endian): magic "RLG1" | boot_id u32 | seq u16 |
# total u16 | text chunk (<=1024). The full snapshot is re-sent at end-of-boot
# (twice), so the listener reassembles by (boot_id, seq) last-write-wins and
# de-dups identical re-sends.
#
# Usage:
#   scripts/netlog-listen.sh                # print every boot's log
#   scripts/netlog-listen.sh -g amdgpu      # only lines matching (case-insensitive)
set -euo pipefail

command -v python3 >/dev/null || { echo "netlog-listen.sh needs python3" >&2; exit 1; }

exec python3 - "$@" <<'PY'
import socket, struct, sys

grep = None
args = sys.argv[1:]
if len(args) >= 2 and args[0] in ("-g", "--grep"):
    grep = args[1].lower()

PORT = 51514
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
try:
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
except OSError:
    pass
s.bind(("", PORT))
print("[netlog] listening on UDP %d (Ctrl-C to stop)..." % PORT, file=sys.stderr)

snaps = {}    # boot_id -> {seq: chunk_bytes}
printed = {}  # boot_id -> assembled length already printed (de-dup re-sends)
try:
    while True:
        data, addr = s.recvfrom(2048)
        if len(data) < 12 or data[:4] != b"RLG1":
            continue
        (boot_id,) = struct.unpack_from("<I", data, 4)
        seq, total = struct.unpack_from("<HH", data, 8)
        if total == 0:
            continue
        snaps.setdefault(boot_id, {})[seq] = data[12:]
        d = snaps[boot_id]
        if len(d) < total:
            continue
        body = b"".join(d.get(i, b"") for i in range(total))
        text = body.replace(b"\x00", b"").decode("utf-8", "replace")
        if printed.get(boot_id) == len(text):
            continue  # identical snapshot re-sent
        printed[boot_id] = len(text)
        print("\n===== netlog boot_id=%#010x from %s (%d chunks) ====="
              % (boot_id, addr[0], total), file=sys.stderr)
        for line in text.splitlines():
            if grep is None or grep in line.lower():
                print(line)
        sys.stdout.flush()
except KeyboardInterrupt:
    print("\n[netlog] stopped", file=sys.stderr)
PY
