//! PE introspection via `object` — Concept §Compatibility Strategy.
//!
//! The hand-rolled `pe_loader` maps and runs PE images; `object` is a vetted,
//! pure-Rust (no_std) reader used here as a cross-check and for the richer
//! metadata (architecture, entry, section count) the loader doesn't surface.
//! Keeping both lets the loader stay lean while still having a robust parser to
//! validate untrusted PEs before mapping them.

extern crate alloc;
use object::read::pe::PeFile64;
use object::{Architecture, Object};

/// Summary of a PE32+ image.
pub struct PeSummary {
    pub arch: Architecture,
    pub entry: u64,
    pub sections: usize,
}

/// Parse a PE32+ image header. Returns `None` on a malformed image (so a
/// hostile/truncated PE is rejected before the loader ever maps it).
pub fn inspect(data: &[u8]) -> Option<PeSummary> {
    let pe = PeFile64::parse(data).ok()?;
    Some(PeSummary {
        arch: pe.architecture(),
        entry: pe.entry(),
        sections: pe.sections().count(),
    })
}

/// Self-test (callable from a kernel R10 boot smoketest). Parses the in-tree
/// hand-assembled test EXE and confirms object agrees it is an x86-64 PE with a
/// real entry point and at least one section. Returns true on PASS.
pub fn run_self_test() -> bool {
    let exe = crate::testpe::build_exit_process_exe();
    match inspect(&exe) {
        Some(s) => s.arch == Architecture::X86_64 && s.entry != 0 && s.sections >= 1,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_in_tree_test_exe() {
        assert!(run_self_test());
    }

    #[test]
    fn rejects_garbage() {
        assert!(inspect(b"MZ not really a pe").is_none());
        assert!(inspect(&[0u8; 4]).is_none());
    }
}
