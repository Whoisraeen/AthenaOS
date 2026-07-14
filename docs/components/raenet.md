# RaeNet

Userspace networking above L3. L2/L3 stays in-kernel for hot-path throughput;
TCP, UDP, QUIC, TLS, WireGuard, and the gaming traffic shaper live in user space.

## Goals

- Built-in WireGuard, no third-party clients
- QUIC priority — the OS knows which sockets are game traffic and prioritizes them
  at the local NIC's QoS queues
- Gaming traffic shaping: bufferbloat-resistant defaults (CAKE-style fq_codel),
  per-app bandwidth budgets when on hotspot tether
- Modern TLS by default (TLS 1.3 minimum); ChaCha20-Poly1305 in software,
  AES-GCM with AES-NI / SHA-NI / VAES where present
- Captive portal handling that doesn't break VPNs

## Non-goals

- Replacing every userspace TCP library — apps can still use their own
- Datacenter networking features (BGP, MPLS, etc.)

## Layering

```
app sockets (POSIX-compatible)
  ↓
raenet-runtime (TCP/UDP/QUIC, in user space, per-app context)
  ↓
raenet-vpn (WireGuard, optionally always-on)
  ↓
raenet-shaper (fq_codel + game-priority queue)
  ↓
kernel L2/L3 + driver (IOMMU-sandboxed NIC driver)
```

## Open design questions

- Per-app routing UI: implicit, or a Little-Snitch-style flow inspector?
- Game traffic detection: cooperative (apps declare) vs. inferred (port + DPI)?
- Tether-aware shaping: heuristic or explicit "I'm tethered" toggle?
