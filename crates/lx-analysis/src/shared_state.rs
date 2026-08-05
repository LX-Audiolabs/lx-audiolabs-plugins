//! Per-plugin real-time GUI shared state (audio ↔ UI atomics / try_lock buffers).
//!
//! ## Layout
//! - **Building blocks** (`PeakMeters`, `ScopeRing`, `SpectrumView`, `SnapPipeline`,
//!   `AutoLoud`, `ShmClaimShared`) — shared field groups, one definition.
//! - **Plugin types** compose only the blocks they need (no kitchen-sink monolith).
//!
//! Thread boundary: audio thread **writes** meters; UI **polls** at ~30 Hz.
//! Prefer `Atomic*` and `try_lock` — never block the audio thread on a full `Mutex`.
//!
//! Access pattern: `params.shared.peaks.output_peak_l`, `params.shared.scope.samples`, …

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

// ── building blocks ──────────────────────────────────────────────────────────

/// Output / correlation / balance meters (stereo + mono peak + holds).
#[derive(Clone)]
pub struct PeakMeters {
    pub phase_correlation: Arc<AtomicF32>,
    pub output_peak: Arc<AtomicF32>,
    pub peak_hold: Arc<AtomicF32>,
    pub output_peak_l: Arc<AtomicF32>,
    pub output_peak_r: Arc<AtomicF32>,
    pub peak_hold_l: Arc<AtomicF32>,
    pub peak_hold_r: Arc<AtomicF32>,
    pub reset_peak: Arc<AtomicBool>,
    pub balance: Arc<AtomicF32>,
}

impl Default for PeakMeters {
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
            balance: af(0.0),
        }
    }
}

/// Goniometer / vectorscope ring.
#[derive(Clone)]
pub struct ScopeRing {
    pub samples: Arc<Mutex<Vec<[f32; 2]>>>,
    pub write_pos: Arc<AtomicUsize>,
}

