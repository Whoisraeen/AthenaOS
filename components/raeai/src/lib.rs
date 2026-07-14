#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ─── Data Types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DType {
    F32,
    F16,
    BF16,
    I32,
    I8,
    U8,
    Bool,
}

impl DType {
    pub fn size_bytes(self) -> usize {
        match self {
            DType::F32 | DType::I32 => 4,
            DType::F16 | DType::BF16 => 2,
            DType::I8 | DType::U8 | DType::Bool => 1,
        }
    }
}

// ─── Tensor ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Tensor {
    data: Vec<u8>,
    shape: Vec<usize>,
    strides: Vec<usize>,
    dtype: DType,
    offset: usize,
}

impl Tensor {
    pub fn zeros(shape: &[usize], dtype: DType) -> Self {
        let numel: usize = shape.iter().product();
        let byte_len = numel * dtype.size_bytes();
        let strides = Self::compute_strides(shape, dtype);
        Self {
            data: vec![0u8; byte_len],
            shape: shape.to_vec(),
            strides,
            dtype,
            offset: 0,
        }
    }

    pub fn ones(shape: &[usize], dtype: DType) -> Self {
        let numel: usize = shape.iter().product();
        let byte_len = numel * dtype.size_bytes();
        let mut data = vec![0u8; byte_len];
        match dtype {
            DType::F32 => {
                let one = 1.0f32.to_le_bytes();
                for i in 0..numel {
                    data[i * 4..i * 4 + 4].copy_from_slice(&one);
                }
            }
            DType::I32 => {
                let one = 1i32.to_le_bytes();
                for i in 0..numel {
                    data[i * 4..i * 4 + 4].copy_from_slice(&one);
                }
            }
            DType::I8 => {
                for b in data.iter_mut() {
                    *b = 1;
                }
            }
            DType::U8 | DType::Bool => {
                for b in data.iter_mut() {
                    *b = 1;
                }
            }
            _ => {}
        }
        let strides = Self::compute_strides(shape, dtype);
        Self {
            data,
            shape: shape.to_vec(),
            strides,
            dtype,
            offset: 0,
        }
    }

