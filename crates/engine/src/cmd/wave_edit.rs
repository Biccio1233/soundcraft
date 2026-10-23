//! WaveCraft's destructive edits: each one changes the file's audio and is one undo step.

use super::wave::{file_len, has_file, info, wave, wave_range, wave_track};
use super::*;
use crate::{cmd, edit, io};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_dsp::offline::{self, FadeShape};
use soundcraft_model::{Clip, ClipContent, Session};

/// Audio copied by `wave.copy`/`wave.cut`: samples, not clips, so it survives opening another file.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveClip {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

/// Longest silence `wave.insert_silence` inserts at once.
const MAX_SILENCE_SECONDS: f64 = 3600.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "wave.cut",
            "Cut",
            [],
            None,
            "{start?, end?} — remove the selection (keeping it on the clipboard); later audio moves left",
            has_file,
            cut
        ),
        cmd!(noundo "wave.copy", "Copy", [], None, "{start?, end?} — copy the selection's audio to the clipboard", has_file, copy),
        cmd!(
            "wave.paste",
            "Paste",
            [],
            None,
            "{at?} — insert the clipboard at `at` (default: the selection start, replacing a non-empty selection); later audio moves right",
            has_file,
            paste
        ),
        cmd!("wave.delete", "Delete", [], None, "{start?, end?} — remove the selection; later audio moves left", has_file, delete),
        cmd!("wave.insert_silence", "Insert Silence", [], None, "{at?, seconds} — insert silence; later audio moves right", has_file, insert_silence),
        cmd!("wave.silence", "Silence", [], None, "{start?, end?} — replace the selection with silence", has_file, silence),
        cmd!(
            "wave.fade_in",
            "Fade In",
            [],
            None,
            "{start?, end?, shape?: linear|equal_power|s_curve|exponential|logarithmic} — fade the selection in",
            has_file,
            fade_in
        ),
        cmd!("wave.fade_out", "Fade Out", [], None, "{start?, end?, shape?} — fade the selection out", has_file, fade_out),
        cmd!("wave.gain", "Gain", [], None, "{db: -96..48, start?, end?} — change the selection's level", has_file, gain),
        cmd!(
            "wave.normalize",
            "Normalize",
            [],
            None,
            "{db?: peak target, -60..0 (default -0.1), start?, end?} — bring the selection's peak to `db`",
            has_file,
            normalize
        ),
        cmd!("wave.invert", "Invert", [], None, "{start?, end?} — flip the selection's polarity", has_file, invert),
        cmd!("wave.reverse", "Reverse", [], None, "{start?, end?} — play the selection backwards", has_file, reverse),
    ]
}

/// `m:ss.mmm` of a position at `sample_rate`.
fn clock(pos: Samples, sample_rate: u32) -> String {
    let secs = pos.max(0) as f64 / f64::from(sample_rate.max(1));
    let m = (secs / 60.0).floor();
    format!("{m}:{:06.3}", secs - m * 60.0)
}

/// An undo label naming the operation and its range, e.g. `Cut 0:00.000–0:02.000`.
pub fn label_range(e: &Engine, verb: &str, r: Range) -> String {
    let sr = wave(e).map_or(48_000, |w| w.sample_rate);
    format!("{verb} {}–{}", clock(r.start, sr), clock(r.end, sr))
}

/// Remove pool sources that no clip of the document uses (older snapshots keep their own).
pub fn prune_pool(s: &mut Session) {
    let used: Vec<_> = s
        .tracks
        .iter()
        .flat_map(|t| t.clips().iter())
        .filter_map(|c| match c.content {
            ClipContent::Audio { source, .. } => Some(source),
            _ => None,
        })
        .collect();
    let stale: Vec<_> = s.pool.ids().filter(|id| !used.contains(id)).collect();
    for id in stale {
        s.pool.remove(id);
    }
}

