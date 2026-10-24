//! Guitar voicings from our own rules: every shape within a four-fret window that sounds only
//! chord tones, has the bass lowest, leaves no hole between sounding strings and needs at most four
//! fingers (a barre counts as one), scored for how hard it is to play: fingers, barre length,
//! stretch and position count against a shape, open strings and fullness for it.

use crate::chords::{Chord, Quality};

/// Standard tuning, low E to high E, as MIDI notes.
const OPEN: [u8; 6] = [40, 45, 50, 55, 59, 64];
const MAX_WINDOW_START: i8 = 12;
const MIN_SOUNDING: usize = 4;
/// A capo is suggested only when it saves at least this much difficulty per chord.
const CAPO_GAIN_PER_CHORD: f32 = 1.5;
const MAX_CAPO: u8 = 7;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Voicing {
    /// Per string (low E first): −1 muted, 0 open, else the fret.
    pub frets: [i8; 6],
    /// Per string: 0 for open or muted, else the finger (1 index … 4 little finger).
    pub fingers: [u8; 6],
    /// The fret the diagram starts at (1 for shapes reaching no higher than the fourth fret).
    pub base_fret: u8,
    /// The fret a barre lies on.
    pub barre: Option<u8>,
    pub difficulty: f32,
}

struct Shape {
    frets: [i8; 6],
    fingers: [u8; 6],
    barre: Option<(i8, usize, usize)>,
    count: usize,
}

/// Fingers for `frets`: a barre on the lowest fretted fret when it covers two or more strings with
/// every string between them fretted, then one finger per remaining note in fret order.
fn fingering(frets: &[i8; 6]) -> Option<Shape> {
    let fretted: Vec<usize> = (0..6).filter(|s| frets[*s] > 0).collect();
    let low = fretted.iter().map(|s| frets[*s]).min();
    let mut barre = None;
    if let Some(low) = low {
        let on_low: Vec<usize> = fretted.iter().copied().filter(|s| frets[*s] == low).collect();
        if let (Some(&a), Some(&b)) = (on_low.first(), on_low.last())
            && on_low.len() >= 2
            && (a..=b).all(|s| frets[s] >= low)
        {
            barre = Some((low, a, b));
        }
    }
    let mut order: Vec<usize> = fretted.iter().copied().filter(|s| !barre.is_some_and(|(f, a, b)| frets[*s] == f && (a..=b).contains(s))).collect();
    order.sort_by_key(|s| (frets[*s], *s));
    let count = order.len() + usize::from(barre.is_some());
    if count > 4 {
        return None;
    }
    let mut fingers = [0u8; 6];
    let first = if let Some((f, a, b)) = barre {
        for s in a..=b {
            if frets[s] == f {
                fingers[s] = 1;
            }
        }
        2
    } else {
        1
    };
    for (i, s) in order.iter().enumerate() {
        fingers[*s] = (first + i) as u8;
    }
    Some(Shape { frets: *frets, fingers, barre, count })
}

fn difficulty(sh: &Shape) -> f32 {
    let fretted: Vec<i8> = sh.frets.iter().copied().filter(|f| *f > 0).collect();
    let span = match (fretted.iter().min(), fretted.iter().max()) {
        (Some(a), Some(b)) => f32::from(b - a),
        _ => 0.0,
    };
    let open = sh.frets.iter().filter(|f| **f == 0).count() as f32;
    let sounding = sh.frets.iter().filter(|f| **f >= 0).count() as f32;
    let barre = sh.barre.map_or(0.0, |(_, a, b)| 0.5 * (b - a + 1) as f32);
    let position =
        fretted.iter().min().map_or(0.0, |m| 0.3 * f32::from((*m - 1).max(0)) * f32::from(u8::from(fretted.iter().max().is_some_and(|x| *x > 4))));
    // Muting the top strings while fretting lower ones needs damping: harder than leaving them out low.
    let top_muted = sh.frets.iter().rev().take_while(|f| **f < 0).count() as f32;
    // Open strings help low down; under a hand above the fifth fret they are an awkward stretch.
    let open_score = if fretted.iter().any(|f| *f > 5) { 0.8 * open } else { -0.3 * open };
    (sh.count as f32 + barre + 0.5 * span + position + open_score + 0.4 * (6.0 - sounding) + 0.8 * top_muted).max(0.0)
}

