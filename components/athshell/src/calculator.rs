//! Full-featured calculator application for the AthenaOS desktop shell.
//!
//! Modes: Standard, Scientific, Programmer, Date calculation, Unit
//! converter, Currency converter, and Graph mode.  Includes a full
//! expression parser with operator precedence, calculation history,
//! memory registers, and keyboard input.

#![allow(unused)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ── no_std math helpers ──────────────────────────────────────────────────

mod math {
    pub fn sqrt(x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        let mut g = x / 2.0;
        for _ in 0..20 {
            g = (g + x / g) * 0.5;
        }
        g
    }

    pub fn pow(base: f64, exp: f64) -> f64 {
        if base == 0.0 {
            return 0.0;
        }
        if exp == 0.0 {
            return 1.0;
        }
        let neg = base < 0.0 && (exp as i64) % 2 != 0;
        let b = fabs(base);
        let result = self::exp(exp * ln(b));
        if neg {
            -result
        } else {
            result
        }
    }

    pub fn exp(x: f64) -> f64 {
        let mut sum: f64 = 1.0;
        let mut term: f64 = 1.0;
        for i in 1..30 {
            term *= x / i as f64;
            sum += term;
        }
        sum
    }

    pub fn ln(x: f64) -> f64 {
        if x <= 0.0 {
            return -1e30;
        }
        let mut val = x;
        let mut result: f64 = 0.0;
        while val > 2.0 {
            val /= 2.718281828459045;
            result += 1.0;
        }
        while val < 0.5 {
            val *= 2.718281828459045;
            result -= 1.0;
        }
        let y = (val - 1.0) / (val + 1.0);
        let y2 = y * y;
        let mut term = y;
        for i in 0..15 {
            result += 2.0 * term / (2 * i + 1) as f64;
            term *= y2;
        }
        result
    }

    pub fn log10(x: f64) -> f64 {
        ln(x) / 2.302585092994046
    }

    pub fn sin(x: f64) -> f64 {
        let pi = core::f64::consts::PI;
        let mut x = x % (2.0 * pi);
        if x < -pi {
            x += 2.0 * pi;
        }
        if x > pi {
            x -= 2.0 * pi;
        }
        let x2 = x * x;
        x * (1.0 - x2 / 6.0 + x2 * x2 / 120.0 - x2 * x2 * x2 / 5040.0
            + x2 * x2 * x2 * x2 / 362880.0
            - x2 * x2 * x2 * x2 * x2 / 39916800.0)
    }

    pub fn cos(x: f64) -> f64 {
        let pi = core::f64::consts::PI;
        let mut x = x % (2.0 * pi);
        if x < -pi {
            x += 2.0 * pi;
        }
        if x > pi {
            x -= 2.0 * pi;
        }
        let x2 = x * x;
        1.0 - x2 / 2.0 + x2 * x2 / 24.0 - x2 * x2 * x2 / 720.0 + x2 * x2 * x2 * x2 / 40320.0
            - x2 * x2 * x2 * x2 * x2 / 3628800.0
    }

    pub fn tan(x: f64) -> f64 {
        let c = cos(x);
        if c == 0.0 {
            return 1e30;
        }
        sin(x) / c
    }

    pub fn asin(x: f64) -> f64 {
        let x2 = x * x;
        let mut result = x;
        let mut term = x;
        for n in 1..20u64 {
            term *= x2 * (2 * n - 1) as f64 * (2 * n - 1) as f64
                / ((2 * n) as f64 * (2 * n + 1) as f64);
            result += term;
        }
        result
    }

    pub fn acos(x: f64) -> f64 {
        core::f64::consts::PI / 2.0 - asin(x)
    }

    pub fn atan(x: f64) -> f64 {
        if fabs(x) > 1.0 {
            let sign = if x > 0.0 { 1.0 } else { -1.0 };
            return sign * core::f64::consts::PI / 2.0 - atan(1.0 / x);
        }
        let x2 = x * x;
        let mut result = x;
        let mut term = x;
        for n in 1..25i64 {
            term *= -x2;
            result += term / (2 * n + 1) as f64;
        }
        result
    }

    pub fn sinh(x: f64) -> f64 {
        (exp(x) - exp(-x)) / 2.0
    }
    pub fn cosh(x: f64) -> f64 {
        (exp(x) + exp(-x)) / 2.0
    }
    pub fn tanh(x: f64) -> f64 {
        let e2 = exp(2.0 * x);
        (e2 - 1.0) / (e2 + 1.0)
    }

    pub fn cbrt(x: f64) -> f64 {
        if x == 0.0 {
            return 0.0;
        }
        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let a = fabs(x);
        sign * pow(a, 1.0 / 3.0)
    }

    pub fn fabs(x: f64) -> f64 {
        if x < 0.0 {
            -x
        } else {
            x
        }
    }

    pub fn floor(x: f64) -> f64 {
        let i = x as i64;
        if (i as f64) > x {
            (i - 1) as f64
        } else {
            i as f64
        }
    }

    pub fn ceil(x: f64) -> f64 {
        let i = x as i64;
        if (i as f64) < x {
            (i + 1) as f64
        } else {
            i as f64
        }
    }

    pub fn round(x: f64) -> f64 {
        floor(x + 0.5)
    }
}

// ── Calculator modes ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalcMode {
    Standard,
    Scientific,
    Programmer,
    DateCalc,
    UnitConverter,
    CurrencyConverter,
    Graph,
}

// ── Angle unit ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngleUnit {
    Degrees,
    Radians,
    Gradians,
}

impl AngleUnit {
    pub fn to_radians(&self, value: f64) -> f64 {
        match self {
            AngleUnit::Radians => value,
            AngleUnit::Degrees => value * core::f64::consts::PI / 180.0,
            AngleUnit::Gradians => value * core::f64::consts::PI / 200.0,
        }
    }

    pub fn from_radians(&self, rad: f64) -> f64 {
        match self {
            AngleUnit::Radians => rad,
            AngleUnit::Degrees => rad * 180.0 / core::f64::consts::PI,
            AngleUnit::Gradians => rad * 200.0 / core::f64::consts::PI,
        }
    }
}

// ── Memory ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MemoryStore {
    pub slots: Vec<f64>,
    pub max_slots: usize,
}

impl MemoryStore {
    pub fn new(max: usize) -> Self {
        Self {
            slots: Vec::new(),
            max_slots: max,
        }
    }

    pub fn store(&mut self, value: f64) {
        if self.slots.len() >= self.max_slots {
            self.slots.remove(0);
        }
        self.slots.push(value);
    }

    pub fn recall(&self) -> f64 {
        self.slots.last().copied().unwrap_or(0.0)
    }

    pub fn add(&mut self, value: f64) {
        if let Some(last) = self.slots.last_mut() {
            *last += value;
        } else {
            self.store(value);
        }
    }

    pub fn subtract(&mut self, value: f64) {
        if let Some(last) = self.slots.last_mut() {
            *last -= value;
        } else {
            self.store(-value);
        }
    }

    pub fn clear(&mut self) {
        self.slots.clear();
    }

    pub fn clear_last(&mut self) {
        self.slots.pop();
    }

    pub fn list(&self) -> &[f64] {
        &self.slots
    }
}

// ── Calculation history ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub expression: String,
    pub result: f64,
    pub result_display: String,
    pub mode: CalcMode,
    pub timestamp: u64,
}

pub struct CalcHistory {
    pub entries: Vec<HistoryEntry>,
    pub max_entries: usize,
}

impl CalcHistory {
    pub fn new(max: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries: max,
        }
    }

    pub fn push(&mut self, expr: &str, result: f64, display: &str, mode: CalcMode, ts: u64) {
        self.entries.push(HistoryEntry {
            expression: String::from(expr),
            result,
            result_display: String::from(display),
            mode,
            timestamp: ts,
        });
        while self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
    }

    pub fn get(&self, idx: usize) -> Option<&HistoryEntry> {
        self.entries.get(idx)
    }

    pub fn last_result(&self) -> Option<f64> {
        self.entries.last().map(|e| e.result)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ── Expression parser tokens ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Number(f64),
    Plus,
    Minus,
    Multiply,
    Divide,
    Percent,
    Power,
    Modulo,
    LParen,
    RParen,
    Function(String),
    UnaryMinus,
    Factorial,
    Comma,
}

