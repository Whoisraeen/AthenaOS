# New OS Bug Discovery

**Date:** 2026-06-15
**Auditor:** Gemini (Agent)
**Scope:** Runtime analysis of `target/daemon-serial.log` from the virtual test environment, focusing on the kernel initialization and subsystem behavior.

This document identifies bugs or anomalies discovered during a runtime analysis of the OS boot process. It complements the static audit findings in `BUG_REPORT.md`.

---

## 1. ACPI Method Failures During Bring-Up

**Subsystem:** ACPI / Platform Bring-up
**Severity:** 🟡 Low (Environment Specific)

During the ACPI initialization and AML method invocation sequence, several expected platform methods are reported as missing or failing to execute.

**Log Excerpt:**
```
[acpi][warn] AML method \_PIC failed or not present: ValueDoesNotExist(AmlName([Root, Segment("_PIC")]))
[acpi][warn] AML method \_SB._PIC failed or not present: ValueDoesNotExist(AmlName([Root, Segment("_SB_"), Segment("_PIC")]))
[acpi][warn] AML method \_SB._REG failed or not present: ValueDoesNotExist(AmlName([Root, Segment("_SB_"), Segment("_REG")]))
[acpi][warn] AML method \_SB.PCI0._REG failed or not present: ValueDoesNotExist(AmlName([Root, Segment("_SB_"), Segment("PCI0"), Segment("_REG")]))
...
[acpi] Platform bring-up audit: 3 method(s) invoked, 10 failed/absent
```

**Analysis:**
While the system successfully falls back and completes ACPI initialization (finding 55 namespace devices and successfully routing IRQs), the failure to execute `_PIC` and `_REG` methods indicates that the ACPI interpreter or the platform-specific bring-up logic is encountering unexpected missing nodes in the DSDT, or it is failing to gracefully handle standard QEMU/Bochs ACPI tables.
According to the `MasterChecklist.md` and `Audit.md`, the lack of `_PIC` can cause issues with interrupt routing on real hardware (e.g., Beelink Athena).

**Recommended Action:**
Investigate the AML execution logic for `_PIC` and `_REG` in `kernel/src/acpi_full.rs` or the related ACPI parser. Ensure that the absence of these methods is expected on QEMU, and if so, demote the warning to an info/debug log. If the OS relies on these methods for real hardware, verify that the ACPI interpreter is correctly executing them when present.

---

## 2. Bootlog Persistence Disabled (Missing File)

**Subsystem:** Bootlog / Telemetry
**Severity:** 🟡 Low

The persistent bootlog subsystem fails to initialize because it cannot find the target file `BOOTLOG.TXT` on the EFI System Partition (ESP).

**Log Excerpt:**
```
[bootlog-persist] active device: no BOOTLOG.TXT (BOOTLOG.TXT not in root directory)
[bootlog-persist] BOOTLOG.TXT not found on any device — persistent log disabled.
[bootlog-persist]   Flash a current image (xtask bakes BOOTLOG.TXT into the ESP),
[bootlog-persist]   or create a 1 MiB B:\BOOTLOG.TXT on the NVMe ESP:
```

**Analysis:**
The system expects `BOOTLOG.TXT` to be pre-allocated on the ESP (NVMe nsid 1). It appears the `xtask` build process did not successfully create or deploy this file into the `target/nvme.img` ESP partition, or the FAT32 parser failed to locate it. This disables persistent crash and boot logging, which is critical for debugging on real hardware.

**Recommended Action:**
Update the `xtask` build script (`xtask/src/main.rs` or similar) to ensure `BOOTLOG.TXT` is created and baked into the ESP image during the `build` process.

---

## 3. VirtIO-GPU Scanout Creation Failure

**Subsystem:** GPU / Display
**Severity:** 🟠 Medium

The VirtIO-GPU driver fails to create a scanout, although the system gracefully falls back to keeping the UEFI GOP scanout active.

**Log Excerpt:**
```
[gpu] Found VirtioGpu GPU: vendor=0x1af4 device=0x1050 at 00:06.0
[gpu] Initializing VirtIO-GPU...
[gpu] VirtIO-GPU Virgl 3D feature negotiated
[gpu] VirtIO-GPU scanout creation failed, driver still usable
[ OK ] GPU driver initialized
[compositor] QEMU: keeping UEFI GOP scanout (skip Bochs VBE attach for visible display)
```

