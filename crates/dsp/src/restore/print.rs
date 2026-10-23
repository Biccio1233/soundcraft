//! The noise print: the power spectrum of a stretch of noise alone, which noise reduction removes.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

/// Bands of a folded print: sixth-octave bands from 20 Hz (band 0 also holds everything below).
pub const BANDS: usize = 62;
/// The level a band with no print reads (a print of silence).
pub const FLOOR_DB: f32 = -160.0;

const MIN_FFT: usize = 256;
const MAX_FFT: usize = 16384;

/// Lower edge in Hz of band `i` (band 0 starts at 0 Hz; the last band runs to Nyquist).
pub fn band_lo_hz(i: usize) -> f32 {
    if i == 0 { 0.0 } else { 20.0 * 2f32.powf(i as f32 / 6.0) }
}

/// Geometric centre of band `i` (band 0's is taken as if it started at 20 Hz).
fn band_centre_hz(i: usize) -> f32 {
    20.0 * 2f32.powf((i as f32 + 0.5) / 6.0)
}

fn band_of(hz: f32) -> usize {
    if hz < band_lo_hz(1) {
        return 0;
    }
    ((6.0 * (hz / 20.0).log2()).floor() as usize).min(BANDS - 1)
}

fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect()
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NoisePrint {
    pub sample_rate: u32,
    pub fft_size: usize,
    /// Per channel, `fft_size / 2 + 1` bins of mean power divided by the window's energy, so
    /// white noise of variance σ² reads σ² in every bin at any frame size.
    pub psd: Vec<Vec<f32>>,
}

