use soundcraft_wave_analysis::chords::{self, Chord, ChordSegment, Grid, Quality};
use soundcraft_wave_analysis::key::Key;
use soundcraft_wave_analysis::tempo::{self, Tempo};
use soundcraft_wave_analysis::testsignal::{chord, drums};
use soundcraft_wave_analysis::{ANALYSIS_RATE, chroma};

const SR: u32 = ANALYSIS_RATE;
const QUALITIES: [Quality; 9] =
    [Quality::Maj, Quality::Min, Quality::Dom7, Quality::Maj7, Quality::Min7, Quality::Sus2, Quality::Sus4, Quality::Dim, Quality::Aug];

/// The chord's bass (or root) in octave 2 and its tones close above C4.
fn render(c: &Chord, secs: f32) -> Vec<f32> {
    let bass = 36.0 + f32::from(c.bass.unwrap_or(c.root));
    let mut notes = vec![bass];
    notes.extend(c.quality.intervals().iter().map(|i| 60.0 + f32::from(c.root) + f32::from(*i)));
    chord(&notes, secs, 440.0, SR)
}

fn at(segs: &[ChordSegment], t: f32) -> Option<Chord> {
    segs.iter().find(|s| s.start <= t && t < s.end).and_then(|s| s.chord)
}

fn ch(root: u8, quality: Quality) -> Chord {
    Chord { root, quality, bass: None }
}

#[test]
fn each_quality_is_recognised() {
    for root in [0u8, 6, 10] {
        for q in QUALITIES {
            let want = ch(root, q);
            let c = chroma::compute(&render(&want, 2.0), SR, 0.0);
            let segs = chords::recognize(&c, &Tempo::default(), Grid::Free);
            assert_eq!(at(&segs, 1.0), Some(want), "root {root} {q:?}: {segs:?}");
        }
    }
}

#[test]
fn slash_bass() {
    for (want, label) in
        [(Chord { root: 0, quality: Quality::Maj, bass: Some(4) }, "C/E"), (Chord { root: 7, quality: Quality::Maj, bass: Some(11) }, "G/B")]
    {
        let c = chroma::compute(&render(&want, 2.0), SR, 0.0);
        let segs = chords::recognize(&c, &Tempo::default(), Grid::Free);
        let got = at(&segs, 1.0);
        assert_eq!(got, Some(want), "{segs:?}");
        assert_eq!(got.map(|c| c.label(None)).as_deref(), Some(label));
    }
}

#[test]
fn progression_bars() {
    let (bpm, bars) = (100.0, 16);
    let bar = 4.0 * 60.0 / bpm;
    let (mut x, _) = drums(bpm, bars, 4, SR);
    let prog = [ch(9, Quality::Min), ch(5, Quality::Maj), ch(0, Quality::Maj), ch(7, Quality::Maj)];
    let mut truth = Vec::new();
    for b in 0..bars {
        let c = prog[b % 4];
        let at = (b as f32 * bar * SR as f32) as usize;
        for (o, v) in x.iter_mut().skip(at).zip(render(&c, bar)) {
            *o += 1.5 * v;
        }
        truth.push((b as f32 * bar, (b + 1) as f32 * bar, c));
    }
    let c = chroma::compute(&x, SR, 0.0);
    let t = tempo::analyze(&x, SR, Some(&c), None);
    let segs = chords::recognize(&c, &t, Grid::Bar);
    let acc = chords::bar_accuracy(&segs, &truth);
    assert!(acc >= 0.85, "bar accuracy {acc}: {segs:?}");
}

#[test]
fn levels_simplify() {
    let label = |c: Chord, level| c.simplify(level).label(None);
    let cmaj7_e = Chord { root: 0, quality: Quality::Maj7, bass: Some(4) };
    assert_eq!([label(cmaj7_e, 1), label(cmaj7_e, 2), label(cmaj7_e, 3)], ["C", "Cmaj7", "Cmaj7/E"]);
    assert_eq!(label(ch(11, Quality::Dim), 1), "Bm");
    assert_eq!(label(ch(0, Quality::Aug), 2), "C");
    assert_eq!([label(ch(0, Quality::Sus4), 1), label(ch(0, Quality::Sus4), 2)], ["C", "Csus4"]);
    assert_eq!(label(ch(9, Quality::Min7), 1), "Am");
    assert_eq!(label(ch(7, Quality::Dom7), 2), "G7");
}

