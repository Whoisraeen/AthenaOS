# AthPlay

Built-in game launcher and library aggregator. One library across Steam, Epic, GOG,
AthStore, and direct installs. Powers GameOS Mode.

## Goals

- One library: every installed game from every storefront, in one list, sortable
  and searchable
- Per-game profiles: resolution, refresh rate, HDR mode, audio device, GPU power
  limit, NULL_LATENCY toggle — saved and auto-applied
- Game Bar overlay (FPS, frametime, temps, voice chat, screenshots, capture) — native
  and fast, not the Windows joke
- Capture & stream at the compositor: zero-cost recording
- DualSense / Xbox / generic controller config in one place

## GameOS Mode

A different shell, same OS. Couch UI, controller-first. Toggleable from the desktop
shell. Same library, same saves, same accounts.

## Layering

- **raeplay-library**: catalog across providers; sourced from Steam manifests, Epic
  manifests, AthStore, and direct installs.
- **raeplay-profiles**: per-game saved profiles, applied by the compositor and AthAudio.
- **raeplay-overlay**: the in-game overlay, drawn by the compositor as a system
  layer above the game's swapchain.
- **raeplay-capture**: video encode pipeline tied to the compositor's framebuffer
  (no readback overhead).
- **raeplay-gameos**: alternate shell, swapped in via the swappable-shells design.

## Open design questions

- Storefront API stability — how much do we lean on official APIs vs. parsing
  manifests on disk?
- Cloud save aggregation policy (per-store accounts vs. AthSync unification)
- Achievement aggregation — show in-OS, or defer to each store?
