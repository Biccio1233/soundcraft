//! WaveCraft restoration: the noise print, the restoration processes (each the matching built-in
//! plugin run offline over the selection, one undo step) and amplitude statistics.

use super::wave::{has_file, wave, wave_range, wave_track};
use super::wave_edit::process_range;
use super::*;
use crate::{cmd, io};
use serde_json::json;
use soundcraft_dsp::biquad::{Biquad, FilterType};
use soundcraft_dsp::restore::{BANDS, NoisePrint, NoiseReduction, detect_mains};
use soundcraft_dsp::{ParamInfo, Plugin, PluginInfo, Unit};

const PRINT_VERSION: u64 = 1;
const DEFAULT_PRINT_FFT: usize = 4096;
const NO_PRINT: &str = "capture a noise print first: select noise only, then wave.noise_print_capture";

/// A captured noise print and how much audio it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseCapture {
    pub print: NoisePrint,
    pub seconds: f64,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "wave.noise_print_capture",
            "Capture Noise Print",
            [],
            None,
            "{start?, end?, fft?: 256..16384} — take the noise print noise reduction removes from the selection (noise alone; the enabled channels); not an undo step",
            has_file,
            capture
        ),
        cmd!(query "wave.noise_print_info", "Noise Print", [], None, "{} — {captured, seconds?, fft?, bands?: [62 dB, sixth-octave bands from 20 Hz]}", has_file, print_info),
        cmd!(query "wave.noise_print_save", "Save Noise Print", [], None, "{path} — write the noise print as JSON", has_file, print_save),
        cmd!(noundo "wave.noise_print_load", "Load Noise Print", [], None, "{path} — read a noise print saved by wave.noise_print_save", has_file, print_load),
        cmd!(noundo "wave.noise_print_clear", "Clear Noise Print", [], None, "{} — forget the noise print", has_file, print_clear),
        cmd!(
            "wave.noise_reduction",
            "Noise Reduction",
            [],
            None,
            "{start?, end?, floor_db?: -80..0, offset_db?: -10..20, smoothing?: 90..99.9, fft?: 512..16384, noise_only?: bool} — remove the captured noise print from the selection (or the whole file)",
            has_file,
            noise_reduction
        ),
        cmd!(
            "wave.adaptive_noise_reduction",
            "Adaptive Denoise",
            [],
            None,
            "{start?, end?, floor_db?: -60..0, offset_db?: -10..20, smoothing?: 90..99.9, fft?: 512..16384, window_s?: 0.5..5, noise_only?: bool} — remove steady noise without a print (steady tones count as noise)",
            has_file,
            adaptive
        ),
        cmd!(
            "wave.dehum",
            "Hum Removal",
            [],
            None,
            "{start?, end?, freq?: \"Auto\"|\"50 Hz\"|\"60 Hz\" (or 50/60), harmonics?: 1..30, q?: 5..100, depth_db?: 0..60, hum_only?: bool} — cut the mains hum and its harmonics",
            has_file,
            dehum
        ),
        cmd!(
            "wave.hiss_reduction",
            "Hiss Reduction",
            [],
            None,
            "{start?, end?, threshold_db?: 0..40, reduce_db?: 0..60, cutoff_hz?: 1000..16000, hiss_only?: bool} — expand the quiet highs down above the cutoff",
            has_file,
            hiss
        ),
        cmd!(
            "wave.declip",
            "Clip Repair",
            [],
            None,
            "{start?, end?, threshold_db?: -20..0, max_run_ms?: 0.1..10, gain_db?: -12..0} — redraw clipped runs by cubic interpolation",
            has_file,
            declip
        ),
        cmd!(
            "wave.dereverb",
            "Reverb Reduction",
            [],
            None,
            "{start?, end?, amount?: 0..100, t60_s?: 0.1..4, floor_db?: -40..0, auto_gain?: bool} — suppress late reverberation (approximate)",
            has_file,
            dereverb
        ),
        cmd!(
            query "wave.stats",
            "Amplitude Statistics",
            [],
            None,
            "{start?, end?, filter?: \"bandpass\"|\"highpass\"|\"lowpass\", hz?, q?} — peak and RMS (all channels together, dBFS) of the selection or the whole file, optionally through an RBJ filter (Q 0.5 band-pass, 0.7071 otherwise) starting from rest",
            has_file,
            stats
        ),
    ]
}

