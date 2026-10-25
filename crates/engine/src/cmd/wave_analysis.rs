//! WaveCraft music analysis: tuning, tempo, key, chords, sections, guitar diagrams, lyrics and
//! ChordPro, over the selection or the whole file. Results are cached in the open file (keyed by
//! audio and range: selecting or zooming keeps it) and are never undo steps.

use super::wave::{audio_key, has_file, render_file, wave, wave_range};
use super::*;
use crate::{cmd, io};
use serde_json::json;
use soundcraft_wave_analysis::chordpro::{self, Song};
use soundcraft_wave_analysis::chords::{Chord, ChordSegment, Grid, Quality};
use soundcraft_wave_analysis::chroma::Chroma;
use soundcraft_wave_analysis::key::{self as keys, Key, pitch_name};
use soundcraft_wave_analysis::lyrics::{self, Line, Word};
use soundcraft_wave_analysis::sections::Section;
use soundcraft_wave_analysis::tempo::Tempo;
use soundcraft_wave_analysis::tuning::Tuning;
use soundcraft_wave_analysis::{ANALYSIS_RATE, analysis_signal, chroma, sections, tempo, tuning, voicing};

/// Below this agreement the tuning estimate is not trusted to recentre the chroma.
const TUNING_TRUSTED: f32 = 0.3;
const MAX_LYRICS_BYTES: usize = 1 << 20;
const MAX_WORDS: usize = 100_000;
const NO_WORDS: &str = "transcribe first (wave.transcribe) or pass timed words";

/// A music analysis of part of the file. Times inside are relative to the range start
/// (`offset_s` seconds into the file); the commands report file times.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// The file's [`audio_key`] when analysed: the analysis is stale once the audio differs.
    pub audio: u64,
    pub range: Range,
    pub offset_s: f64,
    pub grid: Grid,
    pub beats_per_bar: Option<u8>,
    pub tuning: Tuning,
    pub tempo: Tempo,
    pub key: Option<Key>,
    pub chords: Vec<ChordSegment>,
    pub sections: Vec<Section>,
    pub chroma: Chroma,
    /// Chords, key and tuning were read from the harmonic stems (else from the mix).
    pub from_stems: bool,
}

/// Words heard with their times (file times), and supplied lyrics aligned to them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lyrics {
    pub heard: Vec<Word>,
    pub lines: Vec<Line>,
    /// Bumped on every change, so views know when to redraw.
    pub version: u64,
    /// The supplied lyrics, re-aligned when new words are heard.
    pub text: Option<String>,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "wave.analyze",
            "Analyze Song",
            [],
            None,
            "{start?, end?, grid?: beat|bar|free (default bar), beats_per_bar?: 2..7} — tuning, tempo with beats and bars, key, chords and sections of the selection (or the whole file), times in seconds; not an undo step",
            has_file,
            analyze
        ),
        cmd!(query "wave.analyze_tuning", "Analyze Tuning", [], None, "{start?, end?} — {a4_hz, cents, confidence}", has_file, analyze_tuning),
        cmd!(
            query "wave.analyze_tempo",
            "Analyze Tempo",
            [],
            None,
            "{start?, end?, beats_per_bar?: 2..7} — {bpm, beats, downbeats, beats_per_bar, confidence} (seconds)",
            has_file,
            analyze_tempo
        ),
        cmd!(query "wave.analyze_key", "Analyze Key", [], None, "{start?, end?} — {key, tonic, mode, confidence, alternatives} (null key for silence)", has_file, analyze_key),
        cmd!(
            query "wave.analyze_chords",
            "Analyze Chords",
            [],
            None,
            "{start?, end?, grid?: beat|bar|free, level?: 1..3 (default 3), capo?: 0..7} — {key, chords: [{start, end, label, root, quality, bass?, confidence}]}; level 1 = major/minor triads, 2 = adds 7, maj7, m7 and sus, 3 = everything; with a capo the labels are the shapes to play; an empty label is no chord",
            has_file,
            analyze_chords
        ),
        cmd!(query "wave.analyze_sections", "Analyze Sections", [], None, "{start?, end?} — {sections: [{start, end, letter, label}]}: intro, verse, chorus, bridge, instrumental, outro, other", has_file, analyze_sections),
        cmd!(view "wave.view_analysis", "Show Analysis Lanes", [], None, "{on?: bool} — show or hide the beat, chord and section lanes (toggles without `on`)", has_file, view_analysis),
        cmd!(query "wave.analysis_info", "Analysis Info", [], None, "{} — {analyzed, stale?}: stale once the audio changed since the analysis", has_file, analysis_info),
        cmd!(
            query "wave.chord_diagrams",
            "Chord Diagrams",
            [],
            None,
            "{level?: 1..3, capo?: \"auto\"|0..7} — {capo, chords: [{label, frets (low E first, -1 muted), fingers, base_fret, barre?, difficulty}]} for the song's chords in order of appearance",
            has_file,
            chord_diagrams
        ),
        cmd!(
            noundo "wave.lyrics_set",
            "Set Lyrics",
            [],
            None,
            "{words?: [{word, start, end}], text?: string, path?: string} — `words` sets the heard (timed) words; `text` or a UTF-8 `path` (≤ 1 MB) aligns supplied lyrics to them word by word; not an undo step",
            has_file,
            lyrics_set
        ),
        cmd!(query "wave.lyrics", "Lyrics", [], None, "{} — {heard, lines: [{words: [{text, start, end}]}]}", has_file, lyrics_get),
        cmd!(
            query "wave.chords_over_lyrics",
            "Chords over Lyrics",
            [],
            None,
            "{start?, end?, level?: 1..3, capo?: \"auto\"|0..7, title?} — {chordpro, txt, capo}: the chords placed on the lyrics (chord lines where nobody sings)",
            has_file,
            chords_over_lyrics
        ),
        cmd!(
            query "wave.chordpro_export",
            "Export ChordPro",
            [],
            None,
            "{path, format?: cho|txt|json (else from the extension), start?, end?, level?, capo?, title?} — write the chords over the lyrics",
            has_file,
            chordpro_export
        ),
    ]
}

