# Implementation Plan: Phase 1 - Hardware Security & Input (IOMMU & USB)

## Background & Motivation
AthenaOS is built on a "security by default" driver isolation model where every driver runs in its own protection domain with IOMMU enforcement. Currently, the kernel isolates drivers at the CPU ring level (Ring 3 vs Ring 0) and via capability handles, but a malicious or buggy driver could still use DMA (Direct Memory Access) to overwrite kernel memory. To solidify the system's security foundation before building complex user-space drivers, we must implement IOMMU. 

Furthermore, the OS currently relies on legacy PS/2 for input. For a embodiment-first OS in 2026, a modern USB stack (starting with xHCI and HID) is mandatory for supporting high-polling-rate mice, keyboards, and eventually game controllers.

## Scope & Impact
This phase focuses on core hardware infrastructure. It is highly complex and carries significant risk of breaking the boot process.
**In Scope:**
*   **ACPI DMAR parsing:** Detecting Intel VT-d (IOMMU) hardware capabilities via ACPI tables.
*   **IOMMU Initialization:** Enabling VT-d and creating basic root and context entries to block default pass-through DMA.
*   **PCIe xHCI Discovery:** Finding the USB 3.0 eXtensible Host Controller Interface via PCI enumeration.
*   **xHCI Driver (Kernel-level for now):** Initializing the controller, setting up command and event rings, and enumerating the root hub.
*   **USB HID Subsystem:** Basic polling of connected USB mice/keyboards and routing events to the IPC system.

**Out of Scope:**
*   AMD-Vi support (we will target Intel VT-d first as a proof-of-concept, given QEMU's default IOMMU is typically VT-d if configured).
*   Moving the xHCI driver to user-space (we will build it in-kernel first to ensure the hardware interaction works, then migrate it across the capability boundary in a later phase once IOMMU context mapping is mature).
*   USB Mass Storage or Audio devices.

## Proposed Solution

### IOMMU (Intel VT-d)
1.  Parse the `DMAR` (DMA Remapping) ACPI table in `kernel/src/acpi.rs`.
2.  Create a new `kernel/src/iommu.rs` module.
3.  Map the IOMMU register space.
4.  Implement a basic page table structure for VT-d (which is similar to, but distinct from, CPU page tables).
5.  By default, block all DMA. Create an API (`iommu::map_dma(pci_device, phys_addr, size)`) that drivers must call to explicitly allow DMA for specific buffers.

### USB Stack (xHCI)
1.  Update `kernel/src/pci.rs` to identify Class `0x0C` (Serial Bus), Subclass `0x03` (USB), Prog IF `0x30` (xHCI).
2.  Create a new `kernel/src/usb/xhci.rs` module.
3.  Allocate contiguous physical memory for the DCBAA (Device Context Base Address Array), Command Ring, and Event Ring. 
4.  *Crucially*, use the new `iommu::map_dma` API to allow the xHCI controller to read/write these structures.
5.  Implement port enumeration and device reset.

### USB HID
1.  Create `kernel/src/usb/hid.rs`.
2.  Once a device is enumerated by xHCI, read its descriptors. If it is an HID device (keyboard/mouse), configure an interrupt transfer ring.
3.  Translate HID reports into standard `AthUI` input events and push them to the existing keyboard IPC channel.

## Alternatives Considered
*   **User-space xHCI immediately:** Attempting to build IOMMU, capability-based MMIO/IRQ routing, and a complex xHCI driver all at once across the user-space boundary is too risky. Building xHCI in-kernel first proves the hardware logic and IOMMU mapping; we can lift-and-shift it to a user-space ELF later.
*   **EHCI / UHCI (USB 2.0/1.1):** Ignoring legacy controllers simplifies the stack immensely. xHCI is universal on modern hardware.

## Implementation Plan

### Step 1: DMAR Parsing & IOMMU Initialization
*   Update `acpi` crate dependencies or internal parser to find the `DMAR` table.
*   Extract the base address of the IOMMU registers.
*   Write initialization sequence: check capabilities, allocate Root Entry Table.

### Step 2: IOMMU DMA API
*   Implement `IommuPageTable` allocator.
*   Create `iommu::allow_device_dma(vendor, device, phys_start, pages)`.
*   Test by applying it to the existing VirtIO block driver (which uses DMA). If VirtIO breaks, IOMMU is working but misconfigured. Fix until VirtIO works under IOMMU.

### Step 3: xHCI Controller Setup
*   Enumerate xHCI via PCI.
*   Map MMIO registers.
*   Allocate and initialize DCBAA and rings, utilizing the IOMMU API.
*   Start the controller and verify it enters the 'Run' state.

### Step 4: USB Enumeration & HID
*   Send `Enable Slot` and `Address Device` commands.
*   Parse device descriptors.
*   Implement a basic HID report parser for standard boot protocol keyboards and mice.

## Verification
*   **Compilation:** Frequent `cargo check` and `cargo build -p xtask` runs.
*   **Execution:** Booting in QEMU with `-machine q35,kernel-irqchip=split -device intel-iommu,intremap=on,caching-mode=on -device qemu-xhci` to simulate modern hardware.
*   **Validation:** 
    *   VirtIO disk continues to function, proving IOMMU pass-through is correct.
    *   Pressing keys on a simulated USB keyboard produces output, proving the full xHCI -> HID -> IPC pipeline.

## Migration & Rollback
*   IOMMU and xHCI will be controlled via feature flags or boot arguments initially. If the system double-faults during boot, we can disable IOMMU initialization and fall back to the legacy PS/2 driver to regain a bootable state.