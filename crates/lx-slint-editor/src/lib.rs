//! `LxSlintEditor` — AURA `Editor` adapter with product UI zoom.
//!
//! FemtoVG + OpenGL via `aura-baseview`. Product zoom (75/100/125%) multiplies
//! host scale; layout stays at design size.
//!
//! Typed params: store `Arc<P>` at construction; build/sync get
//! [`LxPluginContext<P>`] with `automate` / `get_param` / field access.

use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use aura_baseview::slint_window::SlintWindow;
use aura_baseview::{pack_size, to_physical_px, EditorScale, RequestResizeFn, SizePolicy};
use aura_core::editor::{Editor, PluginContext, RawWindowHandle};
use aura_params::Params;
use baseview::{gl::GlConfig, Window, WindowSettings};
use slint::ComponentHandle;

mod parent;
mod ui_zoom;

pub use aura_baseview::platform::clipboard_get_retry;
pub use paste::paste;
pub use ui_zoom::{apply_ui_zoom, UiZoom, UI_ZOOM_DEFAULT, UI_ZOOM_STEPS};

/// Typed product context: Arc params + host bridge helpers.
pub struct LxPluginContext<P: Params + 'static> {
    pub params: Arc<P>,
    pub host: PluginContext,
}

impl<P: Params + 'static> Clone for LxPluginContext<P> {
    fn clone(&self) -> Self {
        Self {
            params: Arc::clone(&self.params),
            host: self.host.clone(),
        }
    }
}

impl<P: Params + 'static> LxPluginContext<P> {
    #[must_use]
    pub fn new(params: Arc<P>, host: PluginContext) -> Self {
        Self { params, host }
    }

    /// Begin + set + end (or plain set when no bridge).
    pub fn automate(&self, id: impl Into<u32>, normalized: f64) {
        let id = id.into();
        if let Some(bridge) = &self.host.bridge {
            bridge.begin_edit(id);
            bridge.set_param(id, normalized);
            bridge.end_edit(id);
        } else {
            self.params.set_normalized(id, normalized);
        }
    }

    #[must_use]
    pub fn get_param(&self, id: impl Into<u32>) -> f64 {
        let id = id.into();
        if let Some(bridge) = &self.host.bridge {
            bridge.get_param(id)
        } else {
            self.params.get_normalized(id).unwrap_or(0.0)
        }
    }

    #[must_use]
    pub fn get_param_plain(&self, id: impl Into<u32>) -> f64 {
        let id = id.into();
        if let Some(bridge) = &self.host.bridge {
            bridge.get_param_plain(id)
        } else {
            self.params.get_plain(id).unwrap_or(0.0)
        }
    }

    #[must_use]
    pub fn format_param(&self, id: impl Into<u32>) -> String {
        let id = id.into();
        let plain = self.get_param_plain(id);
        self.params
            .format_value(id, plain)
            .unwrap_or_else(|| format!("{plain:.2}"))
    }

    #[must_use]
    pub fn request_resize(&self, w: u32, h: u32) -> bool {
        self.host
            .bridge
            .as_ref()
            .is_some_and(|b| b.request_resize(w, h))
    }
}

impl<P: Params + 'static> Deref for LxPluginContext<P> {
    type Target = P;
    fn deref(&self) -> &P {
        &self.params
    }
}

/// f32 reads for dirty sync macros.
pub trait PluginContextReadF32 {
    fn get_param(&self, id: impl Into<u32>) -> f32;
}

impl<P: Params + 'static> PluginContextReadF32 for LxPluginContext<P> {
    fn get_param(&self, id: impl Into<u32>) -> f32 {
        #[allow(clippy::cast_possible_truncation)]
        {
            LxPluginContext::get_param(self, id) as f32
        }
    }
}

/// Discrete index 0..count-1 → normalized 0..1.
#[must_use]
pub fn discrete_norm(index: usize, count: usize) -> f64 {
    if count <= 1 {
        0.0
    } else {
        (index.min(count - 1) as f64) / ((count - 1) as f64)
    }
}

