//! Quantization to an integer bit depth with dither and noise shaping, for exports.

/// How quantization error is randomised and shaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DitherProfile {
    /// Plain rounding.
    None,
    /// Triangular (TPDF) dither of ±1 LSB, white.
    Tpdf,
    /// TPDF with first-order error feedback: the noise leans to the highs.
    ShapedLight,
    /// TPDF with third-order error feedback: much of the noise moves above ~5 kHz.
    ShapedStrong,
}

impl DitherProfile {
    /// From its snake_case name (`none`, `tpdf`, `shaped_light`, `shaped_strong`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "tpdf" => Some(Self::Tpdf),
            "shaped_light" => Some(Self::ShapedLight),
            "shaped_strong" => Some(Self::ShapedStrong),
            _ => None,
        }
    }

    /// Error-feedback taps (noise transfer function 1 − Σ hₖ z⁻ᵏ).
    fn taps(self) -> [f64; 3] {
        match self {
            Self::None | Self::Tpdf => [0.0; 3],
            Self::ShapedLight => [1.0, 0.0, 0.0],
            Self::ShapedStrong => [1.623, -0.982, 0.109],
        }
    }
}

/// Quantize `ch` in place to `bits` (8..=24): every output becomes `k / 2^(bits-1)` with `k` in
/// `[-2^(bits-1), 2^(bits-1) - 1]`, so an encoder at that depth stores it exactly. Other depths
/// (32-bit int and float) only clamp to [-1, 1]. NaN becomes 0. Samples already on the grid pass
/// through unchanged (they carry no error to dither). The noise is a deterministic
/// sequence of `seed`, so the same input always gives the same output.
pub fn quantize(ch: &mut [f32], bits: u32, profile: DitherProfile, seed: u64) {
    if !(8..=24).contains(&bits) {
        for x in ch.iter_mut() {
            *x = if x.is_nan() { 0.0 } else { x.clamp(-1.0, 1.0) };
        }
        return;
    }
    let scale = (1u64 << (bits - 1)) as f64;
    let (lo, hi) = (-scale, scale - 1.0);
    let taps = profile.taps();
    let mut rng = if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed };
    let mut uniform = || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut err = [0.0f64; 3];
    for x in ch.iter_mut() {
        let v = if x.is_nan() { 0.0 } else { f64::from(*x).clamp(-1.0, 1.0) } * scale;
        let target = v - (taps[0] * err[0] + taps[1] * err[1] + taps[2] * err[2]);
        if target.fract() == 0.0 {
            // Already on the grid: no quantization error, so nothing to dither or shape.
            err = [0.0, err[0], err[1]];
            *x = (target.clamp(lo, hi) / scale) as f32;
            continue;
        }
        let noise = if profile == DitherProfile::None { 0.0 } else { uniform() - uniform() };
        let q = (target + noise).round().clamp(lo, hi);
        // Keep the feedback bounded when the output clips.
        let e = (q - target).clamp(-4.0, 4.0);
        err = [e, err[0], err[1]];
        *x = (q / scale) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::{bin_hz, magnitude_db};

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 24) as f32 * 1.8 - 0.9
            })
            .collect()
    }

    fn sine(n: usize, hz: f32, db: f32) -> Vec<f32> {
        let a = 10f32.powf(db / 20.0);
        (0..n).map(|i| a * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin()).collect()
    }

    /// Mean error power per bin above 10 kHz divided by the mean below 4 kHz.
    fn hf_over_lf(x: &[f32], profile: DitherProfile) -> f32 {
        let mut y = x.to_vec();
        quantize(&mut y, 16, profile, 7);
        let err: Vec<f32> = y.iter().zip(x).map(|(a, b)| a - b).collect();
        let m = magnitude_db(&err, 8192);
        let band = |lo: f32, hi: f32| {
            let v: Vec<f32> =
                m.iter().enumerate().filter(|(k, _)| (lo..hi).contains(&bin_hz(*k, 8192, 48_000.0))).map(|(_, d)| 10f32.powf(d / 10.0)).collect();
            v.iter().sum::<f32>() / v.len().max(1) as f32
        };
        band(10_000.0, 24_000.0) / band(100.0, 4_000.0)
    }

    #[test]
    fn quantize_lands_on_the_grid_16_and_24() {
        for profile in [DitherProfile::None, DitherProfile::Tpdf, DitherProfile::ShapedLight, DitherProfile::ShapedStrong] {
            for (bits, scale) in [(16, 32_768.0f32), (24, 8_388_608.0)] {
                let mut y = noise(4096, 3);
                quantize(&mut y, bits, profile, 1);
                assert!(y.iter().all(|v| (v * scale).fract() == 0.0), "{profile:?} {bits}");
                assert!(y.iter().all(|v| (-1.0..1.0).contains(v)), "{profile:?} {bits}");
            }
        }
    }

    #[test]
    fn tpdf_is_deterministic_per_seed() {
        let x = noise(2048, 5);
        let (mut a, mut b, mut c) = (x.clone(), x.clone(), x.clone());
        quantize(&mut a, 16, DitherProfile::Tpdf, 42);
        quantize(&mut b, 16, DitherProfile::Tpdf, 42);
        quantize(&mut c, 16, DitherProfile::Tpdf, 43);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn shaped_strong_pushes_error_up() {
        let x = sine(8192, 1_000.0, -60.0);
        assert!(hf_over_lf(&x, DitherProfile::Tpdf) < 2.0);
        assert!(hf_over_lf(&x, DitherProfile::ShapedStrong) > 4.0);
        assert!(hf_over_lf(&x, DitherProfile::ShapedLight) > 2.0);
    }

    #[test]
    fn none_profile_keeps_floats() {
        let x = noise(512, 9);
        let mut y = x.clone();
        quantize(&mut y, 32, DitherProfile::None, 0);
        assert_eq!(x, y);
        let mut z = vec![1.5, -2.0, f32::NAN];
        quantize(&mut z, 32, DitherProfile::Tpdf, 0);
        assert_eq!(z, vec![1.0, -1.0, 0.0]);
    }

    #[test]
    fn on_grid_input_passes_through() {
        // Samples already representable at the target depth carry no error to dither: saving an
        // untouched 16-bit file must give it back bit for bit.
        let x: Vec<f32> = noise(4096, 11).iter().map(|v| (v * 32_768.0).round() / 32_768.0).collect();
        for profile in [DitherProfile::None, DitherProfile::Tpdf, DitherProfile::ShapedLight, DitherProfile::ShapedStrong] {
            let mut y = x.clone();
            quantize(&mut y, 16, profile, 3);
            assert_eq!(x, y, "{profile:?}");
        }
    }

    #[test]
    fn profiles_parse_from_their_names() {
        assert_eq!(DitherProfile::parse("none"), Some(DitherProfile::None));
        assert_eq!(DitherProfile::parse("tpdf"), Some(DitherProfile::Tpdf));
        assert_eq!(DitherProfile::parse("shaped_light"), Some(DitherProfile::ShapedLight));
        assert_eq!(DitherProfile::parse("shaped_strong"), Some(DitherProfile::ShapedStrong));
        assert_eq!(DitherProfile::parse("loud"), None);
    }
}
