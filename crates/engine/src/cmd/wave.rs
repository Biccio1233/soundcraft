//! WaveCraft: editing one audio file. The document is a one-track session whose clips form a
//! gap-free piece table over sources; the file's samples are `render_clips` of that track.

use super::*;
use crate::{cmd, edit, io};
use serde_json::json;
use soundcraft_audio_io::{BitDepth, EncodeOptions, FileFormat, SampleFormat};
use soundcraft_dsp::dither::{self, DitherProfile};
use soundcraft_model::{ChannelFormat, Clip, EditMode, Session, TrackKind};
use soundcraft_time::SampleRate;

/// Seed of the export dither ("WCAFT"), xored with the channel index: exports are reproducible.
const DITHER_SEED: u64 = 0x5743_4146_5400_0000;
/// A mono track plays through the stereo main mix at -3 dB (pan law); this puts it back to the
/// file's level. It only affects playback: the file is rendered without the mixer.
const MONO_PLAYBACK_DB: f32 = 3.0103;
/// Default history memory budget.
pub const DEFAULT_HISTORY_BUDGET_MB: u32 = 2048;
/// Step limit for a WaveCraft file: far above anything the byte budget lets through, so the
/// budget (which counts and reports what it drops) decides, never a silent step count.
const WAVE_HISTORY_LIMIT: usize = 1_000_000;

/// The file WaveCraft has open.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveFile {
    /// Where it was opened from (`None` for a file that was never saved).
    pub path: Option<String>,
    pub format: FileFormat,
    pub sample_format: SampleFormat,
    pub channels: usize,
    pub sample_rate: u32,
    /// Which channels edits apply to (all by default).
    pub channels_enabled: Vec<bool>,
    /// The audio the history may keep before its oldest steps are dropped.
    pub history_budget_mb: u32,
    /// How many of the oldest steps the budget has dropped since the file was opened.
    pub history_dropped: usize,
    /// Show the spectral view under the waveform (a view setting: not part of the file).
    pub spectral: bool,
    /// The noise print captured for noise reduction (not part of the file, not an undo step).
    pub noise_print: Option<super::wave_restore::NoiseCapture>,
    /// Show the beat, chord and section lanes when an analysis exists (a view setting).
    pub show_analysis: bool,
    /// The last music analysis (not part of the file, not an undo step).
    pub analysis: Option<super::wave_analysis::Analysis>,
    /// Timed words and the lyrics aligned to them (not part of the file, not an undo step).
    pub lyrics: super::wave_analysis::Lyrics,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "wave.open",
            "Open File",
            [],
            None,
            "{path, discard?: bool} — open one audio file (WAV, AIFF, FLAC, MP3, M4A, Ogg…) as the WaveCraft document, replacing the open one and clearing the history; refuses when the open file has unsaved changes unless `discard` is true, and never replaces a multitrack session",
            always,
            open
        ),
        cmd!(
            query "wave.info",
            "File Info",
            [],
            None,
            "{} — path, format, sample format, channels, sample rate, frames, seconds, selection, enabled channels, dirty, undo/redo counts",
            has_file,
            info_cmd
        ),
        cmd!(
            noundo "wave.select",
            "Select",
            [],
            None,
            "{start?, end?, channels?: [0, 1…]} — time selection (clamped to the file) and the channels edits apply to",
            has_file,
            select
        ),
        cmd!(
            query "wave.export",
            "Export File",
            [],
            None,
            "{path, format?: wav|aiff|flac (default from the extension), bit_depth?: 16|24|32|\"32f\" (default 24), dither?: none|tpdf|shaped_light|shaped_strong (default tpdf at 16 bits, none above)} — write the file as it is now; the rack is not included until applied",
            has_file,
            export
        ),
        cmd!(
            query "wave.history",
            "History",
            [],
            None,
            "{} — the history rows (row 0 is the opened file; `index` is what wave.history_jump takes; state done|current|undone), the budget and the audio it holds",
            has_file,
            history
        ),
        cmd!(noundo "wave.history_jump", "Go to History Step", [], None, "{index} — go to that history row (undoing or redoing the steps in between)", has_file, history_jump),
        cmd!(
            noundo "wave.history_budget",
            "History Memory",
            [],
            None,
            "{mb: 64..65536} — the audio the history may keep (default 2048 MB); older steps beyond it are dropped",
            has_file,
            history_budget
        ),
        cmd!(view "wave.view_spectral", "Spectral Display", [], None, "{on?: bool} — show or hide the spectral view (toggles without `on`)", has_file, view_spectral),
        cmd!(
            noundo "wave.save",
            "Save",
            [],
            None,
            "{dither?} — write the file back to its path, format and bit depth (WAV/AIFF/FLAC at 16 bits or more); anything else needs wave.export",
            has_file,
            save
        ),
    ]
}