    pub fn from_f32(data: &[f32], shape: &[usize]) -> Self {
        let numel: usize = shape.iter().product();
        assert_eq!(data.len(), numel);
        let mut bytes = Vec::with_capacity(numel * 4);
        for &v in data {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let strides = Self::compute_strides(shape, DType::F32);
        Self {
            data: bytes,
            shape: shape.to_vec(),
            strides,
            dtype: DType::F32,
            offset: 0,
        }
    }

    pub fn from_i32(data: &[i32], shape: &[usize]) -> Self {
        let numel: usize = shape.iter().product();
        assert_eq!(data.len(), numel);
        let mut bytes = Vec::with_capacity(numel * 4);
        for &v in data {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let strides = Self::compute_strides(shape, DType::I32);
        Self {
            data: bytes,
            shape: shape.to_vec(),
            strides,
            dtype: DType::I32,
            offset: 0,
        }
    }

    fn compute_strides(shape: &[usize], dtype: DType) -> Vec<usize> {
        let mut strides = vec![0usize; shape.len()];
        if shape.is_empty() {
            return strides;
        }
        strides[shape.len() - 1] = dtype.size_bytes();
        for i in (0..shape.len() - 1).rev() {
            strides[i] = strides[i + 1] * shape[i + 1];
        }
        strides
    }

    pub fn shape(&self) -> &[usize] {
        &self.shape
    }

    pub fn strides(&self) -> &[usize] {
        &self.strides
    }

    pub fn dtype(&self) -> DType {
        self.dtype
    }

    pub fn ndim(&self) -> usize {
        self.shape.len()
    }

    pub fn numel(&self) -> usize {
        self.shape.iter().product()
    }

    pub fn byte_len(&self) -> usize {
        self.numel() * self.dtype.size_bytes()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data[self.offset..self.offset + self.byte_len()]
    }

    pub fn as_f32_slice(&self) -> &[f32] {
        assert_eq!(self.dtype, DType::F32);
        let ptr = self.data[self.offset..].as_ptr() as *const f32;
        unsafe { core::slice::from_raw_parts(ptr, self.numel()) }
    }

    pub fn as_f32_slice_mut(&mut self) -> &mut [f32] {
        assert_eq!(self.dtype, DType::F32);
        let numel = self.numel();
        let offset = self.offset;
        let ptr = self.data[offset..].as_mut_ptr() as *mut f32;
        unsafe { core::slice::from_raw_parts_mut(ptr, numel) }
    }

    pub fn get_f32(&self, indices: &[usize]) -> f32 {
        assert_eq!(self.dtype, DType::F32);
        let byte_offset = self.linear_offset(indices);
        let bytes = &self.data[byte_offset..byte_offset + 4];
        f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    pub fn set_f32(&mut self, indices: &[usize], value: f32) {
        assert_eq!(self.dtype, DType::F32);
        let byte_offset = self.linear_offset(indices);
        let bytes = value.to_le_bytes();
        self.data[byte_offset..byte_offset + 4].copy_from_slice(&bytes);
    }

    fn linear_offset(&self, indices: &[usize]) -> usize {
        assert_eq!(indices.len(), self.shape.len());
        let mut offset = self.offset;
        for (i, &idx) in indices.iter().enumerate() {
            assert!(idx < self.shape[i]);
            offset += idx * self.strides[i];
        }
        offset
    }

    pub fn reshape(&self, new_shape: &[usize]) -> Self {
        let new_numel: usize = new_shape.iter().product();
        assert_eq!(new_numel, self.numel());
        let strides = Self::compute_strides(new_shape, self.dtype);
        Self {
            data: self.data.clone(),
            shape: new_shape.to_vec(),
            strides,
            dtype: self.dtype,
            offset: self.offset,
        }
    }

    pub fn view(&self, new_shape: &[usize]) -> Self {
        self.reshape(new_shape)
    }

    pub fn transpose(&self, dim0: usize, dim1: usize) -> Self {
        let mut new_shape = self.shape.clone();
        let mut new_strides = self.strides.clone();
        new_shape.swap(dim0, dim1);
        new_strides.swap(dim0, dim1);
        Self {
            data: self.data.clone(),
            shape: new_shape,
            strides: new_strides,
            dtype: self.dtype,
            offset: self.offset,
        }
    }

    pub fn permute(&self, dims: &[usize]) -> Self {
        assert_eq!(dims.len(), self.ndim());
        let new_shape: Vec<usize> = dims.iter().map(|&d| self.shape[d]).collect();
        let new_strides: Vec<usize> = dims.iter().map(|&d| self.strides[d]).collect();
        Self {
            data: self.data.clone(),
            shape: new_shape,
            strides: new_strides,
            dtype: self.dtype,
            offset: self.offset,
        }
    }

    pub fn squeeze(&self, dim: usize) -> Self {
        assert!(dim < self.ndim());
        assert_eq!(self.shape[dim], 1);
        let mut new_shape = self.shape.clone();
        let mut new_strides = self.strides.clone();
        new_shape.remove(dim);
        new_strides.remove(dim);
        Self {
            data: self.data.clone(),
            shape: new_shape,
            strides: new_strides,
            dtype: self.dtype,
            offset: self.offset,
        }
    }

    pub fn unsqueeze(&self, dim: usize) -> Self {
        assert!(dim <= self.ndim());
        let mut new_shape = self.shape.clone();
        let mut new_strides = self.strides.clone();
        let stride_val = if dim < self.ndim() {
            self.strides[dim] * self.shape[dim]
        } else if self.ndim() > 0 {
            self.strides[self.ndim() - 1]
        } else {
            self.dtype.size_bytes()
        };
        new_shape.insert(dim, 1);
        new_strides.insert(dim, stride_val);
        Self {
            data: self.data.clone(),
            shape: new_shape,
            strides: new_strides,
            dtype: self.dtype,
            offset: self.offset,
        }
    }

    pub fn expand(&self, new_shape: &[usize]) -> Self {
        assert_eq!(new_shape.len(), self.ndim());
        let mut strides = self.strides.clone();
        for i in 0..self.ndim() {
            if self.shape[i] == 1 && new_shape[i] != 1 {
                strides[i] = 0;
            } else {
                assert_eq!(self.shape[i], new_shape[i]);
            }
        }
        Self {
            data: self.data.clone(),
            shape: new_shape.to_vec(),
            strides,
            dtype: self.dtype,
            offset: self.offset,
        }
    }

    pub fn contiguous(&self) -> Self {
        let numel = self.numel();
        let elem_size = self.dtype.size_bytes();
        let mut new_data = vec![0u8; numel * elem_size];
        let new_strides = Self::compute_strides(&self.shape, self.dtype);
        let ndim = self.ndim();
        let mut indices = vec![0usize; ndim];

        for flat in 0..numel {
            let src_offset = self.linear_offset(&indices);
            let dst_offset = flat * elem_size;
            new_data[dst_offset..dst_offset + elem_size]
                .copy_from_slice(&self.data[src_offset..src_offset + elem_size]);

            for d in (0..ndim).rev() {
                indices[d] += 1;
                if indices[d] < self.shape[d] {
                    break;
                }
                indices[d] = 0;
            }
        }

        Self {
            data: new_data,
            shape: self.shape.clone(),
            strides: new_strides,
            dtype: self.dtype,
            offset: 0,
        }
    }

    pub fn slice(&self, ranges: &[(usize, usize)]) -> Self {
        assert_eq!(ranges.len(), self.ndim());
        let new_shape: Vec<usize> = ranges.iter().map(|(s, e)| e - s).collect();
        let numel: usize = new_shape.iter().product();
        let elem_size = self.dtype.size_bytes();
        let mut new_data = vec![0u8; numel * elem_size];
        let new_strides = Self::compute_strides(&new_shape, self.dtype);
        let ndim = self.ndim();
        let mut indices = vec![0usize; ndim];

        for flat in 0..numel {
            let src_indices: Vec<usize> = indices
                .iter()
                .enumerate()
                .map(|(d, &i)| i + ranges[d].0)
                .collect();
            let src_offset = self.linear_offset(&src_indices);
            let dst_offset = flat * elem_size;
            new_data[dst_offset..dst_offset + elem_size]
                .copy_from_slice(&self.data[src_offset..src_offset + elem_size]);

            for d in (0..ndim).rev() {
                indices[d] += 1;
                if indices[d] < new_shape[d] {
                    break;
                }
                indices[d] = 0;
            }
        }

        Self {
            data: new_data,
            shape: new_shape,
            strides: new_strides,
            dtype: self.dtype,
            offset: 0,
        }
    }
}

// ─── Tensor Operations ──────────────────────────────────────────────────────

pub fn add(a: &Tensor, b: &Tensor) -> Tensor {
    assert_eq!(a.dtype, DType::F32);
    assert_eq!(b.dtype, DType::F32);
    assert_eq!(a.shape, b.shape);
    let a_data = a.as_f32_slice();
    let b_data = b.as_f32_slice();
    let result: Vec<f32> = a_data
        .iter()
        .zip(b_data.iter())
        .map(|(x, y)| x + y)
        .collect();
    Tensor::from_f32(&result, &a.shape)
}

pub fn sub(a: &Tensor, b: &Tensor) -> Tensor {
    assert_eq!(a.dtype, DType::F32);
    assert_eq!(b.dtype, DType::F32);
    assert_eq!(a.shape, b.shape);
    let a_data = a.as_f32_slice();
    let b_data = b.as_f32_slice();
    let result: Vec<f32> = a_data
        .iter()
        .zip(b_data.iter())
        .map(|(x, y)| x - y)
        .collect();
    Tensor::from_f32(&result, &a.shape)
}

pub fn mul(a: &Tensor, b: &Tensor) -> Tensor {
    assert_eq!(a.dtype, DType::F32);
    assert_eq!(b.dtype, DType::F32);
    assert_eq!(a.shape, b.shape);
    let a_data = a.as_f32_slice();
    let b_data = b.as_f32_slice();
    let result: Vec<f32> = a_data
        .iter()
        .zip(b_data.iter())
        .map(|(x, y)| x * y)
        .collect();
    Tensor::from_f32(&result, &a.shape)
}

pub fn div(a: &Tensor, b: &Tensor) -> Tensor {
    assert_eq!(a.dtype, DType::F32);
    assert_eq!(b.dtype, DType::F32);
    assert_eq!(a.shape, b.shape);
    let a_data = a.as_f32_slice();
    let b_data = b.as_f32_slice();
    let result: Vec<f32> = a_data
        .iter()
        .zip(b_data.iter())
        .map(|(x, y)| x / y)
        .collect();
    Tensor::from_f32(&result, &a.shape)
}

pub fn matmul(a: &Tensor, b: &Tensor) -> Tensor {
    assert_eq!(a.dtype, DType::F32);
    assert_eq!(b.dtype, DType::F32);
    assert_eq!(a.ndim(), 2);
    assert_eq!(b.ndim(), 2);
    let m = a.shape[0];
    let k = a.shape[1];
    assert_eq!(b.shape[0], k);
    let n = b.shape[1];

    let a_data = a.as_f32_slice();
    let b_data = b.as_f32_slice();
    let mut result = vec![0.0f32; m * n];

    for i in 0..m {
        for j in 0..n {
            let mut sum = 0.0f32;
            for p in 0..k {
                sum += a_data[i * k + p] * b_data[p * n + j];
            }
            result[i * n + j] = sum;
        }
    }

    Tensor::from_f32(&result, &[m, n])
}

pub fn dot(a: &Tensor, b: &Tensor) -> f32 {
    assert_eq!(a.dtype, DType::F32);
    assert_eq!(b.dtype, DType::F32);
    assert_eq!(a.ndim(), 1);
    assert_eq!(b.ndim(), 1);
    assert_eq!(a.shape[0], b.shape[0]);
    let a_data = a.as_f32_slice();
    let b_data = b.as_f32_slice();
    a_data.iter().zip(b_data.iter()).map(|(x, y)| x * y).sum()
}

pub fn conv2d(
    input: &Tensor,
    weight: &Tensor,
    bias: Option<&Tensor>,
    stride: usize,
    padding: usize,
) -> Tensor {
    assert_eq!(input.dtype, DType::F32);
    assert_eq!(input.ndim(), 4); // N, C_in, H, W
    assert_eq!(weight.ndim(), 4); // C_out, C_in, kH, kW

    let batch = input.shape[0];
    let _c_in = input.shape[1];
    let h_in = input.shape[2];
    let w_in = input.shape[3];
    let c_out = weight.shape[0];
    let c_in_w = weight.shape[1];
    let kh = weight.shape[2];
    let kw = weight.shape[3];

    assert_eq!(_c_in, c_in_w);

    let h_out = (h_in + 2 * padding - kh) / stride + 1;
    let w_out = (w_in + 2 * padding - kw) / stride + 1;

    let mut output = Tensor::zeros(&[batch, c_out, h_out, w_out], DType::F32);
    let in_data = input.as_f32_slice();
    let w_data = weight.as_f32_slice();
    let out_data = output.as_f32_slice_mut();

    for n in 0..batch {
        for oc in 0..c_out {
            for oh in 0..h_out {
                for ow in 0..w_out {
                    let mut sum = 0.0f32;
                    for ic in 0..c_in_w {
                        for fh in 0..kh {
                            for fw in 0..kw {
                                let ih = oh * stride + fh;
                                let iw = ow * stride + fw;
                                let ih = ih as isize - padding as isize;
                                let iw = iw as isize - padding as isize;
                                if ih >= 0 && ih < h_in as isize && iw >= 0 && iw < w_in as isize {
                                    let in_idx = n * _c_in * h_in * w_in
                                        + ic * h_in * w_in
                                        + (ih as usize) * w_in
                                        + iw as usize;
                                    let w_idx = oc * c_in_w * kh * kw + ic * kh * kw + fh * kw + fw;
                                    sum += in_data[in_idx] * w_data[w_idx];
                                }
                            }
                        }
                    }
                    if let Some(b) = bias {
                        sum += b.as_f32_slice()[oc];
                    }
                    let out_idx = n * c_out * h_out * w_out + oc * h_out * w_out + oh * w_out + ow;
                    out_data[out_idx] = sum;
                }
            }
        }
    }

    output
}

pub fn max_pool2d(input: &Tensor, kernel_size: usize, stride: usize, padding: usize) -> Tensor {
    assert_eq!(input.ndim(), 4);
    let batch = input.shape[0];
    let channels = input.shape[1];
    let h_in = input.shape[2];
    let w_in = input.shape[3];
    let h_out = (h_in + 2 * padding - kernel_size) / stride + 1;
    let w_out = (w_in + 2 * padding - kernel_size) / stride + 1;

    let mut output = Tensor::zeros(&[batch, channels, h_out, w_out], DType::F32);
    let in_data = input.as_f32_slice();
    let out_data = output.as_f32_slice_mut();

    for n in 0..batch {
        for c in 0..channels {
            for oh in 0..h_out {
                for ow in 0..w_out {
                    let mut max_val = f32::NEG_INFINITY;
                    for kh in 0..kernel_size {
                        for kw in 0..kernel_size {
                            let ih = (oh * stride + kh) as isize - padding as isize;
                            let iw = (ow * stride + kw) as isize - padding as isize;
                            if ih >= 0 && ih < h_in as isize && iw >= 0 && iw < w_in as isize {
                                let idx = n * channels * h_in * w_in
                                    + c * h_in * w_in
                                    + (ih as usize) * w_in
                                    + iw as usize;
                                if in_data[idx] > max_val {
                                    max_val = in_data[idx];
                                }
                            }
                        }
                    }
                    let out_idx =
                        n * channels * h_out * w_out + c * h_out * w_out + oh * w_out + ow;
                    out_data[out_idx] = max_val;
                }
            }
        }
    }

    output
}

pub fn avg_pool2d(input: &Tensor, kernel_size: usize, stride: usize, padding: usize) -> Tensor {
    assert_eq!(input.ndim(), 4);
    let batch = input.shape[0];
    let channels = input.shape[1];
    let h_in = input.shape[2];
    let w_in = input.shape[3];
    let h_out = (h_in + 2 * padding - kernel_size) / stride + 1;
    let w_out = (w_in + 2 * padding - kernel_size) / stride + 1;

    let mut output = Tensor::zeros(&[batch, channels, h_out, w_out], DType::F32);
    let in_data = input.as_f32_slice();
    let out_data = output.as_f32_slice_mut();

    for n in 0..batch {
        for c in 0..channels {
            for oh in 0..h_out {
                for ow in 0..w_out {
                    let mut sum = 0.0f32;
                    let mut count = 0u32;
                    for kh in 0..kernel_size {
                        for kw in 0..kernel_size {
                            let ih = (oh * stride + kh) as isize - padding as isize;
                            let iw = (ow * stride + kw) as isize - padding as isize;
                            if ih >= 0 && ih < h_in as isize && iw >= 0 && iw < w_in as isize {
                                let idx = n * channels * h_in * w_in
                                    + c * h_in * w_in
                                    + (ih as usize) * w_in
                                    + iw as usize;
                                sum += in_data[idx];
                                count += 1;
                            }
                        }
                    }
                    let out_idx =
                        n * channels * h_out * w_out + c * h_out * w_out + oh * w_out + ow;
                    out_data[out_idx] = if count > 0 { sum / count as f32 } else { 0.0 };
                }
            }
        }
    }

    output
}

pub fn global_avg_pool2d(input: &Tensor) -> Tensor {
    assert_eq!(input.ndim(), 4);
    let batch = input.shape[0];
    let channels = input.shape[1];
    let h = input.shape[2];
    let w = input.shape[3];
    let spatial = h * w;

    let mut output = Tensor::zeros(&[batch, channels, 1, 1], DType::F32);
    let in_data = input.as_f32_slice();
    let out_data = output.as_f32_slice_mut();

    for n in 0..batch {
        for c in 0..channels {
            let mut sum = 0.0f32;
            for s in 0..spatial {
                sum += in_data[n * channels * spatial + c * spatial + s];
            }
            out_data[n * channels + c] = sum / spatial as f32;
        }
    }

    output
}

pub fn batch_norm(
    input: &Tensor,
    gamma: &Tensor,
    beta: &Tensor,
    running_mean: &Tensor,
    running_var: &Tensor,
    epsilon: f32,
) -> Tensor {
    assert_eq!(input.ndim(), 4);
    let batch = input.shape[0];
    let channels = input.shape[1];
    let h = input.shape[2];
    let w = input.shape[3];

    let mut output = Tensor::zeros(&input.shape, DType::F32);
    let in_data = input.as_f32_slice();
    let out_data = output.as_f32_slice_mut();
    let g = gamma.as_f32_slice();
    let b = beta.as_f32_slice();
    let mean = running_mean.as_f32_slice();
    let var = running_var.as_f32_slice();

    for n in 0..batch {
        for c in 0..channels {
            let inv_std = 1.0 / sqrt_f32(var[c] + epsilon);
            for i in 0..h * w {
                let idx = n * channels * h * w + c * h * w + i;
                out_data[idx] = g[c] * (in_data[idx] - mean[c]) * inv_std + b[c];
            }
        }
    }

    output
}

pub fn layer_norm(
    input: &Tensor,
    normalized_shape: &[usize],
    gamma: &Tensor,
    beta: &Tensor,
    epsilon: f32,
) -> Tensor {
    let norm_size: usize = normalized_shape.iter().product();
    let outer_size = input.numel() / norm_size;
    let in_data = input.as_f32_slice();
    let mut result = vec![0.0f32; input.numel()];
    let g = gamma.as_f32_slice();
    let b = beta.as_f32_slice();

    for i in 0..outer_size {
        let offset = i * norm_size;
        let slice = &in_data[offset..offset + norm_size];

        let mean: f32 = slice.iter().sum::<f32>() / norm_size as f32;
        let var: f32 =
            slice.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / norm_size as f32;
        let inv_std = 1.0 / sqrt_f32(var + epsilon);

        for j in 0..norm_size {
            result[offset + j] = g[j] * (slice[j] - mean) * inv_std + b[j];
        }
    }

    Tensor::from_f32(&result, &input.shape)
}

pub fn group_norm(
    input: &Tensor,
    num_groups: usize,
    gamma: &Tensor,
    beta: &Tensor,
    epsilon: f32,
) -> Tensor {
    assert_eq!(input.ndim(), 4);
    let batch = input.shape[0];
    let channels = input.shape[1];
    let h = input.shape[2];
    let w = input.shape[3];
    let channels_per_group = channels / num_groups;

    let in_data = input.as_f32_slice();
    let mut result = vec![0.0f32; input.numel()];
    let g = gamma.as_f32_slice();
    let b = beta.as_f32_slice();

    for n in 0..batch {
        for group in 0..num_groups {
            let c_start = group * channels_per_group;
            let group_size = channels_per_group * h * w;

            let mut mean = 0.0f32;
            for c in c_start..c_start + channels_per_group {
                for s in 0..h * w {
                    mean += in_data[n * channels * h * w + c * h * w + s];
                }
            }
            mean /= group_size as f32;

            let mut var = 0.0f32;
            for c in c_start..c_start + channels_per_group {
                for s in 0..h * w {
                    let v = in_data[n * channels * h * w + c * h * w + s] - mean;
                    var += v * v;
                }
            }
            var /= group_size as f32;
            let inv_std = 1.0 / sqrt_f32(var + epsilon);

            for c in c_start..c_start + channels_per_group {
                for s in 0..h * w {
                    let idx = n * channels * h * w + c * h * w + s;
                    result[idx] = g[c] * (in_data[idx] - mean) * inv_std + b[c];
                }
            }
        }
    }

    Tensor::from_f32(&result, &input.shape)
}

pub fn instance_norm(input: &Tensor, gamma: &Tensor, beta: &Tensor, epsilon: f32) -> Tensor {
    group_norm(input, input.shape[1], gamma, beta, epsilon)
}

pub fn linear(input: &Tensor, weight: &Tensor, bias: Option<&Tensor>) -> Tensor {
    assert_eq!(input.dtype, DType::F32);
    let in_features = weight.shape[1];
    let out_features = weight.shape[0];
    let batch_size = input.numel() / in_features;

    let in_data = input.as_f32_slice();
    let w_data = weight.as_f32_slice();
    let mut result = vec![0.0f32; batch_size * out_features];

    for b in 0..batch_size {
        for o in 0..out_features {
            let mut sum = 0.0f32;
            for i in 0..in_features {
                sum += in_data[b * in_features + i] * w_data[o * in_features + i];
            }
            if let Some(bias_t) = bias {
                sum += bias_t.as_f32_slice()[o];
            }
            result[b * out_features + o] = sum;
        }
    }

    let out_shape = if input.ndim() == 1 {
        vec![out_features]
    } else {
        let mut s = input.shape[..input.ndim() - 1].to_vec();
        s.push(out_features);
        s
    };
    Tensor::from_f32(&result, &out_shape)
}

pub fn embedding(weight: &Tensor, indices: &[u32]) -> Tensor {
    assert_eq!(weight.ndim(), 2);
    let embed_dim = weight.shape[1];
    let w_data = weight.as_f32_slice();
    let mut result = vec![0.0f32; indices.len() * embed_dim];

    for (i, &idx) in indices.iter().enumerate() {
        let src_offset = idx as usize * embed_dim;
        result[i * embed_dim..(i + 1) * embed_dim]
            .copy_from_slice(&w_data[src_offset..src_offset + embed_dim]);
    }

    Tensor::from_f32(&result, &[indices.len(), embed_dim])
}

pub fn softmax(input: &Tensor, dim: usize) -> Tensor {
    let in_data = input.as_f32_slice();
    let mut result = vec![0.0f32; input.numel()];
    let dim_size = input.shape[dim];
    let outer: usize = input.shape[..dim].iter().product();
    let inner: usize = input.shape[dim + 1..].iter().product();

    for o in 0..outer {
        for i in 0..inner {
            let mut max_val = f32::NEG_INFINITY;
            for d in 0..dim_size {
                let idx = o * dim_size * inner + d * inner + i;
                if in_data[idx] > max_val {
                    max_val = in_data[idx];
                }
            }
            let mut sum = 0.0f32;
            for d in 0..dim_size {
                let idx = o * dim_size * inner + d * inner + i;
                let v = exp_f32(in_data[idx] - max_val);
                result[idx] = v;
                sum += v;
            }
            for d in 0..dim_size {
                let idx = o * dim_size * inner + d * inner + i;
                result[idx] /= sum;
            }
        }
    }

    Tensor::from_f32(&result, &input.shape)
}

pub fn log_softmax(input: &Tensor, dim: usize) -> Tensor {
    let s = softmax(input, dim);
    let data = s.as_f32_slice();
    let result: Vec<f32> = data.iter().map(|&x| ln_f32(x)).collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn relu(input: &Tensor) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data
        .iter()
        .map(|&x| if x > 0.0 { x } else { 0.0 })
        .collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn gelu(input: &Tensor) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data
        .iter()
        .map(|&x| 0.5 * x * (1.0 + tanh_f32(0.7978845608 * (x + 0.044715 * x * x * x))))
        .collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn silu(input: &Tensor) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data.iter().map(|&x| x * sigmoid_f32(x)).collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn sigmoid(input: &Tensor) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data.iter().map(|&x| sigmoid_f32(x)).collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn tanh(input: &Tensor) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data.iter().map(|&x| tanh_f32(x)).collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn leaky_relu(input: &Tensor, negative_slope: f32) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data
        .iter()
        .map(|&x| if x > 0.0 { x } else { negative_slope * x })
        .collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn elu(input: &Tensor, alpha: f32) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data
        .iter()
        .map(|&x| {
            if x > 0.0 {
                x
            } else {
                alpha * (exp_f32(x) - 1.0)
            }
        })
        .collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn selu(input: &Tensor) -> Tensor {
    const ALPHA: f32 = 1.6732632;
    const SCALE: f32 = 1.0507010;
    let data = input.as_f32_slice();
    let result: Vec<f32> = data
        .iter()
        .map(|&x| {
            SCALE
                * if x > 0.0 {
                    x
                } else {
                    ALPHA * (exp_f32(x) - 1.0)
                }
        })
        .collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn mish(input: &Tensor) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data
        .iter()
        .map(|&x| x * tanh_f32(ln_f32(1.0 + exp_f32(x))))
        .collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn hardswish(input: &Tensor) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data
        .iter()
        .map(|&x| {
            if x <= -3.0 {
                0.0
            } else if x >= 3.0 {
                x
            } else {
                x * (x + 3.0) / 6.0
            }
        })
        .collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn hardtanh(input: &Tensor, min_val: f32, max_val: f32) -> Tensor {
    let data = input.as_f32_slice();
    let result: Vec<f32> = data.iter().map(|&x| x.max(min_val).min(max_val)).collect();
    Tensor::from_f32(&result, &input.shape)
}

pub fn clamp(input: &Tensor, min_val: f32, max_val: f32) -> Tensor {
    hardtanh(input, min_val, max_val)
}

pub fn dropout(input: &Tensor, _p: f32, training: bool) -> Tensor {
    if !training {
        return input.clone();
    }
    input.clone()
}

pub fn concat(tensors: &[&Tensor], dim: usize) -> Tensor {
    assert!(!tensors.is_empty());
    let _ndim = tensors[0].ndim();
    let mut out_shape = tensors[0].shape.clone();
    out_shape[dim] = tensors.iter().map(|t| t.shape[dim]).sum();

    let total_numel: usize = out_shape.iter().product();
    let mut result = vec![0.0f32; total_numel];

    let outer: usize = out_shape[..dim].iter().product();
    let inner: usize = out_shape[dim + 1..].iter().product();

    let mut dim_offset = 0;
    for t in tensors {
        let t_data = t.as_f32_slice();
        let t_dim = t.shape[dim];
        for o in 0..outer {
            for d in 0..t_dim {
                for i in 0..inner {
                    let src_idx = o * t_dim * inner + d * inner + i;
                    let dst_idx = o * out_shape[dim] * inner + (dim_offset + d) * inner + i;
                    result[dst_idx] = t_data[src_idx];
                }
            }
        }
        dim_offset += t_dim;
    }

    Tensor::from_f32(&result, &out_shape)
}

pub fn stack(tensors: &[&Tensor], dim: usize) -> Tensor {
    let expanded: Vec<Tensor> = tensors.iter().map(|t| t.unsqueeze(dim)).collect();
    let refs: Vec<&Tensor> = expanded.iter().collect();
    concat(&refs, dim)
}

pub fn split(input: &Tensor, split_size: usize, dim: usize) -> Vec<Tensor> {
    let dim_size = input.shape[dim];
    let mut results = Vec::new();
    let mut start = 0;
    while start < dim_size {
        let end = (start + split_size).min(dim_size);
        let ranges: Vec<(usize, usize)> = input
            .shape
            .iter()
            .enumerate()
            .map(|(d, &s)| if d == dim { (start, end) } else { (0, s) })
            .collect();
        results.push(input.slice(&ranges));
        start = end;
    }
    results
}

pub fn chunk(input: &Tensor, chunks: usize, dim: usize) -> Vec<Tensor> {
    let dim_size = input.shape[dim];
    let chunk_size = (dim_size + chunks - 1) / chunks;
    split(input, chunk_size, dim)
}

pub fn topk(input: &Tensor, k: usize, _dim: usize) -> (Tensor, Tensor) {
    assert_eq!(input.ndim(), 1);
    let data = input.as_f32_slice();
    let mut indexed: Vec<(f32, usize)> = data
        .iter()
        .copied()
        .enumerate()
        .map(|(i, v)| (v, i))
        .collect();
    indexed.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(core::cmp::Ordering::Equal));
    let values: Vec<f32> = indexed[..k].iter().map(|(v, _)| *v).collect();
    let indices: Vec<i32> = indexed[..k].iter().map(|(_, i)| *i as i32).collect();
    (
        Tensor::from_f32(&values, &[k]),
        Tensor::from_i32(&indices, &[k]),
    )
}

pub fn argmax(input: &Tensor, dim: usize) -> Tensor {
    let data = input.as_f32_slice();
    let dim_size = input.shape[dim];
    let outer: usize = input.shape[..dim].iter().product();
    let inner: usize = input.shape[dim + 1..].iter().product();
    let mut result = vec![0i32; outer * inner];

    for o in 0..outer {
        for i in 0..inner {
            let mut max_val = f32::NEG_INFINITY;
            let mut max_idx = 0i32;
            for d in 0..dim_size {
                let idx = o * dim_size * inner + d * inner + i;
                if data[idx] > max_val {
                    max_val = data[idx];
                    max_idx = d as i32;
                }
            }
            result[o * inner + i] = max_idx;
        }
    }

    let mut out_shape = input.shape.clone();
    out_shape.remove(dim);
    if out_shape.is_empty() {
        out_shape.push(1);
    }
    Tensor::from_i32(&result, &out_shape)
}

pub fn argmin(input: &Tensor, dim: usize) -> Tensor {
    let data = input.as_f32_slice();
    let dim_size = input.shape[dim];
    let outer: usize = input.shape[..dim].iter().product();
    let inner: usize = input.shape[dim + 1..].iter().product();
    let mut result = vec![0i32; outer * inner];

    for o in 0..outer {
        for i in 0..inner {
            let mut min_val = f32::INFINITY;
            let mut min_idx = 0i32;
            for d in 0..dim_size {
                let idx = o * dim_size * inner + d * inner + i;
                if data[idx] < min_val {
                    min_val = data[idx];
                    min_idx = d as i32;
                }
            }
            result[o * inner + i] = min_idx;
        }
    }

    let mut out_shape = input.shape.clone();
    out_shape.remove(dim);
    if out_shape.is_empty() {
        out_shape.push(1);
    }
    Tensor::from_i32(&result, &out_shape)
}

pub fn sum(input: &Tensor, dim: usize) -> Tensor {
    let data = input.as_f32_slice();
    let dim_size = input.shape[dim];
    let outer: usize = input.shape[..dim].iter().product();
    let inner: usize = input.shape[dim + 1..].iter().product();
    let mut result = vec![0.0f32; outer * inner];

    for o in 0..outer {
        for i in 0..inner {
            let mut s = 0.0f32;
            for d in 0..dim_size {
                s += data[o * dim_size * inner + d * inner + i];
            }
            result[o * inner + i] = s;
        }
    }

    let mut out_shape = input.shape.clone();
    out_shape.remove(dim);
    if out_shape.is_empty() {
        out_shape.push(1);
    }
    Tensor::from_f32(&result, &out_shape)
}

pub fn mean(input: &Tensor, dim: usize) -> Tensor {
    let s = sum(input, dim);
    let dim_size = input.shape[dim] as f32;
    let data = s.as_f32_slice();
    let result: Vec<f32> = data.iter().map(|&x| x / dim_size).collect();
    Tensor::from_f32(&result, &s.shape)
}

pub fn cumsum(input: &Tensor, dim: usize) -> Tensor {
    let data = input.as_f32_slice();
    let dim_size = input.shape[dim];
    let outer: usize = input.shape[..dim].iter().product();
    let inner: usize = input.shape[dim + 1..].iter().product();
    let mut result = vec![0.0f32; input.numel()];

    for o in 0..outer {
        for i in 0..inner {
            let mut running = 0.0f32;
            for d in 0..dim_size {
                let idx = o * dim_size * inner + d * inner + i;
                running += data[idx];
                result[idx] = running;
            }
        }
    }

    Tensor::from_f32(&result, &input.shape)
}

// ─── Math Helpers ───────────────────────────────────────────────────────────

fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x;
    for _ in 0..20 {
        guess = 0.5 * (guess + x / guess);
    }
    guess
}

fn exp_f32(x: f32) -> f32 {
    let x = x.max(-88.0).min(88.0);
    let mut result = 1.0f32;
    let mut term = 1.0f32;
    for i in 1..30 {
        term *= x / i as f32;
        result += term;
        if term.abs() < 1e-7 {
            break;
        }
    }
    result
}

fn ln_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return f32::NEG_INFINITY;
    }
    let y = (x - 1.0) / (x + 1.0);
    let y2 = y * y;
    let mut result = 0.0f32;
    let mut power = y;
    for i in 0..30 {
        result += power / (2 * i + 1) as f32;
        power *= y2;
    }
    2.0 * result
}

fn tanh_f32(x: f32) -> f32 {
    let e2x = exp_f32(2.0 * x);
    (e2x - 1.0) / (e2x + 1.0)
}

fn sigmoid_f32(x: f32) -> f32 {
    1.0 / (1.0 + exp_f32(-x))
}

// ─── ONNX Model Format ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum OnnxOpType {
    Conv,
    Relu,
    MaxPool,
    AveragePool,
    GlobalAveragePool,
    Add,
    MatMul,
    Gemm,
    Reshape,
    Flatten,
    Transpose,
    Concat,
    Softmax,
    BatchNormalization,
    Dropout,
    Sigmoid,
    Tanh,
    LeakyRelu,
    Pad,
    Resize,
    Unsqueeze,
    Squeeze,
    Gather,
    Slice,
    Shape,
    ReduceMean,
    ReduceSum,
    ReduceMax,
    Cast,
    Clip,
    Constant,
    ConstantOfShape,
    Expand,
    Tile,
    Where,
    Split,
    TopK,
    ArgMax,
    ArgMin,
    Pow,
    Sqrt,
    Reciprocal,
    Neg,
    Abs,
    Log,
    Exp,
    Ceil,
    Floor,
    Round,
    Erf,
    LayerNormalization,
    GroupNormalization,
    Attention,
    MultiHeadAttention,
    RotaryEmbedding,
}

