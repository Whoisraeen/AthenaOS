//! raewasm::pkg — the **WebAssembly package format** for sandboxed/untrusted store
//! apps + extensions (Concept §Language Stack — "any language in, one safe runtime";
//! MasterChecklist Phase 15.1). A package is a capability **manifest** + a Wasm
//! **module**: the manifest declares exactly which host imports the module may use
//! and the capability each one requires, and the loader runs the module so that an
//! import is serviceable ONLY if (a) the manifest declares it and (b) the embedder
//! granted its capability. Everything else traps — the store app can do nothing its
//! manifest didn't ask for and the user didn't grant.
//!
//! This is the bridge between [`crate::HostEnv`] (the raw call seam) and a real
//! capability set: raewasm stays kernel-free, so a capability is an opaque `u32` id
//! that the embedder (AthGuard) maps to its `Cap` enum. The container is a small
//! bounds-checked binary format — a malformed/hostile package returns `None`, never
//! panics (same load-bearing property as the module decoder).

extern crate alloc;
use crate::{instantiate, HostEnv, ImportFunc};
use alloc::string::String;
use alloc::vec::Vec;

/// The package container magic: `RWASMPK` + format version 1.
pub const PKG_MAGIC: [u8; 8] = *b"RWASMPK1";

/// One capability grant in a manifest: an import (`module`/`name`) the package may
/// call, plus the capability id required to call it. The embedder maps `cap` to its
/// own `Cap` enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapGrant {
    pub import_module: String,
    pub import_name: String,
    pub cap: u32,
}

/// A package manifest: identity + the complete set of host imports it is allowed to
/// use. The set is **exhaustive** — at load time every import the module declares
/// must appear here, or the package is rejected (a module cannot smuggle in an
/// undeclared host call).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmManifest {
    pub name: String,
    pub version: u32,
    pub grants: Vec<CapGrant>,
}

impl WasmManifest {
    /// The grant matching an import, if the manifest declares it.
    fn grant_for(&self, module: &str, name: &str) -> Option<&CapGrant> {
        self.grants
            .iter()
            .find(|g| g.import_module == module && g.import_name == name)
    }
}

// ── serialization (a small bounds-checked binary container) ──────────────────

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn take_u16(b: &[u8], pos: &mut usize) -> Option<u16> {
    let v = u16::from_le_bytes([*b.get(*pos)?, *b.get(*pos + 1)?]);
    *pos += 2;
    Some(v)
}

fn take_u32(b: &[u8], pos: &mut usize) -> Option<u32> {
    let s = b.get(*pos..pos.checked_add(4)?)?;
    *pos += 4;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn take_str(b: &[u8], pos: &mut usize) -> Option<String> {
    let len = take_u16(b, pos)? as usize;
    let end = pos.checked_add(len)?;
    let s = core::str::from_utf8(b.get(*pos..end)?).ok()?.into();
    *pos = end;
    Some(s)
}

/// Serialize a manifest + a Wasm module into a package container.
pub fn pack_package(manifest: &WasmManifest, wasm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + wasm.len());
    out.extend_from_slice(&PKG_MAGIC);
    put_str(&mut out, &manifest.name);
    out.extend_from_slice(&manifest.version.to_le_bytes());
    out.extend_from_slice(&(manifest.grants.len() as u16).to_le_bytes());
    for g in &manifest.grants {
        put_str(&mut out, &g.import_module);
        put_str(&mut out, &g.import_name);
        out.extend_from_slice(&g.cap.to_le_bytes());
    }
    out.extend_from_slice(&(wasm.len() as u32).to_le_bytes());
    out.extend_from_slice(wasm);
    out
}