/// Enabled when a file is open.
pub fn has_file(e: &Engine) -> std::result::Result<(), String> {
    if e.wave.is_some() { Ok(()) } else { Err("no file open".into()) }
}

/// The open file, or an error.
pub fn wave(e: &Engine) -> Result<&WaveFile> {
    e.wave.as_ref().ok_or_else(|| EngineError::NotFound("no file open".into()))
}

/// The document's one audio track.
pub fn wave_track(e: &Engine) -> Result<TrackId> {
    wave(e)?;
    match e.session().tracks.as_slice() {
        [t] if t.kind == TrackKind::Audio => Ok(t.id),
        _ => Err(EngineError::Internal("wave".into(), "the WaveCraft document must hold exactly one audio track".into())),
    }
}

/// The file's length in frames (the end of its last clip).
pub fn file_len(e: &Engine) -> Samples {
    e.session().tracks.first().map_or(0, |t| t.clips().iter().map(|c| c.start.saturating_add(c.length)).max().unwrap_or(0))
}

/// A key of the file's audio as it now sounds (its clips, their gains and fades): it changes with
/// every edit, comes back with an undo, and never moves with the selection or the view.
pub fn audio_key(e: &Engine) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(t) = e.session().tracks.first() {
        for c in t.clips() {
            (c.id.0, c.start, c.length, c.gain_db.to_bits()).hash(&mut h);
            format!("{:?}{:?}{:?}", c.fade_in, c.fade_out, c.content).hash(&mut h);
        }
    }
    h.finish()
}

/// The file's samples as they are now: its clips rendered without the mixer, one `Vec` per channel.
pub fn render_file(e: &Engine) -> Result<Vec<Vec<f32>>> {
    let t = wave_track(e)?;
    Ok(soundcraft_mix::render_clips(e.session(), t, Range::new(0, file_len(e))))
}

/// The range a command works on: `start`/`end` params, else the selection, clamped to the file.
/// With neither (an empty selection and no params) it is the whole file; an explicit empty range
/// stays empty for the command to refuse.
pub fn wave_range(e: &Engine, cmd: &str, p: &Value) -> Result<Range> {
    let len = file_len(e);
    let explicit = ["start", "end", "length"].iter().any(|k| p.get(k).is_some());
    let r = range_param(e, cmd, p)?;
    let r = Range::new(r.start.clamp(0, len), r.end.clamp(0, len));
    Ok(if r.is_empty() && !explicit { Range::new(0, len) } else { r })
}

fn check_limits(cmd: &str, frames: u64, channels: usize) -> Result<()> {
    if frames == 0 {
        return Err(bad(cmd, "the file holds no audio"));
    }
    if !(1..=2).contains(&channels) {
        return Err(EngineError::Unsupported(format!("{channels}-channel files (WaveCraft edits mono and stereo files)")));
    }
    let too_long = frames > soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES as u64
        || frames.saturating_mul(channels as u64) > soundcraft_mix::MAX_RENDER_SAMPLES as u64;
    if too_long {
        return Err(EngineError::Unsupported(format!(
            "files longer than {} frames per channel ({} samples in all)",
            soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES,
            soundcraft_mix::MAX_RENDER_SAMPLES
        )));
    }
    Ok(())
}

fn open(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.open";
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(ID, "`path` required"))?.to_string();
    // WaveCraft's open never replaces a SoundCraft session, and never drops unsaved edits unasked.
    if e.wave.is_none() && !e.session().tracks.is_empty() {
        return Err(bad(ID, "this engine holds a multitrack session; wave.open is WaveCraft's single-file open"));
    }
    if e.wave.is_some() && e.is_dirty() && !p.get("discard").and_then(Value::as_bool).unwrap_or(false) {
        return Err(bad(ID, "the open file has unsaved changes: save or export it first, or pass `discard: true`"));
    }
    let bytes = io::read_file(&path)?;
    let ext = std::path::Path::new(&path).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let audio_err = |err: soundcraft_audio_io::AudioError| EngineError::Io(format!("{path}: {err}"));
    let probed = soundcraft_audio_io::probe(&bytes, ext.as_deref()).map_err(audio_err)?;
    check_limits(ID, probed.frames, usize::from(probed.channels))?;
    let (info, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(audio_err)?;
    let (channels, frames) = (buf.channels.len(), buf.frames());
    check_limits(ID, frames as u64, channels)?;
    let rate = SampleRate::new(info.sample_rate).map_err(|err| bad(ID, err.to_string()))?;
    let name = std::path::Path::new(&path).file_name().and_then(|n| n.to_str()).unwrap_or("Audio").to_string();
    let mut s = Session::new(name.clone(), rate);
    let t = s.add_track(TrackKind::Audio, ChannelFormat::for_channels(channels), Some(&name));
    let src = io::add_source(&mut s, &name, buf, Some(&path), info.format);
    let id = s.new_clip_id();
    edit::place_clip(&mut s, t, Clip::audio(id, &name, src, 0, 0, frames as Samples));
    s.edit.edit_mode = EditMode::Shuffle;
    s.edit.selected_tracks = vec![t];
    if channels == 1
        && let Some(track) = s.track_mut(t)
    {
        track.mixer.volume_db = MONO_PLAYBACK_DB;
    }
    e.replace_session(s);
    e.history_limit = WAVE_HISTORY_LIMIT;
    e.wave = Some(WaveFile {
        path: Some(path),
        format: info.format,
        sample_format: info.sample_format,
        channels,
        sample_rate: info.sample_rate,
        channels_enabled: vec![true; channels],
        history_budget_mb: DEFAULT_HISTORY_BUDGET_MB,
        history_dropped: 0,
        spectral: false,
        noise_print: None,
        show_analysis: true,
        analysis: None,
        lyrics: Default::default(),
    });
    info_json(e)
}

