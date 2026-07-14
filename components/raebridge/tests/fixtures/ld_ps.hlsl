// SM5 texel load (ps_5_0): Texture2D.Load(int3(x,y,mip)) — read an EXACT texel
// (no filtering, no sampler). The post-processing / deferred-G-buffer idiom,
// common in modern shaders. fxc emits ld (a fetch with an integer coord whose
// .z is the mip level). Proves the translator lowers it to OpImageFetch + Lod.
// Compiled /Od.
Texture2D<float4> tex : register(t0);

struct VsOut {
    float4 uv : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    int3 c = int3((int)i.uv.x, (int)i.uv.y, 0); // x, y, mip
    return tex.Load(c);
}
