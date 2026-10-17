//! wgpu presentation: the emulated LCD (AI-upscaled, exact pixels,
//! sharp-bilinear, an LCD grid look or Catmull-Rom smoothed)
//! plus the egui user interface on top. Display only: nothing here changes
//! what is emulated.

use crate::upscale::{AiSettings, Network, Upscaler};
use anyhow::{Context, Result};
use ribbit_video::{HEIGHT, WIDTH};
use std::sync::Arc;
use winit::dpi::PhysicalSize;
use winit::window::Window;

/// The HD picture over its fallback: the canvas (premultiplied, alpha =
/// how much of each pixel it knows) where it knows and the mask allows,
/// else the LCD's own pixels, exactly (no upscaling or smoothing: what HD
/// cannot draw from the BIOS's vector data shows as the BIOS drew it). The
/// mask is 160x160, one value per LCD pixel.
const HD_SHADER: &str = r#"
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}
@group(0) @binding(0) var hd: texture_2d<f32>;
@group(0) @binding(1) var smooth_samp: sampler;
@group(0) @binding(2) var fallback: texture_2d<f32>;
@group(0) @binding(3) var mask: texture_2d<f32>;
@fragment
fn fs_hd(in: Out) -> @location(0) vec4<f32> {
    let c = textureSample(hd, smooth_samp, in.uv);
    let fd = vec2<f32>(textureDimensions(fallback));
    let f = textureLoad(fallback, vec2<i32>(clamp(in.uv * fd, vec2<f32>(0.0), fd - 1.0)), 0).rgb;
    let md = vec2<f32>(textureDimensions(mask));
    let keep = 1.0 - textureLoad(mask, vec2<i32>(clamp(in.uv * md, vec2<f32>(0.0), md - 1.0)), 0).r;
    let known = c.a * keep;
    return vec4<f32>(c.rgb * keep + f * (1.0 - known), 1.0);
}
"#;

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

// Bilinear between the four texels around texel-space point `t` (texel
// centres at integer + 0.5).
fn bilinear(t: vec2<f32>) -> vec4<f32> {
    let q = t - 0.5;
    let b = vec2<i32>(floor(q));
    let f = q - floor(q);
    let top = mix(texel(b), texel(b + vec2<i32>(1, 0)), f.x);
    let bottom = mix(texel(b + vec2<i32>(0, 1)), texel(b + vec2<i32>(1, 1)), f.x);
    return mix(top, bottom, f.y);
}

