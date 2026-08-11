//! SVG path builders and spectrum helpers for visualizers.

/// 1/3-octave fractional-band smoothing.
///
/// `spectrum` is a half-spectrum magnitude array in dB. `fft_size` is the full
/// FFT size (= `spectrum.len() * 2` for the standard LX spectrum layout).
/// Returns 241 log-spaced dB points from 20 Hz to 20 kHz.
pub fn smooth_spectrum_third_octave(spectrum: &[f32], sample_rate: f32, fft_size: usize) -> Vec<f32> {
    if spectrum.is_empty() || fft_size == 0 {
        return Vec::new();
    }

    const DENOM_LOW: f32 = 3.0;
    const DENOM_HIGH: f32 = 20.0;
    const F_LOW: f32 = 500.0;
    const F_HIGH: f32 = 16000.0;
    const STEPS: usize = 240;

    let log_min = 20.0_f32.ln();
    let log_max = 20000.0_f32.ln();
    let bin_hz = sample_rate / fft_size as f32;
    let taper_lo = F_LOW.ln();
    let taper_hi = F_HIGH.ln();
    let len = spectrum.len();

    let power: Vec<f32> = spectrum.iter().map(|&db| 10.0_f32.powf(db * 0.1)).collect();

    let mut out = Vec::with_capacity(STEPS + 1);
    for i in 0..=STEPS {
        let frac = i as f32 / STEPS as f32;
        let ln_fc = log_min + (log_max - log_min) * frac;
        let fc = ln_fc.exp();
        let t = ((ln_fc - taper_lo) / (taper_hi - taper_lo)).clamp(0.0, 1.0);
        let denom = DENOM_LOW + (DENOM_HIGH - DENOM_LOW) * t;
        let half = 2.0_f32.powf(1.0 / (2.0 * denom));
        const MIN_BIN: f32 = 1.0;
        let lo = (fc / half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
        let hi = (fc * half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
        let avg_power = if hi - lo >= 1.0 {
            let i0 = lo.floor() as usize;
            let i1 = hi.floor() as usize;
            let mut sum = 0.0f32;
            if i0 == i1 {
                sum = power[i0] * (hi - lo);
            } else {
                sum += power[i0] * ((i0 + 1) as f32 - lo);
                for p in &power[i0 + 1..i1] {
                    sum += *p;
                }
                sum += power[i1] * (hi - i1 as f32);
            }
            sum / (hi - lo)
        } else {
            let pos = (fc / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(len - 1);
            let t_bin = pos - i0 as f32;
            power[i0] * (1.0 - t_bin) + power[i1] * t_bin
        };
        out.push((10.0 * avg_power.max(1e-12).log10()).clamp(-90.0, 12.0));
    }
    if out.len() >= 3 {
        let raw = out.clone();
        for i in 1..out.len().saturating_sub(1) {
            out[i] = raw[i - 1] * 0.25 + raw[i] * 0.5 + raw[i + 1] * 0.25;
        }
    }
    out
}

/// Goniometer display window size (number of stereo samples kept for display).
pub const GONIO_WINDOW: usize = 512;

/// Build an M/S-rotated goniometer SVG path.
///
/// `samples` yields fresh `[L, R]` pairs (e.g. `shared.scope.drain()`). They
/// are appended to `window`, which is capped to [`GONIO_WINDOW`] entries, then
/// rendered into `out`.
pub fn gonio_path<I>(samples: I, window: &mut Vec<[f32; 2]>, w: f32, h: f32, out: &mut String)
where
    I: Iterator<Item = [f32; 2]>,
{
    out.clear();
    window.extend(samples);
    let excess = window.len().saturating_sub(GONIO_WINDOW);
    if excess > 0 {
        window.drain(..excess);
    }
    if window.is_empty() {
        return;
    }

    let cx = w * 0.5;
    let cy = h * 0.5;
    let scale = cx.min(cy) * 0.9;
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;

    for (i, [l, r]) in window.iter().enumerate() {
        let m = (l + r) * inv_sqrt2;
        let side = (l - r) * inv_sqrt2;
        let x = cx - side * scale;
        let y = cy - m * scale;
        if i == 0 {
            out.push_str(&format!("M {x:.1} {y:.1}"));
        } else {
            out.push_str(&format!(" L {x:.1} {y:.1}"));
        }
    }
}
