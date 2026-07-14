// SM4/SM5 sincos coverage (ps_5_0): the two-destination transcendental. fxc's
// sincos() intrinsic emits the SINCOS opcode writing both sin and cos. /Od.
struct VsOut {
    float4 a : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    float4 s, c;
    sincos(i.a, s, c);
    return s + c;
}
