use soundcraft_wave_analysis::testsignal::{chord, note};
use soundcraft_wave_analysis::{ANALYSIS_RATE, chroma};

const SR: u32 = ANALYSIS_RATE;

fn mean(frames: &[[f32; 12]]) -> [f32; 12] {
    let mut m = [0.0f32; 12];
    for f in frames {
        for (a, b) in m.iter_mut().zip(f) {
            *a += b / frames.len().max(1) as f32;
        }
    }
    m
}

fn top(v: &[f32; 12], n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..12).collect();
    idx.sort_by(|a, b| v[*b].total_cmp(&v[*a]));
    let mut t: Vec<usize> = idx.into_iter().take(n).collect();
    t.sort_unstable();
    t
}

fn concentration(v: &[f32; 12]) -> f32 {
    let t: f32 = top(v, 3).iter().map(|i| v[*i]).sum();
    t / v.iter().sum::<f32>().max(1e-9)
}

#[test]
fn triad_peaks_on_its_notes() {
    let c = chroma::compute(&chord(&[60.0, 64.0, 67.0], 2.0, 440.0, SR), SR, 0.0);
    assert_eq!(top(&mean(&c.treble), 3), vec![0, 4, 7]);
    assert!((c.hop_s - 1024.0 / SR as f32).abs() < 1e-6);
}

#[test]
fn tuning_offset_is_honoured() {
    // A4 = 452 Hz is 46.6 cents sharp: untuned, every note sits between two bins.
    let x = chord(&[60.0, 64.0, 67.0], 2.0, 452.0, SR);
    let tuned = mean(&chroma::compute(&x, SR, 46.6).treble);
    let untuned = mean(&chroma::compute(&x, SR, 0.0).treble);
    assert_eq!(top(&tuned, 3), vec![0, 4, 7]);
    assert!(concentration(&tuned) > concentration(&untuned) + 0.1, "{} vs {}", concentration(&tuned), concentration(&untuned));
}

#[test]
fn bass_picks_the_lowest_note() {
    let mut x = chord(&[60.0, 64.0, 67.0], 2.0, 440.0, SR);
    for (a, b) in x.iter_mut().zip(note(40.0, 2.0, 440.0, SR)) {
        *a += 2.0 * b;
    }
    let b = mean(&chroma::compute(&x, SR, 0.0).bass);
    assert_eq!(top(&b, 1), vec![4]);
}

#[test]
fn silence_is_zero() {
    let c = chroma::compute(&vec![0.0; SR as usize], SR, 0.0);
    assert!(!c.treble.is_empty());
    assert!(c.treble.iter().chain(&c.bass).flatten().all(|v| *v == 0.0));
    assert!(c.energy.iter().all(|v| *v == 0.0));
    assert!(chroma::compute(&[], SR, 0.0).treble.is_empty());
}

#[test]
fn sync_averages_between_bounds() {
    let mut x = chord(&[60.0, 64.0, 67.0], 1.0, 440.0, SR);
    x.extend(chord(&[57.0, 60.0, 64.0], 1.0, 440.0, SR));
    let c = chroma::compute(&x, SR, 0.0);
    let s = chroma::sync(&c, &[0.0, 1.0, 2.0]);
    assert_eq!(s.len(), 2);
    assert_eq!(top(&s[0].0, 3), vec![0, 4, 7]);
    assert_eq!(top(&s[1].0, 3), vec![0, 4, 9]);
    assert!(s[0].2 > 0.0);
    assert_eq!(chroma::sync(&c, &[5.0, 6.0])[0].2, 0.0);
}
