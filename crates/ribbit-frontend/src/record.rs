//! Screen recording: raw video frames + audio samples written to temp files,
//! then encoded by FFmpeg after the recording stops (so the capture is fast
//! and the output has no real-time stutters regardless of PC load).
//!
//! The game is 160×160 pixels; frames are written at `FPS` by the sound's
//! clock (`frames_due`), whatever rate the window redraws at, with the
//! speaker's 32 kHz mono sound (`AUDIO_RATE`).
//! FFmpeg scales the picture 2× to 320×320 (nearest-neighbour) so the output
//! is watchable without further scaling.

use crate::settings::RecordEncoder;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The recording's frame rate. Frames are written to match the sound
/// recorded so far (`frames_due`), not one per window redraw: on a 144 Hz
/// screen that was 144 frames a second played as 60, the sound running
/// ahead of the picture.
pub const FPS: u32 = 60;

/// The recording's sound: the speaker's 32 kHz (8 kHz sound raised to it).
pub const AUDIO_RATE: u32 = ribbit_audio::DAC_RATE_HZ;

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
    /// Where the finished recording will be written.
    pub output_path: PathBuf,
    pub started: Instant,
}

impl Recorder {
    /// Start a new recording. Returns the recorder and the output path
    /// (written after `finish`).
    pub fn start(dir: &Path, encoder: RecordEncoder) -> std::io::Result<Self> {
        let audio_rate = AUDIO_RATE;
        std::fs::create_dir_all(dir)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let video_path = dir.join(format!("ribbit-rec-{stamp}.video.raw"));
        let audio_path = dir.join(format!("ribbit-rec-{stamp}.audio.raw"));
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

    /// Stop recording and kick off FFmpeg encoding in a background thread.
    /// Returns an `EncodeTask` you can poll or wait on.
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
    /// Final output file (written by `run`).
    pub output_path: PathBuf,
    pub audio_rate: u32,
    pub encoder: RecordEncoder,
}

impl EncodeTask {
    /// Encode, clean up temp files and return Ok if FFmpeg succeeds.
    pub fn run(self) -> anyhow::Result<()> {
        let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
        let (vcodec, acodec, vextra, aextra): (&str, &str, &[&str], &[&str]) = match self.encoder {
            RecordEncoder::H264 => ("libx264", "aac", &["-crf", "18", "-preset", "fast"], &["-b:a", "128k"]),
            RecordEncoder::H265 => ("libx265", "aac", &["-crf", "24", "-preset", "fast"], &["-b:a", "128k"]),
            RecordEncoder::Vp9 => ("libvpx-vp9", "libopus", &["-crf", "30", "-b:v", "0"], &["-b:a", "128k"]),
            RecordEncoder::Lossless => ("ffv1", "pcm_s16le", &[], &[]),
        };
        let status = std::process::Command::new(exe)
            .args([
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
            .status();

        // Always remove temp files even if encoding failed.
        let _ = std::fs::remove_file(&self.video_path);
        let _ = std::fs::remove_file(&self.audio_path);

        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => anyhow::bail!("FFmpeg exited with {s}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!("FFmpeg not found — install it and make sure it is on your PATH")
            }
            Err(e) => anyhow::bail!("Could not run FFmpeg: {e}"),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_follow_the_sound_not_the_redraws() {
        let dir = std::env::temp_dir().join(format!("ribbit-rec-test-{}", std::process::id()));
        let mut rec = Recorder::start(&dir, RecordEncoder::Lossless).unwrap();
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
}
