// Slice-2 rcp/swizzle pixel shader (ps_5_0, /Od so the rcp opcode survives).
//
// fxc fuses rcp into div under optimization, so this fixture is compiled with
// /Od to force a real `rcp` opcode (0x81), a `.xyz` write-mask and an `.xxxx`
// broadcast swizzle -- the reciprocal + broadcast lowering path.
struct VsOut { float4 a : TEXCOORD0; };
float4 main(VsOut i) : SV_Target {
    float r = rcp(i.a.x);
    return float4(r, r, r, 1.0);
}
