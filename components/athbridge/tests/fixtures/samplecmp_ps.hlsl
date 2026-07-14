// SM4/SM5 depth-comparison sampling (ps_5_0): Texture2D.SampleCmp with a
// SamplerComparisonState — THE shadow-mapping idiom (in nearly every 3D scene
// with shadows). fxc emits dcl_sampler s0, mode_comparison + sample_c. Proves the
// translator lowers it to OpImageSampleDrefImplicitLod on a depth image.
// Compiled /Od.
Texture2D shadowMap : register(t0);
SamplerComparisonState shadowSampler : register(s0);

struct VsOut {
    float4 uv : TEXCOORD0; // .xy = shadow-map coord, .z = compare depth
};

float4 main(VsOut i) : SV_Target {
    float lit = shadowMap.SampleCmp(shadowSampler, i.uv.xy, i.uv.z);
    return float4(lit, lit, lit, 1.0f);
}
