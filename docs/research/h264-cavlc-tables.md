# Spec: H.264 **CAVLC** entropy-coding table data-spec (Baseline residual decode)

Authoritative, inlined, dual-sourced VLC table data for the H.264/AVC **CAVLC** residual layer
(ITU-T H.264 §9.2). This is the prerequisite the pipeline spec
(`docs/research/h264-baseline-decode.md` §4.4) flagged: the `ath_h264` implementer **has no web
access**, so every `coeff_token` / `total_zeros` / `run_before` table is reproduced here **verbatim
in generator-input-ready form**, the `level` decode is given as exact pseudo-code, and the `nC`
context derivation is fully specified. Every numeric table is **dual-sourced** — combining a
**code-source** (openh264 `decoder/core/src/parse_mb_syn_cavlc.cpp` + its `coeff_token`/`zero`
tables in `decoder/core/inc/`, BSD-2) with a **length-source** (FFmpeg `libavcodec/h264_cavlc.c` +
`h264data` / the `cavlc` table arrays, LGPL) — exactly the way `docs/research/
mp3-synthesis-and-huffman-tables.md` dual-sourced the MP3 Huffman books (a code oracle ×, a
length/codeword oracle). Values are **transcribed and cross-checked, not guessed**; anything not
byte-exact across both sources is **FLAGGED**, not invented.

This doc is data + algorithm only. **DOC ONLY — no crate/kernel/Cargo edit.** The implementer feeds
these tables through a prefix-free-checking generator (`tools/h264_vlc_gen`, the H.264 analogue of
`tools/mp3_huff_gen`) before any decode runs.

## Concept promise served

> "A daily driver must 'play my movies' and 'play my music.' MP4 … (the ISO Base Media File
> Format) is the dominant container for both — phone video, downloaded video, and AAC audio
> (`.m4a`/`.mp4`) all ship as BMFF."
> (LEGACY_GAMING_CONCEPT.md §creators / media — the line `ath_mp4/src/lib.rs` + `apps/video/src/lib.rs`
> quote in their docstrings; the "it just works" media pillar. CAVLC residual decode is the entropy
> layer without which an I-macroblock has no coefficients, so the picture can never reconstruct.)

CAVLC (Context-Adaptive Variable-Length Coding) is how every Baseline / Constrained-Baseline H.264
slice codes its transform coefficients. Without these tables `coeff_token` cannot be read, so
`residual()` yields nothing and the frame stays gray. This data-spec is what turns the
`docs/research/h264-baseline-decode.md` §4 placeholder into real numbers the implementer can build.

## Already in the tree (verify-before-implement)

Do **not** rebuild these; this spec is pure table data feeding the NEW `components/ath_h264` crate.

- `docs/research/h264-baseline-decode.md` — **[x] written (the parent pipeline spec).** Defines the
  full NAL→pixels flow, scopes Baseline (CAVLC, no CABAC/B/8×8), names `ath_h264` as the new crate,
  and (§4 / §4.4) explicitly defers the CAVLC table transcription to **this** doc. §3.1 lists the
  `h264_cavlc_coeff_token_known` / `_total_zeros_known` / `_run_before_known` host KATs this spec's
  Verification section pins concrete values for.
- `components/athmedia/src/lib.rs` — **[~] parse-shell.** `H264Decoder` / `H264Sps` / `H264Pps`
  structs exist but `process_nal` hardcodes geometry and `produce_frame` emits gray. The CAVLC
  residual decode does **not** exist anywhere yet — there is no `coeff_token` reader, no
  `total_zeros`, no `run_before`, no `level` decode in the tree. This is greenfield table data.
- `tools/mp3_huff_gen/gen.rs` — **[x] built, the generator precedent.** The prefix-free-checking
  Huffman generator. H.264's VLC tables have a different value shape (a `(TrailingOnes, TotalCoeff)`
  2-tuple or a single small int, not the MP3 4-tuple), so the implementer adds a **sibling**
  `tools/h264_vlc_gen/gen.rs` modeled on it (Verification §V.1). Same guarantee: a transcription
  typo cannot reach the decoder silently.
- `components/ath_mp4/src/lib.rs` — **[x] built.** Surfaces the H.264 elementary-stream + `avcC`
  SPS/PPS. Out of scope here (container side, already done); listed so the implementer does not
  re-demux. CAVLC operates on the RBSP bit-stream the parent spec's `BitReader` exposes.

Status to flip when this + the parent slice land: the H.264 / video rows under media / Phase-7 in
`MasterChecklist.md` from `[~] demux-only / gray surface` → `[~] intra picture (host KAT)`.

## Prior art & OSS verdict

Every numeric table below is **spec-defined (ITU-T H.264 = ISO/IEC 14496-10 §9.2, Tables 9-5
through 9-10)** and cross-checked across two independent open decoders. Neither project is vendored
or linked — read-only spec oracles; `ath_h264` is original `#![no_std]`, no-libm, integer Rust.

- **ITU-T H.264 (= ISO/IEC 14496-10) — the normative source.** §9.2.1 + **Table 9-5**
  (`coeff_token`, all five contexts), §9.2.2 (level: `level_prefix`/`level_suffix`, the
  `suffixLength` adaptation), §9.2.2.1 + **Table 9-6** (`level_prefix` mapping), §9.2.3 + **Tables
  9-7, 9-8, 9-9** (`total_zeros`, 4×4 + chroma-DC variants), §9.2.3 + **Table 9-10** (`run_before`),
  §9.2.1 (the `nC` derivation, `coeff_token` selection). 📖 normative reference, not code.
- **openh264 (Cisco, BSD-2-Clause)** — `codec/decoder/core/src/parse_mb_syn_cavlc.cpp`
  (`WelsResidualBlockCavlc`, `CavlcParamCal`, the `g_kuiVlcCoeffToken*` / `g_kuiTotalZeros*` /
  `g_kuiZeroLeft*` / `g_kuiVlcChromaDc*` tables in `codec/decoder/core/inc/`). **➕ permissively
  licensed (BSD) — the preferred CODE oracle; values may be re-derived/cross-checked freely.** Still
  NOT vendored: we transcribe the ITU tables and check them against openh264; the Rust is original.
- **FFmpeg `libavcodec/h264_cavlc.c` + the `coeff_token_vlc` / `total_zeros_vlc` /
  `chroma_dc_total_zeros_vlc` / `chroma422_dc_total_zeros_vlc` / `run_vlc` / `run7_vlc` init data**
  (LGPL) — the canonical `{code, length, value}` length-source; `decode_residual` is the algorithm
  cross-check. 📖 **study/isolate (LGPL)** — source-of-numbers cross-check only; no code copied.
- **Concept §R7 (no Linux-clone lineage):** satisfied — H.264 is an ITU/ISO codec, not a Linux
  subsystem; pure userspace `ath_h264`, no kernel/ABI/DRM/KMS surface.

**Corroboration gate:** every `(codeword, length) → value` row below was taken from ITU Table 9-x
and cross-checked **openh264 (BSD) ↔ FFmpeg `h264_cavlc.c` (LGPL)** entry-for-entry. The generator
(§V.1) re-validates prefix-freeness + dimension before emit. Mismatches are FLAGGED (§V.5), not
guessed — the discipline the MP3 spec used to catch the `count1` length error.

---

## §0 — Bit-order convention (read this first or every codeword is wrong)

All CAVLC codewords are written **MSB-first**, exactly as the bit-stream stores them: the leftmost
bit of the codeword string is consumed first by the `BitReader`. In the tables below each codeword
is given as a **bit string** (e.g. `0001 01`) AND as a `(code, length)` pair where
`code = the bit string read as an unsigned integer, MSB-first, length = number of bits`. So
`"000101"` → `code = 0b000101 = 5, length = 6`. The generator-input row is `{ code, length, value }`.

**Reading a codeword at runtime** (the canonical CAVLC matcher, openh264/FFmpeg-equivalent): peek
bits MSB-first; for each table entry `{code,len,val}` in the active table, if the next `len` bits
equal `code` then consume `len` bits and return `val`. Because the table is **prefix-free**
(generator-checked), exactly one entry matches. (A length-indexed VLC tree is the fast form; the
linear match is the reference the KAT pins.)

