//! Reverb reduction (approximate): late reverberation is predicted per bin from the smoothed power
//! one "early" interval ago (50 ms), decayed by the reverb time (after Lebart et al. 2001:
//! P_late(t) = 10^(−6·Δ/T60) · P̄(t − Δ)), and subtracted spectrally down to a floor. Auto gain
//! restores the level the suppression takes away (up to +12 dB, never in near-silence). It needs a
//! sensible `t60_s`: it does not measure the room.

use super::stft::Stft;
use crate::params::{Params, lin, log, param_plumbing, toggle};
use crate::util::{sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

const FFT: usize = 2048;
const BINS: usize = FFT / 2 + 1;
const EARLY_S: f32 = 0.05;
const POWER_SMOOTH: f32 = 0.6;
const GAIN_TIME_S: f32 = 1.0;
const MAX_AUTO_GAIN: f32 = 3.98;
/// Frames of history for the early interval at the highest sample rate SoundCraft accepts.
const MAX_DELAY_FRAMES: usize = 40;

const AMOUNT: usize = 0;
const T60: usize = 1;
const FLOOR: usize = 2;
const AUTO_GAIN: usize = 3;

pub static DEREVERB_PARAMS: [ParamInfo; 4] = [
    lin("amount", "Amount", 0.0, 100.0, 40.0, Unit::Percent),
    log("t60_s", "Reverb time", 0.1, 4.0, 0.7, Unit::Seconds),
    lin("floor_db", "Floor", -40.0, 0.0, -15.0, Unit::Db),
    toggle("auto_gain", "Auto gain", true),
];

pub static DEREVERB_INFO: PluginInfo = PluginInfo {
    id: "dereverb",
    name: "Reverb Reduction",
    short_name: "Dereverb",
    category: Category::Other,
    params: &DEREVERB_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

struct Lane {
    smoothed: Vec<f32>,
    /// Ring of past smoothed spectra, `MAX_DELAY_FRAMES` deep.
    history: Vec<Vec<f32>>,
    next: usize,
    energy_in: f32,
    energy_out: f32,
    auto: f32,
}

/// Reverb reduction (see the module docs).
pub struct Dereverb {
    p: Params,
    sr: f32,
    stft: Stft,
    lanes: Vec<Lane>,
    delay_frames: usize,
}

impl Default for Dereverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Dereverb {
    pub fn new() -> Self {
        let mut d = Self { p: Params::new(&DEREVERB_PARAMS), sr: 48_000.0, stft: Stft::new(), lanes: Vec::new(), delay_frames: 1 };
        d.prepare(48_000.0, 1024, 2);
        d
    }

    fn update(&mut self) {}

    fn clear(lane: &mut Lane) {
        lane.smoothed.iter_mut().for_each(|v| *v = 0.0);
        lane.history.iter_mut().for_each(|h| h.iter_mut().for_each(|v| *v = 0.0));
        lane.next = 0;
        lane.energy_in = 0.0;
        lane.energy_out = 0.0;
        lane.auto = 1.0;
    }
}

impl Plugin for Dereverb {
    param_plumbing!(DEREVERB_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.stft.prepare(sane_channels(channels));
        self.stft.set_size(FFT);
        self.delay_frames = ((EARLY_S * self.sr / self.stft.hop() as f32).round() as usize).clamp(1, MAX_DELAY_FRAMES);
        self.lanes = (0..sane_channels(channels))
            .map(|_| Lane {
                smoothed: vec![0.0; BINS],
                history: vec![vec![0.0; BINS]; MAX_DELAY_FRAMES],
                next: 0,
                energy_in: 0.0,
                energy_out: 0.0,
                auto: 1.0,
            })
            .collect();
    }

    fn reset(&mut self) {
        self.stft.reset();
        self.lanes.iter_mut().for_each(Self::clear);
    }

    fn latency(&self) -> usize {
        self.stft.latency()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let hop = self.stft.hop() as f32;
        let delta = self.delay_frames as f32 * hop / self.sr;
        let decay = 10f32.powf(-6.0 * delta / self.p.v(T60).max(0.01));
        let over = 2.0 * self.p.v(AMOUNT) / 100.0;
        let floor = 10f32.powf(self.p.v(FLOOR) / 20.0);
        let auto_gain = self.p.on(AUTO_GAIN);
        let follow = 1.0 - (-hop / (GAIN_TIME_S * self.sr)).exp();
        let depth = self.delay_frames;
        let Self { stft, lanes, .. } = self;
        stft.process(io, frames, &mut |c, spec| {
            let Some(lane) = lanes.get_mut(c) else { return };
            let bins = spec.len().min(BINS);
            // The spectrum `depth` frames ago is the slot written `depth` frames before this one.
            let past = (lane.next + MAX_DELAY_FRAMES - depth) % MAX_DELAY_FRAMES;
            let (mut e_in, mut e_out) = (0.0f32, 0.0f32);
            for ((y, s), h) in spec[..bins].iter_mut().zip(&mut lane.smoothed[..bins]).zip(&lane.history[past][..bins]) {
                let p = y.norm_sqr();
                *s = POWER_SMOOTH * *s + (1.0 - POWER_SMOOTH) * p;
                let late = decay * h;
                let g = if *s > 1e-30 { (1.0 - over * late / *s).max(0.0).sqrt().max(floor) } else { 1.0 };
                e_in += p;
                e_out += p * g * g;
                *y *= g;
            }
            lane.history[lane.next][..bins].copy_from_slice(&lane.smoothed[..bins]);
            lane.next = (lane.next + 1) % MAX_DELAY_FRAMES;
            if auto_gain {
                lane.energy_in += (e_in - lane.energy_in) * follow;
                lane.energy_out += (e_out - lane.energy_out) * follow;
                let target = if lane.energy_in > 1e-10 && lane.energy_out > 0.0 {
                    (lane.energy_in / lane.energy_out).sqrt().clamp(1.0, MAX_AUTO_GAIN)
                } else {
                    1.0
                };
                lane.auto += (target - lane.auto) * follow;
                let a = if lane.auto.is_finite() { lane.auto } else { 1.0 };
                spec[..bins].iter_mut().for_each(|y| *y *= a);
            }
        });
    }
}
