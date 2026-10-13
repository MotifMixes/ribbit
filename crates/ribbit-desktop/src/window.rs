//! Desktop window: winit for windowing/input, wgpu for presentation.
//!
//! The window shows exactly what `Machine::render_frame` decodes from
//! emulated VRAM, scaled by an integer factor. Nothing is drawn by the
//! frontend itself.

use anyhow::{Context, Result};
use ribbit_core::{Machine, StopReason};
use ribbit_debug::Breakpoints;
use ribbit_input::Button;
use ribbit_video::{HEIGHT, WIDTH};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Real-time pacing: never run more than this much emulated time per frame,
/// and drop (rather than race to catch up) any lag beyond `MAX_LAG`.
const MAX_SLICE: std::time::Duration = std::time::Duration::from_millis(100);
const MAX_LAG: std::time::Duration = std::time::Duration::from_millis(250);

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

// Display-only smoothing: Catmull-Rom bicubic over the 160x160 LCD image.
// It changes how the emulated picture is shown, never what is emulated.
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

/// How the 160x160 LCD image is scaled to the window (display only).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Display {
    /// Exact pixels, whole-number scale.
    Pixel,
    /// Catmull-Rom bicubic, fills the window.
    Smooth,
}

/// Runs the window; returns the machine afterwards (e.g. to save the EEPROM).
pub fn run(machine: Machine, saves: crate::saves::SavePaths) -> Result<Machine> {
    let event_loop = EventLoop::new().context("creating event loop")?;
    let mut app = App {
        machine,
        gpu: None,
        stopped: None,
        frame: Box::new([0; WIDTH * HEIGHT]),
        error: None,
        display: Display::Smooth,
        cursor: None,
        mouse_down: false,
        pace: None,
        speed: (std::time::Instant::now(), 0, 100.0),
        audio: crate::audio_out::AudioOut::open(),
        audio_buf: Vec::new(),
        saves,
        autosave: Default::default(),
    };
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(err) => Err(err),
        None => Ok(app.machine),
    }
}

struct App {
    machine: Machine,
    gpu: Option<Gpu>,
    stopped: Option<StopReason>,
    frame: Box<[u32; WIDTH * HEIGHT]>,
    error: Option<anyhow::Error>,
    display: Display,
    /// Last cursor position in window pixels.
    cursor: Option<(f64, f64)>,
    mouse_down: bool,
    /// Wall-clock origin and the CPU cycle count at that moment.
    pace: Option<(std::time::Instant, u64)>,
    /// Speed meter: (window start, cycles at start, last measured % of real time).
    speed: (std::time::Instant, u64, f64),
    /// Host audio (None if no output device).
    audio: Option<crate::audio_out::AudioOut>,
    audio_buf: Vec<i16>,
    /// Save files, written soon after the game changes them.
    saves: crate::saves::SavePaths,
    autosave: crate::saves::AutoSave,
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    smooth_pipeline: wgpu::RenderPipeline,
}