/// Parse a package container into its manifest + a borrowed slice of the Wasm module
/// (zero-copy). Bounds-checked end to end: a malformed/hostile package returns `None`.
pub fn parse_package(bytes: &[u8]) -> Option<(WasmManifest, &[u8])> {
    if bytes.get(0..8)? != PKG_MAGIC {
        return None;
    }
    let mut pos = 8usize;
    let name = take_str(bytes, &mut pos)?;
    let version = take_u32(bytes, &mut pos)?;
    let ngrants = take_u16(bytes, &mut pos)? as usize;
    let mut grants = Vec::with_capacity(ngrants.min(256));
    for _ in 0..ngrants {
        let import_module = take_str(bytes, &mut pos)?;
        let import_name = take_str(bytes, &mut pos)?;
        let cap = take_u32(bytes, &mut pos)?;
        grants.push(CapGrant {
            import_module,
            import_name,
            cap,
        });
    }
    let wlen = take_u32(bytes, &mut pos)? as usize;
    let wend = pos.checked_add(wlen)?;
    let wasm = bytes.get(pos..wend)?;
    Some((
        WasmManifest {
            name,
            version,
            grants,
        },
        wasm,
    ))
}

// ── capability-enforcing host ────────────────────────────────────────────────

/// A [`HostEnv`] that enforces a package's manifest against a set of granted
/// capabilities. An imported `call` is serviced only when the manifest declares that
/// import AND its required capability is in `granted`; otherwise it traps (`None`).
/// The actual host behavior is `dispatch` — invoked with the import's `(module, name,
/// args)` only after the gate passes.
pub struct CapHost<'a, F> {
    manifest: &'a WasmManifest,
    imports: &'a [ImportFunc],
    granted: &'a [u32],
    dispatch: F,
}

impl<'a, F> HostEnv for CapHost<'a, F>
where
    F: FnMut(&str, &str, &[i32]) -> Option<Vec<i32>>,
{
    fn call_import(&mut self, index: u32, args: &[i32]) -> Option<Vec<i32>> {
        let imp = self.imports.get(index as usize)?;
        let grant = self.manifest.grant_for(&imp.module, &imp.name)?; // undeclared → trap
        if !self.granted.contains(&grant.cap) {
            return None; // capability not granted → trap
        }
        (self.dispatch)(&imp.module, &imp.name, args)
    }
}

