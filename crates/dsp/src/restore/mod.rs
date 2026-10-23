//! Restoration: the streaming STFT the spectral processors share, the noise print, the gain rules
//! and the restoration plugins (noise reduction, adaptive noise reduction, hum, hiss, clipping,
//! reverb). Algorithms follow the papers named in each module; none comes from a product.

pub mod adaptive;
pub mod declip;
pub mod dehum;
pub mod dereverb;
mod gain;
pub mod hiss;
pub mod noise_reduction;
mod print;
mod stft;

pub use adaptive::{AdaptiveNoiseReduction, MinTracker};
pub use declip::Declip;
pub use dehum::{Dehum, detect_mains};
pub use dereverb::Dereverb;
pub use gain::{e1, lsa_gain};
pub use hiss::HissReduction;
pub use noise_reduction::NoiseReduction;
pub use print::{BANDS, FLOOR_DB, NoisePrint, band_lo_hz, bands_to_bins};
pub use rustfft::num_complex::Complex;
pub use stft::{FFT_CHOICES, FFT_SIZES, Stft};
