// SM4/SM5 discard (ps_5_0): the HLSL clip() / alpha-test idiom — discards the
// fragment when the argument is negative. fxc emits a comparison + discard_z/_nz.
// Proves the translator lowers discard to a structured OpKill. Compiled /Od.
struct VsOut {
    float4 c : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    clip(i.c.x - 0.5f); // discard this pixel if (c.x - 0.5) < 0
    return i.c;
}
