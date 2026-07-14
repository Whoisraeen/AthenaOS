//! xtask — build automation for AthenaOS (forked from AthenaOS).
//!
//! Usage:
//!   cargo run -p xtask -- build            Build the kernel (debug)
//!   cargo run -p xtask -- build --release  Build the kernel (release)
//!   cargo run -p xtask -- run              Build + launch in QEMU (BIOS)
//!   cargo run -p xtask -- run --release    Build release + launch in QEMU
//!   cargo run -p xtask -- run --uefi       Build + launch in QEMU (UEFI)

use std::path::{Path, PathBuf};
use std::process::{self, Command};

mod recipe;

fn run_cargo_fmt() {
    eprintln!("[xtask] Running cargo fmt...");
    // Inherit the invoking CWD (already the workspace root under `cargo run`)
    // instead of forcing `current_dir(project_root())`: on a OneDrive-backed
    // drive, passing that path as a child process CWD fails CreateProcess with
    // OS error 267 ("The directory name is invalid"). fmt is a cosmetic
    // pre-step — a spawn failure must WARN, never panic the whole build.
    let mut cmd = Command::new("cargo");
    cmd.arg("fmt");
    match cmd.status() {
        Ok(status) if !status.success() => eprintln!("[xtask] Warning: cargo fmt failed."),
        Ok(_) => {}
        Err(e) => eprintln!("[xtask] Warning: could not run cargo fmt ({e}); skipping."),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let subcommand = args.first().map(|s| s.as_str()).unwrap_or("help");
    let release = args.iter().any(|a| a == "--release");
    let uefi = args.iter().any(|a| a == "--uefi");
    let ci = args.iter().any(|a| a == "--ci");
    let no_build = args.iter().any(|a| a == "--no-build");
    // --safe: gate every `BlockDevice::write_sector` (NVMe, AHCI,
    // virtio-blk, the userspace driver framework's MMIO doorbell paths,
    // etc.) behind a kernel-side guard that refuses the write and logs it.
    // For bare-metal smoke-boot on a dev machine that already has another
    // OS on the disk — read paths still work, the kernel still mounts and
    // probes, but no sector ever reaches the device.
    let safe = args.iter().any(|a| a == "--safe");
    // --production: clean, silent consumer boot — the on-screen serial mirror
    // starts OFF (no scrolling boot log; UEFI splash -> desktop). Durable logs
    // (COM1 + bootlog ring -> BOOTLOG.TXT + netlog) stay on. Compose with --safe
    // for a flashable image.
    let production = args.iter().any(|a| a == "--production");

    // --target=<triple> (multi-arch Slice A2): which architecture to build the
    // kernel for. Defaults to x86_64-unknown-none (the live, bootable arch). The
    // aarch64-unknown-none-softfloat triple (ADR 0009) builds the cfg-gated
    // arch::aarch64 backend; that build is EXPECTED to fail on the kernel's
    // remaining x86-isms (the A3-A9 inventory) — it is invokable so the gap can
    // be quantified. A non-x86 target builds ONLY the kernel crate (the user-apps
    // / disk-image steps are x86-only and not yet ported).
    let mut target = "x86_64-unknown-none".to_string();
    if let Some(pos) = args.iter().position(|a| a == "--target") {
        if pos + 1 < args.len() {
            target = args[pos + 1].clone();
        }
    }
    for a in &args {
        if let Some(t) = a.strip_prefix("--target=") {
            target = t.to_string();
        }
    }
    let is_x86 = target == "x86_64-unknown-none";

    let mut disk_profile = "virtio".to_string();
    if let Some(pos) = args.iter().position(|a| a == "--disk") {
        if pos + 1 < args.len() {
            disk_profile = args[pos + 1].clone();
        }
    }
    for a in &args {
        if let Some(disk) = a.strip_prefix("--disk=") {
            disk_profile = disk.to_string();
        }
    }

    let mut boot_artifact = "all".to_string();
    for a in &args {
        if let Some(mode) = a.strip_prefix("--boot=") {
            boot_artifact = mode.to_string();
        }
    }

    // --screenshot=PATH (ADR 0004): in CI mode, once the boot marker lands, give
    // the compositor a settle window then capture a PNG of the framebuffer via the
    // QMP `screendump` command (QEMU 7.1+ format=png — avoids the PPM->PNG striping
    // artifact). Pair with --uefi for a clean 32bpp GOP capture. Hands athena-visual-qa
    // a real image (goal #1 acceptance). Reuses run_qemu's known-good UEFI boot.
    let mut screenshot: Option<String> = None;
    for a in &args {
        if let Some(p) = a.strip_prefix("--screenshot=") {
            screenshot = Some(p.to_string());
        }
    }

    match subcommand {
        "build-port" => {
            let port_name = args.get(1).unwrap_or_else(|| {
                eprintln!("Usage: cargo run -p xtask -- build-port <port_name>");
                process::exit(1);
            });
            build_port(port_name);
        }
        "build" => {
            run_cargo_fmt();
            if !is_x86 {
                // Non-x86 (aarch64 Slice A2): kernel-only build. The user-apps /
                // relibc / disk-image steps are x86-only and not yet ported, and
                // the aarch64 kernel build itself is EXPECTED to fail on the
                // remaining x86-isms — the point is to surface + quantify that gap.
                eprintln!(
                    "[xtask] --target {target}: kernel-only build (multi-arch Slice A2). \
                     User-apps + disk-image steps are x86-only; the aarch64 kernel \
                     build is EXPECTED to fail on remaining x86-isms (A3-A9 gap)."
                );
                build_kernel(release, safe, production, &target);
            } else {
                build_user_apps(release);
                build_kernel(release, safe, production, &target);
                if safe {
                    eprintln!("[xtask] --safe: kernel built with `safe_mode` feature; every BlockDevice::write_sector will refuse on this image.");
                }
                match boot_artifact.as_str() {
                    "bios" => {
                        create_disk_image(release, false);
                    }
                    "uefi" => {
                        create_disk_image(release, true);
                    }
                    _ => {
                        create_disk_image(release, false);
                        create_disk_image(release, true);
                    }
                }
            }
        }
        "run" => {
            run_cargo_fmt();
            if !is_x86 {
                // The aarch64 QEMU run path (qemu-system-aarch64 -M virt -kernel)
                // lands with Slice A3 (first boot). Slice A2 only builds.
                eprintln!(
                    "[xtask] --target {target}: `run` is not yet wired for non-x86 \
                     (the qemu-system-aarch64 -M virt -kernel path is Slice A3). \
                     Building the kernel only (Slice A2)."
                );
                build_kernel(release, safe, production, &target);
            } else {
                if !no_build {
                    build_user_apps(release);
                    build_kernel(release, safe, production, &target);
                }
                if safe {
                    eprintln!("[xtask] --safe: kernel built with `safe_mode` feature; every BlockDevice::write_sector will refuse on this image.");
                }
                let image_path = if no_build {
                    let profile = if release { "release" } else { "debug" };
                    let flavor = if uefi { "uefi" } else { "bios" };
                    let path = project_root()
                        .join("target/x86_64-unknown-none")
                        .join(profile)
                        .join(format!("kernel.{flavor}.img"));
                    if !path.is_file() {
                        eprintln!("[xtask] --no-build image missing: {}", path.display());
                        process::exit(1);
                    }
                    path
                } else {
                    create_disk_image(release, uefi)
                };
                run_qemu(&image_path, uefi, ci, &disk_profile, screenshot.as_deref());
            }
        }
        "deploy-ventoy" => {
            let drive = args
                .get(1)
                .expect("Usage: cargo run -p xtask -- deploy-ventoy <drive_letter>");
            run_cargo_fmt();
            build_user_apps(release);
            build_kernel(release, safe, production, &target);
            if safe {
                eprintln!("[xtask] --safe: kernel built with `safe_mode` feature; every BlockDevice::write_sector will refuse on this image.");
            }
            let image_path = create_disk_image(release, uefi);
            deploy_ventoy(&image_path, drive);
        }
        "gpu-test" => {
            run_gpu_test();
        }
        "dist" => {
            // Phase 16.2 — the one-shot signed release/installer bundle.
            // Always release+UEFI (the shippable shape); `--safe` builds a
            // dry-run installer (every sector write refused) for rehearsals.
            run_cargo_fmt();
            build_user_apps(true);
            // The dist kernel carries `installer_image`: it boots straight
            // into the install wizard. Appended via the existing env
            // passthrough so build_kernel's 4 call sites stay untouched.
            let mut feats = std::env::var("RAEEN_KERNEL_FEATURES").unwrap_or_default();
            if !feats.is_empty() {
                feats.push(',');
            }
            feats.push_str("kernel/installer_image");
            std::env::set_var("RAEEN_KERNEL_FEATURES", feats);
            build_kernel(true, safe, production, &target);
            let image_path = create_disk_image(true, true);
            build_dist_bundle(&image_path, safe);
        }
        _ => {
            eprintln!("AthenaOS xtask — build automation");
            eprintln!();
            eprintln!("Usage: cargo run -p xtask -- <build|run|deploy-ventoy> [--release] [--uefi] [--ci] [--no-build] [--safe] [--production] [--disk=<nvme|ata|virtio|smoketest>] [--boot=all|bios|uefi]");
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  build           Build the kernel and create a bootable disk image");
            eprintln!("  run             Build + launch in QEMU");
            eprintln!("  deploy-ventoy   Build + deploy ISO to Ventoy USB drive");
            eprintln!("  dist            Signed release/installer bundle -> target/dist/:");
            eprintln!("                  versioned installer .img (boots into the install");
            eprintln!("                  wizard) + SHA256SUMS + Ed25519 sig + RELEASE-NOTES.md.");
            eprintln!("                  Add --safe for a dry-run installer (no disk writes).");
            eprintln!("  gpu-test        Host-only AMD GPU driver proof — NO .img. Runs the");
            eprintln!("                  ath_amdgpu KATs + the full amdgpu bring-up stage");
            eprintln!(
                "                  transcript on a mock GPU; logs -> %TEMP%/athena-gpu-test.log"
            );
            eprintln!();
            eprintln!("Flags:");
            eprintln!("  --release       Build in release mode (LTO, optimized)");
            eprintln!("  --uefi          Create/boot a UEFI image instead of BIOS");
            eprintln!("  --ci            Run headless and wait for success marker");
            eprintln!("  --safe          Build with kernel `safe_mode` feature — every");
            eprintln!("                  BlockDevice::write_sector is refused at the trait");
            eprintln!("                  level so a bare-metal smoke boot can't clobber a");
            eprintln!("                  host OS already installed on the disk.");
            eprintln!("  --production    Clean consumer boot: on-screen serial mirror starts");
            eprintln!("                  OFF (UEFI splash -> desktop, no scrolling log).");
            eprintln!("                  Durable logs (COM1 + BOOTLOG.TXT + netlog) stay on.");
            eprintln!("  --disk=<type>   QEMU disks: virtio (default), nvme, ata, smoketest (nvme+ahci+virtio)");
            eprintln!("  --target=<t>    Kernel build target triple. Default x86_64-unknown-none.");
            eprintln!("                  aarch64-unknown-none-softfloat (multi-arch Slice A2, ADR");
            eprintln!("                  0009) builds the arch::aarch64 backend; that build is");
            eprintln!("                  EXPECTED to fail on remaining x86-isms (the A3-A9 gap).");
            eprintln!(
                "  --boot=<mode>   build: all (BIOS+UEFI .img), bios, or uefi (R01 packaging)"
            );
            process::exit(1);
        }
    }
}

/// `gpu-test` — host-only AMD GPU driver proof. Builds NO disk image, boots NO
/// QEMU, flashes NO iron: the cheapest layer of the proof ladder for the GPU.
///
/// Two stages, both on the dev host:
///   1. `cargo test -p ath_amdgpu` — the pure-logic KATs (PM4 packet encodings,
///      ATOMBIOS parse incl. the real-Athena-VFCT table, IP-discovery parse,
///      SOC15 offset resolution).
///   2. The LinuxKPI harness in `RAEEN_GPU_ONLY` mode — replays amdgpud's ACTUAL
///      `bringup::bringup` stage sequence (probe -> VBIOS -> GMC -> IH -> SMU ->
///      CP/SDMA rings -> scanout) against a mock GPU with a hardware-reaction
///      model, with `RAEEN_GPU_LOG` surfacing the full `[amdgpu]` stage transcript.
///
/// Combined output is mirrored to %TEMP%/athena-gpu-test.log (same place the QEMU
/// serial log lives) so the stage-by-stage GPU log is one file. A non-zero exit
/// from either stage fails the whole run — this is a FAIL-able gate, not a demo.
fn run_gpu_test() {
    use std::io::Write;
    let root = project_root();
    let log_path = std::env::temp_dir().join("athena-gpu-test.log");
    let mut log = String::new();
    let mut all_ok = true;

    // (1/2) ath_amdgpu pure-logic KATs.
    eprintln!("[gpu-test] (1/2) ath_amdgpu host KATs (cargo test -p ath_amdgpu)");
    let kat = Command::new("cargo")
        .current_dir(&root)
        .args(["test", "-p", "ath_amdgpu"])
        .output()
        .expect("failed to run cargo test -p ath_amdgpu");
    let kat_out = format!(
        "{}{}",
        String::from_utf8_lossy(&kat.stdout),
        String::from_utf8_lossy(&kat.stderr)
    );
    print!("{kat_out}");
    log.push_str("=== ath_amdgpu host KATs ===\n");
    log.push_str(&kat_out);
    all_ok &= kat.status.success();

    // (2/2) Full amdgpu bring-up stage transcript on the mock GPU. The harness
    // is its own workspace (separate target dir), run via --manifest-path.
    eprintln!("[gpu-test] (2/2) amdgpu bring-up transcript (LinuxKPI harness, mock GPU)");
    let harness = Command::new("cargo")
        .current_dir(&root)
        .args([
            "run",
            "--release",
            "--manifest-path",
            "tools/linuxkpi_harness/Cargo.toml",
        ])
        .env("RAEEN_GPU_ONLY", "1")
        .env("RAEEN_GPU_LOG", "1")
        .output()
        .expect("failed to run linuxkpi_harness");
    let h_out = format!(
        "{}{}",
        String::from_utf8_lossy(&harness.stdout),
        String::from_utf8_lossy(&harness.stderr)
    );
    print!("{h_out}");
    log.push_str("\n=== amdgpu bring-up stage transcript (mock GPU) ===\n");
    log.push_str(&h_out);
    all_ok &= harness.status.success();

    if let Ok(mut f) = std::fs::File::create(&log_path) {
        let _ = f.write_all(log.as_bytes());
    }
    eprintln!();
    eprintln!("[gpu-test] GPU logs -> {}", log_path.display());
    if all_ok {
        eprintln!("[gpu-test] RESULT: PASS  (no .img built — host-only GPU proof)");
    } else {
        eprintln!("[gpu-test] RESULT: FAIL");
        process::exit(1);
    }
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must be inside the workspace")
        .to_path_buf()
}

/// Load (or, on first run, generate) the DEV app-signing keypair in `keys/`.
/// `keys/dev-signing.key` is the 32-byte Ed25519 seed; `keys/dev-signing.pub`
/// is the 32-byte public key the kernel embeds (kernel/src/rae_manifest.rs).
/// This is a DEVELOPMENT trust root — the seed lives in the repo so every dev
/// build can sign bundles ("free signing", Concept §Developer onramp). The
/// production signing chain (HSM-held keys, per-developer certs) replaces it
/// in Phase 3.7 / store onboarding.
fn ensure_signing_keys(root: &Path) -> ed25519_dalek::SigningKey {
    let key_dir = root.join("keys");
    let seed_path = key_dir.join("dev-signing.key");
    let pub_path = key_dir.join("dev-signing.pub");
    if !seed_path.exists() {
        std::fs::create_dir_all(&key_dir).expect("create keys/");
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).expect("entropy for signing key");
        std::fs::write(&seed_path, seed).expect("write dev-signing.key");
        eprintln!("[xtask] Generated DEV signing keypair in keys/ (development trust root)");
    }
    let seed: [u8; 32] = std::fs::read(&seed_path)
        .expect("read dev-signing.key")
        .try_into()
        .expect("dev-signing.key must be exactly 32 bytes");
    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
    // Keep the public key in lockstep with the seed (covers a regenerated or
    // hand-replaced seed); the kernel include_bytes!()s this file.
    std::fs::write(&pub_path, sk.verifying_key().to_bytes()).expect("write dev-signing.pub");
    sk
}

