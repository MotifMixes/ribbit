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
    /// The object each interface came from (GetInterface).
    pub object_of: std::collections::BTreeMap<u32, u32>,
    /// Audio players made from an asset descriptor: the clip's length (`clip_length`).
    pub length: std::collections::BTreeMap<u32, Option<std::time::Duration>>,
    /// SLPlayItf callbacks (play interface -> callback, context, event mask).
    pub play_callbacks: std::collections::BTreeMap<u32, (u32, u32, u32)>,
    /// When each playing clip ends (play interface, clock time), for `end_players`.
    pub ends: Vec<(u32, std::time::Duration)>,
}

/// SL_DATALOCATOR_ANDROIDFD
const ANDROID_FD: u32 = 0x8000_07BC;
/// SL_PLAYEVENT_HEADATEND
const HEAD_AT_END: u32 = 1;

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
            rt.sles.object_of.insert(itf, this);
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
            // CreateAudioPlayer(self, *player, source, sink, ...) from an asset descriptor
            // ({type, fd, offset (64-bit), length (64-bit)}): the clip's length, so the player
            // can end (no sound is made yet).
            let src = rt.arg(2);
            if api == "sl" && i == 2 && src != 0 {
                let loc = rt.mem.r32(src);
                if loc != 0 && rt.mem.r32(loc) == ANDROID_FD {
                    let (fd, off, len) = (rt.mem.r32(loc + 4), rt.mem.r32(loc + 8) as usize, rt.mem.r32(loc + 16) as usize);
                    let length = rt.assets.fds.get(&fd).and_then(|d| d.get(off..off.saturating_add(len).min(d.len()))).and_then(clip_length);
                    rt.sles.length.insert(obj, length);
                }
            }
        }
        // SLPlayItf: SetPlayState(self, state), GetPlayState(self, *state), ...
        ("play", 0) => {
            let s = rt.arg(1);
            rt.sles.play_state.push((this, s));
            // Playing starts the clip from where it is; it ends after its length (a clip of
            // unknown length after 1 s, GUESS). Stopped or paused, it doesn't end. Paused
            // clips start again from the beginning (GUESS: no position kept yet).
            rt.sles.ends.retain(|e| e.0 != this);
            let obj = rt.sles.object_of.get(&this).copied().unwrap_or(0);
            if let (3, Some(length)) = (s, rt.sles.length.get(&obj)) {
                let at = rt.state_elapsed() + length.unwrap_or(std::time::Duration::from_secs(1));
                rt.sles.ends.push((this, at));
            }
        }
        ("play", 1) => {
            let out = rt.arg(1);
            let s = rt.sles.play_state.iter().rev().find(|p| p.0 == this).map_or(1, |p| p.1);
            rt.mem.w32(out, s);
        }
        // SLPlayItf: RegisterCallback(self, callback, context), SetCallbackEventsMask(self,
        // mask)
        ("play", 4) => {
            let (cb, ctx) = (rt.arg(1), rt.arg(2));
            let mask = rt.sles.play_callbacks.get(&this).map_or(0, |c| c.2);
            rt.sles.play_callbacks.insert(this, (cb, ctx, mask));
        }
        ("play", 5) => {
            let mask = rt.arg(1);
            rt.sles.play_callbacks.entry(this).or_insert((0, 0, 0)).2 = mask;
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

/// A WAV or Ogg Vorbis clip's length: the data chunk over the byte rate, or the last page's
/// granule position over the sample rate.
pub fn clip_length(d: &[u8]) -> Option<std::time::Duration> {
    let u32_at = |i: usize| d.get(i..i + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let secs = if d.starts_with(b"RIFF") {
        let (mut i, mut rate, mut bytes) = (12, None, None);
        while let (Some(id), Some(n)) = (d.get(i..i + 4), u32_at(i + 4)) {
            match id {
                b"fmt " => rate = u32_at(i + 16),
                b"data" => bytes = Some(n.min((d.len() - i - 8) as u32)),
                _ => {}
            }
            i += 8 + n as usize + (n as usize & 1);
        }
        bytes? as f64 / rate.filter(|&r| r != 0)? as f64
    } else if d.starts_with(b"OggS") {
        let id = d.windows(7).position(|w| w == b"vorbis")?;
        let rate = u32_at(id + 12).filter(|&r| r != 0)?;
        let last = d.windows(4).rposition(|w| w == b"OggS")?;
        let granule = u64::from_le_bytes(d.get(last + 6..last + 14)?.try_into().unwrap());
        granule as f64 / rate as f64
    } else {
        return None;
    };
    Some(std::time::Duration::from_secs_f64(secs.min(3600.0)))
}

impl Runtime {
    /// Ends the clips whose time is up, as the player's thread would: the play state goes to
    /// paused (Android does that at the end) and the SLPlayItf callback gets HEADATEND if its
    /// mask asks for it. Called between frames, at the outer call level.
    pub fn end_players(&mut self) -> Result<(), Stop> {
        let now = self.state_elapsed();
        let due: Vec<u32> = self.sles.ends.iter().filter(|e| e.1 <= now).map(|e| e.0).collect();
        self.sles.ends.retain(|e| e.1 > now);
        for itf in due {
            self.sles.play_state.push((itf, 2));
            if let Some(&(cb, ctx, mask)) = self.sles.play_callbacks.get(&itf)
                && cb != 0
                && mask & HEAD_AT_END != 0
            {
                self.call(cb, &[itf, ctx, HEAD_AT_END])?;
            }
        }
        Ok(())
    }
}
