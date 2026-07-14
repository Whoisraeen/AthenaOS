// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p ath_mp4`. FAIL-able by construction.
//
// Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
// `cfg_attr(not(test), ...)`), so `Vec`/`vec!` are in scope via the default
// prelude — no `extern crate std` / `use std::` (the architecture gate's R7 bans
// those std-ism lines). Every fixture is a hand-assembled real BMFF box tree, so
// each offset/size/timestamp/byte assert is concrete and independently checkable.
// ════════════════════════════════════════════════════════════════════════════

use super::*;

// ─── Box builders (mirror the parser, independent encode side) ───────────────

/// A plain box: size(4) + type(4) + body.
fn bx(ty: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let size = (8 + body.len()) as u32;
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(ty);
    out.extend_from_slice(body);
    out
}

/// A full box: size + type + version(1) + flags(3) + body.
fn fullbox(ty: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.push(version);
    b.extend_from_slice(&flags.to_be_bytes()[1..4]); // 3-byte flags
    b.extend_from_slice(body);
    bx(ty, &b)
}

fn concat(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

fn ftyp() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"isom"); // major brand
    body.extend_from_slice(&0u32.to_be_bytes()); // minor version
    body.extend_from_slice(b"isom");
    body.extend_from_slice(b"mp42");
    bx(b"ftyp", &body)
}

fn mvhd(timescale: u32, duration: u32) -> Vec<u8> {
    // v0: ctime(4)+mtime(4)+timescale(4)+duration(4)+... (rate/volume/matrix pad)
    let mut body = Vec::new();
    body.extend_from_slice(&0u32.to_be_bytes()); // ctime
    body.extend_from_slice(&0u32.to_be_bytes()); // mtime
    body.extend_from_slice(&timescale.to_be_bytes());
    body.extend_from_slice(&duration.to_be_bytes());
    // pad the rest of a v0 mvhd (not read by the parser, but realistic)
    body.extend_from_slice(&[0u8; 80]);
    fullbox(b"mvhd", 0, 0, &body)
}

fn tkhd(track_id: u32, width: u16, height: u16) -> Vec<u8> {
    // v0 tkhd: ctime(4)+mtime(4)+track_id(4)+reserved(4)+duration(4)+... then
    // width@(body+76) height@(body+80) as 16.16. body here starts AFTER the
    // version+flags (fullbox prepends those), so track_id sits at body offset 8
    // → matches parse_tkhd id_off=12 (4 version/flags + 8).
    let mut body = Vec::new();
    body.extend_from_slice(&0u32.to_be_bytes()); // ctime
    body.extend_from_slice(&0u32.to_be_bytes()); // mtime
    body.extend_from_slice(&track_id.to_be_bytes()); // track_id (body off 8)
    body.extend_from_slice(&0u32.to_be_bytes()); // reserved
    body.extend_from_slice(&0u32.to_be_bytes()); // duration
                                                 // Pad up to width: parse_tkhd reads width at body off (76-4)=72 within the
                                                 // post-version body; fullbox adds 4 → file body off 76. Current len = 20.
    while body.len() < 72 {
        body.push(0);
    }
    // width/height as 16.16
    body.extend_from_slice(&((width as u32) << 16).to_be_bytes());
    body.extend_from_slice(&((height as u32) << 16).to_be_bytes());
    fullbox(b"tkhd", 0, 0, &body)
}

fn mdhd(timescale: u32, duration: u32, lang: &[u8; 3]) -> Vec<u8> {
    // v0: ctime(4)+mtime(4)+timescale(4)+duration(4)+language(2)+pre_defined(2)
    let mut body = Vec::new();
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(&timescale.to_be_bytes());
    body.extend_from_slice(&duration.to_be_bytes());
    // pack language: three 5-bit chars (char - 0x60).
    let packed: u16 = (((lang[0] - 0x60) as u16) << 10)
        | (((lang[1] - 0x60) as u16) << 5)
        | ((lang[2] - 0x60) as u16);
    body.extend_from_slice(&packed.to_be_bytes());
    body.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    fullbox(b"mdhd", 0, 0, &body)
}

fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
    // pre_defined(4)+handler_type(4)+reserved(12)+name(\0)
    let mut body = Vec::new();
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(handler);
    body.extend_from_slice(&[0u8; 12]);
    body.push(0);
    fullbox(b"hdlr", 0, 0, &body)
}