fn build_kernel(release: bool, safe: bool, production: bool, target: &str) {
    let root = project_root();
    let mut cmd = Command::new("cargo");
    let mut cargo_args = vec!["build", "-p", "kernel", "--target", target];
    if release {
        cargo_args.push("--release");
    }
    if safe {
        cargo_args.push("--features");
        cargo_args.push("kernel/safe_mode");
    }
    if production {
        cargo_args.push("--features");
        cargo_args.push("kernel/production");
    }
    // Debug passthrough: RAEEN_KERNEL_FEATURES="kernel/embed_test_dsdt" lets a
    // repro run enable extra kernel features without new xtask flags.
    let extra_features = std::env::var("RAEEN_KERNEL_FEATURES").unwrap_or_default();
    if !extra_features.is_empty() {
        eprintln!("[xtask] extra kernel features: {extra_features}");
        cargo_args.push("--features");
        cargo_args.push(&extra_features);
    }
    cmd.current_dir(&root).args(&cargo_args);
    eprintln!(
        "[xtask] Building kernel{}{}...",
        if safe { " (safe-mode)" } else { "" },
        if production { " (production)" } else { "" }
    );
    let status = cmd.status().expect("failed to run cargo build");
    if !status.success() {
        eprintln!("[xtask] Kernel build failed");
        process::exit(1);
    }
    eprintln!("[xtask] Kernel built successfully.");
}

