//! WaveCraft's music analysis, in pure Rust: tuning, tempo, key, chords, sections, guitar
//! voicings, lyrics alignment and ChordPro. Audio is mixed to mono and resampled to
//! [`ANALYSIS_RATE`] once ([`analysis_signal`]); every analysis then runs at that rate.

pub mod chroma;
pub mod cqt;
pub mod key;
pub mod resample;
pub mod spectrum;
pub mod tempo;
pub mod testsignal;
pub mod tuning;

pub use resample::resample;

/// The rate every analysis runs at: enough for chords and onsets, a quarter of the work of 88.2 kHz.
pub const ANALYSIS_RATE: u32 = 22_050;

/// The mono mean of `channels` (non-finite samples count as silence), resampled to [`ANALYSIS_RATE`].
pub fn analysis_signal(channels: &[Vec<f32>], sample_rate: u32) -> Vec<f32> {
    let len = channels.iter().map(Vec::len).max().unwrap_or(0);
    if channels.is_empty() || len == 0 {
        return Vec::new();
    }
    let scale = 1.0 / channels.len() as f32;
    let mut mono = vec![0.0f32; len];
    for ch in channels {
        for (m, v) in mono.iter_mut().zip(ch) {
            if v.is_finite() {
                *m += v * scale;
            }
        }
    }
    resample(&mono, sample_rate, ANALYSIS_RATE)
}
