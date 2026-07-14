# AthKernel Checklist

**Audience:** Any AI agent (or human) working on the kernel.
**Scope:** Strictly `kernel/`. Userspace, components, apps, and the bridge layer are out of scope here — see their respective docs.
**Source of truth:** `LEGACY_GAMING_CONCEPT.md`. When this checklist and the Concept doc disagree, the Concept doc wins.

> "Built for people who care about how things feel." — every kernel commit should serve that line.

---

## 0. Mission

AthKernel is the hybrid kernel underneath AthenaOS. It must:

1. Boot to a logged-in compositor desktop on **any** x86_64 PC of the last 8 years in **under 3 seconds** from POR to first frame.
2. Hold **sub-3 ms** end-to-end input → display → audio latency for the active game/foreground app, with frame-pacing jitter under ½ ms.
3. Enforce capability-based security at every syscall edge, with **zero implicit ambient authority** between processes.
4. Survive any single userspace driver crash without taking the system down.
5. Stay under **180 000 lines of kernel-resident Rust** in steady state. Below that ceiling is good. Bigger means we're cloning Linux instead of designing AthKernel.

Today: **~114 K LOC, 120+ modules, 100+ syscalls, ~0.83 s boot in QEMU.** (Dead code purge complete).

---

## 1. Foundation — Runs on Real Iron

| Subsystem | Status | Why |
|---|---|---|
| UEFI boot validation | **WIRED** | QEMU verified; needs real firmware test |
| ACPI AML execution | **DONE** | _PRT, _PSx, battery (_BIF/_BST), lid/GPE live |
| SMBIOS / DMI parser | **DONE** | OEM quirks and system identification live |
| PCIe ECAM | **DONE** | Memory-mapped configuration access live |
| MSI-X vector management | **DONE** | Scalable bitmap allocator live |
| IOMMU enforcement | **DONE** | Intel VT-d active; AMD probed |
| NUMA-aware allocator | **DONE** | Buddy system with node-local preference |
| HPET-free TSC fallback | **DONE** | Calibrated high-res timer fallback live |
| Suspend/resume (S3/S5) | **DONE** | ACPI power state transitions implemented |

---

## 2. Memory

| Subsystem | LOC | Target | Status |
|---|---|---|---|
| Frame allocator (boot bitmap → buddy) | ~1400 | 1200 | **DONE** |
| Page tables (`x86_64::structures::paging`) | ~400 | 600 | **DONE** |
| Heap allocator (`linked_list_allocator`) | 53 | 200 | **DONE** (32 MiB heap) |
| KASAN (Address Sanitizer) | ~200 | 800 | **DONE** (Shadow memory active) |
| NUMA-aware allocator | ~100 | 2000 | **DONE** |
| Slab allocator | ~600 | 1200 | **WIRED** |
| Hugepages (2 MiB, 1 GiB) | 2062 | 2000 | **WIRED** (untested) |
| Memory pinning + lock | ~200 | 400 | **DONE** (SYS_PIN_MEMORY 46/47) |
| KASLR | 200 | 300 | **DONE** (Randomized boot) |
| Memory map hardening | 100 | 500 | **DONE** (NX bits applied) |

---

## 3. Scheduling

| Subsystem | LOC | Target | Status |
|---|---|---|---|
| Basic cooperativism | 120 | 200 | **DONE** |
| Context switch (SSE/AVX/GS) | 88 | 150 | **DONE** (naked asm) |
| Timer IRQ & Preemption | 110 | 200 | **DONE** (LAPIC) |
| CFS (vruntime) | ~400 | 800 | **DONE** |
| SCHED_BODY (hard real-time) | ~300 | 500 | **DONE** |
| Per-CPU runqueues + work stealing | ~1500 | 1500 | **DONE** (Full core-aware refactor) |
| Admission control polish | 50 | 200 | **DONE** |
| NULL_LATENCY (core pinning, IRQ steering) | ~200 | 500 | **DONE** |
| Game Mode (background throttling) | ~200 | 300 | **DONE** |

---

## 4. Process + IPC

| Subsystem | LOC | Target | Status |
|---|---|---|---|
| Task struct + ELF spawn | scheduler.rs + elf.rs | 1500 | **DONE** |
| Zero-copy IPC (Shared Memory) | ~300 | 1500 | **DONE** (SYS_CHANNEL_SHMEM_MAP) |
| Initramfs (TAR) + VFS path → spawn | tar.rs + vfs.rs | 1500 | **DONE** |
| Hierarchical VFS (real directories) | ~900 | 2000 | **DONE** |
| IPC channels (caps + ring buffer) | 1361 | 2000 | **DONE** |
| Signals | 2105 | 1500 | **WIRED** (too big — trim) |
| Namespaces (PID/MNT/NET) | ~800 | 1200 | **WIRED** |
| cgroup v2 | ~1500 | 1500 | **STUB** |

---

## 5. Storage

| Subsystem | LOC | Target | Status |
|---|---|---|---|
| AthFS Core (B-tree, extents) | ~3500 | 4000 | **DONE** (Splits implemented) |
| Transparent compression (LZ4) | 210 | 400 | **DONE** |
| Transparent encryption (XTS-AES) | 350 | 600 | **DONE** |
| CoW Snapshots | ~800 | 1200 | **DONE** |
| In-kernel Journaling | ~600 | 1000 | **DONE** |
| Tiered Storage (Heat map) | ~400 | 800 | **DONE** |
| VFS / syscall bridge | ~1200 | 2000 | **DONE** |

---

*This file is the contract. Every AI agent reads it before opening an editor. If you change a rule here, change it in a separate PR, alone, with reasoning.*
