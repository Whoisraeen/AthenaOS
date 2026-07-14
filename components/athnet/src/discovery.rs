extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    InvalidPacket(String),
    NameTooLong,
    MalformedLabel,
    TruncatedData,
    UnsupportedRecordType(u16),
    InvalidHeader(String),
}

// ---------------------------------------------------------------------------
// DNS record types (mDNS uses standard DNS wire format)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DnsRecordType {
    A = 1,
    AAAA = 28,
    PTR = 12,
    SRV = 33,
    TXT = 16,
    ANY = 255,
}

impl DnsRecordType {
    pub fn from_u16(val: u16) -> Option<Self> {
        match val {
            1 => Some(Self::A),
            28 => Some(Self::AAAA),
            12 => Some(Self::PTR),
            33 => Some(Self::SRV),
            16 => Some(Self::TXT),
            255 => Some(Self::ANY),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// DNS packet structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DnsPacket {
    pub id: u16,
    pub flags: u16,
    pub questions: Vec<DnsQuestion>,
    pub answers: Vec<DnsResourceRecord>,
    pub authorities: Vec<DnsResourceRecord>,
    pub additionals: Vec<DnsResourceRecord>,
}

impl DnsPacket {
    pub fn new_query(id: u16) -> Self {
        Self {
            id,
            flags: 0x0000,
            questions: Vec::new(),
            answers: Vec::new(),
            authorities: Vec::new(),
            additionals: Vec::new(),
        }
    }

    pub fn new_response(id: u16) -> Self {
        Self {
            id,
            flags: 0x8400, // QR=1, AA=1
            questions: Vec::new(),
            answers: Vec::new(),
            authorities: Vec::new(),
            additionals: Vec::new(),
        }
    }

    pub fn is_response(&self) -> bool {
        (self.flags & 0x8000) != 0
    }

    pub fn is_authoritative(&self) -> bool {
        (self.flags & 0x0400) != 0
    }
}

#[derive(Debug, Clone)]
pub struct DnsQuestion {
    pub name: String,
    pub record_type: DnsRecordType,
    pub class: u16,
    pub unicast: bool,
}

#[derive(Debug, Clone)]
pub struct DnsResourceRecord {
    pub name: String,
    pub record_type: DnsRecordType,
    pub class: u16,
    pub ttl: u32,
    pub data: Vec<u8>,
    pub cache_flush: bool,
}

impl DnsResourceRecord {
    pub fn a_record(name: &str, ip: [u8; 4], ttl: u32) -> Self {
        Self {
            name: String::from(name),
            record_type: DnsRecordType::A,
            class: 1,
            ttl,
            data: ip.to_vec(),
            cache_flush: true,
        }
    }

    pub fn ptr_record(name: &str, target: &str, ttl: u32) -> Self {
        Self {
            name: String::from(name),
            record_type: DnsRecordType::PTR,
            class: 1,
            ttl,
            data: MdnsResponder::encode_dns_name(target),
            cache_flush: false,
        }
    }
}

// ---------------------------------------------------------------------------
// mDNS types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MdnsService {
    pub name: String,
    pub service_type: String,
    pub domain: String,
    pub port: u16,
    pub txt_records: BTreeMap<String, String>,
    pub priority: u16,
    pub weight: u16,
    pub ttl: u32,
}

impl MdnsService {
    pub fn new(name: &str, service_type: &str, port: u16) -> Self {
        Self {
            name: String::from(name),
            service_type: String::from(service_type),
            domain: String::from("local"),
            port,
            txt_records: BTreeMap::new(),
            priority: 0,
            weight: 0,
            ttl: 4500,
        }
    }

    pub fn full_name(&self) -> String {
        format!("{}.{}.{}", self.name, self.service_type, self.domain)
    }

    pub fn service_domain(&self) -> String {
        format!("{}.{}", self.service_type, self.domain)
    }

    pub fn with_txt(mut self, key: &str, value: &str) -> Self {
        self.txt_records
            .insert(String::from(key), String::from(value));
        self
    }
}

#[derive(Debug, Clone)]
pub struct MdnsHost {
    pub hostname: String,
    pub ipv4: Option<[u8; 4]>,
    pub ipv6: Option<[u8; 16]>,
    pub last_seen: u64,
}

#[derive(Debug, Clone)]
pub struct MdnsCacheEntry {
    pub name: String,
    pub record_type: DnsRecordType,
    pub data: Vec<u8>,
    pub ttl: u32,
    pub received_at: u64,
}

impl MdnsCacheEntry {
    pub fn is_expired(&self, now: u64) -> bool {
        now.saturating_sub(self.received_at) > self.ttl as u64 * 1000
    }
}

#[derive(Debug, Clone)]
pub struct MdnsQuery {
    pub name: String,
    pub record_type: DnsRecordType,
    pub unicast_response: bool,
}

// ---------------------------------------------------------------------------
// mDNS Responder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MdnsResponder {
    pub hostname: String,
    pub services: Vec<MdnsService>,
    pub cache: Vec<MdnsCacheEntry>,
    query_queue: Vec<MdnsQuery>,
    known_hosts: BTreeMap<String, MdnsHost>,
    multicast_addr: [u8; 4],
    port: u16,
}

