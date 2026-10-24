use soundcraft_wave_analysis::testsignal::{chord, drums};
use soundcraft_wave_analysis::{ANALYSIS_RATE, chroma, tempo};

const SR: u32 = ANALYSIS_RATE;

fn matched(truth: &[f32], est: &[f32], tol: f32) -> f32 {
    let hits = truth.iter().filter(|t| est.iter().any(|e| (e - *t).abs() <= tol)).count();
    hits as f32 / truth.len().max(1) as f32
}

#[test]
fn finds_tempo_of_a_drum_pattern() {
    let (mut x, beats) = drums(100.0, 9, 4, SR);
    x.truncate(20 * SR as usize);
    let truth: Vec<f32> = beats.into_iter().filter(|t| *t < 19.5).collect();
    let t = tempo::analyze(&x, SR, None, None);
    assert!((t.bpm - 100.0).abs() <= 1.0, "bpm {}", t.bpm);
    let m = matched(&truth, &t.beats, 0.04);
    assert!(m >= 0.9, "only {:.0} % of beats matched", m * 100.0);
    assert!(t.confidence > 0.0);
}

#[test]
fn tempi_away_from_the_prior() {
    for bpm in [72.0, 140.0, 180.0] {
        let bars = (20.0 * bpm / 240.0) as usize;
        let (x, _) = drums(bpm, bars, 4, SR);
        let t = tempo::analyze(&x, SR, None, None);
        assert!((t.bpm - bpm).abs() <= 2.0, "{bpm} BPM: estimated {}", t.bpm);
    }
}

#[test]
fn downbeats_follow_chord_changes() {
    // Bars of C, Am, F, G over the drums; the audio starts two beats into a bar (a pickup).
    let (bpm, bars) = (100.0, 10);
    let beat = 60.0 / bpm;
    let (mut x, _) = drums(bpm, bars, 4, SR);
    let progression = [[48.0, 60.0, 64.0, 67.0], [45.0, 57.0, 60.0, 64.0], [41.0, 57.0, 60.0, 65.0], [43.0, 55.0, 59.0, 62.0]];
    for b in 0..bars {
        let notes = progression[b % 4];
        let c = chord(&notes, 4.0 * beat, 440.0, SR);
        let at = (b as f32 * 4.0 * beat * SR as f32) as usize;
        for (o, v) in x.iter_mut().skip(at).zip(c) {
            *o += 1.5 * v;
        }
    }
    let skip = (2.0 * beat * SR as f32) as usize;
    let x = x.split_off(skip);
    let truth: Vec<f32> = (1..bars).map(|b| 2.0 * beat + (b - 1) as f32 * 4.0 * beat).filter(|t| *t < x.len() as f32 / SR as f32 - 1.0).collect();
    let c = chroma::compute(&x, SR, 0.0);
    let t = tempo::analyze(&x, SR, Some(&c), None);
    assert_eq!(t.beats_per_bar, 4);
    let m = matched(&truth, &t.downbeats, 0.04);
    assert!(m >= 0.9, "downbeats {:?} vs truth {:?}", t.downbeats, truth);
}

#[test]
fn silence_has_no_tempo() {
    let t = tempo::analyze(&vec![0.0; 5 * SR as usize], SR, None, None);
    assert_eq!(t.bpm, 0.0);
    assert!(t.beats.is_empty() && t.downbeats.is_empty());
    let (x, _) = drums(120.0, 1, 4, SR);
    let short = tempo::analyze(&x[..SR as usize], SR, None, Some(3));
    assert!(short.bpm.is_finite());
    let (env, fps) = tempo::onset_envelope(&x, SR);
    assert!(!env.is_empty() && (fps - SR as f32 / 220.0).abs() < 0.01);
}
