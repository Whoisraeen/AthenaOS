//! Byte-exact generator for the hand-authored QEMU-virt DTB test fixture
//! (`tests/virt.dtb`). Run it on the host to regenerate the fixture:
//!
//! ```text
//! cargo run --example gen_fixture -p aarch64_logic
//! ```
//!
//! It is an `examples/` source-of-truth that regenerates `tests/virt.dtb`; the
//! actual KATs live in `src/dtb.rs` and `include_bytes!` the produced
//! `virt.dtb`. It encodes a minimal, spec-valid Devicetree (DTB v17,
//! big-endian) carrying the documented QEMU `-M virt` values: `-smp 4`, RAM at
//! 0x4000_0000 (512 MiB), PL011 UART at 0x0900_0000, GICv2 distributor at
//! 0x0800_0000 + CPU interface at 0x0801_0000.
//!
//! qemu-system-aarch64 was not installed on the authoring host, so this is the
//! honest substitute for a `-machine dumpdtb=` capture; replace with a real
//! capture when QEMU is available (the assertions are identical).

fn main() {
    let dtb = build_virt_dtb();
    std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/virt.dtb"), &dtb)
        .expect("write virt.dtb");
    eprintln!("wrote {} bytes", dtb.len());
}

// ---- FDT tokens (Devicetree Spec §5) ----
const FDT_BEGIN_NODE: u32 = 0x0000_0001;
const FDT_END_NODE: u32 = 0x0000_0002;
const FDT_PROP: u32 = 0x0000_0003;
const FDT_END: u32 = 0x0000_0009;
const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_VERSION: u32 = 17;
const FDT_LAST_COMP_VERSION: u32 = 16;

struct Strings {
    buf: Vec<u8>,
}
impl Strings {
    fn new() -> Self {
        Strings { buf: Vec::new() }
    }
    /// Intern a property name, returning its offset in the strings block.
    fn intern(&mut self, s: &str) -> u32 {
        // linear search for an existing identical string
        let needle = s.as_bytes();
        let mut i = 0;
        while i < self.buf.len() {
            let start = i;
            while self.buf[i] != 0 {
                i += 1;
            }
            if &self.buf[start..i] == needle {
                return start as u32;
            }
            i += 1; // skip NUL
        }
        let off = self.buf.len() as u32;
        self.buf.extend_from_slice(needle);
        self.buf.push(0);
        off
    }
}

