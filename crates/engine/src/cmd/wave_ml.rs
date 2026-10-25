//! WaveCraft's model commands: model downloads, stems, transcription and the practice preview.
//! The work runs through `Engine::wave_models` (the apps install the native runtime). Each long job
//! is a thread-safe *compute* (`*_job(…).run(…)`) and an engine *apply* that refuses results whose
//! file or audio changed meanwhile; the commands run both in line, the app runs the compute on a
//! worker thread.

use super::wave::{audio_key, file_len, has_file, render_file, wave};
use super::*;
use crate::{cmd, edit, io};
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
use soundcraft_time::SampleRate;
use soundcraft_wave_analysis::lyrics::{self, Word};
use soundcraft_wave_analysis::models::{DEFAULT_WHISPER, MODELS, ModelKind, Models, Stem, model};
use std::path::PathBuf;
use std::sync::Arc;

const NO_MODELS: &str = "this build has no model support (use wavecraft or wavecraft-cli)";
const STALE: &str = "discarded: the file changed while the job ran";
const LANGUAGES: [&str; 3] = ["it", "en", "auto"];

/// The stems separated from the file and where they were written.
#[derive(Debug, Clone, PartialEq)]
pub struct Stems {
    /// The file's audio key when separated: stale once it differs.
    pub audio: u64,
    pub model: String,
    pub dir: String,
    /// Stem name and file path.
    pub files: Vec<(String, String)>,
    pub stems: Vec<Stem>,
}

/// A practice render: the file (or a stem mix) at another tempo and pitch, as its own session.
#[derive(Debug, Clone, PartialEq)]
pub struct Practice {
    /// Percent of the original tempo.
    pub tempo: f32,
    pub semitones: f32,
    pub gains: Vec<(String, f32)>,
    pub channels: Arc<Vec<Vec<f32>>>,
    pub session: Arc<Session>,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "wave.models", "Models", [], None, "{} — {models: [{id, kind, stems?, bytes, installed, licence, source}]}: every model WaveCraft knows; none is downloaded unasked", always, models),
        cmd!(noundo "wave.model_download", "Download Model", [], None, "{id} — download one model (size and sha256 checked) into the models folder", always, model_download),
        cmd!(
            noundo "wave.separate_stems",
            "Separate Stems",
            [],
            None,
            "{model?: 4|6 (default 4) or a model id, download?: bool} — split the whole file into drums, bass, other, vocals (+ guitar, piano) with HTDemucs; 32-bit float WAVs in <file folder>/stems/<file name>/; not an undo step",
            has_file,
            separate_stems
        ),
        cmd!(query "wave.stems", "Stems", [], None, "{} — {model, dir, stems: [{name, path}], stale} of the last separation (stale once the audio changed)", has_file, stems_info),
        cmd!(
            noundo "wave.transcribe",
            "Transcribe Lyrics",
            [],
            None,
            "{language?: it|en|auto (default auto), model?: id (default whisper-large-v3-turbo-q5_0), download?: bool} — the sung words with their times, from the vocals stem when there is a current one, else the mix; sets the heard words (supplied lyrics are re-aligned)",
            has_file,
            transcribe
        ),
        cmd!(
            noundo "wave.practice",
            "Practice Preview",
            [],
            None,
            "{tempo?: 50..150 (% of the original, default 100), semitones?: -12..12, stems?: {name: 0..2}} — render the file (or a mix of its current stems) at another tempo and pitch for listening; the file is untouched",
            has_file,
            practice
        ),
        cmd!(noundo "wave.practice_off", "Practice Off", [], None, "{} — drop the practice render and play the file again", has_file, practice_off),
        cmd!(query "wave.practice_export", "Export Practice", [], None, "{path, bit_depth?: 16|24|32|\"32f\" (default 24)} — write the practice render as WAV/AIFF/FLAC", has_file, practice_export),
    ]
}

fn backend(e: &Engine) -> Result<Arc<dyn Models>> {
    e.wave_models.clone().ok_or_else(|| EngineError::Unsupported(NO_MODELS.into()))
}

