//! ACPI bringup — find the RSDP, parse the SDTs, and surface what the kernel
//! needs from the platform tables (LAPIC + IOAPIC physaddrs, CPU count).
//!
//! We use the `acpi` crate (parsing) and feed the results to `crate::apic`.

use acpi::{AcpiHandler, AcpiTables, PhysicalMapping};
use core::ptr::NonNull;

#[derive(Clone)]
pub struct AthenaAcpiHandler;

impl AcpiHandler for AthenaAcpiHandler {
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<Self, T> {
        // We rely on the bootloader's physical-memory-offset window: every
        // physical byte is also reachable at `phys + PHYS_MEM_OFFSET`. ACPI's
        // tables all live in low memory so they're guaranteed mapped.
        let offset = *crate::memory::PHYS_MEM_OFFSET
            .get()
            .expect("PHYS_MEM_OFFSET not initialized");
        let virt = offset + physical_address as u64;

        let ptr = NonNull::new(virt.as_mut_ptr()).unwrap();

        PhysicalMapping::new(physical_address, ptr, size, size, self.clone())
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {
        // The offset mapping is permanent for the kernel's lifetime; nothing to do.
    }
}

/// Parse the ACPI tables, bring up the LAPIC + IOAPIC, and return the list
/// of non-disabled Application Processor APIC ids that the SMP module should
/// start. Returns an empty vec on uniprocessor systems or any parse failure.
pub fn init(rsdp_addr: u64) -> alloc::vec::Vec<u8> {
    use alloc::vec::Vec;
    let handler = AthenaAcpiHandler;
    crate::serial_println!("[acpi] Initializing ACPI from RSDP @ {:#x}", rsdp_addr);

    let tables = match unsafe { AcpiTables::from_rsdp(handler, rsdp_addr as usize) } {
        Ok(t) => t,
        Err(e) => {
            crate::serial_println!("[FAIL] Failed to parse ACPI tables: {:?}", e);
            return Vec::new();
        }
    };
    crate::serial_println!("[ OK ] ACPI tables parsed");

    let platform = match acpi::PlatformInfo::new(&tables) {
        Ok(p) => p,
        Err(e) => {
            crate::serial_println!("[WARN] Failed to read platform info: {:?}", e);
            return Vec::new();
        }
    };

    // Parse HPET
    match acpi::HpetInfo::new(&tables) {
        Ok(hpet_info) => {
            crate::hpet::init(hpet_info.base_address as u64);
        }
        Err(e) => {
            crate::serial_println!("[WARN] HPET info not found in ACPI: {:?}", e);
            crate::hpet::note_absent();
        }
    }

    let apic_info = match platform.interrupt_model {
        acpi::InterruptModel::Apic(ref a) => a,
        _ => {
            crate::serial_println!("[WARN] InterruptModel is not APIC; falling back to PIC");
            return Vec::new();
        }
    };

    crate::serial_println!("[acpi] Local APIC @ {:#x}", apic_info.local_apic_address,);

    // Pull the BSP's APIC id out of processor_info so we know which CPU to
    // aim the IOAPIC redirection entries at. Also collect every non-disabled
    // AP's apic id for the SMP bringup pass.
    let mut ap_apic_ids: Vec<u8> = Vec::new();
    let bsp_apic_id: u8 = if let Some(ref proc_info) = platform.processor_info {
        crate::serial_println!(
            "[acpi] BSP APIC ID: {}",
            proc_info.boot_processor.local_apic_id,
        );
        let mut cpu_count = 1u32;
        for ap in proc_info.application_processors.iter() {
            if !matches!(ap.state, acpi::platform::ProcessorState::Disabled) {
                cpu_count += 1;
                ap_apic_ids.push(ap.local_apic_id as u8);
            }
        }
        crate::serial_println!("[acpi] Total active CPUs: {}", cpu_count);
        proc_info.boot_processor.local_apic_id as u8
    } else {
        crate::serial_println!("[WARN] processor_info missing; defaulting BSP APIC id to 0");
        0
    };

    // 1) LAPIC.
    crate::apic::init_bsp(apic_info.local_apic_address as u64);

    // Phase 1.4: Inform firmware we are switching to APIC mode via _PIC(1).
    // Requirement 1: Wrap AML calls to ensure failure logs [WARN] but does not panic.
    crate::acpi_full::init(rsdp_addr);
    {
        use aml::{value::Args, AmlValue};
        let args = Args::from_list(alloc::vec![AmlValue::Integer(1)]).unwrap_or(Args::default());

        // Audit: Attempt to call \_PIC or \_SB._PIC
        let _ = crate::acpi_full::safe_evaluate_method("\\_PIC", args.clone());
        let _ = crate::acpi_full::safe_evaluate_method("\\_SB._PIC", args);
    }

    // 2) IOAPIC(s). For now we only bring up the first one — most chipsets
    // have a single IOAPIC and any legacy IRQ we care about lives behind it.
    if let Some(first_io) = apic_info.io_apics.first() {
        crate::serial_println!(
            "[acpi] IOAPIC id={} @ {:#x} gsi_base={}",
            first_io.id,
            first_io.address,
            first_io.global_system_interrupt_base,
        );
        crate::apic::init_ioapic(
            first_io.address as u64,
            first_io.global_system_interrupt_base,
            bsp_apic_id,
            &apic_info.interrupt_source_overrides,
        );
    } else {
        crate::serial_println!("[WARN] MADT lists no IOAPIC; cannot route external IRQs.");
    }

    // Note any InterruptSourceOverride entries so we know if standard ISA
    // mapping (IRQ N -> GSI N) doesn't hold. We don't yet ACT on them, but
    // surfacing them in the log makes debugging easier.
    for iso in apic_info.interrupt_source_overrides.iter() {
        crate::serial_println!(
            "[acpi] ISA IRQ {} overridden -> GSI {} (polarity={:?} trigger={:?})",
            iso.isa_source,
            iso.global_system_interrupt,
            iso.polarity,
            iso.trigger_mode,
        );
    }

    ap_apic_ids
}
