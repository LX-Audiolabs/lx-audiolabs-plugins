//! Per-plugin real-time GUI shared state (audio ↔ UI atomics / try_lock buffers).
//!
//! Split from a single `SharedState` monolith so each plugin only allocates
//! and documents the slots it actually uses (see types below).
//!
//! Thread boundary: audio thread **writes** meters; UI **polls** at ~30 Hz.
//! Prefer `Atomic*` and `try_lock` — never block the audio thread on a full
//! `Mutex`.

use super::{ClipWaveRing, SCOPE_BUFFER_LEN};
use crate::{DEFAULT_TOLERANCES, SPECTRUM_BINS};
use atomic_float::AtomicF32;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU8, AtomicUsize};
use std::sync::{Arc, Mutex};

// ── helpers ──────────────────────────────────────────────────────────────────

#[inline]
fn af(v: f32) -> Arc<AtomicF32> {
    Arc::new(AtomicF32::new(v))
}

#[inline]
fn ab(v: bool) -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(v))
}

#[inline]
fn band5(v: f32) -> [Arc<AtomicF32>; 5] {
    [af(v), af(v), af(v), af(v), af(v)]
}

#[inline]
fn band5_tol() -> [Arc<AtomicF32>; 5] {
    [
        af(DEFAULT_TOLERANCES[0]),
        af(DEFAULT_TOLERANCES[1]),
        af(DEFAULT_TOLERANCES[2]),
        af(DEFAULT_TOLERANCES[3]),
        af(DEFAULT_TOLERANCES[4]),
    ]
}

#[inline]
fn spectrum_buf() -> Arc<Mutex<Vec<f32>>> {
    Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS]))
}

#[inline]
fn scope_buf() -> Arc<Mutex<Vec<[f32; 2]>>> {
    Arc::new(Mutex::new(vec![[0.0, 0.0]; SCOPE_BUFFER_LEN]))
}

// ── Aether ───────────────────────────────────────────────────────────────────

/// Aether UI meters — input peak + sample rate only.
pub struct AetherShared {
    /// Input peak (max |L|,|R| per block, dBFS). Editor latches peak-hold.
    pub input_peak: Arc<AtomicF32>,
    pub sample_rate: Arc<AtomicF32>,
}

impl Default for AetherShared {
    fn default() -> Self {
        Self {
            input_peak: af(-90.0),
            sample_rate: af(44100.0),
        }
    }
}

// ── Lucent-relay ─────────────────────────────────────────────────────────────

/// SHM publisher claim — lucent-relay (and the shm_* half of lucent).
pub struct ShmClaimShared {
    /// Registry slot claimed by audio/editor (-1 = none).
    pub shm_slot: Arc<AtomicI32>,
    /// Generation from `RelayHub::claim_slot()` — must travel with the slot.
    pub shm_generation: Arc<AtomicU32>,
}

impl Default for ShmClaimShared {
    fn default() -> Self {
        Self {
            shm_slot: Arc::new(AtomicI32::new(-1)),
            shm_generation: Arc::new(AtomicU32::new(0)),
        }
    }
}

// ── Lucent ───────────────────────────────────────────────────────────────────

/// Lucent spectrum / masking / SHM consumer UI state.
pub struct LucentShared {
    pub phase_correlation: Arc<AtomicF32>,
    pub output_peak: Arc<AtomicF32>,
    pub peak_hold: Arc<AtomicF32>,
    pub input_peak: Arc<AtomicF32>,
    pub output_peak_l: Arc<AtomicF32>,
    pub output_peak_r: Arc<AtomicF32>,
    pub peak_hold_l: Arc<AtomicF32>,
    pub peak_hold_r: Arc<AtomicF32>,
    pub reset_peak: Arc<AtomicBool>,
    pub balance: Arc<AtomicF32>,
    pub spectrum_bins: Arc<Mutex<Vec<f32>>>,
    pub spectrum_avg: Arc<Mutex<Vec<f32>>>,
    pub scope_samples: Arc<Mutex<Vec<[f32; 2]>>>,
    pub scope_write_pos: Arc<AtomicUsize>,
    /// Masking collision map (dB/bin); -90 = no collision.
    pub masking_map: Arc<Mutex<Vec<f32>>>,
    pub shm_slot: Arc<AtomicI32>,
    pub shm_generation: Arc<AtomicU32>,
    /// Bit `i` = SHM slot `i` active; bit 31 = [`super::RELAY_MASK_DRIVEN`].
    pub relay_active_mask: Arc<AtomicU32>,
    /// UI-only 1/3-octave display smooth.
    pub spectrum_smooth: Arc<AtomicBool>,
    pub sample_rate: Arc<AtomicF32>,
}

