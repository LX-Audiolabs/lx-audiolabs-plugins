//! Per-plugin real-time GUI shared state (product composition).
//!
//! Building blocks (`PeakMeters`, `SnapPipeline`, …) live in
//! `aura_dsp::analysis`. This module only defines LX product aggregates
//! (Aether / Meridian / …) — not framework API.

use atomic_float::AtomicF32;
use aura_dsp::analysis::{
    AutoLoud, ClipWaveRing, PeakMeters, SPECTRUM_BINS, ScopeRing, ShmClaimShared, SnapPipeline,
    SpectrumView, band5, band5_tol,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize};
use std::sync::{Arc, Mutex};

#[inline]
fn af(v: f32) -> Arc<AtomicF32> {
    Arc::new(AtomicF32::new(v))
}

#[inline]
fn ab(v: bool) -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(v))
}

#[inline]
fn spectrum_buf() -> Arc<Mutex<Vec<f32>>> {
    Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS]))
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
    /// Bit `i` = SHM slot `i` active; bit 31 = [`crate::RELAY_MASK_DRIVEN`].
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

// ── Mensor ────────────────────────────────────────────────────────────────────

/// Mensor mastering meters — clip/wave, multi-GR, delivery loudness, scope.
#[derive(Clone)]
pub struct MensorShared {
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

impl Default for MensorShared {
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
