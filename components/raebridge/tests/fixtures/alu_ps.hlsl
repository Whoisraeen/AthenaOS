// Slice-2 ALU pixel shader (ps_5_0).
//
// Exercises the new translator subset in one shader:
//   * source swizzles  (.xyz on the normal/light vectors)
//   * dest write-mask   (writing only .xyz then .w of o0)
//   * source modifier   (negate on the light direction)
//   * dp3 dot product   (N.L diffuse term)
//   * mul / mad         (scale + bias the color)
//   * saturate (_sat)   (clamp the diffuse term to 0..1)
//   * temp registers    (r#)
//
// Two interpolated user inputs (a "normal" and a "light dir") plus a base color,
// computing a clamped Lambert term and a mad-blended RGB, alpha forced to 1.
struct VsOut {
    float4 nrm   : TEXCOORD0; // surface normal  (xyz used)
    float4 ldir  : TEXCOORD1; // light direction (xyz used)
    float4 color : TEXCOORD2; // base color
};

float4 main(VsOut i) : SV_Target {
    // saturate(dot(nrm.xyz, -ldir.xyz)) -> clamped Lambert diffuse term.
    float ndotl = saturate(dot(i.nrm.xyz, -i.ldir.xyz));
    // mad: color.rgb * ndotl + 0.05 ambient ; alpha forced to 1.0
    float3 lit = i.color.rgb * ndotl + 0.05;
    return float4(lit, 1.0);
}
