//! OpenGL ES 1.1 (the fixed-function Common profile) and a little EGL, drawn
//! in software. For programs Ribbit runs whose graphics go through GLES 1.1
//! instead of a chip Ribbit models: the LeapPad 2's Lightning (on the LF2000
//! it draws through Nexell's VR5 GPU, docs/leappad2.md) and the LeapPad
//! Academy's Brio-on-Android games (docs/leappad-academy.md).
//!
//! The crate knows nothing of the machine: [`Gl`] is a context with its own
//! state and frame buffer, called with plain Rust values; vertex arrays,
//! indices and buffer data the program points at are read through the
//! [`Memory`] trait. [`abi`] maps the C entry points by name onto [`Gl`] and
//! [`Egl`] for 32-bit ARM soft-float programs (arguments in r0-r3, then the
//! stack; GLfixed is 16.16), which both of those platforms are.
//!
//! What's drawn: points, lines and triangles (lists, strips, fans) with
//! perspective-correct interpolation, the modelview/projection/texture
//! matrix stacks, vertex colours or the current colour, one texture unit
//! (2D; NEAREST/LINEAR, REPEAT/CLAMP_TO_EDGE, mipmaps ignored), the texture
//! environment (REPLACE, MODULATE, DECAL, BLEND, ADD), the alpha, depth and
//! scissor tests, blending, colour/depth masks and culling. Lighting, fog,
//! the stencil buffer, multitexture and point sprites are not drawn (their
//! state is kept). Reference: the OpenGL ES 1.1.12 Common Profile
//! Specification (Khronos).

pub mod abi;
mod raster;

pub use raster::Surface;

/// Reading the program's memory (vertex arrays, indices, pixels).
pub trait Memory {
    /// `len` bytes at `addr`, or None when any is outside its memory.
    fn read(&mut self, addr: u32, len: usize) -> Option<Vec<u8>>;
    /// Writes `data` at `addr`; false when outside its memory.
    fn write(&mut self, addr: u32, data: &[u8]) -> bool;
}

/// GLenum values used here (gl.h of OpenGL ES 1.1).
pub mod gl {
    pub const NO_ERROR: u32 = 0;
    pub const INVALID_ENUM: u32 = 0x0500;
    pub const INVALID_VALUE: u32 = 0x0501;
    pub const INVALID_OPERATION: u32 = 0x0502;
    pub const STACK_OVERFLOW: u32 = 0x0503;
    pub const STACK_UNDERFLOW: u32 = 0x0504;
    pub const OUT_OF_MEMORY: u32 = 0x0505;

    pub const POINTS: u32 = 0;
    pub const LINES: u32 = 1;
    pub const LINE_LOOP: u32 = 2;
    pub const LINE_STRIP: u32 = 3;
    pub const TRIANGLES: u32 = 4;
    pub const TRIANGLE_STRIP: u32 = 5;
    pub const TRIANGLE_FAN: u32 = 6;

    pub const DEPTH_BUFFER_BIT: u32 = 0x100;
    pub const STENCIL_BUFFER_BIT: u32 = 0x400;
    pub const COLOR_BUFFER_BIT: u32 = 0x4000;

    pub const NEVER: u32 = 0x200;
    pub const LESS: u32 = 0x201;
    pub const EQUAL: u32 = 0x202;
    pub const LEQUAL: u32 = 0x203;
    pub const GREATER: u32 = 0x204;
    pub const NOTEQUAL: u32 = 0x205;
    pub const GEQUAL: u32 = 0x206;
    pub const ALWAYS: u32 = 0x207;

    pub const ZERO: u32 = 0;
    pub const ONE: u32 = 1;
    pub const SRC_COLOR: u32 = 0x300;
    pub const ONE_MINUS_SRC_COLOR: u32 = 0x301;
    pub const SRC_ALPHA: u32 = 0x302;
    pub const ONE_MINUS_SRC_ALPHA: u32 = 0x303;
    pub const DST_ALPHA: u32 = 0x304;
    pub const ONE_MINUS_DST_ALPHA: u32 = 0x305;
    pub const DST_COLOR: u32 = 0x306;
    pub const ONE_MINUS_DST_COLOR: u32 = 0x307;
    pub const SRC_ALPHA_SATURATE: u32 = 0x308;

    pub const FRONT: u32 = 0x404;
    pub const BACK: u32 = 0x405;
    pub const FRONT_AND_BACK: u32 = 0x408;
    pub const CW: u32 = 0x900;
    pub const CCW: u32 = 0x901;

    pub const CULL_FACE: u32 = 0x0B44;
    pub const LIGHTING: u32 = 0x0B50;
    pub const FOG: u32 = 0x0B60;
    pub const DEPTH_TEST: u32 = 0x0B71;
    pub const STENCIL_TEST: u32 = 0x0B90;
    pub const NORMALIZE: u32 = 0x0BA1;
    pub const ALPHA_TEST: u32 = 0x0BC0;
    pub const DITHER: u32 = 0x0BD0;
    pub const BLEND: u32 = 0x0BE2;
    pub const COLOR_LOGIC_OP: u32 = 0x0BF2;
    pub const SCISSOR_TEST: u32 = 0x0C11;
    pub const TEXTURE_2D: u32 = 0x0DE1;
    pub const POLYGON_OFFSET_FILL: u32 = 0x8037;
    pub const RESCALE_NORMAL: u32 = 0x803A;
    pub const COLOR_MATERIAL: u32 = 0x0B57;

    pub const VERTEX_ARRAY: u32 = 0x8074;
    pub const NORMAL_ARRAY: u32 = 0x8075;
    pub const COLOR_ARRAY: u32 = 0x8076;
    pub const TEXTURE_COORD_ARRAY: u32 = 0x8078;
    pub const POINT_SIZE_ARRAY_OES: u32 = 0x8B9C;

    pub const BYTE: u32 = 0x1400;
    pub const UNSIGNED_BYTE: u32 = 0x1401;
    pub const SHORT: u32 = 0x1402;
    pub const UNSIGNED_SHORT: u32 = 0x1403;
    pub const INT: u32 = 0x1404;
    pub const UNSIGNED_INT: u32 = 0x1405;
    pub const FLOAT: u32 = 0x1406;
    pub const FIXED: u32 = 0x140C;

    pub const MODELVIEW: u32 = 0x1700;
    pub const PROJECTION: u32 = 0x1701;
    pub const TEXTURE: u32 = 0x1702;

    pub const ALPHA: u32 = 0x1906;
    pub const RGB: u32 = 0x1907;
    pub const RGBA: u32 = 0x1908;
    pub const LUMINANCE: u32 = 0x1909;
    pub const LUMINANCE_ALPHA: u32 = 0x190A;
    pub const BGRA_EXT: u32 = 0x80E1;
    pub const UNSIGNED_SHORT_4_4_4_4: u32 = 0x8033;
    pub const UNSIGNED_SHORT_5_5_5_1: u32 = 0x8034;
    pub const UNSIGNED_SHORT_5_6_5: u32 = 0x8363;

    pub const FLAT: u32 = 0x1D00;
    pub const SMOOTH: u32 = 0x1D01;

    pub const NEAREST: u32 = 0x2600;
    pub const LINEAR: u32 = 0x2601;
    pub const NEAREST_MIPMAP_NEAREST: u32 = 0x2700;
    pub const LINEAR_MIPMAP_NEAREST: u32 = 0x2701;
    pub const NEAREST_MIPMAP_LINEAR: u32 = 0x2702;
    pub const LINEAR_MIPMAP_LINEAR: u32 = 0x2703;
    pub const TEXTURE_MAG_FILTER: u32 = 0x2800;
    pub const TEXTURE_MIN_FILTER: u32 = 0x2801;
    pub const TEXTURE_WRAP_S: u32 = 0x2802;
    pub const TEXTURE_WRAP_T: u32 = 0x2803;
    pub const GENERATE_MIPMAP: u32 = 0x8191;
    pub const REPEAT: u32 = 0x2901;
    pub const CLAMP_TO_EDGE: u32 = 0x812F;