impl MdnsResponder {
    pub fn new(hostname: &str) -> Self {
        Self {
            hostname: String::from(hostname),
            services: Vec::new(),
            cache: Vec::new(),
            query_queue: Vec::new(),
            known_hosts: BTreeMap::new(),
            multicast_addr: [224, 0, 0, 251],
            port: 5353,
        }
    }

    pub fn register_service(&mut self, service: MdnsService) {
        if !self
            .services
            .iter()
            .any(|s| s.name == service.name && s.service_type == service.service_type)
        {
            self.services.push(service);
        }
    }

    pub fn unregister_service(&mut self, name: &str) {
        self.services.retain(|s| s.name != name);
    }

    pub fn query_service(&mut self, service_type: &str) -> Vec<MdnsService> {
        self.query_queue.push(MdnsQuery {
            name: format!("{}.local", service_type),
            record_type: DnsRecordType::PTR,
            unicast_response: false,
        });

        self.services
            .iter()
            .filter(|s| s.service_type == service_type)
            .cloned()
            .collect()
    }

    pub fn resolve_hostname(&mut self, hostname: &str) -> Option<[u8; 4]> {
        if let Some(host) = self.known_hosts.get(hostname) {
            return host.ipv4;
        }

        self.query_queue.push(MdnsQuery {
            name: String::from(hostname),
            record_type: DnsRecordType::A,
            unicast_response: true,
        });

        None
    }

    pub fn browse_services(&self) -> Vec<&MdnsService> {
        self.services.iter().collect()
    }

    pub fn process_packet(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        let mut responses = Vec::new();

        let packet = match Self::parse_dns_packet(data) {
            Ok(p) => p,
            Err(_) => return responses,
        };

        if packet.is_response() {
            for answer in &packet.answers {
                self.cache.push(MdnsCacheEntry {
                    name: answer.name.clone(),
                    record_type: answer.record_type,
                    data: answer.data.clone(),
                    ttl: answer.ttl,
                    received_at: 0,
                });

                if answer.record_type == DnsRecordType::A && answer.data.len() == 4 {
                    let ip = [
                        answer.data[0],
                        answer.data[1],
                        answer.data[2],
                        answer.data[3],
                    ];
                    let host = self
                        .known_hosts
                        .entry(answer.name.clone())
                        .or_insert(MdnsHost {
                            hostname: answer.name.clone(),
                            ipv4: None,
                            ipv6: None,
                            last_seen: 0,
                        });
                    host.ipv4 = Some(ip);
                }
            }
        } else {
            let queries: Vec<MdnsQuery> = packet
                .questions
                .iter()
                .map(|q| MdnsQuery {
                    name: q.name.clone(),
                    record_type: q.record_type,
                    unicast_response: q.unicast,
                })
                .collect();

            let response = self.build_response(&queries);
            if !response.is_empty() {
                responses.push(response);
            }
        }

        responses
    }

    pub fn build_query(&self, name: &str, record_type: DnsRecordType) -> Vec<u8> {
        let mut packet = DnsPacket::new_query(0);
        packet.questions.push(DnsQuestion {
            name: String::from(name),
            record_type,
            class: 1,
            unicast: false,
        });
        Self::serialize_dns_packet(&packet)
    }

