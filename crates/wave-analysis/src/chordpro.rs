//! Chords over lyrics, written as ChordPro (`.cho`) or plain text with the chords above the words.

use crate::lyrics::Line;
use crate::sections::{Section, SectionLabel};
use crate::voicing::{Voicing, define_line};

/// A chord change this close before a word still lands on it.
const TOL_S: f32 = 0.15;
const BARS_PER_LINE: f32 = 4.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    pub title: String,
    pub key: Option<String>,
    pub bpm: Option<f32>,
    pub capo: u8,
    pub lines: Vec<Line>,
    /// Chord spans (start, end, label); an empty label is "no chord" and is never written.
    pub chords: Vec<(f32, f32, String)>,
    pub sections: Vec<Section>,
    /// Diagrams written as `{define}` lines.
    pub defines: Vec<(String, Voicing)>,
    /// Seconds per bar, for chord-only lines (four bars each).
    pub bar_s: f32,
}

enum Item {
    Start(SectionLabel),
    EndChorus,
    /// Words, each with the chord written in front of it.
    Lyric(Vec<(Option<String>, String)>),
    Chords(Vec<String>),
}

fn title_case(label: SectionLabel) -> String {
    let n = label.name();
    let mut c = n.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// Chord changes: the start of each span whose label differs from the span before it.
fn changes(song: &Song) -> Vec<(f32, String)> {
    let mut out: Vec<(f32, String)> = Vec::new();
    let mut prev: Option<&str> = None;
    for (s, _, l) in &song.chords {
        if prev != Some(l.as_str()) && !l.is_empty() {
            out.push((*s, l.clone()));
        }
        prev = Some(l.as_str());
    }
    out
}

fn active(song: &Song, t: f32) -> Option<String> {
    song.chords.iter().find(|(s, e, _)| *s <= t + 1e-4 && t < *e).map(|c| c.2.clone()).filter(|l| !l.is_empty())
}

/// Chord-only lines for the changes in `[from, to)`, four bars per line, restarting at section
/// starts. `carry` writes the chord already sounding at the start of a line that has none.
fn chord_lines(song: &Song, from: f32, to: f32, mut carry: bool, out: &mut Vec<(f32, Item)>) {
    if to <= from {
        return;
    }
    let all = changes(song);
    let window = (BARS_PER_LINE * song.bar_s).max(0.5);
    let mut starts: Vec<f32> = vec![from];
    starts.extend(song.sections.iter().map(|s| s.start).filter(|s| *s > from && *s < to));
    starts.push(to);
    for span in starts.windows(2) {
        let mut ws = span[0];
        while ws < span[1] - 1e-4 {
            let we = (ws + window).min(span[1]);
            let mut tokens: Vec<(f32, String)> = all.iter().filter(|(t, _)| *t >= ws - 1e-4 && *t < we - 1e-4).cloned().collect();
            if carry
                && tokens.first().is_none_or(|(t, _)| *t > ws + TOL_S)
                && let Some(a) = active(song, ws)
            {
                tokens.insert(0, (ws, a));
            }
            if !tokens.is_empty() {
                out.push((tokens[0].0, Item::Chords(tokens.into_iter().map(|t| t.1).collect())));
            }
            carry = true;
            ws = we;
        }
    }
}

fn section_at(song: &Song, t: f32) -> Option<usize> {
    song.sections.iter().position(|s| s.start <= t + TOL_S && t + TOL_S < s.end)
}

fn items(song: &Song) -> Vec<Item> {
    let mut lines: Vec<&Line> = song.lines.iter().filter(|l| !l.words.is_empty()).collect();
    lines.sort_by(|a, b| a.words[0].start.total_cmp(&b.words[0].start));
    let all = changes(song);
    let end = song.chords.iter().map(|c| c.1).fold(0.0f32, f32::max);
    let mut timed: Vec<(f32, Item)> = Vec::new();
    if lines.is_empty() {
        let start = song.chords.first().map_or(0.0, |c| c.0);
        chord_lines(song, start, end, true, &mut timed);
    } else {
        let mut prev_last: Option<f32> = None;
        for line in &lines {
            let first = line.words[0].start;
            let last = line.words.last().map_or(first, |w| w.start);
            // The changes since the previous line, before this one starts.
            let gap_from = prev_last.map_or(f32::NEG_INFINITY, |p| p + TOL_S);
            let mut gap_to = first - TOL_S + 1e-3;
            // The last change before the line is written at the line's start instead, unless a
            // section boundary lies between them.
            if let Some((t, label)) = all.iter().rev().find(|(t, _)| *t > gap_from && *t < gap_to)
                && active(song, first).as_deref() == Some(label.as_str())
                && section_at(song, *t) == section_at(song, first)
            {
                gap_to = *t;
            }
            if all.iter().any(|(t, _)| *t > gap_from && *t < gap_to) {
                let from = all.iter().map(|c| c.0).filter(|t| *t > gap_from).fold(f32::INFINITY, f32::min);
                chord_lines(song, from, gap_to, prev_last.is_none(), &mut timed);
            }
            let mut tokens: Vec<(Option<String>, String)> = line.words.iter().map(|w| (None, w.text.clone())).collect();
            if let Some(first_tok) = tokens.first_mut() {
                first_tok.0 = active(song, first);
            }
            for (t, label) in all.iter().filter(|(t, _)| *t > first + TOL_S && *t <= last + TOL_S) {
                if let Some(i) = line.words.iter().position(|w| w.start >= t - TOL_S)
                    && let Some(tok) = tokens.get_mut(i)
                {
                    tok.0 = Some(label.clone());
                }
            }
            timed.push((first, Item::Lyric(tokens)));
            prev_last = Some(last);
        }
        if let Some(p) = prev_last
            && all.iter().any(|(t, _)| *t > p + TOL_S)
        {
            let from = all.iter().map(|c| c.0).filter(|t| *t > p + TOL_S).fold(f32::INFINITY, f32::min);
            chord_lines(song, from, end, false, &mut timed);
        }
    }
    // Section markers before the first item at or after each section's start.
    let mut sections: Vec<&Section> = song.sections.iter().collect();
    sections.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out = Vec::with_capacity(timed.len() + 2 * sections.len());
    let mut next = 0;
    let mut in_chorus = false;
    for (t, item) in timed {
        let mut entered = None;
        while next < sections.len() && sections[next].start <= t + TOL_S {
            entered = Some(sections[next].label);
            next += 1;
        }
        if let Some(label) = entered {
            if in_chorus {
                out.push(Item::EndChorus);
            }
            in_chorus = label == SectionLabel::Chorus;
            out.push(Item::Start(label));
        }
        out.push(item);
    }
    if in_chorus {
        out.push(Item::EndChorus);
    }
    out
}

fn tempo_text(bpm: f32) -> String {
    if (bpm - bpm.round()).abs() < 0.05 { format!("{}", bpm.round() as i64) } else { format!("{bpm:.1}") }
}

/// The song as ChordPro: header (title, key, tempo, capo, chord definitions), section markers
/// (`{c: Verse}`, the chorus in `{soc}` … `{eoc}`), chords in brackets before their words, and
/// chord-only lines for the music between lyric lines.
pub fn render_cho(song: &Song) -> String {
    let mut out = String::new();
    if !song.title.is_empty() {
        out.push_str(&format!("{{title: {}}}\n", song.title));
    }
    if let Some(k) = &song.key {
        out.push_str(&format!("{{key: {k}}}\n"));
    }
    if let Some(b) = song.bpm.filter(|b| b.is_finite() && *b > 0.0) {
        out.push_str(&format!("{{tempo: {}}}\n", tempo_text(b)));
    }
    if song.capo > 0 {
        out.push_str(&format!("{{capo: {}}}\n", song.capo));
    }
    for (label, v) in &song.defines {
        out.push_str(&define_line(label, v));
        out.push('\n');
    }
    out.push('\n');
    for item in items(song) {
        match item {
            Item::Start(SectionLabel::Chorus) => out.push_str("{soc}\n"),
            Item::Start(label) => out.push_str(&format!("{{c: {}}}\n", title_case(label))),
            Item::EndChorus => out.push_str("{eoc}\n"),
            Item::Lyric(tokens) => {
                let words: Vec<String> = tokens.iter().map(|(c, w)| c.as_ref().map_or_else(|| w.clone(), |c| format!("[{c}]{w}"))).collect();
                out.push_str(&words.join(" "));
                out.push('\n');
            }
            Item::Chords(labels) => {
                let tokens: Vec<String> = labels.iter().map(|l| format!("[{l}]")).collect();
                out.push_str(&tokens.join(" "));
                out.push('\n');
            }
        }
    }
    out
}

/// The song as plain text: each chord on the line above, at the column of its word.
pub fn render_txt(song: &Song) -> String {
    let mut out = String::new();
    if !song.title.is_empty() {
        out.push_str(&song.title);
        out.push('\n');
    }
    let mut facts = Vec::new();
    if let Some(k) = &song.key {
        facts.push(format!("Key: {k}"));
    }
    if let Some(b) = song.bpm.filter(|b| b.is_finite() && *b > 0.0) {
        facts.push(format!("Tempo: {}", tempo_text(b)));
    }
    if song.capo > 0 {
        facts.push(format!("Capo: {}", song.capo));
    }
    if !facts.is_empty() {
        out.push_str(&facts.join("   "));
        out.push('\n');
    }
    out.push('\n');
    for item in items(song) {
        match item {
            Item::Start(label) => out.push_str(&format!("{}:\n", title_case(label))),
            Item::EndChorus => {}
            Item::Lyric(tokens) => {
                let (mut chords, mut words) = (String::new(), String::new());
                for (chord, word) in &tokens {
                    if !words.is_empty() {
                        words.push(' ');
                    }
                    let col = words.chars().count();
                    if let Some(c) = chord {
                        // At the word's column, or one space after a chord that runs past it.
                        let have = chords.chars().count();
                        let pad = if have == 0 || have < col { col - have } else { 1 };
                        chords.push_str(&" ".repeat(pad));
                        chords.push_str(c);
                    }
                    words.push_str(word);
                }
                if !chords.is_empty() {
                    out.push_str(chords.trim_end());
                    out.push('\n');
                }
                out.push_str(&words);
                out.push('\n');
            }
            Item::Chords(labels) => {
                out.push_str(&labels.join("  "));
                out.push('\n');
            }
        }
    }
    out
}