fn build_user_apps(release: bool) {
    let root = project_root();

    let relibc_dir = root.join("components").join("athbridge").join("relibc");
    let build_std = ["-Z", "build-std=core,alloc"];
    let relibc_pkg_args = [
        "build",
        "--target",
        "x86_64-unknown-none",
        "--release",
        "--no-default-features",
        "--features",
        "no_trace",
    ];

    // R11: Build relibc (+ crt0) with rust-src (build-std) for x86_64-unknown-none.
    eprintln!("[xtask] Building relibc (build-std=core,alloc)...");
    let mut cmd_relibc = Command::new("cargo");
    cmd_relibc
        .current_dir(&relibc_dir)
        .arg("+nightly")
        .args(relibc_pkg_args)
        .args(build_std);
    let relibc_status = cmd_relibc
        .status()
        .expect("failed to run cargo build for relibc");
    if !relibc_status.success() {
        eprintln!("[xtask] relibc build failed");
        process::exit(1);
    }

    eprintln!("[xtask] Building relibc crt0...");
    let mut cmd_crt0 = Command::new("cargo");
    cmd_crt0
        .current_dir(&relibc_dir)
        .arg("+nightly")
        .args([
            "build",
            "-p",
            "crt0",
            "--target",
            "x86_64-unknown-none",
            "--release",
        ])
        .args(build_std);
    let crt0_status = cmd_crt0
        .status()
        .expect("failed to run cargo build for crt0");
    if !crt0_status.success() {
        eprintln!("[xtask] relibc crt0 build failed");
        process::exit(1);
    }

    let profile_path = root.join("config").join("base.toml");
    let mut user_apps = recipe::parse_profile(&profile_path);
    let linux_abi_apps = recipe::parse_profile_linux_abi(&profile_path);

    let profile = if release { "release" } else { "debug" };
    let user_bin_dir = root
        .join("target")
        .join("x86_64-unknown-none")
        .join(profile);

    // M5 (opt-in): ATHENA_AMDGPU_REAL=1 builds amdgpud against the REAL upstream
    // Linux amdgpu C driver (the linuxkpi-drm object set) instead of the Rust
    // reimpl, for an Athena GPU bring-up image. Requires the vendored GPL kernel
    // tree + bash (WSL/Linux). OFF by default so the portable image (QEMU CI, the
    // safe image, any box without the GPL tree) always builds. See
    // linuxkpi-drm/M5-BAREMETAL-PLAN.md and amdgpud/build.rs.
    let amdgpu_real = std::env::var_os("ATHENA_AMDGPU_REAL").is_some();
    let prebuilt_amdgpu_obj = std::env::var_os("RAE_AMDGPU_BRINGUP_OBJ")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    if amdgpu_real && prebuilt_amdgpu_obj.is_none() {
        eprintln!(
            "[xtask] ATHENA_AMDGPU_REAL: building the real amdgpu object set (m4c-link.sh)..."
        );
        let status = Command::new("bash")
            .current_dir(&root)
            .arg("linuxkpi-drm/m4c-link.sh")
            .env("FREESTANDING", "1")
            .env("CARGO_TARGET_DIR", root.join("target").join("m4c"))
            .status()
            .expect("failed to run linuxkpi-drm/m4c-link.sh (need bash + vendored GPL source)");
        if !status.success() {
            eprintln!("[xtask] m4c-link.sh failed — unset ATHENA_AMDGPU_REAL to build the Rust-reimpl amdgpud instead");
            process::exit(1);
        }
    } else if let Some(path) = prebuilt_amdgpu_obj.as_ref() {
        eprintln!(
            "[xtask] ATHENA_AMDGPU_REAL: using prebuilt upstream object {}",
            path.display()
        );
    }

    for app in &user_apps {
        let mut cmd = Command::new("cargo");
        if app == "hello_relibc" {
            cmd.arg("+nightly");
        }
        cmd.current_dir(&root)
            .args(["build", "-p", app, "--target", "x86_64-unknown-none"]);
        if app == "hello_relibc" {
            cmd.args(build_std);
        }
        // The real-amdgpu daemon: link the pre-built object set (build.rs picks it
        // up from $HOME/m4-obj). Verified to link in release with precompiled
        // core/alloc — no build-std needed.
        if amdgpu_real && app == "amdgpud" {
            cmd.args(["--features", "real_amdgpu_init"]);
        }
        if release {
            cmd.arg("--release");
        }

        eprintln!("[xtask] Building user app: {}", app);
        let status = cmd
            .status()
            .expect("failed to run cargo build for user app");
        if !status.success() {
            eprintln!("[xtask] User app `{}` build failed", app);
            process::exit(1);
        }
        let osabi = if linux_abi_apps.contains(app) {
            0x03 // ELFOSABI_LINUX → kernel routes through linux_exec
        } else {
            0xAE // ELFOSABI_ATHENAOS → native syscall table
        };
        stamp_osabi(&user_bin_dir.join(app), osabi);
    }

    // Redox-cookbook ports that build cleanly via the simple git+cargo path
    // (build_port). gptman was dropped: its recipe needs `cookbook_cargo
    // --features cli` (the cookbook bash path build_port doesn't run) and its
    // default `nix` feature won't cross-compile for x86_64-unknown-redox — so it
    // only ever emitted a "No binary found" warning. GPT is already covered
    // natively (block_io::parse_gpt + installer/fatfs_esp::seed_minimal_gpt_with_esp).
    // helix stays recipe-only (needs the full cookbook toolchain + tree-sitter
    // grammar .so build). See docs/REDOX_EXTRACTION_MAP.md.
    let ports = ["rustysd", "ripgrep"];
    for port in &ports {
        let bin_path = build_port(port);
        if bin_path.exists() {
            let bin_name = bin_path.file_name().unwrap();
            let dest = user_bin_dir.join(bin_name);
            std::fs::copy(&bin_path, &dest).unwrap();
            // Ports link the in-tree relibc, which speaks NATIVE AthenaOS
            // syscalls — stamp them native regardless of what their build
            // target wrote into the osabi byte.
            stamp_osabi(&dest, 0xAE);
            user_apps.push(bin_name.to_string_lossy().to_string());
        } else {
            eprintln!(
                "[xtask] Warning: No binary found for port {} at {}",
                port,
                bin_path.display()
            );
        }
    }

    let mut tar_cmd = Command::new("tar");
    tar_cmd.current_dir(&root);
    tar_cmd.args(["-cf", "kernel/src/initramfs.tar"]);
    for app in &user_apps {
        tar_cmd.arg("-C");
        tar_cmd.arg(&user_bin_dir);
        tar_cmd.arg(app);
    }
    // Firmware tree: every file under `firmware/` is bundled at its relative
    // path so the kernel can serve it to driver daemons via request_firmware
    // (syscall 142). The lookup key is the path as the driver names it, e.g.
    // `firmware/amdgpu/gc_11_0_1_pfp.bin` for the Radeon 780M (Phoenix) or
    // `firmware/iwlwifi-ty-a0-gf-a0-89.ucode` for the AX210. Drop a blob into
    // firmware/ and it ships — no xtask edit. See docs/FIRMWARE.md for the
    // per-device manifest (which files to fetch from linux-firmware).
    let firmware_dir = root.join("firmware");
    let mut firmware_files: Vec<PathBuf> = Vec::new();
    collect_files_recursive(&firmware_dir, &mut firmware_files);
    if firmware_files.is_empty() {
        eprintln!(
            "[xtask] Warning: firmware/ is empty — driver microcode (amdgpu/iwlwifi) not bundled"
        );
    }
    for f in &firmware_files {
        if let Ok(rel) = f.strip_prefix(&root) {
            // tar entry path = `firmware/...`; -C root keeps it relative.
            tar_cmd.arg("-C");
            tar_cmd.arg(&root);
            tar_cmd.arg(rel);
        }
    }
    eprintln!("[xtask] Firmware blobs bundled: {}", firmware_files.len());

    // rootfs/ tree — the Linux dynamic-linking assets (ld.so + libc.so.6 + a
    // dynamic test binary). Bundled with `-C rootfs/<rel>` so the tar entries
    // land at the FS-ROOT paths a dynamic ELF names (`lib64/ld-linux-x86-64.so.2`,
    // `usr/lib/libc.so.6`, `bin/dh`) — NOT under `rootfs/`. The kernel resolves
    // these via the initramfs so the ELF interpreter + glibc can be loaded.
    let rootfs_dir = root.join("rootfs");
    let mut rootfs_files: Vec<PathBuf> = Vec::new();
    collect_files_recursive(&rootfs_dir, &mut rootfs_files);
    let mut rootfs_bundled = 0usize;
    for f in &rootfs_files {
        // Skip docs — only ship the real binaries.
        if f.extension().and_then(|e| e.to_str()) == Some("md") {
            continue;
        }
        if let Ok(rel) = f.strip_prefix(&rootfs_dir) {
            tar_cmd.arg("-C");
            tar_cmd.arg(&rootfs_dir);
            tar_cmd.arg(rel);
            rootfs_bundled += 1;
        }
    }
    eprintln!(
        "[xtask] rootfs (ld.so/libc) files bundled: {}",
        rootfs_bundled
    );
    // App permission manifests + code signing (Phase 9.2). For each app that
    // ships apps/<name>/RaeManifest.toml: stage a copy with the built ELF's
    // sha256 injected (`elf_sha256 = "..."`), Ed25519-sign the staged bytes
    // with the dev key in keys/, and bundle both the staged manifest and its
    // detached RaeManifest.sig. The kernel verifies the signature with the
    // embedded public key at launch (kernel/src/rae_manifest.rs); a verified
    // manifest is the bundle's trust root, an unsigned one runs in the
    // "unverified developer" posture, and a BAD signature rejects the bundle.
    let signing_key = ensure_signing_keys(&root);
    let stage = root.join("target").join("manifest-stage");
    let mut manifest_count = 0usize;
    for app in &user_apps {
        let src = root.join("apps").join(app).join("RaeManifest.toml");
        if !src.exists() {
            continue;
        }
        let elf_path = user_bin_dir.join(app);
        let elf_bytes = match std::fs::read(&elf_path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!(
                    "[xtask] Warning: cannot read ELF for '{}' ({}); manifest skipped",
                    app, e
                );
                continue;
            }
        };
        let elf_hash = {
            use sha2::Digest;
            hex::encode(sha2::Sha256::digest(&elf_bytes))
        };

        // Inject/refresh the top-level elf_sha256 line: drop any existing one,
        // then insert ours before the first [section] (or at EOF).
        let text = std::fs::read_to_string(&src).expect("manifest read failed");
        let mut staged = String::new();
        let mut injected = false;
        for line in text.lines() {
            if line.trim_start().starts_with("elf_sha256") {
                continue; // refreshed below — hashes never live in the source
            }
            if !injected && line.trim_start().starts_with('[') {
                staged.push_str(&format!("elf_sha256 = \"{}\"\n", elf_hash));
                injected = true;
            }
            staged.push_str(line);
            staged.push('\n');
        }
        if !injected {
            staged.push_str(&format!("elf_sha256 = \"{}\"\n", elf_hash));
        }

        let dir = stage.join("apps").join(app);
        std::fs::create_dir_all(&dir).expect("manifest stage dir");
        std::fs::write(dir.join("RaeManifest.toml"), staged.as_bytes())
            .expect("staged manifest write");
        use ed25519_dalek::Signer;
        let sig = signing_key.sign(staged.as_bytes());
        std::fs::write(dir.join("RaeManifest.sig"), sig.to_bytes()).expect("signature write");

        for entry in ["RaeManifest.toml", "RaeManifest.sig"] {
            tar_cmd.arg("-C");
            tar_cmd.arg(&stage);
            tar_cmd.arg(format!("apps/{}/{}", app, entry));
        }
        manifest_count += 1;
    }
    eprintln!("[xtask] App manifests bundled + signed: {}", manifest_count);
    eprintln!(
        "[xtask] Creating initramfs.tar with {:?} + firmware...",
        user_apps
    );
    let tar_status = tar_cmd.status().expect("failed to run tar");
    if !tar_status.success() {
        eprintln!("[xtask] Failed to create initramfs.tar");
        process::exit(1);
    }
    eprintln!("[xtask] User apps bundled into initramfs.tar successfully.");

    // Phase 3.7 secure-boot manifest: sign sha256(initramfs.tar) with the DEV
    // key so the kernel can verify at boot that its embedded initramfs is the
    // exact, authentic image this build produced — a tamper-evident boot
    // chain. The kernel `include_bytes!`s the blob written here (it compiles
    // AFTER this step). Blob = 48-byte manifest (magic + len + sha256) then a
    // 64-byte Ed25519 signature over the manifest.
    {
        use ed25519_dalek::Signer;
        use sha2::Digest;
        let tar_path = root.join("kernel").join("src").join("initramfs.tar");
        let tar_bytes = std::fs::read(&tar_path).expect("read initramfs.tar for boot manifest");
        let hash = sha2::Sha256::digest(&tar_bytes);
        let mut manifest = Vec::with_capacity(48);
        manifest.extend_from_slice(b"RAEBOOT1");
        manifest.extend_from_slice(&(tar_bytes.len() as u64).to_le_bytes());
        manifest.extend_from_slice(&hash);
        let sig = signing_key.sign(&manifest);
        let mut blob = manifest.clone();
        blob.extend_from_slice(&sig.to_bytes());
        std::fs::write(
            root.join("kernel").join("src").join("boot_manifest.bin"),
            &blob,
        )
        .expect("write boot_manifest.bin");
        eprintln!(
            "[xtask] Boot manifest signed: initramfs {} bytes, sha256 {:02x}{:02x}..{:02x}{:02x}",
            tar_bytes.len(),
            hash[0],
            hash[1],
            hash[30],
            hash[31],
        );
    }
}

