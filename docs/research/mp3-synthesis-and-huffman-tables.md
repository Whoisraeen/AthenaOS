# Spec: MP3 Layer III — polyphase synthesis filterbank + remaining Huffman tables

Authoritative data spec to make `.mp3` **audible** in `athmedia`. The DSP math already
landed and is host-KAT'd (commit `c5100db`: requant, IMDCT, overlap-add, MS-stereo, alias
reduction, reorder, Huffman tables {1,2,3,5,6,7,8,10}). Two data gaps remain — both supplied
here, both with a verification method. The implementer does **not** invent values: they
transcribe the Huffman arrays into `tools/mp3_huff_gen/gen.rs` (which proves prefix-freeness
before emitting) and compute the synthesis cosine matrix at init exactly like the IMDCT
already does.

## Concept promise served

> "AthenaOS ships a media stack that just plays the files people actually have — music, video,
> photos — out of the box, with no codec hunts and no bundled spyware."
> (LEGACY_GAMING_CONCEPT.md, Media / "it just works" pillar — the same line `athmedia/src/lib.rs`
> quotes in its module docstring for FLAC/WAV/MP3.)

MP3 is the single most common consumer audio container in existence; "plays the files people
have" is not true until `.mp3` produces sound. This spec closes the last gap between the
landed hybrid-filterbank output and PCM you can hear.

## Already in the tree (verify-before-implement)

The whole entropy + DSP path is **built and host-KAT'd** — do **not** rebuild it. Only the two
data deltas below are missing.

- `components/athmedia/src/mp3.rs` — header/side-info parse, bit reservoir, `BitReader`,
  `huff_table()`, `decode_huff_pair()` (incl. linbits + sign), `decode_count1_quad()`,
  `decode_huffman_region()`. **[x] built.** `huff_table()` returns `Invalid` for any table not
  in {0,1,2,3,5,6,7,8,10}; `decode_count1_quad()` returns `Invalid` for `count1table_select==0`
  (table A). These two `Invalid` branches are exactly what this spec retires.
- `components/athmedia/src/mp3_tables.rs` — landed Huffman tables `T1,T2,T3,T5,T6,T7,T8,T10`
  (the `he!(code,len,x,y)` format), `VERIFIED_HUFF_TABLES = [0,1,2,3,5,6,7,8,10]`, sfb-band
  tables (all 9 rates), `SLEN_MPEG1`, `PRETAB`. **[x] built.**
- `components/athmedia/src/mp3_dsp.rs` — `decode_scalefactors`, `requantize` (no-libm cbrt +
  `pow2_quarter`), `reorder`, `stereo` (MS done; intensity deferred), `alias_reduce`, `imdct`
  (long/short/start/stop + freq-inversion + overlap-add via `ChannelState`). **[x] built and
  host-KAT'd.** Its module docstring explicitly names the polyphase synthesis filterbank as the
  one remaining deferred stage that this spec supplies.
- `components/athmedia/src/mp3_imdct_tables.rs` — `IMDCT_LONG[36][18]`, `IMDCT_SHORT[12][6]`,
  `WIN0..WIN3` as `const` cosine tables computed at table-build (sin/cos used only on the host
  at gen time). **[x] built.** This is the exact precedent for the synthesis cosine matrix:
  the new `N[64][32]` either reuses this generator or is computed once at runtime init (see
  Design §1; runtime compute is allowed because it is one-time, not per-frame).
- `components/athmedia/src/lib.rs` — `Mp3Decoder`, `run_dsp_granule()` (returns the
  hybrid-filterbank output `[ch][576]`), `decode_hybrid_frame()` (runs the full real path), and
  the `Decoder::decode()` impl that currently emits **geometry-correct silence** because the
  subband output is dropped pre-synthesis (lib.rs ~L2486-2495). **[x] built**; this is the call
  site that must be rewired to run synthesis and emit real samples.
- `tools/mp3_huff_gen/gen.rs` — the generator the implementer feeds. Input rows are
  `Tab{n, x, y, lin, hlen:&[..], hcod:&[..]}` row-major over `(x,y)` (x outer, y inner). It
  **rejects** dimension-mismatch, code-≥-2^len, and any non-prefix-free table before emitting,
  and prints `All N tables verified prefix-free.` **[x] built.** Every Huffman array in this
  spec is already in this exact input shape.

Status to flip when done: `mp3.rs`/`mp3_tables.rs`/`lib.rs` MP3 rows in `MasterChecklist.md`
(media/Phase-7 audio) from `[~]` (hybrid output, silent) to `[~]` audible-on-QEMU → `[x]`
once iron HDA plays it (Phase 2.6 / 7).

## Prior art & OSS verdict

All values below are **corroborated across at least two independent decoders** and then
**validated byte-exact against the already-landed tables** (the corroboration gate; see
Verification §B). None of these projects is vendored or linked — they are read-only spec
oracles. AthenaOS keeps its own `#![no_std]` no-libm decoder.

- **ISO/IEC 11172-3** — the normative source. Table B.7 (Huffman code tables), Table B.3
  (synthesis window D[512]), §2.4.3.2 (synthesis subband filter). 📖 normative reference, not code.
- **LAME `libmp3lame/tables.c`** (LGPL) — explicit `tNHB[]` (code values) + `tNl[]` (lengths)
  arrays in ISO row-major `(x,y)` order; the `ht[]` struct gives the table-select→codebook map
  and linbits. 📖 **study/isolate (LGPL)** — used here only to source the **code values**; no
  code copied. (LAME is an encoder, so its `tNl` lengths are encoder-internal and are **not**
  the decode lengths — see the gotcha in Verification §B.)
- **FFmpeg `libavcodec/mpegaudiodec_common.c`** (LGPL) — `mpa_hufflens[]` + `mpa_huffsymbols[]`
  (canonical lens + symbols) and `ff_mpa_huff_data[32][2]` (table-select→codebook + linbits).
  📖 **study/isolate (LGPL)** — used here only to source the **decode lengths** and the
  table-select map; no code copied.