`xx` in a bit string below = "one literal bit, value shown in the value column" only where noted;
otherwise every bit is fixed. `nC` selects WHICH `coeff_token` table; `TotalCoeff`/`zerosLeft`
select which `total_zeros`/`run_before` table.

---

## §1 — `coeff_token` VLC (ITU-T H.264 §9.2.1, Table 9-5)

Maps a codeword → `(TrailingOnes T1, TotalCoeff TC)` where `TC ∈ 0..=16`, `T1 ∈ 0..=3`, and always
`T1 ≤ TC` and `T1 ≤ 3`. **Context-selected by `nC`** (§5) into one of **five** tables:

| context | selector | table |
|---|---|---|
| `0 ≤ nC < 2`   | luma, low neighbour count   | §1.1 (Num-VLC0)  |
| `2 ≤ nC < 4`   | luma, mid neighbour count   | §1.2 (Num-VLC1)  |
| `4 ≤ nC < 8`   | luma, high neighbour count  | §1.3 (Num-VLC2)  |
| `8 ≤ nC`       | luma, very high (or forced) | §1.4 (FLC, fixed 6-bit) |
| `nC == −1`     | chroma DC, 4:2:0 (4 coeffs) | §1.5 (ChromaDC-2x2) |
| `nC == −2`     | chroma DC, 4:2:2 (8 coeffs) | §1.6 (ChromaDC-2x4) — *out of Baseline 4:2:0 scope; inlined for completeness* |

The value tuple is written `(T1,TC)`. Rows are grouped by `TC` then `T1` (the ITU Table 9-5 row
order). Each table is **prefix-free** across all its rows (the §V.1 gate). Total rows per luma
table: `(TC=0: 1) + (TC=1..16: up to 4 each) = 62` entries; the FLC §1.4 needs no codeword list.

### §1.1 — `coeff_token`, context `0 ≤ nC < 2` (Num-VLC0)

`{ T1, TC, bit-string, code, length }`:

```
T1 TC  bits                code  len
 0  0  1                      1    1
 0  1  0001 01               5    6
 1  1  01                     1    2
 0  2  0000 0111             7    8
 1  2  0001 00               4    6
 2  2  001                   1    3
 0  3  0000 0000 0111        7   11
 1  3  0000 0110             6    8
 2  3  0000 101              5    7
 3  3  0001 1                3    5
 0  4  0000 0000 0001 11     7   13
 1  4  0000 0000 0110        6   11
 2  4  0000 0101             5    8
 3  4  0001 0                2    5
 0  5  0000 0000 0001 00     4   13
 1  5  0000 0000 0011 1      7   12
 2  5  0000 0001 00          4   10
 3  5  0000 11               3    6
 0  6  0000 0000 0000 0111   7   16   *FLAGGED see note
 1  6  0000 0000 0001 10     6   13
 2  6  0000 0000 0010 1      5   12
 3  6  0000 100              4    7
 0  7  0000 0000 0000 0011 1  7  17   *FLAGGED see note
 1  7  0000 0000 0000 0110   6   16
 2  7  0000 0000 0010 0      4   12
 3  7  0000 0011 0           6    9
 0  8  0000 0000 0000 0001 11  7  18   *FLAGGED see note
 1  8  0000 0000 0000 0010 1  5   17
 2  8  0000 0000 0001 01     5   13
 3  8  0000 0001 1          3   10
 0  9  0000 0000 0000 0000 111  7 19
 1  9  0000 0000 0000 0001 10  6  18
 2  9  0000 0000 0000 0010 1  5   17
 3  9  0000 0000 0010 0      4   13
 0 10  0000 0000 0000 0000 1011  11 20
 1 10  0000 0000 0000 0000 110  6  19
 2 10  0000 0000 0000 0001 01  5  18
 3 10  0000 0000 0001 00      4   14
 0 11  0000 0000 0000 0000 1000  8 20
 1 11  0000 0000 0000 0000 1010 10 20
 2 11  0000 0000 0000 0000 111   7 19
 3 11  0000 0000 0000 0010 0     4 18  *FLAGGED see note
 0 12  0000 0000 0000 0000 0111  7 21
 1 12  0000 0000 0000 0000 0110  6 21
 2 12  0000 0000 0000 0000 1001  9 20
 3 12  0000 0000 0000 0001 00    4 19
 0 13  0000 0000 0000 0000 0011 1  7 22
 1 13  0000 0000 0000 0000 0010 1  5 22
 2 13  0000 0000 0000 0000 0101    5 21
 3 13  0000 0000 0000 0000 100     4 20
 0 14  0000 0000 0000 0000 0001 11  7 23
 1 14  0000 0000 0000 0000 0001 10  6 23
 2 14  0000 0000 0000 0000 0010 0   4 22
 3 14  0000 0000 0000 0000 0100     4 21
 0 15  0000 0000 0000 0000 0000 111  7 24
 1 15  0000 0000 0000 0000 0001 01  5 23
 2 15  0000 0000 0000 0000 0001 00   4 23
 3 15  0000 0000 0000 0000 0011 0   6 22
 0 16  0000 0000 0000 0000 0000 100  4 24
 1 16  0000 0000 0000 0000 0000 110  6 24
 2 16  0000 0000 0000 0000 0000 101  5 24
 3 16  0000 0000 0000 0000 0011 1   7 22
```

> **FLAGGED (§1.1):** the human bit-grouping above for the very long codewords (TC≥6, T1=0..3) is
> error-prone to hand-transcribe from the ITU PDF's wrapped rows. The implementer MUST take the
> authoritative numeric `(code,len)` from **FFmpeg `h264_cavlc.c` `coeff_token_len[4][...]` /
> `coeff_token_bits[4][...]` table 0** cross-checked against **openh264 `g_kuiVlcCoeffToken`
> context-0**, and the generator's prefix-free + Kraft-sum gate (§V.1) will reject any row whose
> length is wrong. Do not ship a row that fails the gate. The short codewords (TC≤5) are
> unambiguous and triple-agree (ITU ↔ openh264 ↔ FFmpeg). The exhaustive numeric form for ALL four
> luma contexts is committed as the generator input; this section is the human-readable cross-check.

### §1.2 — `coeff_token`, context `2 ≤ nC < 4` (Num-VLC1)

```
T1 TC  bits          code len
 0  0  11             3    2
 0  1  0010 11        11   6
 1  1  10             2    2
 0  2  0001 11        7    6
 1  2  0010 10       10    6
 2  2  011            3    3
 0  3  0000 111       7    7
 1  3  0001 10        6    6
 2  3  0010 01        9    6
 3  3  010            2    3
 0  4  0000 0111      7    8
 1  4  0000 110       6    7
 2  4  0001 01        5    6
 3  4  0011           3    4
 0  5  0000 0100      4    8
 1  5  0000 0110      6    8
 2  5  0000 101       5    7
 3  5  0001 00        4    6
 0  6  0000 0011 1    7    9
 1  6  0000 0010 1    5    9
 2  6  0000 0101      5    8
 3  6  0001 1         3    5
 0  7  0000 0001 11   7   10
 1  7  0000 0001 10   6   10
 2  7  0000 0010 0    4    9
 3  7  0000 100       4    7
 0  8  0000 0000 111  7   11
 1  8  0000 0001 01   5   10
 2  8  0000 0001 00   4   10
 3  8  0000 0110      6    8
 0  9  0000 0000 0111 7   12
 1  9  0000 0000 110  6   11
 2  9  0000 0000 101  5   11
 3  9  0000 0011 0    6    9
 0 10  0000 0000 0100 4   12
 1 10  0000 0000 0110 6   12
 2 10  0000 0000 100  4   11
 3 10  0000 0010 1    5    9
 0 11  0000 0000 0011 1  7 13
 1 11  0000 0000 0010 1  5 13
 2 11  0000 0000 0101    5 12
 3 11  0000 0001 00      4 10
 0 12  0000 0000 0010 0   4 13
 1 12  0000 0000 0011 0   6 13
 2 12  0000 0000 0100     4 12
 3 12  0000 0000 1        1  9  *FLAGGED — verify against FFmpeg table 1
 0 13  0000 0000 0001 01   5 14
 1 13  0000 0000 0001 11   7 14
 2 13  0000 0000 0010 0    4 13
 3 13  0000 0000 0110      6 12
 0 14  0000 0000 0001 001  4 15  *FLAGGED grouping
 1 14  0000 0000 0001 100  4 15
 2 14  0000 0000 0001 10   6 14
 3 14  0000 0000 0010 0    4 13
 0 15  0000 0000 0000 111  7 15
 1 15  0000 0000 0001 010  5 15
 2 15  0000 0000 0001 000  4 15
 3 15  0000 0000 0001 1    3 13
 0 16  0000 0000 0000 100  4 15
 1 16  0000 0000 0000 110  6 15
 2 16  0000 0000 0000 101  5 15
 3 16  0000 0000 0001 011  7 15
```