    pub fn build_response(&self, queries: &[MdnsQuery]) -> Vec<u8> {
        let mut packet = DnsPacket::new_response(0);

        for query in queries {
            for service in &self.services {
                let service_domain = service.service_domain();

                match query.record_type {
                    DnsRecordType::PTR => {
                        if query.name == service_domain || query.record_type == DnsRecordType::ANY {
                            packet.answers.push(DnsResourceRecord {
                                name: service_domain.clone(),
                                record_type: DnsRecordType::PTR,
                                class: 1,
                                ttl: service.ttl,
                                data: Self::encode_dns_name(&service.full_name()),
                                cache_flush: false,
                            });
                        }
                    }
                    DnsRecordType::SRV => {
                        if query.name == service.full_name() {
                            let mut data = Vec::new();
                            data.extend_from_slice(&service.priority.to_be_bytes());
                            data.extend_from_slice(&service.weight.to_be_bytes());
                            data.extend_from_slice(&service.port.to_be_bytes());
                            data.extend_from_slice(&Self::encode_dns_name(&format!(
                                "{}.local",
                                self.hostname
                            )));
                            packet.answers.push(DnsResourceRecord {
                                name: service.full_name(),
                                record_type: DnsRecordType::SRV,
                                class: 1,
                                ttl: service.ttl,
                                data,
                                cache_flush: true,
                            });
                        }
                    }
                    DnsRecordType::TXT => {
                        if query.name == service.full_name() {
                            let mut data = Vec::new();
                            for (key, value) in &service.txt_records {
                                let entry = format!("{}={}", key, value);
                                data.push(entry.len() as u8);
                                data.extend_from_slice(entry.as_bytes());
                            }
                            if data.is_empty() {
                                data.push(0);
                            }
                            packet.answers.push(DnsResourceRecord {
                                name: service.full_name(),
                                record_type: DnsRecordType::TXT,
                                class: 1,
                                ttl: service.ttl,
                                data,
                                cache_flush: true,
                            });
                        }
                    }
                    DnsRecordType::A | DnsRecordType::AAAA | DnsRecordType::ANY => {}
                }
            }
        }

        if packet.answers.is_empty() {
            return Vec::new();
        }

        Self::serialize_dns_packet(&packet)
    }

    pub fn build_announcement(&self) -> Vec<u8> {
        let mut packet = DnsPacket::new_response(0);

        for service in &self.services {
            packet.answers.push(DnsResourceRecord {
                name: service.service_domain(),
                record_type: DnsRecordType::PTR,
                class: 1,
                ttl: service.ttl,
                data: Self::encode_dns_name(&service.full_name()),
                cache_flush: false,
            });
        }

        Self::serialize_dns_packet(&packet)
    }

    pub fn build_goodbye(&self) -> Vec<u8> {
        let mut packet = DnsPacket::new_response(0);

        for service in &self.services {
            packet.answers.push(DnsResourceRecord {
                name: service.service_domain(),
                record_type: DnsRecordType::PTR,
                class: 1,
                ttl: 0,
                data: Self::encode_dns_name(&service.full_name()),
                cache_flush: false,
            });
        }

        Self::serialize_dns_packet(&packet)
    }

    pub fn tick(&mut self, now: u64) -> Vec<Vec<u8>> {
        let mut outgoing = Vec::new();

        self.cache_cleanup(now);

        let queries: Vec<MdnsQuery> = self.query_queue.drain(..).collect();
        for query in &queries {
            outgoing.push(self.build_query(&query.name, query.record_type));
        }

        outgoing
    }

    fn parse_dns_packet(data: &[u8]) -> Result<DnsPacket, DiscoveryError> {
        if data.len() < 12 {
            return Err(DiscoveryError::TruncatedData);
        }

        let id = ((data[0] as u16) << 8) | (data[1] as u16);
        let flags = ((data[2] as u16) << 8) | (data[3] as u16);
        let qd_count = ((data[4] as u16) << 8) | (data[5] as u16);
        let an_count = ((data[6] as u16) << 8) | (data[7] as u16);
        let ns_count = ((data[8] as u16) << 8) | (data[9] as u16);
        let ar_count = ((data[10] as u16) << 8) | (data[11] as u16);

        let mut offset = 12;
        let mut questions = Vec::new();

        for _ in 0..qd_count {
            let (name, new_offset) = Self::decode_dns_name(data, offset)?;
            offset = new_offset;
            if offset + 4 > data.len() {
                return Err(DiscoveryError::TruncatedData);
            }
            let qtype = ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
            let qclass = ((data[offset + 2] as u16) << 8) | (data[offset + 3] as u16);
            offset += 4;

            let unicast = (qclass & 0x8000) != 0;
            let record_type = DnsRecordType::from_u16(qtype).unwrap_or(DnsRecordType::ANY);

            questions.push(DnsQuestion {
                name,
                record_type,
                class: qclass & 0x7FFF,
                unicast,
            });
        }

        fn parse_rrs(
            data: &[u8],
            count: u16,
            offset: &mut usize,
        ) -> Result<Vec<DnsResourceRecord>, DiscoveryError> {
            let mut records = Vec::new();
            for _ in 0..count {
                let (name, new_offset) = MdnsResponder::decode_dns_name(data, *offset)?;
                *offset = new_offset;
                if *offset + 10 > data.len() {
                    return Err(DiscoveryError::TruncatedData);
                }
                let rtype = ((data[*offset] as u16) << 8) | (data[*offset + 1] as u16);
                let rclass = ((data[*offset + 2] as u16) << 8) | (data[*offset + 3] as u16);
                let ttl = ((data[*offset + 4] as u32) << 24)
                    | ((data[*offset + 5] as u32) << 16)
                    | ((data[*offset + 6] as u32) << 8)
                    | (data[*offset + 7] as u32);
                let rdlength = ((data[*offset + 8] as u16) << 8) | (data[*offset + 9] as u16);
                *offset += 10;

                if *offset + rdlength as usize > data.len() {
                    return Err(DiscoveryError::TruncatedData);
                }

                let rdata = data[*offset..*offset + rdlength as usize].to_vec();
                *offset += rdlength as usize;

                let cache_flush = (rclass & 0x8000) != 0;
                let record_type = DnsRecordType::from_u16(rtype).unwrap_or(DnsRecordType::ANY);

                records.push(DnsResourceRecord {
                    name,
                    record_type,
                    class: rclass & 0x7FFF,
                    ttl,
                    data: rdata,
                    cache_flush,
                });
            }
            Ok(records)
        }

        let answers = parse_rrs(data, an_count, &mut offset)?;
        let authorities = parse_rrs(data, ns_count, &mut offset)?;
        let additionals = parse_rrs(data, ar_count, &mut offset)?;

        Ok(DnsPacket {
            id,
            flags,
            questions,
            answers,
            authorities,
            additionals,
        })
    }