/// Normalized 0..1 → discrete index.
#[must_use]
pub fn discrete_index(norm: f64, count: usize) -> usize {
    if count <= 1 {
        0
    } else {
        let n = norm.clamp(0.0, 1.0);
        ((n * (count - 1) as f64).round() as usize).min(count - 1)
    }
}

pub type BuildFn<P, C> = Arc<dyn Fn(LxPluginContext<P>) -> C + Send + Sync>;
pub type SyncFn<P, C> = Arc<dyn Fn(&C, &LxPluginContext<P>) + Send + Sync>;

/// Slint editor with product UI zoom.
pub struct LxSlintEditor<P, C>
where
    P: Params + 'static,
    C: ComponentHandle + 'static,
{
    params: Arc<P>,
    design_size: (u32, u32),
    build: BuildFn<P, C>,
    sync: SyncFn<P, C>,
    window: Option<Window>,
    can_resize: bool,
    min_size: (u32, u32),
    max_size: (u32, u32),
    ui_zoom: UiZoom,
    pending_size: Arc<AtomicU64>,
}

// SAFETY: baseview Window is GUI-thread only (host contract).
unsafe impl<P, C> Send for LxSlintEditor<P, C>
where
    P: Params + 'static,
    C: ComponentHandle + 'static,
{
}

impl<P, C> LxSlintEditor<P, C>
where
    P: Params + 'static,
    C: ComponentHandle + 'static,
{
    pub fn new(
        params: Arc<P>,
        size: (u32, u32),
        build: impl Fn(LxPluginContext<P>) -> C + Send + Sync + 'static,
        sync: impl Fn(&C, &LxPluginContext<P>) + Send + Sync + 'static,
    ) -> Self {
        Self::new_with_zoom(params, UiZoom::new(size.0, size.1), build, sync)
    }

    pub fn new_with_zoom(
        params: Arc<P>,
        ui_zoom: UiZoom,
        build: impl Fn(LxPluginContext<P>) -> C + Send + Sync + 'static,
        sync: impl Fn(&C, &LxPluginContext<P>) + Send + Sync + 'static,
    ) -> Self {
        let design_size = ui_zoom.design_size();
        let zoomed = ui_zoom.zoomed_size();
        Self {
            params,
            design_size,
            build: Arc::new(build),
            sync: Arc::new(sync),
            window: None,
            can_resize: false,
            min_size: zoomed,
            max_size: zoomed,
            ui_zoom,
            pending_size: Arc::new(AtomicU64::new(0)),
        }
    }

    #[must_use]
    pub fn ui_zoom(&self) -> UiZoom {
        self.ui_zoom.clone()
    }

    #[must_use]
    pub fn resizable(mut self, value: bool) -> Self {
        self.can_resize = value;
        let z = self.ui_zoom.zoomed_size();
        self.min_size = z;
        self.max_size = z;
        self
    }

    fn size_policy(&self) -> SizePolicy {
        let zoomed = self.ui_zoom.zoomed_size();
        SizePolicy {
            design_size: self.design_size,
            min_size: zoomed,
            max_size: zoomed,
            can_resize: self.can_resize,
            scale: self.ui_zoom.scale(),
            host_driven_scale: true,
            pending_size: Arc::clone(&self.pending_size),
        }
    }

    fn sync_minmax_to_zoom(&mut self) {
        let z = self.ui_zoom.zoomed_size();
        self.min_size = z;
        self.max_size = z;
    }
}

