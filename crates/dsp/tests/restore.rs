//! Restoration DSP: the shared STFT, the noise print, the gain rule and the restoration plugins.

use soundcraft_dsp::restore::{BANDS, Complex, NoisePrint, Stft, band_lo_hz, e1, lsa_gain};

const SR: f32 = 48_000.0;

fn noise(channels: usize, frames: usize, sigma: f32, seed: u32) -> Vec<Vec<f32>> {
    let mut s = seed.max(1);
    (0..channels)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    // Sum of 4 uniforms: close enough to Gaussian, unit variance after scaling.
                    let mut acc = 0.0f32;
                    for _ in 0..4 {
                        s ^= s << 13;
                        s ^= s >> 17;
                        s ^= s << 5;
                        acc += s as f32 / u32::MAX as f32 - 0.5;
                    }
                    acc * sigma * (12.0f32 / 4.0).sqrt()
                })
                .collect()
        })
        .collect()
}

fn run_stft(stft: &mut Stft, input: &[Vec<f32>], block: usize) -> Vec<Vec<f32>> {
    let len = input[0].len();
    let mut out = vec![Vec::with_capacity(len); input.len()];
    let mut buf = vec![vec![0.0f32; block]; input.len()];
    let mut pos = 0;
    let mut keep = |_: usize, _: &mut [Complex<f32>]| {};
    while pos < len {
        let n = block.min(len - pos);
        for (b, c) in buf.iter_mut().zip(input) {
            b[..n].copy_from_slice(&c[pos..pos + n]);
        }
        stft.process(&mut buf, n, &mut keep);
        for (o, b) in out.iter_mut().zip(&buf) {
            o.extend_from_slice(&b[..n]);
        }
        pos += n;
    }
    out
}

#[test]
fn stft_untouched_spectrum_is_identity_delayed() {
    for fft in [1024, 4096] {
        let x = noise(2, 30_000, 0.1, 7);
        let mut stft = Stft::new();
        stft.prepare(2);
        stft.set_size(fft);
        let lat = stft.latency();
        let y = run_stft(&mut stft, &x, 333);
        for (xc, yc) in x.iter().zip(&y) {
            for n in 0..xc.len() - lat {
                assert!((yc[n + lat] - xc[n]).abs() < 1e-4, "fft {fft} n {n}: {} vs {}", yc[n + lat], xc[n]);
            }
        }
    }
}

#[test]
fn stft_set_size_changes_latency() {
    let mut stft = Stft::new();
    stft.prepare(1);
    stft.set_size(2048);
    assert_eq!((stft.size(), stft.latency()), (2048, 2048));
    stft.set_size(512);
    assert_eq!((stft.size(), stft.latency()), (512, 512));
    stft.set_size(1000);
    assert_eq!(stft.size(), 512, "a size outside the allowed list is ignored");
}

#[test]
fn print_of_white_noise_is_flat() {
    let sigma = 0.01f32;
    let x = noise(1, (SR * 4.0) as usize, sigma, 3);
    let p = NoisePrint::capture(&x, SR as u32, 4096).unwrap();
    let want = 10.0 * (sigma * sigma).log10();
    let bands = p.bands_db();
    for (i, b) in bands.iter().enumerate() {
        let lo = band_lo_hz(i);
        if (100.0..20_000.0).contains(&lo) {
            assert!((b - want).abs() <= 1.5, "band {i} ({lo} Hz): {b} vs {want}");
        }
    }
}

#[test]
fn print_bin_psd_matches_across_fft_sizes() {
    let x = noise(1, (SR * 4.0) as usize, 0.02, 11);
    let p = NoisePrint::capture(&x, SR as u32, 4096).unwrap();
    let at = |fft: usize| {
        let mut out = vec![0.0f32; fft / 2 + 1];
        p.bin_psd(0, fft, SR as u32, &mut out);
        let k = (1000.0 * fft as f32 / SR).round() as usize;
        10.0 * out[k].log10()
    };
    let (a, b, c) = (at(1024), at(4096), at(16384));
    assert!((a - b).abs() < 1.0 && (c - b).abs() < 1.0, "{a} {b} {c}");
}

