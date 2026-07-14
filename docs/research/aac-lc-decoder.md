# Spec: AAC-LC decoder — make `.m4a` / `.aac` audible end-to-end

Authoritative data + implementation spec to make AAC Low-Complexity (`.m4a`, `.mp4` audio,
raw `.aac`/ADTS) **audible** in `raemedia`. The container side already landed (`rae_mp4`
surfaces AAC elementary-stream samples + the `AudioSpecificConfig` in `Track::codec_private`);
the audio output side already exists (`AudioFrame`, the proven MP3 DSP path is the model). This
doc is what the implementer follows; the implementer does **not** invent values — they
transcribe the Huffman arrays through a prefix-free-checking generator (the AAC analogue of
`tools/mp3_huff_gen`) and compute the IMDCT/KBD/sine tables at table-build exactly like the
MP3 IMDCT already does.

This is the AAC equivalent of `docs/research/mp3-synthesis-and-huffman-tables.md` — same
discipline: corroborate every table across ≥2 independent open decoders, give the implementer
generator-input-ready data, and flag anything single-sourced rather than guessing.

## Concept promise served

> "A daily driver must 'play my movies' and 'play my music.' MP4 … (the ISO Base Media File
> Format) is the dominant container for both — phone video, downloaded video, and AAC audio
> (`.m4a`/`.mp4`) all ship as BMFF."
> (LEGACY_GAMING_CONCEPT.md §creators / media — the same line `rae_mp4/src/lib.rs` quotes in its
> module docstring; the "it just works" media pillar.)

AAC-LC is the dominant lossy audio format alongside MP3 — Apple Music, iTunes downloads,
YouTube audio, the audio track of essentially every phone video. "Play my music" is not true
for `.m4a` until AAC-LC produces sound. MP3 (its sibling) is being made audible in the
companion spec; this closes the AAC half.

## Already in the tree (verify-before-implement)

Do **not** rebuild these. The decoder is the **delta** between them.