struct Struct {
    buf: Vec<u8>,
}
impl Struct {
    fn new() -> Self {
        Struct { buf: Vec::new() }
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn align(&mut self) {
        while self.buf.len() % 4 != 0 {
            self.buf.push(0);
        }
    }
    fn begin_node(&mut self, name: &str) {
        self.u32(FDT_BEGIN_NODE);
        self.buf.extend_from_slice(name.as_bytes());
        self.buf.push(0);
        self.align();
    }
    fn end_node(&mut self) {
        self.u32(FDT_END_NODE);
    }
    fn prop(&mut self, strings: &mut Strings, name: &str, value: &[u8]) {
        self.u32(FDT_PROP);
        self.u32(value.len() as u32);
        self.u32(strings.intern(name));
        self.buf.extend_from_slice(value);
        self.align();
    }
    fn prop_u32(&mut self, strings: &mut Strings, name: &str, v: u32) {
        self.prop(strings, name, &v.to_be_bytes());
    }
    fn prop_str(&mut self, strings: &mut Strings, name: &str, s: &str) {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        self.prop(strings, name, &v);
    }
}

fn be64(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

fn build_virt_dtb() -> Vec<u8> {
    let mut strings = Strings::new();
    let mut s = Struct::new();

    // root node
    s.begin_node("");
    s.prop_u32(&mut strings, "#address-cells", 2);
    s.prop_u32(&mut strings, "#size-cells", 2);
    s.prop_str(&mut strings, "compatible", "linux,dummy-virt");
    s.prop_str(&mut strings, "model", "linux,dummy-virt");

    // /memory@40000000 : reg = <0x0 0x40000000  0x0 0x20000000> (512 MiB)
    {
        s.begin_node("memory@40000000");
        s.prop_str(&mut strings, "device_type", "memory");
        let mut reg = Vec::new();
        reg.extend_from_slice(&be64(0x4000_0000)); // address (2 cells)
        reg.extend_from_slice(&be64(0x2000_0000)); // size 512 MiB (2 cells)
        s.prop(&mut strings, "reg", &reg);
        s.end_node();
    }

    // /cpus : #address-cells=1 #size-cells=0, four cpu@N nodes
    {
        s.begin_node("cpus");
        s.prop_u32(&mut strings, "#address-cells", 1);
        s.prop_u32(&mut strings, "#size-cells", 0);
        for (i, name) in ["cpu@0", "cpu@1", "cpu@2", "cpu@3"].iter().enumerate() {
            s.begin_node(name);
            s.prop_str(&mut strings, "device_type", "cpu");
            s.prop_str(&mut strings, "compatible", "arm,cortex-a72");
            s.prop_u32(&mut strings, "reg", i as u32);
            s.prop_str(&mut strings, "enable-method", "psci");
            s.end_node();
        }
        s.end_node();
    }

    // /pl011@9000000 : compatible "arm,pl011","arm,primecell", reg = base 0x9000000 size 0x1000
    {
        s.begin_node("pl011@9000000");
        // compatible is a NUL-separated string list
        let mut compat = Vec::new();
        compat.extend_from_slice(b"arm,pl011\0");
        compat.extend_from_slice(b"arm,primecell\0");
        s.prop(&mut strings, "compatible", &compat);
        let mut reg = Vec::new();
        reg.extend_from_slice(&be64(0x0900_0000));
        reg.extend_from_slice(&be64(0x0000_1000));
        s.prop(&mut strings, "reg", &reg);
        s.end_node();
    }

    // /intc@8000000 : GICv2, reg has TWO regions: distributor then CPU iface.
    {
        s.begin_node("intc@8000000");
        s.prop_str(&mut strings, "compatible", "arm,cortex-a15-gic");
        s.prop_u32(&mut strings, "#interrupt-cells", 3);
        s.prop(&mut strings, "interrupt-controller", &[]); // empty/bool prop
        let mut reg = Vec::new();
        reg.extend_from_slice(&be64(0x0800_0000)); // dist base
        reg.extend_from_slice(&be64(0x0001_0000)); // dist size
        reg.extend_from_slice(&be64(0x0801_0000)); // cpu iface base
        reg.extend_from_slice(&be64(0x0001_0000)); // cpu iface size
        s.prop(&mut strings, "reg", &reg);
        s.end_node();
    }

    // /psci
    {
        s.begin_node("psci");
        s.prop_str(&mut strings, "compatible", "arm,psci-0.2");
        s.prop_str(&mut strings, "method", "hvc");
        s.end_node();
    }

    s.end_node(); // close root
    s.u32(FDT_END);

    // ---- assemble the blob ----
    // Layout: header (40 bytes) | mem reservation block | struct block | strings block.
    let header_len = 40usize;
    let memrsv_len = 16usize; // one terminating entry of (0,0)

    let off_memrsv = header_len;
    let off_struct = off_memrsv + memrsv_len;
    let off_strings = off_struct + s.buf.len();
    let total = off_strings + strings.buf.len();

    let mut out = Vec::with_capacity(total);
    // header (all big-endian u32)
    let push = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_be_bytes());
    push(&mut out, FDT_MAGIC);
    push(&mut out, total as u32); // totalsize
    push(&mut out, off_struct as u32); // off_dt_struct
    push(&mut out, off_strings as u32); // off_dt_strings
    push(&mut out, off_memrsv as u32); // off_mem_rsvmap
    push(&mut out, FDT_VERSION);
    push(&mut out, FDT_LAST_COMP_VERSION);
    push(&mut out, 0); // boot_cpuid_phys
    push(&mut out, strings.buf.len() as u32); // size_dt_strings
    push(&mut out, s.buf.len() as u32); // size_dt_struct

    // memory reservation block: single terminating (address=0, size=0) entry
    out.extend_from_slice(&be64(0));
    out.extend_from_slice(&be64(0));

    // struct + strings
    out.extend_from_slice(&s.buf);
    out.extend_from_slice(&strings.buf);

    out
}