pub struct Tokenizer {
    input: Vec<char>,
    pos: usize,
}

impl Tokenizer {
    pub fn new(expr: &str) -> Self {
        Self {
            input: expr.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.input.get(self.pos).copied();
        self.pos += 1;
        c
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        while self.pos < self.input.len() {
            self.skip_whitespace();
            if self.pos >= self.input.len() {
                break;
            }
            let c = self.peek().unwrap();
            match c {
                '0'..='9' | '.' => {
                    let num = self.read_number();
                    tokens.push(Token::Number(num));
                }
                '+' => {
                    self.advance();
                    tokens.push(Token::Plus);
                }
                '-' => {
                    self.advance();
                    let is_unary = tokens.is_empty()
                        || matches!(
                            tokens.last(),
                            Some(Token::LParen)
                                | Some(Token::Plus)
                                | Some(Token::Minus)
                                | Some(Token::Multiply)
                                | Some(Token::Divide)
                                | Some(Token::Power)
                                | Some(Token::Modulo)
                                | Some(Token::Comma)
                        );
                    if is_unary {
                        tokens.push(Token::UnaryMinus);
                    } else {
                        tokens.push(Token::Minus);
                    }
                }
                '*' => {
                    self.advance();
                    tokens.push(Token::Multiply);
                }
                '/' => {
                    self.advance();
                    tokens.push(Token::Divide);
                }
                '%' => {
                    self.advance();
                    tokens.push(Token::Percent);
                }
                '^' => {
                    self.advance();
                    tokens.push(Token::Power);
                }
                '(' => {
                    self.advance();
                    tokens.push(Token::LParen);
                }
                ')' => {
                    self.advance();
                    tokens.push(Token::RParen);
                }
                '!' => {
                    self.advance();
                    tokens.push(Token::Factorial);
                }
                ',' => {
                    self.advance();
                    tokens.push(Token::Comma);
                }
                'a'..='z' | 'A'..='Z' => {
                    let name = self.read_identifier();
                    match name.as_str() {
                        "pi" => tokens.push(Token::Number(core::f64::consts::PI)),
                        "e" => tokens.push(Token::Number(core::f64::consts::E)),
                        "phi" => tokens.push(Token::Number(1.618033988749895)),
                        "mod" => tokens.push(Token::Modulo),
                        _ => tokens.push(Token::Function(name)),
                    }
                }
                _ => {
                    self.advance();
                }
            }
        }
        tokens
    }

    fn read_number(&mut self) -> f64 {
        let start = self.pos;
        let mut has_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else if c == '.' && !has_dot {
                has_dot = true;
                self.advance();
            } else {
                break;
            }
        }
        let s: String = self.input[start..self.pos].iter().collect();
        parse_f64(&s)
    }

    fn read_identifier(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }
        self.input[start..self.pos].iter().collect()
    }
}

fn parse_f64(s: &str) -> f64 {
    let mut result: f64 = 0.0;
    let mut decimal_place: f64 = 0.0;
    let mut in_decimal = false;
    for c in s.chars() {
        if c == '.' {
            in_decimal = true;
            decimal_place = 0.1;
        } else if let Some(d) = c.to_digit(10) {
            if in_decimal {
                result += d as f64 * decimal_place;
                decimal_place *= 0.1;
            } else {
                result = result * 10.0 + d as f64;
            }
        }
    }
    result
}

// ── Expression evaluator (recursive descent) ─────────────────────────────

pub struct ExprEvaluator {
    tokens: Vec<Token>,
    pos: usize,
    angle_unit: AngleUnit,
}

impl ExprEvaluator {
    pub fn new(tokens: Vec<Token>, angle_unit: AngleUnit) -> Self {
        Self {
            tokens,
            pos: 0,
            angle_unit,
        }
    }

