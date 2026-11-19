//! OpenSL ES (sound) and OpenMAX AL (video) stand-ins: their objects and interfaces as the API
//! shapes them, an `SLObjectItf` being a pointer to a pointer to a table of functions. Each
//! table entry is a host function (`Runtime::host_fn`) named `sl:<interface>:<index>`, so one
//! dispatcher answers them all. Everything succeeds. Audio players from an asset descriptor play
//! through `mixer` (WAV and Ogg Vorbis clips); buffer queues are only kept track of (no title
//! seen uses them).

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
    /// Audio players made from an asset descriptor: their voices, mixed.
    pub mixer: crate::mixer::Mixer,
    /// SLPlayItf callbacks (play interface -> callback, context, event mask).
    pub play_callbacks: std::collections::BTreeMap<u32, (u32, u32, u32)>,
}

impl Sles {
    /// The voice of the player an interface belongs to.
    fn voice(&mut self, itf: u32) -> Option<&mut crate::mixer::Voice> {
        let obj = self.object_of.get(&itf)?;
        self.mixer.voices.get_mut(obj)
    }
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
        // Destroy: a player's voice goes.
        ("object", 6) => {
            rt.sles.mixer.voices.remove(&this);
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
            // gets a voice.
            let src = rt.arg(2);
            if api == "sl" && i == 2 && src != 0 {
                let loc = rt.mem.r32(src);
                if loc != 0 && rt.mem.r32(loc) == ANDROID_FD {
                    let (fd, off, len) = (rt.mem.r32(loc + 4), rt.mem.r32(loc + 8) as usize, rt.mem.r32(loc + 16) as usize);
                    let clip = rt.assets.fds.get(&fd).and_then(|d| d.get(off..off.saturating_add(len).min(d.len()))).map(<[u8]>::to_vec).unwrap_or_default();
                    rt.sles.mixer.add(obj, &clip);
                }
            }
        }
        // SLPlayItf: SetPlayState(self, state), GetPlayState(self, *state), ...
        ("play", 0) => {
            let s = rt.arg(1);
            rt.sles.play_state.push((this, s));
            // Playing goes on from where the clip is, paused keeps the place, stopped goes
            // back to the start. A clip played to its end is there until stopped or moved.
            let now = rt.state_elapsed();
            rt.sles.mixer.mix(now);
            if let Some(v) = rt.sles.voice(this) {
                v.playing = s == 3;
                if s == 1 {
                    v.pos = 0;
                } else if s == 3 && v.pos >= v.pcm.len() && v.looping.is_none() {
                    v.pos = 0;
                }
            }
        }
        ("play", 1) => {
            let out = rt.arg(1);
            let s = rt.sles.play_state.iter().rev().find(|p| p.0 == this).map_or(1, |p| p.1);
            rt.mem.w32(out, s);
        }
        // GetDuration(self, *ms), GetPosition(self, *ms)
        ("play", 2 | 3) => {
            let out = rt.arg(1);
            let ms = rt.sles.voice(this).map_or(0, |v| if index == 2 { v.length() } else { v.position() }.as_millis() as u32);
            rt.mem.w32(out, ms);
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
        // SLSeekItf: SetPosition(self, ms, mode), SetLoop(self, on, start ms, end ms; end
        // SL_TIME_UNKNOWN = the clip's end)
        ("seek", 0) => {
            let at = crate::mixer::sample_at(rt.arg(1));
            if let Some(v) = rt.sles.voice(this) {
                v.pos = at;
            }
        }
        ("seek", 1) => {
            let (on, start, end) = (rt.arg(1), rt.arg(2), rt.arg(3));
            if let Some(v) = rt.sles.voice(this) {
                v.looping = (on != 0).then(|| (crate::mixer::sample_at(start), if end == u32::MAX { usize::MAX } else { crate::mixer::sample_at(end) }));
            }
        }
        // SLVolumeItf: SetVolumeLevel(self, millibels), GetVolumeLevel, GetMaxVolumeLevel,
        // SetMute(self, on)
        ("volume", 0) => {
            let mb = rt.arg(1) as i16;
            if let Some(v) = rt.sles.voice(this) {
                v.gain = 10f32.powf(mb as f32 / 2000.0);
            }
        }
        ("volume", 1) => {
            let out = rt.arg(1);
            let mb = rt.sles.voice(this).map_or(0, |v| (2000.0 * v.gain.max(1e-5).log10()) as i16);
            rt.mem.w16(out, mb as u16);
        }
        ("volume", 3) => {
            let on = rt.arg(1);
            if let Some(v) = rt.sles.voice(this) {
                v.muted = on != 0;
            }
        }
        // Getters elsewhere (max volume level, stereo position...): zero.
        _ => {}
    }
    rt.ret(SUCCESS);
    Ok(())
}

impl Runtime {
    /// Mixes the players' sound up to now; `sles.mixer.out` has it.
    pub fn mix(&mut self) {
        let now = self.state_elapsed();
        self.sles.mixer.mix(now);
    }

    /// Ends the clips that ran out, as the player's thread would: the play state goes to
    /// paused (Android does that at the end) and the SLPlayItf callback gets HEADATEND if its
    /// mask asks for it. Called between frames, at the outer call level.
    pub fn end_players(&mut self) -> Result<(), Stop> {
        self.mix();
        let ended = std::mem::take(&mut self.sles.mixer.ended);
        // The players' SLPlayItfs: those that set a state or a callback.
        let sl = &self.sles;
        let is_play = |itf: u32| sl.play_callbacks.contains_key(&itf) || sl.play_state.iter().any(|p| p.0 == itf);
        let play_itfs: Vec<u32> = sl.object_of.iter().filter(|&(&itf, obj)| ended.contains(obj) && is_play(itf)).map(|(&itf, _)| itf).collect();
        for itf in play_itfs {
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