pub fn format_name(f: FileFormat) -> &'static str {
    match f {
        FileFormat::Wav => "wav",
        FileFormat::Aiff => "aiff",
        FileFormat::Flac => "flac",
        FileFormat::Mp3 => "mp3",
        FileFormat::Ogg => "ogg",
        FileFormat::Aac => "aac",
        FileFormat::Alac => "alac",
        FileFormat::Caf => "caf",
        FileFormat::Other => "other",
    }
}

pub fn sample_format_name(f: SampleFormat) -> &'static str {
    match f {
        SampleFormat::Int8 => "int8",
        SampleFormat::Int16 => "int16",
        SampleFormat::Int24 => "int24",
        SampleFormat::Int32 => "int32",
        SampleFormat::Float32 => "float32",
        SampleFormat::Float64 => "float64",
        SampleFormat::Compressed => "compressed",
    }
}

/// `wave.info` as JSON.
pub fn info(e: &Engine) -> Result<Value> {
    info_json(e)
}

fn info_json(e: &Engine) -> Result<Value> {
    let w = wave(e)?;
    wave_track(e)?;
    let frames = file_len(e);
    let sel = e.session().edit.selection;
    Ok(json!({
        "path": w.path,
        "format": format_name(w.format),
        "sample_format": sample_format_name(w.sample_format),
        "channels": w.channels,
        "sample_rate": w.sample_rate,
        "frames": frames,
        "seconds": frames as f64 / f64::from(w.sample_rate.max(1)),
        "selection": {"start": sel.start, "end": sel.end},
        "channels_enabled": w.channels_enabled,
        "dirty": e.is_dirty(),
        "undo": e.undo_history().len(),
        "redo": e.redo_history().len(),
    }))
}

fn info_cmd(e: &mut Engine, _: &Value) -> Result<Value> {
    info_json(e)
}

fn select(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.select";
    let t = wave_track(e)?;
    let channels = wave(e)?.channels;
    // Validate everything before changing anything (`e.wave` lives outside the undoable document).
    let mask = match p.get("channels") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let list = v.as_array().ok_or_else(|| bad(ID, "`channels` must be a list of channel indexes"))?;
            let mut mask = vec![false; channels];
            for c in list {
                let i = c.as_u64().and_then(|i| usize::try_from(i).ok()).filter(|i| *i < channels);
                let slot = i.and_then(|i| mask.get_mut(i)).ok_or_else(|| bad(ID, format!("no channel {c} in a {channels}-channel file")))?;
                *slot = true;
            }
            if !mask.contains(&true) {
                return Err(bad(ID, "enable at least one channel"));
            }
            Some(mask)
        }
    };
    let range = if ["start", "end", "length"].iter().any(|k| p.get(k).is_some()) { Some(wave_range(e, ID, p)?) } else { None };
    if let Some(r) = range {
        let s = e.session_mut();
        s.edit.selection = r;
        s.edit.selected_tracks = vec![t];
    }
    if let (Some(mask), Some(w)) = (mask, e.wave.as_mut()) {
        w.channels_enabled = mask;
    }
    info_json(e)
}

/// Keep the history inside the file's budget, counting and reporting dropped steps.
pub fn enforce_history_budget(e: &mut Engine) -> Result<usize> {
    let budget = u64::from(wave(e)?.history_budget_mb).saturating_mul(1 << 20);
    let dropped = e.trim_history_to_bytes(budget);
    if dropped > 0 {
        if let Some(w) = e.wave.as_mut() {
            w.history_dropped = w.history_dropped.saturating_add(dropped);
        }
        e.messages.push(format!("History trimmed to its {} MB budget: dropped the {dropped} oldest steps", budget >> 20));
    }
    Ok(dropped)
}

