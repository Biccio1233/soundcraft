//! The spectral view: an STFT of the visible audio on a log-frequency axis, in our own colours.

use egui::Color32;

/// Lowest frequency shown.
pub const F_MIN: f32 = 30.0;
/// FFT size of the view.
const FFT_SIZE: usize = 2048;
/// Loudness mapped to the darkest and brightest colours.
const DB_FLOOR: f32 = -120.0;

/// Our colour map, quiet to loud: near black, dark violet, magenta, orange, pale yellow.
const STOPS: [(u8, u8, u8); 5] = [(8, 4, 20), (52, 12, 92), (168, 32, 128), (240, 120, 40), (255, 236, 170)];

/// The colour for a level in dB.
pub fn colour(db: f32) -> Color32 {
    let t = if db.is_finite() { ((db - DB_FLOOR) / -DB_FLOOR).clamp(0.0, 1.0) } else { 0.0 };
    let pos = t * (STOPS.len() - 1) as f32;
    let i = (pos.floor() as usize).min(STOPS.len() - 2);
    let f = pos - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * f).round() as u8;
    Color32::from_rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// A grid of levels: `columns` frames by `rows` log-spaced bands, row 0 the highest band.
pub struct Tile {
    pub columns: usize,
    pub rows: usize,
    data: Vec<f32>,
}

impl Tile {
    /// The level at a column and row (row 0 is the top, the highest frequency).
    #[cfg(test)]
    pub fn db(&self, col: usize, row: usize) -> f32 {
        row.checked_mul(self.columns).and_then(|i| i.checked_add(col)).and_then(|i| self.data.get(i)).copied().unwrap_or(-144.0)
    }

    pub fn image(&self) -> egui::ColorImage {
        egui::ColorImage::new([self.columns, self.rows], self.data.iter().map(|d| colour(*d)).collect())
    }
}

/// The spectral tile of `x`: one frame every `hop` samples, `columns` of them, `rows` bands from
/// `F_MIN` to Nyquist. `None` when the sizes are unusable.
pub fn tile(x: &[f32], sample_rate: u32, hop: usize, columns: usize, rows: usize) -> Option<Tile> {
    if columns == 0 || rows == 0 || x.is_empty() {
        return None;
    }
    let frames = soundcraft_dsp::stft::spectrogram(x, sample_rate, FFT_SIZE, hop.max(1), rows, F_MIN).ok()?;
    let mut data = vec![-144.0; columns * rows];
    for (col, frame) in frames.iter().take(columns).enumerate() {
        for (band, db) in frame.iter().enumerate() {
            let row = rows - 1 - band;
            if let Some(cell) = data.get_mut(row * columns + col) {
                *cell = *db;
            }
        }
    }
    Some(Tile { columns, rows, data })
}

/// Where `hz` sits on the view's axis, as a fraction from the bottom.
pub fn axis_fraction(hz: f32, sample_rate: u32) -> f32 {
    let nyquist = sample_rate as f32 / 2.0;
    ((hz / F_MIN).ln() / (nyquist / F_MIN).ln()).clamp(0.0, 1.0)
}

/// The FFT size the view uses (callers render this much audio beyond the visible range).
pub fn fft_size() -> usize {
    FFT_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_for_a_440_tone_peaks_in_the_right_row() {
        let x: Vec<f32> = (0..48_000).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()).collect();
        let rows = 200;
        let t = tile(&x, 48_000, 256, 64, rows).unwrap();
        assert_eq!((t.columns, t.rows), (64, rows));
        let col = 32;
        let row = (0..rows).max_by(|a, b| t.db(col, *a).total_cmp(&t.db(col, *b))).unwrap();
        // Row 0 is the top (highest frequency): rows count down from Nyquist.
        let band = rows - 1 - row;
        let lo = soundcraft_dsp::stft::band_edge_hz(band.saturating_sub(1), rows, F_MIN, 24_000.0);
        let hi = soundcraft_dsp::stft::band_edge_hz(band + 2, rows, F_MIN, 24_000.0);
        assert!((lo..=hi).contains(&440.0), "brightest row {row} spans {lo}..{hi} Hz");
    }

    #[test]
    fn colours_rise_from_dark_to_bright() {
        let (quiet, loud) = (colour(-120.0), colour(0.0));
        let lum = |c: egui::Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(lum(loud) > lum(colour(-60.0)) && lum(colour(-60.0)) > lum(quiet));
    }
}