/// Stamp the ELF OS/ABI identification byte (`e_ident[EI_OSABI]`, offset 7)
/// of a freshly built user ELF. The kernel's SYS_SPAWN dispatches on it:
/// 0xAE (`ELFOSABI_ATHENAOS`, kernel/src/elf_loader.rs) runs the native
/// AthenaOS syscall table; anything else valid (0x00 SysV, 0x03 Linux) routes
/// through `linux_exec` — Linux auxv stack + Linux syscall translation. The
/// byte a toolchain writes reflects the BUILD TARGET, not the syscall ABI the
/// binary actually speaks (relibc-linked apps target a Linux-flavored triple
/// but call native AthenaOS syscall numbers), so xtask stamps the truth from
/// `config/base.toml` rather than trusting the linker.
fn stamp_osabi(path: &Path, osabi: u8) {
    let mut bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "[xtask] Warning: cannot read {} for osabi stamp ({})",
                path.display(),
                e
            );
            return;
        }
    };
    if bytes.len() > 7 && bytes[0..4] == [0x7F, b'E', b'L', b'F'] && bytes[7] != osabi {
        bytes[7] = osabi;
        std::fs::write(path, bytes).expect("osabi stamp write failed");
    }
}

/// Recursively collect every regular file under `dir` into `out`. Used to
/// bundle the whole `firmware/` tree into the initramfs without a hardcoded
/// file list — any blob dropped in ships on the next build.
fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

fn kernel_binary_path(release: bool) -> PathBuf {
    let root = project_root();
    let profile = if release { "release" } else { "debug" };
    root.join("target")
        .join("x86_64-unknown-none")
        .join(profile)
        .join("kernel")
}