fn history_json(e: &Engine) -> Result<Value> {
    let w = wave(e)?;
    let undo = e.undo_history();
    let current = undo.len();
    let first = if w.history_dropped == 0 { "Open".to_string() } else { format!("Open (+{} steps dropped)", w.history_dropped) };
    let labels = std::iter::once(first).chain(undo).chain(e.redo_history());
    let rows: Vec<Value> = labels
        .enumerate()
        .map(|(i, label)| {
            let state = match i.cmp(&current) {
                std::cmp::Ordering::Less => "done",
                std::cmp::Ordering::Equal => "current",
                std::cmp::Ordering::Greater => "undone",
            };
            json!({"index": i, "label": label, "state": state})
        })
        .collect();
    Ok(json!({"rows": rows, "budget_mb": w.history_budget_mb, "bytes": e.history_bytes(), "dropped": w.history_dropped}))
}

fn history(e: &mut Engine, _: &Value) -> Result<Value> {
    history_json(e)
}

fn history_jump(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.history_jump";
    wave(e)?;
    let last = e.undo_history().len().saturating_add(e.redo_history().len());
    let index = p
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|i| usize::try_from(i).ok())
        .filter(|i| *i <= last)
        .ok_or_else(|| bad(ID, format!("`index` must be a history row, 0..={last}")))?;
    e.history_jump(index);
    history_json(e)
}

fn history_budget(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.history_budget";
    wave(e)?;
    let mb = p
        .get("mb")
        .and_then(Value::as_u64)
        .and_then(|m| u32::try_from(m).ok())
        .filter(|m| (64..=65_536).contains(m))
        .ok_or_else(|| bad(ID, "`mb` must be between 64 and 65536"))?;
    if let Some(w) = e.wave.as_mut() {
        w.history_budget_mb = mb;
    }
    enforce_history_budget(e)?;
    history_json(e)
}

fn view_spectral(e: &mut Engine, p: &Value) -> Result<Value> {
    wave(e)?;
    let on = p.get("on").and_then(Value::as_bool);
    let w = e.wave.as_mut().ok_or_else(|| EngineError::NotFound("no file open".into()))?;
    w.spectral = on.unwrap_or(!w.spectral);
    Ok(json!({"spectral": w.spectral}))
}

fn depth_param(cmd: &str, p: &Value) -> Result<Option<BitDepth>> {
    let Some(v) = p.get("bit_depth").filter(|v| !v.is_null()) else { return Ok(None) };
    let text = v.as_u64().map(|n| n.to_string()).or_else(|| v.as_str().map(str::to_string)).unwrap_or_default();
    match text.trim() {
        "16" => Ok(Some(BitDepth::Int16)),
        "24" => Ok(Some(BitDepth::Int24)),
        "32" => Ok(Some(BitDepth::Int32)),
        "32f" | "float" => Ok(Some(BitDepth::Float32)),
        _ => Err(bad(cmd, format!("`bit_depth` must be 16, 24, 32 or \"32f\", not {v}"))),
    }
}

fn dither_param(cmd: &str, p: &Value) -> Result<Option<DitherProfile>> {
    match str_param(p, "dither") {
        None => Ok(None),
        Some(d) => DitherProfile::parse(d.trim())
            .map(Some)
            .ok_or_else(|| bad(cmd, format!("unknown dither `{d}` (none, tpdf, shaped_light, shaped_strong)"))),
    }
}

/// TPDF at 16 bits; at 24 bits and above the quantization error is below any converter's noise,
/// so exports stay transparent.
fn default_dither(depth: BitDepth) -> DitherProfile {
    if depth == BitDepth::Int16 { DitherProfile::Tpdf } else { DitherProfile::None }
}

fn same_file(a: &str, b: Option<&str>) -> bool {
    b.is_some_and(|b| io::same_file(a, b))
}

/// Render the file, dither it to `depth`, and write it to `path` (see [`io::write_audio_file`]).
/// Returns frames and peak dB.
fn write_file(e: &Engine, path: &str, format: FileFormat, depth: BitDepth, profile: DitherProfile) -> Result<(usize, f64)> {
    let sample_rate = wave(e)?.sample_rate;
    let mut chans = render_file(e)?;
    if depth != BitDepth::Float32 {
        let bits = match depth {
            BitDepth::Int16 => 16,
            BitDepth::Int24 => 24,
            _ => 32,
        };
        for (c, ch) in chans.iter_mut().enumerate() {
            dither::quantize(ch, bits, profile, DITHER_SEED ^ c as u64);
        }
    }
    let peak = chans.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
    let opts = EncodeOptions { format, bit_depth: depth, dither: false, bwf: None };
    let frames = io::write_audio_file(path, &opts, &chans, sample_rate)?;
    let peak_db = if peak > 0.0 { 20.0 * f64::from(peak).log10() } else { -144.0 };
    Ok((frames, peak_db))
}

