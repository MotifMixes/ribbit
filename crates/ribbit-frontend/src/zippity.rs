//! Playing the LeapFrog Zippity (`ribbit_zippity`): the machine runs on its
//! own thread at 60 frames a second, and the UI takes its picture and sends
//! it the controllers' state. What the system saves to its NAND (players,
//! scores) is kept in the save folder as the pages that differ from the dump.
//! The first time a dump is played, the session notes whether the system
//! found saved players in it (a previous owner's); the app then offers to
//! remove them from the dump file itself (`clean_dump`).
//!
//! Controls (the Leapster's bindings): the D-pad is the joystick, A chooses,
//! Hint stands on the mat (both foot pads), and B, Home and Pause are the
//! controller bits 4, 5 and 6, whose buttons are not identified yet
//! (docs/zippity.md).

use crate::audio_out::AudioSink;
use ribbit_input::Button;
use ribbit_zippity::{PAGE, Pad, SCREEN_H, SCREEN_W, Zippity};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The Zippity's inputs (its manual, the Parent Guide 605-11430-A, and the
/// factory test menu, which names bits 4-7) on Ribbit's buttons: the
/// Leapster's five action buttons for the Zippity's five other inputs. The
/// mat's spots are red and green to the sides and blue (the "Zippity" oval)
/// in front of the home spot, two footprints the child stands on; the
/// Pause/Home button is on the main unit, by the bopper (the joystick).
/// The feet keys (the Leapster's volume buttons, which the Zippity has no
/// use for: LB and RB on a gamepad) move one foot: lifted while standing on
/// the home spot, put down while not. Mickey Says starts with "march in
/// place", left and right in turn.
pub const CONTROLS: [(Button, u16, &str); 11] = [
    (Button::Up, Pad::UP, "Bopper up (away from you)"),
    (Button::Down, Pad::DOWN, "Bopper down (towards you)"),
    (Button::Left, Pad::LEFT, "Bopper left"),
    (Button::Right, Pad::RIGHT, "Bopper right"),
    (Button::A, Pad::CHOOSE, "Green spot (choose)"),
    (Button::B, Pad::RED, "Red spot"),
    (Button::Hint, Pad::BLUE, "Blue spot"),
    (Button::Home, Pad::HOME, "Pause/Home button"),
    (Button::Pause, Pad::LEFT_FOOT | Pad::RIGHT_FOOT, "Stand on the home spot"),
    (Button::VolumeDown, Pad::LEFT_FOOT, "Left foot (lift / step)"),
    (Button::VolumeUp, Pad::RIGHT_FOOT, "Right foot (lift / step)"),
];

const FEET: u16 = Pad::LEFT_FOOT | Pad::RIGHT_FOOT;

/// Frames a second (the model's stand-in timing: FRAME instructions each).
const FPS: f64 = 60.0;
/// The output queue's rate (mono).
pub(crate) const OUT_HZ: f64 = ribbit_audio::DAC_RATE_HZ as f64;

struct Shared {
    z: Mutex<Zippity>,
    /// The latest picture, 0x00RRGGBB.
    frame: Mutex<Vec<u32>>,
    running: AtomicBool,
    quit: AtomicBool,
    /// Buttons held (Pad bits).
    held: AtomicU16,
    /// Feet keys held (Pad foot bits): each moves its foot from where
    /// standing (or not) on the home spot puts it.
    moved: AtomicU16,
    /// Standing on the home spot (the footprints held) as well.
    on_home: AtomicBool,
    /// Volume, as f32 bits.
    gain: AtomicU32,
    /// Speed over the last second, percent of real time (f32 bits).
    speed: AtomicU32,
    sink: Mutex<Option<AudioSink>>,
    /// Still watching the boot log for saved players.
    watch: AtomicBool,
    /// The system loaded saved players from the dump.
    user_data: AtomicBool,
}

