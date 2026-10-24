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

/// Deterministic white noise in −1..1 (xorshift).
pub fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.max(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as f32 / u32::MAX as f32 * 2.0 - 1.0
        })
        .collect()
}

fn add_at(out: &mut [f32], at: usize, src: &[f32], gain: f32) {
    for (o, v) in out.iter_mut().skip(at).zip(src) {
        *o += v * gain;
    }
}

/// One drum hit: kick (pitch-dropping sine), snare (noise and 190 Hz) or hi-hat (differentiated
/// noise), faded out over its last 10 ms so the cut adds no click.
pub fn drum(kind: Drum, sr: u32, seed: u32) -> Vec<f32> {
    let mut hit = drum_raw(kind, sr, seed);
    let fade = ((0.01 * sr as f32) as usize).clamp(1, hit.len().max(1));
    let len = hit.len();
    for (i, v) in hit.iter_mut().enumerate().skip(len.saturating_sub(fade)) {
        *v *= (len - i) as f32 / fade as f32;
    }
    hit
}

fn drum_raw(kind: Drum, sr: u32, seed: u32) -> Vec<f32> {
    let sr_f = sr as f32;
    match kind {
        Drum::Kick => (0..(0.25 * sr_f) as usize)
            .map(|i| {
                let t = i as f32 / sr_f;
                let phase = std::f32::consts::TAU * (50.0 * t + 60.0 * 0.03 * (1.0 - (-t / 0.03).exp()));
                0.9 * phase.sin() * (-t / 0.12).exp()
            })
            .collect(),
        Drum::Snare => {
            let n = noise((0.15 * sr_f) as usize, seed);
            n.iter()
                .enumerate()
                .map(|(i, v)| {
                    let t = i as f32 / sr_f;
                    (0.35 * v + 0.3 * (std::f32::consts::TAU * 190.0 * t).sin()) * (-t / 0.05).exp()
                })
                .collect()
        }
        Drum::HiHat => {
            let n = noise((0.05 * sr_f) as usize + 1, seed);
            n.windows(2).enumerate().map(|(i, w)| 0.08 * (w[1] - w[0]) * (-(i as f32 / sr_f) / 0.012).exp()).collect()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drum {
    Kick,
    Snare,
    HiHat,
}

/// A drum pattern: kick on the first beat and the middle of each bar, snare on the others' even
/// beats, hi-hat on every eighth. Returns the audio and the beat times (s).
pub fn drums(bpm: f32, bars: usize, beats_per_bar: usize, sr: u32) -> (Vec<f32>, Vec<f32>) {
    let beat = 60.0 / bpm.max(1.0);
    let beats = bars * beats_per_bar.max(1);
    let len = ((beats as f32 * beat + 0.5) * sr as f32) as usize;
    let mut out = vec![0.0f32; len];
    let (kick, snare) = (drum(Drum::Kick, sr, 1), drum(Drum::Snare, sr, 2));
    let mut times = Vec::with_capacity(beats);
    for b in 0..beats {
        let t = b as f32 * beat;
        times.push(t);
        let at = (t * sr as f32) as usize;
        let pos = b % beats_per_bar.max(1);
        if pos == 0 || (beats_per_bar >= 4 && pos == beats_per_bar / 2) {
            add_at(&mut out, at, &kick, 1.0);
        } else if pos % 2 == 1 {
            add_at(&mut out, at, &snare, 1.0);
        }
        for half in 0..2 {
            let hat = drum(Drum::HiHat, sr, 7 + (2 * b + half) as u32);
            add_at(&mut out, ((t + half as f32 * beat / 2.0) * sr as f32) as usize, &hat, 1.0);
        }
    }
    (out, times)
}