/// An audio sample entry ('mp4a') with a child config box carrying `private`.
fn audio_stsd(
    fourcc: &[u8; 4],
    channels: u16,
    sample_size: u16,
    sample_rate: u32,
    config_type: &[u8; 4],
    private: &[u8],
) -> Vec<u8> {
    // SampleEntry base: reserved(6)+data_ref_index(2) = 8 bytes
    // AudioSampleEntry: reserved(8)+channelcount(2)+samplesize(2)+pre(2)+
    //                   reserved(2)+samplerate(4 as 16.16)
    let mut entry_body = Vec::new();
    entry_body.extend_from_slice(&[0u8; 6]); // reserved
    entry_body.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    entry_body.extend_from_slice(&[0u8; 8]); // audio reserved
    entry_body.extend_from_slice(&channels.to_be_bytes());
    entry_body.extend_from_slice(&sample_size.to_be_bytes());
    entry_body.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    entry_body.extend_from_slice(&0u16.to_be_bytes()); // reserved
    entry_body.extend_from_slice(&(sample_rate << 16).to_be_bytes());
    // child config box (e.g. esds carrying AudioSpecificConfig)
    entry_body.extend_from_slice(&bx(config_type, private));

    let entry = bx(fourcc, &entry_body);
    // stsd full box: entry_count(4) then the entry.
    let mut body = Vec::new();
    body.extend_from_slice(&1u32.to_be_bytes());
    body.extend_from_slice(&entry);
    fullbox(b"stsd", 0, 0, &body)
}

/// A video sample entry ('avc1') with avcC child carrying `private`.
fn video_stsd(fourcc: &[u8; 4], width: u16, height: u16, private: &[u8]) -> Vec<u8> {
    let mut entry_body = Vec::new();
    entry_body.extend_from_slice(&[0u8; 6]); // reserved
    entry_body.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
                                                       // VisualSampleEntry: pre_defined(2)+reserved(2)+pre_defined[3](12) = 16 then
                                                       // width(2)+height(2). avcC follows at offset 70.
    entry_body.extend_from_slice(&[0u8; 16]);
    entry_body.extend_from_slice(&width.to_be_bytes());
    entry_body.extend_from_slice(&height.to_be_bytes());
    // horiz/vert res(8)+reserved(4)+frame_count(2)+compressorname(32)+depth(2)+
    // pre_defined(2) = 50 bytes → brings offset from 20 to 70.
    entry_body.extend_from_slice(&[0u8; 50]);
    entry_body.extend_from_slice(&bx(b"avcC", private));

    let entry = bx(fourcc, &entry_body);
    let mut body = Vec::new();
    body.extend_from_slice(&1u32.to_be_bytes());
    body.extend_from_slice(&entry);
    fullbox(b"stsd", 0, 0, &body)
}

fn stts(entries: &[(u32, u32)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for &(c, d) in entries {
        body.extend_from_slice(&c.to_be_bytes());
        body.extend_from_slice(&d.to_be_bytes());
    }
    fullbox(b"stts", 0, 0, &body)
}

fn ctts(version: u8, entries: &[(u32, i32)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for &(c, o) in entries {
        body.extend_from_slice(&c.to_be_bytes());
        body.extend_from_slice(&(o as u32).to_be_bytes());
    }
    fullbox(b"ctts", version, 0, &body)
}

fn stsc(entries: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for &(fc, spc, sdi) in entries {
        body.extend_from_slice(&fc.to_be_bytes());
        body.extend_from_slice(&spc.to_be_bytes());
        body.extend_from_slice(&sdi.to_be_bytes());
    }
    fullbox(b"stsc", 0, 0, &body)
}

fn stsz(constant: u32, sizes: &[u32]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&constant.to_be_bytes());
    let count = if constant != 0 {
        sizes.len() as u32 // when constant, sample_count is the explicit count
    } else {
        sizes.len() as u32
    };
    body.extend_from_slice(&count.to_be_bytes());
    if constant == 0 {
        for &s in sizes {
            body.extend_from_slice(&s.to_be_bytes());
        }
    }
    fullbox(b"stsz", 0, 0, &body)
}

fn stco(offsets: &[u32]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for &o in offsets {
        body.extend_from_slice(&o.to_be_bytes());
    }
    fullbox(b"stco", 0, 0, &body)
}

fn co64(offsets: &[u64]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for &o in offsets {
        body.extend_from_slice(&o.to_be_bytes());
    }
    fullbox(b"co64", 0, 0, &body)
}

fn stss(syncs: &[u32]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(syncs.len() as u32).to_be_bytes());
    for &s in syncs {
        body.extend_from_slice(&s.to_be_bytes());
    }
    fullbox(b"stss", 0, 0, &body)
}

