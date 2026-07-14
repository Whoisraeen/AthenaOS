#![no_std]
#![no_main]

extern crate alloc;

#[allow(unused_imports)]
use athkit;

const PROMPT: &[u8] = b"athena> ";

fn write_out(bytes: &[u8]) {
    let _ = athkit::sys::pty_slave_write(bytes);
}

fn write_err(bytes: &[u8]) {
    // AthenaOS PTY currently exposes a single byte stream; treat this as stderr-equivalent.
    let _ = athkit::sys::pty_slave_write(bytes);
}

fn write_prompt() {
    write_out(PROMPT);
}

fn print_u64(mut val: u64) {
    if val == 0 {
        write_out(b"0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0usize;
    while val > 0 {
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        write_out(&[buf[i]]);
    }
}

fn resolve_path(cwd: &str, input: &str) -> alloc::string::String {
    use alloc::string::String;
    use alloc::vec::Vec;

    let src = input.trim();
    if src.is_empty() {
        return String::from(cwd);
    }

    let mut parts: Vec<&str> = Vec::new();
    if !src.starts_with('/') {
        for p in cwd.split('/') {
            if !p.is_empty() {
                parts.push(p);
            }
        }
    }

    for p in src.split('/') {
        if p.is_empty() || p == "." {
            continue;
        }
        if p == ".." {
            let _ = parts.pop();
            continue;
        }
        parts.push(p);
    }

    if parts.is_empty() {
        return String::from("/");
    }
    let mut out = String::new();
    for p in parts {
        out.push('/');
        out.push_str(p);
    }
    out
}

fn execute_line(line: &[u8], cwd: &mut alloc::string::String, home: &str) {
    let cmd_str = match core::str::from_utf8(line) {
        Ok(s) => s.trim(),
        Err(_) => {
            write_err(b"ath-sh: invalid utf-8\n");
            return;
        }
    };
    if cmd_str.is_empty() {
        return;
    }

    let (verb, args) = match cmd_str.find(' ') {
        Some(pos) => (&cmd_str[..pos], cmd_str[pos + 1..].trim_start()),
        None => (cmd_str, ""),
    };

    match verb {
        "help" => {
            write_out(b"help echo clear pwd cd ls cat rae spawn sysinfo pid exit\n");
        }
        "echo" => {
            write_out(args.as_bytes());
            write_out(b"\n");
        }
        "clear" => {
            write_out(b"\x1b[2J\x1b[H");
        }
        "pwd" => {
            write_out(cwd.as_bytes());
            write_out(b"\n");
        }
        "cd" => {
            let target = if args.trim().is_empty() {
                home
            } else {
                args.trim()
            };
            let new_path = resolve_path(cwd.as_str(), target);
            // Validate by attempting a directory enumeration.
            let mut dir_buf = [0u8; 64];
            let ok = athkit::sys::readdir_at(new_path.as_str(), &mut dir_buf) != u64::MAX;
            if ok {
                *cwd = new_path;
            } else {
                write_err(b"cd: no such directory\n");
            }
        }
        "ls" => {
            let list_path = if args.trim().is_empty() {
                cwd.as_str()
            } else {
                // If the user passed a path, resolve it relative to cwd.
                // (No globbing yet; keep it minimal.)
                // Allocate once so we can pass &str to the syscall.
                // Note: resolve_path returns an owned String.
                // We keep it in a local binding to keep it alive.
                ""
            };
            let resolved;
            let path = if list_path.is_empty() {
                resolved = resolve_path(cwd.as_str(), args.trim());
                resolved.as_str()
            } else {
                list_path
            };
            let mut dir_buf = [0u8; 4096];
            let count = athkit::sys::readdir_at(path, &mut dir_buf);
            if count == u64::MAX {
                write_err(b"ls: failed\n");
            } else {
                let mut off = 0usize;
                for _ in 0..count {
                    if off + 6 > dir_buf.len() {
                        break;
                    }
                    let name_len = u16::from_ne_bytes([dir_buf[off], dir_buf[off + 1]]) as usize;
                    off += 6;
                    if off + name_len > dir_buf.len() {
                        break;
                    }
                    write_out(&dir_buf[off..off + name_len]);
                    write_out(b"\n");
                    off += name_len;
                }
            }
        }
        "cat" => {
            if args.is_empty() {
                write_err(b"usage: cat <file>\n");
            } else if let Some(data) = read_file(&resolve_path(cwd.as_str(), args.trim())) {
                write_out(&data);
                if data.last() != Some(&b'\n') {
                    write_out(b"\n");
                }
            } else {
                write_err(b"cat: not found\n");
            }
        }
        "rae" => {
            // Run a Rae script (Concept §Customization Engine: "Swift
            // scripts for automation") — the shell invocation surface.
            // The user typing `rae` at their own shell IS the capability
            // authorization, so the script gets the full SCRIPT_CAP mask.
            if args.is_empty() {
                write_err(b"usage: rae <script-file>\n");
            } else if let Some(src) = read_file(&resolve_path(cwd.as_str(), args.trim())) {
                run_rae_script(&src);
            } else {
                write_err(b"rae: script not found\n");
            }
        }
        "spawn" => {
            if args.is_empty() {
                write_err(b"usage: spawn <app>\n");
            } else {
                let pid = athkit::sys::spawn(args);
                if pid == u64::MAX {
                    write_err(b"spawn: failed\n");
                } else {
                    write_out(b"pid ");
                    print_u64(pid);
                    write_out(b"\n");
                }
            }
        }
        "sysinfo" => {
            write_out(b"AthenaOS ath-sh\n");
            let ns = athkit::sys::time_ns();
            write_out(b"uptime_s ");
            print_u64(ns / 1_000_000_000);
            write_out(b"\npid ");
            print_u64(athkit::sys::getpid());
            write_out(b"\n");
        }
        "pid" => {
            print_u64(athkit::sys::getpid());
            write_out(b"\n");
        }
        "exit" => {
            athkit::sys::exit(0);
        }
        _ => {
            write_err(b"unknown: ");
            write_err(verb.as_bytes());
            write_err(b"\n");
        }
    }
}

/// Submit a script through the kernel scripting lifecycle, wait for a
/// terminal state (inline sources finish before `script_run` returns;
/// >64 KiB sources run in athlangd — poll with yields), then print the
/// captured output and exit state.
fn run_rae_script(src: &[u8]) {
    // Full cap mask: the interactive user is the authorizer.
    const SCRIPT_CAP_ALL: u64 = (1 << 6) - 1;
    let id = athkit::sys::script_run(src, SCRIPT_CAP_ALL);
    if id == 0 || id > 0xFFFF_FFFF {
        write_err(b"rae: submit failed\n");
        return;
    }
    // ScriptAbi (56 B) + up to 4 KiB captured output.
    let mut buf = [0u8; 56 + 4096];
    for _ in 0..500 {
        let n = athkit::sys::script_status(id, &mut buf);
        if n == u64::MAX || n < 56 {
            write_err(b"rae: status failed\n");
            return;
        }
        let state = u32::from_ne_bytes([buf[16], buf[17], buf[18], buf[19]]);
        if state <= 1 {
            // Queued/Running — the daemon path. Let it breathe.
            athkit::sys::yield_now();
            continue;
        }
        let out_len = (n as usize) - 56;
        if out_len > 0 {
            write_out(&buf[56..56 + out_len]);
            if buf[56 + out_len - 1] != b'\n' {
                write_out(b"\n");
            }
        }
        let exit = i32::from_ne_bytes([buf[20], buf[21], buf[22], buf[23]]);
        match state {
            2 => {
                write_out(b"exit ");
                if exit < 0 {
                    write_out(b"-");
                    print_u64(exit.unsigned_abs() as u64);
                } else {
                    print_u64(exit as u64);
                }
                write_out(b"\n");
            }
            5 => write_err(b"rae: script timed out (out of fuel)\n"),
            4 => write_err(b"rae: script killed\n"),
            _ => write_err(b"rae: script failed\n"),
        }
        return;
    }
    write_err(b"rae: still running in athlangd (check later)\n");
}

fn read_file(path: &str) -> Option<alloc::vec::Vec<u8>> {
    let fd = athkit::sys::open(path, 0);
    if fd == u64::MAX {
        return None;
    }
    let mut out = alloc::vec::Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = athkit::sys::read(fd, &mut buf);
        if n == 0 || n == u64::MAX {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    athkit::sys::close(fd);
    Some(out)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_out(b"AthenaOS shell (ath-sh)\n");
    let home_owned = {
        let mut info = [0u8; 96];
        if let Some(_) = athkit::sys::session_info(&mut info) {
            if let Some(h) = athkit::sys::session_home_from(&info) {
                alloc::string::String::from(h)
            } else {
                alloc::string::String::from("/")
            }
        } else {
            alloc::string::String::from("/")
        }
    };
    let home = home_owned.as_str();
    let mut cwd = alloc::string::String::from(home);
    write_prompt();

    let mut line = [0u8; 256];
    let mut len = 0usize;

    loop {
        let mut ch = [0u8; 1];
        let n = athkit::sys::pty_slave_read(&mut ch);
        if n == 0 {
            athkit::sys::yield_now();
            continue;
        }

        let c = ch[0];
        if c == b'\r' {
            continue;
        }
        if c == b'\n' || c == 0x04 {
            write_out(b"\n");
            execute_line(&line[..len], &mut cwd, home);
            len = 0;
            write_prompt();
            continue;
        }
        if c == 0x7f || c == 0x08 {
            if len > 0 {
                len -= 1;
                write_out(b"\x08 \x08");
            }
            continue;
        }
        if len + 1 < line.len() && c >= 0x20 {
            line[len] = c;
            len += 1;
            write_out(&[c]);
        }
    }
}
