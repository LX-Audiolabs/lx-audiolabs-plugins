//! Per-plugin real-time GUI shared state (product composition).
//!
//! Building blocks (`PeakMeters`, `SnapPipeline`, …) live in
//! `aura_dsp::analysis`. This module only defines LX product aggregates
//! (Aether / Meridian / …) — not framework API.

use aura_dsp::analysis::{
    AutoLoud, PeakMeters, SPECTRUM_BINS, ScopeRing, ShmClaimShared, SnapPipeline, SpectrumView,
    band5, band5_tol,
};
use atomic_float::AtomicF32;
use std::sync::atomic::{AtomicU32, AtomicUsize};
use std::sync::{Arc, Mutex};

#[inline]
fn af(v: f32) -> Arc<AtomicF32> {
    Arc::new(AtomicF32::new(v))
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