impl Default for LucentShared {
    fn default() -> Self {
        Self {
            phase_correlation: af(1.0),
            output_peak: af(-90.0),
            peak_hold: af(-90.0),
            input_peak: af(-90.0),
            output_peak_l: af(-90.0),
            output_peak_r: af(-90.0),
            peak_hold_l: af(-90.0),
            peak_hold_r: af(-90.0),
            reset_peak: ab(false),
            balance: af(0.0),
            spectrum_bins: spectrum_buf(),
            spectrum_avg: spectrum_buf(),
            scope_samples: scope_buf(),
            scope_write_pos: Arc::new(AtomicUsize::new(0)),
            masking_map: spectrum_buf(),
            shm_slot: Arc::new(AtomicI32::new(-1)),
            shm_generation: Arc::new(AtomicU32::new(0)),
            relay_active_mask: Arc::new(AtomicU32::new(0)),
            spectrum_smooth: ab(false),
            sample_rate: af(44100.0),
        }
    }
}

// ── Meridian ─────────────────────────────────────────────────────────────────

/// Meridian meters, spectrum, SNAP, AUTO LOUD, GR.
pub struct MeridianShared {
    pub phase_correlation: Arc<AtomicF32>,
    pub output_peak: Arc<AtomicF32>,
    pub peak_hold: Arc<AtomicF32>,
    pub output_peak_l: Arc<AtomicF32>,
    pub output_peak_r: Arc<AtomicF32>,
    pub peak_hold_l: Arc<AtomicF32>,
    pub peak_hold_r: Arc<AtomicF32>,
    pub reset_peak: Arc<AtomicBool>,
    pub reset_analysis: Arc<AtomicBool>,
    pub gain_reduction: Arc<AtomicF32>,
    pub balance: Arc<AtomicF32>,
    pub auto_loud_trigger: Arc<AtomicBool>,
    pub auto_loud_measuring: Arc<AtomicBool>,
    pub auto_loud_gain_offset: Arc<AtomicF32>,
    pub spectrum_bins: Arc<Mutex<Vec<f32>>>,
    pub spectrum_avg: Arc<Mutex<Vec<f32>>>,
    pub scope_samples: Arc<Mutex<Vec<[f32; 2]>>>,
    pub scope_write_pos: Arc<AtomicUsize>,
    pub band_levels: [Arc<AtomicF32>; 5],
    pub snap_active: Arc<AtomicBool>,
    pub sample_rate: Arc<AtomicF32>,
    pub snap_phase: Arc<AtomicU8>,
    pub snap_stereo_snap: Arc<Mutex<Vec<f32>>>,
    pub snap_mono_snap: Arc<Mutex<Vec<f32>>>,
    pub snap_delta_snap: Arc<Mutex<Vec<f32>>>,
    pub spectrum_smooth: Arc<AtomicBool>,
}

impl Default for MeridianShared {
    fn default() -> Self {
        Self {
            phase_correlation: af(1.0),
            output_peak: af(-90.0),
            peak_hold: af(-90.0),
            output_peak_l: af(-90.0),
            output_peak_r: af(-90.0),
            peak_hold_l: af(-90.0),
            peak_hold_r: af(-90.0),
            reset_peak: ab(false),
            reset_analysis: ab(false),
            gain_reduction: af(0.0),
            balance: af(0.0),
            auto_loud_trigger: ab(false),
            auto_loud_measuring: ab(false),
            auto_loud_gain_offset: af(0.0),
            spectrum_bins: spectrum_buf(),
            spectrum_avg: spectrum_buf(),
            scope_samples: scope_buf(),
            scope_write_pos: Arc::new(AtomicUsize::new(0)),
            band_levels: band5(-90.0),
            snap_active: ab(false),
            sample_rate: af(44100.0),
            snap_phase: Arc::new(AtomicU8::new(0)),
            snap_stereo_snap: spectrum_buf(),
            snap_mono_snap: spectrum_buf(),
            snap_delta_snap: spectrum_buf(),
            spectrum_smooth: ab(false),
        }
    }
}

// ── Equilibrium ──────────────────────────────────────────────────────────────

/// Equilibrium band targets / listen + SNAP + AUTO LOUD + peaks.
pub struct EquilibriumShared {
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
    pub output_peak_l: Arc<AtomicF32>,
    pub output_peak_r: Arc<AtomicF32>,
    pub peak_hold_l: Arc<AtomicF32>,
    pub peak_hold_r: Arc<AtomicF32>,
    pub reset_peak: Arc<AtomicBool>,
    pub reset_analysis: Arc<AtomicBool>,
    pub balance: Arc<AtomicF32>,
    pub auto_loud_trigger: Arc<AtomicBool>,
    pub auto_loud_measuring: Arc<AtomicBool>,
    pub auto_loud_gain_offset: Arc<AtomicF32>,
    pub scope_samples: Arc<Mutex<Vec<[f32; 2]>>>,
    pub scope_write_pos: Arc<AtomicUsize>,
    pub selected_preset_index: Arc<AtomicUsize>,
    pub snap_active: Arc<AtomicBool>,
    pub sample_rate: Arc<AtomicF32>,
    pub snap_phase: Arc<AtomicU8>,
    pub snap_stereo_snap: Arc<Mutex<Vec<f32>>>,
    pub snap_mono_snap: Arc<Mutex<Vec<f32>>>,
    pub snap_delta_snap: Arc<Mutex<Vec<f32>>>,
}

