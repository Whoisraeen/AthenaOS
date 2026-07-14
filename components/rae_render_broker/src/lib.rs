//! `rae_render_broker` — kernel-side `/dev/dri/renderD128` broker policy.
//!
//! Concept §RaeShield: "the anti-cheat answer *without* giving vendors ring 0."
//! A game reaches the GPU through this capability-brokered render node, never
//! through raw MMIO/DMA. This crate is the **fail-closed heart** of the broker's
//! `ioctl()` forwarding: given a raw DRM ioctl request from an untrusted client,
//! decide whether it is permitted, how many bytes to marshal, and in which
//! direction — using the broker's *own* allowlist, never the client's `_IOC`
//! bits. See `docs/research/renderD128-broker-design.md` (step 2 of the owner's
//! GPU vertical-slice plan) and `docs/gpu-oracle/ATHENA-AMDGPU-DRM-ABI-20260711.md`.
//!
//! Pure logic, no_std, no allocation, no hardware — so it is proven by host KATs
//! (`cargo test -p rae_render_broker`) before any kernel wiring, per the project
//! proof ladder.

#![no_std]

/// Linux `_IOC(dir,type,nr,size)` field widths and shifts (asm-generic/ioctl.h).
/// `nr` 0–7, `type` 8–15, `size` 16–29 (14 bits), `dir` 30–31.
const NR_SHIFT: u32 = 0;
const TYPE_SHIFT: u32 = 8;
const SIZE_SHIFT: u32 = 16;
const DIR_SHIFT: u32 = 30;
const NR_MASK: u32 = 0xff;
const TYPE_MASK: u32 = 0xff;
const SIZE_MASK: u32 = 0x3fff;
const DIR_MASK: u32 = 0x3;

/// `_IOC` direction bits. `WRITE` = userspace→kernel (kernel reads from user),
/// `READ` = kernel→userspace (kernel writes to user); `_IOWR` sets both.
pub const IOC_NONE: u8 = 0;
pub const IOC_WRITE: u8 = 1;
pub const IOC_READ: u8 = 2;

/// DRM ioctl type magic (`'d'`) and the AMDGPU driver command base.
pub const DRM_IOCTL_TYPE: u8 = 0x64; // b'd'
pub const DRM_COMMAND_BASE: u8 = 0x40;

/// Largest struct the broker will marshal through the shared frame. The biggest
/// registered command is `GEM_METADATA` (288 bytes); 512 leaves head-room while
/// bounding the per-ioctl copy so a client can never provoke an unbounded copy.
pub const MAX_PAYLOAD: u16 = 512;

/// Decoded Linux `_IOC` request word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ioctl {
    pub dir: u8,
    pub type_: u8,
    pub nr: u8,
    pub size: u16,
}

/// Decode a raw 32-bit ioctl request into its `_IOC` fields.
pub const fn decode(req: u32) -> Ioctl {
    Ioctl {
        dir: ((req >> DIR_SHIFT) & DIR_MASK) as u8,
        type_: ((req >> TYPE_SHIFT) & TYPE_MASK) as u8,
        nr: ((req >> NR_SHIFT) & NR_MASK) as u8,
        size: ((req >> SIZE_SHIFT) & SIZE_MASK) as u16,
    }
}

/// Re-encode `_IOC` fields into a raw request (the inverse of [`decode`]) — used
/// by tests and to synthesize the canonical form when logging.
pub const fn encode(dir: u8, type_: u8, nr: u8, size: u16) -> u32 {
    ((dir as u32 & DIR_MASK) << DIR_SHIFT)
        | ((type_ as u32 & TYPE_MASK) << TYPE_SHIFT)
        | ((size as u32 & SIZE_MASK) << SIZE_SHIFT)
        | ((nr as u32 & NR_MASK) << NR_SHIFT)
}

/// The copy the broker must perform through the shared frame, taken from the
/// broker's allowlist — NOT from the client's (untrusted, and per-Mesa-variable)
/// `_IOC` dir bits. `In` = copy the struct to the daemon; `Out` = copy the
/// daemon's reply back; `InOut` = both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyDir {
    None,
    In,
    Out,
    InOut,
}

/// One permitted render-node command. `nr` is the full DRM command number
/// (`DRM_COMMAND_BASE + amdgpu_nr` for AMDGPU ioctls); `size` is the exact
/// struct size the client must present; `copy` is the canonical marshal plan
/// (from the libdrm header encoding, which is authoritative over Mesa's variant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoctlSpec {
    pub nr: u8,
    pub size: u16,
    pub copy: CopyDir,
    pub name: &'static str,
}