pub struct ZipSession {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub title: String,
    /// When play started (Unix seconds), for Discord's timer.
    pub started: u64,
    /// The dump file.
    pub nand: PathBuf,
    /// The dump as loaded, and where the system's changes to it are kept.
    original: Vec<u8>,
    save_path: PathBuf,
}

impl ZipSession {
    /// Powers on a Zippity from its NAND dump (`nand`, raw or zipped).
    /// Starts the system from its NAND dump, with a cartridge (a raw dump of
    /// its NAND, or a .zip of one) in the slot if given.
    pub fn start(nand: &Path, cart: Option<&Path>, save_dir: &Path, sink: Option<AudioSink>) -> anyhow::Result<Self> {
        let original = ribbit_core::read_image(nand)?;
        let cart = match cart {
            Some(path) => {
                let data = ribbit_core::read_image(path)?;
                anyhow::ensure!(data.len() == ribbit_zippity::CART_SIZE, "not a Zippity cartridge dump (raw, with spare bytes)");
                Some(data)
            }
            None => None,
        };
        anyhow::ensure!(original.len() >= 0x200_0000 && original.len() % PAGE == 0, "not a Zippity NAND dump (raw, with spare bytes)");
        let save_path = save_file(nand, save_dir);
        let mut flash = original.clone();
        // With no saves of ours yet, what the system loads is the dump's.
        let fresh = match std::fs::read(&save_path) {
            Ok(pages) => {
                apply_pages(&mut flash, &pages);
                false
            }
            Err(_) => true,
        };
        let shared = Arc::new(Shared {
            z: Mutex::new({
                let mut z = Zippity::new(flash);
                if let Some(cart) = cart {
                    z.insert_cartridge(cart);
                }
                z
            }),
            frame: Mutex::new(vec![0; SCREEN_W * SCREEN_H]),
            running: AtomicBool::new(true),
            quit: AtomicBool::new(false),
            held: AtomicU16::new(0),
            moved: AtomicU16::new(0),
            on_home: AtomicBool::new(false),
            gain: AtomicU32::new(1f32.to_bits()),
            speed: AtomicU32::new(100f32.to_bits()),
            sink: Mutex::new(sink),
            watch: AtomicBool::new(fresh),
            user_data: AtomicBool::new(false),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("ribbit-zippity".into()).spawn(move || {
            // Kept on the fast cores: on an E-core it falls behind and the
            // sound stutters.
            crate::perf::raise_thread_priority();
            run(&s)
        })?;
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Ok(Self { shared, thread: Some(thread), title: "Zippity".into(), started, nand: nand.to_path_buf(), original, save_path })
    }

    /// Copies the latest picture into `out` (SCREEN_W x SCREEN_H).
    pub fn frame(&self, out: &mut Vec<u32>) {
        out.clone_from(&self.shared.frame.lock().unwrap());
    }

    /// Whether the player stands on the mat's home spot (both footprints
    /// held, whatever else is pressed).
    pub fn set_on_home(&self, on: bool) {
        self.shared.on_home.store(on, Ordering::Relaxed);
    }

    /// True once, when the system has loaded saved players from the dump.
    pub fn take_user_data(&self) -> bool {
        self.shared.user_data.swap(false, Ordering::Relaxed)
    }

    /// Frames run in the last second, as a percentage of real time.
    pub fn speed_percent(&self) -> f64 {
        f32::from_bits(self.shared.speed.load(Ordering::Relaxed)) as f64
    }

    pub fn set_running(&self, running: bool) {
        self.shared.running.store(running, Ordering::Relaxed);
    }

    pub fn set_gain(&self, gain: f32) {
        self.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// A Leapster button, as the Zippity input it stands for (`CONTROLS`).
    pub fn set_button(&self, b: Button, pressed: bool) {
        let Some(&(_, bits, _)) = CONTROLS.iter().find(|c| c.0 == b) else { return };
        if matches!(b, Button::VolumeDown | Button::VolumeUp) {
            if pressed {
                self.shared.moved.fetch_or(bits, Ordering::Relaxed);
            } else {
                self.shared.moved.fetch_and(!bits, Ordering::Relaxed);
            }
            return;
        }
        if pressed {
            self.shared.held.fetch_or(bits, Ordering::Relaxed);
        } else {
            self.shared.held.fetch_and(!bits, Ordering::Relaxed);
        }
    }

    pub fn save_state(&self) -> Vec<u8> {
        self.shared.z.lock().unwrap().save_state()
    }

    pub fn load_state(&self, state: &[u8]) -> Result<(), String> {
        self.shared.z.lock().unwrap().load_state(state)
    }

    /// Restarts the console (keeping what it has saved).
    pub fn reset(&self) {
        let mut z = self.shared.z.lock().unwrap();
        let flash = z.nand().to_vec();
        *z = Zippity::new(flash);
    }

    /// Keeps what the system has saved to its NAND.
    pub fn flush(&self) -> anyhow::Result<()> {
        let mut z = self.shared.z.lock().unwrap();
        if !z.nand_dirty() {
            return Ok(());
        }
        let pages = changed_pages(&self.original, z.nand());
        if let Some(dir) = self.save_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.save_path.with_extension("tmp");
        std::fs::write(&tmp, &pages)?;
        std::fs::rename(&tmp, &self.save_path)?;
        z.nand_saved();
        Ok(())
    }

    /// Stops the machine (after keeping its saves).
    pub fn stop(mut self) {
        if let Err(e) = self.flush() {
            tracing::warn!(target: "ribbit::saves", "saving the Zippity's NAND: {e:#}");
        }
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Where the system's changes to the dump `nand` are kept.
pub fn save_file(nand: &Path, save_dir: &Path) -> PathBuf {
    let stem = nand.file_stem().map_or_else(|| "zippity".into(), |s| s.to_string_lossy().into_owned());
    save_dir.join(format!("{stem}.nandpages"))
}

/// Removes the saved players, progress and play log from the dump file
/// `nand` (`Zippity::factory_reset`, a few seconds). The original is kept
/// beside it as `<name>.with-saves.bak`; the cleaned dump is written as a
/// raw `.bin` (a zipped dump's too), whose path is returned.
pub fn clean_dump(nand: &Path) -> anyhow::Result<PathBuf> {
    let data = ribbit_core::read_image(nand)?;
    let cleaned = Zippity::factory_reset(data).map_err(anyhow::Error::msg)?;
    let out = nand.with_extension("bin");
    let mut backup = nand.as_os_str().to_owned();
    backup.push(".with-saves.bak");
    let backup = PathBuf::from(backup);
    if !backup.exists() {
        std::fs::rename(nand, &backup)?;
    }
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, &cleaned)?;
    std::fs::rename(&tmp, &out)?;
    Ok(out)
}

/// The emulation thread: a frame every 1/60 s of wall time while running.
fn run(s: &Shared) {
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    let mut next = Instant::now();
    let mut resample = Resampler::new(ribbit_zippity::OUTPUT_RATE);
    let mut mono = Vec::new();
    let (mut counted_since, mut frames) = (Instant::now(), 0u32);
    let mut skipped = 0;
    while !s.quit.load(Ordering::Relaxed) {
        if !s.running.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            next = Instant::now();
            (counted_since, frames) = (next, 0);
            continue;
        }
        let elapsed = counted_since.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            s.speed.store(((frames as f64 / elapsed / FPS * 100.0) as f32).to_bits(), Ordering::Relaxed);
            (counted_since, frames) = (Instant::now(), 0);
        }
        frames += 1;
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
        }
        // Behind by more than a few frames (a stall): carry on from now.
        if Instant::now() > next + Duration::from_millis(100) {
            next = Instant::now();
        }
        next += Duration::from_secs_f64(1.0 / FPS);
        // Behind (a busy stretch, such as loading a player): skip drawing,
        // at most four frames running, so the sound keeps up.
        let draw = Instant::now() <= next || skipped >= 4;
        skipped = if draw { 0 } else { skipped + 1 };
        let audio = {
            let mut z = s.z.lock().unwrap();
            // Standing on the home spot (the setting, or Pause held) puts
            // both feet down; a foot key moves its foot from there.
            let held = s.held.load(Ordering::Relaxed);
            let standing = if s.on_home.load(Ordering::Relaxed) { FEET } else { held & FEET };
            let feet = standing ^ s.moved.load(Ordering::Relaxed);
            z.set_pad(Pad::held(held & !FEET | feet));
            z.set_want_picture(draw);
            z.run_frame();
            if draw {
                z.render(&mut img);
            }
            // The boot log says whether the system found saved players.
            if s.watch.load(Ordering::Relaxed) {
                if z.log().contains("LD open OK: UI.SAV") {
                    s.user_data.store(true, Ordering::Relaxed);
                    s.watch.store(false, Ordering::Relaxed);
                } else if z.log().contains("could not open: UI.SAV") {
                    s.watch.store(false, Ordering::Relaxed);
                }
            }
            z.take_audio()
        };
        if draw {
            s.frame.lock().unwrap().copy_from_slice(&img);
        }
        mono.clear();
        resample.push(&audio, &mut mono);
        let gain = f32::from_bits(s.gain.load(Ordering::Relaxed));
        if let Some(sink) = s.sink.lock().unwrap().as_mut() {
            sink.push_32k(&mono, gain);
        }
    }
}

/// Stereo at a sound unit's rate to mono at the output queue's rate
/// (`OUT_HZ`): low-passed below the output's half rate first (a windowed
/// sinc), then linearly interpolated. Also the RockIt Twist's. Without the
/// filter, sound above 16 kHz (MP3s have plenty) folded back as hiss.
pub(crate) struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Position of the next output sample, in input samples.
    pos: f64,
    prev: f32,
    /// The low-pass filter's taps (empty when the input is not faster than
    /// the output), and the last input samples it needs.
    taps: Vec<f32>,
    history: Vec<f32>,
}

