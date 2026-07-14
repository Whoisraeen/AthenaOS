//! Chip identification — decode `NV_PMC_BOOT_0` into an NVIDIA architecture
//! family, chipset id and revision, and classify the firmware each generation
//! needs. This is the first thing `nvkm_device_ctor` does on every NVIDIA GPU,
//! and it is a pure function of one register value, so it is fully host-tested.
//!
//! ## The decode (canonical nouveau form)
//! `chipset  = (boot0 & 0x1ff0_0000) >> 20`  — e.g. `0x134` for a GP104,
//! `revision = boot0 & 0x0000_00ff`,
//! and the architecture family is `chipset & 0x1f0` (Fermi `0x0c0`, Kepler
//! `0x0e0..0x100`, Maxwell `0x110/0x120`, Pascal `0x130`, Volta `0x140`, Turing
//! `0x160`, Ampere `0x170`, Ada `0x190`). A read of `0` (no device / decode
//! aperture unmapped) or all-ones (dead read) is rejected.

use crate::regs;

/// The minimal hardware seam the chip logic needs: 32-bit MMIO register access
/// into BAR0. `nvidiad` implements this over the LinuxKPI shim (real `readl`);
/// the host tests implement it over a mock register file. Mirrors
/// `raeen_amdgpu`'s `GpuOps` so both drivers share one testing discipline.
pub trait GpuOps {
    /// Read a 32-bit register at byte offset `off` in BAR0.
    fn reg_read(&mut self, off: u32) -> u32;
    /// Write `val` to the 32-bit register at byte offset `off` in BAR0.
    fn reg_write(&mut self, off: u32, val: u32);
}

/// NVIDIA GPU architecture family (`card_type` in nouveau terms). Ordered so
/// `>=` comparisons express "this generation or newer".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NvArch {
    /// GF1xx — the oldest generation this driver recognises.
    Fermi,
    /// GK1xx.
    Kepler,
    /// GM1xx/GM2xx.
    Maxwell,
    /// GP1xx.
    Pascal,
    /// GV1xx.
    Volta,
    /// TU1xx — first generation gated behind GSP-RM for a modern init.
    Turing,
    /// GA1xx.
    Ampere,
    /// AD1xx.
    Ada,
    /// A plausible NVIDIA device whose family nibble is not one we map yet
    /// (a newer generation, or an ancient pre-Fermi part).
    Unknown,
}

impl NvArch {
    /// Human-readable family name for logs.
    pub const fn name(self) -> &'static str {
        match self {
            NvArch::Fermi => "Fermi",
            NvArch::Kepler => "Kepler",
            NvArch::Maxwell => "Maxwell",
            NvArch::Pascal => "Pascal",
            NvArch::Volta => "Volta",
            NvArch::Turing => "Turing",
            NvArch::Ampere => "Ampere",
            NvArch::Ada => "Ada Lovelace",
            NvArch::Unknown => "unknown",
        }
    }

    /// Map the architecture family nibble (`chipset & 0x1f0`) to a family. This
    /// is the exact switch `nvkm_device_ctor` uses.
    const fn from_family_nibble(nibble: u32) -> NvArch {
        match nibble {
            0x0c0 | 0x0d0 => NvArch::Fermi,
            0x0e0 | 0x0f0 | 0x100 => NvArch::Kepler,
            0x110 | 0x120 => NvArch::Maxwell,
            0x130 => NvArch::Pascal,
            0x140 => NvArch::Volta,
            0x160 => NvArch::Turing,
            0x170 => NvArch::Ampere,
            0x190 => NvArch::Ada,
            _ => NvArch::Unknown,
        }
    }

    /// What external firmware this generation requires for *full* bring-up. See
    /// [`FwRequirement`]. Display modeset is reachable natively on every pre-GSP
    /// part regardless of this; the tier describes acceleration / full init.
    pub const fn firmware_requirement(self) -> FwRequirement {
        match self {
            // Pre-signing era: nouveau brought Fermi up (accel included) with no
            // NVIDIA-signed blobs.
            NvArch::Fermi => FwRequirement::NoFirmware,
            // Signed falcon microcode (PMU / FECS / GPCCS) required for accel.
            NvArch::Kepler | NvArch::Maxwell | NvArch::Pascal | NvArch::Volta => {
                FwRequirement::SignedUcode
            }
            // Full initialisation routes through the GSP-RM firmware coprocessor.
            NvArch::Turing | NvArch::Ampere | NvArch::Ada => FwRequirement::GspRm,
            // Unknown: assume the hardest wall so we never overclaim.
            NvArch::Unknown => FwRequirement::GspRm,
        }
    }
}