/// The enabled channels of `r`, rendered.
fn enabled_channels(e: &Engine, r: Range) -> Result<Vec<Vec<f32>>> {
    let t = wave_track(e)?;
    let enabled = wave(e)?.channels_enabled.clone();
    Ok(soundcraft_mix::render_clips(e.session(), t, r).into_iter().zip(enabled).filter(|(_, on)| *on).map(|(c, _)| c).collect())
}

fn capture(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.noise_print_capture";
    let r = wave_range(e, ID, p)?;
    if r.is_empty() {
        return Err(bad(ID, "select some noise first"));
    }
    let fft = match p.get("fft") {
        None => DEFAULT_PRINT_FFT,
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| n.is_power_of_two() && (256..=16_384).contains(n))
            .ok_or_else(|| bad(ID, "`fft` must be a power of two from 256 to 16384"))?,
    };
    let sr = wave(e)?.sample_rate;
    let ch = enabled_channels(e, r)?;
    let print = NoisePrint::capture(&ch, sr, fft).map_err(|m| bad(ID, m))?;
    let seconds = r.len() as f64 / f64::from(sr.max(1));
    if let Some(w) = e.wave.as_mut() {
        w.noise_print = Some(NoiseCapture { print, seconds });
    }
    print_json(e)
}

fn print_json(e: &Engine) -> Result<Value> {
    Ok(match &wave(e)?.noise_print {
        Some(c) => {
            json!({"captured": true, "seconds": c.seconds, "fft": c.print.fft_size, "sample_rate": c.print.sample_rate, "bands": round_bands(&c.print.bands_db())})
        }
        None => json!({"captured": false}),
    })
}

/// Band levels rounded to 0.01 dB (as stored in rack parameters and shown to people).
fn round_bands(b: &[f32; BANDS]) -> Vec<f64> {
    b.iter().map(|v| (f64::from(*v) * 100.0).round() / 100.0).collect()
}

fn print_info(e: &mut Engine, _: &Value) -> Result<Value> {
    print_json(e)
}

fn print_save(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.noise_print_save";
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(ID, "`path` required"))?.to_string();
    let c = wave(e)?.noise_print.clone().ok_or_else(|| bad(ID, NO_PRINT))?;
    let text = serde_json::to_string(&json!({"version": PRINT_VERSION, "seconds": c.seconds, "print": c.print}))
        .map_err(|err| EngineError::Internal(ID.into(), err.to_string()))?;
    io::write_file(&path, text.as_bytes())?;
    Ok(json!({"path": path}))
}

fn print_load(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.noise_print_load";
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(ID, "`path` required"))?.to_string();
    let bytes = io::read_file(&path)?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|err| bad(ID, format!("{path}: not a noise print ({err})")))?;
    if v.get("version").and_then(Value::as_u64) != Some(PRINT_VERSION) {
        return Err(bad(ID, format!("{path}: not a version {PRINT_VERSION} noise print")));
    }
    let print: NoisePrint =
        serde_json::from_value(v.get("print").cloned().unwrap_or(Value::Null)).map_err(|err| bad(ID, format!("{path}: {err}")))?;
    let well_formed = print.sample_rate > 0
        && print.fft_size.is_power_of_two()
        && (256..=16_384).contains(&print.fft_size)
        && (1..=2).contains(&print.psd.len())
        && print.psd.iter().all(|c| c.len() == print.fft_size / 2 + 1 && c.iter().all(|x| x.is_finite() && *x >= 0.0));
    if !well_formed {
        return Err(bad(ID, format!("{path}: the noise print is malformed")));
    }
    let seconds = v.get("seconds").and_then(Value::as_f64).filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(0.0);
    if let Some(w) = e.wave.as_mut() {
        w.noise_print = Some(NoiseCapture { print, seconds });
    }
    print_json(e)
}

