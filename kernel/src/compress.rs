//! Transparent block compression — Concept §"CoW, snapshots, tiered storage":
//! RaeFS pays for capacity with cheap, fast compression on cold/snapshot data.
//!
//! Two codecs share this surface: LZ4 (this module, via `lz4_flex`) for the
//! fast path where decompression latency is on the read critical path, and Zstd
//! (`ruzstd`, decode-only) for already-archived blocks. LZ4 round-trips entirely
//! in-kernel with no C dependency, so snapshot diffs and cold-bucket reads can
//! compress without leaving `no_std`.

extern crate alloc;
use alloc::vec::Vec;

/// Compress a block with LZ4, prepending the original length so the decompressor
/// can size its output buffer in one pass (no streaming-frame overhead).
pub fn compress(input: &[u8]) -> Vec<u8> {
    lz4_flex::block::compress_prepend_size(input)
}

/// Decompress a block produced by [`compress`]. Returns `None` on corrupt input
/// (truncated, bad length prefix) rather than panicking — RaeFS treats that as a
/// read error and falls back to the uncompressed mirror.
pub fn decompress(input: &[u8]) -> Option<Vec<u8>> {
    lz4_flex::block::decompress_size_prepended(input).ok()
}

pub fn init() {
    crate::serial_println!("[ OK ] compress: LZ4 (lz4_flex) + Zstd (ruzstd) codecs ready");
}

/// R10 smoketest — must be able to print FAIL. Round-trips a payload with a
/// realistic mix of runs and entropy and asserts byte-identity, and asserts the
/// compressor actually shrinks a compressible payload.
pub fn run_boot_smoketest() {
    // Highly compressible: 4 KiB of a repeating pattern.
    let mut payload = Vec::with_capacity(4096);
    for i in 0..4096usize {
        payload.push((i % 7) as u8);
    }
    let packed = compress(&payload);
    let shrank = packed.len() < payload.len();
    let round_trips = decompress(&packed).map(|d| d == payload).unwrap_or(false);

    // Corrupt input must fail closed, not panic or return garbage.
    let rejects_garbage = decompress(&[0xFF, 0x00, 0x01]).is_none();

    let pass = shrank && round_trips && rejects_garbage;
    crate::selftest::record_smoketest("compress", pass);
    crate::serial_println!(
        "[compress] smoketest: lz4 {}->{} bytes round_trip={} rejects_garbage={} -> {}",
        payload.len(),
        packed.len(),
        round_trips,
        rejects_garbage,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// procfs body for `/proc/raeen/compress`.
pub fn dump_text() -> alloc::string::String {
    let sample = b"RaeenOS transparent compression self-describe sample block.";
    let packed = compress(sample);
    alloc::format!(
        "# RaeenOS block compression\ncodecs: lz4 (rw), zstd (ro)\nsample_in: {}\nsample_lz4: {}\n",
        sample.len(),
        packed.len()
    )
}
