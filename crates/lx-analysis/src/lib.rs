use atomic_float::AtomicF32;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU8, AtomicUsize};
use std::sync::{Arc, Mutex};

pub mod dev_log;
pub mod snap_fft;
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

/// Shared real-time analyzer values for the GUI.
///
/// ## Plugin ownership (ponytail: split into per-plugin state structs before
/// multi-plugin migration — current monolith works but gets painful fast)
///
/// ── Equilibrium ──
///   band_levels, target_levels, target_tolerances, listen_*,
///   selected_preset_index
///
/// ── Meridian ──
///   gain_reduction, EQ-curve fields (via params), reset_analysis,
///   snap_*, sample_rate, auto_loud_*
///
/// ── Aether ──
///   input_peak
///
/// ── All ──
///   phase_correlation, output_peak[_l,_r], peak_hold[_l,_r],
///   reset_peak, balance, spectrum_bins, spectrum_avg,
///   scope_samples, scope_write_pos
///
/// ── Lucent ──
///   masking_map, shm_slot, resonance (via resonance_hub)
pub struct SharedState {
    pub band_levels: [Arc<AtomicF32>; 5],
    pub target_levels: [Arc<AtomicF32>; 5],
    pub target_tolerances: [Arc<AtomicF32>; 5],
    pub listen_levels: [Arc<AtomicF32>; 5],
    pub listen_tolerances: [Arc<AtomicF32>; 5],
    pub listen_level_min: [Arc<AtomicF32>; 5],
    pub listen_level_max: [Arc<AtomicF32>; 5],
    pub listen_samples: Arc<AtomicF32>,
    pub phase_correlation: Arc<AtomicF32>,
    pub output_peak: Arc<AtomicF32>,
    pub peak_hold: Arc<AtomicF32>,
    /// Input peak (max |L|,|R| per block, dBFS) — for Aether's input reader. Fast
    /// value here; the editor latches the peak-hold (like Meridian's GR display).
    pub input_peak: Arc<AtomicF32>,
    /// Pre-clipper block peaks (dBFS) — metering fallback.
    pub clip_pre_peak_l: Arc<AtomicF32>,
    pub clip_pre_peak_r: Arc<AtomicF32>,
    pub clip_pre_peak_mid: Arc<AtomicF32>,
    pub clip_pre_peak_side: Arc<AtomicF32>,
    /// Pre-clipper signed-sample rings — waveform mini-displays.
    pub clip_wave: Arc<Mutex<ClipWaveRing>>,
    pub clip_wave_write_pos: Arc<AtomicUsize>,
    /// FFT EMA after clipper, Mid channel — Aurum Shape tab.
    pub spectrum_mid_avg: Arc<Mutex<Vec<f32>>>,
    /// FFT EMA after clipper, Side channel — Aurum Shape tab.
    pub spectrum_side_avg: Arc<Mutex<Vec<f32>>>,
    /// 2-band comp Lo-band GR (dB, block max) — Aurum Color tab.
    pub comp_gr_lo: Arc<AtomicF32>,
    /// 2-band comp Hi-band GR (dB, block max) — Aurum Color tab.
    pub comp_gr_hi: Arc<AtomicF32>,
    /// FFT EMA after sweetening, (L+R)*0.5 — Aurum Color tab.
    pub spectrum_sweet_avg: Arc<Mutex<Vec<f32>>>,
    /// MB limiter Mid-Lo GR (dB, positive) — Aurum Limit tab.
    pub mb_gr_mid_lo: Arc<AtomicF32>,
    /// MB limiter Mid-Hi GR (dB, positive) — Aurum Limit tab.
    pub mb_gr_mid_hi: Arc<AtomicF32>,
    /// MB limiter Side GR (dB, positive) — Aurum Limit tab.
    pub mb_gr_side: Arc<AtomicF32>,
    /// Integrated LUFS post-TP — Aurum Limit delivery meter.
    pub lufs_integrated: Arc<AtomicF32>,
    /// True-peak hold (dBTP) post-TP — Aurum Limit delivery meter.
    pub true_peak_dbtp: Arc<AtomicF32>,
    /// Loudness range LRA (LU) post-TP — Aurum Limit delivery meter. −1 = not ready.
    pub lra_lu: Arc<AtomicF32>,
    pub output_peak_l: Arc<AtomicF32>,
    pub output_peak_r: Arc<AtomicF32>,
    pub peak_hold_l: Arc<AtomicF32>,
    pub peak_hold_r: Arc<AtomicF32>,
    pub reset_peak: Arc<AtomicBool>,
    pub reset_analysis: Arc<AtomicBool>,
    pub gain_reduction: Arc<AtomicF32>,
    pub balance: Arc<AtomicF32>,
    /// UI sets true to start AUTO LOUD measurement
    pub auto_loud_trigger: Arc<AtomicBool>,
    /// Audio thread sets true while measuring, false when done
    pub auto_loud_measuring: Arc<AtomicBool>,
    /// Audio thread writes computed gain offset (dB) after measurement
    pub auto_loud_gain_offset: Arc<AtomicF32>,
    /// FFT magnitude spectrum — Sum (L+R)*0.5, SPECTRUM_BINS bins, dB with tilt
    pub spectrum_bins: Arc<Mutex<Vec<f32>>>,
    /// Exponential moving average of spectrum_bins (α=1/6 per FFT hop,
    /// ~250 ms at 48 kHz — fast enough to keep transient highs visible)
    pub spectrum_avg: Arc<Mutex<Vec<f32>>>,
    /// Ring buffer of [L, R] pairs for the goniometer/vectorscope display
    pub scope_samples: Arc<Mutex<Vec<[f32; 2]>>>,
    /// Write position in scope_samples ring buffer
    pub scope_write_pos: Arc<AtomicUsize>,
    /// Last selected preset index — survives editor close/reopen
    pub selected_preset_index: Arc<AtomicUsize>,
    /// True while SNAP export is running — GUI shows "ANALYZE..."
    pub snap_active: Arc<AtomicBool>,
    /// Sample rate set by audio thread — used by GUI for frequency labels in snapshots
    pub sample_rate: Arc<AtomicF32>,
    /// SNAP measurement phase: 0=idle, 1=stereo, 2=mono, 3=delta
    pub snap_phase: Arc<AtomicU8>,
    /// Spectrum snapshots captured at end of each SNAP phase
    pub snap_stereo_snap: Arc<Mutex<Vec<f32>>>,
    pub snap_mono_snap: Arc<Mutex<Vec<f32>>>,
    pub snap_delta_snap: Arc<Mutex<Vec<f32>>>,
    /// Masking collision map (dB per bin) — where own signal overlaps competing relay
    /// energy. Lucent only; -90 dB means no collision at that bin.
    pub masking_map: Arc<Mutex<Vec<f32>>>,
    /// Shared-memory registry slot claimed by the audio thread (-1 = none yet).
    /// Published here so the editor can refresh the SHM heartbeat from its GUI
    /// tick — keeps Lucent/Relay discoverable even when transport is stopped
    /// (process() doesn't run, so an audio-only heartbeat would go stale).
    pub shm_slot: Arc<AtomicI32>,
    /// Generation returned alongside `shm_slot` by `RelayHub::claim_slot()`.
    /// Must travel with the slot index everywhere it's used to touch/write —
    /// it's how the hub tells an evicted (stale-reclaimed) owner it no
    /// longer holds the slot, so the GUI-tick heartbeat refresh doesn't keep
    /// a dead claim alive and fighting the new owner.
    pub shm_generation: Arc<AtomicU32>,
    /// Per-relay enable mask keyed by SHM publisher slot (bit `i` = slot `i` active).
    /// Bit 31 = [`RELAY_MASK_DRIVEN`]: editor is driving toggles.
    /// `0` (no driven bit) = no UI preference — treat all relays as active.
    pub relay_active_mask: Arc<AtomicU32>,
    /// UI-only display toggle: 1/3-octave smoothing on the spectrum view.
    /// Read by the editor tick each frame; not persisted, not a plugin param.
    pub spectrum_smooth: Arc<AtomicBool>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            band_levels: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            target_levels: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            target_tolerances: [
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[0])),
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[1])),
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[2])),
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[3])),
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[4])),
            ],
            listen_levels: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            listen_tolerances: [
                Arc::new(AtomicF32::new(0.0)),
                Arc::new(AtomicF32::new(0.0)),
                Arc::new(AtomicF32::new(0.0)),
                Arc::new(AtomicF32::new(0.0)),
                Arc::new(AtomicF32::new(0.0)),
            ],
            listen_level_min: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            listen_level_max: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            listen_samples: Arc::new(AtomicF32::new(0.0)),
            phase_correlation: Arc::new(AtomicF32::new(1.0)),
            output_peak: Arc::new(AtomicF32::new(-90.0)),
            peak_hold: Arc::new(AtomicF32::new(-90.0)),
            input_peak: Arc::new(AtomicF32::new(-90.0)),
            clip_pre_peak_l: Arc::new(AtomicF32::new(-90.0)),
            clip_pre_peak_r: Arc::new(AtomicF32::new(-90.0)),
            clip_pre_peak_mid: Arc::new(AtomicF32::new(-90.0)),
            clip_pre_peak_side: Arc::new(AtomicF32::new(-90.0)),
            clip_wave: Arc::new(Mutex::new(ClipWaveRing::new())),
            clip_wave_write_pos: Arc::new(AtomicUsize::new(0)),
            spectrum_mid_avg: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            spectrum_side_avg: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            comp_gr_lo: Arc::new(AtomicF32::new(0.0)),
            comp_gr_hi: Arc::new(AtomicF32::new(0.0)),
            spectrum_sweet_avg: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            mb_gr_mid_lo: Arc::new(AtomicF32::new(0.0)),
            mb_gr_mid_hi: Arc::new(AtomicF32::new(0.0)),
            mb_gr_side: Arc::new(AtomicF32::new(0.0)),
            lufs_integrated: Arc::new(AtomicF32::new(-70.0)),
            true_peak_dbtp: Arc::new(AtomicF32::new(-100.0)),
            lra_lu: Arc::new(AtomicF32::new(-1.0)),
            output_peak_l: Arc::new(AtomicF32::new(-90.0)),
            output_peak_r: Arc::new(AtomicF32::new(-90.0)),
            peak_hold_l: Arc::new(AtomicF32::new(-90.0)),
            peak_hold_r: Arc::new(AtomicF32::new(-90.0)),
            reset_peak: Arc::new(AtomicBool::new(false)),
            reset_analysis: Arc::new(AtomicBool::new(false)),
            gain_reduction: Arc::new(AtomicF32::new(0.0)),
            balance: Arc::new(AtomicF32::new(0.0)),
            auto_loud_trigger: Arc::new(AtomicBool::new(false)),
            auto_loud_measuring: Arc::new(AtomicBool::new(false)),
            auto_loud_gain_offset: Arc::new(AtomicF32::new(0.0)),
            spectrum_bins: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            spectrum_avg: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            scope_samples: Arc::new(Mutex::new(vec![[0.0, 0.0]; SCOPE_BUFFER_LEN])),
            scope_write_pos: Arc::new(AtomicUsize::new(0)),
            selected_preset_index: Arc::new(AtomicUsize::new(0)),
            snap_active: Arc::new(AtomicBool::new(false)),
            sample_rate: Arc::new(AtomicF32::new(44100.0)),
            snap_phase: Arc::new(AtomicU8::new(0)),
            snap_stereo_snap: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            snap_mono_snap: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            snap_delta_snap: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            masking_map: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            shm_slot: Arc::new(AtomicI32::new(-1)),
            shm_generation: Arc::new(AtomicU32::new(0)),
            relay_active_mask: Arc::new(AtomicU32::new(0)),
            spectrum_smooth: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Editor-driving flag for [`SharedState::relay_active_mask`].
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
