# Spec: H.264 baseline-profile intra (I-frame) decoder — render the first keyframe of a real `.mp4`

Authoritative implementation spec to make the `apps/video` player display **actual decoded
picture** for the first keyframe of a real H.264 `.mp4`, instead of its current honest
"video: decode pending" gray placeholder. The container, the avcC→Annex-B conversion, the
NAL framing the decoder receives, the audio half (AAC), and the YUV→RGB display path are all
already built and proven (see "Already in the tree"). The **only** delta is the body of
`athmedia::H264Decoder` — replacing the flat-gray `produce_frame` stub with a real
intra-frame reconstruction pipeline.

This is the video equivalent of `docs/research/aac-lc-decoder.md` — same discipline:
self-contained `#![no_std]`, soft-float, no external decode deps, never-panic on hostile
bytes, host-KAT-first proof, and a corroboration gate across ≥2 independent open decoders
for every algorithm and table. The implementer transcribes/derives the small tables in this
doc and follows the pipeline; nothing here requires web access.

## Concept promise served

> "A daily driver must 'play my movies' and 'play my music.' MP4 … (the ISO Base Media File
> Format) is the dominant container for both — phone video, downloaded video, and AAC audio
> (`.m4a`/`.mp4`) all ship as BMFF."
> (LEGACY_GAMING_CONCEPT.md §creators / media — the "it just works" media pillar; the same line
> `ath_mp4/src/lib.rs` and the AAC spec quote.)

And the manifesto's first principle:

> "Native everywhere. No Electron tax. No web wrappers. Native rendering, native input,
> native audio." (LEGACY_GAMING_CONCEPT.md §Core Principles 1)

"Play my movies" is not true until a `.mp4` shows a real picture. H.264 baseline/constrained-
baseline is the floor of "movies and downloaded video"; AAC (its usual audio partner) is
already audible via the companion spec. This closes the video half — at minimum the first
keyframe, which is the gate before motion-compensated playback.

## Already in the tree (verify-before-implement)

Do **not** rebuild these. The decoder body is the **only** delta.

