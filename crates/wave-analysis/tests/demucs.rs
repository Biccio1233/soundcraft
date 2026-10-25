use soundcraft_wave_analysis::demucs::{self, BINS, FRAMES, RATE, SEGMENT};
use soundcraft_wave_analysis::testsignal::{noise, sdr};

/// Noise with nothing at Nyquist (a two-tap average): the export drops the Nyquist bin, so only
/// such signals come back exactly.
fn stereo_noise(len: usize, seed: u32) -> [Vec<f32>; 2] {
    let smooth = |n: Vec<f32>| -> Vec<f32> { (0..len).map(|i| 0.25 * (n[i] + n.get(i + 1).copied().unwrap_or(0.0))).collect() };
    [smooth(noise(len + 1, seed)), smooth(noise(len + 1, seed + 1))]
}

/// The segment edges carry the dropped frames (as in the original): compare inside them.
const EDGE: usize = 4096;

#[test]
fn istft_inverts_stft() {
    let x = stereo_noise(SEGMENT, 3);
    let spec = demucs::stft_cac(&x);
    assert_eq!(spec.len(), 4 * BINS * FRAMES);
    let y = demucs::istft_cac(&spec);
    for c in 0..2 {
        assert_eq!(y[c].len(), SEGMENT);
        let err = x[c][EDGE..SEGMENT - EDGE].iter().zip(&y[c][EDGE..SEGMENT - EDGE]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "channel {c}: max error {err}");
    }
}

fn stub(sources: usize) -> impl FnMut(&[f32], &[f32]) -> Result<(Vec<f32>, Vec<f32>), String> {
    move |mix: &[f32], mag: &[f32]| {
        assert_eq!(mix.len(), 2 * SEGMENT);
        assert_eq!(mag.len(), 4 * BINS * FRAMES);
        let spec: Vec<f32> = (0..sources).flat_map(|_| mag.iter().map(|v| v / sources as f32)).collect();
        Ok((spec, vec![0.0; sources * 2 * SEGMENT]))
    }
}

#[test]
fn stub_network_rebuilds_the_mix() {
    // 20 s: three full chunks and a short centred tail.
    let len = 20 * RATE as usize;
    let x = stereo_noise(len, 11);
    let mean = x[0].iter().zip(&x[1]).map(|(a, b)| f64::from(a + b) / 2.0).sum::<f64>() / len as f64;
    let mut net = stub(4);
    let mut last = 0.0f32;
    let stems = demucs::separate_with(&x, 4, &mut net, &mut |p| last = p).unwrap();
    assert_eq!(stems.len(), 4);
    assert!((last - 1.0).abs() < 1e-6);
    for stem in &stems {
        for c in 0..2 {
            assert_eq!(stem[c].len(), len);
            let err = stem[c][EDGE..len - EDGE]
                .iter()
                .zip(&x[c][EDGE..len - EDGE])
                .map(|(s, m)| (f64::from(*s) - ((f64::from(*m) - mean) / 4.0 + mean)).abs())
                .fold(0.0f64, f64::max);
            // Segment starts inside the file inherit a little of the edge error (weighted ~0).
            assert!(err < 5e-3, "max error {err}");
        }
    }
}

#[test]
fn short_and_empty_inputs() {
    let x = stereo_noise(RATE as usize, 5);
    let stems = demucs::separate_with(&x, 4, &mut stub(4), &mut |_| {}).unwrap();
    assert!(stems.iter().all(|s| s[0].len() == RATE as usize && s[1].len() == RATE as usize));
    let empty = demucs::separate_with(&[vec![], vec![]], 4, &mut stub(4), &mut |_| {}).unwrap();
    assert!(empty.len() == 4 && empty.iter().all(|s| s[0].is_empty()));
    let mut wrong = |_: &[f32], _: &[f32]| Ok((vec![0.0; 10], vec![0.0; 10]));
    assert!(demucs::separate_with(&x, 4, &mut wrong, &mut |_| {}).is_err());
    let mut failing = |_: &[f32], _: &[f32]| Err::<(Vec<f32>, Vec<f32>), String>("boom".into());
    assert_eq!(demucs::separate_with(&x, 4, &mut failing, &mut |_| {}).unwrap_err(), "boom");
}

#[test]
fn sdr_of_known_signals() {
    let x = noise(10_000, 1);
    assert!(sdr(&x, &x) > 100.0);
    let half: Vec<f32> = x.iter().map(|v| 0.5 * v).collect();
    assert!((sdr(&x, &half) - 6.0206).abs() < 0.01, "{}", sdr(&x, &half));
}
