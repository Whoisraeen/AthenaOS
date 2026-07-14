# Spec: H.264/AVC **Baseline** intra decode — make `.mp4` video display real picture

The single biggest remaining gap in the #5 "play my movies" pillar. The container side and the
app shell already landed: `ath_mp4` resolves every H.264 sample's bytes + the `avcC` SPS/PPS, the
`apps/video` player wires demux → `H264Decoder` → YUV→ARGB → canvas, and AAC audio is genuinely
audible. The one thing that does **not** work is the picture: `athmedia::H264Decoder` is a
parse-shell that emits a flat gray YUV420 surface (the app honestly shows "Video stream demuxed —
decode pending (engine)"). This spec is the concrete, honest, **provable** plan to close that.

This is deliberately modeled on `docs/research/aac-lc-decoder.md` and
`docs/research/mp3-synthesis-and-huffman-tables.md`: split the codec into a **provable
parser/table/DSP layer** (host-KAT'd from hand-constructible inputs) and a **reference-frame layer**
(the full end-to-end "decode a real `.h264` → exact YUV" rung the host environment cannot reach
without an encoder/corpus). The honest insight up front: **most of H.264 IS host-provable in
isolation; the full-frame bit-exactness is the iron/corpus-gated follow-up — same posture the
MP3/AAC specs took (per-stage referenced, not end-to-end bit-exact in the host KAT).**

---

## Concept promise served

> "A daily driver must 'play my movies' and 'play my music.' MP4 … (the ISO Base Media File
> Format) is the dominant container for both — phone video, downloaded video, and AAC audio
> (`.m4a`/`.mp4`) all ship as BMFF."
> (LEGACY_GAMING_CONCEPT.md §creators / media — the exact line `ath_mp4/src/lib.rs` and
> `apps/video/src/lib.rs` both quote in their module docstrings; the "it just works" media pillar.
> Concept §Roadmap Year-1 also names a decoded picture as the bar: "Boots, draws, plays …".)

H.264/AVC is the codec of essentially every phone recording, every downloaded MP4, every
screen-capture, and most web video keyframes. "Play my movies" is literally a placeholder string
until the decoder reconstructs a frame. This closes the video half of the pillar that the AAC spec
closed for audio.

## Already in the tree (verify-before-implement)

Do **not** rebuild these. The decoder is the **delta** between the parse-shell and real
reconstruction.

- `components/ath_mp4/src/lib.rs` — **[x] built (host-KAT'd).** The demuxer resolves every video
  sample's absolute offset/size/dts/cts/keyframe and hands raw elementary-stream bytes via
  `Track::sample_data(data, i)`. For `Codec::H264` (fourcc `avc1`/`avc3`) `Track::codec_private` is
  the **`avcC` box payload** (AVCDecoderConfigurationRecord: version, profile/compat/level,
  `lengthSizeMinusOne`, the length-prefixed SPS list, then the PPS list). **Input shape for this
  decoder:** per video sample, one `&[u8]` of **length-prefixed** NAL units (avcC framing, NOT
  Annex-B); SPS/PPS come once from `codec_private`. This is the decoder's INPUT — do not re-demux.
- `apps/video/src/lib.rs` — **[~] shell, picture pending.** Already does the load-bearing wiring:
  `open_media` picks the H.264 + AAC tracks; `avcc_to_annexb()` converts the length-prefixed sample
  to Annex-B start-code form; `avcc_extract_param_sets()` pulls SPS(NAL 7)+PPS(NAL 8) out of `avcC`
  in Annex-B form and primes the decoder; `decode_first_video()` feeds the first keyframe and
  converts whatever surface comes back via `yuv_frame_to_argb()` → `RgbFrame{ pixels: Vec<u32> }`
  ARGB8888 → `blit_frame_fit()`. **The output contract is fixed and correct: produce a real
  YUV420p `VideoFrame` and the app already displays it.** When the engine learns reconstruction,
  `decode_first_video` needs *no change* (the app's own docstring says so).
- `components/athmedia/src/lib.rs` — **[~] parse-shell, emits gray.** This is the call site to
  replace. Specifically:
  - `H264Decoder` / `H264Sps` / `H264Pps` / `H264SliceHeader` / `H264SliceType` structs exist
    (good field shapes already: `width_mbs`, `height_mbs`, `chroma_format`, `frame_mbs_only`,
    `poc_type`, `entropy_coding_mode`, `init_qp`, `transform_8x8` …).
  - `parse_nal_units()` (lib.rs ~L1585) splits on Annex-B 3/4-byte start codes — **but does NOT
    remove emulation-prevention bytes** (`00 00 03`), so any real slice RBSP is corrupt. BUG to fix.
  - `process_nal()` (~L1611) reads **only** `profile=nal[1]`, `level=nal[3]` for SPS and
    **hardcodes everything else** (`width_mbs=0, height_mbs=0`, fixed PPS, fixed slice header). No
    Exp-Golomb, no real geometry. This is the parse-shell.
  - `produce_frame()` (~L1675) returns a `VideoFrame` of all-`0` Y + all-`128` UV (a gray frame) at
    `width_mbs*16 × height_mbs*16` (= 0×0 today, so the app falls to "decode pending").
  - `VideoDecoder::decode()` (~L1720) returns that frame whenever a slice NAL was seen.
- `components/athmedia/src/lib.rs::PixelConverter::yuv420_to_rgb()` (~L3903) — **[x] built.** The
  exact YUV(4:2:0)→RGB the app's `yuv_frame_to_argb` consumes (BT.601/709 coefficients, per-sample
  `.get().unwrap_or`-bounded). **Reuse verbatim — do NOT write a second color converter.** The
  ARGB8888 `Vec<u32>` output model is the same one the image decoders (`png.rs`/`jpeg.rs`) feed the
  canvas with.
- `components/athmedia/src/mp3_dsp.rs` / `aac` no-libm DSP precedent — **[x] built.** The
  no-libm/`#![no_std]`/soft-float discipline (no `libm`, integer or host-precomputed-table math)
  is established. H.264 baseline is *easier* here: the core transforms are **integer** (no
  trig/IMDCT at all), so most of the DSP is exact integer arithmetic — no table-build trig needed.

Status to flip when done: the H.264 / video rows under media / Phase-7 (and the Year-1 "draws a
decoded frame" line) in `MasterChecklist.md` from `[~] demux-only / gray surface` →
`[~] intra picture (host KAT)` → `[x]` once iron displays a real `.mp4` keyframe.

## Prior art & OSS verdict

Every numeric table below is **spec-defined (ITU-T H.264 / ISO 14496-10)** and corroborated across
≥2 independent open decoders. None of these projects is vendored or linked — they are read-only
spec oracles; AthenaOS keeps its own `#![no_std]`, no-libm, integer-transform decoder.

- **ITU-T H.264 (= ISO/IEC 14496-10) — the normative source.** §7.3.2.1 (SPS syntax), §7.3.2.2
  (PPS), §7.3.3 (slice header), §7.3.5 (macroblock layer), §8.3 (intra prediction), §8.5 (transform
  + inverse-quant), §8.7 (deblocking), §9.1 (Exp-Golomb), **§9.2 (CAVLC residual + the
  `coeff_token` / `total_zeros` / `run_before` tables)**. 📖 normative reference, not code.
- **openh264 (Cisco, BSD-2-Clause)** — `codec/decoder/core/` (`decode_slice.cpp`,
  `parse_mb_syn_cavlc.cpp`, `get_intra_predictor.cpp`, `deblocking.cpp`) + `codec/common/` CAVLC
  tables. **➕ permissively licensed (BSD) — the preferred corroboration oracle; values may be
  re-derived/cross-checked freely.** Still NOT vendored: we transcribe the spec tables and check
  them against openh264; the Rust is original. (If a vendoring decision is ever made, openh264's
  BSD license makes it the only candidate here — but that is a separate, human-gated call, and the
  decoder this spec describes is from-scratch Rust.)
- **FFmpeg `libavcodec/h264*.{c,h}`** (LGPL) — `h264_cavlc.c` (the canonical
  `coeff_token`/`total_zeros`/`run_before` VLC tables + `decode_residual`), `h264_parser`,
  `h264_intra_pred`, `h264dsp.c` (the 4×4 IDCT + Hadamard), `h264_loopfilter.c`. 📖 **study/isolate
  (LGPL)** — used only as the source-of-numbers cross-check; no code copied/linked.
- **Concept §R7 (no Linux-clone lineage):** satisfied — H.264 is an ITU/ISO codec, not a Linux
  subsystem; the implementation is original Rust over ITU data tables (no DRM/KMS, no Linux media
  framework involvement). The decoder is pure userspace `athmedia`, no kernel/ABI surface.

**Corroboration gate:** the CAVLC VLC tables and the inverse-quant `LevelScale`/`Vmat` constants in
§4 are transcribed from ITU-T H.264 §8/§9 and **cross-checked entry-for-entry against both openh264
(BSD) and FFmpeg `h264_cavlc.c` (LGPL)** before use. Any mismatch is flagged, not guessed — exactly
the discipline the MP3/AAC specs used.

---

## §1 — THE SCOPE DECISION (be honest about the size)

**Target: H.264 Baseline-profile, intra-only first, then P.** Concretely the foundation slice
covers:

| In scope (Baseline) | Out of scope (Main/High follow-up) |
|---|---|
| **CAVLC** entropy coding (§9.2) | **CABAC** (§9.3) — arithmetic coder, ~Main/High only |
| **I-slices** (Intra_4x4 / Intra_16x16 / Intra_chroma) first | **B-slices** / bi-prediction |
| Then **P-slices** (16×16…4×4 inter, single past ref) | Weighted prediction, multiple ref lists |
| **4×4 integer transform** + the 16×16/chroma Hadamard | **8×8 transform** (High profile) |
| **4:2:0**, 8-bit, progressive (`frame_mbs_only_flag=1`) | Interlace (PAFF/MBAFF), 4:2:2/4:4:4, >8-bit |
| In-loop **deblocking** filter | (deblocking is shared — implement once) |
| POC type 0/2 frame ordering | POC type 1, long-term refs, MMCO |

**Why Baseline is the tractable, high-coverage target:** Baseline is exactly the
"no-B-frames, no-CABAC, no-8×8" subset. It decodes:
- **every keyframe (IDR) of essentially every H.264 file** — IDR slices are I-slices using only
  intra prediction + CAVLC-or-CABAC residual; the I-slice intra path here is profile-independent
  except for the entropy coder. (A High-profile file's keyframe still needs CABAC + possibly 8×8, so
  "keyframe of any file" requires the CABAC follow-up; **Baseline/Constrained-Baseline files decode
  fully**, and that is a large real corpus: older recordings, many web/streaming ladders' base
  layer, WhatsApp/many phone exports, screen recorders, game capture defaults.)
- The honest framing for the app: **step 1 (I-only) makes the first keyframe of a Baseline file
  display a real picture** (the exact thing `decode_first_video` asks for). **Step 2 (P-slices)**
  makes it play through. **The Main/High follow-up (CABAC + B + 8×8) is a comparably large second
  effort** — CABAC alone is a multi-week subsystem (context models, the arithmetic engine, the
  binarization tables). Do not let anyone believe "H.264 done" after Baseline; say "Baseline intro
  + progressive 4:2:0, the common-file path."

**Honesty line for the REPORT and the MasterChecklist:** full H.264 is a large, multi-slice,
multi-profile effort. This spec scopes the **provable foundation + the common-file Baseline path**;
CABAC/B/8×8/interlace/HDR are explicitly deferred follow-ups, each its own spec.

---

## §2 — THE PIPELINE (NAL → macroblocks → pixels → ARGB)

The decode flow, each stage cross-referenced to the existing shell it replaces:

```
avcC sample bytes (length-prefixed NAL)            ← ath_mp4 Track::sample_data
  │  (or Annex-B from apps/video::avcc_to_annexb)
  ▼
[A] NAL framing + emulation-prevention removal      → §2.1  (fix parse_nal_units)
  ▼  RBSP bytes per NAL, tagged by nal_unit_type
[B] Exp-Golomb bit reader (ue/se/u(n))              → §2.2  (NEW: the load-bearing primitive)
  ▼
[C] SPS parse  (geometry, profile, cropping)        → §2.3  (replace process_nal SPS stub)
[D] PPS parse  (entropy mode, qp, deblock)          → §2.4  (replace process_nal PPS stub)
  ▼
[E] Slice header parse (type, frame_num, qp, refs)  → §2.5  (replace slice stub)
  ▼  per macroblock, raster order:
[F] Macroblock layer:                               → §2.6
      mb_type → Intra_4x4 / Intra_16x16 / Intra_chroma  (I)   (P: §2.10 inter)
      [G] CAVLC residual decode (coeff_token / level / run_before / total_zeros)  → §4
      [H] inverse-quant + 4×4 integer IDCT (+ Hadamard for DC)  → §2.7
      [I] intra prediction reconstruction (pred + residual → recon)  → §2.8
  ▼  whole picture reconstructed
[J] in-loop DEBLOCKING filter (boundary strength + edge filter)  → §2.9
  ▼  YUV420p planes in VideoFrame
[K] YUV(4:2:0) → RGB → ARGB8888 Vec<u32>            → reuse PixelConverter::yuv420_to_rgb
  ▼
apps/video blit  (already done)
```

### §2.1 — NAL unit parsing + emulation-prevention removal
- **Two framings, both already handled at the boundary:** Annex-B (start codes `00 00 01` /
  `00 00 00 01`) — `apps/video::avcc_to_annexb` already converts the avcC length-prefixed sample to
  this, and `parse_nal_units` already scans for it. The decoder can also accept length-prefixed
  directly (cleaner; `lengthSizeMinusOne+1` byte prefix from `avcC[4]&3`). Pick **one** internal
  framing; the existing Annex-B path is fine for v1 since the app feeds it.
- **Emulation-prevention byte removal (the BUG fix):** within a NAL, the byte sequence
  `00 00 03` has the `03` (emulation_prevention_three_byte) **removed** to recover the RBSP — i.e.
  `00 00 03 xx` (xx ∈ {00,01,02,03}) → `00 00 xx`. The current `parse_nal_units` does not do this,
  so every real RBSP is corrupted at the first `00 00 03`. **Implement RBSP extraction as a
  separate pass that yields a clean `Vec<u8>` RBSP per NAL** before the bit reader touches it.
- `nal_unit_type = byte0 & 0x1F`; `nal_ref_idc = (byte0 >> 5) & 3`. Types this slice cares about:
  1 (non-IDR slice), 5 (IDR slice), 7 (SPS), 8 (PPS), 9 (AUD, skip), 6 (SEI, skip), 13/15 (subset
  SPS, skip for Baseline). Unknown/reserved → skip cleanly.

### §2.2 — Exp-Golomb bit reader (the missing primitive)
Everything from SPS onward is bit-packed with Exp-Golomb (§9.1). NEW `BitReader` over the RBSP:
- `u(n)`: read n bits MSB-first.
- `ue(v)` (unsigned Exp-Golomb): count leading zeros `L`; read `L` more bits as `info`;
  `value = 2^L − 1 + info`.
- `se(v)` (signed): decode `ue` → `k`; `value = (−1)^(k+1) · ceil(k/2)` (i.e. 0→0, 1→+1, 2→−1,
  3→+2, 4→−2 …).
- `more_rbsp_data()` / byte-alignment for trailing bits.
- **Hostile-input discipline:** every read past the RBSP end returns 0/`Err` (never panics); every
  count derived from the stream (mb count, `total_coeff`, `run_before`) is clamped to its syntactic
  max before use. Parsers are the #1 RCE surface — match `ath_mp4`'s posture exactly.

### §2.3 — SPS parse (§7.3.2.1) — the geometry the shell hardcodes to 0
Replace the `profile=nal[1]` stub with a real `ue`/`se` parse. Fields that matter for Baseline:
- `profile_idc` (u8), `constraint_set*`, `level_idc` (u8). (Baseline=66, Constrained-Baseline =
  66 + constraint_set1. Main=77, High=100 → flag as "needs CABAC/8×8 follow-up", decode SPS but
  refuse the slice cleanly.)
- `seq_parameter_set_id = ue`.
- For High-family (≥100) only: `chroma_format_idc = ue` (1 = 4:2:0; reject ≠1 for v1),
  `bit_depth_luma/chroma = ue+8` (reject ≠8), scaling lists (skip). **Baseline has none of these →
  chroma_format = 1 (4:2:0) implied.**
- `log2_max_frame_num_minus4 = ue` → `log2_max_frame_num`.
- `pic_order_cnt_type = ue` (handle 0 and 2; 1 = defer). For type 0:
  `log2_max_pic_order_cnt_lsb_minus4 = ue`.
- `max_num_ref_frames = ue`; `gaps_in_frame_num_value_allowed_flag = u(1)`.
- **`pic_width_in_mbs_minus1 = ue`** → `width_mbs = +1`. **`pic_height_in_map_units_minus1 = ue`**.
- `frame_mbs_only_flag = u(1)` (Baseline target: 1). `height_mbs = (2 − frame_mbs_only) ·
  (pic_height_in_map_units_minus1 + 1)`. (For v1 reject `frame_mbs_only_flag==0` interlace.)
- `direct_8x8_inference_flag = u(1)`.
- **`frame_cropping_flag = u(1)`** + (if set) `crop_left/right/top/bottom = ue` (in chroma-subsample
  units). **Crucial for correct dimensions** — e.g. a 1920×1080 video is coded as 1920×1088 (68 MB
  rows) and cropped 8px; without cropping the output is the wrong size. Final luma:
  `width  = 16·width_mbs  − cropUnitX·(crop_left+crop_right)`,
  `height = 16·height_mbs − cropUnitY·(crop_top+crop_bottom)` (cropUnitX=2, cropUnitY=2 for 4:2:0
  progressive).
- `vui_parameters_present_flag` → VUI (skip for v1; color info can refine the converter later).
- **Store into the existing `H264Sps` fields** (`width_mbs`, `height_mbs`, `chroma_format`,
  `frame_mbs_only`, `poc_type`, `log2_max_frame_num`, `log2_max_poc_lsb`, …) — the struct already
  has the right shape; only the *parse* is missing.

### §2.4 — PPS parse (§7.3.2.2) — replace the hardcoded struct
- `pic_parameter_set_id = ue`, `seq_parameter_set_id = ue`.
- **`entropy_coding_mode_flag = u(1)`** (0 = CAVLC = Baseline target; **1 = CABAC → refuse cleanly
  with a "CABAC not supported" path**, do not pretend). The shell currently hardcodes this `true`
  (CABAC) — wrong.
- `bottom_field_pic_order_in_frame_present_flag = u(1)`.
- `num_slice_groups_minus1 = ue` (Baseline: 0; FMO slice groups = defer).
- `num_ref_idx_l0/l1_default_active_minus1 = ue`.
- `weighted_pred_flag = u(1)`, `weighted_bipred_idc = u(2)` (Baseline: off).
- **`pic_init_qp_minus26 = se`** → `init_qp = 26 + value` (the base QP). `pic_init_qs_minus26 = se`.
- **`chroma_qp_index_offset = se`**.
- `deblocking_filter_control_present_flag = u(1)`, `constrained_intra_pred_flag = u(1)`,
  `redundant_pic_cnt_present_flag = u(1)`.
- Optional trailing High-profile extension (`transform_8x8_mode_flag`, scaling lists) — Baseline has
  none; if present (High file) → already on the refuse path.

### §2.5 — Slice header parse (§7.3.3)
- `first_mb_in_slice = ue`, `slice_type = ue` (mod 5: 0=P,1=B,2=I,3=SP,4=SI — Baseline I/P only;
  B/SP/SI → refuse), `pic_parameter_set_id = ue`.
- `frame_num = u(log2_max_frame_num)`.
- (IDR only:) `idr_pic_id = ue`. POC type 0: `pic_order_cnt_lsb = u(log2_max_poc_lsb)`.
- P-slice: `num_ref_idx_active_override_flag`, ref-pic-list reordering, etc. (§2.10).
- `slice_qp_delta = se` → `SliceQP = 26 + pic_init_qp_minus26 + slice_qp_delta`.
- If `deblocking_filter_control_present`: `disable_deblocking_filter_idc = ue`, `slice_alpha/beta
  offsets = se`.

### §2.6 — Macroblock layer (§7.3.5), I-slice path first
Per MB in raster order (`PicWidthInMbs × PicHeightInMbs`):
- `mb_type = ue`. For I-slices the mb_type maps to:
  - **I_NxN (mb_type 0)** = Intra_4x4 (or Intra_8x8 in High — not Baseline). 16 luma 4×4 blocks,
    each with a prediction mode.
  - **I_16x16 (mb_type 1..24)** encodes (predMode 0..3, chroma-pred-mode, CBP-luma, CBP-chroma) in
    the mb_type value per Table 7-11. One whole-MB luma prediction + a 4×4 Hadamard of the 16 DCs.
  - **I_PCM (mb_type 25)** = raw uncompressed samples (rare; handle: byte-align, copy 256+64+64
    bytes). Cheap to support and a clean KAT.
- **Intra_4x4 pred-mode signalling:** for each of 16 blocks: `prev_intra4x4_pred_mode_flag = u(1)`;
  if 0, `rem_intra4x4_pred_mode = u(3)`. The actual mode derives from the min of the two neighbour
  blocks' modes (§8.3.1.1). Then `intra_chroma_pred_mode = ue` (0..3).
- `coded_block_pattern = ue`-mapped (the `me(v)` mapping, Table 9-4, intra vs inter columns) →
  which of the 4 luma 8×8 regions + chroma have residual.
- `mb_qp_delta = se` (when CBP≠0 or I_16x16) → running QP.
- Then **residual()** (§2.7/§4) per the CBP.

### §2.7 — Inverse quant + the 4×4 integer transform (§8.5) — **pure integer, no trig**
This is the part that is *easier* than MP3/AAC: H.264's transform is a fixed **integer**
approximation of the DCT — no cosine tables, no IMDCT, exact arithmetic.
- **Inverse scan:** zig-zag the decoded coefficient list back into a 4×4 block (the §8.5.6 inverse
  scan order, a fixed 16-entry permutation).
- **Inverse quant (§8.5.9):** `LevelScale(qP%6, i, j)` from the `V` matrix (a 6×3 table of
  {10,16,13}/{11,18,14,…} constants — §4.3) and `qP/6` shift:
  `d[i][j] = (c[i][j] · LevelScale(qP%6,i,j)) << (qP/6)` for AC; DC handled via the Hadamard path.
- **Inverse 4×4 transform (§8.5.12.2):** the butterfly
  ```
  e0=d0+d2; e1=d0−d2; e2=(d1>>1)−d3; e3=d1+(d3>>1);
  f0=e0+e3; f1=e1+e2; f2=e1−e2; f3=e0−e3;   // applied to rows then columns
  residual = (f + 32) >> 6;
  ```
  (all `i32`). This is the canonical core-transform butterfly; no division, no float.
- **I_16x16 / chroma DC:** the 16 (luma) / 4 (chroma) DC coefficients get a **4×4 (luma) /
  2×2 (chroma) Hadamard** transform (§8.5.10) before being distributed back; pure adds/subtracts.

### §2.8 — Intra prediction reconstruction (§8.3)
Predict each block from already-reconstructed neighbour samples (left column + top row + top-left),
then add the §2.7 residual:
- **Intra_4x4 (§8.3.1):** 9 modes — 0 Vertical, 1 Horizontal, 2 DC, 3 Diagonal-Down-Left, 4
  Diagonal-Down-Right, 5 Vertical-Right, 6 Horizontal-Down, 7 Vertical-Left, 8 Horizontal-Up. Each
  is a small fixed-weight average of neighbours (e.g. DDL uses `(a+2b+c+2)>>2` taps). Availability of
  neighbours (edge of picture / `constrained_intra_pred`) gates which modes/samples are usable.
- **Intra_16x16 (§8.3.2):** 4 modes — Vertical, Horizontal, DC, Plane (the plane mode is a
  linear-gradient fit: `a,b,c` from edge sums, `pred = Clip((a + b·(x−7) + c·(y−7) + 16)>>5)`).
- **Intra_chroma (§8.3.3):** 4 modes (DC, Horizontal, Vertical, Plane) over the 8×8 chroma block.
- `recon = Clip1(pred + residual)` (clip to 0..255). Write into the YUV420p planes; these recon
  samples become neighbours for the next block — **reconstruction order matters** (raster MB,
  raster 4×4 within the §8.3.1 scan order).

### §2.9 — In-loop deblocking filter (§8.7)
Applied to the *reconstructed* picture, on 4×4 (and MB) edges, vertical edges first then horizontal:
- **Boundary strength (bS) 0..4** per edge segment (§8.7.2.1): bS=4 at MB edges where either side is
  intra; bS=3 intra inside MB; bS=2 if either side has nonzero coeffs; bS=1 for mv/ref differences
  (P); bS=0 = skip.
- **Thresholds** `α(indexA)`, `β(indexB)` from the §8.7 Table 8-16 (indexed by `qP_av + offset`),
  and `tC0` from Table 8-17 (indexed by indexA, bS).
- **Filter** the up-to-3 samples either side per the §8.7.2.3/.4 equations (the bS<4 "normal" filter
  and the bS=4 "strong" filter for the luma MB edge), all integer with clamps.
- Deblocking is **shared across Baseline/Main/High** — implement once. It is the most fiddly DSP
  block but fully spec-deterministic and host-KAT-able edge-by-edge.

### §2.10 — P-slice inter prediction (step 2, after I works)
- mb_type → partition shape (16×16, 16×8, 8×16, 8×8 → sub-8×8). `ref_idx` (single past frame for
  Baseline), `mvd` (`se`), motion-vector prediction (median of neighbours, §8.4.1.3).
- **Quarter-pel luma interpolation (§8.4.2.2.1):** the 6-tap `(1,−5,20,20,−5,1)` half-pel filter +
  bilinear quarter-pel; chroma is 1/8-pel bilinear. Integer math with rounding/clamp.
- Add residual (same §2.7 path) → recon. Requires a 1-entry DPB (the previous decoded frame). POC
  type 0/2 ordering for display. **This is what makes the file *play* past the first keyframe.**

### §2.11 — Output to ARGB (reuse, no new code)
The reconstructed YUV420p planes go into the existing `VideoFrame { planes: [Y,U,V] }`; the app's
`yuv_frame_to_argb` already calls `PixelConverter::yuv420_to_rgb` → `Vec<u32>` ARGB8888. **No new
output path.** The only `produce_frame` change is filling the planes with real recon samples instead
of `0`/`128`, and reporting the **cropped** width/height (§2.3) so the app letterboxes correctly.

---

## §3 — THE PROVABILITY STRATEGY (the critical part — why this is honest)

An H.264 decoder is **hard to prove end-to-end in the host KAT** for the same reason MP3/AAC were:
the bit-exact rung needs a *reference frame* — a known `.h264`/`.mp4` and the exact YUV a reference
decoder produces — and the host environment **has no web and no H.264 encoder** to manufacture one.
So the spec deliberately **splits the decoder into layers that ARE provable from hand-constructible
inputs**, and flags the full-frame bit-exactness as the iron/corpus-gated follow-up.

### §3.1 — PROVABLE NOW (host KAT, hand-built inputs, FAIL-able) — the foundation slice
Each of these takes a tiny hand-authored input and asserts an exact output. No reference decoder, no
corpus, no encoder needed:

| Layer | Hand-built input | Exact assertion |
|---|---|---|
| **NAL split + emul-prevention** | a byte buffer with start codes + an embedded `00 00 03 01` | exact RBSP bytes out (the `03` removed), exact NAL-type list |
| **Exp-Golomb reader** | known bit patterns (`010` etc.) | exact `ue`/`se`/`u(n)` values (table of codeword→value) |
| **SPS field parse** | a hand-built SPS RBSP (known width/height/profile/crop) | exact `width_mbs`, `height_mbs`, cropped W×H, `profile_idc`, `poc_type` |
| **PPS field parse** | a hand-built PPS RBSP | exact `entropy_coding_mode`, `init_qp`, `chroma_qp_offset`, deblock flag |
| **CAVLC VLC tables** | a known codeword (e.g. `coeff_token` for total_coeff=1,t1=0) | exact decoded `(total_coeff, trailing_ones)`; same for `total_zeros`, `run_before` |
| **4×4 integer transform** | a known coefficient block | exact residual block (compare to an in-test naive reference applying the §8.5.12 butterfly) |
| **Inverse quant** | known `(coeff, qP)` | exact `d[i][j]` vs the §8.5.9 formula computed in-test |
| **Intra prediction modes** | known neighbour samples + mode id | exact predicted 4×4/16×16 block (e.g. Vertical copies the top row; DC = rounded average) |
| **Deblock filter** | a known edge (two sample columns) + bS + qP | exact filtered samples vs the §8.7 equations computed in-test |
| **YUV→ARGB** | already host-KAT-able (PixelConverter) | exact ARGB for known YUV |

**The pattern is identical to the MP3 synthesis KAT** ("compare against an in-test reference computed
from the same constants"): the transform/intra/deblock KATs each carry their own naive reference in
the test, so a wrong butterfly / wrong tap / wrong threshold fails immediately. The VLC KATs assert
known codeword→value pairs (the AAC "known codeword anchor" pattern). **This is real, provable
progress that lands before any reference frame exists.**

### §3.2 — NOT REACHABLE HERE (the iron/corpus-gated rung)
- **Full end-to-end decode of a real `.h264`/`.mp4` → exact YUV.** Needs a committed reference
  fixture: a short Baseline clip + the exact expected YUV (precomputed with openh264/FFmpeg on a
  machine that has them). This is the AAC/MP3 "end-to-end reference match" rung — flagged there too
  as the rung the pure-logic host can't synthesize. **Land it as a follow-up** once a fixture is
  produced off-box (the dev box / Athena-Linux live-USB can run FFmpeg to mint the expected array;
  same posture as the Mesa firmware-blob capture being an owner/off-box action). Until then the proof
  is the per-stage KATs (§3.1) + the **visual** proof on iron (a real keyframe renders).
- **Bit-exact conformance vs the JVT test vectors** (the full ITU conformance suite) — the eventual
  gold standard; a large committed corpus, deferred.

### §3.3 — The honest claim ladder (what each status means)
- `[~] intra picture (host KAT)` — §3.1 layers all pass their FAIL-able KATs **and** a hand-built
  minimal I-slice (one MB, known coeffs) reconstructs to the exact expected block. This is
  genuine, but it is *per-stage referenced*, not end-to-end bit-exact — **state that plainly**,
  exactly as the MP3/AAC specs state it.
- `[~] real clip decodes (QEMU/host)` — the committed reference fixture (§3.2) matches within
  tolerance.
- `[x]` — iron displays a real `.mp4` keyframe (the visual proof) AND the reference KAT passes.

---

## §4 — THE VLC / TABLE DATA (CAVLC) — and the follow-up data-spec

CAVLC (§9.2) is the entropy layer; it needs four spec-defined VLC table families plus the
inverse-quant constants. These are **defined in ITU-T H.264 §8/§9** and reproduced in openh264 (BSD)
+ FFmpeg `h264_cavlc.c` (LGPL). Like the MP3/AAC Huffman books, the implementer **has no web** —
so a follow-up data-spec (`docs/research/h264-cavlc-tables.md`, modeled on the MP3/AAC table specs)
must **inline + corroborate** every table. This spec identifies them; it does not yet transcribe
them (that is the data-spec's job, with the same prefix-free/dimension generator gate).

### §4.1 — `coeff_token` (§9.2.1, Tables 9-5)
Maps a VLC codeword → `(TotalCoeff 0..16, TrailingOnes 0..3)`. **Context-selected** by `nC` (the
predicted number of nonzero coeffs from the left+top blocks), which picks one of **4 tables**
(`0≤nC<2`, `2≤nC<4`, `4≤nC<8`, `8≤nC` = a fixed 6-bit FLC) — plus separate chroma-DC tables. This is
the largest table family (~each table is 17×4 entries). Source: ITU Table 9-5; cross-check openh264
`g_kuiVlcCoeffToken*` ↔ FFmpeg `coeff_token_vlc`.

### §4.2 — level + `total_zeros` + `run_before`
- **level (`level_prefix`/`level_suffix`, §9.2.2):** unary `level_prefix` (count leading ones) +
  a `suffixLength`-bit suffix, with the adaptive `suffixLength` increment rule — mostly an algorithm,
  one small constant table. Decodes each nonzero coefficient's signed level.
- **`total_zeros` (§9.2.3, Tables 9-7/9-8/9-9):** VLC → number of zeros before the last coeff,
  context-selected by `TotalCoeff` (15 luma tables + chroma-DC tables).
- **`run_before` (§9.2.3, Table 9-10):** VLC → run of zeros before each coeff, context-selected by
  `zerosLeft` (7 tables).
Source: ITU Tables 9-7..9-10; cross-check openh264 `g_kuiTotalZeros*`/`g_kuiZeroLeft*` ↔ FFmpeg
`total_zeros_vlc`/`run_vlc`.

### §4.3 — inverse-quant constants (§8.5.9)
- The **`LevelScale` `V` matrix** (6 rows for `qP%6` × the {pos-(0,0), pos-(2,0)/(0,2)/(2,2), other}
  three-class column): values `{10,16,13},{11,18,14},{13,20,16},{14,23,18},{16,25,20},{18,29,23}`.
- The chroma-QP mapping table `qPc(qPi)` (§8.5.8, the `QPc` Table 8-15 for qPi 30..51).
These are tiny fixed tables; transcribe + cross-check.

### §4.4 — the follow-up data-spec
`docs/research/h264-cavlc-tables.md` (NEXT spec, athena-researcher): inline all of §4.1–§4.3 verbatim
from ITU, corroborated openh264 ↔ FFmpeg entry-for-entry, in a **generator-checkable row form**
(codeword,len → value tuple) so a `tools/h264_vlc_gen` can assert prefix-freeness + dimension =
table size before emit — exactly the MP3/AAC `*_huff_gen` discipline. **Flag now:** the implementer
must not transcribe these from memory; they come from the data-spec.

---

## §5 — Interface needs (NEEDS-INTERFACE)

**None.** This is entirely inside the `athmedia` userspace crate (and an optional new `ath_h264`
crate). No new syscall, no `ath_abi` change, no kernel ABI surface. The decoder already returns
`VideoFrame` through the existing `VideoDecoder` trait; only the *contents* go from gray to real
picture, and `apps/video` consumes it unchanged. (If a future GPU-decode path is wanted, *that*
would need an interface — out of scope here; this is the CPU reference decoder.)

---

## §6 — File-by-file plan

**Recommended structure: a new `components/ath_h264` crate** (so the decoder is independently
host-testable like `ath_mp4`, and `athmedia` depends on it), OR extend `athmedia` in-place. New
crate is cleaner for the KAT split and keeps `athmedia/src/lib.rs` (already ~4k lines) from growing;
but extending `athmedia` avoids a new Cargo member. **Decision: new `ath_h264` crate** — matches the
`ath_mp4` precedent (per-codec crate, `#![cfg_attr(not(test), no_std)]`, `forbid(unsafe_code)`,
host-KAT module at the bottom), and `athmedia::H264Decoder` becomes a thin adapter over it.

- `components/ath_h264/src/lib.rs` (NEW) — `BitReader` (Exp-Golomb §2.2), `nal.rs` RBSP extraction +
  emul-prevention (§2.1), `sps.rs`/`pps.rs`/`slice.rs` parsers (§2.3–2.5), `macroblock.rs` (§2.6),
  `cavlc.rs` residual (§4, consuming the data-spec tables), `transform.rs` (§2.7 integer
  IDCT/Hadamard + inverse-quant), `intra.rs` (§2.8), `deblock.rs` (§2.9), `inter.rs` (§2.10, step 2).
  Public API: `H264Decoder::new()` / `decode_nal(&[u8]) -> Result<Option<Frame>, _>` /
  `Frame { width, height, y, u, v }` (cropped dims, YUV420p).
- `components/ath_h264/src/tables/` — the CAVLC + inverse-quant tables emitted by the §4.4 data-spec
  generator (do NOT hand-type).
- `components/athmedia/src/lib.rs` — replace the `H264Decoder` internals (`parse_nal_units`,
  `process_nal`, `produce_frame`) with a thin adapter: feed NALs to `ath_h264`, wrap its
  `Frame` into the existing `VideoFrame { planes }`. **Keep the `VideoDecoder` trait + `VideoFrame`
  shape unchanged** (the app depends on them). Report cropped width/height.
- `apps/video/src/lib.rs` — **no change** (the wiring already consumes a real `VideoFrame`; verify
  `decode_first_video` now returns `Ok(Some(_))` with real pixels and the "decode pending" branch
  stops firing for Baseline files).
- `tools/h264_vlc_gen/gen.rs` (NEW, with the §4.4 data-spec) — prefix-free/dimension generator.

## §7 — Acceptance criteria (the exact proof)

**Foundation slice (§3.1) — each a FAIL-able host KAT (`cargo test -p ath_h264`):**
- `h264_nal_strips_emulation_prevention` — input with `00 00 03 01` → RBSP has `00 00 01`; wrong
  output fails. Asserts the NAL-type list too.
- `h264_exp_golomb_known_codewords` — table of `(bits → ue/se value)`; includes a negative case.
- `h264_sps_parses_known_geometry` — hand-built SPS (1280×720, profile 66, crop 0) → exact
  `width_mbs=80, height_mbs=45`, cropped `1280×720`, `profile_idc=66`; a second SPS with cropping
  asserts the cropped dims differ from `16·mbs`.
- `h264_pps_parses_known_fields` — exact `entropy_coding_mode=false`, `init_qp`, `chroma_qp_offset`.
- `h264_cavlc_coeff_token_known` / `_total_zeros_known` / `_run_before_known` — known codeword →
  exact `(total_coeff, trailing_ones)` / zeros / run (one per table family, incl. a chroma-DC case).
- `h264_inverse_transform_known_block` — known coeff block → exact residual vs the in-test naive
  §8.5.12 reference (tolerance 0; integer-exact). `_inverse_quant_known` likewise.
- `h264_intra_pred_modes` — Vertical mode copies the top row exactly; DC = exact rounded average;
  DDL uses the exact `(a+2b+c+2)>>2` taps — each asserted against the in-test formula.
- `h264_deblock_known_edge` — known edge + bS + qP → exact filtered samples vs the §8.7 equations
  (incl. a bS=0 "no change" case as the FAIL lever).
- `h264_decode_minimal_i_mb` — a hand-built minimal I-slice (1 MB, known coeffs, known intra mode)
  reconstructs to the **exact** expected 16×16 luma + 8×8 chroma block (the per-stage end-to-end
  for one MB — the strongest provable-here assertion).

**Reference-frame rung (§3.2 — follow-up, gated):**
- `h264_decode_reference_clip` — decode a committed short Baseline `.mp4`/`.h264` fixture; assert the
  decoded YUV matches the committed expected YUV (minted off-box with openh264/FFmpeg) within a
  small per-sample tolerance (intra is exact; allow ±1 for any clamp-order ambiguity). **This is the
  single assert that proves "correct picture," not just "correct stage"** — flagged not reachable
  until the fixture is produced off-box.

**Boot/runtime markers:**
- `run_boot_smoketest()` for the H.264 path MUST emit
  `[ath_h264] sps=<WxH> idct=ok intra=ok deblock=ok cavlc=ok -> PASS` (FAIL if the minimal-I-MB
  recon does not match the embedded expected block). The smoketest must be able to print FAIL.
- `/proc/athena/media` MUST report `h264=intra` (was `h264=demux-only`) once §3.1 lands;
  `h264=playing` once P-slices (§2.10) land.
- Docstring on `ath_h264::lib` MUST quote the Concept promise above (R10).
- **Visual proof (the `[x]` lever):** on iron, `apps/video` displays a real first keyframe of a
  committed Baseline `.mp4` (the "decode pending" placeholder no longer shows).

## §8 — Handoff

- **Implementer: athena-media.** Pure userspace (`ath_h264` + the `athmedia` adapter); no kernel/ABI
  touch. Sequencing below keeps the build green and the app's honest "decode pending" behavior until
  real pixels exist.
- **Foundation slice (land this first, fully provable):** in order, each its own FAIL-able KAT —
  (1) NAL/emul-prevention + Exp-Golomb reader; (2) SPS + PPS parse (fix the `width_mbs=0` bug — this
  alone makes the app report correct dimensions); (3) the §4.4 CAVLC data-spec + `h264_vlc_gen` +
  the VLC KATs; (4) the integer transform + inverse-quant; (5) intra prediction; (6) deblocking;
  (7) the minimal-I-MB end-to-end KAT; (8) wire the `athmedia` adapter + flip `h264=intra`. **Each
  step is independently provable before the next — real progress lands at every step.**
- **Then (step 2):** P-slice inter prediction (§2.10) + 1-frame DPB → `h264=playing` (the file
  *plays*, not just shows a keyframe).
- **Gated follow-ups (each its own spec):** the §3.2 reference-clip fixture (off-box mint);
  **CABAC** (the big one — Main/High keyframes); **B-slices** + **8×8 transform** + interlace +
  4:2:2/4:4:4/>8-bit + HDR. Do not let "Baseline done" read as "H.264 done."
- **Prerequisite spec:** `docs/research/h264-cavlc-tables.md` (the §4.4 data-spec) should be written
  next so the implementer has the inlined, corroborated VLC tables (no web).
- **Unblocks checklist lines:** the H.264 / video rows under media / Phase-7 + the Year-1 "draws a
  decoded frame" line in `MasterChecklist.md`; turns `apps/video` from "audio + demux only" into a
  real movie player.

---

### Provenance / corroboration note (for the reviewer)
- All syntax/algorithm references are to **ITU-T H.264 (ISO/IEC 14496-10)** by section number.
  The CAVLC VLC tables (§4) and inverse-quant constants are spec-defined and will be inlined +
  cross-checked **openh264 (BSD, vendorable-if-ever-chosen) ↔ FFmpeg `h264_cavlc.c` (LGPL, study)**
  entry-for-entry in the §4.4 data-spec before any code uses them.
- **The deliberate honesty (the load-bearing point):** like the MP3 and AAC specs, the host KAT
  proves the decoder **per stage** against in-test references and known codewords; the **end-to-end
  bit-exact "real clip → exact YUV"** rung needs a reference fixture the host can't synthesize
  (no encoder/corpus) and is flagged as the gated follow-up. The foundation slice is exactly the set
  of layers that ARE provable now — so the implementer lands proven, falsifiable progress at every
  step, with the reference-frame proof and the CABAC/B/8×8 profiles as the explicitly larger,
  separately-spec'd follow-ups.
