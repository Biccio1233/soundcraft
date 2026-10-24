use soundcraft_wave_analysis::testsignal::chord;
use soundcraft_wave_analysis::{ANALYSIS_RATE, chroma, key};

const SR: u32 = ANALYSIS_RATE;

fn progression(chords: &[&[f32]]) -> Vec<f32> {
    chords.iter().flat_map(|c| chord(c, 2.0, 440.0, SR)).collect()
}

fn key_of(chords: &[&[f32]]) -> Option<key::Key> {
    let c = chroma::compute(&progression(chords), SR, 0.0);
    key::estimate(&key::profile(&c))
}

const AM: &[f32] = &[45.0, 57.0, 60.0, 64.0];
const F: &[f32] = &[41.0, 57.0, 60.0, 65.0];
const C: &[f32] = &[48.0, 55.0, 60.0, 64.0];
const G: &[f32] = &[43.0, 55.0, 59.0, 62.0];
const DM: &[f32] = &[50.0, 57.0, 62.0, 65.0];
const E: &[f32] = &[40.0, 56.0, 59.0, 64.0];

#[test]
fn a_minor_progression() {
    let k = key_of(&[AM, F, C, G, AM, DM, E, AM]).expect("a key");
    assert_eq!(k.name, "A minor");
    assert_eq!((k.tonic, k.minor), (9, true));
    assert!(k.confidence > 0.5 && k.alternatives.len() == 3);
}

#[test]
fn e_flat_major_with_flats() {
    let eb: &[f32] = &[51.0, 55.0, 58.0, 63.0];
    let ab: &[f32] = &[44.0, 56.0, 60.0, 63.0];
    let bb: &[f32] = &[46.0, 58.0, 62.0, 65.0];
    let cm: &[f32] = &[48.0, 55.0, 60.0, 63.0];
    let k = key_of(&[eb, ab, bb, eb, cm, ab, bb, eb]).expect("a key");
    assert_eq!(k.name, "Eb major");
    assert_eq!(key::pitch_name(10, Some(&k)), "Bb");
    assert_eq!(key::pitch_name(3, Some(&k)), "Eb");
}

#[test]
fn c_major() {
    let k = key_of(&[C, F, G, C, F, G, C]).expect("a key");
    assert_eq!(k.name, "C major");
    let e_major = key::Key { tonic: 4, minor: false, name: "E major".into(), confidence: 1.0, alternatives: vec![] };
    assert_eq!(key::pitch_name(8, Some(&e_major)), "G#");
    assert_eq!(key::pitch_name(1, None), "C#");
}

#[test]
fn silence_has_no_key() {
    assert!(key::estimate(&[0.0; 12]).is_none());
    let c = chroma::compute(&vec![0.0; SR as usize], SR, 0.0);
    assert!(key::estimate(&key::profile(&c)).is_none());
}