#[test]
fn print_bands_round_trip() {
    let x = noise(2, (SR * 2.0) as usize, 0.05, 5);
    let p = NoisePrint::capture(&x, SR as u32, 4096).unwrap();
    let q = NoisePrint::from_bands_db(&p.bands_db(), SR as u32);
    for (i, (a, b)) in p.bands_db().iter().zip(q.bands_db()).enumerate() {
        assert!((a - b).abs() <= 0.5, "band {i}: {a} vs {b}");
    }
    assert_eq!(p.bands_db().len(), BANDS);
}

#[test]
fn print_short_input() {
    let p = NoisePrint::capture(&noise(1, 300, 0.1, 9), SR as u32, 4096).unwrap();
    assert_eq!(p.fft_size, 256);
    assert!(NoisePrint::capture(&noise(1, 100, 0.1, 9), SR as u32, 4096).is_err());
    assert!(NoisePrint::capture(&[], SR as u32, 4096).is_err());
}

#[test]
fn print_json_round_trip() {
    let p = NoisePrint::capture(&noise(2, 9000, 0.1, 13), 44_100, 1024).unwrap();
    let text = serde_json::to_string(&p).unwrap();
    let q: NoisePrint = serde_json::from_str(&text).unwrap();
    assert_eq!(p, q);
}

#[test]
fn lsa_gain_limits() {
    assert!(lsa_gain(1e3, 1e3) > 0.99);
    assert!(lsa_gain(1e-3, 1.0) < 0.05);
    for (xi, gamma) in [(f32::NAN, 1.0), (1.0, f32::NAN), (f32::INFINITY, 1.0), (1.0, f32::INFINITY), (0.0, 0.0), (-1.0, -1.0)] {
        let g = lsa_gain(xi, gamma);
        assert!((0.0..=1.0).contains(&g), "lsa_gain({xi}, {gamma}) = {g}");
    }
}

#[test]
fn e1_known_values() {
    for (x, want) in [(0.1f32, 1.8229f32), (1.0, 0.21938), (5.0, 0.001_148_3)] {
        let got = e1(x);
        assert!((got - want).abs() <= 0.01 * want, "E1({x}) = {got}, want {want}");
    }
}

// ---- shared helpers for the plugin tests --------------------------------------------------------

use soundcraft_dsp::biquad::{Biquad, FilterType};
use soundcraft_dsp::offline::apply_plugin;
use soundcraft_dsp::{Plugin, create};

fn db(x: f32) -> f32 {
    10.0 * x.max(1e-30).log10()
}

fn rms_db(x: &[f32]) -> f32 {
    db(x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32)
}

fn sine(hz: f32, rms_dbfs: f32, frames: usize) -> Vec<f32> {
    let a = 10f32.powf(rms_dbfs / 20.0) * std::f32::consts::SQRT_2;
    (0..frames).map(|i| a * (std::f32::consts::TAU * hz * i as f32 / SR).sin()).collect()
}

fn filtered(x: &[f32], stages: &[(FilterType, f32, f32)]) -> Vec<f32> {
    let mut y = x.to_vec();
    for &(kind, hz, q) in stages {
        let mut f = Biquad::design(kind, hz, q, 0.0, SR);
        f.process_block(&mut y);
    }
    y
}

/// RMS in dB of `x[from..to]` (seconds) after `stages`.
fn band_db(x: &[f32], stages: &[(FilterType, f32, f32)], from: f32, to: f32) -> f32 {
    let y = filtered(x, stages);
    rms_db(&y[(from * SR) as usize..((to * SR) as usize).min(y.len())])
}