    pub const TEXTURE_ENV: u32 = 0x2300;
    pub const TEXTURE_ENV_MODE: u32 = 0x2200;
    pub const TEXTURE_ENV_COLOR: u32 = 0x2201;
    pub const MODULATE: u32 = 0x2100;
    pub const DECAL: u32 = 0x2101;
    pub const ADD: u32 = 0x0104;
    pub const REPLACE: u32 = 0x1E01;
    pub const COMBINE: u32 = 0x8570;

    pub const TEXTURE0: u32 = 0x84C0;

    pub const ARRAY_BUFFER: u32 = 0x8892;
    pub const ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
    pub const ARRAY_BUFFER_BINDING: u32 = 0x8894;
    pub const ELEMENT_ARRAY_BUFFER_BINDING: u32 = 0x8895;

    pub const VENDOR: u32 = 0x1F00;
    pub const RENDERER: u32 = 0x1F01;
    pub const VERSION: u32 = 0x1F02;
    pub const EXTENSIONS: u32 = 0x1F03;

    pub const MAX_TEXTURE_SIZE: u32 = 0x0D33;
    pub const MAX_MODELVIEW_STACK_DEPTH: u32 = 0x0D36;
    pub const MAX_PROJECTION_STACK_DEPTH: u32 = 0x0D38;
    pub const MAX_TEXTURE_STACK_DEPTH: u32 = 0x0D39;
    pub const MAX_VIEWPORT_DIMS: u32 = 0x0D3A;
    pub const MAX_TEXTURE_UNITS: u32 = 0x84E2;
    pub const MAX_LIGHTS: u32 = 0x0D31;
    pub const MAX_CLIP_PLANES: u32 = 0x0D32;
    pub const VIEWPORT: u32 = 0x0BA2;
    pub const MATRIX_MODE: u32 = 0x0BA0;
    pub const TEXTURE_BINDING_2D: u32 = 0x8069;
    pub const RED_BITS: u32 = 0x0D52;
    pub const GREEN_BITS: u32 = 0x0D53;
    pub const BLUE_BITS: u32 = 0x0D54;
    pub const ALPHA_BITS: u32 = 0x0D55;
    pub const DEPTH_BITS: u32 = 0x0D56;
    pub const STENCIL_BITS: u32 = 0x0D57;
    pub const NUM_COMPRESSED_TEXTURE_FORMATS: u32 = 0x86A2;
    pub const COMPRESSED_TEXTURE_FORMATS: u32 = 0x86A3;
    pub const UNPACK_ALIGNMENT: u32 = 0x0CF5;
    pub const PACK_ALIGNMENT: u32 = 0x0D05;
    pub const MODELVIEW_MATRIX: u32 = 0x0BA6;
    pub const PROJECTION_MATRIX: u32 = 0x0BA7;
    pub const TEXTURE_MATRIX: u32 = 0x0BA8;
    pub const BLEND_SRC: u32 = 0x0BE1;
    pub const BLEND_DST: u32 = 0x0BE0;

    /// OES_compressed_paletted_texture.
    pub const PALETTE4_RGB8_OES: u32 = 0x8B90;
    pub const PALETTE4_RGBA4_OES: u32 = 0x8B93;
    pub const PALETTE8_RGBA4_OES: u32 = 0x8B98;
    pub const PALETTE8_RGB5_A1_OES: u32 = 0x8B99;
    /// OES_compressed_ETC1_RGB8_texture.
    pub const ETC1_RGB8_OES: u32 = 0x8D64;
}

/// A 4x4 matrix, column-major as GL keeps it.
pub type Mat = [f32; 16];

pub const IDENTITY: Mat = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

pub fn mat_mul(a: &Mat, b: &Mat) -> Mat {
    let mut r = [0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

fn transform(m: &Mat, v: [f32; 4]) -> [f32; 4] {
    let mut r = [0f32; 4];
    for (row, out) in r.iter_mut().enumerate() {
        *out = (0..4).map(|k| m[k * 4 + row] * v[k]).sum();
    }
    r
}

/// A texture object: level 0 only, kept as RGBA8 (0xAABBGGRR, R lowest).
#[derive(Clone, Default)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
    pub min_filter: u32,
    pub mag_filter: u32,
    pub wrap_s: u32,
    pub wrap_t: u32,
    /// The format the program gave (ALPHA/LUMINANCE/... matter to the
    /// texture environment: an ALPHA texture has no colour).
    pub format: u32,
}

impl Texture {
    fn new() -> Self {
        Self { min_filter: gl::NEAREST_MIPMAP_LINEAR, mag_filter: gl::LINEAR, wrap_s: gl::REPEAT, wrap_t: gl::REPEAT, format: gl::RGBA, ..Default::default() }
    }
}

/// A client array: where its data is and how to read it.
#[derive(Clone, Copy, Default, Debug)]
pub struct Array {
    pub enabled: bool,
    pub size: u32,
    pub kind: u32,
    pub stride: u32,
    /// An address in the program's memory, or an offset into `buffer`.
    pub pointer: u32,
    /// The buffer object bound to ARRAY_BUFFER when it was set (0: none).
    pub buffer: u32,
}

/// A vertex after the vertex stage: clip coordinates, colour, texture coordinates.
#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub clip: [f32; 4],
    pub color: [f32; 4],
    pub tex: [f32; 4],
    pub point_size: f32,
}

/// The fragment-stage state the rasterizer reads.
#[derive(Clone)]
pub struct Raster {
    pub viewport: [i32; 4],
    pub depth_range: [f32; 2],
    pub scissor: Option<[i32; 4]>,
    pub blend: Option<(u32, u32)>,
    pub alpha_test: Option<(u32, f32)>,
    pub depth_test: Option<u32>,
    pub depth_mask: bool,
    pub color_mask: [bool; 4],
    pub cull: Option<(u32, u32)>,
    pub flat: bool,
    pub tex_env: u32,
    pub tex_env_color: [f32; 4],
    pub polygon_offset: Option<(f32, f32)>,
}

pub struct Gl {
    pub surface: Surface,
    error: u32,
    matrix_mode: u32,
    stacks: [Vec<Mat>; 3],
    color: [f32; 4],
    normal: [f32; 3],
    tex_coord: [f32; 4],
    point_size: f32,
    line_width: f32,
    clear_color: [f32; 4],
    clear_depth: f32,
    caps: std::collections::HashSet<u32>,
    blend_func: (u32, u32),
    alpha_func: (u32, f32),
    depth_func: u32,
    cull_face: u32,
    front_face: u32,
    shade_model: u32,
    scissor_box: [i32; 4],
    pub raster: Raster,
    textures: std::collections::HashMap<u32, Texture>,
    next_name: u32,
    bound_texture: u32,
    buffers: std::collections::HashMap<u32, Vec<u8>>,
    array_buffer: u32,
    element_buffer: u32,
    pub vertex: Array,
    pub color_array: Array,
    pub normal_array: Array,
    pub tex_array: Array,
    pub point_size_array: Array,
    unpack_alignment: u32,
    pack_alignment: u32,
    active_texture: u32,
    client_active_texture: u32,
    /// Calls not done (or done only in part), named once each, for logs.
    pub unsupported: std::collections::BTreeSet<String>,
    /// Draw calls since the last `take_draws` (diagnostics).
    pub draws: u64,
}

