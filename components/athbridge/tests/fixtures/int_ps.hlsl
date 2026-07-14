// SM4/SM5 integer-ALU coverage (ps_5_0): iadd, and, or, xor, ishl, ushr, not,
// ishr (signed/arithmetic), ineg. Registers are typeless 32-bit; uint<->int and
// asfloat are free bit-reinterpretations (fxc emits `mov`, not a conversion op),
// so this stays inside the translator's supported subset (no ftoi/itof/utof).
struct VsOut {
    uint4 a : TEXCOORD0;
    uint4 b : TEXCOORD1;
};

float4 main(VsOut i) : SV_Target {
    uint4 a = i.a;
    uint4 b = i.b;
    uint4 r = (a + b) & (a | b); // iadd, and, or
    r = r ^ a;                   // xor
    r = r << 2;                  // ishl
    r = r >> 1;                  // ushr (r is uint -> logical right shift)
    r = ~r;                      // not
    int4 sr = (int4)a >> 3;      // ishr (signed -> arithmetic right shift)
    int4 ng = -(int4)b;          // ineg
    r = r + (uint4)sr + (uint4)ng; // iadd (the casts are free bitcasts)
    return asfloat(r);           // bit-reinterpret to the float4 SV_Target (mov)
}
