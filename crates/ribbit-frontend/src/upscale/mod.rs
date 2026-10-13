//! Neural-network upscaling of the LCD image for display (Anime4K v3.2 CNN
//! models, MIT, ported to WGSL by `scripts/gen-anime4k.py`). Display only:
//! nothing here changes what is emulated.
//!
//! A chain of networks runs on the picture: x2 upscalers (their passes run
//! at the input size, then a depth-to-space pass adds the predicted detail
//! to a bilinear 2x of the input) and same-size restorers (add a predicted
//! correction). Intermediate values can be negative, so every texture is
//! Rgba16Float.

mod anime4k_l;
mod anime4k_m;
mod anime4k_s;
mod anime4k_vl;
mod artcnn_c4f16;
mod artcnn_c4f32;
mod restore_m;
mod restore_soft_m;
mod restore_soft_vl;
mod restore_vl;
mod scalefx;
mod sharpen;

use std::collections::HashMap;

pub struct Pass {
    pub desc: &'static str,
    /// Textures bound at @binding(0..), by name ("MAIN" = the stage input).
    pub inputs: &'static [&'static str],
    /// Render targets at @location(0..), by name; one of them is "MAIN" in
    /// the stage's last pass.
    pub outputs: &'static [&'static str],
    pub kind: PassKind,
    pub wgsl: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PassKind {
    /// Output the size of its inputs.
    Conv,
    /// Output twice the size: the stage result.
    DepthToSpace,
    /// Output three times the size: the stage result.
    Scale3,
}

/// One stage: an Anime4K x2 upscaler or same-size restorer, or ScaleFX
/// (pixel-art edge reconstruction, x3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Network {
    UpscaleS,
    UpscaleM,
    UpscaleL,
    UpscaleVL,
    RestoreM,
    RestoreVL,
    RestoreSoftM,
    RestoreSoftVL,
    ScaleFx,
    ArtCnnC4F16,
    ArtCnnC4F32,
    SharpenLow,
    SharpenMedium,
    SharpenHigh,
    SharpenMax,
}

impl Network {
    pub const ALL: [Network; 15] = [
        Network::UpscaleS,
        Network::UpscaleM,
        Network::UpscaleL,
        Network::UpscaleVL,
        Network::RestoreM,
        Network::RestoreVL,
        Network::RestoreSoftM,
        Network::RestoreSoftVL,
        Network::ScaleFx,
        Network::ArtCnnC4F16,
        Network::ArtCnnC4F32,
        Network::SharpenLow,
        Network::SharpenMedium,
        Network::SharpenHigh,
        Network::SharpenMax,
    ];

    pub fn passes(self) -> &'static [Pass] {
        match self {
            Network::UpscaleS => anime4k_s::PASSES,
            Network::UpscaleM => anime4k_m::PASSES,
            Network::UpscaleL => anime4k_l::PASSES,
            Network::UpscaleVL => anime4k_vl::PASSES,
            Network::RestoreM => restore_m::PASSES,
            Network::RestoreVL => restore_vl::PASSES,
            Network::RestoreSoftM => restore_soft_m::PASSES,
            Network::RestoreSoftVL => restore_soft_vl::PASSES,
            Network::ScaleFx => scalefx::PASSES,
            Network::ArtCnnC4F16 => artcnn_c4f16::PASSES,
            Network::ArtCnnC4F32 => artcnn_c4f32::PASSES,
            Network::SharpenLow => sharpen::LOW,
            Network::SharpenMedium => sharpen::MEDIUM,
            Network::SharpenHigh => sharpen::HIGH,
            Network::SharpenMax => sharpen::MAX,
        }
    }

    /// Output size relative to input.
    pub fn scale(self) -> u32 {
        match self.passes().last().unwrap().kind {
            PassKind::Conv => 1,
            PassKind::DepthToSpace => 2,
            PassKind::Scale3 => 3,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "up_s" => Network::UpscaleS,
            "up_m" => Network::UpscaleM,
            "up_l" => Network::UpscaleL,
            "up_vl" => Network::UpscaleVL,
            "restore_m" => Network::RestoreM,
            "restore_vl" => Network::RestoreVL,
            "soft_m" => Network::RestoreSoftM,
            "soft_vl" => Network::RestoreSoftVL,
            "scalefx" => Network::ScaleFx,
            "art16" => Network::ArtCnnC4F16,
            "art32" => Network::ArtCnnC4F32,
            "sharp1" => Network::SharpenLow,
            "sharp2" => Network::SharpenMedium,
            "sharp3" => Network::SharpenHigh,
            "sharp4" => Network::SharpenMax,
            _ => return None,
        })
    }
}