/// What a permitted request resolves to: the broker's marshal plan. Deliberately
/// carries no direction from the client — two `GEM_VA` encodings that differ only
/// in `_IOC` dir resolve to the *same* `Resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub nr: u8,
    pub size: u16,
    pub copy: CopyDir,
    pub name: &'static str,
}

/// Fail-closed reasons a request is refused before it can reach the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchError {
    /// `type_ != 'd'` — not a DRM ioctl at all.
    WrongType,
    /// The command number is not on the render-node allowlist.
    UnknownCommand,
    /// The presented struct size does not match the registered command.
    SizeMismatch,
    /// The registered struct exceeds `MAX_PAYLOAD` (a build/allowlist invariant;
    /// checked at runtime so a bad table addition can never widen the copy).
    PayloadTooLarge,
}

impl DispatchError {
    /// The negated errno the broker returns to the client for this refusal.
    /// `-ENOTTY` for a non-DRM ioctl (matches Linux `drm_ioctl`); `-EINVAL`
    /// otherwise.
    pub const fn errno(self) -> i32 {
        match self {
            DispatchError::WrongType => -25, // -ENOTTY
            _ => -22,                        // -EINVAL
        }
    }
}

// ── The render-node allowlist ────────────────────────────────────────────────
// The generic DRM ioctls the RADV first-frame trace uses, plus the 17 AMDGPU
// render ioctls `linuxkpi-drm/bringup_render.c` registers. Every `(nr, size)`
// is transcribed from docs/gpu-oracle/ATHENA-AMDGPU-DRM-ABI-20260711.md; `copy`
// is the canonical direction from the libdrm header encoding.
//
// Generic DRM nrs are their own value; AMDGPU nrs are DRM_COMMAND_BASE(0x40) +
// the amdgpu command id (so GEM_CREATE 0x00 -> 0x40, INFO 0x05 -> 0x45).

/// AMDGPU render ioctl command numbers, relative to `DRM_COMMAND_BASE`.
mod amdgpu_nr {
    pub const GEM_CREATE: u8 = 0x00;
    pub const GEM_MMAP: u8 = 0x01;
    pub const CTX: u8 = 0x02;
    pub const BO_LIST: u8 = 0x03;
    pub const CS: u8 = 0x04;
    pub const INFO: u8 = 0x05;
    pub const GEM_METADATA: u8 = 0x06;
    pub const GEM_WAIT_IDLE: u8 = 0x07;
    pub const GEM_VA: u8 = 0x08;
    pub const WAIT_CS: u8 = 0x09;
    pub const GEM_OP: u8 = 0x10;
    pub const GEM_USERPTR: u8 = 0x11;
    pub const WAIT_FENCES: u8 = 0x12;
    pub const VM: u8 = 0x13;
    pub const FENCE_TO_HANDLE: u8 = 0x14;
}

const fn amd(nr: u8) -> u8 {
    DRM_COMMAND_BASE + nr
}

/// Generic DRM ioctl command numbers (`drm.h`).
const DRM_NR_VERSION: u8 = 0x00;
const DRM_NR_GET_CAP: u8 = 0x0c;
const DRM_NR_GEM_CLOSE: u8 = 0x09;

/// The fail-closed policy: exactly the commands the render node forwards.
pub static RENDER_IOCTLS: &[IoctlSpec] = &[
    // ── generic DRM ──
    IoctlSpec {
        nr: DRM_NR_VERSION,
        size: 64,
        copy: CopyDir::InOut,
        name: "DRM_VERSION",
    },
    IoctlSpec {
        nr: DRM_NR_GET_CAP,
        size: 16,
        copy: CopyDir::InOut,
        name: "DRM_GET_CAP",
    },
    IoctlSpec {
        nr: DRM_NR_GEM_CLOSE,
        size: 8,
        copy: CopyDir::In,
        name: "DRM_GEM_CLOSE",
    },
    // ── AMDGPU render ──
    IoctlSpec {
        nr: amd(amdgpu_nr::GEM_CREATE),
        size: 32,
        copy: CopyDir::InOut,
        name: "AMDGPU_GEM_CREATE",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::GEM_MMAP),
        size: 8,
        copy: CopyDir::InOut,
        name: "AMDGPU_GEM_MMAP",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::CTX),
        size: 16,
        copy: CopyDir::InOut,
        name: "AMDGPU_CTX",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::BO_LIST),
        size: 24,
        copy: CopyDir::InOut,
        name: "AMDGPU_BO_LIST",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::CS),
        size: 24,
        copy: CopyDir::InOut,
        name: "AMDGPU_CS",
    },
    // INFO's struct is input-only (results go to a separate return_pointer buffer).
    IoctlSpec {
        nr: amd(amdgpu_nr::INFO),
        size: 32,
        copy: CopyDir::In,
        name: "AMDGPU_INFO",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::GEM_METADATA),
        size: 288,
        copy: CopyDir::InOut,
        name: "AMDGPU_GEM_METADATA",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::GEM_WAIT_IDLE),
        size: 16,
        copy: CopyDir::InOut,
        name: "AMDGPU_GEM_WAIT_IDLE",
    },
    // GEM_VA is input-only per the header (_IOW); Mesa's _IOWR variant normalizes here.
    IoctlSpec {
        nr: amd(amdgpu_nr::GEM_VA),
        size: 64,
        copy: CopyDir::In,
        name: "AMDGPU_GEM_VA",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::WAIT_CS),
        size: 32,
        copy: CopyDir::InOut,
        name: "AMDGPU_WAIT_CS",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::GEM_OP),
        size: 16,
        copy: CopyDir::InOut,
        name: "AMDGPU_GEM_OP",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::GEM_USERPTR),
        size: 24,
        copy: CopyDir::InOut,
        name: "AMDGPU_GEM_USERPTR",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::WAIT_FENCES),
        size: 24,
        copy: CopyDir::InOut,
        name: "AMDGPU_WAIT_FENCES",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::VM),
        size: 8,
        copy: CopyDir::InOut,
        name: "AMDGPU_VM",
    },
    IoctlSpec {
        nr: amd(amdgpu_nr::FENCE_TO_HANDLE),
        size: 32,
        copy: CopyDir::InOut,
        name: "AMDGPU_FENCE_TO_HANDLE",
    },
];