impl Default for EquilibriumShared {
    fn default() -> Self {
        Self {
            band_levels: band5(-90.0),
            target_levels: band5(-90.0),
            target_tolerances: band5_tol(),
            listen_levels: band5(-90.0),
            listen_tolerances: band5(0.0),
            listen_level_min: band5(-90.0),
            listen_level_max: band5(-90.0),
            listen_samples: af(0.0),
            phase_correlation: af(1.0),
            output_peak: af(-90.0),
            peak_hold: af(-90.0),
            output_peak_l: af(-90.0),
            output_peak_r: af(-90.0),
            peak_hold_l: af(-90.0),
            peak_hold_r: af(-90.0),
            reset_peak: ab(false),
            reset_analysis: ab(false),
            balance: af(0.0),
            auto_loud_trigger: ab(false),
            auto_loud_measuring: ab(false),
            auto_loud_gain_offset: af(0.0),
            scope_samples: scope_buf(),
            scope_write_pos: Arc::new(AtomicUsize::new(0)),
            selected_preset_index: Arc::new(AtomicUsize::new(0)),
            snap_active: ab(false),
            sample_rate: af(44100.0),
            snap_phase: Arc::new(AtomicU8::new(0)),
            snap_stereo_snap: spectrum_buf(),
            snap_mono_snap: spectrum_buf(),
            snap_delta_snap: spectrum_buf(),
        }
    }
}

// ── Aurum ────────────────────────────────────────────────────────────────────

/// Aurum mastering meters — clip/wave, multi-GR, delivery loudness, scope.
pub struct AurumShared {
    pub phase_correlation: Arc<AtomicF32>,
    pub output_peak: Arc<AtomicF32>,
    pub peak_hold: Arc<AtomicF32>,
    pub input_peak: Arc<AtomicF32>,
    pub clip_pre_peak_l: Arc<AtomicF32>,
    pub clip_pre_peak_r: Arc<AtomicF32>,
    pub clip_pre_peak_mid: Arc<AtomicF32>,
    pub clip_pre_peak_side: Arc<AtomicF32>,
    pub clip_wave: Arc<Mutex<ClipWaveRing>>,
    pub clip_wave_write_pos: Arc<AtomicUsize>,
    pub spectrum_mid_avg: Arc<Mutex<Vec<f32>>>,
    pub spectrum_side_avg: Arc<Mutex<Vec<f32>>>,
    pub comp_gr_lo: Arc<AtomicF32>,
    pub comp_gr_hi: Arc<AtomicF32>,
    pub spectrum_sweet_avg: Arc<Mutex<Vec<f32>>>,
    pub mb_gr_mid_lo: Arc<AtomicF32>,
    pub mb_gr_mid_hi: Arc<AtomicF32>,
    pub mb_gr_side: Arc<AtomicF32>,
    pub lufs_integrated: Arc<AtomicF32>,
    pub true_peak_dbtp: Arc<AtomicF32>,
    pub lra_lu: Arc<AtomicF32>,
    pub output_peak_l: Arc<AtomicF32>,
    pub output_peak_r: Arc<AtomicF32>,
    pub peak_hold_l: Arc<AtomicF32>,
    pub peak_hold_r: Arc<AtomicF32>,
    pub reset_peak: Arc<AtomicBool>,
    pub gain_reduction: Arc<AtomicF32>,
    pub balance: Arc<AtomicF32>,
    pub scope_samples: Arc<Mutex<Vec<[f32; 2]>>>,
    pub scope_write_pos: Arc<AtomicUsize>,
    pub sample_rate: Arc<AtomicF32>,
    /// True while SNAP / analyze export runs — GUI shows analyzing state.
    pub snap_active: Arc<AtomicBool>,
}

impl Default for AurumShared {
    fn default() -> Self {
        Self {
            phase_correlation: af(1.0),
            output_peak: af(-90.0),
            peak_hold: af(-90.0),
            input_peak: af(-90.0),
            clip_pre_peak_l: af(-90.0),
            clip_pre_peak_r: af(-90.0),
            clip_pre_peak_mid: af(-90.0),
            clip_pre_peak_side: af(-90.0),
            clip_wave: Arc::new(Mutex::new(ClipWaveRing::new())),
            clip_wave_write_pos: Arc::new(AtomicUsize::new(0)),
            spectrum_mid_avg: spectrum_buf(),
            spectrum_side_avg: spectrum_buf(),
            comp_gr_lo: af(0.0),
            comp_gr_hi: af(0.0),
            spectrum_sweet_avg: spectrum_buf(),
            mb_gr_mid_lo: af(0.0),
            mb_gr_mid_hi: af(0.0),
            mb_gr_side: af(0.0),
            lufs_integrated: af(-70.0),
            true_peak_dbtp: af(-100.0),
            lra_lu: af(-1.0),
            output_peak_l: af(-90.0),
            output_peak_r: af(-90.0),
            peak_hold_l: af(-90.0),
            peak_hold_r: af(-90.0),
            reset_peak: ab(false),
            gain_reduction: af(0.0),
            balance: af(0.0),
            scope_samples: scope_buf(),
            scope_write_pos: Arc::new(AtomicUsize::new(0)),
            sample_rate: af(44100.0),
            snap_active: ab(false),
        }
    }
}