/// The file's identity for a job: its path and audio key.
#[derive(Debug, Clone, PartialEq)]
struct Origin {
    path: Option<String>,
    audio: u64,
}

fn origin(e: &Engine) -> Result<Origin> {
    Ok(Origin { path: wave(e)?.path.clone(), audio: audio_key(e) })
}

fn check(e: &Engine, o: &Origin) -> Result<()> {
    if e.wave.as_ref().map(|w| &w.path) == Some(&o.path) && audio_key(e) == o.audio { Ok(()) } else { Err(EngineError::NotFound(STALE.into())) }
}

/// Make sure model `id` is installed, downloading it when `download` allows.
fn ensure_model(backend: &dyn Models, id: &str, download: bool, progress: &mut dyn FnMut(f32, &str)) -> std::result::Result<(), String> {
    if backend.installed(id).is_some() {
        return Ok(());
    }
    let m = model(id).ok_or_else(|| format!("unknown model `{id}`"))?;
    if !download {
        return Err(format!(
            "model {} ({} MB) is not installed: run wave.model_download {{\"id\": \"{}\"}} or pass download: true",
            m.id,
            (m.bytes as f64 / 1e6).round(),
            m.id
        ));
    }
    let label = format!("Downloading {}", m.id);
    backend.download(id, &mut |done, total| progress(done as f32 / total.max(1) as f32, &label)).map(|_| ())
}

fn models(e: &mut Engine, _: &Value) -> Result<Value> {
    let b = e.wave_models.clone();
    let list: Vec<Value> = MODELS
        .iter()
        .map(|m| {
            let (kind, stems) = match m.kind {
                ModelKind::Separation { stems } => ("separation", Some(stems)),
                ModelKind::Transcription => ("transcription", None),
            };
            json!({"id": m.id, "kind": kind, "stems": stems, "bytes": m.bytes, "installed": b.as_ref().is_some_and(|b| b.installed(m.id).is_some()), "licence": m.licence, "source": m.source})
        })
        .collect();
    Ok(json!({"models": list, "available": b.is_some()}))
}

/// A model download, runnable off the UI thread.
pub struct DownloadJob {
    backend: Arc<dyn Models>,
    id: String,
}

pub fn download_job(e: &Engine, p: &Value) -> Result<DownloadJob> {
    const ID: &str = "wave.model_download";
    let id = str_param(p, "id").ok_or_else(|| bad(ID, "`id` required (see wave.models)"))?;
    model(id).ok_or_else(|| bad(ID, format!("unknown model `{id}` (see wave.models)")))?;
    Ok(DownloadJob { backend: backend(e)?, id: id.to_string() })
}

impl DownloadJob {
    pub fn run(self, progress: &mut dyn FnMut(f32, &str)) -> std::result::Result<Value, String> {
        ensure_model(self.backend.as_ref(), &self.id, true, progress)?;
        Ok(json!({"id": self.id, "path": self.backend.installed(&self.id).map(|p| p.to_string_lossy().to_string())}))
    }
}

fn model_download(e: &mut Engine, p: &Value) -> Result<Value> {
    download_job(e, p)?.run(&mut |_, _| {}).map_err(EngineError::Io)
}

/// A separation, runnable off the UI thread.
pub struct SeparationJob {
    backend: Arc<dyn Models>,
    model: String,
    download: bool,
    channels: Vec<Vec<f32>>,
    sample_rate: u32,
    dir: PathBuf,
    origin: Origin,
}

/// The separation's result, applied with [`apply_separation`].
pub struct SeparationDone {
    stems: Stems,
    origin: Origin,
}

fn separation_model(cmd: &str, p: &Value) -> Result<String> {
    match p.get("model") {
        None => Ok("htdemucs".into()),
        Some(v) if v.as_u64() == Some(4) => Ok("htdemucs".into()),
        Some(v) if v.as_u64() == Some(6) => Ok("htdemucs_6s".into()),
        Some(v) => match v.as_str().and_then(model) {
            Some(m) if matches!(m.kind, ModelKind::Separation { .. }) => Ok(m.id.into()),
            _ => Err(bad(cmd, "`model` must be 4, 6 or a separation model id")),
        },
    }
}