impl Gl {
    /// A context drawing into a `width` x `height` RGBA surface with a depth buffer.
    pub fn new(width: u32, height: u32) -> Self {
        let mut caps = std::collections::HashSet::new();
        caps.insert(gl::DITHER);
        Self {
            surface: Surface::new(width, height),
            error: 0,
            matrix_mode: gl::MODELVIEW,
            stacks: [vec![IDENTITY], vec![IDENTITY], vec![IDENTITY]],
            color: [1.; 4],
            normal: [0., 0., 1.],
            tex_coord: [0., 0., 0., 1.],
            point_size: 1.,
            line_width: 1.,
            clear_color: [0.; 4],
            clear_depth: 1.,
            caps,
            blend_func: (gl::ONE, gl::ZERO),
            alpha_func: (gl::ALWAYS, 0.),
            depth_func: gl::LESS,
            cull_face: gl::BACK,
            front_face: gl::CCW,
            shade_model: gl::SMOOTH,
            scissor_box: [0, 0, width as i32, height as i32],
            raster: Raster {
                viewport: [0, 0, width as i32, height as i32],
                depth_range: [0., 1.],
                scissor: None,
                blend: None,
                alpha_test: None,
                depth_test: None,
                depth_mask: true,
                color_mask: [true; 4],
                cull: None,
                flat: false,
                tex_env: gl::MODULATE,
                tex_env_color: [0.; 4],
                polygon_offset: None,
            },
            textures: std::collections::HashMap::new(),
            next_name: 1,
            bound_texture: 0,
            buffers: std::collections::HashMap::new(),
            array_buffer: 0,
            element_buffer: 0,
            vertex: Array { size: 4, kind: gl::FLOAT, ..Default::default() },
            color_array: Array { size: 4, kind: gl::FLOAT, ..Default::default() },
            normal_array: Array { size: 3, kind: gl::FLOAT, ..Default::default() },
            tex_array: Array { size: 4, kind: gl::FLOAT, ..Default::default() },
            point_size_array: Array { size: 1, kind: gl::FLOAT, ..Default::default() },
            unpack_alignment: 4,
            pack_alignment: 4,
            active_texture: gl::TEXTURE0,
            client_active_texture: gl::TEXTURE0,
            unsupported: Default::default(),
            draws: 0,
        }
    }

    /// Gives the surface a new size (EGL's window changed); the contents go.
    pub fn resize(&mut self, width: u32, height: u32) {
        if (self.surface.width, self.surface.height) != (width, height) {
            self.surface = Surface::new(width, height);
        }
    }

    fn set_error(&mut self, e: u32) {
        if self.error == 0 {
            self.error = e;
        }
    }

    fn note(&mut self, what: &str) {
        if !self.unsupported.contains(what) {
            self.unsupported.insert(what.to_string());
        }
    }

    pub fn get_error(&mut self) -> u32 {
        std::mem::take(&mut self.error)
    }

    // ---- state ---------------------------------------------------------

    pub fn set_cap(&mut self, cap: u32, on: bool) {
        match cap {
            gl::TEXTURE_2D | gl::BLEND | gl::ALPHA_TEST | gl::DEPTH_TEST | gl::SCISSOR_TEST | gl::CULL_FACE | gl::LIGHTING | gl::FOG
            | gl::DITHER | gl::STENCIL_TEST | gl::NORMALIZE | gl::RESCALE_NORMAL | gl::COLOR_MATERIAL | gl::COLOR_LOGIC_OP
            | gl::POLYGON_OFFSET_FILL | 0x0B10 /* POINT_SMOOTH */ | 0x0B20 /* LINE_SMOOTH */ | 0x809D /* MULTISAMPLE */
            | 0x809E | 0x809F | 0x80A0 | 0x8861 /* POINT_SPRITE_OES */ | 0x4000..=0x4007 /* LIGHTi */ | 0x3000..=0x3005 /* CLIP_PLANEi */ => {
                if on {
                    self.caps.insert(cap);
                } else {
                    self.caps.remove(&cap);
                }
                if on && matches!(cap, gl::LIGHTING | gl::FOG | gl::STENCIL_TEST | gl::COLOR_LOGIC_OP) {
                    self.note(&format!("glEnable({cap:#x})"));
                }
            }
            _ => self.set_error(gl::INVALID_ENUM),
        }
    }

    pub fn is_enabled(&mut self, cap: u32) -> bool {
        match cap {
            gl::VERTEX_ARRAY => self.vertex.enabled,
            gl::COLOR_ARRAY => self.color_array.enabled,
            gl::NORMAL_ARRAY => self.normal_array.enabled,
            gl::TEXTURE_COORD_ARRAY => self.tex_array.enabled,
            _ => self.caps.contains(&cap),
        }
    }

    pub fn client_state(&mut self, array: u32, on: bool) {
        match array {
            gl::VERTEX_ARRAY => self.vertex.enabled = on,
            gl::COLOR_ARRAY => self.color_array.enabled = on,
            gl::NORMAL_ARRAY => self.normal_array.enabled = on,
            gl::TEXTURE_COORD_ARRAY => self.tex_array.enabled = on,
            gl::POINT_SIZE_ARRAY_OES => self.point_size_array.enabled = on,
            _ => self.set_error(gl::INVALID_ENUM),
        }
    }

    pub fn active_texture(&mut self, unit: u32) {
        self.active_texture = unit;
        if unit != gl::TEXTURE0 {
            self.note("texture units above 0");
        }
    }

    pub fn client_active_texture(&mut self, unit: u32) {
        self.client_active_texture = unit;
    }

    pub fn blend_func(&mut self, src: u32, dst: u32) {
        self.blend_func = (src, dst);
    }

    pub fn alpha_func(&mut self, func: u32, reference: f32) {
        self.alpha_func = (func, reference.clamp(0., 1.));
    }

    pub fn depth_func(&mut self, func: u32) {
        self.depth_func = func;
    }

    pub fn depth_mask(&mut self, on: bool) {
        self.raster.depth_mask = on;
    }

    pub fn color_mask(&mut self, m: [bool; 4]) {
        self.raster.color_mask = m;
    }

    pub fn depth_range(&mut self, near: f32, far: f32) {
        self.raster.depth_range = [near.clamp(0., 1.), far.clamp(0., 1.)];
    }

    pub fn cull_face(&mut self, mode: u32) {
        self.cull_face = mode;
    }

    pub fn front_face(&mut self, mode: u32) {
        self.front_face = mode;
    }

    pub fn shade_model(&mut self, mode: u32) {
        self.shade_model = mode;
    }

    pub fn polygon_offset(&mut self, factor: f32, units: f32) {
        self.raster.polygon_offset = Some((factor, units));
    }

    pub fn viewport(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w < 0 || h < 0 {
            return self.set_error(gl::INVALID_VALUE);
        }
        self.raster.viewport = [x, y, w, h];
    }

    pub fn scissor(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w < 0 || h < 0 {
            return self.set_error(gl::INVALID_VALUE);
        }
        self.scissor_box = [x, y, w, h];
    }

    pub fn clear_color(&mut self, c: [f32; 4]) {
        self.clear_color = c.map(|v| v.clamp(0., 1.));
    }

    pub fn clear_depth(&mut self, d: f32) {
        self.clear_depth = d.clamp(0., 1.);
    }

    pub fn color(&mut self, c: [f32; 4]) {
        self.color = c;
    }

    pub fn normal(&mut self, n: [f32; 3]) {
        self.normal = n;
    }

    pub fn multi_tex_coord(&mut self, t: [f32; 4]) {
        self.tex_coord = t;
    }

    pub fn point_size(&mut self, s: f32) {
        self.point_size = s.max(0.);
    }

    pub fn line_width(&mut self, w: f32) {
        self.line_width = w.max(0.);
    }

    pub fn pixel_store(&mut self, pname: u32, value: i32) {
        match (pname, value) {
            (gl::UNPACK_ALIGNMENT, 1 | 2 | 4 | 8) => self.unpack_alignment = value as u32,
            (gl::PACK_ALIGNMENT, 1 | 2 | 4 | 8) => self.pack_alignment = value as u32,
            _ => self.set_error(gl::INVALID_VALUE),
        }
    }

    pub fn tex_env(&mut self, target: u32, pname: u32, params: &[f32]) {
        if target != gl::TEXTURE_ENV {
            if target != 0x8862 {
                self.set_error(gl::INVALID_ENUM);
            }
            return;
        }
        match pname {
            gl::TEXTURE_ENV_MODE => {
                let m = params.first().copied().unwrap_or(0.) as u32;
                if m == gl::COMBINE {
                    self.note("GL_COMBINE texture environment (drawn as MODULATE)");
                }
                self.raster.tex_env = m;
            }
            gl::TEXTURE_ENV_COLOR if params.len() >= 4 => self.raster.tex_env_color = [params[0], params[1], params[2], params[3]],
            _ => self.note(&format!("glTexEnv({pname:#x})")),
        }
    }

    // ---- matrices ------------------------------------------------------

