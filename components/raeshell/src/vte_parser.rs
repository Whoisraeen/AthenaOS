//! ANSI/VT escape parsing — Concept §"RaeShell: a terminal that feels like 2026".
//!
//! The terminal emulator should model a grid and state, not re-implement the
//! decades of VT100/xterm escape-sequence parsing. `vte` (the Alacritty parser)
//! turns a raw byte stream into clean `print` / CSI / OSC / ESC events; this
//! module is the thin sink the emulator drives. Pure-Rust, no_std.

extern crate alloc;
use alloc::string::String;
use vte::{Params, Parser, Perform};

/// A minimal terminal event sink: the printable text, how many CSI sequences
/// were seen, and the last SGR (color/style) parameter. The real emulator swaps
/// this for a grid model — the parsing front-end is identical.
#[derive(Default)]
pub struct TermSink {
    pub text: String,
    pub csi_count: usize,
    pub osc_count: usize,
    pub last_sgr: i64,
}

impl Perform for TermSink {
    fn print(&mut self, c: char) {
        self.text.push(c);
    }
    fn execute(&mut self, _byte: u8) {}
    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        self.osc_count += 1;
    }
    fn csi_dispatch(
        &mut self,
        params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        self.csi_count += 1;
        if action == 'm' {
            // SGR — record the first parameter (e.g. 31 = red, 0 = reset).
            if let Some(first) = params.iter().next() {
                self.last_sgr = first.first().copied().unwrap_or(0) as i64;
            }
        }
    }
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}

/// Feed a byte stream through the VT parser and return the resulting sink.
pub fn parse(input: &[u8]) -> TermSink {
    let mut parser = Parser::new();
    let mut sink = TermSink::default();
    for &b in input {
        parser.advance(&mut sink, b);
    }
    sink
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_text_from_escapes() {
        // "Hello" + SGR red + "Red" + SGR reset + clear-screen.
        let sink = parse(b"Hello\x1b[31mRed\x1b[0m\x1b[2J");
        assert_eq!(sink.text, "HelloRed");
        // Three CSI sequences: [31m, [0m, [2J.
        assert_eq!(sink.csi_count, 3);
        // Last SGR seen was the reset (0); [2J is ED, not SGR.
        assert_eq!(sink.last_sgr, 0);
    }

    #[test]
    fn captures_osc_title() {
        // OSC 0 ; set-title BEL.
        let sink = parse(b"\x1b]0;RaeShell\x07ready");
        assert_eq!(sink.osc_count, 1);
        assert_eq!(sink.text, "ready");
    }
}