    pub fn evaluate(&mut self) -> Result<f64, &'static str> {
        let result = self.parse_expression()?;
        if self.pos < self.tokens.len() {
            return Err("unexpected token");
        }
        Ok(result)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos);
        self.pos += 1;
        t
    }

    fn parse_expression(&mut self) -> Result<f64, &'static str> {
        let mut left = self.parse_term()?;
        loop {
            match self.peek() {
                Some(Token::Plus) => {
                    self.advance();
                    left += self.parse_term()?;
                }
                Some(Token::Minus) => {
                    self.advance();
                    left -= self.parse_term()?;
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> Result<f64, &'static str> {
        let mut left = self.parse_power()?;
        loop {
            match self.peek() {
                Some(Token::Multiply) => {
                    self.advance();
                    left *= self.parse_power()?;
                }
                Some(Token::Divide) => {
                    self.advance();
                    let right = self.parse_power()?;
                    if right == 0.0 {
                        return Err("division by zero");
                    }
                    left /= right;
                }
                Some(Token::Percent) => {
                    self.advance();
                    left /= 100.0;
                }
                Some(Token::Modulo) => {
                    self.advance();
                    let right = self.parse_power()?;
                    if right == 0.0 {
                        return Err("modulo by zero");
                    }
                    left %= right;
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_power(&mut self) -> Result<f64, &'static str> {
        let base = self.parse_unary()?;
        if let Some(Token::Power) = self.peek() {
            self.advance();
            let exp = self.parse_unary()?;
            Ok(math::pow(base, exp))
        } else {
            Ok(base)
        }
    }

    fn parse_unary(&mut self) -> Result<f64, &'static str> {
        if let Some(Token::UnaryMinus) = self.peek() {
            self.advance();
            let val = self.parse_postfix()?;
            return Ok(-val);
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<f64, &'static str> {
        let mut val = self.parse_primary()?;
        while let Some(Token::Factorial) = self.peek() {
            self.advance();
            val = factorial(val as u64) as f64;
        }
        Ok(val)
    }

    fn parse_primary(&mut self) -> Result<f64, &'static str> {
        match self.peek().cloned() {
            Some(Token::Number(n)) => {
                self.advance();
                Ok(n)
            }
            Some(Token::LParen) => {
                self.advance();
                let val = self.parse_expression()?;
                if let Some(Token::RParen) = self.peek() {
                    self.advance();
                } else {
                    return Err("missing closing parenthesis");
                }
                Ok(val)
            }
            Some(Token::Function(ref name)) => {
                let name = name.clone();
                self.advance();
                if let Some(Token::LParen) = self.peek() {
                    self.advance();
                } else {
                    return Err("expected '(' after function name");
                }
                let mut args = vec![self.parse_expression()?];
                while let Some(Token::Comma) = self.peek() {
                    self.advance();
                    args.push(self.parse_expression()?);
                }
                if let Some(Token::RParen) = self.peek() {
                    self.advance();
                } else {
                    return Err("missing closing parenthesis in function call");
                }
                self.eval_function(&name, &args)
            }
            _ => Err("unexpected token in expression"),
        }
    }

    fn eval_function(&self, name: &str, args: &[f64]) -> Result<f64, &'static str> {
        let a = args.first().copied().unwrap_or(0.0);
        match name {
            "sin" => Ok(math::sin(self.angle_unit.to_radians(a))),
            "cos" => Ok(math::cos(self.angle_unit.to_radians(a))),
            "tan" => Ok(math::tan(self.angle_unit.to_radians(a))),
            "asin" => Ok(self.angle_unit.from_radians(math::asin(a))),
            "acos" => Ok(self.angle_unit.from_radians(math::acos(a))),
            "atan" => Ok(self.angle_unit.from_radians(math::atan(a))),
            "sinh" => Ok(math::sinh(a)),
            "cosh" => Ok(math::cosh(a)),
            "tanh" => Ok(math::tanh(a)),
            "log" => {
                if a <= 0.0 {
                    return Err("log of non-positive");
                }
                Ok(math::log10(a))
            }
            "ln" => {
                if a <= 0.0 {
                    return Err("ln of non-positive");
                }
                Ok(math::ln(a))
            }
            "log_base" => {
                let base = args.get(1).copied().unwrap_or(10.0);
                if a <= 0.0 || base <= 0.0 || base == 1.0 {
                    return Err("invalid log_base args");
                }
                Ok(math::ln(a) / math::ln(base))
            }
            "exp" => Ok(math::exp(a)),
            "sqrt" => {
                if a < 0.0 {
                    return Err("sqrt of negative");
                }
                Ok(math::sqrt(a))
            }
            "cbrt" => Ok(math::cbrt(a)),
            "nroot" => {
                let n = args.get(1).copied().unwrap_or(2.0);
                if n == 0.0 {
                    return Err("zeroth root");
                }
                Ok(math::pow(a, 1.0 / n))
            }
            "abs" => Ok(math::fabs(a)),
            "floor" => Ok(math::floor(a)),
            "ceil" => Ok(math::ceil(a)),
            "round" => Ok(math::round(a)),
            "pow" | "power" => {
                let exp = args.get(1).copied().unwrap_or(2.0);
                Ok(math::pow(a, exp))
            }
            _ => Err("unknown function"),
        }
    }
}

fn factorial(n: u64) -> u64 {
    if n <= 1 {
        return 1;
    }
    let mut result: u64 = 1;
    for i in 2..=n.min(20) {
        result = result.saturating_mul(i);
    }
    result
}

// ── Programmer mode ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberBase {
    Hex,
    Dec,
    Oct,
    Bin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitWidth {
    Bits8,
    Bits16,
    Bits32,
    Bits64,
}

impl BitWidth {
    pub fn mask(&self) -> u64 {
        match self {
            BitWidth::Bits8 => 0xFF,
            BitWidth::Bits16 => 0xFFFF,
            BitWidth::Bits32 => 0xFFFF_FFFF,
            BitWidth::Bits64 => u64::MAX,
        }
    }

    pub fn bits(&self) -> u32 {
        match self {
            BitWidth::Bits8 => 8,
            BitWidth::Bits16 => 16,
            BitWidth::Bits32 => 32,
            BitWidth::Bits64 => 64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitwiseOp {
    And,
    Or,
    Xor,
    Not,
    Nand,
    Nor,
    Lsh,
    Rsh,
    Rol,
    Ror,
}

#[derive(Debug, Clone)]
pub struct ProgrammerState {
    pub base: NumberBase,
    pub bit_width: BitWidth,
    pub value: u64,
    pub display_twos_complement: bool,
}

impl ProgrammerState {
    pub fn new() -> Self {
        Self {
            base: NumberBase::Dec,
            bit_width: BitWidth::Bits64,
            value: 0,
            display_twos_complement: false,
        }
    }

    pub fn set_value(&mut self, v: u64) {
        self.value = v & self.bit_width.mask();
    }

    pub fn apply_bitwise(&mut self, op: BitwiseOp, operand: u64) {
        let mask = self.bit_width.mask();
        let bits = self.bit_width.bits();
        self.value = match op {
            BitwiseOp::And => self.value & operand,
            BitwiseOp::Or => self.value | operand,
            BitwiseOp::Xor => self.value ^ operand,
            BitwiseOp::Not => !self.value,
            BitwiseOp::Nand => !(self.value & operand),
            BitwiseOp::Nor => !(self.value | operand),
            BitwiseOp::Lsh => self.value << (operand as u32 % 64),
            BitwiseOp::Rsh => self.value >> (operand as u32 % 64),
            BitwiseOp::Rol => {
                let shift = (operand as u32) % bits;
                (self.value << shift) | (self.value >> (bits - shift))
            }
            BitwiseOp::Ror => {
                let shift = (operand as u32) % bits;
                (self.value >> shift) | (self.value << (bits - shift))
            }
        } & mask;
    }

    pub fn byte_swap(&mut self) {
        self.value = match self.bit_width {
            BitWidth::Bits8 => self.value,
            BitWidth::Bits16 => (self.value as u16).swap_bytes() as u64,
            BitWidth::Bits32 => (self.value as u32).swap_bytes() as u64,
            BitWidth::Bits64 => self.value.swap_bytes(),
        };
    }

    pub fn popcount(&self) -> u32 {
        (self.value & self.bit_width.mask()).count_ones()
    }

    pub fn leading_zeros(&self) -> u32 {
        let masked = self.value & self.bit_width.mask();
        let bits = self.bit_width.bits();
        if masked == 0 {
            return bits;
        }
        let lz = masked.leading_zeros();
        lz.saturating_sub(64 - bits)
    }

    pub fn trailing_zeros(&self) -> u32 {
        let masked = self.value & self.bit_width.mask();
        if masked == 0 {
            return self.bit_width.bits();
        }
        masked.trailing_zeros().min(self.bit_width.bits())
    }

    pub fn twos_complement_signed(&self) -> i64 {
        let bits = self.bit_width.bits();
        let mask = self.bit_width.mask();
        let v = self.value & mask;
        let sign_bit = 1u64 << (bits - 1);
        if v & sign_bit != 0 {
            (v | !mask) as i64
        } else {
            v as i64
        }
    }

    pub fn format_value(&self, base: NumberBase) -> String {
        let v = self.value & self.bit_width.mask();
        match base {
            NumberBase::Hex => format_hex(v),
            NumberBase::Dec => format_dec(v),
            NumberBase::Oct => format_oct(v),
            NumberBase::Bin => format_bin(v, self.bit_width.bits()),
        }
    }

    pub fn ascii_char(&self) -> Option<char> {
        let v = (self.value & 0x7F) as u8;
        if v >= 0x20 && v < 0x7F {
            Some(v as char)
        } else {
            None
        }
    }
}

fn format_hex(v: u64) -> String {
    if v == 0 {
        return String::from("0");
    }
    let mut buf = Vec::new();
    let mut n = v;
    while n > 0 {
        let d = (n & 0xF) as u8;
        buf.push(if d < 10 { b'0' + d } else { b'A' + d - 10 });
        n >>= 4;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap_or_default()
}

fn format_dec(v: u64) -> String {
    if v == 0 {
        return String::from("0");
    }
    let mut buf = Vec::new();
    let mut n = v;
    while n > 0 {
        buf.push(b'0' + (n % 10) as u8);
        n /= 10;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap_or_default()
}

fn format_oct(v: u64) -> String {
    if v == 0 {
        return String::from("0");
    }
    let mut buf = Vec::new();
    let mut n = v;
    while n > 0 {
        buf.push(b'0' + (n & 7) as u8);
        n >>= 3;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap_or_default()
}

fn format_bin(v: u64, bits: u32) -> String {
    let mut buf = Vec::with_capacity(bits as usize);
    for i in (0..bits).rev() {
        buf.push(if (v >> i) & 1 == 1 { b'1' } else { b'0' });
    }
    String::from_utf8(buf).unwrap_or_default()
}

// ── Date calculator ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimpleDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl SimpleDate {
    pub fn new(year: i32, month: u8, day: u8) -> Self {
        Self {
            year,
            month: month.clamp(1, 12),
            day: day.clamp(1, 31),
        }
    }

    pub fn is_leap_year(&self) -> bool {
        let y = self.year;
        (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
    }

    pub fn days_in_month(&self) -> u8 {
        match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if self.is_leap_year() {
                    29
                } else {
                    28
                }
            }
            _ => 30,
        }
    }

    pub fn to_day_number(&self) -> i64 {
        let mut y = self.year as i64;
        let mut m = self.month as i64;
        if m <= 2 {
            y -= 1;
            m += 12;
        }
        let era = y / 400;
        let yoe = y - era * 400;
        let doy = (153 * (m - 3) + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe
    }

    pub fn day_of_week(&self) -> u8 {
        let d = self.to_day_number();
        ((d % 7 + 7) % 7) as u8
    }

    pub fn week_number(&self) -> u8 {
        let jan1 = SimpleDate::new(self.year, 1, 1);
        let diff = self.to_day_number() - jan1.to_day_number();
        ((diff + jan1.day_of_week() as i64) / 7 + 1) as u8
    }

    pub fn difference_days(&self, other: &SimpleDate) -> i64 {
        other.to_day_number() - self.to_day_number()
    }

    pub fn add_days(&self, days: i64) -> SimpleDate {
        let target = self.to_day_number() + days;
        day_number_to_date(target)
    }
}

fn day_number_to_date(dn: i64) -> SimpleDate {
    let era = dn.div_euclid(146097);
    let doe = dn.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = if m <= 2 { y + 1 } else { y };
    SimpleDate::new(yr as i32, m as u8, d)
}

#[derive(Debug, Clone)]
pub struct DateDifference {
    pub days: i64,
    pub weeks: i64,
    pub months: i32,
    pub years: i32,
}

pub fn date_difference(a: &SimpleDate, b: &SimpleDate) -> DateDifference {
    let days = a.difference_days(b).abs();
    let weeks = days / 7;
    let year_diff = (b.year - a.year).abs();
    let month_diff = year_diff * 12 + (b.month as i32 - a.month as i32).abs();
    DateDifference {
        days,
        weeks,
        months: month_diff,
        years: year_diff,
    }
}

// ── Unit converter ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitCategory {
    Length,
    Weight,
    Temperature,
    Energy,
    Area,
    Speed,
    Time,
    Power,
    Data,
    Pressure,
    Angle,
    Volume,
    Frequency,
}

#[derive(Debug, Clone)]
pub struct UnitDef {
    pub name: String,
    pub abbreviation: String,
    pub to_base: f64,
    pub offset: f64,
    pub category: UnitCategory,
}

pub struct UnitConverter {
    pub units: Vec<UnitDef>,
    pub current_category: UnitCategory,
    pub from_unit: usize,
    pub to_unit: usize,
    pub input_value: f64,
}

impl UnitConverter {
    pub fn new() -> Self {
        let mut uc = Self {
            units: Vec::new(),
            current_category: UnitCategory::Length,
            from_unit: 0,
            to_unit: 1,
            input_value: 1.0,
        };
        uc.populate_units();
        uc
    }

    fn add_unit(&mut self, name: &str, abbr: &str, to_base: f64, offset: f64, cat: UnitCategory) {
        self.units.push(UnitDef {
            name: String::from(name),
            abbreviation: String::from(abbr),
            to_base,
            offset,
            category: cat,
        });
    }

    fn populate_units(&mut self) {
        // Length (base: meters)
        self.add_unit("Meter", "m", 1.0, 0.0, UnitCategory::Length);
        self.add_unit("Kilometer", "km", 1000.0, 0.0, UnitCategory::Length);
        self.add_unit("Centimeter", "cm", 0.01, 0.0, UnitCategory::Length);
        self.add_unit("Millimeter", "mm", 0.001, 0.0, UnitCategory::Length);
        self.add_unit("Micrometer", "µm", 1e-6, 0.0, UnitCategory::Length);
        self.add_unit("Nanometer", "nm", 1e-9, 0.0, UnitCategory::Length);
        self.add_unit("Mile", "mi", 1609.344, 0.0, UnitCategory::Length);
        self.add_unit("Yard", "yd", 0.9144, 0.0, UnitCategory::Length);
        self.add_unit("Foot", "ft", 0.3048, 0.0, UnitCategory::Length);
        self.add_unit("Inch", "in", 0.0254, 0.0, UnitCategory::Length);
        self.add_unit("Nautical Mile", "nmi", 1852.0, 0.0, UnitCategory::Length);
        self.add_unit("Light Year", "ly", 9.461e15, 0.0, UnitCategory::Length);
        self.add_unit(
            "Astronomical Unit",
            "AU",
            1.496e11,
            0.0,
            UnitCategory::Length,
        );
        self.add_unit("Fathom", "ftm", 1.8288, 0.0, UnitCategory::Length);
        self.add_unit("Furlong", "fur", 201.168, 0.0, UnitCategory::Length);

        // Weight (base: kilograms)
        self.add_unit("Kilogram", "kg", 1.0, 0.0, UnitCategory::Weight);
        self.add_unit("Gram", "g", 0.001, 0.0, UnitCategory::Weight);
        self.add_unit("Milligram", "mg", 1e-6, 0.0, UnitCategory::Weight);
        self.add_unit("Microgram", "µg", 1e-9, 0.0, UnitCategory::Weight);
        self.add_unit("Metric Ton", "t", 1000.0, 0.0, UnitCategory::Weight);
        self.add_unit("Pound", "lb", 0.45359237, 0.0, UnitCategory::Weight);
        self.add_unit("Ounce", "oz", 0.0283495, 0.0, UnitCategory::Weight);
        self.add_unit("Stone", "st", 6.35029, 0.0, UnitCategory::Weight);
        self.add_unit("Short Ton", "US ton", 907.185, 0.0, UnitCategory::Weight);
        self.add_unit("Long Ton", "UK ton", 1016.05, 0.0, UnitCategory::Weight);
        self.add_unit("Carat", "ct", 0.0002, 0.0, UnitCategory::Weight);
        self.add_unit("Grain", "gr", 6.47989e-5, 0.0, UnitCategory::Weight);
        self.add_unit("Slug", "slug", 14.5939, 0.0, UnitCategory::Weight);
        self.add_unit(
            "Atomic Mass Unit",
            "u",
            1.66054e-27,
            0.0,
            UnitCategory::Weight,
        );
        self.add_unit("Dram", "dr", 0.00177185, 0.0, UnitCategory::Weight);

        // Temperature (special handling via offset)
        self.add_unit("Celsius", "°C", 1.0, 0.0, UnitCategory::Temperature);
        self.add_unit("Fahrenheit", "°F", 0.5556, -32.0, UnitCategory::Temperature);
        self.add_unit("Kelvin", "K", 1.0, -273.15, UnitCategory::Temperature);
        self.add_unit("Rankine", "°R", 0.5556, -491.67, UnitCategory::Temperature);

        // Energy (base: joules)
        self.add_unit("Joule", "J", 1.0, 0.0, UnitCategory::Energy);
        self.add_unit("Kilojoule", "kJ", 1000.0, 0.0, UnitCategory::Energy);
        self.add_unit("Calorie", "cal", 4.184, 0.0, UnitCategory::Energy);
        self.add_unit("Kilocalorie", "kcal", 4184.0, 0.0, UnitCategory::Energy);
        self.add_unit("Watt-hour", "Wh", 3600.0, 0.0, UnitCategory::Energy);
        self.add_unit("Kilowatt-hour", "kWh", 3.6e6, 0.0, UnitCategory::Energy);
        self.add_unit("Electronvolt", "eV", 1.602e-19, 0.0, UnitCategory::Energy);
        self.add_unit("BTU", "BTU", 1055.06, 0.0, UnitCategory::Energy);
        self.add_unit("Foot-pound", "ft·lb", 1.35582, 0.0, UnitCategory::Energy);
        self.add_unit("Erg", "erg", 1e-7, 0.0, UnitCategory::Energy);
        self.add_unit("Therm", "thm", 1.055e8, 0.0, UnitCategory::Energy);

        // Area (base: square meters)
        self.add_unit("Square Meter", "m²", 1.0, 0.0, UnitCategory::Area);
        self.add_unit("Square Kilometer", "km²", 1e6, 0.0, UnitCategory::Area);
        self.add_unit("Square Mile", "mi²", 2.59e6, 0.0, UnitCategory::Area);
        self.add_unit("Square Yard", "yd²", 0.836127, 0.0, UnitCategory::Area);
        self.add_unit("Square Foot", "ft²", 0.092903, 0.0, UnitCategory::Area);
        self.add_unit("Square Inch", "in²", 6.4516e-4, 0.0, UnitCategory::Area);
        self.add_unit("Hectare", "ha", 10000.0, 0.0, UnitCategory::Area);
        self.add_unit("Acre", "ac", 4046.86, 0.0, UnitCategory::Area);

        // Speed (base: meters per second)
        self.add_unit("m/s", "m/s", 1.0, 0.0, UnitCategory::Speed);
        self.add_unit("km/h", "km/h", 0.277778, 0.0, UnitCategory::Speed);
        self.add_unit("mph", "mph", 0.44704, 0.0, UnitCategory::Speed);
        self.add_unit("Knot", "kn", 0.514444, 0.0, UnitCategory::Speed);
        self.add_unit("ft/s", "ft/s", 0.3048, 0.0, UnitCategory::Speed);
        self.add_unit("Mach", "M", 343.0, 0.0, UnitCategory::Speed);
        self.add_unit("Speed of Light", "c", 2.998e8, 0.0, UnitCategory::Speed);

        // Time (base: seconds)
        self.add_unit("Second", "s", 1.0, 0.0, UnitCategory::Time);
        self.add_unit("Millisecond", "ms", 0.001, 0.0, UnitCategory::Time);
        self.add_unit("Microsecond", "µs", 1e-6, 0.0, UnitCategory::Time);
        self.add_unit("Nanosecond", "ns", 1e-9, 0.0, UnitCategory::Time);
        self.add_unit("Minute", "min", 60.0, 0.0, UnitCategory::Time);
        self.add_unit("Hour", "h", 3600.0, 0.0, UnitCategory::Time);
        self.add_unit("Day", "d", 86400.0, 0.0, UnitCategory::Time);
        self.add_unit("Week", "wk", 604800.0, 0.0, UnitCategory::Time);
        self.add_unit("Month (30d)", "mo", 2592000.0, 0.0, UnitCategory::Time);
        self.add_unit("Year (365d)", "yr", 31536000.0, 0.0, UnitCategory::Time);

        // Power (base: watts)
        self.add_unit("Watt", "W", 1.0, 0.0, UnitCategory::Power);
        self.add_unit("Kilowatt", "kW", 1000.0, 0.0, UnitCategory::Power);
        self.add_unit("Megawatt", "MW", 1e6, 0.0, UnitCategory::Power);
        self.add_unit("Horsepower", "hp", 745.7, 0.0, UnitCategory::Power);
        self.add_unit("BTU/h", "BTU/h", 0.29307, 0.0, UnitCategory::Power);
        self.add_unit("Milliwatt", "mW", 0.001, 0.0, UnitCategory::Power);

        // Data (base: bytes)
        self.add_unit("Byte", "B", 1.0, 0.0, UnitCategory::Data);
        self.add_unit("Kilobyte", "KB", 1024.0, 0.0, UnitCategory::Data);
        self.add_unit("Megabyte", "MB", 1048576.0, 0.0, UnitCategory::Data);
        self.add_unit("Gigabyte", "GB", 1073741824.0, 0.0, UnitCategory::Data);
        self.add_unit("Terabyte", "TB", 1099511627776.0, 0.0, UnitCategory::Data);
        self.add_unit(
            "Petabyte",
            "PB",
            1125899906842624.0,
            0.0,
            UnitCategory::Data,
        );
        self.add_unit("Bit", "bit", 0.125, 0.0, UnitCategory::Data);
        self.add_unit("Kilobit", "Kbit", 128.0, 0.0, UnitCategory::Data);
        self.add_unit("Megabit", "Mbit", 131072.0, 0.0, UnitCategory::Data);
        self.add_unit("Gigabit", "Gbit", 134217728.0, 0.0, UnitCategory::Data);

        // Pressure (base: pascals)
        self.add_unit("Pascal", "Pa", 1.0, 0.0, UnitCategory::Pressure);
        self.add_unit("Kilopascal", "kPa", 1000.0, 0.0, UnitCategory::Pressure);
        self.add_unit("Bar", "bar", 100000.0, 0.0, UnitCategory::Pressure);
        self.add_unit("Atmosphere", "atm", 101325.0, 0.0, UnitCategory::Pressure);
        self.add_unit("PSI", "psi", 6894.76, 0.0, UnitCategory::Pressure);
        self.add_unit("Torr", "Torr", 133.322, 0.0, UnitCategory::Pressure);
        self.add_unit("mmHg", "mmHg", 133.322, 0.0, UnitCategory::Pressure);

        // Angle (base: radians)
        self.add_unit("Radian", "rad", 1.0, 0.0, UnitCategory::Angle);
        self.add_unit("Degree", "°", 0.0174533, 0.0, UnitCategory::Angle);
        self.add_unit("Gradian", "gon", 0.015708, 0.0, UnitCategory::Angle);
        self.add_unit("Arcminute", "'", 2.9089e-4, 0.0, UnitCategory::Angle);
        self.add_unit("Arcsecond", "\"", 4.8481e-6, 0.0, UnitCategory::Angle);
        self.add_unit("Revolution", "rev", 6.28318, 0.0, UnitCategory::Angle);

        // Volume (base: liters)
        self.add_unit("Liter", "L", 1.0, 0.0, UnitCategory::Volume);
        self.add_unit("Milliliter", "mL", 0.001, 0.0, UnitCategory::Volume);
        self.add_unit("Cubic Meter", "m³", 1000.0, 0.0, UnitCategory::Volume);
        self.add_unit("Gallon (US)", "gal", 3.78541, 0.0, UnitCategory::Volume);
        self.add_unit("Quart (US)", "qt", 0.946353, 0.0, UnitCategory::Volume);
        self.add_unit("Pint (US)", "pt", 0.473176, 0.0, UnitCategory::Volume);
        self.add_unit("Cup (US)", "cup", 0.236588, 0.0, UnitCategory::Volume);
        self.add_unit(
            "Fluid Ounce (US)",
            "fl oz",
            0.0295735,
            0.0,
            UnitCategory::Volume,
        );
        self.add_unit("Tablespoon", "tbsp", 0.0147868, 0.0, UnitCategory::Volume);
        self.add_unit("Teaspoon", "tsp", 0.00492892, 0.0, UnitCategory::Volume);
        self.add_unit("Gallon (UK)", "gal UK", 4.54609, 0.0, UnitCategory::Volume);
        self.add_unit("Cubic Inch", "in³", 0.0163871, 0.0, UnitCategory::Volume);
        self.add_unit("Cubic Foot", "ft³", 28.3168, 0.0, UnitCategory::Volume);
        self.add_unit("Barrel", "bbl", 158.987, 0.0, UnitCategory::Volume);

        // Frequency (base: hertz)
        self.add_unit("Hertz", "Hz", 1.0, 0.0, UnitCategory::Frequency);
        self.add_unit("Kilohertz", "kHz", 1000.0, 0.0, UnitCategory::Frequency);
        self.add_unit("Megahertz", "MHz", 1e6, 0.0, UnitCategory::Frequency);
        self.add_unit("Gigahertz", "GHz", 1e9, 0.0, UnitCategory::Frequency);
        self.add_unit("RPM", "rpm", 0.0166667, 0.0, UnitCategory::Frequency);
    }

    pub fn units_for_category(&self, cat: UnitCategory) -> Vec<(usize, &UnitDef)> {
        self.units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.category == cat)
            .collect()
    }

    pub fn convert(&self) -> f64 {
        let from = match self.units.get(self.from_unit) {
            Some(u) => u,
            None => return 0.0,
        };
        let to = match self.units.get(self.to_unit) {
            Some(u) => u,
            None => return 0.0,
        };
        if from.category == UnitCategory::Temperature {
            let celsius = (self.input_value + from.offset) * from.to_base;
            celsius / to.to_base - to.offset
        } else {
            let base = self.input_value * from.to_base;
            base / to.to_base
        }
    }
}

// ── Currency converter ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Currency {
    pub code: String,
    pub name: String,
    pub symbol: String,
    pub rate_to_usd: f64,
}

pub struct CurrencyConverter {
    pub currencies: Vec<Currency>,
    pub from_index: usize,
    pub to_index: usize,
    pub amount: f64,
    pub last_update_ts: u64,
    pub history: Vec<(String, String, f64, f64, u64)>,
}

impl CurrencyConverter {
    pub fn new() -> Self {
        let mut cc = Self {
            currencies: Vec::new(),
            from_index: 0,
            to_index: 1,
            amount: 1.0,
            last_update_ts: 0,
            history: Vec::new(),
        };
        cc.populate_defaults();
        cc
    }

    fn add_currency(&mut self, code: &str, name: &str, symbol: &str, rate: f64) {
        self.currencies.push(Currency {
            code: String::from(code),
            name: String::from(name),
            symbol: String::from(symbol),
            rate_to_usd: rate,
        });
    }

    fn populate_defaults(&mut self) {
        self.add_currency("USD", "US Dollar", "$", 1.0);
        self.add_currency("EUR", "Euro", "€", 0.92);
        self.add_currency("GBP", "British Pound", "£", 0.79);
        self.add_currency("JPY", "Japanese Yen", "¥", 149.5);
        self.add_currency("CAD", "Canadian Dollar", "C$", 1.36);
        self.add_currency("AUD", "Australian Dollar", "A$", 1.53);
        self.add_currency("CHF", "Swiss Franc", "Fr", 0.88);
        self.add_currency("CNY", "Chinese Yuan", "¥", 7.24);
        self.add_currency("INR", "Indian Rupee", "₹", 83.1);
        self.add_currency("MXN", "Mexican Peso", "MX$", 17.2);
        self.add_currency("BRL", "Brazilian Real", "R$", 4.97);
        self.add_currency("KRW", "South Korean Won", "₩", 1320.0);
        self.add_currency("SEK", "Swedish Krona", "kr", 10.4);
        self.add_currency("NOK", "Norwegian Krone", "kr", 10.6);
        self.add_currency("DKK", "Danish Krone", "kr", 6.88);
        self.add_currency("NZD", "New Zealand Dollar", "NZ$", 1.63);
        self.add_currency("SGD", "Singapore Dollar", "S$", 1.34);
        self.add_currency("HKD", "Hong Kong Dollar", "HK$", 7.82);
        self.add_currency("TRY", "Turkish Lira", "₺", 28.9);
        self.add_currency("ZAR", "South African Rand", "R", 18.7);
        self.add_currency("RUB", "Russian Ruble", "₽", 92.5);
        self.add_currency("PLN", "Polish Zloty", "zł", 4.02);
        self.add_currency("THB", "Thai Baht", "฿", 35.1);
        self.add_currency("IDR", "Indonesian Rupiah", "Rp", 15500.0);
        self.add_currency("MYR", "Malaysian Ringgit", "RM", 4.68);
        self.add_currency("PHP", "Philippine Peso", "₱", 55.8);
        self.add_currency("CZK", "Czech Koruna", "Kč", 22.5);
        self.add_currency("ILS", "Israeli Shekel", "₪", 3.67);
        self.add_currency("CLP", "Chilean Peso", "CLP$", 880.0);
        self.add_currency("AED", "UAE Dirham", "د.إ", 3.67);
        self.add_currency("SAR", "Saudi Riyal", "﷼", 3.75);
        self.add_currency("EGP", "Egyptian Pound", "E£", 30.9);
        self.add_currency("TWD", "Taiwan Dollar", "NT$", 31.5);
        self.add_currency("ARS", "Argentine Peso", "AR$", 350.0);
        self.add_currency("COP", "Colombian Peso", "COL$", 3950.0);
        self.add_currency("VND", "Vietnamese Dong", "₫", 24300.0);
        self.add_currency("PKR", "Pakistani Rupee", "Rs", 283.0);
        self.add_currency("BDT", "Bangladeshi Taka", "৳", 110.0);
        self.add_currency("NGN", "Nigerian Naira", "₦", 780.0);
        self.add_currency("UAH", "Ukrainian Hryvnia", "₴", 37.5);
        self.add_currency("PEN", "Peruvian Sol", "S/.", 3.74);
        self.add_currency("RON", "Romanian Leu", "lei", 4.57);
        self.add_currency("HUF", "Hungarian Forint", "Ft", 356.0);
        self.add_currency("BGN", "Bulgarian Lev", "лв", 1.80);
        self.add_currency("HRK", "Croatian Kuna", "kn", 6.93);
        self.add_currency("ISK", "Icelandic Krona", "kr", 137.0);
        self.add_currency("KES", "Kenyan Shilling", "KSh", 153.0);
        self.add_currency("GHS", "Ghanaian Cedi", "₵", 12.3);
        self.add_currency("MAD", "Moroccan Dirham", "MAD", 10.1);
        self.add_currency("BTC", "Bitcoin", "₿", 0.0000234);
    }

    pub fn convert(&self) -> f64 {
        let from = match self.currencies.get(self.from_index) {
            Some(c) => c,
            None => return 0.0,
        };
        let to = match self.currencies.get(self.to_index) {
            Some(c) => c,
            None => return 0.0,
        };
        let usd = self.amount / from.rate_to_usd;
        usd * to.rate_to_usd
    }

    pub fn record_conversion(&mut self, ts: u64) {
        let from_code = self
            .currencies
            .get(self.from_index)
            .map(|c| c.code.clone())
            .unwrap_or_default();
        let to_code = self
            .currencies
            .get(self.to_index)
            .map(|c| c.code.clone())
            .unwrap_or_default();
        let result = self.convert();
        self.history
            .push((from_code, to_code, self.amount, result, ts));
    }

    pub fn update_rate(&mut self, code: &str, new_rate: f64) {
        if let Some(c) = self.currencies.iter_mut().find(|c| c.code == code) {
            c.rate_to_usd = new_rate;
        }
    }
}

// ── Graph mode ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GraphFunction {
    pub expression: String,
    pub color: u32,
    pub visible: bool,
}

pub struct GraphView {
    pub functions: Vec<GraphFunction>,
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
    pub show_grid: bool,
    pub show_axis_labels: bool,
    pub trace_x: Option<f64>,
    pub width_px: u32,
    pub height_px: u32,
}

impl GraphView {
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            x_min: -10.0,
            x_max: 10.0,
            y_min: -10.0,
            y_max: 10.0,
            show_grid: true,
            show_axis_labels: true,
            trace_x: None,
            width_px: 400,
            height_px: 300,
        }
    }

    pub fn add_function(&mut self, expr: &str, color: u32) {
        self.functions.push(GraphFunction {
            expression: String::from(expr),
            color,
            visible: true,
        });
    }

    pub fn remove_function(&mut self, idx: usize) {
        if idx < self.functions.len() {
            self.functions.remove(idx);
        }
    }

    pub fn zoom_in(&mut self) {
        let cx = (self.x_min + self.x_max) / 2.0;
        let cy = (self.y_min + self.y_max) / 2.0;
        let xr = (self.x_max - self.x_min) / 4.0;
        let yr = (self.y_max - self.y_min) / 4.0;
        self.x_min = cx - xr;
        self.x_max = cx + xr;
        self.y_min = cy - yr;
        self.y_max = cy + yr;
    }

    pub fn zoom_out(&mut self) {
        let cx = (self.x_min + self.x_max) / 2.0;
        let cy = (self.y_min + self.y_max) / 2.0;
        let xr = (self.x_max - self.x_min);
        let yr = (self.y_max - self.y_min);
        self.x_min = cx - xr;
        self.x_max = cx + xr;
        self.y_min = cy - yr;
        self.y_max = cy + yr;
    }

    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.x_min += dx;
        self.x_max += dx;
        self.y_min += dy;
        self.y_max += dy;
    }

    pub fn screen_to_math(&self, px: u32, py: u32) -> (f64, f64) {
        let x = self.x_min + (px as f64 / self.width_px as f64) * (self.x_max - self.x_min);
        let y = self.y_max - (py as f64 / self.height_px as f64) * (self.y_max - self.y_min);
        (x, y)
    }

    pub fn math_to_screen(&self, x: f64, y: f64) -> (i32, i32) {
        let px = ((x - self.x_min) / (self.x_max - self.x_min) * self.width_px as f64) as i32;
        let py = ((self.y_max - y) / (self.y_max - self.y_min) * self.height_px as f64) as i32;
        (px, py)
    }

    pub fn evaluate_at(&self, func_idx: usize, x: f64, angle_unit: AngleUnit) -> Option<f64> {
        let func = self.functions.get(func_idx)?;
        if !func.visible {
            return None;
        }
        let expr = func.expression.replace("x", &format_f64_simple(x));
        let mut tokenizer = Tokenizer::new(&expr);
        let tokens = tokenizer.tokenize();
        let mut eval = ExprEvaluator::new(tokens, angle_unit);
        eval.evaluate().ok()
    }

    pub fn trace_value(&self, func_idx: usize, angle_unit: AngleUnit) -> Option<(f64, f64)> {
        let x = self.trace_x?;
        let y = self.evaluate_at(func_idx, x, angle_unit)?;
        Some((x, y))
    }
}

