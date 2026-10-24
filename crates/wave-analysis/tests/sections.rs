use soundcraft_wave_analysis::sections::{self, SectionLabel};
use soundcraft_wave_analysis::tempo::Tempo;
use soundcraft_wave_analysis::testsignal::{acceptance_song, song};
use soundcraft_wave_analysis::{ANALYSIS_RATE, analysis_signal, chroma};

fn analysed() -> (Vec<sections::Section>, soundcraft_wave_analysis::testsignal::Truth) {
    let spec = acceptance_song();
    let (channels, truth) = song(&spec);
    let x = analysis_signal(&channels, spec.sample_rate);
    let c = chroma::compute(&x, ANALYSIS_RATE, 0.0);
    let timbre = sections::timbre(&x, ANALYSIS_RATE, c.hop_s);
    let tempo = Tempo { bpm: spec.bpm, beats: truth.beats.clone(), downbeats: truth.downbeats.clone(), beats_per_bar: 4, confidence: 1.0 };
    (sections::segment(&c, &timbre, &tempo, None), truth)
}

#[test]
fn acceptance_form_boundaries() {
    let (found, truth) = analysed();
    let beat = 60.0 / acceptance_song().bpm;
    let true_bounds: Vec<f32> = truth.sections.iter().skip(1).map(|s| s.0).collect();
    let est: Vec<f32> = found.iter().skip(1).map(|s| s.start).collect();
    for t in &true_bounds {
        assert!(est.iter().any(|e| (e - t).abs() <= beat + 1e-3), "boundary {t} missed: {est:?}");
    }
    assert!(est.len() <= true_bounds.len() + 1, "too many boundaries: {est:?} vs {true_bounds:?}");
    assert_eq!(found.first().map(|s| s.start), Some(0.0));
}

#[test]
fn repeats_share_letters() {
    let (found, truth) = analysed();
    let letter_at = |t: f32| found.iter().find(|s| s.start <= t && t < s.end).map(|s| s.letter);
    let mid = |label: SectionLabel| truth.sections.iter().filter(|s| s.2 == label).map(|s| (s.0 + s.1) / 2.0).collect::<Vec<_>>();
    let verses: Vec<_> = mid(SectionLabel::Verse).into_iter().map(letter_at).collect();
    let choruses: Vec<_> = mid(SectionLabel::Chorus).into_iter().map(letter_at).collect();
    assert!(verses.windows(2).all(|w| w[0] == w[1]) && verses[0].is_some(), "{verses:?}");
    assert!(choruses.windows(2).all(|w| w[0] == w[1]) && choruses[0].is_some(), "{choruses:?}");
    assert_ne!(verses[0], choruses[0]);
}

#[test]
fn chorus_and_verse_named() {
    let (found, truth) = analysed();
    let label_at = |t: f32| found.iter().find(|s| s.start <= t && t < s.end).map(|s| s.label);
    for (s, e, label) in &truth.sections {
        if matches!(label, SectionLabel::Verse | SectionLabel::Chorus) {
            assert_eq!(label_at((s + e) / 2.0), Some(*label), "{found:?}");
        }
    }
}

#[test]
fn short_audio_is_one_section() {
    let c = chroma::compute(&vec![0.0; ANALYSIS_RATE as usize * 3], ANALYSIS_RATE, 0.0);
    let timbre = sections::timbre(&vec![0.0; ANALYSIS_RATE as usize * 3], ANALYSIS_RATE, c.hop_s);
    let tempo = Tempo { bpm: 120.0, beats: vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5], downbeats: vec![0.0, 2.0], beats_per_bar: 4, confidence: 1.0 };
    let s = sections::segment(&c, &timbre, &tempo, None);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].label, SectionLabel::Other);
    assert!(sections::segment(&c, &timbre, &Tempo::default(), None).len() <= 1);
}
