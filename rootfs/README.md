# rootfs/ — Linux dynamic-linking assets (ld.so support, WIP)

These are real glibc shared objects + a test binary, sourced from the Athena
Arch box, staged for **dynamic-linking (ld.so / PT_INTERP) support** so RaeenOS
can run *stock* dynamically-linked Linux binaries (not just static ones).

| path | what | size |
|---|---|---|
| `lib64/ld-linux-x86-64.so.2` | the ELF interpreter (dynamic loader) | 246 KB |
| `usr/lib/libc.so.6` | glibc shared object (DT_NEEDED of nearly everything) | 2.2 MB |
| `bin/dh` | a trivial dynamically-linked `printf("dyn-hello-ok")` test binary | 16 KB |

**Status:** assets staged; the kernel-side PT_INTERP loader + the xtask initramfs
bundling + the `/lib64`//usr/lib openat resolution are NOT yet implemented. See
the design (memory: ld-so-dynamic-linking-design) for the full plan:
1. xtask bundles this `rootfs/` tree into `initramfs.tar` (like `firmware/`).
2. `vfs::open_path_exact` resolves `/lib64/*`, `/usr/lib/*`, `/bin/*`.
3. `Task::new_linux_elf` detects PT_INTERP, loads the interpreter as a PIE at a
   high base, sets entry=interp and AT_BASE=interp_base.
4. `linux_exec("/bin/dh")` → ld.so loads → opens libc.so.6 → relocates → runs.

The first run will surface glibc-startup gaps (TLS/IFUNC/vDSO/syscalls) to fill
via the Athena strace oracle.
