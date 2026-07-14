// SM5 integer min/max + multiply-add + bit-manipulation coverage (ps_5_0):
//   imin/imax (min/max on int4), umin/umax (min/max on uint4),
//   imad (a*b+c fused on int4), bfrev (reversebits), countbits,
//   firstbit_lo (firstbitlow), firstbit_hi (firstbithigh on uint),
//   firstbit_shi (firstbithigh on signed int).
// Registers are typeless 32-bit; the int<->uint casts are free bit
// reinterpretations. Everything is summed into the output so nothing is
// dead-stripped by the optimizer.
struct VsOut {
    int4  a : TEXCOORD0;
    int4  b : TEXCOORD1;
    uint4 c : TEXCOORD2;
};

float4 main(VsOut i) : SV_Target {
    int4  a = i.a;
    int4  b = i.b;
    uint4 c = i.c;

    int4  smn = min(a, b);              // imin
    int4  smx = max(a, b);              // imax
    uint4 umn = min(c, (uint4)a);       // umin
    uint4 umx = max(c, (uint4)b);       // umax
    int4  mad = a * b + smx;            // imad (a*b + c fused)

    uint4 rev = reversebits(c);         // bfrev
    uint4 pop = countbits(c);           // countbits
    uint4 fhi = (uint4)firstbithigh(c); // firstbit_hi (unsigned)
    uint4 flo = (uint4)firstbitlow(c);  // firstbit_lo
    uint4 fsh = (uint4)firstbithigh(a); // firstbit_shi (signed input)

    uint4 r = (uint4)smn + (uint4)smx + umn + umx + (uint4)mad
            + rev + pop + fhi + flo + fsh;
    return asfloat(r);
}