fn print_clear(e: &mut Engine, _: &Value) -> Result<Value> {
    if let Some(w) = e.wave.as_mut() {
        w.noise_print = None;
    }
    print_json(e)
}

/// For `wave.rack_add` of noise reduction: put the print's band levels into the insert's
/// `params` (unless the caller already set them).
pub fn fill_print_bands(e: &Engine, q: &mut Value) {
    let Some(c) = e.wave.as_ref().and_then(|w| w.noise_print.as_ref()) else { return };
    let Some(obj) = q.as_object_mut() else { return };
    let params = obj.entry("params").or_insert_with(|| json!({}));
    let Some(params) = params.as_object_mut() else { return };
    if params.contains_key("nb00") {
        return;
    }
    for (i, b) in round_bands(&c.print.bands_db()).into_iter().enumerate() {
        params.insert(format!("nb{i:02}"), json!(b));
    }
}

/// The leading number of a choice label ("60 Hz" → 60, "4096" → 4096).
fn label_number(label: &str) -> Option<f64> {
    label.split_whitespace().next().and_then(|t| t.parse::<f64>().ok())
}

/// Set each of `info`'s parameters present in `p`: numbers; booleans for toggles; for a choice its
/// label ("60 Hz") or the number it starts with (60), never a bare index.
fn set_params(id: &str, info: &PluginInfo, plugin: &mut dyn Plugin, p: &Value) -> Result<()> {
    for pi in info.params {
        let Some(v) = p.get(pi.id) else { continue };
        let value = param_value(id, pi, v)?;
        plugin.set_param(pi.id, value);
    }
    Ok(())
}

fn param_value(id: &str, pi: &ParamInfo, v: &Value) -> Result<f32> {
    match pi.unit {
        Unit::Choice => {
            let pick = match v {
                Value::String(s) => pi.choices.iter().position(|c| c.eq_ignore_ascii_case(s.trim())),
                Value::Number(n) => n.as_f64().and_then(|n| pi.choices.iter().position(|c| label_number(c) == Some(n))),
                _ => None,
            };
            pick.map(|i| i as f32).ok_or_else(|| bad(id, format!("`{}` must be one of {}", pi.id, pi.choices.join(", "))))
        }
        Unit::Toggle => match v {
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Number(n) => {
                n.as_f64().map(|n| if n >= 0.5 { 1.0 } else { 0.0 }).ok_or_else(|| bad(id, format!("`{}` must be true or false", pi.id)))
            }
            _ => Err(bad(id, format!("`{}` must be true or false", pi.id))),
        },
        _ => v
            .as_f64()
            .filter(|n| n.is_finite())
            .map(|n| n as f32)
            .ok_or_else(|| bad(id, format!("`{}` must be a number ({}..{})", pi.id, pi.min, pi.max))),
    }
}

/// Run `plugin` (its parameters set from `p`) over the selection's enabled channels as one undo
/// step; `prepare` sees the audio first (to pick settings that depend on it).
fn apply(e: &mut Engine, id: &str, p: &Value, mut plugin: Box<dyn Plugin>, prepare: impl FnOnce(&mut dyn Plugin, &[Vec<f32>], f32)) -> Result<Value> {
    let info = plugin.info();
    set_params(id, info, plugin.as_mut(), p)?;
    let sr = wave(e)?.sample_rate as f32;
    process_range(e, id, p, info.name, |ch| {
        prepare(plugin.as_mut(), ch, sr);
        soundcraft_dsp::offline::apply_plugin(ch, plugin.as_mut(), sr);
        Ok(())
    })
}

