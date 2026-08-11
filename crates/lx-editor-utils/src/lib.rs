//! Shared editor helpers for LX Audiolabs Slint/AURA plugins.
//!
//! Extracted from the duplicated boilerplate in `plugins/*/src/editor.rs`:
//! dirty-checking, tick throttling, meter formatting, param-binding macros,
//! SVG-path builders, and preset-vault utilities.

pub mod dirty;
pub mod meter;
pub mod params;
pub mod slint_helpers;
pub mod snap;
pub mod tick;
pub mod viz;

// Re-export `paste` so macros defined here can use it without forcing
// every consuming crate to depend on `paste` directly.
pub use paste;
