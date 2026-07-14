// SM4/SM5 float transcendental coverage (ps_5_0): exp2 -> `exp`, log2 -> `log`.
//
// Completes the float-ALU transcendental set the translator lowers (sqrt/rsq/rcp
// /frc/round were already covered). HLSL `exp2`/`log2` map 1:1 to the SM4 `exp`
// /`log` opcodes (both base-2). Kept straight-line (no flow control, no
// resources) so it stays inside the supported ALU subset, and compiled /Od so
// fxc emits the `exp`/`log` opcodes directly rather than fusing them away.
struct VsOut {
    float4 a : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    float4 e = exp2(i.a);          // -> exp
    float4 l = log2(i.a + 1.0);    // -> add (already supported) then log
    return e + l;                  // -> add
}