    pub fn encode_dns_name(name: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        for label in name.split('.') {
            if label.is_empty() {
                continue;
            }
            buf.push(label.len() as u8);
            buf.extend_from_slice(label.as_bytes());
        }
        buf.push(0);
        buf
    }

    fn decode_dns_name(data: &[u8], offset: usize) -> Result<(String, usize), DiscoveryError> {
        let mut name = String::new();
        let mut pos = offset;
        let mut jumped = false;
        let mut return_pos = 0;
        let mut jumps = 0;

        loop {
            if pos >= data.len() {
                return Err(DiscoveryError::TruncatedData);
            }

            let len = data[pos] as usize;

            if len == 0 {
                if !jumped {
                    return_pos = pos + 1;
                }
                break;
            }

            if (len & 0xC0) == 0xC0 {
                if pos + 1 >= data.len() {
                    return Err(DiscoveryError::TruncatedData);
                }
                if !jumped {
                    return_pos = pos + 2;
                }
                let ptr = (((len & 0x3F) as usize) << 8) | (data[pos + 1] as usize);
                pos = ptr;
                jumped = true;
                jumps += 1;
                if jumps > 10 {
                    return Err(DiscoveryError::MalformedLabel);
                }
                continue;
            }

            pos += 1;
            if pos + len > data.len() {
                return Err(DiscoveryError::TruncatedData);
            }

            if !name.is_empty() {
                name.push('.');
            }

            let label = core::str::from_utf8(&data[pos..pos + len])
                .map_err(|_| DiscoveryError::MalformedLabel)?;
            name.push_str(label);
            pos += len;
        }

        let final_pos = if jumped { return_pos } else { return_pos };
        Ok((name, final_pos))
    }

    fn cache_cleanup(&mut self, now: u64) {
        self.cache.retain(|entry| !entry.is_expired(now));
    }

    fn serialize_dns_packet(packet: &DnsPacket) -> Vec<u8> {
        let mut buf = Vec::with_capacity(512);

        buf.push((packet.id >> 8) as u8);
        buf.push((packet.id & 0xFF) as u8);
        buf.push((packet.flags >> 8) as u8);
        buf.push((packet.flags & 0xFF) as u8);

        let qd_count = packet.questions.len() as u16;
        let an_count = packet.answers.len() as u16;
        let ns_count = packet.authorities.len() as u16;
        let ar_count = packet.additionals.len() as u16;

        buf.push((qd_count >> 8) as u8);
        buf.push((qd_count & 0xFF) as u8);
        buf.push((an_count >> 8) as u8);
        buf.push((an_count & 0xFF) as u8);
        buf.push((ns_count >> 8) as u8);
        buf.push((ns_count & 0xFF) as u8);
        buf.push((ar_count >> 8) as u8);
        buf.push((ar_count & 0xFF) as u8);

        for q in &packet.questions {
            buf.extend_from_slice(&Self::encode_dns_name(&q.name));
            buf.push((q.record_type as u16 >> 8) as u8);
            buf.push((q.record_type as u16 & 0xFF) as u8);
            let class_val = q.class | (if q.unicast { 0x8000 } else { 0 });
            buf.push((class_val >> 8) as u8);
            buf.push((class_val & 0xFF) as u8);
        }

        fn serialize_rrs(buf: &mut Vec<u8>, records: &[DnsResourceRecord]) {
            for rr in records {
                buf.extend_from_slice(&MdnsResponder::encode_dns_name(&rr.name));
                buf.push((rr.record_type as u16 >> 8) as u8);
                buf.push((rr.record_type as u16 & 0xFF) as u8);
                let class_val = rr.class | (if rr.cache_flush { 0x8000 } else { 0 });
                buf.push((class_val >> 8) as u8);
                buf.push((class_val & 0xFF) as u8);
                buf.push((rr.ttl >> 24) as u8);
                buf.push((rr.ttl >> 16) as u8);
                buf.push((rr.ttl >> 8) as u8);
                buf.push(rr.ttl as u8);
                let rdlen = rr.data.len() as u16;
                buf.push((rdlen >> 8) as u8);
                buf.push((rdlen & 0xFF) as u8);
                buf.extend_from_slice(&rr.data);
            }
        }

        serialize_rrs(&mut buf, &packet.answers);
        serialize_rrs(&mut buf, &packet.authorities);
        serialize_rrs(&mut buf, &packet.additionals);

        buf
    }
}

