// Exercises the SM5 half-precision conversions f32tof16 / f16tof32.
// f32tof16(float) -> uint with the fp16 bits in the low 16; f16tof32(uint)
// reads the low-16 fp16 back to float. Compiled:
//   fxc /T ps_5_0 /E main /Fo half_ps.dxbc half_ps.hlsl
struct VsOut { float4 a : TEXCOORD0; uint4 b : TEXCOORD1; };

float4 main(VsOut i) : SV_Target {
    uint4 packed = f32tof16(i.a);      // -> f32tof16
    float4 back = f16tof32(i.b);       // -> f16tof32
    float4 roundtrip = f16tof32(packed);
    return back + roundtrip;
}
