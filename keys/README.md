# RaeenOS signing keys

`dev-signing.key` (32-byte Ed25519 seed) + `dev-signing.pub` (32-byte public
key) are the **development** app-bundle signing keypair.

- xtask generates the pair on first build and keeps `.pub` in lockstep with
  the seed (`xtask::ensure_signing_keys`).
- xtask signs every staged `apps/<name>/RaeManifest.toml` (with the built
  ELF's `elf_sha256` injected) and bundles the detached `RaeManifest.sig`.
- The kernel embeds `dev-signing.pub` (`kernel/src/rae_manifest.rs`) and
  verifies bundle signatures + ELF hashes at app launch.

**The seed is deliberately committed.** This is the "free signing for every
dev build" trust root (Concept §Developer onramp) — it authenticates nothing
beyond "built by someone with this repo". It provides tamper-evidence and
exercises the full verify path end-to-end; it is NOT a production secret.
The production chain (HSM-held store keys, per-developer certificates,
secure-boot kernel signing — MasterChecklist Phase 3.7 / 9.2) replaces it,
at which point this pair is rotated out.