fn export(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.export";
    let open_path = wave(e)?.path.clone();
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(ID, "`path` required"))?.to_string();
    if same_file(&path, open_path.as_deref()) {
        return Err(bad(ID, "that is the open file: use wave.save to write it back"));
    }
    let format = match str_param(p, "format") {
        Some(f) => match f.trim().to_ascii_lowercase().as_str() {
            "wav" => FileFormat::Wav,
            "aif" | "aiff" => FileFormat::Aiff,
            "flac" => FileFormat::Flac,
            other => return Err(bad(ID, format!("cannot export `{other}`: use wav, aiff or flac"))),
        },
        None => std::path::Path::new(&path)
            .extension()
            .and_then(|x| x.to_str())
            .and_then(soundcraft_audio_io::encode_format_for)
            .filter(|f| matches!(f, FileFormat::Wav | FileFormat::Aiff | FileFormat::Flac))
            .ok_or_else(|| bad(ID, "export writes .wav, .aif/.aiff or .flac (or pass `format`)"))?,
    };
    let depth = depth_param(ID, p)?.unwrap_or(BitDepth::Int24);
    let profile = dither_param(ID, p)?.unwrap_or(default_dither(depth));
    let (frames, peak_db) = write_file(e, &path, format, depth, profile)?;
    Ok(json!({"path": path, "frames": frames, "peak_db": peak_db}))
}