fn built_in(id: &str, plugin: &str) -> Result<Box<dyn Plugin>> {
    soundcraft_dsp::create(plugin).ok_or_else(|| EngineError::Internal(id.into(), format!("the built-in `{plugin}` is missing")))
}

fn noise_reduction(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.noise_reduction";
    let c = wave(e)?.noise_print.clone().ok_or_else(|| bad(ID, NO_PRINT))?;
    apply(e, ID, p, Box::new(NoiseReduction::with_print(c.print)), |_, _, _| {})
}

fn adaptive(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.adaptive_noise_reduction";
    apply(e, ID, p, built_in(ID, "adaptive_noise_reduction")?, |_, _, _| {})
}

fn dehum(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.dehum";
    apply(e, ID, p, built_in(ID, "dehum")?, |plugin, ch, sr| {
        // Auto decides on the whole selection first, so its start is not processed at 50 Hz.
        if plugin.param("freq").is_some_and(|f| f < 0.5) {
            plugin.set_param("freq", if detect_mains(ch, sr) == 60.0 { 2.0 } else { 1.0 });
        }
    })
}

fn hiss(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.hiss_reduction";
    apply(e, ID, p, built_in(ID, "hiss_reduction")?, |_, _, _| {})
}

fn declip(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.declip";
    apply(e, ID, p, built_in(ID, "declip")?, |_, _, _| {})
}

fn dereverb(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.dereverb";
    apply(e, ID, p, built_in(ID, "dereverb")?, |_, _, _| {})
}

fn stats(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "wave.stats";
    let r = wave_range(e, ID, p)?;
    if r.is_empty() {
        return Err(bad(ID, "select a range first"));
    }
    let sr = wave(e)?.sample_rate as f32;
    let filter = match str_param(p, "filter") {
        None => None,
        Some(name) => {
            let kind = match name {
                "bandpass" => FilterType::BandPass,
                "highpass" => FilterType::HighPass,
                "lowpass" => FilterType::LowPass,
                other => return Err(bad(ID, format!("unknown filter `{other}` (bandpass, highpass, lowpass)"))),
            };
            let hz = p
                .get("hz")
                .and_then(Value::as_f64)
                .filter(|h| h.is_finite() && *h > 0.0 && *h < f64::from(sr) / 2.0)
                .ok_or_else(|| bad(ID, "`hz` required with a filter, between 0 and Nyquist"))? as f32;
            let default_q = if kind == FilterType::BandPass { 0.5 } else { std::f32::consts::FRAC_1_SQRT_2 };
            let q = match p.get("q") {
                None => default_q,
                Some(v) => {
                    v.as_f64().filter(|q| q.is_finite() && *q > 0.0 && *q <= 100.0).ok_or_else(|| bad(ID, "`q` must be between 0 and 100"))? as f32
                }
            };
            Some((kind, hz, q))
        }
    };
    let t = wave_track(e)?;
    let mut ch = soundcraft_mix::render_clips(e.session(), t, r);
    let (mut peak, mut sum, mut count) = (0.0f32, 0.0f64, 0usize);
    for c in &mut ch {
        if let Some((kind, hz, q)) = filter {
            Biquad::design(kind, hz, q, 0.0, sr).process_block(c);
        }
        for v in c.iter() {
            peak = peak.max(v.abs());
            sum += f64::from(*v) * f64::from(*v);
        }
        count += c.len();
    }
    let rms = if count > 0 { (sum / count as f64).sqrt() as f32 } else { 0.0 };
    let db = |x: f32| (f64::from(soundcraft_dsp::gain_to_db(x)) * 1000.0).round() / 1000.0;
    Ok(json!({"peak_db": db(peak), "rms_db": db(rms), "frames": r.len(), "channels": ch.len()}))
}

#[cfg(test)]
mod tests {
    use super::super::wave::render_file;
    use super::super::wave::tests::{opened, tmp_dir};
    use super::*;
    use serde_json::json;
    use soundcraft_audio_io::{AudioBuffer, BitDepth};