    fn stack_index(&self) -> usize {
        match self.matrix_mode {
            gl::PROJECTION => 1,
            gl::TEXTURE => 2,
            _ => 0,
        }
    }

    fn top(&mut self) -> &mut Mat {
        let i = self.stack_index();
        self.stacks[i].last_mut().unwrap()
    }

    pub fn matrix_mode(&mut self, mode: u32) {
        match mode {
            gl::MODELVIEW | gl::PROJECTION | gl::TEXTURE => self.matrix_mode = mode,
            _ => self.set_error(gl::INVALID_ENUM),
        }
    }

    pub fn load_identity(&mut self) {
        *self.top() = IDENTITY;
    }

    pub fn load_matrix(&mut self, m: &Mat) {
        *self.top() = *m;
    }

    pub fn mult_matrix(&mut self, m: &Mat) {
        let t = mat_mul(self.top(), m);
        *self.top() = t;
    }

    pub fn push_matrix(&mut self) {
        let i = self.stack_index();
        let max = if i == 0 { 32 } else { 8 };
        if self.stacks[i].len() >= max {
            return self.set_error(gl::STACK_OVERFLOW);
        }
        let t = *self.stacks[i].last().unwrap();
        self.stacks[i].push(t);
    }

    pub fn pop_matrix(&mut self) {
        let i = self.stack_index();
        if self.stacks[i].len() <= 1 {
            return self.set_error(gl::STACK_UNDERFLOW);
        }
        self.stacks[i].pop();
    }

    pub fn translate(&mut self, x: f32, y: f32, z: f32) {
        let mut m = IDENTITY;
        m[12] = x;
        m[13] = y;
        m[14] = z;
        self.mult_matrix(&m);
    }

    pub fn scale(&mut self, x: f32, y: f32, z: f32) {
        let mut m = IDENTITY;
        m[0] = x;
        m[5] = y;
        m[10] = z;
        self.mult_matrix(&m);
    }

    pub fn rotate(&mut self, angle_deg: f32, x: f32, y: f32, z: f32) {
        let len = (x * x + y * y + z * z).sqrt();
        if len == 0. {
            return;
        }
        let (x, y, z) = (x / len, y / len, z / len);
        let (s, c) = angle_deg.to_radians().sin_cos();
        let t = 1. - c;
        let m = [
            x * x * t + c,
            y * x * t + z * s,
            x * z * t - y * s,
            0.,
            x * y * t - z * s,
            y * y * t + c,
            y * z * t + x * s,
            0.,
            x * z * t + y * s,
            y * z * t - x * s,
            z * z * t + c,
            0.,
            0.,
            0.,
            0.,
            1.,
        ];
        self.mult_matrix(&m);
    }

    pub fn ortho(&mut self, l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) {
        if l == r || b == t || n == f {
            return self.set_error(gl::INVALID_VALUE);
        }
        let mut m = IDENTITY;
        m[0] = 2. / (r - l);
        m[5] = 2. / (t - b);
        m[10] = -2. / (f - n);
        m[12] = -(r + l) / (r - l);
        m[13] = -(t + b) / (t - b);
        m[14] = -(f + n) / (f - n);
        self.mult_matrix(&m);
    }

    pub fn frustum(&mut self, l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) {
        if n <= 0. || f <= 0. || l == r || b == t || n == f {
            return self.set_error(gl::INVALID_VALUE);
        }
        let mut m = [0f32; 16];
        m[0] = 2. * n / (r - l);
        m[5] = 2. * n / (t - b);
        m[8] = (r + l) / (r - l);
        m[9] = (t + b) / (t - b);
        m[10] = -(f + n) / (f - n);
        m[11] = -1.;
        m[14] = -2. * f * n / (f - n);
        self.mult_matrix(&m);
    }

    pub fn matrix(&self, which: u32) -> Mat {
        let i = match which {
            gl::PROJECTION | gl::PROJECTION_MATRIX => 1,
            gl::TEXTURE | gl::TEXTURE_MATRIX => 2,
            _ => 0,
        };
        *self.stacks[i].last().unwrap()
    }

    // ---- textures ------------------------------------------------------

    pub fn gen_names(&mut self, n: usize) -> Vec<u32> {
        (0..n)
            .map(|_| {
                let name = self.next_name;
                self.next_name += 1;
                name
            })
            .collect()
    }

    pub fn bind_texture(&mut self, target: u32, name: u32) {
        if target != gl::TEXTURE_2D {
            return self.set_error(gl::INVALID_ENUM);
        }
        self.bound_texture = name;
        if name != 0 {
            self.textures.entry(name).or_insert_with(Texture::new);
        }
    }

    pub fn delete_textures(&mut self, names: &[u32]) {
        for n in names {
            self.textures.remove(n);
            if self.bound_texture == *n {
                self.bound_texture = 0;
            }
        }
    }

    pub fn is_texture(&self, name: u32) -> bool {
        self.textures.contains_key(&name)
    }

    fn bound(&mut self) -> &mut Texture {
        let name = self.bound_texture;
        self.textures.entry(name).or_insert_with(Texture::new)
    }

    pub fn tex_parameter(&mut self, target: u32, pname: u32, value: u32) {
        if target != gl::TEXTURE_2D {
            return self.set_error(gl::INVALID_ENUM);
        }
        let t = self.bound();
        match pname {
            gl::TEXTURE_MIN_FILTER => t.min_filter = value,
            gl::TEXTURE_MAG_FILTER => t.mag_filter = value,
            gl::TEXTURE_WRAP_S => t.wrap_s = value,
            gl::TEXTURE_WRAP_T => t.wrap_t = value,
            gl::GENERATE_MIPMAP => {}
            _ => self.set_error(gl::INVALID_ENUM),
        }
    }

    pub fn tex_parameter_get(&mut self, pname: u32) -> u32 {
        let t = self.bound();
        match pname {
            gl::TEXTURE_MIN_FILTER => t.min_filter,
            gl::TEXTURE_MAG_FILTER => t.mag_filter,
            gl::TEXTURE_WRAP_S => t.wrap_s,
            gl::TEXTURE_WRAP_T => t.wrap_t,
            _ => 0,
        }
    }

    /// Bytes a row of `width` pixels takes in the program's memory, padded to UNPACK_ALIGNMENT.
    fn row_bytes(&self, width: u32, format: u32, kind: u32) -> Option<usize> {
        let bpp = pixel_bytes(format, kind)?;
        let a = self.unpack_alignment as usize;
        Some((width as usize * bpp).div_ceil(a) * a)
    }

    /// The bytes glTexImage2D/glTexSubImage2D read for a `w` x `h` image.
    pub fn image_bytes(&self, w: u32, h: u32, format: u32, kind: u32) -> Option<usize> {
        if w == 0 || h == 0 {
            return Some(0);
        }
        let bpp = pixel_bytes(format, kind)?;
        Some(self.row_bytes(w, format, kind)? * (h as usize - 1) + w as usize * bpp)
    }

    /// glTexImage2D, level 0 (other levels are ignored); `data` None
    /// leaves the texels undefined (zero).
    pub fn tex_image(&mut self, level: i32, internal: u32, w: u32, h: u32, format: u32, kind: u32, data: Option<&[u8]>) {
        if level != 0 {
            return;
        }
        if w > 4096 || h > 4096 {
            return self.set_error(gl::INVALID_VALUE);
        }
        if pixel_bytes(format, kind).is_none() {
            return self.set_error(gl::INVALID_ENUM);
        }
        let _ = internal;
        let pixels = match data {
            Some(d) => self.decode_rect(w, h, format, kind, d),
            None => vec![0; (w * h) as usize],
        };
        let t = self.bound();
        t.width = w;
        t.height = h;
        t.pixels = pixels;
        t.format = format;
    }

    pub fn tex_sub_image(&mut self, level: i32, x: i32, y: i32, w: u32, h: u32, format: u32, kind: u32, data: &[u8]) {
        if level != 0 {
            return;
        }
        let px = self.decode_rect(w, h, format, kind, data);
        let t = self.bound();
        if x < 0 || y < 0 || x as u32 + w > t.width || y as u32 + h > t.height {
            return self.set_error(gl::INVALID_VALUE);
        }
        for row in 0..h {
            let dst = ((y as u32 + row) * t.width + x as u32) as usize;
            t.pixels[dst..dst + w as usize].copy_from_slice(&px[(row * w) as usize..((row + 1) * w) as usize]);
        }
    }

