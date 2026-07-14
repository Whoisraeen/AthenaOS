// Slice-2 ALU coverage pixel shader #2 (ps_5_0).
//
// Spreads the remaining float ALU ops the translator must lower, each touching a
// swizzle / write-mask so the masked-lane path is exercised broadly:
//   add, mul, div, min, max, dp2, dp4, sqrt, rsq, rcp, frc, round (ne)
// plus an abs() source modifier. Kept as straight-line ALU (no flow control,
// no resources) so it stays inside the slice-2 subset.
struct VsOut {
    float4 a : TEXCOORD0;
    float4 b : TEXCOORD1;
};

float4 main(VsOut i) : SV_Target {
    float4 a = i.a;
    float4 b = i.b;
    float2 s  = a.xy + b.xy;          // add
    float2 p  = a.zw * b.zw;          // mul
    float  d2 = dot(a.xy, b.xy);      // dp2
    float  d4 = dot(a, b);            // dp4
    float  q  = a.x / b.y;            // div
    float  mn = min(a.z, b.z);        // min
    float  mx = max(a.w, b.w);        // max
    float  rt = sqrt(abs(a.x));       // sqrt + abs modifier
    float  rs = rsqrt(abs(b.x) + 1.0);// rsq
    float  rc = 1.0 / (b.z + 2.0);    // rcp pattern
    float  fr = frac(a.y);            // frc
    float  rn = round(a.z);           // round_ne
    float r = s.x + p.x + d2 + d4 + q + mn + mx + rt + rs + rc + fr + rn;
    return float4(r, r, r, 1.0);
}
