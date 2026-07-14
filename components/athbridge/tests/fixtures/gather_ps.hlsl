// SM5 gather (ps_5_0): Texture2D.Gather fetches the 4 texels (the named channel)
// that bilinear filtering would use — the PCF / SSAO / texture-space idiom. fxc
// emits gather4. Proves the translator lowers it to OpImageGather. Compiled /Od.
Texture2D tex : register(t0);
SamplerState smp : register(s0);

struct VsOut {
    float4 uv : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    return tex.Gather(smp, i.uv.xy); // gather4 of the red channel
}
