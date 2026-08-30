//! Meridian process path split for profile/isolation:
//!   param_update  — resets, dirty coefs, per-block param snapshot
//!   process_block — sample loop + accumulators
//!   publish       — meters, FFT, snap, auto-loud, scope
//!
//! ponytail: same-crate module split only — no behavior change.

use aura::prelude::*;
use std::f32::consts::FRAC_PI_4;
use std::sync::atomic::Ordering;

use aura_dsp::analysis::{SCOPE_BUFFER_LEN, SPECTRUM_BINS, SnapMode, compute_spectrum_bins};
use aura_dsp::fx::{DBTP_CEILING, FtzDazGuard};

use crate::{
    MeridianDspState, MeridianParams, db_to_gain, gain_to_db, inflate_shape, soft_clip, tube_warm,
};

/// Per-block control / param snapshot.
pub(crate) struct ProcessControl {
    pub bypass: bool,
    pub sample_rate: f32,
    pub warmth_drive_db: f32,
    pub warmth_mix_pct: f32,
    pub excite_amt: f32,
    pub excite_blend: f32,
    pub comp_t: f32,
    pub comp_m: f32,
    pub comp_att: f32,
    pub comp_rel: f32,
    pub ratio: f32,
    pub knee: f32,
    pub comp_makeup_gain: f32,
    pub inflate_effect: f32,
    pub inflate_curve: f32,
    pub inflate_band_split: bool,
    pub inflate_clip: bool,
    pub width: f32,
    pub pan: f32,
    pub out_gain: f32,
    pub snap_phase: u8,
    pub mono: bool,
    pub delta: bool,
    pub is_measuring: bool,
}

/// Accumulators from the sample loop; consumed by publish.
pub(crate) struct BlockMetrics {
    pub max_out_peak: f32,
    pub max_out_peak_l: f32,
    pub max_out_peak_r: f32,
    pub count_samples: usize,
    pub block_band_power: [f32; 5],
    pub num_samples: usize,
    pub max_gr_db: f32,
}

pub(crate) fn run(
    state: &mut MeridianDspState,
    params: &MeridianParams,
    buffer: &mut AudioBuffer<'_, f32>,
) -> ProcessStatus {
    let _ftz = FtzDazGuard::new();

    if buffer.num_inputs() < 2 || buffer.num_outputs() < 2 {
        return ProcessStatus::Continue;
    }

    let mut ctrl = param_update(state, params);
    let Some(mut metrics) = process_block(state, params, buffer, &mut ctrl) else {
        return ProcessStatus::Continue;
    };
    publish(state, params, buffer, &ctrl, &mut metrics);
    ProcessStatus::Continue
}

