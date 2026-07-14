use athui::wgpu_canvas::WgpuCanvas;
use skia_safe::{Color, Paint, Rect};
use std::sync::Arc;
use wgpu::{Backends, Instance, InstanceDescriptor};

#[tokio::main]
async fn main() {
    println!("Initializing AthenaOS GPU Pipeline...");

    // In a real AthenaOS environment, wgpu::Instance will be backed by RaeGfxApi.
    // For now, we instantiate a standard wgpu instance.
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::all(),
        ..Default::default()
    });

    println!("Creating hardware-accelerated Skia surface...");

    // We would pass the actual window surface here.
    // For a headless/dummy demo, we skip actual presentation if there's no surface.
    // Since we don't have a windowing system yet, we will just create a texture
    // offscreen if we wanted to verify rendering, or panic for now since we don't have a wgpu::Surface.

    println!("hello_athui initialized. (Presentation requires a valid AthenaOS Surface).");
}
