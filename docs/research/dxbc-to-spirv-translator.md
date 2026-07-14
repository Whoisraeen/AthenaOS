# Spec: DXBC → SPIR-V shader translator (AthBridge DirectX path)

## Concept promise served
> "DirectX 11/12 → AthGFX translation at the driver level (DXVK/VKD3D-Proton lineage, but integrated and signed)" (§Gaming-First Design / Compatibility, line 116)
>
> "Steam works day one via AthBridge — non-negotiable; without Steam there is no PC gaming OS" (§Compatibility, line 117)

A D3D game cannot draw a single triangle on AthGFX/Vulkan until its HLSL-compiled
shaders (shipped as DXBC for D3D9/10/11, DXIL for D3D12) become SPIR-V. This spec
designs that translator. Per the agent prompt's central framing: **the translation
is pure CPU-side compute, host-KAT-provable NOW; only the eventual GPU *submit* of
the resulting SPIR-V is GPU-gated** (the owner's amdgpu bring-up). So the translator
is unblocked Concept work that advances the Steam thesis without waiting on the GPU.

## Already in the tree (verify-before-implement)
Verified by reading the source, not the stale crate table:

- `components/raebridge/src/d3d_translate.rs` (~1961 lines) — **rich and real** for
  *API state*, **stub** for *shaders*:
  - `[x]` DONE: `DxgiFormat` (110 formats) → `raegfx::PixelFormat`, `bytes_per_pixel`,
    `is_depth/compressed/srgb`; D3D11 rasterizer/blend/depth-stencil/buffer/texture
    descriptors → AthGFX state; D3D12 resource-state→barrier/layout/access/stage;
    D3D12 root-signature → `DescriptorSetLayoutBinding` + push-constant ranges;
    input-layout (vertex elements) → `VertexBufferLayout`; perf counters; compat DB.
  - `[x]` DONE: `parse_dxbc_header()` — walks the FourCC chunk table, finds
    `SHEX`(0x58454853)/`SHDR`(0x52444853), decodes the version token
    (`program_type`, `major`, `minor`) → `DxbcShaderType` + `ShaderModel`. **This is a
    correct container + header parser already.** Reuse it; do not rewrite it.
  - `[ ]` STUB: `translate_shader()` calls `emit_stub_spirv()` (line 1770) which emits
    only the **5-word SPIR-V header** (magic/version/generator/bound/schema) — no
    types, no entry point, no function, no body. This is the gap.
- `components/raebridge/src/dxgi.rs` — a **second, parallel** stub surface:
  - `DxbcShader::parse()` (line 1579): checks magic only, returns empty signatures
    with a `// Stub` comment.
  - `ShaderTranslator::translate_dxbc_to_spirv()` / `translate_dxil_to_spirv()`
    (lines 1623/1641): both call a duplicate `emit_stub_spirv()` (line 1679).
  - `ShaderError` enum + `ShaderParameter`/`ComponentType` structs already exist here
    and are the natural error/signature types to reuse.
- `components/raebridge/src/d3d9.rs`, `d3d11.rs`, `d3d12.rs` — COM-interface signature
  surfaces; they call into the above. No shader logic of their own.

**Delta this spec designs:** replace the two `emit_stub_spirv()` paths with a real
**DXBC chunk decoder → SM4/SM5 instruction decoder → SPIR-V IR builder → SPIR-V word
emitter**. Keep `parse_dxbc_header` and all the state-mapping code as-is. **Do not
create a third parallel twin** (CLAUDE.md rule 7 / §10.7): the two existing stub sites
must converge on ONE new module (`dxbc_spirv`), with `dxgi::ShaderTranslator` and
`d3d_translate::D3dTranslationLayer::translate_shader` both delegating to it.

## Prior art & OSS verdict
- **DXVK** (`doitsujin/dxvk`, `src/dxbc/`) — D3D9/10/11→Vulkan. Decodes the DXBC
  container, the SM4/SM5 token stream, and `ISGN`/`OSGN`/`RDEF` chunks, then emits
  SPIR-V via its own `SpirvModule` builder. Its register→SSA model (treat each DXBC
  temp register `r#` as a 4-component vector value, recompute on each write, swizzle on
  read) is the proven pattern. **Verdict: zlib — ➕ vendorable** (confirmed
  `docs/OSS_RECOMMENDATIONS.md` line 404: "DXVK is zlib... vendorable, not just
  referenceable"). We may port its DXBC decoder tables and SPIR-V emission *patterns*
  directly. Keep the zlib header on any harvested file. **Do not transplant its C++
  architecture wholesale** (rule 3) — harvest the opcode tables and the SSA model,
  re-express in `no_std` Rust.
- **VKD3D-Proton** (`vkd3d-shader`) — D3D12 DXIL→SPIR-V (DXIL = LLVM bitcode in a DXBC
  envelope). **Verdict: LGPL-2.1 — 📖 study/isolate** (`OSS_RECOMMENDATIONS.md` line
  405). Reference for the DXIL/SM6 path only; **out of scope for the first slices**
  (DXIL needs an LLVM-bitcode reader — a separate multi-month effort). Do NOT link or
  copy LGPL code into the `no_std` crate.
- **Mesa NIR / SPIRV-Tools** — `spirv-val` is our *validation oracle* on the dev box
  (not vendored). Mesa's `nir_to_spirv` is reference reading for SSA→SPIR-V lowering.
- **naga** (already cited in MasterChecklist 6.2 for WGSL→SPIR-V) — its `spv` backend
  is a clean Rust SPIR-V emitter to study for the word-encoding layer (MIT/Apache).
  Verdict: ➕ pattern-reference; we may end up sharing a SPIR-V-builder crate with the
  AthGFX WGSL path, but that is a later consolidation, not slice 1.
- Spec references (public): Microsoft "Direct3D 11 / Shader Model 4/5 Assembly" docs;
  the DXBC container layout (FourCC + checksum + chunk offset table) as documented by
  DXVK and the `wine` `d3dcompiler` reverse-engineering notes; the Khronos **SPIR-V
  Specification** (unified, §2 Binary Form, §3 instruction set) — the authoritative
  source for word encoding, capabilities, decorations, and the entry-point model.

## Design

### Part A — the DXBC container + bytecode model (the parser)
1. **Container** (already parsed by `parse_dxbc_header`): `"DXBC"` magic, 16-byte
   checksum, version(=1), `total_size`, `chunk_count`, then `chunk_count` u32 offsets,
   each pointing at a chunk = `{ FourCC: u32, size: u32, data[size] }`.
2. **Chunks the translator must read** (extend the existing chunk walk to collect, not
   just find SHEX):
   - `SHEX`/`SHDR` — the shader bytecode (SM4/SM5 token stream). **Required.**
   - `ISGN` / `OSGN` (+ `ISG1`/`OSG1`/`PCSG` SM5.1 variants) — input/output signature:
     array of `{ name_offset, semantic_index, system_value_type, component_type,
     register, mask, rw_mask }`. Maps to `ShaderParameter` (already defined in dxgi.rs).
     **Required** — this is how `v#`/`o#` registers bind to Vulkan `location`s/builtins.
   - `RDEF` — resource definitions: constant buffers (name, size, variables), and the
     binding table (textures `t#`, samplers `s#`, cbuffers `cb#`, UAVs `u#`). **Required**
     for any shader that samples a texture or reads a cbuffer (deferred past slice 1).
3. **SM4/SM5 token stream**: a flat `u32` array. First token = version
   (`program_type`/`major`/`minor`, already decoded) + length-in-tokens. Then a stream
   of **instructions**, each `{ opcode_token, operand_token(s)... }`:
   - `opcode_token`: bits[10:0] = opcode (`D3D10_SB_OPCODE_*`), bits[30:24] = instruction
     length in tokens, bit[31] = extended. `dcl_*` declaration opcodes appear first
     (input/output/temps/cbuffers/resources/samplers).
   - **Operand token**: `num_components` (0=scalar/1=N), `selection_mode`
     (mask/swizzle/select1), the 4×2-bit swizzle/mask, `operand_type` (the register
     file: `TEMP`=r#, `INPUT`=v#, `OUTPUT`=o#, `CONSTANT_BUFFER`=cb#, `IMMEDIATE32`,
     `RESOURCE`=t#, `SAMPLER`=s#), `index_dimension` (0/1/2/3), and per-dimension
     `index_representation` (immediate32 / immediate64 / relative `r#[...]`).
   - **Register files** to model: `r#` temps (SSA-rebuilt), `v#` inputs (loaded from
     SPIR-V `Input` vars per ISGN), `o#` outputs (stored to `Output` vars per OSGN),
     `cb#[i]` constant-buffer reads (uniform block load), immediates (OpConstant),
     `t#`/`s#` resources/samplers (descriptor-bound). Relative addressing
     (`cb0[r1.x + 4]`) → SPIR-V `OpAccessChain` with a dynamic index.

### Part B — the SPIR-V emission model
1. **Module skeleton** (SPIR-V spec §2.4 logical layout, in order):
   `OpCapability Shader` → `OpMemoryModel Logical GLSL450` → `OpEntryPoint <stage> %main
   "main" <interface-ids...>` → execution modes (`OriginUpperLeft` for fragment;
   `DepthReplacing` if depth-out) → debug names (optional) → **decorations**
   (`Location`, `BuiltIn`, `Binding`, `DescriptorSet`, `Block`, `Offset`) → **types**
   (`OpTypeFloat 32`, `OpTypeVector %float 4`, `OpTypePointer`, `OpTypeFunction`) →
   **global variables** (`OpVariable ... Input/Output/UniformConstant/Uniform`) →
   **function** (`OpFunction %void None %fn` / `OpLabel` / body / `OpReturn` /
   `OpFunctionEnd`).
2. **ID allocator**: monotonically increasing `result_id`; the final `bound` word =
   `next_id`. A de-dup cache keyed on (type-kind, operands) so each
   `OpTypeVector`/`OpTypePointer`/`OpConstant` is emitted once (SPIR-V requires types &
   constants be unique).
3. **Register → SSA model** (the DXVK pattern): each DXBC `r#` temp is backed by an
   `OpVariable Function %v4float` (a private 4-vector). A *write* to `r0.xy`:
   load the var, `OpVectorShuffle` to splice the new components into the masked lanes,
   `OpStore` back. A *read* `r0.zyx`: `OpLoad` then `OpVectorShuffle` for the swizzle.
   This is the simplest correct model (lets SPIR-V's own optimizer / the GPU driver
   promote to SSA); an SSA-on-the-fly model is a later optimization, not slice 1.
   `v#`/`o#`/`cb#` follow the same load/shuffle/store discipline against their
   respective Input/Output/Uniform variables.
4. **DX semantics → Vulkan locations/builtins** (driven by ISGN/OSGN
   `system_value_type`):
   - `SV_Position` → `BuiltIn Position` (VS out) / `BuiltIn FragCoord` (PS in).
   - `SV_Target[n]` → fragment `OpVariable Output` at `Location n`.
   - `SV_VertexID`/`SV_InstanceID` → `BuiltIn VertexIndex`/`InstanceIndex`.
   - user semantics (`TEXCOORD0`, `COLOR0`, …) → sequential `Location` assigned in
     signature order; **the VS-out `Location` map MUST equal the PS-in `Location` map**
     for the same semantic+index, or the pipeline interface mismatches. The translator
     therefore assigns Locations by a deterministic (semantic-name, semantic-index)
     ordering shared across stages, not by raw register number.
5. **Resource binding contract with AthGFX** (the descriptor-set layout): this MUST
   match what `D3d12RootSignature::to_raegfx_bindings()` already produces. Adopt the
   **DXVK binding convention**, namespaced per resource class to avoid `b#`/`t#`/`s#`
   collisions (D3D has separate register spaces; Vulkan has one binding number space):
   - cbuffers `cb#` → `set 0`, `binding = cb_index`, `OpTypeStruct{Block, Offset}` as
     `Uniform`.
   - textures `t#` → `set 0`, `binding = T_BASE + t_index`, `OpTypeImage`/`SampledImage`.
   - samplers `s#` → `set 0`, `binding = S_BASE + s_index`, `OpTypeSampler`.
   - UAVs `u#` → `set 0`, `binding = U_BASE + u_index`, storage image/buffer.
   `T_BASE`/`S_BASE`/`U_BASE` are fixed offsets (e.g. 0/64/128/192) recorded as a
   `BindingLayout` struct returned alongside the SPIR-V, so the pipeline-create side
   (GPU-gated) builds the matching `VkDescriptorSetLayout`. **This struct is the
   load-bearing seam** between the provable-now translator and the GPU-gated submit.

### Failure modes & security model
- Input DXBC is **untrusted attacker-controlled** (it ships inside game files). Every
  token read MUST be bounds-checked against the chunk size (the existing header parser
  already does `chunk_offset + 8 > len` guards — extend that discipline into the token
  decoder). On any malformed token: return `ShaderError::TranslationFailed`/
  `UnsupportedInstruction` — **never panic, never OOB read** (the SEH work already set
  this "no OOB panic on hostile bytes" bar; match it). Host KATs must include a
  truncated/corrupt-stream case that returns `Err`, not a crash.
- Unknown opcode in the supported-subset slice → `ShaderError::UnsupportedInstruction
  (opcode)` (already in the enum). The caller (`translate_shader`) surfaces this so the
  compat DB can log "shader X unsupported" rather than emitting garbage SPIR-V.
- Emitted SPIR-V is then validatable structurally on the host (`spirv-val`) — a
  malformed *emission* is a translator bug caught by the KAT, distinct from malformed
  *input*.

## Interface needs (NEEDS-INTERFACE)
**None for the translator itself.** It is pure CPU library code inside `raebridge`
(no new syscall, no `rae_abi` change). The translated SPIR-V crosses into the kernel
only via the *existing/future* AthGFX submit path, which is GPU-gated and out of scope.
If/when the GPU submit seam needs a "load SPIR-V module" syscall, that is a separate
`[interface]` escalation to raeen-architect at Phase 6.3 time — flag it, do not bundle.

## File-by-file plan
- `components/raebridge/src/dxbc_spirv/mod.rs` (NEW) — the one true translator module.
  Public API: `pub fn translate(dxbc: &[u8], opts: TranslateOpts) -> Result<Translated,
  ShaderError>` where `Translated { spirv: Vec<u8>, stage: raegfx::ShaderStage,
  bindings: BindingLayout, io: SignatureMap }`.
- `components/raebridge/src/dxbc_spirv/container.rs` (NEW) — chunk collector; reuses the
  `parse_dxbc_header` logic, extends it to return `{ shex, isgn, osgn, rdef }` slices.
- `components/raebridge/src/dxbc_spirv/decode.rs` (NEW) — SM4/SM5 token decoder:
  `Instruction`, `Operand`, `RegisterFile`, swizzle/mask; pure, exhaustively KAT-able.
- `components/raebridge/src/dxbc_spirv/signature.rs` (NEW) — ISGN/OSGN/RDEF parse →
  `ShaderParameter` (reuse the dxgi.rs type) + cbuffer/resource binding tables.
- `components/raebridge/src/dxbc_spirv/spirv.rs` (NEW) — `SpirvBuilder`: ID allocator,
  type/constant de-dup, instruction word emitter, module assembler. Pure word logic.
- `components/raebridge/src/dxbc_spirv/lower.rs` (NEW) — the opcode→SPIR-V lowering
  (the instruction match: `mov`/`add`/`mul`/`mad`/`dp2-4`/`sample`/declarations…).
- `components/raebridge/src/dxgi.rs` (EDIT, by implementer) — delete the local
  `emit_stub_spirv`; `ShaderTranslator::translate_dxbc_to_spirv` delegates to
  `dxbc_spirv::translate`. (DXIL path stays an explicit `Unsupported` stub.)
- `components/raebridge/src/d3d_translate.rs` (EDIT, by implementer) — delete
  `emit_stub_spirv`; `D3dTranslationLayer::translate_shader` delegates to the same.
- `components/raebridge/src/lib.rs` (EDIT) — `pub mod dxbc_spirv;`.
- `components/raebridge/tests/dxbc_spirv_kat.rs` (NEW) — host KATs (see proof recipe).
- `components/raebridge/tests/fixtures/` (NEW) — committed `.dxbc` fixtures + their
  expected `spirv-val`-clean disassembly snapshots.

Constraint reminder: crate is `no_std` + `alloc` (depends on `raegfx`, `iced-x86`
no_std, `object` no_std). The translator MUST be `no_std`-clean — `alloc::vec::Vec`,
`BTreeMap`, no `std`. The KAT crate (`tests/`) builds for the host and may use `std`.

## The first PROVABLE slice (host-KAT, GPU-free)

**Slice 1 scope — the minimal end-to-end, two shaders:**
1. **Passthrough vertex shader**: read `v0` (POSITION, float4), write `o0`
   (`SV_Position`). Opcodes: `dcl_input v0`, `dcl_output_siv o0, position`, `mov o0, v0`,
   `ret`. Exercises: container+ISGN/OSGN parse, input/output var creation, builtin
   mapping, the `mov` lowering, module assembly.
2. **Solid-color pixel shader**: write a constant `float4(R,G,B,A)` to `o0`
   (`SV_Target0`). Opcodes: `dcl_output o0`, `mov o0, l(r,g,b,a)`, `ret`. Exercises:
   immediate→`OpConstantComposite`, fragment `Location 0` output, `OriginUpperLeft`.

**Supported opcode subset for slice 1:** `mov`, `ret`, and the `dcl_*` declarations
needed by the two shaders. (`add`/`mul`/`mad`/`dp2`/`dp3`/`dp4` land in slice 2;
`sample`+textures+cbuffers in slice 3.)

**Fixture generation (exact dev-box commands).** Author two tiny HLSL files and compile
to DXBC with the Windows SDK `fxc` (SM5.0; ships with VS / Win SDK):
```
:: passthrough.hlsl  →  float4 main(float4 pos : POSITION) : SV_Position { return pos; }
fxc /T vs_5_0 /E main /Fo passthrough_vs.dxbc passthrough.hlsl

:: solidcolor.hlsl   →  float4 main() : SV_Target { return float4(1,0,0,1); }
fxc /T ps_5_0 /E main /Fo solidcolor_ps.dxbc solidcolor.hlsl
```
(`dxc /T vs_6_0` would produce DXIL — explicitly NOT slice 1; SM5 via `fxc` is the
target. WSL2 alternative: `wine fxc.exe ...`, or check in pre-built fixtures so the KAT
needs no Windows SDK at run time.) Commit the two `.dxbc` blobs to
`components/raebridge/tests/fixtures/`.

**Proof recipe (the KAT, FAIL-able):** `components/raebridge/tests/dxbc_spirv_kat.rs`,
run with `cargo test -p raebridge` on the dev box:
1. `translate(passthrough_vs.dxbc)` returns `Ok`, `stage == Vertex`.
2. **Structural asserts on the emitted words** (no GPU needed): word[0] ==
   `0x07230203`; `bound` (word[3]) > 1 (proves it's not the stub's `bound = 1`); the
   stream contains `OpEntryPoint Vertex`, `OpDecorate ... BuiltIn Position`, an
   `OpVariable ... Input`, an `OpFunction`/`OpReturn`/`OpFunctionEnd`. (Decode the word
   stream and match opcodes — a hand-checked reference list.)
3. **External validation (when available):** write the bytes to a temp file and shell
   out to `spirv-val` (Vulkan SDK / `spirv-tools`); assert exit 0. Gate this behind a
   `RAEEN_SPIRV_VAL` env / `#[cfg_attr]` so the test still runs (structural-only) on
   boxes without the tool — but CI with the tool present makes it authoritative.
4. **Negative cases (prove FAIL works):** truncated DXBC → `Err(InvalidBytecode)`;
   a DXBC whose SHEX contains an opcode outside the slice-1 subset →
   `Err(UnsupportedInstruction)`. A test that cannot print FAIL is a false green
   (CLAUDE.md §4.16).

This is the entire proof — **zero GPU, zero QEMU, zero iron**. It runs on the dev box
in milliseconds, exactly the "host KAT first" cheapest-real-proof layer (§4.15).

## Scope ladder + honest deferral

| Tier | Scope | Provable how | Gate |
|---|---|---|---|
| **Slice 1** | SM4/5 container + ISGN/OSGN, `mov`/`ret`/`dcl`, passthrough VS + solid PS | host KAT + `spirv-val` | **none — buildable now** |
| Slice 2 | ALU subset: `add mul mad div min max` + `dp2/3/4` + saturate/swizzle/neg/abs modifiers | host KAT vs hand-checked SPIR-V | none |
| Slice 3 | Resources: `cb#` uniform blocks (RDEF), `t#`/`s#` + `sample`/`sample_l`/`ld`; the `BindingLayout` ↔ AthGFX descriptor contract | host KAT (+ `spirv-val`) | none for translate; **pipeline-create is GPU-gated** |
| Slice 4 | Control flow: `if/else/endif`, `loop/endloop/break`, `discard` → SPIR-V structured CFG (`OpSelectionMerge`/`OpLoopMerge`) | host KAT | none |
| Deferred | Geometry/Hull/Domain/Compute stages; SM5 typed/structured/append UAVs; tessellation | — | none, but large |
| **Deferred (separate effort)** | **DXIL / SM6 (D3D12)** — LLVM-bitcode reader + DXIL→SPIR-V; VKD3D-Proton is LGPL 📖 reference only | — | not GPU-gated but a multi-month subsystem of its own; **do not attempt in this workstream** |

**The CPU-side vs GPU-gated boundary (the load-bearing line):**
- **CPU-side, provable NOW (this spec):** parse DXBC → decode SM4/5 → emit valid
  SPIR-V → validate with `spirv-val`. The output is a `Vec<u8>` of SPIR-V words plus a
  `BindingLayout` + `SignatureMap`. Nothing here touches the GPU.
- **GPU-gated, stays stubbed (NOT this spec):** taking that SPIR-V +
  `BindingLayout` and doing `vkCreateShaderModule` / `vkCreateGraphicsPipelines` /
  `vkQueueSubmit` / scanout. That is the AthGFX submit seam, blocked on the owner's
  amdgpu bring-up (MEMORY: amdgpu iron hang / Mesa seam). The translator's job ends at
  "here is provably-valid SPIR-V"; the submit consumer remains a stub until the GPU path
  lands. **This boundary is exactly why the translator is worth building now** — it is
  the largest AthBridge DirectX piece that needs zero GPU to prove.

Honest scale note: a *complete* DXBC→SPIR-V translator (all stages, full SM5, UAVs,
control flow, plus the DXIL path for D3D12) is a multi-month subsystem on the order of
DXVK's `src/dxbc` (~15k LOC). This spec deliberately makes **slice 1 tiny and fully
provable** and ladders the rest, rather than boiling the ocean.

## Acceptance criteria (the exact proof)
- `cargo test -p raebridge` MUST show the new KAT module green, including:
  - `dxbc_spirv_passthrough_vs -> PASS` with assertions: emitted `bound > 1`,
    contains `OpEntryPoint Vertex`, contains `BuiltIn Position` decoration.
  - `dxbc_spirv_solidcolor_ps -> PASS` with assertions: contains `OpEntryPoint
    Fragment` + `OriginUpperLeft`, `Location 0` output, `OpConstantComposite` for the
    color.
  - `dxbc_spirv_rejects_truncated -> Err(InvalidBytecode)` and
    `dxbc_spirv_rejects_unsupported_opcode -> Err(UnsupportedInstruction)` (proves the
    test can FAIL).
- When `spirv-val` is present (CI / Vulkan SDK box): both emitted modules pass
  `spirv-val` with exit 0 (assert in the KAT, gated on `RAEEN_SPIRV_VAL`).
- A `raebridge` boot smoketest line (the R10-style proof carried into QEMU, since
  raebridge already prints `[raebridge] ...` self-tests): translate an *embedded*
  passthrough-VS fixture at init and print
  `[raebridge] DXBC->SPIR-V translate (passthrough VS): bound=<n> entrypoint=Vertex ->
  PASS` (or `-> FAIL` on any error). The fixture is embedded with `include_bytes!` so it
  needs no filesystem.
- The new module's top docstring MUST quote the Concept promise above (R10 / R1).
- `emit_stub_spirv` MUST be deleted from BOTH `d3d_translate.rs` and `dxgi.rs` (no
  surviving stub twin); both call sites delegate to `dxbc_spirv::translate`.

## Handoff
- **Implementer: raeen-compat** (owns `components/raebridge`). All work is in
  `components/raebridge/src/dxbc_spirv/` + the two delegating edits + the tests crate;
  isolated commits; no `rae_abi`/kernel changes.
- **Unblocks checklist lines:**
  - Phase 11.2 line 1432 "DXVK port: DirectX 9/10/11 → Vulkan translation" — the shader
    half (this spec is the shader translator; the API-state half already exists).
  - Phase 11.2 line 1790 "DXVK (zlib) ➕ vendorable" harvest is realized here.
  - Feeds (does not complete) line 1437 "Steam runs" and the Phase 11.3 AAA-game
    acceptance — those additionally need the GPU submit path.
- **Sequencing:** no interface commit needed first. Slice 1 is fully independent and
  can start immediately. Slices 2–4 are linear after it. The DXIL/D3D12 path and the
  GPU submit consumer are explicitly out of this workstream (the latter waits on the
  amdgpu bring-up; flag a Phase 6.3 `[interface]` "load SPIR-V" syscall to
  raeen-architect only when the GPU path is ready to consume the output).
- **OSS hygiene for the implementer:** port DXVK's DXBC opcode tables / SSA pattern
  (zlib — keep the header on harvested files); do **not** pull in VKD3D-Proton (LGPL)
  or any DXIL/LLVM code; do not link `std` SPIRV-Tools into the `no_std` crate
  (`spirv-val` is a dev-box validation oracle invoked from the `tests/` harness only).