- **pdmp3 (`pdmp3.c`)** — public-domain single-file decoder. Source of the float `D[512]`
  synthesis window (`g_synth_dtbl`), the matrix `N[i][j]=cos((16+i)(2j+1)π/64)`, and the exact
  V→U→window→sum synthesis loop. ➕ **public-domain, vendorable** — but we do not vendor; we
  transcribe the D[] values and re-express the algorithm in our own no-libm Rust.
- **minimp3 / dr_mp3** (CC0/public domain) — cross-check of the dewindow (its `g_win` is the
  same prototype filter in scaled-integer form) and the linbits table. ➕ public-domain.
- **Concept §R7 (no Linux-clone lineage):** satisfied — MP3 is an ISO/MPEG codec, not a Linux
  subsystem; the implementation is original Rust over ISO data tables.

## Design

### §1 — Polyphase synthesis filterbank (the matrixed formulation)

Input per granule/channel: the IMDCT/overlap output already produced by `imdct()` —
`sb[32][18]`, i.e. 18 time samples for each of 32 subbands (the current `out[sb*18 + k]`
layout in `mp3_dsp::imdct`, with subband = the outer index). Output: **1152 PCM samples**
(18 sub-sample-passes × 32 = 576 per granule; 2 granules/frame → 1152 per channel per frame),
emitted as `f32` in [-1, 1] (or i16 after the clamp).

This is ISO §2.4.3.2 in the form modern decoders use. **No 512-entry opaque cosine blob** — the
64×32 matrix is computed at init from a closed form, exactly like the landed IMDCT cosines.

**State (persistent per channel, carried across granules — extend `ChannelState`):**

```text
v_fifo: [f32; 1024]   // the V[] FIFO, zero-initialized; shifts up by 64 each sub-pass
```

Add this to `mp3_dsp::ChannelState` next to `overlap`. It MUST be reset (zeroed) on decoder
construction and on any seek/flush, identical to how `overlap` is handled.

**The cosine matrix N (computed once, no per-frame trig):**

```text
N[i][j] = cos( (16 + i) * (2*j + 1) * PI / 64 )      for i in 0..64, j in 0..32
```

Two acceptable no-libm realizations (pick one — prefer (a) for consistency with IMDCT):

  (a) **Build-time `const` table** via `tools/mp3_huff_gen/gentab.rs` (the same generator that
      already emits `IMDCT_LONG`/`WIN*`), producing `pub static SYNTH_N: [[f32; 32]; 64]` in
      `mp3_imdct_tables.rs`. sin/cos run on the host at gen time only. This is the established
      pattern and keeps the runtime allocation-free and trig-free.
  (b) **One-time runtime init** into a `[[f32;32];64]` stored in `ChannelState` or a `OnceCell`,
      computed with the kernel's existing no-libm cosine (the IMDCT generator's algorithm).
      Allowed because it is one-time, not on the hot path. Do NOT recompute per granule.

**The synthesis loop (per granule/channel), exactly 18 sub-passes:**

For `ss` in `0..18`:

1. **Shift the FIFO up by 64:**
   `for i in (64..1024).rev(): v_fifo[i] = v_fifo[i-64]`  (i.e. `v_fifo[64..1024] = v_fifo[0..960]`)

2. **Gather the 32 subband samples for this sub-pass into `s[32]`:**
   `s[k] = sb[k][ss]`  for `k` in `0..32`   (subband k, time-index ss)

3. **Matrix-multiply into the bottom 64 of the FIFO:**
   `for i in 0..64: v_fifo[i] = sum over k in 0..32 of N[i][k] * s[k]`

4. **Build the U[512] vector from V (the ISO "build U" gather):**
   ```text
   for i in 0..8:
     for j in 0..32:
       u[(i<<6) + j]      = v_fifo[(i<<7) + j]        // i*64 + j     <- i*128 + j
       u[(i<<6) + j + 32] = v_fifo[(i<<7) + j + 96]   // i*64+32+j    <- i*128+96+j
   ```

5. **Window:** `for i in 0..512: u[i] *= D[i]`   (D[512] from §1.D below)

6. **Produce 32 PCM samples (the Σ over the 16 window slices):**
   ```text
   for i in 0..32:
     let mut acc = 0.0;
     for j in 0..16: acc += u[(j<<5) + i]     // u[j*32 + i]
     pcm_out[32*ss + i] = clamp(acc)
   ```

