// SM4/SM5 conversion coverage (ps_5_0): ftoi (float->signed int) + itof
// (signed int->float). Compiled /Od so fxc emits the conversions rather than
// folding (int4)(float)->float back to identity.
struct VsOut {
    float4 a : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    int4 ai = (int4)(i.a * 10.0); // mul then ftoi
    float4 r = (float4)ai + 0.5;  // itof then add
    return r;
}
