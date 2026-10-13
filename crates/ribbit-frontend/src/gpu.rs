//! wgpu presentation: the emulated LCD (exact pixels or Catmull-Rom smoothed)
//! plus the egui user interface on top. Display only: nothing here changes
//! what is emulated.

use anyhow::{Context, Result};
use ribbit_video::{HEIGHT, WIDTH};
use std::sync::Arc;
use winit::dpi::PhysicalSize;
use winit::window::Window;

const SHADER: &str = r#"
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}

fn texel(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    return textureLoad(tex, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

fn catmull_rom(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4<f32>(
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    );
}

@fragment
fn fs_smooth(in: Out) -> @location(0) vec4<f32> {
    let p = in.uv * vec2<f32>(textureDimensions(tex)) - 0.5;
    let base = floor(p);
    let f = p - base;
    let wx = catmull_rom(f.x);
    let wy = catmull_rom(f.y);
    let b = vec2<i32>(base);
    var acc = vec4<f32>(0.0);
    for (var j = 0; j < 4; j++) {
        var row = vec4<f32>(0.0);
        for (var i = 0; i < 4; i++) {
            row += texel(b + vec2<i32>(i - 1, j - 1)) * wx[i];
        }
        acc += row * wy[j];
    }
    return clamp(acc, vec4<f32>(0.0), vec4<f32>(1.0));
}
"#;

/// How the 160x160 LCD image is scaled (display only).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Display {
    /// Exact pixels, whole-number scale.
    Pixel,
    /// Catmull-Rom bicubic, fills the space.
    #[default]
    Smooth,
}

pub struct Gpu {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    pixel: wgpu::RenderPipeline,
    smooth: wgpu::RenderPipeline,
    pub egui: egui_wgpu::Renderer,
}

impl Gpu {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).context("creating surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("no compatible GPU adapter")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ribbit"),
            // Lowest common denominator so the same renderer runs on GLES
            // devices (Android) as well as Vulkan/Metal/DX12.
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .context("creating GPU device")?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("surface not supported by adapter")?;
        // egui blends in gamma space and wants a non-sRGB target; the LCD
        // texture is non-sRGB too, so the emulated colours pass through as is.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().copied().find(|f| !f.is_srgb()) {
            config.format = f;
        }
        surface.configure(&device, &config);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lcd"),
            size: wgpu::Extent3d { width: WIDTH as u32, height: HEIGHT as u32, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lcd"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lcd"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lcd"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lcd"),
            bind_group_layouts: &[Some(&bgl)],
            ..Default::default()
        });
        let make = |fragment: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("lcd"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(config.format.into())],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let (pixel, smooth) = (make("fs"), make("fs_smooth"));
        let egui = egui_wgpu::Renderer::new(&device, config.format, egui_wgpu::RendererOptions::default());
        Ok(Self { window, surface, device, queue, config, texture, bind_group, pixel, smooth, egui })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Where the LCD goes inside `area` (x, y, w, h in physical pixels).
    pub fn lcd_rect(area: (f32, f32, f32, f32), display: Display) -> (f32, f32, f32, f32) {
        let (ax, ay, aw, ah) = area;
        let side = match display {
            Display::Pixel => ((aw.min(ah) / WIDTH as f32).floor().max(1.0)) * WIDTH as f32,
            Display::Smooth => aw.min(ah).max(1.0),
        };
        (ax + (aw - side) / 2.0, ay + (ah - side) / 2.0, side, side)
    }

    pub fn upload(&self, frame: &[u32; WIDTH * HEIGHT]) {
        let rgba: Vec<u8> = frame.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 0xFF]).collect();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * WIDTH as u32), rows_per_image: None },
            wgpu::Extent3d { width: WIDTH as u32, height: HEIGHT as u32, depth_or_array_layers: 1 },
        );
    }

    /// Draws the LCD into `lcd` (None: no game screen) and egui on top.
    pub fn render(
        &mut self,
        lcd: Option<((f32, f32, f32, f32), Display)>,
        paint_jobs: &[egui::ClippedPrimitive],
        textures: &egui::TexturesDelta,
        pixels_per_point: f32,
    ) -> Result<()> {
        for (id, deltas) in &textures.set {
            for delta in deltas {
                self.egui.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        use wgpu::CurrentSurfaceTexture as Cst;
        let output = match self.surface.get_current_texture() {
            Cst::Success(t) => t,
            Cst::Suboptimal(t) => {
                self.surface.configure(&self.device, &self.config);
                t
            }
            Cst::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Cst::Timeout | Cst::Occluded => return Ok(()),
            Cst::Lost => anyhow::bail!("GPU surface lost"),
            Cst::Validation => anyhow::bail!("GPU validation error acquiring surface texture"),
        };
        let view = output.texture.create_view(&Default::default());
        let screen =
            egui_wgpu::ScreenDescriptor { size_in_pixels: [self.config.width, self.config.height], pixels_per_point };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let extra = self.egui.update_buffers(&self.device, &self.queue, &mut encoder, paint_jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("frame"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.02, g: 0.025, b: 0.03, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            if let Some(((x, y, w, h), display)) = lcd {
                let (fw, fh) = (self.config.width as f32, self.config.height as f32);
                let (x0, y0) = (x.max(0.0), y.max(0.0));
                let (w, h) = (w.min(fw - x0), h.min(fh - y0));
                if w >= 1.0 && h >= 1.0 {
                    pass.set_viewport(x0, y0, w, h, 0.0, 1.0);
                    pass.set_pipeline(match display {
                        Display::Pixel => &self.pixel,
                        Display::Smooth => &self.smooth,
                    });
                    pass.set_bind_group(0, &self.bind_group, &[]);
                    pass.draw(0..3, 0..1);
                    pass.set_viewport(0.0, 0.0, fw, fh, 0.0, 1.0);
                }
            }
            self.egui.render(&mut pass, paint_jobs, &screen);
        }
        self.queue.submit(extra.into_iter().chain([encoder.finish()]));
        self.queue.present(output);
        for id in &textures.free {
            self.egui.free_texture(id);
        }
        Ok(())
    }
}