// Sharp bilinear: each texel is a solid square; only the one output pixel
// straddling a texel edge is blended, so any scale looks crisp and even.
fn sharp(p: vec2<f32>, px_per_texel: vec2<f32>) -> vec4<f32> {
    let cell = floor(p);
    let f = p - cell;
    // Distance into the cell, pushed towards the centre except within half
    // an output pixel of the edge.
    let g = clamp((f - 0.5) * px_per_texel + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
    return bilinear(cell + g);
}

@fragment
fn fs_sharp(in: Out) -> @location(0) vec4<f32> {
    let p = in.uv * vec2<f32>(textureDimensions(tex));
    let s = 1.0 / max(fwidth(p), vec2<f32>(1e-4));
    return sharp(p, s);
}

// Sharp, plus a faint dark gap between pixels like the real LCD's grid;
// the gap fades out below 3x scale where it would only shimmer.
@fragment
fn fs_lcd(in: Out) -> @location(0) vec4<f32> {
    let p = in.uv * vec2<f32>(textureDimensions(tex));
    let s = 1.0 / max(fwidth(p), vec2<f32>(1e-4));
    let c = sharp(p, s);
    let f = p - floor(p);
    // Distance to the nearest cell edge, in output pixels.
    let d = min(f, 1.0 - f) * s;
    let scale = min(s.x, s.y);
    let gap = 0.08 * scale;
    let line = smoothstep(gap - 0.5, gap + 0.5, min(d.x, d.y));
    let strength = 0.22 * clamp((scale - 2.0) / 2.0, 0.0, 1.0);
    return vec4<f32>(c.rgb * (1.0 - strength * (1.0 - line)), 1.0);
}

// Catmull-Rom kernel.
fn cr(x: f32) -> f32 {
    let a = abs(x);
    if (a < 1.0) {
        return (1.5 * a - 2.5) * a * a + 1.0;
    }
    if (a < 2.0) {
        return ((-0.5 * a + 2.5) * a - 4.0) * a + 2.0;
    }
    return 0.0;
}

// Fits an upscaled picture to the window: Catmull-Rom when enlarging, the
// same kernel widened to the footprint when shrinking (no aliasing).
@fragment
fn fs_resample(in: Out) -> @location(0) vec4<f32> {
    let p = in.uv * vec2<f32>(textureDimensions(tex));
    let fw = fwidth(p);
    let r = clamp(max(fw.x, fw.y), 1.0, 4.0);
    let c = p - 0.5;
    let lo = vec2<i32>(ceil(c - 2.0 * r));
    let hi = vec2<i32>(floor(c + 2.0 * r));
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var j = lo.y; j <= hi.y; j++) {
        let wy = cr((c.y - f32(j)) / r);
        for (var i = lo.x; i <= hi.x; i++) {
            let w = wy * cr((c.x - f32(i)) / r);
            acc += texel(vec2<i32>(i, j)).rgb * w;
            wsum += w;
        }
    }
    return vec4<f32>(clamp(acc / wsum, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

/// How the 160x160 LCD image is scaled (display only).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Display {
    /// Neural-network upscaling (see `upscale`), fills the space.
    #[default]
    Ai,
    /// Exact pixels, whole-number scale.
    Pixel,
    /// Square pixels with antialiased edges, fills the space.
    Sharp,
    /// Sharp plus a faint pixel grid.
    Lcd,
    /// Catmull-Rom bicubic, fills the space.
    Smooth,
}

/// Where the LCD goes (x, y, w, h in physical pixels) and how it is drawn.
pub type LcdView = ((f32, f32, f32, f32), Display, AiSettings);

pub struct Gpu {
    pub window: Arc<Window>,
    /// Kept so a new surface can be made for a new window (Android takes
    /// the window away while the app is in the background).
    instance: wgpu::Instance,
    /// None while detached (`detach`, `reattach`).
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    lcd_view: wgpu::TextureView,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    resample: wgpu::RenderPipeline,
    /// None when the GPU cannot render to Rgba16Float (AI falls back to Sharp).
    upscaler: Option<Upscaler>,
    /// The LCD changed since the last upscale.
    frame_dirty: bool,
    last_frame: Vec<u32>,
    last_chain: Vec<Network>,
    /// EXPERIMENTAL HD picture (ribbit_core::hd), shown instead of the
    /// LCD while `show_hd` is set: the canvas (its side in `hd_size`), its
    /// 160x160 fallback mask, and what draws them.
    hd_texture: wgpu::Texture,
    hd_size: u32,
    mask_texture: wgpu::Texture,
    hd_bgl: wgpu::BindGroupLayout,
    hd_pipeline: wgpu::RenderPipeline,
    linear: wgpu::Sampler,
    pub show_hd: bool,
    /// A TV console's picture (the Zippity: 640x480), shown instead of the
    /// LCD while `show_tv` is set: its texture, size and bind group.
    tv: Option<(wgpu::Texture, (u32, u32), wgpu::BindGroup)>,
    pub show_tv: bool,
    pixel: wgpu::RenderPipeline,
    sharp: wgpu::RenderPipeline,
    lcd: wgpu::RenderPipeline,
    smooth: wgpu::RenderPipeline,
    pub egui: egui_wgpu::Renderer,
}

/// What `Gpu::connect` sets up: instance, surface, adapter, device, queue
/// and the surface's configuration.
type Connection = (wgpu::Instance, wgpu::Surface<'static>, wgpu::Adapter, wgpu::Device, wgpu::Queue, wgpu::SurfaceConfiguration);

impl Gpu {
    /// A GPU set up to show pictures in `window`: the platform's usual API
    /// first (or the one `WGPU_BACKEND` names: vulkan, gl, dx12, metal),
    /// then OpenGL. Some Linux setups (virtual machines, WSL, old drivers)
    /// offer a Vulkan device that says it can present to the window and
    /// then fails when the window is set up; that is tried and caught here.
    fn connect(
        window: &Arc<Window>,
    ) -> Result<Connection> {
        // WSL's Vulkan driver cannot keep a window going (it fails a few
        // frames in), so OpenGL comes first there.
        let wsl = cfg!(target_os = "linux")
            && std::fs::read_to_string("/proc/sys/kernel/osrelease").is_ok_and(|r| r.to_lowercase().contains("microsoft"));
        let usual = if wsl { wgpu::Backends::GL } else { wgpu::Backends::all() };
        let first = wgpu::Backends::from_env().unwrap_or(usual);
        let mut tried = Vec::new();
        let mut last = anyhow::anyhow!("no GPU backend available");
        for backends in [first, wgpu::Backends::GL] {
            if tried.contains(&backends) {
                continue;
            }
            tried.push(backends);
            match Self::connect_with(window, backends) {
                Ok(c) => return Ok(c),
                Err(e) => {
                    tracing::warn!("GPU ({backends:?}) cannot show the window: {e:#}");
                    last = e;
                }
            }
        }
        Err(last.context(format!("no GPU can draw to the window (tried {tried:?})")))
    }

    fn connect_with(
        window: &Arc<Window>,
        backends: wgpu::Backends,
    ) -> Result<Connection> {
        // (OpenGL through EGL needs the display the window is on.)
        let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(window.clone()));
        desc.backends = backends;
        let instance = wgpu::Instance::new(desc);
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
        // Set up the window and show one blank frame, catching what fails.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(&device, &config);
        if let wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) =
            surface.get_current_texture()
        {
            let view = frame.texture.create_view(&Default::default());
            let mut encoder = device.create_command_encoder(&Default::default());
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("first frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            }));
            queue.submit([encoder.finish()]);
            queue.present(frame);
        }
        surface.configure(&device, &config);
        if let Some(e) = pollster::block_on(scope.pop()) {
            anyhow::bail!("{:?} window setup failed: {e}", adapter.get_info().backend);
        }
        Ok((instance, surface, adapter, device, queue, config))
    }

    pub fn new(window: Arc<Window>) -> Result<Self> {
        let (instance, surface, adapter, device, queue, config) = Self::connect(&window)?;

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
        let hd_size = ribbit_core::hd::HD_SIZE as u32;
        let hd_texture = Self::hd_texture(&device, hd_size);
        let mask_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd mask"),
            size: wgpu::Extent3d { width: 160, height: 160, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let hd_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd"),
            entries: &[
                tex_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(2),
                tex_entry(3),
            ],
        });
        let hd_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hd"),
            source: wgpu::ShaderSource::Wgsl(HD_SHADER.into()),
        });
        let hd_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd"),
            bind_group_layouts: &[Some(&hd_bgl)],
            ..Default::default()
        });
        let hd_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hd"),
            layout: Some(&hd_layout),
            vertex: wgpu::VertexState {
                module: &hd_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &hd_shader,
                entry_point: Some("fs_hd"),
                compilation_options: Default::default(),
                targets: &[Some(config.format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
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
        let (pixel, sharp, lcd, smooth) = (make("fs"), make("fs_sharp"), make("fs_lcd"), make("fs_smooth"));
        let resample = make("fs_resample");
        let upscaler = Upscaler::supported(&adapter).then(Upscaler::default);
        if upscaler.is_none() {
            tracing::warn!(target: "ribbit::video", "GPU cannot render Rgba16Float: AI upscaling unavailable");
        }
        let egui = egui_wgpu::Renderer::new(&device, config.format, egui_wgpu::RendererOptions::default());
        Ok(Self {
            window,
            instance,
            surface: Some(surface),
            device,
            queue,
            config,
            texture,
            hd_texture,
            hd_size,
            mask_texture,
            hd_bgl,
            hd_pipeline,
            linear,
            show_hd: false,
            tv: None,
            show_tv: false,
            lcd_view: view,
            bgl,
            sampler,
            bind_group,
            resample,
            upscaler,
            frame_dirty: true,
            last_frame: Vec::new(),
            last_chain: Vec::new(),
            pixel,
            sharp,
            lcd,
            smooth,
            egui,
        })
    }

    /// Lets go of the window's surface (Android: the app went to the
    /// background and its window is going away). The device and every
    /// texture stay, so nothing has to be uploaded again.
    pub fn detach(&mut self) {
        self.surface = None;
    }

    /// Shows pictures in `window` again (Android: the app came back with a
    /// new window), on the same device.
    pub fn reattach(&mut self, window: Arc<Window>) -> Result<()> {
        let surface = self.instance.create_surface(window.clone()).context("creating surface")?;
        let size = window.inner_size();
        self.config.width = size.width.max(1);
        self.config.height = size.height.max(1);
        surface.configure(&self.device, &self.config);
        self.surface = Some(surface);
        self.window = window;
        Ok(())
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            if let Some(surface) = &self.surface {
                surface.configure(&self.device, &self.config);
            }
        }
    }

    /// Where the LCD goes inside `area` (x, y, w, h in physical pixels).
    pub fn lcd_rect(area: (f32, f32, f32, f32), display: Display) -> (f32, f32, f32, f32) {
        let (ax, ay, aw, ah) = area;
        let side = match display {
            // A pixel of tolerance: fractional display scaling can leave
            // the area a fraction short of a whole multiple.
            Display::Pixel => (((aw.min(ah) + 1.0) / WIDTH as f32).floor().max(1.0)) * WIDTH as f32,
            Display::Ai | Display::Sharp | Display::Lcd | Display::Smooth => aw.min(ah).max(1.0),
        };
        (ax + (aw - side) / 2.0, ay + (ah - side) / 2.0, side, side)
    }

    /// Where a TV picture of `size` goes inside `area`: as large as fits
    /// with its shape kept.
    pub fn tv_rect(area: (f32, f32, f32, f32), size: (usize, usize)) -> (f32, f32, f32, f32) {
        let (ax, ay, aw, ah) = area;
        let (tw, th) = (size.0 as f32, size.1 as f32);
        // Not whole multiples even for Pixel: a 640 x 480 TV picture is fine
        // at any size, and rounding down would leave bars.
        let scale = (aw / tw).min(ah / th).max(0.01);
        let (w, h) = (tw * scale, th * scale);
        (ax + (aw - w) / 2.0, ay + (ah - h) / 2.0, w, h)
    }

    /// A TV picture (0x00RRGGBB), `size` pixels.
    pub fn upload_tv(&mut self, pixels: &[u32], size: (usize, usize)) {
        let (w, h) = (size.0 as u32, size.1 as u32);
        if self.tv.as_ref().is_none_or(|t| t.1 != (w, h)) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("tv"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("tv"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            self.tv = Some((texture, (w, h), group));
        }
        let rgba: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 0xFF]).collect();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.tv.as_ref().unwrap().0,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: None },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    pub fn upload(&mut self, frame: &[u32; WIDTH * HEIGHT]) {
        if self.last_frame.as_slice() != frame.as_slice() {
            self.last_frame.clear();
            self.last_frame.extend_from_slice(frame);
            self.frame_dirty = true;
        }
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

    fn hd_texture(device: &wgpu::Device, size: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// The HD fallback mask (160x160, 255 = use the fallback).
    pub fn upload_hd_mask(&mut self, mask: &[u8]) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.mask_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            mask,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(160), rows_per_image: None },
            wgpu::Extent3d { width: 160, height: 160, depth_or_array_layers: 1 },
        );
    }

    /// The HD canvas (0xAARRGGBB premultiplied, `size` square).
    pub fn upload_hd(&mut self, pixels: &[u32], size: usize) {
        let size = size as u32;
        if size != self.hd_size {
            self.hd_texture = Self::hd_texture(&self.device, size);
            self.hd_size = size;
        }
        let rgba: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8]).collect();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.hd_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * size), rows_per_image: None },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );
    }

    fn hd_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        hd: &wgpu::Texture,
        sampler: &wgpu::Sampler,
        fallback: &wgpu::TextureView,
        mask: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let (hd, mask) = (hd.create_view(&Default::default()), mask.create_view(&Default::default()));
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&hd) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(fallback) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&mask) },
            ],
        })
    }

    /// Draws the LCD into `lcd` (None: no game screen) and egui on top.
    pub fn render(
        &mut self,
        lcd: Option<LcdView>,
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
        // Detached (Android, in the background): nothing to show on.
        let Some(surface) = &self.surface else {
            return Ok(());
        };
        let output = match surface.get_current_texture() {
            Cst::Success(t) => t,
            Cst::Suboptimal(t) => {
                surface.configure(&self.device, &self.config);
                t
            }
            Cst::Outdated => {
                surface.configure(&self.device, &self.config);
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
        // AI upscale, only when the picture or the chain changed.
        let mut ai_bind_group = None;
        let mut hd_bind_group = None;
        // Not under HD: its fallback is the LCD's own pixels.
        let ai_for = |lcd: Option<LcdView>, show_hd: bool| match lcd {
            Some((rect, Display::Ai, ai)) if !show_hd => Some((rect, ai)),
            _ => None,
        };
        let tv = self.show_tv.then_some(()).and(self.tv.as_ref()).map(|t| &t.2);
        if let (Some(((_, _, w, h), ai)), Some(up), None) = (ai_for(lcd, self.show_hd), &mut self.upscaler, tv) {
            let size = WIDTH as u32;
            let chain = ai.chain(size, ai.resolution.target(size, w.min(h)));
            let view = if self.frame_dirty || chain != self.last_chain || up.output().is_none() {
                self.frame_dirty = false;
                self.last_chain.clone_from(&chain);
                up.run(&self.device, &mut encoder, &self.lcd_view, size, &chain, 0)
            } else {
                up.output().unwrap()
            };
            if self.show_hd {
                hd_bind_group = Some(Self::hd_group(&self.device, &self.hd_bgl, &self.hd_texture, &self.linear, view, &self.mask_texture));
            } else {
                ai_bind_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("upscaled"),
                    layout: &self.bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    ],
                }));
            }
        }
        if self.show_hd && hd_bind_group.is_none() {
            // The LCD's own pixels are the fallback.
            hd_bind_group =
                Some(Self::hd_group(&self.device, &self.hd_bgl, &self.hd_texture, &self.linear, &self.lcd_view, &self.mask_texture));
        }
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
            if let Some(((x, y, w, h), display, _)) = lcd {
                let (fw, fh) = (self.config.width as f32, self.config.height as f32);
                let (x0, y0) = (x.max(0.0), y.max(0.0));
                let (w, h) = (w.min(fw - x0), h.min(fh - y0));
                if w >= 1.0 && h >= 1.0 {
                    pass.set_viewport(x0, y0, w, h, 0.0, 1.0);
                    match (&ai_bind_group, display) {
                        // A TV picture: drawn as it is (the AI networks are
                        // trained for the 160x160 LCD; Ai shows as Sharp).
                        _ if tv.is_some() => {
                            pass.set_pipeline(match display {
                                Display::Pixel => &self.pixel,
                                Display::Ai | Display::Sharp | Display::Lcd => &self.sharp,
                                Display::Smooth => &self.smooth,
                            });
                            pass.set_bind_group(0, tv.unwrap(), &[]);
                        }
                        _ if hd_bind_group.is_some() => {
                            pass.set_pipeline(&self.hd_pipeline);
                            pass.set_bind_group(0, hd_bind_group.as_ref().unwrap(), &[]);
                        }
                        (Some(group), Display::Ai) => {
                            pass.set_pipeline(&self.resample);
                            pass.set_bind_group(0, group, &[]);
                        }
                        _ => {
                            pass.set_pipeline(match display {
                                Display::Pixel => &self.pixel,
                                Display::Ai | Display::Sharp => &self.sharp,
                                Display::Lcd => &self.lcd,
                                Display::Smooth => &self.smooth,
                            });
                            pass.set_bind_group(0, &self.bind_group, &[]);
                        }
                    }
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

#[cfg(test)]
mod tests {
    /// wgpu only reports shader errors when the window opens; check here.
    #[test]
    fn lcd_shaders_parse_and_validate() {
        let module = naga::front::wgsl::parse_str(super::SHADER)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(super::SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("shader validates");
        for entry in ["fs", "fs_sharp", "fs_lcd", "fs_smooth", "fs_resample"] {
            assert!(module.entry_points.iter().any(|e| e.name == entry), "{entry}");
        }
        let hd = naga::front::wgsl::parse_str(super::HD_SHADER).unwrap_or_else(|e| panic!("{}", e.emit_to_string(super::HD_SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&hd)
            .expect("HD shader validates");
    }
}
