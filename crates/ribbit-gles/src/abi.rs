//! The GLES 1.1 and EGL entry points by name, for 32-bit ARM programs with
//! the soft-float calling convention (AAPCS base standard: every argument
//! in r0-r3 and then on the stack, floats as their bits, results in r0).
//! The caller (an HLE) traps a call to one of these functions, gives its
//! name and a [`Guest`] that reads the arguments and memory, puts the
//! result in r0 and returns to the caller's lr.

use crate::{Egl, Gl, Memory, egl, gl};

/// What `call` needs from the calling program.
pub trait Guest: Memory {
    /// Argument `i` as the callee sees it (r0-r3, then [sp], [sp+4], ...).
    fn arg(&mut self, i: usize) -> u32;
    /// A NUL-terminated string in the program's memory that lives as long
    /// as the program (for glGetString/eglQueryString), by its contents.
    fn static_string(&mut self, s: &str) -> u32;
    /// The size of an EGL native window (`eglCreateWindowSurface`'s argument).
    fn window_size(&mut self, native: u32) -> (u32, u32);
}

/// What a call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Done; r0 = this (0 for void functions).
    Value(u32),
    /// eglSwapBuffers of this surface: show the context's surface, then r0 = EGL_TRUE.
    Swap(u32),
    /// Not a function this module knows.
    Unknown,
}

fn fx(v: u32) -> f32 {
    v as i32 as f32 / 65536.
}

fn fl(v: u32) -> f32 {
    f32::from_bits(v)
}

