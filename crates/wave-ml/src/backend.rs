//! `Backend`: the `Models` the WaveCraft apps hand to the engine.

use crate::{demucs, store, stretch, whisper};
use soundcraft_wave_analysis::lyrics::Word;
use soundcraft_wave_analysis::models::{ModelInfo, ModelKind, Models, Stem, model};
use std::path::PathBuf;

pub struct Backend {
    dir: PathBuf,
    coreml: bool,
}

impl Backend {
    /// Models in [`store::models_dir`]; CoreML for separation when `WAVECRAFT_COREML=1`.
    pub fn new() -> Backend {
        Backend { dir: store::models_dir(), coreml: std::env::var("WAVECRAFT_COREML").is_ok_and(|v| v == "1") }
    }

    /// Models in `dir` (tests, other folders).
    pub fn with_dir(dir: PathBuf) -> Backend {
        Backend { dir, coreml: false }
    }

    fn require(&self, id: &str, kind: fn(&ModelKind) -> bool, what: &str) -> Result<(&'static ModelInfo, PathBuf), String> {
        let m = model(id).ok_or_else(|| format!("unknown model `{id}`"))?;
        if !kind(&m.kind) {
            return Err(format!("{id} is not a {what} model"));
        }
        let path = store::verified(&self.dir, m)?;
        Ok((m, path))
    }
}

impl Default for Backend {
    fn default() -> Self {
        Backend::new()
    }
}

/// "model htdemucs (169 MB) is not installed: …".
pub fn missing(m: &ModelInfo) -> String {
    format!(
        "model {} ({} MB) is not installed: run wave.model_download {{\"id\": \"{}\"}} or pass download: true",
        m.id,
        (m.bytes as f64 / 1e6).round(),
        m.id
    )
}

impl Models for Backend {
    fn installed(&self, id: &str) -> Option<PathBuf> {
        store::installed(&self.dir, model(id)?)
    }

    fn download(&self, id: &str, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<PathBuf, String> {
        let m = model(id).ok_or_else(|| format!("unknown model `{id}`"))?;
        store::ensure(&self.dir, m, m.url, store::STALL, progress)
    }

    fn separate(&self, id: &str, channels: &[Vec<f32>], sample_rate: u32, progress: &mut dyn FnMut(f32) -> bool) -> Result<Vec<Stem>, String> {
        let (m, path) = self.require(id, |k| matches!(k, ModelKind::Separation { .. }), "separation")?;
        let stems = match m.kind {
            ModelKind::Separation { stems } => usize::from(stems),
            ModelKind::Transcription => 0,
        };
        demucs::separate(&path, self.coreml, stems, channels, sample_rate, progress)
    }

    fn transcribe(&self, id: &str, mono_16k: &[f32], language: Option<&str>) -> Result<Vec<Word>, String> {
        let (m, path) = self.require(id, |k| matches!(k, ModelKind::Transcription), "transcription")?;
        whisper::transcribe(&path, m.id, mono_16k, language)
    }

    fn stretch(&self, channels: &[Vec<f32>], sample_rate: u32, tempo: f32, semitones: f32) -> Result<Vec<Vec<f32>>, String> {
        stretch::stretch(channels, sample_rate, tempo, semitones)
    }
}
