//! Lucent Relay process path split for profile/isolation.
//!
//! ponytail: same-crate module split only — no behavior change.

use aura::prelude::*;
use aura_dsp::analysis::*;
use aura_dsp::fx::FtzDazGuard;
use lx_shm::*;

use crate::{FFT_SIZE, LucentRelayDspState, LucentRelayParams, read_persisted, sync_live};

pub(crate) fn run(
    state: &mut LucentRelayDspState,
    params: &LucentRelayParams,
    buffer: &mut AudioBuffer<'_, f32>,
) -> ProcessStatus {
    let _ftz = FtzDazGuard::new();

    let now_ms = now_ms();
    if state.claimed_slot.is_none() {
        state.claim_slot();
    }

    // Lazy state-load resync (truce `state_changed` parity — AURA has no such
    // hook): persisted name/target changes (host state load or editor edits
    // while bypassed) update the caches here, force a target re-resolve, and
    // re-sync the liveness mirror.
    let (n, t) = read_persisted(params);
    let mut changed = false;
    if n != state.cached_name {
        state.cached_name = n;
        changed = true;
    }
    let target_changed = t != state.cached_target;
    if target_changed {
        state.cached_target = t;
        changed = true;
    }
    if changed {
        sync_live(params);
    }
    state.resolve_target(params, now_ms, target_changed);

    // Pass-through (copy input → output per channel). AURA buffer has separate
    // input/output borrows (no dual-mut io()), so snapshot each input first;
    // the FFT below reads from those snapshots.
    let channels = buffer.num_inputs().min(buffer.num_outputs());
    let mut inputs: Vec<Vec<f32>> = Vec::with_capacity(channels);
    for ch in 0..channels {
        let src = buffer.input(ch).to_vec();
        buffer.output(ch).copy_from_slice(&src);
        inputs.push(src);
    }

    // FFT: read the input snapshots (buffer borrows released above).
    let n_samples = buffer.num_samples();
    if !inputs.is_empty() {
        for i in 0..n_samples {
            let l = inputs[0][i];
            let r = if inputs.len() > 1 { inputs[1][i] } else { l };
            state.fft_input[state.fft_write_pos] = (l + r) * 0.5;
            state.fft_write_pos += 1;

            if state.fft_write_pos >= FFT_SIZE {
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
                    compute_spectrum_bins(
                        &state.fft_output,
                        &mut state.fft_bins,
                        FFT_SIZE,
                        state.sample_rate,
                    );
                    state.publish_fft(now_ms);
                }
            }
        }
    }
    ProcessStatus::Continue
}