fn save(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.save";
    let w = wave(e)?.clone();
    let path = w.path.clone().ok_or_else(|| bad(ID, "the file has no path yet: use Export"))?;
    let refuse = || bad(ID, format!("cannot write {} {} back: use Export", format_name(w.format), sample_format_name(w.sample_format)));
    let format = match w.format {
        FileFormat::Wav | FileFormat::Aiff | FileFormat::Flac => w.format,
        _ => return Err(refuse()),
    };
    let depth = match w.sample_format {
        SampleFormat::Int16 => BitDepth::Int16,
        SampleFormat::Int24 => BitDepth::Int24,
        SampleFormat::Int32 => BitDepth::Int32,
        SampleFormat::Float32 => BitDepth::Float32,
        _ => return Err(refuse()),
    };
    let profile = dither_param(ID, p)?.unwrap_or(default_dither(depth));
    write_file(e, &path, format, depth, profile)?;
    e.mark_clean();
    Ok(json!({"path": path}))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, SampleFormat};
    use std::path::PathBuf;

    /// A fresh directory for one test's files.
    pub(crate) fn tmp_dir(test: &str) -> PathBuf {
        let d = std::env::temp_dir().join("wavecraft-tests").join(test);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Deterministic noise in ±0.5, `channels` × `frames`.
    pub(crate) fn noise(sample_rate: u32, channels: usize, frames: usize) -> AudioBuffer {
        let mut s = 0x2545_f491u32;
        let ch = (0..channels)
            .map(|_| {
                (0..frames)
                    .map(|_| {
                        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5
                    })
                    .collect()
            })
            .collect();
        AudioBuffer { sample_rate, channels: ch }
    }

    pub(crate) fn write_wav(path: &std::path::Path, buf: &AudioBuffer, depth: BitDepth) {
        let bytes = soundcraft_audio_io::encode(buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf: None }).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn decoded(path: &std::path::Path) -> AudioBuffer {
        soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), Some("wav")).unwrap().1
    }

    /// An engine with `buf` written as a WAV of `depth` and opened.
    pub(crate) fn opened(test: &str, buf: &AudioBuffer, depth: BitDepth) -> (Engine, PathBuf) {
        let dir = tmp_dir(test);
        let path = dir.join("source.wav");
        write_wav(&path, buf, depth);
        let mut e = Engine::default();
        e.execute("wave.open", &json!({"path": path.to_str().unwrap()})).unwrap();
        (e, dir)
    }

    #[test]
    fn open_keeps_rate_channels_and_samples() {
        let (e, dir) = opened("open_keeps", &noise(44_100, 2, 44_100), BitDepth::Int24);
        let info = Engine::default_info(&e);
        assert_eq!((info["sample_rate"].as_u64(), info["channels"].as_u64(), info["frames"].as_u64()), (Some(44_100), Some(2), Some(44_100)));
        assert_eq!(render_file(&e).unwrap(), decoded(&dir.join("source.wav")).channels);
    }

    #[test]
    fn open_mono_stays_mono_and_exports_mono() {
        let (mut e, dir) = opened("open_mono", &noise(48_000, 1, 4_800), BitDepth::Int16);
        let out = dir.join("out.wav");
        e.execute("wave.export", &json!({"path": out.to_str().unwrap(), "bit_depth": 16})).unwrap();
        assert_eq!(decoded(&out).channels.len(), 1);
    }

    #[test]
    fn export_24_bit_matches_the_source() {
        let (mut e, dir) = opened("export_24", &noise(48_000, 2, 9_600), BitDepth::Int24);
        let out = dir.join("out.wav");
        e.execute("wave.export", &json!({"path": out.to_str().unwrap(), "bit_depth": 24, "dither": "none"})).unwrap();
        assert_eq!(decoded(&out), decoded(&dir.join("source.wav")));
        // 24-bit exports default to no dither, so they are transparent too.
        let out2 = dir.join("out2.wav");
        e.execute("wave.export", &json!({"path": out2.to_str().unwrap()})).unwrap();
        assert_eq!(decoded(&out2), decoded(&dir.join("source.wav")));
    }

    #[test]
    fn export_dither_is_reproducible() {
        let (mut e, dir) = opened("export_dither", &noise(48_000, 2, 9_600), BitDepth::Float32);
        let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
        for p in [&a, &b] {
            e.execute("wave.export", &json!({"path": p.to_str().unwrap(), "bit_depth": 16, "dither": "tpdf"})).unwrap();
        }
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    }

    #[test]
    fn export_formats_follow_the_extension() {
        let (mut e, dir) = opened("export_formats", &noise(48_000, 1, 4_800), BitDepth::Int16);
        for (name, fmt) in [("x.aif", FileFormat::Aiff), ("x.flac", FileFormat::Flac), ("x.wav", FileFormat::Wav)] {
            let p = dir.join(name);
            e.execute("wave.export", &json!({"path": p.to_str().unwrap(), "bit_depth": 16})).unwrap();
            let bytes = std::fs::read(&p).unwrap();
            assert_eq!(soundcraft_audio_io::probe(&bytes, None).unwrap().format, fmt, "{name}");
        }
        assert!(e.execute("wave.export", &json!({"path": dir.join("x.mp3").to_str().unwrap()})).is_err());
    }

    #[test]
    fn open_rejects_garbage_empty_and_huge() {
        let (mut e, dir) = opened("open_rejects", &noise(48_000, 1, 4_800), BitDepth::Int16);
        let before = (e.wave.clone(), render_file(&e).unwrap(), e.undo_history().len());
        let garbage = dir.join("garbage.wav");
        std::fs::write(&garbage, b"this is not audio at all").unwrap();
        // A valid WAV header with an empty data chunk.
        let empty = dir.join("empty.wav");
        std::fs::write(&empty, wav_header(1, 48_000, 16, 0)).unwrap();
        for p in [&garbage, &empty, &dir.join("missing.wav")] {
            assert!(e.execute("wave.open", &json!({"path": p.to_str().unwrap()})).is_err(), "{}", p.display());
            assert_eq!((e.wave.clone(), render_file(&e).unwrap(), e.undo_history().len()), before);
        }
    }

    #[test]
    fn limits_refuse_empty_long_and_multichannel() {
        // A truncated WAV header is recovered by the reader, so the limits are checked directly.
        let max = soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES as u64;
        assert!(check_limits("t", 0, 1).is_err());
        assert!(check_limits("t", 48_000, 3).is_err());
        assert!(check_limits("t", max + 1, 1).is_err());
        assert!(check_limits("t", max, 2).is_ok());
        assert!(check_limits("t", max, 3).is_err());
        assert!(check_limits("t", 48_000, 2).is_ok());
    }

    fn wav_header(channels: u16, sr: u32, bits: u16, data_len: u32) -> Vec<u8> {
        let block = channels * bits / 8;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&36u32.wrapping_add(data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&sr.to_le_bytes());
        v.extend_from_slice(&(sr * u32::from(block)).to_le_bytes());
        v.extend_from_slice(&block.to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        v
    }

    #[test]
    fn export_failures_leave_no_file() {
        let (mut e, dir) = opened("export_failures", &noise(48_000, 1, 4_800), BitDepth::Int16);
        assert!(e.execute("wave.export", &json!({"path": "/nonexistent-dir/x.wav"})).is_err());
        assert!(!std::path::Path::new("/nonexistent-dir/x.wav.part").exists());
        let src = dir.join("source.wav");
        let err = e.execute("wave.export", &json!({"path": src.to_str().unwrap()})).unwrap_err().to_string();
        assert!(err.contains("wave.save"), "{err}");
        assert!(std::fs::read_dir(&dir).unwrap().all(|f| !f.unwrap().file_name().to_string_lossy().ends_with(".part")));
    }

    #[test]
    fn save_refuses_mp3_m4a_and_8_bit() {
        let (mut e, dir) = opened("save_refuses", &noise(48_000, 1, 4_800), BitDepth::Int16);
        for (fmt, sf) in
            [(FileFormat::Mp3, SampleFormat::Compressed), (FileFormat::Aac, SampleFormat::Compressed), (FileFormat::Wav, SampleFormat::Int8)]
        {
            if let Some(w) = e.wave.as_mut() {
                w.format = fmt;
                w.sample_format = sf;
            }
            let err = e.execute("wave.save", &json!({})).unwrap_err().to_string();
            assert!(err.contains("use Export"), "{fmt:?}: {err}");
        }
        if let Some(w) = e.wave.as_mut() {
            w.format = FileFormat::Wav;
            w.sample_format = SampleFormat::Int16;
        }
        let original = std::fs::read(dir.join("source.wav")).unwrap();
        e.execute("wave.save", &json!({})).unwrap();
        assert!(!e.is_dirty());
        assert_eq!(decoded(&dir.join("source.wav")), soundcraft_audio_io::decode(&original, Some("wav")).unwrap().1);
    }

    #[test]
    fn select_sets_range_and_channels() {
        let (mut e, _) = opened("select", &noise(48_000, 2, 48_000), BitDepth::Int16);
        e.execute("wave.select", &json!({"start": 4_800, "end": 9_600, "channels": [1]})).unwrap();
        let info = Engine::default_info(&e);
        assert_eq!(info["selection"], json!({"start": 4_800, "end": 9_600}));
        assert_eq!(info["channels_enabled"], json!([false, true]));
        assert!(e.execute("wave.select", &json!({"channels": [2]})).is_err());
        assert!(!e.is_dirty());
        assert!(e.undo_history().is_empty());
    }

    fn rows(e: &mut Engine) -> Vec<(String, String)> {
        let h = e.execute("wave.history", &json!({})).unwrap();
        h["rows"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, r)| {
                assert_eq!(r["index"].as_u64(), Some(i as u64));
                (r["label"].as_str().unwrap().to_string(), r["state"].as_str().unwrap().to_string())
            })
            .collect()
    }

    #[test]
    fn history_rows_mark_current_and_undone() {
        let (mut e, _) = opened("history_rows", &noise(48_000, 1, 48_000), BitDepth::Float32);
        for db in [1.0, 2.0, 3.0] {
            e.execute("wave.gain", &json!({"db": db})).unwrap();
        }
        e.undo();
        let r = rows(&mut e);
        let labels: Vec<&str> = r.iter().map(|(l, _)| l.as_str()).collect();
        let states: Vec<&str> = r.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(labels.len(), 4);
        assert_eq!(labels[0], "Open");
        assert!(labels[1].starts_with("Gain +1.0 dB") && labels[3].starts_with("Gain +3.0 dB"), "{labels:?}");
        assert_eq!(states, ["done", "done", "current", "undone"]);
    }

    #[test]
    fn jump_to_open_restores_the_file() {
        let (mut e, _) = opened("history_jump", &noise(48_000, 1, 48_000), BitDepth::Float32);
        let orig = render_file(&e).unwrap();
        for db in [1.0, 2.0, 3.0] {
            e.execute("wave.gain", &json!({"db": db})).unwrap();
        }
        let edited = render_file(&e).unwrap();
        e.execute("wave.history_jump", &json!({"index": 0})).unwrap();
        assert_eq!(render_file(&e).unwrap(), orig);
        assert_eq!(rows(&mut e)[0].1, "current");
        e.execute("wave.history_jump", &json!({"index": 3})).unwrap();
        assert_eq!(render_file(&e).unwrap(), edited);
    }

    #[test]
    fn jump_out_of_range_and_after_trim() {
        let (mut e, _) = opened("history_trim", &noise(48_000, 2, 48_000 * 60), BitDepth::Float32);
        e.execute("wave.gain", &json!({"db": 1.0})).unwrap();
        let before = render_file(&e).unwrap();
        assert!(e.execute("wave.history_jump", &json!({"index": 99})).is_err());
        assert!(e.execute("wave.history_jump", &json!({})).is_err());
        assert_eq!(render_file(&e).unwrap(), before);
        assert!(e.execute("wave.history_budget", &json!({"mb": 8})).is_err());
        e.execute("wave.history_budget", &json!({"mb": 64})).unwrap();
        for db in [1.0, 1.0, 1.0, 1.0] {
            e.execute("wave.gain", &json!({"db": db})).unwrap();
        }
        let r = rows(&mut e);
        assert!(r.len() < 6, "{r:?}");
        assert!(r[0].0.starts_with("Open (+"), "{r:?}");
        assert!(e.messages.iter().any(|m| m.contains("History trimmed")), "{:?}", e.messages);
        let h = e.execute("wave.history", &json!({})).unwrap();
        // The budget is enforced before the running step is pushed, so it may be exceeded by one
        // step: here one full-file source of 1 min stereo float (23 040 000 bytes).
        assert!(h["bytes"].as_u64().unwrap() <= (64 << 20) + 23_040_000, "{h}");
        e.execute("wave.history_jump", &json!({"index": 0})).unwrap();
        assert_eq!(rows(&mut e)[0].1, "current");
    }

    #[test]
    fn spectral_toggle_is_a_view_command() {
        let (mut e, _) = opened("spectral_toggle", &noise(48_000, 1, 4_800), BitDepth::Int16);
        let n = e.undo_history().len();
        e.execute("wave.view_spectral", &json!({})).unwrap();
        assert_eq!(e.wave.as_ref().map(|w| w.spectral), Some(true));
        e.execute("wave.view_spectral", &json!({"on": false})).unwrap();
        assert_eq!(e.wave.as_ref().map(|w| w.spectral), Some(false));
        assert!(!e.is_dirty());
        assert_eq!(e.undo_history().len(), n);
    }

    #[test]
    fn open_refuses_over_a_multitrack_session() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let dir = tmp_dir("open_over_session");
        let path = dir.join("a.wav");
        write_wav(&path, &noise(48_000, 1, 4_800), BitDepth::Int16);
        let err = e.execute("wave.open", &json!({"path": path.to_str().unwrap()})).unwrap_err().to_string();
        assert!(err.contains("multitrack"), "{err}");
        assert_eq!((e.session().tracks.len(), e.undo_history().len(), e.wave.is_none()), (1, 1, true));
    }

    #[test]
    fn open_needs_discard_when_the_file_has_unsaved_changes() {
        let (mut e, dir) = opened("open_discard", &noise(48_000, 1, 4_800), BitDepth::Int16);
        let other = dir.join("other.wav");
        write_wav(&other, &noise(48_000, 2, 4_800), BitDepth::Int16);
        e.execute("wave.gain", &json!({"db": 1.0})).unwrap();
        let err = e.execute("wave.open", &json!({"path": other.to_str().unwrap()})).unwrap_err().to_string();
        assert!(err.contains("unsaved"), "{err}");
        assert_eq!(e.wave.as_ref().map(|w| w.channels), Some(1));
        e.execute("wave.open", &json!({"path": other.to_str().unwrap(), "discard": true})).unwrap();
        assert_eq!(e.wave.as_ref().map(|w| w.channels), Some(2));
    }

    #[test]
    fn open_waits_for_a_take_being_recorded() {
        let mut e = Engine::default();
        e.transport.recording = true;
        assert!(e.blocked_while_recording("wave.open"));
    }

    #[test]
    fn undo_then_a_new_edit_keeps_the_open_state() {
        // 10 s stereo float: each full-file step holds 3.84 MB. 17 steps fill a 64 MB budget.
        let (mut e, _) = opened("budget_redo", &noise(48_000, 2, 480_000), BitDepth::Float32);
        let orig = render_file(&e).unwrap();
        e.execute("wave.history_budget", &json!({"mb": 64})).unwrap();
        for _ in 0..17 {
            e.execute("wave.gain", &json!({"db": 0.5})).unwrap();
        }
        for _ in 0..15 {
            e.undo();
        }
        e.execute("wave.gain", &json!({"db": -1.0})).unwrap();
        let h = e.execute("wave.history", &json!({})).unwrap();
        assert_eq!(h["rows"][0]["label"], "Open", "{h}");
        e.execute("wave.history_jump", &json!({"index": 0})).unwrap();
        assert_eq!(render_file(&e).unwrap(), orig);
    }

    #[test]
    fn long_histories_keep_the_open_state() {
        // More steps than the engine's default 200-step limit, on a tiny file: the byte budget,
        // not a step count, decides what goes, so row 0 is still the opened file.
        let (mut e, _) = opened("long_history", &noise(48_000, 1, 480), BitDepth::Float32);
        let orig = render_file(&e).unwrap();
        for _ in 0..210 {
            e.execute("wave.gain", &json!({"db": 0.01})).unwrap();
        }
        let h = e.execute("wave.history", &json!({})).unwrap();
        assert_eq!((h["rows"][0]["label"].as_str(), h["rows"].as_array().map(Vec::len)), (Some("Open"), Some(211)));
        e.execute("wave.history_jump", &json!({"index": 0})).unwrap();
        assert_eq!(render_file(&e).unwrap(), orig);
    }

    #[test]
    fn wave_commands_survive_empty_params() {
        for spec in command_specs().iter().filter(|s| s.id.starts_with("wave.")) {
            let mut e = Engine::default();
            assert!(e.execute(spec.id, &json!({})).is_err(), "{} should need an open file", spec.id);
        }
    }

    impl Engine {
        fn default_info(e: &Engine) -> Value {
            info(e).unwrap()
        }
    }
}