**Output scaling & clamp.** The D[] values are scaled so `acc` is already in roughly [-1, 1]
float PCM. For an `f32` `AudioFrame` (athmedia's native sample format — see `AudioFrame`),
emit `acc` directly, then hard-clamp to `[-1.0, 1.0]`. For an i16 path, `samp = round(acc *
32767.0)` then clamp to `[-32767, 32767]` (note: 32767, not 32768 — the reference clamps
symmetrically). Match whatever `AudioFrame.samples` expects; athmedia uses `f32`, so the f32
clamp is the path. Do NOT add any extra gain; the D[] table carries the full filterbank gain.

**Frame assembly.** Run synthesis for **both granules** of the frame and both channels; the
576 samples/granule concatenate to 1152/channel/frame. Interleave to the `AudioFrame` channel
layout the resampler/remixer expect (athmedia stores planar-per-channel `Vec<f32>` today; keep
that — one `Vec<f32>` of length `granules*576` per channel).

**Failure modes / untrusted-input discipline (match the existing path):** every index above is
a fixed compile-time bound (no side-info-derived indexing into `u`/`v`), so synthesis itself
cannot OOB. A corrupt granule already arrives as zeroed `sb[][]` from the upstream clamp;
zeroed input → silence out, never a panic. The clamp in step 6 bounds any NaN/inf that a
pathological dequant could produce (`acc.is_finite()` guard → 0.0 is acceptable belt-and-braces).

**§1.D — The synthesis window D[512] (ISO Table B.3).** Authoritative values, corroborated
pdmp3 (float) ↔ minimp3 (scaled-int) and validated as multiples of the 2^-16 quantum
(`1/65536 = 0.0000152587890625`). The sign convention below is the **decoder-ready** form
(pdmp3 has the per-block sign already folded in, so step 5 is a plain multiply). Transcribe
verbatim as `pub static SYNTH_D: [f32; 512]` (in `mp3_imdct_tables.rs` next to the IMDCT
tables). Relationship to the analysis window: `D[i] = 32 * C[i]` where `C` is the ISO Layer
I/II analysis window (peak `D[256]=1.144989014`, so `C` peak `= 0.0357809…`); we use D directly
so no C transform is needed.

```
0.000000000, -0.000015259, -0.000015259, -0.000015259, -0.000015259, -0.000015259, -0.000015259, -0.000030518,
-0.000030518, -0.000030518, -0.000030518, -0.000045776, -0.000045776, -0.000061035, -0.000061035, -0.000076294,
-0.000076294, -0.000091553, -0.000106812, -0.000106812, -0.000122070, -0.000137329, -0.000152588, -0.000167847,
-0.000198364, -0.000213623, -0.000244141, -0.000259399, -0.000289917, -0.000320435, -0.000366211, -0.000396729,
-0.000442505, -0.000473022, -0.000534058, -0.000579834, -0.000625610, -0.000686646, -0.000747681, -0.000808716,
-0.000885010, -0.000961304, -0.001037598, -0.001113892, -0.001205444, -0.001296997, -0.001388550, -0.001480103,
-0.001586914, -0.001693726, -0.001785278, -0.001907349, -0.002014160, -0.002120972, -0.002243042, -0.002349854,
-0.002456665, -0.002578735, -0.002685547, -0.002792358, -0.002899170, -0.002990723, -0.003082275, -0.003173828,
 0.003250122,  0.003326416,  0.003387451,  0.003433228,  0.003463745,  0.003479004,  0.003479004,  0.003463745,
 0.003417969,  0.003372192,  0.003280640,  0.003173828,  0.003051758,  0.002883911,  0.002700806,  0.002487183,
 0.002227783,  0.001937866,  0.001617432,  0.001266479,  0.000869751,  0.000442505, -0.000030518, -0.000549316,
-0.001098633, -0.001693726, -0.002334595, -0.003005981, -0.003723145, -0.004486084, -0.005294800, -0.006118774,
-0.007003784, -0.007919312, -0.008865356, -0.009841919, -0.010848999, -0.011886597, -0.012939453, -0.014022827,
-0.015121460, -0.016235352, -0.017349243, -0.018463135, -0.019577026, -0.020690918, -0.021789551, -0.022857666,
-0.023910522, -0.024932861, -0.025909424, -0.026840210, -0.027725220, -0.028533936, -0.029281616, -0.029937744,
-0.030532837, -0.031005859, -0.031387329, -0.031661987, -0.031814575, -0.031845093, -0.031738281, -0.031478882,
 0.031082153,  0.030517578,  0.029785156,  0.028884888,  0.027801514,  0.026535034,  0.025085449,  0.023422241,
 0.021575928,  0.019531250,  0.017257690,  0.014801025,  0.012115479,  0.009231567,  0.006134033,  0.002822876,
-0.000686646, -0.004394531, -0.008316040, -0.012420654, -0.016708374, -0.021179199, -0.025817871, -0.030609131,
-0.035552979, -0.040634155, -0.045837402, -0.051132202, -0.056533813, -0.061996460, -0.067520142, -0.073059082,
-0.078628540, -0.084182739, -0.089706421, -0.095169067, -0.100540161, -0.105819702, -0.110946655, -0.115921021,
-0.120697021, -0.125259399, -0.129562378, -0.133590698, -0.137298584, -0.140670776, -0.143676758, -0.146255493,
-0.148422241, -0.150115967, -0.151306152, -0.151962280, -0.152069092, -0.151596069, -0.150497437, -0.148773193,
-0.146362305, -0.143264771, -0.139450073, -0.134887695, -0.129577637, -0.123474121, -0.116577148, -0.108856201,
 0.100311279,  0.090927124,  0.080688477,  0.069595337,  0.057617188,  0.044784546,  0.031082153,  0.016510010,
 0.001068115, -0.015228271, -0.032379150, -0.050354004, -0.069168091, -0.088775635, -0.109161377, -0.130310059,
-0.152206421, -0.174789429, -0.198059082, -0.221984863, -0.246505737, -0.271591187, -0.297210693, -0.323318481,
-0.349868774, -0.376800537, -0.404083252, -0.431655884, -0.459472656, -0.487472534, -0.515609741, -0.543823242,
-0.572036743, -0.600219727, -0.628295898, -0.656219482, -0.683914185, -0.711318970, -0.738372803, -0.765029907,
-0.791213989, -0.816864014, -0.841949463, -0.866363525, -0.890090942, -0.913055420, -0.935195923, -0.956481934,
-0.976852417, -0.996246338, -1.014617920, -1.031936646, -1.048156738, -1.063217163, -1.077117920, -1.089782715,
-1.101211548, -1.111373901, -1.120223999, -1.127746582, -1.133926392, -1.138763428, -1.142211914, -1.144287109,
 1.144989014,  1.144287109,  1.142211914,  1.138763428,  1.133926392,  1.127746582,  1.120223999,  1.111373901,
 1.101211548,  1.089782715,  1.077117920,  1.063217163,  1.048156738,  1.031936646,  1.014617920,  0.996246338,
 0.976852417,  0.956481934,  0.935195923,  0.913055420,  0.890090942,  0.866363525,  0.841949463,  0.816864014,
 0.791213989,  0.765029907,  0.738372803,  0.711318970,  0.683914185,  0.656219482,  0.628295898,  0.600219727,
 0.572036743,  0.543823242,  0.515609741,  0.487472534,  0.459472656,  0.431655884,  0.404083252,  0.376800537,
 0.349868774,  0.323318481,  0.297210693,  0.271591187,  0.246505737,  0.221984863,  0.198059082,  0.174789429,
 0.152206421,  0.130310059,  0.109161377,  0.088775635,  0.069168091,  0.050354004,  0.032379150,  0.015228271,
-0.001068115, -0.016510010, -0.031082153, -0.044784546, -0.057617188, -0.069595337, -0.080688477, -0.090927124,
 0.100311279,  0.108856201,  0.116577148,  0.123474121,  0.129577637,  0.134887695,  0.139450073,  0.143264771,
 0.146362305,  0.148773193,  0.150497437,  0.151596069,  0.152069092,  0.151962280,  0.151306152,  0.150115967,
 0.148422241,  0.146255493,  0.143676758,  0.140670776,  0.137298584,  0.133590698,  0.129562378,  0.125259399,
 0.120697021,  0.115921021,  0.110946655,  0.105819702,  0.100540161,  0.095169067,  0.089706421,  0.084182739,
 0.078628540,  0.073059082,  0.067520142,  0.061996460,  0.056533813,  0.051132202,  0.045837402,  0.040634155,
 0.035552979,  0.030609131,  0.025817871,  0.021179199,  0.016708374,  0.012420654,  0.008316040,  0.004394531,
 0.000686646, -0.002822876, -0.006134033, -0.009231567, -0.012115479, -0.014801025, -0.017257690, -0.019531250,
-0.021575928, -0.023422241, -0.025085449, -0.026535034, -0.027801514, -0.028884888, -0.029785156, -0.030517578,
 0.031082153,  0.031478882,  0.031738281,  0.031845093,  0.031814575,  0.031661987,  0.031387329,  0.031005859,
 0.030532837,  0.029937744,  0.029281616,  0.028533936,  0.027725220,  0.026840210,  0.025909424,  0.024932861,
 0.023910522,  0.022857666,  0.021789551,  0.020690918,  0.019577026,  0.018463135,  0.017349243,  0.016235352,
 0.015121460,  0.014022827,  0.012939453,  0.011886597,  0.010848999,  0.009841919,  0.008865356,  0.007919312,
 0.007003784,  0.006118774,  0.005294800,  0.004486084,  0.003723145,  0.003005981,  0.002334595,  0.001693726,
 0.001098633,  0.000549316,  0.000030518, -0.000442505, -0.000869751, -0.001266479, -0.001617432, -0.001937866,
-0.002227783, -0.002487183, -0.002700806, -0.002883911, -0.003051758, -0.003173828, -0.003280640, -0.003372192,
-0.003417969, -0.003463745, -0.003479004, -0.003479004, -0.003463745, -0.003433228, -0.003387451, -0.003326416,
 0.003250122,  0.003173828,  0.003082275,  0.002990723,  0.002899170,  0.002792358,  0.002685547,  0.002578735,
 0.002456665,  0.002349854,  0.002243042,  0.002120972,  0.002014160,  0.001907349,  0.001785278,  0.001693726,
 0.001586914,  0.001480103,  0.001388550,  0.001296997,  0.001205444,  0.001113892,  0.001037598,  0.000961304,
 0.000885010,  0.000808716,  0.000747681,  0.000686646,  0.000625610,  0.000579834,  0.000534058,  0.000473022,
 0.000442505,  0.000396729,  0.000366211,  0.000320435,  0.000289917,  0.000259399,  0.000244141,  0.000213623,
 0.000198364,  0.000167847,  0.000152588,  0.000137329,  0.000122070,  0.000106812,  0.000106812,  0.000091553,
 0.000076294,  0.000076294,  0.000061035,  0.000061035,  0.000045776,  0.000045776,  0.000030518,  0.000030518,
 0.000030518,  0.000030518,  0.000015259,  0.000015259,  0.000015259,  0.000015259,  0.000015259,  0.000015259
```
(512 values; row-major, index 0→511 in the order used by step 5.)

### §2 — The remaining Huffman tables

**Table 4 is confirmed unused/empty.** ISO Table B.7 and both LAME (`ht[4]={0,0,NULL,NULL}`)
and FFmpeg (`ff_mpa_huff_data[4]={0,0}`) mark `table_select == 4` as not used; it shares the
empty/zero behavior of table 0. **Table 14 is also unused** (`ht[14]={0,0,NULL,..}`,
"Apparently not used"). The current `huff_table()` already returns `Invalid` for 4 and 14;
leave that — a stream that selects 4 or 14 is non-conformant and the clean-stop behavior is
correct. So tables 4 and 14 need **no** new data.

**Table-select → codebook + linbits map (ISO Table B.7, via `ff_mpa_huff_data` / LAME `ht[]`):**

| select | codebook | linbits || select | codebook | linbits |
|---|---|---|---|---|---|---|
| 0 | (all-zero) | 0 || 16 | T16 | 1 |
| 1 | T1 | 0 || 17 | T16 | 2 |
| 2 | T2 | 0 || 18 | T16 | 3 |
| 3 | T3 | 0 || 19 | T16 | 4 |
| 4 | **unused** | — || 20 | T16 | 6 |
| 5 | T5 | 0 || 21 | T16 | 8 |
| 6 | T6 | 0 || 22 | T16 | 10 |
| 7 | T7 | 0 || 23 | T16 | 13 |
| 8 | T8 | 0 || 24 | T24 | 4 |
| 9 | T9 | 0 || 25 | T24 | 5 |
| 10 | T10 | 0 || 26 | T24 | 6 |
| 11 | T11 | 0 || 27 | T24 | 7 |
| 12 | T12 | 0 || 28 | T24 | 8 |
| 13 | T13 | 0 || 29 | T24 | 9 |
| 14 | **unused** | — || 30 | T24 | 11 |
| 15 | T15 | 0 || 31 | T24 | 13 |

So there are only **seven new codebooks** to transcribe — `T9, T11, T12, T13, T15, T16, T24` —
plus the linbits map above (selects 16-23 reuse T16, 24-31 reuse T24). `linbits` array indexed
by select, for the `huff_table()` match arms:
`[0;16] ++ [1,2,3,4,6,8,10,13, 4,5,6,7,8,9,11,13]`.

**The escape (linbits) decode is already correct** in `decode_huff_pair()`: when the decoded
`x` (or `y`) equals the table's max value `15` and `linbits > 0`, read `linbits` more bits and
add to 15. All seven new big_values tables that carry linbits (T16 family, T24 family) are
16×16 (max index 15) — the existing `x == 15` test fires correctly. No code change to the
escape path; just wire the linbits values from the map above.

**count1 quad tables.** Two count1 tables, `count1table_select` 0 (Table A / ISO t32) and 1
(Table B / ISO t33):

- **Table B (`count1table_select == 1`) is already landed and correct.** Its 16 codes are all
  length 4 and the magnitude of each of the 4 values is `1 - bit`. The landed
  `decode_count1_quad()` (`magnitude = (nibble_bit) ^ 1`) is exactly ISO t33 and is trivially
  prefix-free (all 16 distinct 4-bit codes). **No change.**
- **Table A (`count1table_select == 0`, ISO t32)** is the remaining count1 gap. Its 16 entries
  (a real variable-length Huffman tree over the 4-bit quad value `v*8+w*4+x*2+y`) in
  `{value, code, len}` form, prefix-free-verified:

  | quad value (vwxy) | code | len || quad value | code | len |
  |---|---|---|---|---|---|---|
  | 0000 | 1 | 1 || 1000 | 7 | 4 |
  | 0001 | 5 | 4 || 1001 | 3 | 4 |
  | 0010 | 4 | 4 || 1010 | 6 | 4 |
  | 0011 | 5 | 5 || 1011 | 0 | 6 |
  | 0100 | 6 | 4 || 1100 | 7 | 6 |
  | 0101 | 5 | 6 || 1101 | 2 | 6 |
  | 0110 | 4 | 5 || 1110 | 3 | 6 |
  | 0111 | 4 | 6 || 1111 | 1 | 6 |

  (Source: LAME `t32HB`/`t32l`, FFmpeg `mpa_quad_codes[0]`/`mpa_quad_bits[0]` — identical.
  `value` bit order is v=bit3, w=bit2, x=bit1, y=bit0; after the code, each nonzero magnitude is
  followed by a sign bit, same as table B.) The implementer adds a small fixed lookup for table
  A in `decode_count1_quad()` (the `Invalid` branch becomes a real decode). Magnitudes are 0/1
  per bit of `value`.

**The seven new big_values tables, in `tools/mp3_huff_gen/gen.rs` input form.** Paste these
`Tab{...}` rows directly into the `tables()` vec in `gen.rs`, run the generator (it will print
`All N tables verified prefix-free.`), and copy the emitted `pub static T9/T11/.../T24` into
`mp3_tables.rs`. Then add the match arms + linbits to `huff_table()` and extend
`VERIFIED_HUFF_TABLES`. **Every row below has been checked dimension-correct + prefix-free, and
the method that produced them reproduced the already-landed T1/T2/T3/T5/T6/T7/T8/T10 byte-exact
(corroboration gate — see Verification §B).** Rows are row-major over `(x,y)`, x outer.

```rust
Tab{n:9,x:6,y:6,lin:0,
    hlen:&[3,3,5,6,8,9, 3,3,4,5,6,8, 4,4,5,6,7,8, 6,5,6,7,7,8, 7,6,7,7,8,9, 8,7,8,8,9,9],
    hcod:&[7,5,9,14,15,7, 6,4,5,5,6,7, 7,6,8,8,8,5, 15,6,9,10,5,1, 11,7,9,6,4,1, 14,4,6,2,6,0]},

Tab{n:11,x:8,y:8,lin:0,
    hlen:&[2,3,5,7,8,9,8,9, 3,3,4,6,8,8,7,8, 5,5,6,7,8,9,8,8, 7,6,7,9,8,10,8,9,
           8,8,8,9,9,10,9,10, 8,8,9,10,10,11,10,11, 8,7,7,8,9,10,10,10, 8,7,8,9,10,10,10,10],
    hcod:&[3,4,10,24,34,33,21,15, 5,3,4,10,32,17,11,10, 11,7,13,18,30,31,20,5, 25,11,19,59,27,18,12,5,
           35,33,31,58,30,16,7,5, 28,26,32,19,17,15,8,14, 14,12,9,13,14,9,4,1, 11,4,6,6,6,3,2,0]},

Tab{n:12,x:8,y:8,lin:0,
    hlen:&[4,3,5,7,8,9,9,9, 3,3,4,5,7,7,8,8, 5,4,5,6,7,8,7,8, 6,5,6,6,7,8,8,8,
           7,6,7,7,8,8,8,9, 8,7,8,8,8,9,8,9, 8,7,7,8,8,9,9,10, 9,8,8,9,9,9,9,10],
    hcod:&[9,6,16,33,41,39,38,26, 7,5,6,9,23,16,26,11, 17,7,11,14,21,30,10,7, 17,10,15,12,18,28,14,5,
           32,13,22,19,18,16,9,5, 40,17,31,29,17,13,4,2, 27,12,11,15,10,7,4,1, 27,12,8,12,6,3,1,0]},

Tab{n:13,x:16,y:16,lin:0,
    hlen:&[1,4,6,7,8,9,9,10,9,10,11,11,12,12,13,13, 3,4,6,7,8,8,9,9,9,9,10,10,11,12,12,12,
           6,6,7,8,9,9,10,10,9,10,10,11,11,12,13,13, 7,7,8,9,9,10,10,10,10,11,11,11,11,12,13,13,
           8,7,9,9,10,10,11,11,10,11,11,12,12,13,13,14, 9,8,9,10,10,10,11,11,11,11,12,11,13,13,14,14,
           9,9,10,10,11,11,11,11,11,12,12,12,13,13,14,14, 10,9,10,11,11,11,12,12,12,12,13,13,13,14,16,16,
           9,8,9,10,10,11,11,12,12,12,12,13,13,14,15,15, 10,9,10,10,11,11,11,13,12,13,13,14,14,14,16,15,
           10,10,10,11,11,12,12,13,12,13,14,13,14,15,16,17, 11,10,10,11,12,12,12,12,13,13,13,14,15,15,15,16,
           11,11,11,12,12,13,12,13,14,14,15,15,15,16,16,16, 12,11,12,13,13,13,14,14,14,14,14,15,16,15,16,16,
           13,12,12,13,13,13,15,14,14,17,15,15,15,17,16,16, 12,12,13,14,14,14,15,14,15,15,16,16,19,18,19,16],
    hcod:&[1,5,14,21,34,51,46,71,42,52,68,52,67,44,43,19, 3,4,12,19,31,26,44,33,31,24,32,24,31,35,22,14,
           15,13,23,36,59,49,77,65,29,40,30,40,27,33,42,16, 22,20,37,61,56,79,73,64,43,76,56,37,26,31,25,14,
           35,16,60,57,97,75,114,91,54,73,55,41,48,53,23,24, 58,27,50,96,76,70,93,84,77,58,79,29,74,49,41,17,
           47,45,78,74,115,94,90,79,69,83,71,50,59,38,36,15, 72,34,56,95,92,85,91,90,86,73,77,65,51,44,43,42,
           43,20,30,44,55,78,72,87,78,61,46,54,37,30,20,16, 53,25,41,37,44,59,54,81,66,76,57,54,37,18,39,11,
           35,33,31,57,42,82,72,80,47,58,55,21,22,26,38,22, 53,25,23,38,70,60,51,36,55,26,34,23,27,14,9,7,
           34,32,28,39,49,75,30,52,48,40,52,28,18,17,9,5, 45,21,34,64,56,50,49,45,31,19,12,15,10,7,6,3,
           48,23,20,39,36,35,53,21,16,23,13,10,6,1,4,2, 16,15,17,27,25,20,29,11,17,12,16,8,1,1,0,1]},

Tab{n:15,x:16,y:16,lin:0,
    hlen:&[3,4,5,7,7,8,9,9,9,10,10,11,11,11,12,13, 4,3,5,6,7,7,8,8,8,9,9,10,10,10,11,11,
           5,5,5,6,7,7,8,8,8,9,9,10,10,11,11,11, 6,6,6,7,7,8,8,9,9,9,10,10,10,11,11,11,
           7,6,7,7,8,8,9,9,9,9,10,10,10,11,11,11, 8,7,7,8,8,8,9,9,9,9,10,10,11,11,11,12,
           9,7,8,8,8,9,9,9,9,10,10,10,11,11,12,12, 9,8,8,9,9,9,9,10,10,10,10,10,11,11,11,12,
           9,8,8,9,9,9,9,10,10,10,10,11,11,12,12,12, 9,8,9,9,9,9,10,10,10,11,11,11,11,12,12,12,
           10,9,9,9,10,10,10,10,10,11,11,11,11,12,13,12, 10,9,9,9,10,10,10,10,11,11,11,11,12,12,12,13,
           11,10,9,10,10,10,11,11,11,11,11,11,12,12,13,13, 11,10,10,10,10,11,11,11,11,12,12,12,12,12,13,13,
           12,11,11,11,11,11,11,11,12,12,12,12,13,13,12,13, 12,11,11,11,11,11,11,12,12,12,12,12,13,13,13,13],
    hcod:&[7,12,18,53,47,76,124,108,89,123,108,119,107,81,122,63, 13,5,16,27,46,36,61,51,42,70,52,83,65,41,59,36,
           19,17,15,24,41,34,59,48,40,64,50,78,62,80,56,33, 29,28,25,43,39,63,55,93,76,59,93,72,54,75,50,29,
           52,22,42,40,67,57,95,79,72,57,89,69,49,66,46,27, 77,37,35,66,58,52,91,74,62,48,79,63,90,62,40,38,
           125,32,60,56,50,92,78,65,55,87,71,51,73,51,70,30, 109,53,49,94,88,75,66,122,91,73,56,42,64,44,21,25,
           90,43,41,77,73,63,56,92,77,66,47,67,48,53,36,20, 71,34,67,60,58,49,88,76,67,106,71,54,38,39,23,15,
           109,53,51,47,90,82,58,57,48,72,57,41,23,27,62,9, 86,42,40,37,70,64,52,43,70,55,42,25,29,18,11,11,
           118,68,30,55,50,46,74,65,49,39,24,16,22,13,14,7, 91,44,39,38,34,63,52,45,31,52,28,19,14,8,9,3,
           123,60,58,53,47,43,32,22,37,24,17,12,15,10,2,1, 71,37,34,30,28,20,17,26,21,16,10,6,8,6,2,0]},

Tab{n:16,x:16,y:16,lin:1,
    hlen:&[1,4,6,8,9,9,10,10,11,11,11,12,12,12,13,9, 3,4,6,7,8,9,9,9,10,10,10,11,12,11,12,8,
           6,6,7,8,9,9,10,10,11,10,11,11,11,12,12,9, 8,7,8,9,9,10,10,10,11,11,12,12,12,13,13,10,
           9,8,9,9,10,10,11,11,11,12,12,12,13,13,13,9, 9,8,9,9,10,11,11,12,11,12,12,13,13,13,14,10,
           10,9,9,10,11,11,11,11,12,12,12,12,13,13,14,10, 10,9,10,10,11,11,11,12,12,13,13,13,13,15,15,10,
           10,10,10,11,11,11,12,12,13,13,13,13,14,14,14,10, 11,10,10,11,11,12,12,13,13,13,13,14,13,14,13,11,
           11,11,10,11,12,12,12,12,13,14,14,14,15,15,14,10, 12,11,11,11,12,12,13,14,14,14,14,14,14,13,14,11,
           12,12,12,12,12,13,13,13,13,15,14,14,14,14,16,11, 14,12,12,12,13,13,14,14,14,16,15,15,15,17,15,11,
           13,13,11,12,14,14,13,14,14,15,16,15,17,15,14,11, 9,8,8,9,9,10,10,10,11,11,11,11,11,11,11,8],
    hcod:&[1,5,14,44,74,63,110,93,172,149,138,242,225,195,376,17, 3,4,12,20,35,62,53,47,83,75,68,119,201,107,207,9,
           15,13,23,38,67,58,103,90,161,72,127,117,110,209,206,16, 45,21,39,69,64,114,99,87,158,140,252,212,199,387,365,26,
           75,36,68,65,115,101,179,164,155,264,246,226,395,382,362,9, 66,30,59,56,102,185,173,265,142,253,232,400,388,378,445,16,
           111,54,52,100,184,178,160,133,257,244,228,217,385,366,715,10, 98,48,91,88,165,157,148,261,248,407,397,372,380,889,884,8,
           85,84,81,159,156,143,260,249,427,401,392,383,727,713,708,7, 154,76,73,141,131,256,245,426,406,394,384,735,359,710,352,11,
           139,129,67,125,247,233,229,219,393,743,737,720,885,882,439,4, 243,120,118,115,227,223,396,746,742,736,721,712,706,223,436,6,
           202,224,222,218,216,389,386,381,364,888,443,707,440,437,1728,4, 747,211,210,208,370,379,734,723,714,1735,883,877,876,3459,865,2,
           377,369,102,187,726,722,358,711,709,866,1734,871,3458,870,434,0, 12,10,7,11,10,17,11,9,13,12,10,7,5,3,1,3]},

Tab{n:24,x:16,y:16,lin:4,
    hlen:&[4,4,6,7,8,9,9,10,10,11,11,11,11,11,12,9, 4,4,5,6,7,8,8,9,9,9,10,10,10,10,10,8,
           6,5,6,7,7,8,8,9,9,9,9,10,10,10,11,7, 7,6,7,7,8,8,8,9,9,9,9,10,10,10,10,7,
           8,7,7,8,8,8,8,9,9,9,10,10,10,10,11,7, 9,7,8,8,8,8,9,9,9,9,10,10,10,10,10,7,
           9,8,8,8,8,9,9,9,9,10,10,10,10,10,11,7, 10,8,8,8,9,9,9,9,10,10,10,10,10,11,11,8,
           10,9,9,9,9,9,9,9,9,10,10,10,10,11,11,8, 10,9,9,9,9,9,9,10,10,10,10,10,11,11,11,8,
           11,9,9,9,9,10,10,10,10,10,10,11,11,11,11,8, 11,10,9,9,9,10,10,10,10,10,10,11,11,11,11,8,
           11,10,10,10,10,10,10,10,10,10,11,11,11,11,11,8, 11,10,10,10,10,10,10,10,11,11,11,11,11,11,11,8,
           12,10,10,10,10,10,10,11,11,11,11,11,11,11,11,8, 8,7,7,7,7,7,7,7,7,7,7,8,8,8,8,4],
    hcod:&[15,13,46,80,146,262,248,434,426,669,653,649,621,517,1032,88, 14,12,21,38,71,130,122,216,209,198,327,345,319,297,279,42,
           47,22,41,74,68,128,120,221,207,194,182,340,315,295,541,18, 81,39,75,70,134,125,116,220,204,190,178,325,311,293,271,16,
           147,72,69,135,127,118,112,210,200,188,352,323,306,285,540,14, 263,66,129,126,119,114,214,202,192,180,341,317,301,281,262,12,
           249,123,121,117,113,215,206,195,185,347,330,308,291,272,520,10, 435,115,111,109,211,203,196,187,353,332,313,298,283,531,381,17,
           427,212,208,205,201,193,186,177,169,320,303,286,268,514,377,16, 335,199,197,191,189,181,174,333,321,305,289,275,521,379,371,11,
           668,184,183,179,175,344,331,314,304,290,277,530,383,373,366,10, 652,346,171,168,164,318,309,299,287,276,263,513,375,368,362,6,
           648,322,316,312,307,302,292,284,269,261,512,376,370,364,359,4, 620,300,296,294,288,282,273,266,515,380,374,369,365,361,357,2,
           1033,280,278,274,267,264,259,382,378,372,367,363,360,358,356,0, 43,20,19,17,15,13,11,9,7,6,4,7,5,3,1,3]},
```

After the generator emits these, the implementer must:
1. Add `9 => (&t::T9, 0)`, `11 => (&t::T11, 0)`, `12 => (&t::T12, 0)`, `13 => (&t::T13, 0)`,
   `15 => (&t::T15, 0)` arms to `huff_table()`.
2. Add the T16-family arms `16..=23 => (&t::T16, linbits)` and T24-family `24..=31 => (&t::T24,
   linbits)` with linbits from the map (write them as explicit arms or a small `match` on
   select for the linbits value — do NOT collapse the codebook ref, but DO use the linbits map).
3. Extend `VERIFIED_HUFF_TABLES` to `&[0,1,2,3,5,6,7,8,9,10,11,12,13,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31]`.

## Interface needs (NEEDS-INTERFACE)

**None.** This is entirely inside the `athmedia` userspace crate — no new syscall, no `ath_abi`
change, no kernel ABI surface. The decoder already returns `AudioFrame` through the existing
`Decoder` trait; only the *contents* go from silence to real PCM.

## File-by-file plan

- `tools/mp3_huff_gen/gen.rs` — add the 7 `Tab{}` rows from §2 to `tables()`. Run it; confirm
  `All N tables verified prefix-free.`; it rejects any transcription typo.
- `components/athmedia/src/mp3_tables.rs` — paste the generator output (`pub static T9, T11,
  T12, T13, T15, T16, T24`); extend `VERIFIED_HUFF_TABLES`.
- `components/athmedia/src/mp3_imdct_tables.rs` — add `pub static SYNTH_D: [f32; 512]` (§1.D)
  and `pub static SYNTH_N: [[f32; 32]; 64]` (if using the build-time `const` matrix, option a).
  If option (b), no table here; compute N at init instead.
- `components/athmedia/src/mp3.rs` — add the 5 simple + 16 linbits-sharing match arms to
  `huff_table()`; replace the count1 table-A `Invalid` branch in `decode_count1_quad()` with the
  16-entry table-A lookup from §2.
- `components/athmedia/src/mp3_dsp.rs` — add `v_fifo: [f32; 1024]` to `ChannelState` (+ zero it
  in `new()`); add `pub fn synthesis(sb: &[[f32;18];32], state: &mut ChannelState, pcm_out: &mut
  [f32; 576])` implementing §1 steps 1-6, with the Concept-promise docstring and a
  `run_boot_smoketest`-able pure entry point.
- `components/athmedia/src/lib.rs` — in `Mp3Decoder`, after `run_dsp_granule` produces the
  subband output, call `mp3_dsp::synthesis` per granule/channel and write the resulting PCM into
  the `AudioFrame` instead of the geometry-correct silence at ~L2486-2495. Add the host-KAT
  module asserts (Verification §A/§C) to the existing `#[cfg(test)]` block.

## Acceptance criteria (the exact proof)

- **Prefix-free generator gate (cheapest, run first):** `rustc -O tools/mp3_huff_gen/gen.rs &&
  ./gen 2>&1` MUST print `All N tables verified prefix-free.` (N rises from 8 to 15) and MUST
  exit 0. A bad transcription prints `FAIL T<k>: ...` and exits 1.
- **Host KAT — Huffman round-trip (FAIL-able):** `cargo test -p athmedia mp3_huff` MUST pass a
  `mp3_huff_tables_prefix_free` that now iterates selects `0..=31` and asserts every codebook in
  `VERIFIED_HUFF_TABLES` is prefix-free, AND a new `mp3_huff_decodes_known_codeword` that feeds a
  hand-built bitstream of one known codeword from T13/T16/T24 and asserts the exact `(x,y)` pair
  (incl. a linbits-escape case for T16: x or y == 15 + linbits). The assert MUST be able to
  print FAIL (use a wrong-codeword negative case in the same test).
- **Host KAT — synthesis filterbank (FAIL-able):** a new `mp3_synthesis_known_input` MUST drive
  `synthesis()` with a single-tone subband input (one nonzero subband, constant across the 18
  sub-passes) and assert the output PCM is non-silent and bounded (`max(|pcm|) > 0.01` and
  `<= 1.0`), and that a **zero** subband input yields **all-zero** PCM (the FAIL lever: if the
  D[] table or the V/U gather is wrong, the impulse response is wrong — compare the first 32
  output samples against a reference computed in-test from the same N/D constants, tolerance
  `1e-4`).
- **Host KAT — end-to-end reference match (the audible proof):** `mp3_decode_matches_reference`
  MUST decode a short embedded CBR-128k MP3 (a few frames; commit the bytes as a test fixture)
  and assert the decoded PCM matches a reference decode (precomputed with pdmp3/minimp3 and
  committed as the expected sample array) within RMS tolerance — assert `rms_error < 1e-2` over
  the first 1152 samples/channel. This is the single assert that proves "audible and correct,"
  not just "non-silent."
- **Boot smoketest line:** `run_boot_smoketest()` for the MP3 path MUST emit
  `[athmedia] mp3 synth: frames=<n> peak=<f> nonsilent=<bool> -> PASS` (FAIL if a decoded known
  clip is silent or peak is 0). This is the serial marker to grep.
- **procfs:** `/proc/athena/media` (or the athmedia status line, wherever the crate already
  reports) MUST report `mp3=audible` (was `mp3=hybrid-silent`) once synthesis is wired.
- **Docstring:** `mp3_dsp::synthesis` MUST quote the Concept promise at the top of this spec.

## Handoff

- **Implementer: athena-media.** This is pure `athmedia` userspace work; no kernel/ABI touch.
- **Unblocks checklist lines:** the MP3 rows under media / Phase-7 audio in `MasterChecklist.md`
  (`.mp3` decode `[~] hybrid-silent` → `[~] audible (QEMU/host)` → `[x]` on iron HDA), and feeds
  the Phase-2.6/7 "real HDA PCM playback of a user file" goal (the HDA path already plays
  `wrote_samples` on iron; this makes the source an actual song).
- **Sequencing:** no interface commit needed (NEEDS-INTERFACE = none). Land in this order so
  each step is independently provable: (1) Huffman tables via the generator + host KAT
  (prefix-free + known-codeword); (2) count1 table A + host KAT; (3) `synthesis()` + its host
  KAT against the in-test reference; (4) wire `lib.rs` to emit PCM + the end-to-end
  reference-match KAT + boot smoketest. Each step keeps the build green and the prior `[~]`
  silence behavior until step 4 flips it to audible.

---

### Provenance / corroboration note (for the reviewer)

- **Huffman codes** sourced from LAME `tNHB[]`; **decode lengths** from FFmpeg `mpa_hufflens`/
  `mpa_huffsymbols`. Combining the two and re-validating reproduced the **already-landed**
  T1/T2/T3/T5/T6/T7/T8/T10 **byte-exact** (code+len for every entry), and every new table is
  dimension-correct + prefix-free under the generator's own check. This is the corroboration
  gate: two independent LGPL oracles agree, and the agreement matches the proven-correct subset
  already in the tree. (LAME's own `tNl` length arrays are encoder-internal and do NOT equal the
  decode lengths — they are uniformly off by the encoding convention — so lengths were taken
  from FFmpeg, not LAME. This is the one gotcha; it is why a naive "just copy LAME `tNl`"
  transcription would silently produce wrong lengths that the prefix-free check would then
  reject — the generator catches it.)
- **D[512]** sourced from pdmp3 (public domain), cross-checked against minimp3's scaled-int
  dewindow and confirmed as exact multiples of the 2^-16 quantum, peak `1.144989014` =
  `32 * C_peak`.
- **Could not corroborate / flagged:** nothing in the supplied values is single-sourced. The
  only judgement call is the **sign convention of D[]**: pdmp3 folds the ISO per-sub-block sign
  into the table so windowing is a plain multiply (the form given here). A decoder that instead
  keeps the raw ISO Table B.3 signs must apply the spec's `if (i/64) is odd-ish` sign rule in
  step 5. The §1 algorithm and the §1.D table are a matched pair (pdmp3's) — use them together;
  do not mix this D[] table with a different windowing sign rule. The synthesis host KAT
  (impulse-response compare against the in-test reference built from the same N/D) is exactly
  the check that catches a sign/gather mismatch.
