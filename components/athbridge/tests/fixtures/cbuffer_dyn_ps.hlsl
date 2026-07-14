// SM4/SM5 DYNAMIC constant-buffer index (ps_5_0): cb0 indexed by a runtime value
// (the skinning/instancing idiom — bone/palette arrays indexed per-vertex/pixel).
// fxc emits a relative-addressed cb operand (cb0[rN.x + imm]). Proves the
// translator handles a register-indexed OpAccessChain. Compiled /Od.
cbuffer Palette : register(b0) {
    float4 colors[8];
};

struct VsOut {
    float4 uv : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    int idx = (int)i.uv.x;       // runtime index
    return colors[idx];          // cb0[r#] dynamic read
}