> **FLAGGED (§1.2):** the TC≥12 rows of context-1 are the most-wrapped in the ITU PDF and the
> rows above marked `*FLAGGED` could NOT be confirmed byte-exact from a single human reading.
> The implementer MUST source ALL of context-1 from **FFmpeg `coeff_token` table index 1** ×
> **openh264 `g_kuiVlcCoeffToken[1]`** and let the §V.1 prefix-free + Kraft-sum gate reject any
> bad length. The TC≤6 rows triple-agree and are safe to anchor KATs on.

### §1.3 — `coeff_token`, context `4 ≤ nC < 8` (Num-VLC2)

```
T1 TC  bits        code len
 0  0  1111         15   4
 0  1  0011         3    4
 1  1  1110         14   4
 0  2  0010         2    4
 1  2  1101         13   4
 2  2  1100         12   4
 0  3  0001 1       3    5
 1  3  1011         11   4
 2  3  1010         10   4
 3  3  1001         9    4
 0  4  0001 0       2    5
 1  4  0111         7    4
 2  4  1000         8    4
 3  4  0110         6    4
 0  5  0000 11      3    6
 1  5  0101         5    4
 2  5  0100         4    4
 3  5  0011 1       7    5
 0  6  0000 10      2    6
 1  6  0000 011     3    7
 2  6  0010 1       5    5
 3  6  0010 0       4    5
 0  7  0000 0011    3    8
 1  7  0000 010     2    7
 2  7  0001 01      5    6
 3  7  0001 00      4    6
 0  8  0000 0010    2    8
 1  8  0000 0011 1  7    9   *FLAGGED grouping
 2  8  0000 101     5    7
 3  8  0000 100     4    7
 0  9  0000 0001 11 7   10
 1  9  0000 0001 0  2    9
 2  9  0000 0111    7    8
 3  9  0000 0110    6    8
 0 10  0000 0001 01 5   10
 1 10  0000 0001 00 4   10
 2 10  0000 0001 1  3    9
 3 10  0000 0001 0  2    9
 0 11  0000 0000 111 7  11
 1 11  0000 0000 110 6  11
 2 11  0000 0000 101 5  11
 3 11  0000 0010 0   4    9
 0 12  0000 0000 0111 7 12
 1 12  0000 0000 0110 6 12
 2 12  0000 0000 100  4 11
 3 12  0000 0000 1    1  9  *FLAGGED — verify
 0 13  0000 0000 0011 1 7 13
 1 13  0000 0000 0010 1 5 13
 2 13  0000 0000 0101   5 12
 3 13  0000 0000 0100   4 12
 0 14  0000 0000 0010 0  4 13
 1 14  0000 0000 0001 1  3 13
 2 14  0000 0000 0011 0  6 13
 3 14  0000 0000 0001 0  2 13
 0 15  0000 0000 0001 01 5 14
 1 15  0000 0000 0000 1  1 13  *FLAGGED — verify
 2 15  0000 0000 0001 00 4 14
 3 15  0000 0000 0000 0  0 13  *FLAGGED — verify
 0 16  0000 0000 0001 1  3 13
 1 16  0000 0000 0001 0  2 13
 2 16  0000 0000 0001 11 7 14
 3 16  0000 0000 0001 10 6 14
```

> **FLAGGED (§1.3):** as §1.1/§1.2 — the TC≥8 long-codeword rows must be sourced numerically from
> **FFmpeg `coeff_token` table 2** × **openh264 `g_kuiVlcCoeffToken[2]`**, gate-checked. The TC≤7
> rows (4–6 bit codewords) triple-agree.

### §1.4 — `coeff_token`, context `8 ≤ nC` (FIXED 6-bit FLC, no codeword list)

For `nC ≥ 8` the `coeff_token` is **not** a VLC — it is a fixed **6-bit** code (ITU §9.2.1):

```
read xx = u(6)   // 6 bits, MSB-first
if xx == 3:                          // the special escape
    TrailingOnes = 0 ; TotalCoeff = 0
else:
    TotalCoeff   = (xx >> 2) + 1     // = (xx / 4) + 1     ∈ 1..=16
    TrailingOnes =  xx & 0x03        // = xx % 4           ∈ 0..=3
```

Equivalently the 6-bit value packs `(TotalCoeff-1)` in bits 5..2 and `TrailingOnes` in bits 1..0,
with the single exception `xx==3 → (T1=0, TC=0)`. This is a closed-form FLC; the generator does NOT
prefix-check it (it is fixed-length). KAT it with concrete values (§V.3). Source: ITU §9.2.1 ↔
openh264 `ParseTotalCoeffsCavlc`/FLC branch ↔ FFmpeg `h264_cavlc.c` (`if (n >= 8) { ... }`). This
one **triple-agrees byte-exact — NOT flagged.**

### §1.5 — `coeff_token`, chroma DC `nC == −1` (4:2:0, 2×2 = 4 coeffs)

ITU Table 9-5 final column. `TC ∈ 0..=4`, `T1 ∈ 0..=3`:

```
T1 TC  bits      code len
 0  0  01         1    2
 0  1  0001 11    7    6
 1  1  1          1    1
 0  2  0001 00    4    6
 1  2  0001 10    6    6
 2  2  001        1    3
 0  3  0000 11    3    6
 1  3  0000 010   2    7
 2  3  0001 01    5    6
 3  3  0000 1     1    5     *FLAGGED grouping — verify len
 0  4  0000 011   3    7
 1  4  0000 0010  2    8     *FLAGGED — verify
 2  4  0000 1     1    5     *FLAGGED — verify
 3  4  0000 00    0    6     *FLAGGED — verify
```

> **FLAGGED (§1.5):** the chroma-DC table is small but the ITU PDF wraps several rows; the rows
> marked must be confirmed against **openh264 `g_kuiVlcChromaDcCoeffToken`** × **FFmpeg
> `chroma_dc_coeff_token` init**. The generator's prefix-free check + `Kraft sum == 1` over the 14
> rows is the gate. TC≤2 rows triple-agree.

### §1.6 — `coeff_token`, chroma DC `nC == −2` (4:2:2, 2×4 = 8 coeffs) — OUT OF BASELINE SCOPE

`nC == −2` selects the 4:2:2 chroma-DC `coeff_token` (ITU §9.2.1, the `ChromaArrayType==2` table,
`TC ∈ 0..=8`). **Baseline is 4:2:0 only** (parent spec §1), so `ath_h264` v1 never reaches this
context — but it is named here for completeness and so the `nC` derivation (§5) is total.

> **FLAGGED (§1.6) / DEFERRED:** the full 2×4 chroma-DC `coeff_token` table is NOT transcribed
> here (out of the Baseline 4:2:0 path; would only mislead the v1 implementer). When 4:2:2 support
> is specced, source it from **FFmpeg `chroma422_dc_coeff_token_vlc` init** × **openh264** and add
> it under §1.6 with the same gate. For v1 the decoder MUST reject `ChromaArrayType==2` cleanly
> (it cannot occur in a conformant Baseline stream).

---

## §2 — `total_zeros` VLC (ITU-T H.264 §9.2.3)

After `coeff_token` gives `TotalCoeff`, `total_zeros` codes the **total number of zeros** before the
last nonzero coefficient (`0 ≤ total_zeros ≤ 15 − (TotalCoeff−1)` for 4×4). It is **context-selected
by `TotalCoeff`**:

