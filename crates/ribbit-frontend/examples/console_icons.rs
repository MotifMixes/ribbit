//! Renders every console icon offscreen to one PNG sheet (no window, no GPU):
//! a row per console with its tile at 128, 64 and 32 pixels, then its chip as
//! the library draws it, at 1 and 2 pixels per point. egui tessellates the
//! shapes; a small software rasteriser fills the triangles.
//!   cargo run -p ribbit-frontend --example console_icons -- <out.png>

#[path = "../src/console_icons.rs"]
#[allow(dead_code)]
mod console_icons;

use ribbit_memory::known::Hardware;
use std::collections::HashMap;

const ALL: [Hardware; 12] = [
    Hardware::Leapster,
    Hardware::LeapsterLmax,
    Hardware::Leapster2,
    Hardware::LeapsterTv,
    Hardware::Explorer,
    Hardware::Didj,
    Hardware::Zippity,
    Hardware::RockItTwist,
    Hardware::LeapPad,
    Hardware::LeapPadExplorer,
    Hardware::LeapPad2,
    Hardware::ClickStart,
];

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    /// Fills a triangle: colours interpolated, times the texture (nearest),
    /// blended premultiplied over what is there.
    fn triangle(&mut self, v: [&egui::epaint::Vertex; 3], tex: Option<&egui::ColorImage>) {
        let [a, b, c] = v;
        let area = (b.pos.x - a.pos.x) * (c.pos.y - a.pos.y) - (b.pos.y - a.pos.y) * (c.pos.x - a.pos.x);
        if area.abs() < 1e-6 {
            return;
        }
        let x0 = a.pos.x.min(b.pos.x).min(c.pos.x).floor().max(0.0) as usize;
        let y0 = a.pos.y.min(b.pos.y).min(c.pos.y).floor().max(0.0) as usize;
        let x1 = (a.pos.x.max(b.pos.x).max(c.pos.x).ceil() as usize).min(self.w);
        let y1 = (a.pos.y.max(b.pos.y).max(c.pos.y).ceil() as usize).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((b.pos.x - p.x) * (c.pos.y - p.y) - (b.pos.y - p.y) * (c.pos.x - p.x)) / area;
                let w1 = ((c.pos.x - p.x) * (a.pos.y - p.y) - (c.pos.y - p.y) * (a.pos.x - p.x)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let lerp = |f: &dyn Fn(&egui::epaint::Vertex) -> f32| f(a) * w0 + f(b) * w1 + f(c) * w2;
                let mut s = [0usize, 1, 2, 3].map(|k| lerp(&|v| v.color.to_array()[k] as f32 / 255.0));
                if let Some(t) = tex {
                    let u = lerp(&|v| v.uv.x);
                    let vv = lerp(&|v| v.uv.y);
                    let tx = ((u * t.size[0] as f32) as usize).min(t.size[0] - 1);
                    let ty = ((vv * t.size[1] as f32) as usize).min(t.size[1] - 1);
                    let tc = t.pixels[ty * t.size[0] + tx].to_array();
                    for k in 0..4 {
                        s[k] *= tc[k] as f32 / 255.0;
                    }
                }
                let d = &mut self.px[y * self.w + x];
                for k in 0..4 {
                    d[k] = s[k] + d[k] * (1.0 - s[3]);
                }
            }
        }
    }

    fn save(&self, path: &std::path::Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut e = png::Encoder::new(file, self.w as u32, self.h as u32);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let data: Vec<u8> = self.px.iter().flat_map(|p| p.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)).collect();
        e.write_header().unwrap().write_image_data(&data).unwrap();
    }
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: console_icons <out.png>"));
    let (row, w) = (140.0f32, 620.0f32);
    let h = row * ALL.len() as f32;
    let ctx = egui::Context::default();
    let mut textures: HashMap<egui::TextureId, egui::ColorImage> = HashMap::new();
    let mut canvas = Canvas { w: w as usize, h: h as usize, px: vec![[0.11, 0.12, 0.14, 1.0]; (w * h) as usize] };
    // Two passes: the first lets egui build its font atlas.
    for pass in 0..2 {
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))), ..Default::default() };
        let mut full = ctx.run_ui(raw, |ui| {
            let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("icons")));
            for (i, &hw) in ALL.iter().enumerate() {
                let y = i as f32 * row + 6.0;
                console_icons::paint(&p, egui::Rect::from_min_size(egui::pos2(6.0, y), egui::vec2(128.0, 128.0)), 16.0, hw);
                console_icons::paint(&p, egui::Rect::from_min_size(egui::pos2(142.0, y), egui::vec2(64.0, 64.0)), 8.0, hw);
                console_icons::paint(&p, egui::Rect::from_min_size(egui::pos2(214.0, y), egui::vec2(32.0, 32.0)), 5.0, hw);
                console_icons::chip(&p, egui::pos2(142.0, y + 76.0), hw);
                // The chip at 2 pixels per point: drawn in a scaled layer below.
                let big = egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("big", i)));
                let pb = ui.ctx().layer_painter(big);
                console_icons::chip(&pb, egui::pos2(0.0, 0.0), hw);
                ui.ctx().set_transform_layer(big, egui::emath::TSTransform::new(egui::vec2(270.0, y + 4.0), 2.0));
                console_icons::paint_device(&p, egui::Rect::from_min_size(egui::pos2(270.0, y + 60.0), egui::vec2(72.0, 72.0)), hw);
            }
        });
        for (id, deltas) in &full.textures_delta.set {
            for delta in deltas.iter() {
            let egui::ImageData::Color(img) = &delta.image;
            match delta.pos {
                None => {
                    textures.insert(*id, (**img).clone());
                }
                Some([px, py]) => {
                    let t = textures.get_mut(id).expect("partial update of an unknown texture");
                    for yy in 0..img.size[1] {
                        for xx in 0..img.size[0] {
                            t.pixels[(py + yy) * t.size[0] + px + xx] = img.pixels[yy * img.size[0] + xx];
                        }
                    }
                }
            }
            }
        }
        full.textures_delta.clear();
        if pass == 1 {
            for prim in ctx.tessellate(full.shapes, 1.0) {
                if let egui::epaint::Primitive::Mesh(m) = prim.primitive {
                    let tex = textures.get(&m.texture_id);
                    for t in m.indices.chunks(3) {
                        canvas.triangle([&m.vertices[t[0] as usize], &m.vertices[t[1] as usize], &m.vertices[t[2] as usize]], tex);
                    }
                }
            }
        }
    }
    canvas.save(&out);
    println!("wrote {}", out.display());
}