/// The filter: passes up to `PASS_HZ`, its Blackman window's transition
/// reaching about 17 kHz with `TAPS` taps at 72 kHz.
const TAPS: usize = 95;
const PASS_HZ: f64 = 13_000.0;

impl Resampler {
    pub(crate) fn new(from_hz: f64) -> Self {
        let taps = if from_hz > OUT_HZ * 1.1 {
            let fc = PASS_HZ / from_hz;
            let mid = (TAPS - 1) as f64 / 2.0;
            let mut t: Vec<f64> = (0..TAPS)
                .map(|i| {
                    let x = i as f64 - mid;
                    let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x) };
                    let w = 2.0 * std::f64::consts::PI * i as f64 / (TAPS - 1) as f64;
                    sinc * (0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos())
                })
                .collect();
            let sum: f64 = t.iter().sum();
            t.iter_mut().for_each(|v| *v /= sum);
            t.into_iter().map(|v| v as f32).collect()
        } else {
            Vec::new()
        };
        let history = vec![0.0; taps.len().saturating_sub(1)];
        Self { step: from_hz / OUT_HZ, pos: 0.0, prev: 0.0, taps, history }
    }

    pub(crate) fn push(&mut self, stereo: &[i16], out: &mut Vec<i16>) {
        let step = self.step;
        let mut mono: Vec<f32> = stereo.as_chunks::<2>().0.iter().map(|p| (p[0] as f32 + p[1] as f32) / 2.0).collect();
        if !self.taps.is_empty() {
            // The filter runs over the kept history and the new samples (a
            // fixed delay of half the taps, the same for everything).
            let keep = self.history.len();
            let mut all = std::mem::take(&mut self.history);
            all.extend_from_slice(&mono);
            for (i, m) in mono.iter_mut().enumerate() {
                *m = all[i..i + self.taps.len()].iter().zip(&self.taps).map(|(a, b)| a * b).sum();
            }
            self.history = all.split_off(all.len() - keep);
        }
        let n = mono.len() as f64;
        while self.pos < n {
            let i = self.pos.floor() as usize;
            let f = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.prev } else { mono[i - 1] };
            let b = mono[i];
            out.push((a + (b - a) * f).clamp(-32768.0, 32767.0) as i16);
            self.pos += step;
        }
        self.pos -= n;
        if let Some(&last) = mono.last() {
            self.prev = last;
        }
    }
}