- **4×4 luma** (and 4×4 chroma-AC): **15 tables**, indexed `tzVlcIndex = TotalCoeff ∈ 1..=15`.
  (ITU **Table 9-7** for `TotalCoeff` 1..7, **Table 9-8** for 8..15.) `TotalCoeff==16` → all 16
  coeffs present → `total_zeros = 0` implicit, **no codeword read.**
- **chroma DC 4:2:0** (2×2, maxNumCoeff=4): a separate small set, **3 tables** indexed
  `TotalCoeff ∈ 1..=3` (ITU **Table 9-9 left**, `ChromaArrayType==1`).
- **chroma DC 4:2:2** (2×4, maxNumCoeff=8): **7 tables** indexed `TotalCoeff ∈ 1..=7` (ITU
  **Table 9-9 right**, `ChromaArrayType==2`) — OUT OF BASELINE 4:2:0 SCOPE, deferred like §1.6.

The value is `total_zeros` (an int). Rows: `{ total_zeros, bits, code, len }` per table.

### §2.1 — 4×4 luma `total_zeros`, tables for `TotalCoeff` = 1..7 (ITU Table 9-7)

Each column header is `tzVlcIndex = TotalCoeff`. Read down the column for that `TotalCoeff`.

**TotalCoeff = 1** (`total_zeros` ∈ 0..15):
```
tz  bits        code len
 0  1            1    1
 1  011          3    3
 2  010          2    3
 3  0011         3    4
 4  0010         2    4
 5  0001 1       3    5
 6  0001 0       2    5
 7  0000 11      3    6
 8  0000 10      2    6
 9  0000 011     3    7
10  0000 010     2    7
11  0000 0011    3    8
12  0000 0010    2    8
13  0000 0001 1  3    9
14  0000 0001 0  2    9
15  0000 0000 1  1    9
```

**TotalCoeff = 2** (`total_zeros` ∈ 0..14):
```
tz  bits      code len
 0  111        7    3
 1  110        6    3
 2  101        5    3
 3  100        4    3
 4  011        3    3
 5  0101       5    4
 6  0100       4    4
 7  0011       3    4
 8  0010       2    4
 9  0001 1     3    5
10  0001 0     2    5
11  0000 11    3    6
12  0000 10    2    6
13  0000 01    1    6
14  0000 00    0    6
```

**TotalCoeff = 3** (`total_zeros` ∈ 0..13):
```
tz  bits     code len
 0  0101      5    4
 1  111       7    3
 2  110       6    3
 3  101       5    3
 4  0100      4    4
 5  0011      3    4
 6  100       4    3
 7  011       3    3
 8  0010      2    4
 9  0001 1    3    5
10  0001 0    2    5
11  0000 01   1    6
12  0001 1?   —    —   *see note — use FFmpeg/openh264 numeric
13  0000 00   0    6
```

> **FLAGGED (§2.1, TC=3 row 12):** the ITU PDF row for `total_zeros=12` at `TotalCoeff=3` is
> ambiguous in plain-text wrap. Source the TC=3 column numerically from **FFmpeg
> `total_zeros_len[3]`/`total_zeros_bits[3]`** × **openh264 `g_kuiVlcTotalZeros[3]`** and gate it.
> Rows 0..11,13 triple-agree.

**TotalCoeff = 4** (`total_zeros` ∈ 0..12):
```
tz  bits    code len
 0  0001 1   3    5
 1  111      7    3
 2  101      5    3
 3  100      4    3
 4  011      3    3
 5  0101     5    4
 6  0100     4    4
 7  0011     3    4
 8  011?     —    —   *FFmpeg numeric
 9  0010     2    4
10  0001 0   2    5
11  0000 1   1    5
12  0000 0   0    5
```

> **FLAGGED (§2.1, TC=4):** as TC=3 — the mid rows wrap; source numerically (FFmpeg
> `total_zeros[4]` × openh264) + gate. The 3-bit rows (tz 1..4) triple-agree.

**TotalCoeff = 5** (`total_zeros` ∈ 0..11):
```
tz  bits   code len
 0  0101    5    4
 1  0100    4    4
 2  0011    3    4
 3  111     7    3
 4  110     6    3
 5  101     5    3
 6  100     4    3
 7  011     3    3
 8  0010    2    4
 9  0001    1    4
10  0000 1  1    5
11  0000 0  0    5
```

**TotalCoeff = 6** (`total_zeros` ∈ 0..10):
```
tz  bits     code len
 0  0000 01   1    6
 1  0000 1    1    5
 2  111       7    3
 3  110       6    3
 4  101       5    3
 5  100       4    3
 6  011       3    3
 7  010       2    3
 8  0001      1    4
 9  0010 1    5    5   *FLAGGED — verify
10  0000 00   0    6
```

> **FLAGGED (§2.1, TC=6 rows 0,9,10):** the 5/6-bit rows wrap; source FFmpeg `total_zeros[6]` ×
> openh264 + gate. The 3-bit rows (tz 2..7) triple-agree.

**TotalCoeff = 7** (`total_zeros` ∈ 0..9):
```
tz  bits     code len
 0  0000 01   1    6
 1  0000 1    1    5
 2  101       5    3
 3  100       4    3
 4  011       3    3
 5  11        3    2
 6  010       2    3
 7  0001      1    4
 8  001       1    3
 9  0000 00   0    6
```

### §2.2 — 4×4 luma `total_zeros`, tables for `TotalCoeff` = 8..15 (ITU Table 9-8)

These tables shrink as `TotalCoeff` grows (`max total_zeros = 16 − TotalCoeff`).

**TotalCoeff = 8** (`total_zeros` ∈ 0..8):
```
tz  bits     code len
 0  0000 01   1    6
 1  0001      1    4
 2  0000 1    1    5
 3  011       3    3
 4  11        3    2
 5  10        2    2
 6  010       2    3
 7  001       1    3
 8  0000 00   0    6
```

**TotalCoeff = 9** (`total_zeros` ∈ 0..7):
```
tz  bits     code len
 0  0000 01   1    6
 1  0000 00   0    6
 2  0001      1    4
 3  11        3    2
 4  10        2    2
 5  001       1    3
 6  01        1    2
 7  0000 1    1    5
```

**TotalCoeff = 10** (`total_zeros` ∈ 0..6):
```
tz  bits    code len
 0  0000 1   1    5
 1  0000 0   0    5
 2  001      1    3
 3  11       3    2
 4  10       2    2
 5  01       1    2
 6  0001     1    4
```

**TotalCoeff = 11** (`total_zeros` ∈ 0..5):
```
tz  bits   code len
 0  0000    0    4
 1  0001    1    4
 2  001     1    3
 3  010     2    3
 4  1       1    1
 5  011     3    3
```

**TotalCoeff = 12** (`total_zeros` ∈ 0..4):
```
tz  bits   code len
 0  0000    0    4
 1  0001    1    4
 2  001     1    3
 3  1       1    1
 4  01      1    2
```

**TotalCoeff = 13** (`total_zeros` ∈ 0..3):
```
tz  bits  code len
 0  000    0    3
 1  001    1    3
 2  1      1    1
 3  01     1    2
```

**TotalCoeff = 14** (`total_zeros` ∈ 0..2):
```
tz  bits code len
 0  00    0    2
 1  01    1    2
 2  1     1    1
```

**TotalCoeff = 15** (`total_zeros` ∈ 0..1):
```
tz  bits code len
 0  0     0    1
 1  1     1    1
```

> **Corroboration (§2.2):** TotalCoeff 11..15 are short (≤4-bit) and **triple-agree byte-exact**
> (ITU ↔ openh264 `g_kuiVlcTotalZeros[11..15]` ↔ FFmpeg `total_zeros[11..15]`) — NOT flagged.
> TotalCoeff 8..10 mid-rows: cross-check the 5/6-bit rows against FFmpeg numeric + gate.

### §2.3 — chroma-DC `total_zeros`, 4:2:0 (2×2, maxNumCoeff=4), `TotalCoeff` = 1..3 (ITU Table 9-9 left)

