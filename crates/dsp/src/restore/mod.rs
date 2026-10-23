//! Restoration: the streaming STFT the spectral processors share, the noise print, the gain rules
//! and the restoration plugins (noise reduction, adaptive noise reduction, hum, hiss, clipping,
//! reverb). Algorithms follow the papers named in each module; none comes from a product.

mod gain;
mod print;
mod stft;

pub use gain::{e1, lsa_gain};
pub use print::{BANDS, FLOOR_DB, NoisePrint, band_lo_hz};
pub use rustfft::num_complex::Complex;
pub use stft::{FFT_CHOICES, FFT_SIZES, Stft};