- `components/ath_mp4/src/lib.rs` — **[x] built (host-KAT'd).** Resolves each sample's absolute
  offset/size (`Track::sample_data`), splits H.264 (`avc1`/`avc3`) vs AAC tracks, and surfaces
  the **`avcC` config record** in `Track::codec_private`. Sync (key) samples are flagged
  (`Sample::is_sync`). The demuxer never parses codec internals.
- `apps/video/src/lib.rs` — **[x] built.** `decode_first_video()` already:
  (1) extracts SPS (NAL 7) + PPS (NAL 8) from the avcC record (`avcc_extract_param_sets`) and
  feeds them to the decoder as an **Annex-B** parameter-set `MediaPacket` *before* the slice;
  (2) finds the first sync sample, converts its length-prefixed NALs to Annex-B
  (`avcc_to_annexb`), and feeds it as a second `MediaPacket`;
  (3) takes the returned `VideoFrame` (expects `PixelFormat::Yuv420p`, 3 planes) and converts
  it to ARGB8888 (`yuv_frame_to_argb`), with a `MAX_FRAME_PIXELS` DoS guard and clean
  `Ok(None)` on a 0-sized or absent frame. **The wiring is complete and correct** — it
  "will display real picture the moment the engine learns reconstruction." No change needed in
  `apps/video` for v1.
- `components/athmedia/src/lib.rs` — **[~] scaffold, emits flat gray.** The pieces to keep:
  - `VideoDecoder` trait (`codec`, `decode(&MediaPacket) -> Result<Option<VideoFrame>>`,
    `flush`, `reset`, `capabilities`) — **frozen, do NOT change** (the consumer depends on it).
  - `VideoFrame { width, height, pixel_format, planes: Vec<VideoPlane{data,stride}>, pts,
    duration, keyframe, color_space, color_range, ... }` — the output shape. v1 emits exactly
    `PixelFormat::Yuv420p`, 3 planes (Y full-res, Cb/Cr quarter-res), strides = plane width.
  - `H264Decoder { sps, pps, dpb, current_slice, nal_buffer, output_queue }` + `H264Sps`,
    `H264Pps`, `H264SliceHeader`, `H264SliceType` structs — **the fields exist but are never
    populated with real values.** The Exp-Golomb parse must fill `width_mbs`/`height_mbs`/etc.
  - `H264Decoder::parse_nal_units()` — **[x] correct.** Annex-B start-code scanner (3- and
    4-byte) that splits the byte stream into NAL units and calls `process_nal`. Reuse as-is.
  - `H264Decoder::process_nal()` — **[~] stub.** It type-dispatches (7=SPS, 8=PPS, 1/5=slice)
    but stores **placeholder constants** (width_mbs=0, init_qp=26, etc.) instead of parsing.
    **This is the call site to replace** with real Exp-Golomb SPS/PPS/slice-header parsing +
    the reconstruction call.
  - `H264Decoder::produce_frame()` — **[~] the stub to delete.** Defaults to 1920×1080 (or
    `width_mbs*16` which is always 0), fills Y=0, Cb=Cr=128 → flat gray. Replace with the
    reconstructed frame.
  - `PixelConverter::yuv420_to_rgb` — **[x] built.** BT.601 integer conversion the consumer
    uses. Not on the decode path but confirms the output contract.
- `components/athmedia/src/{jpeg.rs, mp3.rs, mp3_dsp.rs, mp3_imdct_tables.rs, aac.rs}` — **the
  no_std soft-float decode-style precedent.** In particular:
  - `jpeg.rs` — **[x] built.** The closest sibling: a from-scratch **8×8 integer IDCT**,
    zig-zag, dequant, YCbCr→RGB, 4:2:0 chroma upsample, and the *exact* hostile-input posture
    to match (truncated/garbage → clean `Err`, never panic). H.264's 4×4 integer transform,
    chroma subsampling, and block raster are the same family of code; **read `jpeg.rs` first**
    and mirror its structure, error handling, and table-as-`const` style.
  - The MP3/AAC bit-reader + "skip-and-flag a deferred tool, never emit wrong output" posture
    is the model for how this decoder degrades on CABAC/inter/multi-slice (see Honest scope).

Status to flip when done: the H.264 / video rows under media / Phase-6 (AthGFX/playback) in
`MasterChecklist.md` from `[~] decode pending (flat surface)` → `[~] first keyframe decoded
(host/QEMU)` → `[x]` once an Athena boot shows a real-picture frame from a user `.mp4`.

## Prior art & OSS verdict

Every algorithm and table below is **corroborated across ≥2 independent open decoders**. None
is vendored or linked — they are read-only spec oracles; AthenaOS keeps its own `#![no_std]`,
no-libm, soft-float decoder (Concept §R7: H.264 is an ITU-T/MPEG codec, not a Linux subsystem;
the implementation is original Rust over ITU data tables, no `libavcodec`/`openh264` link).

- **ITU-T H.264 (= ISO/IEC 14496-10), the normative source.** Clause map used throughout:
  §7.3.2.1 (SPS syntax), §7.3.2.2 (PPS), §7.3.3 (slice header), §7.4.x (semantics),
  §8.3 (intra prediction: 8.3.1 Intra_4x4, 8.3.2 Intra_8x8 [High only — N/A], 8.3.3
  Intra_16x16, 8.3.4 chroma), §8.5 (transform + scaling + reconstruction: 8.5.10 scaling,
  8.5.12 inverse 4×4 transform, 8.5.8 chroma DC, 8.5.13 picture construction), §8.6/§8.7
  (deblocking in-loop filter), §9.1 (Exp-Golomb `ue(v)`/`se(v)`), §9.2 (CAVLC residual).
  📖 normative reference, not code.
- **FFmpeg `libavcodec/h264*` (`h264_ps.c`, `h264_slice.c`, `h264_cavlc.c`, `h264_mb.c`,
  `h264idct.c`, `h264pred.c`, `h264_loopfilter.c`, `cavlc table h264data`)** (LGPL) — first
  oracle for the CAVLC coeff_token / total_zeros / run_before tables, the intra-prediction
  edge-availability rules, the 4×4 inverse-transform integer math, the dequant `LevelScale`,
  and the deblock boundary-strength + filter clip tables. 📖 **study/isolate (LGPL)** — source
  of the numeric tables reproduced/cited below; **no code copied.**
- **openh264 (Cisco, BSD-2-Clause)** — second oracle, and the **permissive** one. Its
  `decoder/core/src/{parse_mb_syn_cavlc.cpp, decode_slice.cpp, get_intra_predictor.cpp,
  deblocking.cpp}` and the CAVLC VLC tables in `decoder/core/inc/`. 📖 **➕ permissively
  licensed (BSD-2) — could be vendored**, but the recommendation is **study/isolate**: we
  keep the from-scratch `#![no_std]` decoder (consistent with jpeg/mp3/aac being native, and
  avoiding a C/C++ build dependency in the userspace media crate). openh264 is the algorithm +
  table cross-check oracle, not a dependency. *(If a future decision wants a fast-path C
  decoder, openh264's BSD-2 license makes it the only vendorable option — flag for the lead.)*
- **The H.264 "white book" / Iain Richardson, *The H.264 Advanced Video Compression Standard*
  (2nd ed.)** — the canonical pedagogical reference for CAVLC, the 4×4 transform/quant
  derivation, and intra prediction; the third corroboration where FFmpeg and openh264 differ
  in bookkeeping. 📖 study only.
- **`docs/OSS_RECOMMENDATIONS.md`** — symphonia (audio) is userspace-only and explicitly
  **not** for video; the GStreamer/FFmpeg-binding crates are rejected (C codecs in the native
  stack). No existing recommendation covers an H.264 decoder → this native decoder is the
  intended path, matching the jpeg/mp3/aac precedent. **Verdict: build native, no new dep.**

**Corroboration gate (how every table is trusted):** the CAVLC VLC tables (§D.1, the bulk of
the data) and the intra/transform/deblock constant tables (§D.2–D.4) are reproduced or
specified below from the ITU-T tables and cross-checked FFmpeg ↔ openh264. The implementer
re-keys the VLC tables into a prefix-free-checking generator (the H.264 analogue of
`tools/mp3_huff_gen`, see §7.1) which **rejects** any non-prefix-free table — a transcription
typo cannot reach the decoder silently. Any FFmpeg↔openh264 mismatch is flagged, not guessed.

---

## Design

### §0 — Pipeline overview (what the decoder does, end to end)

Input arrives as two `MediaPacket`s (the consumer already builds them, see "Already in the
tree"): first an **Annex-B parameter-set packet** (SPS NAL 7 + PPS NAL 8), then an **Annex-B
slice packet** (one IDR slice, NAL 5). `parse_nal_units` already splits these into NAL units.
The new pipeline, per the ITU-T clauses:

```
 NAL byte stream
   → parse_nal_units()  [EXISTS]  → per-NAL units
   → for each NAL, strip emulation-prevention bytes (00 00 03 → 00 00) into an RBSP   §7.3.1
   → process_nal():
        NAL 7 SPS  → parse_sps()   (Exp-Golomb)                                       §7.3.2.1
        NAL 8 PPS  → parse_pps()   (Exp-Golomb)                                       §7.3.2.2
        NAL 5 IDR / NAL 1 (I-slice only) → decode_slice():
            parse_slice_header()                                                       §7.3.3
            if slice_type not I/SI → return clean Err (defer P/B; see Honest scope)
            if pps.entropy_coding_mode (CABAC) → return clean Err (defer; CAVLC only)
            allocate the frame (Y: w×h, Cb/Cr: (w/2)×(h/2)), w/h from SPS (§1)
            for each macroblock in raster order (mb_addr 0..PicSizeInMbs):
                parse mb_type (ue) → I_NxN | I_16x16_* | I_PCM                         §7.3.5
                if I_PCM: copy raw samples                                             §7.3.5
                else:
                    parse intra pred modes (4×4: per-block; 16×16: from mb_type)       §7.3.5.1
                    parse chroma intra pred mode (ue)                                  §7.3.5.1
                    parse mb_qp_delta (se), residual via CAVLC                         §7.3.5.3 / §9.2
                    intra-predict each block from already-reconstructed neighbours     §8.3
                    dequant + inverse 4×4 transform the residual                       §8.5
                    reconstruct = clip(prediction + residual)                          §8.5.13
            in-loop deblocking filter over the whole frame                             §8.7
   → emit VideoFrame { Yuv420p, planes=[Y,Cb,Cr], width=cropped, height=cropped, ... }
```

The reconstruction is **causal in raster order**: each MB's intra prediction reads only
already-reconstructed pixels above/left, so a single pass over MBs (predict→residual→
reconstruct) is correct. Deblocking is a separate final pass (it reads reconstructed pixels
across MB edges).

### §1 — SPS parse (recover the REAL width/height) — the highest-value first step

This alone fixes the most visible current bug (the decoder defaults to 1920×1080 and ignores
the real geometry). `parse_sps()` reads the SPS RBSP with an Exp-Golomb bit reader (§9.1):

```
profile_idc                : u(8)     // 66 = Baseline, 77 = Main, 88 = Extended, 100 = High …
constraint_set0..5 + 2 rsv : u(8)     // constraint_set1_flag=1 ⇒ Constrained Baseline
level_idc                  : u(8)
seq_parameter_set_id       : ue(v)
// (profile_idc in {100,110,122,244,44,83,86,118,128,138,139,134,135} ⇒ High-family extra
//  fields: chroma_format_idc, bit_depth, scaling lists. For BASELINE these are ABSENT.
//  If present AND chroma_format_idc != 1 (4:2:0) ⇒ clean Err. If scaling lists present ⇒
//  clean Err for v1 (flat 4×4 scaling only). See Honest scope.)
log2_max_frame_num_minus4          : ue(v)   // → log2_max_frame_num
pic_order_cnt_type                 : ue(v)
  if == 0: log2_max_pic_order_cnt_lsb_minus4 : ue(v)
  if == 1: delta_pic_order_always_zero_flag u(1); offset_for_non_ref_pic se(v);
           offset_for_top_to_bottom_field se(v); num_ref_frames_in_pic_order_cnt_cycle ue(v);
           then that many se(v)        // I-only decode doesn't need POC, but MUST skip it correctly
max_num_ref_frames                 : ue(v)
gaps_in_frame_num_value_allowed    : u(1)
pic_width_in_mbs_minus1            : ue(v)   // → PicWidthInMbs   = val + 1
pic_height_in_map_units_minus1     : ue(v)   // → PicHeightInMapUnits = val + 1
frame_mbs_only_flag                : u(1)    // BASELINE: 1 (no fields). If 0 ⇒ clean Err (no interlace v1)
  if !frame_mbs_only_flag: mb_adaptive_frame_field_flag u(1)
direct_8x8_inference_flag          : u(1)
frame_cropping_flag                : u(1)
  if frame_cropping_flag: crop_left ue, crop_right ue, crop_top ue, crop_bottom ue
vui_parameters_present_flag        : u(1)   // VUI (color/aspect) — OPTIONAL to parse; skip safely.
```

Derived geometry (§7.4.2.1.1):
```
FrameHeightInMbs = (2 - frame_mbs_only_flag) * PicHeightInMapUnits      // = PicHeightInMapUnits for baseline
PicSizeInMbs     = PicWidthInMbs * FrameHeightInMbs
coded_width  = PicWidthInMbs  * 16
coded_height = FrameHeightInMbs * 16
// Cropping (4:2:0 ⇒ CropUnitX=2, CropUnitY=2 when frame_mbs_only):
display_width  = coded_width  - 2*(crop_left + crop_right)
display_height = coded_height - 2*(crop_top  + crop_bottom)
```

**v1 reconstructs at coded (MB-aligned) resolution, then crops the output planes to
`display_width × display_height`** (the existing `PixelConverter::crop` is a model; do it
inline on the YUV planes). Populate the existing `H264Sps` fields (`width_mbs`, `height_mbs`,
`profile`, `level`, `frame_mbs_only`, `log2_max_frame_num`, `poc_type`, etc.) with these
**real** values. **Bound everything:** clamp `PicWidthInMbs`/`FrameHeightInMbs` so
`PicSizeInMbs * 256 * 3/2 <= MAX_FRAME_BYTES` (a crafted SPS must not allocate gigabytes —
return `Err` past a sane cap, e.g. 8192×8192). This is the #1 RCE/DoS surface; match
`ath_mp4`'s posture.

### §2 — PPS parse — `parse_pps()` (§7.3.2.2)

```
pic_parameter_set_id               : ue(v)
seq_parameter_set_id               : ue(v)
entropy_coding_mode_flag           : u(1)   // 0 = CAVLC (v1), 1 = CABAC ⇒ clean Err (deferred)
bottom_field_pic_order_in_frame_present_flag : u(1)
num_slice_groups_minus1            : ue(v)  // baseline FMO: if > 0 ⇒ slice-group map; v1 supports
                                            //   0 (single slice group). >0 ⇒ clean Err (rare; deferred)
num_ref_idx_l0_default_active_minus1 : ue(v)   // I-only: unused, but skip correctly
num_ref_idx_l1_default_active_minus1 : ue(v)
weighted_pred_flag                 : u(1)
weighted_bipred_idc                : u(2)
pic_init_qp_minus26                : se(v)   // → init_qp = 26 + val   (the slice base QP)
pic_init_qs_minus26                : se(v)
chroma_qp_index_offset             : se(v)
deblocking_filter_control_present_flag : u(1)
constrained_intra_pred_flag        : u(1)    // if 1, intra pred treats inter neighbours as
                                             //   unavailable — for an all-intra IDR frame this
                                             //   is moot (all neighbours are intra), handle anyway
redundant_pic_cnt_present_flag     : u(1)
// (more fields only if more_rbsp_data: transform_8x8_mode_flag, scaling lists, 2nd chroma qp
//  offset — High profile; ABSENT for baseline. If present transform_8x8 ⇒ clean Err.)
```
Populate the existing `H264Pps` (`init_qp`, `entropy_coding_mode`, `chroma_qp_offset`,
`deblocking_filter_present`, etc.) with real values.

### §3 — Slice header parse — `parse_slice_header()` (§7.3.3)

For the first keyframe we only need the IDR I-slice header:
```
first_mb_in_slice          : ue(v)   // 0 for a single-slice frame; if !=0 ⇒ multi-slice (defer)
slice_type                 : ue(v)   // 2 or 7 = I ; 4 or 9 = SI ; others (P/B) ⇒ clean Err
pic_parameter_set_id       : ue(v)
frame_num                  : u(log2_max_frame_num)
// frame_mbs_only ⇒ no field_pic_flag
idr_pic_id                 : ue(v)   // present iff IdrPicFlag (NAL type 5)
if pic_order_cnt_type == 0: pic_order_cnt_lsb : u(log2_max_pic_order_cnt_lsb)   // skip (I-only)
// dec_ref_pic_marking (IDR): no_output_of_prior_pics_flag u(1), long_term_reference_flag u(1)
slice_qp_delta             : se(v)   // → SliceQPY = 26 + pic_init_qp_minus26 + slice_qp_delta
if deblocking_filter_control_present_flag:
    disable_deblocking_filter_idc ue(v)   // 0 = on (default), 1 = off, 2 = on except slice edges
    if idc != 1: slice_alpha_c0_offset_div2 se(v); slice_beta_offset_div2 se(v)
```
Populate `H264SliceHeader` (`slice_type`, `frame_num`, `qp_delta`). Carry `SliceQPY` (the
per-MB running QP base) and the two deblock offsets into the MB loop.

### §4 — CAVLC residual decode (§9.2) — the entropy core

CAVLC decodes the quantized transform coefficients of each 4×4 block. This is the largest
table-driven piece. Per 4×4 luma block (16 coeffs) — and the chroma DC/AC blocks — the
procedure (ITU-T §9.2.1–9.2.4):

```
1. coeff_token (VLC) → (TotalCoeff, TrailingOnes)        §9.2.1, Tables 9-5
   The VLC table is selected by nC (a context = number of nonzero coeffs in the LEFT and
   ABOVE 4×4 blocks; nC = (nA+nB+1)>>1 if both available, else nA or nB or 0). For chroma DC
   (4:2:0) nC = -1 selects a dedicated table (Table 9-5 last column).
2. for each TrailingOne: read 1 sign bit (±1).            §9.2.2
3. for the remaining (TotalCoeff - TrailingOnes) levels:  §9.2.2.1
   level_prefix (VLC = count leading 0s then a 1) + level_suffix (suffixLength bits) →
   level value, with the documented suffixLength adaptation (starts 0 or 1, grows).
4. total_zeros (VLC, table indexed by TotalCoeff)         §9.2.3, Tables 9-7/9-8
5. for each level except the last: run_before (VLC, indexed by zerosLeft) → run lengths
                                                          §9.2.4, Table 9-10
6. scatter the levels into the 16-entry block in zig-zag (Fig 8-? / the 4×4 zig-zag scan)
   using the runs; the result is the dequant-input coefficient block.
```

**Context maintenance:** the decoder MUST keep, per 4×4 block position, the `TotalCoeff`
("nnz", number of nonzero coeffs) of the left and above neighbours to compute `nC`. Store a
per-MB `nnz[16]` (luma) + `nnz_chroma[2][4]` and a frame-level edge cache (the row above /
column left). This is the only stateful bookkeeping in CAVLC; openh264 `iNonZeroCount` and
FFmpeg `non_zero_count_cache` are the reference layouts.

The block scan order within a macroblock and the Intra_16x16 DC/AC split:
- **I_16x16:** one 4×4 luma **DC** block (Hadamard-transformed DC of the 16 sub-blocks,
  CAVLC table with nC from neighbours) + sixteen 4×4 luma **AC** blocks (15 coeffs each, DC
  excluded). §8.5.10 / §8.5.6 (the DC uses the 4×4 Hadamard inverse).
- **I_NxN (Intra_4x4):** sixteen 4×4 luma blocks (full 16 coeffs each), no separate DC.
- **Chroma (4:2:0):** for each of Cb/Cr: one 2×2 chroma **DC** block (CAVLC, nC=-1) +
  four 4×4 chroma **AC** blocks (15 coeffs). §8.5.11.

All CAVLC VLC tables are reproduced/specified in **§D.1**.

### §5 — Intra prediction (§8.3) — predict each block from reconstructed neighbours

**Intra_4x4 (nine modes, §8.3.1.2):** for each 4×4 luma block, a per-block 4×4 prediction
mode is decoded (`prev_intra4x4_pred_mode_flag` u(1); if 0, `rem_intra4x4_pred_mode` u(3)) and
combined with the **predicted mode** (min of the left & above blocks' modes, §8.3.1.1). The
nine modes, each producing the 4×4 prediction from the 13 boundary samples (4 left, 4 above,
4 above-right, 1 above-left):
```
0 Vertical    1 Horizontal   2 DC          3 Diagonal-Down-Left   4 Diagonal-Down-Right
5 Vertical-Right   6 Horizontal-Down   7 Vertical-Left   8 Horizontal-Up
```
Each mode's exact pixel formula is in §8.3.1.2.1–.9; they are short averaging/copy filters
(reproduced compactly in **§D.2**). **Edge availability:** a neighbour that is outside the
picture, outside the slice, or (if `constrained_intra_pred_flag`) inter-coded is "not
available"; DC mode substitutes 128, directional modes have documented fallbacks (§8.3.1.2.2).
For an all-intra IDR frame, only the picture-boundary unavailability matters.

**Intra_16x16 (four modes, §8.3.3):** the mode comes from `mb_type` (not separately coded):
```
0 Vertical    1 Horizontal    2 DC    3 Plane
```
Plane mode (mode 3) is the gradient predictor (§8.3.3.4) — the one with real arithmetic
(H/V gradient sums, clipped). Predicts the whole 16×16 luma block at once.

**Chroma intra (four modes, §8.3.4):** `intra_chroma_pred_mode` ue(v) selects:
```
0 DC    1 Horizontal    2 Vertical    3 Plane
```
applied to each 8×8 chroma block (Cb and Cr). Same shapes as 16×16 luma modes, 8×8 size.

All mode formulas + the predicted-mode derivation are in **§D.2**.

### §6 — Inverse transform, dequant, reconstruction (§8.5)

**Dequant (scaling, §8.5.10 with flat scaling lists — baseline has no custom lists):**
```
qP        = current MB QP (luma: SliceQPY + Σ mb_qp_delta, wrapped 0..51; chroma: mapped via
            Table 8-15 from qPI = clip(qP + chroma_qp_index_offset))
LevelScale4x4[m][i][j] = weightScale4x4[i][j] * normAdjust4x4[m][i][j]   // weightScale = 16 (flat)
// normAdjust4x4 is the 6×(positional) table {v[m][0],v[m][1],v[m][2]} from §8.5.9 (reproduced §D.3)
d[i][j] = (c[i][j] * LevelScale4x4[qP%6][i][j]) << (qP/6)     // for qP>=24 region; the general
          form with the (qP/6) shift / rounding per §8.5.12.1
// I_16x16 DC and chroma DC use the separate Hadamard-domain scaling §8.5.10 step.
```

**Inverse 4×4 transform (§8.5.12.2) — the integer "core transform" (no multiplies, only
add/shift), the H.264 hallmark:**
```
// 1-D inverse on rows, then columns, of the dequantized 4×4 block d:
e0 = d0 + d2;  e1 = d0 - d2;  e2 = (d1>>1) - d3;  e3 = d1 + (d3>>1)
f0 = e0 + e3;  f1 = e1 + e2;  f2 = e1 - e2;  f3 = e0 - e3
// apply on rows then columns, then residual r = (f + 32) >> 6
```
**I_16x16 luma DC:** inverse 4×4 **Hadamard** on the 16 DC coeffs first (§8.5.10), redistribute
to the 16 blocks' DC positions before the per-block inverse transform. **Chroma DC:** inverse
2×2 Hadamard (§8.5.11).

**Reconstruction (§8.5.13):** `recon[y][x] = Clip1( pred[y][x] + r[y][x] )` (Clip1 = clamp to
[0,255] for 8-bit). Write into the frame's Y/Cb/Cr planes at the MB's raster position. Because
intra prediction of later MBs reads these reconstructed samples, **reconstruction must complete
each MB before the next MB is predicted** (a single raster pass; deblocking is deferred to §7).

The exact `normAdjust4x4`, the chroma QP map (Table 8-15), and the zig-zag scan order are in
**§D.3**.

### §7 — In-loop deblocking filter (§8.7) — the final pass

After all MBs are reconstructed, filter the 4×4-block edges (vertical edges left-to-right, then
horizontal edges top-to-bottom, per MB in raster order, §8.7). Per edge:
```
1. Boundary strength bS (§8.7.2.1): for an all-intra frame, bS = 4 on MB edges and bS = 3 on
   internal 4×4 edges (the intra rules — §8.7.2.1 Table; inter/motion rules are N/A for I).
2. Filter on/off + clip thresholds α(indexA), β(indexB) from Tables 8-16/8-17 (§D.4), where
   indexA = Clip(qPav + FilterOffsetA), indexB = Clip(qPav + FilterOffsetB), qPav = average of
   the two blocks' QP, FilterOffset* = 2*slice_alpha_c0_offset_div2 / *_beta_offset_div2.
3. If |p0-q0| < α and |p1-p0| < β and |q1-q0| < β: filter.
   bS<4: the normal filter (4-tap, clipped by tC0 from Table 8-17 + a chroma/luma tweak).
   bS==4: the strong filter (the wider 16x16-edge filter, §8.7.2.4).
```
`disable_deblocking_filter_idc` (from §3): 0 = filter all edges, 1 = filter none (skip the
pass), 2 = filter internal edges but not slice boundaries (single-slice frame ⇒ same as 0
internally). The α/β/tC0 tables are in **§D.4**. Deblocking materially affects visual quality
at MB boundaries; it is **in scope for v1** (a frame without it has visible blocking).

### §8 — Output (the `VideoFrame` contract)

Emit exactly the shape the consumer expects:
```
VideoFrame {
  width:  display_width,  height: display_height,        // cropped (§1)
  pixel_format: PixelFormat::Yuv420p,
  planes: vec![
    VideoPlane { data: Y  (display_width * display_height),         stride: display_width },
    VideoPlane { data: Cb ((display_width/2)*(display_height/2)),   stride: display_width/2 },
    VideoPlane { data: Cr (same as Cb) },
  ],
  pts: packet.pts, duration, keyframe: true,
  color_space: from VUI if parsed else Bt709 (HD) / Bt601 (SD), color_range: Limited (default),
  ...
}
```
Crop from the coded (MB-aligned) reconstruction to display size by copying rows (Y full-res;
Cb/Cr at half the crop offsets, 4:2:0). `H264Decoder::decode` returns `Ok(Some(frame))` for the
slice packet, `Ok(None)` for the parameter-set-only packet (matching the consumer's
expectation — it `let _ =`s the param-set result).

**Untrusted-input discipline (non-negotiable, matches `jpeg.rs`/`ath_mp4`):** every
Exp-Golomb/CAVLC read is bounds-checked (a read past the RBSP end returns an error → the frame
yields `Err`/`None`, never a panic); every count derived from the bitstream (`PicSizeInMbs`,
`mb_qp_delta`, `TotalCoeff`, intra mode index, `n_filt`-equivalents) is clamped to its ITU
maximum before indexing a table or buffer; the frame allocation is capped (§1). Parsers are the
#1 RCE surface — a hostile `.mp4` must degrade to "can't decode this," never to memory unsafety.

---

## Honest scope (what's IN for v1, what's deferred, how it degrades)

**IN (the first-keyframe gate):**
- Baseline / Constrained Baseline profile (profile_idc 66; accept Main 77 too if it's
  CAVLC-only, all-intra, no custom scaling — many "baseline-ish" files are tagged Main).
- **I-slices / IDR only** (slice_type I/SI). **CAVLC** entropy only. **4:2:0, 8-bit** only.
- SPS/PPS/slice-header Exp-Golomb parse with real geometry + crop.
- Intra_4x4 (9 modes) + Intra_16x16 (4 modes) + chroma intra (4 modes) + I_PCM.
- 4×4 integer inverse transform + Hadamard DC (16×16 luma, 2×2 chroma) + flat dequant.
- The in-loop deblocking filter (intra bS=3/4 rules).
- Single slice group (no FMO), single slice per frame.

**DEFERRED (return a clean `Err(MediaError::DecoderError("..."))` the instant the bitstream
demands it — the consumer already turns `Err` into `Ok(None)` = the honest placeholder; NEVER
emit a wrong-shape frame or panic):**
- **CABAC** (`entropy_coding_mode_flag==1`) → `Err("h264: CABAC not supported")`. (Main/High
  default to CABAC; this is the most common reason a given file won't decode in v1 — detect it
  in `parse_pps` and bail before any MB work.)
- **P/B slices / inter prediction / motion compensation** (slice_type P/B) → `Err`. This is
  why v1 is "first keyframe only": subsequent frames reference the keyframe via motion vectors,
  which is a whole second milestone. The keyframe itself is fully self-contained (intra).
- **Main/High profile tools:** 8×8 transform (`transform_8x8_mode_flag`), custom scaling
  lists, monochrome/4:2:2/4:4:4 chroma, >8-bit depth → `Err`.
- **Interlaced** (`frame_mbs_only_flag==0`, MBAFF/PAFF) → `Err`.
- **Multiple slices / slice groups (FMO/ASO)** (`first_mb_in_slice!=0` or
  `num_slice_groups_minus1>0`) → `Err`.

This mirrors the AAC decoder's posture exactly: HE-AAC SBR/PS, PNS, intensity are skipped with
"audible but band-limited / slightly wrong, never wrong PCM, never a crash." Here the analogue
is "decode the keyframe correctly, or cleanly report we can't decode this stream" — the app
keeps its honest "decode pending / can't play this file" UI and stays alive.

---

## §D — Data tables (generator-input form + the constant tables)

The implementer transcribes these into `tools/h264_vlc_gen/gen.rs` (which re-verifies the VLC
tables are prefix-free, §7.1) and `components/athmedia/src/h264_tables.rs`. The numeric
contents are ITU-T H.264 tables; this doc cites the clause + the FFmpeg/openh264 array name for
each so the implementer can cross-check entry-for-entry (the corroboration gate). The tables are
**not** reproduced in full inline here (CAVLC is ~thousands of VLC entries across the nC
contexts — far larger than the AAC books) — instead each is specified by its **ITU table number
+ the two oracle array names + its dimensions + a worked anchor entry** the KAT pins. *This is
the one place this spec is a transcription pointer rather than a full inline dump; the §7 KAT's
prefix-free + known-codeword gates make a transcription error fail loudly, and both oracle
arrays are permissive-enough to read (openh264 is BSD-2).*

### §D.1 — CAVLC VLC tables
- **coeff_token** — ITU-T **Table 9-5** (4 sub-tables for nC ranges 0≤nC<2, 2≤nC<4, 4≤nC<8,
  8≤nC, plus the nC=-1 chroma-DC table). FFmpeg `coeff_token_vlc` / `chroma_dc_coeff_token_vlc`;
  openh264 `g_kpCavlcTempBoxTable`/`CavlcGetCoeffToken`. Dim: each maps a VLC code →
  (TotalCoeff 0..16, TrailingOnes 0..3). **Anchor:** nC<2, code `1` (1 bit) → (0 coeffs, 0 T1s)
  — the "all-zero block" token; nC=-1 chroma anchor likewise.
- **level_prefix** — ITU-T §9.2.2.1: a pure leading-zeros count (count 0-bits until a 1). No
  table; an algorithm. **Anchor:** bits `1` → prefix 0; `01` → 1; `001` → 2.
- **total_zeros** — ITU-T **Tables 9-7 and 9-8** (9-7 for 4×4 blocks indexed by TotalCoeff
  1..15; 9-8 for the 2×2 chroma-DC block). FFmpeg `total_zeros_vlc[]` /
  `chroma_dc_total_zeros_vlc[]`; openh264 `g_kpTotalZeros*`. **Anchor:** TotalCoeff=1 table,
  code → total_zeros value per the table's first row.
- **run_before** — ITU-T **Table 9-10**, indexed by `zerosLeft` (1..6, and >6). FFmpeg
  `run_vlc[]` / `run7_vlc`; openh264 `g_kpRunBefore`. **Anchor:** zerosLeft=1: code `1`→run 0,
  `0`→run 1.

### §D.2 — Intra prediction
- The 9 Intra_4x4 mode formulas — ITU-T §8.3.1.2.1–.9 (short averaging/copy filters over the
  13 boundary samples). FFmpeg `h264pred.c` `pred4x4_*`; openh264 `WelsI4x4LumaPred*`.
- The predicted-4×4-mode derivation (min of left/above) — ITU-T §8.3.1.1.
- The 4 Intra_16x16 modes (Vertical/Horizontal/DC/**Plane**) — §8.3.3; Plane gradient §8.3.3.4.
  FFmpeg `pred16x16_*`; openh264 `WelsI16x16Luma*`.
- The 4 chroma modes (DC/H/V/Plane) — §8.3.4. FFmpeg `pred8x8_*`; openh264 `WelsIChroma*`.
  These are pure closed-form formulas (no tables) — the implementer writes them from the clause;
  the KAT (§7) validates each against an in-test reference.

### §D.3 — Transform / dequant
- `normAdjust4x4` (the 6×3 dequant scale table) — ITU-T **§8.5.9** (the `{10,16,13},{11,18,14},
  {13,20,16},{14,23,18},{16,25,20},{18,29,23}` matrix, mapped to positions by parity). FFmpeg
  `dequant4_coeff_init` / `ff_h264_dequant4_coeff`. Reproduce inline in `h264_tables.rs`.
- Chroma QP map **Table 8-15** (qPI 30..51 → qPC). FFmpeg `ff_h264_chroma_qp`. 22-entry table —
  reproduce inline.
- The 4×4 zig-zag scan order (§8.5.6) — the 16-entry permutation. FFmpeg `ff_zigzag_scan`.
- The 4×4 inverse core transform + Hadamard (§6) are closed-form (no table).

### §D.4 — Deblocking
- α (Table 8-16, 52 entries indexed by indexA), β (Table 8-17, 52 entries by indexB), and
  tC0 (Table 8-17, indexed by indexA × bS) — ITU-T §8.7.2.2. FFmpeg `alpha_table`/`beta_table`/
  `tc0_table` in `h264_loopfilter.c`; openh264 `g_kiAlphaTable`/`g_kiBetaTable`/`g_kiTc0Table`.
  Three small `const` tables — reproduce inline in `h264_tables.rs`.

---

## Interface needs (NEEDS-INTERFACE)

**None.** Entirely inside the `athmedia` userspace crate, consuming `ath_mp4` output through its
existing public API and emitting through the existing `VideoDecoder`/`VideoFrame` contract. No
new syscall, no `ath_abi` change, no kernel ABI surface, **and no change to the `VideoDecoder`
trait or `VideoFrame` struct** (the consumer `apps/video` depends on the current shape). Only
the *contents* of the decoded frame go from flat gray to real picture.

## File-by-file plan

- `tools/h264_vlc_gen/gen.rs` — **new.** The prefix-free-checking VLC generator (§7.1), modeled
  on `tools/mp3_huff_gen/gen.rs` and `tools/aac_huff_gen/gen.rs`. Holds the CAVLC tables
  (coeff_token per nC context, total_zeros, run_before, chroma-DC variants) as `{code, len,
  value...}` rows; asserts each context is prefix-free + correctly dimensioned; emits the
  `h264_tables.rs` VLC arrays; prints `All H.264 CAVLC tables verified prefix-free.` (or
  `FAIL <table>: ...` + exit 1).
- `components/athmedia/src/h264_tables.rs` — **new.** Generator output (the CAVLC VLC arrays) +
  the hand-reproduced `const` tables: `normAdjust4x4`, the chroma-QP map (Table 8-15), the 4×4
  zig-zag scan, the deblock α/β/tC0 tables (§D.4). All `const`, no runtime computation.
- `components/athmedia/src/h264.rs` — **new.** The decoder core: an Exp-Golomb/CAVLC `BitReader`
  (reuse the MP3/AAC bit-reader pattern) with RBSP emulation-prevention stripping; `parse_sps`,
  `parse_pps`, `parse_slice_header`; the MB loop (mb_type, intra modes, CAVLC residual); intra
  prediction (§5); inverse transform + dequant + reconstruction (§6); the deblocking pass (§7);
  the frame crop + `VideoFrame` build (§8). Concept docstring quoting the promise above. A
  FAIL-able `run_boot_smoketest()`.
- `components/athmedia/src/lib.rs` — rewire `H264Decoder::process_nal` to call
  `h264::parse_sps`/`parse_pps`/`decode_slice` and `H264Decoder::decode` to return the
  reconstructed `VideoFrame`; **delete `produce_frame`'s flat-gray body** (or keep it only as
  the explicit fallback for an unsupported-feature `Err`, returning `Ok(None)` instead — prefer
  clean `Err`→`None`). Populate the real `H264Sps`/`H264Pps`/`H264SliceHeader` fields. Add the
  host-KAT module asserts to the `#[cfg(test)]` block. Declare `pub mod h264;` + `mod
  h264_tables;`. **Do not** touch the `VideoDecoder` trait or `VideoFrame`/`VideoPlane` structs.
- `apps/video/src/lib.rs` — **no code change.** Once the decoder returns real picture, update
  the module docstring + the "video: decode pending" UI string to reflect "first keyframe
  decoded" (cosmetic; can be a follow-up). The wiring already works.

## §7 — Host-KAT proof strategy (the reason for this spec)

Proof is **host-KAT-first**, exactly like MP3/AAC: pure decode logic (no syscalls, `#![no_std]`
+ `alloc`) runs under `cargo test -p athmedia` on the dev box, gated against a reference the
implementer generates with **ffmpeg in WSL2**. Every test below can print FAIL.

### §7.0 — Generating the in-tree fixture with ffmpeg (WSL2) — exact commands

The fixture is a **tiny, single-I-frame, baseline-profile** clip plus its **ffmpeg-decoded raw
YUV reference**, both embedded as byte arrays in the test. Use a 16×16 (one macroblock) and a
32×32 (four MBs, exercises inter-MB intra prediction + a deblock edge) fixture. Resolution is
tiny so the embedded reference YUV is a few hundred bytes, not megabytes.

Run in WSL2 (`~/athenaos`, full toolchain per the dev-env memory). Generate a **deterministic
synthetic source** (a known gradient/pattern, no camera), encode one baseline I-frame, then
decode it back to raw YUV with ffmpeg as the golden reference:

```bash
# 0. tiny deterministic 16x16 source: a smooth color gradient (1 frame), raw YUV420p.
ffmpeg -f lavfi -i "testsrc2=size=16x16:rate=1" -frames:v 1 -pix_fmt yuv420p src16.yuv

# 1. encode ONE baseline-profile, CAVLC, all-intra I-frame to a raw Annex-B .h264:
#    -profile:v baseline  => no CABAC, no 8x8 transform, no B-frames (baseline forbids them)
#    -coder 0             => CAVLC (belt-and-suspenders; baseline already implies CAVLC)
#    -g 1 / -intra        => every frame is an IDR keyframe
#    -bf 0                => no B-frames
#    -qp 26               => fixed QP (deterministic, no rate control jitter)
ffmpeg -f rawvideo -pix_fmt yuv420p -s 16x16 -i src16.yuv \
  -c:v libx264 -profile:v baseline -coder 0 -bf 0 -g 1 -intra -qp 26 \
  -frames:v 1 -f h264 frame16.h264

# 2. ALSO produce it inside a real .mp4 (avcC path — what apps/video actually feeds):
ffmpeg -f rawvideo -pix_fmt yuv420p -s 16x16 -i src16.yuv \
  -c:v libx264 -profile:v baseline -coder 0 -bf 0 -g 1 -intra -qp 26 \
  -frames:v 1 -movflags +faststart frame16.mp4

# 3. GOLDEN REFERENCE: decode the encoded .h264 back to raw YUV420p with ffmpeg.
#    This is what OUR decoder must reproduce (bit-exact for intra is the goal; see tolerance).
ffmpeg -i frame16.h264 -f rawvideo -pix_fmt yuv420p frame16.ref.yuv

# 4. repeat 0-3 at 32x32 (size=32x32, -s 32x32) → frame32.{h264,mp4,ref.yuv}.

# 5. sanity: confirm the encode is actually baseline + CAVLC + 1 I-frame:
ffprobe -show_streams -select_streams v frame16.mp4 | grep -E "profile|codec"
ffprobe -show_frames -select_streams v frame16.h264 | grep pict_type   # must be "I"
```

Embed `frame16.h264` (a few hundred bytes), `frame16.mp4`, `frame16.ref.yuv`, and the 32×32
variants as `static [u8]` arrays in `h264.rs`'s test module (or a small `tests/fixtures/`
`include_bytes!`). Keep each ≤ a few KB — 16×16 and 32×32 keep the reference YUV tiny
(16×16 YUV420p = 384 bytes; 32×32 = 1536 bytes). Commit them. **Dual-source corroboration:**
ffmpeg here uses libx264 to *encode* and libavcodec to *decode* — the reference is ffmpeg's own
round-trip. For a second independent oracle, optionally also decode with openh264's `h264dec`
(BSD) and assert both references agree (they must, for intra). Document which oracle produced
the committed `.ref.yuv`.

### §7.1 — VLC generator gate (cheapest, run first)
`rustc -O tools/h264_vlc_gen/gen.rs && ./gen` MUST print `All H.264 CAVLC tables verified
prefix-free.` and exit 0; a bad transcription prints `FAIL <table>: ...` and exits 1. Proves
the coeff_token (all nC contexts + chroma-DC), total_zeros, and run_before tables are
well-formed before any decode.

### §7.2 — SPS geometry KAT (FAIL-able, the highest-value early proof)
`cargo test -p athmedia h264_sps_geometry` MUST feed the SPS NAL extracted from `frame16.mp4`'s
avcC and assert `parse_sps` returns `PicWidthInMbs==1, FrameHeightInMbs==1, display_width==16,
display_height==16` (and 32×32 → 2×2 MBs, 32×32 display). A second case with a cropped SPS (a
640×360 clip cropped from 640×368 coded) asserts the crop math. Negative case: a truncated SPS
RBSP returns `Err`, not a panic. **This single test kills the "defaults to 1920×1080" bug.**

### §7.3 — CAVLC known-block KAT (FAIL-able)
`cargo test -p athmedia h264_cavlc_block` MUST decode hand-built bitstreams: (a) the all-zero
coeff_token (`TotalCoeff==0`) → an all-zero 16-coeff block; (b) a single-trailing-one block →
the expected ±1 at the expected scan position; (c) a multi-level block with a known
`total_zeros`+`run_before` → the exact 16-entry coefficient array (computed by hand in the
test). Include a deliberately wrong bit pattern whose decode must NOT match (the FAIL lever).

### §7.4 — Inverse transform + intra prediction KATs (FAIL-able, concrete values)
- `h264_inverse_transform`: drive the §6 integer inverse 4×4 transform with a known
  dequantized block (e.g. DC-only) and assert every one of the 16 residual samples equals a
  hand-computed reference; a zero block → all-zero residual.
- `h264_intra_dc_16x16`: with a synthetic top row + left column of known reconstructed samples,
  assert DC mode fills the 16×16 block with the correct average; assert Vertical copies the top
  row, Horizontal the left column, and Plane matches a hand-computed gradient corner.
- `h264_intra_4x4_modes`: validate at least modes 0 (Vertical), 1 (Horizontal), 2 (DC) against
  in-test references over known boundaries.

### §7.5 — Deblocking KAT (FAIL-able)
`h264_deblock_edge`: construct a 4-sample edge `p1 p0 | q0 q1` straddling a known step, run the
bS=3 normal filter at a known QP, and assert the output matches the §7-clause hand computation;
assert `disable_deblocking_filter_idc==1` is an exact pass-through (identity).

### §7.6 — End-to-end keyframe KAT (the picture proof, FAIL-able) — the gate
`cargo test -p athmedia h264_decode_known_keyframe` MUST:
1. Decode the embedded `frame16.h264` (Annex-B) **and** drive the full `apps/video`-style path
   on `frame16.mp4` (avcC param sets + first sync sample) through `H264Decoder::decode`.
2. Assert the **shape**: `Ok(Some(frame))`, `pixel_format==Yuv420p`, `width==16`, `height==16`,
   `planes.len()==3`, `planes[0].data.len()==256`, `planes[1].data.len()==64`,
   `planes[2].data.len()==64`.
3. Assert **correctness vs the ffmpeg golden YUV** (`frame16.ref.yuv`): compare every Y/Cb/Cr
   sample. **Target: bit-exact** (intra H.264 reconstruction is fully specified integer math;
   ffmpeg's reference is the same spec). The committed assert: `max_abs_diff == 0` (bit-exact)
   for the primary gate. If a documented, understood rounding divergence appears (e.g. a
   deblock clip edge case), the fallback is `max_abs_diff <= 1 && rmse < 0.5` with the
   divergence noted — but **prefer and pursue bit-exact**; a >1 difference is a real bug, not
   tolerance.
4. Repeat for `frame32.*` (exercises inter-MB intra prediction + a real deblock MB edge).
5. **Negative/degradation cases (FAIL-able):** a CABAC-encoded clip
   (`ffmpeg ... -profile:v high -coder 1`) MUST return `Err`/`Ok(None)`, NOT a panic and NOT a
   wrong-shape frame; a truncated slice MUST return `Err`/`Ok(None)`; a P-frame-bearing stream's
   second packet MUST return `Ok(None)`. These prove the clean-degradation contract.

### §7.7 — R10 boot smoketest + procfs (FAIL-able)
- `h264::run_boot_smoketest()` decodes the embedded 16×16 keyframe and emits
  `[athmedia] h264-iframe: <w>x<h> mbs=<n> bitexact=<bool> -> PASS` (FAIL if decode errors, the
  frame is the wrong shape, or `max_abs_diff != 0` against the embedded reference). This is the
  serial marker to grep on QEMU and iron.
- The athmedia status/procfs line (alongside the AAC `aac=...` / MP3 `mp3=...` entries) MUST
  report `h264=iframe` (was `h264=pending`) once decode is wired.
- The `h264.rs` module docstring + `decode_slice` MUST quote the Concept promise at the top of
  this spec.

## Acceptance criteria (the exact proof)

- VLC generator prints `All H.264 CAVLC tables verified prefix-free.` exit 0 (§7.1).
- `cargo test -p athmedia` passes `h264_sps_geometry`, `h264_cavlc_block`,
  `h264_inverse_transform`, `h264_intra_dc_16x16`, `h264_intra_4x4_modes`, `h264_deblock_edge`,
  and `h264_decode_known_keyframe` (§7.2–7.6) — the last asserting **bit-exact** match to the
  ffmpeg golden YUV for both 16×16 and 32×32 fixtures, plus the clean-degradation negatives.
- Boot log MUST show `[athmedia] h264-iframe: 16x16 mbs=1 bitexact=true -> PASS` (§7.7).
- `/proc/athena` (the athmedia status line) MUST report `h264=iframe`.
- `H264Decoder::decode`, fed `apps/video`'s real param-set + first-sync-sample packets from a
  user `.mp4`, returns `Ok(Some(VideoFrame))` with real picture; `apps/video` displays it (no
  app code change). On iron, the Video app shows a real first frame of a `.mp4` (the `[x]` gate).
- The `VideoDecoder` trait and `VideoFrame`/`VideoPlane` structs are **unchanged** (diff proves
  it); no `ath_abi`/kernel touch.

## Handoff

- **Implementer: athena-media.** Pure `athmedia` userspace work over `ath_mp4`'s existing API and
  the existing `VideoDecoder` contract; no kernel/ABI/app-code touch. (A one-line cosmetic
  `apps/video` docstring/UI-string update is an optional follow-up, not part of the decode gate.)
- **Precise files touched:** new `tools/h264_vlc_gen/gen.rs`, new
  `components/athmedia/src/h264.rs`, new `components/athmedia/src/h264_tables.rs`, and a rewire
  of `components/athmedia/src/lib.rs` (`H264Decoder::process_nal` + `decode`). **Read
  `components/athmedia/src/jpeg.rs` first** — its integer IDCT, dequant, chroma subsampling,
  table-as-`const`, and never-panic error posture are the direct stylistic precedent; reuse the
  MP3/AAC bit-reader pattern.
- **Generate the fixture in WSL2** with the exact ffmpeg commands in §7.0; commit the tiny
  `.h264`/`.mp4`/`.ref.yuv` byte arrays (16×16 and 32×32) as the embedded KAT golden data.
- **Unblocks checklist lines:** the H.264 / video-playback rows under media / Phase-6 in
  `MasterChecklist.md` (`[~] decode pending` → `[~] first keyframe decoded (host/QEMU)` → `[x]`
  on Athena showing a real `.mp4` frame). Makes `apps/video` show real picture; pairs with the
  already-real AAC audio for a `.mp4` that both plays sound and shows a frame.
- **Sequencing (each step independently provable, build stays green; the app shows the honest
  placeholder until the last step, never wrong picture):**
  1. `h264_vlc_gen` + the CAVLC tables + the prefix-free gate (§7.1) + the §D.3/§D.4 const tables.
  2. RBSP/Exp-Golomb bit reader + `parse_sps`/`parse_pps`/`parse_slice_header` + the deferred-
     feature `Err` bails (CABAC/inter/High) + the SPS-geometry KAT (§7.2). *(After this step the
     decoder reports the REAL resolution even before reconstruction — immediate visible win.)*
  3. CAVLC residual decode + the known-block KAT (§7.3).
  4. Intra prediction (4×4 + 16×16 + chroma) + inverse transform + dequant + reconstruction +
     their KATs (§7.4). *(At this step the frame decodes correctly except for block edges.)*
  5. Deblocking filter + its KAT (§7.5); wire `lib.rs` to emit the real `VideoFrame`.
  6. The end-to-end bit-exact `h264_decode_known_keyframe` KAT (§7.6) + the boot smoketest →
     flip `h264=iframe`; verify `apps/video` displays a real keyframe.
  - **Defer (documented later pass, never blocking, always a clean `Err`):** CABAC, P/B inter
    prediction (the path to *motion* playback — the next milestone after this keyframe gate),
    Main/High tools (8×8 transform, scaling lists), 4:2:2/4:4:4/>8-bit, interlaced, multi-slice/
    FMO. Each is detected at parse time and bailed before any wrong output.

## Provenance / corroboration note (for the reviewer)

- **Syntax (§1–§3, §D):** ITU-T H.264 clauses cited per stage, cross-checked FFmpeg `h264_ps.c`/
  `h264_slice.c` ↔ openh264 `decode_slice.cpp`/parser. The two oracles agree on syntax order.
- **CAVLC tables (§D.1):** ITU-T Tables 9-5/9-7/9-8/9-10, to be transcribed and **prefix-free-
  verified by `tools/h264_vlc_gen`** (the first gate) and cross-checked FFmpeg `h264data`/CAVLC
  arrays ↔ openh264 `g_kp*` arrays entry-for-entry (the second gate). Any mismatch flagged.
- **Transform/dequant (§6, §D.3):** ITU-T §8.5.9/§8.5.12 integer math + `normAdjust4x4` + the
  Table 8-15 chroma-QP map, corroborated FFmpeg `ff_h264_dequant4_coeff`/`ff_h264_chroma_qp` ↔
  openh264. The integer inverse transform is fully specified (no float) → bit-exact achievable.
- **Intra prediction (§5, §D.2):** ITU-T §8.3 closed-form formulas, corroborated FFmpeg
  `h264pred.c` ↔ openh264 `get_intra_predictor.cpp`; validated by the §7.4 KATs vs in-test refs.
- **Deblocking (§7, §D.4):** ITU-T §8.7 + Tables 8-16/8-17 (α/β/tC0), corroborated FFmpeg
  `h264_loopfilter.c` ↔ openh264 `deblocking.cpp`.
- **Golden reference (§7.0):** ffmpeg encode (libx264, baseline/CAVLC) + ffmpeg decode
  (libavcodec) round-trip, optionally double-checked against openh264 `h264dec`. Bit-exact intra
  reconstruction is the spec's guarantee; the committed assert pursues `max_abs_diff == 0`.
- **OSS verdict:** openh264 (BSD-2) is the one **permissively vendorable** full decoder and the
  primary table oracle; the recommendation is still **build native** (no C dep in the userspace
  media crate, consistent with jpeg/mp3/aac). FLAG for the lead: if a hardware-free fast-path
  ever justifies vendoring a C decoder, openh264 is the license-clean choice.
- **FLAGGED (decide/verify before committing, do not guess):**
  1. **Whether to accept Main-profile-tagged CAVLC all-intra files** (some "baseline" content is
     muxed as Main but is still CAVLC/no-B). Recommend: accept if `entropy_coding_mode_flag==0`
     && no 8×8 transform && no custom scaling && I-slice; else clean `Err`. Verify against a few
     real phone `.mp4`s in WSL2.
  2. **Bit-exactness of deblocking** vs ffmpeg at clip-table edges — pursue `max_abs_diff==0`;
     if a single understood ±1 clip divergence appears, document it before relaxing the assert.
  3. **The CAVLC table transcription** is the bulk of the data and the main risk — the generator
     prefix-free gate + the known-block KAT (§7.3) + the bit-exact end-to-end KAT (§7.6) are the
     three independent gates that make any transcription error fail loudly rather than silently
     corrupt a frame.