    /// glCompressedTexImage2D: the paletted formats and ETC1.
    pub fn compressed_tex_image(&mut self, level: i32, format: u32, w: u32, h: u32, data: &[u8]) {
        let pixels = match format {
            0x8B90..=0x8B99 => decode_paletted(format, w, h, level, data),
            gl::ETC1_RGB8_OES if level == 0 => decode_etc1(w, h, data),
            gl::ETC1_RGB8_OES => return,
            _ => {
                self.note(&format!("compressed texture format {format:#x}"));
                return self.set_error(gl::INVALID_ENUM);
            }
        };
        let Some(pixels) = pixels else { return };
        let t = self.bound();
        t.width = w;
        t.height = h;
        t.pixels = pixels;
        t.format = gl::RGBA;
    }

    fn decode_rect(&self, w: u32, h: u32, format: u32, kind: u32, data: &[u8]) -> Vec<u32> {
        let (bpp, row) = (pixel_bytes(format, kind).unwrap_or(4), self.row_bytes(w, format, kind).unwrap_or(0));
        let mut out = Vec::with_capacity((w * h) as usize);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let at = y * row + x * bpp;
                let p = data.get(at..at + bpp).unwrap_or(&[]);
                out.push(decode_pixel(format, kind, p));
            }
        }
        out
    }

    /// The texture `name` (for tests and tools).
    pub fn texture(&self, name: u32) -> Option<&Texture> {
        self.textures.get(&name)
    }

    // ---- buffer objects ------------------------------------------------

    pub fn bind_buffer(&mut self, target: u32, name: u32) {
        match target {
            gl::ARRAY_BUFFER => self.array_buffer = name,
            gl::ELEMENT_ARRAY_BUFFER => self.element_buffer = name,
            _ => return self.set_error(gl::INVALID_ENUM),
        }
        if name != 0 {
            self.buffers.entry(name).or_default();
        }
    }

    fn target_buffer(&self, target: u32) -> Option<u32> {
        match target {
            gl::ARRAY_BUFFER => Some(self.array_buffer),
            gl::ELEMENT_ARRAY_BUFFER => Some(self.element_buffer),
            _ => None,
        }
    }

    pub fn buffer_data(&mut self, target: u32, data: Vec<u8>) {
        match self.target_buffer(target) {
            Some(0) => self.set_error(gl::INVALID_OPERATION),
            Some(b) => {
                self.buffers.insert(b, data);
            }
            None => self.set_error(gl::INVALID_ENUM),
        }
    }

    pub fn buffer_sub_data(&mut self, target: u32, offset: usize, data: &[u8]) {
        match self.target_buffer(target) {
            Some(b) if b != 0 => {
                let buf = self.buffers.entry(b).or_default();
                if offset + data.len() > buf.len() {
                    return self.set_error(gl::INVALID_VALUE);
                }
                buf[offset..offset + data.len()].copy_from_slice(data);
            }
            Some(_) => self.set_error(gl::INVALID_OPERATION),
            None => self.set_error(gl::INVALID_ENUM),
        }
    }

    pub fn delete_buffers(&mut self, names: &[u32]) {
        for n in names {
            self.buffers.remove(n);
            if self.array_buffer == *n {
                self.array_buffer = 0;
            }
            if self.element_buffer == *n {
                self.element_buffer = 0;
            }
        }
    }

    pub fn is_buffer(&self, name: u32) -> bool {
        self.buffers.contains_key(&name)
    }

    pub fn set_array(&mut self, which: u32, size: u32, kind: u32, stride: u32, pointer: u32) {
        let buffer = self.array_buffer;
        let a = match which {
            gl::VERTEX_ARRAY => &mut self.vertex,
            gl::COLOR_ARRAY => &mut self.color_array,
            gl::NORMAL_ARRAY => &mut self.normal_array,
            gl::TEXTURE_COORD_ARRAY => &mut self.tex_array,
            gl::POINT_SIZE_ARRAY_OES => &mut self.point_size_array,
            _ => return self.set_error(gl::INVALID_ENUM),
        };
        a.size = size;
        a.kind = kind;
        a.stride = stride;
        a.pointer = pointer;
        a.buffer = buffer;
    }

    // ---- queries ---------------------------------------------------------

    /// glGetIntegerv's values (None: unknown name).
    pub fn get_integers(&mut self, pname: u32) -> Option<Vec<i32>> {
        let b = |v: bool| v as i32;
        Some(match pname {
            gl::MAX_TEXTURE_SIZE => vec![2048],
            gl::MAX_MODELVIEW_STACK_DEPTH => vec![32],
            gl::MAX_PROJECTION_STACK_DEPTH | gl::MAX_TEXTURE_STACK_DEPTH => vec![8],
            gl::MAX_VIEWPORT_DIMS => vec![4096, 4096],
            gl::MAX_TEXTURE_UNITS => vec![1],
            gl::MAX_LIGHTS => vec![8],
            gl::MAX_CLIP_PLANES => vec![6],
            gl::VIEWPORT => self.raster.viewport.to_vec(),
            0x0C10 /* SCISSOR_BOX */ => self.scissor_box.to_vec(),
            gl::MATRIX_MODE => vec![self.matrix_mode as i32],
            gl::TEXTURE_BINDING_2D => vec![self.bound_texture as i32],
            gl::ARRAY_BUFFER_BINDING => vec![self.array_buffer as i32],
            gl::ELEMENT_ARRAY_BUFFER_BINDING => vec![self.element_buffer as i32],
            gl::RED_BITS | gl::GREEN_BITS | gl::BLUE_BITS | gl::ALPHA_BITS => vec![8],
            gl::DEPTH_BITS => vec![16],
            gl::STENCIL_BITS => vec![0],
            gl::NUM_COMPRESSED_TEXTURE_FORMATS => vec![11],
            gl::COMPRESSED_TEXTURE_FORMATS => (0x8B90..=0x8B99).chain([gl::ETC1_RGB8_OES]).map(|v| v as i32).collect(),
            gl::UNPACK_ALIGNMENT => vec![self.unpack_alignment as i32],
            gl::PACK_ALIGNMENT => vec![self.pack_alignment as i32],
            gl::BLEND_SRC => vec![self.blend_func.0 as i32],
            gl::BLEND_DST => vec![self.blend_func.1 as i32],
            0x0B74 /* DEPTH_FUNC */ => vec![self.depth_func as i32],
            0x0B72 /* DEPTH_WRITEMASK */ => vec![b(self.raster.depth_mask)],
            0x0BC1 /* ALPHA_TEST_FUNC */ => vec![self.alpha_func.0 as i32],
            0x0B54 /* SHADE_MODEL */ => vec![self.shade_model as i32],
            0x0B45 /* CULL_FACE_MODE */ => vec![self.cull_face as i32],
            0x0B46 /* FRONT_FACE */ => vec![self.front_face as i32],
            0x84E0 /* ACTIVE_TEXTURE */ => vec![self.active_texture as i32],
            0x84E1 /* CLIENT_ACTIVE_TEXTURE */ => vec![self.client_active_texture as i32],
            0x0BA3 /* MODELVIEW_STACK_DEPTH */ => vec![self.stacks[0].len() as i32],
            0x0BA4 /* PROJECTION_STACK_DEPTH */ => vec![self.stacks[1].len() as i32],
            0x0BA5 /* TEXTURE_STACK_DEPTH */ => vec![self.stacks[2].len() as i32],
            0x0D50 /* SUBPIXEL_BITS */ => vec![4],
            0x8B9A /* IMPLEMENTATION_COLOR_READ_TYPE_OES */ => vec![gl::UNSIGNED_BYTE as i32],
            0x8B9B /* IMPLEMENTATION_COLOR_READ_FORMAT_OES */ => vec![gl::RGBA as i32],
            _ if self.caps_query(pname).is_some() => vec![b(self.caps_query(pname).unwrap())],
            _ => {
                self.note(&format!("glGetIntegerv({pname:#x})"));
                return None;
            }
        })
    }

    fn caps_query(&self, pname: u32) -> Option<bool> {
        match pname {
            gl::TEXTURE_2D | gl::BLEND | gl::ALPHA_TEST | gl::DEPTH_TEST | gl::SCISSOR_TEST | gl::CULL_FACE | gl::LIGHTING | gl::FOG | gl::DITHER => {
                Some(self.caps.contains(&pname))
            }
            gl::VERTEX_ARRAY => Some(self.vertex.enabled),
            gl::COLOR_ARRAY => Some(self.color_array.enabled),
            gl::NORMAL_ARRAY => Some(self.normal_array.enabled),
            gl::TEXTURE_COORD_ARRAY => Some(self.tex_array.enabled),
            _ => None,
        }
    }

    /// glGetFloatv's values: the matrices, the colours, else the integers.
    pub fn get_floats(&mut self, pname: u32) -> Option<Vec<f32>> {
        Some(match pname {
            gl::MODELVIEW_MATRIX | gl::PROJECTION_MATRIX | gl::TEXTURE_MATRIX => self.matrix(pname).to_vec(),
            0x0C22 /* COLOR_CLEAR_VALUE */ => self.clear_color.to_vec(),
            0x0B00 /* CURRENT_COLOR */ => self.color.to_vec(),
            0x0B21 /* LINE_WIDTH */ => vec![self.line_width],
            0x0B11 /* POINT_SIZE */ => vec![self.point_size],
            0x0B70 /* DEPTH_RANGE */ => self.raster.depth_range.to_vec(),
            0x0BC2 /* ALPHA_TEST_REF */ => vec![self.alpha_func.1],
            0x846E /* ALIASED_LINE_WIDTH_RANGE */ | 0x846D /* ALIASED_POINT_SIZE_RANGE */ | 0x0B12 /* SMOOTH_POINT_SIZE_RANGE */ | 0x0B22 => vec![1., 64.],
            _ => self.get_integers(pname)?.into_iter().map(|v| v as f32).collect(),
        })
    }

    pub fn get_string(&self, name: u32) -> Option<&'static str> {
        Some(match name {
            gl::VENDOR => "Ribbit",
            gl::RENDERER => "Ribbit software GLES 1.1",
            gl::VERSION => "OpenGL ES-CM 1.1",
            gl::EXTENSIONS => "GL_OES_compressed_paletted_texture GL_OES_compressed_ETC1_RGB8_texture GL_OES_read_format GL_OES_byte_coordinates GL_OES_fixed_point GL_OES_single_precision GL_OES_matrix_get GL_EXT_texture_format_BGRA8888",
            _ => return None,
        })
    }

    // ---- drawing -------------------------------------------------------

    pub fn clear(&mut self, mask: u32) {
        let scissor = self.caps.contains(&gl::SCISSOR_TEST).then_some(self.scissor_box);
        if mask & gl::COLOR_BUFFER_BIT != 0 {
            let c = self.clear_color;
            self.surface.clear_color(rgba8(c), self.raster.color_mask, scissor);
        }
        if mask & gl::DEPTH_BUFFER_BIT != 0 && self.raster.depth_mask {
            self.surface.clear_depth(self.clear_depth, scissor);
        }
    }

    /// The fragment state from the GL state, for the rasterizer.
    fn sync_raster(&mut self) {
        let on = |c: u32| self.caps.contains(&c);
        let r = &mut self.raster;
        r.blend = on(gl::BLEND).then_some(self.blend_func);
        r.alpha_test = on(gl::ALPHA_TEST).then_some(self.alpha_func);
        r.depth_test = on(gl::DEPTH_TEST).then_some(self.depth_func);
        r.scissor = on(gl::SCISSOR_TEST).then_some(self.scissor_box);
        r.cull = on(gl::CULL_FACE).then_some((self.cull_face, self.front_face));
        r.flat = self.shade_model == gl::FLAT;
        if !on(gl::POLYGON_OFFSET_FILL) {
            r.polygon_offset = None;
        }
    }

    /// Reads array `a`'s element `i` as up to 4 floats (`norm`: normalise
    /// integer types to 0..1 / -1..1, as colours are).
    fn fetch(&mut self, mem: &mut dyn Memory, a: &Array, i: u32, norm: bool, cache: &mut Fetcher) -> Option<[f32; 4]> {
        let elem = match a.kind {
            gl::BYTE | gl::UNSIGNED_BYTE => 1,
            gl::SHORT | gl::UNSIGNED_SHORT => 2,
            gl::FIXED | gl::FLOAT | gl::INT | gl::UNSIGNED_INT => 4,
            _ => return None,
        };
        let stride = if a.stride == 0 { elem * a.size } else { a.stride };
        let off = a.pointer.wrapping_add(i.wrapping_mul(stride));
        let len = (elem * a.size) as usize;
        let bytes: Vec<u8> = if a.buffer != 0 {
            self.buffers.get(&a.buffer)?.get(off as usize..off as usize + len)?.to_vec()
        } else {
            cache.read(mem, off, len)?
        };
        let mut v = [0., 0., 0., 1.];
        for k in 0..a.size.min(4) as usize {
            let b = &bytes[k * elem as usize..];
            v[k] = match a.kind {
                gl::BYTE => {
                    let x = b[0] as i8 as f32;
                    if norm { (2. * x + 1.) / 255. } else { x }
                }
                gl::UNSIGNED_BYTE => {
                    let x = b[0] as f32;
                    if norm { x / 255. } else { x }
                }
                gl::SHORT => {
                    let x = i16::from_le_bytes([b[0], b[1]]) as f32;
                    if norm { (2. * x + 1.) / 65535. } else { x }
                }
                gl::UNSIGNED_SHORT => {
                    let x = u16::from_le_bytes([b[0], b[1]]) as f32;
                    if norm { x / 65535. } else { x }
                }
                gl::FIXED => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 65536.,
                gl::FLOAT => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                _ => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
            };
        }
        Some(v)
    }

    /// Vertex `i` through the vertex stage.
    fn vertex_at(&mut self, mem: &mut dyn Memory, i: u32, mvp: &Mat, texm: &Mat, cache: &mut Fetcher) -> Option<Vertex> {
        let va = self.vertex;
        let p = self.fetch(mem, &va, i, false, cache)?;
        let p = if va.size < 4 { [p[0], p[1], if va.size < 3 { 0. } else { p[2] }, 1.] } else { p };
        let color = if self.color_array.enabled {
            let ca = self.color_array;
            self.fetch(mem, &ca, i, true, cache)?
        } else {
            self.color
        };
        let tex = if self.tex_array.enabled {
            let ta = self.tex_array;
            let t = self.fetch(mem, &ta, i, false, cache)?;
            let t = [t[0], if ta.size < 2 { 0. } else { t[1] }, if ta.size < 3 { 0. } else { t[2] }, if ta.size < 4 { 1. } else { t[3] }];
            transform(texm, t)
        } else {
            transform(texm, self.tex_coord)
        };
        let point_size = if self.point_size_array.enabled {
            let pa = self.point_size_array;
            self.fetch(mem, &pa, i, false, cache)?[0]
        } else {
            self.point_size
        };
        Some(Vertex { clip: transform(mvp, p), color: color.map(|c| c.clamp(0., 1.)), tex, point_size })
    }

    fn texture_for_draw(&self) -> Option<&Texture> {
        if !self.caps.contains(&gl::TEXTURE_2D) {
            return None;
        }
        self.textures.get(&self.bound_texture).filter(|t| t.width > 0 && t.height > 0)
    }

    /// glDrawArrays.
    pub fn draw_arrays(&mut self, mem: &mut dyn Memory, mode: u32, first: i32, count: i32) {
        if first < 0 || count < 0 {
            return self.set_error(gl::INVALID_VALUE);
        }
        let idx: Vec<u32> = (first as u32..(first + count) as u32).collect();
        self.draw(mem, mode, &idx);
    }

    /// glDrawElements: `indices` an address in memory, or an offset into the bound element buffer.
    pub fn draw_elements(&mut self, mem: &mut dyn Memory, mode: u32, count: i32, kind: u32, indices: u32) {
        if count < 0 {
            return self.set_error(gl::INVALID_VALUE);
        }
        let size = match kind {
            gl::UNSIGNED_BYTE => 1,
            gl::UNSIGNED_SHORT => 2,
            gl::UNSIGNED_INT => 4,
            _ => return self.set_error(gl::INVALID_ENUM),
        };
        let len = count as usize * size;
        let bytes = if self.element_buffer != 0 {
            self.buffers.get(&self.element_buffer).and_then(|b| b.get(indices as usize..indices as usize + len)).map(<[u8]>::to_vec)
        } else {
            mem.read(indices, len)
        };
        let Some(bytes) = bytes else { return self.set_error(gl::INVALID_OPERATION) };
        let idx: Vec<u32> = bytes
            .chunks(size)
            .map(|c| match size {
                1 => c[0] as u32,
                2 => u16::from_le_bytes([c[0], c[1]]) as u32,
                _ => u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            })
            .collect();
        self.draw(mem, mode, &idx);
    }

    fn draw(&mut self, mem: &mut dyn Memory, mode: u32, idx: &[u32]) {
        if !self.vertex.enabled || idx.is_empty() {
            return;
        }
        self.draws += 1;
        self.sync_raster();
        let mvp = mat_mul(&self.matrix(gl::PROJECTION), &self.matrix(gl::MODELVIEW));
        let texm = self.matrix(gl::TEXTURE);
        let mut cache = Fetcher::default();
        let mut verts = Vec::with_capacity(idx.len());
        for &i in idx {
            match self.vertex_at(mem, i, &mvp, &texm, &mut cache) {
                Some(v) => verts.push(v),
                None => return self.set_error(gl::INVALID_OPERATION),
            }
        }
        let tex = self.texture_for_draw().cloned();
        let raster = self.raster.clone();
        let surface = &mut self.surface;
        let tri = |s: &mut Surface, a: &Vertex, b: &Vertex, c: &Vertex| raster::triangle(s, &raster, tex.as_ref(), [a, b, c]);
        match mode {
            gl::TRIANGLES => {
                for t in verts.chunks_exact(3) {
                    tri(surface, &t[0], &t[1], &t[2]);
                }
            }
            gl::TRIANGLE_STRIP => {
                for i in 2..verts.len() {
                    if i % 2 == 0 {
                        tri(surface, &verts[i - 2], &verts[i - 1], &verts[i]);
                    } else {
                        tri(surface, &verts[i - 1], &verts[i - 2], &verts[i]);
                    }
                }
            }
            gl::TRIANGLE_FAN => {
                for i in 2..verts.len() {
                    tri(surface, &verts[0], &verts[i - 1], &verts[i]);
                }
            }
            gl::POINTS => {
                for v in &verts {
                    raster::point(surface, &raster, tex.as_ref(), v);
                }
            }
            gl::LINES | gl::LINE_STRIP | gl::LINE_LOOP => {
                let mut pairs = Vec::new();
                match mode {
                    gl::LINES => pairs.extend((0..verts.len() / 2).map(|i| (2 * i, 2 * i + 1))),
                    _ => {
                        pairs.extend((1..verts.len()).map(|i| (i - 1, i)));
                        if mode == gl::LINE_LOOP && verts.len() > 2 {
                            pairs.push((verts.len() - 1, 0));
                        }
                    }
                }
                for (a, b) in pairs {
                    raster::line(surface, &raster, tex.as_ref(), &verts[a], &verts[b], self.line_width);
                }
            }
            _ => self.set_error(gl::INVALID_ENUM),
        }
    }

    /// glReadPixels in RGBA/UNSIGNED_BYTE, rows bottom-up as GL gives them.
    pub fn read_pixels(&self, x: i32, y: i32, w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                out.extend_from_slice(&self.surface.pixel(x + col, y + row).to_le_bytes());
            }
        }
        out
    }
}

