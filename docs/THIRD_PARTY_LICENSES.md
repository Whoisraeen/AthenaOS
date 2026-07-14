# Third-Party Licenses

AthenaOS may include or adapt components from other open-source projects. This file tracks **substantial** vendored or forked trees.

## Redox OS (MIT)

**Reference clone:** `redox_reference/` (not shipped as-is; used for migration per `.cursor/rules/redox-migration.mdc`)

**Upstream:** [https://github.com/redox-os](https://github.com/redox-os)

**License:** MIT — retain `LICENSE` / `LICENSE-MIT` and copyright headers in any copied crate.

**Typical destinations when migrated:** `components/relibc/`, `components/raefs/` (from redoxfs), userspace drivers, low-level crates.

When a component is first migrated from Redox into the build, add a row below:

| Component path | Upstream crate | Migrated (date/commit) | Notes |
|----------------|----------------|------------------------|-------|
| `components/raefs/src/redoxfs_adapter/` | redoxfs (`tree.rs`, `header.rs`) | 2026-05-28 | MIT; see `components/raefs/vendor/redoxfs/` |
| `components/pcid/` | pciids patterns (subset) | 2026-05-28 | Curated IDs only; not full pciids DB |
| `components/raefat/` | redox-fatfs BPB patterns | 2026-05-28 | Boot-sector layout only; no full FS port |
| `components/raehid/` | `hidreport` (hidutils) via Redox `rehid` | 2026-06-16 | MIT; `raehid` wraps the upstream `hidreport` crate (`default-features=false`, `#![no_std]`) — the same HID report-descriptor parser Redox's `usbhidd`→`rehid` uses. Linked into the kernel for report-protocol HID decoding. Pulls `hidreport` (MIT) + `thiserror` 2.0 (MIT/Apache-2.0) from crates.io. |

## Kanata (mixed: MIT + LGPL-3.0)

**Reference:** Vendored into `components/kanata_daemon/vendor/`

**Upstream:** [https://github.com/jtroo/kanata](https://github.com/jtroo/kanata)

The Kanata project is itself a multi-license tree. We vendored two of its crates and they carry **different** licenses — that matters for what we can link where:

| Vendored crate                                        | License        | Currently linked? | Notes |
|-------------------------------------------------------|----------------|-------------------|-------|
| `components/kanata_daemon/vendor/kanata-keyberon/`    | **MIT**        | yes (layout engine) | Fork of TeXitoi's `keyberon`. `no_std`-clean. Used as the daemon's actual key-event state machine. |
| `components/kanata_daemon/vendor/kanata-parser/`      | **LGPL-3.0**   | **no — gated off** | `.kbd` config file parser. Requires `std`; daemon Cargo.toml comment marks it disabled until Phase 11 AthBridge brings full `std` userspace. |

**Linkage policy.** `kanata_daemon` is — and must remain — a **separate userspace ELF**, talking to the kernel only over capability-IPC. The kernel itself never links any kanata crate (verify with `grep kanata kernel/Cargo.toml` — must be empty). This keeps the LGPL-3.0 obligation scoped to the daemon binary: the kernel + first-party userspace can stay MIT/Apache-style, and the daemon ships its own `LICENSE` (LGPL-3.0) preserved verbatim at `components/kanata_daemon/LICENSE`.

**`LICENSE` preservation.** Both vendored crates carry their original upstream `LICENSE` files in place — verified at build of this row. Do not delete or rewrite them; the parser's LGPL grant in particular requires the license text to travel with the source.

**Status row (mirrors the MIT table format):**

| Component path | Upstream source | Migrated (date/commit) | Notes |
|----------------|-----------------|------------------------|-------|
| `components/kanata_daemon/`                       | kanata 1.x (jtroo)        | 2026-05-28 | Userspace ELF; LGPL-3.0 inherited from parser, MIT-only at runtime today (parser gated off). |
| `components/kanata_daemon/vendor/kanata-keyberon/`| keyberon (TeXitoi → kanata fork) | 2026-05-28 | MIT, `no_std`, key event state machine. |
| `components/kanata_daemon/vendor/kanata-parser/`  | kanata-parser             | 2026-05-28 | LGPL-3.0, vendored for future `.kbd` config parsing; **not linked yet**. |

## AMD GPU microcode (proprietary, redistributable)

| Component path | Upstream source | Migrated (date/commit) | Notes |
|----------------|-----------------|------------------------|-------|
| `firmware/amdgpu/*.bin` (12 Phoenix1 blobs) | linux-firmware `amdgpu/` (GitLab mirror, main) | 2026-06-12 | Binary-only redistribution explicitly granted by `firmware/amdgpu/LICENSE.amdgpu` (travels with the blobs). Never modified, never linked — served to the userspace `amdgpud` daemon via `request_firmware` (syscall 142) and loaded onto the GPU. Refresh from the same upstream path; kernel.org cgit bot-blocks downloads, use the GitLab mirror. |
