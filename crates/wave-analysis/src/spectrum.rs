//! Short-time spectra for the analyses.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

/// Periodic Hann window.
pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect()
}

/// Magnitude spectra (`n / 2 + 1` bins) of Hann-windowed frames of `n` samples every `hop`, the
/// first frame starting at sample 0; a signal shorter than `n` gives one zero-padded frame.
pub fn magnitudes(x: &[f32], n: usize, hop: usize) -> Vec<Vec<f32>> {
    if x.is_empty() || n == 0 || hop == 0 {
        return Vec::new();
    }
    let frames = if x.len() <= n { 1 } else { 1 + (x.len() - n) / hop };
    let window = hann(n);
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    (0..frames)
        .map(|f| {
            let start = f * hop;
            for (i, b) in buf.iter_mut().enumerate() {
                let v = x.get(start + i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                *b = Complex::new(v * window.get(i).copied().unwrap_or(0.0), 0.0);
            }
            fft.process(&mut buf);
            buf.iter().take(n / 2 + 1).map(|c| c.norm()).collect()
        })
        .collect()
}