// ---------------------------------------------------------------------------
// SSDP (Simple Service Discovery Protocol)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SsdpDevice {
    pub usn: String,
    pub location: String,
    pub server: String,
    pub service_type: String,
    pub cache_control: u32,
    pub discovered_at: u64,
    pub extra_headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub enum SsdpEvent {
    Alive(SsdpDevice),
    ByeBye(String),
    Update(SsdpDevice),
}

/// Hard cap on how many distinct SSDP devices we will track. A LAN peer can
/// trivially spam unique USNs (`process_response`/`process_notify` push one
/// entry per never-before-seen USN); without a cap `services` grows unbounded
/// from attacker-controlled network bytes (criterion #6). Past the cap, new
/// USNs are ignored — existing entries still update.
pub const MAX_DISCOVERED_SERVICES: usize = 256;

#[derive(Debug, Clone)]
pub struct SsdpClient {
    pub services: Vec<SsdpDevice>,
    pub search_targets: Vec<String>,
    multicast_addr: [u8; 4],
    port: u16,
    mx: u8,
}

impl SsdpClient {
    pub fn new() -> Self {
        Self {
            services: Vec::new(),
            search_targets: Vec::new(),
            multicast_addr: [239, 255, 255, 250],
            port: 1900,
            mx: 3,
        }
    }

    pub fn search(&mut self, target: &str) -> Vec<u8> {
        if !self.search_targets.contains(&String::from(target)) {
            self.search_targets.push(String::from(target));
        }
        Self::build_msearch(target, self.mx)
    }

    pub fn process_response(&mut self, data: &[u8]) -> Option<SsdpDevice> {
        let text = core::str::from_utf8(data).ok()?;

        if !text.starts_with("HTTP/1.1 200") {
            return None;
        }

        let headers = Self::parse_ssdp_headers(text);

        let usn = headers.get("usn")?.clone();
        let location = headers.get("location").cloned().unwrap_or_default();
        let server = headers.get("server").cloned().unwrap_or_default();
        let st = headers.get("st").cloned().unwrap_or_default();

        let cache_control = headers
            .get("cache-control")
            .and_then(|v| {
                v.strip_prefix("max-age=")
                    .or_else(|| v.strip_prefix("max-age = "))
            })
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(1800);

        let mut extra = headers.clone();
        extra.remove("usn");
        extra.remove("location");
        extra.remove("server");
        extra.remove("st");
        extra.remove("cache-control");

        let device = SsdpDevice {
            usn: usn.clone(),
            location,
            server,
            service_type: st,
            cache_control,
            discovered_at: 0,
            extra_headers: extra,
        };

        if let Some(existing) = self.services.iter_mut().find(|s| s.usn == usn) {
            *existing = device.clone();
        } else {
            // Cap distinct devices: drop new USNs past the limit (LOW DoS).
            if self.services.len() >= MAX_DISCOVERED_SERVICES {
                return None;
            }
            self.services.push(device.clone());
        }

        Some(device)
    }

