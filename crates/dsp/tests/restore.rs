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
