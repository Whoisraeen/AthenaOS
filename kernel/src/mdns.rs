//! mDNS / DNS-SD responder (Concept §AthNet: zero-config LAN discovery —
//! AthenaOS machines find each other and advertise services without any
//! server). MasterChecklist Phase 10.2 — "mDNS / DNS-SD for LAN discovery".
//!
//! The wire-format engine is `athnet::discovery::MdnsResponder` (RFC 6762/6763
//! encode/decode, PTR/SRV/TXT/A records, response cache). The kernel registers
//! the machine's own `_athena._tcp.local` service at boot and answers queries
//! through `handle_packet`. The smoketest proves the full DNS-SD round trip
//! deterministically — PTR + SRV queries are serialized to wire bytes,
//! answered by the live responder, and the responses fed back through the
//! parser into the cache — so it is identical on QEMU and iron with no live
//! network. Live UDP-5353 multicast socket polling is the wire-up follow-up
//! (MasterChecklist Phase 10.2 — same item, iron half).

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use athnet::discovery::{DnsRecordType, MdnsResponder, MdnsService};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

pub static RESPONDER: Mutex<Option<MdnsResponder>> = Mutex::new(None);

static QUERIES_ANSWERED: AtomicU64 = AtomicU64::new(0);

const SERVICE_NAME: &str = "AthenaOS";
const SERVICE_TYPE: &str = "_athena._tcp";
const SERVICE_PORT: u16 = 7690;

/// Register the machine's own service records and stand the responder up.
pub fn init() {
    let mut responder = MdnsResponder::new("athenaos");
    responder.register_service(
        MdnsService::new(SERVICE_NAME, SERVICE_TYPE, SERVICE_PORT).with_txt("os", "athena"),
    );
    *RESPONDER.lock() = Some(responder);
    crate::serial_println!(
        "[mdns] responder up: advertising {}.local (DNS-SD), host athenaos.local",
        SERVICE_TYPE,
    );
}

/// Feed an inbound mDNS packet (UDP 5353 payload) to the responder. Queries
/// that match our records return the response bytes to multicast back;
/// inbound responses are absorbed into the responder's cache and return None.
pub fn handle_packet(payload: &[u8]) -> Option<Vec<u8>> {
    let mut guard = RESPONDER.lock();
    let responder = guard.as_mut()?;
    let mut responses = responder.process_packet(payload);
    if responses.is_empty() {
        None
    } else {
        QUERIES_ANSWERED.fetch_add(1, Ordering::Relaxed);
        Some(responses.remove(0))
    }
}

/// Deterministic proof of the DNS-SD round trip: serialize PTR + SRV queries
/// for our own service, answer them through the live responder, feed the
/// responses back through the wire parser, and assert the cache holds the
/// PTR pointing at our instance and the SRV carrying port 7690.
pub fn run_boot_smoketest() {
    let service_domain = alloc::format!("{}.local", SERVICE_TYPE);
    let full_name = alloc::format!("{}.{}.local", SERVICE_NAME, SERVICE_TYPE);

    let (ptr_query, srv_query) = {
        let guard = RESPONDER.lock();
        match guard.as_ref() {
            Some(r) => (
                r.build_query(&service_domain, DnsRecordType::PTR),
                r.build_query(&full_name, DnsRecordType::SRV),
            ),
            None => {
                crate::serial_println!("[mdns] smoketest: responder not initialized -> FAIL");
                return;
            }
        }
    };

    // Query -> response through the real wire format.
    let ptr_response = handle_packet(&ptr_query);
    let srv_response = handle_packet(&srv_query);
    let answered = ptr_response.is_some() && srv_response.is_some();

    // Response bytes -> parser -> cache (the receive half of the round trip).
    if let Some(bytes) = ptr_response.as_deref() {
        handle_packet(bytes);
    }
    if let Some(bytes) = srv_response.as_deref() {
        handle_packet(bytes);
    }

    let (ptr_ok, srv_port_ok) = {
        let guard = RESPONDER.lock();
        match guard.as_ref() {
            Some(r) => {
                let want_ptr_target = MdnsResponder::encode_dns_name(&full_name);
                let ptr_ok = r.cache.iter().any(|e| {
                    e.name == service_domain
                        && e.record_type == DnsRecordType::PTR
                        && e.data == want_ptr_target
                });
                // SRV rdata: priority(2) weight(2) port(2) target(name).
                let srv_port_ok = r.cache.iter().any(|e| {
                    e.name == full_name
                        && e.record_type == DnsRecordType::SRV
                        && e.data.len() >= 6
                        && u16::from_be_bytes([e.data[4], e.data[5]]) == SERVICE_PORT
                });
                (ptr_ok, srv_port_ok)
            }
            None => (false, false),
        }
    };

    let pass = answered && ptr_ok && srv_port_ok;
    crate::serial_println!(
        "[mdns] smoketest: query_answered={} ptr_cached={} srv_port_{}={} -> {}",
        answered,
        ptr_ok,
        SERVICE_PORT,
        srv_port_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/athena/mdns` — responder state.
pub fn dump_text() -> String {
    let guard = RESPONDER.lock();
    match guard.as_ref() {
        Some(r) => alloc::format!(
            "# mDNS/DNS-SD responder ({}.local)\nhostname: {}.local\nservices: {}\ncache_entries: {}\nqueries_answered: {}\n",
            SERVICE_TYPE,
            r.hostname,
            r.services.len(),
            r.cache.len(),
            QUERIES_ANSWERED.load(Ordering::Relaxed),
        ),
        None => String::from("status: not initialized\n"),
    }
}
