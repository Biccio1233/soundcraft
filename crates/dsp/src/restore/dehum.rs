//! Hum removal: a comb of narrow cuts on the mains frequency and its harmonics. Each cut is a
//! second-order notch with finite depth (zeros and poles on the same angle; poles at the radius
//! that gives the bandwidth f/Q, zeros closer to the circle by the depth), so the response away
//! from the cuts stays flat in level and nearly in phase. *Auto* tells 50 Hz from 60 Hz by Goertzel energy on the first four harmonics of
//! each, re-deciding every half second (it starts at 50 Hz). Sections run in f64: at Q 30 and
//! 50 Hz the poles sit within 1e-4 of the unit circle.

use crate::params::{Params, choice, lin, param_plumbing, toggle};
use crate::util::{sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

const MAX_HARMONICS: usize = 30;

const FREQ: usize = 0;
const HARMONICS: usize = 1;
const Q: usize = 2;
const DEPTH: usize = 3;
const HUM_ONLY: usize = 4;

pub static DEHUM_PARAMS: [ParamInfo; 5] = [
    choice("freq", "Frequency", &["Auto", "50 Hz", "60 Hz"], 0),
    lin("harmonics", "Harmonics", 1.0, MAX_HARMONICS as f32, 8.0, Unit::None),
    lin("q", "Q", 5.0, 100.0, 30.0, Unit::None),
    lin("depth_db", "Depth", 0.0, 60.0, 40.0, Unit::Db),
    toggle("hum_only", "Output hum only", false),
];

pub static DEHUM_INFO: PluginInfo = PluginInfo {
    id: "dehum",
    name: "Hum Removal",
    short_name: "Dehum",
    category: Category::Other,
    params: &DEHUM_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

/// A finite-depth notch in f64 (transposed direct form II), unity gain at DC.
#[derive(Debug, Clone, Copy, Default)]
struct Section {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Section {
    fn set(&mut self, hz: f64, q: f64, depth_db: f64, sr: f64) {
        let w0 = std::f64::consts::TAU * hz / sr;
        let rp = (-std::f64::consts::PI * (hz / q.max(0.1)) / sr).exp();
        let rz = 1.0 - 10f64.powf(-depth_db / 20.0) * (1.0 - rp);
        let c = w0.cos();
        let k = (1.0 - 2.0 * rp * c + rp * rp) / (1.0 - 2.0 * rz * c + rz * rz);
        let k = if k.is_finite() { k } else { 1.0 };
        self.b = [k, -2.0 * rz * c * k, rz * rz * k];
        self.a = [-2.0 * rp * c, rp * rp];
    }

    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        if y.is_finite() {
            y
        } else {
            self.z = [0.0; 2];
            0.0
        }
    }
}

/// Goertzel power of `f` Hz over `x` (mean of the channels per sample is the caller's job).
fn goertzel(x: impl Iterator<Item = f32>, hz: f64, sr: f64) -> f64 {
    let coeff = 2.0 * (std::f64::consts::TAU * hz / sr).cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for v in x {
        let s = f64::from(if v.is_finite() { v } else { 0.0 }) + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// The mains frequency of the hum in `ch`: 60.0 when the 60 Hz family (60…240 Hz) holds more
/// energy than the 50 Hz one (50…200 Hz), else 50.0.
pub fn detect_mains(ch: &[Vec<f32>], sample_rate: f32) -> f32 {
    let sr = f64::from(sane_sr(sample_rate));
    let len = ch.iter().map(Vec::len).min().unwrap_or(0);
    let n = ch.len().max(1) as f32;
    let mix = |i: usize| ch.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / n;
    let family = |f0: f64| (1..=4).map(|h| goertzel((0..len).map(mix), f0 * h as f64, sr)).sum::<f64>();
    if family(60.0) > family(50.0) { 60.0 } else { 50.0 }
}

/// Running Goertzel state for the realtime *Auto* decision.
#[derive(Debug, Clone, Copy, Default)]
struct Probe {
    coeff: f64,
    s1: f64,
    s2: f64,
}

/// Hum removal (see the module docs).
pub struct Dehum {
    p: Params,
    sr: f32,
    channels: usize,
    sections: Vec<[Section; MAX_HARMONICS]>,
    active: usize,
    mains: f32,
    probes: [Probe; 8],
    probe_len: usize,
    probe_count: usize,
}

impl Default for Dehum {
    fn default() -> Self {
        Self::new()
    }
}

impl Dehum {
    pub fn new() -> Self {
        let mut d = Self {
            p: Params::new(&DEHUM_PARAMS),
            sr: 48_000.0,
            channels: 0,
            sections: Vec::new(),
            active: 0,
            mains: 50.0,
            probes: [Probe::default(); 8],
            probe_len: 24_000,
            probe_count: 0,
        };
        d.prepare(48_000.0, 1024, 2);
        d
    }

    fn fundamental(&self) -> f32 {
        match self.p.choice(FREQ) {
            1 => 50.0,
            2 => 60.0,
            _ => self.mains,
        }
    }

    fn update(&mut self) {
        let f0 = f64::from(self.fundamental());
        let sr = f64::from(self.sr);
        let harmonics = (self.p.v(HARMONICS).round() as usize).clamp(1, MAX_HARMONICS);
        let q = f64::from(self.p.v(Q));
        let depth = f64::from(self.p.v(DEPTH));
        self.active = (1..=harmonics).take_while(|h| f0 * *h as f64 <= 0.45 * sr).count();
        for bank in &mut self.sections {
            for (h, s) in bank.iter_mut().enumerate().take(self.active) {
                s.set(f0 * (h + 1) as f64, q, depth, sr);
            }
        }
    }

    fn reset_probes(&mut self) {
        let sr = f64::from(self.sr);
        for (i, p) in self.probes.iter_mut().enumerate() {
            let f0 = if i < 4 { 50.0 } else { 60.0 };
            let hz = f0 * ((i % 4) + 1) as f64;
            *p = Probe { coeff: 2.0 * (std::f64::consts::TAU * hz / sr).cos(), s1: 0.0, s2: 0.0 };
        }
        self.probe_count = 0;
    }

    /// Feed the *Auto* probes one input sample; re-decide at the end of each half second.
    fn probe(&mut self, x: f64) {
        for p in &mut self.probes {
            let s = x + p.coeff * p.s1 - p.s2;
            p.s2 = p.s1;
            p.s1 = s;
        }
        self.probe_count += 1;
        if self.probe_count < self.probe_len {
            return;
        }
        let power = |p: &Probe| p.s1 * p.s1 + p.s2 * p.s2 - p.coeff * p.s1 * p.s2;
        let e50: f64 = self.probes[..4].iter().map(power).sum();
        let e60: f64 = self.probes[4..].iter().map(power).sum();
        let switch = if self.mains == 50.0 { e60 > 2.0 * e50 } else { e50 > 2.0 * e60 };
        self.reset_probes();
        if switch {
            self.mains = if self.mains == 50.0 { 60.0 } else { 50.0 };
            self.update();
        }
    }
}

impl Plugin for Dehum {
    param_plumbing!(DEHUM_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.channels = sane_channels(channels);
        self.sections = vec![[Section::default(); MAX_HARMONICS]; self.channels];
        self.probe_len = ((self.sr * 0.5) as usize).max(1);
        self.reset_probes();
        self.update();
    }

    fn reset(&mut self) {
        for bank in &mut self.sections {
            bank.iter_mut().for_each(|s| s.z = [0.0; 2]);
        }
        self.mains = 50.0;
        self.reset_probes();
        self.update();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let auto = self.p.choice(FREQ) == 0;
        let hum_only = self.p.on(HUM_ONLY);
        let n = io.len().min(self.channels).max(1) as f64;
        for i in 0..frames {
            if auto {
                let mix: f64 =
                    io.iter().take(self.channels).map(|c| c.get(i).copied().filter(|v| v.is_finite()).map_or(0.0, f64::from)).sum::<f64>() / n;
                self.probe(mix);
            }
            for (c, bank) in io.iter_mut().zip(self.sections.iter_mut()) {
                let Some(s) = c.get_mut(i) else { continue };
                let x = if s.is_finite() { f64::from(*s) } else { 0.0 };
                let mut y = x;
                for sec in bank.iter_mut().take(self.active) {
                    y = sec.run(y);
                }
                *s = (if hum_only { x - y } else { y }) as f32;
            }
        }
    }
}
