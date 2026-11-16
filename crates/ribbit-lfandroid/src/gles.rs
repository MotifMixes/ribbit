//! OpenGL ES 1.1 and EGL through ribbit-gles (software). On Android the Java shell's
//! GLSurfaceView owns the context and swaps after each `naDrawFrame`; the game draws into
//! the context that is current, so one context is made the first time GL is called.

use crate::runtime::{Runtime, Stop};
use ribbit_gles::abi::{self, Outcome};

/// The tablet's screen (Epic, LeapPad Academy: 1024x600); `Runtime::screen` is the window,
/// turned for a portrait title.
pub const SCREEN: (u32, u32) = (1024, 600);

pub struct Gles {
    pub gl: ribbit_gles::Gl,
    pub egl: ribbit_gles::Egl,
    /// LFA_GL=1: calls logged so far, by function (the first few of each are printed).
    logged: Option<std::collections::BTreeMap<String, u32>>,
}

struct Guest<'a>(&'a mut Runtime);

impl ribbit_gles::Memory for Guest<'_> {
    fn read(&mut self, addr: u32, len: usize) -> Option<Vec<u8>> {
        Some(self.0.mem.read(addr, len))
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> bool {
        self.0.mem.write(addr, data);
        true
    }
}

impl abi::Guest for Guest<'_> {
    fn arg(&mut self, i: usize) -> u32 {
        self.0.arg(i)
    }
    fn static_string(&mut self, s: &str) -> u32 {
        if let Some(&a) = self.0.gl_strings.get(s) {
            return a;
        }
        let a = self.0.alloc_str(s);
        self.0.gl_strings.insert(s.to_string(), a);
        a
    }
    fn window_size(&mut self, _native: u32) -> (u32, u32) {
        self.0.screen
    }
}

/// What GLSurfaceView sets up before the game draws: a window surface the screen's size
/// with a context current on it (handles of its own, above any EGL hands out).
fn new_context(screen: (u32, u32)) -> Box<Gles> {
    let mut egl = ribbit_gles::Egl::default();
    let (surface, context) = (0x5F00_0001, 0x5F00_0002);
    egl.initialized = true;
    egl.surfaces.insert(surface, screen);
    egl.contexts.insert(context);
    egl.current = Some((surface, context));
    let mut gl = ribbit_gles::Gl::new(screen.0, screen.1);
    gl.viewport(0, 0, screen.0 as i32, screen.1 as i32);
    gl.scissor(0, 0, screen.0 as i32, screen.1 as i32);
    Box::new(Gles { gl, egl, logged: std::env::var_os("LFA_GL").map(|_| Default::default()) })
}

pub fn call(rt: &mut Runtime, name: &str) -> Result<(), Stop> {
    let screen = rt.screen;
    let mut g = rt.gles.take().unwrap_or_else(|| new_context(screen));
    if let Some(n) = g.logged.as_mut().map(|l| l.entry(name.to_string()).or_default()) {
        *n += 1;
        if *n <= 4 {
            let args: Vec<u32> = (0..6).map(|i| rt.arg(i)).collect();
            eprintln!("[gl] {name}({args:x?})");
        }
    }
    let outcome = {
        let mut guest = Guest(rt);
        if name.starts_with("egl") { abi::egl_call(&mut g.egl, &mut g.gl, name, &mut guest) } else { abi::gl_call(&mut g.gl, name, &mut guest) }
    };
    rt.gles = Some(g);
    match outcome {
        Outcome::Value(v) => rt.ret(v),
        Outcome::Swap(_) => rt.ret(1),
        Outcome::Unknown => return Err(Stop::Import(name.to_string(), rt.cpu.r[14])),
    }
    Ok(())
}