fn format_f64_simple(v: f64) -> String {
    let neg = v < 0.0;
    let abs = if neg { -v } else { v };
    let int_part = abs as u64;
    let frac = ((abs - int_part as f64) * 1000000.0) as u64;
    let mut s = if neg {
        format!("(-{}.{:06})", int_part, frac)
    } else {
        format!("{}.{:06}", int_part, frac)
    };
    s
}

// ── Main calculator ──────────────────────────────────────────────────────

pub struct Calculator {
    pub mode: CalcMode,
    pub display: String,
    pub expression: String,
    pub result: f64,
    pub angle_unit: AngleUnit,
    pub memory: MemoryStore,
    pub history: CalcHistory,
    pub programmer: ProgrammerState,
    pub unit_converter: UnitConverter,
    pub currency_converter: CurrencyConverter,
    pub graph: GraphView,
    pub always_on_top: bool,
    pub compact_view: bool,
    pub error_message: Option<String>,
    pub last_operator: Option<Token>,
    pub clear_on_next_digit: bool,
}

impl Calculator {
    pub fn new() -> Self {
        Self {
            mode: CalcMode::Standard,
            display: String::from("0"),
            expression: String::new(),
            result: 0.0,
            angle_unit: AngleUnit::Degrees,
            memory: MemoryStore::new(10),
            history: CalcHistory::new(200),
            programmer: ProgrammerState::new(),
            unit_converter: UnitConverter::new(),
            currency_converter: CurrencyConverter::new(),
            graph: GraphView::new(),
            always_on_top: false,
            compact_view: false,
            error_message: None,
            last_operator: None,
            clear_on_next_digit: false,
        }
    }

