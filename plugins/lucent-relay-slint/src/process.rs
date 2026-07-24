//! Lucent Relay process path split for profile/isolation.
//!
//! ponytail: same-crate module split only — no behavior change.

use truce::prelude::*;
use lx_analysis::compute_spectrum_bins;
use lx_dsp::FtzDazGuard;

use crate::{
    read_persisted, reconcile_stale_target, sync_live, FFT_SIZE, LucentRelayDspState,
    LucentRelayParams,
};

pub(crate) fn run(
    state: &mut LucentRelayDspState,
    params: &LucentRelayParams,
    buffer: &mut AudioBuffer,
) -> ProcessStatus {
    let _ftz = FtzDazGuard::new();

    let now_ms = lx_analysis::shm::now_ms();
    if state.claimed_slot.is_none() {
        state.claim_slot();
    }

    let (n, t) = read_persisted(params);
    if n != state.cached_name {
        state.cached_name = n;
    }
    if t != state.cached_target {
        state.cached_target = t;
    }
    reconcile_stale_target(params, &mut state.cached_target);
    sync_live(params);

    // Pass-through (copy input ÔåÆ output per channel)
    for ch in 0..buffer.channels() {
        let (inp, out) = buffer.io(ch);
        out.copy_from_slice(inp);
    }

    // FFT: read inputs via &self method to avoid double &mut borrow
    let n_samples = buffer.num_samples();
    for i in 0..n_samples {
        let l = buffer.input(0)[i];
        let r = if buffer.num_input_channels() > 1 {
            buffer.input(1)[i]
        } else {
            l
        };
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
    ProcessStatus::Normal
}
