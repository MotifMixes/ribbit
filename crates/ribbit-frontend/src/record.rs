//! Screen recording: raw video frames + audio samples written to temp files,
//! then encoded after the recording stops (so the capture is fast and the
//! output has no real-time stutters regardless of PC load).
//!
//! The game is 160×160 pixels; frames are written at `FPS` by the sound's
//! clock (`frames_due`), whatever rate the window redraws at, with the
//! speaker's 32 kHz mono sound (`AUDIO_RATE`).
//!
//! Encoding, best first, so a recording works without installing anything:
//! 1. FFmpeg, if it is on PATH (the only way to get the H.265/VP9/Lossless
//!    choices);
//! 2. on Windows, Media Foundation (built into Windows 10/11): H.264 + AAC
//!    in an .mp4;
//! 3. anywhere, an uncompressed .avi, so a recording is never lost.
//!
//! The picture is scaled 2× to 320×320 (nearest-neighbour) so the output is
//! watchable without further scaling. The temp files are only deleted once a
//! finished video exists.

use crate::settings::RecordEncoder;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The recording's frame rate. Frames are written to match the sound
/// recorded so far (`frames_due`), not one per window redraw: on a 144 Hz
/// screen that was 144 frames a second played as 60, the sound running
/// ahead of the picture.
pub const FPS: u32 = 60;

/// The recording's sound: the speaker's 32 kHz (8 kHz sound raised to it).
pub const AUDIO_RATE: u32 = ribbit_audio::DAC_RATE_HZ;

/// The game's picture, as recorded.
const WIDTH: usize = 160;
const HEIGHT: usize = 160;
const FRAME_BYTES: usize = WIDTH * HEIGHT * 4;

/// Where recordings go unless a folder is chosen in the settings: the
/// player's own Videos folder (Movies on macOS), where they look for videos;
/// on Android, the app's data folder.
pub fn default_folder(data_dir: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(dir) = windows_videos_folder() {
        return dir;
    }
    #[cfg(not(target_os = "android"))]
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            return home.join("Movies");
        }
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
        if let Some(dir) = xdg_videos_dir(&home) {
            return dir;
        }
        return home.join("Videos");
    }
    data_dir.join("recordings")
}

/// The Videos known folder (wherever the player moved it, e.g. OneDrive).
#[cfg(windows)]
fn windows_videos_folder() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Videos, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // Safety: the returned string is ours to free with CoTaskMemFree.
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None).ok()?;
        let path = PathBuf::from(std::ffi::OsString::from_wide(p.as_wide()));
        CoTaskMemFree(Some(p.0 as *const _));
        (!path.as_os_str().is_empty()).then_some(path)
    }
}

/// Linux: the XDG_VIDEOS_DIR from `~/.config/user-dirs.dirs` (localised
/// desktops name it differently).
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
fn xdg_videos_dir(home: &Path) -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    let line = text.lines().find_map(|l| l.trim().strip_prefix("XDG_VIDEOS_DIR="))?;
    let value = line.trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    (path.is_absolute() && path != home).then_some(path)
}

pub struct Recorder {
    video: BufWriter<File>,
    audio: BufWriter<File>,
    /// Number of video frames written so far.
    pub frames: u64,
    /// Number of audio samples written so far.
    pub audio_samples: u64,
    /// Audio sample rate (`AUDIO_RATE`).
    pub audio_rate: u32,
    video_path: PathBuf,
    audio_path: PathBuf,
    /// Where the finished recording will be written (the extension can
    /// change if FFmpeg isn't there: see `EncodeTask::run`).
    pub output_path: PathBuf,
    pub started: Instant,
}

impl Recorder {
    /// Start a new recording into `dir`. The raw frames and sound go to
    /// `temp_dir` meanwhile (not the player's Videos folder).
    pub fn start(dir: &Path, temp_dir: &Path, encoder: RecordEncoder) -> std::io::Result<Self> {
        let audio_rate = AUDIO_RATE;
        std::fs::create_dir_all(dir)?;
        std::fs::create_dir_all(temp_dir)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let video_path = temp_dir.join(format!("ribbit-rec-{stamp}.video.raw"));
        let audio_path = temp_dir.join(format!("ribbit-rec-{stamp}.audio.raw"));
        let output_path = dir.join(format!("ribbit-rec-{stamp}.{}", encoder.extension()));
        Ok(Self {
            video: BufWriter::new(File::create(&video_path)?),
            audio: BufWriter::new(File::create(&audio_path)?),
            frames: 0,
            audio_samples: 0,
            audio_rate,
            video_path,
            audio_path,
            output_path,
            started: Instant::now(),
        })
    }