fn param_update(state: &mut MeridianDspState, params: &MeridianParams) -> ProcessControl {
    let bypass = params.bypass_active.value();

    // Reset analysis
    if params
        .shared
        .snap
        .reset_analysis
        .swap(false, Ordering::Acquire)
    {
        state.fft_input.fill(0.0);
        state.fft_write_pos = 0;
        if let Ok(mut avg) = params.shared.spectrum.avg.try_lock() {
            avg.fill(-90.0);
        }
        if let Ok(mut bins) = params.shared.spectrum.bins.try_lock() {
            bins.fill(-90.0);
        }
        state.hpf_l.reset();
        state.hpf_r.reset();
        state.lpf_l.reset();
        state.lpf_r.reset();
        state.hpf2_l.reset();
        state.hpf2_r.reset();
        state.lpf2_l.reset();
        state.lpf2_r.reset();
        state.bass_l.reset();
        state.bass_r.reset();
        state.lo_mid_l.reset();
        state.lo_mid_r.reset();
        state.mid_l.reset();
        state.mid_r.reset();
        state.high_l.reset();
        state.high_r.reset();
        state.excite_l.reset();
        state.excite_r.reset();
        state.tilt_l.reset();
        state.tilt_r.reset();
        state.excite_hp_l.reset();
        state.excite_hp_r.reset();
        state.xo_inflate_lo_l.reset();
        state.xo_inflate_lo_r.reset();
        state.xo_inflate_hi_l.reset();
        state.xo_inflate_hi_r.reset();
        state.xo_bass_mid_l.reset();
        state.xo_bass_mid_r.reset();
        state.xo_low_bass_l.reset();
        state.xo_low_bass_r.reset();
        state.xo_mid_high_l.reset();
        state.xo_mid_high_r.reset();
        state.xo_highmid_high_l.reset();
        state.xo_highmid_high_r.reset();
    }

    let sample_rate = params.shared.spectrum.sample_rate.load(Ordering::Acquire);
    state.compressor.set_sample_rate(sample_rate);

    // Dirty-flag coefficient update
    let hpf_f = params.hpf_freq.raw_target() as f32;
    let lpf_f = params.lpf_freq.raw_target() as f32;
    let cut_slope_val = params.cut_slope.value();
    let bass_gain_val = params.bass_gain.raw_target() as f32;
    let bass_slope_val = params.bass_slope.value();
    let lo_mid_gain_val = params.lo_mid_gain.raw_target() as f32;
    let lo_mid_slope_val = params.lo_mid_slope.value();
    let mid_gain_val = params.mid_gain.raw_target() as f32;
    let mid_slope_val = params.mid_slope.value();
    let high_gain_val = params.high_gain.raw_target() as f32;
    let high_slope_val = params.high_slope.value();
    let excite_gain_val = params.excite_gain.raw_target() as f32;
    let excite_slope_val = params.excite_slope.value();
    let eq_f1 = params.eq_freq_1.raw_target() as f32;
    let eq_f2 = params.eq_freq_2.raw_target() as f32;
    let eq_f3 = params.eq_freq_3.raw_target() as f32;
    let eq_f4 = params.eq_freq_4.raw_target() as f32;
    let eq_f5 = params.eq_freq_5.raw_target() as f32;
    let tilt_db = params.tilt_gain.raw_target() as f32;
    let excite_freq = params.excite_freq.raw_target() as f32;

    let slope_val = |slope_idx: i64| -> f32 {
        match slope_idx {
            0 => 0.5,
            1 => 1.0,
            _ => 2.0,
        }
    };
    let q_val = |slope_idx: i64| -> f32 {
        match slope_idx {
            0 => 0.4,
            1 => 0.7,
            _ => 1.5,
        }
    };

    let coef_dirty = sample_rate != state.cached_sample_rate;
    state.cached_sample_rate = sample_rate;

    if hpf_f != state.cached_hpf_freq
        || lpf_f != state.cached_lpf_freq
        || cut_slope_val != state.cached_cut_slope
        || coef_dirty
    {
        // Safety: reset filter state on >4 octave jump (right-click reset fix)
        let hpf_jump = if state.cached_hpf_freq > 1.0 && hpf_f > 1.0 {
            (hpf_f / state.cached_hpf_freq).max(state.cached_hpf_freq / hpf_f)
        } else if hpf_f != state.cached_hpf_freq {
            4.1
        } else {
            1.0
        };
        let lpf_jump = if state.cached_lpf_freq > 1.0 && lpf_f > 1.0 {
            (lpf_f / state.cached_lpf_freq).max(state.cached_lpf_freq / lpf_f)
        } else if lpf_f != state.cached_lpf_freq {
            4.1
        } else {
            1.0
        };
        if hpf_jump > 4.0 {
            state.hpf_l.reset();
            state.hpf_r.reset();
            state.hpf2_l.reset();
            state.hpf2_r.reset();
        }
        if lpf_jump > 4.0 {
            state.lpf_l.reset();
            state.lpf_r.reset();
            state.lpf2_l.reset();
            state.lpf2_r.reset();
        }
        state.cached_hpf_freq = hpf_f;
        state.cached_lpf_freq = lpf_f;
        state.cached_cut_slope = cut_slope_val;
        const Q1: f32 = 0.541_196_1;
        const Q2: f32 = 1.306_563;
        if cut_slope_val >= 1 {
            state.hpf_l.set_butterworth_hp_q(hpf_f, Q1, sample_rate);
            state.hpf_r.set_butterworth_hp_q(hpf_f, Q1, sample_rate);
            state.hpf2_l.set_butterworth_hp_q(hpf_f, Q2, sample_rate);
            state.hpf2_r.set_butterworth_hp_q(hpf_f, Q2, sample_rate);
            state.lpf_l.set_butterworth_lp_q(lpf_f, Q1, sample_rate);
            state.lpf_r.set_butterworth_lp_q(lpf_f, Q1, sample_rate);
            state.lpf2_l.set_butterworth_lp_q(lpf_f, Q2, sample_rate);
            state.lpf2_r.set_butterworth_lp_q(lpf_f, Q2, sample_rate);
        } else {
            state.hpf_l.set_butterworth_hp(hpf_f, sample_rate);
            state.hpf_r.set_butterworth_hp(hpf_f, sample_rate);
            state.lpf_l.set_butterworth_lp(lpf_f, sample_rate);
            state.lpf_r.set_butterworth_lp(lpf_f, sample_rate);
            state.hpf2_l.set_identity();
            state.hpf2_r.set_identity();
            state.lpf2_l.set_identity();
            state.lpf2_r.set_identity();
        }
    }

    if bass_gain_val != state.cached_bass_gain
        || bass_slope_val != state.cached_bass_slope
        || eq_f1 != state.cached_eq_freq_1
        || coef_dirty
    {
        state.cached_bass_gain = bass_gain_val;
        state.cached_bass_slope = bass_slope_val;
        state.cached_eq_freq_1 = eq_f1;
        let bass_slope = slope_val(bass_slope_val);
        state
            .bass_l
            .set_low_shelf(eq_f1, bass_gain_val, bass_slope, sample_rate);
        state
            .bass_r
            .set_low_shelf(eq_f1, bass_gain_val, bass_slope, sample_rate);
    }

    if lo_mid_gain_val != state.cached_lo_mid_gain
        || lo_mid_slope_val != state.cached_lo_mid_slope
        || eq_f2 != state.cached_eq_freq_2
        || coef_dirty
    {
        state.cached_lo_mid_gain = lo_mid_gain_val;
        state.cached_lo_mid_slope = lo_mid_slope_val;
        state.cached_eq_freq_2 = eq_f2;
        let lo_mid_q = q_val(lo_mid_slope_val);
        state
            .lo_mid_l
            .set_peaking_eq(eq_f2, lo_mid_gain_val, lo_mid_q, sample_rate);
        state
            .lo_mid_r
            .set_peaking_eq(eq_f2, lo_mid_gain_val, lo_mid_q, sample_rate);
    }

    if mid_gain_val != state.cached_mid_gain
        || mid_slope_val != state.cached_mid_slope
        || eq_f3 != state.cached_eq_freq_3
        || coef_dirty
    {
        state.cached_mid_gain = mid_gain_val;
        state.cached_mid_slope = mid_slope_val;
        state.cached_eq_freq_3 = eq_f3;
        let mid_q = q_val(mid_slope_val);
        state
            .mid_l
            .set_peaking_eq(eq_f3, mid_gain_val, mid_q, sample_rate);
        state
            .mid_r
            .set_peaking_eq(eq_f3, mid_gain_val, mid_q, sample_rate);
    }

    if high_gain_val != state.cached_high_gain
        || high_slope_val != state.cached_high_slope
        || eq_f4 != state.cached_eq_freq_4
        || coef_dirty
    {
        state.cached_high_gain = high_gain_val;
        state.cached_high_slope = high_slope_val;
        state.cached_eq_freq_4 = eq_f4;
        let high_q = q_val(high_slope_val);
        state
            .high_l
            .set_peaking_eq(eq_f4, high_gain_val, high_q, sample_rate);
        state
            .high_r
            .set_peaking_eq(eq_f4, high_gain_val, high_q, sample_rate);
    }

    if excite_gain_val != state.cached_excite_gain
        || excite_slope_val != state.cached_excite_slope
        || eq_f5 != state.cached_eq_freq_5
        || coef_dirty
    {
        state.cached_excite_gain = excite_gain_val;
        state.cached_excite_slope = excite_slope_val;
        state.cached_eq_freq_5 = eq_f5;
        let excite_slope = slope_val(excite_slope_val);
        state
            .excite_l
            .set_high_shelf(eq_f5, excite_gain_val, excite_slope, sample_rate);
        state
            .excite_r
            .set_high_shelf(eq_f5, excite_gain_val, excite_slope, sample_rate);
    }

    if tilt_db != state.cached_tilt_gain || coef_dirty {
        state.cached_tilt_gain = tilt_db;
        state.tilt_l.set(1000.0, tilt_db, sample_rate);
        state.tilt_r.set(1000.0, tilt_db, sample_rate);
    }

    if excite_freq != state.cached_excite_freq || coef_dirty {
        state.cached_excite_freq = excite_freq;
        state
            .excite_hp_l
            .set_butterworth_hp(excite_freq, sample_rate);
        state
            .excite_hp_r
            .set_butterworth_hp(excite_freq, sample_rate);
    }

    // Reset peak
    if params
        .shared
        .peaks
        .reset_peak
        .swap(false, Ordering::Release)
    {
        state.peak_hold_value = -90.0;
        state.peak_hold_l_value = -90.0;
        state.peak_hold_r_value = -90.0;
    }

    // Smoothed parameter values (per-block reads via value(), truce pattern)
    let warmth_drive_db = params.warmth_drive.value();
    let warmth_mix_pct = params.warmth_mix.value();
    let excite_amt = params.excite_amount.value();
    let excite_blend = params.excite_blend.value();
    let comp_t = params.comp_threshold.value();
    let comp_m = params.comp_mix.value();
    let comp_att = params.comp_attack.value();
    let comp_rel = params.comp_release.value();
    let ratio = params.comp_character.value();
    let knee = (1.0 - (ratio - 1.5) / 2.5) * 6.0;
    let comp_makeup_gain = db_to_gain(params.comp_makeup.value());

    let inflate_effect = params.inflate_effect.value() / 100.0;
    let inflate_curve = params.inflate_curve.value();
    let inflate_band_split = params.inflate_band_split.value();
    let inflate_clip = params.inflate_clip.value();

    let width = params.stereo_width.value() / 100.0;
    let pan = params.pan.value();
    let out_gain = db_to_gain(params.output_gain.value());

    let snap_phase = params.shared.snap.phase.load(Ordering::Acquire);
    let mono = match snap_phase {
        2 => true,
        3 => false,
        _ => params.mono_active.value(),
    };
    let delta = match snap_phase {
        3 => true,
        _ => params.delta_active.value(),
    };

    let is_measuring = params.shared.auto_loud.measuring.load(Ordering::Acquire);

    ProcessControl {
        bypass,
        sample_rate,
        warmth_drive_db,
        warmth_mix_pct,
        excite_amt,
        excite_blend,
        comp_t,
        comp_m,
        comp_att,
        comp_rel,
        ratio,
        knee,
        comp_makeup_gain,
        inflate_effect,
        inflate_curve,
        inflate_band_split,
        inflate_clip,
        width,
        pan,
        out_gain,
        snap_phase,
        mono,
        delta,
        is_measuring,
    }
}
fn process_block(
    state: &mut MeridianDspState,
    _params: &MeridianParams,
    buffer: &mut AudioBuffer<'_, f32>,
    ctrl: &mut ProcessControl,
) -> Option<BlockMetrics> {
    let bypass = ctrl.bypass;
    let warmth_drive_db = ctrl.warmth_drive_db;
    let warmth_mix_pct = ctrl.warmth_mix_pct;
    let excite_amt = ctrl.excite_amt;
    let excite_blend = ctrl.excite_blend;
    let comp_t = ctrl.comp_t;
    let comp_m = ctrl.comp_m;
    let comp_att = ctrl.comp_att;
    let comp_rel = ctrl.comp_rel;
    let ratio = ctrl.ratio;
    let knee = ctrl.knee;
    let comp_makeup_gain = ctrl.comp_makeup_gain;
    let inflate_effect = ctrl.inflate_effect;
    let inflate_curve = ctrl.inflate_curve;
    let inflate_band_split = ctrl.inflate_band_split;
    let inflate_clip = ctrl.inflate_clip;
    let width = ctrl.width;
    let pan = ctrl.pan;
    let out_gain = ctrl.out_gain;
    let mono = ctrl.mono;
    let delta = ctrl.delta;
    let is_measuring = ctrl.is_measuring;
    let mut max_out_peak = 0.0f32;
    let mut max_out_peak_l = 0.0f32;
    let mut max_out_peak_r = 0.0f32;
    let mut count_samples: usize = 0;

    let mut block_band_power = [0.0f32; 5];

    if buffer.num_samples() == 0 {
        return None;
    }
    let num_samples = buffer.num_samples();

    let mut gr_db = 0.0f32;
    let mut max_gr_db = 0.0f32;

    // Copy I/O — AURA buffer has separate input/output borrows (no dual-mut io()).
    let in0: Vec<f32> = buffer.input(0).to_vec();
    let in1: Vec<f32> = buffer.input(1).to_vec();
    let mut out0 = vec![0.0f32; num_samples];
    let mut out1 = vec![0.0f32; num_samples];

    // Feed input LUFS
    if is_measuring {
        state.auto_loud_in.feed(&in0, &in1);
        state.pre_sat_buf_l.clear();
        state.pre_sat_buf_r.clear();
    }

    for i in 0..num_samples {
        count_samples += 1;
        let in_l = in0[i];
        let in_r = in1[i];

        // HPF & LPF
        let mut x_l = state.lpf2_l.process(
            state
                .lpf_l
                .process(state.hpf2_l.process(state.hpf_l.process(in_l))),
        );
        let mut x_r = state.lpf2_r.process(
            state
                .lpf_r
                .process(state.hpf2_r.process(state.hpf_r.process(in_r))),
        );

        // Series EQ
        x_l = state.excite_l.process(
            state.high_l.process(
                state
                    .mid_l
                    .process(state.lo_mid_l.process(state.bass_l.process(x_l))),
            ),
        );
        x_r = state.excite_r.process(
            state.high_r.process(
                state
                    .mid_r
                    .process(state.lo_mid_r.process(state.bass_r.process(x_r))),
            ),
        );

        // Tilt
        x_l = state.tilt_l.process(x_l);
        x_r = state.tilt_r.process(x_r);

        // Exciter
        if excite_amt > 0.0 || excite_blend > 0.0 {
            let high_l = state.excite_hp_l.process(x_l);
            let high_r = state.excite_hp_r.process(x_r);
            let drive = 1.0 + (excite_amt / 30.0) * 59.0;
            let sat_high_l = soft_clip(high_l * drive);
            let sat_high_r = soft_clip(high_r * drive);
            let blend = excite_blend / 100.0;
            x_l += (sat_high_l - high_l) * blend;
            x_r += (sat_high_r - high_r) * blend;
        }

        // Compressor
        let (mut comp_l, mut comp_r) = state.compressor.process(
            x_l, x_r, comp_t, comp_m, comp_att, comp_rel, ratio, knee, &mut gr_db,
        );
        max_gr_db = max_gr_db.max(gr_db);
        comp_l *= comp_makeup_gain;
        comp_r *= comp_makeup_gain;

        // Pre-sat LUFS
        if is_measuring {
            state.pre_sat_buf_l.push(comp_l);
            state.pre_sat_buf_r.push(comp_r);
        }

        // Warmth
        if warmth_drive_db > 0.0 || warmth_mix_pct > 0.0 {
            let drive = db_to_gain(warmth_drive_db);
            let wet_l = tube_warm(comp_l * drive) / drive;
            let wet_r = tube_warm(comp_r * drive) / drive;
            let mix = warmth_mix_pct / 100.0;
            comp_l = comp_l * (1.0 - mix) + wet_l * mix;
            comp_r = comp_r * (1.0 - mix) + wet_r * mix;
        }

        // Inflate (Oxford-Inflator-inspired loudness/density waveshaper)
        if inflate_effect > 0.0 {
            let shape_one = |v: f32| -> f32 {
                let v = if inflate_clip {
                    v.clamp(-1.0, 1.0)
                } else {
                    v.clamp(-2.0, 2.0)
                };
                inflate_shape(v, inflate_curve)
            };
            let (wet_l, wet_r) = if inflate_band_split {
                let (lo_l, hi_l) = state.xo_inflate_lo_l.process(comp_l);
                let (mid_l, top_l) = state.xo_inflate_hi_l.process(hi_l);
                let (lo_r, hi_r) = state.xo_inflate_lo_r.process(comp_r);
                let (mid_r, top_r) = state.xo_inflate_hi_r.process(hi_r);
                (
                    shape_one(lo_l) + shape_one(mid_l) + shape_one(top_l),
                    shape_one(lo_r) + shape_one(mid_r) + shape_one(top_r),
                )
            } else {
                (shape_one(comp_l), shape_one(comp_r))
            };
            comp_l = comp_l * (1.0 - inflate_effect) + wet_l * inflate_effect;
            comp_r = comp_r * (1.0 - inflate_effect) + wet_r * inflate_effect;
        }

        // Pan
        let pan_val = pan.clamp(-1.0, 1.0);
        let pan_angle = (pan_val + 1.0) * FRAC_PI_4;
        let raw_l = pan_angle.cos();
        let raw_r = pan_angle.sin();
        let max_raw = raw_l.max(raw_r);
        let pan_norm = if max_raw > 0.001 { 1.0 / max_raw } else { 1.0 };
        let pan_l = raw_l * pan_norm;
        let pan_r = raw_r * pan_norm;
        let mut out_l = comp_l * pan_l;
        let mut out_r = comp_r * pan_r;

        // Stereo Width
        let w = width.clamp(0.0, 2.0);
        let a = 0.5 * (1.0 + w);
        let b = 0.5 * (1.0 - w);
        let width_l = out_l * a + out_r * b;
        let width_r = out_r * a + out_l * b;
        let width_norm = 1.0 / (1.0 + (w - 1.0).max(0.0) * 0.20);
        out_l = width_l * width_norm;
        out_r = width_r * width_norm;

        // Mono
        if mono {
            let m = (out_l + out_r) * 0.5;
            out_l = m;
            out_r = m;
        }

        let mut processed_l = out_l;
        let mut processed_r = out_r;

        // Delta
        if delta {
            processed_l = out_l - in_l;
            processed_r = out_r - in_r;
        }

        processed_l *= out_gain;
        processed_r *= out_gain;

        // Safety clamp
        processed_l = processed_l.clamp(-1.0, 1.0);
        processed_r = processed_r.clamp(-1.0, 1.0);

        // Crossover analysis for visualizer
        let (low_group_l, high_group_l) = state.xo_bass_mid_l.process(processed_l);
        let (band1_l, band2_l) = state.xo_low_bass_l.process(low_group_l);
        let (mid_group_l, super_high_group_l) = state.xo_mid_high_l.process(high_group_l);
        let (band3_l, band4_l) = (mid_group_l, super_high_group_l);
        let (band4_l_split, band5_l) = state.xo_highmid_high_l.process(band4_l);
        let band4_l = band4_l_split;

        let (low_group_r, high_group_r) = state.xo_bass_mid_r.process(processed_r);
        let (band1_r, band2_r) = state.xo_low_bass_r.process(low_group_r);
        let (mid_group_r, super_high_group_r) = state.xo_mid_high_r.process(high_group_r);
        let (band3_r, band4_r) = (mid_group_r, super_high_group_r);
        let (band4_r_split, band5_r) = state.xo_highmid_high_r.process(band4_r);
        let band4_r = band4_r_split;

        let bands_l = [band1_l, band2_l, band3_l, band4_l, band5_l];
        let bands_r = [band1_r, band2_r, band3_r, band4_r, band5_r];
        for b in 0..5 {
            let band_power = (bands_l[b] * bands_l[b] + bands_r[b] * bands_r[b]) * 0.5;
            block_band_power[b] += band_power;
        }

        let (meter_l, meter_r) = if bypass {
            out0[i] = in_l;
            out1[i] = in_r;
            (in_l, in_r)
        } else {
            max_out_peak = max_out_peak.max(processed_l.abs()).max(processed_r.abs());
            max_out_peak_l = max_out_peak_l.max(processed_l.abs());
            max_out_peak_r = max_out_peak_r.max(processed_r.abs());
            out0[i] = processed_l;
            out1[i] = processed_r;
            (processed_l, processed_r)
        };

        let corr_lr = meter_l * meter_r;
        let corr_l2 = meter_l * meter_l;
        let corr_r2 = meter_r * meter_r;
        state.corr_avg_lr = (1.0 - state.correlation_decay_coef) * state.corr_avg_lr
            + state.correlation_decay_coef * corr_lr;
        state.corr_avg_l2 = (1.0 - state.correlation_decay_coef) * state.corr_avg_l2
            + state.correlation_decay_coef * corr_l2;
        state.corr_avg_r2 = (1.0 - state.correlation_decay_coef) * state.corr_avg_r2
            + state.correlation_decay_coef * corr_r2;
    }

    buffer.output(0).copy_from_slice(&out0);
    buffer.output(1).copy_from_slice(&out1);

    Some(BlockMetrics {
        max_out_peak,
        max_out_peak_l,
        max_out_peak_r,
        count_samples,
        block_band_power,
        num_samples,
        max_gr_db,
    })
}
fn publish(
    state: &mut MeridianDspState,
    params: &MeridianParams,
    buffer: &mut AudioBuffer<'_, f32>,
    ctrl: &ProcessControl,
    metrics: &mut BlockMetrics,
) {
    let sample_rate = ctrl.sample_rate;
    let mut snap_phase = ctrl.snap_phase;
    let is_measuring = ctrl.is_measuring;
    let max_out_peak = metrics.max_out_peak;
    let max_out_peak_l = metrics.max_out_peak_l;
    let max_out_peak_r = metrics.max_out_peak_r;
    let count_samples = metrics.count_samples;
    let block_band_power = metrics.block_band_power;
    let num_samples = metrics.num_samples;
    let max_gr_db = metrics.max_gr_db;

    // Copy outs for analysis (can't hold two mut output borrows).
    let out0: Vec<f32> = buffer.output(0).to_vec();
    let out1: Vec<f32> = buffer.output(1).to_vec();
    let in0: Vec<f32> = buffer.input(0).to_vec();
    let in1: Vec<f32> = buffer.input(1).to_vec();

    // Gain reduction
    params
        .shared
        .gain_reduction
        .store(max_gr_db, Ordering::Release);

    // Smoothed band levels
    let sample_weight = 1.0 / count_samples as f32;
    let buf_coef = 1.0 - (-(num_samples as f32) / (0.1 * sample_rate)).exp();
    for (b, &band_power) in block_band_power.iter().enumerate() {
        let average_band_power = band_power * sample_weight;
        state.smoothed_band_power[b] =
            (1.0 - buf_coef) * state.smoothed_band_power[b] + buf_coef * average_band_power;
        let band_db = gain_to_db(state.smoothed_band_power[b].sqrt());
        params.shared.band_levels[b].store(band_db, Ordering::Release);
    }

    // Correlation
    let denom = (state.corr_avg_l2 * state.corr_avg_r2).sqrt();
    let corr = if denom > 1e-6 {
        state.corr_avg_lr / denom
    } else {
        1.0
    };
    params
        .shared
        .peaks
        .phase_correlation
        .store(corr, Ordering::Release);

    // Peak meters
    let peak_db = gain_to_db(max_out_peak);
    let peak_l_db = gain_to_db(max_out_peak_l);
    let peak_r_db = gain_to_db(max_out_peak_r);
    params
        .shared
        .peaks
        .output_peak
        .store(peak_db, Ordering::Release);
    params
        .shared
        .peaks
        .output_peak_l
        .store(peak_l_db, Ordering::Release);
    params
        .shared
        .peaks
        .output_peak_r
        .store(peak_r_db, Ordering::Release);
    state.peak_hold_value = state.peak_hold_value.max(peak_db);
    state.peak_hold_l_value = state.peak_hold_l_value.max(peak_l_db);
    state.peak_hold_r_value = state.peak_hold_r_value.max(peak_r_db);
    params
        .shared
        .peaks
        .peak_hold
        .store(state.peak_hold_value, Ordering::Release);
    params
        .shared
        .peaks
        .peak_hold_l
        .store(state.peak_hold_l_value, Ordering::Release);
    params
        .shared
        .peaks
        .peak_hold_r
        .store(state.peak_hold_r_value, Ordering::Release);

    // Balance
    let rms_l = state.corr_avg_l2.sqrt();
    let rms_r = state.corr_avg_r2.sqrt();
    let sum_lr = rms_l + rms_r;
    let balance = if sum_lr > 1e-6 {
        (rms_l - rms_r) / sum_lr
    } else {
        0.0
    };
    params
        .shared
        .peaks
        .balance
        .store(balance, Ordering::Release);

    // FFT Spectrum
    {
        let n = num_samples;
        let fft_size = state.fft_input.len();
        for i in 0..n {
            state.fft_input[state.fft_write_pos] = (out0[i] + out1[i]) * 0.5;
            state.fft_write_pos += 1;
            if state.fft_write_pos >= fft_size {
                let half = fft_size / 2;
                for j in 0..half {
                    state.fft_input[j] = state.fft_input[j + half];
                }
                state.fft_write_pos = half;

                for i in 0..fft_size {
                    state.fft_windowed[i] = state.fft_input[i] * state.fft_hann[i];
                }
                let fft = state.fft_planner.plan_fft_forward(fft_size);
                fft.process(&mut state.fft_windowed, &mut state.fft_output_cache)
                    .ok();
            }
        }

        // Compute and write spectrum after each buffer
        if let Ok(mut spectrum_frame) = params.shared.spectrum.bins.try_lock() {
            compute_spectrum_bins(
                &state.fft_output_cache,
                &mut spectrum_frame,
                fft_size,
                sample_rate,
            );
        }

        // Update spectrum_avg (EMA) from spectrum_bins
        if let Ok(mut avg) = params.shared.spectrum.avg.try_lock()
            && let Ok(bins) = params.shared.spectrum.bins.try_lock()
        {
            let n_bins = SPECTRUM_BINS;
            // Energy-gating: only update EMA if signal above -80 dB
            let frame_energy = bins.iter().map(|x| x * x).sum::<f32>() / n_bins as f32;
            let energy_db = 10.0 * frame_energy.log10().max(-40.0);
            let gate = energy_db > -80.0;

            if !gate {
                for sample in state.fft_input.iter_mut() {
                    *sample = 0.0;
                }
            }

            for k in 0..n_bins {
                let freq = k as f32 * sample_rate / fft_size as f32;
                let log_norm = ((freq.max(20.0).ln() - 20.0_f32.ln())
                    / (20000.0_f32.ln() - 20.0_f32.ln()))
                .clamp(0.0, 1.0);
                let alpha = 0.02 + (0.10 - 0.02) * log_norm;
                let input = if gate { bins[k] } else { 0.0 };
                avg[k] = avg[k] * (1.0 - alpha) + input * alpha;
            }
        }
    }

    // SNAP FFT
    if snap_phase > 0 {
        for i in 0..num_samples {
            let sample = match snap_phase {
                1 | 2 => (out0[i] + out1[i]) * 0.5,
                3 => {
                    let out_mono = (out0[i] + out1[i]) * 0.5;
                    let in_mono = (in0[i] + in1[i]) * 0.5;
                    out_mono - in_mono
                }
                _ => 0.0,
            };
            if state.snap_fft.push_sample(sample) {
                let frame = state.snap_fft.compute_fft(sample_rate);
                let threshold = if snap_phase == 2 || snap_phase == 3 {
                    30
                } else {
                    60
                };
                if state
                    .snap_fft
                    .accumulate_snap(&frame, snap_phase, threshold)
                {
                    let mode = match snap_phase {
                        1 => SnapMode::Stereo,
                        2 => SnapMode::Mono,
                        _ => SnapMode::Delta,
                    };
                    let snapshot = state.snap_fft.read_snapshot(mode);
                    if let Ok(mut buf) = match mode {
                        SnapMode::Stereo => params.shared.snap.stereo.try_lock(),
                        SnapMode::Mono => params.shared.snap.mono.try_lock(),
                        SnapMode::Delta => params.shared.snap.delta.try_lock(),
                    } {
                        buf.copy_from_slice(&snapshot);
                    }
                    let next_phase = if snap_phase < 3 { snap_phase + 1 } else { 0 };
                    if next_phase == 0 {
                        params.shared.snap.active.store(false, Ordering::Release);
                    } else {
                        state.snap_fft.reset_snapshots();
                    }
                    params
                        .shared
                        .snap
                        .phase
                        .store(next_phase, Ordering::Release);
                    snap_phase = next_phase;
                }
            }
        }
    }

    // AUTO LOUD
    if params.shared.auto_loud.trigger.load(Ordering::Acquire) {
        params
            .shared
            .auto_loud
            .trigger
            .store(false, Ordering::Release);
        params
            .shared
            .auto_loud
            .measuring
            .store(true, Ordering::Release);
        state.auto_loud_in.reset();
        state.auto_loud_pre_sat.reset();
        state.auto_loud_out.reset();
    }
    if is_measuring {
        if !state.pre_sat_buf_l.is_empty() {
            state
                .auto_loud_pre_sat
                .feed(&state.pre_sat_buf_l, &state.pre_sat_buf_r);
        }
        state.auto_loud_out.feed(&out0, &out1);
        let target_samples = (5.0 * sample_rate as f64) as u64;
        if state.auto_loud_out.sample_count() >= target_samples {
            let in_lufs = state.auto_loud_in.loudness_db();
            let _pre_lufs = state.auto_loud_pre_sat.loudness_db();
            let out_lufs = state.auto_loud_out.loudness_db();
            let out_tp = state.auto_loud_out.true_peak_db();
            let lufs_offset = in_lufs - out_lufs;
            let peak_limit = DBTP_CEILING - out_tp;
            let offset_clamped = lufs_offset.clamp(-24.0, peak_limit);
            params
                .shared
                .auto_loud
                .gain_offset
                .store(offset_clamped, Ordering::Release);
            params
                .shared
                .auto_loud
                .measuring
                .store(false, Ordering::Release);
        }
    }

    // Goniometer scope buffer
    {
        let n = num_samples.min(SCOPE_BUFFER_LEN);
        let block_peak = (0..n)
            .map(|i| out0[i].abs().max(out1[i].abs()))
            .fold(0.0f32, f32::max)
            .max(1e-9);
        let att = 1.0 - (-(n as f32) / (0.005 * sample_rate)).exp();
        let rel = 1.0 - (-(n as f32) / (0.300 * sample_rate)).exp();
        if block_peak > state.scope_vis_envelope {
            state.scope_vis_envelope += att * (block_peak - state.scope_vis_envelope);
        } else {
            state.scope_vis_envelope += rel * (block_peak - state.scope_vis_envelope);
        }
        let vis_gain = if state.scope_vis_envelope > 1e-5 {
            (0.9 / state.scope_vis_envelope).min(20.0)
        } else {
            0.0
        };
        for i in 0..n {
            params
                .shared
                .scope
                .push(out0[i] * vis_gain, out1[i] * vis_gain);
        }
    }
}