    pub fn set_mode(&mut self, mode: CalcMode) {
        self.mode = mode;
        self.error_message = None;
    }

    pub fn input_digit(&mut self, d: char) {
        if self.clear_on_next_digit {
            self.display.clear();
            self.clear_on_next_digit = false;
        }
        if self.display == "0" && d != '.' {
            self.display.clear();
        }
        if d == '.' && self.display.contains('.') {
            return;
        }
        self.display.push(d);
    }

    pub fn input_operator(&mut self, op: Token) {
        if !self.expression.is_empty() && !self.display.is_empty() {
            self.evaluate_expression();
        }
        let op_str = match &op {
            Token::Plus => "+",
            Token::Minus => "-",
            Token::Multiply => "×",
            Token::Divide => "÷",
            _ => "",
        };
        self.expression = format!("{} {} ", self.display, op_str);
        self.last_operator = Some(op);
        self.clear_on_next_digit = true;
    }

    pub fn negate(&mut self) {
        if self.display.starts_with('-') {
            self.display = self.display[1..].into();
        } else if self.display != "0" {
            self.display = format!("-{}", self.display);
        }
    }

    pub fn percentage(&mut self) {
        if let Ok(v) = self.parse_display() {
            self.result = v / 100.0;
            self.display = self.format_result(self.result);
        }
    }