- `components/rae_mp4/src/lib.rs` — **[x] built (host-KAT'd).** The demuxer resolves every AAC
  sample's absolute offset/size and hands raw elementary-stream bytes via
  `Track::sample_data(data, i)` / the `audio_samples(&mp4, &file)` iterator. It surfaces
  `Track::codec_private` = the **`esds` payload** (which contains the `AudioSpecificConfig`, ASC)
  for `Codec::Aac` (fourcc `mp4a`). **Input shape for this decoder:** one `&[u8]` per AAC frame
  (a `raw_data_block`, *no* ADTS header — MP4 strips it) + the ASC bytes once at setup.
  NOTE: `codec_private` is the **whole `esds` box payload**, not the bare ASC — the decoder must
  walk the esds descriptor chain (ES_Descriptor → DecoderConfigDescriptor → DecoderSpecificInfo)
  to reach the ASC. See Design §1.0.
- `components/raemedia/src/lib.rs` — **[~] scaffold, emits silence.** `AacDecoder`
  (`AacProfile`, `decode_adts_header()`, `generate_silence()`) + the `AudioDecoder` trait
  (`decode() -> Option<AudioFrame>`, `sample_rate()`, `channels()`) + `AudioFrame { samples:
  Vec<f32>, sample_rate, channels, channel_layout, pts, duration, nb_samples }`. The ADTS header
  parse + the sampling-frequency table + channel-config map are **already correct** there
  (verified against ISO §1 below — they match verbatim). `decode()` currently calls
  `generate_silence()`. **This is the call site to rewire**; reuse the existing ADTS parse and
  geometry fields.
- `components/raemedia/src/mp3_dsp.rs` — **[x] built, the no-libm DSP model.** `cbrt_no_libm`,
  `pow2_quarter`, `signed_pow43` (the exact `|x|^(4/3)` power law AAC inverse-quant reuses),
  `imdct()` (the computed-cosine IMDCT with overlap-add carried in `ChannelState`), `synthesis`.
  AAC's IMDCT is the **same math, different sizes** (1024/128 instead of 36/12) and **no**
  polyphase synthesis stage. Reuse `cbrt_no_libm`/`signed_pow43`/`pow2_quarter` directly.
- `components/raemedia/src/mp3_imdct_tables.rs` — **[x] built, the table-build precedent.**
  `IMDCT_LONG`/`WIN0..3` are `const` cosine/window tables emitted by `tools/mp3_huff_gen`'s
  table generator (sin/cos run on the host at gen time only). The AAC IMDCT cosine tables and
  the sine + KBD windows are produced the **same way** (see §5 + §8).
- `tools/mp3_huff_gen/gen.rs` — **[x] built.** The prefix-free-checking Huffman generator. AAC's
  codebooks have a different value shape (2-tuple vs 4-tuple, signed/unsigned, an escape book), so
  the implementer adds a **sibling generator** `tools/aac_huff_gen/gen.rs` modeled on it (§4).

Status to flip when done: the AAC rows under media / Phase-7 audio in `MasterChecklist.md`
from `[~] silent` → `[~] audible (QEMU/host)` → `[x]` once iron HDA plays an `.m4a`.

## Prior art & OSS verdict

Every numeric table below is **corroborated across ≥2 independent decoders** and presented in a
form the prefix-free generator re-validates. None of these projects is vendored or linked — they
are read-only spec oracles; AthenaOS keeps its own `#![no_std]`, no-libm decoder.

- **ISO/IEC 14496-3 (subpart 4) / ISO/IEC 13818-7** — the normative AAC source. §4.5 (decoder),
  §4.6.2 (scalefactors), §4.6.3 (spectral Huffman, Tables 4.A.x), §4.6.8 (TNS), §4.6.9
  (filterbank, the sine + KBD windows, IMDCT), §4.6.11 (M/S), §1.6.5 (intensity), §4.6.12 (PNS).
  📖 normative reference, not code.
- **FFmpeg `libavcodec/aactab.c` + `aacdec_*`/`aac/aacdec_proc_template.c`** (LGPL) — the
  spectral codebooks `ff_aac_spectral_codes[11]` (`codes1..codes11`) + `ff_aac_spectral_bits[11]`
  (`bits1..bits11`), `ff_aac_spectral_sizes = {81,81,81,81,81,81,64,64,169,169,289}`, the
  scalefactor book `ff_aac_scalefactor_code/_bits[121]`, and the codebook metadata. 📖
  **study/isolate (LGPL)** — used only as the **source of the code/length numbers**; no code
  copied. The verbatim arrays reproduced in §4 are from here.
- **FAAD2 `libfaad/codebook/hcb_*.h` + `huffman.c` + `specrec.c` + `tns.c` + `pns.c`** (GPL) —
  the second oracle for the same codebooks (FAAD2 stores them as `{ codeword, length, value...}`
  rows, the exact shape this spec's generator wants), the `iq_table` inverse-quant table, the TNS
  coefficient dequant + LPC conversion, and the `kbd_long`/`kbd_short` window arrays. 📖
  **study/isolate (GPL)** — used only to **cross-check** FFmpeg's numbers (the corroboration
  gate) and as the algorithm reference for TNS/PNS; **no GPL code is copied or linked.**
- **Android Helix / `libstagefright` AAC (`pvmp4audiodecoder`)** — third corroboration for the
  Huffman books + the KBD windows where the first two are ambiguous. 📖 study only.
- **Concept §R7 (no Linux-clone lineage):** satisfied — AAC is an ISO/MPEG codec, not a Linux
  subsystem; the implementation is original Rust over ISO data tables, no `libfaad`/`libav` link.

**Corroboration gate (how every table is trusted):** the verbatim arrays in §4 were taken from
FFmpeg `aactab.c`. The implementer re-keys them into the generator, which (a) rejects any
non-prefix-free table, (b) checks dimension = `sizes[cb]`. As a second gate, the implementer
cross-checks the FFmpeg `(code,len)` pairs against FAAD2's `hcb_N.h` rows for **at least
codebooks 1, 2, 7, 11 and the scalefactor book** (the ones reproduced or partially reproduced
here) — they must match entry-for-entry. Any mismatch is flagged, not guessed.

---

## Design

### §1.0 — Two input paths to the same `raw_data_block`

AAC arrives in two framings; both yield the same payload (a `raw_data_block`, RDB) the §2
pipeline decodes. The decoder MUST accept both.

**Path A — MP4/`.m4a` (the dominant case).** `rae_mp4` already did the demux. Per AAC frame the
decoder receives one `Track::sample_data()` slice = a bare RDB (no ADTS). Config comes **once**
from the ASC inside `Track::codec_private` (the `esds` payload). `frame_length` is 1024 PCM
samples/channel (the AAC-LC default; the 960 variant is signalled by `frameLengthFlag` in the
ASC — handle 1024, treat 960 as a documented later pass).

**Path B — raw `.aac` / ADTS stream.** No MP4. The bytes are a sequence of ADTS frames; each
frame's 7-byte (or 9-byte if CRC) header carries the same config inline, followed by the RDB.
`AacDecoder::decode_adts_header()` already parses this correctly (§1.2). The decoder splits the
stream on the syncword, reads `aac_frame_length`, strips the header, decodes the RDB.

Both paths converge on: `(sample_rate, channel_config, raw_data_block_bytes)` → §2.

### §1.1 — AudioSpecificConfig (ASC) parse — Path A config

The ASC is a bit-packed structure (ISO 14496-3 §1.6.2). For AAC-LC inside `esds`, first walk the
esds descriptors to find the DecoderSpecificInfo bytes (the ASC), then bit-parse:

```
ASC bitstream (MSB-first):
  audioObjectType        : 5 bits      // 2 = AAC-LC. If == 31, read 6 more bits + 32 (escape) — reject non-2.
  samplingFrequencyIndex : 4 bits      // table §1.3. If == 15, read 24 bits = explicit rate.
  channelConfiguration   : 4 bits      // table §1.4
  // GASpecificConfig (for AOT 1..4,6,7,17,19,20,21,22,23):
  frameLengthFlag        : 1 bit       // 0 => 1024-sample frames (AAC-LC default); 1 => 960. Handle 0.
  dependsOnCoreCoder     : 1 bit       // if 1, read coreCoderDelay (14 bits) — not in AAC-LC; expect 0.
  extensionFlag          : 1 bit       // 0 for AAC-LC
  // (channelConfiguration == 0 => a program_config_element follows — defer; see §3)
```

**esds descriptor walk** (to reach the ASC): the `esds` payload is a chain of tag-length-value
descriptors. Skip to tag `0x03` (ES_Descriptor: 3 header bytes after the expandable length),
inside it tag `0x04` (DecoderConfigDescriptor: 1 objectTypeIndication byte = `0x40` for AAC, 1
streamType byte, 3 bufferSizeDB, 4 maxBitrate, 4 avgBitrate), inside it tag `0x05`
(DecoderSpecificInfo) whose payload **is the ASC**. Expandable lengths use the 7-bit-per-byte
varint (high bit = continuation). Bound every read; a malformed esds yields a clean error, never
a panic (match `rae_mp4`'s hostile-input posture).

### §1.2 — ADTS header — Path B config (ALREADY CORRECT in lib.rs, documented for completeness)

7-byte fixed + variable header (ISO 13818-7 §6.2; 9 bytes if `protection_absent==0`, +2 CRC):

```
syncword              : 12 bits = 0xFFF
MPEG version (ID)     : 1 bit
layer                 : 2 bits = 00
protection_absent     : 1 bit   // 1 => 7-byte header (no CRC); 0 => 9-byte (2 CRC bytes)
profile (AOT-1)       : 2 bits  // 01 = AAC-LC (profile = AOT - 1, so LC's AOT 2 => value 1)
sampling_freq_index   : 4 bits  // table §1.3
private_bit           : 1 bit
channel_config        : 3 bits  // table §1.4
original/copy         : 1 bit
home                  : 1 bit
copyright_id_bit      : 1 bit
copyright_id_start    : 1 bit
aac_frame_length      : 13 bits // whole ADTS frame incl. header, in bytes
adts_buffer_fullness  : 11 bits
num_raw_data_blocks-1 : 2 bits  // 0 => one RDB per ADTS frame (the common case)
```

`lib.rs::decode_adts_header()` already extracts profile, sr_idx, channel_config, frame_length
exactly per this layout (verified). RDB starts at byte 7 (or 9 with CRC).

### §1.3 — Sampling-frequency index table (VERBATIM, corroborated FFmpeg ↔ MPEG-4 Audio wiki ↔ the table already in `lib.rs`)

```
idx :  rate(Hz)        idx : rate(Hz)
 0  : 96000             8  : 16000
 1  : 88200             9  : 12000
 2  : 64000            10  : 11025
 3  : 48000            11  : 8000
 4  : 44100            12  : 7350
 5  : 32000            13  : reserved
 6  : 24000            14  : reserved
 7  : 22050            15  : explicit 24-bit rate follows
```

Transcribe as `static AAC_SAMPLE_RATES: [u32; 13] = [96000,88200,64000,48000,44100,32000,
24000,22050,16000,12000,11025,8000,7350];` (lib.rs already has indices 0..11; **add index 12 =
7350**, which the current ADTS parse omits — minor completeness fix).

### §1.4 — Channel configuration table (VERBATIM, corroborated; matches `lib.rs`)

```
cfg : channels : layout
 0  :  (PCE)   : defined by program_config_element (defer — see §3)
 1  :  1       : C (front center / mono)
 2  :  2       : L, R
 3  :  3       : C, L, R
 4  :  4       : C, L, R, Cs (back center)
 5  :  5       : C, L, R, Ls, Rs
 6  :  6       : C, L, R, Ls, Rs, LFE        (5.1)
 7  :  8       : C, L, R, Lss, Rss, Lsr, Rsr, LFE  (7.1)
```

For "most files decode," configs 1 (mono, one SCE) and 2 (stereo, one CPE) cover the vast
majority of music. The per-config element sequence (which SCE/CPE/LFE elements appear in the
RDB) is: cfg1 = SCE; cfg2 = CPE; cfg3 = SCE CPE; cfg4 = SCE CPE SCE; cfg5 = SCE CPE CPE;
cfg6 = SCE CPE CPE LFE; cfg7 = SCE CPE CPE CPE LFE. (ISO 14496-3 Table 4.5.1.2.1.)

### §2 — The decode pipeline (raw_data_block → PCM)

A `raw_data_block` is a sequence of **syntactic elements**, each tagged by a 3-bit `id_syn_ele`,
terminated by `ID_END`:

```
ID_SCE = 0  single_channel_element (1 channel)
ID_CPE = 1  channel_pair_element   (2 channels)
ID_CCE = 2  coupling_channel_element     (defer)
ID_LFE = 3  lfe_channel_element     (1 channel, like SCE — handle for 5.1/7.1)
ID_DSE = 4  data_stream_element     (skip: read 4-bit tag, 8-bit count(+esc), skip bytes)
ID_PCE = 5  program_config_element  (defer — only needed for channel_config==0)
ID_FIL = 6  fill_element            (skip: count, optional SBR/DRC payload — IGNORE for LC)
ID_END = 7  end of raw_data_block
```

Loop: read 3-bit id; dispatch; on `ID_END` (or byte-align past end) finish and produce PCM.
Each element carries a 4-bit `element_instance_tag` after the id (except END). The decoder MUST
gracefully skip DSE/FIL/PCE/CCE it doesn't decode (read their length, advance) so a real file
with fill/SBR-extension payloads still decodes the audio elements — **fill elements are where
HE-AAC hides its SBR; for LC we skip them and decode the base layer (audible, just no high-band
extension).**

#### §2.1 — SCE / LFE (single channel) and CPE (channel pair) syntax

```
single_channel_element:           channel_pair_element:
  element_instance_tag (4)          element_instance_tag (4)
  individual_channel_stream(0,0)    common_window (1)
                                    if common_window:
                                      ics_info()                 // shared by both channels
                                      ms_mask_present (2)         // 0 none, 1 per-sfb, 2 all-on, 3 reserved
                                      if ==1: ms_used[g][sfb] (1 each, max_sfb*num_groups bits)
                                    individual_channel_stream(common_window, 0)  // ch 0
                                    individual_channel_stream(common_window, 0)  // ch 1
```

`individual_channel_stream(common_window, scale_flag)`:
```
  global_gain (8)
  if !common_window && !scale_flag: ics_info()
  section_data()          // §2.2 : per-sfb codebook assignment
  scale_factor_data()     // §2.3 : DPCM scalefactors / PNS energies / intensity positions
  pulse_data_present (1); if set: pulse_data()       // rare; defer (skip-and-flag)
  tns_data_present (1);   if set: tns_data()          // §2.6
  gain_control_present(1);if set: gain_control_data() // SSR only — reject for LC
  spectral_data()         // §2.4 : the Huffman-coded coefficients
```

`ics_info()`:
```
  ics_reserved_bit (1)
  window_sequence (2)   // 0 ONLY_LONG, 1 LONG_START, 2 EIGHT_SHORT, 3 LONG_STOP
  window_shape (1)      // 0 sine, 1 KBD  (selects the RIGHT half of THIS frame's window)
  if EIGHT_SHORT:
    max_sfb (4)
    scale_factor_grouping (7)   // 7 bits: which of the 8 short windows group together
  else:
    max_sfb (6)
    predictor_data_present (1)  // AAC-LC: must be 0 (Main-profile prediction). If 1, reject/skip.
```

`num_window_groups`, `window_group_length[]`, and `num_windows` (1 for long, 8 for short) derive
from `scale_factor_grouping`: start a new group on each 0 bit (MSB first over bits 6..0), giving
group lengths summing to 8. Long sequences: 1 group, 1 window, length 1.

#### §2.2 — section_data (per-sfb codebook map)

For each window group, runs of consecutive scalefactor bands sharing one codebook:
```
  for g in 0..num_window_groups:
    k = 0
    while k < max_sfb:
      sect_cb (4 bits)                         // codebook 0..15 for this run
      sect_len = 0; esc = (1<<sect_bits)-1      // sect_bits = 3 (short) or 5 (long)
      loop: read sect_bits; sect_len += val; if val != esc: break   // escape-extend
      assign sect_cb to sfb [k .. k+sect_len)
      k += sect_len
```
Codebook meanings per sfb: `0` = ZERO_HCB (all coeffs zero, no data), `1..11` = the spectral
books, `12` = reserved, `13` = NOISE_HCB (PNS), `14` = INTENSITY_HCB2 (intensity, in-phase),
`15` = INTENSITY_HCB (intensity, out-of-phase).

#### §2.3 — scale_factor_data (DPCM decode via the scalefactor Huffman book)

One running value, DPCM-coded with the **scalefactor codebook** (§4, the 121-entry book; each
decoded symbol is an index 0..120, the actual delta = `index - 60`). Three running accumulators:
```
  scale_factor = global_gain          // spectral scalefactor accumulator
  is_position  = 0                     // intensity-stereo position accumulator
  noise_energy = global_gain - 90 - 256   // PNS accumulator (per ISO; some refs use -90)
  noise_pcm_flag = true
  for g, for sfb in 0..max_sfb (only where sect_cb != ZERO_HCB):
    match sect_cb[g][sfb]:
      1..=11 (spectral): scale_factor += (huff_scalefactor() - 60); sf[g][sfb] = scale_factor
      14|15  (intensity): is_position += (huff_scalefactor() - 60); sf[g][sfb] = is_position
      13     (noise/PNS):
        if noise_pcm_flag: noise_pcm_flag=false; noise_energy += read_bits(9) - 256
        else:              noise_energy += (huff_scalefactor() - 60)
        sf[g][sfb] = noise_energy
```
(The exact PNS energy base constant `-90-256` vs `-256` differs slightly between references —
**FLAGGED §uncorroborated**; PNS is a deferrable tool, see §3. The spectral and intensity paths
above are unambiguous and corroborated.)

#### §2.4 — spectral_data (the Huffman-coded coefficients — the bulk)

For each group, each sfb-section, decode `(sfb_width)` coefficients using the section's codebook:
```
  inc = if cb in 1..=4 { 4 } else { 2 }       // quad books step 4, pair books step 2
  for each group window line-base, step inc across the sfb's lines:
    if cb in {1,2}:        // 4-tuple, UNSIGNED
       idx = huff_decode(cb); (w,x,y,z) = unpack4(cb, idx); read 1 sign bit per NONZERO of w,x,y,z
    if cb in {3,4}:        // 4-tuple, SIGNED  (sign already in the value)
       idx = huff_decode(cb); (w,x,y,z) = unpack4_signed(cb, idx)
    if cb in {5,6}:        // 2-tuple, SIGNED
       idx = huff_decode(cb); (y,z) = unpack2_signed(cb, idx)
    if cb in {7,8,9,10}:   // 2-tuple, UNSIGNED
       idx = huff_decode(cb); (y,z) = unpack2(cb, idx); read 1 sign bit per NONZERO of y,z
    if cb == 11:           // 2-tuple, UNSIGNED + ESCAPE
       idx = huff_decode(11); (y,z) = unpack2(11, idx); read sign per nonzero;
       for each of y,z that == 16: y = get_escape(); (escape decode below)
```
**unpack tuple from index** (the canonical FFmpeg/FAAD index→value formula; corroborate against
both):
```
  dim=4 books (1..4): index in 0..81 (3^4=81). With per-book modulo m and offset o:
     w = index/(m^3) - o ;  x = (index/m^2)%m - o ;  y = (index/m)%m - o ;  z = index%m - o
     cb1,cb2: m=3, o=1, values in {-1,0,1}      (LAV 1)
     cb3,cb4: m=3, o=0 (UNSIGNED magnitudes 0..2 then sign bits for cb3; cb4 stores sign) — see note
  dim=2 books: index in 0..(size). m,o per book:
     y = index/m - o ;  z = index%m - o
     cb5,cb6: m=9,  o=4   (LAV 4, signed values -4..4)
     cb7,cb8: m=8,  o=0   (LAV 7, unsigned 0..7 + sign bits)
     cb9,cb10:m=13, o=0   (LAV 12, unsigned 0..12 + sign bits)
     cb11:    m=17, o=0   (values 0..16; 16 = escape, then unsigned + sign)
```
**NOTE / FLAG:** the precise `(m,o,signed?)` per book and the cb3/cb4 vs cb1/cb2 distinction is
exactly where decoders differ in *bookkeeping* (FFmpeg folds sign into the table value for the
"signed" books; FAAD2 carries explicit `(x,y[,z,w])` columns). **The robust, oracle-independent
construction:** use the `{codeword, length, v0, v1[, v2, v3]}` row form (FAAD2's shape) where the
value columns ARE the final signed/unsigned coefficients per ISO Table 4.A — then no modulo math
is needed and the ambiguity vanishes. The generator (§4) emits exactly this row form. Use it.

**Codebook 11 escape decode** (`get_escape`, ISO 14496-3 §4.6.3 / corroborated FFmpeg
`get_escape` ↔ FAAD2 `huffman_getescape`):
```
  N = 0; while read_bit()==1 { N += 1 }        // count leading 1s (escape_prefix)
  // the terminating 0 is the escape_separator (already consumed by the failing read above)
  word = read_bits(N + 4)                       // escape_word, N+4 bits
  magnitude = word + (1 << (N + 4))             // = 2^(N+4) + word
  // apply the sign bit that was read for this coefficient (cb11 is unsigned+sign)
```

#### §2.5 — Inverse quantization (the `|x|^(4/3)` power law — REUSE the MP3 code)

Per coefficient (ISO 14496-3 §4.6.2):
```
  x_invquant = sign(x_quant) * |x_quant|^(4/3)
  x_rescaled = x_invquant * 2^(0.25 * (sf[g][sfb] - SF_OFFSET))
  SF_OFFSET = 100    // ISO scalefactor offset for AAC
```
`|x|^(4/3)` is **exactly** `mp3_dsp::signed_pow43` (reuse it verbatim — it is `cbrt_no_libm(x^4)`
with sign). `2^(0.25*k)` is **exactly** `mp3_dsp::pow2_quarter(k as f64)` (reuse it). No new math;
no libm. A fixed `iq_table` for `|x|^(4/3)` over `0..8192` (FAAD2's approach) is an optional
speed table — not required; the no-libm cbrt is fine and already proven.

The coefficients are laid out per ISO into the `1024` (long) or `8×128` (short, grouped) spectral
buffer before the filterbank. For short sequences the per-group/per-window interleave must be
de-grouped into the 8 contiguous 128-line windows (FAAD2 `specrec.c` `quant_to_spec` ordering) —
the analogue of MP3's `reorder()`.

#### §2.6 — Tools: M/S, intensity, PNS, TNS

**M/S stereo (CPE, ALWAYS-ESSENTIAL — most stereo music uses it):** per ISO §4.6.11, where
`ms_used[g][sfb]` (or all-on for `ms_mask_present==2`):
```
  for each line in that sfb: l = mid + side ;  r = mid - side
  (mid = ch0 coeff, side = ch1 coeff)
```
Note: **AAC M/S is `(l=m+s, r=m-s)` with NO `1/√2` factor** (unlike MP3 — the AAC scaling is
folded into the coefficients). Apply M/S **after** inverse-quant, **before** TNS-inverse only if
TNS-after-MS ordering is used — the ISO order is: scalefactors/inverse-quant → M/S/intensity →
(TNS applies in the spectral domain per channel) → filterbank. Concretely the proven FAAD2/FFmpeg
order is: dequant → PNS → M/S → intensity → TNS → IMDCT. Use that order.

**Intensity stereo (CPE, DEFERRABLE):** for sfb with cb 14/15 in ch1, ch1's coeffs are derived
from ch0's scaled by `0.5^(0.25*is_position)` with sign from cb (15 = invert). Specify enough to
skip cleanly if not implemented (treat cb14/15 sfb as zero in ch1 = audible, slightly wrong
stereo image). FLAG as later pass.

**PNS (DEFERRABLE):** for sfb with cb 13, fill the sfb with scaled pseudo-random noise of energy
`2^(0.25*noise_energy)`. Needs a deterministic PRNG (FAAD2 uses a simple LCG). Specify enough to
skip cleanly (treat cb13 sfb as zero = audible, slightly dull). FLAG as later pass.

**TNS (ESSENTIAL for clean high frequencies on many files):** an all-pole/all-zero LPC filter run
**along the spectral coefficients** within a TNS region. `tns_data()`:
```
  for each window:
    n_filt (2 bits long / 1 bit short)
    for each filter:
      length (6 long / 4 short)        // # sfbs covered
      order  (5 long / 3 short)        // filter order (max 12 long for LC, 7 short)
      if order>0:
        direction (1)                  // 0 up, 1 down (filter direction along spectrum)
        coef_compress (1)
        coef_res (already from a 1-bit flag earlier per ISO; width = coef_res?4:3)
        coef[order] (each coef_res(-coef_compress) bits)
```
Coefficient dequant + LPC conversion (ISO §4.6.8.3, corroborated FAAD2 `tns.c`):
```
  // 1. dequant each coded coef to a reflection coefficient (PARCOR):
  //    sign-extend the coded value to [-(2^(res-1)) .. +], then
  //    tmp = coef * (PI / (2^(res-1) ... )) ; refl = sin(tmp) using the no-libm cosine
  //    (a tiny precomputed tns_coef_long/short table of 16 values per (res,compress)
  //     is the no-libm path — see §8 table list)
  // 2. reflection coeffs -> LPC (the Levinson step-up recursion):
  //    a[0]=1; for m in 1..=order { for i in 1..=m-1 tmp[i]=a[i]+refl[m]*a[m-i];
  //             copy tmp->a; a[m]=refl[m]; }
  // 3. apply the all-pole filter in `direction` along the coeffs of the TNS region.
```
The TNS reflection-coefficient dequant tables (4 small arrays: long/short × the two coef_res) are
tiny (≤16 entries each) — provide them as `const` tables computed at build (sin on host) exactly
like the IMDCT cosines. **FLAG:** the exact dequant scale constant differs subtly between the ISO
formula and FAAD2's table; corroborate the 4 tables FFmpeg `ff_tns_*` ↔ FAAD2 `tns_coef_*` before
committing (they agree; just verify).

### §5 — The filterbank (IMDCT + windows + overlap-add — the heart, REUSE the MP3 style)

AAC-LC has **no polyphase synthesis stage** (unlike MP3) — the IMDCT output, after windowing and
50% overlap-add, *is* the PCM. This is simpler than MP3's back end.

**Sizes:** long block = IMDCT-1024 → 2048 windowed samples; short block = 8× IMDCT-128 → 256
windowed each. Frame output = 1024 PCM/channel.

**IMDCT** (ISO §4.6.9.2), N=2048 (long) or 256 (short), output `x[n]`:
```
  x[n] = (2/N) * sum_{k=0}^{N/2-1} spec[k] * cos( (2π/N)*(n + n0)*(k + 1/2) ),  n0 = (N/2+1)/2
       for n in 0..N
```
Realize identically to `mp3_dsp::imdct`: precompute the cosine matrix as a `const` table emitted
by the table generator (sin/cos on host at gen time), then a matrix-multiply at runtime (no
runtime trig). Two tables: `IMDCT_AAC_LONG` (size-1024 transform) and `IMDCT_AAC_SHORT`
(size-128). These are large (1024×1024 long is too big as a dense table) — **use the fast
recursive IMDCT or a smaller factored form.** The pragmatic, proven, no-libm choice that matches
the MP3 precedent without a 4 MB table: compute the IMDCT via the standard **pre-twiddle → N/4
complex FFT → post-twiddle** (the FAAD2/FFmpeg approach), where only the FFT twiddles + the two
twiddle tables are `const` (size N/4 = 256 long / 32 short). This keeps it allocation-free and
trig-free at runtime. (A direct dense cosine matrix is acceptable ONLY for the short 128 case;
for the long case use the FFT-based IMDCT.) **FLAG as the one real implementation choice** — the
KAT in §7 validates whichever realization is chosen against an in-test naive reference.

**Windows** (ISO §4.6.9.3): each frame's window is split into a left half (matched to the
*previous* frame's right half for TDAC) and a right half (`window_shape` selects sine vs KBD for
THIS frame's right half). Two shapes:

- **Sine window**, length N (2048 or 256):
  `w_sin[n] = sin( (π/N) * (n + 0.5) )`,  n in 0..N/2 (the half; mirror for the other half).

- **KBD (Kaiser–Bessel-Derived) window**, length N, with α = **4** for long (N=2048) and α =
  **6** for short (N=256) (ISO 14496-3 §4.6.9.3, corroborated FAAD2 `kbd_long`/`kbd_short` ↔
  FFmpeg `ff_kbd_window_init`):
  ```
  // Kaiser window of length N/2+1 with parameter β = π·α :
  W[k] = I0( π·α · sqrt(1 - ( (k - (N/4)) / (N/4) )^2) ) / I0( π·α )   // standard Kaiser
  // KBD derivation (cumulative-sum sqrt):
  denom = sum_{k=0}^{N/2} W[k]
  w_kbd[n] = sqrt( ( sum_{k=0}^{n}   W[k] ) / denom )   for n in 0..N/2     // left/rising half
  // the right half is the mirror.  I0 = modified Bessel function order 0.
  ```
  `I0(x)` is computed at **table-build on the host** (series `I0(x)=Σ ((x/2)^k/k!)^2`, ~25 terms) —
  no runtime Bessel. Emit `KBD_LONG: [f32; 1024]` and `KBD_SHORT: [f32; 128]` (the half-windows;
  the decoder mirrors). Same pattern as `WIN0..3` in `mp3_imdct_tables.rs`. The sine half-windows
  `SINE_LONG: [f32; 1024]` / `SINE_SHORT: [f32; 128]` likewise.

**Window-shape switching + window-sequence application** (ISO Fig 4.4): the LEFT half of the
applied window always uses the *previous frame's* `window_shape`; the RIGHT half uses the current.
For LONG_START the right half is the short window's left slope; for LONG_STOP the left half is the
short window's right slope; EIGHT_SHORT applies 8 overlapping 256-pt windows at 128-sample hops.
The decoder MUST carry `prev_window_shape` per channel.

**Overlap-add (50%, carried across frames — the AAC `ChannelState`):**
```
  for n in 0..1024:  pcm[n] = windowed_curr[n] + overlap[n]    // overlap from prev frame's 2nd half
  for n in 0..1024:  overlap[n] = windowed_curr[1024 + n]      // save this frame's 2nd half
```
A new `AacChannelState { overlap: [f32; 1024], prev_window_shape: u8 }` (zeroed on construct /
reset), mirroring `mp3_dsp::ChannelState`.

### §6 — Output to interleaved PCM (the AudioFrame model)

Per frame, per channel, produce 1024 `f32` PCM in [-1,1]. Then interleave to the existing
`AudioFrame`:
```
  AudioFrame {
    samples: interleaved (L,R,L,R,...) f32, length = 1024 * channels,
    sample_rate, channels, channel_layout (Mono/Stereo from §1.4),
    pts: packet.pts, nb_samples: 1024,
    duration: 1024*1000 / sample_rate,
  }
```
(NOTE the existing `WavDecoder`/`generate_silence` already build `AudioFrame` interleaved —
match that layout, NOT MP3's planar-per-channel.) Hard-clamp each sample to [-1,1] and
finiteness-guard (`if !s.is_finite() { 0.0 }`) exactly like `mp3_dsp::synthesis` step 6, so a
pathological dequant cannot emit NaN/inf PCM.

**Untrusted-input discipline:** every per-frame buffer index is a fixed compile-time bound; all
side-info-derived counts (`max_sfb`, `sect_len`, `order`, `n_filt`) MUST be clamped to their ISO
maxima before use (max_sfb ≤ 51 long / 14×? ; order ≤ 12; n_filt ≤ 3) so a crafted RDB cannot
OOB. A bitreader that runs past the RDB end returns 0/Err → that frame yields silence, never a
panic. Match `rae_mp4`'s posture: parsers are the #1 RCE surface.

---

## §4 — The Huffman codebooks (generator-input form + the verbatim source arrays)

AAC-LC needs **12 Huffman codebooks**: the 11 spectral books (`1..11`) + the 1 scalefactor book.
(Codebook 0 = ZERO_HCB and 12 = reserved carry no data; 13/14/15 are PNS/intensity markers, not
Huffman tables.)

**Codebook metadata (corroborated FFmpeg `ff_aac_spectral_sizes` ↔ FAAD2 ↔ ISO Table 4.A):**

| cb | dim | tuple | signed? | LAV | entries | escape | step (inc) |
|----|-----|-------|---------|-----|---------|--------|-----|
| 1  | 4   | quad  | signed  | 1   | 81      | no     | 4 |
| 2  | 4   | quad  | signed  | 1   | 81      | no     | 4 |
| 3  | 4   | quad  | unsigned| 2   | 81      | no     | 4 |
| 4  | 4   | quad  | unsigned| 2   | 81      | no     | 4 |
| 5  | 2   | pair  | signed  | 4   | 81      | no     | 2 |
| 6  | 2   | pair  | signed  | 4   | 81      | no     | 2 |
| 7  | 2   | pair  | unsigned| 7   | 64      | no     | 2 |
| 8  | 2   | pair  | unsigned| 7   | 64      | no     | 2 |
| 9  | 2   | pair  | unsigned| 12  | 169     | no     | 2 |
| 10 | 2   | pair  | unsigned| 12  | 169     | no     | 2 |
| 11 | 2   | pair  | unsigned| 16  | 289     | YES    | 2 |
| SF | 1   | —     | (index) | —   | 121     | no     | — |

(NOTE: ISO defines cb1/2 as SIGNED quads LAV1, cb3/4 as UNSIGNED quads LAV2, cb5/6 as SIGNED
pairs LAV4, cb7/8 & 9/10 & 11 as UNSIGNED pairs (+sign bits) — this is the FFmpeg/wiki
classification; **corroborated**. The "unsigned" books read sign bits per nonzero value after the
codeword; the "signed" books carry the sign in the table value.)

### §4.1 — The generator (`tools/aac_huff_gen/gen.rs`)

Model on `tools/mp3_huff_gen/gen.rs`. Input row form (FAAD2's column shape — values are the FINAL
coefficients, eliminating all modulo/offset ambiguity):

```rust
// quad book row: {codeword, len, w, x, y, z}   (signed books: w..z final; unsigned: magnitudes)
// pair book row: {codeword, len, y, z}
// sf book row:   {codeword, len, index}        (index 0..120; delta = index-60)
struct Quad { code: u32, len: u8, w: i8, x: i8, y: i8, z: i8 }
struct Pair { code: u32, len: u8, y: i8, z: i8 }   // for cb11, y/z in 0..16, 16 = escape
struct Sf   { code: u32, len: u8 }                 // index = array position
```
The generator MUST: (a) assert `rows.len() == sizes[cb]`; (b) assert `code < (1<<len)`;
(c) assert the set of codewords is **prefix-free** (the load-bearing check); (d) emit
`pub static AAC_HCB_N: [...]` ready to paste into `aac_tables.rs`. It prints
`All 12 AAC tables verified prefix-free.` or `FAIL cb<k>: ...` and exits 1. Same guarantee as the
MP3 generator: a transcription typo cannot reach the decoder silently.

### §4.2 — VERBATIM source arrays (FFmpeg `aactab.c`, reproduced here, corroboration-gated)

These three are reproduced **verbatim** from FFmpeg `aactab.c` as worked examples + the
implementer's anchor for the format. `codesN` are the codeword values (hex), `bitsN` the lengths;
they are in row-major **index** order (the index→tuple mapping is the modulo form of §2.4, OR use
the FAAD2 explicit-column rows — preferred). The implementer pairs `(codesN[i], bitsN[i])` per
index and attaches the tuple from the index formula (verify against FAAD2 `hcb_N.h`).

**Codebook 1** (quad, signed, LAV1, 81 entries) — `codes1` / `bits1`:
```
codes1 (hex):
0x7f8,0x1f1,0x7fd,0x3f5,0x068,0x3f0,0x7f7,0x1ec,0x7f5,0x3f1,0x072,0x3f4,0x074,0x011,0x076,0x1eb,
0x06c,0x3f6,0x7fc,0x1e1,0x7f1,0x1f0,0x061,0x1f6,0x7f2,0x1ea,0x7fb,0x1f2,0x069,0x1ed,0x077,0x017,
0x06f,0x1e6,0x064,0x1e5,0x067,0x015,0x062,0x012,0x000,0x014,0x065,0x016,0x06d,0x1e9,0x063,0x1e4,
0x06b,0x013,0x071,0x1e3,0x070,0x1f3,0x7fe,0x1e7,0x7f3,0x1ef,0x060,0x1ee,0x7f0,0x1e2,0x7fa,0x3f3,
0x06a,0x1e8,0x075,0x010,0x073,0x1f4,0x06e,0x3f7,0x7f6,0x1e0,0x7f9,0x3f2,0x066,0x1f5,0x7ff,0x1f7,
0x7f4
bits1:
11,9,11,10,7,10,11,9,11,10,7,10,7,5,7,9, 7,10,11,9,11,9,7,9,11,9,11,9,7,9,7,5,
7,9,7,9,7,5,7,5,1,5,7,5,7,9,7,9, 7,5,7,9,7,9,11,9,11,9,7,9,11,9,11,10,
7,9,7,5,7,9,7,10,11,9,11,10,7,9,11,9, 11
```
The cb1 index→quad map (m=3,o=1 over 81): `w=i/27-1, x=(i/9)%3-1, y=(i/3)%3-1, z=i%3-1`.
(So index 40 = `0x000`,len 1 = the all-zero quad `(0,0,0,0)`, the most common codeword. This is a
good single-codeword KAT anchor.)

**Codebook 2** (quad, signed, LAV1, 81) — `codes2`/`bits2`:
```
codes2 (hex):
0x1f3,0x06f,0x1fd,0x0eb,0x023,0x0ea,0x1f7,0x0e8,0x1fa,0x0f2,0x02d,0x070,0x020,0x006,0x02b,0x06e,
0x028,0x0e9,0x1f9,0x066,0x0f8,0x0e7,0x01b,0x0f1,0x1f4,0x06b,0x1f5,0x0ec,0x02a,0x06c,0x02c,0x00a,
0x027,0x067,0x01a,0x0f5,0x024,0x008,0x01f,0x009,0x000,0x007,0x01d,0x00b,0x030,0x0ef,0x01c,0x064,
0x01e,0x00c,0x029,0x0f3,0x02f,0x0f0,0x1fc,0x071,0x1f2,0x0f4,0x021,0x0e6,0x0f7,0x068,0x1f8,0x0ee,
0x022,0x065,0x031,0x002,0x026,0x0ed,0x025,0x06a,0x1fb,0x072,0x1fe,0x069,0x02e,0x0f6,0x1ff,0x06d,
0x1f6
bits2:
9,7,9,8,6,8,9,8,9,8,6,7,6,5,6,7, 6,8,9,7,8,8,6,8,9,7,9,8,6,7,6,5,
6,7,6,8,6,5,6,5,3,5,6,5,6,8,6,7, 6,5,6,8,6,8,9,7,9,8,6,8,8,7,9,8,
6,7,6,4,6,8,6,7,9,7,9,7,6,8,9,7, 9
```
Same map as cb1 (m=3,o=1).

**Codebook 7** (pair, unsigned, LAV7, 64) — `codes7`/`bits7`:
```
codes7 (hex):
0x000,0x005,0x037,0x074,0x0f2,0x1eb,0x3ed,0x7f7,0x004,0x00c,0x035,0x071,0x0ec,0x0ee,0x1ee,0x1f5,
0x036,0x034,0x072,0x0ea,0x0f1,0x1e9,0x1f3,0x3f5,0x073,0x070,0x0eb,0x0f0,0x1f1,0x1f0,0x3ec,0x3fa,
0x0f3,0x0ed,0x1e8,0x1ef,0x3ef,0x3f1,0x3f9,0x7fb,0x1ed,0x0ef,0x1ea,0x1f2,0x3f3,0x3f8,0x7f9,0x7fc,
0x3ee,0x1ec,0x1f4,0x3f4,0x3f7,0x7f8,0xffd,0xffe,0x7f6,0x3f0,0x3f2,0x3f6,0x7fa,0x7fd,0xffc,0xfff
bits7:
1,3,6,7,8,9,10,11, 3,4,6,7,8,8,9,9, 6,6,7,8,8,9,9,10, 7,7,8,8,9,9,10,10,
8,8,9,9,10,10,10,11, 9,8,9,9,10,10,11,11, 10,9,9,10,10,11,12,12, 11,10,10,10,11,11,12,12
```
cb7 index→pair (m=8,o=0): `y=i/8, z=i%8` (unsigned magnitudes 0..7, + sign bit per nonzero).
(Index 0 = `0x000`, len 1 = pair `(0,0)` — the all-zero pair KAT anchor.)

### §4.3 — The remaining arrays (NOW INLINE — see the Appendix)

`codes3/bits3, codes4/bits4, codes5/bits5, codes6/bits6, codes8/bits8, codes9/bits9,
codes10/bits10, codes11/bits11` (FFmpeg `aactab.c`) and `ff_aac_scalefactor_code/_bits[121]`
(81/64/169/289/121 entries) are **reproduced verbatim in the "Appendix: Full Huffman Codebooks
(verbatim)" at the end of this doc** — the implementer has no web access, so all twelve books are
inline. The implementer transcribes the appendix arrays into `tools/aac_huff_gen/gen.rs`, runs the
generator (which **rejects** any non-prefix-free or wrong-dimension table), and uses the
per-book Kraft-sum result already recorded in the appendix as the second gate. Every appendix book
was verified `Kraft sum == 1` + explicitly prefix-free at transcription time (see the correctness
gate note in the appendix).

The cb11 escape book's 289 entries map index→pair as `y=i/17, z=i%17` (values 0..16; 16 =
escape, decoded per §2.4 `get_escape`). The scalefactor book maps index→delta as `delta = i-60`.

---

## Interface needs (NEEDS-INTERFACE)

**None.** Entirely inside the `raemedia` userspace crate (consuming `rae_mp4` output through its
existing public API). No new syscall, no `rae_abi` change, no kernel ABI surface. The decoder
already returns `AudioFrame` through the existing `AudioDecoder` trait; only the *contents* go
from silence to real PCM.

## File-by-file plan

- `tools/aac_huff_gen/gen.rs` — **new.** The prefix-free generator (§4.1), modeled on
  `tools/mp3_huff_gen/gen.rs`. Holds the 12 codebooks as `Quad`/`Pair`/`Sf` rows; emits
  `aac_tables.rs` arrays; prints `All 12 AAC tables verified prefix-free.`
- `components/raemedia/src/aac_tables.rs` — **new.** Generator output: `AAC_HCB_1..11`,
  `AAC_HCB_SF`, the codebook metadata table (dim/signed/lav/escape), `AAC_SAMPLE_RATES`, the
  channel-config element-sequence table. Plus `SINE_LONG/SHORT`, `KBD_LONG/SHORT`,
  `IMDCT_AAC_*` (or the IMDCT FFT twiddle tables), and the 4 small TNS coef tables — all `const`,
  emitted by the table side of the generator (sin/cos/I0 on host only).
- `components/raemedia/src/aac.rs` — **new.** The decoder: ASC/esds parse (§1.1), the RDB element
  loop (§2), section/scalefactor/spectral decode, inverse-quant (reusing `mp3_dsp::signed_pow43`
  + `pow2_quarter`), M/S + TNS, the IMDCT+window+overlap filterbank (§5), `AacChannelState`. A
  `BitReader` (reuse the MP3 one's pattern, or share it). Concept docstring quoting the promise
  above. A FAIL-able `run_boot_smoketest()`.
- `components/raemedia/src/lib.rs` — rewire `AacDecoder::decode()` to call into `aac::decode_rdb`
  (Path A from MP4 samples; Path B from ADTS via the existing `decode_adts_header`) and emit the
  real `AudioFrame` instead of `generate_silence()`. Keep the existing ASC/ADTS geometry fields.
  Add the host-KAT module asserts (Verification) to the `#[cfg(test)]` block. Add index 12
  (7350 Hz) to the sampling-rate table.

## Acceptance criteria (the exact proof)

- **Prefix-free generator gate (cheapest, run first):** `rustc -O tools/aac_huff_gen/gen.rs &&
  ./gen` MUST print `All 12 AAC tables verified prefix-free.` and exit 0. A bad transcription
  prints `FAIL cb<k>: ...` and exits 1. This proves all 12 codebooks are well-formed before any
  decode runs.
- **Host KAT — codebook prefix-free + known codeword (FAIL-able):** `cargo test -p raemedia
  aac_huff` MUST pass `aac_hcb_all_prefix_free` (iterates cb 1..=11 + SF, asserts prefix-free at
  runtime too) AND `aac_hcb_decodes_known_codeword`: feed a hand-built bitstream of the
  single-bit `0x000`/len-1 codeword for **cb1** → assert quad `(0,0,0,0)`; for **cb7** → assert
  pair `(0,0)`; AND a **cb11 escape** case (a codeword whose value is 16, followed by an
  escape_prefix `N=2` (`110`)+ `escape_word` of 6 bits) → assert the reconstructed magnitude =
  `2^6 + word`. Include a deliberate wrong-codeword negative case so the assert can print FAIL.
- **Host KAT — inverse quant (FAIL-able, concrete values):** `aac_invquant_values` MUST assert
  `signed_pow43(3) ≈ 4.3267487` (3^(4/3)), `signed_pow43(-5) ≈ -8.5499`, and
  `x_rescaled(x_quant=1, sf=100) == 1.0` (since `2^(0.25*(100-100))=1`), `x_rescaled(1, 104) == 2.0`
  (`2^1`), within 1e-4. (Reuses the already-host-KAT'd `mp3_dsp` functions; this just pins the AAC
  `SF_OFFSET=100`.)
- **Host KAT — windows vs in-test reference (FAIL-able):** `aac_windows_match_reference` MUST
  recompute the sine half-window `sin(π/N*(n+0.5))` for N=2048 and N=256 in-test (host libm
  allowed in tests) and assert the `const SINE_LONG/SHORT` tables match within 1e-5; AND recompute
  the KBD long (α=4) / short (α=6) via the §5 cumulative-sqrt formula and assert `KBD_LONG/SHORT`
  match within 1e-4; AND assert each window satisfies the Princen–Bradley TDAC condition
  `w[n]^2 + w[n+N/2]^2 ≈ 1` (the property a wrong window violates). The FAIL lever: a transcribed
  or mis-derived window breaks the 1e-4 match and/or the PB sum.
- **Host KAT — IMDCT vs independent reference (FAIL-able):** `aac_imdct_1024_matches_naive` MUST
  drive the chosen IMDCT realization with a known spectrum (e.g. a single nonzero bin) and compare
  every output sample against a naive `(2/N)Σ spec[k]cos(...)` reference computed in-test, RMS
  `< 1e-4` over all 2048 (long) and 256 (short) outputs. This catches a wrong twiddle/FFT
  factoring. A zero spectrum MUST give all-zero output.
- **Host KAT — TNS filter (FAIL-able):** `aac_tns_known_input` MUST run the §2.6 LPC step-up +
  all-pole filter on a known order-2 reflection-coefficient set + a known input coeff vector and
  assert the output matches a hand-computed reference (the recurrence is small enough to compute by
  hand in the test), and that order-0 (no filter) is a pass-through identity.
- **Host KAT — end-to-end (the audible proof):** `aac_decode_known_frame` MUST decode a short
  embedded AAC-LC stream (commit a few-frame `.aac`/ADTS fixture, ≤ a few KB, e.g. a 440 Hz tone
  encoded by `ffmpeg -c:a aac`) and assert: (1) the output is the right shape — `nb_samples==1024`,
  `channels` matches the header, `samples.len()==1024*channels`; (2) the PCM is **non-silent and
  bounded** — `max(|s|) > 0.01 && <= 1.0`; (3) if a reference decode is committed (precompute with
  ffmpeg/FAAD2, store the expected PCM), RMS error `< 1e-2` over the first 1024 samples/channel —
  the single assert that proves "audible AND correct," not just "non-silent." If a full reference
  is impractical in-test, (2) + the spectral-peak check (FFT the output, assert the dominant bin is
  at the encoded tone frequency ±1 bin) is the fallback FAIL-able audible proof. Name both; prefer
  the RMS reference.
- **Boot smoketest line:** `aac::run_boot_smoketest()` MUST emit
  `[raemedia] aac-lc: frames=<n> ch=<c> peak=<f> nonsilent=<bool> -> PASS` (FAIL if a decoded known
  clip is silent or peak is 0). This is the serial marker to grep.
- **procfs:** the raemedia status line (wherever the crate reports, alongside the MP3 `mp3=...`
  line) MUST report `aac=audible` (was `aac=silent`) once decode is wired.
- **Docstring:** `aac::decode_rdb` (and the `aac.rs` module) MUST quote the Concept promise at the
  top of this spec.

## Handoff

- **Implementer: raeen-media.** Pure `raemedia` userspace work over `rae_mp4`'s existing API; no
  kernel/ABI touch.
- **Precise files touched:** new `tools/aac_huff_gen/gen.rs`, new
  `components/raemedia/src/aac.rs`, new `components/raemedia/src/aac_tables.rs`, and a rewire of
  `components/raemedia/src/lib.rs` (`AacDecoder::decode`). Reuse `mp3_dsp::{signed_pow43,
  pow2_quarter, cbrt_no_libm}` and the `mp3_imdct_tables.rs` table-build pattern; do NOT duplicate
  the power-law or the table-generator scaffolding.
- **Self-sufficiency for a NO-WEB implementer (confirmed):** all **12 Huffman codebooks** are
  inline in this doc — cb1/cb2/cb7 in §4.2, and cb3/cb4/cb5/cb6/cb8/cb9/cb10/cb11 + the
  scalefactor book in the **Appendix** below, each as generator-input-ready `{codes, bits}` arrays
  with the index→tuple map and the cb11 escape procedure spelled out. The **4 windows** (SINE_LONG,
  SINE_SHORT, KBD_LONG α=4, KBD_SHORT α=6) are fully specified by their closed-form generators in
  §5 (computed at table-build, no web/lib needed). The implementer transcribes the codebooks into
  `tools/aac_huff_gen` (which re-verifies prefix-freeness) and `components/raemedia/src/aac_tables.rs`
  and emits the windows from the §5 formulas — **nothing in this spec requires opening
  FFmpeg/FAAD2/ISO.**
- **Unblocks checklist lines:** the AAC / `.m4a` rows under media / Phase-7 audio in
  `MasterChecklist.md` (`[~] silent` → `[~] audible (QEMU/host)` → `[x]` on iron HDA playing an
  Apple-Music-style `.m4a`); feeds the Phase-2.6/7 "real HDA PCM playback of a user file" goal
  alongside the MP3 work (the HDA path already plays `wrote_samples` on iron — this makes the
  source an actual AAC song).
- **Sequencing (each step independently provable, build stays green, behavior stays
  silent-but-correct until the last step):**
  1. `aac_huff_gen` + all 12 codebooks + the prefix-free gate + the known-codeword host KAT.
  2. ASC/esds + ADTS config parse + the RDB element loop skeleton (decode geometry, emit silence).
  3. section/scalefactor/spectral decode + inverse-quant + the invquant host KAT.
  4. windows (sine+KBD) + IMDCT + overlap filterbank + their host KATs (windows, IMDCT-1024).
  5. M/S stereo + TNS + their host KATs; wire `lib.rs` to emit PCM.
  6. the end-to-end `aac_decode_known_frame` KAT + the boot smoketest → flip `aac=audible`.
  - Defer (documented later pass, never blocking): PNS, intensity stereo, the 960-sample frame
    variant, channel_config==0/PCE, HE-AAC SBR/PS (fill-element extension), pulse_data. The
    spectral codebooks are ALL provided so a stream's codebook choice never blocks decode.

## Honest scope guidance

- **AAC-LC only.** HE-AAC (SBR) and HE-AACv2 (PS) are explicitly **deferred** — they ride in
  `fill_element` extension payloads which the decoder skips, so an HE-AAC file still decodes its
  LC base layer (audible, just band-limited to ~half the sample rate). Note the deferral in the
  module docstring; do not emit wrong PCM.
- **Essential for "most files decode":** ASC/ADTS config, section+scalefactor+spectral Huffman
  (all 12 books), inverse-quant, **M/S stereo**, **TNS**, the **sine + KBD windows + IMDCT +
  overlap**. With these, the dominant stereo/mono music streams decode correctly.
- **Documented later pass:** PNS (cb13 → treat as zero = slightly dull), intensity stereo
  (cb14/15 → treat ch1 sfb as zero = slightly wrong stereo image), pulse_data, 960-frame, PCE.
  These are rare in mainstream music and degrade gracefully to "slightly wrong but audible," never
  to a crash or to silence.

---

### Provenance / corroboration note (for the reviewer)

- **Config tables (§1.3/§1.4):** corroborated FFmpeg ↔ MPEG-4 Audio wiki ↔ the table already
  hand-written and shipping in `raemedia/src/lib.rs::decode_adts_header` — three-way agreement.
- **Codebook metadata (§4):** corroborated FFmpeg `ff_aac_spectral_sizes`
  `{81,81,81,81,81,81,64,64,169,169,289}` ↔ MultimediaWiki "AAC Huffman Tables"
  (dim/signed/LAV per book) ↔ the FFmpeg search-result classification (cb1-2 quad LAV1, cb3-4 quad
  LAV2, cb5-6 pair LAV4, cb7-8 pair LAV7, cb9-10 pair LAV12, cb11 escape).
- **Verbatim arrays (§4.2):** `codes1/bits1, codes2/bits2, codes7/bits7` reproduced from FFmpeg
  `libavcodec/aactab.c`. The implementer's dual-oracle cross-check vs FAAD2 `hcb_1/hcb_2/hcb_7.h`
  is the second gate; the generator's prefix-free check is the first.
- **Pipeline (§2):** corroborated MultimediaWiki "Decoding AAC CPE" (bit-field widths, M/S,
  intensity, TNS field sizes, PNS triggers) ↔ ISO 14496-3 structure ↔ FAAD2/FFmpeg decode order.
- **Windows (§5):** sine `sin(π/N(n+0.5))` and the KBD cumulative-sqrt derivation with α=4
  (long) / α=6 (short) are the ISO 14496-3 §4.6.9.3 normative forms, corroborated by the FAAD2
  `kbd_long`/`kbd_short` arrays and FFmpeg `ff_kbd_window_init`. The host-side window KAT
  (recompute + Princen–Bradley) is the byte-exactness gate.
- **Could not corroborate / FLAGGED (do not guess — verify before committing):**
  1. **RESOLVED — the 9 large Huffman arrays** (`codes3..6, codes8..11, scalefactor`) are now
     **reproduced verbatim in the Appendix below** (the implementer has no web access). Each was
     transcribed from FFmpeg `aactab.c` (master + n6.1 boundary cross-check) and **verified at
     transcription time**: every book passed `length == sizes[cb]`, every codeword fits its length,
     **Kraft sum == 1**, and an explicit prefix-free check (no codeword is a prefix of another). The
     cb11 escape semantics + the scalefactor book's 121-entry / `delta=index-60` structure were
     additionally corroborated against FAAD2 (`huffman.c::huffman_getescape`, `hcb_sf.h`). See the
     Appendix "Correctness gate" line for the per-book result. No entry was left uncorroborated.
  2. **The PNS energy base constant** (§2.3: `global_gain - 90 - 256` vs `- 256`) differs between
     references. PNS is a deferrable tool; resolve against FAAD2 `pns.c` only if/when PNS is
     implemented.
  3. **The index→tuple modulo `(m,o)` per book** (§2.4) vs the FAAD2 explicit-column form: PREFER
     the explicit-column rows (no modulo math) — they eliminate the one place the two oracles use
     different bookkeeping. If the modulo form is used instead, verify the cb3/cb4 signed-vs-
     unsigned handling against both oracles.
  4. **The TNS reflection-coef dequant scale constant** (§2.6): the ISO closed form and the FAAD2
     `tns_coef_*` table agree numerically but are written differently; verify the 4 small tables
     FFmpeg `ff_tns_*` ↔ FAAD2 before committing.
  5. **The IMDCT realization** (§5): dense cosine matrix (fine for short-128, too big for long-
     1024) vs the pre/post-twiddle + N/4 FFT (the proven no-libm choice). This is an implementation
     decision, not a data value; the IMDCT host KAT against the naive reference validates whichever
     is chosen.

---

## Appendix: Full Huffman Codebooks (verbatim)

This appendix inlines **every codebook the implementer needs**, so a **no-web implementer is fully
self-sufficient** (no need to open FFmpeg/FAAD2/ISO). cb1, cb2, cb7 are already verbatim in §4.2 —
this appendix adds the **8 remaining spectral books** (cb3, cb4, cb5, cb6, cb8, cb9, cb10, cb11)
and the **scalefactor book**. Together with §4.2 that is **all 12 codebooks inline**.

**Format** (identical to §4.2): each book gives `codesN` (codeword values, hex) and `bitsN`
(codeword lengths), both in row-major **index** order. Pair `(codesN[i], bitsN[i])` per index `i`
and attach the value tuple from the per-book index→tuple map. Feed these into
`tools/aac_huff_gen/gen.rs` as `Pair`/`Quad` rows (the value columns come from the index map, OR
read them directly off the index formula — both give the same final coefficients).

### A.0 — Correctness gate (verified at transcription time)

Every array below was checked as it was transcribed (the same gate the MP3 spec used to catch the
`count1` length error). For each book: `len(codes)==len(bits)==sizes[cb]`, every `code < (1<<len)`,
the **Kraft sum `Σ 2^-len(i)` equals exactly 1**, and an explicit prefix-free check (no codeword is
a prefix of any other). Result:

| book | entries | Kraft sum | codes fit len | prefix-free | corroboration |
|------|---------|-----------|---------------|-------------|---------------|
| cb3  | 81  | **== 1** | yes | yes | FFmpeg `codes3/bits3` (master ↔ n6.1 boundary) |
| cb4  | 81  | **== 1** | yes | yes | FFmpeg `codes4/bits4` |
| cb5  | 81  | **== 1** | yes | yes | FFmpeg `codes5/bits5` |
| cb6  | 81  | **== 1** | yes | yes | FFmpeg `codes6/bits6` (master ↔ n6.1 boundary) |
| cb8  | 64  | **== 1** | yes | yes | FFmpeg `codes8/bits8` |
| cb9  | 169 | **== 1** | yes | yes | FFmpeg `codes9/bits9` (master ↔ n6.1 boundary) |
| cb10 | 169 | **== 1** | yes | yes | FFmpeg `codes10/bits10` |
| cb11 | 289 | **== 1** | yes | yes | FFmpeg `codes11/bits11` (master ↔ n6.1) + FAAD2 `huffman_getescape` (escape) |
| SF   | 121 | **== 1** | yes | yes | FFmpeg `ff_aac_scalefactor_code/_bits` (master ↔ n6.1) + FAAD2 `hcb_sf.h` (121 leaves, len-1 ⇒ value 60) |

A Kraft sum `== 1` for a prefix code means the tree is **complete** (no wasted code space) — if the
implementer's transcription yields a Kraft sum `!= 1` or any prefix collision, a number was copied
wrong; fix it before proceeding. **No entry below was left uncorroborated.** (cb1/cb2/cb7 in §4.2
were likewise verified — generator + dual-oracle.)

### A.1 — Codebook 3 (quad, UNSIGNED, LAV 2, 81 entries, step 4)

Index→tuple (m=3, o=0 → magnitudes 0..2): `w=i/27, x=(i/9)%3, y=(i/3)%3, z=i%3`. UNSIGNED — read
**one sign bit per nonzero** of `w,x,y,z` after the codeword.

```
codes3 (hex):
0x0000,0x0009,0x00ef,0x000b,0x0019,0x00f0,0x01eb,0x01e6,0x03f2,0x000a,0x0035,0x01ef,0x0034,0x0037,0x01e9,0x01ed,
0x01e7,0x03f3,0x01ee,0x03ed,0x1ffa,0x01ec,0x01f2,0x07f9,0x07f8,0x03f8,0x0ff8,0x0008,0x0038,0x03f6,0x0036,0x0075,
0x03f1,0x03eb,0x03ec,0x0ff4,0x0018,0x0076,0x07f4,0x0039,0x0074,0x03ef,0x01f3,0x01f4,0x07f6,0x01e8,0x03ea,0x1ffc,
0x00f2,0x01f1,0x0ffb,0x03f5,0x07f3,0x0ffc,0x00ee,0x03f7,0x7ffe,0x01f0,0x07f5,0x7ffd,0x1ffb,0x3ffa,0xffff,0x00f1,
0x03f0,0x3ffc,0x01ea,0x03ee,0x3ffb,0x0ff6,0x0ffa,0x7ffc,0x07f2,0x0ff5,0xfffe,0x03f4,0x07f7,0x7ffb,0x0ff7,0x0ff9,
0x7ffa
bits3:
1,4,8,4,5,8,9,9,10,4,6,9,6,6,9,9, 9,10,9,10,13,9,9,11,11,10,12,4,6,10,6,7,
10,10,10,12,5,7,11,6,7,10,9,9,11,9,10,13, 8,9,12,10,11,12,8,10,15,9,11,15,13,14,16,8,
10,14,9,10,14,12,12,15,11,12,16,10,11,15,12,12, 15
```
(Index 0 = `0x0000`/len 1 = quad `(0,0,0,0)` — the all-zero quad KAT anchor.)

### A.2 — Codebook 4 (quad, UNSIGNED, LAV 2, 81 entries, step 4)

Index→tuple (m=3, o=0): `w=i/27, x=(i/9)%3, y=(i/3)%3, z=i%3`. UNSIGNED — sign bit per nonzero.

```
codes4 (hex):
0x007,0x016,0x0f6,0x018,0x008,0x0ef,0x1ef,0x0f3,0x7f8,0x019,0x017,0x0ed,0x015,0x001,0x0e2,0x0f0,
0x070,0x3f0,0x1ee,0x0f1,0x7fa,0x0ee,0x0e4,0x3f2,0x7f6,0x3ef,0x7fd,0x005,0x014,0x0f2,0x009,0x004,
0x0e5,0x0f4,0x0e8,0x3f4,0x006,0x002,0x0e7,0x003,0x000,0x06b,0x0e3,0x069,0x1f3,0x0eb,0x0e6,0x3f6,
0x06e,0x06a,0x1f4,0x3ec,0x1f0,0x3f9,0x0f5,0x0ec,0x7fb,0x0ea,0x06f,0x3f7,0x7f9,0x3f3,0xfff,0x0e9,
0x06d,0x3f8,0x06c,0x068,0x1f5,0x3ee,0x1f2,0x7f4,0x7f7,0x3f1,0xffe,0x3ed,0x1f1,0x7f5,0x7fe,0x3f5,
0x7fc
bits4:
4,5,8,5,4,8,9,8,11,5,5,8,5,4,8,8, 7,10,9,8,11,8,8,10,11,10,11,4,5,8,4,4,
8,8,8,10,4,4,8,4,4,7,8,7,9,8,8,10, 7,7,9,10,9,10,8,8,11,8,7,10,11,10,12,8,
7,10,7,7,9,10,9,11,11,10,12,10,9,11,11,10, 11
```
(Index 40 = `0x000`/len 4 = quad `(1,1,1,1)` — note cb4's len-4 minimum, no len-1 codeword.)

### A.3 — Codebook 5 (pair, SIGNED, LAV 4, 81 entries, step 2)

Index→tuple (m=9, o=4 → signed −4..4): `y=i/9 - 4, z=i%9 - 4`. SIGNED — the sign is in the value;
**no extra sign bits**.

```
codes5 (hex):
0x1fff,0x0ff7,0x07f4,0x07e8,0x03f1,0x07ee,0x07f9,0x0ff8,0x1ffd,0x0ffd,0x07f1,0x03e8,0x01e8,0x00f0,0x01ec,0x03ee,
0x07f2,0x0ffa,0x0ff4,0x03ef,0x01f2,0x00e8,0x0070,0x00ec,0x01f0,0x03ea,0x07f3,0x07eb,0x01eb,0x00ea,0x001a,0x0008,
0x0019,0x00ee,0x01ef,0x07ed,0x03f0,0x00f2,0x0073,0x000b,0x0000,0x000a,0x0071,0x00f3,0x07e9,0x07ef,0x01ee,0x00ef,
0x0018,0x0009,0x001b,0x00eb,0x01e9,0x07ec,0x07f6,0x03eb,0x01f3,0x00ed,0x0072,0x00e9,0x01f1,0x03ed,0x07f7,0x0ff6,
0x07f0,0x03e9,0x01ed,0x00f1,0x01ea,0x03ec,0x07f8,0x0ff9,0x1ffc,0x0ffc,0x0ff5,0x07ea,0x03f3,0x03f2,0x07f5,0x0ffb,
0x1ffe
bits5:
13,12,11,11,10,11,11,12,13,12,11,10,9,8,9,10, 11,12,12,10,9,8,7,8,9,10,11,11,9,8,5,4,
5,8,9,11,10,8,7,4,1,4,7,8,11,11,9,8, 5,4,5,8,9,11,11,10,9,8,7,8,9,10,11,12,
11,10,9,8,9,10,11,12,13,12,12,11,10,10,11,12, 13
```
(Index 40 = `0x0000`/len 1 = pair `(0,0)` — the all-zero pair KAT anchor for cb5.)

### A.4 — Codebook 6 (pair, SIGNED, LAV 4, 81 entries, step 2)

Index→tuple (m=9, o=4): `y=i/9 - 4, z=i%9 - 4`. SIGNED — no extra sign bits.

```
codes6 (hex):
0x7fe,0x3fd,0x1f1,0x1eb,0x1f4,0x1ea,0x1f0,0x3fc,0x7fd,0x3f6,0x1e5,0x0ea,0x06c,0x071,0x068,0x0f0,
0x1e6,0x3f7,0x1f3,0x0ef,0x032,0x027,0x028,0x026,0x031,0x0eb,0x1f7,0x1e8,0x06f,0x02e,0x008,0x004,
0x006,0x029,0x06b,0x1ee,0x1ef,0x072,0x02d,0x002,0x000,0x003,0x02f,0x073,0x1fa,0x1e7,0x06e,0x02b,
0x007,0x001,0x005,0x02c,0x06d,0x1ec,0x1f9,0x0ee,0x030,0x024,0x02a,0x025,0x033,0x0ec,0x1f2,0x3f8,
0x1e4,0x0ed,0x06a,0x070,0x069,0x074,0x0f1,0x3fa,0x7ff,0x3f9,0x1f6,0x1ed,0x1f8,0x1e9,0x1f5,0x3fb,
0x7fc
bits6:
11,10,9,9,9,9,9,10,11,10,9,8,7,7,7,8, 9,10,9,8,6,6,6,6,6,8,9,9,7,6,4,4,
4,6,7,9,9,7,6,4,4,4,6,7,9,9,7,6, 4,4,4,6,7,9,9,8,6,6,6,6,6,8,9,10,
9,8,7,7,7,7,8,10,11,10,9,9,9,9,9,10, 11
```
(Index 40 = `0x000`/len 4 = pair `(0,0)`; cb6 has no len-1 codeword.)

### A.5 — Codebook 8 (pair, UNSIGNED, LAV 7, 64 entries, step 2)

Index→tuple (m=8, o=0 → magnitudes 0..7): `y=i/8, z=i%8`. UNSIGNED — **one sign bit per nonzero**
of `y,z`.

```
codes8 (hex):
0x00e,0x005,0x010,0x030,0x06f,0x0f1,0x1fa,0x3fe,0x003,0x000,0x004,0x012,0x02c,0x06a,0x075,0x0f8,
0x00f,0x002,0x006,0x014,0x02e,0x069,0x072,0x0f5,0x02f,0x011,0x013,0x02a,0x032,0x06c,0x0ec,0x0fa,
0x071,0x02b,0x02d,0x031,0x06d,0x070,0x0f2,0x1f9,0x0ef,0x068,0x033,0x06b,0x06e,0x0ee,0x0f9,0x3fc,
0x1f8,0x074,0x073,0x0ed,0x0f0,0x0f6,0x1f6,0x1fd,0x3fd,0x0f3,0x0f4,0x0f7,0x1f7,0x1fb,0x1fc,0x3ff
bits8:
5,4,5,6,7,8,9,10,4,3,4,5,6,7,7,8, 5,4,4,5,6,7,7,8,6,5,5,6,6,7,8,8,
7,6,6,6,7,7,8,9,8,7,6,7,7,8,8,10, 9,7,7,8,8,8,9,9,10,8,8,8,9,9,9,10
```
(Index 9 = `0x000`/len 3 = pair `(1,1)`; cb8's shortest code is len 3 at index 9.)

### A.6 — Codebook 9 (pair, UNSIGNED, LAV 12, 169 entries, step 2)

Index→tuple (m=13, o=0 → magnitudes 0..12): `y=i/13, z=i%13`. UNSIGNED — sign bit per nonzero.

```
codes9 (hex):
0x0000,0x0005,0x0037,0x00e7,0x01de,0x03ce,0x03d9,0x07c8,0x07cd,0x0fc8,0x0fdd,0x1fe4,0x1fec,0x0004,0x000c,0x0035,
0x0072,0x00ea,0x00ed,0x01e2,0x03d1,0x03d3,0x03e0,0x07d8,0x0fcf,0x0fd5,0x0036,0x0034,0x0071,0x00e8,0x00ec,0x01e1,
0x03cf,0x03dd,0x03db,0x07d0,0x0fc7,0x0fd4,0x0fe4,0x00e6,0x0070,0x00e9,0x01dd,0x01e3,0x03d2,0x03dc,0x07cc,0x07ca,
0x07de,0x0fd8,0x0fea,0x1fdb,0x01df,0x00eb,0x01dc,0x01e6,0x03d5,0x03de,0x07cb,0x07dd,0x07dc,0x0fcd,0x0fe2,0x0fe7,
0x1fe1,0x03d0,0x01e0,0x01e4,0x03d6,0x07c5,0x07d1,0x07db,0x0fd2,0x07e0,0x0fd9,0x0feb,0x1fe3,0x1fe9,0x07c4,0x01e5,
0x03d7,0x07c6,0x07cf,0x07da,0x0fcb,0x0fda,0x0fe3,0x0fe9,0x1fe6,0x1ff3,0x1ff7,0x07d3,0x03d8,0x03e1,0x07d4,0x07d9,
0x0fd3,0x0fde,0x1fdd,0x1fd9,0x1fe2,0x1fea,0x1ff1,0x1ff6,0x07d2,0x03d4,0x03da,0x07c7,0x07d7,0x07e2,0x0fce,0x0fdb,
0x1fd8,0x1fee,0x3ff0,0x1ff4,0x3ff2,0x07e1,0x03df,0x07c9,0x07d6,0x0fca,0x0fd0,0x0fe5,0x0fe6,0x1feb,0x1fef,0x3ff3,
0x3ff4,0x3ff5,0x0fe0,0x07ce,0x07d5,0x0fc6,0x0fd1,0x0fe1,0x1fe0,0x1fe8,0x1ff0,0x3ff1,0x3ff8,0x3ff6,0x7ffc,0x0fe8,
0x07df,0x0fc9,0x0fd7,0x0fdc,0x1fdc,0x1fdf,0x1fed,0x1ff5,0x3ff9,0x3ffb,0x7ffd,0x7ffe,0x1fe7,0x0fcc,0x0fd6,0x0fdf,
0x1fde,0x1fda,0x1fe5,0x1ff2,0x3ffa,0x3ff7,0x3ffc,0x3ffd,0x7fff
bits9:
1,3,6,8,9,10,10,11,11,12,12,13,13,3,4,6, 7,8,8,9,10,10,10,11,12,12,6,6,7,8,8,9,
10,10,10,11,12,12,12,8,7,8,9,9,10,10,11,11, 11,12,12,13,9,8,9,9,10,10,11,11,11,12,12,12,
13,10,9,9,10,11,11,11,12,11,12,12,13,13,11,9, 10,11,11,11,12,12,12,12,13,13,13,11,10,10,11,11,
12,12,13,13,13,13,13,13,11,10,10,11,11,11,12,12, 13,13,14,13,14,11,10,11,11,12,12,12,12,13,13,14,
14,14,12,11,11,12,12,12,13,13,13,14,14,14,15,12, 11,12,12,12,13,13,13,13,14,14,15,15,13,12,12,12,
13,13,13,13,14,14,14,14,15
```
(Index 0 = `0x0000`/len 1 = pair `(0,0)`.)

### A.7 — Codebook 10 (pair, UNSIGNED, LAV 12, 169 entries, step 2)

Index→tuple (m=13, o=0): `y=i/13, z=i%13`. UNSIGNED — sign bit per nonzero.

```
codes10 (hex):
0x022,0x008,0x01d,0x026,0x05f,0x0d3,0x1cf,0x3d0,0x3d7,0x3ed,0x7f0,0x7f6,0xffd,0x007,0x000,0x001,
0x009,0x020,0x054,0x060,0x0d5,0x0dc,0x1d4,0x3cd,0x3de,0x7e7,0x01c,0x002,0x006,0x00c,0x01e,0x028,
0x05b,0x0cd,0x0d9,0x1ce,0x1dc,0x3d9,0x3f1,0x025,0x00b,0x00a,0x00d,0x024,0x057,0x061,0x0cc,0x0dd,
0x1cc,0x1de,0x3d3,0x3e7,0x05d,0x021,0x01f,0x023,0x027,0x059,0x064,0x0d8,0x0df,0x1d2,0x1e2,0x3dd,
0x3ee,0x0d1,0x055,0x029,0x056,0x058,0x062,0x0ce,0x0e0,0x0e2,0x1da,0x3d4,0x3e3,0x7eb,0x1c9,0x05e,
0x05a,0x05c,0x063,0x0ca,0x0da,0x1c7,0x1ca,0x1e0,0x3db,0x3e8,0x7ec,0x1e3,0x0d2,0x0cb,0x0d0,0x0d7,
0x0db,0x1c6,0x1d5,0x1d8,0x3ca,0x3da,0x7ea,0x7f1,0x1e1,0x0d4,0x0cf,0x0d6,0x0de,0x0e1,0x1d0,0x1d6,
0x3d1,0x3d5,0x3f2,0x7ee,0x7fb,0x3e9,0x1cd,0x1c8,0x1cb,0x1d1,0x1d7,0x1df,0x3cf,0x3e0,0x3ef,0x7e6,
0x7f8,0xffa,0x3eb,0x1dd,0x1d3,0x1d9,0x1db,0x3d2,0x3cc,0x3dc,0x3ea,0x7ed,0x7f3,0x7f9,0xff9,0x7f2,
0x3ce,0x1e4,0x3cb,0x3d8,0x3d6,0x3e2,0x3e5,0x7e8,0x7f4,0x7f5,0x7f7,0xffb,0x7fa,0x3ec,0x3df,0x3e1,
0x3e4,0x3e6,0x3f0,0x7e9,0x7ef,0xff8,0xffe,0xffc,0xfff
bits10:
6,5,6,6,7,8,9,10,10,10,11,11,12,5,4,4, 5,6,7,7,8,8,9,10,10,11,6,4,5,5,6,6,
7,8,8,9,9,10,10,6,5,5,5,6,7,7,8,8, 9,9,10,10,7,6,6,6,6,7,7,8,8,9,9,10,
10,8,7,6,7,7,7,8,8,8,9,10,10,11,9,7, 7,7,7,8,8,9,9,9,10,10,11,9,8,8,8,8,
8,9,9,9,10,10,11,11,9,8,8,8,8,8,9,9, 10,10,10,11,11,10,9,9,9,9,9,9,10,10,10,11,
11,12,10,9,9,9,9,10,10,10,10,11,11,11,12,11, 10,9,10,10,10,10,10,11,11,11,11,12,11,10,10,10,
10,10,10,11,11,12,12,12,12
```
(Index 14 = `0x000`/len 4 = pair `(1,1)`; cb10's shortest code is len 4 at index 14.)

### A.8 — Codebook 11 (pair, UNSIGNED + ESCAPE, LAV 16, 289 entries, step 2)

Index→tuple (m=17, o=0 → values 0..16): `y=i/17, z=i%17`. UNSIGNED — sign bit per nonzero of
`y,z`; **value 16 is the ESCAPE marker** (see A.10 for the escape decode).

```
codes11 (hex):
0x000,0x006,0x019,0x03d,0x09c,0x0c6,0x1a7,0x390,0x3c2,0x3df,0x7e6,0x7f3,0xffb,0x7ec,0xffa,0xffe,
0x38e,0x005,0x001,0x008,0x014,0x037,0x042,0x092,0x0af,0x191,0x1a5,0x1b5,0x39e,0x3c0,0x3a2,0x3cd,
0x7d6,0x0ae,0x017,0x007,0x009,0x018,0x039,0x040,0x08e,0x0a3,0x0b8,0x199,0x1ac,0x1c1,0x3b1,0x396,
0x3be,0x3ca,0x09d,0x03c,0x015,0x016,0x01a,0x03b,0x044,0x091,0x0a5,0x0be,0x196,0x1ae,0x1b9,0x3a1,
0x391,0x3a5,0x3d5,0x094,0x09a,0x036,0x038,0x03a,0x041,0x08c,0x09b,0x0b0,0x0c3,0x19e,0x1ab,0x1bc,
0x39f,0x38f,0x3a9,0x3cf,0x093,0x0bf,0x03e,0x03f,0x043,0x045,0x09e,0x0a7,0x0b9,0x194,0x1a2,0x1ba,
0x1c3,0x3a6,0x3a7,0x3bb,0x3d4,0x09f,0x1a0,0x08f,0x08d,0x090,0x098,0x0a6,0x0b6,0x0c4,0x19f,0x1af,
0x1bf,0x399,0x3bf,0x3b4,0x3c9,0x3e7,0x0a8,0x1b6,0x0ab,0x0a4,0x0aa,0x0b2,0x0c2,0x0c5,0x198,0x1a4,
0x1b8,0x38c,0x3a4,0x3c4,0x3c6,0x3dd,0x3e8,0x0ad,0x3af,0x192,0x0bd,0x0bc,0x18e,0x197,0x19a,0x1a3,
0x1b1,0x38d,0x398,0x3b7,0x3d3,0x3d1,0x3db,0x7dd,0x0b4,0x3de,0x1a9,0x19b,0x19c,0x1a1,0x1aa,0x1ad,
0x1b3,0x38b,0x3b2,0x3b8,0x3ce,0x3e1,0x3e0,0x7d2,0x7e5,0x0b7,0x7e3,0x1bb,0x1a8,0x1a6,0x1b0,0x1b2,
0x1b7,0x39b,0x39a,0x3ba,0x3b5,0x3d6,0x7d7,0x3e4,0x7d8,0x7ea,0x0ba,0x7e8,0x3a0,0x1bd,0x1b4,0x38a,
0x1c4,0x392,0x3aa,0x3b0,0x3bc,0x3d7,0x7d4,0x7dc,0x7db,0x7d5,0x7f0,0x0c1,0x7fb,0x3c8,0x3a3,0x395,
0x39d,0x3ac,0x3ae,0x3c5,0x3d8,0x3e2,0x3e6,0x7e4,0x7e7,0x7e0,0x7e9,0x7f7,0x190,0x7f2,0x393,0x1be,
0x1c0,0x394,0x397,0x3ad,0x3c3,0x3c1,0x3d2,0x7da,0x7d9,0x7df,0x7eb,0x7f4,0x7fa,0x195,0x7f8,0x3bd,
0x39c,0x3ab,0x3a8,0x3b3,0x3b9,0x3d0,0x3e3,0x3e5,0x7e2,0x7de,0x7ed,0x7f1,0x7f9,0x7fc,0x193,0xffd,
0x3dc,0x3b6,0x3c7,0x3cc,0x3cb,0x3d9,0x3da,0x7d3,0x7e1,0x7ee,0x7ef,0x7f5,0x7f6,0xffc,0xfff,0x19d,
0x1c2,0x0b5,0x0a1,0x096,0x097,0x095,0x099,0x0a0,0x0a2,0x0ac,0x0a9,0x0b1,0x0b3,0x0bb,0x0c0,0x18f,
0x004
bits11:
4,5,6,7,8,8,9,10,10,10,11,11,12,11,12,12, 10,5,4,5,6,7,7,8,8,9,9,9,10,10,10,10,
11,8,6,5,5,6,7,7,8,8,8,9,9,9,10,10, 10,10,8,7,6,6,6,7,7,8,8,8,9,9,9,10,
10,10,10,8,8,7,7,7,7,8,8,8,8,9,9,9, 10,10,10,10,8,8,7,7,7,7,8,8,8,9,9,9,
9,10,10,10,10,8,9,8,8,8,8,8,8,8,9,9, 9,10,10,10,10,10,8,9,8,8,8,8,8,8,9,9,
9,10,10,10,10,10,10,8,10,9,8,8,9,9,9,9, 9,10,10,10,10,10,10,11,8,10,9,9,9,9,9,9,
9,10,10,10,10,10,10,11,11,8,11,9,9,9,9,9, 9,10,10,10,10,10,11,10,11,11,8,11,10,9,9,10,
9,10,10,10,10,10,11,11,11,11,11,8,11,10,10,10, 10,10,10,10,10,10,10,11,11,11,11,11,9,11,10,9,
9,10,10,10,10,10,10,11,11,11,11,11,11,9,11,10, 10,10,10,10,10,10,10,10,11,11,11,11,11,11,9,12,
10,10,10,10,10,10,10,11,11,11,11,11,11,12,12,9, 9,8,8,8,8,8,8,8,8,8,8,8,8,8,8,9,
5
```
(Index 0 = `0x000`/len 4 = pair `(0,0)`. The last entry, index 288 = `0x004`/len 5, is pair
`(16,16)` — both values are the escape marker, so BOTH trigger A.10's escape decode.)

### A.9 — Scalefactor codebook (1-D DPCM, 121 entries)

Decoded symbol is an index `0..120`; the actual scalefactor delta is `delta = index - 60`. FAAD2
`hcb_sf.h` independently confirms 121 leaves over `0..120` with the **length-1 codeword (index 60,
`0x00000`) ⇒ value 60 ⇒ delta 0** (the most common, no change). Step `inc` is not used (1-D).

```
sf_codes (hex):
0x3ffe8,0x3ffe6,0x3ffe7,0x3ffe5,0x7fff5,0x7fff1,0x7ffed,0x7fff6,0x7ffee,0x7ffef,0x7fff0,0x7fffc,0x7fffd,0x7ffff,0x7fffe,0x7fff7,
0x7fff8,0x7fffb,0x7fff9,0x3ffe4,0x7fffa,0x3ffe3,0x1ffef,0x1fff0,0x0fff5,0x1ffee,0x0fff2,0x0fff3,0x0fff4,0x0fff1,0x07ff6,0x07ff7,
0x03ff9,0x03ff5,0x03ff7,0x03ff3,0x03ff6,0x03ff2,0x01ff7,0x01ff5,0x00ff9,0x00ff7,0x00ff6,0x007f9,0x00ff4,0x007f8,0x003f9,0x003f7,
0x003f5,0x001f8,0x001f7,0x000fa,0x000f8,0x000f6,0x00079,0x0003a,0x00038,0x0001a,0x0000b,0x00004,0x00000,0x0000a,0x0000c,0x0001b,
0x00039,0x0003b,0x00078,0x0007a,0x000f7,0x000f9,0x001f6,0x001f9,0x003f4,0x003f6,0x003f8,0x007f5,0x007f4,0x007f6,0x007f7,0x00ff5,
0x00ff8,0x01ff4,0x01ff6,0x01ff8,0x03ff8,0x03ff4,0x0fff0,0x07ff4,0x0fff6,0x07ff5,0x3ffe2,0x7ffd9,0x7ffda,0x7ffdb,0x7ffdc,0x7ffdd,
0x7ffde,0x7ffd8,0x7ffd2,0x7ffd3,0x7ffd4,0x7ffd5,0x7ffd6,0x7fff2,0x7ffdf,0x7ffe7,0x7ffe8,0x7ffe9,0x7ffea,0x7ffeb,0x7ffe6,0x7ffe0,
0x7ffe1,0x7ffe2,0x7ffe3,0x7ffe4,0x7ffe5,0x7ffd7,0x7ffec,0x7fff4,0x7fff3
sf_bits:
18,18,18,18,19,19,19,19,19,19,19,19,19,19,19,19, 19,19,19,18,19,18,17,17,16,17,16,16,16,16,15,15,
14,14,14,14,14,14,13,13,12,12,12,11,12,11,10,10, 10,9,9,8,8,8,7,6,6,5,4,3,1,4,4,5,
6,6,7,7,8,8,9,9,10,10,10,11,11,11,11,12, 12,13,13,13,14,14,16,15,16,15,18,19,19,19,19,19,
19,19,19,19,19,19,19,19,19,19,19,19,19,19,19,19, 19,19,19,19,19,19,19,19,19
```
(Index 60 = `0x00000`/len 1 = delta 0. This is the SF KAT anchor: the single shortest codeword
must decode to "no change".)

### A.10 — Codebook 11 escape decode (the `{N}`-style escape, exact bit layout)

When a cb11 codeword yields `y == 16` (and/or `z == 16`), the magnitude of THAT value is not 16 —
it is read as an escape. Corroborated FFmpeg `get_escape` ↔ FAAD2 `huffman_getescape` (which starts
its width counter at `i = 4` and reads `i` bits where `i = 4 + (#leading 1s)`):

```
// per value v in {y,z} that equals 16:
N = 0
while read_bit() == 1 { N += 1 }      // escape_prefix: count leading 1s
                                      // the terminating 0 (escape_separator) is consumed by the failing read
word = read_bits(N + 4)               // escape_word: N+4 bits, MSB-first
magnitude = word + (1 << (N + 4))     // = 2^(N+4) + word   (== FAAD2's  word | (1<<i),  i = N+4)
// then apply the per-value sign bit read for this coefficient (cb11 is unsigned + sign).
```

Worked example (matches the §acceptance cb11 KAT): `N = 2` (escape_prefix `110` → two 1s then the
separating 0), `escape_word = 6 bits`, so `i = N+4 = 6`; magnitude `= 2^6 + word = 64 + word`. The
FAAD2 cross-check bounds `i < 16` (i.e. `N <= 11`); a stream with `i >= 16` is malformed → that
frame yields silence, never a panic (§6 untrusted-input discipline).

### A.11 — Index→tuple summary (all 11 spectral books, one place)

| cb | dim | index→tuple | signed? | escape |
|----|-----|-------------|---------|--------|
| 1  | 4 | `w=i/27-1, x=(i/9)%3-1, y=(i/3)%3-1, z=i%3-1` (m=3,o=1) | SIGNED (value carries sign) | no |
| 2  | 4 | same as cb1 (m=3,o=1) | SIGNED | no |
| 3  | 4 | `w=i/27, x=(i/9)%3, y=(i/3)%3, z=i%3` (m=3,o=0) | UNSIGNED (+sign bit/nonzero) | no |
| 4  | 4 | same as cb3 (m=3,o=0) | UNSIGNED | no |
| 5  | 2 | `y=i/9-4, z=i%9-4` (m=9,o=4) | SIGNED | no |
| 6  | 2 | same as cb5 (m=9,o=4) | SIGNED | no |
| 7  | 2 | `y=i/8, z=i%8` (m=8,o=0) | UNSIGNED (+sign bit/nonzero) | no |
| 8  | 2 | same as cb7 (m=8,o=0) | UNSIGNED | no |
| 9  | 2 | `y=i/13, z=i%13` (m=13,o=0) | UNSIGNED | no |
| 10 | 2 | same as cb9 (m=13,o=0) | UNSIGNED | no |
| 11 | 2 | `y=i/17, z=i%17` (m=17,o=0) | UNSIGNED | YES (value 16 ⇒ A.10) |

**Implementer note:** the §4.1 generator stores each book as explicit `{code, len, w,x,y,z}` /
`{code, len, y,z}` rows — apply the map above once at gen-time to fill the value columns, so the
runtime decoder never does modulo math (this is the FAAD2 "explicit-column" form the §2.4 note
recommends, now fully derivable from inline data with zero web access).
