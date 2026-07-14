# AthenaOS App Developer Guide

How to build, bundle, sign, and ship an app for AthenaOS. Grounded in the real SDK
(`components/raekit`), manifest parser (`kernel/src/rae_manifest.rs`), bundle format
(`kernel/src/app_bundle.rs`), and signing chain (xtask + `keys/`).

> **Status honesty:** the SDK (`view!` macro, widgets, syscall surface) works; permission
> manifests + Ed25519 signing work end-to-end today; the one-command packager
> (`raekit bundle`) and the AthStore submission flow are **planned** — for now apps are
> bundled by xtask into the initramfs. Each section flags what's real vs planned.

---

## 1. What a AthenaOS app *is*

A bundle — a directory that ships as `apps/<name>/`:

```
apps/myapp/
  Cargo.toml          # builds the app ELF
  RaeManifest.toml     # sandbox level + permissions (REQUIRED)
  RaeManifest.sig      # Ed25519 detached signature (added by the build/signing step)
  src/main.rs          # your code
  assets/              # icons, fonts, data (optional)
```

On disk/store an app is also describable as a flat **`.raeapp` bundle** with a hashed
dependency list (`app_bundle.rs`, `SYS_BUNDLE_VERIFY`) — "DLL hell → explicit, hashed
dependencies." The kernel won't launch an app whose declared deps aren't installed at the
declared hashes.

**Trust chain at launch:** `RaeManifest.sig` → verifies `RaeManifest.toml` → which pins
the ELF's `elf_sha256` → which is checked against the actual binary. Tamper anywhere and
the bundle is rejected (tampered ≠ unsigned).

---

## 2. Three ways to build an app

| Path | Use when | How it runs |
|---|---|---|
| **AthKit (native Rust)** ← recommended | New apps, best integration, declarative UI | ELF tagged `osabi = 0xAE` → native syscalls |
| **Raw native syscalls** | Tiny tools, you want full control | same native ABI, no SDK |
| **Port a POSIX/Linux app (relibc)** | Bringing existing C/Rust software | relibc speaks AthenaOS **native** syscalls under a Linux-looking ELF; `SYS_SPAWN` routes by the `osabi` byte |

AthenaOS dispatches `SYS_SPAWN` on the ELF `osabi` byte: `0xAE` = native, anything else =
the Linux-ABI translation path. So both native and ported apps coexist.

---

## 3. Quickstart — a AthKit app

`apps/myapp/src/main.rs`:

```rust
#![no_std]
#![no_main]

use raekit::{view, App};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Declarative UI tree via the view! macro.
    let _ui = view! {
        VStack {
            Text("Hello, AthenaOS")
            Button("Click me") { App::print_debug(0x1234) }
            Spacer()
        }
    };
    // ... render loop (see §6 for drawing) ...
    App::exit(0)
}
```

The `view!` macro builds a `ViewNode` tree from `VStack`/`HStack`/`ZStack` containers and
`Text`/`Button`/`Spacer`/`Divider` terminals (`components/raekit/src/lib.rs`). State and
data-binding come from `raekit::state` (`State`, `Binding`, `ObservableObject`); layout
from `raekit::layout` (flexbox-style: `ZStack`, `Padding`, `Frame`, `Overlay`).

`apps/calculator/` is a complete worked example (draws with `raegfx::Canvas` into a
surface — see §6).

---

## 4. The app directory & Cargo.toml

`apps/myapp/Cargo.toml` builds a `#![no_std] #![no_main]` ELF that depends on `raekit`
(and `raegfx` if you draw). Add the crate as a workspace member (root `Cargo.toml`) so
xtask builds and bundles it into the initramfs. *(The `raekit new myapp` scaffolder is
planned; today copy `apps/calculator/` as the template.)*

---

## 5. The manifest — `RaeManifest.toml` (REQUIRED)

Parsed at launch by `rae_manifest.rs`. Real schema (from `apps/calculator/RaeManifest.toml`):

```toml
name = "myapp"          # MUST match the bundle dir name (spoof guard)
version = "1.0.0"
sandbox = "app"          # trusted | app | strict

[permissions]
network = true           # socket syscalls (native 121/122 + Linux 41-45/49/50)
devices = false          # raw device claim / DMA / PCI (109/111/113/117/132)
install = false          # disk install class (256/257)
```

Each `[permissions]` key maps **1:1 to a gated syscall class**. A malformed manifest fails
**closed** (drops to the first-party allowlist). Unknown keys/sections are ignored for
forward-compat.

---

## 6. Drawing & UI

- **Surfaces:** `App::surface_create(w, h, user_virt)` gives you a framebuffer mapped at
  `user_virt`; draw into it, then `App::surface_present(id, x, y)` to composite. The
  compositor handles glassmorphism/VRR — you just present.
- **Canvas:** `raegfx::Canvas` wraps a surface with primitives (rects, text via the font
  engine, blits) — see `apps/calculator/src/main.rs`.
- **Input:** `App::read_key()`, `App::poll_mouse()` (and the full input pipeline routes
  focus to you — see `INPUT_PIPELINE.md`).
- **Higher-level:** the AthKit `view!` tree + layout engine is the SwiftUI-style path;
  Skia/wgpu backends are Phase 8 (`raeui.md`).

The app syscall surface (`raekit::syscalls` / `App`): `exit`, `write`, `spawn`,
`pty_*`, `read_key`, `poll_mouse`, `mmap`/`munmap`, `surface_create`/`surface_present`,
`yield_now`. Full numbers in `docs/SYSCALL_TABLE.md`.

---

## 7. Storage, data & limits

- **Per-app data bucket:** an isolated AthFS subtree per app id (`raefs::create_bucket`),
  with a quota. Your app can't see another app's bucket — sandboxing at the FS layer.