    pub fn reciprocal(&mut self) {
        if let Ok(v) = self.parse_display() {
            if v == 0.0 {
                self.error_message = Some(String::from("Cannot divide by zero"));
                return;
            }
            self.result = 1.0 / v;
            self.display = self.format_result(self.result);
        }
    }

    pub fn square_root(&mut self) {
        if let Ok(v) = self.parse_display() {
            if v < 0.0 {
                self.error_message = Some(String::from("Invalid input"));
                return;
            }
            self.result = math::sqrt(v);
            self.display = self.format_result(self.result);
        }
    }

    pub fn clear_entry(&mut self) {
        self.display = String::from("0");
    }

    pub fn clear_all(&mut self) {
        self.display = String::from("0");
        self.expression.clear();
        self.result = 0.0;
        self.last_operator = None;
        self.error_message = None;
        self.clear_on_next_digit = false;
    }

    pub fn backspace(&mut self) {
        if self.display.len() > 1 {
            self.display.pop();
        } else {
            self.display = String::from("0");
        }
    }

    pub fn evaluate_expression(&mut self) {
        let full_expr = if self.expression.is_empty() {
            self.display.clone()
        } else {
            format!("{}{}", self.expression, self.display)
        };
        let normalized = full_expr.replace('×', "*").replace('÷', "/");
        let mut tokenizer = Tokenizer::new(&normalized);
        let tokens = tokenizer.tokenize();
        let mut eval = ExprEvaluator::new(tokens, self.angle_unit);
        match eval.evaluate() {
            Ok(val) => {
                self.result = val;
                self.display = self.format_result(val);
                self.history
                    .push(&full_expr, val, &self.display, self.mode, 0);
                self.expression.clear();
                self.clear_on_next_digit = true;
                self.error_message = None;
            }
            Err(e) => {
                self.error_message = Some(String::from(e));
            }
        }
    }