**Analysis:**
The VirtIO-GPU initialization encounters an error during the scanout creation phase (`VirtIO-GPU scanout creation failed`). This means the kernel cannot natively drive the VirtIO display via its own 2D/3D command streams for frame presentation, and is instead relying entirely on the pre-boot UEFI GOP framebuffer. This significantly degrades performance and prevents the use of advanced compositor features (like hardware cursor or page flipping) within the VM.

**Recommended Action:**
Investigate `kernel/src/gpu.rs` or the VirtIO-GPU specific implementation. The driver likely sends an invalid command to the VirtIO queue (e.g., `VIRTIO_GPU_CMD_SET_SCANOUT` or `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D`) or fails to correctly map the backing memory for the scanout resource.

---

## 4. Stale Page Mapping Recoveries During Compositor Surface Creation

**Subsystem:** Memory Management / TLB
**Severity:** 🟠 Medium (Potential Latent Issue)

During the creation of a compositor surface late in the boot process, the memory manager logs a massive flood of "recovering stale mapping" warnings.

**Log Excerpt:**
```
[mem] recovering stale mapping: page 0x39c000 was mapped to 0x7b626000, remapping to 0x7b626000
[mem] recovering stale mapping: page 0x39d000 was mapped to 0x7b627000, remapping to 0x7b627000
...
[mem] recovering stale mapping: page 0x3ff000 was mapped to 0x7b689000, remapping to 0x7b689000
```
(This occurs for hundreds of contiguous pages right before `DEBUG: create_surface start` and `DEBUG: alloc_contig_frames pages=150`).

**Analysis:**
The page table manipulation code is detecting that virtual pages in the `0x300000` range are already mapped to physical addresses in the `0x7b000000` range. The fact that the physical address it is *remapping* to is identical to the *mapped* address suggests that `map_to` is being called on pages that are already mapped, or the TLB/PageTable state is out of sync with the frame allocator.
This often happens when `allocate_contiguous_frames` re-issues memory that wasn't properly unmapped, or when a large allocation function incorrectly tries to map memory that the bootloader (or earlier kernel code) already mapped.

**Recommended Action:**
Audit the page mapping logic around `alloc_contig_frames` and `create_surface` in `kernel/src/compositor.rs` or the underlying memory manager. Ensure that memory returned by the frame allocator is checked against the page tables, and if it's already mapped, either reuse the mapping silently or ensure it was properly unmapped during the previous free.

---

## Conclusion regarding Mouse/Keyboard Issue

The user reported that their mouse and keyboard might not be working.

**Log Evidence:**
```
[ OK ] Input subsystem initialized (DualSense/Xbox/HID/RGB)
[ OK ] USB HID boot-keyboard + boot-mouse parsers (R05)
...
[xhci] HID boot keyboard: SET_CONFIGURATION + interrupt IN OK
[xhci] interrupt-IN doorbell ep_index=2 target=3
[xhci] armed 1 HID interrupt-IN endpoint(s) for live input
[xhci] HID input servicing thread spawned (task=TaskId(67))
[xhci] smoketest passed
[ OK ] USB core framework initialized
[usb-hid] smoketest kbd:   reports+=3 keydowns+=2 keyups+=2 -> PASS
[usb-hid] smoketest mouse: reports+=3 btn_dn+=1 btn_up+=1 motion+=1 wheel+=1 -> PASS
```

**Finding:**
The kernel logs show that the xHCI controller successfully enumerates the USB HID keyboard and mouse. The endpoints are configured, and the input servicing thread is spawned. Furthermore, the `usb-hid` smoketest passes, indicating that the kernel is successfully parsing HID reports.
If the keyboard and mouse are "not working" for the user, the issue is **not** in the low-level kernel driver initialization or interrupt routing. It is highly likely to be:
1. **User Space Routing:** The events are parsed by the kernel but not correctly routed to the active window/compositor surface (e.g., `raeshell` or the `first-boot setup wizard`).
2. **Virtualization Issue:** The QEMU window might not be capturing the host's mouse/keyboard input correctly. (The user must click into the window or use the QEMU grab hotkey, usually Ctrl+Alt+G).
3. **Interrupt Delivery (Runtime):** While the smoketest passes (which often synthesizes events), the actual runtime hardware interrupts from the xHCI controller might not be firing or being acknowledged correctly after boot.