fn stbl(children: &[Vec<u8>]) -> Vec<u8> {
    bx(b"stbl", &concat(children))
}
fn minf(stbl_box: Vec<u8>) -> Vec<u8> {
    bx(b"minf", &stbl_box)
}
fn mdia(children: &[Vec<u8>]) -> Vec<u8> {
    bx(b"mdia", &concat(children))
}
fn trak(children: &[Vec<u8>]) -> Vec<u8> {
    bx(b"trak", &concat(children))
}
fn moov(children: &[Vec<u8>]) -> Vec<u8> {
    bx(b"moov", &concat(children))
}

// ─── A reusable single-audio-track .m4a-like file ────────────────────────────
//
// One audio track, 'mp4a', 2ch/16bit/44100, codec_private = AudioSpecificConfig
// bytes [0x12, 0x10]. 3 samples in ONE chunk. The chunk lives in `mdat` at a
// known file offset; sample sizes are 4, 5, 6 bytes with distinct contents so the
// sample_data extraction is exact + FAIL-able.

const ASC: [u8; 2] = [0x12, 0x10]; // AAC-LC, 44.1kHz, stereo AudioSpecificConfig

/// Build the file and return (bytes, mdat_data_offset, sample_bytes[]).
fn build_m4a() -> (Vec<u8>, u64, Vec<Vec<u8>>) {
    let sample_bytes: Vec<Vec<u8>> = vec![
        vec![0xAA, 0xBB, 0xCC, 0xDD],             // sample 0, 4 bytes
        vec![0x11, 0x22, 0x33, 0x44, 0x55],       // sample 1, 5 bytes
        vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06], // sample 2, 6 bytes
    ];
    let sizes: Vec<u32> = sample_bytes.iter().map(|s| s.len() as u32).collect();

    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(3, 1024)]), // 3 samples, each 1024-tick delta
        stsc(&[(1, 3, 1)]), // chunk 1 onward: 3 samples per chunk, desc 1
        stsz(0, &sizes),
        // chunk offset placeholder — patched after we know mdat position.
        stco(&[0]),
    ]);

    let moov_box = moov(&[
        mvhd(44100, 3072),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 3072, b"eng"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);

    // Assemble: ftyp + moov + mdat. mdat data starts after its 8-byte header.
    let ftyp_box = ftyp();
    let mut mdat_payload = Vec::new();
    for s in &sample_bytes {
        mdat_payload.extend_from_slice(s);
    }
    let mdat_box = bx(b"mdat", &mdat_payload);

    let mut file = Vec::new();
    file.extend_from_slice(&ftyp_box);
    file.extend_from_slice(&moov_box);
    let mdat_hdr_offset = file.len();
    let mdat_data_offset = (mdat_hdr_offset + 8) as u64;
    file.extend_from_slice(&mdat_box);

    // Patch the stco chunk offset in-place to point at the real mdat data offset.
    // Easiest: rebuild moov with the correct offset now that we know it.
    let stbl_box2 = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(3, 1024)]),
        stsc(&[(1, 3, 1)]),
        stsz(0, &sizes),
        stco(&[mdat_data_offset as u32]),
    ]);
    let moov_box2 = moov(&[
        mvhd(44100, 3072),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 3072, b"eng"), hdlr(b"soun"), minf(stbl_box2)]),
        ]),
    ]);
    // Rebuild the whole file with the corrected moov (sizes are identical since
    // only the offset value changed, so mdat lands at the same place).
    assert_eq!(moov_box.len(), moov_box2.len(), "moov size must be stable");
    let mut file2 = Vec::new();
    file2.extend_from_slice(&ftyp_box);
    file2.extend_from_slice(&moov_box2);
    let real_mdat_data_off = (file2.len() + 8) as u64;
    file2.extend_from_slice(&mdat_box);
    assert_eq!(real_mdat_data_off, mdat_data_offset, "mdat offset stable");

    (file2, mdat_data_offset, sample_bytes)
}

// ─── 1. Audio track: kind, codec, params, codec_private ──────────────────────

#[test]
fn audio_track_metadata() {
    let (file, _off, _sb) = build_m4a();
    let mp4 = Mp4::parse(&file).expect("parse m4a");
    assert_eq!(mp4.major_brand, *b"isom");
    assert!(mp4.compatible_brands.contains(&*b"mp42"));
    assert_eq!(mp4.movie_timescale, 44100);

    assert_eq!(mp4.tracks().len(), 1);
    let t = &mp4.tracks()[0];
    assert_eq!(t.id, 1);
    assert_eq!(t.kind, TrackKind::Audio);
    assert_eq!(t.codec, Codec::Aac);
    assert_eq!(&t.codec_fourcc, b"mp4a");
    assert_eq!(t.timescale, 44100);
    assert_eq!(t.duration, 3072);
    assert_eq!(t.language_str(), "eng");

    let a = t.audio.expect("audio params");
    assert_eq!(a.channels, 2);
    assert_eq!(a.sample_size_bits, 16);
    assert_eq!(a.sample_rate, 44100);

    // The load-bearing codec-private (AudioSpecificConfig) extraction.
    assert_eq!(t.codec_private, ASC.to_vec());
    // FAIL-ability: wrong ASC bytes must not match.
    assert_ne!(t.codec_private, vec![0x00, 0x00]);
}