/// `grid`, or without it the grid of the analysis already made (bar for a first one).
fn grid_param(cmd: &str, p: &Value, cached: Option<Grid>) -> Result<Grid> {
    match str_param(p, "grid") {
        None => Ok(cached.unwrap_or(Grid::Bar)),
        Some("bar") => Ok(Grid::Bar),
        Some("beat") => Ok(Grid::Beat),
        Some("free") => Ok(Grid::Free),
        Some(other) => Err(bad(cmd, format!("`grid` must be beat, bar or free, not `{other}`"))),
    }
}

fn int_param(cmd: &str, p: &Value, key: &str, range: std::ops::RangeInclusive<i64>) -> Result<Option<i64>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    let n = v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0).map(|f| f as i64));
    match n {
        Some(n) if range.contains(&n) => Ok(Some(n)),
        _ => Err(bad(cmd, format!("`{key}` must be a whole number from {} to {}", range.start(), range.end()))),
    }
}

fn level_param(cmd: &str, p: &Value) -> Result<u8> {
    Ok(int_param(cmd, p, "level", 1..=3)?.unwrap_or(3) as u8)
}

/// `capo`: a fret (0–7), or with `auto` allowed the string "auto" (`None`).
fn capo_param(cmd: &str, p: &Value, auto: bool) -> Result<Option<u8>> {
    if auto && str_param(p, "capo") == Some("auto") {
        return Ok(None);
    }
    if p.get("capo").is_some_and(Value::is_string) {
        return Err(bad(cmd, if auto { "`capo` must be \"auto\" or a fret from 0 to 7" } else { "`capo` must be a fret from 0 to 7" }));
    }
    Ok(Some(int_param(cmd, p, "capo", 0..=7)?.unwrap_or(0) as u8))
}

/// Make sure the open file has an analysis of the command's range at its grid and bar length.
/// Only `wave.analyze` follows the selection: the other commands keep the analysis already made
/// (while the audio is unchanged) unless they name `start`/`end`.
fn ensure(e: &mut Engine, cmd: &str, p: &Value) -> Result<()> {
    let explicit = ["start", "end", "length"].iter().any(|k| p.get(k).is_some());
    let audio = audio_key(e);
    let stems = super::wave_ml::has_harmonic_stems(e);
    let kept = wave(e)?.analysis.as_ref().filter(|a| a.audio == audio && a.from_stems == stems).map(|a| a.range);
    let range = match kept {
        Some(r) if !explicit && cmd != "wave.analyze" => r,
        _ => wave_range(e, cmd, p)?,
    };
    if range.is_empty() {
        return Err(bad(cmd, "the range is empty"));
    }
    let w = wave(e)?;
    let fresh = w.analysis.as_ref().filter(|a| a.audio == audio && a.range == range && a.from_stems == stems);
    let grid = grid_param(cmd, p, fresh.map(|a| a.grid))?;
    let bpb = match int_param(cmd, p, "beats_per_bar", 2..=7)? {
        Some(v) => Some(v as u8),
        None => fresh.and_then(|a| a.beats_per_bar),
    };
    if let Some(a) = fresh
        && a.beats_per_bar == bpb
    {
        if a.grid != grid {
            let chords = soundcraft_wave_analysis::chords::recognize(&a.chroma, &a.tempo, grid);
            let key = a.key.clone().map(|k| soundcraft_wave_analysis::chords::settle_key(k, &chords));
            if let Some(a) = e.wave.as_mut().and_then(|w| w.analysis.as_mut()) {
                a.chords = chords;
                a.grid = grid;
                a.key = key;
            }
        }
        return Ok(());
    }
    let done = new_job(e, range, grid, bpb)?.run();
    if let Some(w) = e.wave.as_mut() {
        w.analysis = Some(done.analysis);
    }
    Ok(())
}

/// An analysis, runnable off the UI thread ([`AnalysisJob::run`], then [`apply_analysis`]).
pub struct AnalysisJob {
    part: Vec<Vec<f32>>,
    /// The harmonic stems over the same range, when current stems exist.
    harmonic: Option<Vec<Vec<f32>>>,
    sample_rate: u32,
    range: Range,
    grid: Grid,
    beats_per_bar: Option<u8>,
    audio: u64,
    path: Option<String>,
}