/// End every edit: tidy the pool, name the undo step, and keep the history inside its budget.
fn finish_edit(e: &mut Engine, label: String) -> Result<Value> {
    prune_pool(e.session_mut());
    e.set_undo_label(label);
    super::wave::enforce_history_budget(e)?;
    info(e)
}

fn render_range(e: &Engine, r: Range) -> Result<Vec<Vec<f32>>> {
    let t = wave_track(e)?;
    Ok(soundcraft_mix::render_clips(e.session(), t, r))
}

/// A new source holding `channels`, as a clip of the document placed at `at`.
fn place_audio(e: &mut Engine, channels: Vec<Vec<f32>>, at: Samples) -> Result<Samples> {
    let t = wave_track(e)?;
    let sr = wave(e)?.sample_rate;
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0) as Samples;
    if frames == 0 {
        return Ok(0);
    }
    let s = e.session_mut();
    let name = s.track(t).map_or_else(|| "Audio".to_string(), |tr| tr.name.clone());
    let src = io::add_source(s, &name, AudioBuffer { sample_rate: sr, channels }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    edit::place_clip(s, t, Clip::audio(id, &name, src, 0, at, frames));
    Ok(frames)
}

/// Replace `r` with `samples` of the same length (the clips inside are separated at its edges).
pub fn replace_range(e: &mut Engine, r: Range, samples: Vec<Vec<f32>>, label: String) -> Result<Value> {
    let t = wave_track(e)?;
    let s = e.session_mut();
    edit::separate_at(s, t, r.start);
    edit::separate_at(s, t, r.end);
    edit::clear_range(s, t, r, false);
    place_audio(e, samples, r.start)?;
    finish_edit(e, label)
}

/// The shared path of every in-place process: render the range, run `f` on the enabled channels,
/// put the result back as one undo step.
fn process_range(e: &mut Engine, cmd: &str, p: &Value, verb: &str, f: impl FnOnce(&mut Vec<Vec<f32>>) -> Result<()>) -> Result<Value> {
    let r = wave_range(e, cmd, p)?;
    if r.is_empty() {
        return Err(bad(cmd, "select a range first"));
    }
    let enabled = wave(e)?.channels_enabled.clone();
    let mut all = render_range(e, r)?;
    let mut picked: Vec<Vec<f32>> = all.iter().zip(&enabled).filter(|(_, on)| **on).map(|(c, _)| c.clone()).collect();
    f(&mut picked)?;
    let mut done = picked.into_iter();
    for (c, on) in all.iter_mut().zip(&enabled) {
        if *on && let Some(new) = done.next() {
            *c = new;
        }
    }
    let label = label_range(e, verb, r);
    replace_range(e, r, all, label)
}

fn selected_range(e: &Engine, cmd: &str, p: &Value) -> Result<Range> {
    let r = wave_range(e, cmd, p)?;
    if r.is_empty() { Err(bad(cmd, "select a range first")) } else { Ok(r) }
}

fn copy_to_clipboard(e: &mut Engine, r: Range) -> Result<()> {
    let channels = render_range(e, r)?;
    let sample_rate = wave(e)?.sample_rate;
    e.wave_clipboard = Some(WaveClip { sample_rate, channels });
    Ok(())
}

/// Remove `r`, moving later audio left.
fn remove_range(e: &mut Engine, r: Range) -> Result<()> {
    let t = wave_track(e)?;
    let s = e.session_mut();
    edit::separate_at(s, t, r.start);
    edit::separate_at(s, t, r.end);
    edit::clear_range(s, t, r, true);
    s.edit.selection = Range::point(r.start);
    Ok(())
}

fn cut(e: &mut Engine, p: &Value) -> Result<Value> {
    let r = selected_range(e, "wave.cut", p)?;
    copy_to_clipboard(e, r)?;
    remove_range(e, r)?;
    let label = label_range(e, "Cut", r);
    finish_edit(e, label)
}

