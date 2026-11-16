//! OpenSL ES (sound) and OpenMAX AL (video) stand-ins: their objects and interfaces as the API
//! shapes them, an `SLObjectItf` being a pointer to a pointer to a table of functions. Each
//! table entry is a host function (`Runtime::host_fn`) named `sl:<interface>:<index>`, so one
//! dispatcher answers them all. Everything succeeds; what matters for sound later (the buffer
//! queue's callback and the buffers enqueued) is kept in `Sles`.

use crate::runtime::{Runtime, Stop};

const SUCCESS: u32 = 0;
/// SL_OBJECT_STATE_REALIZED
const REALIZED: u32 = 2;
/// Room in every table (the longest, SLEngineItf, has 13 entries).
const TABLE: u32 = 24;

#[derive(Default)]
pub struct Sles {
    /// Buffer-queue callbacks registered (queue interface, callback, context).
    pub callbacks: Vec<(u32, u32, u32)>,
    /// Buffers enqueued (queue interface, address, bytes), oldest first.
    pub queued: Vec<(u32, u32, u32)>,
    /// Play states set (play interface, state: 1 stopped, 2 paused, 3 playing).
    pub play_state: Vec<(u32, u32)>,
}

/// A new interface of kind `kind`: a pointer to a pointer to its table.
fn interface(rt: &mut Runtime, api: &str, kind: &str) -> u32 {
    let table = rt.malloc(4 * TABLE);
    for i in 0..TABLE {
        let f = rt.host_fn(&format!("{api}:{kind}:{i}"));
        rt.mem.w32(table + 4 * i, f);
    }
    let itf = rt.malloc(4);
    rt.mem.w32(itf, table);
    itf
}

/// The interface an IID names (its first 16 bytes hold the IID's name, `imports::init_data`).
fn kind_of(rt: &mut Runtime, iid: u32) -> String {
    let raw = rt.mem.read(iid, 16);
    let name = String::from_utf8_lossy(&raw).trim_end_matches('\0').to_string();
    let n = name.trim_start_matches("SL_IID_").trim_start_matches("XA_IID_");
    match n {
        "ENGINE" | "PLAY" | "VOLUME" | "SEEK" | "RECORD" | "NULL" => n.to_lowercase(),
        n if n.starts_with("ANDROIDSIMPLEBUF") || n.starts_with("BUFFERQUEUE") || n.starts_with("ANDROIDBUFFERQUE") => "queue".into(),
        n if n.starts_with("PREFETCH") => "prefetch".into(),
        n if n.starts_with("STREAMINFO") => "streaminfo".into(),
        n => n.to_lowercase(),
    }
}

/// `slCreateEngine` / `xaCreateEngine(pEngine, ...)`.
pub fn create_engine(rt: &mut Runtime, api: &str) {
    let out = rt.arg(0);
    let obj = interface(rt, api, "object");
    rt.mem.w32(out, obj);
    rt.ret(SUCCESS)
}

/// A function of one of the tables, by name `api:kind:index`.
pub fn call(rt: &mut Runtime, name: &str) -> Result<(), Stop> {
    let mut parts = name.splitn(3, ':');
    let (api, kind, index) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().and_then(|i| i.parse::<u32>().ok()).unwrap_or(99));
    let this = rt.arg(0);
    match (kind, index) {
        // SLObjectItf: Realize, Resume, GetState, GetInterface, RegisterCallback,
        // AbortAsyncOperation, Destroy, SetPriority, GetPriority, SetLossOfControlInterfaces
        ("object", 2) => {
            let out = rt.arg(1);
            rt.mem.w32(out, REALIZED);
        }
        ("object", 3) => {
            let (iid, out) = (rt.arg(1), rt.arg(2));
            let k = kind_of(rt, iid);
            let itf = interface(rt, api, &k);
            rt.mem.w32(out, itf);
        }
        // SLEngineItf: CreateLEDDevice, CreateVibraDevice, CreateAudioPlayer,
        // CreateAudioRecorder, CreateMidiPlayer, CreateListener, Create3DGroup,
        // CreateOutputMix, ... (XAEngineItf: CreateCameraDevice, CreateRadioDevice,
        // CreateLEDDevice, CreateVibraDevice, CreateMediaPlayer, ...): every Create* makes an
        // object.
        ("engine", i) if i < 12 => {
            let out = rt.arg(1);
            let obj = interface(rt, api, "object");
            rt.mem.w32(out, obj);
        }
        // SLPlayItf: SetPlayState(self, state), GetPlayState(self, *state), ...
        ("play", 0) => {
            let s = rt.arg(1);
            rt.sles.play_state.push((this, s));
        }
        ("play", 1) => {
            let out = rt.arg(1);
            let s = rt.sles.play_state.iter().rev().find(|p| p.0 == this).map_or(1, |p| p.1);
            rt.mem.w32(out, s);
        }
        // SLAndroidSimpleBufferQueueItf: Enqueue(self, buf, size), Clear, GetState(self,
        // *{count, index}), RegisterCallback(self, cb, ctx)
        ("queue", 0) => {
            let (buf, size) = (rt.arg(1), rt.arg(2));
            rt.sles.queued.push((this, buf, size));
        }
        ("queue", 1) => rt.sles.queued.retain(|q| q.0 != this),
        ("queue", 2) => {
            let out = rt.arg(1);
            let n = rt.sles.queued.iter().filter(|q| q.0 == this).count() as u32;
            rt.mem.w32(out, n);
            rt.mem.w32(out + 4, 0);
        }
        ("queue", 3) => {
            let (cb, ctx) = (rt.arg(1), rt.arg(2));
            rt.sles.callbacks.retain(|c| c.0 != this);
            rt.sles.callbacks.push((this, cb, ctx));
        }
        // Getters elsewhere (volume levels, durations...): zero.
        _ => {}
    }
    rt.ret(SUCCESS);
    Ok(())
}
