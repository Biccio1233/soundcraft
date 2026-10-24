//! Key estimate (Krumhansl–Kessler probe-tone profiles) and pitch spelling that follows the key.

use crate::chroma::Chroma;

const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
const MAJOR_TONICS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
const MINOR_TONICS: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B"];
const SHARPS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const FLATS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
/// Tonics (pitch classes) whose key signature has flats.
const FLAT_MAJORS: [u8; 5] = [5, 10, 3, 8, 1];
const FLAT_MINORS: [u8; 6] = [2, 7, 0, 5, 10, 3];

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Key {
    /// Pitch class of the tonic (0 = C).
    pub tonic: u8,
    pub minor: bool,
    /// For example "A minor", "Eb major".
    pub name: String,
    /// Correlation with the winning profile, −1..1.
    pub confidence: f32,
    /// The next three candidates with their correlation.
    pub alternatives: Vec<(String, f32)>,
}

fn name(tonic: u8, minor: bool) -> String {
    let i = usize::from(tonic % 12);
    if minor { format!("{} minor", MINOR_TONICS[i]) } else { format!("{} major", MAJOR_TONICS[i]) }
}

fn pearson(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let (ma, mb) = (a.iter().sum::<f32>() / 12.0, b.iter().sum::<f32>() / 12.0);
    let (mut num, mut da, mut db) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        num += (x - ma) * (y - mb);
        da += (x - ma) * (x - ma);
        db += (y - mb) * (y - mb);
    }
    if da > 1e-12 && db > 1e-12 { num / (da * db).sqrt() } else { 0.0 }
}

/// The pitch-class profile of a whole chroma: treble plus half the bass, summed over the frames.
pub fn profile(c: &Chroma) -> [f32; 12] {
    let mut p = [0.0f32; 12];
    for (t, b) in c.treble.iter().zip(&c.bass) {
        for i in 0..12 {
            p[i] += t[i] + 0.5 * b[i];
        }
    }
    p
}

/// The key whose rotated profile correlates best with `profile`; `None` for a flat or silent profile.
pub fn estimate(profile: &[f32; 12]) -> Option<Key> {
    if profile.iter().any(|v| !v.is_finite()) || profile.iter().all(|v| *v <= 0.0) {
        return None;
    }
    let mut scores: Vec<(u8, bool, f32)> = Vec::with_capacity(24);
    for tonic in 0..12u8 {
        for minor in [false, true] {
            let base = if minor { &MINOR } else { &MAJOR };
            let mut rotated = [0.0f32; 12];
            for (i, r) in rotated.iter_mut().enumerate() {
                *r = base[(i + 12 - usize::from(tonic)) % 12];
            }
            scores.push((tonic, minor, pearson(profile, &rotated)));
        }
    }
    scores.sort_by(|a, b| b.2.total_cmp(&a.2));
    let &(tonic, minor, confidence) = scores.first()?;
    if confidence <= 0.0 {
        return None;
    }
    let alternatives = scores.iter().skip(1).take(3).map(|(t, m, s)| (name(*t, *m), *s)).collect();
    Some(Key { tonic, minor, name: name(tonic, minor), confidence, alternatives })
}

/// The name of pitch class `pc` in `key`: flats in flat keys, sharps otherwise (and without a key).
pub fn pitch_name(pc: u8, key: Option<&Key>) -> &'static str {
    let flat = key.is_some_and(|k| if k.minor { FLAT_MINORS.contains(&(k.tonic % 12)) } else { FLAT_MAJORS.contains(&(k.tonic % 12)) });
    let i = usize::from(pc % 12);
    if flat { FLATS[i] } else { SHARPS[i] }
}
