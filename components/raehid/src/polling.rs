//! Mouse polling-rate control (MasterChecklist L1564) — Concept §Input,
//! "gaming isn't a mode": let the user pick a mouse's report rate (125 Hz …
//! 8000 Hz) instead of accepting the device default.
//!
//! A USB interrupt endpoint reports at an interval encoded by `bInterval`, and
//! the encoding depends on device speed:
//!   * Low/Full speed — `bInterval` is in **1 ms frames**, so rate = 1000 / b;
//!     the ceiling is 1000 Hz (`bInterval = 1`).
//!   * High/Super speed — `bInterval` is **exponent + 1** in 125 µs microframes
//!     (period = 2^(bInterval-1) × 125 µs), so rate = 8000 / 2^(bInterval-1);
//!     the ceiling is 8000 Hz (`bInterval = 1`).
//!
//! The kernel xHCI driver (`kernel/src/xhci.rs`) already decodes a device's
//! `bInterval` into the xHCI `Interval` field when configuring the endpoint;
//! this module adds the *control* direction — pick a target rate, get the
//! `bInterval` to program — clamped to what the link speed allows.
//!
//! Validated against the live **Razer DeathAdder Essential** read off the
//! Athena (`lsusb`, 2026-06-28): Full-Speed, interrupt `bInterval = 1` → 1000 Hz.

/// USB link speed, as it affects interrupt-endpoint polling encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbSpeed {
    /// Low or Full speed — `bInterval` counts 1 ms frames (max 1000 Hz).
    LowFull,
    /// High or Super speed — `bInterval` is exponent+1 of 125 µs microframes
    /// (max 8000 Hz).
    HighSuper,
}

/// The polling rate (Hz) a given speed + endpoint `bInterval` yields.
pub fn binterval_to_rate_hz(speed: UsbSpeed, binterval: u8) -> u32 {
    let b = (binterval.max(1)) as u32;
    match speed {
        UsbSpeed::LowFull => 1000 / b,
        UsbSpeed::HighSuper => {
            let exp = (b - 1).min(15);
            8000 / (1u32 << exp)
        }
    }
}

/// The `bInterval` to program so the endpoint polls at the rate closest to —
/// and never exceeding — `target_hz`, clamped to what the link speed supports
/// (Full-Speed caps at 1000 Hz; High-Speed at 8000 Hz). This is what L1564's
/// "set my mouse to 1000 / 8000 Hz" control hands the xHCI endpoint config.
pub fn rate_hz_to_binterval(speed: UsbSpeed, target_hz: u32) -> u8 {
    let hz = target_hz.max(1);
    match speed {
        UsbSpeed::LowFull => {
            // bInterval = ceil(1000 / hz), so the actual rate never exceeds the
            // request; capped at 1000 Hz (bInterval 1) and 1 Hz (bInterval 255).
            let hz = hz.min(1000);
            let b = 1000_u32.div_ceil(hz);
            b.clamp(1, 255) as u8
        }
        UsbSpeed::HighSuper => {
            // rate = 8000 / 2^(bInterval-1): pick the smallest exponent whose
            // rate does not exceed the target. Cap 8000 Hz, exponent ≤ 15.
            let hz = hz.min(8000);
            let mut exp = 0u32;
            while exp < 15 && (8000_u32 >> exp) > hz {
                exp += 1;
            }
            (exp + 1) as u8
        }
    }
}

/// The discrete polling rates (Hz) the OS offers in the UI for a link speed.
pub fn supported_rates(speed: UsbSpeed) -> &'static [u32] {
    match speed {
        UsbSpeed::LowFull => &[125, 250, 500, 1000],
        UsbSpeed::HighSuper => &[125, 250, 500, 1000, 2000, 4000, 8000],
    }
}

#[cfg(test)]
mod polling_kat {
    use super::*;

    #[test]
    fn real_razer_deathadder_fs_binterval1_is_1000hz() {
        // Ground truth from the live device (lsusb on the Athena, 2026-06-28):
        // Razer DeathAdder Essential = Full-Speed, interrupt EP bInterval = 1.
        assert_eq!(binterval_to_rate_hz(UsbSpeed::LowFull, 1), 1000);
        // ...so to set that FS device to 1000 Hz we program bInterval = 1.
        assert_eq!(rate_hz_to_binterval(UsbSpeed::LowFull, 1000), 1);
    }

    #[test]
    fn fs_rate_control_steps_and_clamps() {
        // Standard FS mouse rates: 1000/500/250/125 Hz -> bInterval 1/2/4/8.
        assert_eq!(rate_hz_to_binterval(UsbSpeed::LowFull, 500), 2);
        assert_eq!(rate_hz_to_binterval(UsbSpeed::LowFull, 250), 4);
        assert_eq!(rate_hz_to_binterval(UsbSpeed::LowFull, 125), 8);
        assert_eq!(binterval_to_rate_hz(UsbSpeed::LowFull, 2), 500);
        // A Full-Speed link cannot reach 8000 Hz -> clamps to 1000 Hz (b=1).
        assert_eq!(rate_hz_to_binterval(UsbSpeed::LowFull, 8000), 1);
        // Round-trip every offered FS rate.
        for &r in supported_rates(UsbSpeed::LowFull) {
            let b = rate_hz_to_binterval(UsbSpeed::LowFull, r);
            assert_eq!(
                binterval_to_rate_hz(UsbSpeed::LowFull, b),
                r,
                "fs rate {}",
                r
            );
        }
    }

    #[test]
    fn hs_supports_8000hz_down_to_125() {
        // HS: bInterval 1 -> 8000 Hz, 4 -> 1000 Hz, 7 -> 125 Hz.
        assert_eq!(rate_hz_to_binterval(UsbSpeed::HighSuper, 8000), 1);
        assert_eq!(binterval_to_rate_hz(UsbSpeed::HighSuper, 1), 8000);
        assert_eq!(rate_hz_to_binterval(UsbSpeed::HighSuper, 1000), 4);
        assert_eq!(binterval_to_rate_hz(UsbSpeed::HighSuper, 4), 1000);
        assert_eq!(rate_hz_to_binterval(UsbSpeed::HighSuper, 125), 7);
        // Round-trip every offered HS rate.
        for &r in supported_rates(UsbSpeed::HighSuper) {
            let b = rate_hz_to_binterval(UsbSpeed::HighSuper, r);
            assert_eq!(
                binterval_to_rate_hz(UsbSpeed::HighSuper, b),
                r,
                "hs rate {}",
                r
            );
        }
    }
}
