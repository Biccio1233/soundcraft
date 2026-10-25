//! Tempo, beats and downbeats: log-spectral-flux onsets, an autocorrelation tempo with a
//! log-Gaussian prior around 120 BPM, Ellis (2007) dynamic-programming beat tracking, and the bar
//! phase where chords change and the bass hits.

use crate::chroma::{Chroma, sync};
use crate::spectrum::magnitudes;

const FFT: usize = 1024;
/// Frames per second of the onset envelope at 22,050 Hz (hop 220).
const HOP_S: f32 = 220.0 / 22_050.0;
const MIN_BPM: f32 = 50.0;
const MAX_BPM: f32 = 220.0;
const PRIOR_BPM: f32 = 120.0;
const PRIOR_OCTAVES: f32 = 1.4;
const TIGHTNESS: f32 = 100.0;
const LOW_BAND_HZ: f32 = 150.0;
const BANDS: usize = 40;
const COMPRESSION: f32 = 100.0;
const BAND_LO_HZ: f32 = 30.0;
const BAND_HI_HZ: f32 = 10_000.0;
const MIN_SECONDS: f32 = 2.0;
/// Below this raw onset strength there is no beat to find (a held note, a drone).
const MIN_ONSET_STRENGTH: f32 = 2.0;
/// Estimates below this tempo are doubled when their half-beats are strong (see `midpoint_ratio`).
const DOUBLE_BELOW_BPM: f32 = 100.0;
const DOUBLE_RATIO: f32 = 0.5;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tempo {
    pub bpm: f32,
    /// Beat times in seconds.
    pub beats: Vec<f32>,
    /// The first beat of each bar.
    pub downbeats: Vec<f32>,
    pub beats_per_bar: u8,
    /// Strength of the beat periodicity, 0..1.
    pub confidence: f32,
}

struct Onsets {
    full: Vec<f32>,
    low: Vec<f32>,
    fps: f32,
    /// The 99th percentile of the raw flux.
    strength: f32,
}

fn hop_for(sr: u32) -> usize {
    ((sr as f32 * HOP_S).round() as usize).max(1)
}

/// Remove the local mean (0.5 s), keep what rises above it, and scale to unit standard deviation.
fn normalise(env: &mut [f32], fps: f32) {
    let w = ((0.25 * fps) as usize).max(1);
    let mut prefix = vec![0.0f64; env.len() + 1];
    for (i, v) in env.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(*v);
    }
    let local: Vec<f32> = (0..env.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(w), (i + w + 1).min(env.len()));
            ((prefix[b] - prefix[a]) / (b - a).max(1) as f64) as f32
        })
        .collect();
    for (v, m) in env.iter_mut().zip(&local) {
        *v = (*v - m).max(0.0);
    }
    let n = env.len().max(1) as f32;
    let mean = env.iter().sum::<f32>() / n;
    let sd = (env.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n).sqrt();
    if sd > 1e-9 {
        env.iter_mut().for_each(|v| *v /= sd);
    }
}

fn onsets(x: &[f32], sr: u32) -> Onsets {
    let hop = hop_for(sr);
    let fps = sr as f32 / hop as f32;
    let spec = magnitudes(x, FFT, hop);
    let bin_hz = sr.max(1) as f32 / FFT as f32;
    // Log-spaced bands, so the many high bins of a hi-hat do not outweigh a kick or a snare.
    let edges: Vec<usize> =
        (0..=BANDS).map(|i| ((BAND_LO_HZ * (BAND_HI_HZ / BAND_LO_HZ).powf(i as f32 / BANDS as f32)) / bin_hz).round() as usize).collect();
    let bands: Vec<Vec<f32>> = spec
        .iter()
        .map(|frame| {
            edges
                .windows(2)
                .map(|w| {
                    let (a, b) = (w[0], w[1].max(w[0] + 1));
                    let band = frame.get(a..b.min(frame.len())).unwrap_or(&[]);
                    band.iter().sum::<f32>() / band.len().max(1) as f32
                })
                .collect()
        })
        .collect();
    // Relative to the loudest band of the whole signal, so the compression is level independent but
    // a quiet hi-hat still weighs less than a kick.
    let peak = bands.iter().flatten().copied().fold(0.0f32, f32::max);
    let scale = if peak > 1e-12 { COMPRESSION / peak } else { 0.0 };
    let mut prev: Option<Vec<f32>> = None;
    let (mut full, mut low) = (Vec::with_capacity(spec.len()), Vec::with_capacity(spec.len()));
    for frame in &bands {
        let logm: Vec<f32> = frame.iter().map(|m| (1.0 + scale * m).ln()).collect();
        let (mut f, mut l) = (0.0f32, 0.0f32);
        if let Some(p) = &prev {
            for (k, (a, b)) in logm.iter().zip(p).enumerate() {
                let d = (a - b).max(0.0);
                let top = edges.get(k + 1).map_or(f32::INFINITY, |e| *e as f32 * bin_hz);
                f += d;
                if top <= LOW_BAND_HZ {
                    l += d;
                }
            }
        }
        full.push(f);
        low.push(l);
        prev = Some(logm);
    }
    // How strong the onsets are before normalising: a held tone only ripples (≈ 0.5 here), drums
    // and played music reach 15–30.
    let mut sorted = full.clone();
    sorted.sort_by(f32::total_cmp);
    let strength = sorted.get(sorted.len().saturating_sub(1) * 99 / 100).copied().unwrap_or(0.0);
    normalise(&mut full, fps);
    normalise(&mut low, fps);
    Onsets { full, low, fps, strength }
}

