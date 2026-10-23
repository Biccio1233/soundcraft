//! F1 acceptance.
//!
//! - Synthetic: 440 Hz at −20 dBFS over white noise at −70 dBFS. A script captures the print on
//!   the noise alone and runs noise reduction with its defaults; the tone stays within 1 dB and the
//!   floor drops by at least 20 dB.
//! - Real take (ignored: the owner's file lives outside the repo): the measurement method settled
//!   at the start of F1 — pause = RMS over 211.4–212.1 s, "3 kHz" = RMS over 30–90 s after an RBJ
//!   band-pass at 3000 Hz, Q 0.5. After capture on the pause and default noise reduction, the pause
//!   is at most −70 dB and the 3 kHz band loses at most 1 dB. Run it with
//!   `cargo test -p wavecraft-cli --test acceptance_f1 -- --ignored` (`WAVECRAFT_REAL_TAKE` overrides
//!   the path).

use serde_json::Value;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_dsp::biquad::{Biquad, FilterType};
use std::path::{Path, PathBuf};
use std::process::Command;

const SR: u32 = 48_000;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("wavecraft-cli-tests").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Run a script; returns each `wave.stats` result in order.
fn script(dir: &Path, text: &str) -> Vec<Value> {
    let path = dir.join("f1.wavecraft");
    std::fs::write(&path, text).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wavecraft-cli")).arg("script").arg(&path).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    stdout.lines().filter_map(|l| l.strip_prefix("wave.stats: ")).map(|j| serde_json::from_str(j).unwrap()).collect()
}

fn rms_db(x: &[f32]) -> f32 {
    10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).max(1e-30).log10()
}

fn band_db(x: &[f32], stages: &[(FilterType, f32, f32)], from: f32, to: f32) -> f32 {
    let mut y = x.to_vec();
    for &(kind, hz, q) in stages {
        Biquad::design(kind, hz, q, 0.0, SR as f32).process_block(&mut y);
    }
    rms_db(&y[(from * SR as f32) as usize..(to * SR as f32) as usize])
}

#[test]
fn synthetic_noise_reduction_keeps_tone_lowers_floor() {
    let d = dir("acceptance_f1_synthetic");
    let n = 4 * SR as usize;
    let mut s = 0x1357_9bdfu32;
    let sigma = 10f32.powf(-70.0 / 20.0) * 3f32.sqrt() * 2.0;
    let ch: Vec<f32> = (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let noise = (s as f32 / u32::MAX as f32 - 0.5) * sigma;
            let tone = if i >= SR as usize { 0.141_42 * (std::f32::consts::TAU * 440.0 * i as f32 / SR as f32).sin() } else { 0.0 };
            noise + tone
        })
        .collect();
    let src = d.join("take.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None };
    std::fs::write(&src, soundcraft_audio_io::encode(&AudioBuffer { sample_rate: SR, channels: vec![ch] }, &opts).unwrap()).unwrap();
    let p = |name: &str| d.join(name).to_str().unwrap().to_string();
    script(
        &d,
        &format!(
            "wave.open {{\"path\": \"{src}\"}}\n\
             wave.export {{\"path\": \"{a}\", \"bit_depth\": \"32f\"}}\n\
             wave.noise_print_capture {{\"start\": 0, \"end\": {{\"seconds\": 1}}}}\n\
             wave.noise_reduction {{}}\n\
             wave.export {{\"path\": \"{b}\", \"bit_depth\": \"32f\"}}\n",
            src = src.to_str().unwrap(),
            a = p("a.wav"),
            b = p("b.wav"),
        ),
    );
    let read = |name: &str| soundcraft_audio_io::decode(&std::fs::read(d.join(name)).unwrap(), Some("wav")).unwrap().1.channels.remove(0);
    let (a, b) = (read("a.wav"), read("b.wav"));
    let tone = [(FilterType::BandPass, 440.0, 8.0), (FilterType::BandPass, 440.0, 8.0)];
    let highs = [
        (FilterType::HighPass, 4000.0, 0.707),
        (FilterType::HighPass, 4000.0, 0.707),
        (FilterType::HighPass, 4000.0, 0.707),
        (FilterType::HighPass, 4000.0, 0.707),
        (FilterType::LowPass, 8000.0, 0.707),
        (FilterType::LowPass, 8000.0, 0.707),
    ];
    let t = band_db(&a, &tone, 2.0, 4.0) - band_db(&b, &tone, 2.0, 4.0);
    assert!(t.abs() <= 1.0, "tone changed {t} dB");
    let h = band_db(&a, &highs, 2.0, 4.0) - band_db(&b, &highs, 2.0, 4.0);
    assert!(h >= 20.0, "4-8 kHz floor dropped only {h} dB");
    println!("synthetic: tone {t:+.2} dB, 4-8 kHz floor −{h:.1} dB");
}

#[test]
#[ignore = "needs the owner's 8-bit take (WAVECRAFT_REAL_TAKE)"]
fn real_take_meets_the_f1_target() {
    let take = std::env::var("WAVECRAFT_REAL_TAKE").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/Desktop/0. MBS-OS/11. AirCraft/SoundCraft/reclusa-nuova1/sorgente-8bit.aiff")
    });
    assert!(Path::new(&take).exists(), "{take}: set WAVECRAFT_REAL_TAKE to the 8-bit take");
    let d = dir("acceptance_f1_real");
    let pause = r#"{"start": {"seconds": 211.4}, "end": {"seconds": 212.1}}"#;
    let band = r#"{"start": {"seconds": 30}, "end": {"seconds": 90}, "filter": "bandpass", "hz": 3000, "q": 0.5}"#;
    let stats = script(
        &d,
        &format!(
            "wave.open {{\"path\": \"{take}\"}}\n\
             wave.stats {pause}\n\
             wave.stats {band}\n\
             wave.noise_print_capture {pause}\n\
             wave.noise_reduction {{\"start\": 0, \"end\": {{\"seconds\": 100000}}}}\n\
             wave.stats {pause}\n\
             wave.stats {band}\n"
        ),
    );
    let rms = |i: usize| stats.get(i).and_then(|s| s["rms_db"].as_f64()).unwrap();
    let (pause_src, band_src, pause_out, band_out) = (rms(0), rms(1), rms(2), rms(3));
    println!("real take: pause {pause_src:.2} → {pause_out:.2} dB; 3 kHz {band_src:.2} → {band_out:.2} dB ({:+.2})", band_out - band_src);
    assert!(pause_out <= -70.0, "pause {pause_out:.2} dB (limit −70)");
    assert!(band_out >= band_src - 1.0, "3 kHz lost {:.2} dB (limit 1)", band_src - band_out);
}
