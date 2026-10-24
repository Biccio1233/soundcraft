//! Chord recognition, phase A: beat-, bar- or frame-synchronous chroma scored against harmonic
//! templates of the vocabulary, smoothed by Viterbi, with the bass deciding inversions.

use crate::chroma::{Chroma, sync};
use crate::key::{Key, pitch_name};
use crate::tempo::Tempo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    Maj,
    Min,
    Dom7,
    Maj7,
    Min7,
    Sus2,
    Sus4,
    Dim,
    Aug,
}

pub const QUALITIES: [Quality; 9] =
    [Quality::Maj, Quality::Min, Quality::Dom7, Quality::Maj7, Quality::Min7, Quality::Sus2, Quality::Sus4, Quality::Dim, Quality::Aug];

impl Quality {
    /// Semitones above the root.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            Quality::Maj => &[0, 4, 7],
            Quality::Min => &[0, 3, 7],
            Quality::Dom7 => &[0, 4, 7, 10],
            Quality::Maj7 => &[0, 4, 7, 11],
            Quality::Min7 => &[0, 3, 7, 10],
            Quality::Sus2 => &[0, 2, 7],
            Quality::Sus4 => &[0, 5, 7],
            Quality::Dim => &[0, 3, 6],
            Quality::Aug => &[0, 4, 8],
        }
    }

    /// The label suffix after the root.
    pub fn suffix(self) -> &'static str {
        match self {
            Quality::Maj => "",
            Quality::Min => "m",
            Quality::Dom7 => "7",
            Quality::Maj7 => "maj7",
            Quality::Min7 => "m7",
            Quality::Sus2 => "sus2",
            Quality::Sus4 => "sus4",
            Quality::Dim => "dim",
            Quality::Aug => "aug",
        }
    }

    /// Score handicap against plain triads, so a richer chord needs real evidence.
    fn penalty(self) -> f32 {
        match self {
            Quality::Maj | Quality::Min => 0.0,
            Quality::Dom7 | Quality::Maj7 | Quality::Min7 => 0.01,
            Quality::Sus2 | Quality::Sus4 => 0.03,
            Quality::Dim | Quality::Aug => 0.04,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Chord {
    /// Pitch class of the root (0 = C).
    pub root: u8,
    pub quality: Quality,
    /// A bass note other than the root (an inversion), as a pitch class.
    pub bass: Option<u8>,
}

impl Chord {
    /// Pitch classes of the chord tones.
    pub fn tones(&self) -> Vec<u8> {
        self.quality.intervals().iter().map(|i| (self.root + i) % 12).collect()
    }

    /// "Am", "Cmaj7", "C/E"…, spelled for `key` (sharps without one).
    pub fn label(&self, key: Option<&Key>) -> String {
        let mut s = format!("{}{}", pitch_name(self.root, key), self.quality.suffix());
        if let Some(b) = self.bass {
            s.push('/');
            s.push_str(pitch_name(b, key));
        }
        s
    }

    /// The chord at difficulty `level`: 1 = major/minor triads without slash, 2 = adds 7, maj7, m7
    /// and sus (dim → minor, aug → major, no slash), 3 or more = unchanged.
    pub fn simplify(&self, level: u8) -> Chord {
        use Quality::*;
        let quality = match (level, self.quality) {
            (0 | 1, Maj | Dom7 | Maj7 | Sus2 | Sus4 | Aug) => Maj,
            (0 | 1, Min | Min7 | Dim) => Min,
            (2, Dim) => Min,
            (2, Aug) => Maj,
            (_, q) => q,
        };
        Chord { root: self.root, quality, bass: if level >= 3 { self.bass } else { None } }
    }

    /// The chord moved by `semitones`.
    pub fn transpose(&self, semitones: i8) -> Chord {
        let shift = |pc: u8| ((i16::from(pc) + i16::from(semitones)).rem_euclid(12)) as u8;
        Chord { root: shift(self.root), quality: self.quality, bass: self.bass.map(shift) }
    }

    /// Reads the labels [`Chord::label`] writes, with sharps or flats.
    pub fn parse(s: &str) -> Option<Chord> {
        let (head, bass) = match s.split_once('/') {
            Some((h, b)) => (h, Some(parse_note(b).filter(|(_, rest)| rest.is_empty())?.0)),
            None => (s, None),
        };
        let (root, rest) = parse_note(head)?;
        let quality = QUALITIES.iter().copied().find(|q| q.suffix() == rest)?;
        Some(Chord { root, quality, bass })
    }
}

/// A note name at the start of `s` and what follows it.
fn parse_note(s: &str) -> Option<(u8, &str)> {
    let mut chars = s.chars();
    let base: i16 = match chars.next()? {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (shift, rest) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b') {
        (-1, r)
    } else {
        (0, rest)
    };
    Some(((base + shift).rem_euclid(12) as u8, rest))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Grid {
    Beat,
    Bar,
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChordSegment {
    pub start: f32,
    pub end: f32,
    /// `None`: no chord (silence, drums alone).
    pub chord: Option<Chord>,
    /// Mean template match of the chosen chord, 0..1.
    pub confidence: f32,
}

/// Partial weights of a harmonic note on its own pitch class (partials 1, 2, 4), its fifth (3, 6)
/// and its major third (5), with partial k weighted 0.6^(k-1).
const HARMONIC_SELF: f32 = 1.0 + 0.6 + 0.216;
const HARMONIC_FIFTH: f32 = 0.36 + 0.077_76;
const HARMONIC_THIRD: f32 = 0.129_6;
/// Emission scale and per-change penalties of the Viterbi path, per grid.
const BETA: f32 = 10.0;
const NO_CHORD_ENERGY: f32 = 0.05;
/// Bonus for a chord whose root is the strongest bass note: it separates chords sharing their notes
/// (Csus2 = Gsus4 without the bass).
const BASS_ROOT_BONUS: f32 = 0.05;
/// A slash bass needs the root at under this fraction of the strongest bass pitch class.
const SLASH_ROOT_BELOW: f32 = 0.6;

fn change_penalty(grid: Grid) -> f32 {
    match grid {
        Grid::Bar => 1.0,
        Grid::Beat => 2.0,
        Grid::Free => 8.0,
    }
}

fn normalised(v: &[f32; 12]) -> [f32; 12] {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-9 { v.map(|x| x / n) } else { [0.0; 12] }
}

fn templates() -> Vec<(Chord, [f32; 12])> {
    let mut out = Vec::with_capacity(108);
    for root in 0..12u8 {
        for q in QUALITIES {
            let mut t = [0.0f32; 12];
            for i in q.intervals() {
                let pc = usize::from((root + i) % 12);
                t[pc] += HARMONIC_SELF;
                t[(pc + 7) % 12] += HARMONIC_FIFTH;
                t[(pc + 4) % 12] += HARMONIC_THIRD;
            }
            out.push((Chord { root, quality: q, bass: None }, normalised(&t)));
        }
    }
    out
}

/// Chords over `c`, synchronised to the beats, the bars (downbeats) or the chroma frames.
pub fn recognize(c: &Chroma, tempo: &Tempo, grid: Grid) -> Vec<ChordSegment> {
    if c.treble.is_empty() {
        return Vec::new();
    }
    let duration = c.treble.len() as f32 * c.hop_s;
    let marks = match grid {
        Grid::Beat => &tempo.beats,
        Grid::Bar => &tempo.downbeats,
        Grid::Free => &Vec::new(),
    };
    let mut bounds: Vec<f32> = if grid == Grid::Free || marks.is_empty() {
        (0..=c.treble.len()).map(|i| i as f32 * c.hop_s).collect()
    } else {
        let mut b = vec![0.0];
        b.extend(marks.iter().copied().filter(|t| *t > 0.0 && *t < duration));
        b.push(duration);
        b
    };
    bounds.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let obs = sync(c, &bounds);
    let n = obs.len();
    if n == 0 {
        return Vec::new();
    }
    let mut energies: Vec<f32> = obs.iter().map(|o| o.2).collect();
    energies.sort_by(f32::total_cmp);
    let median = energies.get(energies.len() / 2).copied().unwrap_or(0.0);
    let floor = (NO_CHORD_ENERGY * median).max(1e-6);
    let tpl = templates();
    let states = tpl.len() + 1; // the last state is "no chord"
    let none = tpl.len();
    let emission = |o: &([f32; 12], [f32; 12], f32), s: usize| -> f32 {
        let quiet = o.2 <= floor;
        if s == none {
            return if quiet { 1.0 } else { 0.0 };
        }
        if quiet {
            return 0.0;
        }
        let v = normalised(&o.0);
        let (chord, t) = &tpl[s];
        let bass_max = o.1.iter().copied().fold(0.0f32, f32::max);
        let bass_root = if bass_max > 1e-9 { o.1[usize::from(chord.root)] / bass_max } else { 0.0 };
        v.iter().zip(t).map(|(a, b)| a * b).sum::<f32>() - chord.quality.penalty() + BASS_ROOT_BONUS * bass_root
    };
    let penalty = change_penalty(grid);
    // Viterbi over states; the score is BETA × emission summed, minus `penalty` per change.
    let mut score: Vec<f32> = (0..states).map(|s| BETA * emission(&obs[0], s)).collect();
    let mut back = vec![vec![0usize; states]; n];
    for (i, o) in obs.iter().enumerate().skip(1) {
        let (best_prev, best_score) = score.iter().enumerate().fold((0, f32::NEG_INFINITY), |acc, (s, v)| if *v > acc.1 { (s, *v) } else { acc });
        let mut next = vec![0.0f32; states];
        for s in 0..states {
            let stay = score[s];
            let (from, base) = if stay >= best_score - penalty { (s, stay) } else { (best_prev, best_score - penalty) };
            next[s] = base + BETA * emission(o, s);
            back[i][s] = from;
        }
        score = next;
    }
    let mut path = vec![0usize; n];
    path[n - 1] = score.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(none, |(s, _)| s);
    for i in (1..n).rev() {
        path[i - 1] = back[i][path[i]];
    }
    // Merge runs, then decide each run's bass.
    let mut out: Vec<ChordSegment> = Vec::new();
    let mut i = 0;
    while i < n {
        let s = path[i];
        let mut j = i;
        while j + 1 < n && path[j + 1] == s {
            j += 1;
        }
        let (start, end) = (bounds.get(i).copied().unwrap_or(0.0), bounds.get(j + 1).copied().unwrap_or(duration));
        let confidence = (i..=j).map(|k| emission(&obs[k], s)).sum::<f32>() / (j - i + 1) as f32;
        let chord = tpl.get(s).map(|(chord, _)| {
            let mut bass = [0.0f32; 12];
            for o in &obs[i..=j] {
                for (b, v) in bass.iter_mut().zip(&o.1) {
                    *b += v;
                }
            }
            let (top, top_v) = bass.iter().enumerate().fold((0usize, 0.0f32), |acc, (p, v)| if *v > acc.1 { (p, *v) } else { acc });
            let top = top as u8;
            let slash = top != chord.root && chord.tones().contains(&top) && bass[usize::from(chord.root)] < SLASH_ROOT_BELOW * top_v;
            Chord { bass: slash.then_some(top), ..*chord }
        });
        out.push(ChordSegment { start, end, chord, confidence: confidence.clamp(0.0, 1.0) });
        i = j + 1;
    }
    out
}

/// Share of the true bars whose most-overlapping estimated chord has the right root and quality
/// (inversions ignored).
pub fn bar_accuracy(est: &[ChordSegment], truth: &[(f32, f32, Chord)]) -> f32 {
    if truth.is_empty() {
        return 0.0;
    }
    let right = truth
        .iter()
        .filter(|(s, e, want)| {
            let best = est.iter().max_by(|a, b| {
                let ov = |x: &ChordSegment| (x.end.min(*e) - x.start.max(*s)).max(0.0);
                ov(a).total_cmp(&ov(b))
            });
            best.and_then(|b| b.chord).is_some_and(|c| c.root == want.root && c.quality == want.quality)
        })
        .count();
    right as f32 / truth.len() as f32
}