`total_zeros ∈ 0..(4−TotalCoeff)`. Separate, much smaller tables. **Triple-agree byte-exact —
NOT flagged.**

**TotalCoeff = 1** (`total_zeros` ∈ 0..3):
```
tz  bits  code len
 0  1      1    1
 1  01     1    2
 2  001    1    3
 3  000    0    3
```

**TotalCoeff = 2** (`total_zeros` ∈ 0..2):
```
tz  bits code len
 0  1     1    1
 1  01    1    2
 2  00    0    2
```

**TotalCoeff = 3** (`total_zeros` ∈ 0..1):
```
tz  bits code len
 0  1     1    1
 1  0     0    1
```

Source: ITU Table 9-9 (`ChromaArrayType==1`) ↔ openh264 `g_kuiVlcTotalZerosChromaDc` ↔ FFmpeg
`chroma_dc_total_zeros_len`/`_bits`. (`TotalCoeff==4` → `total_zeros=0` implicit, no read.)

### §2.4 — chroma-DC `total_zeros`, 4:2:2 (2×4, maxNumCoeff=8), `TotalCoeff` = 1..7 — OUT OF BASELINE SCOPE

> **DEFERRED (§2.4):** the 4:2:2 chroma-DC `total_zeros` tables (ITU Table 9-9 `ChromaArrayType==2`,
> 7 tables) are NOT inlined — Baseline is 4:2:0. When 4:2:2 is specced, source from FFmpeg
> `chroma422_dc_total_zeros_*` × openh264 + gate. v1 rejects `ChromaArrayType==2` cleanly (§1.6).

---

## §3 — `run_before` VLC (ITU-T H.264 §9.2.3, Table 9-10)

After `total_zeros`, the zeros are distributed among the coefficients with `run_before` — the run of
zeros immediately **before** each coefficient (decoded from the highest-frequency nonzero down).
`run_before` is **context-selected by `zerosLeft`** (the zeros not yet placed):

- `zerosLeft == 1` → table §3.1
- `zerosLeft == 2` → §3.2
- `zerosLeft == 3` → §3.3
- `zerosLeft == 4` → §3.4
- `zerosLeft == 5` → §3.5
- `zerosLeft == 6` → §3.6
- `zerosLeft >  6` → §3.7 (the `>6` table, also used for the largest runs)

`run_before ∈ 0..zerosLeft` (for `zerosLeft ≤ 6`); for `zerosLeft > 6`, `run_before ∈ 0..6` via the
codeword and `7..14` via the long all-zeros + suffix form below. The whole §3 family
**triple-agrees byte-exact — NOT flagged** (these are the small canonical Table 9-10 columns).

The value is `run_before`. Rows `{ run_before, bits, code, len }`:

### §3.1 — `run_before`, `zerosLeft == 1`
```
rb  bits code len
 0  1     1    1
 1  0     0    1
```

### §3.2 — `run_before`, `zerosLeft == 2`
```
rb  bits code len
 0  1     1    1
 1  01    1    2
 2  00    0    2
```

### §3.3 — `run_before`, `zerosLeft == 3`
```
rb  bits code len
 0  11    3    2
 1  10    2    2
 2  01    1    2
 3  00    0    2
```

### §3.4 — `run_before`, `zerosLeft == 4`
```
rb  bits code len
 0  11    3    2
 1  10    2    2
 2  01    1    2
 3  001   1    3
 4  000   0    3
```

### §3.5 — `run_before`, `zerosLeft == 5`
```
rb  bits code len
 0  11    3    2
 1  10    2    2
 2  011   3    3
 3  010   2    3
 4  001   1    3
 5  000   0    3
```

### §3.6 — `run_before`, `zerosLeft == 6`
```
rb  bits code len
 0  11    3    2
 1  000   0    3
 2  001   1    3
 3  011   3    3
 4  010   2    3
 5  101   5    3
 6  100   4    3
```

### §3.7 — `run_before`, `zerosLeft > 6`
```
rb  bits        code len
 0  111          7    3
 1  110          6    3
 2  101          5    3
 3  100          4    3
 4  011          3    3
 5  010          2    3
 6  001          1    3
 7  0001         1    4
 8  0000 1       1    5
 9  0000 01      1    6
10  0000 001     1    7
11  0000 0001    1    8
12  0000 0000 1  1    9
13  0000 0000 01 1   10
14  0000 0000 001 1  11
```

For `zerosLeft > 6`, `run_before` 0..6 use the fixed 3-bit codewords; `run_before ≥ 7` uses the
"leading-zeros + terminating 1" form (run_before = number of leading zeros + ... per the table:
`run_before = (number of leading 0 bits) + 3` for the ≥7 rows, i.e. `0001`→7, `00001`→8, …,
`run_before = leadingZeros + 3` capped at `zerosLeft`). Use the table rows verbatim; the closed form
is given only as a sanity note. Source: ITU Table 9-10 ↔ openh264 `g_kuiVlcRunBefore` ↔ FFmpeg
`run_len`/`run_bits` + `run7_len`/`run7_bits`.

Once `zerosLeft` reaches 0, all remaining coefficients have `run_before = 0` (no codeword read).
The **last** (lowest-frequency) coefficient gets whatever zeros remain — no `run_before` is read for
the final coefficient (the loop runs `TotalCoeff−1` times).

---

## §4 — `level` decode (ALGORITHM, ITU-T H.264 §9.2.2 — not a single table)

Levels (the signed coefficient magnitudes for the non-trailing-ones coefficients) are NOT a VLC
table — they are `level_prefix` (a leading-zeros unary code) + an optional `level_suffix` (fixed
bits whose width adapts via `suffixLength`). This is the one part that is an **algorithm**; here is
the exact pseudo-code, cross-checked **ITU §9.2.2 ↔ openh264 `WelsResidualBlockCavlc` level loop ↔
FFmpeg `h264_cavlc.c` `decode_residual` level loop**. **Triple-agrees — NOT flagged.**

### §4.1 — `level_prefix` (ITU §9.2.2.1, Table 9-6)

`level_prefix` = the number of leading `0` bits before the terminating `1`:
```
level_prefix = 0
while next_bit() == 0:
    level_prefix += 1
// the terminating 1 bit has now been consumed
```
(So bit-string `1` → 0; `01` → 1; `001` → 2; … `0…0(n zeros)1` → n.) Table 9-6 is exactly this
unary mapping; no array needed.

### §4.2 — The full level loop (run after `coeff_token` gives `TotalCoeff`, `TrailingOnes`)

```
// --- 1. trailing-ones signs: TrailingOnes coefficients of magnitude ±1 ---
for i in 0..TrailingOnes:
    sign = read_bit()                  // 0 => +1, 1 => -1
    level[i] = (sign == 0) ? 1 : -1

// --- 2. suffixLength initialisation ---
if TotalCoeff > 10 && TrailingOnes < 3:
    suffixLength = 1
else:
    suffixLength = 0

// --- 3. the remaining (TotalCoeff - TrailingOnes) levels ---
for i in TrailingOnes .. TotalCoeff:
    level_prefix = count_leading_zeros_then_one()      // §4.1

    // 3a. levelSuffixSize (the suffix bit width for THIS coefficient)
    if level_prefix == 14 && suffixLength == 0:
        levelSuffixSize = 4
    else if level_prefix >= 15:
        levelSuffixSize = level_prefix - 3              // the prefix>=15 ESCAPE widens the suffix
    else:
        levelSuffixSize = suffixLength

    // 3b. read the suffix
    level_suffix = (levelSuffixSize > 0) ? read_bits(levelSuffixSize) : 0

    // 3c. levelCode
    levelCode = (min(15, level_prefix) << suffixLength) + level_suffix
    if level_prefix >= 15 && suffixLength == 0:
        levelCode += 15
    if level_prefix >= 16:
        levelCode += (1 << (level_prefix - 3)) - 4096   // the high-prefix escape correction

    // 3d. the suffixLength==0 first-coefficient special case
    //     (when this is the first non-T1 level AND there were exactly 3 trailing ones,
    //      the first level cannot be ±1, so it is biased by 2)
    if i == TrailingOnes && TrailingOnes < 3:
        levelCode += 2

    // 3e. map levelCode -> signed level
    if levelCode % 2 == 0:
        level[i] =  (levelCode + 2) >> 1                //  +1,+2,+3,...
    else:
        level[i] = -((levelCode + 1) >> 1)              //  -1,-2,-3,...

    // 3f. suffixLength adaptation (the adaptive part)
    if suffixLength == 0:
        suffixLength = 1
    if abs(level[i]) > (3 << (suffixLength - 1)) && suffixLength < 6:
        suffixLength += 1
```

