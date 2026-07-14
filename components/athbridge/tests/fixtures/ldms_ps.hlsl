// SM5 multisample texel load (ps_5_0): Texture2DMS.Load(coord, sampleIndex) —
// the MSAA-resolve idiom (read one sample of a multisampled target). fxc emits
// ldms (a fetch from a multisampled image, no sampler). Proves the translator
// lowers it to OpImageFetch with the Sample image operand. Compiled /Od.
Texture2DMS<float4> tex : register(t0);

struct VsOut {
    float4 uv : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    int2 c = int2(i.uv.xy);
    return tex.Load(c, 0); // sample 0
}
