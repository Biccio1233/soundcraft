//! Where a voice sings: the regions of a (vocals) signal well above its own floor, and word ends read
//! from the same envelope. Transcription runs on the regions only, so silence and the band's quiet
//! bleed in a vocals stem never reach the model, which would otherwise invent words there.

use crate::lyrics::Word;
use std::ops::Range;

/// Envelope frame, in seconds.
const FRAME_S: f32 = 0.02;
/// Voiced: within this many dB of the loud level (the 99th percentile frame)…
const RANGE_DB: f32 = 30.0;
/// …and above this level in dBFS, where a stem's bleed sits below.
const FLOOR_DB: f32 = -50.0;
/// Pauses shorter than this stay inside one region.
const MERGE_S: f32 = 1.0;
/// Margin kept around each region.
const PAD_S: f32 = 0.3;
/// Voiced runs shorter than this in all are clicks, not singing.
const MIN_S: f32 = 0.15;
/// A word lasts at most this long.
const MAX_WORD_S: f32 = 2.5;

struct Envelope {
    hop: usize,
    db: Vec<f32>,
    threshold: f32,
}

impl Envelope {
    fn new(x: &[f32], sample_rate: u32) -> Envelope {
        let hop = ((sample_rate as f32 * FRAME_S) as usize).max(1);
        let db: Vec<f32> = x
            .chunks(hop)
            .map(|c| {
                let e = c.iter().map(|v| if v.is_finite() { f64::from(*v).powi(2) } else { 0.0 }).sum::<f64>() / c.len().max(1) as f64;
                (10.0 * (e + 1e-12).log10()) as f32
            })
            .collect();
        let mut sorted = db.clone();
        sorted.sort_by(f32::total_cmp);
        let loud = sorted.get((sorted.len() as f32 * 0.99) as usize).or(sorted.last()).copied().unwrap_or(-120.0);
        Envelope { hop, db, threshold: (loud - RANGE_DB).max(FLOOR_DB) }
    }

    fn voiced(&self, frame: usize) -> bool {
        self.db.get(frame).is_some_and(|d| *d > self.threshold)
    }
}

fn frames(seconds: f32) -> usize {
    ((seconds / FRAME_S).round() as usize).max(1)
}

/// The sample ranges of `x` where a voice sings, padded and with short pauses kept inside.
pub fn regions(x: &[f32], sample_rate: u32) -> Vec<Range<usize>> {
    if x.is_empty() || sample_rate == 0 {
        return Vec::new();
    }
    let env = Envelope::new(x, sample_rate);
    // (start, end, voiced frames) of runs merged across short pauses.
    let mut merged: Vec<(usize, usize, usize)> = Vec::new();
    let mut run: Option<usize> = None;
    for i in 0..=env.db.len() {
        match (env.voiced(i), run) {
            (true, None) => run = Some(i),
            (false, Some(s)) => {
                run = None;
                match merged.last_mut() {
                    Some(last) if s.saturating_sub(last.1) < frames(MERGE_S) => {
                        last.1 = i;
                        last.2 += i - s;
                    }
                    _ => merged.push((s, i, i - s)),
                }
            }
            _ => {}
        }
    }
    let pad = frames(PAD_S);
    merged
        .into_iter()
        .filter(|m| m.2 >= frames(MIN_S))
        .map(|(s, e, _)| {
            let end = e.saturating_add(pad).saturating_mul(env.hop).min(x.len());
            s.saturating_sub(pad).saturating_mul(env.hop).min(end)..end
        })
        .collect()
}

/// Set each word's end where the voice stops after its start (three quiet frames), never past the
/// next word's start nor [`MAX_WORD_S`] after its own; legato words end where the next begins.
pub fn settle_word_ends(words: &mut [Word], x: &[f32], sample_rate: u32) {
    if sample_rate == 0 {
        return;
    }
    let env = Envelope::new(x, sample_rate);
    let frame_s = env.hop as f32 / sample_rate as f32;
    let len_s = x.len() as f32 / sample_rate as f32;
    let starts: Vec<f32> = words.iter().map(|w| w.start).collect();
    for (i, w) in words.iter_mut().enumerate() {
        let next = starts.get(i + 1).copied().unwrap_or(f32::INFINITY);
        let lo = w.start + 0.05;
        let hi = next.min(w.start + MAX_WORD_S).min(len_s).max(lo);
        let first = (w.start.max(0.0) / frame_s) as usize;
        let last = (hi / frame_s).ceil() as usize;
        let onset = (first..first.saturating_add(frames(0.3)).min(last)).find(|f| env.voiced(*f));
        let end = match onset {
            None => w.start + 0.3,
            Some(on) => {
                let mut quiet = 0;
                let stop = (on + 1..last).find(|f| {
                    quiet = if env.voiced(*f) { 0 } else { quiet + 1 };
                    quiet == 3
                });
                stop.map_or(hi, |f| (f + 1 - 3) as f32 * frame_s)
            }
        };
        w.end = end.clamp(lo, hi);
    }
}
