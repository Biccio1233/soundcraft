//! Adaptive noise reduction without a print: the noise spectrum is tracked by minimum statistics
//! (after Martin 2001: the periodogram smoothed with a time-varying factor that drops when the
//! power stands far above the noise estimate, its minimum over a sliding window kept in
//! sub-windows, times a bias factor) and removed with the same decision-directed log-spectral
//! amplitude rule as the print-based noise reduction. Steady tones count as noise.

use super::gain::dd_frame;
use super::stft::{FFT_CHOICES, FFT_SIZES, Stft};
use crate::params::{Params, choice, lin, param_plumbing, toggle};
use crate::util::{sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

const SUBWINDOWS: usize = 8;
/// Largest and smallest smoothing factor (Martin's optimal smoothing, clamped).
const SMOOTH_MAX: f32 = 0.96;
const SMOOTH_MIN: f32 = 0.7;
/// Compensates for the minimum of a smoothed periodogram sitting below its mean.
const BIAS: f32 = 1.5;
const MAX_BINS: usize = 16384 / 2 + 1;

/// Minimum-statistics noise tracker over `bins` power values per frame.
pub struct MinTracker {
    smoothed: Vec<f32>,
    current: Vec<f32>,
    subs: Vec<Vec<f32>>,
    noise: Vec<f32>,
    per_sub: usize,
    count: usize,
    next: usize,
    filled: usize,
    started: bool,
}

impl Default for MinTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl MinTracker {
    pub fn new() -> Self {
        Self {
            smoothed: Vec::new(),
            current: Vec::new(),
            subs: Vec::new(),
            noise: Vec::new(),
            per_sub: 1,
            count: 0,
            next: 0,
            filled: 0,
            started: false,
        }
    }

    /// Allocate for up to `bins_max` bins.
    pub fn prepare(&mut self, bins_max: usize) {
        self.smoothed = vec![0.0; bins_max];
        self.current = vec![f32::INFINITY; bins_max];
        self.subs = vec![vec![f32::INFINITY; bins_max]; SUBWINDOWS];
        self.noise = vec![0.0; bins_max];
        self.reset();
    }

    /// The search window in frames (split into eight sub-windows). No allocation.
    pub fn set_window(&mut self, frames: usize) {
        self.per_sub = (frames / SUBWINDOWS).max(1);
    }

    pub fn reset(&mut self) {
        self.current.iter_mut().for_each(|v| *v = f32::INFINITY);
        self.subs.iter_mut().for_each(|s| s.iter_mut().for_each(|v| *v = f32::INFINITY));
        self.noise.iter_mut().for_each(|v| *v = 0.0);
        self.count = 0;
        self.next = 0;
        self.filled = 0;
        self.started = false;
    }

    /// Feed one frame of power values (`power.len()` bins, at most the prepared number).
    pub fn update(&mut self, power: &[f32]) {
        let bins = power.len().min(self.smoothed.len());
        for k in 0..bins {
            let p = power.get(k).copied().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0);
            let s = &mut self.smoothed[k];
            let noise = self.noise[k];
            let ratio = if noise > 0.0 { *s / noise } else { 1.0 };
            let alpha = (SMOOTH_MAX / (1.0 + (ratio - 1.0) * (ratio - 1.0))).clamp(SMOOTH_MIN, SMOOTH_MAX);
            *s = if self.started { alpha * *s + (1.0 - alpha) * p } else { p };
            let c = &mut self.current[k];
            *c = c.min(*s);
        }
        self.started = true;
        self.count += 1;
        if self.count >= self.per_sub {
            if let Some(slot) = self.subs.get_mut(self.next) {
                slot[..bins].copy_from_slice(&self.current[..bins]);
            }
            self.next = (self.next + 1) % SUBWINDOWS;
            self.filled = (self.filled + 1).min(SUBWINDOWS);
            self.current[..bins].copy_from_slice(&self.smoothed[..bins]);
            self.count = 0;
        }
        for k in 0..bins {
            let mut m = self.current[k];
            for s in self.subs.iter().take(self.filled) {
                m = m.min(s[k]);
            }
            self.noise[k] = if m.is_finite() { BIAS * m } else { 0.0 };
        }
    }

    /// The noise power estimate per bin.
    pub fn noise(&self) -> &[f32] {
        &self.noise
    }
}

