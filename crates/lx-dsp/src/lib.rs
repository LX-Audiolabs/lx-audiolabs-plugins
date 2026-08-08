//! Thin re-export of AURA product-grade FX (`aura_dsp::fx`).
//!
//! Implementation lives in the AURA framework. Keep this crate so plugins can
//! keep `use lx_dsp::…` without a bulk import rewrite.

pub use aura_dsp::fx::*;