/// Network size the user picks: bigger is sharper and slower.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Model {
    Fast,
    Balanced,
    High,
    #[default]
    Ultra,
}

impl Model {
    pub const ALL: [Model; 4] = [Model::Fast, Model::Balanced, Model::High, Model::Ultra];

    pub fn label(self) -> &'static str {
        match self {
            Model::Fast => "Fast",
            Model::Balanced => "Balanced",
            Model::High => "High",
            Model::Ultra => "Ultra",
        }
    }

    /// The x2 network for doubling number `step` (0 = first) in `style`.
    /// The big ArtCNN model is used where pictures are small and it is
    /// cheap; later doublings use the light one (2-3 ms in total to 2560
    /// pixels on an RTX 4090).
    fn upscaler(self, style: Style, step: u32) -> Network {
        match style {
            Style::Soft => match self {
                Model::Fast => Network::UpscaleS,
                Model::Balanced => Network::UpscaleM,
                Model::High => Network::UpscaleL,
                Model::Ultra => Network::UpscaleVL,
            },
            Style::Clean | Style::Cartoon => match (self, step) {
                (Model::Fast, 0) => Network::ArtCnnC4F16,
                (Model::Fast, _) => Network::UpscaleS,
                (Model::Balanced, _) => Network::ArtCnnC4F16,
                (Model::High, 0) => Network::ArtCnnC4F32,
                (Model::Ultra, 0 | 1) => Network::ArtCnnC4F32,
                _ => Network::ArtCnnC4F16,
            },
        }
    }
}

/// The look.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Style {
    /// ArtCNN (luma network trained on drawn art): crisp lines, flat fills.
    /// Best for Leapster Flash games (antialiased vector art).
    #[default]
    Clean,
    /// Anime4K: softer.
    Soft,
    /// ScaleFX rebuilds outlines first, then ArtCNN. Suits flat pixel art;
    /// on antialiased art it turns blended edge pixels into blotches.
    Cartoon,
}

/// How many pixels the upscaled picture has before it is fitted to the
/// window.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Resolution {
    /// As many as the picture covers on screen.
    #[default]
    Auto,
    X2,
    X3,
    X4,
    X16,
}

impl Resolution {
    pub const ALL: [Resolution; 5] =
        [Resolution::Auto, Resolution::X2, Resolution::X3, Resolution::X4, Resolution::X16];

    pub fn label(self) -> &'static str {
        match self {
            Resolution::Auto => "Auto (match the window)",
            Resolution::X2 => "2x (320 x 320)",
            Resolution::X3 => "3x (480 x 480)",
            Resolution::X4 => "4x (640 x 640)",
            Resolution::X16 => "16x (2560 x 2560, 4K class)",
        }
    }

    /// Target side in pixels for a picture shown `shown` pixels wide.
    pub fn target(self, size: u32, shown: f32) -> f32 {
        match self {
            Resolution::Auto => shown,
            Resolution::X2 => (size * 2) as f32,
            Resolution::X3 => (size * 3) as f32,
            Resolution::X4 => (size * 4) as f32,
            Resolution::X16 => (size * 16) as f32,
        }
    }
}

/// Sharpening after the upscale (contrast-adaptive; Max: limited unsharp
/// mask).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Sharpness {
    Off,
    Low,
    Medium,
    #[default]
    High,
    Max,
}

impl Sharpness {
    pub const ALL: [Sharpness; 5] =
        [Sharpness::Off, Sharpness::Low, Sharpness::Medium, Sharpness::High, Sharpness::Max];