/// Load a package and run its exported function `name` with i32 `args`, enforcing the
/// manifest against `granted_caps`. `dispatch` services an allowed import call.
///
/// Fail-closed: the package is rejected (`None`) if it is malformed, if the module
/// declares ANY import the manifest does not cover (no smuggling), or if execution
/// traps (incl. a call to an import whose capability was not granted).
pub fn load_and_run<F>(
    package: &[u8],
    name: &str,
    args: &[i32],
    granted_caps: &[u32],
    dispatch: F,
    fuel: u64,
) -> Option<Vec<i32>>
where
    F: FnMut(&str, &str, &[i32]) -> Option<Vec<i32>>,
{
    let (manifest, wasm) = parse_package(package)?;
    let inst = instantiate(wasm)?;
    let imports = inst.imports().to_vec();
    // Fail-closed: every import the module needs must be declared in the manifest.
    for imp in &imports {
        if manifest.grant_for(&imp.module, &imp.name).is_none() {
            return None;
        }
    }
    let mut host = CapHost {
        manifest: &manifest,
        imports: &imports,
        granted: granted_caps,
        dispatch,
    };
    inst.call_export(name, args, &mut host, fuel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A module that imports `env.host_add (i32 i32)->i32` and exports
    /// `use_host(a,b) = host_add(a,b) + 1`. (Same shape as the runtime KAT.)
    fn host_add_module() -> Vec<u8> {
        let mut m = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let sections: [(u8, Vec<u8>); 4] = [
            (0x01, vec![0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F]), // TYPE (i32,i32)->i32
            (
                0x02,
                vec![
                    0x01, 0x03, b'e', b'n', b'v', 0x08, b'h', b'o', b's', b't', b'_', b'a', b'd',
                    b'd', 0x00, 0x00,
                ],
            ), // IMPORT env.host_add
            (0x03, vec![0x01, 0x00]),                               // FUNCTION
            (
                0x07,
                vec![
                    0x01, 0x08, b'u', b's', b'e', b'_', b'h', b'o', b's', b't', 0x00, 0x01,
                ],
            ), // EXPORT use_host = func 1
        ];
        for (id, body) in sections {
            m.push(id);
            m.push(body.len() as u8);
            m.extend_from_slice(&body);
        }
        // CODE: local.get0 local.get1 call0 i32.const1 i32.add end
        let code = vec![
            0x00, 0x20, 0x00, 0x20, 0x01, 0x10, 0x00, 0x41, 0x01, 0x6A, 0x0B,
        ];
        let mut code_section = vec![0x01u8, code.len() as u8];
        code_section.extend_from_slice(&code);
        m.push(0x0A);
        m.push(code_section.len() as u8);
        m.extend_from_slice(&code_section);
        m
    }

    fn host_add_manifest() -> WasmManifest {
        WasmManifest {
            name: "adder".into(),
            version: 0x0001_0000,
            grants: vec![CapGrant {
                import_module: "env".into(),
                import_name: "host_add".into(),
                cap: 5,
            }],
        }
    }

    #[test]
    fn package_round_trips() {
        let wasm = host_add_module();
        let manifest = host_add_manifest();
        let pkg = pack_package(&manifest, &wasm);
        let (m2, w2) = parse_package(&pkg).expect("parse");
        assert_eq!(m2, manifest);
        assert_eq!(w2, &wasm[..]);
    }

    #[test]
    fn malformed_package_is_rejected() {
        assert!(parse_package(&[]).is_none());
        assert!(parse_package(b"NOTAPKG_").is_none());
        // Truncated: valid magic but a name length that runs off the end.
        let mut p = PKG_MAGIC.to_vec();
        p.extend_from_slice(&[0xFF, 0xFF]); // name len 65535, no bytes follow
        assert!(parse_package(&p).is_none());
    }

    /// Adds the two args — the only host function this package's dispatch provides.
    fn host_dispatch(module: &str, name: &str, args: &[i32]) -> Option<Vec<i32>> {
        if module == "env" && name == "host_add" && args.len() == 2 {
            Some(vec![args[0].wrapping_add(args[1])])
        } else {
            None
        }
    }

    #[test]
    fn capability_granted_runs() {
        let pkg = pack_package(&host_add_manifest(), &host_add_module());
        // cap 5 granted → host_add(3,4)=7, +1 = 8.
        assert_eq!(
            load_and_run(
                &pkg,
                "use_host",
                &[3, 4],
                &[5],
                host_dispatch,
                crate::DEFAULT_FUEL
            ),
            Some(vec![8])
        );
    }

    #[test]
    fn capability_denied_traps() {
        let pkg = pack_package(&host_add_manifest(), &host_add_module());
        // cap 5 NOT in the granted set → the import call traps → None.
        assert_eq!(
            load_and_run(
                &pkg,
                "use_host",
                &[3, 4],
                &[9],
                host_dispatch,
                crate::DEFAULT_FUEL
            ),
            None
        );
        // No caps granted at all → also traps.
        assert_eq!(
            load_and_run(
                &pkg,
                "use_host",
                &[3, 4],
                &[],
                host_dispatch,
                crate::DEFAULT_FUEL
            ),
            None
        );
    }

    #[test]
    fn undeclared_import_rejects_package() {
        // A manifest that declares NO grants — the module still imports host_add, so
        // loading must fail closed (the module cannot use an undeclared host call)
        // even though cap 5 is "granted".
        let empty_manifest = WasmManifest {
            name: "sneaky".into(),
            version: 1,
            grants: vec![],
        };
        let pkg = pack_package(&empty_manifest, &host_add_module());
        assert_eq!(
            load_and_run(
                &pkg,
                "use_host",
                &[3, 4],
                &[5],
                host_dispatch,
                crate::DEFAULT_FUEL
            ),
            None
        );
    }
}
