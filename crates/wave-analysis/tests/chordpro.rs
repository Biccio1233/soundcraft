use soundcraft_wave_analysis::chordpro::{self, Song};
use soundcraft_wave_analysis::lyrics::{self, Line, Word};
use soundcraft_wave_analysis::sections::{Section, SectionLabel};
use soundcraft_wave_analysis::voicing::Voicing;

fn w(text: &str, start: f32, end: f32) -> Word {
    Word { text: text.into(), start, end }
}

fn song(lines: Vec<Line>, chords: &[(f32, f32, &str)]) -> Song {
    Song {
        title: "Prova".into(),
        key: Some("Am".into()),
        bpm: Some(96.0),
        capo: 0,
        lines,
        chords: chords.iter().map(|(s, e, l)| (*s, *e, l.to_string())).collect(),
        sections: vec![],
        defines: vec![],
        bar_s: 2.5,
    }
}

fn two_lines() -> Vec<Line> {
    vec![
        Line { words: vec![w("Chiusa", 0.0, 0.5), w("vive", 0.5, 1.0), w("dentro", 1.0, 1.5)] },
        Line { words: vec![w("di", 2.0, 2.3), w("me", 2.3, 2.8)] },
    ]
}

#[test]
fn align_transfers_times_and_fills_gaps() {
    let heard = [w("chiusa", 0.0, 0.5), w("vive", 0.5, 1.0), w("dentro", 1.0, 1.5), w("me", 2.0, 2.5)];
    let lines = lyrics::align("Chiusa, vive\ndentro di me", &heard);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].words[0].text, "Chiusa,");
    assert_eq!((lines[0].words[0].start, lines[0].words[0].end), (0.0, 0.5));
    let di = &lines[1].words[1];
    assert_eq!(di.text, "di");
    assert!(di.start >= 1.5 && di.end <= 2.0 && di.start <= di.end, "{di:?}");
    assert_eq!(lines[1].words[2].start, 2.0);
}

#[test]
fn align_tolerates_misheard_words() {
    let heard = [w("chiusa", 0.0, 0.5), w("vivi", 0.5, 1.0)];
    let lines = lyrics::align("Chiusa vive", &heard);
    assert_eq!((lines[0].words[1].start, lines[0].words[1].end), (0.5, 1.0));
}

#[test]
fn align_without_overlap_is_monotone() {
    let heard = [w("a", 0.0, 0.5), w("b", 1.0, 1.5)];
    for text in ["x y z w q", "x", "uno due\ntre quattro cinque sei sette"] {
        let lines = lyrics::align(text, &heard);
        let words: Vec<&Word> = lines.iter().flat_map(|l| &l.words).collect();
        assert_eq!(words.len(), text.split_whitespace().count());
        let extra = words.len() as f32 * 0.3;
        for p in words.windows(2) {
            assert!(p[1].start >= p[0].start && p[0].end >= p[0].start, "{p:?}");
        }
        assert!(words.iter().all(|x| x.start >= 0.0 && x.end <= 1.5 + extra + 1e-4), "{words:?}");
    }
    assert!(lyrics::align("", &heard).is_empty());
    let alone = lyrics::align("solo due", &[]);
    assert_eq!(alone[0].words[1].start, 0.3);
}

#[test]
fn lines_from_words_split_on_gaps() {
    let heard = [w("a", 0.0, 0.4), w("b", 0.5, 0.9), w("c", 2.0, 2.4)];
    let lines = lyrics::lines_from_words(&heard);
    assert_eq!(lines.iter().map(|l| l.words.len()).collect::<Vec<_>>(), vec![2, 1]);
}

#[test]
fn chordpro_places_chords_on_words() {
    let s = song(two_lines(), &[(0.0, 1.0, "Am"), (1.0, 2.3, "F"), (2.3, 4.0, "C")]);
    assert_eq!(chordpro::render_cho(&s), "{title: Prova}\n{key: Am}\n{tempo: 96}\n\n[Am]Chiusa vive [F]dentro\n[F]di [C]me\n");
}

#[test]
fn instrumental_gap_gets_a_chord_line() {
    let lines = vec![Line { words: vec![w("Chiusa", 0.0, 0.5)] }, Line { words: vec![w("vive", 6.0, 6.5)] }];
    let s = song(lines, &[(0.0, 2.0, "Am"), (2.0, 4.0, "G"), (4.0, 6.0, "Em"), (6.0, 8.0, "Am")]);
    assert!(chordpro::render_cho(&s).ends_with("[Am]Chiusa\n[G] [Em]\n[Am]vive\n"), "{}", chordpro::render_cho(&s));
}