/// The pages of `now` that differ from `original`: each a 32-bit page
/// number then the page.
fn changed_pages(original: &[u8], now: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, (a, b)) in original.chunks(PAGE).zip(now.chunks(PAGE)).enumerate() {
        if a != b {
            out.extend_from_slice(&(i as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
    }
    out
}

fn apply_pages(flash: &mut [u8], pages: &[u8]) {
    for entry in pages.as_chunks::<{ 4 + PAGE }>().0 {
        let i = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize;
        if let Some(page) = flash.get_mut(i * PAGE..(i + 1) * PAGE) {
            page.copy_from_slice(&entry[4..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_pages_round_trip() {
        let original = vec![0xFFu8; PAGE * 4];
        let mut now = original.clone();
        now[PAGE * 2 + 5] = 0x12;
        let pages = changed_pages(&original, &now);
        assert_eq!(pages.len(), 4 + PAGE);
        let mut flash = original.clone();
        apply_pages(&mut flash, &pages);
        assert_eq!(flash, now);
    }

    /// Cleans a copy of a real dump (`RIBBIT_ZIPPITY_DUMP=<file>`, which is
    /// replaced by the cleaned dump; its original is kept as the backup).
    #[test]
    #[ignore = "needs a Zippity NAND dump"]
    fn clean_dump_keeps_a_backup() {
        let dump = PathBuf::from(std::env::var("RIBBIT_ZIPPITY_DUMP").unwrap());
        let before = std::fs::read(&dump).unwrap();
        let out = clean_dump(&dump).unwrap();
        let mut backup = dump.clone().into_os_string();
        backup.push(".with-saves.bak");
        assert_eq!(std::fs::read(backup).unwrap(), before);
        let after = std::fs::read(&out).unwrap();
        assert_eq!(after.len(), before.len());
        assert!(!after.windows(7).any(|w| w == b"MATTHEW"));
    }

    /// A tone above the output's half rate is filtered out instead of
    /// folding back as an audible one; a tone well below passes.
    #[test]
    fn resampler_filters_what_would_fold_back() {
        let rate = ribbit_twist::sound::OUTPUT_RATE;
        let level = |hz: f64| {
            let mut r = Resampler::new(rate);
            let input: Vec<i16> = (0..rate as usize)
                .flat_map(|i| {
                    let v = (10000.0 * (2.0 * std::f64::consts::PI * hz * i as f64 / rate).sin()) as i16;
                    [v, v]
                })
                .collect();
            let mut out = Vec::new();
            for chunk in input.chunks(2 * 720) {
                r.push(chunk, &mut out);
            }
            let tail = &out[out.len() / 2..];
            (tail.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt()
        };
        let (low, high) = (level(1000.0), level(20000.0));
        assert!(low > 6500.0, "1 kHz passes: {low}");
        assert!(high < 100.0, "20 kHz (it would fold to 12 kHz) is gone: {high}");
    }

    #[test]
    fn resampler_rate() {
        let mut r = Resampler::new(ribbit_zippity::OUTPUT_RATE);
        let mut out = Vec::new();
        // One second of input gives about one second of output.
        let input = vec![0i16; 2 * ribbit_zippity::OUTPUT_RATE as usize];
        r.push(&input, &mut out);
        assert!((out.len() as f64 - OUT_HZ).abs() < 2.0, "{}", out.len());
    }
}
