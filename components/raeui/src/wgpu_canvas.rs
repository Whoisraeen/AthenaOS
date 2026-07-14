use alloc::sync::Arc;
use skia_safe::{
    gpu::{backend_render_targets, wgpu::BackendContext, DirectContext, SurfaceOrigin},
    ColorType, Surface,
};
use wgpu::{Device, Instance, Queue, Surface as WgpuSurface, SurfaceConfiguration, TextureFormat};

pub struct WgpuCanvas {
    pub surface: Surface,
    pub direct_context: DirectContext,
    pub wgpu_device: Arc<Device>,
    pub wgpu_queue: Arc<Queue>,
    pub wgpu_surface: WgpuSurface<'static>,
}

impl WgpuCanvas {
    /// Initialize a new GPU-accelerated Skia canvas on top of wgpu.
    /// In AthenaOS, the wgpu Instance is backed by our zero-syscall RaeGfxApi HAL backend.
    pub async fn new(
        instance: Instance,
        wgpu_surface: WgpuSurface<'static>,
        width: u32,
        height: u32,
    ) -> Self {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&wgpu_surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find a suitable wgpu adapter (AthGFX backend)");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("AthenaOS GPU Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .expect("Failed to request wgpu device");

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        // Configure surface
        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: TextureFormat::Bgra8Unorm, // AthenaOS default
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: alloc::vec![],
            desired_maximum_frame_latency: 2,
        };
        wgpu_surface.configure(&device, &config);

        // Bridge to Skia
        let backend_context = BackendContext {
            instance: instance.clone(),
            device: device.clone(),
            queue: queue.clone(),
        };

        let mut direct_context = DirectContext::new_wgpu(&backend_context, None)
            .expect("Failed to create Skia DirectContext from wgpu");

        // We create a backend render target mapped to the wgpu surface texture during render loops.
        // For now, we will allocate an offscreen surface if we want to draw into a texture,
        // or just wait for `render` to acquire the surface texture.

        // Let's create a placeholder Skia Surface for now. We usually swap this out per-frame
        // in a real presentation loop by wrapping the acquired wgpu surface texture.

        let image_info = skia_safe::ImageInfo::new_n32_premul((width as i32, height as i32), None);
        let surface = Surface::new_render_target(
            &mut direct_context,
            skia_safe::Budgeted::Yes,
            &image_info,
            None,
            SurfaceOrigin::TopLeft,
            None,
            false,
            None,
        )
        .expect("Failed to create Skia surface");

        Self {
            surface,
            direct_context,
            wgpu_device: device,
            wgpu_queue: queue,
            wgpu_surface,
        }
    }
}
