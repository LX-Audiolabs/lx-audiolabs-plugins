//! Product analysis façade — thin re-exports from `aura_dsp::analysis`.
//!
//! - Portable FFT / spectrum / meter blocks → [`aura_dsp::analysis`]
//! - Multi-plugin SHM → `aura-shm` re-exports
//! - Plugin config / preset scanner → [`aura_dsp::analysis::vault`]
//! - Per-plugin `*Shared` UI state → [`aura_dsp::analysis::product_shared`]

pub use aura_dsp::analysis::product_shared::{
    AetherShared, AurumShared, EquilibriumShared, LucentShared, MeridianShared,
};
pub use aura_dsp::analysis::vault::{
    get_plugin_dir, load_config, parse_frontmatter, preset_plugin_name, save_config, PluginConfig,
};
pub use aura_dsp::analysis::{
    AutoLoud, ClipWaveRing, PeakMeters, ScopeRing, ShmClaimShared, SnapFFT, SnapMode, SnapPipeline,
    SpectrumView, CLIP_WAVE_LEN, DEFAULT_BAND_TOLERANCES, EQ_BANDS, RELAY_MASK_DRIVEN,
    SCOPE_BUFFER_LEN, SPECTRUM_BINS, SPECTRUM_TILT_RAW_GATE_DB, band5, band5_tol,
    clip_wave_minmax_window, clip_wave_scroll_phase, compute_spectrum_bins, relay_slot_active,
    spectrum_physical_db, spectrum_tilt_db,
};

/// Dev file logger (feature `dev-logging`).
pub mod dev_log {
    pub use aura_dsp::analysis::dev_log::*;
}

// Re-export aura-shm so existing callers keep working
pub use aura_shm as shm;
pub use aura_shm::{
    display_name, now_ms, relay_hub, resolve_from_consumers, resolve_relay_target, RelayHub,
    MAX_CONSUMERS, MAX_NAME_LEN, MAX_SLOTS, STALE_MS,
};