#[derive(Clone)]
pub struct OnnxAttribute {
    pub name: String,
    pub int_val: i64,
    pub float_val: f32,
    pub ints: Vec<i64>,
    pub floats: Vec<f32>,
    pub string_val: String,
}

impl OnnxAttribute {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            int_val: 0,
            float_val: 0.0,
            ints: Vec::new(),
            floats: Vec::new(),
            string_val: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct OnnxNode {
    pub op_type: OnnxOpType,
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub attributes: Vec<OnnxAttribute>,
}

#[derive(Clone)]
pub struct OnnxTensorInfo {
    pub name: String,
    pub shape: Vec<usize>,
    pub dtype: DType,
}

#[derive(Clone)]
pub struct OnnxInitializer {
    pub name: String,
    pub tensor: Tensor,
}

#[derive(Clone)]
pub struct OnnxModel {
    pub opset_version: u32,
    pub nodes: Vec<OnnxNode>,
    pub inputs: Vec<OnnxTensorInfo>,
    pub outputs: Vec<OnnxTensorInfo>,
    pub initializers: Vec<OnnxInitializer>,
}

impl OnnxModel {
    pub fn new() -> Self {
        Self {
            opset_version: 13,
            nodes: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            initializers: Vec::new(),
        }
    }

    pub fn load_from_bytes(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 8 {
            return Err("model data too small");
        }
        let mut model = Self::new();
        model.opset_version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let num_nodes = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

        let mut offset = 8;
        for _ in 0..num_nodes {
            if offset + 4 > data.len() {
                break;
            }
            let op_code = u16::from_le_bytes([data[offset], data[offset + 1]]);
            let name_len = u16::from_le_bytes([data[offset + 2], data[offset + 3]]) as usize;
            offset += 4;

            let name = if offset + name_len <= data.len() {
                let s = core::str::from_utf8(&data[offset..offset + name_len]).unwrap_or("");
                offset += name_len;
                String::from(s)
            } else {
                String::new()
            };

            let op_type = Self::op_from_code(op_code);
            model.nodes.push(OnnxNode {
                op_type,
                name,
                inputs: Vec::new(),
                outputs: Vec::new(),
                attributes: Vec::new(),
            });
        }

        Ok(model)
    }