#[test]
fn chorus_uses_soc_eoc() {
    let mut s = song(vec![Line { words: vec![w("uno", 0.5, 1.0)] }, Line { words: vec![w("due", 2.5, 3.0)] }], &[(0.0, 2.0, "C"), (2.0, 4.0, "G")]);
    s.sections = vec![
        Section { start: 0.0, end: 2.0, letter: 'A', label: SectionLabel::Verse },
        Section { start: 2.0, end: 4.0, letter: 'B', label: SectionLabel::Chorus },
    ];
    let cho = chordpro::render_cho(&s);
    assert!(cho.contains("{c: Verse}\n[C]uno\n{soc}\n[G]due\n{eoc}\n"), "{cho}");
}

#[test]
fn no_lyrics_four_bars_per_line() {
    let mut s = song(vec![], &[(0.0, 2.0, "Am"), (2.0, 4.0, "F"), (4.0, 6.0, "C"), (6.0, 8.0, "G"), (8.0, 10.0, "Am"), (10.0, 12.0, "F")]);
    s.bar_s = 2.0;
    assert!(chordpro::render_cho(&s).ends_with("\n[Am] [F] [C] [G]\n[Am] [F]\n"), "{}", chordpro::render_cho(&s));
}

#[test]
fn capo_header_only_when_set() {
    let mut s = song(two_lines(), &[(0.0, 4.0, "Am")]);
    assert!(!chordpro::render_cho(&s).contains("{capo"));
    s.capo = 2;
    assert!(chordpro::render_cho(&s).contains("{capo: 2}\n"));
    s.defines = vec![("Am".into(), Voicing { frets: [-1, 0, 2, 2, 1, 0], fingers: [0, 0, 2, 3, 1, 0], base_fret: 1, barre: None, difficulty: 3.0 })];
    assert!(chordpro::render_cho(&s).contains("{define: Am base-fret 1 frets x 0 2 2 1 0 fingers 0 0 2 3 1 0}\n"));
}

#[test]
fn txt_puts_chords_above_words() {
    let s = song(two_lines(), &[(0.0, 1.0, "Am"), (1.0, 2.3, "F"), (2.3, 4.0, "C")]);
    let txt = chordpro::render_txt(&s);
    let lines: Vec<&str> = txt.lines().collect();
    let i = lines.iter().position(|l| l.starts_with("Chiusa")).expect("lyric line");
    let chords = lines[i - 1];
    let col = lines[i].find("dentro").expect("dentro");
    assert_eq!(chords.get(0..2), Some("Am"));
    assert_eq!(chords.get(col..col + 1), Some("F"), "{txt}");
}

#[test]
fn no_chord_is_never_a_label() {
    let s = song(vec![Line { words: vec![w("uno", 0.0, 0.5), w("due", 1.2, 1.6)] }], &[(0.0, 1.0, ""), (1.0, 2.0, "C")]);
    let cho = chordpro::render_cho(&s);
    assert!(cho.ends_with("uno [C]due\n") && !cho.contains("[]"), "{cho}");
}

#[test]
fn line_just_after_a_section_start_gets_no_extra_chord_line() {
    // Am from the intro runs into the verse; the singer enters 0.3 s after the verse starts.
    let mut s = song(vec![Line { words: vec![w("Chiusa", 10.3, 10.8)] }], &[(0.0, 12.5, "Am"), (12.5, 15.0, "F")]);
    s.sections = vec![
        Section { start: 0.0, end: 10.0, letter: 'A', label: SectionLabel::Intro },
        Section { start: 10.0, end: 20.0, letter: 'B', label: SectionLabel::Verse },
    ];
    let cho = chordpro::render_cho(&s);
    assert!(cho.contains("{c: Intro}\n[Am]\n{c: Verse}\n[Am]Chiusa\n"), "{cho}");
}

#[test]
fn early_changes_still_group_by_bar() {
    // Detected bar starts run a few hundredths early: the fifth chord must open the next line.
    let times = [0.0, 2.48, 4.97, 7.46, 9.95, 12.44];
    let labels = ["C", "G", "Am", "F", "C", "G"];
    let chords: Vec<(f32, f32, &str)> =
        times.iter().zip(labels).enumerate().map(|(i, (t, l))| (*t, times.get(i + 1).copied().unwrap_or(14.9), l)).collect();
    let s = song(vec![], &chords);
    assert!(chordpro::render_cho(&s).ends_with("\n[C] [G] [Am] [F]\n[C] [G]\n"), "{}", chordpro::render_cho(&s));
}