/// Up to three playable shapes for `chord`, easiest first.
pub fn voicings(chord: &Chord) -> Vec<Voicing> {
    let tones = chord.tones();
    let bass = chord.bass.unwrap_or(chord.root);
    let fifth = (chord.root + 7) % 12;
    let required: Vec<u8> =
        tones.iter().copied().filter(|t| !(*t == fifth && matches!(chord.quality, Quality::Dom7 | Quality::Maj7 | Quality::Min7))).collect();
    let mut found: Vec<(f32, [i8; 6], [u8; 6], Option<u8>)> = Vec::new();
    for start in 1..=MAX_WINDOW_START {
        // Options per string: muted, open, or a fret of the window, sounding a chord tone.
        let options: Vec<Vec<i8>> = OPEN
            .iter()
            .map(|open| {
                let mut o = vec![-1i8];
                for f in std::iter::once(0).chain(start..start + 4) {
                    if tones.contains(&((open + f as u8) % 12)) {
                        o.push(f);
                    }
                }
                o
            })
            .collect();
        let mut frets = [-1i8; 6];
        search(0, &options, &mut frets, &mut |f| {
            let sounding: Vec<usize> = (0..6).filter(|s| f[*s] >= 0).collect();
            let (Some(&lo), Some(&hi)) = (sounding.first(), sounding.last()) else { return };
            if sounding.len() < MIN_SOUNDING || (lo..=hi).any(|s| f[s] < 0) {
                return;
            }
            let note = |s: usize| OPEN[s] + f[s] as u8;
            let lowest = sounding.iter().map(|s| note(*s)).min().unwrap_or(0);
            if lowest % 12 != bass || !required.iter().all(|t| sounding.iter().any(|s| note(*s) % 12 == *t)) {
                return;
            }
            // Fretted notes must sit inside this window (the open strings are free).
            if f.iter().any(|x| *x > 0 && (*x < start || *x >= start + 4)) || (!f.contains(&start) && start > 1) {
                return;
            }
            if let Some(sh) = fingering(f) {
                let d = difficulty(&sh);
                if !found.iter().any(|(_, fr, _, _)| fr == f) {
                    found.push((d, sh.frets, sh.fingers, sh.barre.map(|(b, _, _)| b as u8)));
                }
            }
        });
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    found
        .into_iter()
        .take(3)
        .map(|(d, frets, fingers, barre)| {
            let max = frets.iter().copied().max().unwrap_or(0);
            let min_fretted = frets.iter().copied().filter(|f| *f > 0).min().unwrap_or(1);
            let base_fret = if max <= 4 { 1 } else { min_fretted.max(1) as u8 };
            Voicing { frets, fingers, base_fret, barre, difficulty: d }
        })
        .collect()
}

fn search(string: usize, options: &[Vec<i8>], frets: &mut [i8; 6], visit: &mut impl FnMut(&[i8; 6])) {
    let Some(opts) = options.get(string) else {
        visit(frets);
        return;
    };
    for f in opts {
        frets[string] = *f;
        search(string + 1, options, frets, visit);
    }
    frets[string] = -1;
}

/// The capo (0–7) that makes `chords` easiest to play as shapes, with the summed difficulty. A capo
/// is chosen only when it saves at least 1.5 per chord over playing without one.
pub fn suggest_capo(chords: &[Chord]) -> (u8, f32) {
    if chords.is_empty() {
        return (0, 0.0);
    }
    let total = |capo: u8| -> f32 { chords.iter().map(|c| voicings(&c.transpose(-(capo as i8))).first().map_or(20.0, |v| v.difficulty)).sum() };
    let open = total(0);
    let (best, best_total) = (1..=MAX_CAPO).map(|c| (c, total(c))).fold((0u8, open), |acc, (c, t)| if t < acc.1 - 1e-4 { (c, t) } else { acc });
    if best > 0 && open - best_total >= CAPO_GAIN_PER_CHORD * chords.len() as f32 { (best, best_total) } else { (0, open) }
}

/// The ChordPro `{define}` line for `v`: frets relative to the base fret, `x` for muted strings.
pub fn define_line(label: &str, v: &Voicing) -> String {
    let base = i16::from(v.base_fret.max(1));
    let frets: Vec<String> = v
        .frets
        .iter()
        .map(|f| match f {
            f if *f < 0 => "x".to_string(),
            0 => "0".to_string(),
            f => (i16::from(*f) - base + 1).to_string(),
        })
        .collect();
    let fingers: Vec<String> = v.fingers.iter().map(u8::to_string).collect();
    format!("{{define: {label} base-fret {} frets {} fingers {}}}", v.base_fret.max(1), frets.join(" "), fingers.join(" "))
}