/// Reads of the program's memory for one draw, a page at a time.
#[derive(Default)]
struct Fetcher {
    page: Option<(u32, Vec<u8>)>,
}

impl Fetcher {
    const PAGE: u32 = 4096;

    fn read(&mut self, mem: &mut dyn Memory, addr: u32, len: usize) -> Option<Vec<u8>> {
        let base = addr & !(Self::PAGE - 1);
        if (addr - base) as usize + len > Self::PAGE as usize {
            return mem.read(addr, len);
        }
        if self.page.as_ref().is_none_or(|(b, _)| *b != base) {
            match mem.read(base, Self::PAGE as usize) {
                Some(p) => self.page = Some((base, p)),
                // Part of the page isn't readable: read just this.
                None => return mem.read(addr, len),
            }
        }
        let (_, d) = self.page.as_ref()?;
        d.get((addr - base) as usize..(addr - base) as usize + len).map(<[u8]>::to_vec)
    }
}

/// Bytes a pixel takes in format/type (None: not a valid pair).
pub fn pixel_bytes(format: u32, kind: u32) -> Option<usize> {
    Some(match (format, kind) {
        (gl::RGBA | gl::BGRA_EXT, gl::UNSIGNED_BYTE) => 4,
        (gl::RGB, gl::UNSIGNED_BYTE) => 3,
        (gl::LUMINANCE_ALPHA, gl::UNSIGNED_BYTE) => 2,
        (gl::LUMINANCE | gl::ALPHA, gl::UNSIGNED_BYTE) => 1,
        (gl::RGB, gl::UNSIGNED_SHORT_5_6_5) | (gl::RGBA, gl::UNSIGNED_SHORT_4_4_4_4 | gl::UNSIGNED_SHORT_5_5_5_1) => 2,
        _ => return None,
    })
}