    /// Write one 160×160 BGRA frame (`0x00RRGGBB` u32 pixels, native-endian
    /// bytes = BGRA on x86-64).
    pub fn add_frame(&mut self, pixels: &[u32]) -> std::io::Result<()> {
        // Safety: casting &[u32] as &[u8] is valid on any platform.
        let bytes = unsafe {
            std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4)
        };
        self.video.write_all(bytes)?;
        self.frames += 1;
        Ok(())
    }

    /// Video frames still to write for the sound recorded so far (the
    /// picture follows the sound: frames are repeated or skipped as the
    /// window redraws faster or slower than `FPS`).
    pub fn frames_due(&self) -> u64 {
        (self.audio_samples * FPS as u64 / self.audio_rate as u64).saturating_sub(self.frames)
    }

    /// Write raw PCM samples (i16, little-endian, mono).
    pub fn add_audio(&mut self, samples: &[i16]) -> std::io::Result<()> {
        for &s in samples {
            self.audio.write_all(&s.to_le_bytes())?;
        }
        self.audio_samples += samples.len() as u64;
        Ok(())
    }

    /// Stop recording. Returns an `EncodeTask` to `run` in a background thread.
    pub fn finish(mut self, encoder: RecordEncoder) -> std::io::Result<EncodeTask> {
        self.video.flush()?;
        self.audio.flush()?;
        Ok(EncodeTask {
            video_path: self.video_path,
            audio_path: self.audio_path,
            output_path: self.output_path,
            audio_rate: self.audio_rate,
            encoder,
        })
    }
}

/// Encodes the raw temp files to a finished video. Run in a background
/// thread (`spawn`) so the UI stays responsive during encoding.
pub struct EncodeTask {
    pub video_path: PathBuf,
    pub audio_path: PathBuf,
    /// Final output file, if FFmpeg makes it (`Saved::path` is the real one).
    pub output_path: PathBuf,
    pub audio_rate: u32,
    pub encoder: RecordEncoder,
}

/// A finished recording.
#[derive(Debug)]
pub struct Saved {
    /// The video file (the first part, if it was split).
    pub path: PathBuf,
    /// How many files (an uncompressed .avi is split at `avi::MAX_PART_BYTES`).
    pub parts: usize,
    /// Why it isn't in the chosen format, if it isn't.
    pub note: Option<&'static str>,
}

impl EncodeTask {
    /// Encode (FFmpeg, else Media Foundation, else .avi), then remove the
    /// temp files. They are kept if nothing could be written.
    pub fn run(self) -> anyhow::Result<Saved> {
        let mut failed = Vec::new();
        let ffmpeg_missing = match self.ffmpeg() {
            Ok(()) => return self.done(self.output_path.clone(), 1, None),
            Err(e) => {
                let _ = std::fs::remove_file(&self.output_path);
                let missing = e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound);
                if !missing {
                    tracing::warn!(target: "ribbit::record", "FFmpeg failed: {e:#}");
                    failed.push(format!("FFmpeg: {e:#}"));
                }
                missing
            }
        };

        #[cfg(windows)]
        {
            let out = self.output_path.with_extension("mp4");
            match self.raw().and_then(|mut raw| mf::encode(&mut raw, &out)) {
                Ok(()) => {
                    let note = (self.encoder != RecordEncoder::H264).then_some(if ffmpeg_missing {
                        "FFmpeg isn't installed, so it was saved as H.264 (.mp4) by Windows."
                    } else {
                        "FFmpeg failed, so it was saved as H.264 (.mp4) by Windows."
                    });
                    return self.done(out, 1, note);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&out);
                    tracing::warn!(target: "ribbit::record", "Media Foundation failed: {e:#}");
                    failed.push(format!("Windows' encoder: {e:#}"));
                }
            }
        }
        let _ = ffmpeg_missing;

        let out = self.output_path.with_extension("avi");
        match self.raw().and_then(|mut raw| Ok(avi::write(&mut raw, &out, avi::MAX_PART_BYTES)?)) {
            Ok(parts) => {
                let note = Some("It was saved uncompressed (.avi): install FFmpeg for smaller files.");
                self.done(out, parts.len(), note)
            }
            Err(e) => {
                for p in avi::part_paths(&out).take(64) {
                    let _ = std::fs::remove_file(p);
                }
                failed.push(format!("AVI: {e:#}"));
                anyhow::bail!(
                    "Ribbit couldn't write the video to {}.\n{}\nThe raw recording was kept in {}.",
                    folder_of(&out).display(),
                    failed.join("\n"),
                    folder_of(&self.video_path).display(),
                )
            }
        }
    }

    fn done(&self, path: PathBuf, parts: usize, note: Option<&'static str>) -> anyhow::Result<Saved> {
        let _ = std::fs::remove_file(&self.video_path);
        let _ = std::fs::remove_file(&self.audio_path);
        Ok(Saved { path, parts, note })
    }

    fn raw(&self) -> anyhow::Result<Raw> {
        let raw = Raw::open(&self.video_path, &self.audio_path, self.audio_rate)?;
        anyhow::ensure!(raw.frames > 0, "the recording is empty");
        Ok(raw)
    }

    fn ffmpeg(&self) -> anyhow::Result<()> {
        let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
        let (vcodec, acodec, vextra, aextra): (&str, &str, &[&str], &[&str]) = match self.encoder {
            RecordEncoder::H264 => ("libx264", "aac", &["-crf", "18", "-preset", "fast"], &["-b:a", "128k"]),
            RecordEncoder::H265 => ("libx265", "aac", &["-crf", "24", "-preset", "fast"], &["-b:a", "128k"]),
            RecordEncoder::Vp9 => ("libvpx-vp9", "libopus", &["-crf", "30", "-b:v", "0"], &["-b:a", "128k"]),
            RecordEncoder::Lossless => ("ffv1", "pcm_s16le", &[], &[]),
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.args([
                "-y",
                "-f", "rawvideo",
                "-r", &FPS.to_string(),
                "-pix_fmt", "bgra",
                "-s", "160x160",
                "-i",
            ])
            .arg(&self.video_path)
            .args(["-f", "s16le", "-ar", &self.audio_rate.to_string(), "-ac", "1", "-i"])
            .arg(&self.audio_path)
            .args(["-c:v", vcodec])
            .args(vextra)
            .args(["-c:a", acodec])
            .args(aextra)
            .args(["-vf", "scale=320:320:flags=neighbor"])
            .arg(&self.output_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // No console window flashing up over the game.
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
        let status = cmd.status()?;
        anyhow::ensure!(status.success(), "FFmpeg exited with {status}");
        Ok(())
    }
}

/// The folder a file is in (for messages).
pub fn folder_of(path: &Path) -> &Path {
    path.parent().unwrap_or(path)
}

impl RecordEncoder {
    pub fn label(self) -> &'static str {
        match self {
            RecordEncoder::H264 => "H.264 (.mp4)",
            RecordEncoder::H265 => "H.265 (.mp4)",
            RecordEncoder::Vp9 => "VP9 (.webm)",
            RecordEncoder::Lossless => "Lossless (.mkv)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            RecordEncoder::H264 | RecordEncoder::H265 => "mp4",
            RecordEncoder::Vp9 => "webm",
            RecordEncoder::Lossless => "mkv",
        }
    }
}

