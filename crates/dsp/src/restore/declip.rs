//! Clip repair: a run of at least two consecutive samples at or above the threshold (and no longer
//! than the longest run allowed) is redrawn by the cubic through the two samples before it and the
//! two after it. A redrawn sample keeps the run's sign and never falls below the threshold (the
//! true waveform was at least that loud). Longer runs are left alone. Works on a delay line as
//! long as the longest run at 10 ms plus its four neighbours.

use crate::params::{Params, lin, param_plumbing};
use crate::util::{sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

const MAX_RUN_MS: f32 = 10.0;

const THRESHOLD: usize = 0;
const MAX_RUN: usize = 1;
const GAIN: usize = 2;

pub static DECLIP_PARAMS: [ParamInfo; 3] = [
    lin("threshold_db", "Clip level", -20.0, 0.0, -0.1, Unit::Db),
    lin("max_run_ms", "Longest run", 0.1, MAX_RUN_MS, 2.0, Unit::Ms),
    lin("gain_db", "Output gain", -12.0, 0.0, 0.0, Unit::Db),
];

pub static DECLIP_INFO: PluginInfo = PluginInfo {
    id: "declip",
    name: "Clip Repair",
    short_name: "Declip",
    category: Category::Other,
    params: &DECLIP_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

/// One channel's delay line and run detector.
#[derive(Default)]
struct Line {
    buf: Vec<f32>,
    /// Samples written so far (the next sample's absolute index).
    written: u64,
    /// Start (absolute index) and length of the run being read, if any.
    run_start: u64,
    run_len: usize,
    /// A run that has ended and waits for its second following sample.
    pending: Option<(u64, usize)>,
}

impl Line {
    fn at(&self, abs: u64) -> f32 {
        let cap = self.buf.len().max(1) as u64;
        self.buf.get((abs % cap) as usize).copied().unwrap_or(0.0)
    }

    fn set(&mut self, abs: u64, v: f32) {
        let cap = self.buf.len().max(1) as u64;
        if let Some(slot) = self.buf.get_mut((abs % cap) as usize) {
            *slot = v;
        }
    }

    /// Redraw the run `[start, start+len)` from its two neighbours on each side.
    fn repair(&mut self, start: u64, len: usize, threshold: f32) {
        if start < 2 || self.written < start + len as u64 + 2 {
            return;
        }
        let t = [-2.0f64, -1.0, len as f64, len as f64 + 1.0];
        let y = [self.at(start - 2), self.at(start - 1), self.at(start + len as u64), self.at(start + len as u64 + 1)].map(f64::from);
        for i in 0..len {
            let x = i as f64;
            let mut v = 0.0f64;
            for (a, ya) in t.iter().zip(&y) {
                let mut w = 1.0;
                for b in &t {
                    if a != b {
                        w *= (x - b) / (a - b);
                    }
                }
                v += ya * w;
            }
            let abs = start + i as u64;
            let orig = self.at(abs);
            let sign = if orig < 0.0 { -1.0f32 } else { 1.0 };
            let v = v as f32;
            let mag = if v.is_finite() && v * sign > 0.0 { (v * sign).max(threshold) } else { orig.abs().max(threshold) };
            self.set(abs, sign * mag);
        }
    }

    /// Push one input sample; returns the sample leaving the delay line.
    fn push(&mut self, x: f32, threshold: f32, max_run: usize, delay: u64) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let now = self.written;
        self.set(now, x);
        self.written += 1;
        if let Some((start, len)) = self.pending.take() {
            // `x` is the run's second following sample.
            self.repair(start, len, threshold);
        }
        if x.abs() >= threshold {
            if self.run_len == 0 {
                self.run_start = now;
            }
            self.run_len += 1;
        } else if self.run_len > 0 {
            if (2..=max_run).contains(&self.run_len) {
                self.pending = Some((self.run_start, self.run_len));
            }
            self.run_len = 0;
        }
        if self.written > delay { self.at(self.written - 1 - delay) } else { 0.0 }
    }
}

/// Clip repair (see the module docs).
pub struct Declip {
    p: Params,
    sr: f32,
    delay: usize,
    lines: Vec<Line>,
}

impl Default for Declip {
    fn default() -> Self {
        Self::new()
    }
}

impl Declip {
    pub fn new() -> Self {
        let mut d = Self { p: Params::new(&DECLIP_PARAMS), sr: 48_000.0, delay: 0, lines: Vec::new() };
        d.prepare(48_000.0, 1024, 2);
        d
    }

    fn update(&mut self) {}
}

impl Plugin for Declip {
    param_plumbing!(DECLIP_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.delay = (self.sr * MAX_RUN_MS / 1000.0).ceil() as usize + 4;
        self.lines = (0..sane_channels(channels)).map(|_| Line { buf: vec![0.0; self.delay + 1], ..Line::default() }).collect();
    }

    fn reset(&mut self) {
        for l in &mut self.lines {
            l.buf.iter_mut().for_each(|v| *v = 0.0);
            l.written = 0;
            l.run_len = 0;
            l.pending = None;
        }
    }

    fn latency(&self) -> usize {
        self.delay
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let threshold = 10f32.powf(self.p.v(THRESHOLD) / 20.0);
        let max_run = ((self.p.v(MAX_RUN) / 1000.0 * self.sr) as usize).clamp(2, self.delay.saturating_sub(4).max(2));
        let gain = 10f32.powf(self.p.v(GAIN) / 20.0);
        let delay = self.delay as u64;
        for (c, line) in io.iter_mut().zip(self.lines.iter_mut()) {
            for s in c.iter_mut().take(frames) {
                *s = line.push(*s, threshold, max_run, delay) * gain;
            }
        }
    }
}