    pub fn label(self) -> &'static str {
        match self {
            Sharpness::Off => "Off",
            Sharpness::Low => "Low",
            Sharpness::Medium => "Medium",
            Sharpness::High => "High",
            Sharpness::Max => "Max",
        }
    }

    fn network(self) -> Option<Network> {
        match self {
            Sharpness::Off => None,
            Sharpness::Low => Some(Network::SharpenLow),
            Sharpness::Medium => Some(Network::SharpenMedium),
            Sharpness::High => Some(Network::SharpenHigh),
            Sharpness::Max => Some(Network::SharpenMax),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AiSettings {
    pub style: Style,
    pub model: Model,
    pub resolution: Resolution,
    pub sharpness: Sharpness,
}

impl AiSettings {
    /// The networks that take a `size` picture to about `target` pixels:
    /// x2 steps while the result is over 1.2x short of the target
    /// (Anime4K's own rule), never beyond `MAX_SIDE`.
    pub fn chain(&self, size: u32, target: f32) -> Vec<Network> {
        let mut chain = Vec::new();
        let mut side = size;
        if self.style == Style::Cartoon {
            chain.push(Network::ScaleFx);
            side *= 3;
        }
        let mut step = 0;
        while side as f32 * 1.2 < target && side * 2 <= MAX_SIDE {
            chain.push(self.model.upscaler(self.style, step));
            side *= 2;
            step += 1;
        }
        if chain.is_empty() {
            chain.push(self.model.upscaler(self.style, 0));
        }
        chain.extend(self.sharpness.network());
        chain
    }
}

/// Largest upscaled picture (160 x 16). Bigger costs GPU memory for no
/// visible gain on today's screens.
pub const MAX_SIDE: u32 = 2560;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

const VERTEX: &str = r#"
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
}
"#;

/// Full shader source of a pass (vertex stage + generated fragment stage).
pub fn shader_source(pass: &Pass) -> String {
    format!("{VERTEX}\n{}", pass.wgsl)
}

struct Compiled {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

struct Stage {
    network: Network,
    /// Pass outputs, by name; "MAIN" is the stage result.
    views: HashMap<&'static str, wgpu::TextureView>,
    bind_groups: Vec<wgpu::BindGroup>,
    /// Render targets of each pass.
    targets: Vec<Vec<wgpu::TextureView>>,
}

#[derive(Default)]
pub struct Upscaler {
    compiled: HashMap<Network, Vec<Compiled>>,
    stages: Vec<Stage>,
    /// Chain, input size and input identity the stages were built for.
    built: Option<(Vec<Network>, u32, usize)>,
}

fn compile(device: &wgpu::Device, network: Network) -> Vec<Compiled> {
    network
        .passes()
        .iter()
        .map(|p| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(p.desc),
                source: wgpu::ShaderSource::Wgsl(shader_source(p).into()),
            });
            let entries: Vec<_> = (0..p.inputs.len() as u32)
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                })
                .collect();
            let layout = device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(p.desc), entries: &entries });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(p.desc),
                bind_group_layouts: &[Some(&layout)],
                ..Default::default()
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(p.desc),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &vec![Some(FORMAT.into()); p.outputs.len()],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
            Compiled { pipeline, layout }
        })
        .collect()
}

impl Upscaler {
    /// Whether the GPU can render to the intermediate format.
    pub fn supported(adapter: &wgpu::Adapter) -> bool {
        adapter.get_texture_format_features(FORMAT).allowed_usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
    }