pub fn separation_job(e: &Engine, p: &Value) -> Result<SeparationJob> {
    const ID: &str = "wave.separate_stems";
    let model = separation_model(ID, p)?;
    let b = backend(e)?;
    let w = wave(e)?;
    let path = w.path.clone().ok_or_else(|| bad(ID, "save the file first: stems are written beside it"))?;
    let file = std::path::Path::new(&path);
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("audio").to_string();
    let dir = file.parent().map_or_else(|| PathBuf::from("stems"), |d| d.join("stems")).join(stem);
    Ok(SeparationJob {
        backend: b,
        model,
        download: bool_or(p, "download", false),
        channels: render_file(e)?,
        sample_rate: w.sample_rate,
        dir,
        origin: origin(e)?,
    })
}

impl SeparationJob {
    pub fn run(self, progress: &mut dyn FnMut(f32, &str)) -> std::result::Result<SeparationDone, String> {
        ensure_model(self.backend.as_ref(), &self.model, self.download, progress)?;
        let label = format!("Separating ({})", self.model);
        let stems = self.backend.separate(&self.model, &self.channels, self.sample_rate, &mut |f| progress(f * 0.95, &label))?;
        io::make_dir(&self.dir).map_err(|e| e.to_string())?;
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
        let mut files = Vec::with_capacity(stems.len());
        for s in &stems {
            let path = self.dir.join(format!("{}.wav", s.name)).to_string_lossy().to_string();
            io::write_audio_file(&path, &opts, &s.channels, self.sample_rate).map_err(|e| e.to_string())?;
            files.push((s.name.clone(), path));
        }
        progress(1.0, &label);
        Ok(SeparationDone {
            stems: Stems { audio: self.origin.audio, model: self.model, dir: self.dir.to_string_lossy().to_string(), files, stems },
            origin: self.origin,
        })
    }
}

fn stems_json(s: &Stems, stale: bool) -> Value {
    json!({"model": s.model, "dir": s.dir, "stems": s.files.iter().map(|(n, p)| json!({"name": n, "path": p})).collect::<Vec<_>>(), "stale": stale})
}

pub fn apply_separation(e: &mut Engine, done: SeparationDone) -> Result<Value> {
    check(e, &done.origin)?;
    let out = stems_json(&done.stems, false);
    if let Some(w) = e.wave.as_mut() {
        w.stems = Some(done.stems);
    }
    Ok(out)
}

fn separate_stems(e: &mut Engine, p: &Value) -> Result<Value> {
    let done = separation_job(e, p)?.run(&mut |_, _| {}).map_err(EngineError::Io)?;
    apply_separation(e, done)
}

fn stems_info(e: &mut Engine, _: &Value) -> Result<Value> {
    let audio = audio_key(e);
    Ok(wave(e)?.stems.as_ref().map_or_else(|| json!({"stems": []}), |s| stems_json(s, s.audio != audio)))
}

/// The current stems (separated from the audio as it is now).
fn fresh_stems(e: &Engine) -> Option<&Stems> {
    let audio = audio_key(e);
    e.wave.as_ref()?.stems.as_ref().filter(|s| s.audio == audio)
}

/// Whether current stems with a harmonic part exist (cheap: nothing is summed).
pub fn has_harmonic_stems(e: &Engine) -> bool {
    fresh_stems(e).is_some_and(|s| s.stems.iter().any(|st| st.name != "vocals" && st.name != "drums"))
}

/// The harmonic part of the current stems — everything but the vocals and the drums — summed: what
/// chords and key are best read from.
pub fn harmonic_stems(e: &Engine) -> Option<Vec<Vec<f32>>> {
    let stems = fresh_stems(e)?;
    let parts: Vec<&Stem> = stems.stems.iter().filter(|s| s.name != "vocals" && s.name != "drums").collect();
    let first = parts.first()?;
    let mut sum: Vec<Vec<f32>> = first.channels.iter().map(|c| vec![0.0f32; c.len()]).collect();
    for s in &parts {
        for (acc, c) in sum.iter_mut().zip(&s.channels) {
            for (a, v) in acc.iter_mut().zip(c) {
                *a += v;
            }
        }
    }
    Some(sum)
}

