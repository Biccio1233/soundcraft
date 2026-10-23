//! Noise reduction from a noise print: Ephraim–Malah (1985) log-spectral amplitude estimator with
//! the decision-directed a-priori SNR. Offline it uses the full-resolution print; in the rack it
//! reads the print folded into sixth-octave band parameters.

use super::gain::dd_frame;
use super::print::{BANDS, FLOOR_DB, NoisePrint, bands_to_bins};
use super::stft::{FFT_CHOICES, FFT_SIZES, Stft};
use crate::params::{Params, choice, lin, param_plumbing, toggle};
use crate::util::{sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};

const FLOOR: usize = 0;
const OFFSET: usize = 1;
const SMOOTHING: usize = 2;
const FFT: usize = 3;
const NOISE_ONLY: usize = 4;
const BAND0: usize = 5;

const BAND_IDS: [&str; BANDS] = [
    "nb00", "nb01", "nb02", "nb03", "nb04", "nb05", "nb06", "nb07", "nb08", "nb09", "nb10", "nb11", "nb12", "nb13", "nb14", "nb15", "nb16", "nb17",
    "nb18", "nb19", "nb20", "nb21", "nb22", "nb23", "nb24", "nb25", "nb26", "nb27", "nb28", "nb29", "nb30", "nb31", "nb32", "nb33", "nb34", "nb35",
    "nb36", "nb37", "nb38", "nb39", "nb40", "nb41", "nb42", "nb43", "nb44", "nb45", "nb46", "nb47", "nb48", "nb49", "nb50", "nb51", "nb52", "nb53",
    "nb54", "nb55", "nb56", "nb57", "nb58", "nb59", "nb60", "nb61",
];

const fn params() -> [ParamInfo; BAND0 + BANDS] {
    // Defaults tuned on the F1 real-take acceptance (see docs/wavecraft/README.md).
    const FILL: ParamInfo = lin("floor_db", "Reduce by", -80.0, 0.0, -40.0, Unit::Db);
    let mut out = [FILL; BAND0 + BANDS];
    out[OFFSET] = lin("offset_db", "Noise offset", -10.0, 20.0, 2.0, Unit::Db);
    out[SMOOTHING] = lin("smoothing", "Smoothing", 90.0, 99.9, 99.5, Unit::Percent);
    out[FFT] = choice("fft", "FFT size", &FFT_CHOICES, 3);
    out[NOISE_ONLY] = toggle("noise_only", "Output noise only", false);
    let mut i = 0;
    while i < BANDS {
        out[BAND0 + i] = ParamInfo {
            id: BAND_IDS[i],
            name: BAND_IDS[i],
            min: FLOOR_DB,
            max: 0.0,
            default: FLOOR_DB,
            unit: Unit::Db,
            taper: Taper::Linear,
            choices: &[],
        };
        i += 1;
    }
    out
}

pub static NR_PARAMS: [ParamInfo; BAND0 + BANDS] = params();

pub static NR_INFO: PluginInfo = PluginInfo {
    id: "noise_reduction",
    name: "Noise Reduction",
    short_name: "Denoise",
    category: Category::Other,
    params: &NR_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

const MAX_BINS: usize = 16384 / 2 + 1;

/// Noise reduction from a noise print (see the module docs).
pub struct NoiseReduction {
    p: Params,
    sr: f32,
    channels: usize,
    stft: Stft,
    print: Option<NoisePrint>,
    /// Noise power per bin and channel at the current frame size (zero where there is none).
    lambda: Vec<Vec<f32>>,
    gain: Vec<Vec<f32>>,
    gamma: Vec<Vec<f32>>,
    dirty: bool,
    has_noise: bool,
}

impl Default for NoiseReduction {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseReduction {
    pub fn new() -> Self {
        let mut nr = Self {
            p: Params::new(&NR_PARAMS),
            sr: 48_000.0,
            channels: 0,
            stft: Stft::new(),
            print: None,
            lambda: Vec::new(),
            gain: Vec::new(),
            gamma: Vec::new(),
            dirty: true,
            has_noise: false,
        };
        nr.prepare(48_000.0, 1024, 2);
        nr
    }

    /// Use a full-resolution print instead of the band parameters.
    pub fn with_print(print: NoisePrint) -> Self {
        let mut nr = Self::new();
        nr.print = Some(print);
        nr.dirty = true;
        nr
    }

    fn fft(&self) -> usize {
        FFT_SIZES.get(self.p.choice(FFT)).copied().unwrap_or(4096)
    }

    fn update(&mut self) {
        self.stft.set_size(self.fft());
        self.dirty = true;
    }

    /// Rebuild the per-bin noise power for the current frame size (no allocation).
    fn rebuild(&mut self) {
        self.dirty = false;
        let n = self.stft.size();
        let bins = n / 2 + 1;
        // Expected |X|² of a Hann-windowed frame is the power density times the window energy.
        let scale = 0.375 * n as f32 * 10f32.powf(self.p.v(OFFSET) / 10.0);
        let sr = self.sr.round() as u32;
        let bands_live = (0..BANDS).any(|i| self.p.v(BAND0 + i) > FLOOR_DB + 0.5);
        self.has_noise = self.print.is_some() || bands_live;
        let mut band_vals = [FLOOR_DB; BANDS];
        for (i, b) in band_vals.iter_mut().enumerate() {
            *b = self.p.v(BAND0 + i);
        }
        for (c, lam) in self.lambda.iter_mut().enumerate() {
            let out = &mut lam[..bins];
            match &self.print {
                Some(print) => print.bin_psd(c, n, sr, out),
                None if bands_live => bands_to_bins(&band_vals, n, sr, out),
                None => out.iter_mut().for_each(|v| *v = 0.0),
            }
            out.iter_mut().for_each(|v| *v = if v.is_finite() && *v > 0.0 { *v * scale } else { 0.0 });
        }
        self.reset_state();
    }

    fn reset_state(&mut self) {
        self.gain.iter_mut().for_each(|g| g.iter_mut().for_each(|v| *v = 1.0));
        self.gamma.iter_mut().for_each(|g| g.iter_mut().for_each(|v| *v = 1.0));
    }
}

impl Plugin for NoiseReduction {
    param_plumbing!(NR_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.channels = sane_channels(channels);
        self.stft.prepare(self.channels);
        self.stft.set_size(self.fft());
        self.lambda = vec![vec![0.0; MAX_BINS]; self.channels];
        self.gain = vec![vec![1.0; MAX_BINS]; self.channels];
        self.gamma = vec![vec![1.0; MAX_BINS]; self.channels];
        self.dirty = true;
    }

    fn reset(&mut self) {
        self.stft.reset();
        self.reset_state();
    }

    fn latency(&self) -> usize {
        self.stft.latency()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if self.dirty {
            self.rebuild();
        }
        let alpha = self.p.v(SMOOTHING) / 100.0;
        let floor = 10f32.powf(self.p.v(FLOOR) / 20.0);
        let noise_only = self.p.on(NOISE_ONLY);
        let has_noise = self.has_noise;
        let Self { stft, lambda, gain, gamma, .. } = self;
        stft.process(io, frames, &mut |c, spec| {
            if !has_noise {
                if noise_only {
                    spec.iter_mut().for_each(|y| *y = super::Complex::new(0.0, 0.0));
                }
                return;
            }
            if let (Some(l), Some(g), Some(gm)) = (lambda.get(c), gain.get_mut(c), gamma.get_mut(c)) {
                dd_frame(spec, l, g, gm, alpha, floor, noise_only);
            }
        });
    }
}
