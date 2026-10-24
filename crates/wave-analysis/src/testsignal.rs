//! Synthetic signals with a known truth, for the analysis tests and the WaveCraft acceptance.

/// Frequency of MIDI note `midi` with A4 at `a4` Hz.
pub fn midi_hz(midi: f32, a4: f32) -> f32 {
    a4 * 2f32.powf((midi - 69.0) / 12.0)
}

/// A harmonic note (partials 1–6 at 1/k, 5 ms attack, exponential decay), peak about 0.25.
pub fn note(midi: f32, secs: f32, a4: f32, sr: u32) -> Vec<f32> {
    let f0 = midi_hz(midi, a4);
    let n = (secs.max(0.0) * sr as f32) as usize;
    let nyquist = sr as f32 * 0.45;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr as f32;
            let env = (t / 0.005).min(1.0) * (-t / 1.5).exp();
            let mut v = 0.0;
            for k in 1..=6 {
                let f = f0 * k as f32;
                if f < nyquist {
                    v += (std::f32::consts::TAU * f * t).sin() / k as f32;
                }
            }
            0.1 * env * v
        })
        .collect()
}

/// The notes `notes` (MIDI) played together.
pub fn chord(notes: &[f32], secs: f32, a4: f32, sr: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (secs.max(0.0) * sr as f32) as usize];
    for m in notes {
        for (o, v) in out.iter_mut().zip(note(*m, secs, a4, sr)) {
            *o += v;
        }
    }
    out
}
