//! Runs the display upscaler on a saved frame, without a window.
//!
//! cargo run --release -p ribbit-frontend --example upscale -- IN.ppm OUT.png CHAIN
//!
//! CHAIN is a comma-separated list of networks applied in order: up_s,
//! up_m, up_l, up_vl (x2) and restore_m, restore_vl, soft_m, soft_vl (x1).
//!
//! IN is a binary PPM (e.g. from `ribbit headless --dump-frame`). Also
//! prints the GPU time of one upscale.

use ribbit_frontend::upscale::{Network, Upscaler};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: upscale IN.ppm OUT.png CHAIN");
        std::process::exit(2);
    }
    let chain: Vec<Network> =
        args[3].split(',').map(|n| Network::from_name(n).unwrap_or_else(|| panic!("unknown network {n}"))).collect();
    let (w, h, rgb) = read_ppm(&std::fs::read(&args[1]).unwrap());
    assert_eq!(w, h, "square frames only");

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).expect("GPU adapter");
    println!("adapter: {:?}", adapter.get_info().name);
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();

    let input = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let rgba: Vec<u8> = rgb.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    queue.write_texture(
        input.as_image_copy(),
        &rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: None },
        input.size(),
    );
    let input_view = input.create_view(&Default::default());
    let mut up = Upscaler::default();

    let side = w * chain.iter().map(|n| n.scale()).product::<u32>();
    let bytes_per_row = side * 8;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (bytes_per_row * side) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut timings = Vec::new();
    for round in 0..6 {
        let start = Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        let out = up.run(&device, &mut encoder, &input_view, w, &chain, 0);
        let out_texture = out.texture().clone();
        if round == 5 {
            encoder.copy_texture_to_buffer(
                out_texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: None,
                    },
                },
                out_texture.size(),
            );
        }
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        timings.push(start.elapsed());
    }
    timings.sort();
    println!("{} -> {side}x{side}: {:.2} ms (median of 6, incl. submit)", args[3], timings[3].as_secs_f64() * 1e3);

    readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = readback.slice(..).get_mapped_range().unwrap().to_vec();
    let mut out = Vec::with_capacity((side * side * 3) as usize);
    for px in data.chunks(8) {
        for c in 0..3 {
            let v = f16_to_f32(u16::from_le_bytes([px[c * 2], px[c * 2 + 1]]));
            out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(&args[2]).unwrap());
    let mut enc = png::Encoder::new(file, side, side);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().unwrap().write_image_data(&out).unwrap();
    println!("wrote {}", args[2]);
}

fn read_ppm(d: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while d[i].is_ascii_whitespace() {
            i += 1;
        }
        if d[i] == b'#' {
            while d[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        while !d[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(&d[start..i]).into_owned());
    }
    assert_eq!(fields[0], "P6");
    let (w, h): (u32, u32) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
    (w, h, d[i + 1..i + 1 + (w * h * 3) as usize].to_vec())
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let frac = (h & 0x3FF) as f32;
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}