pub struct AnalysisDone {
    analysis: Analysis,
    path: Option<String>,
}

fn new_job(e: &Engine, range: Range, grid: Grid, beats_per_bar: Option<u8>) -> Result<AnalysisJob> {
    let w = wave(e)?;
    let file = render_file(e)?;
    let (from, to) = (usize::try_from(range.start).unwrap_or(0), usize::try_from(range.end).unwrap_or(0));
    let slice = |chs: &[Vec<f32>]| -> Vec<Vec<f32>> { chs.iter().map(|c| c.get(from..to.min(c.len())).unwrap_or(&[]).to_vec()).collect() };
    let part = slice(&file);
    let harmonic = super::wave_ml::harmonic_stems(e).map(|h| slice(&h));
    Ok(AnalysisJob { part, harmonic, sample_rate: w.sample_rate, range, grid, beats_per_bar, audio: audio_key(e), path: w.path.clone() })
}

/// The job `wave.analyze` would run with `p` (the selection or the whole file).
pub fn analysis_job(e: &Engine, p: &Value) -> Result<AnalysisJob> {
    const ID: &str = "wave.analyze";
    let range = wave_range(e, ID, p)?;
    if range.is_empty() {
        return Err(bad(ID, "the range is empty"));
    }
    let grid = grid_param(ID, p, None)?;
    let bpb = int_param(ID, p, "beats_per_bar", 2..=7)?.map(|v| v as u8);
    new_job(e, range, grid, bpb)
}

impl AnalysisJob {
    pub fn run(self) -> AnalysisDone {
        let sr = self.sample_rate;
        let x = analysis_signal(&self.part, sr);
        // Harmony (tuning, chroma: chords, key) from the harmonic stems when there are some — the
        // voice and the drums blur it — while tempo and sections keep the whole mix.
        let harmonic = self.harmonic.as_ref().map(|h| analysis_signal(h, sr));
        let tonal = harmonic.as_ref().unwrap_or(&x);
        let tuning = tuning::estimate(tonal, ANALYSIS_RATE);
        let cents = if tuning.confidence >= TUNING_TRUSTED { tuning.cents } else { 0.0 };
        let chroma = chroma::compute(tonal, ANALYSIS_RATE, cents);
        let tempo = tempo::analyze(&x, ANALYSIS_RATE, Some(&chroma), self.beats_per_bar);
        let chords = soundcraft_wave_analysis::chords::recognize(&chroma, &tempo, self.grid);
        let key = keys::estimate(&keys::profile(&chroma)).map(|k| soundcraft_wave_analysis::chords::settle_key(k, &chords));
        let timbre = sections::timbre(&x, ANALYSIS_RATE, chroma.hop_s);
        let sections = sections::segment(&chroma, &timbre, &tempo, None);
        let offset_s = self.range.start.max(0) as f64 / f64::from(sr.max(1));
        let analysis = Analysis {
            audio: self.audio,
            range: self.range,
            offset_s,
            grid: self.grid,
            beats_per_bar: self.beats_per_bar,
            tuning,
            tempo,
            key,
            chords,
            sections,
            chroma,
            from_stems: harmonic.is_some(),
        };
        AnalysisDone { analysis, path: self.path }
    }
}

/// Store a finished analysis, unless the file or its audio changed while it ran.
pub fn apply_analysis(e: &mut Engine, done: AnalysisDone) -> Result<Value> {
    let w = wave(e)?;
    if w.path != done.path || audio_key(e) != done.analysis.audio {
        return Err(EngineError::NotFound("discarded: the file changed while the job ran".into()));
    }
    if let Some(w) = e.wave.as_mut() {
        w.analysis = Some(done.analysis);
    }
    analysis_json(e)
}

fn current(e: &Engine) -> Result<&Analysis> {
    wave(e)?.analysis.as_ref().ok_or_else(|| EngineError::Internal("wave".into(), "the analysis is missing".into()))
}

fn ms(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

impl Analysis {
    /// A time inside the analysis as a file time in seconds.
    fn at(&self, t: f32) -> f64 {
        ms(self.offset_s + f64::from(t))
    }
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Maj => "maj",
        Quality::Min => "min",
        Quality::Dom7 => "7",
        Quality::Maj7 => "maj7",
        Quality::Min7 => "m7",
        Quality::Sus2 => "sus2",
        Quality::Sus4 => "sus4",
        Quality::Dim => "dim",
        Quality::Aug => "aug",
    }
}

/// The key the capo shapes are spelled in.
fn shape_key(key: Option<&Key>, capo: u8) -> Option<Key> {
    key.map(|k| Key { tonic: ((i16::from(k.tonic) - i16::from(capo)).rem_euclid(12)) as u8, ..k.clone() })
}

fn short_key(k: &Key) -> String {
    format!("{}{}", pitch_name(k.tonic, Some(k)), if k.minor { "m" } else { "" })
}

/// The analysed chord of a segment at `level`, as the shape to play with `capo`.
fn shaped(seg: &ChordSegment, level: u8, capo: u8) -> Option<Chord> {
    seg.chord.map(|c| c.simplify(level).transpose(-(capo as i8)))
}

