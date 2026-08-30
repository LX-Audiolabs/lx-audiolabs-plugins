//! Aether process path split for profile/isolation.
//!
//! ponytail: same-crate module split only — no behavior change.

use aura::prelude::*;
use aura_dsp::fx::FtzDazGuard;

use crate::{AetherDspState, AetherParams, CF_DELAY_MAX, NUM_BANDS};

pub(crate) fn run(
    state: &mut AetherDspState,
    params: &AetherParams,
    buffer: &mut AudioBuffer<'_, f32>,
) -> ProcessStatus {
    let _ftz = FtzDazGuard::new();

    if buffer.num_inputs() < 2 {
        return ProcessStatus::Continue;
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

    // Copy inputs — AURA buffer has separate input/output borrows (no dual-mut io()).
    let in0: Vec<f32> = buffer.input(0).to_vec();
    let in1: Vec<f32> = buffer.input(1).to_vec();
    let mut out0 = vec![0.0f32; num_samples];
    let mut out1 = vec![0.0f32; num_samples];

    let mut in_peak = 0.0f32;
    for i in 0..num_samples {
        in_peak = in_peak.max(in0[i].abs()).max(in1[i].abs());
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
        for ch in 0..buffer.num_outputs() {
            let src = buffer.input(ch).to_vec();
            buffer.output(ch).copy_from_slice(&src);
        }
        return ProcessStatus::Continue;
    }

    state.update_eq_coeffs(params);
    let blend = params.blend.raw_target() as f32 / 100.0;
    let (itd_ms, cut_mul, feed_mul) = match params.cf_realism.value_i32() {
        1 => (0.32, 0.85, 1.05),
        2 => (0.45, 0.70, 1.15),
        _ => (0.22, 1.00, 1.00),
    };
    let cf_mix = ((params.cf_amount.raw_target() as f32 / 100.0) * 0.5 * feed_mul).min(0.75);
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
        let in_l = in0[i];
        let in_r = in1[i];

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
        out0[i] = cf_l * g;
        out1[i] = cf_r * g;
    }

    buffer.output(0).copy_from_slice(&out0);
    buffer.output(1).copy_from_slice(&out1);

    ProcessStatus::Continue
}
