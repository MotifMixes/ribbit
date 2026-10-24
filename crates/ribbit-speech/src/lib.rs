//! Speech recognition for Ribbit: OpenAI's Whisper (English-only models) on
//! the CPU through candle, for transcribing what a LeapPad book's spots say
//! (the cartridges hold no text). Greedy decoding with an optional prompt,
//! as `whisper.transcribe(temperature=0, initial_prompt=...)` does.

use anyhow::{Context, Result, anyhow};
use candle_core::{D, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{self as m, Config, audio, model::Whisper};
use std::path::Path;
use tokenizers::Tokenizer;

/// The sample rate Whisper listens at.
pub const SAMPLE_HZ: usize = m::SAMPLE_RATE;

/// The model files a recognizer needs, in one folder.
pub const MODEL_FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

/// A loaded Whisper model.
pub struct Recognizer {
    model: Whisper,
    config: Config,
    tokenizer: Tokenizer,
    filters: Vec<f32>,
    device: Device,
    sot: u32,
    eot: u32,
    no_timestamps: u32,
    start_of_prev: u32,
    /// Tokens never chosen: the config's and every timestamp.
    suppress: Tensor,
}

impl Recognizer {
    /// Loads the model in `dir` (`MODEL_FILES`; an English-only model such as
    /// openai/whisper-base.en).
    pub fn load(dir: &Path) -> Result<Self> {
        let device = Device::Cpu;
        let config: Config = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).context("config.json")?)?;
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| anyhow!("tokenizer.json: {e}"))?;
        // SAFETY: the weights file is only read; mapping it keeps loading fast.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], m::DTYPE, &device)? };
        let model = Whisper::load(&vb, config.clone())?;
        let id = |t: &str| tokenizer.token_to_id(t).ok_or_else(|| anyhow!("the tokenizer has no {t}"));
        let (sot, eot, no_timestamps, start_of_prev) =
            (id(m::SOT_TOKEN)?, id(m::EOT_TOKEN)?, id(m::NO_TIMESTAMPS_TOKEN)?, id("<|startofprev|>")?);
        let mask: Vec<f32> = (0..config.vocab_size as u32)
            .map(|t| if config.suppress_tokens.contains(&t) || t > no_timestamps { f32::NEG_INFINITY } else { 0.0 })
            .collect();
        let suppress = Tensor::new(mask.as_slice(), &device)?;
        let filters = mel_filters(config.num_mel_bins, m::N_FFT, SAMPLE_HZ);
        Ok(Recognizer { model, config, tokenizer, filters, device, sot, eot, no_timestamps, start_of_prev, suppress })
    }

    /// The words in `pcm` (mono, `SAMPLE_HZ`, -1..1, up to 30 s). `prompt`:
    /// text that came before (it steers spelling: "shore", not "sure").
    pub fn transcribe(&mut self, pcm: &[f32], prompt: Option<&str>) -> Result<String> {
        // A 30 s window, as Whisper was trained: with only the clip's own
        // frames it never ends a short word and runs to its token limit
        // (11 s a word instead of 0.6 s).
        let mut padded = pcm[..pcm.len().min(m::N_SAMPLES)].to_vec();
        padded.resize(m::N_SAMPLES, 0.0);
        let mel = audio::pcm_to_mel(&self.config, &padded, &self.filters);
        let frames = mel.len() / self.config.num_mel_bins;
        let mel = Tensor::from_vec(mel, (1, self.config.num_mel_bins, frames), &self.device)?;
        let mel = mel.narrow(2, 0, frames.min(m::N_FRAMES))?;
        let features = self.model.encoder.forward(&mel, true)?;

        let mut tokens = Vec::new();
        if let Some(p) = prompt.filter(|p| !p.trim().is_empty()) {
            let ids = self.tokenizer.encode(format!(" {}", p.trim()), false).map_err(|e| anyhow!("{e}"))?.get_ids().to_vec();
            let keep = self.config.max_target_positions / 2 - 1;
            tokens.push(self.start_of_prev);
            tokens.extend_from_slice(&ids[ids.len().saturating_sub(keep)..]);
        }
        tokens.push(self.sot);
        tokens.push(self.no_timestamps);
        let first = tokens.len();
        // Speech is at most a few words a second: a clip's text is cut at
        // ten tokens and eight a second, and when the last six tokens repeat
        // the six before them (a loop Whisper can fall into).
        let secs = pcm.len() as f32 / SAMPLE_HZ as f32;
        let limit = (10 + (secs * 8.0) as usize).min(self.config.max_target_positions / 2);
        for i in 0..limit {
            let input = Tensor::new(tokens.as_slice(), &self.device)?.unsqueeze(0)?;
            let ys = self.model.decoder.forward(&input, &features, i == 0)?;
            let (_, len, _) = ys.dims3()?;
            let logits = self.model.decoder.final_linear(&ys.i((..1, len - 1..))?)?.i(0)?.i(0)?;
            let logits = logits.broadcast_add(&self.suppress)?;
            let next = logits.argmax(D::Minus1)?.to_scalar::<u32>()?;
            if next == self.eot {
                break;
            }
            tokens.push(next);
            let t = &tokens[first..];
            if t.len() >= 12 && t[t.len() - 6..] == t[t.len() - 12..t.len() - 6] {
                tokens.truncate(tokens.len() - 6);
                break;
            }
        }
        let text = self.tokenizer.decode(&tokens[first..], true).map_err(|e| anyhow!("{e}"))?;
        Ok(text.trim().to_string())
    }

    /// Whether `t` is a special token (for callers that inspect tokens).
    pub fn is_special(&self, t: u32) -> bool {
        t >= self.eot || t == self.no_timestamps
    }
}