    fn op_from_code(code: u16) -> OnnxOpType {
        match code {
            0 => OnnxOpType::Conv,
            1 => OnnxOpType::Relu,
            2 => OnnxOpType::MaxPool,
            3 => OnnxOpType::AveragePool,
            4 => OnnxOpType::GlobalAveragePool,
            5 => OnnxOpType::Add,
            6 => OnnxOpType::MatMul,
            7 => OnnxOpType::Gemm,
            8 => OnnxOpType::Reshape,
            9 => OnnxOpType::Flatten,
            10 => OnnxOpType::Transpose,
            11 => OnnxOpType::Concat,
            12 => OnnxOpType::Softmax,
            13 => OnnxOpType::BatchNormalization,
            14 => OnnxOpType::Dropout,
            15 => OnnxOpType::Sigmoid,
            16 => OnnxOpType::Tanh,
            17 => OnnxOpType::LeakyRelu,
            18 => OnnxOpType::Pad,
            19 => OnnxOpType::Resize,
            20 => OnnxOpType::Unsqueeze,
            21 => OnnxOpType::Squeeze,
            22 => OnnxOpType::Gather,
            23 => OnnxOpType::Slice,
            24 => OnnxOpType::Shape,
            25 => OnnxOpType::ReduceMean,
            26 => OnnxOpType::ReduceSum,
            27 => OnnxOpType::ReduceMax,
            28 => OnnxOpType::Cast,
            29 => OnnxOpType::Clip,
            30 => OnnxOpType::Constant,
            31 => OnnxOpType::ConstantOfShape,
            32 => OnnxOpType::Expand,
            33 => OnnxOpType::Tile,
            34 => OnnxOpType::Where,
            35 => OnnxOpType::Split,
            36 => OnnxOpType::TopK,
            37 => OnnxOpType::ArgMax,
            38 => OnnxOpType::ArgMin,
            39 => OnnxOpType::Pow,
            40 => OnnxOpType::Sqrt,
            41 => OnnxOpType::Reciprocal,
            42 => OnnxOpType::Neg,
            43 => OnnxOpType::Abs,
            44 => OnnxOpType::Log,
            45 => OnnxOpType::Exp,
            46 => OnnxOpType::Ceil,
            47 => OnnxOpType::Floor,
            48 => OnnxOpType::Round,
            49 => OnnxOpType::Erf,
            50 => OnnxOpType::LayerNormalization,
            51 => OnnxOpType::GroupNormalization,
            52 => OnnxOpType::Attention,
            53 => OnnxOpType::MultiHeadAttention,
            54 => OnnxOpType::RotaryEmbedding,
            _ => OnnxOpType::Relu,
        }
    }
}

// ─── Graph Executor ─────────────────────────────────────────────────────────

pub struct GraphExecutor {
    model: OnnxModel,
    execution_order: Vec<usize>,
    tensor_map: Vec<(String, Tensor)>,
    memory_plan: MemoryPlan,
}

struct MemoryPlan {
    tensor_lifetimes: Vec<(usize, usize)>,
    arena_size: usize,
    tensor_offsets: Vec<usize>,
}

impl MemoryPlan {
    fn new() -> Self {
        Self {
            tensor_lifetimes: Vec::new(),
            arena_size: 0,
            tensor_offsets: Vec::new(),
        }
    }

