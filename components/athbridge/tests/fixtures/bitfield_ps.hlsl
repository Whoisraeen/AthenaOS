// Exercises the SM5 bitfield opcodes ubfe / ibfe / bfi. fxc lowers the
// mask-shift extract/insert idioms below into ubfe/ibfe/bfi (verified via
// `fxc /T ps_5_0 /Fc`). Compiled: fxc /T ps_5_0 /E main /Fo bitfield_ps.dxbc
struct VsOut { uint4 a : TEXCOORD0; int4 b : TEXCOORD1; };

float4 main(VsOut i) : SV_Target {
    uint4 a = i.a;
    int4  b = i.b;
    // Unsigned bitfield extract: 8 bits at offset 4  -> ubfe.
    uint4 ue = (a >> 4) & 0xFFu;
    // Signed bitfield extract: 8 bits at offset 4, sign-extended -> ibfe.
    int4  se = (b << (32 - 8 - 4)) >> (32 - 8);
    // Bitfield insert: put low 8 bits of a into b at offset 4 -> bfi.
    uint4 mask = 0xFFu << 4;
    uint4 bi = ((uint4)b & ~mask) | ((a << 4) & mask);
    uint4 r = ue + (uint4)se + bi;
    return asfloat(r);
}