fn tuning_json(a: &Analysis) -> Value {
    json!({"a4_hz": ms(f64::from(a.tuning.a4_hz)), "cents": ms(f64::from(a.tuning.cents)), "confidence": ms(f64::from(a.tuning.confidence))})
}

fn tempo_json(a: &Analysis) -> Value {
    json!({
        "bpm": ms(f64::from(a.tempo.bpm)),
        "beats": a.tempo.beats.iter().map(|t| a.at(*t)).collect::<Vec<_>>(),
        "downbeats": a.tempo.downbeats.iter().map(|t| a.at(*t)).collect::<Vec<_>>(),
        "beats_per_bar": a.tempo.beats_per_bar,
        "confidence": ms(f64::from(a.tempo.confidence)),
    })
}

fn key_json(a: &Analysis) -> Value {
    a.key.as_ref().map_or(Value::Null, |k| {
        json!({
            "key": k.name,
            "tonic": pitch_name(k.tonic, Some(k)),
            "mode": if k.minor { "minor" } else { "major" },
            "confidence": ms(f64::from(k.confidence)),
            "alternatives": k.alternatives.iter().map(|(n, c)| json!({"key": n, "confidence": ms(f64::from(*c))})).collect::<Vec<_>>(),
        })
    })
}

fn chords_json(a: &Analysis, level: u8, capo: u8) -> Vec<Value> {
    let key = shape_key(a.key.as_ref(), capo);
    a.chords
        .iter()
        .map(|seg| {
            let c = shaped(seg, level, capo);
            json!({
                "start": a.at(seg.start),
                "end": a.at(seg.end),
                "label": c.map(|c| c.label(key.as_ref())).unwrap_or_default(),
                "root": c.map(|c| pitch_name(c.root, key.as_ref())),
                "quality": c.map(|c| quality_name(c.quality)),
                "bass": c.and_then(|c| c.bass).map(|b| pitch_name(b, key.as_ref())),
                "confidence": ms(f64::from(seg.confidence)),
            })
        })
        .collect()
}

fn sections_json(a: &Analysis) -> Vec<Value> {
    a.sections.iter().map(|s| json!({"start": a.at(s.start), "end": a.at(s.end), "letter": s.letter.to_string(), "label": s.label.name()})).collect()
}

fn analyze(e: &mut Engine, p: &Value) -> Result<Value> {
    ensure(e, "wave.analyze", p)?;
    analysis_json(e)
}

/// Everything `wave.analyze` reports about the current analysis.
fn analysis_json(e: &Engine) -> Result<Value> {
    let a = current(e)?;
    Ok(json!({
        "start": ms(a.offset_s),
        "end": ms(a.range.end as f64 / f64::from(wave(e)?.sample_rate.max(1))),
        "tuning": tuning_json(a),
        "tempo": tempo_json(a),
        "key": key_json(a),
        "chords": chords_json(a, 3, 0),
        "sections": sections_json(a),
        "source": source(a),
        "stale": false,
    }))
}

fn analyze_tuning(e: &mut Engine, p: &Value) -> Result<Value> {
    ensure(e, "wave.analyze_tuning", p)?;
    Ok(tuning_json(current(e)?))
}

fn analyze_tempo(e: &mut Engine, p: &Value) -> Result<Value> {
    ensure(e, "wave.analyze_tempo", p)?;
    Ok(tempo_json(current(e)?))
}

fn analyze_key(e: &mut Engine, p: &Value) -> Result<Value> {
    ensure(e, "wave.analyze_key", p)?;
    let a = current(e)?;
    let mut k = key_json(a);
    if k.is_null() {
        k = json!({"key": null});
    }
    k["source"] = json!(source(a));
    Ok(k)
}

/// Where chords, key and tuning were read: "stems" (the harmonic stems) or "mix".
fn source(a: &Analysis) -> &'static str {
    if a.from_stems { "stems" } else { "mix" }
}

fn analyze_chords(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.analyze_chords";
    let (level, capo) = (level_param(ID, p)?, capo_param(ID, p, false)?.unwrap_or(0));
    ensure(e, ID, p)?;
    let a = current(e)?;
    Ok(json!({"key": a.key.as_ref().map(|k| k.name.clone()), "capo": capo, "chords": chords_json(a, level, capo)}))
}

fn analyze_sections(e: &mut Engine, p: &Value) -> Result<Value> {
    ensure(e, "wave.analyze_sections", p)?;
    Ok(json!({"sections": sections_json(current(e)?)}))
}

fn view_analysis(e: &mut Engine, p: &Value) -> Result<Value> {
    let on = p.get("on").and_then(Value::as_bool);
    let w = e.wave.as_mut().ok_or_else(|| EngineError::NotFound("no file open".into()))?;
    w.show_analysis = on.unwrap_or(!w.show_analysis);
    Ok(json!({"show_analysis": w.show_analysis}))
}

fn analysis_info(e: &mut Engine, _: &Value) -> Result<Value> {
    let audio = audio_key(e);
    let stems = super::wave_ml::has_harmonic_stems(e);
    Ok(match &wave(e)?.analysis {
        Some(a) => json!({"analyzed": true, "stale": a.audio != audio || a.from_stems != stems, "source": source(a)}),
        None => json!({"analyzed": false}),
    })
}

