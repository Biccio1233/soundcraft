//! Tempo and pitch with Signalsmith Stretch, offline over the whole buffer.

use signalsmith_stretch::Stretch;

/// `channels` played `tempo` times as fast and moved by `semitones`: `round(len / tempo)` samples.
pub fn stretch(channels: &[Vec<f32>], sample_rate: u32, tempo: f32, semitones: f32) -> Result<Vec<Vec<f32>>, String> {
    if !(tempo.is_finite() && (0.25..=4.0).contains(&tempo)) {
        return Err(format!("tempo factor {tempo} is outside 0.25–4"));
    }
    if !(semitones.is_finite() && semitones.abs() <= 24.0) {
        return Err(format!("{semitones} semitones is outside ±24"));
    }
    let Some(first) = channels.first() else { return Ok(Vec::new()) };
    let n = channels.len();
    let len = first.len();
    if len == 0 || sample_rate == 0 {
        return Ok(vec![Vec::new(); n]);
    }
    let out_len = ((len as f64 / f64::from(tempo)).round() as usize).max(1);
    let mut st = Stretch::preset_default(n as u32, sample_rate);
    st.set_transpose_factor_semitones(semitones, None);
    // Silence around the audio covers the stretcher's latency; the output is trimmed back.
    let pad = st.input_latency() + st.output_latency();
    let padded = len + 2 * pad;
    let mut input = vec![0.0f32; padded * n];
    for (c, ch) in channels.iter().enumerate() {
        for (i, v) in ch.iter().enumerate().take(len) {
            if let Some(slot) = input.get_mut((pad + i) * n + c) {
                *slot = if v.is_finite() { *v } else { 0.0 };
            }
        }
    }
    let padded_out = ((padded as f64 / f64::from(tempo)).round() as usize).max(out_len);
    let mut output = vec![0.0f32; padded_out * n];
    if !st.exact(&input, &mut output) {
        return Err("the audio is too short to stretch".into());
    }
    let skip = (pad as f64 / f64::from(tempo)).round() as usize;
    Ok((0..n).map(|c| (0..out_len).map(|i| output.get((skip + i) * n + c).copied().unwrap_or(0.0)).collect()).collect())
}
