#!/usr/bin/env python3
"""QMP-driven QEMU screenshot harness for RaeenOS (ADR 0004).

Boots a built disk image headlessly, waits for a serial sentinel (plus a
settle delay for the compositor), then captures the framebuffer via the QMP
`screendump` command with format=png (QEMU 7.1+; avoids the PPM->PNG striping
artifact documented in project memory). Lead-run only (subagent Bash is
sandboxed); hands the PNG to raeen-visual-qa.

Usage:
  python tools/screenshot_qemu.py --image <bios.img> --out shot.png \
      [--marker "System successfully booted"] [--settle 6] [--timeout 90] \
      [--qmp-port 5599] [--smp 2]
"""
import argparse, json, os, socket, subprocess, sys, time

QEMU = os.environ.get(
    "RAEEN_QEMU",
    "/c/Program Files/qemu/qemu-system-x86_64.exe",
)


def qmp_send(sock, obj):
    sock.sendall((json.dumps(obj) + "\r\n").encode())


def qmp_recv(sock, timeout=10):
    sock.settimeout(timeout)
    buf = b""
    while b"\n" not in buf:
        chunk = sock.recv(4096)
        if not chunk:
            break
        buf += chunk
    # return the first complete JSON line
    line, _, rest = buf.partition(b"\n")
    return json.loads(line.decode()) if line.strip() else {}


def wait_for_marker(serial_path, marker, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with open(serial_path, "r", errors="ignore") as f:
                if marker in f.read():
                    return True
        except FileNotFoundError:
            pass
        time.sleep(0.5)
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--image", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--marker", default="System successfully booted")
    ap.add_argument("--settle", type=float, default=6.0)
    ap.add_argument("--timeout", type=float, default=90.0)
    ap.add_argument("--qmp-port", type=int, default=5599)
    ap.add_argument("--smp", default="2")
    ap.add_argument("--uefi", action="store_true")
    args = ap.parse_args()

    serial_path = os.path.join(
        os.environ.get("TEMP", "/tmp"), "raeen-screenshot-serial.log"
    )
    try:
        os.remove(serial_path)
    except OSError:
        pass

    qemu_args = [
        QEMU,
        "-drive", f"format=raw,file={args.image}",
        "-m", "2G",
        "-smp", str(args.smp),
        "-display", "none",
        "-no-reboot",
        "-qmp", f"tcp:127.0.0.1:{args.qmp_port},server,nowait",
        "-serial", f"file:{serial_path.replace(os.sep, '/')}",
    ]
    if args.uefi:
        # UEFI needs OVMF; the bios image is simpler for a screenshot probe.
        ovmf = os.environ.get("RAEEN_OVMF")
        if ovmf:
            qemu_args += ["-bios", ovmf]

    print(f"[shot] launching QEMU: {' '.join(qemu_args)}", flush=True)
    proc = subprocess.Popen(qemu_args)
    out_abs = os.path.abspath(args.out).replace(os.sep, "/")
    try:
        if not wait_for_marker(serial_path, args.marker, args.timeout):
            print(f"[shot] WARN: marker '{args.marker}' not seen in "
                  f"{args.timeout}s; capturing anyway", flush=True)
        else:
            print(f"[shot] marker seen; settling {args.settle}s for compositor",
                  flush=True)
        time.sleep(args.settle)

        # Connect QMP (server is up once QEMU started listening).
        sock = None
        for _ in range(40):
            try:
                sock = socket.create_connection(("127.0.0.1", args.qmp_port), 2)
                break
            except OSError:
                time.sleep(0.25)
        if sock is None:
            print("[shot] FAIL: could not connect QMP", flush=True)
            return 2
        greeting = qmp_recv(sock)
        print(f"[shot] QMP greeting: {bool(greeting.get('QMP'))}", flush=True)
        qmp_send(sock, {"execute": "qmp_capabilities"})
        qmp_recv(sock)
        qmp_send(sock, {"execute": "screendump",
                        "arguments": {"filename": out_abs, "format": "png"}})
        resp = qmp_recv(sock, timeout=20)
        if "error" in resp:
            # Older fallback: no format arg -> PPM
            print(f"[shot] png screendump error: {resp['error']}; trying ppm",
                  flush=True)
            ppm = out_abs.rsplit(".", 1)[0] + ".ppm"
            qmp_send(sock, {"execute": "screendump",
                            "arguments": {"filename": ppm}})
            resp = qmp_recv(sock, timeout=20)
            print(f"[shot] ppm screendump resp: {resp}", flush=True)
            out_abs = ppm
        qmp_send(sock, {"execute": "quit"})
        sock.close()
    finally:
        time.sleep(0.5)
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

    if os.path.exists(out_abs):
        sz = os.path.getsize(out_abs)
        print(f"[shot] OK: {out_abs} ({sz} bytes)", flush=True)
        return 0
    print("[shot] FAIL: no output file produced", flush=True)
    return 3


if __name__ == "__main__":
    sys.exit(main())