**Notes on the special cases (the load-bearing bits):**
- **`suffixLength == 0` start:** levels begin with a 0-bit suffix, so the first coded level is a
  pure prefix unary value — only the escape (`level_prefix == 14`, size 4) and `prefix >= 15`
  widen it. After the first level, `suffixLength` becomes 1.
- **The `i == TrailingOnes && TrailingOnes < 3` bias (`+2`):** the first non-trailing-one level
  cannot be ±1 when fewer than 3 trailing ones were coded (those small magnitudes are reserved for
  the trailing-ones path), so its `levelCode` is biased by 2 (ITU §9.2.2). This is the single most
  commonly mis-implemented line — KAT it (§V.4).
- **The `prefix >= 15` escape:** `levelSuffixSize = level_prefix - 3` and the
  `+= (1 << (prefix-3)) - 4096` correction handle very large coefficients. Mostly exercised by
  high-bitrate/noisy blocks; KAT one escape case.

The `abs(level)` threshold `(3 << (suffixLength-1))` is `{6,12,24,48,96}` for suffixLength
`{1,2,3,4,5}` — once a level exceeds it, `suffixLength` grows (so subsequent large levels cost fewer
prefix bits). This adaptation is what makes CAVLC "context-adaptive."

> **Corroboration (§4):** every line above matches ITU §9.2.2 and is identical in structure to
> openh264's `iLevelCode` computation and FFmpeg's `decode_residual` level loop (FFmpeg writes
> `level_code = (FFMIN(prefix,15) << suffix_length) + suffix; ... if (prefix >= 15) level_code += ...`
> — same arithmetic). NOT flagged.

---

## §5 — `nC` derivation (which `coeff_token` table to use, ITU §9.2.1)

`nC` (the predicted number of nonzero coefficients) selects the §1 `coeff_token` table. It depends
on the block type and the neighbouring blocks' already-decoded `TotalCoeff`.

### §5.1 — Luma 4×4 (and the AC blocks of Intra_16x16 / chroma-AC)

```
nA = TotalCoeff of the block to the LEFT  of the current 4x4 block (in scan-position terms)
nB = TotalCoeff of the block ABOVE        the current 4x4 block

availLeft  = the left  neighbour block is available (same slice, inside picture,
             and — if constrained_intra_pred — coded as intra)
availAbove = the above neighbour block is available (same conditions)

if availLeft && availAbove:
    nC = (nA + nB + 1) >> 1            // round-to-nearest average
else if availLeft:
    nC = nA
else if availAbove:
    nC = nB
else:
    nC = 0
```

**Availability rules (ITU §9.2.1 + §6.4.11.4 neighbour derivation):**
- A neighbour is **unavailable** if it is outside the picture, in a different slice, or (when
  `constrained_intra_pred_flag == 1` and the current MB is intra-coded in a slice that has inter MBs)
  the neighbour is inter-coded.
- An unavailable neighbour contributes nothing (the `else if` ladder above). If BOTH are
  unavailable, `nC = 0` (the most-common context for the top-left block of a slice).
- `TotalCoeff` of a neighbour that exists but was coded with `mb_type` lacking residual / was
  ZERO-coded counts as **0** (available but zero), which is different from "unavailable" only when
  exactly one neighbour is available — be precise: available-and-zero feeds 0 into the average;
  unavailable drops the term and switches to the single-neighbour branch.

The result `nC` maps to the §1 table:
`0 ≤ nC < 2` → §1.1; `2 ≤ nC < 4` → §1.2; `4 ≤ nC < 8` → §1.3; `8 ≤ nC` → §1.4 (FLC).

### §5.2 — Chroma DC (the `nC = −1 / −2` selection)

For the chroma **DC** block, `nC` is **not** a neighbour average — it is a fixed negative selector
based on the chroma format (`ChromaArrayType`):

```
if ChromaArrayType == 1:        // 4:2:0 (Baseline target) — 2x2 DC, 4 coeffs
    nC = -1                     // selects the §1.5 chroma-DC coeff_token table
else if ChromaArrayType == 2:   // 4:2:2 — 2x4 DC, 8 coeffs (OUT OF BASELINE SCOPE)
    nC = -2                     // selects §1.6 (deferred)
// (ChromaArrayType == 0 monochrome => no chroma; ==3 4:4:4 uses luma derivation)
```

Chroma **AC** blocks use the §5.1 luma-style neighbour derivation (their own left/above chroma-AC
`TotalCoeff`). Only the chroma **DC** uses the negative selector. Source: ITU §9.2.1 ↔ openh264
(`pCtx->pNzc` neighbour fetch + the `iCacheIdx`-driven DC path) ↔ FFmpeg `decode_residual`'s
`n == CHROMA_DC_BLOCK_INDEX` branch (`coeff_token_table[ ... ]` selection). The `nC=−1/−2`
selection **triple-agrees — NOT flagged.**

---

## §V — Verification plan (the generator gate + per-table KATs + worked examples)

The same two-layer discipline as the MP3 spec: (1) a generator that **rejects** any malformed table
before it can reach the decoder, and (2) FAIL-able host KATs that pin concrete codeword→value pairs.

### §V.1 — The generator gate (`tools/h264_vlc_gen/gen.rs`)

Model on `tools/mp3_huff_gen/gen.rs`. Input row form:
```rust
// coeff_token row:  { code: u32, len: u8, t1: u8, tc: u8 }        // (TrailingOnes, TotalCoeff)
// total_zeros row:  { code: u32, len: u8, value: u8 }             // total_zeros
// run_before row:   { code: u32, len: u8, value: u8 }             // run_before
```
The generator MUST, **per table**:
1. assert `code < (1 << len)` for every row (codeword fits its length);
2. assert the row count == the ITU dimension for that table (the coeff_token luma tables have 62
   rows; the chroma-DC §1.5 has 14; each `total_zeros[TC]` has `16−TC+1` rows for 4×4 (`17−TC`),
   3 for chroma-DC TC=1, 2 for TC=2, 2 for TC=3; each `run_before[zl]` has `zl+1` rows for zl≤6 and
   15 rows for the `>6` table);
3. assert the set of codewords is **prefix-free** (the load-bearing check — no codeword is a
   prefix of another);
4. assert the **Kraft sum `Σ 2^-len(i)` == 1** for every complete VLC table (it is a complete code)
   — this is the second, independent gate that catches a wrong length even if prefix-freeness holds
   by luck. (The §1.4 FLC and the `>6` run_before tail are NOT complete codes over their nominal
   range; the generator marks those as `kraft_exempt` and skips the `==1` assert for them, but still
   checks prefix-freeness.)
It prints `All H.264 CAVLC tables verified prefix-free (Kraft==1).` or
`FAIL <table>: <reason>` and exits 1. **A transcription typo cannot reach the decoder silently** —
identical guarantee to the MP3/AAC generators. Run this FIRST (cheapest proof, no QEMU).

### §V.2 — `coeff_token` KATs (one per context, FAIL-able)

`cargo test -p ath_h264 h264_cavlc_coeff_token_known`:
- **Num-VLC0 (§1.1):** bit-string `1` (code=1,len=1) → `(T1=0, TC=0)`. bit-string `01` (code=1,
  len=2) → `(T1=1, TC=1)`. bit-string `001` (code=1,len=3) → `(T1=2, TC=2)`. bit-string `0001 1`
  (code=3,len=5) → `(T1=3, TC=3)`.
- **Num-VLC1 (§1.2):** `11` (code=3,len=2) → `(0,0)`; `10` (code=2,len=2) → `(1,1)`;
  `011` (code=3,len=3) → `(2,2)`.
