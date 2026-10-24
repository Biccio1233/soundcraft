use soundcraft_wave_analysis::chords::{Chord, QUALITIES, Quality};
use soundcraft_wave_analysis::voicing::{self, Voicing};

const OPEN: [u8; 6] = [40, 45, 50, 55, 59, 64];

fn shape(s: &str) -> [i8; 6] {
    let mut f = [0i8; 6];
    for (i, c) in s.chars().enumerate().take(6) {
        f[i] = if c == 'x' { -1 } else { c.to_digit(10).map_or(-1, |d| d as i8) };
    }
    f
}

fn easiest(label: &str) -> Voicing {
    let c = Chord::parse(label).expect("a chord");
    voicing::voicings(&c).into_iter().next().unwrap_or_else(|| panic!("no voicing for {label}"))
}

#[test]
fn open_chords_first() {
    for (label, want) in [("C", "x32010"), ("Am", "x02210"), ("E", "022100"), ("Em", "022000"), ("D", "xx0232"), ("A", "x02220"), ("Dm", "xx0231")] {
        assert_eq!(easiest(label).frets, shape(want), "{label}");
    }
    let g = easiest("G").frets;
    assert!(g == shape("320003") || g == shape("320033"), "G: {g:?}");
    assert_eq!(easiest("Am").fingers, [0, 0, 2, 3, 1, 0]);
}

#[test]
fn f_is_a_barre_and_harder_than_c() {
    let f = easiest("F");
    assert!(f.frets == shape("xx3211") || f.frets == shape("133211"), "F: {:?}", f.frets);
    assert_eq!(f.barre, Some(1));
    assert!(f.difficulty > easiest("C").difficulty);
}

#[test]
fn every_chord_has_a_voicing() {
    let mut chords: Vec<Chord> = Vec::new();
    for root in 0..12u8 {
        for q in QUALITIES {
            chords.push(Chord { root, quality: q, bass: None });
        }
        chords.push(Chord { root, quality: Quality::Maj, bass: Some((root + 4) % 12) });
    }
    for c in chords {
        let vs = voicing::voicings(&c);
        assert!(!vs.is_empty() && vs.len() <= 3, "{}: {} voicings", c.label(None), vs.len());
        for v in vs {
            let sounding: Vec<u8> = (0..6).filter(|s| v.frets[*s] >= 0).map(|s| OPEN[s] + v.frets[s] as u8).collect();
            assert!(sounding.len() >= 4, "{}: {:?}", c.label(None), v.frets);
            assert!(sounding.iter().all(|n| c.tones().contains(&(n % 12))), "{}: {:?}", c.label(None), v.frets);
            assert_eq!(sounding.iter().min().map(|n| n % 12), Some(c.bass.unwrap_or(c.root)), "{}: {:?}", c.label(None), v.frets);
        }
    }
}

#[test]
fn capo_suggestion() {
    let chords = |labels: &[&str]| labels.iter().map(|l| Chord::parse(l).expect("chord")).collect::<Vec<_>>();
    let (capo, total) = voicing::suggest_capo(&chords(&["Eb", "Ab", "Bb", "Cm"]));
    assert!(capo == 1 || capo == 3, "capo {capo}");
    let no_capo: f32 = chords(&["Eb", "Ab", "Bb", "Cm"]).iter().map(|c| voicing::voicings(c)[0].difficulty).sum();
    assert!(total < no_capo);
    assert_eq!(voicing::suggest_capo(&chords(&["C", "G", "Am", "F"])).0, 0);
    assert_eq!(voicing::suggest_capo(&[]).0, 0);
}

#[test]
fn define_line_format() {
    assert_eq!(voicing::define_line("Am", &easiest("Am")), "{define: Am base-fret 1 frets x 0 2 2 1 0 fingers 0 0 2 3 1 0}");
    let high = Voicing { frets: [-1, 7, 9, 9, 8, 7], fingers: [0, 1, 3, 4, 2, 1], base_fret: 7, barre: Some(7), difficulty: 8.0 };
    assert_eq!(voicing::define_line("Em/B", &high), "{define: Em/B base-fret 7 frets x 1 3 3 2 1 fingers 0 1 3 4 2 1}");
}
