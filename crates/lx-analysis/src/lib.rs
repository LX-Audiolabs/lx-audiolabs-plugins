pub mod dev_log;
pub mod shared_state;
pub mod snap_fft;
pub use shared_state::{
    AetherShared, AurumShared, AutoLoud, EquilibriumShared, LucentShared, MeridianShared,
    PeakMeters, ScopeRing, ShmClaimShared, SnapPipeline, SpectrumView,
};
pub use snap_fft::{SnapFFT, SnapMode};

// Re-export lx-shm transparently so existing callers keep working
pub use lx_shm as shm;
pub use lx_shm::{
    display_name, now_ms, relay_hub, resolve_from_consumers, resolve_relay_target, RelayHub,
    EQ_BANDS, MAX_CONSUMERS, MAX_NAME_LEN, MAX_SLOTS, SPECTRUM_BINS, STALE_MS,
};

// Re-export vault/preset/config types so existing callers don't need to change imports
pub use lx_vault::{
    export_preset_to_markdown, get_plugin_dir, list_custom_presets, load_config,
    preset_plugin_name, save_config, PluginConfig, Profile, DEFAULT_TOLERANCES,
};

pub const SCOPE_BUFFER_LEN: usize = 4096;

/// Pre-clipper waveform ring (signed linear samples) — Aurum SHAPE clipper display.
/// Ring length alone ≈340 ms @ 48 kHz of *slots*; Aurum peak-hold-hops writes so
/// the visible window is longer without EMA (peaks stay exact within each hop).
pub const CLIP_WAVE_LEN: usize = 16384;

#[derive(Clone, Default)]
pub struct ClipWaveRing {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub mid: Vec<f32>,
    pub side: Vec<f32>,
}

impl ClipWaveRing {
    pub fn new() -> Self {
        Self {
            l: vec![0.0; CLIP_WAVE_LEN],
            r: vec![0.0; CLIP_WAVE_LEN],
            mid: vec![0.0; CLIP_WAVE_LEN],
            side: vec![0.0; CLIP_WAVE_LEN],
        }
    }
}

/// Sub-pixel scroll phase within the newest min/max bucket (0..1).
pub fn clip_wave_scroll_phase(write_pos: usize, ring_len: usize, cols: usize) -> f32 {
    let spp = (ring_len / cols.max(1)).max(1);
    (write_pos % spp) as f32 / spp as f32
}

/// Chronological min/max buckets for filled waveform display (oldest left, newest right).
pub fn clip_wave_minmax_window(ring: &[f32], write_pos: usize, cols: usize) -> Vec<(f32, f32)> {
    let len = ring.len();
    if len == 0 || cols == 0 {
        return Vec::new();
    }
    let cols = cols.min(len);
    let start = write_pos.wrapping_sub(len) % len;
    let spp = (len / cols).max(1);
    (0..cols)
        .map(|col| {
            let i0 = col * spp;
            let i1 = if col + 1 == cols {
                len
            } else {
                ((col + 1) * spp).min(len)
            };
            let mut min = f32::MAX;
            let mut max = f32::MIN;
            for i in i0..i1 {
                let v = ring[(start + i) % len];
                min = min.min(v);
                max = max.max(v);
            }
            if min == f32::MAX {
                (0.0, 0.0)
            } else {
                (min, max)
            }
        })
        .collect()
}

#[cfg(test)]
mod clip_wave_tests {
    use super::*;

    #[test]
    fn minmax_window_buckets_signed_samples() {
        let ring = vec![1.0, -0.5, 0.8, -1.0, 0.0, 0.2];
        let out = clip_wave_minmax_window(&ring, 6, 3);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], (-0.5, 1.0));
        assert_eq!(out[1], (-1.0, 0.8));
        assert_eq!(out[2], (0.0, 0.2));
    }

    #[test]
    fn scroll_phase_wraps_within_bucket() {
        assert_eq!(clip_wave_scroll_phase(0, 8192, 320), 0.0);
        assert!((clip_wave_scroll_phase(13, 8192, 320) - 13.0 / 25.0).abs() < 1e-5);
    }
}

/// Raw dB above which display tilt is applied in [`compute_spectrum_bins`].
/// Bins at the -90 floor (true digital silence) stay un-tilted so silence is
/// not boosted; everything above gets tilted, matching SPAN's slope behavior.
pub const SPECTRUM_TILT_RAW_GATE_DB: f32 = -90.0;

/// 4.5 dB/octave display tilt at `freq` (0 below 20 Hz).
#[inline]
pub fn spectrum_tilt_db(freq: f32) -> f32 {
    if freq > 20.0 {
        4.5 * (freq / 1000.0).log2()
    } else {
        0.0
    }
}

/// Physical (pre-tilt) dB underlying a display bin — undoes tilt when applied.
#[inline]
pub fn spectrum_physical_db(displayed_db: f32, freq: f32) -> f32 {
    if displayed_db > SPECTRUM_TILT_RAW_GATE_DB {
        (displayed_db - spectrum_tilt_db(freq)).max(-90.0)
    } else {
        displayed_db
    }
}

/// Compute display-ready spectrum bins from raw FFT output.
/// Applies 4.5 dB/octave tilt compensation so pink noise appears flat.
/// `fft_output` = complex FFT bins (RealFft half-spectrum).
/// `frame` = output slice of length SPECTRUM_BINS, filled with dB values.
#[inline]
pub fn compute_spectrum_bins(
    fft_output: &[realfft::num_complex::Complex<f32>],
    frame: &mut [f32],
    fft_size: usize,
    sample_rate: f32,
) {
    let inv_norm = 2.0 / fft_size as f32;
    for (k, slot) in frame.iter_mut().enumerate() {
        let mag = fft_output[k].norm() * inv_norm;
        let db = if mag > 1e-9 {
            20.0 * mag.log10()
        } else {
            -90.0
        };
        let freq = k as f32 * sample_rate / fft_size as f32;
        let tilt = if db > SPECTRUM_TILT_RAW_GATE_DB {
            spectrum_tilt_db(freq)
        } else {
            0.0
        };
        *slot = (db + tilt).clamp(-90.0, 12.0);
    }
}

// Per-plugin GUI shared state: `shared_state` module (AetherShared, …).

/// Editor-driving flag for [`LucentShared::relay_active_mask`].
/// Without this bit the mask is "no preference" (all slots active) — even when
/// the low bits are zero. That way "user turned every relay off" (driven +
/// bits=0) is distinct from "editor closed / never set a mask" (0).
pub const RELAY_MASK_DRIVEN: u32 = 1u32 << 31;

/// Whether SHM publisher `slot` is enabled under the editor's relay mask.
#[inline]
pub fn relay_slot_active(mask: u32, slot: u8) -> bool {
    if mask & RELAY_MASK_DRIVEN == 0 {
        return true;
    }
    mask & (1u32 << slot) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_slot_active_respects_bits() {
        // No driven bit → all pass (editor not driving).
        assert!(relay_slot_active(0, 0));
        assert!(relay_slot_active(0, 2));
        // Driven + bit 2 only → slot 2 on, slot 0 off.
        let mask = (1 << 2) | RELAY_MASK_DRIVEN;
        assert!(!relay_slot_active(mask, 0));
        assert!(relay_slot_active(mask, 2));
        // Driven + zero bits → none pass (user disabled every relay).
        assert!(!relay_slot_active(RELAY_MASK_DRIVEN, 0));
        assert!(!relay_slot_active(RELAY_MASK_DRIVEN, 2));
    }
}
