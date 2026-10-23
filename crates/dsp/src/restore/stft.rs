//! A streaming short-time Fourier transform for spectral processors: periodic Hann analysis and
//! synthesis windows, hop of a quarter frame, overlap-add. An untouched spectrum gives back the
//! input delayed by one frame. Buffers for the largest size are allocated in `prepare`, so
//! `process` and `set_size` never allocate.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// The frame sizes a processor may use.
pub const FFT_SIZES: [usize; 6] = [512, 1024, 2048, 4096, 8192, 16384];
/// `FFT_SIZES` as choice labels, for a `Unit::Choice` parameter.
pub const FFT_CHOICES: [&str; 6] = ["512", "1024", "2048", "4096", "8192", "16384"];

const MAX: usize = 16384;
const DEFAULT_SIZE: usize = 2048;

struct Plan {
    size: usize,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
}

/// Per-channel state.
#[derive(Default)]
struct Channel {
    input: Vec<f32>,
    output: Vec<f32>,
    accum: Vec<f32>,
}

pub struct Stft {
    size: usize,
    plans: Vec<Plan>,
    window: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    channels: Vec<Channel>,
    /// Write position in the input FIFO, from `size - hop` to `size`.
    rover: usize,
}

impl Default for Stft {
    fn default() -> Self {
        Self::new()
    }
}

impl Stft {
    pub fn new() -> Self {
        Self { size: DEFAULT_SIZE, plans: Vec::new(), window: Vec::new(), spectrum: Vec::new(), scratch: Vec::new(), channels: Vec::new(), rover: 0 }
    }

    /// Allocate for `channels` channels at the largest frame size and plan every size.
    pub fn prepare(&mut self, channels: usize) {
        if self.plans.is_empty() {
            let mut planner = FftPlanner::<f32>::new();
            self.plans = FFT_SIZES
                .iter()
                .map(|&size| Plan { size, forward: planner.plan_fft_forward(size), inverse: planner.plan_fft_inverse(size) })
                .collect();
            let scratch = self.plans.iter().map(|p| p.forward.get_inplace_scratch_len().max(p.inverse.get_inplace_scratch_len())).max().unwrap_or(0);
            self.scratch = vec![Complex::new(0.0, 0.0); scratch];
            self.spectrum = vec![Complex::new(0.0, 0.0); MAX];
            self.window = vec![0.0; MAX];
        }
        self.channels = (0..channels).map(|_| Channel { input: vec![0.0; MAX], output: vec![0.0; MAX], accum: vec![0.0; 2 * MAX] }).collect();
        self.fill_window();
        self.reset();
    }

    /// Switch to `fft` (one of `FFT_SIZES`; anything else is ignored). Resets the state.
    pub fn set_size(&mut self, fft: usize) {
        if fft == self.size || !FFT_SIZES.contains(&fft) {
            return;
        }
        self.size = fft;
        self.fill_window();
        self.reset();
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Samples between an input sample and the output it becomes.
    pub fn latency(&self) -> usize {
        self.size
    }

    pub fn hop(&self) -> usize {
        self.size / 4
    }

    pub fn reset(&mut self) {
        for c in &mut self.channels {
            c.input.iter_mut().for_each(|v| *v = 0.0);
            c.output.iter_mut().for_each(|v| *v = 0.0);
            c.accum.iter_mut().for_each(|v| *v = 0.0);
        }
        self.rover = self.size - self.hop();
    }

    fn fill_window(&mut self) {
        let n = self.size;
        for (i, w) in self.window.iter_mut().take(n).enumerate() {
            *w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
        }
    }

    /// Run `frames` samples of `io` through the transform; `f(channel, half_spectrum)` may change
    /// bins `0..=size/2` of each frame (the upper half is mirrored from them).
    pub fn process(&mut self, io: &mut [Vec<f32>], frames: usize, f: &mut dyn FnMut(usize, &mut [Complex<f32>])) {
        let n = self.size;
        let hop = self.hop();
        let latency_in = n - hop;
        if self.channels.is_empty() || self.window.len() < n {
            return;
        }
        for i in 0..frames {
            let out_at = self.rover - latency_in;
            for (c, chan) in self.channels.iter_mut().enumerate() {
                let Some(sample) = io.get_mut(c).and_then(|ch| ch.get_mut(i)) else { continue };
                let x = if sample.is_finite() { *sample } else { 0.0 };
                if let Some(slot) = chan.input.get_mut(self.rover) {
                    *slot = x;
                }
                *sample = chan.output.get(out_at).copied().unwrap_or(0.0);
            }
            self.rover += 1;
            if self.rover >= n {
                self.rover = latency_in;
                for c in 0..self.channels.len() {
                    self.frame(c, f);
                }
            }
        }
    }

    /// Transform the frame held in channel `c`'s input FIFO, let `f` change it, overlap-add it.
    fn frame(&mut self, c: usize, f: &mut dyn FnMut(usize, &mut [Complex<f32>])) {
        let n = self.size;
        let hop = n / 4;
        let Some(plan) = self.plans.iter().find(|p| p.size == n) else { return };
        let Some(chan) = self.channels.get_mut(c) else { return };
        let (spec, window) = (&mut self.spectrum[..n], &self.window[..n]);
        for ((s, x), w) in spec.iter_mut().zip(&chan.input[..n]).zip(window) {
            *s = Complex::new(x * w, 0.0);
        }
        plan.forward.process_with_scratch(spec, &mut self.scratch);
        f(c, &mut spec[..=n / 2]);
        for k in 1..n / 2 {
            spec[n - k] = spec[k].conj();
        }
        spec[0].im = 0.0;
        spec[n / 2].im = 0.0;
        plan.inverse.process_with_scratch(spec, &mut self.scratch);
        // Periodic Hann squared, overlapped at a quarter frame, sums to 1.5; the inverse FFT is
        // unnormalised (a factor n).
        let scale = 1.0 / (1.5 * n as f32);
        for ((a, s), w) in chan.accum[..n].iter_mut().zip(spec.iter()).zip(window) {
            let v = s.re * w * scale;
            *a += if v.is_finite() { v } else { 0.0 };
        }
        chan.output[..hop].copy_from_slice(&chan.accum[..hop]);
        chan.accum.copy_within(hop..n + hop, 0);
        chan.accum[n..n + hop].iter_mut().for_each(|v| *v = 0.0);
        chan.input.copy_within(hop..n, 0);
    }
}
