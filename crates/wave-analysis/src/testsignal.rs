//! Synthetic signals with a known truth, for the analysis tests and the WaveCraft acceptance.

/// Frequency of MIDI note `midi` with A4 at `a4` Hz.
pub fn midi_hz(midi: f32, a4: f32) -> f32 {
    a4 * 2f32.powf((midi - 69.0) / 12.0)
}

/// A harmonic note (partials 1–6 at 1/k, 5 ms attack, exponential decay), peak about 0.25.
pub fn note(midi: f32, secs: f32, a4: f32, sr: u32) -> Vec<f32> {
    let f0 = midi_hz(midi, a4);
    let n = (secs.max(0.0) * sr as f32) as usize;
    let nyquist = sr as f32 * 0.45;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr as f32;
            let env = (t / 0.005).min(1.0) * (-t / 1.5).exp();
            let mut v = 0.0;
            for k in 1..=6 {
                let f = f0 * k as f32;
                if f < nyquist {
                    v += (std::f32::consts::TAU * f * t).sin() / k as f32;
                }
            }
            0.1 * env * v
        })
        .collect()
}

/// The notes `notes` (MIDI) played together.
pub fn chord(notes: &[f32], secs: f32, a4: f32, sr: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (secs.max(0.0) * sr as f32) as usize];
    for m in notes {
        for (o, v) in out.iter_mut().zip(note(*m, secs, a4, sr)) {
            *o += v;
        }
    }
    out
}

/// Deterministic white noise in −1..1 (xorshift).
pub fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.max(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as f32 / u32::MAX as f32 * 2.0 - 1.0
        })
        .collect()
}

fn add_at(out: &mut [f32], at: usize, src: &[f32], gain: f32) {
    for (o, v) in out.iter_mut().skip(at).zip(src) {
        *o += v * gain;
    }
}

/// One drum hit: kick (pitch-dropping sine), snare (noise and 190 Hz) or hi-hat (differentiated
/// noise), faded out over its last 10 ms so the cut adds no click.
pub fn drum(kind: Drum, sr: u32, seed: u32) -> Vec<f32> {
    let mut hit = drum_raw(kind, sr, seed);
    let fade = ((0.01 * sr as f32) as usize).clamp(1, hit.len().max(1));
    let len = hit.len();
    for (i, v) in hit.iter_mut().enumerate().skip(len.saturating_sub(fade)) {
        *v *= (len - i) as f32 / fade as f32;
    }
    hit
}

