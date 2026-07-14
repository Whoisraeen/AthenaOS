// SM4/SM5 constant-buffer read (ps_5_0): the MOST common shader input — material
// params, transforms, light data live in a cbuffer. fxc emits
// dcl_constantbuffer cb0[N] and reads cb0[immediate]. Proves the translator
// emits a uniform block + OpAccessChain/OpLoad. Compiled /Od.
cbuffer Params : register(b0) {
    float4 tint;  // cb0[0]
    float4 scale; // cb0[1]
};

struct VsOut {
    float4 c : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    return i.c * scale + tint;
}