    fn plan_for_graph(num_nodes: usize) -> Self {
        let mut plan = Self::new();
        for i in 0..num_nodes {
            plan.tensor_lifetimes.push((i, i + 1));
            plan.tensor_offsets.push(i * 4096);
        }
        plan.arena_size = num_nodes * 4096;
        plan
    }
}

impl GraphExecutor {
    pub fn new(model: OnnxModel) -> Self {
        let num_nodes = model.nodes.len();
        let execution_order = Self::topological_sort(&model);
        let memory_plan = MemoryPlan::plan_for_graph(num_nodes);
        Self {
            model,
            execution_order,
            tensor_map: Vec::new(),
            memory_plan,
        }
    }

    fn topological_sort(model: &OnnxModel) -> Vec<usize> {
        let n = model.nodes.len();
        let mut in_degree = vec![0u32; n];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];

        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                for out in &model.nodes[i].outputs {
                    if model.nodes[j].inputs.contains(out) {
                        adj[i].push(j);
                        in_degree[j] += 1;
                    }
                }
            }
        }

        let mut queue: Vec<usize> = Vec::new();
        for i in 0..n {
            if in_degree[i] == 0 {
                queue.push(i);
            }
        }

        let mut order = Vec::new();
        while let Some(node) = queue.pop() {
            order.push(node);
            for &next in &adj[node] {
                in_degree[next] -= 1;
                if in_degree[next] == 0 {
                    queue.push(next);
                }
            }
        }

        if order.len() < n {
            (0..n).collect()
        } else {
            order
        }
    }

    pub fn execute(&mut self, inputs: &[(String, Tensor)]) -> Vec<Tensor> {
        self.tensor_map.clear();
        for (name, tensor) in inputs {
            self.tensor_map.push((name.clone(), tensor.clone()));
        }
        for init in &self.model.initializers {
            self.tensor_map
                .push((init.name.clone(), init.tensor.clone()));
        }

        for &node_idx in &self.execution_order {
            if node_idx >= self.model.nodes.len() {
                continue;
            }
            let node = &self.model.nodes[node_idx];
            let result = self.execute_node(node);
            if let Some(output) = result {
                if !node.outputs.is_empty() {
                    self.tensor_map.push((node.outputs[0].clone(), output));
                }
            }
        }

        let mut outputs = Vec::new();
        for out_info in &self.model.outputs {
            for (name, tensor) in &self.tensor_map {
                if name == &out_info.name {
                    outputs.push(tensor.clone());
                    break;
                }
            }
        }
        outputs
    }

    fn get_tensor(&self, name: &str) -> Option<&Tensor> {
        for (n, t) in self.tensor_map.iter().rev() {
            if n == name {
                return Some(t);
            }
        }
        None
    }

    fn execute_node(&self, node: &OnnxNode) -> Option<Tensor> {
        match node.op_type {
            OnnxOpType::Relu => {
                let input = self.get_tensor(&node.inputs[0])?;
                Some(relu(input))
            }
            OnnxOpType::Sigmoid => {
                let input = self.get_tensor(&node.inputs[0])?;
                Some(sigmoid(input))
            }
            OnnxOpType::Tanh => {
                let input = self.get_tensor(&node.inputs[0])?;
                Some(tanh(input))
            }
            OnnxOpType::Add => {
                let a = self.get_tensor(&node.inputs[0])?;
                let b = self.get_tensor(&node.inputs[1])?;
                Some(add(a, b))
            }
            OnnxOpType::MatMul => {
                let a = self.get_tensor(&node.inputs[0])?;
                let b = self.get_tensor(&node.inputs[1])?;
                Some(matmul(a, b))
            }
            OnnxOpType::Softmax => {
                let input = self.get_tensor(&node.inputs[0])?;
                let axis = node
                    .attributes
                    .iter()
                    .find(|a| a.name == "axis")
                    .map(|a| a.int_val as usize)
                    .unwrap_or(input.ndim() - 1);
                Some(softmax(input, axis))
            }
            OnnxOpType::Reshape => {
                let input = self.get_tensor(&node.inputs[0])?;
                Some(input.clone())
            }
            OnnxOpType::Flatten => {
                let input = self.get_tensor(&node.inputs[0])?;
                let total = input.numel();
                Some(input.reshape(&[1, total]))
            }
            OnnxOpType::Transpose => {
                let input = self.get_tensor(&node.inputs[0])?;
                if input.ndim() == 2 {
                    Some(input.transpose(0, 1))
                } else {
                    Some(input.clone())
                }
            }
            OnnxOpType::Neg => {
                let input = self.get_tensor(&node.inputs[0])?;
                let data = input.as_f32_slice();
                let result: Vec<f32> = data.iter().map(|&x| -x).collect();
                Some(Tensor::from_f32(&result, &input.shape))
            }
            OnnxOpType::Abs => {
                let input = self.get_tensor(&node.inputs[0])?;
                let data = input.as_f32_slice();
                let result: Vec<f32> = data.iter().map(|&x| if x < 0.0 { -x } else { x }).collect();
                Some(Tensor::from_f32(&result, &input.shape))
            }
            OnnxOpType::Sqrt => {
                let input = self.get_tensor(&node.inputs[0])?;
                let data = input.as_f32_slice();
                let result: Vec<f32> = data.iter().map(|&x| sqrt_f32(x)).collect();
                Some(Tensor::from_f32(&result, &input.shape))
            }
            OnnxOpType::Exp => {
                let input = self.get_tensor(&node.inputs[0])?;
                let data = input.as_f32_slice();
                let result: Vec<f32> = data.iter().map(|&x| exp_f32(x)).collect();
                Some(Tensor::from_f32(&result, &input.shape))
            }
            OnnxOpType::Log => {
                let input = self.get_tensor(&node.inputs[0])?;
                let data = input.as_f32_slice();
                let result: Vec<f32> = data.iter().map(|&x| ln_f32(x)).collect();
                Some(Tensor::from_f32(&result, &input.shape))
            }
            OnnxOpType::Reciprocal => {
                let input = self.get_tensor(&node.inputs[0])?;
                let data = input.as_f32_slice();
                let result: Vec<f32> = data.iter().map(|&x| 1.0 / x).collect();
                Some(Tensor::from_f32(&result, &input.shape))
            }
            OnnxOpType::LeakyRelu => {
                let input = self.get_tensor(&node.inputs[0])?;
                let alpha = node
                    .attributes
                    .iter()
                    .find(|a| a.name == "alpha")
                    .map(|a| a.float_val)
                    .unwrap_or(0.01);
                Some(leaky_relu(input, alpha))
            }
            OnnxOpType::Clip => {
                let input = self.get_tensor(&node.inputs[0])?;
                let min_v = node
                    .attributes
                    .iter()
                    .find(|a| a.name == "min")
                    .map(|a| a.float_val)
                    .unwrap_or(f32::NEG_INFINITY);
                let max_v = node
                    .attributes
                    .iter()
                    .find(|a| a.name == "max")
                    .map(|a| a.float_val)
                    .unwrap_or(f32::INFINITY);
                Some(clamp(input, min_v, max_v))
            }
            _ => {
                if !node.inputs.is_empty() {
                    self.get_tensor(&node.inputs[0]).cloned()
                } else {
                    None
                }
            }
        }
    }
}