impl Default for ScopeRing {
    fn default() -> Self {
        Self {
            samples: scope_buf(),
            write_pos: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// FFT magnitude bins + EMA + optional 1/3-oct display smooth + sample rate.
#[derive(Clone)]
pub struct SpectrumView {
    pub bins: Arc<Mutex<Vec<f32>>>,
    pub avg: Arc<Mutex<Vec<f32>>>,
    /// UI-only 1/3-octave display smooth (not a plugin param).
    pub smooth: Arc<AtomicBool>,
    pub sample_rate: Arc<AtomicF32>,
}

impl Default for SpectrumView {
    fn default() -> Self {
        Self {
            bins: spectrum_buf(),
            avg: spectrum_buf(),
            smooth: ab(false),
            sample_rate: af(44100.0),
        }
    }
}

/// SNAP / ANALYZE multi-phase capture (stereo → mono → delta).
#[derive(Clone)]
pub struct SnapPipeline {
    pub active: Arc<AtomicBool>,
    /// 0=idle, 1=stereo, 2=mono, 3=delta
    pub phase: Arc<AtomicU8>,
    pub stereo: Arc<Mutex<Vec<f32>>>,
    pub mono: Arc<Mutex<Vec<f32>>>,
    pub delta: Arc<Mutex<Vec<f32>>>,
    pub reset_analysis: Arc<AtomicBool>,
}

impl Default for SnapPipeline {
    fn default() -> Self {
        Self {
            active: ab(false),
            phase: Arc::new(AtomicU8::new(0)),
            stereo: spectrum_buf(),
            mono: spectrum_buf(),
            delta: spectrum_buf(),
            reset_analysis: ab(false),
        }
    }
}

/// AUTO LOUD measurement handoff UI ↔ audio.
#[derive(Clone)]
pub struct AutoLoud {
    pub trigger: Arc<AtomicBool>,
    pub measuring: Arc<AtomicBool>,
    pub gain_offset: Arc<AtomicF32>,
}

impl Default for AutoLoud {
    fn default() -> Self {
        Self {
            trigger: ab(false),
            measuring: ab(false),
            gain_offset: af(0.0),
        }
    }
}

/// SHM publisher claim — lucent-relay (and lucent’s consumer slot).
#[derive(Clone)]
pub struct ShmClaimShared {
    /// Registry slot claimed by audio/editor (-1 = none).
    pub slot: Arc<AtomicI32>,
    /// Generation from `RelayHub::claim_slot()` — must travel with the slot.
    pub generation: Arc<AtomicU32>,
}

impl Default for ShmClaimShared {
    fn default() -> Self {
        Self {
            slot: Arc::new(AtomicI32::new(-1)),
            generation: Arc::new(AtomicU32::new(0)),
        }
    }
}

// ── Aether ───────────────────────────────────────────────────────────────────

/// Aether UI meters — input peak + sample rate only.
#[derive(Clone)]
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

// ── Lucent ───────────────────────────────────────────────────────────────────

/// Lucent spectrum / masking / SHM consumer UI state.
#[derive(Clone)]
pub struct LucentShared {
    pub peaks: PeakMeters,
    pub scope: ScopeRing,
    pub spectrum: SpectrumView,
    pub shm: ShmClaimShared,
    /// Masking collision map (dB/bin); -90 = no collision.
    pub masking_map: Arc<Mutex<Vec<f32>>>,
    /// Bit `i` = SHM slot `i` active; bit 31 = [`super::RELAY_MASK_DRIVEN`].
    pub relay_active_mask: Arc<AtomicU32>,
    pub input_peak: Arc<AtomicF32>,
}

impl Default for LucentShared {
    fn default() -> Self {
        Self {
            peaks: PeakMeters::default(),
            scope: ScopeRing::default(),
            spectrum: SpectrumView::default(),
            shm: ShmClaimShared::default(),
            masking_map: spectrum_buf(),
            relay_active_mask: Arc::new(AtomicU32::new(0)),
            input_peak: af(-90.0),
        }
    }
}

// ── Meridian ─────────────────────────────────────────────────────────────────

/// Meridian meters, spectrum, SNAP, AUTO LOUD, GR.
#[derive(Clone)]
pub struct MeridianShared {
    pub peaks: PeakMeters,
    pub scope: ScopeRing,
    pub spectrum: SpectrumView,
    pub snap: SnapPipeline,
    pub auto_loud: AutoLoud,
    pub band_levels: [Arc<AtomicF32>; 5],
    pub gain_reduction: Arc<AtomicF32>,
}

impl Default for MeridianShared {
    fn default() -> Self {
        Self {
            peaks: PeakMeters::default(),
            scope: ScopeRing::default(),
            spectrum: SpectrumView::default(),
            snap: SnapPipeline::default(),
            auto_loud: AutoLoud::default(),
            band_levels: band5(-90.0),
            gain_reduction: af(0.0),
        }
    }
}

// ── Equilibrium ──────────────────────────────────────────────────────────────

/// Equilibrium band targets / listen + SNAP + AUTO LOUD + peaks.
#[derive(Clone)]
pub struct EquilibriumShared {
    pub peaks: PeakMeters,
    pub scope: ScopeRing,
    pub snap: SnapPipeline,
    pub auto_loud: AutoLoud,
    pub band_levels: [Arc<AtomicF32>; 5],
    pub target_levels: [Arc<AtomicF32>; 5],
    pub target_tolerances: [Arc<AtomicF32>; 5],
    pub listen_levels: [Arc<AtomicF32>; 5],
    pub listen_tolerances: [Arc<AtomicF32>; 5],
    pub listen_level_min: [Arc<AtomicF32>; 5],
    pub listen_level_max: [Arc<AtomicF32>; 5],
    pub listen_samples: Arc<AtomicF32>,
    pub selected_preset_index: Arc<AtomicUsize>,
    /// Sample rate for SNAP frequency labels (no full spectrum view).
    pub sample_rate: Arc<AtomicF32>,
}

impl Default for EquilibriumShared {
    fn default() -> Self {
        Self {
            peaks: PeakMeters::default(),
            scope: ScopeRing::default(),
            snap: SnapPipeline::default(),
            auto_loud: AutoLoud::default(),
            band_levels: band5(-90.0),
            target_levels: band5(-90.0),
            target_tolerances: band5_tol(),
            listen_levels: band5(-90.0),
            listen_tolerances: band5(0.0),
            listen_level_min: band5(-90.0),
            listen_level_max: band5(-90.0),
            listen_samples: af(0.0),
            selected_preset_index: Arc::new(AtomicUsize::new(0)),
            sample_rate: af(44100.0),
        }
    }
}

// ── Aurum ────────────────────────────────────────────────────────────────────

/// Aurum mastering meters — clip/wave, multi-GR, delivery loudness, scope.
#[derive(Clone)]
pub struct AurumShared {
    pub peaks: PeakMeters,
    pub scope: ScopeRing,
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
    pub gain_reduction: Arc<AtomicF32>,
    pub sample_rate: Arc<AtomicF32>,
    /// True while SNAP / analyze export runs — GUI shows analyzing state.
    pub snap_active: Arc<AtomicBool>,
}

impl Default for AurumShared {
    fn default() -> Self {
        Self {
            peaks: PeakMeters::default(),
            scope: ScopeRing::default(),
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
            gain_reduction: af(0.0),
            sample_rate: af(44100.0),
            snap_active: ab(false),
        }
    }
}
