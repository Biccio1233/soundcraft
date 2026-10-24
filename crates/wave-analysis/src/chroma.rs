//! Chroma: constant-Q energy folded onto the twelve pitch classes, for the treble (harmony) and the
//! bass (inversions), centred on the music's own tuning.

use crate::cqt::Cqt;

const HOP: usize = 1024;
/// C1 at A4 = 440 Hz.
const C1_HZ: f32 = 32.703_197;
const BINS: usize = 84;
const TREBLE_HZ: (f32, f32) = (130.0, 1976.0);
const BASS_HZ: (f32, f32) = (32.0, 208.0);
const BASS_OCTAVE_WEIGHTS: [f32; 3] = [1.0, 0.8, 0.6];
/// Log compression after normalising to the loudest bin of the whole signal.
const COMPRESSION: f32 = 100.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chroma {
    /// Seconds between frames.
    pub hop_s: f32,
    /// Per frame, pitch-class energy between 130 Hz and 2 kHz (index 0 = C).
    pub treble: Vec<[f32; 12]>,
    /// Per frame, pitch-class energy between 32 and 208 Hz, lower octaves weighted more.
    pub bass: Vec<[f32; 12]>,
    /// Per frame, the treble total.
    pub energy: Vec<f32>,
}

/// The chroma of `x` (at `sr`), with bins centred `tuning_cents` away from A4 = 440 Hz.
pub fn compute(x: &[f32], sr: u32, tuning_cents: f32) -> Chroma {
    let cents = if tuning_cents.is_finite() { tuning_cents.clamp(-100.0, 100.0) } else { 0.0 };
    let fmin = C1_HZ * 2f32.powf(cents / 1200.0);
    let hop_s = HOP as f32 / sr.max(1) as f32;
    let Ok(cqt) = Cqt::new(sr, fmin, BINS, 12, HOP) else {
        return Chroma { hop_s, ..Chroma::default() };
    };
    let mags = cqt.magnitudes(x);
    let peak = mags.iter().flatten().copied().fold(0.0f32, f32::max);
    let scale = if peak > 1e-9 { COMPRESSION / peak } else { 0.0 };
    let freq = |k: usize| fmin * 2f32.powf(k as f32 / 12.0);
    let mut c = Chroma { hop_s, ..Chroma::default() };
    for frame in &mags {
        let (mut treble, mut bass) = ([0.0f32; 12], [0.0f32; 12]);
        for (k, m) in frame.iter().enumerate() {
            let v = (1.0 + scale * m).log10();
            let f = freq(k);
            if (TREBLE_HZ.0..=TREBLE_HZ.1).contains(&f) {
                treble[k % 12] += v;
            }
            if (BASS_HZ.0..=BASS_HZ.1).contains(&f) {
                bass[k % 12] += v * BASS_OCTAVE_WEIGHTS.get(k / 12).copied().unwrap_or(0.0);
            }
        }
        c.energy.push(treble.iter().sum());
        c.treble.push(treble);
        c.bass.push(bass);
    }
    c
}

/// Mean treble, bass and energy between consecutive `bounds_s` (seconds); a span holding no frame
/// gives zeros.
pub fn sync(c: &Chroma, bounds_s: &[f32]) -> Vec<([f32; 12], [f32; 12], f32)> {
    bounds_s
        .windows(2)
        .map(|w| {
            let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else {
                return ([0.0; 12], [0.0; 12], 0.0);
            };
            let (mut t, mut bs, mut e, mut n) = ([0.0f32; 12], [0.0f32; 12], 0.0f32, 0usize);
            for (i, (tr, ba)) in c.treble.iter().zip(&c.bass).enumerate() {
                let time = i as f32 * c.hop_s;
                if time >= a && time < b {
                    for p in 0..12 {
                        t[p] += tr[p];
                        bs[p] += ba[p];
                    }
                    e += c.energy.get(i).copied().unwrap_or(0.0);
                    n += 1;
                }
            }
            if n > 0 {
                let inv = 1.0 / n as f32;
                t.iter_mut().chain(bs.iter_mut()).for_each(|v| *v *= inv);
                e *= inv;
            }
            (t, bs, e)
        })
        .collect()
}
