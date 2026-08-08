//! Thin re-export of `aura_editor::typed` for backward-compatible `lx_slint_editor` paths.
//!
//! All implementation lives in aura-editor. Plugin crates keep `use lx_slint_editor`.

pub use aura_editor::typed::*;
pub use aura_editor::ui_zoom::{apply_ui_zoom, UiZoom, UI_ZOOM_DEFAULT, UI_ZOOM_STEPS};

// Re-exported for plugin bind macros
pub use paste::paste;
pub use aura_editor::platform::clipboard_get_retry;
