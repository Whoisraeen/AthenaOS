//! raepkg — the host-side packer for RaeenOS application bundles (the `raekit bundle`
//! producer half).
//!
//! Produces the `REBP` ("RaeEnv Bundled Package") manifest the kernel's `app_bundle`
//! loader verifies via `SYS_BUNDLE_VERIFY`. The on-wire format is defined by
//! `kernel/src/app_bundle.rs` (keep the two in sync — the constants below mirror it
//! exactly). A manifest names the app + its packed-semver version + each dependency
//! `(name, required_version, sha256)`; the kernel answers "all deps installed at
//! these hashes" or "missing: libfoo 1.2.3". No DLL hell, no PATH wars.
//!
//! Pure byte serialization — `#![no_std]` + alloc — host-KAT'd with a round-trip
//! parser, so a packed manifest is provably loadable by the kernel's REBP parser.

#![cfg_attr(not(test), no_std)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

/// `REBP` (little-endian). Mirrors `kernel/src/app_bundle.rs::MAGIC`.
pub const MAGIC: u32 = 0x5245_4250;
/// The on-wire format version this packer emits. Mirrors the kernel's `version = 1`.
pub const FORMAT_VERSION: u32 = 1;
/// Header size: magic + version + name_len + app_version + dep_count (5 × u32).
const HEADER_LEN: usize = 20;
/// Per-dep fixed prefix: name_len + required_version + sha256[32].
const DEP_PREFIX_LEN: usize = 40;

/// Pack a `(major, minor, patch)` semver into the kernel's `u32` form
/// (`major<<16 | minor<<8 | patch`).
pub const fn pack_semver(major: u8, minor: u8, patch: u8) -> u32 {
    ((major as u32) << 16) | ((minor as u32) << 8) | (patch as u32)
}

/// One dependency: its name, the required packed-semver version, and the expected
/// SHA-256 of the installed dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    pub required_version: u32,
    pub sha256: [u8; 32],
}

/// An app-bundle manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub app_version: u32,
    pub deps: Vec<Dep>,
}

/// Errors from [`parse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Fewer than the fixed header / a fixed field's bytes are present.
    TooShort,
    /// First u32 is not [`MAGIC`].
    BadMagic,
    /// Format version is not [`FORMAT_VERSION`].
    BadVersion,
    /// A name field is not valid UTF-8.
    BadUtf8,
    /// A declared length runs past the end of the buffer.
    Truncated,
}

impl Manifest {
    /// Build a manifest for `name` at `app_version` with no dependencies.
    pub fn new(name: impl Into<String>, app_version: u32) -> Self {
        Self {
            name: name.into(),
            app_version,
            deps: Vec::new(),
        }
    }

    /// Declare a dependency (builder-style).
    pub fn with_dep(
        mut self,
        name: impl Into<String>,
        required_version: u32,
        sha256: [u8; 32],
    ) -> Self {
        self.deps.push(Dep {
            name: name.into(),
            required_version,
            sha256,
        });
        self
    }

    /// Serialize into the on-wire `REBP` byte layout the kernel parses. The exact
    /// inverse of [`parse`].
    pub fn pack(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(
            HEADER_LEN
                + self.name.len()
                + self
                    .deps
                    .iter()
                    .map(|d| DEP_PREFIX_LEN + d.name.len())
                    .sum::<usize>(),
        );
        b.extend_from_slice(&MAGIC.to_le_bytes());
        b.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        b.extend_from_slice(&(self.name.len() as u32).to_le_bytes());
        b.extend_from_slice(&self.app_version.to_le_bytes());
        b.extend_from_slice(&(self.deps.len() as u32).to_le_bytes());
        b.extend_from_slice(self.name.as_bytes());
        for d in &self.deps {
            b.extend_from_slice(&(d.name.len() as u32).to_le_bytes());
            b.extend_from_slice(&d.required_version.to_le_bytes());
            b.extend_from_slice(&d.sha256);
            b.extend_from_slice(d.name.as_bytes());
        }
        b
    }
}

