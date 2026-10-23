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

/// One tile per channel for a view: column `c` analyses the `fft_size()` samples that `render`
/// returns for position `start + c × hop` (planar, one `Vec` per channel). Only those samples are
/// ever rendered, however far apart the columns are.
pub fn tiles_from(
    mut render: impl FnMut(soundcraft_time::Samples, usize) -> Vec<Vec<f32>>,
    start: soundcraft_time::Samples,
    hop: usize,
    columns: usize,
    rows: usize,
    sample_rate: u32,
    channels: usize,
) -> Vec<Tile> {
    let mut tiles: Vec<Tile> = (0..channels).map(|_| Tile { columns, rows, data: vec![-144.0; columns * rows] }).collect();
    if columns == 0 || rows == 0 {
        return tiles;
    }
    let silence = vec![0.0f32; FFT_SIZE];
    for col in 0..columns {
        let at = start.saturating_add((col as i64).saturating_mul(hop as i64));
        let block = render(at, FFT_SIZE);
        for (ch, tile) in tiles.iter_mut().enumerate() {
            let x = block.get(ch).filter(|b| !b.is_empty()).unwrap_or(&silence);
            let Ok(frames) = soundcraft_dsp::stft::spectrogram(x, sample_rate, FFT_SIZE, FFT_SIZE, rows, F_MIN) else { continue };
            let Some(frame) = frames.first() else { continue };
            for (band, db) in frame.iter().enumerate() {
                let row = rows - 1 - band;
                if let Some(cell) = tile.data.get_mut(row * columns + col) {
                    *cell = *db;
                }
            }
        }
    }
    tiles
}

/// The spectral tile of `x`: one frame every `hop` samples, `columns` of them, `rows` bands from
/// `F_MIN` to Nyquist. `None` when the sizes are unusable.
#[cfg(test)]
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

/// The FFT size the view uses.
#[cfg(test)]
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
    fn columns_render_only_what_they_analyse() {
        // A long view (one column every 48 000 samples): each column asks for one FFT's worth of
        // audio, never the whole span.
        let mut asked = 0usize;
        let render = |start: i64, len: usize| {
            asked += len;
            vec![(0..len).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * (start as f32 + i as f32) / 48_000.0).sin()).collect::<Vec<f32>>()]
        };
        let tiles = tiles_from(render, 0, 48_000, 40, 120, 48_000, 1);
        assert_eq!(asked, 40 * fft_size());
        let t = &tiles[0];
        let row = (0..t.rows).max_by(|a, b| t.db(20, *a).total_cmp(&t.db(20, *b))).unwrap();
        let band = t.rows - 1 - row;
        let lo = soundcraft_dsp::stft::band_edge_hz(band.saturating_sub(1), t.rows, F_MIN, 24_000.0);
        let hi = soundcraft_dsp::stft::band_edge_hz(band + 2, t.rows, F_MIN, 24_000.0);
        assert!((lo..=hi).contains(&440.0), "{lo}..{hi}");
    }

    #[test]
    fn colours_rise_from_dark_to_bright() {
        let (quiet, loud) = (colour(-120.0), colour(0.0));
        let lum = |c: egui::Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(lum(loud) > lum(colour(-60.0)) && lum(colour(-60.0)) > lum(quiet));
    }
}