    pub fn evaluate_scientific(&mut self, func: &str) {
        if let Ok(v) = self.parse_display() {
            let res = match func {
                "sin" => math::sin(self.angle_unit.to_radians(v)),
                "cos" => math::cos(self.angle_unit.to_radians(v)),
                "tan" => math::tan(self.angle_unit.to_radians(v)),
                "asin" => self.angle_unit.from_radians(math::asin(v)),
                "acos" => self.angle_unit.from_radians(math::acos(v)),
                "atan" => self.angle_unit.from_radians(math::atan(v)),
                "sinh" => math::sinh(v),
                "cosh" => math::cosh(v),
                "tanh" => math::tanh(v),
                "log" => math::log10(v),
                "ln" => math::ln(v),
                "exp" => math::exp(v),
                "10^x" => math::pow(10.0, v),
                "2^x" => math::pow(2.0, v),
                "e^x" => math::exp(v),
                "x^2" => v * v,
                "x^3" => v * v * v,
                "sqrt" => math::sqrt(v),
                "cbrt" => math::cbrt(v),
                "abs" => math::fabs(v),
                "floor" => math::floor(v),
                "ceil" => math::ceil(v),
                "round" => math::round(v),
                "fact" => factorial(v as u64) as f64,
                _ => v,
            };
            self.result = res;
            self.display = self.format_result(res);
            self.clear_on_next_digit = true;
        }
    }

    pub fn memory_store(&mut self) {
        if let Ok(v) = self.parse_display() {
            self.memory.store(v);
        }
    }

    pub fn memory_recall(&mut self) {
        let v = self.memory.recall();
        self.display = self.format_result(v);
        self.clear_on_next_digit = true;
    }

    pub fn memory_add(&mut self) {
        if let Ok(v) = self.parse_display() {
            self.memory.add(v);
        }
    }

    pub fn memory_subtract(&mut self) {
        if let Ok(v) = self.parse_display() {
            self.memory.subtract(v);
        }
    }

    pub fn memory_clear(&mut self) {
        self.memory.clear();
    }

    pub fn recall_history(&mut self, idx: usize) {
        if let Some(entry) = self.history.get(idx) {
            self.display = entry.result_display.clone();
            self.result = entry.result;
            self.clear_on_next_digit = true;
        }
    }

    pub fn copy_result(&self) -> String {
        self.display.clone()
    }

    pub fn paste_expression(&mut self, text: &str) {
        self.expression = String::from(text);
        self.display = String::from(text);
    }

    fn parse_display(&self) -> Result<f64, &'static str> {
        let s = &self.display;
        if s.is_empty() {
            return Err("empty");
        }
        Ok(parse_f64(s))
    }

    fn format_result(&self, v: f64) -> String {
        if v == math::floor(v) && math::fabs(v) < 1e15 {
            format_dec(v as u64)
        } else {
            format_f64_simple(v)
        }
    }

    pub fn toggle_always_on_top(&mut self) {
        self.always_on_top = !self.always_on_top;
    }

    pub fn toggle_compact(&mut self) {
        self.compact_view = !self.compact_view;
    }
}