- **Memory limit:** a per-bundle address-space cap (`process::set_bundle_memory_limit`);
  `mmap`/`brk` past it return `MemoryLimitExceeded`. Visible at `/proc/raeen/memlimits`.
- **Config:** persistent prefs belong in the versioned `config_registry` (snapshotted,
  rollback-able), not ad-hoc files.

---

## 8. Dependencies without DLL hell

Declare shared deps as `(name, version, sha256)` triples; the bundle's `.raeapp` manifest
lists them (`app_bundle.rs`). At launch `SYS_BUNDLE_VERIFY` either confirms every dep is
installed at the requested hash or reports exactly what's missing/mismatched. No PATH wars,
no "works on my machine." Shared runtimes/frameworks register themselves with the same
triple via `SYS_BUNDLE_REGISTER`.

---

## 9. Sandbox & permission model (what users see)

Your `sandbox` level sets the default posture; `[permissions]` opens specific classes.

| Level | Posture | Gets permission grants? |
|---|---|---|
| `trusted` | Full access (first-party / verified) | n/a |
| `app` | Sandboxed; opt-in per permission | yes, the ones you declare |
| `strict` | Locked down; minimal surface | never receives device/install grants |

**Trust rules (until full store PKI):** a non-first-party `sandbox = "trusted"` is **capped
to `app`** unless the bundle is signed (see §10). `strict` never gets device/install grants.
Sensitive grants surface a **`perm_prompt` consent dialog** to the user at first use, and
are revocable in Settings (`SETTINGS_CATALOG.md` §7/§12). Gated classes today: device
claim/DMA/PCI, network sockets, install; file/IPC/mmap are capability- and
isolation-protected (see `THREAT_MODEL.md` §5).

---

## 10. Signing & developer trust

End-to-end Ed25519 signing **works today**:

- **Build side (xtask):** stages your `RaeManifest.toml` with the built ELF's `elf_sha256`
  injected, signs the staged bytes with the dev key (`keys/`, committed as the dev trust
  root — production HSM/store chain replaces it), and bundles the detached
  `RaeManifest.sig`.
- **Kernel side (`rae_manifest::lookup`):** embeds `keys/dev-signing.pub`, verifies the
  signature with the KAT-proven in-kernel Ed25519, then verifies the ELF hash — the
  `sig → manifest → ELF` chain. Proof line: `[manifest] ... signed=true signed_trust=true
  reject_tamper=true -> PASS`.
- **A verified signature is a sufficient trust root** for `sandbox = "trusted"` (no
  allowlist needed).
- **Unverified developer (sideload):** an unsigned bundle isn't blocked — it runs at `app`
  level with a clear "unverified developer" warning (not a punitive wall). Informed user
  choice, mac-Gatekeeper-style but friendlier.

To sign your own: drop your bundle in the build, and xtask signs it with the dev key. (Your
own developer cert + revocation is the planned store-PKI step.)

---

## 11. Shipping on AthStore  *(planned)*

`raestore` is the store service. The intended flow: `raekit bundle` produces a signed
`.raeapp` → submit to AthStore → automated checks (manifest sanity, declared-permission
review, signature) → store countersign → users install with one click, auto-updated.
First-year developers get **free signing** (build-system support). Today this path is
scaffolding; the kernel-side trust + bundle format it depends on are real.

---

## 12. On-device lifecycle (what happens at launch)

1. `shell_runner::spawn_app_from_vfs` reads the bundle.
2. `rae_manifest::assign_for_spawn` parses + verifies the manifest/signature and assigns
   the sandbox level + permission grants (falls back to the first-party allowlist if no
   valid manifest).
3. `SYS_SPAWN` routes by `osabi` (native `0xAE` vs Linux).
4. The kernel sets up the address space, the per-task TLS (`Task::fs_base`), the data
   bucket, and the memory limit; AthGuard arms the syscall-edge gate for the assigned
   level.
5. App runs; sensitive permissions prompt on first use; everything is auditable
   (`/proc/raeen/sandbox`, `/proc/raeen/manifests`).
6. On exit/crash, the sandbox grants are torn down.

---

## 13. Status at a glance

| Capability | Status |
|---|---|
| AthKit `view!` + widgets + state/layout | 🟡 works (Skia/wgpu backend = Phase 8) |
| Native syscall surface (`App::*`) | ✅ |
| `RaeManifest.toml` parse + sandbox assign | ✅ |
| Permission gating (device/net/install) | ✅ |
| Ed25519 sign → manifest → ELF chain | ✅ |
| Per-app data buckets + memory limits | 🟡 |
| Hashed-dependency verify (`.raeapp`) | 🟡 |
| POSIX port via relibc | 🟡 |
| `raekit bundle` one-command packager | ⬜ planned |
| AthStore submission / review / auto-update | ⬜ planned |
| Unverified-developer warning UI | ⬜ (kernel posture exists; UI pending) |

---

## 14. From-zero checklist

1. Copy `apps/calculator/` → `apps/myapp/`; add it as a workspace member.
2. Write `src/main.rs` (`#![no_std] #![no_main]`, `_start`, AthKit `view!` or `raegfx`).
3. Write `RaeManifest.toml` (name = dir, version, sandbox level, only the permissions you
   need — least privilege).
4. Build via xtask; it bundles into the initramfs and signs with the dev key.
5. Boot QEMU; launch from AthShell; check `/proc/raeen/manifests` shows your bundle
   `signed=true` and the right level.
6. Verify least-privilege: a denied class should log `[sandbox] DENY ... pid=<you>`.
7. (Planned) `raekit bundle` → submit to AthStore.
```