/// 4–8 kHz, steep enough below (8th order) that a −20 dBFS 440 Hz tone leaks under −150 dB.
const HIGHS: [(FilterType, f32, f32); 6] = [
    (FilterType::HighPass, 4000.0, 0.707),
    (FilterType::HighPass, 4000.0, 0.707),
    (FilterType::HighPass, 4000.0, 0.707),
    (FilterType::HighPass, 4000.0, 0.707),
    (FilterType::LowPass, 8000.0, 0.707),
    (FilterType::LowPass, 8000.0, 0.707),
];
const TONE: [(FilterType, f32, f32); 2] = [(FilterType::BandPass, 440.0, 8.0), (FilterType::BandPass, 440.0, 8.0)];

/// 1 s of white noise at −70 dBFS, then 3 s of 440 Hz at −20 dBFS over the same noise (mono).
fn synthetic() -> Vec<f32> {
    let n = (SR * 4.0) as usize;
    let mut x = noise(1, n, 10f32.powf(-70.0 / 20.0), 21).remove(0);
    let tone = sine(440.0, -20.0, n);
    for (i, v) in x.iter_mut().enumerate().skip(SR as usize) {
        *v += tone[i];
    }
    x
}

fn print_of_first_second(x: &[f32]) -> NoisePrint {
    NoisePrint::capture(&[x[..SR as usize].to_vec()], SR as u32, 4096).unwrap()
}

fn processed(x: &[f32], plugin: &mut dyn Plugin) -> Vec<f32> {
    let mut ch = vec![x.to_vec()];
    apply_plugin(&mut ch, plugin, SR);
    ch.remove(0)
}

// ---- noise reduction ------------------------------------------------------------------------------

use soundcraft_dsp::restore::NoiseReduction;

#[test]
fn nr_synthetic_tone_kept_noise_lowered() {
    let x = synthetic();
    let mut nr = NoiseReduction::with_print(print_of_first_second(&x));
    let y = processed(&x, &mut nr);
    let tone = (band_db(&x, &TONE, 2.0, 4.0), band_db(&y, &TONE, 2.0, 4.0));
    assert!((tone.0 - tone.1).abs() <= 1.0, "tone {tone:?}");
    let highs = (band_db(&x, &HIGHS, 2.0, 4.0), band_db(&y, &HIGHS, 2.0, 4.0));
    assert!(highs.0 - highs.1 >= 20.0, "4-8 kHz {highs:?}");
}

#[test]
fn nr_noise_only_is_the_difference() {
    let x = synthetic();
    let p = print_of_first_second(&x);
    let y = processed(&x, &mut NoiseReduction::with_print(p.clone()));
    let mut only = NoiseReduction::with_print(p);
    assert!(only.set_param("noise_only", 1.0));
    let n = processed(&x, &mut only);
    let residual: Vec<f32> = x.iter().zip(&y).zip(&n).map(|((a, b), c)| a - b - c).collect();
    assert!(rms_db(&residual) < -60.0, "residual {}", rms_db(&residual));
}

#[test]
fn nr_band_params_reduce_noise_in_the_rack_path() {
    let x = synthetic();
    let bands = print_of_first_second(&x).bands_db();
    let mut nr = create("noise_reduction").unwrap();
    for (i, b) in bands.iter().enumerate() {
        assert!(nr.set_param(&format!("nb{i:02}"), *b), "nb{i:02}");
    }
    let y = processed(&x, nr.as_mut());
    let highs = (band_db(&x, &HIGHS, 2.0, 4.0), band_db(&y, &HIGHS, 2.0, 4.0));
    assert!(highs.0 - highs.1 >= 15.0, "4-8 kHz {highs:?}");
}

#[test]
fn nr_without_print_passes_through() {
    let x = synthetic();
    let mut nr = create("noise_reduction").unwrap();
    let y = processed(&x, nr.as_mut());
    let worst = x.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "worst {worst}");
}