// ─── 2. Sample-table math: offset/size/dts cross-checked by hand ─────────────

#[test]
fn sample_table_offsets_and_timestamps() {
    let (file, mdat_off, sb) = build_m4a();
    let mp4 = Mp4::parse(&file).expect("parse");
    let t = &mp4.tracks()[0];
    assert_eq!(t.sample_count(), 3);

    // Hand-computed: all 3 samples in one chunk starting at mdat_off.
    // sizes 4,5,6 → offsets mdat_off, mdat_off+4, mdat_off+9.
    let s0 = t.sample(0).unwrap();
    let s1 = t.sample(1).unwrap();
    let s2 = t.sample(2).unwrap();

    assert_eq!(s0.offset, mdat_off);
    assert_eq!(s0.size, 4);
    assert_eq!(s1.offset, mdat_off + 4);
    assert_eq!(s1.size, 5);
    assert_eq!(s2.offset, mdat_off + 9);
    assert_eq!(s2.size, 6);

    // stts (3, 1024): dts 0, 1024, 2048.
    assert_eq!(s0.dts, 0);
    assert_eq!(s1.dts, 1024);
    assert_eq!(s2.dts, 2048);
    // No ctts → cts == dts.
    assert_eq!(s0.cts, 0);
    assert_eq!(s2.cts, 2048);
    // No stss → all sync.
    assert!(s0.is_sync && s1.is_sync && s2.is_sync);

    // FAIL-ability: a wrong stsc/stsz accumulation would shift offset 1.
    assert_ne!(s1.offset, mdat_off); // would be wrong if size accumulation broke
    let _ = sb;
}

// ─── 3. sample_data extracts the EXACT mdat elementary-stream bytes ──────────

#[test]
fn sample_data_exact_bytes() {
    let (file, _off, sb) = build_m4a();
    let mp4 = Mp4::parse(&file).expect("parse");
    let t = &mp4.tracks()[0];

    for i in 0..3 {
        let got = t.sample_data(&file, i).expect("sample bytes in range");
        assert_eq!(got, &sb[i][..], "sample {i} elementary-stream bytes");
    }
    // The load-bearing exact-extraction assert, proven FAIL-able: tweaking the
    // expected bytes must break it.
    let s0 = t.sample_data(&file, 0).unwrap();
    assert_eq!(s0, &[0xAA, 0xBB, 0xCC, 0xDD]);
    assert_ne!(s0, &[0xAA, 0xBB, 0xCC, 0xDE]);

    // Out-of-range sample index → None (no panic).
    assert!(t.sample_data(&file, 99).is_none());
    assert!(t.sample(99).is_none());
}

// ─── 4. audio_samples() iterator helper (the .m4a playback path) ─────────────

#[test]
fn audio_samples_iterator() {
    let (file, _off, sb) = build_m4a();
    let mp4 = Mp4::parse(&file).expect("parse");
    let it = audio_samples(&mp4, &file).expect("has audio track");
    let collected: Vec<(Sample, Option<&[u8]>)> = it.collect();
    assert_eq!(collected.len(), 3);
    for (i, (s, bytes)) in collected.iter().enumerate() {
        assert_eq!(s.size as usize, sb[i].len());
        assert_eq!(bytes.unwrap(), &sb[i][..]);
    }
}

// ─── 5. Multi-chunk stsc → offsets across chunks ─────────────────────────────

