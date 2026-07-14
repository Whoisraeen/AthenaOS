//! USB descriptor walking for xHCI bring-up (configuration / HID endpoints).
//!
//! Layout per USB 2.0 / HID 1.11 — public specification, not Redox-derived.

/// Interrupt-IN endpoint discovered on a HID boot interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HidInterruptEndpoint {
    /// Index into [`DeviceSlot::transfer_rings`] / endpoint context array.
    pub ep_index: u8,
    pub ep_address: u8,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub max_packet_size: u16,
    pub interval: u8,
    /// HID boot interface protocol: 1 = keyboard, 2 = mouse, 0 = none/other.
    pub protocol: u8,
    /// bInterfaceSubClass: 1 = Boot Interface Subclass, 0 = none. A device that
    /// is NOT boot-subclass must be driven in REPORT protocol (decode via the
    /// report descriptor — see `raehid`), not the fixed boot layout.
    pub subclass: u8,
    /// wDescriptorLength from the HID class descriptor (0x21): the size of the
    /// REPORT descriptor (type 0x22) to fetch for report-protocol decoding. 0 if
    /// the HID descriptor was absent/unparsed.
    pub report_desc_len: u16,
}

const DESC_CONFIG: u8 = 2;
const DESC_INTERFACE: u8 = 4;
const DESC_ENDPOINT: u8 = 5;
/// HID class descriptor (USB HID 1.11 §6.2.1) — carries wDescriptorLength of the
/// report descriptor.
const DESC_HID: u8 = 0x21;
/// Report descriptor type, the bDescriptorType inside the HID descriptor.
const HID_DESC_TYPE_REPORT: u8 = 0x22;

const CLASS_HID: u8 = 0x03;
pub const HID_PROTO_KEYBOARD: u8 = 0x01;
pub const HID_PROTO_MOUSE: u8 = 0x02;

const EP_ATTR_INTERRUPT: u8 = 0x03;

/// Device-context [`EndpointContext`] array index for a USB endpoint address.
///
/// Maps USB endpoint address to `endpoints[]` index; add-flag `A{n}` = `1 << (index + 1)`.
///
/// The array index is `DCI - 1` (since `endpoints[0]` = EP0 = DCI 1).
/// EP0 control (DCI 1) → index `0` / A1. EP1 IN `0x81` (DCI 3) → index `2` / A3.
pub fn xhci_ep_index(addr: u8) -> u8 {
    let num = addr & 0x0f;
    let dir_in = (addr & 0x80) != 0;
    if num == 0 {
        return 0;
    }
    // DCI = 2*num + dir_in; array index = DCI - 1.
    2 * num - 1 + if dir_in { 1 } else { 0 }
}

/// Expected `InputControlContext.add_flags` bit for `xhci_ep_index(addr)`.
/// Used by the unit tests; the kernel sets add-flags via `add_endpoint`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn xhci_add_flag_bit(ep_index: u8) -> u32 {
    1 << (ep_index + 1)
}

/// Walk configuration-level descriptors; `f` receives `(bDescriptorType, slice)`.
pub fn walk_descriptors(data: &[u8], mut f: impl FnMut(u8, &[u8])) {
    let mut pos = 0usize;
    while pos + 2 <= data.len() {
        let len = data[pos] as usize;
        if len < 2 || pos + len > data.len() {
            break;
        }
        let typ = data[pos + 1];
        f(typ, &data[pos..pos + len]);
        pos += len;
    }
}

