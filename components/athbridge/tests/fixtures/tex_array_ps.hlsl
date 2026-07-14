// SM4/SM5 Texture2DArray sampling (ps_5_0): a texture ARRAY binding. fxc emits
// dcl_resource_texture2darray t0 + sample with a 3-component coord (.xy = uv,
// .z = array slice). Proves the translator reads the resource dimension and
// widens the sample coordinate. Compiled /Od.
Texture2DArray tex : register(t0);
SamplerState smp : register(s0);

struct VsOut {
    float4 uv : TEXCOORD0; // .xy = uv, .z = array slice
};

float4 main(VsOut i) : SV_Target {
    return tex.Sample(smp, i.uv.xyz);
}