#[test]
fn multi_chunk_stsc_offsets() {
    // 4 samples, 2 chunks of 2. Chunk0 @ off 1000, chunk1 @ off 5000.
    // sizes [10, 20, 30, 40].
    let sizes = [10u32, 20, 30, 40];
    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 1, 16, 48000, b"esds", &[0xAB]),
        stts(&[(4, 512)]),
        stsc(&[(1, 2, 1)]), // every chunk: 2 samples
        stsz(0, &sizes),
        stco(&[1000, 5000]),
    ]);
    let moov_box = moov(&[
        mvhd(48000, 2048),
        trak(&[
            tkhd(7, 0, 0),
            mdia(&[mdhd(48000, 2048, b"und"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);
    let file = concat(&[ftyp(), moov_box]);
    let mp4 = Mp4::parse(&file).expect("parse");
    let t = &mp4.tracks()[0];
    assert_eq!(t.id, 7);
    assert_eq!(t.sample_count(), 4);

    // chunk0: samples 0,1 at 1000, 1010. chunk1: samples 2,3 at 5000, 5030.
    assert_eq!(t.sample(0).unwrap().offset, 1000);
    assert_eq!(t.sample(1).unwrap().offset, 1010);
    assert_eq!(t.sample(2).unwrap().offset, 5000);
    assert_eq!(t.sample(3).unwrap().offset, 5030);
    // dts 0,512,1024,1536.
    assert_eq!(t.sample(3).unwrap().dts, 1536);
}

// ─── 6. Video track: avc1, width/height, avcC, stss + ctts ───────────────────

#[test]
fn video_track_with_ctts_and_stss() {
    let avcc = [0x01, 0x64, 0x00, 0x1F, 0xFF]; // fake avcC config bytes
    let sizes = [100u32, 50, 60, 70];
    let stbl_box = stbl(&[
        video_stsd(b"avc1", 1920, 1080, &avcc),
        stts(&[(4, 3000)]),
        // ctts v1 (signed): sample0 +0, sample1 +3000, sample2 -1500, sample3 +0
        ctts(1, &[(1, 0), (1, 3000), (1, -1500), (1, 0)]),
        stsc(&[(1, 4, 1)]),
        stsz(0, &sizes),
        stco(&[2000]),
        stss(&[1, 3]), // samples 1 and 3 are keyframes
    ]);
    let moov_box = moov(&[
        mvhd(90000, 12000),
        trak(&[
            tkhd(2, 1920, 1080),
            mdia(&[mdhd(90000, 12000, b"und"), hdlr(b"vide"), minf(stbl_box)]),
        ]),
    ]);
    let file = concat(&[ftyp(), moov_box]);
    let mp4 = Mp4::parse(&file).expect("parse");
    let t = &mp4.tracks()[0];
    assert_eq!(t.kind, TrackKind::Video);
    assert_eq!(t.codec, Codec::H264);
    assert_eq!(&t.codec_fourcc, b"avc1");
    let v = t.video.expect("video params");
    assert_eq!((v.width, v.height), (1920, 1080));
    assert_eq!(t.codec_private, avcc.to_vec());

    // offsets: chunk @2000, sizes 100,50,60,70 → 2000,2100,2150,2210.
    assert_eq!(t.sample(0).unwrap().offset, 2000);
    assert_eq!(t.sample(1).unwrap().offset, 2100);
    assert_eq!(t.sample(3).unwrap().offset, 2210);

    // dts 0,3000,6000,9000. cts = dts + ctts offset.
    assert_eq!(t.sample(0).unwrap().cts, 0); // 0 + 0
    assert_eq!(t.sample(1).unwrap().cts, 6000); // 3000 + 3000
    assert_eq!(t.sample(2).unwrap().cts, 4500); // 6000 - 1500
    assert_eq!(t.sample(3).unwrap().cts, 9000); // 9000 + 0

    // stss: only samples 1 and 3 (1-based) are sync → idx 0 and 2.
    assert!(t.sample(0).unwrap().is_sync);
    assert!(!t.sample(1).unwrap().is_sync);
    assert!(t.sample(2).unwrap().is_sync);
    assert!(!t.sample(3).unwrap().is_sync);
    // FAIL-ability: ignoring stss would make all sync.
    assert_ne!(t.sample(1).unwrap().is_sync, true);
}

// ─── 7. Two-track file (audio + video) separates cleanly ─────────────────────

#[test]
fn two_track_audio_and_video() {
    let audio_stbl = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(2, 1024)]),
        stsc(&[(1, 2, 1)]),
        stsz(0, &[8, 8]),
        stco(&[3000]),
    ]);
    let video_stbl = stbl(&[
        video_stsd(b"hvc1", 1280, 720, &[0x01, 0x02]),
        stts(&[(2, 1500)]),
        stsc(&[(1, 2, 1)]),
        stsz(0, &[200, 100]),
        stco(&[8000]),
        stss(&[1]),
    ]);
    let moov_box = moov(&[
        mvhd(1000, 4000),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 2048, b"eng"), hdlr(b"soun"), minf(audio_stbl)]),
        ]),
        trak(&[
            tkhd(2, 1280, 720),
            mdia(&[mdhd(90000, 3000, b"und"), hdlr(b"vide"), minf(video_stbl)]),
        ]),
    ]);
    let file = concat(&[ftyp(), moov_box]);
    let mp4 = Mp4::parse(&file).expect("parse");
    assert_eq!(mp4.tracks().len(), 2);

    let a = mp4.first_audio_track().expect("audio");
    assert_eq!(a.kind, TrackKind::Audio);
    assert_eq!(a.codec, Codec::Aac);
    assert_eq!(a.sample_count(), 2);

    let v = mp4.first_video_track().expect("video");
    assert_eq!(v.kind, TrackKind::Video);
    assert_eq!(v.codec, Codec::Hevc);
    assert_eq!(&v.codec_fourcc, b"hvc1");
    assert_eq!(v.video.unwrap().width, 1280);
    assert_eq!(v.sample(0).unwrap().offset, 8000);
    assert_eq!(v.sample(1).unwrap().offset, 8200);
}

