//! Embeds the frog icon into ribbit.exe on Windows, so the taskbar, Explorer
//! and Alt-Tab show it. The .ico is drawn here by the same code the window
//! uses (ribbit-frontend/src/frog_glyph.rs); no image file is checked in.

include!("../ribbit-frontend/src/frog_glyph.rs");

fn main() {
    println!("cargo:rerun-if-changed=../ribbit-frontend/src/frog_glyph.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("ribbit.ico");
    std::fs::write(&ico, ico_bytes(&[16, 24, 32, 48, 64, 128, 256])).unwrap();
    let rc = out.join("ribbit.rc");
    std::fs::write(&rc, format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "\\\\"))).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().unwrap();
}

/// One 32-bpp BMP image (BITMAPINFOHEADER + bottom-up BGRA + AND mask).
fn bmp(size: usize) -> Vec<u8> {
    let rgba = frog_glyph_rgba(size as u32);
    let mut v = Vec::new();
    let mask_row = size.div_ceil(32) * 4;
    let push32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    push32(&mut v, 40);
    push32(&mut v, size as u32);
    push32(&mut v, 2 * size as u32); // XOR + AND masks
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    for _ in 0..6 {
        push32(&mut v, 0);
    }
    let px = |x: usize, y: usize| -> [u8; 4] { rgba[(y * size + x) * 4..][..4].try_into().unwrap() };
    for y in (0..size).rev() {
        for x in 0..size {
            let [r, g, b, a] = px(x, y);
            v.extend_from_slice(&[b, g, r, a]);
        }
    }
    for y in (0..size).rev() {
        let mut row = vec![0u8; mask_row];
        for x in 0..size {
            if px(x, y)[3] < 128 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        v.extend_from_slice(&row);
    }
    v
}

fn ico_bytes(sizes: &[usize]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| bmp(s)).collect();
    let mut v = vec![0, 0, 1, 0];
    v.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&s, img) in sizes.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        v.extend_from_slice(&[dim, dim, 0, 0]);
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&(img.len() as u32).to_le_bytes());
        v.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += img.len();
    }
    for img in images {
        v.extend_from_slice(&img);
    }
    v
}