fn mono_16k(channels: &[Vec<f32>], sample_rate: u32) -> Vec<f32> {
    let n = channels.iter().map(Vec::len).min().unwrap_or(0);
    let scale = 1.0 / channels.len().max(1) as f32;
    let mono: Vec<f32> = (0..n).map(|i| channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() * scale).collect();
    soundcraft_wave_analysis::resample(&mono, sample_rate, 16_000)
}

/// A transcription, runnable off the UI thread.
pub struct TranscriptionJob {
    backend: Arc<dyn Models>,
    model: String,
    download: bool,
    language: Option<String>,
    audio_16k: Vec<f32>,
    source: &'static str,
    origin: Origin,
}

pub struct TranscriptionDone {
    words: Vec<Word>,
    source: &'static str,
    origin: Origin,
}

pub fn transcription_job(e: &Engine, p: &Value) -> Result<TranscriptionJob> {
    const ID: &str = "wave.transcribe";
    let language = match str_param(p, "language") {
        None | Some("auto") => None,
        Some(l) if LANGUAGES.contains(&l) => Some(l.to_string()),
        Some(l) => return Err(bad(ID, format!("`language` must be it, en or auto, not `{l}`"))),
    };
    let model = match str_param(p, "model") {
        None => DEFAULT_WHISPER.to_string(),
        Some(id) => match model(id) {
            Some(m) if matches!(m.kind, ModelKind::Transcription) => m.id.to_string(),
            _ => return Err(bad(ID, format!("`{id}` is not a transcription model (see wave.models)"))),
        },
    };
    let b = backend(e)?;
    let sr = wave(e)?.sample_rate;
    let (audio_16k, source) = match fresh_stems(e).and_then(|s| s.stems.iter().find(|st| st.name == "vocals")) {
        Some(v) => (mono_16k(&v.channels, sr), "vocals"),
        None => (mono_16k(&render_file(e)?, sr), "mix"),
    };
    Ok(TranscriptionJob { backend: b, model, download: bool_or(p, "download", false), language, audio_16k, source, origin: origin(e)? })
}

impl TranscriptionJob {
    pub fn run(self, progress: &mut dyn FnMut(f32, &str)) -> std::result::Result<TranscriptionDone, String> {
        ensure_model(self.backend.as_ref(), &self.model, self.download, progress)?;
        progress(0.0, "Transcribing");
        let words = self.backend.transcribe(&self.model, &self.audio_16k, self.language.as_deref())?;
        progress(1.0, "Transcribing");
        Ok(TranscriptionDone { words, source: self.source, origin: self.origin })
    }
}

pub fn apply_transcription(e: &mut Engine, done: TranscriptionDone) -> Result<Value> {
    check(e, &done.origin)?;
    let end_s = file_len(e) as f32 / wave(e)?.sample_rate.max(1) as f32;
    let text: String = done.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    let w = e.wave.as_mut().ok_or_else(|| EngineError::NotFound("no file open".into()))?;
    let n = done.words.len();
    w.lyrics.heard = done.words.into_iter().map(|x| Word { start: x.start.min(end_s), end: x.end.min(end_s), ..x }).collect();
    w.lyrics.lines = match &w.lyrics.text {
        Some(t) => lyrics::align(t, &w.lyrics.heard, end_s),
        None => Vec::new(),
    };
    w.lyrics.version = w.lyrics.version.wrapping_add(1);
    Ok(json!({"words": n, "text": text, "source": done.source}))
}

fn transcribe(e: &mut Engine, p: &Value) -> Result<Value> {
    let done = transcription_job(e, p)?.run(&mut |_, _| {}).map_err(EngineError::Io)?;
    apply_transcription(e, done)
}

/// A practice render, runnable off the UI thread.
pub struct PracticeJob {
    backend: Arc<dyn Models>,
    channels: Vec<Vec<f32>>,
    sample_rate: u32,
    tempo: f32,
    semitones: f32,
    gains: Vec<(String, f32)>,
    origin: Origin,
}