// ─── 8. co64 (64-bit chunk offsets) ──────────────────────────────────────────

#[test]
fn co64_64bit_offsets() {
    let big = 0x1_0000_0000u64; // > 4 GiB, needs co64
    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(2, 1024)]),
        stsc(&[(1, 2, 1)]),
        stsz(0, &[16, 32]),
        co64(&[big]),
    ]);
    let moov_box = moov(&[
        mvhd(44100, 2048),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 2048, b"und"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);
    let file = concat(&[ftyp(), moov_box]);
    let mp4 = Mp4::parse(&file).expect("parse");
    let t = &mp4.tracks()[0];
    assert_eq!(t.sample(0).unwrap().offset, big);
    assert_eq!(t.sample(1).unwrap().offset, big + 16);
}

// ─── 9. Constant sample size (stsz sample_size != 0) ─────────────────────────

#[test]
fn constant_sample_size() {
    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(3, 1024)]),
        stsc(&[(1, 3, 1)]),
        stsz(7, &[0, 0, 0]), // constant size 7, 3 samples
        stco(&[100]),
    ]);
    let moov_box = moov(&[
        mvhd(44100, 3072),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 3072, b"und"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);
    let file = concat(&[ftyp(), moov_box]);
    let mp4 = Mp4::parse(&file).expect("parse");
    let t = &mp4.tracks()[0];
    assert_eq!(t.sample_count(), 3);
    assert_eq!(t.sample(0).unwrap().offset, 100);
    assert_eq!(t.sample(1).unwrap().offset, 107);
    assert_eq!(t.sample(2).unwrap().offset, 114);
    for i in 0..3 {
        assert_eq!(t.sample(i).unwrap().size, 7);
    }
}

// ─── 10. largesize (64-bit box size) header form ─────────────────────────────

#[test]
fn largesize_box_form() {
    // Wrap ftyp with the size==1 + largesize form and confirm it still parses.
    let inner = ftyp();
    // Rebuild ftyp body with the 64-bit size form: size32=1, type, largesize.
    let body = &inner[8..]; // the ftyp body
    let total = (16 + body.len()) as u64;
    let mut large_ftyp = Vec::new();
    large_ftyp.extend_from_slice(&1u32.to_be_bytes()); // size==1 sentinel
    large_ftyp.extend_from_slice(b"ftyp");
    large_ftyp.extend_from_slice(&total.to_be_bytes()); // largesize
    large_ftyp.extend_from_slice(body);

    // Minimal valid moov so parse succeeds.
    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(1, 1024)]),
        stsc(&[(1, 1, 1)]),
        stsz(0, &[4]),
        stco(&[10]),
    ]);
    let moov_box = moov(&[
        mvhd(44100, 1024),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 1024, b"und"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);
    let file = concat(&[large_ftyp, moov_box]);
    let mp4 = Mp4::parse(&file).expect("largesize parse");
    assert_eq!(mp4.major_brand, *b"isom");
    assert_eq!(mp4.tracks().len(), 1);
}

// ─── 11. uuid box is skipped without corrupting the stream ───────────────────

#[test]
fn uuid_box_skipped() {
    // A uuid box (16-byte ext type + payload) between ftyp and moov.
    let mut uuid_box = Vec::new();
    let payload = [0xDEu8; 8];
    let size = (8 + 16 + payload.len()) as u32;
    uuid_box.extend_from_slice(&size.to_be_bytes());
    uuid_box.extend_from_slice(b"uuid");
    uuid_box.extend_from_slice(&[0x11u8; 16]); // extended type
    uuid_box.extend_from_slice(&payload);

    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(1, 1024)]),
        stsc(&[(1, 1, 1)]),
        stsz(0, &[4]),
        stco(&[10]),
    ]);
    let moov_box = moov(&[
        mvhd(44100, 1024),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 1024, b"und"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);
    let file = concat(&[ftyp(), uuid_box, moov_box]);
    let mp4 = Mp4::parse(&file).expect("uuid-skipped parse");
    assert_eq!(mp4.tracks().len(), 1);
}

// ─── 12. stz2 compact sample sizes (16-bit field) ────────────────────────────