impl Gpu {
    fn new(window: Arc<Window>) -> Result<Self> {
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
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("surface not supported by adapter")?;
        surface.configure(&device, &config);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lcd"),
            size: wgpu::Extent3d { width: WIDTH as u32, height: HEIGHT as u32, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
        let make_pipeline = |fragment: &str| {
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
        let pipeline = make_pipeline("fs");
        let smooth_pipeline = make_pipeline("fs_smooth");
        Ok(Self { window, surface, device, queue, config, texture, bind_group, pipeline, smooth_pipeline })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Where the LCD image is drawn: (x, y, width, height) in window pixels.
    fn viewport(&self, display: Display) -> (u32, u32, u32, u32) {
        let (w, h) = (self.config.width, self.config.height);
        let (vw, vh) = match display {
            Display::Pixel => {
                let scale = (w / WIDTH as u32).min(h / HEIGHT as u32).max(1);
                (WIDTH as u32 * scale, HEIGHT as u32 * scale)
            }
            Display::Smooth => (w.min(h), w.min(h)),
        };
        (w.saturating_sub(vw) / 2, h.saturating_sub(vh) / 2, vw, vh)
    }

    fn present(&mut self, frame: &[u32; WIDTH * HEIGHT], display: Display) -> Result<()> {
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
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lcd"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            // Centred square: whole-number scale for Pixel, any size for Smooth.
            let (x, y, vw, vh) = self.viewport(display);
            pass.set_viewport(x as f32, y as f32, vw as f32, vh as f32, 0.0, 1.0);
            pass.set_pipeline(match display {
                Display::Pixel => &self.pipeline,
                Display::Smooth => &self.smooth_pipeline,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(output);
        Ok(())
    }
}

fn map_key(code: KeyCode) -> Option<Button> {
    Some(match code {
        KeyCode::ArrowUp => Button::Up,
        KeyCode::ArrowDown => Button::Down,
        KeyCode::ArrowLeft => Button::Left,
        KeyCode::ArrowRight => Button::Right,
        KeyCode::KeyZ => Button::A,
        KeyCode::KeyX => Button::B,
        KeyCode::KeyH => Button::Hint,
        KeyCode::Enter => Button::Pause,
        KeyCode::Backspace => Button::Home,
        _ => return None,
    })
}

impl App {
    fn title(&self) -> String {
        let touch =
            if self.machine.experimental_touch_enabled() { " - EXPERIMENTAL touch: mouse = stylus" } else { "" };
        let mode = match self.display {
            Display::Pixel => "pixel",
            Display::Smooth => "smooth",
        };
        let speed = self.speed.2;
        match &self.stopped {
            None => format!("Ribbit (in development) - {speed:.0}% speed - display: {mode} (F to switch){touch}"),
            Some(StopReason::Cpu(err)) => format!("Ribbit: CPU stopped: {err}"),
            Some(other) => format!("Ribbit: stopped: {other:?}"),
        }
    }
}

impl App {
    /// EXPERIMENTAL touch only: the left mouse button is the stylus. The LCD
    /// pixel under the cursor is converted to a raw ADC value with MAME's
    /// crosshair mapping (0..=0x7F0 across 160 pixels); the BIOS's own
    /// calibration then maps it back to the screen.
    fn update_stylus(&mut self) {
        let (Some(gpu), Some(touch)) = (&self.gpu, self.machine.io.touch.as_mut()) else {
            return;
        };
        touch.live = match (self.mouse_down, self.cursor) {
            (true, Some((cx, cy))) => {
                let (x, y, w, h) = gpu.viewport(self.display);
                let px = ((cx - x as f64) / w as f64 * WIDTH as f64).floor();
                let py = ((cy - y as f64) / h as f64 * HEIGHT as f64).floor();
                let inside = (0.0..WIDTH as f64).contains(&px) && (0.0..HEIGHT as f64).contains(&py);
                let raw = |p: f64| ((p as u32 * 0x7F0 + 79) / 159).min(0x7F0);
                inside.then(|| (raw(px), raw(py)))
            }
            _ => None,
        };
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.title())
            .with_inner_size(PhysicalSize::new(WIDTH as u32 * 4, HEIGHT as u32 * 4))
            .with_min_inner_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32))
            .with_window_icon(crate::icon::window_icon());
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows as _;
            attrs.with_taskbar_icon(crate::icon::taskbar_icon())
        };
        let result = event_loop.create_window(attrs).context("creating window").and_then(|w| Gpu::new(Arc::new(w)));
        match result {
            Ok(gpu) => self.gpu = Some(gpu),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(code), state, repeat: false, .. },
                ..
            } => {
                if code == KeyCode::KeyF && state == ElementState::Pressed {
                    self.display = match self.display {
                        Display::Pixel => Display::Smooth,
                        Display::Smooth => Display::Pixel,
                    };
                    if let Some(gpu) = &self.gpu {
                        gpu.window.set_title(&self.title());
                    }
                } else if let Some(button) = map_key(code) {
                    self.machine.buttons.set(button, state == ElementState::Pressed);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                self.update_stylus();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.mouse_down = state == ElementState::Pressed;
                self.update_stylus();
            }
            WindowEvent::RedrawRequested => {
                if self.stopped.is_none() {
                    // Run exactly as many cycles as real time allows (one cycle
                    // per step at the configured CPU clock).
                    let hz = self.machine.clock.cpu_hz;
                    let now = std::time::Instant::now();
                    let (origin, base) = *self.pace.get_or_insert((now, self.machine.cpu.cycles));
                    let target = base + (now.duration_since(origin).as_secs_f64() * hz as f64) as u64;
                    let behind = target.saturating_sub(self.machine.cpu.cycles);
                    let max_lag = (MAX_LAG.as_secs_f64() * hz as f64) as u64;
                    if behind > max_lag {
                        // Too slow (or paused): resynchronise instead of racing.
                        self.pace = Some((now, self.machine.cpu.cycles));
                    }
                    let steps = behind.min(max_lag).min((MAX_SLICE.as_secs_f64() * hz as f64) as u64);
                    let report = self.machine.run(steps, &Breakpoints::default());
                    let out = &mut self.machine.io.voices.output;
                    self.audio_buf.resize(out.len(), 0);
                    out.pop_into(&mut self.audio_buf);
                    if let Some(a) = &mut self.audio {
                        a.push(&self.audio_buf);
                    }
                    if let Err(e) = self.autosave.tick(&self.saves, &mut self.machine, now) {
                        tracing::warn!(target: "ribbit::saves", "autosave failed: {e:#}");
                    }
                    let elapsed = now.duration_since(self.speed.0);
                    if elapsed.as_secs_f64() >= 1.0 {
                        let done = self.machine.cpu.cycles - self.speed.1;
                        self.speed =
                            (now, self.machine.cpu.cycles, 100.0 * done as f64 / (elapsed.as_secs_f64() * hz as f64));
                        if let Some(gpu) = &self.gpu {
                            gpu.window.set_title(&self.title());
                        }
                    }
                    if report.stop != StopReason::StepLimit {
                        tracing::warn!(target: "ribbit::boot", ?report, "emulation stopped");
                        self.stopped = Some(report.stop);
                        if let Some(gpu) = &self.gpu {
                            gpu.window.set_title(&self.title());
                        }
                    }
                }
                self.machine.render_frame(&mut self.frame);
                if let Some(gpu) = &mut self.gpu
                    && let Err(e) = gpu.present(&self.frame, self.display)
                {
                    self.error = Some(e);
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}