/// Check whether ffmpeg is available in PATH (used for the recording button).
#[allow(dead_code)]
pub fn ffmpeg_available() -> bool {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    std::process::Command::new(exe).arg("-version").output().is_ok()
}

/// The temp files read back, frame by frame with each frame's sound.
struct Raw {
    video: BufReader<File>,
    audio: BufReader<File>,
    frames: u64,
    audio_samples: u64,
    rate: u32,
    /// Next frame to read.
    at: u64,
    bytes: Vec<u8>,
}

impl Raw {
    fn open(video: &Path, audio: &Path, rate: u32) -> std::io::Result<Self> {
        let (v, a) = (File::open(video)?, File::open(audio)?);
        let frames = v.metadata()?.len() / FRAME_BYTES as u64;
        let audio_samples = a.metadata()?.len() / 2;
        Ok(Self { video: BufReader::new(v), audio: BufReader::new(a), frames, audio_samples, rate, at: 0, bytes: vec![0; FRAME_BYTES] })
    }

    /// The next frame's pixels (`0x00RRGGBB`) and the sound that plays
    /// during it (the last frame takes whatever sound is left).
    fn next(&mut self, px: &mut [u32], sound: &mut Vec<i16>) -> std::io::Result<()> {
        self.video.read_exact(&mut self.bytes)?;
        for (p, b) in px.iter_mut().zip(self.bytes.chunks_exact(4)) {
            *p = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]);
        }
        let start = self.at * self.rate as u64 / FPS as u64;
        let end = if self.at + 1 == self.frames {
            self.audio_samples
        } else {
            ((self.at + 1) * self.rate as u64 / FPS as u64).min(self.audio_samples)
        };
        self.at += 1;
        sound.clear();
        let mut b = [0u8; 2];
        for _ in start.min(end)..end {
            self.audio.read_exact(&mut b)?;
            sound.push(i16::from_le_bytes(b));
        }
        Ok(())
    }
}

/// Linear-interpolating resampler between whole rates (32 kHz → 48 kHz for
/// the AAC encoder, which only takes 44.1 or 48 kHz), fed in pieces. Only
/// the Windows encoder uses it.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Resampler {
    from: u64,
    to: u64,
    /// Output samples made so far.
    made: u64,
    /// Input index of `buf[0]`.
    base: u64,
    buf: Vec<i16>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self { from: from as u64, to: to as u64, made: 0, base: 0, buf: Vec::new() }
    }

    /// Adds input; appends the output it completes.
    pub fn push(&mut self, input: &[i16], out: &mut Vec<i16>) {
        self.buf.extend_from_slice(input);
        loop {
            let pos = self.made * self.from;
            let i = (pos / self.to - self.base) as usize;
            if i + 1 >= self.buf.len() {
                break;
            }
            let (a, b) = (self.buf[i] as i64, self.buf[i + 1] as i64);
            out.push((a + (b - a) * (pos % self.to) as i64 / self.to as i64) as i16);
            self.made += 1;
        }
        let used = ((self.made * self.from / self.to - self.base) as usize).min(self.buf.len());
        self.buf.drain(..used);
        self.base += used as u64;
    }

    /// The end of the input: the output up to it (`ceil(n × to / from)` in all).
    pub fn finish(&mut self, out: &mut Vec<i16>) {
        let end = self.base + self.buf.len() as u64;
        while self.made * self.from / self.to < end {
            out.push(self.buf[(self.made * self.from / self.to - self.base) as usize]);
            self.made += 1;
        }
    }
}

