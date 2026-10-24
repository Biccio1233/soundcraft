//! Song sections: bar-synchronous chroma and timbre, a self-similarity matrix, Foote's checkerboard
//! novelty for the boundaries, then letters for repeats and names from position and repetition.

use crate::chroma::{Chroma, sync};
use crate::spectrum::magnitudes;
use crate::tempo::Tempo;

const MEL_BANDS: usize = 20;
const MEL_LO_HZ: f32 = 40.0;
const MEL_HI_HZ: f32 = 10_000.0;
const TIMBRE_FFT: usize = 2048;
/// Half the checkerboard kernel, in bars.
const KERNEL_HALF: usize = 4;
const MIN_SECTION_BARS: usize = 4;
/// Segments whose bar-by-bar similarity reaches this share the letter.
const SAME_LETTER: f32 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionLabel {
    Intro,
    Verse,
    Chorus,
    Bridge,
    Instrumental,
    Outro,
    Other,
}

impl SectionLabel {
    pub fn name(self) -> &'static str {
        match self {
            SectionLabel::Intro => "intro",
            SectionLabel::Verse => "verse",
            SectionLabel::Chorus => "chorus",
            SectionLabel::Bridge => "bridge",
            SectionLabel::Instrumental => "instrumental",
            SectionLabel::Outro => "outro",
            SectionLabel::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Section {
    pub start: f32,
    pub end: f32,
    /// Repeats share a letter (A, B, C…).
    pub letter: char,
    pub label: SectionLabel,
}

fn mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

/// Per frame (centred every `hop_s`, like the chroma): 20 log mel-band energies and the loudness.
pub fn timbre(x: &[f32], sr: u32, hop_s: f32) -> Vec<[f32; 21]> {
    let hop = ((hop_s * sr as f32).round() as usize).max(1);
    if x.is_empty() || sr == 0 {
        return Vec::new();
    }
    // Centre the frames like the chroma's: pad half a window in front.
    let mut padded = vec![0.0f32; TIMBRE_FFT / 2];
    padded.extend_from_slice(x);
    let frames = x.len().div_ceil(hop);
    let bin_hz = sr as f32 / TIMBRE_FFT as f32;
    let (lo, hi) = (mel(MEL_LO_HZ), mel(MEL_HI_HZ));
    let edges: Vec<usize> = (0..=MEL_BANDS)
        .map(|i| {
            let m = lo + (hi - lo) * i as f32 / MEL_BANDS as f32;
            ((700.0 * (10f32.powf(m / 2595.0) - 1.0)) / bin_hz).round() as usize
        })
        .collect();
    let mut out: Vec<[f32; 21]> = magnitudes(&padded, TIMBRE_FFT, hop)
        .iter()
        .take(frames)
        .map(|frame| {
            let mut f = [0.0f32; 21];
            for (band, w) in edges.windows(2).enumerate() {
                let (a, b) = (w[0], w[1].max(w[0] + 1));
                let e: f32 = frame.get(a..b.min(frame.len())).unwrap_or(&[]).iter().map(|m| m * m).sum();
                f[band] = (1e-6 + e / (b - a) as f32).log10();
            }
            let power = frame.iter().map(|m| m * m).sum::<f32>() / frame.len().max(1) as f32;
            f[MEL_BANDS] = (1e-6 + power).log10();
            f
        })
        .collect();
    out.resize(frames, [(1e-6f32).log10(); 21]);
    out
}

fn unit(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-9 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Bar-by-bar similarity of two segments (bars `a..` and `b..`), compared position by position.
fn diagonal(sim: &[Vec<f32>], a: (usize, usize), b: (usize, usize)) -> f32 {
    let n = (a.1 - a.0).min(b.1 - b.0);
    if n == 0 {
        return 0.0;
    }
    (0..n).map(|k| sim.get(a.0 + k).and_then(|r| r.get(b.0 + k)).copied().unwrap_or(0.0)).sum::<f32>() / n as f32
}

/// Sections of the music described by `c` and `timbre` (same frames), on the bars of `tempo`.
/// `vocal` (per chroma frame, optional) marks instrumental sections.
pub fn segment(c: &Chroma, timbre: &[[f32; 21]], tempo: &Tempo, vocal: Option<&[f32]>) -> Vec<Section> {
    let duration = c.treble.len() as f32 * c.hop_s;
    if duration <= 0.0 {
        return Vec::new();
    }
    let mut bounds: Vec<f32> = Vec::new();
    if tempo.downbeats.first().is_none_or(|d| *d > 0.5) {
        bounds.push(0.0);
    }
    bounds.extend(tempo.downbeats.iter().copied().filter(|t| *t >= 0.0 && *t < duration));
    if let Some(first) = bounds.first_mut() {
        *first = 0.0;
    }
    bounds.push(duration);
    let bars = bounds.len().saturating_sub(1);
    let whole = || vec![Section { start: 0.0, end: duration, letter: 'A', label: SectionLabel::Other }];
    if bars < MIN_SECTION_BARS {
        return whole();
    }
    // Bar features: unit chroma plus standardised timbre scaled to unit length.
    let chroma_bars = sync(c, &bounds);
    let mut timbre_bars: Vec<[f32; 21]> = bounds
        .windows(2)
        .map(|w| {
            let (mut m, mut n) = ([0.0f32; 21], 0usize);
            for (i, f) in timbre.iter().enumerate() {
                let t = i as f32 * c.hop_s;
                if t >= w[0] && t < w[1] {
                    m.iter_mut().zip(f).for_each(|(a, b)| *a += b);
                    n += 1;
                }
            }
            m.map(|v| v / n.max(1) as f32)
        })
        .collect();
    for d in 0..21 {
        let mean = timbre_bars.iter().map(|f| f[d]).sum::<f32>() / bars as f32;
        let sd = (timbre_bars.iter().map(|f| (f[d] - mean).powi(2)).sum::<f32>() / bars as f32).sqrt();
        timbre_bars.iter_mut().for_each(|f| f[d] = if sd > 1e-6 { (f[d] - mean) / sd } else { 0.0 });
    }
    let features: Vec<Vec<f32>> = chroma_bars
        .iter()
        .zip(&timbre_bars)
        .map(|((tr, _, _), tb)| {
            let mut ch = tr.to_vec();
            unit(&mut ch);
            let mut tm = tb.to_vec();
            unit(&mut tm);
            ch.extend(tm);
            unit(&mut ch);
            ch
        })
        .collect();
    let sim: Vec<Vec<f32>> = features.iter().map(|a| features.iter().map(|b| a.iter().zip(b).map(|(x, y)| x * y).sum()).collect()).collect();
    // Checkerboard novelty at each bar start, Gaussian tapered.
    let k = KERNEL_HALF as isize;
    let novelty: Vec<f32> = (0..bars as isize)
        .map(|i| {
            let mut v = 0.0f32;
            for a in -k..k {
                for b in -k..k {
                    let (ia, ib) = ((i + a).clamp(0, bars as isize - 1) as usize, (i + b).clamp(0, bars as isize - 1) as usize);
                    let sign = if (a < 0) == (b < 0) { 1.0 } else { -1.0 };
                    let (fa, fb) = ((a as f32 + 0.5) / k as f32, (b as f32 + 0.5) / k as f32);
                    let w = (-2.0 * (fa * fa + fb * fb)).exp();
                    v += sign * w * sim[ia][ib];
                }
            }
            v
        })
        .collect();
    let mean = novelty.iter().sum::<f32>() / bars as f32;
    let sd = (novelty.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / bars as f32).sqrt();
    let threshold = mean + 0.5 * sd;
    let mut peaks: Vec<usize> = (1..bars)
        .filter(|&i| {
            let v = novelty[i];
            v > threshold && novelty.get(i - 1).is_none_or(|p| v >= *p) && novelty.get(i + 1).is_none_or(|n| v >= *n)
        })
        .collect();
    // Keep peaks at least MIN_SECTION_BARS apart, the stronger first.
    peaks.sort_by(|a, b| novelty[*b].total_cmp(&novelty[*a]));
    let mut kept: Vec<usize> = Vec::new();
    for p in peaks {
        if kept.iter().all(|q| p.abs_diff(*q) >= MIN_SECTION_BARS) && p >= MIN_SECTION_BARS.min(bars) / 2 {
            kept.push(p);
        }
    }
    kept.sort_unstable();
    let mut cuts = vec![0usize];
    cuts.extend(kept);
    cuts.push(bars);
    let segs: Vec<(usize, usize)> = cuts.windows(2).map(|w| (w[0], w[1])).collect();
    // Letters: a segment takes the letter of the first earlier segment it matches bar by bar.
    let mut letters: Vec<usize> = Vec::with_capacity(segs.len());
    let mut firsts: Vec<usize> = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let found = firsts.iter().position(|f| diagonal(&sim, segs[*f], *s) >= SAME_LETTER);
        match found {
            Some(l) => letters.push(l),
            None => {
                letters.push(firsts.len());
                firsts.push(i);
            }
        }
    }
    let n = segs.len();
    let count = |l: usize| letters.iter().filter(|x| **x == l).count();
    let at_edges_only = |l: usize| letters.iter().enumerate().all(|(i, x)| *x != l || i == 0 || i + 1 == n);
    let loudness = |s: (usize, usize)| (s.0..s.1).map(|b| timbre_bars.get(b).map_or(0.0, |f| f[20])).sum::<f32>() / (s.1 - s.0).max(1) as f32;
    let mut labels = vec![SectionLabel::Other; n];
    if n > 1 && at_edges_only(letters[0]) {
        labels[0] = SectionLabel::Intro;
    }
    if n > 1 && at_edges_only(letters[n - 1]) {
        labels[n - 1] = SectionLabel::Outro;
    }
    let middle: Vec<usize> = (0..n).filter(|i| labels[*i] == SectionLabel::Other).collect();
    let chorus = middle.iter().map(|i| letters[*i]).max_by(|a, b| {
        count(*a).cmp(&count(*b)).then_with(|| {
            let la = loudness(segs[firsts[*a]]);
            let lb = loudness(segs[firsts[*b]]);
            la.total_cmp(&lb)
        })
    });
    if let Some(ch) = chorus.filter(|ch| count(*ch) >= 2) {
        let mut before: Vec<usize> = vec![0; firsts.len()];
        for i in 1..n {
            if letters[i] == ch && letters[i - 1] != ch && labels[i - 1] == SectionLabel::Other {
                before[letters[i - 1]] += 1;
            }
        }
        let verse = before.iter().enumerate().filter(|(_, c)| **c > 0).max_by_key(|(_, c)| **c).map(|(l, _)| l);
        for i in middle.iter().copied() {
            if letters[i] == ch {
                labels[i] = SectionLabel::Chorus;
            } else if Some(letters[i]) == verse {
                labels[i] = SectionLabel::Verse;
            } else if count(letters[i]) == 1 && 2 * i >= n {
                labels[i] = SectionLabel::Bridge;
            }
        }
    }
    if let Some(v) = vocal {
        let frame_mean = |s: (usize, usize)| {
            let (a, b) = (bounds.get(s.0).copied().unwrap_or(0.0), bounds.get(s.1).copied().unwrap_or(duration));
            let idx = |t: f32| ((t / c.hop_s) as usize).min(v.len());
            let span = v.get(idx(a)..idx(b)).unwrap_or(&[]);
            span.iter().sum::<f32>() / span.len().max(1) as f32
        };
        let mut levels: Vec<f32> = segs.iter().map(|s| frame_mean(*s)).collect();
        let means = levels.clone();
        levels.sort_by(f32::total_cmp);
        let median = levels.get(levels.len() / 2).copied().unwrap_or(0.0);
        for (i, m) in means.iter().enumerate() {
            if median > 0.0 && *m < 0.1 * median && !matches!(labels[i], SectionLabel::Intro | SectionLabel::Outro) {
                labels[i] = SectionLabel::Instrumental;
            }
        }
    }
    segs.iter()
        .zip(letters.iter().zip(&labels))
        .map(|(s, (l, label))| Section {
            start: bounds.get(s.0).copied().unwrap_or(0.0),
            end: bounds.get(s.1).copied().unwrap_or(duration),
            letter: char::from(b'A' + (*l).min(25) as u8),
            label: *label,
        })
        .collect()
}
