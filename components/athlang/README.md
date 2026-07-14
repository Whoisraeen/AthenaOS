# athlang — the AthenaOS scripting language

> *"Scripting layer — Swift scripts for automation, no PowerShell archaeology
> required."* — LEGACY_GAMING_CONCEPT.md, §Customization Engine

**athlang** (surface name: **Rae script**) is AthenaOS's first-class automation
language: a small, Swift-flavored, capability-sandboxed scripting language
implemented from scratch in Rust. One interpreter — this crate — runs in two
places:

- **In the kernel** (`kernel/src/scripting.rs`): sources up to 64 KiB execute
  inline at submit, fuel-limited, with system bindings gated on a
  user-authorized capability mask.
- **In userspace** (`athlangd/`): a daemon drains queued larger scripts through
  the same interpreter.

`no_std` + `alloc`, zero dependencies, host-testable: `cargo test -p athlang`.

---

## Why not just ship a real language?

The same reason AthenaOS doesn't ship ext4 or Wayland: the existing options
drag in the wrong architecture. A Python/JS runtime is megabytes of GC and
attack surface that can't run in-kernel and can't be fuel-metered. AppleScript
is half-deprecated; PowerShell is its own forbidden cuneiform. Rae script is
built around three properties the OS actually needs from automation:

1. **Deterministic termination.** The interpreter is fuel-limited: every
   statement, loop iteration, call, and host call burns fuel. `while true {}`
   ends in a `Timeout`, never a hung kernel. There is no way to write a script
   that wedges its host.
2. **Capability sandboxing (AthGuard model).** A script can compute anything
   but *touch* nothing by default. Every system binding is gated on a
   `cap_mask` bit the **user** grants at submit. A denied call fails the whole
   script closed — never a silent no-op.
3. **One implementation, everywhere.** The same crate compiles into the
   kernel, the daemon, and the host test suite. There is no "kernel dialect."

## Language tour

Swift-flavored on purpose — familiar to the largest pool of app developers the
Concept courts, without dragging in the Swift runtime.

```swift
// Bindings: let is immutable (enforced, including through paths), var isn't.
let name = "Athena"
var count = 0

// String interpolation.
print("hello \(name), \(2 + 3) things")

// Ranges + for/while, break/continue.
for i in 0..<10 { count = count + i }       // exclusive: 0..9
for i in 1...10 { count = count + i }       // inclusive: 1..10
while count > 0 {
    if count % 2 == 0 { count = count - 3; continue }
    if count < 10 { break }
    count = count - 1
}

// Arrays + dictionaries (String keys), with path assignment.
var scores = ["ada": 95, "grace": 99]
scores["linus"] = 87
var grid = [[1, 2], [3, 4]]
grid[1][0] = 30

// Floats. Int-Float arithmetic promotes to Float.
let ratio = 7.0 / 2.0            // 3.5
let scaled = 3 * 1.5             // 4.5

// Functions (recursion capped), first-class and passable.
func fib(n) {
    if n < 2 { return n }
    return fib(n - 1) + fib(n - 2)
}

// Closures — Swift syntax, capture-by-value snapshot, implicit return
// of a trailing expression.
let double = { x in x * 2 }
let evens = [1, 2, 3, 4].filter({ x in x % 2 == 0 }).map(double)
let sum = evens.reduce(0, { a, b in a + b })

// Minimal structs: declare fields, construct positionally, mutate via var.
struct Point { x, y }
var p = Point(1, 2)
p.x = 5

// A top-level `return <Int>` becomes the script's exit code.
return fib(10)
```

### Values

| Type | Literals / notes |
|---|---|
| `Int` | `42`, 64-bit, wrapping arithmetic |
| `Float` | `1.5`; mixed Int/Float math promotes to Float |
| `Bool` | `true` / `false` |
| `String` | `"hi \(expr)"` interpolation, `\n` `\t` `\\` `\"` escapes |
| `Array` | `[1, 2, 3]`, heterogeneous, `+` concatenates |
| `Dictionary` | `["k": v]`, empty `[:]`, String keys, sorted iteration |
| `Range` | `0..<10` exclusive, `1...10` inclusive (Int bounds) |
| `Function` | `func` declarations and closure literals `{ x in … }` |
| `Struct` | `struct Name { a, b }` + positional constructor `Name(1, 2)` |
| `Unit` | `()` — what `print` returns; missing dict values, empty `.first` |

### Methods and properties

| Receiver | Surface |
|---|---|
| `String` | `.count` `.isEmpty` `.uppercased()` `.lowercased()` `.trimmed()` `.contains(s)` `.hasPrefix(s)` `.hasSuffix(s)` `.split(sep)` |
| `Array` | `.count` `.isEmpty` `.first` `.last` `.contains(v)` `.reversed()` `.joined(sep)` `.map(f)` `.filter(f)` `.reduce(init, f)` — mutating: `.append(v)` `.removeLast()` |
| `Dictionary` | `.count` `.isEmpty` `.keys` `.values` `.hasKey(k)` — mutating: `.remove(k)` |
| `Range` | `.count` |

Mutating methods require a `var`-rooted receiver — `let a = [1]` rejects both
`a[0] = 2` and `a.append(2)` with `AssignToLet`, exactly like Swift.

Global builtins: `print(…)` (captured, newline-joined, returned to the
submitter), `String(x)`, `Int(x)`, `Float(x)`, `abs(x)`, `min(a,b)`, `max(a,b)`.

### Iteration

`for x in …` walks ranges, arrays, dictionary **keys** (sorted), and string
characters. Loop variables are immutable per iteration.

### Semantics worth knowing

- **Closures capture by value** — a snapshot of every visible binding at
  creation. Mutations inside a closure don't leak out. Deterministic and
  allocation-bounded by design (kernel-friendly), not a GC'd environment.
