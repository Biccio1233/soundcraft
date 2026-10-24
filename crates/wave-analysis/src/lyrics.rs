//! Lyrics with times: supplied text aligned word by word to the words a transcription heard.

/// Seconds given to a supplied word with no heard neighbour to borrow time from.
const WORD_S: f32 = 0.3;
const MATCH_AT: f32 = 0.5;
/// Larger alignments fall back to spreading the words over the heard span.
const MAX_CELLS: usize = 16_000_000;
const LINE_GAP_S: f32 = 0.8;
const LINE_WORDS: usize = 12;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Word {
    pub text: String,
    pub start: f32,
    pub end: f32,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Line {
    pub words: Vec<Word>,
}

/// Lowercase, accents folded, letters and digits only.
fn normalise(s: &str) -> Vec<char> {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        })
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// 1 − Levenshtein distance / the longer length.
fn similarity(a: &[char], b: &[char]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut row = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            row[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(row[j] + 1);
        }
        prev = row;
    }
    1.0 - prev[b.len()] as f32 / a.len().max(b.len()) as f32
}

/// For each supplied word, the heard word it matches (maximum total similarity, in order).
fn matches(supplied: &[Vec<char>], heard: &[Vec<char>]) -> Vec<Option<usize>> {
    let (n, m) = (supplied.len(), heard.len());
    let mut out = vec![None; n];
    if n == 0 || m == 0 || (n + 1).saturating_mul(m + 1) > MAX_CELLS {
        return out;
    }
    // 0 = from the diagonal (a match), 1 = skip a supplied word, 2 = skip a heard word.
    let mut back = vec![0u8; (n + 1) * (m + 1)];
    let mut prev = vec![0.0f32; m + 1];
    back.iter_mut().take(m + 1).skip(1).for_each(|b| *b = 2);
    for i in 1..=n {
        let mut row = vec![0.0f32; m + 1];
        back[i * (m + 1)] = 1;
        for j in 1..=m {
            let sim = similarity(&supplied[i - 1], &heard[j - 1]);
            let diag = if sim >= MATCH_AT { prev[j - 1] + sim } else { f32::NEG_INFINITY };
            let (up, left) = (prev[j], row[j - 1]);
            let (best, dir) = if diag >= up && diag >= left {
                (diag, 0)
            } else if up >= left {
                (up, 1)
            } else {
                (left, 2)
            };
            row[j] = best;
            back[i * (m + 1) + j] = dir;
        }
        prev = row;
    }
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        match back[i * (m + 1) + j] {
            0 => {
                out[i - 1] = Some(j - 1);
                i -= 1;
                j -= 1;
            }
            1 => i -= 1,
            _ => j -= 1,
        }
    }
    out
}

/// The supplied text's lines and words (punctuation kept for display), timed from `heard`: matched
/// words copy the heard times; the others share the time between their matched neighbours by
/// length, or take 0.3 s each before the first and after the last match. Times never go back.
pub fn align(supplied: &str, heard: &[Word]) -> Vec<Line> {
    let lines: Vec<Vec<&str>> = supplied.lines().map(|l| l.split_whitespace().collect::<Vec<_>>()).filter(|l| !l.is_empty()).collect();
    let flat: Vec<&str> = lines.iter().flatten().copied().collect();
    if flat.is_empty() {
        return Vec::new();
    }
    let norm_s: Vec<Vec<char>> = flat.iter().map(|w| normalise(w)).collect();
    let norm_h: Vec<Vec<char>> = heard.iter().map(|w| normalise(&w.text)).collect();
    let m = matches(&norm_s, &norm_h);
    let mut times: Vec<(f32, f32)> = vec![(0.0, 0.0); flat.len()];
    for (i, h) in m.iter().enumerate() {
        if let Some(w) = h.and_then(|h| heard.get(h)) {
            times[i] = (w.start.max(0.0), w.end.max(w.start).max(0.0));
        }
    }
    let matched: Vec<usize> = (0..flat.len()).filter(|i| m[*i].is_some()).collect();
    let weight = |i: usize| norm_s[i].len().max(1) as f32 + 1.0;
    // Before the first match: 0.3 s per word, ending where it starts (from the first heard word without one).
    let first_start = matched.first().map_or_else(|| heard.first().map_or(0.0, |w| w.start.max(0.0)), |f| times[*f].0);
    let lead = matched.first().copied().unwrap_or(flat.len());
    let mut t = if matched.is_empty() { first_start } else { (first_start - WORD_S * lead as f32).max(0.0) };
    let step = if matched.is_empty() { WORD_S } else { ((first_start - t) / lead.max(1) as f32).min(WORD_S) };
    for slot in times.iter_mut().take(lead) {
        *slot = (t, t + step);
        t += step;
    }
    // Between matches: share the gap by word length.
    for pair in matched.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b <= a + 1 {
            continue;
        }
        let (from, to) = (times[a].1, times[b].0.max(times[a].1));
        let total: f32 = (a + 1..b).map(weight).sum();
        let mut t = from;
        for (i, slot) in times.iter_mut().enumerate().take(b).skip(a + 1) {
            let d = (to - from) * weight(i) / total;
            *slot = (t, t + d);
            t += d;
        }
    }
    // After the last match: 0.3 s per word.
    if let Some(&last) = matched.last() {
        let mut t = times[last].1;
        for slot in times.iter_mut().skip(last + 1) {
            *slot = (t, t + WORD_S);
            t += WORD_S;
        }
    }
    // Never go back.
    let mut floor = 0.0f32;
    for (s, e) in times.iter_mut() {
        *s = s.max(floor);
        *e = e.max(*s);
        floor = *s;
    }
    let mut out = Vec::with_capacity(lines.len());
    let mut k = 0;
    for l in &lines {
        let words = l
            .iter()
            .map(|text| {
                let (start, end) = times.get(k).copied().unwrap_or((floor, floor));
                k += 1;
                Word { text: (*text).to_string(), start, end }
            })
            .collect();
        out.push(Line { words });
    }
    out
}

/// Lines from heard words alone: a new line after a pause over 0.8 s or every 12 words.
pub fn lines_from_words(heard: &[Word]) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut current = Line::default();
    for w in heard {
        let gap = current.words.last().is_some_and(|p| w.start - p.end > LINE_GAP_S);
        if gap || current.words.len() >= LINE_WORDS {
            out.push(std::mem::take(&mut current));
        }
        current.words.push(w.clone());
    }
    if !current.words.is_empty() {
        out.push(current);
    }
    out
}
