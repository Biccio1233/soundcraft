//! Reference-tuning estimate: how far the music sits from A4 = 440 Hz.

use crate::spectrum::magnitudes;

const FFT: usize = 8192;
const LO_HZ: f32 = 80.0;
const HI_HZ: f32 = 2000.0;
/// Peaks this far below the loudest bin of their frame are ignored.
const RANGE_DB: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tuning {
    /// The reference the music is tuned to.
    pub a4_hz: f32,
    /// Its offset from 440 Hz in cents.
    pub cents: f32,
    /// How consistently the peaks agree, 0 (no tuning) to 1.
    pub confidence: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { a4_hz: 440.0, cents: 0.0, confidence: 0.0 }
    }
}

/// The tuning of `x` (at `sr`): the magnitude-weighted circular mean of the spectral peaks' offsets
/// from the equal-tempered grid, between 80 Hz and 2 kHz.
pub fn estimate(x: &[f32], sr: u32) -> Tuning {
    if sr == 0 {
        return Tuning::default();
    }
    let bin_hz = sr as f32 / FFT as f32;
    let (lo, hi) = ((LO_HZ / bin_hz).ceil() as usize, (HI_HZ / bin_hz).floor() as usize);
    let (mut sc, mut ss, mut sw) = (0.0f64, 0.0f64, 0.0f64);
    for frame in magnitudes(x, FFT, FFT / 2) {
        let top = frame.iter().copied().fold(0.0f32, f32::max);
        if top <= 1e-9 {
            continue;
        }
        let floor = top * 10f32.powf(-RANGE_DB / 20.0);
        for k in lo.max(2)..hi.min(frame.len().saturating_sub(2)) {
            let (Some(&m), Some(&a), Some(&b), Some(&a2), Some(&b2)) =
                (frame.get(k), frame.get(k - 1), frame.get(k + 1), frame.get(k - 2), frame.get(k + 2))
            else {
                continue;
            };
            if m < floor || m <= a || m < b || m <= a2 || m < b2 {
                continue;
            }
            let (la, lm, lb) = (a.max(1e-12).ln(), m.ln(), b.max(1e-12).ln());
            let den = la - 2.0 * lm + lb;
            let delta = if den.abs() > 1e-12 { (0.5 * (la - lb) / den).clamp(-0.5, 0.5) } else { 0.0 };
            let hz = (k as f32 + delta) * bin_hz;
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            let dev = f64::from(midi - midi.round());
            let w = f64::from(m);
            let angle = std::f64::consts::TAU * dev;
            sc += w * angle.cos();
            ss += w * angle.sin();
            sw += w;
        }
    }
    if sw <= 0.0 {
        return Tuning::default();
    }
    let cents = (ss.atan2(sc) / std::f64::consts::TAU * 100.0) as f32;
    let confidence = ((sc * sc + ss * ss).sqrt() / sw) as f32;
    Tuning { a4_hz: 440.0 * 2f32.powf(cents / 1200.0), cents, confidence: confidence.clamp(0.0, 1.0) }
}