    /// (Re)creates the textures for `chain` applied to a square `input` of
    /// side `size`. `input_id` changes when the input view is recreated.
    fn build(
        &mut self,
        device: &wgpu::Device,
        input: &wgpu::TextureView,
        size: u32,
        chain: &[Network],
        input_id: usize,
    ) {
        if self.built.as_ref().is_some_and(|(c, s, i)| c == chain && *s == size && *i == input_id) {
            return;
        }
        self.stages.clear();
        let texture = |side: u32, label: &str| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let mut side = size;
        for &network in chain {
            let compiled = self.compiled.entry(network).or_insert_with(|| compile(device, network));
            let stage_input = match self.stages.last() {
                None => input.clone(),
                Some(prev) => prev.views["MAIN"].clone(),
            };
            let mut views: HashMap<&'static str, wgpu::TextureView> = HashMap::new();
            let mut targets = Vec::new();
            let mut bind_groups = Vec::new();
            for (p, c) in network.passes().iter().zip(compiled.iter()) {
                let inputs: Vec<_> = p
                    .inputs
                    .iter()
                    .map(|name| if *name == "MAIN" { stage_input.clone() } else { views[name].clone() })
                    .collect();
                let entries: Vec<_> = inputs
                    .iter()
                    .enumerate()
                    .map(|(i, view)| wgpu::BindGroupEntry {
                        binding: i as u32,
                        resource: wgpu::BindingResource::TextureView(view),
                    })
                    .collect();
                bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(p.desc),
                    layout: &c.layout,
                    entries: &entries,
                }));
                let out_side = match p.kind {
                    PassKind::Conv => side,
                    PassKind::DepthToSpace => side * 2,
                    PassKind::Scale3 => side * 3,
                };
                let outs: Vec<_> = p
                    .outputs
                    .iter()
                    .map(|name| {
                        let view = texture(out_side, name);
                        views.insert(name, view.clone());
                        view
                    })
                    .collect();
                targets.push(outs);
            }
            side *= network.scale();
            self.stages.push(Stage { network, views, bind_groups, targets });
        }
        self.built = Some((chain.to_vec(), size, input_id));
    }

    /// The result of the last run.
    pub fn output(&self) -> Option<&wgpu::TextureView> {
        self.stages.last().map(|s| &s.views["MAIN"])
    }

    /// Records `chain` applied to `input` (side `size`) and returns the result.
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        size: u32,
        chain: &[Network],
        input_id: usize,
    ) -> &wgpu::TextureView {
        assert!(!chain.is_empty());
        self.build(device, input, size, chain, input_id);
        for stage in &self.stages {
            let compiled = &self.compiled[&stage.network];
            for ((c, bind_group), targets) in compiled.iter().zip(&stage.bind_groups).zip(&stage.targets) {
                let attachments: Vec<_> = targets
                    .iter()
                    .map(|view| {
                        Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })
                    })
                    .collect();
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("upscale"),
                    color_attachments: &attachments,
                    ..Default::default()
                });
                pass.set_pipeline(&c.pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        &self.stages.last().unwrap().views["MAIN"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_generated_pass_validates() {
        for network in Network::ALL {
            let passes = network.passes();
            assert!(passes.last().unwrap().outputs.contains(&"MAIN"), "{network:?}");
            for p in passes {
                let src = shader_source(p);
                let module = naga::front::wgsl::parse_str(&src)
                    .unwrap_or_else(|e| panic!("{}: {}", p.desc, e.emit_to_string(&src)));
                naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
                    .validate(&module)
                    .unwrap_or_else(|e| panic!("{}: {e:?}", p.desc));
                // Every input is the stage input or an earlier pass's output.
                for i in p.inputs {
                    assert!(*i == "MAIN" || passes.iter().any(|q| q.outputs.contains(i)), "{}: {i}", p.desc);
                }
                // Portable limit: 32 bytes of render targets per pass.
                assert!(p.outputs.len() <= 4, "{}", p.desc);
            }
        }
    }

    #[test]
    fn upscalers_double_and_restorers_keep_the_size() {
        assert_eq!(Network::UpscaleM.scale(), 2);
        assert_eq!(Network::RestoreVL.scale(), 1);
        assert_eq!(Network::ScaleFx.scale(), 3);
    }

    #[test]
    fn chains_reach_the_target_by_the_1_2x_rule() {
        use Network::*;
        let soft = AiSettings {
            style: Style::Soft,
            model: Model::Balanced,
            resolution: Resolution::Auto,
            sharpness: Sharpness::Off,
        };
        assert_eq!(soft.chain(160, 150.0), vec![UpscaleM], "never empty");
        assert_eq!(soft.chain(160, 640.0), vec![UpscaleM; 2]);
        assert_eq!(soft.chain(160, 2160.0), vec![UpscaleM; 4]);
        assert_eq!(soft.chain(160, 10_000.0), vec![UpscaleM; 4], "capped at 2560");
        let cartoon = AiSettings {
            style: Style::Cartoon,
            model: Model::High,
            resolution: Resolution::Auto,
            sharpness: Sharpness::Off,
        };
        assert_eq!(cartoon.chain(160, 480.0), vec![ScaleFx]);
        assert_eq!(cartoon.chain(160, 1080.0), vec![ScaleFx, ArtCnnC4F32]);
        assert_eq!(cartoon.chain(160, 2160.0), vec![ScaleFx, ArtCnnC4F32, ArtCnnC4F16]);
        let clean = AiSettings::default();
        assert_eq!(clean.chain(160, 2160.0), vec![ArtCnnC4F32, ArtCnnC4F32, ArtCnnC4F16, ArtCnnC4F16, SharpenHigh]);
        let fast = AiSettings { model: Model::Fast, ..clean };
        assert_eq!(fast.chain(160, 640.0), vec![ArtCnnC4F16, UpscaleS, SharpenHigh]);
        assert_eq!(Resolution::X16.target(160, 500.0), 2560.0);
        assert_eq!(Resolution::Auto.target(160, 500.0), 500.0);
    }
}