/// A clip as the LeapPad plays it (8 kHz, 16-bit) ready for `transcribe`:
/// 16 kHz, levelled to 0.8 of full scale, with a quarter second of quiet
/// before and half a second after (as the transcriber was tuned with).
pub fn prepare_8k(audio: &[i16]) -> Vec<f32> {
    let s: Vec<f32> = audio.iter().map(|&v| v as f32 / 32768.0).collect();
    let mut out = vec![0.0; SAMPLE_HZ / 4];
    for i in 0..s.len() * 2 {
        let (k, f) = (i / 2, (i % 2) as f32 * 0.5);
        out.push(s[k] * (1.0 - f) + s.get(k + 1).copied().unwrap_or(0.0) * f);
    }
    let peak = out.iter().fold(1e-4f32, |m, v| m.max(v.abs()));
    out.iter_mut().for_each(|v| *v = *v / peak * 0.8);
    out.extend(std::iter::repeat_n(0.0, SAMPLE_HZ / 2));
    out
}

/// Mel filters as Whisper's `mel_filters` (librosa's Slaney mel scale and
/// area normalisation): `n_mels` rows of `n_fft / 2 + 1` weights.
pub fn mel_filters(n_mels: usize, n_fft: usize, sample_hz: usize) -> Vec<f32> {
    let bins = n_fft / 2 + 1;
    let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0f64);
    let min_log_mel = min_log_hz / f_sp;
    let log_step = 6.4f64.ln() / 27.0;
    let hz_to_mel = |f: f64| if f < min_log_hz { f / f_sp } else { min_log_mel + (f / min_log_hz).ln() / log_step };
    let mel_to_hz = |mel: f64| if mel < min_log_mel { mel * f_sp } else { min_log_hz * ((mel - min_log_mel) * log_step).exp() };
    let top = hz_to_mel(sample_hz as f64 / 2.0);
    let points: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz(top * i as f64 / (n_mels + 1) as f64)).collect();
    let mut w = vec![0.0f32; n_mels * bins];
    for m in 0..n_mels {
        let norm = 2.0 / (points[m + 2] - points[m]);
        for k in 0..bins {
            let f = k as f64 * sample_hz as f64 / n_fft as f64;
            let lower = (f - points[m]) / (points[m + 1] - points[m]);
            let upper = (points[m + 2] - f) / (points[m + 2] - points[m + 1]);
            w[m * bins + k] = (lower.min(upper).max(0.0) * norm) as f32;
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// librosa.filters.mel(sr=16000, n_fft=400, n_mels=80): the first filter
    /// peaks at bin 1 with 0.02486 (the value Whisper's mel_filters.npz holds).
    #[test]
    fn the_mel_filters_match_librosas() {
        let w = mel_filters(80, 400, 16000);
        assert_eq!(w.len(), 80 * 201);
        assert!((w[1] - 0.02486).abs() < 2e-4, "{}", w[1]);
        assert_eq!(w[0], 0.0);
    }
}