/// Phase 16.2 — assemble the signed release/installer bundle in `target/dist/`:
/// the versioned installer image, a SHA256SUMS manifest, an Ed25519 signature
/// over that manifest (dev trust root — the production signing authority is
/// the open deployment half), the matching public key, and RELEASE-NOTES.md
/// generated from git history. Ends with a FAIL-able verify pass: the manifest
/// hash is recomputed from the copied image and the signature is verified with
/// the public key — a corrupt copy or bad signature aborts the dist.
fn build_dist_bundle(image_path: &Path, safe: bool) {
    use ed25519_dalek::{Signer, Verifier};
    use sha2::Digest;

    let root = project_root();
    let dist_dir = root.join("target").join("dist");
    std::fs::create_dir_all(&dist_dir).expect("create target/dist");

    // Version = workspace version (xtask inherits it) + git short sha.
    let version = env!("CARGO_PKG_VERSION");
    let sha = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let variant = if safe { "-dryrun" } else { "" };
    let image_name = format!("AthenaOS-{version}-{sha}-installer{variant}.img");
    let dist_image = dist_dir.join(&image_name);
    std::fs::copy(image_path, &dist_image).expect("copy installer image into target/dist");
    let image_bytes = std::fs::read(&dist_image).expect("read dist image for hashing");
    let image_hash = sha2::Sha256::digest(&image_bytes);

    // SHA256SUMS (the standard `<hex>  <name>` checksum-manifest shape) +
    // Ed25519 signature over the manifest bytes, so one signature covers
    // every artifact hash.
    let hash_hex: String = image_hash.iter().map(|b| format!("{b:02x}")).collect();
    let manifest = format!("{hash_hex}  {image_name}\n");
    let manifest_path = dist_dir.join("SHA256SUMS");
    std::fs::write(&manifest_path, &manifest).expect("write SHA256SUMS");

    let signing_key = ensure_signing_keys(&root);
    let signature = signing_key.sign(manifest.as_bytes());
    std::fs::write(dist_dir.join("SHA256SUMS.sig"), signature.to_bytes())
        .expect("write SHA256SUMS.sig");
    std::fs::copy(
        root.join("keys").join("dev-signing.pub"),
        dist_dir.join("dev-signing.pub"),
    )
    .expect("copy dev-signing.pub into dist");

    // RELEASE-NOTES.md from git history: since the last tag when one exists,
    // else the last 30 commits.
    let range = Command::new("git")
        .current_dir(&root)
        .args(["describe", "--tags", "--abbrev=0"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| format!("{}..HEAD", String::from_utf8_lossy(&o.stdout).trim()));
    let mut log_args = vec!["log", "--no-merges", "--pretty=format:- %s (%h)"];
    let range_arg;
    if let Some(r) = range {
        range_arg = r;
        log_args.push(&range_arg);
    } else {
        log_args.push("-30");
    }
    let log = Command::new("git")
        .current_dir(&root)
        .args(&log_args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_else(|| "- (git history unavailable)".to_string());
    let notes = format!(
        "# AthenaOS {version} ({sha}) — installer image\n\n\
         Artifact: `{image_name}`{}\n\n\
         Verify: `sha256sum -c SHA256SUMS`, then check `SHA256SUMS.sig`\n\
         (Ed25519 over the SHA256SUMS bytes, public key `dev-signing.pub` —\n\
         DEV trust root; not a production release authority).\n\n\
         Flash (removable media ONLY): `scripts/flash-usb.ps1` refuses\n\
         internal drives. This image boots straight into the install wizard.\n\n\
         ## Changes\n\n{log}\n",
        if safe {
            "\n\n**DRY-RUN build** (`--safe`): every sector write is refused —\n\
             the wizard rehearses the full install without touching any disk."
        } else {
            "\n\n**This is a REAL installer** — it writes disks when the user\n\
             confirms an install. Flash to removable media only."
        }
    );
    std::fs::write(dist_dir.join("RELEASE-NOTES.md"), &notes).expect("write RELEASE-NOTES.md");

    // FAIL-able verify pass (a dist that can't prove itself doesn't ship):
    // re-hash the copied image against the manifest + verify the signature.
    let reread = std::fs::read(&dist_image).expect("re-read dist image");
    let rehash: String = sha2::Sha256::digest(&reread)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let hash_ok = manifest.starts_with(&rehash);
    let sig_ok = signing_key
        .verifying_key()
        .verify(manifest.as_bytes(), &signature)
        .is_ok();
    if !hash_ok || !sig_ok {
        eprintln!("[xtask] dist VERIFY FAILED: hash_ok={hash_ok} sig_ok={sig_ok}");
        process::exit(1);
    }
    eprintln!(
        "[xtask] dist bundle READY: {} ({:.1} MiB) — sha256 {}.. sig=VERIFIED",
        dist_image.display(),
        image_bytes.len() as f64 / (1024.0 * 1024.0),
        &hash_hex[..8],
    );
    eprintln!(
        "[xtask] dist artifacts: SHA256SUMS + SHA256SUMS.sig + dev-signing.pub + RELEASE-NOTES.md"
    );
    if !safe {
        eprintln!("[xtask] SAFETY: this is a REAL installer image (writes disks on user confirm) — flash to REMOVABLE media only (scripts/flash-usb.ps1 enforces this).");
    }
}

fn create_disk_image(release: bool, uefi: bool) -> PathBuf {
    let kernel_binary = kernel_binary_path(release);
    if !kernel_binary.exists() {
        eprintln!(
            "[xtask] Kernel binary not found at {}",
            kernel_binary.display()
        );
        process::exit(1);
    }

    let image_path = if uefi {
        kernel_binary.with_extension("uefi.img")
    } else {
        kernel_binary.with_extension("bios.img")
    };

    eprintln!(
        "[xtask] Creating {} disk image...",
        if uefi { "UEFI" } else { "BIOS" }
    );

    // Bake a pre-allocated BOOTLOG.TXT into the image's ESP. The kernel's
    // bootlog persistence (kernel/src/bootlog_persist.rs) only ever
    // overwrites an EXISTING file's data clusters — it never allocates —
    // so the file must already be on the stick. Creating it here means
    // every flash carries it; previously a re-flash wiped any hand-created
    // copy and the bare-metal boot log was silently lost.
    let mut builder = bootloader::DiskImageBuilder::new(kernel_binary);
    builder.set_file_contents("BOOTLOG.TXT".to_string(), bootlog_placeholder());
    if uefi {
        builder
            .create_uefi_image(&image_path)
            .expect("failed to create UEFI disk image");
    } else {
        builder
            .create_bios_image(&image_path)
            .expect("failed to create BIOS disk image");
    }

    eprintln!(
        "[xtask] Disk image: {} (with 1 MiB BOOTLOG.TXT)",
        image_path.display()
    );
    image_path
}

/// 1 MiB BOOTLOG.TXT placeholder. The leading line distinguishes "kernel
/// never wrote here" from "file missing" when the stick is read back on
/// Windows; the kernel's first flush overwrites the whole file.
fn bootlog_placeholder() -> Vec<u8> {
    let mut data = vec![0u8; 1024 * 1024];
    let msg =
        b"ATHENAOS BOOTLOG: placeholder - the kernel has not written a log into this file yet.\r\n";
    data[..msg.len()].copy_from_slice(msg);
    data
}

/// Build the QEMU USB-MSC backing image so the boot smoketest exercises the
/// real bare-metal bootlog-on-USB path:
///
///   * sector 0: the smoketest READ(10) signature in the MBR boot-code area
///     (bytes 0..446 are free), plus a partition entry (type 0x0E = FAT16
///     LBA) and the 0x55AA signature — all three coexist in one sector;
///   * a FAT16 partition at LBA 2048 holding a pre-allocated 1 MiB
///     BOOTLOG.TXT, mirroring what a flashed boot stick carries.
///
/// kernel/src/bootlog_persist.rs prefers USB media, so on this image QEMU
/// proves: MSC enumeration → FAT16 locate → WRITE(10) flush → SYNC CACHE.
fn write_usb_msc_image(path: &Path) -> std::io::Result<()> {
    use fatfs::{FatType, FormatVolumeOptions, FsOptions};
    use fscommon::StreamSlice;
    use std::io::{Cursor, Write};

    const IMG_SIZE: usize = 16 * 1024 * 1024;
    const PART_START_LBA: u64 = 2048;

    let mut img = vec![0u8; IMG_SIZE];

    // Smoketest signature (read back via SCSI READ(10) of sector 0).
    let sig = b"ATHENAOS-USB-MSC-SECTOR0";
    img[..sig.len()].copy_from_slice(sig);

    // MBR partition entry #0: FAT16 LBA from sector 2048 to end of image.
    let part_sectors = (IMG_SIZE as u64 / 512 - PART_START_LBA) as u32;
    let e = 446;
    img[e + 4] = 0x0E; // FAT16 with LBA addressing
    img[e + 8..e + 12].copy_from_slice(&(PART_START_LBA as u32).to_le_bytes());
    img[e + 12..e + 16].copy_from_slice(&part_sectors.to_le_bytes());
    img[510] = 0x55;
    img[511] = 0xAA;

    // Format the partition FAT16 and create the pre-allocated BOOTLOG.TXT.
    let part_byte_start = PART_START_LBA * 512;
    {
        let slice = StreamSlice::new(Cursor::new(&mut img[..]), part_byte_start, IMG_SIZE as u64)?;
        fatfs::format_volume(
            slice,
            FormatVolumeOptions::new()
                .fat_type(FatType::Fat16)
                .volume_label(*b"RAEENUSB   "),
        )?;
    }
    {
        let slice = StreamSlice::new(Cursor::new(&mut img[..]), part_byte_start, IMG_SIZE as u64)?;
        let fs = fatfs::FileSystem::new(slice, FsOptions::new())?;
        let mut f = fs.root_dir().create_file("BOOTLOG.TXT")?;
        f.write_all(&bootlog_placeholder())?;
        f.flush()?;
    }

    std::fs::write(path, &img)
}

fn deploy_ventoy(image_path: &Path, drive: &str) {
    let mut drive_path = drive.to_string();
    if !drive_path.ends_with(':') && !drive_path.ends_with(":\\") && !drive_path.starts_with('/') {
        drive_path.push(':');
    }

    // Ensure the destination ends properly
    let dest = if drive_path.starts_with('/') {
        PathBuf::from(drive_path).join(image_path.file_name().unwrap())
    } else {
        PathBuf::from(format!("{}\\", drive_path)).join(image_path.file_name().unwrap())
    };

    eprintln!("[xtask] Deploying to Ventoy drive: {}", dest.display());

    match std::fs::copy(image_path, &dest) {
        Ok(_) => eprintln!("[xtask] Successfully deployed to Ventoy."),
        Err(e) => {
            eprintln!("[xtask] Failed to copy to Ventoy: {}", e);
            process::exit(1);
        }
    }
}

/// TCP port for the optional QMP screenshot socket (ADR 0004).
const QMP_SCREENSHOT_PORT: u16 = 55556;

/// Minimal QMP client: connect to the QEMU monitor socket, do the capabilities
/// handshake, and issue `screendump` to a PNG (QEMU 7.1+ `format=png`, falling
/// back to PPM on older builds). ADR 0004. No external crate — hand-rolled JSON
/// lines are sufficient for these three commands.
fn qmp_screendump(port: u16, out_path: &str) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};

    // The QMP server is opened at QEMU startup, but connect-retry to be robust.
    let mut stream = None;
    for _ in 0..40 {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(250)),
        }
    }
    let stream = stream.ok_or_else(|| "QMP connect failed".to_string())?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(20)));
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);

    // Read JSON lines until one carries a `return`/`error` (skip greeting + async events).
    fn wait_reply(reader: &mut impl BufRead) -> String {
        let mut line = String::new();
        for _ in 0..64 {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if line.contains("\"return\"") || line.contains("\"error\"") {
                return line;
            }
        }
        line
    }

    // QMP greeting.
    {
        let mut g = String::new();
        let _ = reader.read_line(&mut g);
    }
    writer
        .write_all(b"{\"execute\":\"qmp_capabilities\"}\r\n")
        .map_err(|e| e.to_string())?;
    let _ = wait_reply(&mut reader);

    // Drive the guest before the dump (RAEEN_QMP_KEYS="esc,tab,ret" — comma-separated
    // QEMU qcodes) so we can screenshot surfaces PAST the boot marker: e.g. "esc"
    // skips the OOBE and lands on the desktop. Each key is a press+release; a short
    // gap lets the UI react, then RAEEN_QMP_SETTLE_MS (default 2500) lets the target
    // surface compose before the single screendump (TCG coalesces to one/session).
    if let Ok(keys) = std::env::var("RAEEN_QMP_KEYS") {
        for qcode in keys.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let cmd = format!(
                "{{\"execute\":\"send-key\",\"arguments\":{{\"keys\":[{{\"type\":\"qcode\",\"data\":\"{qcode}\"}}]}}}}\r\n"
            );
            let _ = writer.write_all(cmd.as_bytes());
            let _ = wait_reply(&mut reader);
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let settle = std::env::var("RAEEN_QMP_SETTLE_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(2500);
        std::thread::sleep(std::time::Duration::from_millis(settle));
    }

    // Absolute, forward-slashed path (QEMU writes it relative to its own CWD otherwise).
    let abs = if std::path::Path::new(out_path).is_absolute() {
        out_path.to_string()
    } else {
        std::env::current_dir()
            .map(|d| d.join(out_path).to_string_lossy().to_string())
            .unwrap_or_else(|_| out_path.to_string())
    };
    let fname = abs.replace('\\', "/");

    let cmd = format!(
        "{{\"execute\":\"screendump\",\"arguments\":{{\"filename\":\"{fname}\",\"format\":\"png\"}}}}\r\n"
    );
    writer
        .write_all(cmd.as_bytes())
        .map_err(|e| e.to_string())?;
    let reply = wait_reply(&mut reader);
    if reply.contains("\"error\"") {
        // Older QEMU without the `format` arg: default PPM.
        let ppm = fname.trim_end_matches(".png").to_string() + ".ppm";
        let cmd2 =
            format!("{{\"execute\":\"screendump\",\"arguments\":{{\"filename\":\"{ppm}\"}}}}\r\n");
        writer
            .write_all(cmd2.as_bytes())
            .map_err(|e| e.to_string())?;
        let r2 = wait_reply(&mut reader);
        if r2.contains("\"error\"") {
            return Err(format!("screendump error: {}", r2.trim()));
        }
    }
    let _ = writer.write_all(b"{\"execute\":\"quit\"}\r\n");
    Ok(())
}

