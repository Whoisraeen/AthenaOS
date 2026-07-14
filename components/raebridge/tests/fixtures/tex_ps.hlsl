// SM4/SM5 Texture2D sampling (ps_5_0): the texture/sampler binding model.
// Emits dcl_resource_texture2d t0 + dcl_sampler s0 + sample. Compiled /Od.
Texture2D tex : register(t0);
SamplerState smp : register(s0);

struct VsOut {
    float4 uv : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    return tex.Sample(smp, i.uv.xy);
}
