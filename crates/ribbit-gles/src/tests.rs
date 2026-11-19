use super::*;

/// Flat memory at address 0 for the arrays.
struct Ram(Vec<u8>);

impl Memory for Ram {
    fn read(&mut self, addr: u32, len: usize) -> Option<Vec<u8>> {
        self.0.get(addr as usize..addr as usize + len).map(<[u8]>::to_vec)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> bool {
        match self.0.get_mut(addr as usize..addr as usize + data.len()) {
            Some(d) => {
                d.copy_from_slice(data);
                true
            }
            None => false,
        }
    }
}

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn fixed(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| ((f * 65536.) as i32).to_le_bytes()).collect()
}

/// A 2D setup as Lightning makes it: ortho in pixels, y down.
fn pixels_ortho(g: &mut Gl, w: f32, h: f32) {
    g.matrix_mode(gl::PROJECTION);
    g.load_identity();
    g.ortho(0., w, h, 0., -1., 1.);
    g.matrix_mode(gl::MODELVIEW);
    g.load_identity();
}

#[test]
fn clears_and_fills_a_triangle() {
    let mut g = Gl::new(8, 8);
    g.clear_color([0., 0., 1., 1.]);
    g.clear(gl::COLOR_BUFFER_BIT);
    pixels_ortho(&mut g, 8., 8.);
    let mut mem = Ram(floats(&[0., 0., 8., 0., 0., 8.]));
    g.set_array(gl::VERTEX_ARRAY, 2, gl::FLOAT, 0, 0);
    g.client_state(gl::VERTEX_ARRAY, true);
    g.color([1., 0., 0., 1.]);
    g.draw_arrays(&mut mem, gl::TRIANGLES, 0, 3);
    let img = g.surface.to_xrgb();
    // Top-left (screen) is inside, bottom-right outside.
    assert_eq!(img[0], 0xFF0000);
    assert_eq!(img[7 * 8 + 7], 0x0000FF);
    assert_eq!(g.get_error(), 0);
}

#[test]
fn shared_edges_are_drawn_once() {
    // Two triangles of a quad with blending ADD: no pixel twice.
    let mut g = Gl::new(16, 16);
    g.clear_color([0., 0., 0., 0.]);
    g.clear(gl::COLOR_BUFFER_BIT);
    pixels_ortho(&mut g, 16., 16.);
    let mut mem = Ram(floats(&[1.5, 1.5, 13.25, 2.5, 2., 12.75, 13.25, 2.5, 12.5, 13.5, 2., 12.75]));
    g.set_array(gl::VERTEX_ARRAY, 2, gl::FLOAT, 0, 0);
    g.client_state(gl::VERTEX_ARRAY, true);
    g.set_cap(gl::BLEND, true);
    g.blend_func(gl::ONE, gl::ONE);
    g.color([0.25, 0., 0., 0.]);
    g.draw_arrays(&mut mem, gl::TRIANGLES, 0, 6);
    let max = g.surface.color.iter().map(|c| c & 0xFF).max().unwrap();
    assert_eq!(max, 64);
}

#[test]
fn textured_quad_modulate_and_blend() {
    let mut g = Gl::new(4, 4);
    g.clear_color([1., 1., 1., 1.]);
    g.clear(gl::COLOR_BUFFER_BIT);
    pixels_ortho(&mut g, 4., 4.);
    // A 2x2 texture: red opaque, green half, blue clear, white opaque.
    let tex = [255u8, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 255, 255, 255, 255];
    let names = g.gen_names(1);
    g.bind_texture(gl::TEXTURE_2D, names[0]);
    g.tex_parameter(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST);
    g.tex_parameter(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST);
    g.tex_image(0, gl::RGBA, 2, 2, gl::RGBA, gl::UNSIGNED_BYTE, Some(&tex));
    g.set_cap(gl::TEXTURE_2D, true);
    g.set_cap(gl::BLEND, true);
    g.blend_func(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA);
    // Fixed-point strip: x, y then s, t.
    let mut data = fixed(&[0., 0., 4., 0., 0., 4., 4., 4.]);
    data.extend(fixed(&[0., 0., 1., 0., 0., 1., 1., 1.]));
    let mut mem = Ram(data);
    g.set_array(gl::VERTEX_ARRAY, 2, gl::FIXED, 0, 0);
    g.set_array(gl::TEXTURE_COORD_ARRAY, 2, gl::FIXED, 0, 32);
    g.client_state(gl::VERTEX_ARRAY, true);
    g.client_state(gl::TEXTURE_COORD_ARRAY, true);
    g.draw_arrays(&mut mem, gl::TRIANGLE_STRIP, 0, 4);
    let img = g.surface.to_xrgb();
    // Screen row 0 (y down in the ortho) is texture row 0: red, then green over white.
    assert_eq!(img[0], 0xFF0000);
    assert_eq!(img[3], 0x7FFF7F);
    // Texture row 1: blue fully clear (white stays), white.
    assert_eq!(img[3 * 4], 0xFFFFFF);
    assert_eq!(img[3 * 4 + 3], 0xFFFFFF);
}

#[test]
fn matrix_stack_and_errors() {
    let mut g = Gl::new(2, 2);
    g.translate(1., 2., 3.);
    g.push_matrix();
    g.scale(2., 2., 2.);
    assert_eq!(g.matrix(gl::MODELVIEW)[0], 2.);
    g.pop_matrix();
    assert_eq!(g.matrix(gl::MODELVIEW)[12], 1.);
    g.pop_matrix();
    assert_eq!(g.get_error(), gl::STACK_UNDERFLOW);
    assert_eq!(g.get_error(), 0);
}

#[test]
fn etc1_block_decodes() {
    // Individual mode, both halves base 0x88 grey, table 0, all pixel indices 0 (+2).
    let block = [0x88, 0x88, 0x88, 0x00, 0, 0, 0, 0];
    let px = decode_etc1(4, 4, &block).unwrap();
    assert!(px.iter().all(|&p| p == 0xFF8A8A8A));
}

#[test]
fn framebuffer_draws_into_its_texture() {
    let mut g = Gl::new(8, 8);
    g.clear_color([0., 0., 1., 1.]);
    g.clear(gl::COLOR_BUFFER_BIT);
    let tex = g.gen_names(1)[0];
    g.bind_texture(gl::TEXTURE_2D, tex);
    g.tex_image(0, gl::RGBA, 4, 2, gl::RGBA, gl::UNSIGNED_BYTE, None);
    let fb = g.gen_names(1)[0];
    g.bind_framebuffer(gl::FRAMEBUFFER_OES, fb);
    g.framebuffer_attach(gl::COLOR_ATTACHMENT0_OES, tex, false);
    assert_eq!(g.check_framebuffer_status(), gl::FRAMEBUFFER_COMPLETE_OES);
    assert_eq!((g.surface.width, g.surface.height), (4, 2));
    g.clear_color([1., 0., 0., 1.]);
    g.clear(gl::COLOR_BUFFER_BIT);
    g.bind_framebuffer(gl::FRAMEBUFFER_OES, 0);
    // The texture has the red, the window kept its blue.
    assert!(g.texture(tex).unwrap().pixels.iter().all(|&p| p == 0xFF00_00FF));
    assert_eq!((g.surface.width, g.surface.height), (8, 8));
    assert_eq!(g.surface.to_xrgb()[0], 0x0000FF);
    assert_eq!(g.get_error(), 0);
}