/// The external-firmware wall a generation imposes on a from-scratch driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FwRequirement {
    /// No NVIDIA-signed firmware needed — the driver can bring the GPU up
    /// (including acceleration) on its own.
    NoFirmware,
    /// Acceleration requires NVIDIA-signed falcon microcode (PMU/FECS/GPCCS).
    /// Display modeset still works without it.
    SignedUcode,
    /// Full initialisation is mediated by the GSP-RM firmware coprocessor
    /// (Turing and later). Without running GSP-RM the driver cannot complete
    /// engine bring-up — the hard wall for a native driver.
    GspRm,
}

impl FwRequirement {
    /// One-line description for logs.
    pub const fn describe(self) -> &'static str {
        match self {
            FwRequirement::NoFirmware => "no external firmware required",
            FwRequirement::SignedUcode => "acceleration needs NVIDIA-signed falcon microcode",
            FwRequirement::GspRm => "full init requires the GSP-RM firmware coprocessor",
        }
    }
}

/// A decoded NVIDIA chip identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvIdentity {
    /// The raw `NV_PMC_BOOT_0` value the identity was decoded from.
    pub boot0: u32,
    /// Chipset id, e.g. `0x134` (GP104). 9-bit value.
    pub chipset: u16,
    /// Silicon revision (`boot0 & 0xff`).
    pub revision: u8,
    /// Architecture family.
    pub arch: NvArch,
}

impl NvIdentity {
    /// The firmware wall this part imposes (delegates to [`NvArch`]).
    pub const fn firmware_requirement(self) -> FwRequirement {
        self.arch.firmware_requirement()
    }
}

/// Decode a raw `NV_PMC_BOOT_0` value. Returns `None` when the value cannot be a
/// live NVIDIA identification register: `0x0000_0000` (nothing mapped / no
/// device) or `0xffff_ffff` (a dead read off an unpowered or absent aperture),
/// and when the architecture strap bits are clear (pre-NV10 / not decodable).
pub fn decode_boot0(boot0: u32) -> Option<NvIdentity> {
    // Reject the two classic "no valid read" sentinels outright.
    if boot0 == 0x0000_0000 || boot0 == 0xffff_ffff {
        return None;
    }
    // nvkm_device_ctor only decodes when the architecture strap is set.
    if boot0 & 0x1f00_0000 == 0 {
        return None;
    }
    let chipset = ((boot0 & 0x1ff0_0000) >> 20) as u16;
    let revision = (boot0 & 0x0000_00ff) as u8;
    let arch = NvArch::from_family_nibble((chipset as u32) & 0x1f0);
    Some(NvIdentity {
        boot0,
        chipset,
        revision,
        arch,
    })
}