- **Implicit return**: a function/closure body whose last statement is an
  expression returns it (`{ x in x * 2 }` needs no `return`).
- **Fuel**: statements, loop iterations, calls, and per-element `map`/`filter`/
  `reduce` work each cost 1. Kernel inline budget: 1M. Daemon budget: 10M.
  Recursion depth caps at 64.
- Comments are `// line comments`.

## Embedding API

```rust
// Pure computation:
let outcome = athlang::run(source, fuel)?;
// outcome.exit_code (top-level `return <Int>`), outcome.output (print text),
// outcome.steps (fuel consumed)

// With system bindings:
struct MyHost { /* … */ }
impl athlang::Host for MyHost {
    fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, HostError> {
        match name {
            "beep" => { /* … */ Ok(Value::Unit) }
            _ => Err(HostError::Unknown),          // → UndefinedFunction
        }
    }
}
let outcome = athlang::run_with_host(source, fuel, &mut MyHost { … })?;
```

Any call a script makes that isn't script-defined, a struct constructor, or a
builtin is offered to the `Host`. Three refusal shapes, three meanings:

| `HostError` | Script-visible result | Meaning |
|---|---|---|
| `Unknown` | `UndefinedFunction` | Host doesn't export this name |
| `Denied(msg)` | `CapabilityDenied` — script fails closed | Exported, but the cap_mask doesn't grant it |
| `Failed(msg)` | `HostFailed` | Granted, attempted, system op failed |

## The kernel surface (system bindings)

Submitting a script authorizes it with a `cap_mask` (deny-by-default; the bits
live in `ath_abi::syscall::SCRIPT_CAP_*`):

| Binding | Cap bit | Backs onto |
|---|---|---|
| `uptimeMs()` `wallClock()` `windowCount()` `osVersion()` | `SCRIPT_CAP_SYSINFO` (1) | boot clock, tray clock, compositor |
| `notify(title)` | `SCRIPT_CAP_NOTIFY` (2) | notification center |
| `getAccent()` `setAccent(argb)` | `SCRIPT_CAP_THEME` (4) | theme engine (re-skins the shell) |
| `getConfig(key)` `setConfig(key, v)` | `SCRIPT_CAP_CONFIG` (8) | versioned config registry (snapshot/rollback like all settings) |
| `setWallpaper(name)` | `SCRIPT_CAP_WALLPAPER` (16) | live wallpaper engine |
| `launchApp(path)` | `SCRIPT_CAP_LAUNCH` (32) | VFS app spawn |

A one-line theme macro, end to end:

```swift
setAccent(4294901760)                 // ARGB red
setConfig("/scripting/last_vibe", "red")
notify("Vibe applied: \(getConfig("/scripting/last_vibe"))")
```

## Lifecycle (kernel + daemon)

Syscalls (`docs/SYSCALL_TABLE.md` Block 14; constants in `ath_abi`):

| nr | syscall | role |
|---:|---|---|
| 78 | `SYS_SCRIPT_RUN(src, len, cap_mask)` | submit → script id. ≤64 KiB runs inline (fuel 1M) before returning; larger queues for the daemon |
| 79 | `SYS_SCRIPT_STATUS(id, out, cap)` | `ScriptAbi` (state/exit/…); a buffer larger than 56 bytes also receives the captured `print` output |
| 80 | `SYS_SCRIPT_KILL(id)` | kill a queued/running script |
| 294 | `SYS_SCRIPT_FETCH(out, cap)` | **athlangd**: claim the next queued job (id + cap_mask + source) |
| 295 | `SYS_SCRIPT_COMPLETE(id, exit, out, len)` | **athlangd**: report exit + output (negative exit = Failed) |

States: `Queued → Running → Completed | Failed | Killed | Timeout`.
`/proc/athena/scripts` lists every script with state, exit code, cap_mask, and
source hash.

`athlangd` runs the same interpreter with a per-job bump arena (reset between
jobs — flat memory footprint) and a 10M fuel budget. Daemon-side bindings
cover what has a syscall route today (`uptimeMs`, `getConfig`, `setConfig`);
the rest honestly report they're inline-only rather than no-opping.

## Invoking scripts

- **Shell**: `rae <file>` in `ath-sh` — runs the file under the full cap mask
  (you at your own shell are the authorizer), prints output + exit state.
- **Command palette**: the *Run Rae Script* action executes the source stored
  at config key `/scripting/palette_script` and toasts its first output line.
- **Syscall**: `SYS_SCRIPT_RUN` from any app (`athkit::sys::script_run`),
  authorized by whatever cap_mask the calling surface grants.

## Proof (how we know it works)

- **Host KATs**: `cargo test -p athlang` — 41 tests, each FAIL-able, covering
  the v0.1 regression suite plus floats, collections, closures/capture,
  structs, path-assignment immutability, control flow, and the Host trait
  (granted / denied-fails-closed / unknown).
- **Boot smoketest** (QEMU CI + iron): `[scripting] smoketest: …` runs a real
  script through the full lifecycle and asserts, among others,
  `cap_granted=true` (a config-registry roundtrip through script bindings),
  `cap_denied_closed=true` (the same call under `cap_mask=0` fails closed),
  and `daemon_fetch_complete=true` (the FETCH/COMPLETE protocol).
- **Daemon proof**: `user_init` submits a >64 KiB script at boot and polls
  until `athlangd` completes it (serial sentinel `8860`).

## Non-goals (v0.2)

No GC (arena/fuel model instead), no reference semantics or `inout`, no
threads, no exceptions (errors fail the script with a typed `RaeError`), no
imports/modules yet, no type annotations. The next planned slices are listed
in `MasterChecklist.md` (§Customization Engine).
