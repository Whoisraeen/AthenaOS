// SM4/SM5 TextureCube sampling (ps_5_0): a cube-map binding (skyboxes /
// reflections). fxc emits dcl_resource_texturecube t0 + sample with a
// 3-component direction coord. Proves the translator emits a Cube OpTypeImage
// and a vec3 coord from the resource dimension. Compiled /Od.
TextureCube tex : register(t0);
SamplerState smp : register(s0);

struct VsOut {
    float4 dir : TEXCOORD0; // .xyz = cube direction
};

float4 main(VsOut i) : SV_Target {
    return tex.Sample(smp, i.dir.xyz);
}