/// Read `NV_PMC_BOOT_0` through `ops` and decode it. This is `nvidiad`'s first
/// bring-up stage: it needs only BAR0 mapped, no power/clock setup. Returns
/// `None` if the register does not read back as a valid NVIDIA identity (see
/// [`decode_boot0`]).
pub fn identify(ops: &mut impl GpuOps) -> Option<NvIdentity> {
    let boot0 = ops.reg_read(regs::NV_PMC_BOOT_0);
    decode_boot0(boot0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock register file: BAR0 offset -> value. Any unset register reads 0.
    struct MockGpu {
        boot0: u32,
    }
    impl GpuOps for MockGpu {
        fn reg_read(&mut self, off: u32) -> u32 {
            match off {
                regs::NV_PMC_BOOT_0 => self.boot0,
                _ => 0,
            }
        }
        fn reg_write(&mut self, _off: u32, _val: u32) {}
    }

    /// Real `NV_PMC_BOOT_0` values (chipset in the 0x1ff0_0000 field, rev 0xa1)
    /// across the whole modern line. Each row asserts the exact decode.
    #[test]
    fn decode_known_parts() {
        let cases: &[(u32, u16, NvArch, FwRequirement)] = &[
            // Fermi GF100 (GTX 480)
            (0x0c0000a1, 0x0c0, NvArch::Fermi, FwRequirement::NoFirmware),
            // Kepler GK110 (GTX 780 Ti)
            (
                0x0f0000a1,
                0x0f0,
                NvArch::Kepler,
                FwRequirement::SignedUcode,
            ),
            // Maxwell GM204 (GTX 980)
            (
                0x124000a1,
                0x124,
                NvArch::Maxwell,
                FwRequirement::SignedUcode,
            ),
            // Pascal GP104 (GTX 1080)
            (
                0x134000a1,
                0x134,
                NvArch::Pascal,
                FwRequirement::SignedUcode,
            ),
            // Volta GV100 (Titan V)
            (0x140000a1, 0x140, NvArch::Volta, FwRequirement::SignedUcode),
            // Turing TU104 (RTX 2080)
            (0x164000a1, 0x164, NvArch::Turing, FwRequirement::GspRm),
            // Ampere GA102 (RTX 3090)
            (0x172000a1, 0x172, NvArch::Ampere, FwRequirement::GspRm),
            // Ada AD102 (RTX 4090)
            (0x192000a1, 0x192, NvArch::Ada, FwRequirement::GspRm),
        ];
        for &(boot0, chipset, arch, fw) in cases {
            let id = decode_boot0(boot0).expect("valid part must decode");
            assert_eq!(id.chipset, chipset, "chipset for boot0={boot0:#010x}");
            assert_eq!(id.revision, 0xa1, "revision for boot0={boot0:#010x}");
            assert_eq!(id.arch, arch, "arch for boot0={boot0:#010x}");
            assert_eq!(
                id.firmware_requirement(),
                fw,
                "fw tier for boot0={boot0:#010x}"
            );
        }
    }

    #[test]
    fn revision_is_low_byte() {
        // GP104 with revision 0xa2 rather than 0xa1.
        let id = decode_boot0(0x134000a2).unwrap();
        assert_eq!(id.chipset, 0x134);
        assert_eq!(id.revision, 0xa2);
    }

    #[test]
    fn dead_and_absent_reads_reject() {
        // All-ones (dead aperture) and zero (nothing mapped) must NOT decode as
        // a phantom device — this is the guard the daemon relies on to know a
        // real NVIDIA GPU answered.
        assert_eq!(decode_boot0(0xffff_ffff), None);
        assert_eq!(decode_boot0(0x0000_0000), None);
        // Architecture strap clear (pre-NV10 / bogus low value) also rejects.
        assert_eq!(decode_boot0(0x0000_00a1), None);
    }

    #[test]
    fn future_family_is_unknown_not_a_panic() {
        // A hypothetical post-Ada family nibble (0x1b0) decodes as a device but
        // with Unknown arch and the conservative GSP wall — never a panic, never
        // a false NoFirmware claim.
        let id = decode_boot0(0x1b0000a1).unwrap();
        assert_eq!(id.arch, NvArch::Unknown);
        assert_eq!(id.firmware_requirement(), FwRequirement::GspRm);
    }

    #[test]
    fn identify_reads_boot0_through_ops() {
        let mut gpu = MockGpu { boot0: 0x164000a1 };
        let id = identify(&mut gpu).expect("mock RTX 2080 must identify");
        assert_eq!(id.arch, NvArch::Turing);
        assert_eq!(id.chipset, 0x164);

        // An unpowered aperture (all-ones) must fail identification.
        let mut dead = MockGpu { boot0: 0xffff_ffff };
        assert_eq!(identify(&mut dead), None);
    }

    #[test]
    fn arch_ordering_expresses_generation() {
        // The Ord derive must place newer generations higher so `>=` reads as
        // "this generation or newer" (used to gate GSP-only code paths).
        assert!(NvArch::Turing > NvArch::Pascal);
        assert!(NvArch::Ada > NvArch::Turing);
        assert!(NvArch::Fermi < NvArch::Kepler);
    }
}
