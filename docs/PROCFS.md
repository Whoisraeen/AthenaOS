# RaeenOS procfs introspection

**Authoritative source.** Required by `kernelchecklist.md` R10: every new
`/proc/raeen/<name>` entry MUST add a row here in the same commit.

Every subsystem visible at boot MUST expose a text-dump endpoint under
`/proc/raeen/`. Reading it should require no parsing — humans `cat`,
machines parse-by-line if they want. No binary blobs.

## Index

`cat /proc/raeen/index` lists every endpoint below.

## Endpoints

| Path | Subsystem | Source file | Contents |
|---|---|---|---|
| `/proc/raeen/index`       | meta              | `procfs.rs::proc_raeen_index`        | List of all endpoints |
| `/proc/raeen/boot`        | boot benchmark    | `procfs.rs::proc_raeen_boot`         | T0→userspace elapsed ms vs concept-doc target |
| `/proc/raeen/gaming`      | game session      | `procfs.rs::proc_raeen_gaming`       | Game mode counters, deadline stats |
| `/proc/raeen/config`      | config registry   | `procfs.rs::proc_raeen_config`       | Versioned config tree dump |
| `/proc/raeen/search`      | search index      | `procfs.rs::proc_raeen_search`       | Query-latency stats |
| `/proc/raeen/games`       | per-game profiles | `procfs.rs::proc_raeen_games`        | Stored profiles + last applied |
| `/proc/raeen/rgb`         | unified RGB       | `procfs.rs::proc_raeen_rgb`          | Devices + per-zone color state |
| `/proc/raeen/bundles`     | app bundles       | `procfs.rs::proc_raeen_bundles`      | Installed components + verify stats |
| `/proc/raeen/perm`        | permission queue  | `procfs.rs::proc_raeen_perm`         | Pending permission prompts |
| `/proc/raeen/themes`      | theme engine      | `procfs.rs::proc_raeen_themes`       | Built-in + registered themes + current |
| `/proc/raeen/scripts`     | scripting         | `procfs.rs::proc_raeen_scripts`      | Rae script lifecycle state |
| `/proc/raeen/wireguard`   | WireGuard         | `procfs.rs::proc_raeen_wireguard`    | Tunnel registry + handshake stats |
| `/proc/raeen/wallpaper`   | live wallpaper    | `procfs.rs::proc_raeen_wallpaper`    | Current wallpaper + occlusion stats |
| `/proc/raeen/caps`        | capabilities      | `procfs.rs::proc_raeen_caps`         | Capability audit log (grant/revoke/use) |
| `/proc/raeen/memory`      | memory mgmt       | `procfs.rs::proc_raeen_memory`       | Heap base/size, pinned pages |
| `/proc/raeen/sched_stats` | scheduler         | `procfs.rs::proc_raeen_sched_stats`  | Deadline counters, game-mode flags |
| `/proc/raeen/compositor`  | compositor        | `procfs.rs::proc_raeen_compositor`   | Resolution, surface counters |
| `/proc/raeen/cpu`         | CPU features      | `procfs.rs::proc_raeen_cpu`          | Vendor, features, Zen 4 detect |
| `/proc/raeen/hardening`   | kernel hardening  | `procfs.rs::proc_raeen_hardening`    | SMEP/SMAP/KASLR status |
| `/proc/raeen/windows_gap` | pain-point map    | `procfs.rs::proc_raeen_windows_gap`  | Concept §Windows Pain Points rows |
| `/proc/raeen/clipboard`   | clipboard         | `procfs.rs::proc_raeen_clipboard`    | Session clipboard stats |
| `/proc/raeen/storage_irq` | storage IRQ       | `procfs.rs::proc_raeen_storage_irq`  | MSI-X vs INTx per controller |
| `/proc/raeen/syscall_guard` | syscall hardening | `procfs.rs::proc_raeen_syscall_guard` | Pointer/bounds/cap rejection counters + active limits |
| `/proc/raeen/linux_kabi`   | Linux kABI scaffold | `procfs.rs::proc_raeen_linux_kabi` | Symbol counts by category/status (not GPL code) |

## Conventions

- **First line** is `# RaeenOS <subsystem> <summary>` — humans see what
  they're reading at a glance.
- **No tabs** in output — use spaces, predictable column widths.
- **No timestamps in absolute form** in the dump body — use TSC deltas
  or "since boot" counters so dumps are reproducible.
- **No PII** — process names are OK, user account strings are not.
- **Dump must complete in <10 ms** to stay snappy in the Settings →
  Diagnostics panel.

## Adding a new endpoint

1. Implement `dump_text() -> String` on the module.
2. Add a `pub fn proc_raeen_<name>() -> String` to `procfs.rs` that calls it.
3. Add a row in this table.
4. Add a line in `procfs.rs::proc_raeen_index()`.
5. Mention it in the module's docstring.