    const SR: u32 = 48_000;

    /// 1 s of noise (about −50 dBFS), then 2 s of 440 Hz (−20 dBFS RMS) over it.
    fn take(test: &str, channels: usize) -> Engine {
        let n = 3 * SR as usize;
        let mut s = 0x2468_ace1u32;
        let ch: Vec<f32> = (0..n)
            .map(|i| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let noise = (s as f32 / u32::MAX as f32 - 0.5) * 0.01;
                let tone = if i >= SR as usize { 0.141_4 * (std::f32::consts::TAU * 440.0 * i as f32 / SR as f32).sin() } else { 0.0 };
                noise + tone
            })
            .collect();
        opened(test, &AudioBuffer { sample_rate: SR, channels: vec![ch; channels] }, BitDepth::Float32).0
    }

    fn sines(test: &str, hz: &[f32]) -> Engine {
        let ch: Vec<f32> =
            (0..SR as usize).map(|i| hz.iter().map(|f| 0.1 * (std::f32::consts::TAU * f * i as f32 / SR as f32).sin()).sum()).collect();
        opened(test, &AudioBuffer { sample_rate: SR, channels: vec![ch] }, BitDepth::Float32).0
    }

    fn run(e: &mut Engine, id: &str, p: Value) -> Value {
        e.execute(id, &p).unwrap_or_else(|err| panic!("{id}: {err}"))
    }

    fn capture(e: &mut Engine) -> Value {
        run(e, "wave.noise_print_capture", json!({"start": 0, "end": {"seconds": 1}}))
    }

    #[test]
    fn capture_then_noise_reduction_is_one_undo_step() {
        let mut e = take("restore_one_step", 1);
        let orig = render_file(&e).unwrap();
        let n0 = e.undo_history().len();
        let got = capture(&mut e);
        assert_eq!(got["bands"].as_array().map(Vec::len), Some(62));
        assert_eq!(e.undo_history().len(), n0, "capturing is not an edit");
        run(&mut e, "wave.noise_reduction", json!({}));
        assert_eq!(e.undo_history().len(), n0 + 1);
        assert!(e.undo_label().unwrap_or_default().starts_with("Noise Reduction"), "{:?}", e.undo_label());
        let now = render_file(&e).unwrap();
        assert_eq!(now[0].len(), orig[0].len());
        assert_ne!(now, orig);
        e.undo();
        assert_eq!(render_file(&e).unwrap(), orig);
    }

    #[test]
    fn noise_reduction_without_print_refuses() {
        let mut e = take("restore_no_print", 1);
        let err = e.execute("wave.noise_reduction", &json!({})).unwrap_err().to_string();
        assert!(err.contains("capture a noise print first"), "{err}");
    }

    #[test]
    fn print_save_load_round_trip() {
        let mut e = take("restore_print_io", 2);
        let bands = capture(&mut e)["bands"].clone();
        let path = tmp_dir("restore_print_io").join("print.json");
        let path = path.to_str().unwrap();
        run(&mut e, "wave.noise_print_save", json!({"path": path}));
        run(&mut e, "wave.noise_print_clear", json!({}));
        assert_eq!(run(&mut e, "wave.noise_print_info", json!({}))["captured"], json!(false));
        run(&mut e, "wave.noise_print_load", json!({"path": path}));
        let info = run(&mut e, "wave.noise_print_info", json!({}));
        assert_eq!(info["captured"], json!(true));
        assert_eq!(info["bands"], bands);
        let bad = tmp_dir("restore_print_io").join("bad.json");
        std::fs::write(&bad, r#"{"version": 1, "seconds": 1, "print": {"sample_rate": 48000, "fft_size": 1000, "psd": [[1.0]]}}"#).unwrap();
        assert!(e.execute("wave.noise_print_load", &json!({"path": bad.to_str().unwrap()})).is_err());
    }

    #[test]
    fn each_restoration_command_runs_and_keeps_length() {
        let mut e = take("restore_each", 2);
        capture(&mut e);
        let len = render_file(&e).unwrap()[0].len();
        for id in ["wave.noise_reduction", "wave.adaptive_noise_reduction", "wave.dehum", "wave.hiss_reduction", "wave.declip", "wave.dereverb"] {
            let n0 = e.undo_history().len();
            run(&mut e, id, json!({}));
            assert_eq!(e.undo_history().len(), n0 + 1, "{id}");
            let now = render_file(&e).unwrap();
            assert!(now.iter().all(|c| c.len() == len && c.iter().all(|v| v.is_finite())), "{id}");
        }
    }

    #[test]
    fn choice_params_accept_labels() {
        let mut e = take("restore_choices", 1);
        run(&mut e, "wave.adaptive_noise_reduction", json!({"fft": 4096}));
        run(&mut e, "wave.dehum", json!({"freq": "60 Hz"}));
        run(&mut e, "wave.dehum", json!({"freq": 50}));
        assert!(e.execute("wave.adaptive_noise_reduction", &json!({"fft": 1000})).is_err());
        assert!(e.execute("wave.dehum", &json!({"freq": "55 Hz"})).is_err());
        assert!(e.execute("wave.hiss_reduction", &json!({"reduce_db": "a lot"})).is_err());
    }

    #[test]
    fn rack_add_noise_reduction_takes_the_print() {
        let mut e = take("restore_rack", 1);
        let bands = capture(&mut e)["bands"].as_array().cloned().unwrap();
        let rack = run(&mut e, "wave.rack_add", json!({"plugin": "noise_reduction"}));
        let params = rack["slots"][0]["params"].as_array().cloned().unwrap();
        for (i, b) in bands.iter().enumerate() {
            let id = format!("nb{i:02}");
            let v = params.iter().find(|p| p["id"] == id.as_str()).and_then(|p| p["value"].as_f64()).unwrap();
            assert!((v - b.as_f64().unwrap()).abs() <= 0.01, "{id}: {v} vs {b}");
        }
    }

    #[test]
    fn stats_matches_hand_computation() {
        let mut e = sines("restore_stats", &[1000.0]);
        let s = run(&mut e, "wave.stats", json!({}));
        assert!((s["rms_db"].as_f64().unwrap() + 23.01).abs() <= 0.05, "{s}");
        assert!((s["peak_db"].as_f64().unwrap() + 20.0).abs() <= 0.05, "{s}");
        assert_eq!(s["frames"], json!(SR));
        // RBJ band-pass at 3 kHz, Q 0.5: |H(1 kHz)| = 1/√(1 + Q²(1/3 − 3)²) = −4.44 dB.
        let b = run(&mut e, "wave.stats", json!({"start": {"seconds": 0.5}, "end": {"seconds": 1}, "filter": "bandpass", "hz": 3000}));
        assert!((b["rms_db"].as_f64().unwrap() + 23.01 + 4.44).abs() <= 0.2, "{b}");
        let h = run(&mut e, "wave.stats", json!({"start": {"seconds": 0.5}, "end": {"seconds": 1}, "filter": "highpass", "hz": 8000}));
        assert!(h["rms_db"].as_f64().unwrap() < -55.0, "{h}");
        assert!(e.execute("wave.stats", &json!({"filter": "comb", "hz": 100})).is_err());
        assert!(e.execute("wave.stats", &json!({"filter": "bandpass"})).is_err());
    }

    #[test]
    fn disabled_channel_is_untouched() {
        let mut e = take("restore_channels", 2);
        let orig = render_file(&e).unwrap();
        run(&mut e, "wave.select", json!({"channels": [0]}));
        run(&mut e, "wave.hiss_reduction", json!({}));
        let now = render_file(&e).unwrap();
        assert_eq!(now[1], orig[1]);
        assert_ne!(now[0], orig[0]);
    }
}
