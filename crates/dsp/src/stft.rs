//! Short-time Fourier transform on a log-frequency axis, for spectral displays and analysis.

use rustfft::{FftPlanner, num_complex::Complex};

/// Most cells (frames × bands) one call may produce.
pub const MAX_CELLS: usize = 1 << 20;

/// Lower edge in Hz of log-spaced band `i` of `bands` between `f_min` and `f_max`
/// (`band_edge_hz(bands, …) == f_max`).
pub fn band_edge_hz(i: usize, bands: usize, f_min: f32, f_max: f32) -> f32 {
    if bands == 0 || f_min <= 0.0 || f_max <= f_min {
        return f_min.max(0.0);
    }
    f_min * (f_max / f_min).powf(i as f32 / bands as f32)
}

/// Hann-windowed STFT of `x`, one row per frame (`hop` apart), each row `bands` values in dBFS on
/// log-spaced bands from `f_min` to Nyquist (power mean of the bins in a band, or the bin under
/// its centre when the band is narrower than a bin). Floored at -144 dB.
pub fn spectrogram(x: &[f32], sample_rate: u32, fft_size: usize, hop: usize, bands: usize, f_min: f32) -> Result<Vec<Vec<f32>>, &'static str> {
    if !fft_size.is_power_of_two() || !(64..=65_536).contains(&fft_size) {
        return Err("fft_size must be a power of two in 64..=65536");
    }
    if hop == 0 || bands == 0 {
        return Err("hop and bands must be positive");
    }
    if x.is_empty() {
        return Err("no samples");
    }
    let nyquist = sample_rate as f32 / 2.0;
    if !(f_min > 0.0 && f_min < nyquist) {
        return Err("f_min must be between 0 and Nyquist");
    }
    let frames = if x.len() <= fft_size { 1 } else { 1 + (x.len() - fft_size) / hop };
    if frames.saturating_mul(bands) > MAX_CELLS {
        return Err("spectrogram too large");
    }
    let bin_hz = sample_rate as f32 / fft_size as f32;
    let half = fft_size / 2;
    // For each band, the half-open bin range it averages (at least one bin).
    let ranges: Vec<(usize, usize)> = (0..bands)
        .map(|b| {
            let (lo, hi) = (band_edge_hz(b, bands, f_min, nyquist), band_edge_hz(b + 1, bands, f_min, nyquist));
            let (a, z) = ((lo / bin_hz).ceil() as usize, (hi / bin_hz).ceil() as usize);
            if z > a {
                (a.min(half), z.min(half + 1))
            } else {
                let c = (((lo + hi) / 2.0) / bin_hz).round() as usize;
                (c.min(half), (c + 1).min(half + 1))
            }
        })
        .collect();
    let window: Vec<f32> = (0..fft_size).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / fft_size as f32).cos()).collect();
    let fft = FftPlanner::<f32>::new().plan_fft_forward(fft_size);
    let scale = 2.0 / (fft_size as f32 * 0.5);
    let mut buf = vec![Complex::new(0.0f32, 0.0); fft_size];
    let mut power = vec![0.0f32; half + 1];
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let start = f.saturating_mul(hop);
        for (i, (c, w)) in buf.iter_mut().zip(&window).enumerate() {
            let v = x.get(start + i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
            *c = Complex::new(v * w, 0.0);
        }
        fft.process(&mut buf);
        for (p, c) in power.iter_mut().zip(&buf) {
            let m = c.norm() * scale;
            *p = m * m;
        }
        let row = ranges
            .iter()
            .map(|&(a, z)| {
                let slice = power.get(a..z).unwrap_or(&[]);
                let mean = slice.iter().sum::<f32>() / slice.len().max(1) as f32;
                (10.0 * mean.max(1e-30).log10()).max(-144.0)
            })
            .collect();
        out.push(row);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, secs: f32) -> Vec<f32> {
        let n = (48_000.0 * secs) as usize;
        (0..n).map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin()).collect()
    }

    #[test]
    fn spectrogram_finds_a_440_tone() {
        let frames = spectrogram(&tone(440.0, 1.0), 48_000, 4096, 1024, 256, 20.0).unwrap();
        assert!(!frames.is_empty());
        let mid = &frames[frames.len() / 2];
        assert_eq!(mid.len(), 256);
        let (peak, _) = mid.iter().enumerate().fold((0, f32::MIN), |b, (i, &v)| if v > b.1 { (i, v) } else { b });
        // ±1 band: the Hann main lobe can put the neighbouring band on top.
        let lo = band_edge_hz(peak.saturating_sub(1), 256, 20.0, 24_000.0);
        let hi = band_edge_hz(peak + 2, 256, 20.0, 24_000.0);
        assert!((lo..=hi).contains(&440.0), "peak band {lo}..{hi}");
        let mut sorted = mid.clone();
        sorted.sort_by(f32::total_cmp);
        assert!(mid[peak] - sorted[sorted.len() / 2] >= 40.0);
    }

    #[test]
    fn spectrogram_rejects_bad_sizes() {
        let x = tone(440.0, 0.1);
        assert!(spectrogram(&x, 48_000, 1000, 256, 64, 20.0).is_err());
        assert!(spectrogram(&x, 48_000, 1024, 0, 64, 20.0).is_err());
        assert!(spectrogram(&x, 48_000, 1024, 256, 0, 20.0).is_err());
        assert!(spectrogram(&[], 48_000, 1024, 256, 64, 20.0).is_err());
    }

    #[test]
    fn spectrogram_caps_its_output() {
        let x = vec![0.0f32; 1 << 21];
        assert!(spectrogram(&x, 48_000, 64, 1, 4096, 20.0).is_err());
    }
}
