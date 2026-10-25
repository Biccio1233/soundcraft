use soundcraft_wave_analysis::models::{DEFAULT_WHISPER, Models};
use soundcraft_wave_analysis::spectrum::magnitudes;
use soundcraft_wave_ml::Backend;
use std::path::PathBuf;

const SR: u32 = 48_000;

fn empty_dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("wavecraft-ml-tests").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sine(hz: f32, secs: f32) -> Vec<f32> {
    (0..(secs * SR as f32) as usize).map(|i| 0.4 * (std::f32::consts::TAU * hz * i as f32 / SR as f32).sin()).collect()
}

/// The strongest frequency of `x` (65,536-point spectrum of its middle, parabolic peak).
fn peak_hz(x: &[f32]) -> f32 {
    let n = 65_536.min(x.len().next_power_of_two() / 2);
    let start = x.len().saturating_sub(n) / 2;
    let m = &magnitudes(&x[start..start + n], n, n)[0];
    let (k, _) = m.iter().enumerate().skip(1).fold((0, 0.0f32), |a, (i, v)| if *v > a.1 { (i, *v) } else { a });
    let (a, b, c) = (m[k - 1].ln(), m[k].ln(), m[k + 1].ln());
    let delta = 0.5 * (a - c) / (a - 2.0 * b + c);
    (k as f32 + delta) * SR as f32 / n as f32
}

#[test]
fn stretch_slows_without_changing_pitch() {
    let b = Backend::with_dir(empty_dir("stretch_slow"));
    let out = b.stretch(&[sine(440.0, 2.0)], SR, 0.5, 0.0).unwrap();
    assert_eq!(out.len(), 1);
    let want = 4.0 * SR as f32;
    assert!((out[0].len() as f32 - want).abs() <= 0.01 * want, "{} samples", out[0].len());
    assert!((peak_hz(&out[0]) - 440.0).abs() <= 3.0, "{} Hz", peak_hz(&out[0]));
}

#[test]
fn transpose_moves_the_pitch() {
    let b = Backend::with_dir(empty_dir("stretch_transpose"));
    let x = sine(440.0, 2.0);
    let out = b.stretch(&[x.clone(), x.clone()], SR, 1.0, 12.0).unwrap();
    assert_eq!(out.len(), 2);
    assert!((out[0].len() as f32 - x.len() as f32).abs() <= 0.01 * x.len() as f32);
    assert!((peak_hz(&out[1]) - 880.0).abs() <= 5.0, "{} Hz", peak_hz(&out[1]));
    let short = b.stretch(&[sine(440.0, 0.3)], SR, 1.5, -12.0).unwrap();
    let want = 0.2 * SR as f32;
    assert!((short[0].len() as f32 - want).abs() <= 0.01 * want, "{} samples", short[0].len());
    assert!(short[0].iter().all(|v| v.is_finite()));
    assert!(b.stretch(&[], SR, 1.0, 0.0).unwrap().is_empty());
    assert!(b.stretch(&[x], SR, 0.0, 0.0).is_err());
}

#[test]
fn missing_model_names_its_size() {
    let b = Backend::with_dir(empty_dir("missing_model"));
    let err = b.separate("htdemucs", &[sine(440.0, 1.0)], SR, &mut |_| true).unwrap_err();
    assert!(err.contains("htdemucs") && err.contains("169 MB"), "{err}");
    let err = b.transcribe(DEFAULT_WHISPER, &vec![0.0; 16_000], Some("it")).unwrap_err();
    assert!(err.contains(DEFAULT_WHISPER) && err.contains("574 MB"), "{err}");
    assert!(b.separate("whisper-tiny", &[sine(440.0, 1.0)], SR, &mut |_| true).is_err(), "a transcription model cannot separate");
    assert!(b.installed("nope").is_none());
}

#[test]
#[ignore = "needs the htdemucs model in the models folder"]
fn demucs_model_runs() {
    let b = Backend::new();
    let mut x = sine(220.0, 3.0);
    let n = soundcraft_wave_analysis::testsignal::noise(x.len(), 3);
    for (a, v) in x.iter_mut().zip(n) {
        *a += 0.05 * v;
    }
    let stems = b.separate("htdemucs", &[x.clone(), x.clone()], SR, &mut |_| true).unwrap();
    assert_eq!(stems.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["drums", "bass", "other", "vocals"]);
    let rms = |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt();
    let mut sum = vec![0.0f32; x.len()];
    for s in &stems {
        assert_eq!((s.channels.len(), s.channels[0].len()), (2, x.len()));
        assert!(s.channels[0].iter().all(|v| v.is_finite()));
        for (a, v) in sum.iter_mut().zip(&s.channels[0]) {
            *a += v;
        }
    }
    let db = 20.0 * (rms(&sum) / rms(&x)).log10();
    assert!(db.abs() <= 1.0, "stems sum {db:.2} dB off the mix");
}

#[test]
#[ignore = "needs the default Whisper model and macOS `say`"]
fn whisper_hears_italian() {
    let dir = empty_dir("whisper_it");
    let aiff = dir.join("buongiorno.aiff");
    let ok = std::process::Command::new("say").args(["-v", "Alice", "-o"]).arg(&aiff).arg("buongiorno a tutti").status().unwrap();
    assert!(ok.success());
    let bytes = std::fs::read(&aiff).unwrap();
    let (info, buf) = soundcraft_audio_io::decode(&bytes, Some("aiff")).unwrap();
    let mono = soundcraft_wave_analysis::resample(&buf.channels[0], info.sample_rate, 16_000);
    let words = Backend::new().transcribe(DEFAULT_WHISPER, &mono, Some("it")).unwrap();
    let text: Vec<String> = words.iter().map(|w| w.text.to_lowercase()).collect();
    assert!(text.iter().any(|t| t.contains("buongiorno")), "{text:?}");
    assert!(words.windows(2).all(|w| w[1].start >= w[0].start));
}

#[test]
fn damaged_model_never_reaches_the_runtime() {
    // A file of whisper-tiny's exact size but the wrong bytes: whisper.cpp must never load it.
    let d = empty_dir("damaged_whisper");
    let m = soundcraft_wave_analysis::models::model("whisper-tiny").unwrap();
    std::fs::File::create(d.join(m.file)).unwrap().set_len(m.bytes).unwrap();
    let b = Backend::with_dir(d.clone());
    let err = b.transcribe("whisper-tiny", &vec![0.0; 16_000], Some("it")).unwrap_err();
    assert!(err.contains("damaged"), "{err}");
    assert!(b.installed("whisper-tiny").is_none(), "offered for download again");
}
