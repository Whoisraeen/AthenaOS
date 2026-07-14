//! RaeGFX Shader Compiler IR — custom intermediate representation for shaders,
//! CPU-side interpreter (software path), and basic SPIR-V parsing for hardware interop.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════════════════
// Shader IR Types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    Float,
    Vec2,
    Vec3,
    Vec4,
    Int,
    IVec2,
    IVec3,
    IVec4,
    Mat3,
    Mat4,
    Sampler2D,
    Bool,
}

impl VarType {
    pub fn component_count(&self) -> usize {
        match self {
            Self::Float | Self::Int | Self::Bool => 1,
            Self::Vec2 | Self::IVec2 => 2,
            Self::Vec3 | Self::IVec3 => 3,
            Self::Vec4 | Self::IVec4 => 4,
            Self::Mat3 => 9,
            Self::Mat4 => 16,
            Self::Sampler2D => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShaderVariable {
    pub name: String,
    pub var_type: VarType,
    pub location: u32,
}

#[derive(Debug, Clone)]
pub struct ShaderModule {
    pub stage: ShaderStage,
    pub instructions: Vec<ShaderIR>,
    pub inputs: Vec<ShaderVariable>,
    pub outputs: Vec<ShaderVariable>,
    pub uniforms: Vec<ShaderVariable>,
    pub entry_point: String,
}

#[derive(Debug, Clone)]
pub enum ShaderIR {
    // Variable declaration
    DeclareVar {
        id: u32,
        var_type: VarType,
    },
    LoadInput {
        dst: u32,
        location: u32,
    },
    StoreOutput {
        src: u32,
        location: u32,
    },
    LoadUniform {
        dst: u32,
        binding: u32,
        offset: u32,
    },

    // Arithmetic
    Add {
        dst: u32,
        a: u32,
        b: u32,
    },
    Sub {
        dst: u32,
        a: u32,
        b: u32,
    },
    Mul {
        dst: u32,
        a: u32,
        b: u32,
    },
    Div {
        dst: u32,
        a: u32,
        b: u32,
    },
    Dot {
        dst: u32,
        a: u32,
        b: u32,
    },
    Cross {
        dst: u32,
        a: u32,
        b: u32,
    },
    Normalize {
        dst: u32,
        src: u32,
    },
    Negate {
        dst: u32,
        src: u32,
    },

    // Matrix ops
    MatMul {
        dst: u32,
        mat: u32,
        vec: u32,
    },
    Transpose {
        dst: u32,
        src: u32,
    },

    // Texture sampling
    SampleTexture {
        dst: u32,
        texture: u32,
        sampler: u32,
        coord: u32,
    },

    // Control flow
    If {
        condition: u32,
        then_block: Vec<ShaderIR>,
        else_block: Vec<ShaderIR>,
    },
    Loop {
        body: Vec<ShaderIR>,
        max_iterations: u32,
    },
    Break,
    Return {
        value: Option<u32>,
    },

    // Built-in math functions
    Clamp {
        dst: u32,
        val: u32,
        min: u32,
        max: u32,
    },
    Mix {
        dst: u32,
        a: u32,
        b: u32,
        t: u32,
    },
    Pow {
        dst: u32,
        base: u32,
        exp: u32,
    },
    Sqrt {
        dst: u32,
        src: u32,
    },
    Sin {
        dst: u32,
        src: u32,
    },
    Cos {
        dst: u32,
        src: u32,
    },
    Abs {
        dst: u32,
        src: u32,
    },
    Min {
        dst: u32,
        a: u32,
        b: u32,
    },
    Max {
        dst: u32,
        a: u32,
        b: u32,
    },
    Floor {
        dst: u32,
        src: u32,
    },
    Fract {
        dst: u32,
        src: u32,
    },
    Step {
        dst: u32,
        edge: u32,
        x: u32,
    },
    SmoothStep {
        dst: u32,
        edge0: u32,
        edge1: u32,
        x: u32,
    },

    // Comparison
    LessThan {
        dst: u32,
        a: u32,
        b: u32,
    },
    GreaterThan {
        dst: u32,
        a: u32,
        b: u32,
    },
    Equal {
        dst: u32,
        a: u32,
        b: u32,
    },

    // Swizzle/component access
    Swizzle {
        dst: u32,
        src: u32,
        components: [u8; 4],
        count: u8,
    },

    // Construct vector from components
    Construct {
        dst: u32,
        var_type: VarType,
        components: Vec<u32>,
    },

    // Constants
    ConstFloat {
        dst: u32,
        value: u32,
    },
    ConstVec4 {
        dst: u32,
        x: u32,
        y: u32,
        z: u32,
        w: u32,
    },
    ConstMat4 {
        dst: u32,
        values: [u32; 16],
    },
    ConstInt {
        dst: u32,
        value: i32,
    },
}

// ═══════════════════════════════════════════════════════════════════════════
// Shader Value — runtime representation for the interpreter
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum ShaderValue {
    Float(f32),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    Int(i32),
    IVec2([i32; 2]),
    IVec3([i32; 3]),
    IVec4([i32; 4]),
    Mat3([f32; 9]),
    Mat4([f32; 16]),
    Bool(bool),
    Sampler(u32),
}

impl ShaderValue {
    pub fn as_float(&self) -> f32 {
        match self {
            Self::Float(v) => *v,
            Self::Int(v) => *v as f32,
            Self::Bool(v) => {
                if *v {
                    1.0
                } else {
                    0.0
                }
            }
            _ => 0.0,
        }
    }

    pub fn as_vec4(&self) -> [f32; 4] {
        match self {
            Self::Vec4(v) => *v,
            Self::Vec3(v) => [v[0], v[1], v[2], 0.0],
            Self::Vec2(v) => [v[0], v[1], 0.0, 0.0],
            Self::Float(v) => [*v, *v, *v, *v],
            _ => [0.0; 4],
        }
    }

    pub fn as_vec3(&self) -> [f32; 3] {
        match self {
            Self::Vec4(v) => [v[0], v[1], v[2]],
            Self::Vec3(v) => *v,
            Self::Vec2(v) => [v[0], v[1], 0.0],
            Self::Float(v) => [*v, *v, *v],
            _ => [0.0; 3],
        }
    }

    pub fn as_vec2(&self) -> [f32; 2] {
        match self {
            Self::Vec4(v) => [v[0], v[1]],
            Self::Vec3(v) => [v[0], v[1]],
            Self::Vec2(v) => *v,
            Self::Float(v) => [*v, *v],
            _ => [0.0; 2],
        }
    }

    pub fn as_mat4(&self) -> [f32; 16] {
        match self {
            Self::Mat4(m) => *m,
            _ => MAT4_IDENTITY,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Float(f) => *f != 0.0,
            Self::Int(i) => *i != 0,
            _ => false,
        }
    }
}

const MAT4_IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

// ═══════════════════════════════════════════════════════════════════════════
// no_std float helpers (avoid libm dependency)
// ═══════════════════════════════════════════════════════════════════════════

fn f32_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x;
    for _ in 0..15 {
        guess = 0.5 * (guess + x / guess);
    }
    guess
}

fn f32_abs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

fn f32_floor(x: f32) -> f32 {
    let i = x as i32;
    if x < 0.0 && x != i as f32 {
        (i - 1) as f32
    } else {
        i as f32
    }
}

fn f32_fract(x: f32) -> f32 {
    x - f32_floor(x)
}

fn f32_sin(x: f32) -> f32 {
    // Normalize to [-PI, PI]
    const PI: f32 = 3.14159265;
    const TWO_PI: f32 = 6.2831853;
    let mut a = x;
    a = a - f32_floor(a / TWO_PI) * TWO_PI;
    if a > PI {
        a -= TWO_PI;
    }
    if a < -PI {
        a += TWO_PI;
    }
    // Bhaskara approximation
    let abs_a = f32_abs(a);
    let sign = if a < 0.0 { -1.0 } else { 1.0 };
    let result = 16.0 * abs_a * (PI - abs_a) / (5.0 * PI * PI - 4.0 * abs_a * (PI - abs_a));
    sign * result
}

fn f32_cos(x: f32) -> f32 {
    const HALF_PI: f32 = 1.5707963;
    f32_sin(x + HALF_PI)
}

fn f32_pow(base: f32, exp: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    // exp2(exp * log2(base)) via iterative approach
    let ln_base = f32_ln(base);
    f32_exp(exp * ln_base)
}

fn f32_ln(x: f32) -> f32 {
    if x <= 0.0 {
        return -100.0;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127;
    let mantissa_bits = (bits & 0x007F_FFFF) | 0x3F80_0000;
    let m = f32::from_bits(mantissa_bits);
    // ln(x) = exponent * ln(2) + ln(m), m in [1, 2)
    const LN2: f32 = 0.6931472;
    let t = m - 1.0;
    let ln_m = t * (2.0 - t * 0.5 + t * t * 0.3333 - t * t * t * 0.25);
    exponent as f32 * LN2 + ln_m
}

fn f32_exp(x: f32) -> f32 {
    if x < -80.0 {
        return 0.0;
    }
    if x > 80.0 {
        return f32::MAX;
    }
    // exp(x) = 2^(x/ln2) using integer + fractional decomposition
    const LOG2E: f32 = 1.442695;
    let t = x * LOG2E;
    let i = f32_floor(t) as i32;
    let f = t - i as f32;
    // 2^f approximation for f in [0, 1)
    let pow2_f = 1.0 + f * (0.6931472 + f * (0.2402265 + f * 0.0555041));
    // 2^i
    if i < -126 {
        return 0.0;
    }
    if i > 127 {
        return f32::MAX;
    }
    let pow2_i = f32::from_bits(((i + 127) as u32) << 23);
    pow2_i * pow2_f
}

fn f32_clamp(val: f32, lo: f32, hi: f32) -> f32 {
    if val < lo {
        lo
    } else if val > hi {
        hi
    } else {
        val
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Texture buffer for software sampling
// ═══════════════════════════════════════════════════════════════════════════

pub struct TextureBuffer {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub bpp: u32,
}

impl TextureBuffer {
    pub fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let u = f32_fract(u);
        let v = f32_fract(v);
        let x = (u * (self.width as f32 - 1.0)) as u32;
        let y = (v * (self.height as f32 - 1.0)) as u32;
        let idx = ((y * self.width + x) * self.bpp) as usize;
        if idx + 3 < self.data.len() {
            [
                self.data[idx] as f32 / 255.0,
                self.data[idx + 1] as f32 / 255.0,
                self.data[idx + 2] as f32 / 255.0,
                if self.bpp >= 4 {
                    self.data[idx + 3] as f32 / 255.0
                } else {
                    1.0
                },
            ]
        } else {
            [0.0, 0.0, 0.0, 1.0]
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Shader Interpreter — software execution path
// ═══════════════════════════════════════════════════════════════════════════

pub struct ShaderInterpreter {
    registers: Vec<ShaderValue>,
}

pub struct ShaderInputs<'a> {
    pub vertex_data: &'a [f32],
    pub uniforms: &'a [f32],
    pub textures: &'a [TextureBuffer],
}

impl ShaderInterpreter {
    pub fn new(max_registers: usize) -> Self {
        let mut registers = Vec::with_capacity(max_registers);
        for _ in 0..max_registers {
            registers.push(ShaderValue::Float(0.0));
        }
        Self { registers }
    }

    fn get(&self, id: u32) -> &ShaderValue {
        let idx = id as usize;
        if idx < self.registers.len() {
            &self.registers[idx]
        } else {
            &self.registers[0]
        }
    }

    fn set(&mut self, id: u32, val: ShaderValue) {
        let idx = id as usize;
        if idx < self.registers.len() {
            self.registers[idx] = val;
        }
    }

    pub fn reset(&mut self) {
        for reg in self.registers.iter_mut() {
            *reg = ShaderValue::Float(0.0);
        }
    }

    /// Execute a vertex shader on a single vertex.
    /// `vertex_data` is the vertex attributes (floats) for one vertex.
    /// Returns the transformed output values (typically position + varyings).
    pub fn interpret_vertex_shader(
        &mut self,
        module: &ShaderModule,
        inputs: &ShaderInputs,
    ) -> Vec<f32> {
        self.reset();
        self.load_inputs(module, inputs.vertex_data);
        self.load_uniforms(module, inputs.uniforms);
        self.execute_instructions(&module.instructions, inputs);
        self.collect_outputs(module)
    }

    /// Execute a fragment shader for a single pixel.
    /// `interpolated` are the interpolated varyings from vertex shader.
    /// Returns RGBA color.
    pub fn interpret_fragment_shader(
        &mut self,
        module: &ShaderModule,
        inputs: &ShaderInputs,
    ) -> [f32; 4] {
        self.reset();
        self.load_inputs(module, inputs.vertex_data);
        self.load_uniforms(module, inputs.uniforms);
        self.execute_instructions(&module.instructions, inputs);

        if let Some(output) = module.outputs.first() {
            self.get(output.location).as_vec4()
        } else {
            [1.0, 0.0, 1.0, 1.0]
        }
    }

    fn load_inputs(&mut self, module: &ShaderModule, vertex_data: &[f32]) {
        let mut offset = 0usize;
        for input in &module.inputs {
            let count = input.var_type.component_count();
            if offset + count <= vertex_data.len() {
                let val = match input.var_type {
                    VarType::Float => ShaderValue::Float(vertex_data[offset]),
                    VarType::Vec2 => {
                        ShaderValue::Vec2([vertex_data[offset], vertex_data[offset + 1]])
                    }
                    VarType::Vec3 => ShaderValue::Vec3([
                        vertex_data[offset],
                        vertex_data[offset + 1],
                        vertex_data[offset + 2],
                    ]),
                    VarType::Vec4 => ShaderValue::Vec4([
                        vertex_data[offset],
                        vertex_data[offset + 1],
                        vertex_data[offset + 2],
                        vertex_data[offset + 3],
                    ]),
                    _ => ShaderValue::Float(0.0),
                };
                self.set(input.location, val);
            }
            offset += count;
        }
    }

    fn load_uniforms(&mut self, module: &ShaderModule, uniforms: &[f32]) {
        let mut offset = 0usize;
        for uniform in &module.uniforms {
            let count = uniform.var_type.component_count();
            if offset + count <= uniforms.len() {
                let val = match uniform.var_type {
                    VarType::Float => ShaderValue::Float(uniforms[offset]),
                    VarType::Vec2 => ShaderValue::Vec2([uniforms[offset], uniforms[offset + 1]]),
                    VarType::Vec3 => ShaderValue::Vec3([
                        uniforms[offset],
                        uniforms[offset + 1],
                        uniforms[offset + 2],
                    ]),
                    VarType::Vec4 => ShaderValue::Vec4([
                        uniforms[offset],
                        uniforms[offset + 1],
                        uniforms[offset + 2],
                        uniforms[offset + 3],
                    ]),
                    VarType::Mat4 => {
                        let mut m = [0.0f32; 16];
                        for i in 0..16 {
                            m[i] = uniforms[offset + i];
                        }
                        ShaderValue::Mat4(m)
                    }
                    VarType::Mat3 => {
                        let mut m = [0.0f32; 9];
                        for i in 0..9 {
                            m[i] = uniforms[offset + i];
                        }
                        ShaderValue::Mat3(m)
                    }
                    VarType::Int => ShaderValue::Int(uniforms[offset] as i32),
                    VarType::Bool => ShaderValue::Bool(uniforms[offset] != 0.0),
                    _ => ShaderValue::Float(0.0),
                };
                self.set(uniform.location, val);
            }
            offset += count;
        }
    }

    fn collect_outputs(&self, module: &ShaderModule) -> Vec<f32> {
        let mut out = Vec::new();
        for output in &module.outputs {
            let val = self.get(output.location);
            match output.var_type {
                VarType::Float => out.push(val.as_float()),
                VarType::Vec2 => {
                    let v = val.as_vec2();
                    out.extend_from_slice(&v);
                }
                VarType::Vec3 => {
                    let v = val.as_vec3();
                    out.extend_from_slice(&v);
                }
                VarType::Vec4 => {
                    let v = val.as_vec4();
                    out.extend_from_slice(&v);
                }
                VarType::Mat4 => {
                    let m = val.as_mat4();
                    out.extend_from_slice(&m);
                }
                _ => out.push(val.as_float()),
            }
        }
        out
    }

    fn execute_instructions(&mut self, instructions: &[ShaderIR], inputs: &ShaderInputs) -> bool {
        for instr in instructions {
            let should_break = self.execute_one(instr, inputs);
            if should_break {
                return true;
            }
        }
        false
    }

    fn execute_one(&mut self, instr: &ShaderIR, inputs: &ShaderInputs) -> bool {
        match instr {
            ShaderIR::DeclareVar { id, var_type } => {
                let val = match var_type {
                    VarType::Float | VarType::Int | VarType::Bool => ShaderValue::Float(0.0),
                    VarType::Vec2 => ShaderValue::Vec2([0.0; 2]),
                    VarType::Vec3 => ShaderValue::Vec3([0.0; 3]),
                    VarType::Vec4 => ShaderValue::Vec4([0.0; 4]),
                    VarType::Mat3 => ShaderValue::Mat3([0.0; 9]),
                    VarType::Mat4 => ShaderValue::Mat4(MAT4_IDENTITY),
                    VarType::Sampler2D => ShaderValue::Sampler(0),
                    _ => ShaderValue::Float(0.0),
                };
                self.set(*id, val);
            }
            ShaderIR::LoadInput { dst, location } => {
                let val = self.get(*location).clone();
                self.set(*dst, val);
            }
            ShaderIR::StoreOutput { src, location } => {
                let val = self.get(*src).clone();
                self.set(*location, val);
            }
            ShaderIR::LoadUniform {
                dst,
                binding: _,
                offset,
            } => {
                let val = self.get(*offset).clone();
                self.set(*dst, val);
            }
            ShaderIR::ConstFloat { dst, value } => {
                self.set(*dst, ShaderValue::Float(f32::from_bits(*value)));
            }
            ShaderIR::ConstVec4 { dst, x, y, z, w } => {
                self.set(
                    *dst,
                    ShaderValue::Vec4([
                        f32::from_bits(*x),
                        f32::from_bits(*y),
                        f32::from_bits(*z),
                        f32::from_bits(*w),
                    ]),
                );
            }
            ShaderIR::ConstMat4 { dst, values } => {
                let mut m = [0.0f32; 16];
                for i in 0..16 {
                    m[i] = f32::from_bits(values[i]);
                }
                self.set(*dst, ShaderValue::Mat4(m));
            }
            ShaderIR::ConstInt { dst, value } => {
                self.set(*dst, ShaderValue::Int(*value));
            }
            ShaderIR::Add { dst, a, b } => {
                let va = self.get(*a).clone();
                let vb = self.get(*b).clone();
                self.set(*dst, vec_binop(&va, &vb, |x, y| x + y));
            }
            ShaderIR::Sub { dst, a, b } => {
                let va = self.get(*a).clone();
                let vb = self.get(*b).clone();
                self.set(*dst, vec_binop(&va, &vb, |x, y| x - y));
            }
            ShaderIR::Mul { dst, a, b } => {
                let va = self.get(*a).clone();
                let vb = self.get(*b).clone();
                self.set(*dst, vec_binop(&va, &vb, |x, y| x * y));
            }
            ShaderIR::Div { dst, a, b } => {
                let va = self.get(*a).clone();
                let vb = self.get(*b).clone();
                self.set(
                    *dst,
                    vec_binop(&va, &vb, |x, y| if y != 0.0 { x / y } else { 0.0 }),
                );
            }
            ShaderIR::Negate { dst, src } => {
                let v = self.get(*src).clone();
                self.set(*dst, vec_unop(&v, |x| -x));
            }
            ShaderIR::Dot { dst, a, b } => {
                let va = self.get(*a).as_vec4();
                let vb = self.get(*b).as_vec4();
                let d = va[0] * vb[0] + va[1] * vb[1] + va[2] * vb[2] + va[3] * vb[3];
                self.set(*dst, ShaderValue::Float(d));
            }
            ShaderIR::Cross { dst, a, b } => {
                let va = self.get(*a).as_vec3();
                let vb = self.get(*b).as_vec3();
                self.set(
                    *dst,
                    ShaderValue::Vec3([
                        va[1] * vb[2] - va[2] * vb[1],
                        va[2] * vb[0] - va[0] * vb[2],
                        va[0] * vb[1] - va[1] * vb[0],
                    ]),
                );
            }
            ShaderIR::Normalize { dst, src } => {
                let v = self.get(*src).as_vec4();
                let len = f32_sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2] + v[3] * v[3]);
                if len > 0.0 {
                    self.set(
                        *dst,
                        ShaderValue::Vec4([v[0] / len, v[1] / len, v[2] / len, v[3] / len]),
                    );
                } else {
                    self.set(*dst, ShaderValue::Vec4([0.0; 4]));
                }
            }
            ShaderIR::MatMul { dst, mat, vec } => {
                let m = self.get(*mat).as_mat4();
                let v = self.get(*vec).as_vec4();
                let mut result = [0.0f32; 4];
                for row in 0..4 {
                    for col in 0..4 {
                        result[row] += m[row * 4 + col] * v[col];
                    }
                }
                self.set(*dst, ShaderValue::Vec4(result));
            }
            ShaderIR::Transpose { dst, src } => {
                let m = self.get(*src).as_mat4();
                let mut t = [0.0f32; 16];
                for row in 0..4 {
                    for col in 0..4 {
                        t[col * 4 + row] = m[row * 4 + col];
                    }
                }
                self.set(*dst, ShaderValue::Mat4(t));
            }
            ShaderIR::SampleTexture {
                dst,
                texture: _,
                sampler: _,
                coord,
            } => {
                let uv = self.get(*coord).as_vec2();
                let color = if !inputs.textures.is_empty() {
                    inputs.textures[0].sample(uv[0], uv[1])
                } else {
                    [1.0, 0.0, 1.0, 1.0]
                };
                self.set(*dst, ShaderValue::Vec4(color));
            }
            ShaderIR::Clamp { dst, val, min, max } => {
                let v = self.get(*val).clone();
                let lo = self.get(*min).clone();
                let hi = self.get(*max).clone();
                let v4 = v.as_vec4();
                let lo4 = lo.as_vec4();
                let hi4 = hi.as_vec4();
                self.set(
                    *dst,
                    ShaderValue::Vec4([
                        f32_clamp(v4[0], lo4[0], hi4[0]),
                        f32_clamp(v4[1], lo4[1], hi4[1]),
                        f32_clamp(v4[2], lo4[2], hi4[2]),
                        f32_clamp(v4[3], lo4[3], hi4[3]),
                    ]),
                );
            }
            ShaderIR::Mix { dst, a, b, t } => {
                let va = self.get(*a).as_vec4();
                let vb = self.get(*b).as_vec4();
                let vt = self.get(*t).as_vec4();
                self.set(
                    *dst,
                    ShaderValue::Vec4([
                        va[0] + (vb[0] - va[0]) * vt[0],
                        va[1] + (vb[1] - va[1]) * vt[1],
                        va[2] + (vb[2] - va[2]) * vt[2],
                        va[3] + (vb[3] - va[3]) * vt[3],
                    ]),
                );
            }
            ShaderIR::Pow { dst, base, exp } => {
                let vb = self.get(*base).clone();
                let ve = self.get(*exp).clone();
                self.set(*dst, vec_binop(&vb, &ve, f32_pow));
            }
            ShaderIR::Sqrt { dst, src } => {
                let v = self.get(*src).clone();
                self.set(*dst, vec_unop(&v, f32_sqrt));
            }
            ShaderIR::Sin { dst, src } => {
                let v = self.get(*src).clone();
                self.set(*dst, vec_unop(&v, f32_sin));
            }
            ShaderIR::Cos { dst, src } => {
                let v = self.get(*src).clone();
                self.set(*dst, vec_unop(&v, f32_cos));
            }
            ShaderIR::Abs { dst, src } => {
                let v = self.get(*src).clone();
                self.set(*dst, vec_unop(&v, f32_abs));
            }
            ShaderIR::Min { dst, a, b } => {
                let va = self.get(*a).clone();
                let vb = self.get(*b).clone();
                self.set(*dst, vec_binop(&va, &vb, |x, y| if x < y { x } else { y }));
            }
            ShaderIR::Max { dst, a, b } => {
                let va = self.get(*a).clone();
                let vb = self.get(*b).clone();
                self.set(*dst, vec_binop(&va, &vb, |x, y| if x > y { x } else { y }));
            }
            ShaderIR::Floor { dst, src } => {
                let v = self.get(*src).clone();
                self.set(*dst, vec_unop(&v, f32_floor));
            }
            ShaderIR::Fract { dst, src } => {
                let v = self.get(*src).clone();
                self.set(*dst, vec_unop(&v, f32_fract));
            }
            ShaderIR::Step { dst, edge, x } => {
                let ve = self.get(*edge).as_vec4();
                let vx = self.get(*x).as_vec4();
                self.set(
                    *dst,
                    ShaderValue::Vec4([
                        if vx[0] >= ve[0] { 1.0 } else { 0.0 },
                        if vx[1] >= ve[1] { 1.0 } else { 0.0 },
                        if vx[2] >= ve[2] { 1.0 } else { 0.0 },
                        if vx[3] >= ve[3] { 1.0 } else { 0.0 },
                    ]),
                );
            }
            ShaderIR::SmoothStep {
                dst,
                edge0,
                edge1,
                x,
            } => {
                let e0 = self.get(*edge0).as_vec4();
                let e1 = self.get(*edge1).as_vec4();
                let vx = self.get(*x).as_vec4();
                let mut result = [0.0f32; 4];
                for i in 0..4 {
                    let range = e1[i] - e0[i];
                    if range == 0.0 {
                        result[i] = 0.0;
                    } else {
                        let t = f32_clamp((vx[i] - e0[i]) / range, 0.0, 1.0);
                        result[i] = t * t * (3.0 - 2.0 * t);
                    }
                }
                self.set(*dst, ShaderValue::Vec4(result));
            }
            ShaderIR::LessThan { dst, a, b } => {
                let va = self.get(*a).as_float();
                let vb = self.get(*b).as_float();
                self.set(*dst, ShaderValue::Bool(va < vb));
            }
            ShaderIR::GreaterThan { dst, a, b } => {
                let va = self.get(*a).as_float();
                let vb = self.get(*b).as_float();
                self.set(*dst, ShaderValue::Bool(va > vb));
            }
            ShaderIR::Equal { dst, a, b } => {
                let va = self.get(*a).as_float();
                let vb = self.get(*b).as_float();
                self.set(*dst, ShaderValue::Bool(f32_abs(va - vb) < 1.0e-6));
            }
            ShaderIR::Swizzle {
                dst,
                src,
                components,
                count,
            } => {
                let v = self.get(*src).as_vec4();
                let c = *count as usize;
                match c {
                    1 => self.set(*dst, ShaderValue::Float(v[components[0] as usize])),
                    2 => self.set(
                        *dst,
                        ShaderValue::Vec2([v[components[0] as usize], v[components[1] as usize]]),
                    ),
                    3 => self.set(
                        *dst,
                        ShaderValue::Vec3([
                            v[components[0] as usize],
                            v[components[1] as usize],
                            v[components[2] as usize],
                        ]),
                    ),
                    _ => self.set(
                        *dst,
                        ShaderValue::Vec4([
                            v[components[0] as usize],
                            v[components[1] as usize],
                            v[components[2] as usize],
                            v[components[3] as usize],
                        ]),
                    ),
                }
            }
            ShaderIR::Construct {
                dst,
                var_type,
                components,
            } => match var_type {
                VarType::Vec2 => {
                    let x = if components.len() > 0 {
                        self.get(components[0]).as_float()
                    } else {
                        0.0
                    };
                    let y = if components.len() > 1 {
                        self.get(components[1]).as_float()
                    } else {
                        0.0
                    };
                    self.set(*dst, ShaderValue::Vec2([x, y]));
                }
                VarType::Vec3 => {
                    let x = if components.len() > 0 {
                        self.get(components[0]).as_float()
                    } else {
                        0.0
                    };
                    let y = if components.len() > 1 {
                        self.get(components[1]).as_float()
                    } else {
                        0.0
                    };
                    let z = if components.len() > 2 {
                        self.get(components[2]).as_float()
                    } else {
                        0.0
                    };
                    self.set(*dst, ShaderValue::Vec3([x, y, z]));
                }
                VarType::Vec4 => {
                    let x = if components.len() > 0 {
                        self.get(components[0]).as_float()
                    } else {
                        0.0
                    };
                    let y = if components.len() > 1 {
                        self.get(components[1]).as_float()
                    } else {
                        0.0
                    };
                    let z = if components.len() > 2 {
                        self.get(components[2]).as_float()
                    } else {
                        0.0
                    };
                    let w = if components.len() > 3 {
                        self.get(components[3]).as_float()
                    } else {
                        0.0
                    };
                    self.set(*dst, ShaderValue::Vec4([x, y, z, w]));
                }
                _ => {
                    if !components.is_empty() {
                        let val = self.get(components[0]).clone();
                        self.set(*dst, val);
                    }
                }
            },
            ShaderIR::If {
                condition,
                then_block,
                else_block,
            } => {
                let cond = self.get(*condition).as_bool();
                if cond {
                    if self.execute_instructions(then_block, inputs) {
                        return true;
                    }
                } else {
                    if self.execute_instructions(else_block, inputs) {
                        return true;
                    }
                }
            }
            ShaderIR::Loop {
                body,
                max_iterations,
            } => {
                for _ in 0..*max_iterations {
                    if self.execute_instructions(body, inputs) {
                        break;
                    }
                }
            }
            ShaderIR::Break => return true,
            ShaderIR::Return { .. } => return true,
        }
        false
    }
}

// ── Vector helper operations ─────────────────────────────────────────────

fn vec_binop(a: &ShaderValue, b: &ShaderValue, op: fn(f32, f32) -> f32) -> ShaderValue {
    match (a, b) {
        (ShaderValue::Float(x), ShaderValue::Float(y)) => ShaderValue::Float(op(*x, *y)),
        (ShaderValue::Vec2(x), ShaderValue::Vec2(y)) => {
            ShaderValue::Vec2([op(x[0], y[0]), op(x[1], y[1])])
        }
        (ShaderValue::Vec3(x), ShaderValue::Vec3(y)) => {
            ShaderValue::Vec3([op(x[0], y[0]), op(x[1], y[1]), op(x[2], y[2])])
        }
        (ShaderValue::Vec4(x), ShaderValue::Vec4(y)) => ShaderValue::Vec4([
            op(x[0], y[0]),
            op(x[1], y[1]),
            op(x[2], y[2]),
            op(x[3], y[3]),
        ]),
        // Scalar-vector broadcast
        (ShaderValue::Float(s), ShaderValue::Vec4(v)) => {
            ShaderValue::Vec4([op(*s, v[0]), op(*s, v[1]), op(*s, v[2]), op(*s, v[3])])
        }
        (ShaderValue::Vec4(v), ShaderValue::Float(s)) => {
            ShaderValue::Vec4([op(v[0], *s), op(v[1], *s), op(v[2], *s), op(v[3], *s)])
        }
        (ShaderValue::Float(s), ShaderValue::Vec3(v)) => {
            ShaderValue::Vec3([op(*s, v[0]), op(*s, v[1]), op(*s, v[2])])
        }
        (ShaderValue::Vec3(v), ShaderValue::Float(s)) => {
            ShaderValue::Vec3([op(v[0], *s), op(v[1], *s), op(v[2], *s)])
        }
        _ => {
            let va = a.as_vec4();
            let vb = b.as_vec4();
            ShaderValue::Vec4([
                op(va[0], vb[0]),
                op(va[1], vb[1]),
                op(va[2], vb[2]),
                op(va[3], vb[3]),
            ])
        }
    }
}

fn vec_unop(a: &ShaderValue, op: fn(f32) -> f32) -> ShaderValue {
    match a {
        ShaderValue::Float(x) => ShaderValue::Float(op(*x)),
        ShaderValue::Vec2(v) => ShaderValue::Vec2([op(v[0]), op(v[1])]),
        ShaderValue::Vec3(v) => ShaderValue::Vec3([op(v[0]), op(v[1]), op(v[2])]),
        ShaderValue::Vec4(v) => ShaderValue::Vec4([op(v[0]), op(v[1]), op(v[2]), op(v[3])]),
        _ => {
            let v = a.as_vec4();
            ShaderValue::Vec4([op(v[0]), op(v[1]), op(v[2]), op(v[3])])
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SPIR-V Parser — basic binary parsing for hardware interop
// ═══════════════════════════════════════════════════════════════════════════

const SPIRV_MAGIC: u32 = 0x0723_0203;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpirVExecutionModel {
    Vertex,
    Fragment,
    GLCompute,
    Unknown(u32),
}

#[derive(Debug, Clone)]
pub struct SpirVEntryPoint {
    pub execution_model: SpirVExecutionModel,
    pub name: String,
    pub id: u32,
}

#[derive(Debug, Clone)]
pub struct SpirVHeader {
    pub version_major: u8,
    pub version_minor: u8,
    pub generator: u32,
    pub bound: u32,
}

#[derive(Debug, Clone)]
pub struct SpirVModule {
    pub header: SpirVHeader,
    pub entry_points: Vec<SpirVEntryPoint>,
    pub word_count: usize,
}

impl SpirVModule {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 20 {
            return None;
        }

        let words = spirv_words(data);
        if words.is_empty() {
            return None;
        }

        let magic = words[0];
        if magic != SPIRV_MAGIC {
            return None;
        }

        let version = words[1];
        let version_major = ((version >> 16) & 0xFF) as u8;
        let version_minor = ((version >> 8) & 0xFF) as u8;
        let generator = words[2];
        let bound = words[3];

        let header = SpirVHeader {
            version_major,
            version_minor,
            generator,
            bound,
        };

        let mut entry_points = Vec::new();
        let mut i = 5;
        while i < words.len() {
            let instr = words[i];
            let word_count = (instr >> 16) as usize;
            let opcode = instr & 0xFFFF;

            if word_count == 0 {
                break;
            }

            // OpEntryPoint = 15
            if opcode == 15 && word_count >= 4 {
                let exec_model = match words[i + 1] {
                    0 => SpirVExecutionModel::Vertex,
                    4 => SpirVExecutionModel::Fragment,
                    5 => SpirVExecutionModel::GLCompute,
                    x => SpirVExecutionModel::Unknown(x),
                };
                let entry_id = words[i + 2];
                let name = spirv_read_string(&words[i + 3..]);
                entry_points.push(SpirVEntryPoint {
                    execution_model: exec_model,
                    name,
                    id: entry_id,
                });
            }

            i += word_count;
        }

        Some(Self {
            header,
            entry_points,
            word_count: words.len(),
        })
    }

    pub fn to_stage(&self) -> Option<ShaderStage> {
        self.entry_points
            .first()
            .map(|ep| match ep.execution_model {
                SpirVExecutionModel::Vertex => ShaderStage::Vertex,
                SpirVExecutionModel::Fragment => ShaderStage::Fragment,
                SpirVExecutionModel::GLCompute => ShaderStage::Compute,
                SpirVExecutionModel::Unknown(_) => ShaderStage::Compute,
            })
    }
}

fn spirv_words(data: &[u8]) -> Vec<u32> {
    let count = data.len() / 4;
    let mut words = Vec::with_capacity(count);
    for i in 0..count {
        let offset = i * 4;
        let word = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        words.push(word);
    }
    words
}

fn spirv_read_string(words: &[u32]) -> String {
    let mut bytes = Vec::new();
    for &word in words {
        let b = word.to_le_bytes();
        for &byte in &b {
            if byte == 0 {
                return String::from_utf8_lossy(&bytes).into_owned();
            }
            bytes.push(byte);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

// ═══════════════════════════════════════════════════════════════════════════
// Shader module builder helpers
// ═══════════════════════════════════════════════════════════════════════════

impl ShaderModule {
    pub fn new_vertex(entry_point: &str) -> Self {
        Self {
            stage: ShaderStage::Vertex,
            instructions: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            uniforms: Vec::new(),
            entry_point: String::from(entry_point),
        }
    }

    pub fn new_fragment(entry_point: &str) -> Self {
        Self {
            stage: ShaderStage::Fragment,
            instructions: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            uniforms: Vec::new(),
            entry_point: String::from(entry_point),
        }
    }

    pub fn add_input(&mut self, name: &str, var_type: VarType, location: u32) {
        self.inputs.push(ShaderVariable {
            name: String::from(name),
            var_type,
            location,
        });
    }

    pub fn add_output(&mut self, name: &str, var_type: VarType, location: u32) {
        self.outputs.push(ShaderVariable {
            name: String::from(name),
            var_type,
            location,
        });
    }

    pub fn add_uniform(&mut self, name: &str, var_type: VarType, location: u32) {
        self.uniforms.push(ShaderVariable {
            name: String::from(name),
            var_type,
            location,
        });
    }

    pub fn emit(&mut self, instr: ShaderIR) {
        self.instructions.push(instr);
    }
}