/// The last resort: an uncompressed AVI (RIFF AVI 1.0 with an idx1 index),
/// which any player opens. 24-bit bottom-up DIB at the game's 160×160 + PCM.
/// Split into parts of at most `MAX_PART_BYTES` (~3.7 minutes each) because
/// AVI 1.0 offsets are 32-bit and many players stop at 1-2 GB.
mod avi {
    use super::{Raw, FPS, HEIGHT, WIDTH};
    use std::fs::File;
    use std::io::{BufWriter, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};

    pub const MAX_PART_BYTES: u64 = 1 << 30;
    const DIB_BYTES: usize = WIDTH * HEIGHT * 3;
    /// Bytes from the file's start to the first chunk in 'movi'.
    pub const HEADER_LEN: usize = 12 + 12 + 64 + 12 + 64 + 48 + 12 + 64 + 26 + 12;

    /// `out`, then `out-part2.avi`, `out-part3.avi`…
    pub fn part_paths(out: &Path) -> impl Iterator<Item = PathBuf> + '_ {
        (1..).map(move |n| {
            if n == 1 {
                out.to_path_buf()
            } else {
                let stem = out.file_stem().unwrap_or_default().to_string_lossy();
                out.with_file_name(format!("{stem}-part{n}.avi"))
            }
        })
    }

    /// Writes the whole recording; returns the files written.
    pub fn write(raw: &mut Raw, out: &Path, max_bytes: u64) -> std::io::Result<Vec<PathBuf>> {
        let mut paths = part_paths(out);
        let mut written = Vec::new();
        let (mut px, mut sound) = (vec![0u32; WIDTH * HEIGHT], Vec::new());
        let mut dib = vec![0u8; DIB_BYTES];
        let mut part: Option<Part> = None;
        for _ in 0..raw.frames {
            raw.next(&mut px, &mut sound)?;
            let need = (8 + DIB_BYTES + 8 + sound.len() * 2 + 32) as u64;
            if part.as_ref().is_some_and(|p| p.len() + need > max_bytes) {
                part.take().unwrap().finish(raw.rate)?;
            }
            if part.is_none() {
                let path = paths.next().unwrap();
                written.push(path.clone());
                part = Some(Part::create(&path, raw.rate)?);
            }
            let p = part.as_mut().unwrap();
            for (y, row) in dib.chunks_exact_mut(WIDTH * 3).enumerate() {
                // Bottom-up: the DIB's first row is the picture's last.
                let src = &px[(HEIGHT - 1 - y) * WIDTH..][..WIDTH];
                for (d, &s) in row.chunks_exact_mut(3).zip(src) {
                    d.copy_from_slice(&[s as u8, (s >> 8) as u8, (s >> 16) as u8]);
                }
            }
            p.chunk(*b"00db", &dib)?;
            if !sound.is_empty() {
                let bytes: Vec<u8> = sound.iter().flat_map(|s| s.to_le_bytes()).collect();
                p.chunk(*b"01wb", &bytes)?;
                p.samples += sound.len() as u64;
                p.max_audio = p.max_audio.max(bytes.len() as u32);
            }
            p.frames += 1;
        }
        if let Some(p) = part {
            p.finish(raw.rate)?;
        }
        Ok(written)
    }

    struct Part {
        file: BufWriter<File>,
        /// Bytes written to the 'movi' list after its fourcc.
        movi: u64,
        index: Vec<u8>,
        frames: u32,
        samples: u64,
        max_audio: u32,
    }

    impl Part {
        fn create(path: &Path, rate: u32) -> std::io::Result<Self> {
            let mut file = BufWriter::new(File::create(path)?);
            file.write_all(&header(0, 0, 0, 0, rate))?;
            Ok(Self { file, movi: 0, index: Vec::new(), frames: 0, samples: 0, max_audio: 0 })
        }

        fn len(&self) -> u64 {
            (HEADER_LEN as u64) + self.movi + 8 + self.index.len() as u64
        }

        fn chunk(&mut self, id: [u8; 4], data: &[u8]) -> std::io::Result<()> {
            // idx1 offsets count from the 'movi' fourcc.
            let offset = 4 + self.movi as u32;
            self.index.extend_from_slice(&id);
            self.index.extend_from_slice(&0x10u32.to_le_bytes()); // AVIIF_KEYFRAME
            self.index.extend_from_slice(&offset.to_le_bytes());
            self.index.extend_from_slice(&(data.len() as u32).to_le_bytes());
            self.file.write_all(&id)?;
            self.file.write_all(&(data.len() as u32).to_le_bytes())?;
            self.file.write_all(data)?;
            if data.len() % 2 == 1 {
                self.file.write_all(&[0])?;
            }
            self.movi += 8 + data.len().next_multiple_of(2) as u64;
            Ok(())
        }

        fn finish(mut self, rate: u32) -> std::io::Result<()> {
            self.file.write_all(b"idx1")?;
            self.file.write_all(&(self.index.len() as u32).to_le_bytes())?;
            self.file.write_all(&self.index)?;
            let riff = self.len() - 8;
            let h = header(riff as u32, 4 + self.movi as u32, self.frames, self.samples as u32, rate);
            let h = {
                let mut h = h;
                // The audio stream's suggested buffer size (its biggest chunk).
                let at = AUDIO_STRH_BUF;
                h[at..at + 4].copy_from_slice(&self.max_audio.to_le_bytes());
                h
            };
            let mut file = self.file.into_inner().map_err(|e| e.into_error())?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&h)?;
            file.sync_all()
        }
    }

    /// Offset of the audio 'strh' dwSuggestedBufferSize in `header`.
    const AUDIO_STRH_BUF: usize = 12 + 12 + 64 + 12 + 64 + 48 + 12 + 8 + 36;

    fn u32s(v: &mut Vec<u8>, xs: &[u32]) {
        for x in xs {
            v.extend_from_slice(&x.to_le_bytes());
        }
    }

    /// 'strh' (56 bytes) for one stream.
    fn strh(v: &mut Vec<u8>, kind: &[u8; 4], scale: u32, rate: u32, length: u32, buffer: u32, sample_size: u32, w: u16, h: u16) {
        v.extend_from_slice(b"strh");
        u32s(v, &[56]);
        v.extend_from_slice(kind);
        u32s(v, &[0, 0, 0, 0, scale, rate, 0, length, buffer, u32::MAX, sample_size]);
        for x in [0u16, 0, w, h] {
            v.extend_from_slice(&x.to_le_bytes());
        }
    }

    /// Everything before the first 'movi' chunk.
    pub fn header(riff: u32, movi: u32, frames: u32, samples: u32, rate: u32) -> Vec<u8> {
        let (w, h) = (WIDTH as u32, HEIGHT as u32);
        let mut v = Vec::with_capacity(HEADER_LEN);
        v.extend_from_slice(b"RIFF");
        u32s(&mut v, &[riff]);
        v.extend_from_slice(b"AVI LIST");
        u32s(&mut v, &[4 + 64 + 12 + 64 + 48 + 12 + 64 + 26]);
        v.extend_from_slice(b"hdrlavih");
        let max_bytes = DIB_BYTES as u32 * FPS + rate * 2;
        // AVIF_HASINDEX | AVIF_ISINTERLEAVED; two streams.
        u32s(&mut v, &[56, 1_000_000 / FPS, max_bytes, 0, 0x110, frames, 0, 2, DIB_BYTES as u32, w, h, 0, 0, 0, 0]);
        // Video: 60 frames a second, 24-bit BI_RGB.
        v.extend_from_slice(b"LIST");
        u32s(&mut v, &[4 + 64 + 48]);
        v.extend_from_slice(b"strl");
        strh(&mut v, b"vids", 1, FPS, frames, DIB_BYTES as u32, 0, w as u16, h as u16);
        v.extend_from_slice(b"strf");
        u32s(&mut v, &[40, 40, w, h]);
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        u32s(&mut v, &[0, DIB_BYTES as u32, 0, 0, 0, 0]);
        // Sound: mono 16-bit PCM (scale/rate in blocks: samples a second).
        v.extend_from_slice(b"LIST");
        u32s(&mut v, &[4 + 64 + 26]);
        v.extend_from_slice(b"strl");
        strh(&mut v, b"auds", 2, rate * 2, samples, 0, 2, 0, 0);
        v.extend_from_slice(b"strf");
        u32s(&mut v, &[18]);
        for x in [1u16, 1] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        u32s(&mut v, &[rate, rate * 2]);
        for x in [2u16, 16, 0] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(b"LIST");
        u32s(&mut v, &[movi]);
        v.extend_from_slice(b"movi");
        debug_assert_eq!(v.len(), HEADER_LEN);
        v
    }
}

