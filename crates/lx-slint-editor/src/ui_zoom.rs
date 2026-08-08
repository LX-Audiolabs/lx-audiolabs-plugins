//! Product UI zoom (75% / 100% / 125%) — separate from host HiDPI scale.
//!
//! Layout stays at design logical size. Effective content scale is
//! `host_scale × ui_zoom`. Host frame size is `design × ui_zoom`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use aura_baseview::EditorScale;

/// Allowed UI zoom steps (percent).
pub const UI_ZOOM_STEPS: [u32; 3] = [75, 100, 125];
pub const UI_ZOOM_DEFAULT: u32 = 100;

/// Shared zoom state for one editor instance.
#[derive(Clone)]
pub struct UiZoom {
    inner: Arc<UiZoomInner>,
}

struct UiZoomInner {
    design: (u32, u32),
    percent: AtomicU32,
    host_scale_bits: AtomicU64,
    scale: EditorScale,
}

impl UiZoom {
    #[must_use]
    pub fn new(design_w: u32, design_h: u32) -> Self {
        let percent = load_saved_percent().unwrap_or(UI_ZOOM_DEFAULT);
        Self::with_percent(design_w, design_h, percent)
    }

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

    #[must_use]
    pub fn zoomed_size(&self) -> (u32, u32) {
        let (dw, dh) = self.inner.design;
        let p = self.percent();
        (scale_dim(dw, p), scale_dim(dh, p))
    }

    #[must_use]
    pub fn scale(&self) -> EditorScale {
        self.inner.scale.clone()
    }

    pub fn set_percent(&self, percent: u32) {
        let percent = snap_percent(percent);
        self.inner.percent.store(percent, Ordering::Relaxed);
        self.recompute_effective();
        save_percent(percent);
    }

    pub fn set_host_scale(&self, host: f64) {
        if host.is_finite() && host > 0.0 {
            self.inner
                .host_scale_bits
                .store(host.to_bits(), Ordering::Relaxed);
            self.recompute_effective();
        }
    }

    fn recompute_effective(&self) {
        let host = f64::from_bits(self.inner.host_scale_bits.load(Ordering::Relaxed));
        let eff = host * factor_from_percent(self.percent());
        self.inner.scale.set(eff);
    }
}

/// Apply zoom percent and request host resize to the new zoomed frame.
pub fn apply_ui_zoom(zoom: &UiZoom, mut request_resize: impl FnMut(u32, u32) -> bool, percent: u32) {
    zoom.set_percent(percent);
    let (w, h) = zoom.zoomed_size();
    let _ = request_resize(w, h);
}

fn factor_from_percent(p: u32) -> f64 {
    f64::from(p) / 100.0
}

fn snap_percent(p: u32) -> u32 {
    UI_ZOOM_STEPS
        .iter()
        .copied()
        .min_by_key(|&s| (s as i32 - p as i32).unsigned_abs())
        .unwrap_or(UI_ZOOM_DEFAULT)
}

fn scale_dim(design: u32, percent: u32) -> u32 {
    ((u64::from(design) * u64::from(percent)) / 100).max(1) as u32
}

fn config_path() -> Option<PathBuf> {
    let base = dirs_next_home()?.join(".config").join("lx-audiolabs");
    Some(base.join("ui-zoom-percent"))
}

fn dirs_next_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn load_saved_percent() -> Option<u32> {
    let path = config_path()?;
    let s = std::fs::read_to_string(path).ok()?;
    let p: u32 = s.trim().parse().ok()?;
    Some(snap_percent(p))
}

fn save_percent(percent: u32) {
    if let Some(path) = config_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, percent.to_string());
    }
}