fn copy(e: &mut Engine, p: &Value) -> Result<Value> {
    let r = selected_range(e, "wave.copy", p)?;
    copy_to_clipboard(e, r)?;
    info(e)
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let r = selected_range(e, "wave.delete", p)?;
    remove_range(e, r)?;
    let label = label_range(e, "Delete", r);
    finish_edit(e, label)
}

/// Insert `channels` at `at`, moving later audio right; the inserted audio becomes the selection.
fn insert_audio(e: &mut Engine, cmd: &str, at: Samples, channels: Vec<Vec<f32>>, verb: &str) -> Result<Value> {
    let t = wave_track(e)?;
    let len = channels.iter().map(Vec::len).min().unwrap_or(0);
    let total = (file_len(e).max(0) as u64).saturating_add(len as u64);
    if total > soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES as u64 {
        return Err(EngineError::Unsupported(format!("files longer than {} frames", soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES)));
    }
    let len = len as Samples;
    let s = e.session_mut();
    edit::separate_at(s, t, at);
    edit::make_room(s, t, at, len).map_err(|err| bad(cmd, err))?;
    place_audio(e, channels, at)?;
    let r = Range::new(at, at.saturating_add(len));
    let s = e.session_mut();
    s.edit.selection = r;
    let label = label_range(e, verb, r);
    finish_edit(e, label)
}

fn paste(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.paste";
    let w = wave(e)?.clone();
    let clip = e.wave_clipboard.clone().ok_or_else(|| bad(ID, "the clipboard is empty: copy or cut first"))?;
    if clip.sample_rate != w.sample_rate {
        return Err(bad(ID, format!("the clipboard holds {} Hz audio and the file is {} Hz", clip.sample_rate, w.sample_rate)));
    }
    if clip.channels.len() != w.channels {
        return Err(bad(ID, format!("the clipboard holds {} channel(s) and the file has {}", clip.channels.len(), w.channels)));
    }
    let at = match position_param(e, ID, p, "at")? {
        Some(at) => at.clamp(0, file_len(e)),
        None => {
            // A selection is replaced; an empty one is the insertion point.
            let len = file_len(e);
            let s = e.session().edit.selection;
            let sel = Range::new(s.start.clamp(0, len), s.end.clamp(0, len));
            if !sel.is_empty() {
                remove_range(e, sel)?;
            }
            sel.start
        }
    };
    insert_audio(e, ID, at, clip.channels, "Paste")
}

fn insert_silence(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.insert_silence";
    let w = wave(e)?.clone();
    let secs = p
        .get("seconds")
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite() && *s > 0.0 && *s <= MAX_SILENCE_SECONDS)
        .ok_or_else(|| bad(ID, format!("`seconds` must be between 0 and {MAX_SILENCE_SECONDS}")))?;
    let at = position_param(e, ID, p, "at")?.unwrap_or(e.session().edit.selection.start).clamp(0, file_len(e));
    let frames = (secs * f64::from(w.sample_rate)).round() as usize;
    insert_audio(e, ID, at, vec![vec![0.0; frames]; w.channels], "Insert Silence")
}

fn silence(e: &mut Engine, p: &Value) -> Result<Value> {
    process_range(e, "wave.silence", p, "Silence", |ch| {
        for c in ch.iter_mut() {
            c.fill(0.0);
        }
        Ok(())
    })
}

fn shape_param(cmd: &str, p: &Value) -> Result<FadeShape> {
    match str_param(p, "shape").map(str::trim) {
        None | Some("") => Ok(FadeShape::EqualPower),
        Some("linear") => Ok(FadeShape::Linear),
        Some("equal_power") => Ok(FadeShape::EqualPower),
        Some("s_curve") => Ok(FadeShape::SCurve),
        Some("exponential") => Ok(FadeShape::Exponential),
        Some("logarithmic") => Ok(FadeShape::Logarithmic),
        Some(other) => Err(bad(cmd, format!("unknown shape `{other}` (linear, equal_power, s_curve, exponential, logarithmic)"))),
    }
}

