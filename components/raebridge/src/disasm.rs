//! x86-64 disassembly — Concept §"RaeBridge: run Windows games, day one".
//!
//! The Win32 compatibility path needs to *read* guest instructions, not just
//! map them: x64 calling-convention marshaling, SEH `.pdata`/`.xdata` unwind
//! walking, and generating import thunks / inline hooks all start with decoding
//! the target's code. `iced-x86` is the pure-Rust, no_std decoder that powers
//! that — no external disassembler, no C.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter, Mnemonic};

/// One decoded instruction: its IP, length, mnemonic, and Intel-syntax text.
pub struct Decoded {
    pub ip: u64,
    pub len: usize,
    pub mnemonic: Mnemonic,
    pub text: String,
}

/// Disassemble up to `max` instructions of 64-bit `code` starting at `rip`.
pub fn disassemble(code: &[u8], rip: u64, max: usize) -> Vec<Decoded> {
    let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
    let mut formatter = IntelFormatter::new();
    let mut instr = Instruction::default();
    let mut text = String::new();
    let mut out = Vec::new();
    while decoder.can_decode() && out.len() < max {
        decoder.decode_out(&mut instr);
        text.clear();
        formatter.format(&instr, &mut text);
        out.push(Decoded {
            ip: instr.ip(),
            len: instr.len(),
            mnemonic: instr.mnemonic(),
            text: text.clone(),
        });
    }
    out
}

/// Self-test (callable from a kernel R10 boot smoketest). Decodes the exact
/// prologue the RaeBridge test EXE uses — `sub rsp,0x28` then `mov ecx,42` —
/// and confirms the mnemonics and instruction lengths. Returns true on PASS.
pub fn run_self_test() -> bool {
    // 48 83 EC 28          sub rsp, 0x28
    // B9 2A 00 00 00       mov ecx, 0x2A (42)
    let code = [0x48, 0x83, 0xEC, 0x28, 0xB9, 0x2A, 0x00, 0x00, 0x00];
    let d = disassemble(&code, 0x1000, 8);
    d.len() == 2
        && d[0].mnemonic == Mnemonic::Sub
        && d[0].len == 4
        && d[1].mnemonic == Mnemonic::Mov
        && d[1].len == 5
        && d[1].ip == 0x1004
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_test_exe_prologue() {
        assert!(run_self_test());
    }

    #[test]
    fn decodes_rip_relative_call() {
        // FF 15 xx xx xx xx — call qword [rip + disp32] (the IAT call form).
        let code = [0xFF, 0x15, 0x10, 0x20, 0x00, 0x00];
        let d = disassemble(&code, 0x2000, 4);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].mnemonic, Mnemonic::Call);
        assert_eq!(d[0].len, 6);
    }
}
