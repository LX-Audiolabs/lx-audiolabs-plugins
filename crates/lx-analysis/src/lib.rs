//! LX product analysis layer — **not** AURA framework.
//!
//! - Portable FFT / meters / spectrum maths: re-exported from
//!   [`aura_dsp::analysis`]
//! - Product `*Shared` aggregates: [`product_shared`]
//! - Vault / MD frontmatter / config paths: re-exported from [`lx_vault`]
//!
//! SNAP→`SNAPSHOT-*.md` writers and vault scanning live in plugins +
//! `lx-editor-utils::snap`.

pub mod product_shared;

pub use aura_dsp::analysis::{
    AutoLoud, CLIP_WAVE_LEN, ClipWaveRing, DEFAULT_BAND_TOLERANCES, EQ_BANDS, PeakMeters,
    RELAY_MASK_DRIVEN, SCOPE_BUFFER_LEN, SPECTRUM_BINS, SPECTRUM_TILT_RAW_GATE_DB, ScopeRing,
    ShmClaimShared, SnapFFT, SnapMode, SnapPipeline, SpectrumView, band5, band5_tol,
    clip_wave_minmax_window, clip_wave_scroll_phase, compute_spectrum_bins, new_clip_wave_shared,
    new_spectrum_buf, relay_slot_active, spectrum_physical_db, spectrum_tilt_db,
};

/// Product vault / config / MD frontmatter (was wrongly under `aura_dsp::analysis::vault`).
pub use lx_vault::{
    PluginConfig, get_plugin_dir, load_config, parse_frontmatter, preset_plugin_name, save_config,
};

pub use product_shared::{
    AetherShared, EquilibriumShared, LucentShared, MensorShared, MeridianShared,
};
