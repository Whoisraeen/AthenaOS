# AthenaOS procfs introspection

**Authoritative source.** Required by `kernelchecklist.md` R10: every new
`/proc/athena/<name>` entry MUST add a row here in the same commit.

Every subsystem visible at boot MUST expose a text-dump endpoint under
`/proc/athena/`. Reading it should require no parsing — humans `cat`,
machines parse-by-line if they want. No binary blobs.

## Index

`cat /proc/athena/index` lists every endpoint below.

## Endpoints

| Path | Subsystem | Source file | Contents |
|---|---|---|---|
| `/proc/athena/index`       | meta              | `procfs.rs::proc_athena_index`        | List of all endpoints |
| `/proc/athena/boot`        | boot benchmark    | `procfs.rs::proc_athena_boot`         | T0→userspace elapsed ms vs concept-doc target |
| `/proc/athena/gaming`      | game session      | `procfs.rs::proc_athena_gaming`       | Game mode counters, deadline stats |
| `/proc/athena/config`      | config registry   | `procfs.rs::proc_athena_config`       | Versioned config tree dump |
| `/proc/athena/search`      | search index      | `procfs.rs::proc_athena_search`       | Query-latency stats |
| `/proc/athena/games`       | per-game profiles | `procfs.rs::proc_athena_games`        | Stored profiles + last applied |
| `/proc/athena/rgb`         | unified RGB       | `procfs.rs::proc_athena_rgb`          | Devices + per-zone color state |
| `/proc/athena/bundles`     | app bundles       | `procfs.rs::proc_athena_bundles`      | Installed components + verify stats |
| `/proc/athena/perm`        | permission queue  | `procfs.rs::proc_athena_perm`         | Pending permission prompts |
| `/proc/athena/themes`      | theme engine      | `procfs.rs::proc_athena_themes`       | Built-in + registered themes + current |
| `/proc/athena/scripts`     | scripting         | `procfs.rs::proc_athena_scripts`      | Rae script lifecycle state |
| `/proc/athena/wireguard`   | WireGuard         | `procfs.rs::proc_athena_wireguard`    | Tunnel registry + handshake stats |
| `/proc/athena/wallpaper`   | live wallpaper    | `procfs.rs::proc_athena_wallpaper`    | Current wallpaper + occlusion stats |
| `/proc/athena/caps`        | capabilities      | `procfs.rs::proc_athena_caps`         | Capability audit log (grant/revoke/use) |
| `/proc/athena/memory`      | memory mgmt       | `procfs.rs::proc_athena_memory`       | Heap base/size, pinned pages |
| `/proc/athena/sched_stats` | scheduler         | `procfs.rs::proc_athena_sched_stats`  | Deadline counters, game-mode flags |
| `/proc/athena/compositor`  | compositor        | `procfs.rs::proc_athena_compositor`   | Resolution, surface counters |
| `/proc/athena/cpu`         | CPU features      | `procfs.rs::proc_athena_cpu`          | Vendor, features, Zen 4 detect |
| `/proc/athena/hardening`   | kernel hardening  | `procfs.rs::proc_athena_hardening`    | SMEP/SMAP/KASLR status |
| `/proc/athena/windows_gap` | pain-point map    | `procfs.rs::proc_athena_windows_gap`  | Concept §Windows Pain Points rows |
| `/proc/athena/clipboard`   | clipboard         | `procfs.rs::proc_athena_clipboard`    | Session clipboard stats |
| `/proc/athena/storage_irq` | storage IRQ       | `procfs.rs::proc_athena_storage_irq`  | MSI-X vs INTx per controller |
| `/proc/athena/syscall_guard` | syscall hardening | `procfs.rs::proc_athena_syscall_guard` | Pointer/bounds/cap rejection counters + active limits |
| `/proc/athena/linux_kabi`   | Linux kABI scaffold | `procfs.rs::proc_athena_linux_kabi` | Symbol counts by category/status (not GPL code) |

## Conventions

- **First line** is `# AthenaOS <subsystem> <summary>` — humans see what
  they're reading at a glance.
- **No tabs** in output — use spaces, predictable column widths.
- **No timestamps in absolute form** in the dump body — use TSC deltas
  or "since boot" counters so dumps are reproducible.
- **No PII** — process names are OK, user account strings are not.
- **Dump must complete in <10 ms** to stay snappy in the Settings →
  Diagnostics panel.

## Adding a new endpoint

1. Implement `dump_text() -> String` on the module.
2. Add a `pub fn proc_athena_<name>() -> String` to `procfs.rs` that calls it.
3. Add a row in this table.
4. Add a line in `procfs.rs::proc_athena_index()`.
5. Mention it in the module's docstring.
