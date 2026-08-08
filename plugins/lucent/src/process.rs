//! Lucent process path — RT polish from dev 1.0.0:
//! FtzDazGuard, relay_scratch / read_active_into, masking id path,
//! EMA α=1/6, no SnapFFT on audio thread.
//! Peak/resonance/masking publish uses reused scratch + in-place registries.
//!
//! ponytail: same-crate module split — behavior matches lx-audiolabs-dev lucent.

use std::sync::atomic::Ordering;
use aura::prelude::*;
use lx_analysis::{relay_hub, SPECTRUM_BINS};
use aura_dsp::fx::FtzDazGuard;

use crate::{
    attribute_contributors_into, gain_to_db, power_sum_named_into, publish_masking, publish_relays,
    publish_resonance, sensitivity_thresholds, LucentDspState, LucentParams,
};

pub(crate) fn run(
    state: &mut LucentDspState,
    params: &LucentParams,
    buffer: &mut AudioBuffer<'_, f32>,
) -> ProcessStatus {
    let _ftz = FtzDazGuard::new();
    let fft_size = state.fft_input.len();
    let now_ms = lx_analysis::shm::now_ms();

    state.ensure_consumer_slot(params, now_ms);
    state.publish_consumer_name(params, now_ms);

    // Reset peak holds on request
    if params.shared.peaks.reset_peak.swap(false, Ordering::Release) {
        state.peak_hold_value = -100.0;
        state.peak_hold_l_value = -100.0;
        state.peak_hold_r_value = -100.0;
    }

    let mode = params.analyze_mode.value();

    // Pass-through: copy input → output. AURA's buffer has no dual-mut
    // io(), so inputs are copied to local Vecs before the output borrows.
    let n_ch = buffer.num_inputs().min(buffer.num_outputs());
    let n = buffer.num_samples();
    if n_ch == 0 || n == 0 {
        return ProcessStatus::Continue;
    }
    for ch in 0..n_ch {
        if buffer.input(ch).is_empty() {
            continue;
        }
        let inp = buffer.input(ch).to_vec();
        buffer.output(ch).copy_from_slice(&inp);
    }

    // Snapshot L/R from output (always holds the pass-through result).
    // ponytail: fixed stack cap — DAW blocks stay well under 8k.
    const MAX_BLOCK: usize = 8192;
    let n = n.min(MAX_BLOCK);
    let mut lbuf = [0.0f32; MAX_BLOCK];
    let mut rbuf = [0.0f32; MAX_BLOCK];
    lbuf[..n].copy_from_slice(&buffer.output(0)[..n]);
    if n_ch > 1 {
        rbuf[..n].copy_from_slice(&buffer.output(1)[..n]);
    } else {
        rbuf[..n].copy_from_slice(&lbuf[..n]);
    }

    // Analysis
    let sample_rate = state.sample_rate;
    let scope_len = lx_analysis::SCOPE_BUFFER_LEN;

    let mut max_out_l = 0.0f32;
    let mut max_out_r = 0.0f32;
    let mut sum_power_out_l = 0.0f32;
    let mut sum_power_out_r = 0.0f32;
    let mut sum_lr = 0.0f32;
    let mut sum_l2 = 0.0f32;
    let mut sum_r2 = 0.0f32;

    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        let in_l = lbuf[i];
        let in_r = rbuf[i];
        let mono_in = (in_l + in_r) * 0.5;

        max_out_l = max_out_l.max(in_l.abs());
        max_out_r = max_out_r.max(in_r.abs());
        sum_power_out_l += in_l * in_l;
        sum_power_out_r += in_r * in_r;
        sum_lr += in_l * in_r;
        sum_l2 += in_l * in_l;
        sum_r2 += in_r * in_r;

        state.fft_input[state.fft_write_pos] = mono_in;
        state.fft_write_pos += 1;

        if state.fft_write_pos >= fft_size {
            state.fft_write_pos = 0;

            for (d, (s, w)) in state
                .fft_windowed
                .iter_mut()
                .zip(state.fft_input.iter().zip(state.fft_hann.iter()))
            {
                *d = s * w;
            }

            if state
                .fft_fwd
                .process(&mut state.fft_windowed, &mut state.fft_output)
                .is_ok()
            {
                let n_bins = SPECTRUM_BINS;
                let mut frame = [0.0f32; SPECTRUM_BINS];
                lx_analysis::compute_spectrum_bins(
                    &state.fft_output,
                    &mut frame,
                    fft_size,
                    sample_rate,
                );
                let sensitivity =
                    sensitivity_thresholds(params.sensitivity.raw_target() as f32 / 100.0);

                match mode {
                    0 => {
                        // Standalone: no relay interaction — UI registry cleared.
                        publish_relays(state.instance_key, &[]);
                        state.peak_tracker.detect(&frame, &sensitivity, sample_rate);
                        publish_resonance(
                            state.instance_key,
                            state.peak_tracker.res_peaks(),
                            &[],
                        );
                        publish_masking(state.instance_key, &[]);
                        if let Ok(mut mm) = params.shared.masking_map.try_lock() {
                            mm.iter_mut().for_each(|m| *m = -90.0);
                        }
                        if let Ok(mut bins) = params.shared.spectrum.bins.try_lock() {
                            bins.copy_from_slice(&frame);
                        }
                        if let Ok(mut avg) = params.shared.spectrum.avg.try_lock() {
                            // Energy-gating: only update EMA if signal above -80 dB
                            let frame_energy =
                                frame.iter().map(|x| x * x).sum::<f32>() / n_bins as f32;
                            let energy_db = 10.0 * frame_energy.log10().max(-40.0);
                            let gate = energy_db > -80.0;
                            // α=1/6 per FFT hop ≈ 250 ms at 48 kHz — SPAN-like
                            // speed; was 49/50 (~2.1 s), which smeared
                            // transient highs into invisibility.
                            for k in 0..n_bins {
                                let input = if gate { frame[k] } else { 0.0 };
                                avg[k] = avg[k] * (5.0 / 6.0) + input * (1.0 / 6.0);
                            }
                        }
                    }
                    1 => {
                        state.peak_tracker.detect(&frame, &sensitivity, sample_rate);

                        let mask = params
                            .shared
                            .relay_active_mask
                            .load(Ordering::Acquire);
                        if let Some(hub) = relay_hub() {
                            state.relay_scratch_n = hub.read_active_into(
                                &state.cached_display_name,
                                now_ms,
                                &mut state.relay_scratch,
                            );
                        } else {
                            state.relay_scratch_n = 0;
                        }
                        let n_feeds = state.relay_scratch_n;

                        // Relay feeds for the editor (curves + toggle bar +
                        // "N relays online") — before the active-mask filter.
                        publish_relays(
                            state.instance_key,
                            &state.relay_scratch[..n_feeds],
                        );

                        // Group-level resonance: power-sum of Relay tracks (scratch buf).
                        power_sum_named_into(
                            &state.relay_scratch[..n_feeds],
                            mask,
                            &mut state.relay_sum_buf,
                        );
                        state.relay_peak_tracker.detect(
                            &state.relay_sum_buf,
                            &sensitivity,
                            sample_rate,
                        );
                        attribute_contributors_into(
                            state.relay_peak_tracker.res_peaks(),
                            &state.relay_scratch[..n_feeds],
                            mask,
                            &mut state.contrib_scratch,
                            &mut state.contrib_n,
                        );

                        publish_resonance(
                            state.instance_key,
                            state.peak_tracker.res_peaks(),
                            &state.contrib_scratch[..state.contrib_n],
                        );

                        state.masking_analyzer.compute_masking(
                            Some(&frame),
                            &state.relay_scratch[..n_feeds],
                            mask,
                            sensitivity.masking_floor_db,
                            sample_rate,
                            sensitivity.persistence_min,
                        );
                        // Full list for SNAP; UI still truncates when formatting text.
                        state
                            .masking_analyzer
                            .fill_peaks_above_floor(sensitivity.masking_floor_db);
                        publish_masking(
                            state.instance_key,
                            state.masking_analyzer.peaks_above_floor(),
                        );
                        if let Ok(mut mm) = params.shared.masking_map.try_lock() {
                            mm.copy_from_slice(&state.masking_analyzer.masking_map);
                        }
                        if let Ok(mut bins) = params.shared.spectrum.bins.try_lock() {
                            bins.copy_from_slice(&frame);
                        }
                        if let Ok(mut avg) = params.shared.spectrum.avg.try_lock() {
                            let frame_energy =
                                frame.iter().map(|x| x * x).sum::<f32>() / n_bins as f32;
                            let energy_db = 10.0 * frame_energy.log10().max(-40.0);
                            let gate = energy_db > -80.0;
                            // α=1/6 per FFT hop ≈ 250 ms at 48 kHz — SPAN-like
                            // speed; was 49/50 (~2.1 s), which smeared
                            // transient highs into invisibility.
                            for k in 0..n_bins {
                                let input = if gate { frame[k] } else { 0.0 };
                                avg[k] = avg[k] * (5.0 / 6.0) + input * (1.0 / 6.0);
                            }
                        }
                    }
                    _ => {
                        if let Ok(mut bins) = params.shared.spectrum.bins.try_lock() {
                            bins.iter_mut().for_each(|b| *b = -90.0);
                        }
                        if let Ok(mut avg) = params.shared.spectrum.avg.try_lock() {
                            avg.iter_mut().for_each(|b| *b = -90.0);
                        }
                        let mask = params
                            .shared
                            .relay_active_mask
                            .load(Ordering::Acquire);
                        if let Some(hub) = relay_hub() {
                            state.relay_scratch_n = hub.read_active_into(
                                &state.cached_display_name,
                                now_ms,
                                &mut state.relay_scratch,
                            );
                        } else {
                            state.relay_scratch_n = 0;
                        }
                        let n_feeds = state.relay_scratch_n;
                        publish_relays(
                            state.instance_key,
                            &state.relay_scratch[..n_feeds],
                        );

                        // RELAY mode: group resonance from Relay sum only.
                        power_sum_named_into(
                            &state.relay_scratch[..n_feeds],
                            mask,
                            &mut state.relay_sum_buf,
                        );
                        state.relay_peak_tracker.detect(
                            &state.relay_sum_buf,
                            &sensitivity,
                            sample_rate,
                        );
                        attribute_contributors_into(
                            state.relay_peak_tracker.res_peaks(),
                            &state.relay_scratch[..n_feeds],
                            mask,
                            &mut state.contrib_scratch,
                            &mut state.contrib_n,
                        );
                        publish_resonance(
                            state.instance_key,
                            &[],
                            &state.contrib_scratch[..state.contrib_n],
                        );

                        state.masking_analyzer.compute_masking(
                            None,
                            &state.relay_scratch[..n_feeds],
                            mask,
                            sensitivity.masking_floor_db,
                            sample_rate,
                            sensitivity.persistence_min,
                        );
                        state
                            .masking_analyzer
                            .fill_peaks_above_floor(sensitivity.masking_floor_db);
                        publish_masking(
                            state.instance_key,
                            state.masking_analyzer.peaks_above_floor(),
                        );
                        if let Ok(mut mm) = params.shared.masking_map.try_lock() {
                            mm.copy_from_slice(&state.masking_analyzer.masking_map);
                        }
                    }
                }
            }
        }
    }

    // Peak meters
    let peak_l_db = gain_to_db(max_out_l.max(1e-9));
    let peak_r_db = gain_to_db(max_out_r.max(1e-9));
    let peak_mono_db = peak_l_db.max(peak_r_db);
    params
        .shared
        .peaks.output_peak_l
        .store(peak_l_db, Ordering::Release);
    params
        .shared
        .peaks.output_peak_r
        .store(peak_r_db, Ordering::Release);
    params
        .shared
        .peaks.output_peak
        .store(peak_mono_db, Ordering::Release);
    if peak_l_db > state.peak_hold_l_value {
        state.peak_hold_l_value = peak_l_db;
    }
    if peak_r_db > state.peak_hold_r_value {
        state.peak_hold_r_value = peak_r_db;
    }
    if peak_mono_db > state.peak_hold_value {
        state.peak_hold_value = peak_mono_db;
    }
    params
        .shared
        .peaks.peak_hold_l
        .store(state.peak_hold_l_value, Ordering::Release);
    params
        .shared
        .peaks.peak_hold_r
        .store(state.peak_hold_r_value, Ordering::Release);
    params
        .shared
        .peaks.peak_hold
        .store(state.peak_hold_value, Ordering::Release);

    // Stereo balance + correlation
    if n > 0 {
        let sw = 1.0 / n as f32;
        let rms_l = (sum_power_out_l * sw).sqrt();
        let rms_r = (sum_power_out_r * sw).sqrt();
        let balance = if rms_l + rms_r > 1e-6 {
            (rms_l - rms_r) / (rms_l + rms_r)
        } else {
            0.0
        };
        params.shared.peaks.balance.store(balance, Ordering::Release);

        let corr = if sum_l2 > 1e-9 && sum_r2 > 1e-9 {
            sum_lr / (sum_l2.sqrt() * sum_r2.sqrt())
        } else {
            1.0
        };
        params
            .shared
            .peaks.phase_correlation
            .store(corr.clamp(-1.0, 1.0), Ordering::Release);
    }

    // Goniometer scope buffer — visual auto-gain envelope, same pattern
    // as Equilibrium/Meridian (5ms attack / 300ms release envelope
    // scaling samples to ~90% of the display), so Lucent's vectorscope
    // fills the same visual range as theirs instead of showing a tiny
    // raw-amplitude dot cluster at typical (well below full-scale) mix levels.
    {
        let start_pos = params.shared.scope.write_pos.load(Ordering::Acquire);
        if let Ok(mut scope) = params.shared.scope.samples.try_lock() {
            let buf_len = scope_len;
            let block_peak = (0..n)
                .map(|i| lbuf[i].abs().max(rbuf[i].abs()))
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
                let pos = (start_pos + i) % buf_len;
                scope[pos] = [lbuf[i] * vis_gain, rbuf[i] * vis_gain];
            }
            params
                .shared
                .scope.write_pos
                .store((start_pos + n) % buf_len, Ordering::Release);
        }
    }

    ProcessStatus::Continue
}
