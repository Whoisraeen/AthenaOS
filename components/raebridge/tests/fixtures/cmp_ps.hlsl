// SM4/SM5 comparison + conditional-select coverage (ps_5_0): ge/lt/eq/ne produce
// per-lane uint masks, movc selects per lane. Compiled /Od so fxc keeps the
// comparisons + movc rather than folding them.
struct VsOut {
    float4 a : TEXCOORD0;
    float4 b : TEXCOORD1;
};

float4 main(VsOut i) : SV_Target {
    float4 a = i.a;
    float4 b = i.b;
    float4 r = (a >= b) ? a : b; // ge + movc
    r = (a < b) ? r : a;         // lt + movc
    r = (a == b) ? r : b;        // eq + movc
    r = (a != b) ? a : r;        // ne + movc
    return r;
}