// ─── Quantization ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizationMode {
    PerTensor,
    PerChannel,
}

#[derive(Clone)]
pub struct QuantizedTensor {
    pub data: Vec<i8>,
    pub shape: Vec<usize>,
    pub scale: Vec<f32>,
    pub zero_point: Vec<i8>,
    pub mode: QuantizationMode,
    pub channel_axis: usize,
}

pub fn quantize_tensor(
    input: &Tensor,
    mode: QuantizationMode,
    channel_axis: usize,
) -> QuantizedTensor {
    let data = input.as_f32_slice();

    match mode {
        QuantizationMode::PerTensor => {
            let min_val = data.iter().cloned().fold(f32::INFINITY, f32::min);
            let max_val = data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let scale = (max_val - min_val) / 255.0;
            let zero_point = ((-min_val / scale) - 128.0) as i8;

            let quantized: Vec<i8> = data
                .iter()
                .map(|&x| {
                    let q = (x / scale) as i32 + zero_point as i32;
                    q.max(-128).min(127) as i8
                })
                .collect();

            QuantizedTensor {
                data: quantized,
                shape: input.shape.clone(),
                scale: vec![scale],
                zero_point: vec![zero_point],
                mode,
                channel_axis,
            }
        }
        QuantizationMode::PerChannel => {
            let num_channels = input.shape[channel_axis];
            let channel_size = input.numel() / num_channels;
            let mut scales = Vec::with_capacity(num_channels);
            let mut zero_points = Vec::with_capacity(num_channels);
            let mut quantized = vec![0i8; input.numel()];

            for ch in 0..num_channels {
                let offset = ch * channel_size;
                let channel_data = &data[offset..offset + channel_size];

                let min_val = channel_data.iter().cloned().fold(f32::INFINITY, f32::min);
                let max_val = channel_data
                    .iter()
                    .cloned()
                    .fold(f32::NEG_INFINITY, f32::max);
                let scale = (max_val - min_val).max(1e-8) / 255.0;
                let zp = ((-min_val / scale) - 128.0) as i8;

                scales.push(scale);
                zero_points.push(zp);

                for (i, &x) in channel_data.iter().enumerate() {
                    let q = (x / scale) as i32 + zp as i32;
                    quantized[offset + i] = q.max(-128).min(127) as i8;
                }
            }

            QuantizedTensor {
                data: quantized,
                shape: input.shape.clone(),
                scale: scales,
                zero_point: zero_points,
                mode,
                channel_axis,
            }
        }
    }
}

pub fn dequantize_tensor(qt: &QuantizedTensor) -> Tensor {
    let numel: usize = qt.shape.iter().product();
    let mut result = vec![0.0f32; numel];

    match qt.mode {
        QuantizationMode::PerTensor => {
            let scale = qt.scale[0];
            let zp = qt.zero_point[0] as i32;
            for (i, &q) in qt.data.iter().enumerate() {
                result[i] = (q as i32 - zp) as f32 * scale;
            }
        }
        QuantizationMode::PerChannel => {
            let num_channels = qt.shape[qt.channel_axis];
            let channel_size = numel / num_channels;
            for ch in 0..num_channels {
                let scale = qt.scale[ch];
                let zp = qt.zero_point[ch] as i32;
                let offset = ch * channel_size;
                for i in 0..channel_size {
                    result[offset + i] = (qt.data[offset + i] as i32 - zp) as f32 * scale;
                }
            }
        }
    }

    Tensor::from_f32(&result, &qt.shape)
}

pub fn quantized_matmul(a: &QuantizedTensor, b: &QuantizedTensor) -> Tensor {
    let a_deq = dequantize_tensor(a);
    let b_deq = dequantize_tensor(b);
    matmul(&a_deq, &b_deq)
}

// ─── Memory Management / Tensor Arena ───────────────────────────────────────