/// The onset-strength envelope of `x` (at `sr`) and its frame rate.
pub fn onset_envelope(x: &[f32], sr: u32) -> (Vec<f32>, f32) {
    let o = onsets(x, sr);
    (o.full, o.fps)
}

fn autocorr(env: &[f32], lag: usize) -> f32 {
    env.iter().zip(env.iter().skip(lag)).map(|(a, b)| a * b).sum::<f32>() / env.len().max(1) as f32
}

/// The beat period in frames (fractional) and the periodicity strength.
fn global_period(env: &[f32], fps: f32) -> Option<(f32, f32)> {
    let (lo, hi) = ((60.0 / MAX_BPM * fps).floor() as usize, (60.0 / MIN_BPM * fps).ceil() as usize);
    let r0 = autocorr(env, 0);
    if r0 <= 1e-9 || hi + 2 >= env.len() {
        return None;
    }
    let prior0 = 60.0 / PRIOR_BPM * fps;
    let score = |lag: usize| -> f32 {
        let r = autocorr(env, lag).max(0.0);
        let octaves = (lag as f32 / prior0).log2() / PRIOR_OCTAVES;
        r * (-0.5 * octaves * octaves).exp()
    };
    let scores: Vec<f32> = (lo..=hi).map(score).collect();
    let (best_i, _) = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    let lag = lo + best_i;
    let (a, b, c) = (
        scores.get(best_i.wrapping_sub(1)).copied().unwrap_or(0.0),
        scores.get(best_i).copied().unwrap_or(0.0),
        scores.get(best_i + 1).copied().unwrap_or(0.0),
    );
    let den = a - 2.0 * b + c;
    let delta = if den.abs() > 1e-12 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
    Some((lag as f32 + delta, (autocorr(env, lag) / r0).clamp(0.0, 1.0)))
}

/// Ellis's dynamic programme: beat frames that sit on onsets and keep the period.
fn track(env: &[f32], period: f32) -> Vec<usize> {
    // Local score: the envelope smoothed over a Gaussian a period wide.
    let half = period.round() as isize;
    let g: Vec<f32> = (-half..=half).map(|n| (-0.5 * (n as f32 * 32.0 / period).powi(2)).exp()).collect();
    let local: Vec<f32> = (0..env.len() as isize)
        .map(|i| {
            g.iter()
                .enumerate()
                .map(|(j, w)| {
                    let k = i + j as isize - half;
                    if k >= 0 { env.get(k as usize).copied().unwrap_or(0.0) * w } else { 0.0 }
                })
                .sum()
        })
        .collect();
    let n = local.len();
    let mut score = vec![0.0f32; n];
    let mut back = vec![usize::MAX; n];
    let (min_gap, max_gap) = ((period / 2.0).round().max(1.0) as usize, (2.0 * period).round() as usize);
    for t in 0..n {
        let mut best = (f32::NEG_INFINITY, usize::MAX);
        for gap in min_gap..=max_gap {
            let Some(p) = t.checked_sub(gap) else { break };
            let cost = TIGHTNESS * (gap as f32 / period).ln().powi(2);
            let s = score[p] - cost;
            if s > best.0 {
                best = (s, p);
            }
        }
        let l = local[t];
        if best.1 != usize::MAX && best.0 > 0.0 {
            score[t] = l + best.0;
            back[t] = best.1;
        } else {
            score[t] = l;
        }
    }
    // Start from the best score in the last period and walk back.
    let tail = n.saturating_sub(period.round() as usize).min(n.saturating_sub(1));
    let Some((mut t, _)) = score.iter().enumerate().skip(tail).max_by(|a, b| a.1.total_cmp(b.1)) else {
        return Vec::new();
    };
    let mut beats = vec![t];
    while back[t] != usize::MAX {
        t = back[t];
        beats.push(t);
    }
    beats.reverse();
    // Drop weak beats at the edges (silence before the music starts or after it ends).
    let rms = (local.iter().map(|v| v * v).sum::<f32>() / n.max(1) as f32).sqrt();
    let strong = |b: &usize| local.get(*b).copied().unwrap_or(0.0) >= 0.5 * rms;
    let first = beats.iter().position(strong).unwrap_or(beats.len());
    let last = beats.iter().rposition(strong).map_or(0, |i| i + 1);
    beats.get(first..last.max(first)).map(<[usize]>::to_vec).unwrap_or_default()
}

