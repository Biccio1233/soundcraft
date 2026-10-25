use soundcraft_wave_analysis::lyrics::Word;
use soundcraft_wave_analysis::voiced;

const SR: u32 = 16_000;

/// `secs` of bleed (a quiet 97 Hz hum at -54 dBFS) with 300 Hz "voice" bursts at -13 dBFS.
fn take(secs: f32, bursts: &[(f32, f32)]) -> Vec<f32> {
    (0..(secs * SR as f32) as usize)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let bleed = 0.0028 * (std::f32::consts::TAU * 97.0 * t).sin();
            let voice = if bursts.iter().any(|(a, b)| t >= *a && t < *b) { 0.3 * (std::f32::consts::TAU * 300.0 * t).sin() } else { 0.0 };
            bleed + voice
        })
        .collect()
}

fn secs(r: &std::ops::Range<usize>) -> (f32, f32) {
    (r.start as f32 / SR as f32, r.end as f32 / SR as f32)
}

#[test]
fn regions_cover_the_voice_and_skip_the_bleed() {
    let x = take(60.0, &[(10.0, 12.0), (12.5, 13.0), (40.0, 41.0)]);
    let r: Vec<(f32, f32)> = voiced::regions(&x, SR).iter().map(secs).collect();
    assert_eq!(r.len(), 2, "{r:?}");
    let near = |a: f32, b: f32| (a - b).abs() <= 0.35;
    assert!(near(r[0].0, 10.0) && near(r[0].1, 13.0), "{r:?}");
    assert!(near(r[1].0, 40.0) && near(r[1].1, 41.0), "{r:?}");
    assert!(r[0].0 <= 10.0 && r[0].1 >= 13.0 && r[1].0 <= 40.0 && r[1].1 >= 41.0, "regions cut the voice: {r:?}");
}

#[test]
fn regions_of_silence_bleed_and_constant_sound() {
    assert!(voiced::regions(&[], SR).is_empty());
    assert!(voiced::regions(&take(20.0, &[]), SR).is_empty(), "bleed alone is not a voice");
    let loud = take(5.0, &[(0.0, 5.0)]);
    let r = voiced::regions(&loud, SR);
    assert_eq!(r.len(), 1);
    assert!(r[0].start == 0 && r[0].end == loud.len(), "{r:?}");
    assert!(voiced::regions(&[f32::NAN; 1000], SR).is_empty());
}

#[test]
fn word_ends_follow_the_voice() {
    // A short word, a held word right after it, a word alone, and two words sung legato.
    let x = take(10.0, &[(1.0, 1.4), (1.5, 2.5), (5.0, 5.3), (7.0, 8.0)]);
    let w = |text: &str, start: f32| Word { text: text.into(), start, end: start };
    let mut words = vec![w("a", 1.0), w("held", 1.5), w("alone", 5.0), w("le", 7.0), w("gato", 7.5)];
    voiced::settle_word_ends(&mut words, &x, SR);
    let ends: Vec<f32> = words.iter().map(|w| w.end).collect();
    let want = [1.4, 2.5, 5.3, 7.5, 8.0];
    for (i, (e, w)) in ends.iter().zip(want).enumerate() {
        assert!((e - w).abs() <= 0.08, "word {i}: end {e}, want {w} ({ends:?})");
    }
    assert!(words.windows(2).all(|p| p[0].end <= p[1].start + 1e-6), "words overlap: {ends:?}");
}
