//! Product UI zoom (75% / 100% / 125%) — separate from host HiDPI scale.
//!
//! Layout stays at design logical size. Effective content scale is
//! `host_scale × ui_zoom` so knobs, paths, and meters scale with FemtoVG.
//! Host frame size is `design × ui_zoom` (CLAP/VST3 `get_size`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use lx_slint_baseview::EditorScale;

/// Allowed UI zoom steps (percent).
pub const UI_ZOOM_STEPS: [u32; 3] = [75, 100, 125];
pub const UI_ZOOM_DEFAULT: u32 = 100;

/// Shared zoom state for one editor instance (and optionally load/save global).
#[derive(Clone)]
pub struct UiZoom {
    inner: Arc<UiZoomInner>,
}

struct UiZoomInner {
    design: (u32, u32),
    percent: AtomicU32,
    /// Host content scale only (from `Editor::set_scale_factor`).
    host_scale_bits: AtomicU64,
    /// Effective scale shared with baseview: host × ui_zoom.
    scale: EditorScale,
}

impl UiZoom {
    /// Create zoom state for a fixed design size. Loads last global percent if present.
    #[must_use]
    pub fn new(design_w: u32, design_h: u32) -> Self {
        let percent = load_saved_percent().unwrap_or(UI_ZOOM_DEFAULT);
        Self::with_percent(design_w, design_h, percent)
    }

    /// Create at an explicit percent (no disk load). Use for compact UIs that
    /// should stay at 100% regardless of the global zoom preference.
    #[must_use]
    pub fn with_percent(design_w: u32, design_h: u32, percent: u32) -> Self {
        let percent = snap_percent(percent);
        let host = 1.0_f64;
        let scale = EditorScale::new(host * factor_from_percent(percent));
        Self {
            inner: Arc::new(UiZoomInner {
                design: (design_w.max(1), design_h.max(1)),
                percent: AtomicU32::new(percent),
                host_scale_bits: AtomicU64::new(host.to_bits()),
                scale,
            }),
        }
    }

    #[must_use]
    pub fn design_size(&self) -> (u32, u32) {
        self.inner.design
    }

    #[must_use]
    pub fn percent(&self) -> u32 {
        self.inner.percent.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn factor(&self) -> f64 {
        factor_from_percent(self.percent())
    }

    /// Host-reported logical size (`design × zoom`, integer ratio — no float drift).
    #[must_use]
    pub fn zoomed_size(&self) -> (u32, u32) {
        let (dw, dh) = self.inner.design;
        let p = self.percent();
        (scale_dim(dw, p), scale_dim(dh, p))
    }

    /// Effective content scale cell (host × ui_zoom) — share with baseview handler.
    #[must_use]
    pub fn scale(&self) -> EditorScale {
        self.inner.scale.clone()
    }

    #[must_use]
    pub fn host_scale(&self) -> f64 {
        f64::from_bits(self.inner.host_scale_bits.load(Ordering::Relaxed))
    }

    /// Host announced a new content scale (HiDPI). Recomputes effective scale.
    pub fn set_host_scale(&self, host: f64) {
        if !(host.is_finite() && host > 0.0) {
            return;
        }
        self.inner
            .host_scale_bits
            .store(host.to_bits(), Ordering::Relaxed);
        self.recompute_effective();
    }

    /// Set UI zoom percent (clamped to allowed steps). Persists globally.
    /// Returns the applied percent.
    pub fn set_percent(&self, percent: u32) -> u32 {
        let p = snap_percent(percent);
        self.inner.percent.store(p, Ordering::Relaxed);
        self.recompute_effective();
        save_percent(p);
        p
    }

    fn recompute_effective(&self) {
        let host = self.host_scale();
        let z = self.factor();
        self.inner.scale.set(host * z);
    }
}

/// Set zoom percent and request host resize to the new frame size.
///
/// Use from the logo menu callback:
/// `apply_ui_zoom(&zoom, |w, h| state.request_resize(w, h), percent);`
pub fn apply_ui_zoom(
    zoom: &UiZoom,
    request_resize: impl FnOnce(u32, u32) -> bool,
    percent: u32,
) {
    let _ = zoom.set_percent(percent);
    let (w, h) = zoom.zoomed_size();
    let _ = request_resize(w, h);
}

#[inline]
fn factor_from_percent(percent: u32) -> f64 {
    f64::from(snap_percent(percent)) / 100.0
}

#[inline]
fn snap_percent(percent: u32) -> u32 {
    // Nearest of the fixed steps.
    let mut best = UI_ZOOM_DEFAULT;
    let mut best_d = u32::MAX;
    for &s in &UI_ZOOM_STEPS {
        let d = percent.abs_diff(s);
        if d < best_d {
            best_d = d;
            best = s;
        }
    }
    best
}

/// Scale one design dimension by a fixed percent step (75 / 100 / 125).
#[inline]
fn scale_dim(design: u32, percent: u32) -> u32 {
    let d = u64::from(design.max(1));
    let p = u64::from(snap_percent(percent));
    // integer: design * percent / 100  (742 for 990@75%, 1237 for 990@125%)
    ((d * p) / 100).max(1) as u32
}

fn config_path() -> Option<PathBuf> {
    // Tests / tooling may redirect so we never touch the real user file.
    if let Some(p) = std::env::var_os("LX_UI_ZOOM_PATH") {
        return Some(PathBuf::from(p));
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(base).join("LX Audiolabs").join("ui-zoom"))
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
            })?;
        Some(base.join("lx-audiolabs").join("ui-zoom"))
    }
}

/// Only exact allowed steps (75/100/125). Missing/invalid → treat as no preference.
fn load_saved_percent() -> Option<u32> {
    let path = config_path()?;
    let s = std::fs::read_to_string(path).ok()?;
    let n: u32 = s.trim().parse().ok()?;
    if UI_ZOOM_STEPS.contains(&n) {
        Some(n)
    } else {
        None
    }
}

fn save_percent(percent: u32) {
    let p = snap_percent(percent);
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format!("{p}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_to_steps() {
        assert_eq!(snap_percent(75), 75);
        assert_eq!(snap_percent(100), 100);
        assert_eq!(snap_percent(125), 125);
        assert_eq!(snap_percent(90), 100);
        assert_eq!(snap_percent(50), 75);
        assert_eq!(snap_percent(200), 125);
    }

    #[test]
    fn zoomed_size_integer() {
        // with_percent: no disk I/O — must not pollute APPDATA (set_percent would).
        assert_eq!(
            UiZoom::with_percent(990, 660, 100).zoomed_size(),
            (990, 660)
        );
        assert_eq!(
            UiZoom::with_percent(990, 660, 75).zoomed_size(),
            (742, 495)
        );
        assert_eq!(
            UiZoom::with_percent(990, 660, 125).zoomed_size(),
            (1237, 825)
        );
    }

    #[test]
    fn default_without_file_is_100() {
        // Point load/save at a missing path → first open must be 100%.
        let dir = std::env::temp_dir().join(format!(
            "lx-ui-zoom-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let path = dir.join("ui-zoom");
        // SAFETY: test-only env for this process; no parallel test races on this key.
        unsafe {
            std::env::set_var("LX_UI_ZOOM_PATH", &path);
        }
        let z = UiZoom::new(990, 660);
        assert_eq!(z.percent(), 100);
        assert_eq!(z.zoomed_size(), (990, 660));
        let _ = std::fs::remove_dir_all(dir);
        unsafe {
            std::env::remove_var("LX_UI_ZOOM_PATH");
        }
    }
}
