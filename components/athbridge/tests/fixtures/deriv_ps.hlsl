// SM4/SM5 screen-space derivatives (ps_5_0): HLSL ddx()/ddy() — used for mip
// selection, edge AA, procedural/screen-space effects. fxc emits deriv_rtx*/
// deriv_rty*. Proves the translator lowers them to OpDPdx/OpDPdy. Compiled /Od.
struct VsOut {
    float4 uv : TEXCOORD0;
};

float4 main(VsOut i) : SV_Target {
    float2 dx = ddx(i.uv.xy);
    float2 dy = ddy(i.uv.xy);
    return float4(dx, dy);
}