/// The song's distinct chords at `level`, in order of appearance.
fn distinct(a: &Analysis, level: u8) -> Vec<Chord> {
    let mut out: Vec<Chord> = Vec::new();
    for c in a.chords.iter().filter_map(|s| s.chord.map(|c| c.simplify(level))) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

fn resolve_capo(a: &Analysis, level: u8, capo: Option<u8>) -> u8 {
    capo.unwrap_or_else(|| voicing::suggest_capo(&distinct(a, level)).0)
}

fn diagrams(a: &Analysis, level: u8, capo: u8) -> Vec<(String, Option<voicing::Voicing>)> {
    let key = shape_key(a.key.as_ref(), capo);
    distinct(a, level)
        .iter()
        .map(|c| {
            let shape = c.transpose(-(capo as i8));
            (shape.label(key.as_ref()), voicing::voicings(&shape).into_iter().next())
        })
        .collect()
}

fn chord_diagrams(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.chord_diagrams";
    let (level, capo) = (level_param(ID, p)?, capo_param(ID, p, true)?);
    ensure(e, ID, p)?;
    let a = current(e)?;
    let capo = resolve_capo(a, level, capo);
    let chords: Vec<Value> = diagrams(a, level, capo)
        .into_iter()
        .map(|(label, v)| match v {
            Some(v) => json!({"label": label, "frets": v.frets, "fingers": v.fingers, "base_fret": v.base_fret, "barre": v.barre, "difficulty": ms(f64::from(v.difficulty))}),
            None => json!({"label": label, "frets": null}),
        })
        .collect();
    Ok(json!({"capo": capo, "chords": chords}))
}

fn parse_words(cmd: &str, v: &Value) -> Result<Vec<Word>> {
    let arr = v.as_array().ok_or_else(|| bad(cmd, "`words` must be an array of {word, start, end}"))?;
    if arr.len() > MAX_WORDS {
        return Err(bad(cmd, format!("at most {MAX_WORDS} words")));
    }
    arr.iter()
        .map(|w| {
            let text = w.get("word").or_else(|| w.get("text")).and_then(Value::as_str);
            let start = w.get("start").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0);
            let end = w.get("end").and_then(Value::as_f64).filter(|v| v.is_finite());
            match (text, start, end) {
                (Some(t), Some(s), Some(en)) if en >= s => Ok(Word { text: t.to_string(), start: s as f32, end: en as f32 }),
                _ => Err(bad(cmd, "each word needs `word`, `start` ≥ 0 and `end` ≥ `start` (seconds)")),
            }
        })
        .collect()
}

fn lyrics_set(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.lyrics_set";
    let words = p.get("words").map(|v| parse_words(ID, v)).transpose()?;
    let text = match (str_param(p, "text"), str_param(p, "path")) {
        (Some(t), _) => Some(t.to_string()),
        (None, Some(path)) => {
            let bytes = io::read_file(path)?;
            if bytes.len() > MAX_LYRICS_BYTES {
                return Err(bad(ID, "the lyrics file is larger than 1 MB"));
            }
            Some(String::from_utf8(bytes).map_err(|_| bad(ID, format!("{path}: not UTF-8 text")))?)
        }
        (None, None) => None,
    };
    if words.is_none() && text.is_none() {
        return Err(bad(ID, "pass `words`, `text` or `path`"));
    }
    if text.as_ref().is_some_and(|t| t.len() > MAX_LYRICS_BYTES) {
        return Err(bad(ID, "the lyrics are longer than 1 MB"));
    }
    let end_s = super::wave::file_len(e) as f32 / wave(e)?.sample_rate.max(1) as f32;
    let w = e.wave.as_mut().ok_or_else(|| EngineError::NotFound("no file open".into()))?;
    if let Some(words) = words {
        w.lyrics.heard = words;
        w.lyrics.lines.clear();
    }
    if let Some(text) = text {
        if w.lyrics.heard.is_empty() {
            return Err(bad(ID, NO_WORDS));
        }
        w.lyrics.lines = lyrics::align(&text, &w.lyrics.heard, end_s);
        w.lyrics.text = Some(text);
    }
    w.lyrics.version = w.lyrics.version.wrapping_add(1);
    let heard = &w.lyrics.heard;
    let aligned = w.lyrics.lines.iter().flat_map(|l| &l.words).filter(|x| heard.iter().any(|h| h.start == x.start && h.end == x.end)).count();
    Ok(json!({"heard": heard.len(), "lines": w.lyrics.lines.len(), "aligned": aligned}))
}

/// The lyric lines to show: the supplied text aligned, else the heard words in lines.
fn lyric_lines(l: &Lyrics) -> Vec<Line> {
    if l.lines.is_empty() { lyrics::lines_from_words(&l.heard) } else { l.lines.clone() }
}

fn lyrics_get(e: &mut Engine, _: &Value) -> Result<Value> {
    let l = &wave(e)?.lyrics;
    let lines: Vec<Value> = lyric_lines(l)
        .iter()
        .map(|line| json!({"words": line.words.iter().map(|w| json!({"text": w.text, "start": ms(f64::from(w.start)), "end": ms(f64::from(w.end))})).collect::<Vec<_>>()}))
        .collect();
    Ok(json!({"heard": l.heard.len(), "lines": lines}))
}