#[test]
fn spelling_follows_key() {
    let key = |tonic, minor, name: &str| Key { tonic, minor, name: name.into(), confidence: 1.0, alternatives: vec![] };
    let eb = key(3, false, "Eb major");
    assert_eq!(ch(3, Quality::Maj).label(Some(&eb)), "Eb");
    assert_eq!(Chord { root: 10, quality: Quality::Dom7, bass: Some(2) }.label(Some(&eb)), "Bb7/D");
    assert_eq!(ch(8, Quality::Min).label(Some(&key(4, false, "E major"))), "G#m");
    assert_eq!(ch(1, Quality::Maj).transpose(-2).label(None), "B");
}

#[test]
fn parse_round_trips() {
    for root in 0..12u8 {
        for q in QUALITIES {
            let c = ch(root, q);
            assert_eq!(Chord::parse(&c.label(None)), Some(c), "{}", c.label(None));
            let slash = Chord { bass: Some((root + 4) % 12), ..c };
            assert_eq!(Chord::parse(&slash.label(None)), Some(slash));
        }
    }
    assert_eq!(Chord::parse("Bbmaj7"), Some(ch(10, Quality::Maj7)));
    assert_eq!(Chord::parse("Ebm7/Db"), Some(Chord { root: 3, quality: Quality::Min7, bass: Some(1) }));
    assert_eq!(Chord::parse("H"), None);
    assert_eq!(Chord::parse(""), None);
    assert_eq!(Chord::parse("C/"), None);
}

#[test]
fn silence_is_no_chord() {
    let c = chroma::compute(&vec![0.0; 3 * SR as usize], SR, 0.0);
    let segs = chords::recognize(&c, &Tempo::default(), Grid::Free);
    assert!(segs.iter().all(|s| s.chord.is_none()));
    assert!(chords::recognize(&chroma::Chroma::default(), &Tempo::default(), Grid::Bar).is_empty());
}

#[test]
fn settle_key_prefers_the_tonic_chord() {
    let key = |tonic, minor, name: &str, alt: &str| Key { tonic, minor, name: name.into(), confidence: 0.90, alternatives: vec![(alt.into(), 0.85)] };
    let seg = |s: f32, c: Chord| ChordSegment { start: s, end: s + 2.0, chord: Some(c), confidence: 1.0 };
    let (am, c, f, g) = (ch(9, Quality::Min), ch(0, Quality::Maj), ch(5, Quality::Maj), ch(7, Quality::Maj));
    // KK reads the relative major, but the song lives on Am.
    let minor_song: Vec<ChordSegment> = [am, am, f, c, g, am, f, g, c, am, am].iter().enumerate().map(|(i, x)| seg(i as f32 * 2.0, *x)).collect();
    let settled = chords::settle_key(key(0, false, "C major", "A minor"), &minor_song);
    assert_eq!((settled.name.as_str(), settled.tonic, settled.minor), ("A minor", 9, true));
    assert_eq!(settled.alternatives.first().map(|a| a.0.as_str()), Some("C major"));
    // A song on C stays in C; a distant runner-up is left alone.
    let major_song: Vec<ChordSegment> = [c, f, g, c, am, f, g, c].iter().enumerate().map(|(i, x)| seg(i as f32 * 2.0, *x)).collect();
    assert_eq!(chords::settle_key(key(0, false, "C major", "A minor"), &major_song).name, "C major");
    let far = Key { alternatives: vec![("A minor".into(), 0.5)], ..key(0, false, "C major", "A minor") };
    assert_eq!(chords::settle_key(far, &minor_song).name, "C major");
}
