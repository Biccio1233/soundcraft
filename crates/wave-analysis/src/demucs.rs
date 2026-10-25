//! HTDemucs, host side: everything around the network that ONNX cannot express. It follows the
//! contract of the MIT export the owner chose (design, "Settled at the start of F2"): the mix is
//! normalised by the mean and standard deviation of its mono downmix, cut into 343,980-sample
//! segments a quarter overlapping, each turned into a "channels as complex" spectrogram with the
//! original's padding; the network returns a spectral and a waveform branch per source, the stem is
//! their sum, and the segments are cross-faded with a triangular window.

use crate::spectrum::hann;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub const RATE: u32 = 44_100;
pub const SEGMENT: usize = 343_980;
pub const NFFT: usize = 4096;
pub const HOP: usize = 1024;
/// Frequency bins kept (the Nyquist bin is dropped).
pub const BINS: usize = NFFT / 2;
/// Spectrogram frames per segment: `ceil(SEGMENT / HOP)`.
pub const FRAMES: usize = SEGMENT.div_ceil(HOP);
const PAD: usize = HOP / 2 * 3;
const RIGHT: usize = PAD + FRAMES * HOP - SEGMENT;
/// Frames of the doubly padded signal (two dropped at each end).
const TOTAL_FRAMES: usize = 1 + (SEGMENT + PAD + RIGHT) / HOP;
const OFFSET: usize = NFFT / 2 + PAD;
const OVERLAP: f32 = 0.25;

/// Reflect-pad `x` by `left` and `right` samples (the edge sample is not repeated).
fn reflect(x: &[f32], left: usize, right: usize) -> Vec<f32> {
    let n = x.len();
    if n == 0 {
        return vec![0.0; left + right];
    }
    let at = |i: isize| -> f32 {
        let last = n as isize - 1;
        let mut j = i;
        // Repeated reflection keeps any index inside, even for pads longer than the signal.
        while j < 0 || j > last {
            j = if j < 0 { -j } else { 2 * last - j };
            if last == 0 {
                j = 0;
            }
        }
        x.get(j as usize).copied().unwrap_or(0.0)
    };
    (-(left as isize)..(n + right) as isize).map(at).collect()
}

struct Plans {
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
}

fn plans() -> Plans {
    let mut p = FftPlanner::<f32>::new();
    Plans { fwd: p.plan_fft_forward(NFFT), inv: p.plan_fft_inverse(NFFT), window: hann(NFFT) }
}

/// The CaC spectrogram of one stereo segment: `[4, BINS, FRAMES]` row-major (planes: real L,
/// imaginary L, real R, imaginary R), Hann window, scaled by 1/√NFFT.
pub fn stft_cac(seg: &[Vec<f32>; 2]) -> Vec<f32> {
    stft_with(&plans(), seg)
}

fn stft_with(p: &Plans, seg: &[Vec<f32>; 2]) -> Vec<f32> {
    let mut out = vec![0.0f32; 4 * BINS * FRAMES];
    let scale = 1.0 / (NFFT as f32).sqrt();
    let mut buf = vec![Complex::new(0.0f32, 0.0); NFFT];
    for (c, ch) in seg.iter().enumerate() {
        let mut x: Vec<f32> = ch.iter().copied().take(SEGMENT).map(|v| if v.is_finite() { v } else { 0.0 }).collect();
        x.resize(SEGMENT, 0.0);
        let z = reflect(&reflect(&x, PAD, RIGHT), NFFT / 2, NFFT / 2);
        for t in 0..FRAMES {
            let start = (t + 2) * HOP;
            for (i, b) in buf.iter_mut().enumerate() {
                *b = Complex::new(z.get(start + i).copied().unwrap_or(0.0) * p.window.get(i).copied().unwrap_or(0.0), 0.0);
            }
            p.fwd.process(&mut buf);
            for (f, v) in buf.iter().take(BINS).enumerate() {
                let (re, im) = ((2 * c) * BINS + f, (2 * c + 1) * BINS + f);
                if let Some(o) = out.get_mut(re * FRAMES + t) {
                    *o = v.re * scale;
                }
                if let Some(o) = out.get_mut(im * FRAMES + t) {
                    *o = v.im * scale;
                }
            }
        }
    }
    out
}

/// The stereo segment of a CaC spectrogram (`[4, BINS, FRAMES]`), inverting [`stft_cac`].
pub fn istft_cac(spec: &[f32]) -> [Vec<f32>; 2] {
    istft_with(&plans(), spec)
}

fn istft_with(p: &Plans, spec: &[f32]) -> [Vec<f32>; 2] {
    let out_len = NFFT + HOP * (TOTAL_FRAMES - 1);
    let mut env = vec![0.0f32; out_len];
    for t in 0..TOTAL_FRAMES {
        for (i, w) in p.window.iter().enumerate() {
            if let Some(e) = env.get_mut(t * HOP + i) {
                *e += w * w;
            }
        }
    }
    let iscale = (NFFT as f32).sqrt() / NFFT as f32;
    let mut buf = vec![Complex::new(0.0f32, 0.0); NFFT];
    let mut result = [vec![0.0f32; SEGMENT], vec![0.0f32; SEGMENT]];
    for (c, res) in result.iter_mut().enumerate() {
        let mut out = vec![0.0f32; out_len];
        for t in 0..FRAMES {
            buf.iter_mut().for_each(|b| *b = Complex::new(0.0, 0.0));
            for f in 0..BINS {
                let re = spec.get(((2 * c) * BINS + f) * FRAMES + t).copied().unwrap_or(0.0);
                let im = spec.get(((2 * c + 1) * BINS + f) * FRAMES + t).copied().unwrap_or(0.0);
                let v = Complex::new(re, im);
                if let Some(b) = buf.get_mut(f) {
                    *b = v;
                }
                if f > 0
                    && let Some(b) = buf.get_mut(NFFT - f)
                {
                    *b = v.conj();
                }
            }
            p.inv.process(&mut buf);
            let at = (t + 2) * HOP;
            for (i, (b, w)) in buf.iter().zip(&p.window).enumerate() {
                if let Some(o) = out.get_mut(at + i) {
                    *o += b.re * iscale * w;
                }
            }
        }
        for (k, r) in res.iter_mut().enumerate() {
            let e = env.get(OFFSET + k).copied().unwrap_or(0.0);
            *r = if e > 1e-12 { out.get(OFFSET + k).copied().unwrap_or(0.0) / e } else { 0.0 };
        }
    }
    result
}