/// The song to write as ChordPro or text, for the command's params.
fn song(e: &mut Engine, cmd: &str, p: &Value) -> Result<(Song, u8)> {
    let (level, capo) = (level_param(cmd, p)?, capo_param(cmd, p, true)?);
    ensure(e, cmd, p)?;
    let w = wave(e)?;
    let a = current(e)?;
    let capo = resolve_capo(a, level, capo);
    let key = shape_key(a.key.as_ref(), capo);
    let title = str_param(p, "title").map(str::to_string).unwrap_or_else(|| {
        w.path.as_deref().and_then(|p| std::path::Path::new(p).file_stem()).and_then(|s| s.to_str()).unwrap_or("Untitled").to_string()
    });
    let shift = a.offset_s as f32;
    let bpm = (a.tempo.bpm > 0.0).then_some(a.tempo.bpm);
    let song = Song {
        title,
        key: a.key.as_ref().map(short_key),
        bpm,
        capo,
        lines: lyric_lines(&w.lyrics),
        chords: a
            .chords
            .iter()
            .map(|s| (s.start + shift, s.end + shift, shaped(s, level, capo).map(|c| c.label(key.as_ref())).unwrap_or_default()))
            .collect(),
        sections: a.sections.iter().map(|s| Section { start: s.start + shift, end: s.end + shift, ..s.clone() }).collect(),
        defines: diagrams(a, level, capo).into_iter().filter_map(|(l, v)| v.map(|v| (l, v))).collect(),
        bar_s: bpm.map_or(2.0, |b| 60.0 / b * f32::from(a.tempo.beats_per_bar.max(1))),
    };
    Ok((song, capo))
}

fn chords_over_lyrics(e: &mut Engine, p: &Value) -> Result<Value> {
    let (song, capo) = song(e, "wave.chords_over_lyrics", p)?;
    Ok(json!({"chordpro": chordpro::render_cho(&song), "txt": chordpro::render_txt(&song), "capo": capo}))
}

fn chordpro_export(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.chordpro_export";
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(ID, "`path` required"))?.to_string();
    let ext = std::path::Path::new(&path).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let format = match str_param(p, "format").map(str::to_ascii_lowercase).or(ext).as_deref() {
        Some("cho" | "chopro" | "chordpro" | "crd" | "pro") => "cho",
        Some("txt") => "txt",
        Some("json") => "json",
        other => return Err(bad(ID, format!("format must be cho, txt or json, not {}", other.unwrap_or("none")))),
    };
    let (song, capo) = song(e, ID, p)?;
    let text = match format {
        "cho" => chordpro::render_cho(&song),
        "txt" => chordpro::render_txt(&song),
        _ => {
            let level = level_param(ID, p)?;
            let lines = lyrics_get(e, &json!({}))?["lines"].clone();
            let a = current(e)?;
            let doc = json!({
                "title": song.title,
                "key": key_json(a),
                "tuning": tuning_json(a),
                "tempo": tempo_json(a),
                "capo": capo,
                "level": level,
                "chords": chords_json(a, level, capo),
                "sections": sections_json(a),
                "lyrics": lines,
                "chordpro": chordpro::render_cho(&song),
            });
            serde_json::to_string_pretty(&doc).map_err(|err| EngineError::Internal(ID.into(), err.to_string()))?
        }
    };
    io::write_file(&path, text.as_bytes())?;
    Ok(json!({"path": path, "format": format, "capo": capo}))
}

#[cfg(test)]
mod tests {
    use super::super::wave::tests::{tmp_dir, write_wav};
    use super::*;
    use serde_json::json;
    use soundcraft_audio_io::{AudioBuffer, BitDepth};
    use soundcraft_wave_analysis::chords::{Chord, Quality};
    use soundcraft_wave_analysis::sections::SectionLabel;
    use soundcraft_wave_analysis::testsignal::{SongSpec, acceptance_song, song};

    fn run(e: &mut Engine, id: &str, p: Value) -> Value {
        e.execute(id, &p).unwrap_or_else(|err| panic!("{id}: {err}"))
    }

    /// A 40 s song in A minor (intro, verse, bridge) at 96 BPM, A4 = 436 Hz.
    fn short_song() -> SongSpec {
        let c = |root, quality| Chord { root, quality, bass: None };
        let (am, f, cm, g, dm, e) =
            (c(9, Quality::Min), c(5, Quality::Maj), c(0, Quality::Maj), c(7, Quality::Maj), c(2, Quality::Min), c(4, Quality::Maj));
        SongSpec {
            sections: vec![
                (SectionLabel::Intro, vec![am, am]),
                (SectionLabel::Verse, vec![am, f, cm, g, am, f, cm, g]),
                (SectionLabel::Bridge, vec![dm, e, am, dm, e, e]),
            ],
            ..acceptance_song()
        }
    }