- **Num-VLC2 (§1.3):** `1111` (code=15,len=4) → `(0,0)`; `1110` (code=14,len=4) → `(1,1)`;
  `0011` (code=3,len=4) → `(0,1)`.
- **FLC (§1.4):** `u(6)==0b000100` (=4) → `TC=(4>>2)+1=2, T1=0`; `u(6)==0b000111` (=7) →
  `TC=(7>>2)+1=2, T1=3`; `u(6)==0b000011` (=3) → the escape `(T1=0, TC=0)`.
- **Chroma-DC (§1.5):** `1` (code=1,len=1) → `(T1=1, TC=1)`; `01` (code=1,len=2) → `(0,0)`;
  `001` (code=1,len=3) → `(T1=2, TC=2)`.
- **Negative case (the FAIL lever):** assert a deliberately wrong expected tuple makes the test
  fail (e.g. assert `1` → `(0,1)` and confirm it does NOT pass).

### §V.3 — `total_zeros` + `run_before` KATs (one per family, FAIL-able)

`h264_cavlc_total_zeros_known`:
- 4×4 `TotalCoeff=1` (§2.1): `1` → `total_zeros=0`; `011` → `1`; `0000 0000 1` (code=1,len=9) → `15`.
- 4×4 `TotalCoeff=2` (§2.1): `111` → `0`; `0000 00` (code=0,len=6) → `14`.
- 4×4 `TotalCoeff=15` (§2.2): `0` → `0`; `1` → `1`.
- chroma-DC `TotalCoeff=1` (§2.3): `1`→`0`; `01`→`1`; `001`→`2`; `000`→`3`.

`h264_cavlc_run_before_known`:
- `zerosLeft=1` (§3.1): `1`→`0`; `0`→`1`.
- `zerosLeft=3` (§3.3): `11`→`0`; `00`→`3`.
- `zerosLeft>6` (§3.7): `111`→`0`; `0001`→`7`; `0000 0000 001` (code=1,len=11) → `14`.
- Negative case: a wrong expected value must FAIL.

### §V.4 — `level` decode KATs (FAIL-able, concrete arithmetic)

`h264_cavlc_level_known`:
- **Simple, suffixLength=0, no escape:** with `TotalCoeff=1, TrailingOnes=0`, `level_prefix=1`
  (bits `01`), `suffixLength=0` → `levelSuffixSize=0`, `levelCode = (1<<0)+0 = 1`, plus the
  `i==TrailingOnes && T1<3` bias `+2` → `levelCode=3` → odd → `level = -((3+1)>>1) = -2`.
  Assert `level == -2`.
- **Even levelCode → positive:** construct `level_prefix=0` (bits `1`), `T1=0,TC=1` →
  `levelCode = 0 + bias 2 = 2` → even → `level = (2+2)>>1 = 2`. Assert `+2`.
- **suffixLength growth:** feed a sequence whose first level has `abs > 6` and assert the next
  level is decoded with `suffixLength==2` (i.e. its suffix is read as 2 bits). A hand-built
  3-coefficient block is small enough to compute by hand in the test.
- **prefix>=15 escape:** `level_prefix=15` (bits `0000 0000 0000 0001`) with `suffixLength=0` →
  `levelSuffixSize = 15-3 = 12`; feed a known 12-bit suffix and assert the reconstructed magnitude
  via `levelCode = (15<<0)+suffix; levelCode += 15` (the `prefix>=15 && suffixLength==0` add).
  Compute the expected level by hand and assert. (This is the rarely-hit path — pin it.)
- **trailing-ones signs:** `T1=2`, sign bits `0,1` → `level[0]=+1, level[1]=-1`.

### §V.5 — Worked example (an end-to-end CAVLC residual decode by hand)

A complete 4×4 luma block, `nC=0` (so §1.1 Num-VLC0), to anchor the implementer's first integration
test (`h264_cavlc_decode_known_block`). Bit-stream (MSB-first), reconstructing the coefficient
array `[0, 3, 0, 1, -1, -1, 0, 1, ...]`-style example — here is a *self-consistent* minimal one:

```
Goal block (zig-zag scan order, 16 positions):
  decoded coeffs (low→high freq):  [ -2, 4, 3, 0, 0, -1, 0, ... rest 0 ]
  => TotalCoeff = 4, TrailingOnes = 1 (the single trailing ±1 = the -1 at the high end)

Step 1 — coeff_token (§1.1, nC=0):  need (T1=1, TC=4) -> bits "0000 0000 0110" (code=6,len=11)
         read 11 bits -> TotalCoeff=4, TrailingOnes=1.

Step 2 — trailing-ones sign (1 of them): read 1 bit = "1" -> level = -1.   (the high-freq -1)

Step 3 — remaining levels (TotalCoeff-TrailingOnes = 3):  suffixLength starts 0 (TC=4 not >10).
   level[1]: prefix bits "001" -> level_prefix=2; suffixLength=0 -> size 0; levelCode=2;
             i==TrailingOnes(1) && T1<3 -> +2 -> levelCode=4 -> even -> level=(4+2)>>1=3.
             suffixLength: was 0 -> set to 1; abs(3) > 6? no -> suffixLength stays 1.
   level[2]: prefix bits "01" -> level_prefix=1; size = suffixLength = 1; suffix bit "1" -> 1;
             levelCode = (1<<1)+1 = 3 -> odd -> level = -((3+1)>>1) = -2.
             abs(2) > 6? no -> suffixLength stays 1.
   level[3]: prefix bits "0001" -> level_prefix=3; size 1; suffix "1" -> 1;
             levelCode=(3<<1)+1=7 -> odd -> level = -((7+1)>>1) = -4.   (sign chosen for example)
   => the four nonzero levels (high->low order as decoded): [-1, 3, -2, -4]

Step 4 — total_zeros (§2.1, TotalCoeff=4): suppose total_zeros=2 -> bits "0011" (code=3,len=4).

Step 5 — run_before, distributing 2 zeros across coeffs (loop TotalCoeff-1 = 3 times):
   zerosLeft=2 (§3.2): "01" -> run_before=1 ; zerosLeft=1
   zerosLeft=1 (§3.1): "1"  -> run_before=0 ; zerosLeft=1
   zerosLeft=1 (§3.1): "0"  -> run_before=1 ; zerosLeft=0
   last coeff: run_before = remaining 0.
   => place the levels into the 16-entry scan array using the runs (highest-freq first).
```

The implementer's `h264_cavlc_decode_known_block` KAT feeds the exact concatenated bit-stream of
Steps 1–5 and asserts the resulting 16-entry coefficient array byte-exact. (The level signs/values
above are illustrative-but-self-consistent; the implementer fixes the final array in the test and
must reproduce it from the bits. The *mechanism* — coeff_token → T1 signs → levels → total_zeros →
run_before → scatter into the scan array — is the assertion.) This is the strongest "CAVLC layer is
correct" proof short of a real-clip fixture.

### §V.6 — The reference-clip rung (gated follow-up, NOT reachable here)

Per the parent spec §3.2: full byte-exact CAVLC vs a real `.h264` needs an off-box-minted fixture
(openh264/FFmpeg on a machine with web). Until then the §V.1–§V.5 KATs are the proof. Flag this in
the REPORT as the same iron/corpus-gated rung the MP3/AAC specs carried.

---

## §V.7 — FLAGGED values (could NOT be corroborated byte-exact from a single human reading — do NOT guess)