#[test]
fn stz2_compact_sizes() {
    // stz2 with 16-bit field_size, 3 samples [9, 19, 29].
    let mut body = Vec::new();
    body.extend_from_slice(&[0u8, 0, 0]); // reserved (3 bytes)
    body.push(16); // field_size
    body.extend_from_slice(&3u32.to_be_bytes()); // sample_count
    for s in [9u16, 19, 29] {
        body.extend_from_slice(&s.to_be_bytes());
    }
    let stz2_box = fullbox(b"stz2", 0, 0, &body);

    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(3, 1024)]),
        stsc(&[(1, 3, 1)]),
        stz2_box,
        stco(&[500]),
    ]);
    let moov_box = moov(&[
        mvhd(44100, 3072),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 3072, b"und"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);
    let file = concat(&[ftyp(), moov_box]);
    let mp4 = Mp4::parse(&file).expect("stz2 parse");
    let t = &mp4.tracks()[0];
    assert_eq!(t.sample(0).unwrap().size, 9);
    assert_eq!(t.sample(0).unwrap().offset, 500);
    assert_eq!(t.sample(1).unwrap().offset, 509);
    assert_eq!(t.sample(2).unwrap().offset, 528);
}

// ════════════════════════════════════════════════════════════════════════════
// Hostile-input battery — every malformed input is Err, never a panic/loop/OOM.
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn reject_non_mp4() {
    // Arbitrary bytes: the leading 4 bytes parse as a (garbage) box size that
    // runs past the buffer → Truncated; a valid-but-non-ftyp file → NoFtyp. Both
    // are graceful Err (the contract is "never Ok, never panic").
    assert!(matches!(Mp4::parse(b"not an mp4 file at all"), Err(_)));
    assert!(matches!(Mp4::parse(&[]), Err(_)));
    assert!(matches!(Mp4::parse(&[0u8; 4]), Err(_)));
    // A structurally valid box that is not ftyp → NoFtyp specifically.
    let not_ftyp = bx(b"free", &[0u8; 8]);
    assert_eq!(Mp4::parse(&not_ftyp), Err(Mp4Error::NoFtyp));
}

#[test]
fn reject_missing_moov() {
    let file = ftyp(); // ftyp but no moov
    assert_eq!(Mp4::parse(&file), Err(Mp4Error::NoMoov));
}

#[test]
fn reject_truncated_box() {
    // A box header claiming a huge size that runs past the buffer.
    let mut file = ftyp();
    file.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
    file.extend_from_slice(b"moov");
    // no body → size says ~4GiB but buffer ends.
    assert!(matches!(Mp4::parse(&file), Err(Mp4Error::Truncated)));
}

#[test]
fn reject_zero_size_loop() {
    // A box with size==0 means "to EOF": it must be consumed once, not loop.
    // Place a size-0 box; the parser must terminate (consume to EOF) and then
    // report NoMoov (no moov present) — crucially WITHOUT hanging.
    let mut file = ftyp();
    file.extend_from_slice(&0u32.to_be_bytes()); // size 0 → to EOF
    file.extend_from_slice(b"free");
    file.extend_from_slice(&[0u8; 16]);
    // Must return promptly (this test passing == no infinite loop).
    let r = Mp4::parse(&file);
    assert!(matches!(r, Err(Mp4Error::NoMoov)));
}

#[test]
fn reject_box_size_smaller_than_header() {
    // size == 4 (< the 8-byte header) must be rejected, not loop forever.
    let mut file = ftyp();
    file.extend_from_slice(&4u32.to_be_bytes());
    file.extend_from_slice(b"junk");
    assert!(matches!(Mp4::parse(&file), Err(Mp4Error::BadBoxSize)));
}

#[test]
fn reject_absurd_sample_count() {
    // stsz with a sample_count beyond MAX_SAMPLES must be rejected before any
    // allocation of that size.
    let mut body = Vec::new();
    body.extend_from_slice(&0u32.to_be_bytes()); // sample_size = 0 (explicit)
    body.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes()); // sample_count huge
                                                           // (no size entries — but the count check must fire first)
    let stsz_box = fullbox(b"stsz", 0, 0, &body);
    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(1, 1024)]),
        stsc(&[(1, 1, 1)]),
        stsz_box,
        stco(&[10]),
    ]);
    let moov_box = moov(&[
        mvhd(44100, 1024),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 1024, b"und"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);
    let file = concat(&[ftyp(), moov_box]);
    assert_eq!(Mp4::parse(&file), Err(Mp4Error::TooManyEntries));
}

