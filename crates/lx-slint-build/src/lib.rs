//! Build-script helper for LX plugins with Slint GUI.
//!
//! Thin wrap of [`aura_build::compile`] — fonts + `@aura` widgets.
//! Product chrome stays in `lx-ui-slint` (imported by path from `.slint`).

pub use aura_build::{compile, CompileError};