/// Mean onset strength halfway between consecutive beats over the mean strength at the beats.
fn midpoint_ratio(env: &[f32], frames: &[usize]) -> f32 {
    let strength = |f: usize| (f.saturating_sub(2)..=f + 2).filter_map(|k| env.get(k)).copied().fold(0.0f32, f32::max);
    let on = frames.iter().map(|f| strength(*f)).sum::<f32>() / frames.len().max(1) as f32;
    let mids: Vec<f32> = frames.windows(2).map(|w| strength((w[0] + w[1]) / 2)).collect();
    let off = mids.iter().sum::<f32>() / mids.len().max(1) as f32;
    if on > 1e-9 { off / on } else { 0.0 }
}

fn cosine(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|v| v * v).sum::<f32>().sqrt();
    let nb = b.iter().map(|v| v * v).sum::<f32>().sqrt();
    if na > 1e-9 && nb > 1e-9 { dot / (na * nb) } else { 0.0 }
}

/// Tempo, beats and downbeats of `x` (at `sr`). `chroma` (of the same audio) locates the bar
/// starts at chord changes; `beats_per_bar` defaults to 4 and is clamped to 2..=7.
pub fn analyze(x: &[f32], sr: u32, chroma: Option<&Chroma>, beats_per_bar: Option<u8>) -> Tempo {
    let bpb = beats_per_bar.unwrap_or(4).clamp(2, 7);
    let none = Tempo { beats_per_bar: bpb, ..Tempo::default() };
    if sr == 0 || (x.len() as f32) < MIN_SECONDS * sr as f32 {
        return none;
    }
    let o = onsets(x, sr);
    if o.strength < MIN_ONSET_STRENGTH {
        return none;
    }
    let Some((period, confidence)) = global_period(&o.full, o.fps) else {
        return none;
    };
    let mut frames = track(&o.full, period);
    // Octave check: a slow estimate whose half-beats carry onsets as strong as half the beats'
    // (the snare between kicks of a fast song) is half the real tempo.
    if 60.0 * o.fps / period < DOUBLE_BELOW_BPM && midpoint_ratio(&o.full, &frames) >= DOUBLE_RATIO {
        let faster = track(&o.full, period / 2.0);
        if faster.len() >= 2 {
            frames = faster;
        }
    }
    if frames.len() < 2 {
        return none;
    }
    // Frame f covers samples f·hop .. f·hop + FFT; its flux belongs to the window's centre.
    let centre = FFT as f32 / 2.0 / sr as f32;
    let beats: Vec<f32> = frames.iter().map(|f| *f as f32 / o.fps + centre).collect();
    let mut ibi: Vec<f32> = beats.windows(2).map(|w| w[1] - w[0]).collect();
    ibi.sort_by(f32::total_cmp);
    let median = ibi.get(ibi.len() / 2).copied().unwrap_or(0.0);
    // The mean of the intervals near the median: whole frames would quantise the tempo by ~3 % at 180.
    let near: Vec<f32> = ibi.iter().copied().filter(|v| (*v - median).abs() <= 0.1 * median).collect();
    let mean = near.iter().sum::<f32>() / near.len().max(1) as f32;
    let bpm = if mean > 0.0 { 60.0 / mean } else { 0.0 };

    // Bar phase: chord change between beat-synchronous chromas plus half the bass onset strength.
    let change: Vec<f32> = match chroma {
        Some(c) => {
            let mut bounds = beats.clone();
            bounds.push(beats.last().copied().unwrap_or(0.0) + median);
            let s = sync(c, &bounds);
            (0..beats.len())
                .map(|i| match (i.checked_sub(1).and_then(|p| s.get(p)), s.get(i)) {
                    (Some(a), Some(b)) => 1.0 - cosine(&a.0, &b.0),
                    _ => 0.0,
                })
                .collect()
        }
        None => vec![0.0; beats.len()],
    };
    let low: Vec<f32> = frames.iter().map(|f| (f.saturating_sub(2)..=f + 2).filter_map(|k| o.low.get(k)).copied().fold(0.0f32, f32::max)).collect();
    let low_max = low.iter().copied().fold(0.0f32, f32::max).max(1e-9);
    let change_max = change.iter().copied().fold(0.0f32, f32::max).max(1e-9);
    let n = usize::from(bpb);
    let phase = (0..n)
        .max_by(|a, b| {
            let s = |k: usize| -> f32 {
                (k..beats.len())
                    .step_by(n)
                    .map(|i| change.get(i).copied().unwrap_or(0.0) / change_max + 0.5 * low.get(i).copied().unwrap_or(0.0) / low_max)
                    .sum::<f32>()
                    / (k..beats.len()).step_by(n).count().max(1) as f32
            };
            s(*a).total_cmp(&s(*b))
        })
        .unwrap_or(0);
    let downbeats = beats.iter().skip(phase).step_by(n).copied().collect();
    Tempo { bpm, beats, downbeats, beats_per_bar: bpb, confidence }
}
