//! Hiss reduction: a downward expander per FFT bin above a cutoff. The noise floor is tracked by
//! minimum statistics and averaged over ±8 neighbouring bins (hiss is smooth across frequency, and
//! the per-bin minimum scatters a few dB); a bin whose level (smoothed over frames and over its two
//! neighbours, so noise alone does not flicker) falls within `threshold` dB of its floor is cut by
//! up to `reduce` dB (fully at the floor). The cut opens at once and closes over ~60 ms, applies
//! fully above the cutoff and fades in over the half octave below it.

use super::adaptive::MinTracker;
use super::stft::Stft;
use crate::params::{Params, lin, log, param_plumbing, toggle};
use crate::util::{sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

const FFT: usize = 2048;
const BINS: usize = FFT / 2 + 1;
const WINDOW_S: f32 = 1.5;
const ATTACK_S: f32 = 0.01;
const RELEASE_S: f32 = 0.06;
/// Frame-to-frame smoothing of the level the expander reads.
const LEVEL_SMOOTH: f32 = 0.7;
/// Bins on each side the floor is averaged over.
const FLOOR_SPREAD: usize = 8;

const THRESHOLD: usize = 0;
const REDUCE: usize = 1;
const CUTOFF: usize = 2;
const HISS_ONLY: usize = 3;

pub static HISS_PARAMS: [ParamInfo; 4] = [
    lin("threshold_db", "Noise floor", 0.0, 40.0, 10.0, Unit::Db),
    lin("reduce_db", "Reduce by", 0.0, 60.0, 24.0, Unit::Db),
    log("cutoff_hz", "Cutoff", 1000.0, 16_000.0, 4000.0, Unit::Hz),
    toggle("hiss_only", "Output hiss only", false),
];

pub static HISS_INFO: PluginInfo = PluginInfo {
    id: "hiss_reduction",
    name: "Hiss Reduction",
    short_name: "Dehiss",
    category: Category::Other,
    params: &HISS_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

struct Lane {
    tracker: MinTracker,
    power: Vec<f32>,
    level: Vec<f32>,
    floor: Vec<f32>,
    gain_db: Vec<f32>,
}

/// Hiss reduction (see the module docs).
pub struct HissReduction {
    p: Params,
    sr: f32,
    stft: Stft,
    lanes: Vec<Lane>,
    /// Share of the cut each bin gets (0 below the ramp, 1 above the cutoff).
    weight: Vec<f32>,
}

impl Default for HissReduction {
    fn default() -> Self {
        Self::new()
    }
}

impl HissReduction {
    pub fn new() -> Self {
        let mut h = Self { p: Params::new(&HISS_PARAMS), sr: 48_000.0, stft: Stft::new(), lanes: Vec::new(), weight: vec![0.0; BINS] };
        h.prepare(48_000.0, 1024, 2);
        h
    }

    fn update(&mut self) {
        let cutoff = self.p.v(CUTOFF).max(1.0);
        let start = cutoff / std::f32::consts::SQRT_2;
        let bin_hz = self.sr / FFT as f32;
        for (k, w) in self.weight.iter_mut().enumerate() {
            let hz = k as f32 * bin_hz;
            *w = if hz >= cutoff {
                1.0
            } else if hz <= start {
                0.0
            } else {
                (hz / start).log2() / (cutoff / start).log2()
            };
        }
    }
}

impl Plugin for HissReduction {
    param_plumbing!(HISS_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let channels = sane_channels(channels);
        self.stft.prepare(channels);
        self.stft.set_size(FFT);
        let frames = (WINDOW_S * self.sr / self.stft.hop() as f32) as usize;
        self.lanes = (0..channels)
            .map(|_| {
                let mut tracker = MinTracker::new();
                tracker.prepare(BINS);
                tracker.set_window(frames);
                Lane { tracker, power: vec![0.0; BINS], level: vec![0.0; BINS], floor: vec![0.0; BINS], gain_db: vec![0.0; BINS] }
            })
            .collect();
        self.update();
    }

    fn reset(&mut self) {
        self.stft.reset();
        for lane in &mut self.lanes {
            lane.tracker.reset();
            lane.level.iter_mut().for_each(|l| *l = 0.0);
            lane.gain_db.iter_mut().for_each(|g| *g = 0.0);
        }
    }

    fn latency(&self) -> usize {
        self.stft.latency()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let threshold = self.p.v(THRESHOLD);
        let reduce = self.p.v(REDUCE);
        let hiss_only = self.p.on(HISS_ONLY);
        let hop = self.stft.hop() as f32;
        let attack = 1.0 - (-hop / (ATTACK_S * self.sr)).exp();
        let release = 1.0 - (-hop / (RELEASE_S * self.sr)).exp();
        let Self { stft, lanes, weight, .. } = self;
        stft.process(io, frames, &mut |c, spec| {
            let Some(lane) = lanes.get_mut(c) else { return };
            let bins = spec.len().min(BINS);
            for (p, y) in lane.power[..bins].iter_mut().zip(spec.iter()) {
                *p = y.norm_sqr();
            }
            lane.tracker.update(&lane.power[..bins]);
            for (l, p) in lane.level[..bins].iter_mut().zip(&lane.power[..bins]) {
                *l = LEVEL_SMOOTH * *l + (1.0 - LEVEL_SMOOTH) * p;
            }
            // Running mean of the tracked floor over ±FLOOR_SPREAD bins.
            let noise = lane.tracker.noise();
            let mut sum: f32 = noise[..bins.min(FLOOR_SPREAD + 1)].iter().sum();
            for k in 0..bins {
                let (lo, hi) = (k.saturating_sub(FLOOR_SPREAD), (k + FLOOR_SPREAD).min(bins - 1));
                lane.floor[k] = sum / (hi - lo + 1) as f32;
                if let Some(add) = noise.get(k + FLOOR_SPREAD + 1).filter(|_| k + FLOOR_SPREAD + 1 < bins) {
                    sum += add;
                }
                if k >= FLOOR_SPREAD {
                    sum -= noise[k - FLOOR_SPREAD];
                }
            }
            for k in 0..bins {
                let floor = lane.floor[k];
                let target = if weight[k] <= 0.0 || floor <= 0.0 {
                    0.0
                } else {
                    let (lo, hi) = (k.saturating_sub(1), (k + 1).min(bins - 1));
                    let level = lane.level[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32;
                    let over = 10.0 * (level.max(1e-30) / floor).log10();
                    let depth = if threshold > 0.0 {
                        ((threshold - over) / threshold).clamp(0.0, 1.0)
                    } else if over < 0.0 {
                        1.0
                    } else {
                        0.0
                    };
                    -reduce * depth * weight[k]
                };
                let g = &mut lane.gain_db[k];
                *g += (target - *g) * if target > *g { attack } else { release };
                let lin = 10f32.powf(*g / 20.0);
                spec[k] *= if hiss_only { 1.0 - lin } else { lin };
            }
        });
    }
}
