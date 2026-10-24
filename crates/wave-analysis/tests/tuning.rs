use soundcraft_wave_analysis::testsignal::note;
use soundcraft_wave_analysis::{ANALYSIS_RATE, analysis_signal, resample, tuning};

fn rms_db(x: &[f32]) -> f32 {
    10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).max(1e-30).log10()
}

fn sine(hz: f32, secs: f32, sr: u32) -> Vec<f32> {
    (0..(secs * sr as f32) as usize).map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / sr as f32).sin()).collect()
}

fn crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
}

#[test]
fn resample_keeps_a_tone() {
    let x = sine(1000.0, 2.0, 48_000);
    let y = resample(&x, 48_000, 22_050);
    // Skip the edges, where the filter sees zeros.
    let (xm, ym) = (&x[4800..x.len() - 4800], &y[2205..y.len() - 2205]);
    assert!((rms_db(xm) - rms_db(ym)).abs() < 0.1, "{} vs {}", rms_db(xm), rms_db(ym));
    let expected = 1000.0 * ym.len() as f32 / 22_050.0;
    assert!((crossings(ym) as f32 - expected).abs() <= 1.0, "{} crossings, expected {expected}", crossings(ym));
}

#[test]
fn resample_rejects_above_nyquist() {
    let x = sine(12_000.0, 1.0, 48_000);
    let y = resample(&x, 48_000, 22_050);
    let mid = &y[2205..y.len() - 2205];
    assert!(rms_db(mid) < rms_db(&x) - 60.0, "only {} dB down", rms_db(&x) - rms_db(mid));
}

#[test]
fn resample_length_and_identity() {
    let x = sine(440.0, 1.0, 44_100);
    assert_eq!(resample(&x, 44_100, 44_100), x);
    assert_eq!(resample(&x, 44_100, 22_050).len(), 22_050);
    assert_eq!(resample(&x[..1000], 48_000, 22_050).len(), 459);
    assert!(resample(&x, 0, 22_050).is_empty());
    assert!(resample(&[], 48_000, 22_050).is_empty());
}

#[test]
fn analysis_signal_is_mono_at_the_analysis_rate() {
    let l = sine(440.0, 1.0, 48_000);
    let r: Vec<f32> = l.iter().map(|v| -v).chain([f32::NAN]).collect();
    let y = analysis_signal(&[l, r], 48_000);
    assert_eq!(y.len(), ANALYSIS_RATE as usize);
    assert!(y.iter().all(|v| v.is_finite() && v.abs() < 1e-3));
}

fn reference_notes(a4: f32) -> Vec<f32> {
    let sr = ANALYSIS_RATE;
    let mut x = Vec::new();
    for m in [57.0, 61.0, 64.0, 69.0] {
        x.extend(note(m, 1.5, a4, sr));
    }
    x
}

#[test]
fn tuning_finds_a_detuned_reference() {
    for (a4, tol) in [(435.0, 1.0), (446.0, 1.0), (440.0, 0.5)] {
        let t = tuning::estimate(&reference_notes(a4), ANALYSIS_RATE);
        assert!((t.a4_hz - a4).abs() <= tol, "A4 {a4}: estimated {}", t.a4_hz);
        assert!(t.confidence > 0.5, "A4 {a4}: confidence {}", t.confidence);
    }
}

#[test]
fn tuning_of_noise_has_low_confidence() {
    let mut s = 0x2468_ace1u32;
    let x: Vec<f32> = (0..ANALYSIS_RATE as usize * 4)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as f32 / u32::MAX as f32 - 0.5
        })
        .collect();
    assert!(tuning::estimate(&x, ANALYSIS_RATE).confidence < 0.3);
    let empty = tuning::estimate(&[], ANALYSIS_RATE);
    assert_eq!((empty.a4_hz, empty.confidence), (440.0, 0.0));
}