fn read_u32s(g: &mut dyn Guest, at: u32, n: usize) -> Option<Vec<u32>> {
    Some(g.read(at, n * 4)?.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn write_u32s(g: &mut dyn Guest, at: u32, v: &[u32]) {
    let b: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    g.write(at, &b);
}

fn matrix(g: &mut dyn Guest, at: u32, fixed: bool) -> Option<crate::Mat> {
    let w = read_u32s(g, at, 16)?;
    let mut m = [0f32; 16];
    for i in 0..16 {
        m[i] = if fixed { fx(w[i]) } else { fl(w[i]) };
    }
    Some(m)
}

/// GLES 1.1 entry point `name` (a `gl...` symbol) with the arguments in `g`.
pub fn gl_call(gl: &mut Gl, name: &str, g: &mut dyn Guest) -> Outcome {
    let a = |g: &mut dyn Guest, i: usize| g.arg(i);
    // A 4-component call in fixed (x), float (f) or unsigned byte (ub).
    macro_rules! four {
        ($conv:expr) => {{
            let v = [a(g, 0), a(g, 1), a(g, 2), a(g, 3)];
            v.map($conv)
        }};
    }
    let v = match name {
        "glActiveTexture" => gl.active_texture(a(g, 0)),
        "glClientActiveTexture" => gl.client_active_texture(a(g, 0)),
        "glAlphaFunc" => gl.alpha_func(a(g, 0), fl(a(g, 1))),
        "glAlphaFuncx" => gl.alpha_func(a(g, 0), fx(a(g, 1))),
        "glBindTexture" => gl.bind_texture(a(g, 0), a(g, 1)),
        "glBlendFunc" => gl.blend_func(a(g, 0), a(g, 1)),
        "glClear" => gl.clear(a(g, 0)),
        "glClearColor" => gl.clear_color(four!(fl)),
        "glClearColorx" => gl.clear_color(four!(fx)),
        "glClearDepthf" => gl.clear_depth(fl(a(g, 0))),
        "glClearDepthx" => gl.clear_depth(fx(a(g, 0))),
        "glClearStencil" => {}
        "glColor4f" => gl.color(four!(fl)),
        "glColor4x" => gl.color(four!(fx)),
        "glColor4ub" => gl.color(four!(|v: u32| (v & 0xFF) as f32 / 255.)),
        "glColorMask" => gl.color_mask(four!(|v: u32| v & 0xFF != 0)),
        "glCullFace" => gl.cull_face(a(g, 0)),
        "glFrontFace" => gl.front_face(a(g, 0)),
        "glDepthFunc" => gl.depth_func(a(g, 0)),
        "glDepthMask" => gl.depth_mask(a(g, 0) & 0xFF != 0),
        "glDepthRangef" => gl.depth_range(fl(a(g, 0)), fl(a(g, 1))),
        "glDepthRangex" => gl.depth_range(fx(a(g, 0)), fx(a(g, 1))),
        "glDisable" => gl.set_cap(a(g, 0), false),
        "glEnable" => gl.set_cap(a(g, 0), true),
        "glDisableClientState" => gl.client_state(a(g, 0), false),
        "glEnableClientState" => gl.client_state(a(g, 0), true),
        "glIsEnabled" => return Outcome::Value(gl.is_enabled(a(g, 0)) as u32),
        "glFinish" | "glFlush" | "glHint" | "glStencilFunc" | "glStencilOp" | "glStencilMask" | "glLogicOp" | "glSampleCoverage" | "glSampleCoveragex" => {}
        "glFogf" | "glFogfv" | "glFogx" | "glFogxv" | "glLightf" | "glLightfv" | "glLightx" | "glLightxv" | "glLightModelf" | "glLightModelfv" | "glLightModelx"
        | "glLightModelxv" | "glMaterialf" | "glMaterialfv" | "glMaterialx" | "glMaterialxv" | "glClipPlanef" | "glClipPlanex" | "glPointParameterf"
        | "glPointParameterfv" | "glPointParameterx" | "glPointParameterxv" => gl.unsupported_call(name),
        "glGetError" => return Outcome::Value(gl.get_error()),
        "glLineWidth" => gl.line_width(fl(a(g, 0))),
        "glLineWidthx" => gl.line_width(fx(a(g, 0))),
        "glPointSize" => gl.point_size(fl(a(g, 0))),
        "glPointSizex" => gl.point_size(fx(a(g, 0))),
        "glPolygonOffset" => gl.polygon_offset(fl(a(g, 0)), fl(a(g, 1))),
        "glPolygonOffsetx" => gl.polygon_offset(fx(a(g, 0)), fx(a(g, 1))),
        "glShadeModel" => gl.shade_model(a(g, 0)),
        "glPixelStorei" => gl.pixel_store(a(g, 0), a(g, 1) as i32),
        "glScissor" => gl.scissor(a(g, 0) as i32, a(g, 1) as i32, a(g, 2) as i32, a(g, 3) as i32),
        "glViewport" => gl.viewport(a(g, 0) as i32, a(g, 1) as i32, a(g, 2) as i32, a(g, 3) as i32),
        "glNormal3f" => gl.normal([fl(a(g, 0)), fl(a(g, 1)), fl(a(g, 2))]),
        "glNormal3x" => gl.normal([fx(a(g, 0)), fx(a(g, 1)), fx(a(g, 2))]),
        "glMultiTexCoord4f" => gl.multi_tex_coord([fl(a(g, 1)), fl(a(g, 2)), fl(a(g, 3)), fl(a(g, 4))]),
        "glMultiTexCoord4x" => gl.multi_tex_coord([fx(a(g, 1)), fx(a(g, 2)), fx(a(g, 3)), fx(a(g, 4))]),
        // Matrices.
        "glMatrixMode" => gl.matrix_mode(a(g, 0)),
        "glLoadIdentity" => gl.load_identity(),
        "glPushMatrix" => gl.push_matrix(),
        "glPopMatrix" => gl.pop_matrix(),
        "glLoadMatrixf" | "glLoadMatrixx" | "glMultMatrixf" | "glMultMatrixx" => {
            let at = a(g, 0);
            match matrix(g, at, name.ends_with('x')) {
                Some(m) if name.starts_with("glLoad") => gl.load_matrix(&m),
                Some(m) => gl.mult_matrix(&m),
                None => {}
            }
        }
        "glTranslatef" => gl.translate(fl(a(g, 0)), fl(a(g, 1)), fl(a(g, 2))),
        "glTranslatex" => gl.translate(fx(a(g, 0)), fx(a(g, 1)), fx(a(g, 2))),
        "glScalef" => gl.scale(fl(a(g, 0)), fl(a(g, 1)), fl(a(g, 2))),
        "glScalex" => gl.scale(fx(a(g, 0)), fx(a(g, 1)), fx(a(g, 2))),
        "glRotatef" => gl.rotate(fl(a(g, 0)), fl(a(g, 1)), fl(a(g, 2)), fl(a(g, 3))),
        "glRotatex" => gl.rotate(fx(a(g, 0)), fx(a(g, 1)), fx(a(g, 2)), fx(a(g, 3))),
        "glOrthof" | "glOrthox" | "glFrustumf" | "glFrustumx" => {
            let conv: fn(u32) -> f32 = if name.ends_with('x') { fx } else { fl };
            let v: Vec<f32> = (0..6).map(|i| conv(a(g, i))).collect();
            if name.starts_with("glOrtho") {
                gl.ortho(v[0], v[1], v[2], v[3], v[4], v[5]);
            } else {
                gl.frustum(v[0], v[1], v[2], v[3], v[4], v[5]);
            }
        }
        // Textures.
        "glGenTextures" | "glGenBuffers" => {
            let (n, at) = (a(g, 0) as usize, a(g, 1));
            let names = gl.gen_names(n.min(1 << 16));
            write_u32s(g, at, &names);
        }
        "glDeleteTextures" | "glDeleteBuffers" => {
            let (n, at) = (a(g, 0) as usize, a(g, 1));
            if let Some(names) = read_u32s(g, at, n.min(1 << 16)) {
                if name == "glDeleteTextures" { gl.delete_textures(&names) } else { gl.delete_buffers(&names) }
            }
        }
        "glIsTexture" => return Outcome::Value(gl.is_texture(a(g, 0)) as u32),
        "glIsBuffer" => return Outcome::Value(gl.is_buffer(a(g, 0)) as u32),
        "glTexParameteri" | "glTexParameterx" => gl.tex_parameter(a(g, 0), a(g, 1), a(g, 2)),
        "glTexParameterf" => gl.tex_parameter(a(g, 0), a(g, 1), fl(a(g, 2)) as u32),
        "glTexParameteriv" | "glTexParameterxv" | "glTexParameterfv" => {
            let at = a(g, 2);
            if let Some(v) = read_u32s(g, at, 1) {
                let value = if name.ends_with("fv") { fl(v[0]) as u32 } else { v[0] };
                gl.tex_parameter(a(g, 0), a(g, 1), value);
            }
        }
        "glGetTexParameteriv" | "glGetTexParameterxv" | "glGetTexParameterfv" => {
            let v = gl.tex_parameter_get(a(g, 1));
            let at = a(g, 2);
            write_u32s(g, at, &[if name.ends_with("fv") { (v as f32).to_bits() } else { v }]);
        }
        "glTexEnvi" | "glTexEnvx" => {
            let v = a(g, 2);
            gl.tex_env(a(g, 0), a(g, 1), &[v as f32]);
        }
        "glTexEnvf" => gl.tex_env(a(g, 0), a(g, 1), &[fl(a(g, 2))]),
        "glTexEnviv" | "glTexEnvxv" | "glTexEnvfv" => {
            let (target, pname, at) = (a(g, 0), a(g, 1), a(g, 2));
            let n = if pname == gl::TEXTURE_ENV_COLOR { 4 } else { 1 };
            if let Some(v) = read_u32s(g, at, n) {
                let vals: Vec<f32> = v
                    .iter()
                    .map(|&x| match (name, n) {
                        ("glTexEnvfv", _) => fl(x),
                        ("glTexEnvxv", 4) => fx(x),
                        ("glTexEnviv", 4) => x as i32 as f32 / i32::MAX as f32,
                        _ => x as f32,
                    })
                    .collect();
                gl.tex_env(target, pname, &vals);
            }
        }
        "glTexImage2D" => {
            let (level, internal, w, h, _border, format, kind, pixels) = (a(g, 1) as i32, a(g, 2), a(g, 3), a(g, 4), a(g, 5), a(g, 6), a(g, 7), a(g, 8));
            let data = if pixels == 0 { None } else { gl.image_bytes(w, h, format, kind).and_then(|n| g.read(pixels, n)) };
            gl.tex_image(level, internal, w, h, format, kind, data.as_deref());
        }
        "glTexSubImage2D" => {
            let (level, x, y, w, h, format, kind, pixels) = (a(g, 1) as i32, a(g, 2) as i32, a(g, 3) as i32, a(g, 4), a(g, 5), a(g, 6), a(g, 7), a(g, 8));
            if let Some(d) = gl.image_bytes(w, h, format, kind).and_then(|n| g.read(pixels, n)) {
                gl.tex_sub_image(level, x, y, w, h, format, kind, &d);
            }
        }
        "glCompressedTexImage2D" => {
            let (level, format, w, h, size, data) = (a(g, 1) as i32, a(g, 2), a(g, 3), a(g, 4), a(g, 6), a(g, 7));
            if let Some(d) = g.read(data, size as usize) {
                gl.compressed_tex_image(level, format, w, h, &d);
            }
        }
        "glCopyTexImage2D" | "glCopyTexSubImage2D" | "glCompressedTexSubImage2D" => gl.unsupported_call(name),
        // Buffers and arrays.
        "glBindBuffer" => gl.bind_buffer(a(g, 0), a(g, 1)),
        "glBufferData" => {
            let (target, size, data) = (a(g, 0), a(g, 1) as usize, a(g, 2));
            let bytes = if data == 0 { vec![0; size] } else { g.read(data, size).unwrap_or_else(|| vec![0; size]) };
            gl.buffer_data(target, bytes);
        }
        "glBufferSubData" => {
            let (target, offset, size, data) = (a(g, 0), a(g, 1) as usize, a(g, 2) as usize, a(g, 3));
            if let Some(d) = g.read(data, size) {
                gl.buffer_sub_data(target, offset, &d);
            }
        }
        "glVertexPointer" => gl.set_array(gl::VERTEX_ARRAY, a(g, 0), a(g, 1), a(g, 2), a(g, 3)),
        "glColorPointer" => gl.set_array(gl::COLOR_ARRAY, a(g, 0), a(g, 1), a(g, 2), a(g, 3)),
        "glTexCoordPointer" => gl.set_array(gl::TEXTURE_COORD_ARRAY, a(g, 0), a(g, 1), a(g, 2), a(g, 3)),
        "glNormalPointer" => gl.set_array(gl::NORMAL_ARRAY, 3, a(g, 0), a(g, 1), a(g, 2)),
        "glPointSizePointerOES" => gl.set_array(gl::POINT_SIZE_ARRAY_OES, 1, a(g, 0), a(g, 1), a(g, 2)),
        "glDrawArrays" => {
            let (mode, first, count) = (a(g, 0), a(g, 1) as i32, a(g, 2) as i32);
            gl.draw_arrays(g, mode, first, count);
        }
        "glDrawElements" => {
            let (mode, count, kind, indices) = (a(g, 0), a(g, 1) as i32, a(g, 2), a(g, 3));
            gl.draw_elements(g, mode, count, kind, indices);
        }
        // Queries.
        "glGetIntegerv" | "glGetBooleanv" | "glGetFixedv" | "glGetFloatv" => {
            let (pname, at) = (a(g, 0), a(g, 1));
            match name {
                "glGetIntegerv" => {
                    if let Some(v) = gl.get_integers(pname) {
                        write_u32s(g, at, &v.iter().map(|&x| x as u32).collect::<Vec<_>>());
                    }
                }
                "glGetBooleanv" => {
                    if let Some(v) = gl.get_integers(pname) {
                        let b: Vec<u8> = v.iter().map(|&x| (x != 0) as u8).collect();
                        g.write(at, &b);
                    }
                }
                _ => {
                    if let Some(v) = gl.get_floats(pname) {
                        let w: Vec<u32> = v.iter().map(|&x| if name == "glGetFixedv" { (x * 65536.) as i32 as u32 } else { x.to_bits() }).collect();
                        write_u32s(g, at, &w);
                    }
                }
            }
        }
        "glGetString" => {
            return Outcome::Value(match gl.get_string(a(g, 0)) {
                Some(s) => g.static_string(s),
                None => 0,
            });
        }
        "glGetPointerv" => {
            let (pname, at) = (a(g, 0), a(g, 1));
            let p = match pname {
                0x808E => gl.vertex.pointer,
                0x8090 => gl.color_array.pointer,
                0x808F => gl.normal_array.pointer,
                0x8092 => gl.tex_array.pointer,
                _ => 0,
            };
            write_u32s(g, at, &[p]);
        }
        "glReadPixels" => {
            let (x, y, w, h, _format, _kind, at) = (a(g, 0) as i32, a(g, 1) as i32, a(g, 2), a(g, 3), a(g, 4), a(g, 5), a(g, 6));
            let px = gl.read_pixels(x, y, w, h);
            g.write(at, &px);
        }
        _ => return Outcome::Unknown,
    };
    let () = v;
    Outcome::Value(0)
}

/// EGL 1.x entry point `name` (an `egl...` symbol).
pub fn egl_call(e: &mut Egl, gl: &mut Gl, name: &str, g: &mut dyn Guest) -> Outcome {
    const TRUE: u32 = 1;
    let ok = |e: &mut Egl, v: u32| {
        e.error = egl::SUCCESS;
        Outcome::Value(v)
    };
    match name {
        "eglGetDisplay" => ok(e, egl::DISPLAY),
        "eglInitialize" => {
            e.initialized = true;
            let (major, minor) = (g.arg(1), g.arg(2));
            if major != 0 {
                write_u32s(g, major, &[1]);
            }
            if minor != 0 {
                write_u32s(g, minor, &[4]);
            }
            ok(e, TRUE)
        }
        "eglTerminate" | "eglReleaseThread" | "eglWaitGL" | "eglWaitNative" | "eglWaitClient" | "eglBindAPI" => ok(e, TRUE),
        "eglQueryAPI" => ok(e, 0x30A0), // OPENGL_ES_API
        "eglGetError" => Outcome::Value(std::mem::replace(&mut e.error, egl::SUCCESS).max(egl::SUCCESS)),
        "eglGetConfigs" | "eglChooseConfig" => {
            // (dpy, attrib_list, configs, size, num) / (dpy, configs, size, num).
            let (configs, size, num) = if name == "eglChooseConfig" { (g.arg(2), g.arg(3), g.arg(4)) } else { (g.arg(1), g.arg(2), g.arg(3)) };
            let n = if configs == 0 { 1 } else { size.min(1) };
            if configs != 0 && n > 0 {
                write_u32s(g, configs, &[egl::CONFIG]);
            }
            if num != 0 {
                write_u32s(g, num, &[n]);
            }
            ok(e, TRUE)
        }
        "eglGetConfigAttrib" => {
            let (attr, at) = (g.arg(2), g.arg(3));
            match Egl::config_attrib(attr) {
                Some(v) => {
                    write_u32s(g, at, &[v as u32]);
                    ok(e, TRUE)
                }
                None => {
                    e.error = 0x3004; // BAD_ATTRIBUTE
                    Outcome::Value(0)
                }
            }
        }
        "eglCreateWindowSurface" | "eglCreatePbufferSurface" | "eglCreatePixmapSurface" => {
            let size = if name == "eglCreateWindowSurface" {
                let native = g.arg(2);
                g.window_size(native)
            } else {
                // Pbuffer: EGL_WIDTH/EGL_HEIGHT in the attribute list.
                let mut size = (1, 1);
                let mut at = g.arg(2);
                while at != 0 {
                    let Some(kv) = read_u32s(g, at, 2) else { break };
                    match kv[0] {
                        egl::WIDTH => size.0 = kv[1],
                        egl::HEIGHT => size.1 = kv[1],
                        egl::NONE => break,
                        _ => {}
                    }
                    at += 8;
                }
                size
            };
            let h = e.handle();
            e.surfaces.insert(h, size);
            ok(e, h)
        }
        "eglDestroySurface" => {
            e.surfaces.remove(&g.arg(1));
            ok(e, TRUE)
        }
        "eglCreateContext" => {
            let h = e.handle();
            e.contexts.insert(h);
            ok(e, h)
        }
        "eglDestroyContext" => {
            e.contexts.remove(&g.arg(1));
            ok(e, TRUE)
        }
        "eglMakeCurrent" => {
            let (draw, ctx) = (g.arg(1), g.arg(3));
            if ctx == 0 {
                e.current = None;
            } else {
                e.current = Some((draw, ctx));
                if let Some(&(w, h)) = e.surfaces.get(&draw) {
                    let first = gl.surface.width != w || gl.surface.height != h;
                    gl.resize(w, h);
                    if first {
                        gl.viewport(0, 0, w as i32, h as i32);
                        gl.scissor(0, 0, w as i32, h as i32);
                    }
                }
            }
            ok(e, TRUE)
        }
        "eglGetCurrentContext" => ok(e, e.current.map_or(0, |c| c.1)),
        "eglGetCurrentSurface" => ok(e, e.current.map_or(0, |c| c.0)),
        "eglGetCurrentDisplay" => ok(e, if e.current.is_some() { egl::DISPLAY } else { 0 }),
        "eglQuerySurface" => {
            let (s, attr, at) = (g.arg(1), g.arg(2), g.arg(3));
            let (w, h) = e.surfaces.get(&s).copied().unwrap_or((0, 0));
            let v = match attr {
                egl::WIDTH => w,
                egl::HEIGHT => h,
                egl::CONFIG_ID => 1,
                _ => 0,
            };
            write_u32s(g, at, &[v]);
            ok(e, TRUE)
        }
        "eglQueryContext" => {
            let at = g.arg(3);
            write_u32s(g, at, &[1]);
            ok(e, TRUE)
        }
        "eglQueryString" => {
            let s = match g.arg(1) {
                egl::VENDOR => "Ribbit",
                egl::VERSION => "1.4 Ribbit",
                egl::EXTENSIONS => "",
                egl::CLIENT_APIS => "OpenGL_ES",
                _ => return ok(e, 0),
            };
            let p = g.static_string(s);
            ok(e, p)
        }
        "eglSwapInterval" => {
            e.swap_interval = g.arg(1) as i32;
            ok(e, TRUE)
        }
        "eglSwapBuffers" => {
            e.error = egl::SUCCESS;
            Outcome::Swap(g.arg(1))
        }
        "eglSurfaceAttrib" | "eglBindTexImage" | "eglReleaseTexImage" | "eglCopyBuffers" => ok(e, TRUE),
        "eglGetProcAddress" => ok(e, 0),
        _ => Outcome::Unknown,
    }
}

impl Gl {
    fn unsupported_call(&mut self, name: &str) {
        self.note(name);
    }
}