/// A pixel as RGBA8 (R in the low byte).
pub fn decode_pixel(format: u32, kind: u32, p: &[u8]) -> u32 {
    let g = |i: usize| p.get(i).copied().unwrap_or(0) as u32;
    let pack = |r: u32, gg: u32, b: u32, a: u32| r | gg << 8 | b << 16 | a << 24;
    let x5 = |v: u32| (v << 3) | (v >> 2);
    let x6 = |v: u32| (v << 2) | (v >> 4);
    let x4 = |v: u32| v * 17;
    match (format, kind) {
        (gl::RGBA, gl::UNSIGNED_BYTE) => pack(g(0), g(1), g(2), g(3)),
        (gl::BGRA_EXT, _) => pack(g(2), g(1), g(0), g(3)),
        (gl::RGB, gl::UNSIGNED_BYTE) => pack(g(0), g(1), g(2), 255),
        (gl::LUMINANCE_ALPHA, _) => pack(g(0), g(0), g(0), g(1)),
        (gl::LUMINANCE, _) => pack(g(0), g(0), g(0), 255),
        (gl::ALPHA, _) => pack(0, 0, 0, g(0)),
        (_, gl::UNSIGNED_SHORT_5_6_5) => {
            let v = g(0) | g(1) << 8;
            pack(x5(v >> 11), x6((v >> 5) & 63), x5(v & 31), 255)
        }
        (_, gl::UNSIGNED_SHORT_4_4_4_4) => {
            let v = g(0) | g(1) << 8;
            pack(x4(v >> 12), x4((v >> 8) & 15), x4((v >> 4) & 15), x4(v & 15))
        }
        (_, gl::UNSIGNED_SHORT_5_5_5_1) => {
            let v = g(0) | g(1) << 8;
            pack(x5(v >> 11), x5((v >> 6) & 31), x5((v >> 1) & 31), if v & 1 != 0 { 255 } else { 0 })
        }
        _ => 0,
    }
}

pub fn rgba8(c: [f32; 4]) -> u32 {
    let q = |v: f32| (v.clamp(0., 1.) * 255. + 0.5) as u32;
    q(c[0]) | q(c[1]) << 8 | q(c[2]) << 16 | q(c[3]) << 24
}