/// Find EVERY HID interrupt-IN endpoint in the configuration — one per HID
/// interface. Composite peripherals are common: a gaming "mouse" is frequently
/// a mouse interface PLUS a keyboard/media interface in one device, so binding
/// only the FIRST interface misses the pointer when interface 0 is the
/// media/keyboard one (the Athena Razer 1532:0098: interface 0 = keyboard, the
/// mouse on a later interface). Binding every interface fixes that. One endpoint
/// per interface (the first interrupt-IN); each carries its own
/// subclass/protocol/report-descriptor len.
pub fn find_all_hid_interfaces(config: &[u8]) -> alloc::vec::Vec<HidInterruptEndpoint> {
    let mut out: alloc::vec::Vec<HidInterruptEndpoint> = alloc::vec::Vec::new();
    if config.len() < 9 || config[1] != DESC_CONFIG {
        return out;
    }

    let mut in_hid = false;
    let mut pushed_this_iface = false;
    let mut iface_number = 0u8;
    let mut alt_setting = 0u8;
    let mut iface_protocol = 0u8;
    let mut iface_subclass = 0u8;
    let mut hid_report_len = 0u16;

    walk_descriptors(config, |typ, desc| match typ {
        DESC_INTERFACE if desc.len() >= 9 => {
            iface_number = desc[2];
            alt_setting = desc[3];
            let class = desc[5];
            iface_subclass = desc[6];
            iface_protocol = desc[7];
            hid_report_len = 0;
            in_hid = class == CLASS_HID;
            pushed_this_iface = false;
        }
        DESC_HID if in_hid && desc.len() >= 9 && desc[6] == HID_DESC_TYPE_REPORT => {
            hid_report_len = u16::from_le_bytes([desc[7], desc[8]]);
        }
        DESC_ENDPOINT if in_hid && !pushed_this_iface && desc.len() >= 7 => {
            let addr = desc[2];
            let attr = desc[3];
            if addr & 0x80 == 0 || attr & EP_ATTR_INTERRUPT != EP_ATTR_INTERRUPT {
                return;
            }
            let mps = u16::from_le_bytes([desc[4], desc[5]]);
            let ep_num = addr & 0x0F;
            if ep_num == 0 || ep_num > 30 {
                return;
            }
            out.push(HidInterruptEndpoint {
                ep_index: xhci_ep_index(addr),
                ep_address: addr,
                interface_number: iface_number,
                alternate_setting: alt_setting,
                max_packet_size: mps.max(8),
                interval: desc[6],
                protocol: iface_protocol,
                subclass: iface_subclass,
                report_desc_len: hid_report_len,
            });
            pushed_this_iface = true;
        }
        DESC_CONFIG => in_hid = false,
        _ => {}
    });

    out
}

// ─── USB Mass Storage Class (MSC BOT) interface + bulk endpoints ────────────

const CLASS_MSC: u8 = 0x08;
const SUBCLASS_SCSI: u8 = 0x06;
const PROTO_BOT: u8 = 0x50;
const EP_ATTR_BULK: u8 = 0x02;
/// SuperSpeed Endpoint Companion descriptor (USB 3.2 §9.6.7). Present on
/// SS+ devices, immediately following each endpoint descriptor. Carries
/// `bMaxBurst` (byte 2, 0-based) — the burst size USB2 endpoints don't have.
const DESC_SS_EP_COMPANION: u8 = 0x30;

/// The two bulk endpoints of an MSC Bulk-Only-Transport interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MscBulkEndpoints {
    pub interface_number: u8,
    pub config_value_hint: u8,
    pub in_ep_address: u8,
    /// `endpoints[]`/`transfer_rings[]` array index (DCI − 1).
    pub in_ep_index: u8,
    pub in_max_packet: u16,
    pub out_ep_address: u8,
    pub out_ep_index: u8,
    pub out_max_packet: u16,
    /// SuperSpeed Max Burst Size (0-based, from the SS EP Companion
    /// descriptor) for bulk-IN / bulk-OUT. 0 on USB2 (no companion) = a
    /// burst of one packet; >0 only on SS+ devices.
    pub in_max_burst: u8,
    pub out_max_burst: u8,
}