fn fade_in(e: &mut Engine, p: &Value) -> Result<Value> {
    let shape = shape_param("wave.fade_in", p)?;
    process_range(e, "wave.fade_in", p, "Fade In", |ch| {
        let len = ch.first().map_or(0, Vec::len);
        offline::fade(ch, len, 0, shape);
        Ok(())
    })
}

fn fade_out(e: &mut Engine, p: &Value) -> Result<Value> {
    let shape = shape_param("wave.fade_out", p)?;
    process_range(e, "wave.fade_out", p, "Fade Out", |ch| {
        let len = ch.first().map_or(0, Vec::len);
        offline::fade(ch, 0, len, shape);
        Ok(())
    })
}

fn db_param(cmd: &str, p: &Value, key: &str, range: std::ops::RangeInclusive<f64>) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|d| d.is_finite() && range.contains(d))
            .map(Some)
            .ok_or_else(|| bad(cmd, format!("`{key}` must be a number of dB in {}..{}", range.start(), range.end()))),
    }
}

fn gain(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.gain";
    let db = db_param(ID, p, "db", -96.0..=48.0)?.ok_or_else(|| bad(ID, "`db` required"))?;
    process_range(e, ID, p, &format!("Gain {db:+.1} dB"), |ch| {
        offline::gain(ch, db as f32);
        Ok(())
    })
}

fn normalize(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.normalize";
    let db = db_param(ID, p, "db", -60.0..=0.0)?.unwrap_or(-0.1);
    process_range(e, ID, p, &format!("Normalize {db:.1} dB"), |ch| {
        offline::normalize(ch, db as f32, false);
        Ok(())
    })
}

fn invert(e: &mut Engine, p: &Value) -> Result<Value> {
    process_range(e, "wave.invert", p, "Invert", |ch| {
        offline::invert(ch);
        Ok(())
    })
}