    fn open_song(test: &str, spec: &SongSpec, rate: u32) -> Engine {
        let (channels, _) = song(spec);
        let channels = if rate == spec.sample_rate {
            channels
        } else {
            channels.iter().map(|c| soundcraft_wave_analysis::resample(c, spec.sample_rate, rate)).collect()
        };
        let path = tmp_dir(test).join("song.wav");
        write_wav(&path, &AudioBuffer { sample_rate: rate, channels }, BitDepth::Float32);
        let mut e = Engine::default();
        run(&mut e, "wave.open", json!({"path": path.to_str().unwrap()}));
        e
    }

    #[test]
    fn analyze_finds_tempo_key_chords() {
        let mut e = open_song("analyze_finds", &acceptance_song(), 48_000);
        let a = run(&mut e, "wave.analyze", json!({}));
        let bpm = a["tempo"]["bpm"].as_f64().unwrap();
        assert!((bpm - 96.0).abs() <= 2.0, "bpm {bpm}");
        assert_eq!(a["key"]["key"], "A minor");
        assert!(a["chords"].as_array().unwrap().iter().any(|c| c["label"] == "Am"));
        assert_eq!(a["stale"], false);
        assert!((a["tuning"]["a4_hz"].as_f64().unwrap() - 436.0).abs() <= 1.0);
        assert!(a["sections"].as_array().unwrap().iter().any(|s| s["label"] == "chorus"));
    }

    #[test]
    fn analysis_is_not_an_undo_step_and_goes_stale() {
        let mut e = open_song("analysis_stale", &short_song(), 48_000);
        let n = e.undo_history().len();
        run(&mut e, "wave.analyze", json!({}));
        assert_eq!(e.undo_history().len(), n);
        assert_eq!(run(&mut e, "wave.analysis_info", json!({}))["stale"], false);
        run(&mut e, "wave.gain", json!({"db": -1}));
        let info = run(&mut e, "wave.analysis_info", json!({}));
        assert_eq!((info["analyzed"].as_bool(), info["stale"].as_bool()), (Some(true), Some(true)));
    }

    #[test]
    fn selecting_keeps_the_analysis_fresh() {
        let mut e = open_song("analysis_select_fresh", &short_song(), 48_000);
        run(&mut e, "wave.analyze", json!({}));
        run(&mut e, "wave.select", json!({"start": 44_100, "end": 88_200}));
        assert_eq!(run(&mut e, "wave.analysis_info", json!({}))["stale"], false);
        // An export with the selection active still writes the whole analysed song.
        let col = run(&mut e, "wave.chords_over_lyrics", json!({}));
        assert!(col["chordpro"].as_str().unwrap().matches('[').count() > 8, "{}", col["chordpro"]);
        run(&mut e, "wave.gain", json!({"db": -1}));
        assert_eq!(run(&mut e, "wave.analysis_info", json!({}))["stale"], true);
        run(&mut e, "edit.undo", json!({}));
        assert_eq!(run(&mut e, "wave.analysis_info", json!({}))["stale"], false);
    }

    #[test]
    fn analysis_job_runs_apart_and_refuses_stale_results() {
        let mut e = open_song("analysis_job", &short_song(), 48_000);
        let job = analysis_job(&e, &json!({})).unwrap();
        let done = job.run();
        let r = apply_analysis(&mut e, done).unwrap();
        assert!((r["tempo"]["bpm"].as_f64().unwrap() - 96.0).abs() <= 2.0);
        assert_eq!(run(&mut e, "wave.analysis_info", json!({}))["stale"], false);
        let stale = analysis_job(&e, &json!({})).unwrap().run();
        run(&mut e, "wave.gain", json!({"db": -1}));
        assert!(apply_analysis(&mut e, stale).is_err());
    }

    #[test]
    fn queries_keep_the_analysed_grid() {
        let mut e = open_song("analysis_keeps_grid", &short_song(), 48_000);
        let beat = run(&mut e, "wave.analyze", json!({"grid": "beat"}))["chords"].as_array().unwrap().len();
        run(&mut e, "wave.analyze_tempo", json!({}));
        let after = run(&mut e, "wave.analyze_chords", json!({}))["chords"].as_array().unwrap().len();
        assert_eq!(after, beat);
        assert_eq!(e.wave.as_ref().and_then(|w| w.analysis.as_ref()).map(|a| a.grid), Some(Grid::Beat));
    }

    #[test]
    fn chords_levels_and_capo() {
        let mut e = open_song("chords_levels", &short_song(), 48_000);
        let level1 = run(&mut e, "wave.analyze_chords", json!({"grid": "bar", "level": 1}));
        for c in level1["chords"].as_array().unwrap() {
            let l = c["label"].as_str().unwrap();
            assert!(!["7", "maj7", "sus", "dim", "aug", "/"].iter().any(|s| l.contains(s)), "{l}");
        }
        let full = run(&mut e, "wave.analyze_chords", json!({"grid": "bar"}));
        let capo = run(&mut e, "wave.analyze_chords", json!({"grid": "bar", "capo": 2}));
        let first =
            |v: &Value| Chord::parse(v["chords"].as_array().unwrap().iter().find(|c| c["label"] != "").unwrap()["label"].as_str().unwrap()).unwrap();
        assert_eq!(first(&capo), first(&full).transpose(-2));
        let diagrams = run(&mut e, "wave.chord_diagrams", json!({"capo": "auto"}));
        assert!(diagrams["capo"].as_u64().is_some());
        assert!(diagrams["chords"].as_array().unwrap().iter().all(|c| c["frets"].as_array().map(Vec::len) == Some(6)));
    }