pub struct PracticeDone {
    practice: Practice,
    origin: Origin,
}

pub fn practice_job(e: &Engine, p: &Value) -> Result<PracticeJob> {
    const ID: &str = "wave.practice";
    let num = |key: &str, d: f64, lo: f64, hi: f64| -> Result<f64> {
        match p.get(key) {
            None => Ok(d),
            Some(v) => v
                .as_f64()
                .filter(|x| x.is_finite() && (lo..=hi).contains(x))
                .ok_or_else(|| bad(ID, format!("`{key}` must be a number from {lo} to {hi}"))),
        }
    };
    let tempo = num("tempo", 100.0, 50.0, 150.0)? as f32;
    let semitones = num("semitones", 0.0, -12.0, 12.0)? as f32;
    let b = backend(e)?;
    let sr = wave(e)?.sample_rate;
    let (channels, gains) = match p.get("stems").and_then(Value::as_object) {
        Some(map) => {
            let stems = fresh_stems(e).ok_or_else(|| bad(ID, "no current stems: run wave.separate_stems first"))?;
            let mut gains = Vec::new();
            for s in &stems.stems {
                let g = match map.get(&s.name) {
                    None => 1.0,
                    Some(v) => v
                        .as_f64()
                        .filter(|x| x.is_finite() && (0.0..=2.0).contains(x))
                        .ok_or_else(|| bad(ID, format!("stem gain `{}` must be from 0 to 2", s.name)))?,
                };
                gains.push((s.name.clone(), g as f32));
            }
            if let Some(unknown) = map.keys().find(|k| !stems.stems.iter().any(|s| &s.name == *k)) {
                return Err(bad(ID, format!("no stem named `{unknown}`")));
            }
            let n = stems.stems.first().map_or(0, |s| s.channels.len());
            let len = stems.stems.first().and_then(|s| s.channels.first()).map_or(0, Vec::len);
            let mut mix = vec![vec![0.0f32; len]; n];
            for (s, (_, g)) in stems.stems.iter().zip(&gains) {
                for (m, c) in mix.iter_mut().zip(&s.channels) {
                    for (a, v) in m.iter_mut().zip(c) {
                        *a += v * g;
                    }
                }
            }
            (mix, gains)
        }
        None => (render_file(e)?, Vec::new()),
    };
    Ok(PracticeJob { backend: b, channels, sample_rate: sr, tempo, semitones, gains, origin: origin(e)? })
}

