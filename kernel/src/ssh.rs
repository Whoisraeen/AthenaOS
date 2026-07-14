//! In-kernel SSH server (`raessh`) — remote administration of RaeenOS.
//!
//! Concept §"The user owns the machine": you can reach a shell on your own OS
//! over the network, on your terms — no cloud broker, no telemetry, an SSH-2.0
//! server built from scratch (`components/raessh`, `#![forbid(unsafe_code)]`) on
//! RaeenOS's own KAT-proven crypto (`rae_crypto`: curve25519, ed25519,
//! chacha20-poly1305). This module is the kernel binding: it owns the ed25519
//! host key + the `authorized_keys` allow-list, proves the whole stack at boot
//! with a loopback self-test, and (next increment) drives a TCP `:22` listener
//! from the net poll loop, binding an authenticated shell channel to RaeShell.
//!
//! R10 contract: [`init`] (called from `kernel_main`) + [`run_boot_smoketest`]
//! (FAIL-able) + [`dump_text`] (`/proc/raeen/ssh`) + this docstring.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use raessh::userauth::AuthorizedKey;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use spin::Mutex;

/// The well-known SSH port the server listens on.
pub const SSH_PORT: u16 = 22;

/// Server identity + policy, established once at boot.
struct SshServer {
    /// The long-term ed25519 host-key seed (32-byte private) — a fresh random
    /// key per boot for now; persistence to RaeFS is a follow-up
    /// (MasterChecklist RaeNet SSH server: host-key persistence).
    host_seed: [u8; 32],
    /// The host public key (advertised to clients as `ssh-ed25519`).
    host_pub: [u8; 32],
    /// Keys permitted to authenticate. Empty until an `authorized_keys` source
    /// is wired (the TCP-listener increment) — so no client can log in yet even
    /// once the listener exists: fail-closed by construction.
    authorized: Vec<AuthorizedKey>,
    /// Result of the boot loopback self-test (the R10 proof).
    self_test_ok: bool,
    /// The smoltcp handle of the TCP `:22` listening socket, once bound.
    listen_handle: Option<SocketHandle>,
}

static SSH: Mutex<Option<SshServer>> = Mutex::new(None);

/// Generate the host key and register the (initially empty) policy. Safe to call
/// once from `kernel_main`; a real random host key comes from the kernel RNG.
pub fn init() {
    let mut host_seed = [0u8; 32];
    // A random per-boot host key. If the RNG is somehow unavailable, fall back
    // to a fixed dev seed rather than a weak all-zero key (still logged).
    if crate::crypto::getrandom(&mut host_seed).is_err() {
        host_seed = *b"RaeSSH-dev-fallback-hostkey-seed"; // exactly 32 bytes
        crate::serial_println!("[ssh] WARN: RNG unavailable, using dev host key");
    }
    let host_pub = rae_crypto::ed25519::derive_public_key(&host_seed);
    *SSH.lock() = Some(SshServer {
        host_seed,
        host_pub,
        authorized: Vec::new(),
        self_test_ok: false,
        listen_handle: None,
    });
    crate::serial_println!(
        "[ssh] host key ready: ed25519 fp={} (raessh SSH-2.0 server)",
        fingerprint(&host_pub)
    );
}

/// A short SHA-256 fingerprint of the host public key (first 8 bytes, hex) —
/// enough to eyeball key identity in the bootlog / procfs.
fn fingerprint(host_pub: &[u8; 32]) -> String {
    let d = rae_crypto::sha256::sha256(host_pub);
    let mut s = String::with_capacity(23);
    for (i, b) in d[..8].iter().enumerate() {
        if i != 0 {
            s.push(':');
        }
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// R10 boot smoketest: run the raessh loopback (a full handshake → publickey
/// auth → shell channel against a simulated client) IN the kernel, proving the
/// whole SSH stack + `rae_crypto` work under the kernel's build-std/soft-float
/// config. FAIL-able: a regression prints `FAIL` with the failing stage.
pub fn run_boot_smoketest() {
    // Copy the real host key out, then release the lock BEFORE running the
    // loopback (it must not be held across the crypto work / a re-lock).
    let key = SSH.lock().as_ref().map(|s| (s.host_seed, s.host_pub));
    let Some((host_seed, host_pub)) = key else {
        crate::serial_println!("[ssh] loopback self-test -> FAIL (not initialized)");
        return;
    };
    match raessh::selftest::loopback(&host_seed, &host_pub) {
        Ok(()) => {
            if let Some(s) = SSH.lock().as_mut() {
                s.self_test_ok = true;
            }
            crate::serial_println!(
                "[ssh] loopback self-test: real host key, handshake+auth+shell-channel OK -> PASS"
            );
        }
        Err(reason) => {
            crate::serial_println!("[ssh] loopback self-test -> FAIL ({})", reason);
        }
    }
}

/// Bind the TCP `:22` listening socket (Increment B1 — the first `listen()` in
/// the kernel). The socket sits in `Listen` state in the shared `NET_STACK`
/// socket set; once DHCP assigns an IP, a client SYN to `<ip>:22` transitions it
/// to `Established` and [`tick`] (next increment) drives the SSH handshake.
/// FAIL-able: prints FAIL if the net stack is down or the socket does not reach
/// `Listen`.
pub fn start_listener() {
    let mut guard = crate::net::NET_STACK.lock();
    let Some(stack) = guard.as_mut() else {
        crate::serial_println!("[ssh] listener -> FAIL (net stack not initialized)");
        return;
    };
    let rx = tcp::SocketBuffer::new(alloc::vec![0u8; 8192]);
    let tx = tcp::SocketBuffer::new(alloc::vec![0u8; 8192]);
    let mut socket = tcp::Socket::new(rx, tx);
    if socket.listen(SSH_PORT).is_err() {
        crate::serial_println!("[ssh] listener -> FAIL (listen({}) rejected)", SSH_PORT);
        return;
    }
    let is_listening = socket.state() == tcp::State::Listen;
    let handle = stack.sockets.add(socket);
    drop(guard);

    if let Some(s) = SSH.lock().as_mut() {
        s.listen_handle = Some(handle);
    }
    if is_listening {
        crate::serial_println!(
            "[ssh] listening on TCP :{} (state=Listen) -> PASS",
            SSH_PORT
        );
    } else {
        crate::serial_println!("[ssh] listener -> FAIL (socket did not reach Listen)");
    }
}

/// `/proc/raeen/ssh` — server identity + readiness (the R10 procfs line).
pub fn dump_text() -> String {
    let guard = SSH.lock();
    match guard.as_ref() {
        None => String::from("ssh: not initialized\n"),
        Some(s) => format!(
            "ssh: raessh SSH-2.0 server\n\
             host_key: ssh-ed25519 fp={}\n\
             self_test: {}\n\
             authorized_keys: {}\n\
             listener: {}\n",
            fingerprint(&s.host_pub),
            if s.self_test_ok {
                "PASS"
            } else {
                "FAIL/pending"
            },
            s.authorized.len(),
            if s.listen_handle.is_some() {
                "TCP :22 (Listen); per-connection pump is the next increment"
            } else {
                "not bound"
            },
        ),
    }
}
