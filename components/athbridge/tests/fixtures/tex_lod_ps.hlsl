// SM4/SM5 explicit-LOD texture sampling (ps_5_0): Texture2D.SampleLevel emits
// the sample_l opcode (explicit LOD source). Compiled /Od.
Texture2D tex : register(t0);
SamplerState smp : register(s0);

struct VsOut {
    float4 uv : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    return tex.SampleLevel(smp, i.uv.xy, i.uv.z);
}