/// A one-track session playing `channels` (stereo: a mono render is doubled, so it plays at level).
fn practice_session(channels: &[Vec<f32>], sample_rate: u32) -> std::result::Result<Session, String> {
    let rate = SampleRate::new(sample_rate).map_err(|e| e.to_string())?;
    let stereo: Vec<Vec<f32>> = match channels {
        [m] => vec![m.clone(), m.clone()],
        more => more.to_vec(),
    };
    let frames = stereo.first().map_or(0, Vec::len);
    let mut s = Session::new("Practice".to_string(), rate);
    let t = s.add_track(TrackKind::Audio, ChannelFormat::for_channels(stereo.len()), Some("Practice"));
    let src = io::add_source(&mut s, "Practice", AudioBuffer { sample_rate, channels: stereo }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    edit::place_clip(&mut s, t, Clip::audio(id, "Practice", src, 0, 0, frames as soundcraft_time::Samples));
    Ok(s)
}

impl PracticeJob {
    pub fn run(self, progress: &mut dyn FnMut(f32, &str)) -> std::result::Result<PracticeDone, String> {
        progress(0.0, "Rendering practice");
        let out = self.backend.stretch(&self.channels, self.sample_rate, self.tempo / 100.0, self.semitones)?;
        let session = practice_session(&out, self.sample_rate)?;
        progress(1.0, "Rendering practice");
        Ok(PracticeDone {
            practice: Practice {
                tempo: self.tempo,
                semitones: self.semitones,
                gains: self.gains,
                channels: Arc::new(out),
                session: Arc::new(session),
            },
            origin: self.origin,
        })
    }
}

pub fn apply_practice(e: &mut Engine, done: PracticeDone) -> Result<Value> {
    check(e, &done.origin)?;
    let sr = wave(e)?.sample_rate.max(1);
    let p = done.practice;
    let seconds = p.channels.first().map_or(0, Vec::len) as f64 / f64::from(sr);
    let out = json!({"tempo": p.tempo, "semitones": p.semitones, "seconds": (seconds * 1000.0).round() / 1000.0});
    if let Some(w) = e.wave.as_mut() {
        w.practice = Some(p);
    }
    Ok(out)
}

fn practice(e: &mut Engine, p: &Value) -> Result<Value> {
    let done = practice_job(e, p)?.run(&mut |_, _| {}).map_err(EngineError::Io)?;
    apply_practice(e, done)
}

fn practice_off(e: &mut Engine, _: &Value) -> Result<Value> {
    let w = e.wave.as_mut().ok_or_else(|| EngineError::NotFound("no file open".into()))?;
    let was = w.practice.take().is_some();
    Ok(json!({"was_on": was}))
}

fn practice_export(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.practice_export";
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(ID, "`path` required"))?.to_string();
    let depth = super::wave::depth_param(ID, p)?.unwrap_or(BitDepth::Int24);
    let w = wave(e)?;
    let pr = w.practice.as_ref().ok_or_else(|| bad(ID, "no practice render: run wave.practice first"))?;
    let format = match std::path::Path::new(&path).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("aif" | "aiff") => FileFormat::Aiff,
        Some("flac") => FileFormat::Flac,
        _ => FileFormat::Wav,
    };
    let opts = EncodeOptions { format, bit_depth: depth, dither: depth == BitDepth::Int16, bwf: None };
    let frames = io::write_audio_file(&path, &opts, &pr.channels, w.sample_rate)?;
    Ok(json!({"path": path, "frames": frames}))
}

#[cfg(test)]
mod tests {
    use super::super::wave::tests::{opened, tmp_dir};
    use super::*;
    use serde_json::json;
    use soundcraft_audio_io::{AudioBuffer, BitDepth};
    use soundcraft_wave_analysis::lyrics::Word;
    use soundcraft_wave_analysis::models::Models;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Separation = four quarters of the mix; transcription = two fixed words; stretch = linear
    /// interpolation to the new length (pitch ignored).
    #[derive(Default)]
    struct Fake {
        installed: Mutex<bool>,
        downloads: AtomicUsize,
        transcribed_peak: Mutex<f32>,
    }

    impl Models for Fake {
        fn installed(&self, _: &str) -> Option<PathBuf> {
            self.installed.lock().ok().filter(|i| **i).map(|_| PathBuf::from("/fake"))
        }
        fn download(&self, _: &str, progress: &mut dyn FnMut(u64, u64)) -> std::result::Result<PathBuf, String> {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            progress(1, 1);
            if let Ok(mut i) = self.installed.lock() {
                *i = true;
            }
            Ok(PathBuf::from("/fake"))
        }
        fn separate(&self, id: &str, ch: &[Vec<f32>], _: u32, progress: &mut dyn FnMut(f32)) -> std::result::Result<Vec<Stem>, String> {
            if self.installed(id).is_none() {
                return Err(format!("model {id} (169 MB) is not installed"));
            }
            progress(1.0);
            Ok(["drums", "bass", "other", "vocals"]
                .iter()
                .map(|n| Stem { name: n.to_string(), channels: ch.iter().map(|c| c.iter().map(|v| v * 0.25).collect()).collect() })
                .collect())
        }
        fn transcribe(&self, id: &str, mono: &[f32], _: Option<&str>) -> std::result::Result<Vec<Word>, String> {
            if self.installed(id).is_none() {
                return Err(format!("model {id} (574 MB) is not installed"));
            }
            if let Ok(mut p) = self.transcribed_peak.lock() {
                *p = mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            }
            Ok(vec![Word { text: "ciao".into(), start: 0.5, end: 0.8 }, Word { text: "mondo".into(), start: 0.9, end: 1.3 }])
        }
        fn stretch(&self, ch: &[Vec<f32>], _: u32, tempo: f32, _: f32) -> std::result::Result<Vec<Vec<f32>>, String> {
            Ok(ch
                .iter()
                .map(|c| {
                    let n = (c.len() as f32 / tempo).round() as usize;
                    (0..n).map(|i| c.get(((i as f32 * tempo) as usize).min(c.len().saturating_sub(1))).copied().unwrap_or(0.0)).collect()
                })
                .collect())
        }
    }