    #[test]
    fn lyrics_words_then_text() {
        let mut e = open_song("lyrics_words", &short_song(), 48_000);
        let err = e.execute("wave.lyrics_set", &json!({"text": "chiusa vive"})).unwrap_err().to_string();
        assert!(err.contains("transcribe first"), "{err}");
        let words = json!([{"word": "chiusa", "start": 5.2, "end": 5.6}, {"word": "vivi", "start": 5.7, "end": 6.0}, {"word": "dentro", "start": 8.0, "end": 8.4}]);
        run(&mut e, "wave.lyrics_set", json!({"words": words}));
        let r = run(&mut e, "wave.lyrics_set", json!({"text": "Chiusa vive\ndentro"}));
        assert_eq!((r["lines"].as_u64(), r["aligned"].as_u64()), (Some(2), Some(3)));
        let l = run(&mut e, "wave.lyrics", json!({}));
        assert_eq!(l["lines"][0]["words"][1]["text"], "vive");
        assert!((l["lines"][0]["words"][1]["start"].as_f64().unwrap() - 5.7).abs() < 1e-3);
        let col = run(&mut e, "wave.chords_over_lyrics", json!({"title": "Prova"}));
        let cho = col["chordpro"].as_str().unwrap();
        assert!(cho.contains("{title: Prova}") && cho.contains("Chiusa"), "{cho}");
    }

    #[test]
    fn chordpro_export_writes_three_formats() {
        let mut e = open_song("chordpro_export", &short_song(), 48_000);
        let dir = tmp_dir("chordpro_export_out");
        for ext in ["cho", "txt", "json"] {
            let path = dir.join(format!("song.{ext}"));
            run(&mut e, "wave.chordpro_export", json!({"path": path.to_str().unwrap()}));
            let text = std::fs::read_to_string(&path).unwrap();
            match ext {
                "cho" => assert!(text.contains("{key: Am}") && text.contains("[Am]"), "{text}"),
                "txt" => assert!(text.contains("Am")),
                _ => assert!(serde_json::from_str::<Value>(&text).unwrap()["chords"].is_array()),
            }
        }
    }

    #[test]
    fn analysis_lanes_toggle_is_a_view_command() {
        let mut e = open_song("analysis_lanes_toggle", &short_song(), 48_000);
        assert_eq!(e.wave.as_ref().map(|w| w.show_analysis), Some(true));
        let n = e.undo_history().len();
        assert_eq!(run(&mut e, "wave.view_analysis", json!({}))["show_analysis"], false);
        assert_eq!(run(&mut e, "wave.view_analysis", json!({"on": true}))["show_analysis"], true);
        assert_eq!(e.undo_history().len(), n);
    }

    #[test]
    fn hostile_params_refused() {
        let mut e = open_song("hostile_analysis", &short_song(), 48_000);
        for (id, p) in [
            ("wave.analyze", json!({"grid": "x"})),
            ("wave.analyze_chords", json!({"level": 9})),
            ("wave.analyze_chords", json!({"capo": 8})),
            ("wave.analyze_tempo", json!({"beats_per_bar": 1})),
            ("wave.chord_diagrams", json!({"capo": "sometimes"})),
            ("wave.lyrics_set", json!({"words": [{"word": "x", "start": "soon"}]})),
            ("wave.chordpro_export", json!({"path": "/tmp/x.docx"})),
        ] {
            assert!(e.execute(id, &p).is_err(), "{id} {p} accepted");
        }
        assert!(Engine::default().execute("wave.analyze", &json!({})).is_err());
    }

    #[test]
    fn silence_short_and_odd_rates() {
        let path = tmp_dir("analysis_silence").join("silence.wav");
        write_wav(&path, &AudioBuffer { sample_rate: 44_100, channels: vec![vec![0.0; 3 * 44_100]] }, BitDepth::Int16);
        let mut e = Engine::default();
        run(&mut e, "wave.open", json!({"path": path.to_str().unwrap()}));
        let a = run(&mut e, "wave.analyze", json!({}));
        assert_eq!(a["tempo"]["bpm"].as_f64(), Some(0.0));
        assert!(a["chords"].as_array().unwrap().iter().all(|c| c["label"] == ""));
        assert!(a["key"].is_null());
        let mut e = open_song("analysis_short", &short_song(), 48_000);
        run(&mut e, "wave.analyze", json!({"start": 0, "end": 24_000}));
        for rate in [8_000, 96_000] {
            let mut e = open_song(&format!("analysis_rate_{rate}"), &short_song(), rate);
            let t = run(&mut e, "wave.analyze_tempo", json!({}));
            let bpm = t["bpm"].as_f64().unwrap();
            assert!((bpm - 96.0).abs() <= 2.0, "{rate} Hz: bpm {bpm}");
            assert_eq!(run(&mut e, "wave.analyze_key", json!({}))["key"], "A minor", "{rate} Hz");
        }
    }
}