/// The triangular cross-fade of a segment: up over the first half, down over the second, peak 1.
fn transition_weight() -> Vec<f32> {
    let half = SEGMENT / 2;
    let peak = half.max(SEGMENT - half) as f32;
    (1..=half).chain((1..=SEGMENT - half).rev()).map(|v| v as f32 / peak).collect()
}

/// The network's call: `net(mix, mag)` with `mix` = `[1, 2, SEGMENT]` and `mag` = `[1, 4, BINS,
/// FRAMES]`, returning `(spec, wav)` = `([1, S, 4, BINS, FRAMES], [1, S, 2, SEGMENT])`.
pub type Network<'a> = dyn FnMut(&[f32], &[f32]) -> Result<(Vec<f32>, Vec<f32>), String> + 'a;

/// Separate a stereo 44.1 kHz signal into `sources` stereo stems of the same length; `progress`
/// (0 to 1, after each segment) returns false to cancel.
pub fn separate_with(
    stereo_44k: &[Vec<f32>; 2],
    sources: usize,
    net: &mut Network,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Vec<[Vec<f32>; 2]>, String> {
    let total = stereo_44k[0].len().min(stereo_44k[1].len());
    if total == 0 || sources == 0 {
        let _ = progress(1.0);
        return Ok((0..sources).map(|_| [Vec::new(), Vec::new()]).collect());
    }
    let clean = |v: f32| if v.is_finite() { v } else { 0.0 };
    let (sum, sum2) = stereo_44k[0].iter().zip(&stereo_44k[1]).take(total).fold((0.0f64, 0.0f64), |(s, s2), (l, r)| {
        let m = f64::from(clean(*l) + clean(*r)) / 2.0;
        (s + m, s2 + m * m)
    });
    let mean = sum / total as f64;
    let gain = ((sum2 / total as f64 - mean * mean).max(0.0)).sqrt() + 1e-8;
    let (mean_f, gain_f) = (mean as f32, gain as f32);
    let stride = (((1.0 - OVERLAP) * SEGMENT as f32) as usize).max(1);
    let offsets: Vec<usize> = (0..total).step_by(stride).collect();
    let weight = transition_weight();
    let p = plans();
    let mut acc: Vec<[Vec<f32>; 2]> = (0..sources).map(|_| [vec![0.0f32; total], vec![0.0f32; total]]).collect();
    let mut wsum = vec![0.0f32; total];
    for (k, &offset) in offsets.iter().enumerate() {
        let length = (total - offset).min(SEGMENT);
        let delta = SEGMENT - length;
        let start = offset as isize - (delta / 2) as isize;
        let mut mix = [vec![0.0f32; SEGMENT], vec![0.0f32; SEGMENT]];
        for (c, m) in mix.iter_mut().enumerate() {
            for (i, v) in m.iter_mut().enumerate() {
                let src = start + i as isize;
                if src >= 0
                    && let Some(x) = stereo_44k[c].get(src as usize).filter(|_| (src as usize) < total)
                {
                    *v = (clean(*x) - mean_f) / gain_f;
                }
            }
        }
        let mag = stft_with(&p, &mix);
        let flat: Vec<f32> = mix.iter().flatten().copied().collect();
        let (spec, wav) = net(&flat, &mag)?;
        let plane = 4 * BINS * FRAMES;
        if spec.len() != sources * plane || wav.len() != sources * 2 * SEGMENT {
            return Err(format!(
                "the separation network returned {} spectral and {} waveform values, expected {} and {}",
                spec.len(),
                wav.len(),
                sources * plane,
                sources * 2 * SEGMENT
            ));
        }
        let trim = delta / 2;
        for (s, stem) in acc.iter_mut().enumerate() {
            let branch = istft_with(&p, spec.get(s * plane..(s + 1) * plane).unwrap_or(&[]));
            for (c, out) in stem.iter_mut().enumerate() {
                let w0 = (s * 2 + c) * SEGMENT;
                for i in 0..length {
                    let v = wav.get(w0 + trim + i).copied().unwrap_or(0.0) + branch[c].get(trim + i).copied().unwrap_or(0.0);
                    if let (Some(o), Some(w)) = (out.get_mut(offset + i), weight.get(i)) {
                        *o += v * w;
                    }
                }
            }
        }
        for i in 0..length {
            if let (Some(o), Some(w)) = (wsum.get_mut(offset + i), weight.get(i)) {
                *o += w;
            }
        }
        if !progress((k + 1) as f32 / offsets.len() as f32) {
            return Err("separation cancelled".into());
        }
    }
    for stem in acc.iter_mut() {
        for ch in stem.iter_mut() {
            for (v, w) in ch.iter_mut().zip(&wsum) {
                *v = if *w > 0.0 { *v / w * gain_f + mean_f } else { mean_f };
            }
        }
    }
    Ok(acc)
}