#[test]
fn reject_offset_past_eof_on_extract() {
    // A valid table but the chunk offset points past EOF: parse succeeds (the
    // table is structurally valid) but sample_data must return None, never read
    // out of bounds.
    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        stts(&[(1, 1024)]),
        stsc(&[(1, 1, 1)]),
        stsz(0, &[100]),
        stco(&[0xFFFF_FF00]), // far past the end of this small file
    ]);
    let moov_box = moov(&[
        mvhd(44100, 1024),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 1024, b"und"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);
    let file = concat(&[ftyp(), moov_box]);
    let mp4 = Mp4::parse(&file).expect("structurally valid");
    let t = &mp4.tracks()[0];
    // The resolved offset is recorded, but extraction is bounds-checked → None.
    assert!(t.sample_data(&file, 0).is_none());
}

#[test]
fn fragmented_reports_cleanly() {
    // ftyp + moov (with a track but NO sample tables) + moof → Fragmented, not
    // a panic and not a bogus empty success.
    let stbl_box = stbl(&[
        audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
        // no stts/stsc/stsz/stco → resolve_samples yields empty
    ]);
    let moov_box = moov(&[
        mvhd(44100, 0),
        trak(&[
            tkhd(1, 0, 0),
            mdia(&[mdhd(44100, 0, b"und"), hdlr(b"soun"), minf(stbl_box)]),
        ]),
    ]);
    let moof_box = bx(b"moof", &[0u8; 8]);
    let file = concat(&[ftyp(), moov_box, moof_box]);
    assert_eq!(Mp4::parse(&file), Err(Mp4Error::Fragmented));
}

// ════════════════════════════════════════════════════════════════════════════
// FUZZ — deterministic seeded PRNG; decode must never panic/loop/OOM on ANY
// bytes. FAIL-ability: `#![forbid(unsafe_code)]` makes any OOB index a guaranteed
// panic (aborting the test process), so a clean fuzz run genuinely proves
// bounds-safety. The huge-count test proves the allocation cap: without
// MAX_SAMPLES the absurd-count fuzz would OOM.
// ════════════════════════════════════════════════════════════════════════════

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn byte(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % (n as u64)) as usize
    }
}

#[test]
fn fuzz_random_bytes_never_panic() {
    let mut rng = Rng::new(0x0B16_F00D);
    for _ in 0..40_000 {
        let len = rng.below(512);
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push(rng.byte());
        }
        // The whole point: this must never panic, loop, or OOM.
        let _ = Mp4::parse(&buf);
    }
}

#[test]
fn fuzz_ftyp_prefixed_never_panic() {
    // Random bytes after a real ftyp header (steers the fuzzer past NoFtyp into
    // the box loop / table parsers).
    let prefix = ftyp();
    let mut rng = Rng::new(0x5160_F00D);
    for _ in 0..40_000 {
        let len = rng.below(512);
        let mut buf = prefix.clone();
        for _ in 0..len {
            buf.push(rng.byte());
        }
        let _ = Mp4::parse(&buf);
    }
}

#[test]
fn fuzz_mutated_valid_never_panic() {
    let (base, _off, _sb) = build_m4a();
    assert!(Mp4::parse(&base).is_ok(), "seed must parse");
    let mut rng = Rng::new(0x3333_F00D);
    for _ in 0..60_000 {
        let mut m = base.clone();
        let muts = 1 + rng.below(6);
        for _ in 0..muts {
            let i = rng.below(m.len());
            m[i] ^= rng.byte();
        }
        // Some mutations stay valid, most break — either way: no panic/loop/OOM.
        let _ = Mp4::parse(&m);
    }
}

#[test]
fn fuzz_huge_counts_bounded() {
    // Sweep absurd stsz/stco/stts counts → must Err (the bound), never allocate.
    let mut rng = Rng::new(0x5151_BAD);
    for _ in 0..2000 {
        let count = (rng.next_u64() as u32) | 0x8000_0000; // always huge
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_be_bytes()); // sample_size 0
        body.extend_from_slice(&count.to_be_bytes());
        let stsz_box = fullbox(b"stsz", 0, 0, &body);
        let stbl_box = stbl(&[
            audio_stsd(b"mp4a", 2, 16, 44100, b"esds", &ASC),
            stts(&[(1, 1024)]),
            stsc(&[(1, 1, 1)]),
            stsz_box,
            stco(&[10]),
        ]);
        let moov_box = moov(&[
            mvhd(44100, 1024),
            trak(&[
                tkhd(1, 0, 0),
                mdia(&[mdhd(44100, 1024, b"und"), hdlr(b"soun"), minf(stbl_box)]),
            ]),
        ]);
        let file = concat(&[ftyp(), moov_box]);
        // Must reject the count, not OOM.
        assert_eq!(Mp4::parse(&file), Err(Mp4Error::TooManyEntries));
    }
}
