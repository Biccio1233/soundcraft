//! Suppression gain rules.

/// Exponential integral E1(x) for x > 0 (+∞ at 0 and below): the power series below 1,
/// Abramowitz & Stegun 5.1.56 (error < 5e-5 relative) from 1 up.
pub fn e1(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return f32::INFINITY;
    }
    let x = f64::from(x);
    let v = if x < 1.0 {
        const EULER: f64 = 0.577_215_664_901_532_9;
        let (mut term, mut sum) = (1.0f64, 0.0f64);
        for k in 1..=30 {
            term *= -x / k as f64;
            let add = -term / k as f64;
            sum += add;
            if add.abs() < 1e-12 {
                break;
            }
        }
        -EULER - x.ln() + sum
    } else {
        let (a1, a2, b1, b2) = (2.334_733, 0.250_621, 3.330_657, 1.681_534);
        (x * x + a1 * x + a2) / (x * x + b1 * x + b2) * (-x).exp() / x
    };
    v as f32
}

/// Ephraim–Malah (1985) minimum mean-square error log-spectral amplitude gain for a-priori SNR
/// `xi` and a-posteriori SNR `gamma`: ξ/(1+ξ)·exp(½·E1(v)), v = ξγ/(1+ξ). Always in [0, 1].
pub fn lsa_gain(xi: f32, gamma: f32) -> f32 {
    let xi = if xi.is_nan() { 0.0 } else { xi.max(0.0) };
    let gamma = if gamma.is_nan() { 0.0 } else { gamma.max(0.0) };
    if xi == 0.0 {
        return 0.0;
    }
    if xi.is_infinite() {
        return 1.0;
    }
    let ratio = xi / (1.0 + xi);
    let v = (ratio * gamma).max(1e-10);
    let g = ratio * (0.5 * e1(v)).exp();
    if g.is_finite() { g.clamp(0.0, 1.0) } else { 1.0 }
}

/// One frame of decision-directed (Ephraim–Malah) suppression: `spec` holds bins `0..=n/2`,
/// `lambda` the noise power per bin, `gain`/`gamma` the previous frame's gain and a-posteriori SNR
/// (updated in place). Gains are clamped to `[floor, 1]`, and the a-priori SNR never goes below
/// floor² (so noise alone settles on the floor instead of warbling above it); `noise_only` keeps
/// what is removed.
pub(crate) fn dd_frame(
    spec: &mut [rustfft::num_complex::Complex<f32>],
    lambda: &[f32],
    gain: &mut [f32],
    gamma: &mut [f32],
    alpha: f32,
    floor: f32,
    noise_only: bool,
) {
    let xi_min = (floor * floor).max(1e-8);
    for (((y, l), g), gm) in spec.iter_mut().zip(lambda).zip(gain.iter_mut()).zip(gamma.iter_mut()) {
        if l.is_nan() || *l <= 1e-30 {
            *g = 1.0;
            *gm = 1.0;
            if noise_only {
                *y = rustfft::num_complex::Complex::new(0.0, 0.0);
            }
            continue;
        }
        let post = y.norm_sqr() / l;
        let post = if post.is_finite() { post } else { 0.0 };
        let xi = (alpha * *g * *g * *gm + (1.0 - alpha) * (post - 1.0).max(0.0)).max(xi_min);
        let new = lsa_gain(xi, post).clamp(floor, 1.0);
        *g = new;
        *gm = post;
        *y *= if noise_only { 1.0 - new } else { new };
    }
}
