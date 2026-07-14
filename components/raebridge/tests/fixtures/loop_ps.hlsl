// SM4/SM5 loop control flow (ps_5_0): a real counted loop. [loop] forces fxc to
// emit loop/breakc/endloop (with a back-edge) instead of unrolling, exercising
// the translator's OpLoopMerge block scaffold. Compiled /Od.
struct VsOut {
    float4 a : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    float4 acc = 0;
    [loop]
    for (int k = 0; k < 4; k++) {
        acc += i.a * (float)k;
    }
    return acc;
}