/// OES_compressed_paletted_texture: the palette, then the indices (4 or 8
/// bits); only the level asked for is decoded (level 0 here).
fn decode_paletted(format: u32, w: u32, h: u32, level: i32, data: &[u8]) -> Option<Vec<u32>> {
    if level != 0 {
        return None;
    }
    let (bits, entry, fmt, kind) = match format {
        0x8B90 => (4, 3, gl::RGB, gl::UNSIGNED_BYTE),
        0x8B91 => (4, 4, gl::RGBA, gl::UNSIGNED_BYTE),
        0x8B92 => (4, 2, gl::RGB, gl::UNSIGNED_SHORT_5_6_5),
        0x8B93 => (4, 2, gl::RGBA, gl::UNSIGNED_SHORT_4_4_4_4),
        0x8B94 => (4, 2, gl::RGBA, gl::UNSIGNED_SHORT_5_5_5_1),
        0x8B95 => (8, 3, gl::RGB, gl::UNSIGNED_BYTE),
        0x8B96 => (8, 4, gl::RGBA, gl::UNSIGNED_BYTE),
        0x8B97 => (8, 2, gl::RGB, gl::UNSIGNED_SHORT_5_6_5),
        0x8B98 => (8, 2, gl::RGBA, gl::UNSIGNED_SHORT_4_4_4_4),
        _ => (8, 2, gl::RGBA, gl::UNSIGNED_SHORT_5_5_5_1),
    };
    let entries = 1usize << bits;
    let palette: Vec<u32> = (0..entries).map(|i| decode_pixel(fmt, kind, data.get(i * entry..(i + 1) * entry).unwrap_or(&[]))).collect();
    let idx = &data.get(entries * entry..)?;
    Some(
        (0..(w * h) as usize)
            .map(|i| {
                let v = if bits == 8 {
                    idx.get(i).copied().unwrap_or(0) as usize
                } else {
                    let b = idx.get(i / 2).copied().unwrap_or(0);
                    (if i % 2 == 0 { b >> 4 } else { b & 15 }) as usize
                };
                palette[v]
            })
            .collect(),
    )
}

/// ETC1 (OES_compressed_ETC1_RGB8_texture): 4x4 blocks of 64 bits, big-endian.
fn decode_etc1(w: u32, h: u32, data: &[u8]) -> Option<Vec<u32>> {
    const TABLE: [[i32; 4]; 8] =
        [[2, 8, -2, -8], [5, 17, -5, -17], [9, 29, -9, -29], [13, 42, -13, -42], [18, 60, -18, -60], [24, 80, -24, -80], [33, 106, -33, -106], [47, 183, -47, -183]];
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    let mut out = vec![0u32; (w * h) as usize];
    for by in 0..bh {
        for bx in 0..bw {
            let at = ((by * bw + bx) * 8) as usize;
            let b = data.get(at..at + 8)?;
            let hi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
            let lo = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
            let diff = hi & 2 != 0;
            let flip = hi & 1 != 0;
            let (c1, c2) = if diff {
                let base = |s: u32| (hi >> s) & 31;
                let delta = |s: u32| (((hi >> s) & 7) as i32) << 29 >> 29;
                let e5 = |v: i32| ((v << 3) | (v >> 2)) as u32;
                let r = base(27) as i32;
                let g = base(19) as i32;
                let bb = base(11) as i32;
                ([e5(r), e5(g), e5(bb)], [e5((r + delta(24)) & 31), e5((g + delta(16)) & 31), e5((bb + delta(8)) & 31)])
            } else {
                let n = |s: u32| ((hi >> s) & 15) * 17;
                ([n(28), n(20), n(12)], [n(24), n(16), n(8)])
            };
            let (t1, t2) = ((hi >> 5) & 7, (hi >> 2) & 7);
            for i in 0..16u32 {
                let (x, y) = (i / 4, i % 4);
                let (px, py) = (bx * 4 + x, by * 4 + y);
                if px >= w || py >= h {
                    continue;
                }
                let second = if flip { y >= 2 } else { x >= 2 };
                let (base, t) = if second { (c2, t2) } else { (c1, t1) };
                let m = ((lo >> i) & 1) | ((lo >> (i + 16)) & 1) << 1;
                let d = TABLE[t as usize][match m {
                    0 => 0,
                    1 => 1,
                    2 => 2,
                    _ => 3,
                }];
                let c = base.map(|v| (v as i32 + d).clamp(0, 255) as u32);
                out[(py * w + px) as usize] = c[0] | c[1] << 8 | c[2] << 16 | 0xFF00_0000;
            }
        }
    }
    Some(out)
}

/// EGL: one display, a few configs, window/pbuffer surfaces and contexts as
/// handles. The program's native windows aren't interpreted: each surface
/// is the size its owner gives.
#[derive(Default)]
pub struct Egl {
    pub error: u32,
    pub initialized: bool,
    next: u32,
    /// Surface handle -> (width, height).
    pub surfaces: std::collections::HashMap<u32, (u32, u32)>,
    pub contexts: std::collections::HashSet<u32>,
    pub current: Option<(u32, u32)>,
    pub swap_interval: i32,
}

pub mod egl {
    pub const SUCCESS: u32 = 0x3000;
    pub const NOT_INITIALIZED: u32 = 0x3001;
    pub const BAD_DISPLAY: u32 = 0x3008;
    pub const BAD_SURFACE: u32 = 0x300D;
    pub const BUFFER_SIZE: u32 = 0x3020;
    pub const ALPHA_SIZE: u32 = 0x3021;
    pub const BLUE_SIZE: u32 = 0x3022;
    pub const GREEN_SIZE: u32 = 0x3023;
    pub const RED_SIZE: u32 = 0x3024;
    pub const DEPTH_SIZE: u32 = 0x3025;
    pub const STENCIL_SIZE: u32 = 0x3026;
    pub const CONFIG_ID: u32 = 0x3028;
    pub const SURFACE_TYPE: u32 = 0x3033;
    pub const RENDERABLE_TYPE: u32 = 0x3040;
    pub const NONE: u32 = 0x3038;
    pub const HEIGHT: u32 = 0x3056;
    pub const WIDTH: u32 = 0x3057;
    pub const VENDOR: u32 = 0x3053;
    pub const VERSION: u32 = 0x3054;
    pub const EXTENSIONS: u32 = 0x3055;
    pub const CLIENT_APIS: u32 = 0x308D;
    /// The one display handle.
    pub const DISPLAY: u32 = 1;
    /// The one config handle.
    pub const CONFIG: u32 = 1;
}

impl Egl {
    pub fn handle(&mut self) -> u32 {
        self.next += 1;
        0x100 + self.next
    }

    /// A config attribute (RGBA8888, 16-bit depth, window and pbuffer, ES 1).
    pub fn config_attrib(attr: u32) -> Option<i32> {
        Some(match attr {
            egl::BUFFER_SIZE => 32,
            egl::RED_SIZE | egl::GREEN_SIZE | egl::BLUE_SIZE | egl::ALPHA_SIZE => 8,
            egl::DEPTH_SIZE => 16,
            egl::STENCIL_SIZE => 0,
            egl::CONFIG_ID => 1,
            egl::SURFACE_TYPE => 0x5,  // WINDOW | PBUFFER
            egl::RENDERABLE_TYPE => 1, // OPENGL_ES
            0x302E /* NATIVE_VISUAL_ID */ | 0x302F /* NATIVE_VISUAL_TYPE */ => 0,
            0x3027 /* CONFIG_CAVEAT */ => egl::NONE as i32,
            0x3032 /* SAMPLE_BUFFERS */ | 0x3031 /* SAMPLES */ => 0,
            0x302A /* MAX_PBUFFER_WIDTH */ | 0x302C /* MAX_PBUFFER_HEIGHT */ => 4096,
            0x3029 /* LEVEL */ => 0,
            0x3039 /* BIND_TO_TEXTURE_RGB */ | 0x303A => 0,
            0x303B /* MIN_SWAP_INTERVAL */ => 0,
            0x303C /* MAX_SWAP_INTERVAL */ => 1,
            0x302D /* NATIVE_RENDERABLE */ => 0,
            0x3042 /* CONFORMANT */ => 1,
            0x303F /* COLOR_BUFFER_TYPE */ => 0x308E, // RGB_BUFFER
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests;
