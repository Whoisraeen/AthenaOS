// SM4/SM5 structured control flow (ps_5_0): a real if/else/endif. The [branch]
// attribute forces fxc to emit if_nz/else/endif rather than flattening to movc,
// so this exercises the translator's basic-block emission. Compiled /Od.
struct VsOut {
    float4 a : TEXCOORD0;
    float4 b : TEXCOORD1;
};

float4 main(VsOut i) : SV_Target {
    float4 r;
    [branch]
    if (i.a.x > 0.5) {
        r = i.a + i.b;
    } else {
        r = i.a - i.b;
    }
    return r;
}