pub struct TensorArena {
    buffer: Vec<u8>,
    capacity: usize,
    allocations: Vec<ArenaAllocation>,
    free_list: Vec<(usize, usize)>,
}

struct ArenaAllocation {
    offset: usize,
    size: usize,
    tensor_id: u32,
    in_use: bool,
}

impl TensorArena {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0u8; capacity],
            capacity,
            allocations: Vec::new(),
            free_list: vec![(0, capacity)],
        }
    }

    pub fn allocate(&mut self, size: usize, tensor_id: u32) -> Option<usize> {
        let aligned_size = (size + 63) & !63;

        for i in 0..self.free_list.len() {
            let (offset, free_size) = self.free_list[i];
            if free_size >= aligned_size {
                self.free_list.remove(i);
                if free_size > aligned_size {
                    self.free_list
                        .push((offset + aligned_size, free_size - aligned_size));
                }
                self.allocations.push(ArenaAllocation {
                    offset,
                    size: aligned_size,
                    tensor_id,
                    in_use: true,
                });
                return Some(offset);
            }
        }
        None
    }

    pub fn deallocate(&mut self, tensor_id: u32) {
        if let Some(alloc) = self
            .allocations
            .iter_mut()
            .find(|a| a.tensor_id == tensor_id)
        {
            alloc.in_use = false;
            self.free_list.push((alloc.offset, alloc.size));
            self.coalesce_free_list();
        }
    }

    fn coalesce_free_list(&mut self) {
        self.free_list.sort_by_key(|&(offset, _)| offset);
        let mut i = 0;
        while i + 1 < self.free_list.len() {
            let (off1, size1) = self.free_list[i];
            let (off2, size2) = self.free_list[i + 1];
            if off1 + size1 == off2 {
                self.free_list[i] = (off1, size1 + size2);
                self.free_list.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }

    pub fn get_slice(&self, offset: usize, size: usize) -> &[u8] {
        &self.buffer[offset..offset + size]
    }

    pub fn get_slice_mut(&mut self, offset: usize, size: usize) -> &mut [u8] {
        &mut self.buffer[offset..offset + size]
    }

    pub fn used_bytes(&self) -> usize {
        self.allocations
            .iter()
            .filter(|a| a.in_use)
            .map(|a| a.size)
            .sum()
    }

    pub fn reset(&mut self) {
        self.allocations.clear();
        self.free_list.clear();
        self.free_list.push((0, self.capacity));
    }
}

// ─── Hardware Abstraction ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeBackend {
    Cpu,
    Gpu,
    Npu,
}

pub trait ComputeDevice {
    fn backend(&self) -> ComputeBackend;
    fn name(&self) -> &str;
    fn available_memory(&self) -> usize;
    fn supports_dtype(&self, dtype: DType) -> bool;
    fn matmul(&self, a: &Tensor, b: &Tensor) -> Tensor;
    fn conv2d(&self, input: &Tensor, weight: &Tensor, stride: usize, padding: usize) -> Tensor;
}

pub struct CpuDevice;

impl ComputeDevice for CpuDevice {
    fn backend(&self) -> ComputeBackend {
        ComputeBackend::Cpu
    }
    fn name(&self) -> &str {
        "AthenaOS CPU"
    }
    fn available_memory(&self) -> usize {
        1024 * 1024 * 1024
    }
    fn supports_dtype(&self, _dtype: DType) -> bool {
        true
    }

    fn matmul(&self, a: &Tensor, b: &Tensor) -> Tensor {
        matmul(a, b)
    }

    fn conv2d(&self, input: &Tensor, weight: &Tensor, stride: usize, padding: usize) -> Tensor {
        conv2d(input, weight, None, stride, padding)
    }
}

pub struct GpuDispatch {
    pub available: bool,
    pub device_name: String,
    pub memory_mb: usize,
}

impl GpuDispatch {
    pub fn new() -> Self {
        Self {
            available: false,
            device_name: String::from("none"),
            memory_mb: 0,
        }
    }

    pub fn dispatch_matmul(&self, a: &Tensor, b: &Tensor) -> Tensor {
        matmul(a, b)
    }
}

pub struct NpuDispatch {
    pub available: bool,
    pub tops: u32,
}

impl NpuDispatch {
    pub fn new() -> Self {
        Self {
            available: false,
            tops: 0,
        }
    }

    pub fn dispatch_inference(&self, _model: &OnnxModel, input: &Tensor) -> Tensor {
        input.clone()
    }
}

// ─── BPE Tokenizer ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BpeTokenizer {
    vocab: Vec<String>,
    merges: Vec<(String, String)>,
    special_tokens: Vec<(String, u32)>,
    vocab_size: u32,
}

impl BpeTokenizer {
    pub fn new() -> Self {
        Self {
            vocab: Vec::new(),
            merges: Vec::new(),
            special_tokens: Vec::new(),
            vocab_size: 0,
        }
    }

    pub fn from_vocab_and_merges(vocab: Vec<String>, merges: Vec<(String, String)>) -> Self {
        let vocab_size = vocab.len() as u32;
        Self {
            vocab,
            merges,
            special_tokens: Vec::new(),
            vocab_size,
        }
    }

    pub fn add_special_token(&mut self, token: &str, id: u32) {
        self.special_tokens.push((String::from(token), id));
    }

    pub fn encode(&self, text: &str) -> Vec<u32> {
        for (special, id) in &self.special_tokens {
            if text == special.as_str() {
                return vec![*id];
            }
        }

        let mut tokens: Vec<String> = text
            .chars()
            .map(|c| {
                let mut s = String::new();
                s.push(c);
                s
            })
            .collect();

        for (left, right) in &self.merges {
            let mut i = 0;
            while i + 1 < tokens.len() {
                if &tokens[i] == left && &tokens[i + 1] == right {
                    let mut merged = tokens[i].clone();
                    merged.push_str(&tokens[i + 1]);
                    tokens[i] = merged;
                    tokens.remove(i + 1);
                } else {
                    i += 1;
                }
            }
        }

        tokens
            .iter()
            .map(|t| self.vocab.iter().position(|v| v == t).unwrap_or(0) as u32)
            .collect()
    }

    pub fn decode(&self, ids: &[u32]) -> String {
        let mut result = String::new();
        for &id in ids {
            if let Some((special, _)) = self.special_tokens.iter().find(|(_, sid)| *sid == id) {
                result.push_str(special);
            } else if (id as usize) < self.vocab.len() {
                result.push_str(&self.vocab[id as usize]);
            }
        }
        result
    }

    pub fn vocab_size(&self) -> u32 {
        self.vocab_size
    }
}

// ─── Pre-built Model Types ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    ImageClassifier,
    ObjectDetector,
    TextClassifier,
    TextGenerator,
    SpeechRecognizer,
    ImageSegmenter,
    FeatureExtractor,
}

pub struct ImageClassification {
    pub class_id: u32,
    pub confidence: f32,
    pub label: String,
}

pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub class_id: u32,
    pub confidence: f32,
    pub label: String,
}

pub struct TextClassification {
    pub class_id: u32,
    pub confidence: f32,
    pub label: String,
}

pub struct GeneratedText {
    pub tokens: Vec<u32>,
    pub text: String,
}

pub struct ModelConfig {
    pub model_type: ModelType,
    pub input_shape: Vec<usize>,
    pub num_classes: u32,
    pub max_sequence_length: usize,
    pub vocab_size: u32,
    pub hidden_size: usize,
    pub num_heads: usize,
    pub num_layers: usize,
}

pub struct LoadedModel {
    pub config: ModelConfig,
    pub onnx: OnnxModel,
    pub executor: GraphExecutor,
    pub tokenizer: Option<BpeTokenizer>,
    pub warmed_up: bool,
}

impl LoadedModel {
    pub fn classify_image(&mut self, input: &Tensor) -> Vec<ImageClassification> {
        let outputs = self
            .executor
            .execute(&[(String::from("input"), input.clone())]);
        if outputs.is_empty() {
            return Vec::new();
        }
        let probs = softmax(&outputs[0], outputs[0].ndim() - 1);
        let data = probs.as_f32_slice();
        let mut results: Vec<ImageClassification> = data
            .iter()
            .enumerate()
            .map(|(i, &conf)| ImageClassification {
                class_id: i as u32,
                confidence: conf,
                label: String::new(),
            })
            .collect();
        results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        results.truncate(5);
        results
    }

    pub fn detect_objects(&mut self, input: &Tensor) -> Vec<BoundingBox> {
        let outputs = self
            .executor
            .execute(&[(String::from("input"), input.clone())]);
        let _ = outputs;
        Vec::new()
    }

    pub fn classify_text(&mut self, tokens: &[u32]) -> Vec<TextClassification> {
        let input = Tensor::from_f32(
            &tokens.iter().map(|&t| t as f32).collect::<Vec<_>>(),
            &[1, tokens.len()],
        );
        let outputs = self.executor.execute(&[(String::from("input"), input)]);
        if outputs.is_empty() {
            return Vec::new();
        }
        let probs = softmax(&outputs[0], outputs[0].ndim() - 1);
        let data = probs.as_f32_slice();
        data.iter()
            .enumerate()
            .map(|(i, &conf)| TextClassification {
                class_id: i as u32,
                confidence: conf,
                label: String::new(),
            })
            .collect()
    }