fn reverse(e: &mut Engine, p: &Value) -> Result<Value> {
    process_range(e, "wave.reverse", p, "Reverse", |ch| {
        offline::reverse(ch);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::super::wave::render_file;
    use super::super::wave::tests::{noise, opened, tmp_dir, write_wav};
    use super::*;
    use serde_json::json;
    use soundcraft_audio_io::{AudioBuffer, BitDepth};
    use soundcraft_model::ClipContent;

    /// A ramp (sample i = i / 200 000) on `channels` channels, 3 s at 48 kHz, opened as a float WAV.
    fn ramp(test: &str, channels: usize) -> Engine {
        let ch: Vec<f32> = (0..144_000).map(|i| i as f32 / 200_000.0).collect();
        opened(test, &AudioBuffer { sample_rate: 48_000, channels: vec![ch; channels] }, BitDepth::Float32).0
    }

    fn run(e: &mut Engine, id: &str, p: Value) -> Value {
        e.execute(id, &p).unwrap_or_else(|err| panic!("{id}: {err}"))
    }

    #[test]
    fn cut_closes_the_gap_and_is_one_step() {
        let mut e = ramp("cut", 1);
        let orig = render_file(&e).unwrap();
        let n0 = e.undo_history().len();
        run(&mut e, "wave.cut", json!({"start": 0, "end": 96_000}));
        let now = render_file(&e).unwrap();
        assert_eq!(now[0].len(), 48_000);
        assert_eq!(now[0][0], orig[0][96_000]);
        assert_eq!(e.undo_history().len(), n0 + 1);
        assert_eq!(e.undo_label(), Some("Cut 0:00.000–0:02.000"));
        e.undo();
        assert_eq!(render_file(&e).unwrap(), orig);
    }

    #[test]
    fn paste_inserts_and_shifts() {
        let mut e = ramp("paste", 1);
        let orig = render_file(&e).unwrap();
        run(&mut e, "wave.copy", json!({"start": 0, "end": 48_000}));
        run(&mut e, "wave.paste", json!({"at": 96_000}));
        let now = render_file(&e).unwrap();
        assert_eq!(now[0].len(), 192_000);
        assert_eq!(now[0][96_000..144_000], orig[0][0..48_000]);
        assert_eq!(now[0][144_000..], orig[0][96_000..]);
        assert_eq!(now[0][..96_000], orig[0][..96_000]);
    }

    #[test]
    fn paste_replaces_a_selection_or_inserts_at_the_cursor() {
        let mut e = ramp("paste_sel", 1);
        let orig = render_file(&e).unwrap();
        run(&mut e, "wave.copy", json!({"start": 0, "end": 1_000}));
        run(&mut e, "wave.select", json!({"start": 10_000, "end": 20_000}));
        run(&mut e, "wave.paste", json!({}));
        let now = render_file(&e).unwrap();
        assert_eq!(now[0].len(), 144_000 - 10_000 + 1_000);
        assert_eq!(now[0][10_000..11_000], orig[0][0..1_000]);
        assert_eq!(now[0][11_000], orig[0][20_000]);
        run(&mut e, "wave.select", json!({"start": 5, "end": 5}));
        run(&mut e, "wave.paste", json!({}));
        assert_eq!(render_file(&e).unwrap()[0][5..1_005], orig[0][0..1_000]);
    }

    #[test]
    fn delete_and_insert_silence_change_the_length() {
        let mut e = ramp("delete_insert", 1);
        run(&mut e, "wave.delete", json!({"start": 0, "end": 48_000}));
        assert_eq!(render_file(&e).unwrap()[0].len(), 96_000);
        run(&mut e, "wave.insert_silence", json!({"at": 0, "seconds": 0.5}));
        let now = render_file(&e).unwrap();
        assert_eq!(now[0].len(), 120_000);
        assert!(now[0][..24_000].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn gain_and_normalize_touch_only_the_range() {
        let mut e = ramp("gain", 1);
        let orig = render_file(&e).unwrap();
        run(&mut e, "wave.gain", json!({"db": 6.0206, "start": 48_000, "end": 96_000}));
        let now = render_file(&e).unwrap();
        for i in (48_000..96_000).step_by(997) {
            assert!((now[0][i] - 2.0 * orig[0][i]).abs() <= 1e-5 * orig[0][i].abs().max(1e-3), "{i}");
        }
        assert_eq!(now[0][..48_000], orig[0][..48_000]);
        assert_eq!(now[0][96_000..], orig[0][96_000..]);
        run(&mut e, "wave.normalize", json!({"db": -0.1}));
        let peak = render_file(&e).unwrap()[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((20.0 * peak.log10() + 0.1).abs() < 0.01, "{peak}");
    }

    #[test]
    fn channel_mask_limits_processing() {
        let mut e = ramp("mask", 2);
        let orig = render_file(&e).unwrap();
        run(&mut e, "wave.select", json!({"channels": [0]}));
        run(&mut e, "wave.invert", json!({}));
        let now = render_file(&e).unwrap();
        assert!(now[0].iter().zip(&orig[0]).all(|(a, b)| *a == -*b));
        assert_eq!(now[1], orig[1]);
    }

    #[test]
    fn fades_follow_their_shape() {
        let mut e = ramp("fades", 1);
        let orig = render_file(&e).unwrap();
        run(&mut e, "wave.fade_in", json!({"start": 0, "end": 48_000, "shape": "linear"}));
        let now = render_file(&e).unwrap();
        assert!((now[0][24_000] - 0.5 * orig[0][24_000]).abs() < 1e-4);
        assert_eq!(now[0][48_000..], orig[0][48_000..]);
        run(&mut e, "wave.fade_out", json!({"start": 96_000, "end": 144_000, "shape": "linear"}));
        let last = render_file(&e).unwrap()[0][143_999];
        assert!(last.abs() < 1e-4, "{last}");
        assert!(e.execute("wave.fade_in", &json!({"shape": "wobbly"})).is_err());
    }

    #[test]
    fn reverse_and_silence_stay_in_place() {
        let mut e = ramp("reverse", 1);
        let orig = render_file(&e).unwrap();
        run(&mut e, "wave.reverse", json!({"start": 0, "end": 1_000}));
        let now = render_file(&e).unwrap();
        assert_eq!(now[0][0], orig[0][999]);
        assert_eq!(now[0][1_000..], orig[0][1_000..]);
        run(&mut e, "wave.silence", json!({"start": 2_000, "end": 3_000}));
        assert!(render_file(&e).unwrap()[0][2_000..3_000].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn bad_selections_never_panic() {
        let mut e = ramp("bad_sel", 2);
        run(&mut e, "wave.gain", json!({"db": -6.0, "start": 96_000, "end": 48_000}));
        run(&mut e, "wave.gain", json!({"db": -6.0, "start": 0, "end": 10_000_000}));
        let err = e.execute("wave.cut", &json!({"start": 1_000, "end": 1_000})).unwrap_err().to_string();
        assert!(err.contains("select"), "{err}");
        assert!(e.execute("wave.select", &json!({"channels": [5]})).is_err());
        assert!(e.execute("wave.gain", &json!({"db": 200.0})).is_err());
        assert!(e.execute("wave.gain", &json!({})).is_err());
        assert_eq!(render_file(&e).unwrap()[0].len(), 144_000);
    }

    #[test]
    fn paste_refuses_empty_or_foreign_clipboard() {
        let mut e = ramp("paste_refuses", 1);
        let err = e.execute("wave.paste", &json!({"at": 0})).unwrap_err().to_string();
        assert!(err.contains("clipboard"), "{err}");
        let dir = tmp_dir("paste_foreign");
        let other = dir.join("other.wav");
        write_wav(&other, &noise(44_100, 1, 44_100), BitDepth::Int16);
        run(&mut e, "wave.open", json!({"path": other.to_str().unwrap()}));
        run(&mut e, "wave.copy", json!({"start": 0, "end": 4_410}));
        let back = dir.join("back.wav");
        write_wav(&back, &noise(48_000, 1, 48_000), BitDepth::Int16);
        run(&mut e, "wave.open", json!({"path": back.to_str().unwrap()}));
        let err = e.execute("wave.paste", &json!({"at": 0})).unwrap_err().to_string();
        assert!(err.contains("44100") && err.contains("48000"), "{err}");
    }

    #[test]
    fn pool_is_pruned_after_processing() {
        let mut e = ramp("prune", 1);
        for _ in 0..3 {
            run(&mut e, "wave.gain", json!({"db": 1.0, "start": 0, "end": 48_000}));
        }
        let s = e.session();
        let used: Vec<_> = s
            .tracks
            .iter()
            .flat_map(|t| t.clips().iter())
            .filter_map(|c| match c.content {
                ClipContent::Audio { source, .. } => Some(source),
                _ => None,
            })
            .collect();
        assert!(s.pool.ids().all(|id| used.contains(&id)), "pool {:?} clips {used:?}", s.pool.ids().collect::<Vec<_>>());
    }

    #[test]
    fn f0_acceptance_in_engine() {
        let (mut e, dir) = opened("f0_acceptance", &noise(48_000, 2, 144_000), BitDepth::Int24);
        let path = |n: &str| dir.join(n).to_str().unwrap().to_string();
        run(&mut e, "wave.export", json!({"path": path("a.wav"), "bit_depth": 24}));
        run(&mut e, "wave.cut", json!({"start": 0, "end": {"seconds": 2.0}}));
        run(&mut e, "wave.normalize", json!({}));
        run(&mut e, "wave.export", json!({"path": path("b.wav"), "bit_depth": 24}));
        run(&mut e, "edit.undo", json!({}));
        run(&mut e, "edit.undo", json!({}));
        run(&mut e, "wave.export", json!({"path": path("c.wav"), "bit_depth": 24}));
        let read = |n: &str| std::fs::read(dir.join(n)).unwrap();
        assert_eq!(read("c.wav"), read("a.wav"));
        assert_ne!(read("b.wav"), read("a.wav"));
    }
}
