//! Constant-Q transform with Brown–Puckette sparse spectral kernels: each bin is the correlation of
//! the signal with a Hann-windowed complex exponential whose length holds `Q_FACTOR × Q` periods.

use crate::spectrum::hann;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// Kernel entries below this fraction of their bin's peak are dropped.
const SPARSITY: f32 = 0.0054;
/// Window length in units of the classic Q: at 2, a Hann kernel puts the neighbouring bin on its
/// first null instead of 6 dB down, so semitones stay apart at 12 bins per octave.
const Q_FACTOR: f32 = 2.0;
/// The longest kernel the transform accepts (2^17 samples).
const MAX_FFT: usize = 1 << 17;

pub struct Cqt {
    fft_size: usize,
    hop: usize,
    fft: Arc<dyn Fft<f32>>,
    /// Per bin, the nonzero entries of its conjugated spectral kernel, scaled by 1/fft_size.
    kernels: Vec<Vec<(usize, Complex<f32>)>>,
}

impl Cqt {
    /// Bins from `fmin_hz` up, `bins_per_octave` to the octave, frames every `hop` samples at `sr`.
    pub fn new(sr: u32, fmin_hz: f32, bins: usize, bins_per_octave: usize, hop: usize) -> Result<Cqt, &'static str> {
        if sr == 0 || hop == 0 || bins == 0 || bins_per_octave == 0 || fmin_hz.is_nan() || fmin_hz <= 0.0 {
            return Err("constant-Q transform: rate, hop, bins and lowest frequency must be positive");
        }
        let top = fmin_hz * 2f32.powf((bins - 1) as f32 / bins_per_octave as f32);
        if top >= sr as f32 / 2.0 {
            return Err("constant-Q transform: the top bin is above Nyquist");
        }
        let q = Q_FACTOR / (2f32.powf(1.0 / bins_per_octave as f32) - 1.0);
        let longest = (q * sr as f32 / fmin_hz).ceil() as usize;
        let fft_size = longest.next_power_of_two();
        if fft_size > MAX_FFT {
            return Err("constant-Q transform: the lowest bin needs too long a kernel");
        }
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let mut buf = vec![Complex::new(0.0f32, 0.0); fft_size];
        let kernels = (0..bins)
            .map(|k| {
                let f = fmin_hz * 2f32.powf(k as f32 / bins_per_octave as f32);
                let len = ((q * sr as f32 / f).ceil() as usize).clamp(1, fft_size);
                let w = hann(len);
                let offset = (fft_size - len) / 2;
                buf.iter_mut().for_each(|b| *b = Complex::new(0.0, 0.0));
                for (n, wn) in w.iter().enumerate() {
                    let phase = std::f32::consts::TAU * q * n as f32 / len as f32;
                    if let Some(b) = buf.get_mut(offset + n) {
                        *b = Complex::from_polar(wn / len as f32, phase);
                    }
                }
                fft.process(&mut buf);
                let peak = buf.iter().map(|c| c.norm()).fold(0.0f32, f32::max);
                buf.iter().enumerate().filter(|(_, c)| c.norm() >= peak * SPARSITY).map(|(j, c)| (j, c.conj() / fft_size as f32)).collect()
            })
            .collect();
        Ok(Cqt { fft_size, hop, fft, kernels })
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Magnitudes (frames × bins); frame `f` is centred on sample `f × hop`, `ceil(len / hop)` frames.
    pub fn magnitudes(&self, x: &[f32]) -> Vec<Vec<f32>> {
        if x.is_empty() {
            return Vec::new();
        }
        let frames = x.len().div_ceil(self.hop);
        let half = self.fft_size / 2;
        let mut buf = vec![Complex::new(0.0f32, 0.0); self.fft_size];
        (0..frames)
            .map(|f| {
                let centre = f * self.hop;
                for (i, b) in buf.iter_mut().enumerate() {
                    let v = (centre + i).checked_sub(half).and_then(|s| x.get(s)).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                    *b = Complex::new(v, 0.0);
                }
                self.fft.process(&mut buf);
                self.kernels
                    .iter()
                    .map(|kernel| {
                        let mut acc = Complex::new(0.0f32, 0.0);
                        for (j, k) in kernel {
                            if let Some(xj) = buf.get(*j) {
                                acc += xj * k;
                            }
                        }
                        acc.norm()
                    })
                    .collect()
            })
            .collect()
    }
}