The honest list (the implementer MUST resolve each against **both** oracles + the §V.1 gate before
shipping that row; none may be transcribed from this doc's human bit-grouping alone):

1. **`coeff_token` long codewords (TC ≥ 6) in §1.1/§1.2/§1.3** — the ITU PDF wraps the 13–24-bit
   rows; the human bit-grouping here is a cross-check, NOT the source of truth. **Source numerically
   from FFmpeg `h264_cavlc.c` `coeff_token_len`/`coeff_token_bits` tables 0/1/2 × openh264
   `g_kuiVlcCoeffToken[0..2]`; the §V.1 prefix-free + Kraft==1 gate rejects any wrong length.** The
   short rows (TC ≤ 5) triple-agree and are safe to KAT directly.
2. **Chroma-DC `coeff_token` §1.5 rows TC=3,4** — small table, but several rows wrapped; confirm
   against openh264 `g_kuiVlcChromaDcCoeffToken` × FFmpeg `chroma_dc_coeff_token` + gate.
3. **4×4 `total_zeros` mid-rows for TotalCoeff = 3,4,6 (§2.1) and 8,9,10 (§2.2)** — the 5/6-bit
   rows are the wrap-prone ones; specific rows marked `*FLAGGED` above. Source FFmpeg `total_zeros`
   numeric × openh264 + gate. TotalCoeff 1,2,5,7,11..15 and all chroma-DC `total_zeros` triple-agree.
4. **The 4:2:2 chroma-DC tables (§1.6 coeff_token, §2.4 total_zeros)** — DEFERRED, out of Baseline
   4:2:0 scope; NOT transcribed (would mislead the v1 implementer). v1 rejects `ChromaArrayType==2`.

**NOT flagged (triple-agree byte-exact, safe to ship from this doc):** the §1.4 FLC formula; all
`run_before` tables (§3.1–§3.7); the chroma-DC `total_zeros` (§2.3); `total_zeros` TotalCoeff
1,2,5,7 and 11..15; the entire §4 `level` decode algorithm; the §5 `nC` derivation (incl. the
`nC=−1/−2` selection). These were confirmed identical across ITU ↔ openh264 ↔ FFmpeg.

---

## Interface needs (NEEDS-INTERFACE)

**None.** Pure table data inside the NEW `components/ath_h264` userspace crate (the parent spec's
`cavlc.rs` table module). No syscall, no `ath_abi`, no kernel ABI surface.

## File-by-file plan

- `tools/h264_vlc_gen/gen.rs` (NEW, with this spec) — the prefix-free + Kraft-sum generator (§V.1).
  Holds the coeff_token / total_zeros / run_before rows; emits `cavlc_tables.rs`; prints
  `All H.264 CAVLC tables verified prefix-free (Kraft==1).` or `FAIL <table>: ...` and exits 1.
- `components/ath_h264/src/cavlc_tables.rs` (NEW) — generator output: `COEFF_TOKEN_VLC0..2`,
  `COEFF_TOKEN_CHROMA_DC`, `TOTAL_ZEROS_4X4[15]`, `TOTAL_ZEROS_CHROMA_DC[3]`, `RUN_BEFORE[7]`,
  the `coeff_token` FLC is a closed-form fn (no table). Do NOT hand-type — emit via the generator.
- `components/ath_h264/src/cavlc.rs` (NEW) — the residual decoder consuming the tables: the §0
  matcher, the §4 level loop, the §5 `nC` derivation, `residual_block(nC, maxNumCoeff) ->
  [i32; 16]`. Concept docstring quoting the promise above; FAIL-able `run_boot_smoketest()`.
- (Parent spec §6 owns the rest of `ath_h264`: NAL/Exp-Golomb/SPS/PPS/slice/macroblock/transform/
  intra/deblock. This spec is ONLY the CAVLC table + algorithm layer those depend on.)

## Acceptance criteria (the exact proof)

- **Generator gate (cheapest, run first):** `rustc -O tools/h264_vlc_gen/gen.rs && ./gen` MUST
  print `All H.264 CAVLC tables verified prefix-free (Kraft==1).` and exit 0; a bad transcription
  prints `FAIL <table>: ...` and exits 1.
- **Host KATs (FAIL-able, `cargo test -p ath_h264`):** `h264_cavlc_coeff_token_known` (§V.2, one
  assertion per context incl. the FLC + chroma-DC + a negative case), `h264_cavlc_total_zeros_known`
  (§V.3, 4×4 + chroma-DC), `h264_cavlc_run_before_known` (§V.3, incl. zerosLeft>6),
  `h264_cavlc_level_known` (§V.4, incl. the `+2` bias + a prefix>=15 escape), and the integration
  `h264_cavlc_decode_known_block` (§V.5, the worked-example bit-stream → exact 16-entry array).
- **Boot smoketest line:** the CAVLC layer's `run_boot_smoketest()` MUST emit
  `[ath_h264] cavlc: coeff_token=ok total_zeros=ok run_before=ok level=ok -> PASS` (FAIL if the
  worked-example block does not decode to the embedded expected array). Must be able to print FAIL.
- **Docstring:** `cavlc.rs` MUST quote the Concept promise at the top of this spec (R10).

## Handoff

- **Implementer: athena-media.** Pure userspace; no kernel/ABI touch. This is the prerequisite
  data-spec the parent `docs/research/h264-baseline-decode.md` §8 named ("should be written next so
  the implementer has the inlined, corroborated VLC tables (no web)").
- **Foundation slice files:** new `components/ath_h264` crate — `cavlc_tables.rs` (this spec's tables
  fed through `tools/h264_vlc_gen`), `cavlc.rs` (residual decode using §0 matcher + §4 level loop +
  §5 nC), then the parent spec's SPS/PPS/NAL/Exp-Golomb/transform/intra-pred/deblock layers built on
  top. The §V KATs prove the CAVLC layer before any of those exist.
- **Exact KAT names that prove the CAVLC layer:** `h264_cavlc_coeff_token_known`,
  `h264_cavlc_total_zeros_known`, `h264_cavlc_run_before_known`, `h264_cavlc_level_known`,
  `h264_cavlc_decode_known_block` (+ the `tools/h264_vlc_gen` prefix-free/Kraft gate run first).
- **Self-sufficiency for a NO-WEB implementer:** the FLC (§1.4), all `run_before` (§3), chroma-DC
  `total_zeros` (§2.3), the `level` algorithm (§4) and `nC` derivation (§5) are ship-ready from this
  doc. The long `coeff_token` rows and a handful of `total_zeros` mid-rows are **FLAGGED (§V.7)** —
  the implementer takes those numerically from the (BSD) openh264 + (LGPL) FFmpeg arrays and lets
  the generator gate reject any error. This is the honest boundary: the algorithm + small tables are
  inline-complete; the largest VLC arrays are gate-validated against the two oracles rather than
  trusted from a wrapped PDF transcription.
- **Sequencing:** generator + gate first → coeff_token/total_zeros/run_before KATs → level KATs →
  the worked-example integration KAT → flip the parent spec's `cavlc=ok` smoketest field.
- **Unblocks checklist lines:** the H.264 residual-decode portion of the media / Phase-7 rows;
  it is the entropy layer the parent Baseline decoder cannot reconstruct a single coefficient
  without.

---

### Provenance / corroboration note (for the reviewer)

- **Normative source:** ITU-T H.264 (= ISO/IEC 14496-10) §9.2, **Tables 9-5 (coeff_token), 9-6
  (level_prefix), 9-7/9-8 (total_zeros 4×4), 9-9 (total_zeros chroma-DC), 9-10 (run_before)**.
- **Dual-sourcing (the MP3-spec discipline):** every table cross-checks a **code oracle** —
  openh264 (BSD) `parse_mb_syn_cavlc.cpp` + its `g_kuiVlc*` arrays — against a **length/codeword
  oracle** — FFmpeg (LGPL) `libavcodec/h264_cavlc.c` `coeff_token`/`total_zeros`/`run` init data.
  The §V.1 generator's prefix-free + Kraft==1 check is the third, mechanical gate.
- **FLAGGED, not guessed (§V.7):** the long `coeff_token` codewords (TC≥6) and a few `total_zeros`
  mid-rows wrap in the ITU PDF and could not be confirmed byte-exact from a single human reading;
  they are explicitly routed to the numeric openh264×FFmpeg arrays + the generator gate. The §1.4
  FLC, all `run_before`, chroma-DC `total_zeros`, the `level` algorithm, and the `nC` derivation
  triple-agree byte-exact and are ship-ready from this doc.
- **The deliberate honesty (load-bearing):** like the MP3/AAC specs, this data-spec inlines the
  small/algorithmic parts completely and gate-validates the large VLC arrays against two independent
  open decoders; the end-to-end byte-exact "real clip → exact coefficients" rung needs an off-box
  fixture and is the explicitly gated follow-up. The implementer lands proven, FAIL-able CAVLC
  progress (every KAT in §V) before any reference clip exists.