/// Look up a command number in the allowlist.
fn lookup(nr: u8) -> Option<&'static IoctlSpec> {
    let mut i = 0;
    while i < RENDER_IOCTLS.len() {
        if RENDER_IOCTLS[i].nr == nr {
            return Some(&RENDER_IOCTLS[i]);
        }
        i += 1;
    }
    None
}

/// Resolve a raw client ioctl request to the broker's marshal plan, or a
/// fail-closed refusal. Dispatch is on `(type, nr, size)` — the client's `dir`
/// bits are deliberately ignored, so both observed `GEM_VA` encodings
/// (`0x40406448` `_IOW` and Mesa's `0xc0406448` `_IOWR`) resolve identically.
pub fn dispatch(req: u32) -> Result<Resolved, DispatchError> {
    let ioc = decode(req);
    if ioc.type_ != DRM_IOCTL_TYPE {
        return Err(DispatchError::WrongType);
    }
    let spec = lookup(ioc.nr).ok_or(DispatchError::UnknownCommand)?;
    if ioc.size != spec.size {
        return Err(DispatchError::SizeMismatch);
    }
    if spec.size > MAX_PAYLOAD {
        return Err(DispatchError::PayloadTooLarge);
    }
    Ok(Resolved {
        nr: spec.nr,
        size: spec.size,
        copy: spec.copy,
        name: spec.name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the exact 32-bit ioctl values captured from Athena's Mesa/RADV run ──
    // docs/gpu-oracle/ATHENA-AMDGPU-DRM-ABI-20260711.md.
    const VERSION: u32 = 0xc040_6400;
    const GET_CAP: u32 = 0xc010_640c;
    const GEM_CLOSE: u32 = 0x4008_6409;
    const GEM_CREATE: u32 = 0xc020_6440;
    const GEM_MMAP: u32 = 0xc008_6441;
    const CTX: u32 = 0xc010_6442;
    const CS: u32 = 0xc018_6444;
    const INFO: u32 = 0x4020_6445;
    const GEM_METADATA: u32 = 0xc120_6446;
    const GEM_WAIT_IDLE: u32 = 0xc010_6447;
    const GEM_VA_HEADER: u32 = 0x4040_6448; // libdrm header (_IOW)
    const GEM_VA_MESA: u32 = 0xc040_6448; // what Mesa actually issues (_IOWR)
    const WAIT_CS: u32 = 0xc020_6449;

    #[test]
    fn decode_matches_the_ioc_field_layout() {
        let d = decode(INFO);
        assert_eq!(d.dir, IOC_WRITE, "INFO is _IOW");
        assert_eq!(d.type_, DRM_IOCTL_TYPE, "DRM type 'd'");
        assert_eq!(d.nr, 0x45, "INFO nr = COMMAND_BASE + 0x05");
        assert_eq!(d.size, 32, "drm_amdgpu_info is 32 bytes");
        // decode/encode round-trips for every captured value.
        for req in [VERSION, GET_CAP, GEM_CLOSE, GEM_CREATE, INFO, GEM_VA_MESA] {
            let d = decode(req);
            assert_eq!(
                encode(d.dir, d.type_, d.nr, d.size),
                req,
                "round-trip {req:#x}"
            );
        }
    }

    #[test]
    fn every_captured_command_resolves_to_its_registered_spec() {
        let cases: &[(u32, &str, u16, CopyDir)] = &[
            (VERSION, "DRM_VERSION", 64, CopyDir::InOut),
            (GET_CAP, "DRM_GET_CAP", 16, CopyDir::InOut),
            (GEM_CLOSE, "DRM_GEM_CLOSE", 8, CopyDir::In),
            (GEM_CREATE, "AMDGPU_GEM_CREATE", 32, CopyDir::InOut),
            (GEM_MMAP, "AMDGPU_GEM_MMAP", 8, CopyDir::InOut),
            (CTX, "AMDGPU_CTX", 16, CopyDir::InOut),
            (CS, "AMDGPU_CS", 24, CopyDir::InOut),
            (INFO, "AMDGPU_INFO", 32, CopyDir::In),
            (GEM_METADATA, "AMDGPU_GEM_METADATA", 288, CopyDir::InOut),
            (GEM_WAIT_IDLE, "AMDGPU_GEM_WAIT_IDLE", 16, CopyDir::InOut),
            (WAIT_CS, "AMDGPU_WAIT_CS", 32, CopyDir::InOut),
        ];
        for &(req, name, size, copy) in cases {
            let r = dispatch(req).unwrap_or_else(|e| panic!("{name} {req:#x} refused: {e:?}"));
            assert_eq!(r.name, name, "{req:#x} name");
            assert_eq!(r.size, size, "{name} size");
            assert_eq!(r.copy, copy, "{name} copy plan");
        }
    }

    /// The load-bearing normalization: Mesa's `_IOWR` GEM-VA and libdrm's `_IOW`
    /// GEM-VA differ ONLY in the direction bits, and must resolve identically.
    #[test]
    fn gem_va_direction_bit_variance_normalizes_to_one_command() {
        assert_ne!(
            GEM_VA_HEADER, GEM_VA_MESA,
            "the two encodings really differ"
        );
        assert_ne!(
            decode(GEM_VA_HEADER).dir,
            decode(GEM_VA_MESA).dir,
            "and they differ in the dir bits specifically"
        );
        let from_header = dispatch(GEM_VA_HEADER).expect("header GEM_VA permitted");
        let from_mesa = dispatch(GEM_VA_MESA).expect("Mesa GEM_VA permitted");
        assert_eq!(
            from_header, from_mesa,
            "both must resolve to the same command"
        );
        assert_eq!(from_header.name, "AMDGPU_GEM_VA");
        assert_eq!(from_header.size, 64);
        assert_eq!(
            from_header.copy,
            CopyDir::In,
            "canonical (header) copy plan wins"
        );
    }

    #[test]
    fn non_drm_ioctl_is_refused_as_wrong_type() {
        // TCGETS = 0x5401: type 0x54 ('T'), not 'd'.
        assert_eq!(dispatch(0x5401), Err(DispatchError::WrongType));
        assert_eq!(
            DispatchError::WrongType.errno(),
            -25,
            "-ENOTTY like drm_ioctl"
        );
    }

    #[test]
    fn unknown_drm_command_fails_closed() {
        // A well-formed DRM ioctl (type 'd', plausible size) for a command NOT on
        // the allowlist — e.g. a modeset ioctl a render node must never forward.
        let bogus = encode(IOC_READ | IOC_WRITE, DRM_IOCTL_TYPE, 0xA0, 32);
        assert_eq!(dispatch(bogus), Err(DispatchError::UnknownCommand));
    }

    #[test]
    fn wrong_struct_size_fails_closed() {
        // INFO nr with a truncated size: a client under-declaring the struct must
        // be rejected before any copy, not silently handled.
        let truncated = encode(IOC_WRITE, DRM_IOCTL_TYPE, 0x45, 8);
        assert_eq!(dispatch(truncated), Err(DispatchError::SizeMismatch));
        let oversized = encode(IOC_WRITE, DRM_IOCTL_TYPE, 0x45, 4000);
        assert_eq!(dispatch(oversized), Err(DispatchError::SizeMismatch));
    }

    #[test]
    fn no_registered_command_exceeds_the_marshal_bound() {
        for spec in RENDER_IOCTLS {
            assert!(
                spec.size <= MAX_PAYLOAD,
                "{} size {} exceeds MAX_PAYLOAD {}",
                spec.name,
                spec.size,
                MAX_PAYLOAD
            );
        }
    }

    #[test]
    fn allowlist_has_no_duplicate_command_numbers() {
        for (i, a) in RENDER_IOCTLS.iter().enumerate() {
            for b in &RENDER_IOCTLS[i + 1..] {
                assert_ne!(
                    a.nr, b.nr,
                    "duplicate nr {:#x} ({}, {})",
                    a.nr, a.name, b.name
                );
            }
        }
    }
}