/// Windows' own encoder (Media Foundation's sink writer): H.264 + AAC in an
/// .mp4, so recording works without installing FFmpeg.
#[cfg(windows)]
mod mf {
    use super::{Raw, Resampler, FPS, HEIGHT, WIDTH};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows::core::{GUID, PCWSTR};

    const W: usize = WIDTH * 2;
    const H: usize = HEIGHT * 2;
    /// Generous for 320×320: crisp pixel edges even in busy scenes.
    const VIDEO_BITRATE: u32 = 5_000_000;
    /// The AAC encoder takes 44.1 or 48 kHz only.
    const AAC_RATE: u32 = 48_000;
    /// 128 kbit/s (the encoder allows 12000, 16000, 20000 or 24000 bytes/s).
    const AAC_BYTES_PER_SEC: u32 = 16_000;
    const HNS: i64 = 10_000_000;

    /// COM + Media Foundation, started for this thread and stopped on drop.
    pub struct Started {
        com: bool,
    }

    impl Started {
        pub fn new() -> windows::core::Result<Self> {
            // Safety: balanced by Drop on this same thread.
            unsafe {
                let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
                if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                    if com {
                        CoUninitialize();
                    }
                    return Err(e);
                }
                Ok(Self { com })
            }
        }
    }

    impl Drop for Started {
        fn drop(&mut self) {
            unsafe {
                let _ = MFShutdown();
                if self.com {
                    CoUninitialize();
                }
            }
        }
    }

    pub fn wide(path: &std::path::Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().chain([0]).collect()
    }

    pub fn set_pair(t: &IMFMediaType, key: &GUID, hi: u32, lo: u32) -> windows::core::Result<()> {
        unsafe { t.SetUINT64(key, (u64::from(hi) << 32) | u64::from(lo)) }
    }

    fn video_type(subtype: &GUID) -> windows::core::Result<IMFMediaType> {
        unsafe {
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            set_pair(&t, &MF_MT_FRAME_SIZE, W as u32, H as u32)?;
            set_pair(&t, &MF_MT_FRAME_RATE, FPS, 1)?;
            set_pair(&t, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            // What `nv12_2x` makes (and what players assume for SD video).
            t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)?;
            t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            Ok(t)
        }
    }

    fn audio_type(subtype: &GUID, bytes_per_sec: u32) -> windows::core::Result<IMFMediaType> {
        unsafe {
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
            t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AAC_RATE)?;
            t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_sec)?;
            if *subtype == MFAudioFormat_PCM {
                t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2)?;
            }
            Ok(t)
        }
    }

    fn write(writer: &IMFSinkWriter, stream: u32, bytes: &[u8], time: i64, duration: i64) -> windows::core::Result<()> {
        unsafe {
            let buf = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut p = std::ptr::null_mut();
            buf.Lock(&mut p, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            buf.Unlock()?;
            buf.SetCurrentLength(bytes.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buf)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(duration)?;
            writer.WriteSample(stream, &sample)
        }
    }

    /// BT.601 limited-range NV12 at 2× nearest-neighbour. Each game pixel
    /// is a 2×2 block, so the 4:2:0 chroma is exact: no colour bleeding.
    pub fn nv12_2x(px: &[u32], out: &mut [u8]) {
        let (luma, chroma) = out.split_at_mut(W * H);
        for (y, row) in px.chunks_exact(WIDTH).enumerate() {
            for (x, &p) in row.iter().enumerate() {
                let (r, g, b) = (((p >> 16) & 0xff) as i32, ((p >> 8) & 0xff) as i32, (p & 0xff) as i32);
                let l = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
                let u = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
                let v = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
                for dy in 0..2 {
                    luma[(2 * y + dy) * W + 2 * x..][..2].fill(l);
                }
                chroma[y * W + 2 * x..][..2].copy_from_slice(&[u, v]);
            }
        }
    }

    pub fn encode(raw: &mut Raw, out: &std::path::Path) -> anyhow::Result<()> {
        let _mf = Started::new()?;
        let url = wide(out);
        // Safety: plain Media Foundation calls on COM objects owned here.
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1)?;
            let attrs = attrs.ok_or_else(|| anyhow::anyhow!("no attributes"))?;
            // Writing as fast as the encoder goes (not in real time).
            attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
            let writer = MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attrs)?;

            let vout = video_type(&MFVideoFormat_H264)?;
            vout.SetUINT32(&MF_MT_AVG_BITRATE, VIDEO_BITRATE)?;
            vout.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            let v = writer.AddStream(&vout)?;
            writer.SetInputMediaType(v, &video_type(&MFVideoFormat_NV12)?, None)?;

            let a = writer.AddStream(&audio_type(&MFAudioFormat_AAC, AAC_BYTES_PER_SEC)?)?;
            writer.SetInputMediaType(a, &audio_type(&MFAudioFormat_PCM, AAC_RATE * 2)?, None)?;
            writer.BeginWriting()?;

            let mut px = vec![0u32; WIDTH * HEIGHT];
            let mut nv12 = vec![0u8; W * H * 3 / 2];
            let (mut sound, mut resampled, mut bytes) = (Vec::new(), Vec::new(), Vec::new());
            let mut resampler = Resampler::new(raw.rate, AAC_RATE);
            let mut heard = 0i64; // 48 kHz samples written
            for i in 0..raw.frames as i64 {
                raw.next(&mut px, &mut sound)?;
                nv12_2x(&px, &mut nv12);
                let (t, next) = (i * HNS / FPS as i64, (i + 1) * HNS / FPS as i64);
                write(&writer, v, &nv12, t, next - t)?;

                resampled.clear();
                resampler.push(&sound, &mut resampled);
                if i + 1 == raw.frames as i64 {
                    resampler.finish(&mut resampled);
                }
                if !resampled.is_empty() {
                    bytes.clear();
                    bytes.extend(resampled.iter().flat_map(|s| s.to_le_bytes()));
                    let n = resampled.len() as i64;
                    let t = heard * HNS / AAC_RATE as i64;
                    write(&writer, a, &bytes, t, (heard + n) * HNS / AAC_RATE as i64 - t)?;
                    heard += n;
                }
            }
            writer.Finalize()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ribbit-rec-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// `frames` frames of a solid colour (red, changing to blue halfway) and
    /// a 1 kHz-ish square wave, as the recorder writes them.
    fn record(dir: &Path, frames: u64) -> EncodeTask {
        let mut rec = Recorder::start(dir, &dir.join("temp"), RecordEncoder::H264).unwrap();
        let per_frame = AUDIO_RATE as usize / FPS as usize + 1;
        let tone: Vec<i16> = (0..per_frame).map(|i| if i / 16 % 2 == 0 { 8000 } else { -8000 }).collect();
        while rec.frames < frames {
            rec.add_audio(&tone).unwrap();
            for _ in 0..rec.frames_due().min(frames - rec.frames) {
                let colour = if rec.frames < frames / 2 { 0x00c8_2828 } else { 0x0028_28c8 };
                rec.add_frame(&vec![colour; WIDTH * HEIGHT]).unwrap();
            }
        }
        rec.finish(RecordEncoder::H264).unwrap()
    }

    #[test]
    fn frames_follow_the_sound_not_the_redraws() {
        let dir = temp("follow");
        let mut rec = Recorder::start(&dir, &dir, RecordEncoder::Lossless).unwrap();
        let frame = vec![0u32; 160 * 160];
        // A 144 Hz window: 144 redraws in one second of sound, 60 frames.
        for _ in 0..144 {
            rec.add_audio(&vec![0; AUDIO_RATE as usize / 144]).unwrap();
            for _ in 0..rec.frames_due() {
                rec.add_frame(&frame).unwrap();
            }
        }
        assert!((59..=60).contains(&rec.frames), "{} frames", rec.frames);
        // A slow redraw (a quarter second of sound at once): frames repeated.
        rec.add_audio(&vec![0; AUDIO_RATE as usize / 4]).unwrap();
        assert_eq!(rec.frames_due(), (rec.audio_samples * FPS as u64 / AUDIO_RATE as u64) - rec.frames);
        drop(rec);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_resampler_makes_the_right_length_in_any_pieces() {
        let input: Vec<i16> = (0..32_000).map(|i| (i % 2000) as i16).collect();
        let mut whole = Vec::new();
        let mut r = Resampler::new(32_000, 48_000);
        r.push(&input, &mut whole);
        r.finish(&mut whole);
        assert_eq!(whole.len(), 48_000);
        let mut pieces = Vec::new();
        let mut r = Resampler::new(32_000, 48_000);
        for c in input.chunks(533) {
            r.push(c, &mut pieces);
        }
        r.finish(&mut pieces);
        assert_eq!(pieces, whole);
        // Interpolated between neighbours: 0, 2/3, 4/3, 2…
        assert_eq!(&whole[..4], &[0, 0, 1, 2]);
        let mut odd = Vec::new();
        let mut r = Resampler::new(32_000, 48_000);
        r.push(&[5; 7], &mut odd);
        r.finish(&mut odd);
        assert_eq!(odd, vec![5; 11]); // ceil(7 × 3 / 2)
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn record_avi_is_well_formed_and_splits() {
        let dir = temp("avi");
        let task = record(&dir, 30);
        let mut raw = Raw::open(&task.video_path, &task.audio_path, task.audio_rate).unwrap();
        let out = dir.join("clip.avi");
        let parts = avi::write(&mut raw, &out, avi::MAX_PART_BYTES).unwrap();
        assert_eq!(parts, vec![out.clone()]);
        let b = std::fs::read(&out).unwrap();
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[8..12], b"AVI ");
        assert_eq!(u32_at(&b, 4) as usize, b.len() - 8);
        assert_eq!(&b[avi::HEADER_LEN - 4..avi::HEADER_LEN], b"movi");
        let movi = u32_at(&b, avi::HEADER_LEN - 8) as usize;
        let idx = avi::HEADER_LEN - 4 + movi;
        assert_eq!(&b[idx..idx + 4], b"idx1");
        assert_eq!(idx + 8 + u32_at(&b, idx + 4) as usize, b.len());
        assert_eq!(u32_at(&b, 48), 30, "avih total frames");
        // The first index entry points at the first frame chunk.
        let first = avi::HEADER_LEN - 4 + u32_at(&b, idx + 16) as usize;
        assert_eq!(&b[first..first + 4], b"00db");
        // Bottom-up BGR: red picture.
        assert_eq!(&b[first + 8..first + 11], &[0x28, 0x28, 0xc8]);
        // Every sample of sound is in it.
        let samples = std::fs::metadata(&task.audio_path).unwrap().len() / 2;
        let audio_len = u32_at(&b, 12 + 12 + 64 + 12 + 64 + 48 + 12 + 8 + 32);
        assert_eq!(u64::from(audio_len), samples);

        // A small part limit: split into several well-formed files.
        let mut raw = Raw::open(&task.video_path, &task.audio_path, task.audio_rate).unwrap();
        let parts = avi::write(&mut raw, &dir.join("split.avi"), 10 * 80_000).unwrap();
        assert_eq!(parts.len(), 3, "{parts:?}");
        let frames: u32 = parts.iter().map(|p| u32_at(&std::fs::read(p).unwrap(), 48)).sum();
        assert_eq!(frames, 30);
        assert!(parts[1].ends_with("split-part2.avi"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_keeps_the_temp_files_until_a_video_exists() {
        let dir = temp("keep");
        let mut task = record(&dir, 10);
        // A folder that can't be written: nothing is made, nothing lost.
        let blocker = dir.join("not-a-folder");
        std::fs::write(&blocker, b"").unwrap();
        task.output_path = blocker.join("x.mp4");
        let (video, audio) = (task.video_path.clone(), task.audio_path.clone());
        let err = task.run().unwrap_err();
        assert!(format!("{err:#}").contains("raw recording was kept"), "{err:#}");
        assert!(video.exists() && audio.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_default_folder_is_a_videos_folder() {
        let data = Path::new("data");
        let dir = default_folder(data);
        if cfg!(target_os = "android") {
            assert_eq!(dir, data.join("recordings"));
        } else {
            assert!(dir.is_absolute(), "{}", dir.display());
        }
    }

    /// Encodes with Windows' own encoder and reads the .mp4 back with
    /// Windows' own decoder.
    #[cfg(windows)]
    #[test]
    fn record_media_foundation_mp4_plays_back() {
        use windows::Win32::Media::MediaFoundation::*;
        let dir = temp("mf");
        let frames = 120;
        let task = record(&dir, frames);
        let out = dir.join("clip.mp4");
        let mut raw = Raw::open(&task.video_path, &task.audio_path, task.audio_rate).unwrap();
        mf::encode(&mut raw, &out).unwrap();

        let b = std::fs::read(&out).unwrap();
        assert!(b.len() > 10_000, "{} bytes", b.len());
        assert_eq!(&b[4..8], b"ftyp");
        assert!(b.windows(4).any(|w| w == b"moov"));
        assert!(b.windows(4).any(|w| w == b"mp4a"), "AAC track");
        assert!(b.windows(4).any(|w| w == b"avc1"), "H.264 track");

        let _mf = mf::Started::new().unwrap();
        let url = mf::wide(&out);
        unsafe {
            let reader = MFCreateSourceReaderFromURL(windows::core::PCWSTR(url.as_ptr()), None).unwrap();
            let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let t = MFCreateMediaType().unwrap();
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
            reader.SetCurrentMediaType(video, None, &t).unwrap();
            let size = reader.GetCurrentMediaType(video).unwrap().GetUINT64(&MF_MT_FRAME_SIZE).unwrap();
            assert_eq!((size >> 32, size & 0xffff_ffff), (320, 320));
            let (mut decoded, mut lumas) = (0, Vec::new());
            loop {
                let (mut flags, mut sample) = (0u32, None);
                reader.ReadSample(video, 0, None, Some(&raw mut flags), None, Some(&raw mut sample)).unwrap();
                if let Some(s) = sample {
                    let buf = s.ConvertToContiguousBuffer().unwrap();
                    let mut p = std::ptr::null_mut();
                    buf.Lock(&mut p, None, None).unwrap();
                    lumas.push(*p.add(160 * 320 + 160));
                    buf.Unlock().unwrap();
                    decoded += 1;
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
            }
            assert_eq!(decoded, frames);
            // Red (Y 92) then blue (Y 66), as written.
            assert!(lumas[..50].iter().all(|&y| y.abs_diff(92) <= 4), "{lumas:?}");
            assert!(lumas[70..].iter().all(|&y| y.abs_diff(66) <= 4), "{lumas:?}");

            let audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
            let t = MFCreateMediaType().unwrap();
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
            reader.SetCurrentMediaType(audio, None, &t).unwrap();
            let mut bytes = 0;
            loop {
                let (mut flags, mut sample) = (0u32, None);
                reader.ReadSample(audio, 0, None, Some(&raw mut flags), None, Some(&raw mut sample)).unwrap();
                if let Some(s) = sample {
                    bytes += s.GetTotalLength().unwrap();
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
            }
            let rate = reader.GetCurrentMediaType(audio).unwrap().GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap();
            let ch = reader.GetCurrentMediaType(audio).unwrap().GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap();
            let secs = bytes as f64 / (rate * ch * 2) as f64;
            assert!((secs - 2.0).abs() < 0.1, "{secs} s of sound");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole chain without FFmpeg leaves one video and no temp files.
    #[test]
    fn record_run_saves_a_video_and_cleans_up() {
        let dir = temp("run");
        let task = record(&dir, 20);
        let (video, audio) = (task.video_path.clone(), task.audio_path.clone());
        let saved = task.run().unwrap();
        assert!(saved.path.exists() && std::fs::metadata(&saved.path).unwrap().len() > 0);
        assert!(!video.exists() && !audio.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
