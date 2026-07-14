// Slice-2 standalone-mul pixel shader (ps_5_0).
//
// A bare component-wise multiply of two interpolated inputs with a write-mask,
// so fxc emits a real `mul` opcode (0x38) rather than fusing it into a mad.
struct VsOut {
    float4 a : TEXCOORD0;
    float4 b : TEXCOORD1;
};
float4 main(VsOut i) : SV_Target {
    return i.a * i.b;
}