fn rd_u32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parse a `REBP` manifest — the inverse of [`Manifest::pack`], so the round-trip is
/// provable on the host. Bounds-checked end to end (a truncated or hostile buffer
/// returns `Err`, never reads out of bounds).
pub fn parse(bytes: &[u8]) -> Result<Manifest, ParseError> {
    let magic = rd_u32(bytes, 0).ok_or(ParseError::TooShort)?;
    if magic != MAGIC {
        return Err(ParseError::BadMagic);
    }
    if rd_u32(bytes, 4).ok_or(ParseError::TooShort)? != FORMAT_VERSION {
        return Err(ParseError::BadVersion);
    }
    let name_len = rd_u32(bytes, 8).ok_or(ParseError::TooShort)? as usize;
    let app_version = rd_u32(bytes, 12).ok_or(ParseError::TooShort)?;
    let dep_count = rd_u32(bytes, 16).ok_or(ParseError::TooShort)? as usize;

    let mut off = HEADER_LEN;
    let name_end = off.checked_add(name_len).ok_or(ParseError::Truncated)?;
    let name = core::str::from_utf8(bytes.get(off..name_end).ok_or(ParseError::Truncated)?)
        .map_err(|_| ParseError::BadUtf8)?
        .into();
    off = name_end;

    let mut deps = Vec::with_capacity(dep_count.min(1024));
    for _ in 0..dep_count {
        let d_name_len = rd_u32(bytes, off).ok_or(ParseError::Truncated)? as usize;
        let required_version = rd_u32(bytes, off + 4).ok_or(ParseError::Truncated)?;
        let sha_slice = bytes
            .get(off + 8..off + DEP_PREFIX_LEN)
            .ok_or(ParseError::Truncated)?;
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(sha_slice);
        off += DEP_PREFIX_LEN;
        let d_end = off.checked_add(d_name_len).ok_or(ParseError::Truncated)?;
        let d_name = core::str::from_utf8(bytes.get(off..d_end).ok_or(ParseError::Truncated)?)
            .map_err(|_| ParseError::BadUtf8)?
            .into();
        off = d_end;
        deps.push(Dep {
            name: d_name,
            required_version,
            sha256,
        });
    }
    Ok(Manifest {
        name,
        app_version,
        deps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_and_semver_match_the_kernel() {
        // kernel/src/app_bundle.rs: MAGIC = 0x52454250 ('REBP'); semver maj<<16|min<<8|patch.
        assert_eq!(MAGIC, 0x5245_4250);
        assert_eq!(&MAGIC.to_le_bytes(), b"PBER"); // little-endian on the wire
        assert_eq!(pack_semver(1, 2, 3), 0x0001_0203);
        assert_eq!(pack_semver(255, 255, 255), 0x00FF_FFFF);
    }

    #[test]
    fn round_trip_with_deps() {
        let m = Manifest::new("photos", pack_semver(2, 1, 0))
            .with_dep("rae_image", pack_semver(1, 0, 0), [0xAB; 32])
            .with_dep("raekit", pack_semver(0, 9, 4), [0x11; 32]);
        let bytes = m.pack();
        let back = parse(&bytes).expect("packed manifest must parse");
        assert_eq!(back, m, "pack -> parse must round-trip exactly");
        // Header is byte-exact at the documented offsets.
        assert_eq!(&bytes[0..4], &MAGIC.to_le_bytes());
        assert_eq!(rd_u32(&bytes, 16), Some(2), "dep_count");
    }

    #[test]
    fn round_trip_no_deps() {
        let m = Manifest::new("calculator", pack_semver(1, 0, 0));
        assert_eq!(parse(&m.pack()).unwrap(), m);
    }

    #[test]
    fn rejects_bad_magic_version_and_truncation() {
        let good = Manifest::new("x", 1).with_dep("d", 1, [0; 32]).pack();
        // Bad magic.
        let mut bad = good.clone();
        bad[0] ^= 0xFF;
        assert_eq!(parse(&bad), Err(ParseError::BadMagic));
        // Bad version.
        let mut bad_ver = good.clone();
        bad_ver[4] = 9;
        assert_eq!(parse(&bad_ver), Err(ParseError::BadVersion));
        // Truncated mid-dep (drop the dependency name bytes).
        let trunc = &good[..good.len() - 1];
        assert_eq!(parse(trunc), Err(ParseError::Truncated));
        // Empty / too short.
        assert_eq!(parse(&[]), Err(ParseError::TooShort));
        assert_eq!(parse(&[0x50, 0x42]), Err(ParseError::TooShort));
    }

    #[test]
    fn rejects_non_utf8_name() {
        let mut m = Manifest::new("ok", 1).pack();
        // name is 2 bytes at offset 20 ("ok"); corrupt to an invalid UTF-8 lead byte.
        m[20] = 0xFF;
        assert_eq!(parse(&m), Err(ParseError::BadUtf8));
    }
}