impl<P, C> Editor for LxSlintEditor<P, C>
where
    P: Params + 'static,
    C: ComponentHandle + 'static,
{
    fn size(&self) -> (u32, u32) {
        self.ui_zoom.zoomed_size()
    }

    fn open(&mut self, parent: RawWindowHandle, context: PluginContext) {
        self.close();
        self.pending_size.store(0, Ordering::Relaxed);
        self.sync_minmax_to_zoom();

        let lx = LxPluginContext::new(self.params.clone(), context.clone());
        let parent_window = parent::ParentedWindow::from_raw(parent);

        let (dw, dh) = self.design_size;
        let content_scale = self.ui_zoom.scale().get();
        let phys_w = to_physical_px(dw, content_scale);
        let phys_h = to_physical_px(dh, content_scale);

        let options = WindowSettings::new()
            .with_title("LX Audiolabs")
            .with_size(baseview::dpi::PhysicalSize::new(phys_w, phys_h))
            .with_gl_config(GlConfig {
                alpha_bits: 8,
                ..GlConfig::default()
            });

        let build = Arc::clone(&self.build);
        let sync = Arc::clone(&self.sync);
        let policy = self.size_policy();
        let params = self.params.clone();

        let request_resize: Option<RequestResizeFn> = {
            let zoom = self.ui_zoom.clone();
            context.bridge.clone().map(|bridge| {
                Arc::new(move |_rw: u32, _rh: u32| {
                    let (w, h) = zoom.zoomed_size();
                    bridge.request_resize(w, h)
                }) as RequestResizeFn
            })
        };

        let window = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            SlintWindow::open_parented_with_policy(
                &parent_window,
                options,
                lx,
                move |state: &mut LxPluginContext<P>| build(state.clone()),
                move |component: &C, state: &mut LxPluginContext<P>| {
                    // Keep host PluginContext sample_rate if present; refresh
                    // params Arc is already typed.
                    let _ = &params;
                    sync(component, state);
                },
                policy,
                request_resize,
            )
        }));

        match window {
            Ok(Ok(w)) => {
                let (zw, zh) = self.ui_zoom.zoomed_size();
                if let Some(b) = &context.bridge {
                    let _ = b.request_resize(zw, zh);
                }
                let _ = w.resize(baseview::dpi::Size::Physical(
                    baseview::dpi::PhysicalSize::new(phys_w, phys_h),
                ));
                self.window = Some(w);
            }
            Ok(Err(e)) => {
                log::error!(
                    "LX UI: OpenGL/FemtoVG init failed — editor left closed (audio still runs). Detail: {e}"
                );
            }
            Err(_) => {
                log::error!(
                    "LX UI: window open panicked — editor left closed. Need OpenGL 3.2 Core. Audio still runs."
                );
            }
        }
    }

    fn close(&mut self) {
        if let Some(window) = self.window.take() {
            window.close();
        }
    }

    fn show(&mut self) {
        if let Some(window) = &self.window {
            let _ = window.show();
        }
    }

    fn hide(&mut self) {
        if let Some(window) = &self.window {
            let _ = window.hide();
        }
    }

    fn can_resize(&self) -> bool {
        self.can_resize && self.min_size != self.max_size
    }

    fn min_size(&self) -> (u32, u32) {
        self.min_size
    }

    fn max_size(&self) -> (u32, u32) {
        self.max_size
    }

    fn set_scale(&mut self, factor: f64) {
        if factor.is_finite() && factor > 0.0 {
            self.ui_zoom.set_host_scale(factor);
        }
    }

    fn set_size(&mut self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        let zoomed = self.ui_zoom.zoomed_size();
        if (width, height) != zoomed {
            self.pending_size
                .store(pack_size(self.design_size), Ordering::Release);
            return false;
        }
        true
    }
}

impl<P, C> From<LxSlintEditor<P, C>> for Box<dyn Editor>
where
    P: Params + 'static,
    C: ComponentHandle + 'static,
{
    fn from(editor: LxSlintEditor<P, C>) -> Self {
        Box::new(editor)
    }
}

// Silence unused import when EditorScale only used via UiZoom re-exports.
#[allow(dead_code)]
fn _scale_ty(_: EditorScale) {}