// ── Colour palette ───────────────────────────────────────────────────────

const CALC_BG: u32 = 0xFF_1A_1A_22;
const CALC_DISPLAY: u32 = 0xFF_0A_0E_1A;
const CALC_BTN: u32 = 0xFF_28_2C_44;
const CALC_BTN_OP: u32 = 0xFF_33_55_88;
const CALC_BTN_EQ: u32 = 0xFF_4E_9C_FF;
const CALC_FG: u32 = 0xFF_FF_FF_FF;
const CALC_DIM: u32 = 0xFF_90_90_A0;
const CALC_ACCENT: u32 = 0xFF_4E_9C_FF;
const CALC_ERR: u32 = 0xFF_FF_44_44;
const CALC_HIST_BG: u32 = 0xFF_12_14_20;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

// ── Rendering ────────────────────────────────────────────────────────────

impl Calculator {
    pub fn render(&self, canvas: &mut athgfx::Canvas, ox: usize, oy: usize, w: usize, h: usize) {
        canvas.fill_rect(ox, oy, w, h, CALC_BG);
        canvas.draw_rect_outline(ox, oy, w, h, CALC_ACCENT);

        canvas.fill_rect(ox + 4, oy + 4, w - 8, 44, CALC_DISPLAY);
        canvas.draw_rect_outline(ox + 4, oy + 4, w - 8, 44, CALC_ACCENT);

        if !self.expression.is_empty() {
            let expr_max = (w - 24) / GLYPH_W;
            let expr_disp = if self.expression.len() > expr_max {
                &self.expression[self.expression.len() - expr_max..]
            } else {
                self.expression.as_str()
            };
            canvas.draw_text(ox + 12, oy + 10, expr_disp, CALC_DIM, None);
        }

        let disp_max = (w - 24) / GLYPH_W;
        let disp_str = if self.display.len() > disp_max {
            &self.display[self.display.len() - disp_max..]
        } else {
            self.display.as_str()
        };
        let disp_x = ox + w - 12 - disp_str.len() * GLYPH_W;
        let fg = if self.error_message.is_some() {
            CALC_ERR
        } else {
            CALC_FG
        };
        canvas.draw_text(disp_x, oy + 30, disp_str, fg, None);

        if let Some(ref err) = self.error_message {
            let err_max = (w - 24) / GLYPH_W;
            let e = crate::text_util::truncate_chars(err, err_max);
            canvas.draw_text(ox + 12, oy + 10, e, CALC_ERR, None);
        }

        let grid_y = oy + 56;
        let buttons: &[&[(&str, u32)]] = &[
            &[
                ("%", CALC_BTN),
                ("CE", CALC_BTN),
                ("C", CALC_BTN),
                ("<", CALC_BTN),
            ],
            &[
                ("1/x", CALC_BTN),
                ("x2", CALC_BTN),
                ("V", CALC_BTN),
                ("/", CALC_BTN_OP),
            ],
            &[
                ("7", CALC_BTN),
                ("8", CALC_BTN),
                ("9", CALC_BTN),
                ("x", CALC_BTN_OP),
            ],
            &[
                ("4", CALC_BTN),
                ("5", CALC_BTN),
                ("6", CALC_BTN),
                ("-", CALC_BTN_OP),
            ],
            &[
                ("1", CALC_BTN),
                ("2", CALC_BTN),
                ("3", CALC_BTN),
                ("+", CALC_BTN_OP),
            ],
            &[
                ("+/-", CALC_BTN),
                ("0", CALC_BTN),
                (".", CALC_BTN),
                ("=", CALC_BTN_EQ),
            ],
        ];

        let btn_w = (w - 12) / 4;
        let btn_h = (h.saturating_sub(grid_y - oy + 4)) / buttons.len();

        for (ri, row) in buttons.iter().enumerate() {
            for (ci, &(label, bg)) in row.iter().enumerate() {
                let bx = ox + 4 + ci * (btn_w + 1);
                let by = grid_y + ri * (btn_h + 1);
                canvas.fill_rect(bx, by, btn_w, btn_h, bg);
                let lx = bx + (btn_w.saturating_sub(label.len() * GLYPH_W)) / 2;
                let ly = by + (btn_h.saturating_sub(GLYPH_H)) / 2;
                canvas.draw_text(lx, ly, label, CALC_FG, None);
            }
        }

        let hist: Vec<&HistoryEntry> = self.history.entries.iter().rev().take(5).collect();
        if !hist.is_empty() {
            let hx = ox + w + 4;
            canvas.fill_rect(hx, oy, 160, h, CALC_HIST_BG);
            canvas.draw_text(hx + 4, oy + 4, "History", CALC_DIM, None);
            for (i, entry) in hist.iter().enumerate() {
                let hy = oy + 20 + i * (GLYPH_H + 12);
                canvas.draw_text(hx + 4, hy, &entry.expression, CALC_DIM, None);
                canvas.draw_text(
                    hx + 4,
                    hy + GLYPH_H + 2,
                    &entry.result_display,
                    CALC_FG,
                    None,
                );
            }
        }
    }

    pub fn handle_key_input(&mut self, key: u8) {
        match key {
            b'0'..=b'9' => self.input_digit(key as char),
            b'.' => self.input_digit('.'),
            b'+' => self.input_operator(Token::Plus),
            b'-' => self.input_operator(Token::Minus),
            b'*' => self.input_operator(Token::Multiply),
            b'/' => self.input_operator(Token::Divide),
            0x0D => self.evaluate_expression(),
            0x1B => self.clear_all(),
            0x08 => self.backspace(),
            b'%' => self.percentage(),
            _ => {}
        }
    }

    pub fn handle_button_click(
        &mut self,
        bx: usize,
        by: usize,
        ox: usize,
        oy: usize,
        w: usize,
        h: usize,
    ) {
        let grid_y = oy + 56;
        if by < grid_y {
            return;
        }

        let btn_w = (w - 12) / 4;
        let btn_h = (h.saturating_sub(grid_y - oy + 4)) / 6;
        if btn_w == 0 || btn_h == 0 {
            return;
        }

        let col = (bx.saturating_sub(ox + 4)) / (btn_w + 1);
        let row = (by.saturating_sub(grid_y)) / (btn_h + 1);
        if col > 3 || row > 5 {
            return;
        }

        match (row, col) {
            (0, 0) => self.percentage(),
            (0, 1) => self.clear_entry(),
            (0, 2) => self.clear_all(),
            (0, 3) => self.backspace(),
            (1, 0) => self.reciprocal(),
            (1, 1) => {
                if let Ok(v) = self.parse_display() {
                    self.result = v * v;
                    self.display = self.format_result(self.result);
                    self.clear_on_next_digit = true;
                }
            }
            (1, 2) => self.square_root(),
            (1, 3) => self.input_operator(Token::Divide),
            (2, 0) => self.input_digit('7'),
            (2, 1) => self.input_digit('8'),
            (2, 2) => self.input_digit('9'),
            (2, 3) => self.input_operator(Token::Multiply),
            (3, 0) => self.input_digit('4'),
            (3, 1) => self.input_digit('5'),
            (3, 2) => self.input_digit('6'),
            (3, 3) => self.input_operator(Token::Minus),
            (4, 0) => self.input_digit('1'),
            (4, 1) => self.input_digit('2'),
            (4, 2) => self.input_digit('3'),
            (4, 3) => self.input_operator(Token::Plus),
            (5, 0) => self.negate(),
            (5, 1) => self.input_digit('0'),
            (5, 2) => self.input_digit('.'),
            (5, 3) => self.evaluate_expression(),
            _ => {}
        }
    }
}

// ── Global instance ──────────────────────────────────────────────────────

static mut CALCULATOR: Option<Calculator> = None;

pub unsafe fn init() {
    CALCULATOR = Some(Calculator::new());
}

pub unsafe fn get() -> &'static mut Calculator {
    CALCULATOR.as_mut().expect("calculator::init() not called")
}