/// Find an MSC BOT interface (class 0x08 / subclass 0x06 / protocol 0x50) and
/// its bulk-IN + bulk-OUT endpoints. Returns `None` unless both are present.
pub fn find_msc_bulk(config: &[u8]) -> Option<MscBulkEndpoints> {
    if config.len() < 9 || config[1] != DESC_CONFIG {
        return None;
    }
    let config_value = *config.get(5).unwrap_or(&1);

    let mut in_msc = false;
    let mut iface_number = 0u8;
    let (mut in_addr, mut in_idx, mut in_mps) = (0u8, 0u8, 0u16);
    let (mut out_addr, mut out_idx, mut out_mps) = (0u8, 0u8, 0u16);
    let (mut have_in, mut have_out) = (false, false);
    let (mut in_burst, mut out_burst) = (0u8, 0u8);
    // Which bulk endpoint the next SS Endpoint Companion descriptor applies
    // to: Some(true)=bulk-IN, Some(false)=bulk-OUT, None=not after a bulk EP.
    let mut companion_target: Option<bool> = None;

    walk_descriptors(config, |typ, desc| match typ {
        DESC_INTERFACE if desc.len() >= 9 => {
            let class = desc[5];
            let subclass = desc[6];
            let proto = desc[7];
            in_msc = class == CLASS_MSC && subclass == SUBCLASS_SCSI && proto == PROTO_BOT;
            if in_msc {
                iface_number = desc[2];
            }
            companion_target = None;
        }
        DESC_ENDPOINT if in_msc && desc.len() >= 7 => {
            let addr = desc[2];
            let attr = desc[3];
            companion_target = None;
            if attr & 0x03 != EP_ATTR_BULK {
                return;
            }
            let mps = u16::from_le_bytes([desc[4], desc[5]]);
            let ep_num = addr & 0x0F;
            if ep_num == 0 || ep_num > 30 {
                return;
            }
            if addr & 0x80 != 0 {
                if !have_in {
                    in_addr = addr;
                    in_idx = xhci_ep_index(addr);
                    in_mps = mps.max(8);
                    have_in = true;
                    companion_target = Some(true);
                }
            } else if !have_out {
                out_addr = addr;
                out_idx = xhci_ep_index(addr);
                out_mps = mps.max(8);
                have_out = true;
                companion_target = Some(false);
            }
        }
        // SuperSpeed Endpoint Companion follows its endpoint descriptor and
        // carries bMaxBurst (byte 2, 0-based). USB2 has no such descriptor,
        // so burst stays 0 there — the SS-distinct enumeration path.
        DESC_SS_EP_COMPANION if desc.len() >= 3 => match companion_target.take() {
            Some(true) => in_burst = desc[2],
            Some(false) => out_burst = desc[2],
            None => {}
        },
        _ => {
            companion_target = None;
        }
    });

    if have_in && have_out {
        Some(MscBulkEndpoints {
            interface_number: iface_number,
            config_value_hint: config_value,
            in_ep_address: in_addr,
            in_ep_index: in_idx,
            in_max_packet: in_mps,
            out_ep_address: out_addr,
            out_ep_index: out_idx,
            out_max_packet: out_mps,
            in_max_burst: in_burst,
            out_max_burst: out_burst,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ep_index_maps_hid_interrupt_in() {
        // EP1-IN (0x81) → DCI 3 → array index 2 → add-flag A3.
        assert_eq!(xhci_ep_index(0x81), 2);
        assert_eq!(xhci_add_flag_bit(2), 1 << 3);
    }

    #[test]
    fn ep_index_maps_ep0_and_ep1() {
        assert_eq!(xhci_ep_index(0x00), 0);
        assert_eq!(xhci_ep_index(0x80), 0);
        // EP1-OUT (0x01) → DCI 2 → array index 1.
        assert_eq!(xhci_ep_index(0x01), 1);
    }

    #[test]
    fn find_all_hid_interfaces_handles_composite() {
        // Composite device: interface 0 = boot keyboard (EP 0x81), interface 1 =
        // mouse (EP 0x82) — the Razer-style layout where binding only the first
        // misses the pointer.
        #[rustfmt::skip]
        let config: &[u8] = &[
            // Configuration descriptor (9): 2 interfaces, total len 59.
            0x09, 0x02, 0x3B, 0x00, 0x02, 0x01, 0x00, 0x80, 0x32,
            // Interface 0: HID(3) boot(1) keyboard(1).
            0x09, 0x04, 0x00, 0x00, 0x01, 0x03, 0x01, 0x01, 0x00,
            // HID descriptor: report descriptor length 63.
            0x09, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 0x3F, 0x00,
            // Endpoint 0x81 IN, interrupt, mps 8, bInterval 10.
            0x07, 0x05, 0x81, 0x03, 0x08, 0x00, 0x0A,
            // Interface 1: HID(3) non-boot(0) mouse(2).
            0x09, 0x04, 0x01, 0x00, 0x01, 0x03, 0x00, 0x02, 0x00,
            // HID descriptor: report descriptor length 50.
            0x09, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 0x32, 0x00,
            // Endpoint 0x82 IN, interrupt, mps 8, bInterval 10.
            0x07, 0x05, 0x82, 0x03, 0x08, 0x00, 0x0A,
        ];
        let ifaces = find_all_hid_interfaces(config);
        assert_eq!(ifaces.len(), 2, "both HID interfaces must be found");

        assert_eq!(ifaces[0].interface_number, 0);
        assert_eq!(ifaces[0].protocol, HID_PROTO_KEYBOARD);
        assert_eq!(ifaces[0].subclass, 1);
        assert_eq!(ifaces[0].report_desc_len, 63);
        assert_eq!(ifaces[0].ep_index, xhci_ep_index(0x81));

        assert_eq!(ifaces[1].interface_number, 1);
        assert_eq!(ifaces[1].protocol, HID_PROTO_MOUSE);
        assert_eq!(ifaces[1].subclass, 0);
        assert_eq!(ifaces[1].report_desc_len, 50);
        assert_eq!(ifaces[1].ep_index, xhci_ep_index(0x82));
    }
}