    fn take(test: &str) -> (Engine, std::path::PathBuf, Arc<Fake>) {
        let ch: Vec<f32> = (0..96_000).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
        let (mut e, dir) = opened(test, &AudioBuffer { sample_rate: 48_000, channels: vec![ch.clone(), ch] }, BitDepth::Float32);
        let fake = Arc::new(Fake::default());
        e.wave_models = Some(fake.clone());
        (e, dir, fake)
    }

    fn run(e: &mut Engine, id: &str, p: Value) -> Value {
        e.execute(id, &p).unwrap_or_else(|err| panic!("{id}: {err}"))
    }

    #[test]
    fn separate_writes_stems_beside_the_file() {
        let (mut e, dir, fake) = take("ml_separate");
        *fake.installed.lock().unwrap() = true;
        let n = e.undo_history().len();
        let r = run(&mut e, "wave.separate_stems", json!({}));
        assert_eq!(e.undo_history().len(), n);
        let stems = r["stems"].as_array().unwrap();
        assert_eq!(stems.len(), 4);
        assert!(r["dir"].as_str().unwrap().starts_with(dir.join("stems").to_str().unwrap()));
        for s in stems {
            let bytes = std::fs::read(s["path"].as_str().unwrap()).unwrap();
            let (info, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).unwrap();
            assert_eq!((info.sample_rate, buf.channels.len(), buf.frames()), (48_000, 2, 96_000));
        }
        assert_eq!(run(&mut e, "wave.stems", json!({}))["stale"], false);
    }

    #[test]
    fn missing_model_refuses_unless_download() {
        let (mut e, _, fake) = take("ml_missing");
        let err = e.execute("wave.separate_stems", &json!({"model": 4})).unwrap_err().to_string();
        assert!(err.contains("not installed"), "{err}");
        run(&mut e, "wave.separate_stems", json!({"model": 4, "download": true}));
        assert_eq!(fake.downloads.load(Ordering::SeqCst), 1);
        let models = run(&mut e, "wave.models", json!({}));
        assert!(models["models"].as_array().unwrap().iter().any(|m| m["id"] == "htdemucs" && m["bytes"] == 168_683_222));
        run(&mut e, "wave.model_download", json!({"id": "whisper-large-v3-turbo-q5_0"}));
        assert_eq!(fake.downloads.load(Ordering::SeqCst), 1, "already installed: no second download");
    }

    #[test]
    fn transcribe_prefers_the_vocals_stem_and_sets_words() {
        let (mut e, _, fake) = take("ml_transcribe");
        *fake.installed.lock().unwrap() = true;
        let mix = run(&mut e, "wave.transcribe", json!({"language": "it"}));
        assert_eq!((mix["source"].as_str(), mix["words"].as_u64()), (Some("mix"), Some(2)));
        let mix_peak = *fake.transcribed_peak.lock().unwrap();
        run(&mut e, "wave.separate_stems", json!({}));
        let voc = run(&mut e, "wave.transcribe", json!({}));
        assert_eq!(voc["source"], "vocals");
        assert!(*fake.transcribed_peak.lock().unwrap() < 0.5 * mix_peak, "the vocals stem (a quarter of the mix) was not used");
        assert_eq!(run(&mut e, "wave.lyrics", json!({}))["heard"], 2);
        run(&mut e, "wave.lyrics_set", json!({"text": "Ciao mondo"}));
        let col = run(&mut e, "wave.chords_over_lyrics", json!({}));
        assert!(col["chordpro"].as_str().unwrap().contains("Ciao"), "{}", col["chordpro"]);
    }

