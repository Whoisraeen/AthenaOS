// SM4/SM5 unsigned conversion coverage (ps_5_0): ftou (float->uint) + utof
// (uint->float). Compiled /Od so fxc keeps the conversions.
struct VsOut {
    float4 a : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    uint4 au = (uint4)(i.a * 8.0); // mul then ftou
    float4 r = (float4)au * 0.25;  // utof then mul
    return r;
}