fn run_qemu(image_path: &Path, uefi: bool, ci: bool, disk_profile: &str, screenshot: Option<&str>) {
    let qemu = find_qemu();

    eprintln!("[xtask] Launching QEMU...");
    eprintln!("[xtask] Image: {}", image_path.display());
    eprintln!("[xtask] Disk Profile: {}", disk_profile);

    let mut cmd = Command::new(&qemu);

    if uefi {
        if let Some(ovmf) = find_ovmf() {
            // QEMU 11 rejects the split, non-4MB-aligned `edk2-x86_64-code.fd`
            // (~3.48 MB) when passed via `-bios` ("could not load PC BIOS").
            // Split OVMF firmware must be mapped as a pflash device. Combined
            // `OVMF.fd` images still load fine this way too (readonly code).
            let lower = ovmf.to_ascii_lowercase();
            if lower.ends_with("ovmf.fd") {
                cmd.args(["-bios", &ovmf]);
            } else {
                cmd.args([
                    "-drive",
                    &format!("if=pflash,format=raw,readonly=on,file={}", ovmf),
                ]);
            }
        } else {
            eprintln!("[xtask] WARN: OVMF firmware not found; UEFI boot may fail");
            cmd.args(["-bios", "OVMF.fd"]);
        }
    }

    if disk_profile == "smoketest" {
        ensure_smoketest_disks(&project_root());
        cmd.args([
            "-drive",
            &format!(
                "file={},format=raw,if=none,id=nvm0",
                project_root().join("target/nvme.img").display()
            ),
            "-device",
            "nvme,drive=nvm0,serial=athena001",
            "-drive",
            &format!(
                "file={},format=raw,if=none,id=ahcidisk",
                project_root().join("target/ahci.img").display()
            ),
            "-device",
            "ahci,id=ahci0",
            "-device",
            "ide-hd,drive=ahcidisk,bus=ahci0.0",
        ]);
    }

    if disk_profile == "nvme" {
        cmd.args([
            "-drive",
            &format!("file={},format=raw,if=none,id=drv0", image_path.display()),
            "-device",
            "nvme,drive=drv0,serial=NVME_SERIAL",
        ]);
    } else if disk_profile == "ata" {
        cmd.args([
            "-drive",
            &format!("file={},format=raw,if=ide", image_path.display()),
        ]);
    } else {
        cmd.args([
            "-drive",
            &format!("file={},format=raw", image_path.display()),
        ]);
    }

    // Common QEMU flags.
    //
    // RAM: the kernel image embeds the initramfs (currently ~72 MB of debug-build
    // user apps), so the UEFI bootloader needs enough memory to load it or it
    // panics with OUT_OF_RESOURCES at bootloader main.rs:334. 2G is comfortably
    // above the image size and realistic for the target hardware (Athena ≥16 GB).
    //
    // `-d int` (per-interrupt CPU-state dump) was a debug leftover: it floods
    // stderr and drastically slows boot (a full register dump on every timer
    // tick). Re-enable manually only when diagnosing interrupt/fault issues.
    // Hardware acceleration (opt-in): set ATHENA_ACCEL=whpx to run the guest on
    // the Windows Hypervisor Platform — orders of magnitude faster than the
    // default TCG software emulation (under TCG the heavy AthFS bucket I/O in the
    // boot smoketest crawls for minutes). `kernel-irqchip=off` is required (WHPX
    // has no in-kernel IRQ chip).
    //
    // Default is TCG because WHPX currently surfaces a kernel bug: QEMU assigns
    // 64-bit PCI BARs (e.g. xHCI at ~0x3800_0000_8000) above the kernel's mapped
    // physical window, so the first MMIO access page-faults in kernel context.
    // Once the kernel maps high PCI BARs on demand (ioremap), make WHPX the
    // default. See docs / MasterChecklist Phase 1 §1.5.
    // Acceleration priority: ATHENA_ACCEL override → else auto by host OS.
    //   ATHENA_ACCEL=whpx  — Windows Hypervisor Platform (needs kernel-irqchip=off)
    //   ATHENA_ACCEL=kvm   — force KVM; ATHENA_ACCEL=tcg — force software emulation
    //   (unset) Linux with a usable /dev/kvm → KVM; otherwise TCG.
    // On a Linux host (incl. WSL2 once the user is in the `kvm` group so /dev/kvm
    // is rw) KVM gives the hardware-virt speedup vs ~30x-slower TCG. We keep the
    // same qemu64 CPU model the kernel is tested against rather than `-cpu host`.
    let accel = std::env::var("ATHENA_ACCEL").ok();
    let accel = accel.as_deref();
    let kvm_usable = cfg!(target_os = "linux") && Path::new("/dev/kvm").exists();
    // CPU model. Default is the qemu64 model the kernel is regression-tested
    // against. RAEEN_CPU overrides it — chiefly `RAEEN_CPU=host` under KVM, which
    // passes the real host CPUID through so the guest sees the actual AMD family
    // (0x19 on the Athena's Ryzen), exercising the `is_amd() && cpu_family()>=0x17`
    // gated paths (e.g. the AMD SMU/SMN temperature read) that the qemu64 model
    // (Family 0xF) skips. Only meaningful with -accel kvm.
    // +smep exposes SMEP in CPUID so the kernel's real CR4.SMEP enable path (the
    // hardware ret2usr guard) is exercised + verified in CI. +smap exposes SMAP:
    // now that every kernel->user access routes through the stac/clac uaccess
    // chokepoint, the real CR4.SMAP trap is exercised + verified in CI (the
    // behavioral SMAP smoketest asserts a non-stac supervisor read of a user
    // page faults while the chokepoint stays open).
    // +umip exposes UMIP so the kernel's real CR4.UMIP enable path (blocking
    // userspace SGDT/SIDT/SLDT/STR/SMSW descriptor-table leaks) is exercised in CI.
    let cpu_model = std::env::var("RAEEN_CPU")
        .unwrap_or_else(|_| "qemu64,+topoext,+x2apic,+smep,+smap,+umip,+aes,+rdrand".to_string());
    if std::env::var("RAEEN_CPU").is_ok() {
        eprintln!("[xtask] CPU model: {cpu_model} (RAEEN_CPU override)");
    }
    if accel == Some("whpx") {
        eprintln!("[xtask] Acceleration: WHPX (ATHENA_ACCEL=whpx)");
        cmd.args(["-accel", "whpx,kernel-irqchip=off", "-cpu", &cpu_model]);
    } else if accel == Some("kvm") || (accel.is_none() && kvm_usable) {
        eprintln!("[xtask] Acceleration: KVM (/dev/kvm)");
        cmd.args(["-accel", "kvm", "-cpu", &cpu_model]);
    } else {
        let hint = if cfg!(target_os = "linux") {
            "TCG (no usable /dev/kvm — add your user to the `kvm` group for KVM)"
        } else {
            "TCG (set ATHENA_ACCEL=whpx for WHPX)"
        };
        eprintln!("[xtask] Acceleration: {hint}");
        cmd.args(["-cpu", &cpu_model]);
    }

    // Write the serial log to the system temp dir, NOT the OneDrive-synced repo:
    // OneDrive's filter driver intermittently locks the file, which breaks QEMU's
    // `-serial file:` open ("could not connect serial device") and corrupts CI
    // reads. Forward slashes so QEMU's `file:` parser accepts the Windows path.
    let serial_path = std::env::temp_dir().join("athena-serial.log");
    let _ = std::fs::remove_file(&serial_path);
    let serial_arg = format!("file:{}", serial_path.to_string_lossy().replace('\\', "/"));
    cmd.args(["-serial", &serial_arg]);
    // --screenshot: open a QMP socket so the CI marker-hit path can issue a
    // `screendump` (the framebuffer device renders internally even under
    // `-display none`). Reuses this known-good boot rather than a standalone harness.
    if screenshot.is_some() {
        cmd.args([
            "-qmp",
            &format!("tcp:127.0.0.1:{QMP_SCREENSHOT_PORT},server,nowait"),
        ]);
    }
    // SMP count: default 2 (the documented "clean" config). Override with
    // ATHENA_SMP=<n> — `ATHENA_SMP=1` avoids the work-stealing scheduler entirely,
    // which is the deterministic config for verifying a kernel change while the
    // multi-CPU duplicate-task race (MasterChecklist 4.8) is still open.
    let smp = std::env::var("ATHENA_SMP")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "2".to_string());
    eprintln!(
        "[xtask] SMP: {} vCPU(s) (set ATHENA_SMP=<n> to change)",
        smp
    );
    cmd.args([
        "-no-reboot",
        "-no-shutdown",
        "-device",
        "isa-debug-exit,iobase=0xf4,iosize=0x04",
        "-m",
        "2G",
        "-smp",
        &smp,
    ]);

    // Fault diagnostics (opt-in): RAEEN_QEMU_DEBUG=1 turns on QEMU's
    // interrupt + cpu-reset tracing to a side log. Invaluable for
    // diagnosing a silent triple-fault (guest resets, -no-reboot makes
    // QEMU exit, but the kernel never logged a panic). The reset trace
    // shows the exact vector + error code + RIP that caused the fault
    // cascade. Off by default — it floods and slows boot.
    if std::env::var("RAEEN_QEMU_DEBUG").is_ok() {
        let dbg_path = std::env::temp_dir().join("athena-qemu-debug.log");
        let _ = std::fs::remove_file(&dbg_path);
        eprintln!("[xtask] QEMU debug trace: {}", dbg_path.display());
        cmd.args([
            "-d",
            "int,cpu_reset,guest_errors",
            "-D",
            &dbg_path.to_string_lossy().replace('\\', "/"),
        ]);
    }

    if ci {
        cmd.args(["-display", "none"]);
    }

    // Create a dummy disk for VirtIO testing
    let dummy_disk_path = std::path::Path::new("target/virtio.img");
    if !dummy_disk_path.exists() {
        std::fs::write(dummy_disk_path, vec![0u8; 1024 * 1024]).unwrap(); // 1MB dummy disk
    }

    // Add VirtIO block device on a different index so we don't boot from it!
    cmd.args([
        "-drive",
        &format!(
            "file={},format=raw,if=virtio,index=1",
            dummy_disk_path.display()
        ),
    ]);

    // User-mode netdev + virtio-net-pci for RX/DHCP bring-up (#110).
    // hostfwd: host localhost:2222 -> guest :22, so a real `ssh -p 2222
    // athena@localhost` reaches the in-kernel RaeSSH listener (AthNet SSH server
    // Increment B). Harmless when nothing listens; guest DHCP IP is 10.0.2.15.
    cmd.args([
        "-netdev",
        "user,id=net0,hostfwd=tcp::2222-:22",
        "-device",
        "virtio-net-pci,netdev=net0",
        "-device",
        // p2=8/p3=8 → 8 USB2 ports (1-8) + 8 USB3 ports. The default (4+4) put
        // every USB2 slot in use (tablet/hub/mouse/storage on 1-4), leaving only
        // USB3 ports for the full-speed usb-audio device; the extra USB2 ports
        // give it a home on port 5.
        "qemu-xhci,id=xhci,p2=8,p3=8",
        "-device",
        "usb-tablet,bus=xhci.0,port=1",
        // usb-mouse = HID boot protocol 2 (relative), exercises the live boot-mouse
        // path (dispatch_boot_mouse + SET_PROTOCOL/SET_IDLE), distinct from the
        // usb-tablet absolute digitizer.
        "-device",
        "usb-mouse,bus=xhci.0,port=3",
        // Exercise USB hub enumeration (MasterChecklist 2.1): a keyboard behind
        // a hub validates route-string + TT addressing for HID-behind-hub, the
        // common bare-metal case where the keyboard isn't on a root port.
        "-device",
        "usb-hub,id=usbhub,bus=xhci.0,port=2",
        "-device",
        "usb-kbd,bus=xhci.0,port=2.1",
        // USB Audio Class DAC (MasterChecklist 2.6) on a free root port (5). The
        // `none` audiodev is a null backend (headless CI has no host audio) so the
        // UAC device attaches and the kernel's usb_audio descriptor parse runs
        // against a REAL device, not just the synthetic smoketest.
        "-audiodev",
        "none,id=snd0",
        "-device",
        "usb-audio,audiodev=snd0,bus=xhci.0,port=5",
        // Intel HDA controller + output codec (MasterChecklist 7.1): gives the
        // kernel a REAL HDA controller to bring up CORB/RIRB on and a REAL
        // codec to walk (GET_PARAMETER verbs, widget graph) under QEMU —
        // previously the codec walk could only run on iron. Same null
        // audiodev: enumeration + verbs need no host audio hardware.
        "-device",
        "intel-hda,id=hda",
        "-device",
        "hda-output,audiodev=snd0,bus=hda.0",
        // Secondary virtio-gpu adapter for the Phase 6 CPU→GPU on-ramp
        // (kernel/src/virtio_gpu.rs). Attached alongside the default VGA so the
        // bootloader's GOP framebuffer (primary) is undisturbed; the virtio-gpu
        // driver exercises the GPU command/scanout path on this second adapter.
        "-device",
        "virtio-gpu-pci",
    ]);

    // USB Mass Storage device (Phase 2.1) — a bulk-only-transport disk on the
    // xHCI bus so kernel/src/usb_msc.rs can enumerate it, run INQUIRY/
    // READ_CAPACITY/READ(10), and register it as a BlockDevice. The backing
    // image carries a signature at sector 0 so the smoketest can verify a real
    // read landed.
    let usb_msc_path = std::path::Path::new("target/usb-msc.img");
    if let Err(e) = write_usb_msc_image(usb_msc_path) {
        eprintln!("[xtask] WARN: usb-msc.img build failed ({e}); USB bootlog test disabled");
    }
    cmd.args([
        "-drive",
        &format!(
            "if=none,id=usbmsc,format=raw,file={}",
            usb_msc_path.display()
        ),
        "-device",
        "usb-storage,drive=usbmsc,bus=xhci.0,port=4",
    ]);

    // Second MSC stick BEHIND the hub (port 2.2): regression coverage for
    // MSC-behind-hub classification — the bare-metal "boot stick in a front
    // panel port" case that the HID-only hub-child path used to miss.
    let usb_msc2_path = std::path::Path::new("target/usb-msc2.img");
    if let Err(e) = write_usb_msc_image(usb_msc2_path) {
        eprintln!("[xtask] WARN: usb-msc2.img build failed ({e}); hub-MSC test disabled");
    }
    cmd.args([
        "-drive",
        &format!(
            "if=none,id=usbmsc2,format=raw,file={}",
            usb_msc2_path.display()
        ),
        "-device",
        "usb-storage,drive=usbmsc2,bus=xhci.0,port=2.2",
    ]);

    if ci {
        eprintln!("[xtask] CI Headless Mode: Waiting for boot success marker...");
        eprintln!("[xtask] Serial log: {}", serial_path.display());
        let serial_log = serial_path.clone();

        let mut child = cmd.spawn().expect("Failed to spawn QEMU in CI mode");
        let pid = child.id();

        // Force-kill the whole QEMU process tree and reap the handle. `Child::kill`
        // alone does not reliably tear down a QEMU stuck in a WHPX hypervisor call
        // on Windows — the leaked process keeps serial.log open and breaks every
        // subsequent run. `taskkill /T` kills the tree; `wait()` reaps it.
        fn reap(child: &mut std::process::Child, pid: u32) {
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .status();
            }
            let _ = pid;
            let _ = child.kill();
            let _ = child.wait();
        }

        let start = std::time::Instant::now();
        // Screenshot mode boots UEFI (OVMF POST + ~72MB initramfs) under TCG, which
        // is far slower than the BIOS CI path — give it a longer marker window.
        let timeout = std::time::Duration::from_secs(if screenshot.is_some() { 560 } else { 300 });

        // Wait briefly for QEMU to create the file.
        std::thread::sleep(std::time::Duration::from_millis(500));

        loop {
            if start.elapsed() > timeout {
                eprintln!(
                    "[xtask] CI Timeout: QEMU did not boot within {}s.",
                    timeout.as_secs()
                );
                reap(&mut child, pid);
                process::exit(1);
            }
            if let Ok(content) = std::fs::read_to_string(&serial_log) {
                let booted = content.contains("[ OS ] System successfully booted.");
                // In screenshot mode also trigger on a desktop-up sentinel present in
                // BOTH dev and --production builds (the latter suppresses the boot
                // marker's serial line): the compositor's root desktop surface means
                // the framebuffer is composited and ready to capture.
                let desktop_up = screenshot.is_some()
                    && content.contains("kernel surface 1 created")
                    && content.contains("(z=0, desktop)");
                if booted || desktop_up {
                    eprintln!("[xtask] CI Success: Detected boot completion marker!");
                    // --screenshot: the marker is up; give the compositor a settle
                    // window to composite the OOBE/desktop, then capture a PNG via QMP
                    // and exit (skip the daemon drain — we want the visual frame).
                    if let Some(out) = screenshot {
                        // Settle window before the capture. Default 8s; override with
                        // RAEEN_SHOT_SETTLE_MS for surfaces that take longer to reach a
                        // steady frame (e.g. the desktop_autologin re-assert loop).
                        let settle_ms = std::env::var("RAEEN_SHOT_SETTLE_MS")
                            .ok()
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(8000);
                        let settle = std::time::Duration::from_millis(settle_ms);
                        eprintln!(
                            "[xtask] --screenshot: settling {}ms then capturing {out}",
                            settle.as_millis()
                        );
                        std::thread::sleep(settle);
                        match qmp_screendump(QMP_SCREENSHOT_PORT, out) {
                            Ok(()) => eprintln!("[xtask] --screenshot: captured {out}"),
                            Err(e) => eprintln!("[xtask] --screenshot: FAILED: {e}"),
                        }
                        reap(&mut child, pid);
                        process::exit(0);
                    }
                    // Post-boot drain: the userspace daemons (user_init, athbridge_host,
                    // i915d, amdgpud, …) run AFTER the boot marker. Give them a bounded
                    // window to finish so their serial output lands in the log for CI/dev
                    // inspection, rather than reaping QEMU mid-bring-up. Exit early once
                    // amdgpud reaches a terminal startup result (9004 = retained real
                    // AMDGPU device service-ready; 9099 = clean no-Radeon result;
                    // 7690 = spawn failed). user_init deliberately does not reap the
                    // persistent GPU daemon. 300s
                    // bounds the window under QEMU TCG (~30x slower than iron, and the
                    // BlockedOnWait wake after each child exit is currently slowed by
                    // the CPU0↔CPU1 steal ping-pong — see MasterChecklist "Latent
                    // kernel bugs"); the marker hit ends the drain early, so real
                    // hardware and fast hosts never wait the full window.
                    let drain_deadline =
                        std::time::Instant::now() + std::time::Duration::from_secs(300);
                    while std::time::Instant::now() < drain_deadline {
                        if let Ok(c) = std::fs::read_to_string(&serial_log) {
                            if c.contains("msg: 9004")
                                || c.contains("msg: 9099")
                                || c.contains("msg: 7690")
                            {
                                break;
                            }
                        }
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                    reap(&mut child, pid);
                    process::exit(0);
                }
                if content.contains("[PANIC]") || content.contains("panicked at") {
                    eprintln!("[xtask] CI Error: Kernel panic detected!");
                    reap(&mut child, pid);
                    process::exit(1);
                }
            }

            if let Ok(Some(status)) = child.try_wait() {
                eprintln!("[xtask] QEMU exited prematurely with status: {}", status);
                reap(&mut child, pid);
                process::exit(1);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    } else {
        let status = cmd.status().unwrap_or_else(|e| {
            eprintln!("[xtask] Failed to launch QEMU: {}", e);
            eprintln!("[xtask] Make sure qemu-system-x86_64 is installed and in PATH.");
            eprintln!("[xtask] Install: winget install SoftwareFreedomConservancy.QEMU");
            process::exit(1);
        });

        if !status.success() {
            let code = status.code().unwrap_or(-1);
            // Exit code 33 = (0x10 << 1) | 1 = success via isa-debug-exit
            if code != 33 {
                eprintln!("[xtask] QEMU exited with code {}", code);
            }
        }
    }
}

/// R01: OVMF search paths (Redox mk/qemu.mk style).
fn find_ovmf() -> Option<String> {
    let candidates = [
        "OVMF.fd",
        r"C:\Program Files\qemu\share\edk2-x86_64-code.fd",
        r"C:\Program Files\qemu\share\OVMF.fd",
        "/usr/share/OVMF/OVMF_CODE.fd",
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
    ];
    for path in &candidates {
        if Path::new(path).exists() {
            return Some(path.to_string());
        }
    }
    None
}

fn ensure_smoketest_disks(root: &Path) {
    let target = root.join("target");
    let _ = std::fs::create_dir_all(&target);

    let nvme = target.join("nvme.img");
    if !nvme.exists() {
        let mut bytes = vec![0u8; 16 * 1024 * 1024];
        let marker = b"AthenaOS-NVMe-block-0-ok!";
        bytes[..marker.len()].copy_from_slice(marker);
        let _ = std::fs::write(&nvme, bytes);
    }

    let ahci = target.join("ahci.img");
    if !ahci.exists() {
        let mut bytes = vec![0u8; 1024 * 1024];
        let marker = b"AthenaOS-AHCI-block-0-ok!";
        bytes[..marker.len()].copy_from_slice(marker);
        let _ = std::fs::write(&ahci, bytes);
    }

    let virtio = target.join("virtio.img");
    if !virtio.exists() {
        let mut bytes = vec![0u8; 1024 * 1024];
        let marker = b"AthFS-VirtIO-Block-0 hello!";
        bytes[..marker.len()].copy_from_slice(marker);
        let _ = std::fs::write(&virtio, bytes);
    }
}

fn find_qemu() -> String {
    // Check PATH first
    if Command::new("qemu-system-x86_64")
        .arg("--version")
        .output()
        .is_ok()
    {
        return "qemu-system-x86_64".to_string();
    }

    // Check common Windows install locations
    let candidates = [
        r"C:\Program Files\qemu\qemu-system-x86_64.exe",
        r"C:\Program Files (x86)\qemu\qemu-system-x86_64.exe",
        r"C:\qemu\qemu-system-x86_64.exe",
    ];

    for path in &candidates {
        if Path::new(path).exists() {
            return path.to_string();
        }
    }

    // Fall back to hoping it's in PATH (will error later with a helpful message)
    "qemu-system-x86_64".to_string()
}

fn build_port(port_name: &str) -> PathBuf {
    let root = project_root();
    let port_dir = root.join("ports").join(port_name);
    let recipe_path = port_dir.join("recipe.toml");

    if !recipe_path.exists() {
        eprintln!(
            "[xtask] Port recipe not found at: {}",
            recipe_path.display()
        );
        process::exit(1);
    }

    let recipe = recipe::parse_port_recipe(&recipe_path);

    let sources_dir = root.join("target").join("ports").join("sources");
    std::fs::create_dir_all(&sources_dir).unwrap();
    let source_dir = sources_dir.join(port_name);

    if let Some(git_url) = recipe.source.git {
        if !source_dir.exists() {
            eprintln!("[xtask] Cloning {} from {}...", port_name, git_url);
            let mut cmd = Command::new("git");
            cmd.arg("clone").arg(&git_url).arg(&source_dir);
            if let Some(branch) = recipe.source.branch {
                cmd.arg("--branch").arg(&branch);
            }
            let status = cmd.status().unwrap();
            if !status.success() {
                eprintln!("[xtask] Failed to clone {}", port_name);
                process::exit(1);
            }
        } else {
            eprintln!(
                "[xtask] Source for {} already exists, pulling latest...",
                port_name
            );
            let mut cmd = Command::new("git");
            cmd.current_dir(&source_dir).arg("pull");
            let _ = cmd.status(); // Ignore errors if offline
        }
    } else {
        eprintln!("[xtask] Only git sources are supported currently.");
        process::exit(1);
    }

    // Cross-compile for bare metal
    eprintln!(
        "[xtask] Cross-compiling {} for x86_64-unknown-redox...",
        port_name
    );
    let mut cross_cmd = Command::new("cargo");
    cross_cmd
        .current_dir(&source_dir)
        .arg("+nightly")
        .arg("build")
        .arg("--target")
        .arg("x86_64-unknown-redox")
        .arg("-Z")
        .arg("build-std=std,panic_abort")
        .arg("--release")
        // Strip symbols: an unstripped release `ripgrep` is ~21 MB, which (embedded
        // in the initramfs → kernel ELF) bloated the boot image to 30 MB and stalled
        // the BIOS bootloader before it could even load the kernel (0-byte serial /
        // CI timeout). Stripped, it is a few MB. `strip` keeps the ELF header intact
        // so stamp_osabi still works.
        .arg("--config")
        .arg("profile.release.strip=true");

    let sysroot_lib = root.join("target").join("sysroot").join("lib");
    std::fs::create_dir_all(&sysroot_lib).unwrap();
    let relibc_lib = root
        .join("components")
        .join("athbridge")
        .join("relibc")
        .join("target")
        .join("x86_64-unknown-none")
        .join("release")
        .join("librelibc.a");
    if relibc_lib.exists() {
        std::fs::copy(&relibc_lib, sysroot_lib.join("libc.a")).unwrap();
        std::fs::copy(&relibc_lib, sysroot_lib.join("libgcc_eh.a")).unwrap();
    }

    let sysroot_lib_str = sysroot_lib.display().to_string().replace('\\', "/");
    cross_cmd.env(
        "RUSTFLAGS",
        format!(
            "-C linker=rust-lld -C panic=abort -C link-arg=-L{} -C link-arg=--allow-multiple-definition -C link-arg=--unresolved-symbols=ignore-all",
            sysroot_lib_str
        ),
    );

    if port_name == "helix" {
        cross_cmd.arg("-p").arg("helix-term");
    }

    let status = cross_cmd.status().unwrap();
    if !status.success() {
        eprintln!("[xtask] Cross-compile failed for {}.", port_name);
        process::exit(1);
    }

    eprintln!("[xtask] Successfully built {}!", port_name);

    let bin_name = match port_name {
        "ripgrep" => "rg",
        "helix" => "hx",
        _ => port_name,
    };
    source_dir
        .join("target")
        .join("x86_64-unknown-redox")
        .join("release")
        .join(bin_name)
}