    #[test]
    fn practice_scales_length_and_exports() {
        let (mut e, _, _) = take("ml_practice");
        let r = run(&mut e, "wave.practice", json!({"tempo": 80, "semitones": 2}));
        assert!((r["seconds"].as_f64().unwrap() - 2.5).abs() < 0.01, "{r}");
        let p = e.wave.as_ref().and_then(|w| w.practice.clone()).unwrap();
        assert_eq!(p.channels[0].len(), 120_000);
        assert_eq!(p.session.tracks.len(), 1);
        let path = tmp_dir("ml_practice_out").join("practice.wav");
        run(&mut e, "wave.practice_export", json!({"path": path.to_str().unwrap(), "bit_depth": 24}));
        let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), Some("wav")).unwrap();
        assert_eq!(buf.frames(), 120_000);
        run(&mut e, "wave.practice_off", json!({}));
        assert!(e.wave.as_ref().unwrap().practice.is_none());
        assert_eq!(super::super::wave::render_file(&e).unwrap()[0].len(), 96_000, "the file is untouched");
    }

    #[test]
    fn practice_mixes_stems_with_gains() {
        let (mut e, _, fake) = take("ml_practice_stems");
        *fake.installed.lock().unwrap() = true;
        run(&mut e, "wave.separate_stems", json!({}));
        run(&mut e, "wave.practice", json!({"stems": {"vocals": 0, "drums": 0, "bass": 0, "other": 2}}));
        let p = e.wave.as_ref().and_then(|w| w.practice.clone()).unwrap();
        let file = super::super::wave::render_file(&e).unwrap();
        // other ×2 = half the mix.
        assert!((p.channels[0][1000] - 0.5 * file[0][1000]).abs() < 1e-5);
    }

    #[test]
    fn chords_come_from_the_harmonic_stems_once_separated() {
        let (mut e, _, fake) = take("ml_harmonic");
        *fake.installed.lock().unwrap() = true;
        assert_eq!(run(&mut e, "wave.analyze", json!({}))["source"], "mix");
        run(&mut e, "wave.separate_stems", json!({}));
        assert_eq!(run(&mut e, "wave.analysis_info", json!({}))["stale"], true, "an analysis of the mix is superseded by the stems");
        assert_eq!(run(&mut e, "wave.analyze_key", json!({}))["source"], "stems");
        let harmonic = harmonic_stems(&e).expect("harmonic stems");
        // Fake stems are quarters of the mix: other + bass = half of it.
        let file = super::super::wave::render_file(&e).unwrap();
        assert!((harmonic[0][500] - 0.5 * file[0][500]).abs() < 1e-6);
    }

    #[test]
    fn stale_job_results_are_refused() {
        let (mut e, _, fake) = take("ml_stale");
        *fake.installed.lock().unwrap() = true;
        let job = separation_job(&e, &json!({})).unwrap();
        let done = job.run(&mut |_, _| {}).unwrap();
        run(&mut e, "wave.gain", json!({"db": -1}));
        let err = apply_separation(&mut e, done).unwrap_err().to_string();
        assert!(err.contains("changed"), "{err}");
        assert!(e.wave.as_ref().unwrap().stems.is_none());
    }

    #[test]
    fn no_backend_and_hostile_params_refused() {
        let (mut e, _, fake) = take("ml_hostile");
        *fake.installed.lock().unwrap() = true;
        for (id, p) in [
            ("wave.separate_stems", json!({"model": 5})),
            ("wave.practice", json!({"tempo": 300})),
            ("wave.practice", json!({"semitones": 20})),
            ("wave.practice", json!({"stems": {"vocals": 9}})),
            ("wave.transcribe", json!({"language": "klingon"})),
            ("wave.model_download", json!({"id": "nope"})),
            ("wave.practice_export", json!({"path": "/tmp/x.wav"})),
        ] {
            assert!(e.execute(id, &p).is_err(), "{id} {p} accepted");
        }
        e.wave_models = None;
        let err = e.execute("wave.separate_stems", &json!({})).unwrap_err().to_string();
        assert!(err.contains("no model support"), "{err}");
        assert!(e.execute("wave.models", &json!({})).is_ok());
    }
}