    pub fn generate_text(&mut self, prompt_tokens: &[u32], max_new_tokens: usize) -> GeneratedText {
        let mut tokens = prompt_tokens.to_vec();
        for _ in 0..max_new_tokens {
            let input = Tensor::from_f32(
                &tokens.iter().map(|&t| t as f32).collect::<Vec<_>>(),
                &[1, tokens.len()],
            );
            let outputs = self.executor.execute(&[(String::from("input"), input)]);
            if outputs.is_empty() {
                break;
            }
            let logits = &outputs[0];
            let last_pos = logits.numel() - self.config.vocab_size as usize;
            let logit_slice = &logits.as_f32_slice()[last_pos..];
            let next_token = logit_slice
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal))
                .map(|(i, _)| i as u32)
                .unwrap_or(0);
            tokens.push(next_token);
        }

        let text = if let Some(ref tok) = self.tokenizer {
            tok.decode(&tokens[prompt_tokens.len()..])
        } else {
            String::new()
        };

        GeneratedText {
            tokens: tokens[prompt_tokens.len()..].to_vec(),
            text,
        }
    }

    pub fn warmup(&mut self) {
        let dummy = Tensor::zeros(&self.config.input_shape, DType::F32);
        let _ = self.executor.execute(&[(String::from("input"), dummy)]);
        self.warmed_up = true;
    }
}

// ─── Model Cache ────────────────────────────────────────────────────────────

pub struct ModelCache {
    models: Vec<(String, LoadedModel)>,
    max_models: usize,
}

impl ModelCache {
    pub fn new(max_models: usize) -> Self {
        Self {
            models: Vec::new(),
            max_models,
        }
    }

    pub fn get(&self, name: &str) -> Option<&LoadedModel> {
        self.models.iter().find(|(n, _)| n == name).map(|(_, m)| m)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut LoadedModel> {
        self.models
            .iter_mut()
            .find(|(n, _)| n == name)
            .map(|(_, m)| m)
    }

    pub fn insert(&mut self, name: String, model: LoadedModel) {
        if self.models.len() >= self.max_models {
            self.models.remove(0);
        }
        self.models.push((name, model));
    }

    pub fn remove(&mut self, name: &str) -> bool {
        if let Some(pos) = self.models.iter().position(|(n, _)| n == name) {
            self.models.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn cached_count(&self) -> usize {
        self.models.len()
    }
}

// ─── Batch Inference ────────────────────────────────────────────────────────

pub struct BatchRequest {
    pub id: u64,
    pub input: Tensor,
    pub priority: u8,
}

pub struct BatchResult {
    pub id: u64,
    pub output: Tensor,
}

pub struct BatchScheduler {
    pending: Vec<BatchRequest>,
    max_batch_size: usize,
    timeout_ms: u64,
}

impl BatchScheduler {
    pub fn new(max_batch_size: usize, timeout_ms: u64) -> Self {
        Self {
            pending: Vec::new(),
            max_batch_size,
            timeout_ms,
        }
    }

    pub fn submit(&mut self, request: BatchRequest) {
        self.pending.push(request);
        self.pending.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    pub fn collect_batch(&mut self) -> Vec<BatchRequest> {
        let count = self.pending.len().min(self.max_batch_size);
        self.pending.drain(..count).collect()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn execute_batch(
        &self,
        batch: &[BatchRequest],
        executor: &mut GraphExecutor,
    ) -> Vec<BatchResult> {
        let mut results = Vec::with_capacity(batch.len());
        for req in batch {
            let outputs = executor.execute(&[(String::from("input"), req.input.clone())]);
            let output = outputs
                .into_iter()
                .next()
                .unwrap_or_else(|| Tensor::zeros(&[1], DType::F32));
            results.push(BatchResult { id: req.id, output });
        }
        results
    }
}

// ─── Global AI Engine ───────────────────────────────────────────────────────

pub struct AiEngine {
    pub initialized: bool,
    pub cpu_device: CpuDevice,
    pub gpu: GpuDispatch,
    pub npu: NpuDispatch,
    pub model_cache: ModelCache,
    pub batch_scheduler: BatchScheduler,
    pub arena: TensorArena,
    pub default_backend: ComputeBackend,
}

impl AiEngine {
    pub const fn uninit() -> Self {
        Self {
            initialized: false,
            cpu_device: CpuDevice,
            gpu: GpuDispatch {
                available: false,
                device_name: String::new(),
                memory_mb: 0,
            },
            npu: NpuDispatch {
                available: false,
                tops: 0,
            },
            model_cache: ModelCache {
                models: Vec::new(),
                max_models: 0,
            },
            batch_scheduler: BatchScheduler {
                pending: Vec::new(),
                max_batch_size: 0,
                timeout_ms: 0,
            },
            arena: TensorArena {
                buffer: Vec::new(),
                capacity: 0,
                allocations: Vec::new(),
                free_list: Vec::new(),
            },
            default_backend: ComputeBackend::Cpu,
        }
    }
}

static INIT: AtomicBool = AtomicBool::new(false);

pub static mut AI_ENGINE: AiEngine = AiEngine::uninit();

pub fn init() {
    if INIT.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        AI_ENGINE.initialized = true;
        AI_ENGINE.model_cache = ModelCache::new(8);
        AI_ENGINE.batch_scheduler = BatchScheduler::new(32, 100);
        AI_ENGINE.arena = TensorArena::new(64 * 1024 * 1024);
        AI_ENGINE.gpu = GpuDispatch::new();
        AI_ENGINE.npu = NpuDispatch::new();
        AI_ENGINE.default_backend = ComputeBackend::Cpu;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(data: &[f32], shape: &[usize]) -> Tensor {
        Tensor::from_f32(data, shape)
    }

    #[test]
    fn tensor_shape_indexing_and_mutation() {
        let mut x = t(&[1.0, 2.0, 3.0, 4.0], &[2, 2]);
        assert_eq!(x.numel(), 4);
        assert_eq!(x.shape(), &[2, 2]);
        assert_eq!(x.ndim(), 2);
        assert_eq!(x.get_f32(&[0, 0]), 1.0);
        assert_eq!(x.get_f32(&[1, 1]), 4.0);
        x.set_f32(&[0, 1], 9.0);
        assert_eq!(x.get_f32(&[0, 1]), 9.0);
    }

    #[test]
    fn elementwise_ops() {
        assert_eq!(
            add(&t(&[1.0, 2.0, 3.0], &[3]), &t(&[10.0, 20.0, 30.0], &[3])).as_f32_slice(),
            &[11.0, 22.0, 33.0]
        );
        assert_eq!(
            sub(&t(&[5.0, 5.0], &[2]), &t(&[1.0, 2.0], &[2])).as_f32_slice(),
            &[4.0, 3.0]
        );
        assert_eq!(
            mul(&t(&[1.0, 2.0, 3.0], &[3]), &t(&[10.0, 20.0, 30.0], &[3])).as_f32_slice(),
            &[10.0, 40.0, 90.0]
        );
        assert_eq!(
            div(&t(&[10.0, 20.0], &[2]), &t(&[2.0, 5.0], &[2])).as_f32_slice(),
            &[5.0, 4.0]
        );
    }

    #[test]
    fn dot_product_known_answer() {
        // 1*4 + 2*5 + 3*6 = 32.
        assert_eq!(
            dot(&t(&[1.0, 2.0, 3.0], &[3]), &t(&[4.0, 5.0, 6.0], &[3])),
            32.0
        );
    }

    #[test]
    fn matmul_2x2_known_answer() {
        // [[1,2],[3,4]] x [[5,6],[7,8]] = [[19,22],[43,50]].
        let a = t(&[1.0, 2.0, 3.0, 4.0], &[2, 2]);
        let b = t(&[5.0, 6.0, 7.0, 8.0], &[2, 2]);
        let c = matmul(&a, &b);
        assert_eq!(c.shape(), &[2, 2]);
        assert_eq!(c.as_f32_slice(), &[19.0, 22.0, 43.0, 50.0]);
    }

    #[test]
    fn matmul_nonsquare_shapes() {
        // (2x3) x (3x1) = (2x1): [[1,2,3],[4,5,6]] x [[1],[1],[1]] = [[6],[15]].
        let a = t(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
        let b = t(&[1.0, 1.0, 1.0], &[3, 1]);
        let c = matmul(&a, &b);
        assert_eq!(c.shape(), &[2, 1]);
        assert_eq!(c.as_f32_slice(), &[6.0, 15.0]);
    }

    #[test]
    fn softmax_is_a_normalized_monotonic_distribution() {
        let out = softmax(&t(&[1.0, 2.0, 3.0], &[3]), 0);
        let v = out.as_f32_slice();
        let sum: f32 = v.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5); // a valid probability distribution
        assert!(v[2] > v[1] && v[1] > v[0]); // order preserved
        assert!(v.iter().all(|&p| (0.0..=1.0).contains(&p)));
        // Largest logit gets the most mass; e^2/(e^0+e^1+e^2) ~ 0.665.
        assert!((v[2] - 0.665).abs() < 0.01);
    }
}
