//! Equilibrium process path split for profile/isolation:
//!   param_update  — dirty coefs, UI resets, control flags
//!   process_block — sample loop + per-block accumulators
//!   publish       — meters, listen, auto-loud, pre-master, scope
//!
//! ponytail: same-crate module split only — no behavior change.

use aura::prelude::*;
use std::f32::consts::FRAC_PI_4;
use std::sync::atomic::Ordering;

use aura_dsp::analysis::*;
use aura_dsp::fx::{DBTP_CEILING, FtzDazGuard};

use crate::{
    BAND_COUNT, EquilibriumDspState, EquilibriumParams, MINUS_INF_DB, db_to_gain, gain_to_db,
};

/// Per-block control flags (param snapshot + UI modes).
pub(crate) struct ProcessControl {
    pub sample_rate: f32,
    pub mono_maker_freq: f32,
    pub s_low: bool,
    pub s_bass: bool,
    pub s_mid: bool,
    pub s_high_mid: bool,
    pub s_high: bool,
    pub bypass: bool,
    pub snap_phase: u8,
    pub mono: bool,
    pub delta: bool,
    pub listen: bool,
    pub auto_gain: bool,
    pub is_measuring: bool,
}

/// Accumulators from the sample loop; consumed by publish.
pub(crate) struct BlockMetrics {
    pub max_out_peak: f32,
    pub max_out_peak_l: f32,
    pub max_out_peak_r: f32,
    pub sum_power_in: f32,
    pub sum_power_out: f32,
    pub sum_power_l: f32,
    pub sum_power_r: f32,
    pub count_samples: usize,
    pub block_band_power: [f32; 5],
    pub block_input_band_power: [f32; 5],
    pub num_samples: usize,
}

/// Full process entry (called from PluginLogic::process).
pub(crate) fn run(
    state: &mut EquilibriumDspState,
    params: &EquilibriumParams,
    buffer: &mut AudioBuffer<'_, f32>,
) -> ProcessStatus {
    let _ftz = FtzDazGuard::new();

    if buffer.num_inputs() < 2 || buffer.num_outputs() < 2 {
        return ProcessStatus::Continue;
    }

    let mut ctrl = param_update(state, params);
    let mut metrics = process_block(state, params, buffer, &mut ctrl);
    publish(state, params, buffer, &ctrl, &mut metrics);
    ProcessStatus::Continue
}