impl NoisePrint {
    /// Welch estimate of `ch` (frames of `fft` samples, a quarter apart). A selection shorter
    /// than `fft` uses the largest power of two that fits, down to 256 samples.
    pub fn capture(ch: &[Vec<f32>], sample_rate: u32, fft: usize) -> Result<NoisePrint, &'static str> {
        let len = ch.iter().map(Vec::len).min().unwrap_or(0);
        if ch.is_empty() || len == 0 {
            return Err("no audio to take a noise print from");
        }
        if sample_rate == 0 {
            return Err("the sample rate must be positive");
        }
        let mut n = fft.clamp(MIN_FFT, MAX_FFT).next_power_of_two().min(MAX_FFT);
        while n > len && n > MIN_FFT {
            n /= 2;
        }
        if n > len {
            return Err("select at least 256 samples of noise alone");
        }
        let hop = n / 4;
        let frames = 1 + (len - n) / hop;
        let window = hann(n);
        let energy: f32 = window.iter().map(|w| w * w).sum();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
        let mut buf = vec![Complex::new(0.0f32, 0.0); n];
        let psd = ch
            .iter()
            .map(|c| {
                let mut acc = vec![0.0f64; n / 2 + 1];
                for f in 0..frames {
                    let start = f * hop;
                    for ((b, x), w) in buf.iter_mut().zip(c.iter().skip(start)).zip(&window) {
                        let v = if x.is_finite() { *x } else { 0.0 };
                        *b = Complex::new(v * w, 0.0);
                    }
                    fft.process(&mut buf);
                    for (a, b) in acc.iter_mut().zip(&buf) {
                        *a += f64::from(b.norm_sqr());
                    }
                }
                acc.iter().map(|a| (*a / frames as f64 / f64::from(energy)) as f32).collect()
            })
            .collect();
        Ok(NoisePrint { sample_rate, fft_size: n, psd })
    }

    /// A one-channel print at 4096 points whose spectrum follows `bands` (dB per band,
    /// interpolated in dB over log frequency between band centres).
    pub fn from_bands_db(bands: &[f32], sample_rate: u32) -> NoisePrint {
        let n = 4096usize;
        let sr = sample_rate.max(1) as f32;
        let level = |b: usize| bands.get(b).copied().filter(|v| v.is_finite()).unwrap_or(FLOOR_DB).clamp(FLOOR_DB, 0.0);
        let psd = (0..=n / 2)
            .map(|k| {
                let hz = k as f32 * sr / n as f32;
                let b = band_of(hz);
                let c = band_centre_hz(b);
                let (lo, hi) = if hz < c { (b.saturating_sub(1), b) } else { (b, (b + 1).min(BANDS - 1)) };
                let (clo, chi) = (band_centre_hz(lo), band_centre_hz(hi));
                let db = if lo == hi || hz <= 0.0 {
                    level(b)
                } else {
                    let t = ((hz / clo).ln() / (chi / clo).ln()).clamp(0.0, 1.0);
                    level(lo) + (level(hi) - level(lo)) * t
                };
                10f32.powf(db / 10.0)
            })
            .collect();
        NoisePrint { sample_rate, fft_size: n, psd: vec![psd] }
    }

    /// Level of each band in dB, averaged over the channels.
    pub fn bands_db(&self) -> [f32; BANDS] {
        let mut out = [FLOOR_DB; BANDS];
        let bins = self.fft_size / 2 + 1;
        let bin_hz = self.sample_rate as f32 / self.fft_size.max(1) as f32;
        let mean_at = |k: usize| -> f32 {
            let v: f32 = self.psd.iter().map(|c| c.get(k).copied().unwrap_or(0.0)).sum();
            v / self.psd.len().max(1) as f32
        };
        for (b, slot) in out.iter_mut().enumerate() {
            let lo = band_lo_hz(b);
            let hi = if b + 1 == BANDS { f32::INFINITY } else { band_lo_hz(b + 1) };
            let (mut sum, mut count) = (0.0f32, 0usize);
            for k in 0..bins {
                let hz = k as f32 * bin_hz;
                if hz >= lo && hz < hi {
                    sum += mean_at(k);
                    count += 1;
                }
            }
            let power = if count > 0 {
                sum / count as f32
            } else {
                // A band narrower than a bin: interpolate at its centre.
                let pos = (band_centre_hz(b) / bin_hz).min((bins - 1) as f32);
                let (k0, t) = (pos.floor() as usize, pos.fract());
                mean_at(k0) * (1.0 - t) + mean_at((k0 + 1).min(bins - 1)) * t
            };
            *slot = if power > 0.0 && power.is_finite() { (10.0 * power.log10()).max(FLOOR_DB) } else { FLOOR_DB };
        }
        out
    }

    /// The print as seen by a `fft`-point transform at `sample_rate`: `fft / 2 + 1` values written
    /// to `out`. Coarser bins average the print's bins they cover; finer ones interpolate. Never
    /// allocates. A channel past the print's last uses the last.
    pub fn bin_psd(&self, ch: usize, fft: usize, sample_rate: u32, out: &mut [f32]) {
        let Some(src) = self.psd.get(ch.min(self.psd.len().saturating_sub(1))) else {
            out.iter_mut().for_each(|v| *v = 0.0);
            return;
        };
        let last = src.len().saturating_sub(1);
        if src.is_empty() || fft == 0 || self.sample_rate == 0 {
            out.iter_mut().for_each(|v| *v = 0.0);
            return;
        }
        // Source bins per target bin.
        let ratio = (sample_rate as f32 / fft as f32) / (self.sample_rate as f32 / self.fft_size as f32);
        for (j, v) in out.iter_mut().take(fft / 2 + 1).enumerate() {
            let pos = j as f32 * ratio;
            *v = if ratio > 1.0 {
                let a = (pos - ratio / 2.0).max(0.0).round() as usize;
                let z = ((pos + ratio / 2.0).round() as usize).max(a + 1);
                let (a, z) = (a.min(last), z.min(last + 1));
                let slice = src.get(a..z.max(a + 1)).unwrap_or(&[]);
                slice.iter().sum::<f32>() / slice.len().max(1) as f32
            } else {
                let p = pos.min(last as f32);
                let (k0, t) = (p.floor() as usize, p.fract());
                let a = src.get(k0).copied().unwrap_or(0.0);
                let b = src.get((k0 + 1).min(last)).copied().unwrap_or(a);
                a * (1.0 - t) + b * t
            };
        }
    }
}