const FLOOR: usize = 0;
const OFFSET: usize = 1;
const SMOOTHING: usize = 2;
const FFT: usize = 3;
const WINDOW: usize = 4;
const NOISE_ONLY: usize = 5;

pub static ADAPTIVE_PARAMS: [ParamInfo; 6] = [
    lin("floor_db", "Reduce by", -60.0, 0.0, -20.0, Unit::Db),
    lin("offset_db", "Noise offset", -10.0, 20.0, 3.0, Unit::Db),
    lin("smoothing", "Smoothing", 90.0, 99.9, 98.0, Unit::Percent),
    choice("fft", "FFT size", &FFT_CHOICES, 2),
    lin("window_s", "Tracking window", 0.5, 5.0, 1.5, Unit::Seconds),
    toggle("noise_only", "Output noise only", false),
];

pub static ADAPTIVE_INFO: PluginInfo = PluginInfo {
    id: "adaptive_noise_reduction",
    name: "Adaptive Denoise",
    short_name: "AdaptDN",
    category: Category::Other,
    params: &ADAPTIVE_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

struct Lane {
    tracker: MinTracker,
    power: Vec<f32>,
    lambda: Vec<f32>,
    gain: Vec<f32>,
    gamma: Vec<f32>,
}

/// Adaptive noise reduction (see the module docs).
pub struct AdaptiveNoiseReduction {
    p: Params,
    sr: f32,
    stft: Stft,
    lanes: Vec<Lane>,
}

impl Default for AdaptiveNoiseReduction {
    fn default() -> Self {
        Self::new()
    }
}

impl AdaptiveNoiseReduction {
    pub fn new() -> Self {
        let mut a = Self { p: Params::new(&ADAPTIVE_PARAMS), sr: 48_000.0, stft: Stft::new(), lanes: Vec::new() };
        a.prepare(48_000.0, 1024, 2);
        a
    }

    fn fft(&self) -> usize {
        FFT_SIZES.get(self.p.choice(FFT)).copied().unwrap_or(2048)
    }

    fn update(&mut self) {
        let before = self.stft.size();
        self.stft.set_size(self.fft());
        let frames = (self.p.v(WINDOW) * self.sr / self.stft.hop().max(1) as f32) as usize;
        for lane in &mut self.lanes {
            lane.tracker.set_window(frames);
            if before != self.stft.size() {
                lane.tracker.reset();
                lane.gain.iter_mut().for_each(|v| *v = 1.0);
                lane.gamma.iter_mut().for_each(|v| *v = 1.0);
            }
        }
    }
}

impl Plugin for AdaptiveNoiseReduction {
    param_plumbing!(ADAPTIVE_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let channels = sane_channels(channels);
        self.stft.prepare(channels);
        self.lanes = (0..channels)
            .map(|_| {
                let mut tracker = MinTracker::new();
                tracker.prepare(MAX_BINS);
                Lane { tracker, power: vec![0.0; MAX_BINS], lambda: vec![0.0; MAX_BINS], gain: vec![1.0; MAX_BINS], gamma: vec![1.0; MAX_BINS] }
            })
            .collect();
        self.update();
    }

    fn reset(&mut self) {
        self.stft.reset();
        for lane in &mut self.lanes {
            lane.tracker.reset();
            lane.gain.iter_mut().for_each(|v| *v = 1.0);
            lane.gamma.iter_mut().for_each(|v| *v = 1.0);
        }
    }

    fn latency(&self) -> usize {
        self.stft.latency()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let alpha = self.p.v(SMOOTHING) / 100.0;
        let floor = 10f32.powf(self.p.v(FLOOR) / 20.0);
        let offset = 10f32.powf(self.p.v(OFFSET) / 10.0);
        let noise_only = self.p.on(NOISE_ONLY);
        let Self { stft, lanes, .. } = self;
        stft.process(io, frames, &mut |c, spec| {
            let Some(lane) = lanes.get_mut(c) else { return };
            let bins = spec.len().min(lane.power.len());
            for (p, y) in lane.power[..bins].iter_mut().zip(spec.iter()) {
                *p = y.norm_sqr();
            }
            lane.tracker.update(&lane.power[..bins]);
            for (l, n) in lane.lambda[..bins].iter_mut().zip(lane.tracker.noise()) {
                *l = n * offset;
            }
            dd_frame(&mut spec[..bins], &lane.lambda[..bins], &mut lane.gain[..bins], &mut lane.gamma[..bins], alpha, floor, noise_only);
        });
    }
}