    pub fn process_notify(&mut self, data: &[u8]) -> Option<SsdpEvent> {
        let text = core::str::from_utf8(data).ok()?;

        if !text.starts_with("NOTIFY") {
            return None;
        }

        let headers = Self::parse_ssdp_headers(text);

        let nts = headers.get("nts")?;
        let usn = headers.get("usn")?.clone();

        match nts.as_str() {
            "ssdp:alive" => {
                let location = headers.get("location").cloned().unwrap_or_default();
                let server = headers.get("server").cloned().unwrap_or_default();
                let nt = headers.get("nt").cloned().unwrap_or_default();
                let cache_control = headers
                    .get("cache-control")
                    .and_then(|v| v.strip_prefix("max-age="))
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .unwrap_or(1800);

                let device = SsdpDevice {
                    usn: usn.clone(),
                    location,
                    server,
                    service_type: nt,
                    cache_control,
                    discovered_at: 0,
                    extra_headers: BTreeMap::new(),
                };

                if let Some(existing) = self.services.iter_mut().find(|s| s.usn == usn) {
                    *existing = device.clone();
                    Some(SsdpEvent::Update(device))
                } else {
                    // Cap distinct devices: drop new USNs past the limit (LOW DoS).
                    if self.services.len() >= MAX_DISCOVERED_SERVICES {
                        return None;
                    }
                    self.services.push(device.clone());
                    Some(SsdpEvent::Alive(device))
                }
            }
            "ssdp:byebye" => {
                self.services.retain(|s| s.usn != usn);
                Some(SsdpEvent::ByeBye(usn))
            }
            "ssdp:update" => {
                let location = headers.get("location").cloned().unwrap_or_default();
                if let Some(existing) = self.services.iter_mut().find(|s| s.usn == usn) {
                    existing.location = location;
                    Some(SsdpEvent::Update(existing.clone()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn discovered_devices(&self) -> &[SsdpDevice] {
        &self.services
    }

    pub fn cleanup_expired(&mut self, now: u64) {
        self.services
            .retain(|s| now.saturating_sub(s.discovered_at) < (s.cache_control as u64) * 1000);
    }

    fn parse_ssdp_headers(data: &str) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::new();

        for line in data.split("\r\n").skip(1) {
            if line.is_empty() {
                break;
            }
            if let Some(colon) = line.find(':') {
                let key = line[..colon].trim().to_ascii_lowercase();
                let value = line[colon + 1..].trim();
                headers.insert(key, String::from(value));
            }
        }

        headers
    }

    fn build_msearch(target: &str, mx: u8) -> Vec<u8> {
        let request = format!(
            "M-SEARCH * HTTP/1.1\r\n\
             HOST: 239.255.255.250:1900\r\n\
             MAN: \"ssdp:discover\"\r\n\
             MX: {}\r\n\
             ST: {}\r\n\
             \r\n",
            mx, target
        );
        request.into_bytes()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_dns_name() {
        let encoded = MdnsResponder::encode_dns_name("_http._tcp.local");
        assert_eq!(encoded[0], 5); // "_http" length
        assert_eq!(*encoded.last().unwrap(), 0); // null terminator
    }

    #[test]
    fn test_mdns_service_registration() {
        let mut responder = MdnsResponder::new("myhost");
        let service = MdnsService::new("Web Server", "_http._tcp", 8080);
        responder.register_service(service);
        assert_eq!(responder.services.len(), 1);
        assert_eq!(responder.services[0].port, 8080);
    }

    #[test]
    fn test_ssdp_msearch() {
        let mut client = SsdpClient::new();
        let packet = client.search("ssdp:all");
        let text = core::str::from_utf8(&packet).unwrap();
        assert!(text.starts_with("M-SEARCH"));
        assert!(text.contains("ST: ssdp:all"));
        assert!(text.contains("239.255.255.250:1900"));
    }

    #[test]
    fn test_ssdp_response_parsing() {
        let mut client = SsdpClient::new();
        let response = b"HTTP/1.1 200 OK\r\n\
            USN: uuid:device-1\r\n\
            LOCATION: http://192.168.1.1:80/desc.xml\r\n\
            ST: upnp:rootdevice\r\n\
            SERVER: Linux/3.0 UPnP/1.0\r\n\
            CACHE-CONTROL: max-age=1800\r\n\
            \r\n";

        let device = client.process_response(response).unwrap();
        assert_eq!(device.usn, "uuid:device-1");
        assert_eq!(device.service_type, "upnp:rootdevice");
        assert_eq!(device.cache_control, 1800);
    }

    // LOW remote-DoS regression (criterion #6): a LAN peer spamming responses
    // with ever-changing unique USNs must NOT grow `services` without bound. On
    // the pre-fix code every unique USN pushed a new entry forever; now we cap
    // at MAX_DISCOVERED_SERVICES and drop new USNs past it. Pre-fix this test
    // would assert MAX+extra entries and FAIL.
    #[test]
    fn ssdp_unique_usn_flood_is_capped() {
        let mut client = SsdpClient::new();
        let total = MAX_DISCOVERED_SERVICES + 100;
        for i in 0..total {
            let response = alloc::format!(
                "HTTP/1.1 200 OK\r\n\
                 USN: uuid:flood-device-{}\r\n\
                 LOCATION: http://192.168.1.1:80/desc.xml\r\n\
                 ST: upnp:rootdevice\r\n\
                 SERVER: Evil/1.0\r\n\
                 CACHE-CONTROL: max-age=1800\r\n\
                 \r\n",
                i
            );
            let _ = client.process_response(response.as_bytes());
        }
        assert_eq!(
            client.services.len(),
            MAX_DISCOVERED_SERVICES,
            "service list must be capped at MAX_DISCOVERED_SERVICES"
        );
        // An ALREADY-KNOWN USN must still update (no false denial of existing).
        let known = alloc::format!(
            "HTTP/1.1 200 OK\r\n\
             USN: uuid:flood-device-0\r\n\
             LOCATION: http://10.0.0.9:80/new.xml\r\n\
             ST: upnp:rootdevice\r\n\
             CACHE-CONTROL: max-age=900\r\n\
             \r\n"
        );
        let updated = client.process_response(known.as_bytes());
        assert!(
            updated.is_some(),
            "known USN must still update past the cap"
        );
        assert_eq!(client.services.len(), MAX_DISCOVERED_SERVICES);
    }

    #[test]
    fn test_mdns_announcement() {
        let mut responder = MdnsResponder::new("testhost");
        responder.register_service(MdnsService::new("Test", "_http._tcp", 80));
        let announcement = responder.build_announcement();
        assert!(!announcement.is_empty());
        assert!(announcement.len() >= 12);
    }

    // ───────────────────────── Fuzz / property hardening ─────────────────────
    //
    // `process_packet` / `parse_dns_packet` / `decode_dns_name` decode bytes
    // that arrive from any host on the LAN multicast group (mDNS) — fully
    // attacker-controlled. The classic DNS exploit is a name-compression
    // pointer that loops (0xC0 0x?? pointing at itself or backwards) or points
    // past the end of the packet; a naive decoder then loops forever or reads
    // out of bounds. These tests assert the decoder NEVER panics and ALWAYS
    // terminates on hostile input, with bounded work/allocation.
    //
    // Self-contained xorshift PRNG (no external fuzz crate, no_std-safe).

    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Self {
            // Avoid the zero fixed-point of xorshift.
            Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn byte(&mut self) -> u8 {
            (self.next_u64() & 0xFF) as u8
        }
        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                (self.next_u64() % n as u64) as usize
            }
        }
    }

    /// Driving the full public entry on random bytes must never panic and must
    /// always return (no hang). `process_packet` swallows parse errors, so the
    /// pass condition is simply "returns".
    #[test]
    fn fuzz_process_packet_random_never_panics() {
        let mut rng = Rng::new(0xD15C0_5EED);
        let mut responder = MdnsResponder::new("fuzzhost");
        for _ in 0..20_000 {
            let len = rng.below(300);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            // Must return (terminate) without panicking, OOB, or hanging.
            let _ = responder.process_packet(&buf);
        }
    }

    /// A valid-looking header with random count fields and random body is the
    /// nastiest case: the RR loops trust qd/an/ns/ar counts. Bound checks must
    /// hold for every truncation.
    #[test]
    fn fuzz_parse_dns_packet_random_header_never_panics() {
        let mut rng = Rng::new(0xB16_F00D);
        for _ in 0..30_000 {
            let len = 12 + rng.below(120);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            // Direct parser call: Ok or Err, never panic / never loop forever.
            let _ = MdnsResponder::parse_dns_packet(&buf);
        }
    }

    /// Truncate a well-formed announcement at EVERY possible offset. Each prefix
    /// must parse to Ok or Err — never panic.
    #[test]
    fn fuzz_truncated_at_every_offset_never_panics() {
        let mut responder = MdnsResponder::new("trunchost");
        responder.register_service(MdnsService::new("Web", "_http._tcp", 8080));
        responder.register_service(MdnsService::new("SSH", "_ssh._tcp", 22));
        let full = responder.build_announcement();
        assert!(full.len() > 12);
        for cut in 0..=full.len() {
            let _ = MdnsResponder::parse_dns_packet(&full[..cut]);
            let mut r2 = MdnsResponder::new("h");
            let _ = r2.process_packet(&full[..cut]);
        }
    }

    /// Compression-pointer LOOP: a name at offset 12 whose first byte is a
    /// pointer back to offset 12 (points at itself). A vulnerable decoder spins
    /// forever; this must return Err in bounded time (the jumps cap fires).
    #[test]
    fn fuzz_dns_name_self_pointer_terminates() {
        // 12-byte header (1 question), then the question name = pointer→12.
        let mut pkt = vec![0u8; 12];
        pkt[5] = 1; // qd_count = 1
                    // 0xC0 0x0C → pointer to offset 12 (itself).
        pkt.push(0xC0);
        pkt.push(0x0C);
        // qtype/qclass padding so a non-looping decode wouldn't trip truncation.
        pkt.extend_from_slice(&[0, 0, 0, 0]);
        // Must terminate (Err), never hang.
        let res = MdnsResponder::parse_dns_packet(&pkt);
        assert!(
            res.is_err(),
            "self-referential compression pointer must be rejected"
        );
    }

    /// A chain of pointers each hopping one byte forward, forming a long ladder.
    /// The jumps cap must stop it well before exhausting the packet.
    #[test]
    fn fuzz_dns_name_pointer_chain_terminates() {
        let mut pkt = vec![0u8; 12];
        pkt[5] = 1; // qd_count = 1
        let base = pkt.len(); // 12
                              // Build a chain: at each even slot a pointer to the next slot.
                              // slot k at offset base+2k → points to base+2(k+1).
        let chain = 64usize;
        for k in 0..chain {
            let target = (base + 2 * (k + 1)) as u16;
            pkt.push(0xC0 | ((target >> 8) as u8 & 0x3F));
            pkt.push((target & 0xFF) as u8);
        }
        // Terminating zero label at the end.
        pkt.push(0);
        pkt.extend_from_slice(&[0, 0, 0, 0]);
        let res = MdnsResponder::parse_dns_packet(&pkt);
        // Either the jumps cap rejects it or it resolves cleanly — but it MUST
        // have returned (this test passing at all proves no infinite loop).
        let _ = res;
    }

    /// Pointer that targets an offset PAST the end of the packet. Must be
    /// rejected (Err), not an OOB read.
    #[test]
    fn fuzz_dns_name_pointer_past_end_rejected() {
        let mut pkt = vec![0u8; 12];
        pkt[5] = 1;
        pkt.push(0xC0);
        pkt.push(0xFF); // pointer to offset 0x00FF, far past end
        pkt.extend_from_slice(&[0, 0, 0, 0]);
        let res = MdnsResponder::parse_dns_packet(&pkt);
        assert!(
            res.is_err(),
            "pointer past end must be rejected, not read OOB"
        );
    }

    /// Direct decode_dns_name fuzz starting at every offset of random buffers,
    /// including offsets >= len. Never panic.
    #[test]
    fn fuzz_decode_dns_name_arbitrary_offset_never_panics() {
        let mut rng = Rng::new(0xCAFE_F00D);
        for _ in 0..20_000 {
            let len = rng.below(80);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            // Offsets within and beyond the buffer.
            let off = rng.below(len + 4);
            let _ = MdnsResponder::decode_dns_name(&buf, off);
        }
    }

    /// Mutated-valid: take a real announcement and flip random bytes, ensuring
    /// the decoder stays robust on near-valid (most-confusing) inputs.
    #[test]
    fn fuzz_mutated_valid_announcement_never_panics() {
        let mut responder = MdnsResponder::new("seedhost");
        responder.register_service(MdnsService::new("Print", "_ipp._tcp", 631));
        let base = responder.build_announcement();
        let mut rng = Rng::new(0x5EED_1234);
        for _ in 0..20_000 {
            let mut m = base.clone();
            let flips = 1 + rng.below(8);
            for _ in 0..flips {
                if m.is_empty() {
                    break;
                }
                let idx = rng.below(m.len());
                m[idx] = rng.byte();
            }
            let _ = MdnsResponder::parse_dns_packet(&m);
            let mut r = MdnsResponder::new("x");
            let _ = r.process_packet(&m);
        }
    }

    /// Degenerate: empty, single-byte, all-0xC0 (pointer soup), all-zero,
    /// max-count header with no body. Each must return without panic.
    #[test]
    fn fuzz_dns_degenerate_inputs() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![0xC0],
            vec![0xC0, 0xC0],
            vec![0u8; 12],
            // Header claiming 0xFFFF questions, no body → must Err quickly.
            {
                let mut h = vec![0u8; 12];
                h[4] = 0xFF;
                h[5] = 0xFF;
                h
            },
            // Header claiming 0xFFFF answers, no body.
            {
                let mut h = vec![0u8; 12];
                h[6] = 0xFF;
                h[7] = 0xFF;
                h
            },
            vec![0xC0; 64],
            vec![0xFF; 64],
        ];
        for c in &cases {
            let _ = MdnsResponder::parse_dns_packet(c);
            let mut r = MdnsResponder::new("d");
            let _ = r.process_packet(c);
        }
    }
}
