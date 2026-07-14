# RaeSync

Optional cross-device sync. End-to-end encrypted. Requires RaeID, and is off
by default until the user opts in.

## What syncs

- Per-app preferences (apps choose to participate)
- Window manager layouts and themes (Vibe Mode, custom themes)
- Hardware profiles (keyboard, mouse, controller bindings; per-device fan curves
  and RGB profiles)
- Bookmarks, recent files (opt-in per data class)
- Saved games (per-game; deduped; conflict-resolution UI when needed)

## What does *not* sync

- Anything where the user has explicitly opted out
- Capability grants (capabilities are per-device by design)
- Per-machine performance tunes (CPU power limits, etc.)

## Crypto

- Per-user master key derived on device from passkey + device entropy
- Per-data-class subkeys (HKDF-derived) so leaking a single class doesn't leak the rest
- Server stores ciphertext + minimal metadata (no plaintext keys, no readable filenames)
- Restore from another device uses passkey handshake; recovery code is the only
  out-of-band fallback

## Open design questions

- Saved-game conflict resolution UI — three-way merge for text saves, "pick a version"
  for binary
- LAN-only sync mode for the privacy-maximalist user
- Whether to expose the sync server as self-hostable from day one
