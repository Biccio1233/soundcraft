//! Sample-rate conversion by windowed-sinc interpolation (Kaiser window, β 8.6).

/// Zero crossings of the sinc on each side, counted at the lower of the two rates.
const ZERO_CROSSINGS: f64 = 32.0;
const BETA: f64 = 8.6;
/// Kernel table resolution, in steps per input sample.
const STEPS: f64 = 256.0;

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0f64, 1.0f64);
    for k in 1..64 {
        let f = x / (2.0 * k as f64);
        term *= f * f;
        sum += term;
        if term < sum * 1e-16 {
            break;
        }
    }
    sum
}

/// `x` converted from `from` Hz to `to` Hz: `round(len × to / from)` samples, band-limited to 0.95 of
/// the lower Nyquist. Returns `x` unchanged when the rates match and nothing for a zero rate.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == 0 || to == 0 || x.is_empty() {
        return Vec::new();
    }
    if from == to {
        return x.to_vec();
    }
    let out_len = (x.len() as u64).saturating_mul(u64::from(to)).saturating_add(u64::from(from) / 2) / u64::from(from);
    // Cutoff in cycles per input sample, and the kernel's half width in input samples.
    let fc = 0.95 * 0.5 * f64::from(from.min(to)) / f64::from(from);
    let half = ZERO_CROSSINGS / (2.0 * fc);
    let i0_beta = bessel_i0(BETA);
    let table: Vec<f64> = (0..=(half * STEPS).ceil() as usize + 1)
        .map(|i| {
            let d = i as f64 / STEPS;
            if d >= half {
                return 0.0;
            }
            let arg = 2.0 * fc * d;
            let sinc = if arg == 0.0 { 1.0 } else { (std::f64::consts::PI * arg).sin() / (std::f64::consts::PI * arg) };
            let u = d / half;
            2.0 * fc * sinc * bessel_i0(BETA * (1.0 - u * u).max(0.0).sqrt()) / i0_beta
        })
        .collect();
    let kernel = |d: f64| -> f64 {
        let p = d * STEPS;
        let i = p as usize;
        let t = p - i as f64;
        let a = table.get(i).copied().unwrap_or(0.0);
        let b = table.get(i + 1).copied().unwrap_or(0.0);
        a + (b - a) * t
    };
    let step = f64::from(from) / f64::from(to);
    let last = x.len() - 1;
    (0..out_len)
        .map(|n| {
            let t = n as f64 * step;
            let lo = (t - half).ceil().max(0.0) as usize;
            let hi = ((t + half).floor().max(0.0) as usize).min(last);
            let mut acc = 0.0f64;
            if let Some(span) = x.get(lo..=hi) {
                for (k, v) in span.iter().enumerate() {
                    acc += f64::from(*v) * kernel(((lo + k) as f64 - t).abs());
                }
            }
            acc as f32
        })
        .collect()
}
