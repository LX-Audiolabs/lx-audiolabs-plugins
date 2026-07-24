//! Lucent process path split for profile/isolation.
//!
//! ponytail: same-crate module split only — no behavior change.

use std::sync::atomic::Ordering;
use truce::prelude::*;

use lx_analysis::{filter_relays_by_mask, relay_hub, SnapMode, SPECTRUM_BINS};

use crate::{
    attribute_contributors, gain_to_db, power_sum_spectrum, publish_masking, publish_resonance,
    sensitivity_thresholds, suppress_harmonics, LucentDspState, LucentParams, ResonanceLists,
};

pub(crate) fn run(
    state: &mut LucentDspState,
    params: &LucentParams,
    buffer: &mut AudioBuffer,
) -> ProcessStatus {
    let fft_size = state.fft_input.len();
    let now_ms = lx_analysis::shm::now_ms();

    state.ensure_consumer_slot(params, now_ms);
    state.publish_consumer_name(params, now_ms);

    // Reset peak holds on request
    if params.shared.reset_peak.swap(false, Ordering::Release) {
        state.peak_hold_value = -100.0;
        state.peak_hold_l_value = -100.0;
        state.peak_hold_r_value = -100.0;
    }

    let mode = params.analyze_mode.value();
    let snap_phase = params.shared.snap_phase.load(Ordering::Acquire);

    // Pass-through: copy input to output
    for ch in 0..buffer.channels() {
        let (inp, out) = buffer.io(ch);
        out.copy_from_slice(inp);
    }

    // Analysis
    let n = buffer.num_samples();
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
        let in_l = buffer.input(0)[i];
        let in_r = buffer.input(1)[i];
        let mono_in = (in_l + in_r) * 0.5;

        // SNAP FFT (same pattern as Meridian/Equilibrium)
        if snap_phase > 0 {
            let sample = match snap_phase {
                1 | 2 => mono_in,
                3 => {
                    let in_mono = (in_l + in_r) * 0.5;
                    let out_mono = mono_in; // Lucent is pass-through, so out = in for SNAP
                    out_mono - in_mono // delta = 0 for pure analyzer
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
                if state.snap_fft.accumulate_snap(&frame, snap_phase, threshold) {
                    let mode_snap = match snap_phase {
                        1 => SnapMode::Stereo,
                        2 => SnapMode::Mono,
                        _ => SnapMode::Delta,
                    };
                    let snapshot = state.snap_fft.read_snapshot(mode_snap);
                    if let Ok(mut buf) = match mode_snap {
                        SnapMode::Stereo => params.shared.snap_stereo_snap.try_lock(),
                        SnapMode::Mono => params.shared.snap_mono_snap.try_lock(),
                        SnapMode::Delta => params.shared.snap_delta_snap.try_lock(),
                    } {
                        *buf = snapshot;
                    }
                    let next_phase = if snap_phase < 3 { snap_phase + 1 } else { 0 };
                    params
                        .shared
                        .snap_phase
                        .store(next_phase, Ordering::Release);
                    if next_phase == 0 {
                        params
                            .shared
                            .snap_active
                            .store(false, Ordering::Release);
                        state.snap_fft.reset_snapshots();
                    }
                }
            }
        }

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
                        let peaks =
                            state.peak_tracker
                                .find_peaks(&frame, &sensitivity, sample_rate);
                        let peaks = suppress_harmonics(&frame, peaks);
                        state.peak_tracker.update(&peaks);
                        let own_resonances = state.peak_tracker.resonance_peaks(&sensitivity);
                        publish_resonance(
                            state.instance_key,
                            ResonanceLists {
                                own: own_resonances,
                                relay: Vec::new(),
                            },
                        );
                        publish_masking(state.instance_key, Vec::new());
                        if let Ok(mut mm) = params.shared.masking_map.try_lock() {
                            mm.iter_mut().for_each(|m| *m = -90.0);
                        }
                        if let Ok(mut bins) = params.shared.spectrum_bins.try_lock() {
                            bins.copy_from_slice(&frame);
                        }
                        if let Ok(mut avg) = params.shared.spectrum_avg.try_lock() {
                            // Energy-gating: only update EMA if signal above -80 dB
                            let frame_energy =
                                frame.iter().map(|x| x * x).sum::<f32>() / n_bins as f32;
                            let energy_db = 10.0 * frame_energy.log10().max(-40.0);
                            let gate = energy_db > -80.0;
                            for k in 0..n_bins {
                                let input = if gate { frame[k] } else { 0.0 };
                                avg[k] = avg[k] * (49.0 / 50.0) + input * (1.0 / 50.0);
                            }
                        }
                    }
                    1 => {
                        let peaks =
                            state.peak_tracker
                                .find_peaks(&frame, &sensitivity, sample_rate);
                        let peaks = suppress_harmonics(&frame, peaks);
                        state.peak_tracker.update(&peaks);
                        let own_resonances = state.peak_tracker.resonance_peaks(&sensitivity);

                        let my_name = state.cached_display_name.clone();
                        let mask = params
                            .shared
                            .relay_active_mask
                            .load(Ordering::Acquire);
                        let relay_named: Vec<(String, Vec<f32>)> = relay_hub()
                            .map(|hub| {
                                filter_relays_by_mask(
                                    mask,
                                    hub.read_active(&my_name, now_ms),
                                )
                            })
                            .unwrap_or_default();
                        let relay_spectra: Vec<Vec<f32>> =
                            relay_named.iter().map(|(_, spec)| spec.clone()).collect();

                        // Group-level resonance: power-sum of the Relay tracks can show
                        // a buildup that no single track (nor this bus's own signal) has.
                        let relay_sum = power_sum_spectrum(&relay_spectra);
                        let relay_peaks = state.relay_peak_tracker.find_peaks(
                            &relay_sum,
                            &sensitivity,
                            sample_rate,
                        );
                        let relay_peaks = suppress_harmonics(&relay_sum, relay_peaks);
                        state.relay_peak_tracker.update(&relay_peaks);
                        let relay_resonances = attribute_contributors(
                            &state.relay_peak_tracker.resonance_peaks(&sensitivity),
                            &relay_named,
                        );

                        publish_resonance(
                            state.instance_key,
                            ResonanceLists {
                                own: own_resonances,
                                relay: relay_resonances,
                            },
                        );

                        state.masking_analyzer.compute_masking(
                            Some(&frame),
                            &relay_named,
                            sensitivity.masking_floor_db,
                            sample_rate,
                            sensitivity.persistence_min,
                        );
                        publish_masking(
                            state.instance_key,
                            state.masking_analyzer
                                .top_peaks(3, sensitivity.masking_floor_db),
                        );
                        if let Ok(mut mm) = params.shared.masking_map.try_lock() {
                            mm.copy_from_slice(&state.masking_analyzer.masking_map);
                        }
                        if let Ok(mut bins) = params.shared.spectrum_bins.try_lock() {
                            bins.copy_from_slice(&frame);
                        }
                        if let Ok(mut avg) = params.shared.spectrum_avg.try_lock() {
                            let frame_energy =
                                frame.iter().map(|x| x * x).sum::<f32>() / n_bins as f32;
                            let energy_db = 10.0 * frame_energy.log10().max(-40.0);
                            let gate = energy_db > -80.0;
                            for k in 0..n_bins {
                                let input = if gate { frame[k] } else { 0.0 };
                                avg[k] = avg[k] * (49.0 / 50.0) + input * (1.0 / 50.0);
                            }
                        }
                    }
                    _ => {
                        if let Ok(mut bins) = params.shared.spectrum_bins.try_lock() {
                            bins.iter_mut().for_each(|b| *b = -90.0);
                        }
                        if let Ok(mut avg) = params.shared.spectrum_avg.try_lock() {
                            avg.iter_mut().for_each(|b| *b = -90.0);
                        }
                        let my_name = state.cached_display_name.clone();
                        let mask = params
                            .shared
                            .relay_active_mask
                            .load(Ordering::Acquire);
                        let relay_named: Vec<(String, Vec<f32>)> = relay_hub()
                            .map(|hub| {
                                filter_relays_by_mask(
                                    mask,
                                    hub.read_active(&my_name, now_ms),
                                )
                            })
                            .unwrap_or_default();
                        let relay_spectra: Vec<Vec<f32>> =
                            relay_named.iter().map(|(_, spec)| spec.clone()).collect();

                        // RELAY mode: no own signal, so resonance is purely the
                        // Relay tracks "untereinander und zusammen" ÔÇö masking below
                        // covers "untereinander" (pairwise), this covers "zusammen".
                        let relay_sum = power_sum_spectrum(&relay_spectra);
                        let relay_peaks = state.relay_peak_tracker.find_peaks(
                            &relay_sum,
                            &sensitivity,
                            sample_rate,
                        );
                        let relay_peaks = suppress_harmonics(&relay_sum, relay_peaks);
                        state.relay_peak_tracker.update(&relay_peaks);
                        let relay_resonances = attribute_contributors(
                            &state.relay_peak_tracker.resonance_peaks(&sensitivity),
                            &relay_named,
                        );
                        publish_resonance(
                            state.instance_key,
                            ResonanceLists {
                                own: Vec::new(),
                                relay: relay_resonances,
                            },
                        );

                        state.masking_analyzer.compute_masking(
                            None,
                            &relay_named,
                            sensitivity.masking_floor_db,
                            sample_rate,
                            sensitivity.persistence_min,
                        );
                        publish_masking(
                            state.instance_key,
                            state.masking_analyzer
                                .top_peaks(3, sensitivity.masking_floor_db),
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
        .output_peak_l
        .store(peak_l_db, Ordering::Release);
    params
        .shared
        .output_peak_r
        .store(peak_r_db, Ordering::Release);
    params
        .shared
        .output_peak
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
        .peak_hold_l
        .store(state.peak_hold_l_value, Ordering::Release);
    params
        .shared
        .peak_hold_r
        .store(state.peak_hold_r_value, Ordering::Release);
    params
        .shared
        .peak_hold
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
        params.shared.balance.store(balance, Ordering::Release);

        let corr = if sum_l2 > 1e-9 && sum_r2 > 1e-9 {
            sum_lr / (sum_l2.sqrt() * sum_r2.sqrt())
        } else {
            1.0
        };
        params
            .shared
            .phase_correlation
            .store(corr.clamp(-1.0, 1.0), Ordering::Release);
    }

    // Goniometer scope buffer ÔÇö visual auto-gain envelope, same pattern
    // as Equilibrium/Meridian (5ms attack / 300ms release envelope
    // scaling samples to ~90% of the display), so Lucent's vectorscope
    // fills the same visual range as theirs instead of showing a tiny
    // raw-amplitude dot cluster at typical (well below full-scale) mix levels.
    {
        let start_pos = params.shared.scope_write_pos.load(Ordering::Acquire);
        if let Ok(mut scope) = params.shared.scope_samples.try_lock() {
            let buf_len = scope_len;
            let in0 = buffer.input(0);
            let in1 = buffer.input(1);
            let block_peak = (0..n)
                .map(|i| in0[i].abs().max(in1[i].abs()))
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
                scope[pos] = [in0[i] * vis_gain, in1[i] * vis_gain];
            }
            params
                .shared
                .scope_write_pos
                .store((start_pos + n) % buf_len, Ordering::Release);
        }
    }

    ProcessStatus::Normal
}
