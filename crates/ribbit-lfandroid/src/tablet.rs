//! A title playing: what the Java shell does with the native library (onCreate, the adapters'
//! init calls, the GLSurfaceView's callbacks, then a frame per vertical blank), for a host that
//! shows the picture and passes on touches and buttons. The same steps as `lfa_run`.

use crate::jni::Obj;
use crate::runtime::{Runtime, Stop};
use std::path::Path;

pub struct Tablet {
    pub rt: Runtime,
    pub package: String,
    pub portrait: bool,
}

impl Tablet {
    /// Opens the APK (with `system_lib`, a firmware's /system/lib, for titles that need it),
    /// loads its library and starts it up to its first frame. `data` is the tablet's storage
    /// (saves, `files.rs`); None leaves LFA_DATA's.
    pub fn open(apk: &Path, system_lib: Option<&Path>, data: Option<&Path>) -> Result<Self, String> {
        let title = crate::open_title(apk, system_lib)?;
        let Some(loaded) = &title.library else { return Err(format!("{}: no native library found", title.package)) };
        let mut rt = Runtime::new(loaded);
        if let Some(d) = data {
            rt.files.root = Some(d.to_path_buf());
        }
        if title.portrait {
            rt.screen = (rt.screen.1, rt.screen.0);
        }
        let data = std::fs::read(apk).map_err(|e| format!("{}: {e}", apk.display()))?;
        rt.assets.apk = Some(crate::apk::Apk::open(data)?);
        let stop = |what: &str, e: Stop| format!("{what}: {e}");
        rt.run_constructors().map_err(|e| stop("constructors", e))?;
        rt.jni_onload().map_err(|e| stop("JNI_OnLoad", e))?;
        if let Some(n) = rt.natives.get("naInitMainActivity") {
            let activity = rt.jni.new_handle(Obj::Object(n.class.clone()));
            let dir = rt.jni.new_handle(Obj::Str(format!("/data/data/{}", title.package)));
            rt.call_native("naInitMainActivity", &[activity, dir]).map_err(|e| stop("naInitMainActivity", e))?;
        }
        let inits: Vec<(String, String)> = rt
            .natives
            .iter()
            .filter(|(n, v)| n.starts_with("naInit") && *n != "naInitMainActivity" && v.signature.ends_with(";)V"))
            .map(|(n, v)| (n.clone(), v.signature[2..v.signature.len() - 3].to_string()))
            .collect();
        for (n, class) in inits {
            let o = rt.jni.new_handle(Obj::Object(class));
            rt.call_native(&n, &[o]).map_err(|e| stop(&n, e))?;
        }
        let surface = rt.jni.new_handle(Obj::Object("android/view/Surface".into()));
        for n in ["naLFGLSurfaceViewSurfaceCreated", "naLFGLSurfaceViewSurfaceChanged", "naResume"] {
            if rt.natives.contains_key(n) {
                let args: &[u32] = if n == "naResume" { &[] } else { &[surface] };
                rt.call_native(n, args).map_err(|e| stop(n, e))?;
            }
        }
        Ok(Self { rt, package: title.package, portrait: title.portrait })
    }

    /// One frame: players that ended are told so, then `naDrawFrame` and the wait for the
    /// next vertical blank (other threads run there).
    pub fn frame(&mut self) -> Result<(), String> {
        self.rt.end_players().map_err(|e| format!("end_players: {e}"))?;
        self.rt.call_native("naDrawFrame", &[]).and_then(|_| self.rt.wait_vsync()).map_err(|e| format!("naDrawFrame: {e}"))
    }

    /// The window's size (1024x600, or 600x1024 for a portrait title).
    pub fn screen(&self) -> (u32, u32) {
        self.rt.screen
    }

    /// The window's picture (XRGB, `screen()` sized), or None before the game made its surface.
    pub fn picture(&self) -> Option<(u32, u32, Vec<u32>)> {
        let s = &self.rt.gles.as_ref()?.gl.surface;
        Some((s.width, s.height, s.to_xrgb()))
    }

    /// Moves the sound made since the last call (mono, `mixer::RATE`) to the end of `out`.
    pub fn take_audio(&mut self, out: &mut Vec<i16>) {
        self.rt.mix();
        out.append(&mut self.rt.sles.mixer.out);
    }

    /// The finger at a window position, or lifted.
    pub fn touch(&mut self, at: Option<(f32, f32)>) -> Result<(), String> {
        self.rt.touch(at).map_err(|e| format!("touch: {e}"))
    }

    /// The buttons held (`input::button` bits).
    pub fn set_buttons(&mut self, held: u32) -> Result<(), String> {
        self.rt.set_buttons(held).map_err(|e| format!("buttons: {e}"))
    }
}