fn param_update(state: &mut EquilibriumDspState, params: &EquilibriumParams) -> ProcessControl {
    let sample_rate = params.shared.sample_rate.load(Ordering::Acquire);

    // Dirty-flag for mono_floor
    let mono_maker_freq = params.mono_floor.raw_target() as f32;
    let coef_dirty = sample_rate != state.cached_sample_rate;
    if coef_dirty {
        state.cached_sample_rate = sample_rate;
    }
    if (mono_maker_freq != state.cached_mono_floor_freq || coef_dirty) && mono_maker_freq > 1.0 {
        state.cached_mono_floor_freq = mono_maker_freq;
        state
            .mono_floor_filter
            .set_butterworth_hp(mono_maker_freq, sample_rate);
    }

    // Reset peak
    if params
        .shared
        .peaks
        .reset_peak
        .swap(false, Ordering::Release)
    {
        state.peak_hold_value = MINUS_INF_DB;
        state.peak_hold_l_value = MINUS_INF_DB;
        state.peak_hold_r_value = MINUS_INF_DB;
    }

    // Reset analysis
    if params
        .shared
        .snap
        .reset_analysis
        .swap(false, Ordering::Release)
    {
        for b in 0..BAND_COUNT {
            state.listen_band_power_sum[b] = 0.0;
            state.listen_lo_ema[b] = f64::INFINITY;
            state.listen_hi_ema[b] = f64::NEG_INFINITY;
            state.listen_ref_ema[b] = 0.0;
            state.listen_levels_ema[b] = -90.0;
            state.listen_min_ema[b] = -90.0;
            state.listen_max_ema[b] = -90.0;
        }
        state.listen_sample_count = 0;
        state.low_cut_l.reset();
        state.low_cut_r.reset();
        state.high_cut_l.reset();
        state.high_cut_r.reset();
        state.xo_bass_mid_l.reset();
        state.xo_bass_mid_r.reset();
        state.xo_low_bass_l.reset();
        state.xo_low_bass_r.reset();
        state.xo_mid_high_l.reset();
        state.xo_mid_high_r.reset();
        state.xo_highmid_high_l.reset();
        state.xo_highmid_high_r.reset();
        state.mono_floor_filter.reset();
    }

    let any_solo = params.solo_low.value()
        || params.solo_bass.value()
        || params.solo_mid.value()
        || params.solo_high_mid.value()
        || params.solo_high.value();

    let s_low = if any_solo {
        params.solo_low.value()
    } else {
        true
    };
    let s_bass = if any_solo {
        params.solo_bass.value()
    } else {
        true
    };
    let s_mid = if any_solo {
        params.solo_mid.value()
    } else {
        true
    };
    let s_high_mid = if any_solo {
        params.solo_high_mid.value()
    } else {
        true
    };
    let s_high = if any_solo {
        params.solo_high.value()
    } else {
        true
    };

    let bypass = params.bypass_active.value();

    let snap_phase = params.shared.snap.phase.load(Ordering::Acquire);
    let mono = match snap_phase {
        2 => true,
        _ => params.mono_active.value(),
    };
    let delta = match snap_phase {
        3 => true,
        _ => params.delta_active.value(),
    };
    let listen = params.listen_active.value();
    let auto_gain = params.auto_gain_active.value();

    let is_measuring = params.shared.auto_loud.measuring.load(Ordering::Acquire);

    ProcessControl {
        sample_rate,
        mono_maker_freq,
        s_low,
        s_bass,
        s_mid,
        s_high_mid,
        s_high,
        bypass,
        snap_phase,
        mono,
        delta,
        listen,
        auto_gain,
        is_measuring,
    }
}
fn process_block(
    state: &mut EquilibriumDspState,
    params: &EquilibriumParams,
    buffer: &mut AudioBuffer<'_, f32>,
    ctrl: &mut ProcessControl,
) -> BlockMetrics {
    let sample_rate = ctrl.sample_rate;
    let mono_maker_freq = ctrl.mono_maker_freq;
    let s_low = ctrl.s_low;
    let s_bass = ctrl.s_bass;
    let s_mid = ctrl.s_mid;
    let s_high_mid = ctrl.s_high_mid;
    let s_high = ctrl.s_high;
    let bypass = ctrl.bypass;
    let mut snap_phase = ctrl.snap_phase;
    let mono = ctrl.mono;
    let delta = ctrl.delta;
    let auto_gain = ctrl.auto_gain;
    let is_measuring = ctrl.is_measuring;
    let mut max_out_peak = 0.0f32;
    let mut max_out_peak_l = 0.0f32;
    let mut max_out_peak_r = 0.0f32;
    let mut sum_power_in = 0.0f32;
    let mut sum_power_out = 0.0f32;
    let mut sum_power_l = 0.0f32;
    let mut sum_power_r = 0.0f32;
    let mut count_samples: usize = 0;

    let mut block_band_power = [0.0f32; 5];
    let mut block_input_band_power = [0.0f32; 5];

    // Copy I/O — AURA buffer has separate input/output borrows (no dual-mut io()).
    let num_samples = buffer.num_samples();
    let in0: Vec<f32> = buffer.input(0).to_vec();
    let in1: Vec<f32> = buffer.input(1).to_vec();
    let mut out0 = vec![0.0f32; num_samples];
    let mut out1 = vec![0.0f32; num_samples];

    // Feed input to LUFS meter BEFORE we modify the buffer
    if is_measuring {
        state.auto_loud_in.feed(&in0, &in1);
    }

    for i in 0..num_samples {
        count_samples += 1;
        let in_l = in0[i];
        let in_r = in1[i];

        sum_power_in += in_l * in_l + in_r * in_r;

        // HP @8 Hz always, LP @35 kHz only at ≥ 88.2 kHz
        let dc_l = state.low_cut_l.process(in_l);
        let dc_r = state.low_cut_r.process(in_r);
        let cut_l = if sample_rate >= 88_200.0 {
            state.high_cut_l.process(dc_l)
        } else {
            dc_l
        };
        let cut_r = if sample_rate >= 88_200.0 {
            state.high_cut_r.process(dc_r)
        } else {
            dc_r
        };

        // Crossover tree
        let (low_group_l, high_group_l) = state.xo_bass_mid_l.process_transparent(cut_l);
        let (band1_l, band2_l) = state.xo_low_bass_l.process_transparent(low_group_l);
        let (mid_group_l, super_high_group_l) =
            state.xo_mid_high_l.process_transparent(high_group_l);
        let (band3_l, band4_l_pre) = (mid_group_l, super_high_group_l);
        let (band4_l, band5_l) = state.xo_highmid_high_l.process_transparent(band4_l_pre);

        let (low_group_r, high_group_r) = state.xo_bass_mid_r.process_transparent(cut_r);
        let (band1_r, band2_r) = state.xo_low_bass_r.process_transparent(low_group_r);
        let (mid_group_r, super_high_group_r) =
            state.xo_mid_high_r.process_transparent(high_group_r);
        let (band3_r, band4_r_pre) = (mid_group_r, super_high_group_r);
        let (band4_r, band5_r) = state.xo_highmid_high_r.process_transparent(band4_r_pre);

        let mut bands_l = [band1_l, band2_l, band3_l, band4_l, band5_l];
        let mut bands_r = [band1_r, band2_r, band3_r, band4_r, band5_r];

        let band_gains = [
            db_to_gain(params.low_gain.value()),
            db_to_gain(params.bass_gain.value()),
            db_to_gain(params.mid_gain.value()),
            db_to_gain(params.high_mid_gain.value()),
            db_to_gain(params.high_gain.value()),
        ];
        let band_widths = [
            params.low_width.value() / 100.0,
            params.bass_width.value() / 100.0,
            params.mid_width.value() / 100.0,
            params.high_mid_width.value() / 100.0,
            params.high_width.value() / 100.0,
        ];
        let band_pans = [
            params.low_pan.value(),
            params.bass_pan.value(),
            params.mid_pan.value(),
            params.high_mid_pan.value(),
            params.high_pan.value(),
        ];
        let band_solos = [s_low, s_bass, s_mid, s_high_mid, s_high];

        for b in 0..BAND_COUNT {
            let bl = bands_l[b];
            let br = bands_r[b];

            // Pre-EQ input band power for LISTEN analysis
            let input_power = (bl * bl + br * br) * 0.5;
            block_input_band_power[b] += input_power;

            let mut bl_g = bl * band_gains[b];
            let mut br_g = br * band_gains[b];

            // M/S Width
            let mid = (bl_g + br_g) * 0.5;
            let side = (bl_g - br_g) * 0.5;
            let width_scale = if band_widths[b] > 1.0 {
                match b {
                    0 => 1.0 + (band_widths[b] - 1.0) * 0.25,
                    1 => 1.0 + (band_widths[b] - 1.0) * 0.65,
                    _ => band_widths[b],
                }
            } else {
                band_widths[b]
            };
            let side_w = side * width_scale;
            let width_norm = 1.0 / (1.0 + (width_scale - 1.0).max(0.0) * 0.20);

            // Constant-power pan with center normalization
            let pan_val = band_pans[b].clamp(-1.0, 1.0);
            let pan_angle = (pan_val + 1.0) * FRAC_PI_4;
            let raw_l = pan_angle.cos();
            let raw_r = pan_angle.sin();
            let max_raw = raw_l.max(raw_r);
            let pan_norm = if max_raw > 0.001 { 1.0 / max_raw } else { 1.0 };
            let pan_l = raw_l * pan_norm;
            let pan_r = raw_r * pan_norm;

            bl_g = (mid + side_w) * pan_l * width_norm;
            br_g = (mid - side_w) * pan_r * width_norm;

            // Band power post-EQ (pre-solo)
            let band_power = (bl_g * bl_g + br_g * br_g) * 0.5;
            block_band_power[b] += band_power;

            if !band_solos[b] {
                bl_g = 0.0;
                br_g = 0.0;
            }

            bands_l[b] = bl_g;
            bands_r[b] = br_g;
        }

        let mut out_l = bands_l[0] + bands_l[1] + bands_l[2] + bands_l[3] + bands_l[4];
        let mut out_r = bands_r[0] + bands_r[1] + bands_r[2] + bands_r[3] + bands_r[4];

        // Mono Floor (Side HPF)
        if mono_maker_freq > 1.0 {
            let out_mid = (out_l + out_r) * 0.5;
            let out_side = (out_l - out_r) * 0.5;
            let out_side_filtered = state.mono_floor_filter.process(out_side);
            out_l = out_mid + out_side_filtered;
            out_r = out_mid - out_side_filtered;
        }

        if mono {
            let m = (out_l + out_r) * 0.5;
            out_l = m;
            out_r = m;
        }

        let mut processed_l = out_l;
        let mut processed_r = out_r;

        if delta {
            processed_l = out_l - cut_l;
            processed_r = out_r - cut_r;
        }

        let out_gain = db_to_gain(params.output_gain.value());
        processed_l *= out_gain;
        processed_r *= out_gain;

        if auto_gain {
            processed_l *= state.auto_gain_comp;
            processed_r *= state.auto_gain_comp;
        }

        // Safety clamp
        processed_l = processed_l.clamp(-1.0, 1.0);
        processed_r = processed_r.clamp(-1.0, 1.0);

        sum_power_out += processed_l * processed_l + processed_r * processed_r;
        sum_power_l += processed_l * processed_l;
        sum_power_r += processed_r * processed_r;

        if bypass {
            out0[i] = in_l;
            out1[i] = in_r;
        } else {
            max_out_peak = max_out_peak.max(processed_l.abs()).max(processed_r.abs());
            max_out_peak_l = max_out_peak_l.max(processed_l.abs());
            max_out_peak_r = max_out_peak_r.max(processed_r.abs());
            out0[i] = processed_l;
            out1[i] = processed_r;
        }

        let (output_l, output_r) = if bypass {
            (in_l, in_r)
        } else {
            (processed_l, processed_r)
        };

        // Correlation
        let corr_lr = output_l * output_r;
        let corr_l2 = output_l * output_l;
        let corr_r2 = output_r * output_r;
        state.corr_avg_lr = (1.0 - state.correlation_decay_coef) * state.corr_avg_lr
            + state.correlation_decay_coef * corr_lr;
        state.corr_avg_l2 = (1.0 - state.correlation_decay_coef) * state.corr_avg_l2
            + state.correlation_decay_coef * corr_l2;
        state.corr_avg_r2 = (1.0 - state.correlation_decay_coef) * state.corr_avg_r2
            + state.correlation_decay_coef * corr_r2;

        // SNAP FFT capture
        if snap_phase > 0 {
            let sample = match snap_phase {
                1 | 2 => (output_l + output_r) * 0.5,
                3 => {
                    let out_mono = (output_l + output_r) * 0.5;
                    let in_mono = (in_l + in_r) * 0.5;
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

    ctrl.snap_phase = snap_phase;

    buffer.output(0).copy_from_slice(&out0);
    buffer.output(1).copy_from_slice(&out1);

    BlockMetrics {
        max_out_peak,
        max_out_peak_l,
        max_out_peak_r,
        sum_power_in,
        sum_power_out,
        sum_power_l,
        sum_power_r,
        count_samples,
        block_band_power,
        block_input_band_power,
        num_samples,
    }
}
fn publish(
    state: &mut EquilibriumDspState,
    params: &EquilibriumParams,
    buffer: &mut AudioBuffer<'_, f32>,
    ctrl: &ProcessControl,
    metrics: &mut BlockMetrics,
) {
    let sample_rate = ctrl.sample_rate;
    let listen = ctrl.listen;
    let auto_gain = ctrl.auto_gain;
    let is_measuring = ctrl.is_measuring;
    // Sample-loop peaks superseded by final-buffer metering after PRE-MASTER.
    let _ = (
        metrics.max_out_peak,
        metrics.max_out_peak_l,
        metrics.max_out_peak_r,
    );
    let sum_power_in = metrics.sum_power_in;
    let sum_power_out = metrics.sum_power_out;
    let sum_power_l = metrics.sum_power_l;
    let sum_power_r = metrics.sum_power_r;
    let count_samples = metrics.count_samples;
    let mut block_band_power = metrics.block_band_power;
    let mut block_input_band_power = metrics.block_input_band_power;
    let num_samples = metrics.num_samples;

    // Copy outs for post-process (pre-master, scope, auto-loud out) — AURA
    // buffer has separate input/output borrows (no dual-mut io()).
    let mut out0: Vec<f32> = buffer.output(0).to_vec();
    let mut out1: Vec<f32> = buffer.output(1).to_vec();
    // Pink noise carries equal energy per octave, so a band's total power
    // scales with its octave span — wider bands read hotter and the display
    // stair-steps upward. Divide each band's power by its octave width so a
    // pink-noise reference lands on a flat spectrum. Applied to both the
    // post-EQ meter and the pre-EQ LISTEN power so every downstream reading
    // (bars, listen, min/max, learned targets) stays per-octave consistent.
    // ponytail: SUB_LO_HZ is the Sub band's lower edge — empirically the pink
    // test source is band-limited near 20 Hz (not the 8 Hz DC highpass), so
    // 20 lands Sub flat. Air's upper edge is Nyquist, so band 4 is
    // sample-rate dependent. CAL_TRIM_DB is the residual per-band calibration
    // left after the octave model: measured by feeding pink noise and reading
    // the bars flat. Air runs ~1 dB hot from LR2 (12 dB/oct) skirt overlap.
    // Retune both only if a pink reference no longer sits flat.
    const SUB_LO_HZ: f32 = 20.0;
    const CAL_TRIM_DB: [f32; 5] = [0.0, 0.0, 0.0, 0.0, -1.0];
    let band_octaves = [
        (80.0f32 / SUB_LO_HZ).log2(),
        (300.0f32 / 80.0).log2(),
        (2000.0f32 / 300.0).log2(),
        (6000.0f32 / 2000.0).log2(),
        (sample_rate * 0.5 / 6000.0).log2(),
    ];
    for b in 0..BAND_COUNT {
        // Fold octave width + calibration trim into one power divisor.
        // trim adds dB to the reading, so divide power by 10^(-trim/10).
        let div = band_octaves[b] * 10f32.powf(-CAL_TRIM_DB[b] / 10.0);
        block_band_power[b] /= div;
        block_input_band_power[b] /= div;
    }

    let sample_weight = 1.0 / count_samples as f32;
    let buf_coef = 1.0 - (-(num_samples as f32) / (0.1 * sample_rate)).exp();

    // Stereo balance smoothing
    let avg_power_l = sum_power_l * sample_weight;
    let avg_power_r = sum_power_r * sample_weight;
    state.smoothed_power_l = (1.0 - buf_coef) * state.smoothed_power_l + buf_coef * avg_power_l;
    state.smoothed_power_r = (1.0 - buf_coef) * state.smoothed_power_r + buf_coef * avg_power_r;
    let rms_l = state.smoothed_power_l.sqrt();
    let rms_r = state.smoothed_power_r.sqrt();
    let sum_rms = rms_l + rms_r;
    let balance = if sum_rms > 1e-6 {
        (rms_l - rms_r) / sum_rms
    } else {
        0.0
    };
    params
        .shared
        .peaks
        .balance
        .store(balance, Ordering::Release);

    // Band power → dB
    for b in 0..BAND_COUNT {
        let average_band_power = block_band_power[b] * sample_weight;
        state.smoothed_band_power[b] =
            (1.0 - buf_coef) * state.smoothed_band_power[b] + buf_coef * average_band_power;
        let band_db = gain_to_db(state.smoothed_band_power[b].sqrt());
        params.shared.band_levels[b].store(band_db, Ordering::Release);

        if listen {
            let pow = block_input_band_power[b] as f64;
            state.listen_band_power_sum[b] += pow;

            let input_avg_pow = block_input_band_power[b] * sample_weight;
            let input_avg_f64 = input_avg_pow as f64;

            state.listen_ref_ema[b] = 0.01 * input_avg_f64 + 0.99 * state.listen_ref_ema[b];
            let gate = (state.listen_ref_ema[b] * 0.01).max(1e-6);

            if input_avg_f64 > gate {
                if !state.listen_lo_ema[b].is_finite() {
                    state.listen_lo_ema[b] = input_avg_f64;
                    state.listen_hi_ema[b] = input_avg_f64;
                } else {
                    if input_avg_f64 < state.listen_lo_ema[b] {
                        state.listen_lo_ema[b] += 0.15 * (input_avg_f64 - state.listen_lo_ema[b]);
                    } else {
                        state.listen_lo_ema[b] += 0.02 * (input_avg_f64 - state.listen_lo_ema[b]);
                    }
                    if input_avg_f64 > state.listen_hi_ema[b] {
                        state.listen_hi_ema[b] += 0.15 * (input_avg_f64 - state.listen_hi_ema[b]);
                    } else {
                        state.listen_hi_ema[b] += 0.02 * (input_avg_f64 - state.listen_hi_ema[b]);
                    }
                }
            }
        }
    }

    // Listen analysis post-processing
    if listen {
        state.listen_sample_count += count_samples as u64;
        params
            .shared
            .listen_samples
            .store(state.listen_sample_count as f32, Ordering::Release);

        if state.listen_sample_count > 0 {
            let div = 1.0 / state.listen_sample_count as f64;
            for b in 0..BAND_COUNT {
                let avg_pow = state.listen_band_power_sum[b] * div;
                let lo_pow = if state.listen_lo_ema[b].is_finite() {
                    state.listen_lo_ema[b]
                } else {
                    avg_pow
                };
                let hi_pow = if state.listen_hi_ema[b].is_finite() {
                    state.listen_hi_ema[b]
                } else {
                    avg_pow
                };

                let avg_db = gain_to_db((avg_pow as f32).sqrt());
                let lo_db = gain_to_db((lo_pow.max(1e-10) as f32).sqrt());
                let hi_db = gain_to_db((hi_pow.max(1e-10) as f32).sqrt());

                const ALPHA: f32 = 0.2;
                state.listen_levels_ema[b] =
                    ALPHA * avg_db + (1.0 - ALPHA) * state.listen_levels_ema[b];
                let listen_tolerance = (hi_db - lo_db) * 0.5;

                params.shared.listen_levels[b].store(state.listen_levels_ema[b], Ordering::Release);
                params.shared.listen_level_min[b].store(lo_db, Ordering::Release);
                params.shared.listen_level_max[b].store(hi_db, Ordering::Release);
                params.shared.listen_tolerances[b].store(listen_tolerance, Ordering::Release);
            }
        }
    } else if state.listen_sample_count > 0 {
        state.listen_sample_count = 0;
        params.shared.listen_samples.store(0.0, Ordering::Release);
        for b in 0..BAND_COUNT {
            state.listen_band_power_sum[b] = 0.0;
            state.listen_lo_ema[b] = f64::INFINITY;
            state.listen_hi_ema[b] = f64::NEG_INFINITY;
            state.listen_ref_ema[b] = 0.0;
            params.shared.listen_tolerances[b].store(0.0, Ordering::Release);
        }
    }

    // Correlation
    let den = (state.corr_avg_l2 * state.corr_avg_r2).sqrt();
    let correlation = if den > 1e-9 {
        state.corr_avg_lr / den
    } else {
        1.0
    };
    params
        .shared
        .peaks
        .phase_correlation
        .store(correlation.clamp(-1.0, 1.0), Ordering::Release);

    // Auto gain
    if auto_gain && count_samples > 0 {
        let avg_power_in = sum_power_in * sample_weight;
        let avg_power_out = sum_power_out * sample_weight;
        if avg_power_out > 1e-9 && avg_power_in > 1e-9 {
            let ratio = (avg_power_in / avg_power_out).sqrt();
            state.auto_gain_comp = 0.95 * state.auto_gain_comp + 0.05 * ratio;
        } else {
            state.auto_gain_comp = 1.0;
        }
    } else {
        state.auto_gain_comp = 1.0;
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
        state.auto_loud_out.reset();
    }
    if is_measuring {
        state.auto_loud_out.feed(&out0, &out1);
        let target_samples = (5.0 * sample_rate as f64) as u64;
        if state.auto_loud_out.sample_count() >= target_samples {
            let in_lufs = state.auto_loud_in.loudness_db();
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

    // PRE-MASTER (gain on final buffer; peaks published once after this)
    if params.pre_master_active.value() {
        let target_linear = db_to_gain(params.pre_master_target_db.raw_target() as f32);
        let n = out0.len().min(out1.len());
        let sr_safe = if sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        // Peak catch window — was 200ms (too short for sparse transients).
        // 2s ≈ safer PRE-MASTER gain; still much shorter than AUTO LOUD (5s LUFS).
        let measure_samples = (2.0 * sr_safe) as u32;

        if !state.pre_master_active_prev {
            state.pre_master_measure_peak = 0.0;
            state.pre_master_measure_count = 0;
            state.pre_master_gain = 1.0;
            state.pre_master_active_prev = true;
            // Fresh holds for the new gain stage
            state.peak_hold_value = MINUS_INF_DB;
            state.peak_hold_l_value = MINUS_INF_DB;
            state.peak_hold_r_value = MINUS_INF_DB;
        }

        if state.pre_master_measure_count < measure_samples {
            let mut block_peak = 0.0f32;
            for i in 0..n {
                block_peak = block_peak.max(out0[i].abs()).max(out1[i].abs());
            }
            state.pre_master_measure_peak = state.pre_master_measure_peak.max(block_peak);
            state.pre_master_measure_count += n as u32;
        }

        if state.pre_master_measure_count >= measure_samples && state.pre_master_gain == 1.0 {
            let gate = db_to_gain(-50.0);
            if state.pre_master_measure_peak > gate {
                let max_boost = db_to_gain(12.0);
                let max_cut = db_to_gain(-24.0);
                state.pre_master_gain =
                    (target_linear / state.pre_master_measure_peak).clamp(max_cut, max_boost);
                // Gain just engaged — drop pre-measure holds
                state.peak_hold_value = MINUS_INF_DB;
                state.peak_hold_l_value = MINUS_INF_DB;
                state.peak_hold_r_value = MINUS_INF_DB;
            } else {
                state.pre_master_measure_count = 0;
                state.pre_master_measure_peak = 0.0;
            }
        }

        for i in 0..n {
            out0[i] *= state.pre_master_gain;
            out1[i] *= state.pre_master_gain;
        }
    } else {
        if state.pre_master_active_prev {
            state.peak_hold_value = MINUS_INF_DB;
            state.peak_hold_l_value = MINUS_INF_DB;
            state.peak_hold_r_value = MINUS_INF_DB;
        }
        state.pre_master_gain = 1.0;
        state.pre_master_active_prev = false;
    }

    // Peak meters from final output (post PRE-MASTER / bypass path)
    {
        let n = out0.len().min(out1.len());
        let mut peak = 0.0f32;
        let mut peak_l = 0.0f32;
        let mut peak_r = 0.0f32;
        for i in 0..n {
            let al = out0[i].abs();
            let ar = out1[i].abs();
            peak = peak.max(al).max(ar);
            peak_l = peak_l.max(al);
            peak_r = peak_r.max(ar);
        }
        let block_peak_db = gain_to_db(peak);
        params
            .shared
            .peaks
            .output_peak
            .store(block_peak_db, Ordering::Release);
        if block_peak_db > state.peak_hold_value {
            state.peak_hold_value = block_peak_db;
        }
        params
            .shared
            .peaks
            .peak_hold
            .store(state.peak_hold_value, Ordering::Release);

        let peak_l_db = gain_to_db(peak_l);
        let peak_r_db = gain_to_db(peak_r);
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
        if peak_l_db > state.peak_hold_l_value {
            state.peak_hold_l_value = peak_l_db;
        }
        if peak_r_db > state.peak_hold_r_value {
            state.peak_hold_r_value = peak_r_db;
        }
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
    }

    // Goniometer scope buffer
    {
        let n = out0.len().min(out1.len());
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

    buffer.output(0).copy_from_slice(&out0);
    buffer.output(1).copy_from_slice(&out1);
}
