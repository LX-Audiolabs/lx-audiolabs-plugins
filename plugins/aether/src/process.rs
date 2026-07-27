//! Aether process path split for profile/isolation.
//!
//! ponytail: same-crate module split only — no behavior change.

use truce::prelude::*;
use lx_dsp::FtzDazGuard;

use crate::{AetherDspState, AetherParams, CF_DELAY_MAX, NUM_BANDS};

pub(crate) fn run(
    state: &mut AetherDspState,
    params: &AetherParams,
    buffer: &mut AudioBuffer,
) -> ProcessStatus {
    let _ftz = FtzDazGuard::new();

    if buffer.num_input_channels() < 2 {
        return ProcessStatus::Normal;
    }
    // ponytail: never process with empty delay (would panic on % 0)
    if state.cf_delay_l.len() < 2 || state.cf_delay_r.len() < 2 {
        if state.cf_delay_l.len() != CF_DELAY_MAX {
            state.cf_delay_l.resize(CF_DELAY_MAX, 0.0);
        }
        if state.cf_delay_r.len() != CF_DELAY_MAX {
            state.cf_delay_r.resize(CF_DELAY_MAX, 0.0);
        }
        state.cf_delay_pos = 0;
    }
    let delay_len = state.cf_delay_l.len();
    let sr = state.sample_rate.max(1.0);
    let num_samples = buffer.num_samples();

    let mut in_peak = 0.0f32;
    for i in 0..num_samples {
        in_peak = in_peak
            .max(buffer.input(0)[i].abs())
            .max(buffer.input(1)[i].abs());
    }
    let in_db = if in_peak < 1e-9 {
        -90.0
    } else {
        20.0 * in_peak.log10()
    };
    params
        .shared
        .input_peak
        .store(in_db, std::sync::atomic::Ordering::Release);

    if params.bypass.value() {
        for ch in 0..buffer.channels() {
            let (inp, out) = buffer.io(ch);
            out.copy_from_slice(inp);
        }
        return ProcessStatus::Normal;
    }

    state.update_eq_coeffs(params);
    let blend = params.blend.raw_target() as f32 / 100.0;
    let (itd_ms, cut_mul, feed_mul) = match params.cf_realism.value_i32() {
        1 => (0.32, 0.85, 1.05),
        2 => (0.45, 0.70, 1.15),
        _ => (0.22, 1.00, 1.00),
    };
    let cf_mix =
        ((params.cf_amount.raw_target() as f32 / 100.0) * 0.5 * feed_mul).min(0.75);
    let cf_norm = ((params.cf_angle.raw_target() as f32 - 30.0) / 45.0).clamp(0.0, 1.0);
    let cf_fc = (700.0 + cf_norm * 1300.0) * cut_mul;
    let cf_a = 1.0 - (-2.0 * std::f32::consts::PI * cf_fc / sr).exp();
    let max_delay = delay_len - 1;
    let delay_samples = ((itd_ms * 0.001 * sr).round() as usize).min(max_delay);
    // Keep write head in range if state was resized mid-flight.
    if state.cf_delay_pos >= delay_len {
        state.cf_delay_pos = 0;
    }

    for i in 0..num_samples {
        let in_l = buffer.input(0)[i];
        let in_r = buffer.input(1)[i];

        let mut eq_l = in_l;
        let mut eq_r = in_r;
        for b in 0..NUM_BANDS {
            eq_l = state.eq_l[b].process(eq_l);
            eq_r = state.eq_r[b].process(eq_r);
        }
        let h_l = in_l + (eq_l - in_l) * blend;
        let h_r = in_r + (eq_r - in_r) * blend;

        let wp = state.cf_delay_pos;
        state.cf_delay_l[wp] = h_l;
        state.cf_delay_r[wp] = h_r;
        let rp = (wp + delay_len - delay_samples) % delay_len;
        let del_l = state.cf_delay_l[rp];
        let del_r = state.cf_delay_r[rp];
        state.cf_delay_pos = (wp + 1) % delay_len;

        state.cf_lp_l += cf_a * (del_r - state.cf_lp_l);
        state.cf_lp_r += cf_a * (del_l - state.cf_lp_r);
        let cf_l = h_l + state.cf_lp_l * cf_mix;
        let cf_r = h_r + state.cf_lp_r * cf_mix;

        let gain_smoothed = params.gain.value();
        let g = 10.0_f32.powf(gain_smoothed / 20.0);
        buffer.output(0)[i] = cf_l * g;
        buffer.output(1)[i] = cf_r * g;
    }

    ProcessStatus::Normal
}
