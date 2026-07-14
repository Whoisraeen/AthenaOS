# AthKit

App development SDK. Rust-first, declarative, SwiftUI-style ergonomics without
the Apple lock-in.

## Goals

- A single `raekit::App` entry point that ties together UI, state, capabilities,
  IPC, and the app lifecycle
- Capability declarations are *types*: the app's permission manifest is the
  set of capability tokens it asks for at compile time
- An IDE on-ramp: `rae new my-app` produces a runnable Hello World in under 30s
- Porting from Mac or Windows is a week, not a quarter — the pitch to skeptical devs

## App bundle format

```
my-app.rae/
├── manifest.toml          # name, version, capabilities, signature
├── bin/                   # compiled binaries per arch
├── resources/             # localized strings, icons, themes
└── deps/                  # hashed dependency tree (no DLL hell)
```

## Surface sketch

```rust
use raekit::prelude::*;
use raekit::caps::{Camera, Microphone};

#[raekit::app]
struct MyApp;

impl App for MyApp {
    type Capabilities = (Camera, Microphone);

    fn scene(&self) -> impl Scene {
        Window::new("Hello").content(HelloView::default())
    }
}
```

The `Capabilities` associated type IS the permission manifest. The OS reads
the type at install time and prompts the user.

## Open design questions

- Macro vs. trait surface — how SwiftUI-y do we get before it stops feeling Rusty?
- Cross-compilation story for shipping to ARM, x86, and RISC-V from one workspace
- Signing flow that's free for indie devs (concept doc promises a year free)