fn drum_raw(kind: Drum, sr: u32, seed: u32) -> Vec<f32> {
    let sr_f = sr as f32;
    match kind {
        Drum::Kick => (0..(0.25 * sr_f) as usize)
            .map(|i| {
                let t = i as f32 / sr_f;
                let phase = std::f32::consts::TAU * (50.0 * t + 60.0 * 0.03 * (1.0 - (-t / 0.03).exp()));
                0.9 * phase.sin() * (-t / 0.12).exp()
            })
            .collect(),
        Drum::Snare => {
            let n = noise((0.15 * sr_f) as usize, seed);
            n.iter()
                .enumerate()
                .map(|(i, v)| {
                    let t = i as f32 / sr_f;
                    (0.35 * v + 0.3 * (std::f32::consts::TAU * 190.0 * t).sin()) * (-t / 0.05).exp()
                })
                .collect()
        }
        Drum::HiHat => {
            let n = noise((0.05 * sr_f) as usize + 1, seed);
            n.windows(2).enumerate().map(|(i, w)| 0.08 * (w[1] - w[0]) * (-(i as f32 / sr_f) / 0.012).exp()).collect()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drum {
    Kick,
    Snare,
    HiHat,
}

/// A drum pattern: kick on the first beat and the middle of each bar, snare on the others' even
/// beats, hi-hat on every eighth. Returns the audio and the beat times (s).
pub fn drums(bpm: f32, bars: usize, beats_per_bar: usize, sr: u32) -> (Vec<f32>, Vec<f32>) {
    let beat = 60.0 / bpm.max(1.0);
    let beats = bars * beats_per_bar.max(1);
    let len = ((beats as f32 * beat + 0.5) * sr as f32) as usize;
    let mut out = vec![0.0f32; len];
    let (kick, snare) = (drum(Drum::Kick, sr, 1), drum(Drum::Snare, sr, 2));
    let mut times = Vec::with_capacity(beats);
    for b in 0..beats {
        let t = b as f32 * beat;
        times.push(t);
        let at = (t * sr as f32) as usize;
        let pos = b % beats_per_bar.max(1);
        if pos == 0 || (beats_per_bar >= 4 && pos == beats_per_bar / 2) {
            add_at(&mut out, at, &kick, 1.0);
        } else if pos % 2 == 1 {
            add_at(&mut out, at, &snare, 1.0);
        }
        for half in 0..2 {
            let hat = drum(Drum::HiHat, sr, 7 + (2 * b + half) as u32);
            add_at(&mut out, ((t + half as f32 * beat / 2.0) * sr as f32) as usize, &hat, 1.0);
        }
    }
    (out, times)
}

use crate::chords::{Chord, Quality};
use crate::sections::SectionLabel;

/// A song to render: one chord per bar, grouped in labelled sections.
#[derive(Debug, Clone, PartialEq)]
pub struct SongSpec {
    pub bpm: f32,
    pub a4_hz: f32,
    pub sample_rate: u32,
    pub sections: Vec<(SectionLabel, Vec<Chord>)>,
    pub seed: u32,
    /// The key the chords are in, carried into the truth.
    pub key: String,
}

/// What a rendered song contains, in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Truth {
    pub beats: Vec<f32>,
    pub downbeats: Vec<f32>,
    pub bars: Vec<(f32, f32, Chord)>,
    pub sections: Vec<(f32, f32, SectionLabel)>,
    pub key: String,
}

/// Stereo audio of `spec` (4/4) and its truth. Intro and outro: pad only. Verse: drums, bass on
/// beats 1 and 3, pad. Chorus: louder hi-hats, bass on every beat, pad doubled an octave up.
/// Bridge: no kick. The mix peaks at 0.8.
pub fn song(spec: &SongSpec) -> (Vec<Vec<f32>>, Truth) {
    let sr = spec.sample_rate.max(1);
    let beat = 60.0 / spec.bpm.max(1.0);
    let bar = 4.0 * beat;
    let total_bars: usize = spec.sections.iter().map(|s| s.1.len()).sum();
    let len = ((total_bars as f32 * bar + 1.0) * sr as f32) as usize;
    let (mut left, mut right) = (vec![0.0f32; len], vec![0.0f32; len]);
    let mut truth = Truth { beats: Vec::new(), downbeats: Vec::new(), bars: Vec::new(), sections: Vec::new(), key: spec.key.clone() };
    let (kick, snare) = (drum(Drum::Kick, sr, spec.seed), drum(Drum::Snare, sr, spec.seed.wrapping_add(1)));
    let mut b = 0usize;
    for (label, chords) in &spec.sections {
        let section_start = b as f32 * bar;
        let (drums_on, kick_on, chorus) = match label {
            SectionLabel::Intro | SectionLabel::Outro => (false, false, false),
            SectionLabel::Bridge => (true, false, false),
            SectionLabel::Chorus => (true, true, true),
            _ => (true, true, false),
        };
        for chord_bar in chords {
            let t0 = b as f32 * bar;
            truth.downbeats.push(t0);
            truth.bars.push((t0, t0 + bar, *chord_bar));
            let at = |t: f32| (t * sr as f32) as usize;
            // Pad: the chord tones from C4 up, re-struck each bar.
            let mut notes: Vec<f32> = chord_bar.quality.intervals().iter().map(|i| 60.0 + f32::from(chord_bar.root) + f32::from(*i)).collect();
            if chorus {
                notes.extend(notes.clone().iter().map(|n| n + 12.0));
            }
            let pad = chord(&notes, bar, spec.a4_hz, sr);
            add_at(&mut left, at(t0), &pad, 1.4);
            add_at(&mut right, at(t0), &pad, 1.0);
            for k in 0..4 {
                let t = t0 + k as f32 * beat;
                truth.beats.push(t);
                if !drums_on {
                    continue;
                }
                let hit = if k % 2 == 0 { kick_on.then_some(&kick) } else { Some(&snare) };
                if let Some(h) = hit {
                    add_at(&mut left, at(t), h, 1.0);
                    add_at(&mut right, at(t), h, 1.0);
                }
                for half in 0..2 {
                    let hat = drum(Drum::HiHat, sr, spec.seed.wrapping_add(10 + (8 * b + 2 * k + half) as u32));
                    let g = if chorus { 2.5 } else { 1.0 };
                    add_at(&mut left, at(t + half as f32 * beat / 2.0), &hat, g * 0.8);
                    add_at(&mut right, at(t + half as f32 * beat / 2.0), &hat, g * 1.2);
                }
                if chorus || k % 2 == 0 {
                    let bass =
                        note(36.0 + f32::from(chord_bar.bass.unwrap_or(chord_bar.root)), if chorus { beat } else { 2.0 * beat }, spec.a4_hz, sr);
                    add_at(&mut left, at(t), &bass, 2.0);
                    add_at(&mut right, at(t), &bass, 2.0);
                }
            }
            b += 1;
        }
        truth.sections.push((section_start, b as f32 * bar, *label));
    }
    let peak = left.iter().chain(&right).fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > 0.0 {
        let g = 0.8 / peak;
        left.iter_mut().chain(right.iter_mut()).for_each(|v| *v *= g);
    }
    (vec![left, right], truth)
}

/// The F2 acceptance song: A minor, 96 BPM, A4 = 436 Hz, 48 kHz; intro 4 | verse 8 | chorus 8 |
/// verse 8 | chorus 8 | bridge 4 | chorus 8 | outro 4 bars; verse Am F C G, chorus F G C Am,
/// bridge Dm E, intro and outro Am.
pub fn acceptance_song() -> SongSpec {
    let c = |root, quality| Chord { root, quality, bass: None };
    let (am, f, cm, g, dm, e) =
        (c(9, Quality::Min), c(5, Quality::Maj), c(0, Quality::Maj), c(7, Quality::Maj), c(2, Quality::Min), c(4, Quality::Maj));
    let verse: Vec<Chord> = [am, f, cm, g].iter().copied().cycle().take(8).collect();
    let chorus: Vec<Chord> = [f, g, cm, am].iter().copied().cycle().take(8).collect();
    SongSpec {
        bpm: 96.0,
        a4_hz: 436.0,
        sample_rate: 48_000,
        sections: vec![
            (SectionLabel::Intro, vec![am; 4]),
            (SectionLabel::Verse, verse.clone()),
            (SectionLabel::Chorus, chorus.clone()),
            (SectionLabel::Verse, verse),
            (SectionLabel::Chorus, chorus.clone()),
            (SectionLabel::Bridge, vec![dm, dm, e, e]),
            (SectionLabel::Chorus, chorus),
            (SectionLabel::Outro, vec![am; 4]),
        ],
        seed: 7,
        key: "A minor".into(),
    }
}
