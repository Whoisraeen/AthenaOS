// SM4/SM5 canonical vertex shader (vs_5_0): transform a position by an MVP
// matrix read from a constant buffer -> SV_Position. THE most-run shader type
// (every rendered object). Exercises cbuffer + the 4x dp4 matrix multiply + the
// SV_Position system-value output together. Compiled /Od.
cbuffer Transform : register(b0) {
    float4x4 mvp;
};

struct VsIn {
    float4 pos : POSITION;
};

struct VsOut {
    float4 pos : SV_Position;
};

VsOut main(VsIn i) {
    VsOut o;
    o.pos = mul(i.pos, mvp);
    return o;
}
