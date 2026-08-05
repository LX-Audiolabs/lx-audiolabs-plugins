//! `LxSlintEditor` — truce `Editor` adapter for lx-slint-baseview.
//!
//! Default: FemtoVG + OpenGL (baseview 0.3, slint 1.17.1). Stable in Windows
//! DAW hosts; better cross-compile story than Skia. Swap Cargo feature later
//! for `backend-skia` or `backend-wgpu` A/B.
//!
//! HiDPI / multi-monitor: mirrors truce-slint — shared [`EditorScale`],
//! `set_scale_factor` / `set_size`, open-time scale announce, and resize
//! reconcile that keeps logical layout stable across DPI changes.
//!
//! Product UI zoom (75/100/125%): layout stays at design size; effective
//! content scale is `host_scale × ui_zoom`. Host frame is `design × ui_zoom`.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use baseview::{gl::GlConfig, Window, WindowSettings};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::ComponentHandle;
use lx_slint_baseview::slint_window::SlintWindow;
use lx_slint_baseview::{pack_size, to_physical_px, SizePolicy};
use truce_core::editor::{Editor, RawWindowHandle};
use truce_params::Params;

mod parent;
mod ui_zoom;

/// Re-export for plugin bind macros (replaces `truce_slint::paste`).
pub use paste::paste;
/// Re-export so plugins need not depend on truce-core editor types directly.
pub use truce_core::editor::PluginContext;
/// OS clipboard helper (vault PASTE button, Ctrl+V inject, etc.).
pub use lx_slint_baseview::platform::clipboard_get_retry;
/// Shared content-scale cell (also used by the window handler).
pub use lx_slint_baseview::EditorScale;
/// Product UI zoom handle (75/100/125%) shared with logo menu callbacks.
pub use ui_zoom::{apply_ui_zoom, UiZoom, UI_ZOOM_DEFAULT, UI_ZOOM_STEPS};

/// Build closure: creates the Slint component and wires UI callbacks.
pub type BuildFn<P, C> = Arc<dyn Fn(PluginContext<P>) -> C + Send + Sync>;

/// Per-frame sync closure: pushes host parameter values into the component.
pub type SyncFn<P, C> = Arc<dyn Fn(&C, &PluginContext<P>) + Send + Sync>;

/// Slint-based editor implementing truce's `Editor` trait.
///
/// Generic over the concrete Slint component type, because the GPU-backed
/// `lx_slint_baseview::SlintWindow` needs to own the generated component.
///
/// # Example
///
/// ```ignore
/// use lx_slint_editor::LxSlintEditor;
///
/// fn editor(params: Arc<MyParams>) -> Box<dyn Editor> {
///     LxSlintEditor::new(
///         params,
///         (800, 600),
///         |ctx| {
///             let ui = MyPluginUi::new().unwrap();
///             let s = ctx.clone();
///             ui.on_gain_changed(move |v| s.automate(P::Gain, v as f64));
///             ui
///         },
///         |ui, ctx| {
///             ui.set_gain(ctx.get_param(P::Gain) as f32);
///         },
///     )
///     .resizable(true)
///     .into()
/// }
/// ```
pub struct LxSlintEditor<P, C>
where
    P: Params + ?Sized + 'static,
    C: ComponentHandle + 'static,
{
    params: Arc<P>,
    /// Design logical size (layout coordinates — never reflowed by UI zoom).
    design_size: (u32, u32),
    build: BuildFn<P, C>,
    sync: SyncFn<P, C>,
    window: Option<Window>,
    can_resize: bool,
    min_size: (u32, u32),
    max_size: (u32, u32),
    /// Product UI zoom + effective content scale (`host × zoom`).
    ui_zoom: UiZoom,
    /// Host announced a content scale via [`Editor::set_scale_factor`].
    host_scale_set: bool,
    /// Standalone hosts set this so Linux honors desktop scale.
    use_system_scale: bool,
    /// Packed **design** logical size for handler reconcile (not host frame).
    pending_size: Arc<AtomicU64>,
}

// SAFETY: baseview::Window holds raw native window pointers (HWND/NSView)
// and is not auto-Send. Hosts call Editor::open/close from a single GUI
// thread, never concurrently. Same pattern as truce-slint, truce-egui, truce-iced.
unsafe impl<P, C> Send for LxSlintEditor<P, C>
where
    P: Params + ?Sized + 'static,
    C: ComponentHandle + 'static,
{
}

impl<P, C> LxSlintEditor<P, C>
where
    P: Params + ?Sized + 'static,
    C: ComponentHandle + 'static,
{
    /// Create a new editor adapter.
    ///
    /// - `build` is called once when the host opens the editor. It receives a
    ///   `PluginContext` and must return the concrete Slint component.
    /// - `sync` is called every frame to copy host parameter values into the
    ///   component.
    pub fn new(
        params: Arc<P>,
        size: (u32, u32),
        build: impl Fn(PluginContext<P>) -> C + Send + Sync + 'static,
        sync: impl Fn(&C, &PluginContext<P>) + Send + Sync + 'static,
    ) -> Self {
        Self::new_with_zoom(params, UiZoom::new(size.0, size.1), build, sync)
    }

    /// Like [`Self::new`], but takes a pre-built [`UiZoom`] so the `build`
    /// closure can clone it and wire the logo zoom menu.
    ///
    /// ```ignore
    /// let zoom = UiZoom::new(990, 670);
    /// let z = zoom.clone();
    /// LxSlintEditor::new_with_zoom(params, zoom, move |state| {
    ///     let ui = MyUi::new().unwrap();
    ///     ui.set_ui_zoom_percent(z.percent() as i32);
    ///     let z2 = z.clone();
    ///     let s = state.clone();
    ///     ui.on_ui_zoom_changed(move |p| {
    ///         z2.set_percent(p as u32);
    ///         let (w, h) = z2.zoomed_size();
    ///         let _ = s.request_resize(w, h);
    ///     });
    ///     ui
    /// }, sync)
    /// ```
    pub fn new_with_zoom(
        params: Arc<P>,
        ui_zoom: UiZoom,
        build: impl Fn(PluginContext<P>) -> C + Send + Sync + 'static,
        sync: impl Fn(&C, &PluginContext<P>) + Send + Sync + 'static,
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
            // Host min/max track zoomed frame (fixed-layout: min == max == size()).
            min_size: zoomed,
            max_size: zoomed,
            ui_zoom,
            host_scale_set: false,
            use_system_scale: false,
            pending_size: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Shared UI-zoom handle (same cell as the live editor).
    #[must_use]
    pub fn ui_zoom(&self) -> UiZoom {
        self.ui_zoom.clone()
    }

    #[must_use]
    pub fn resizable(mut self, value: bool) -> Self {
        // Report can_resize to the host if requested, but keep min=max=zoomed
        // so format wrappers (CLAP fit_logical_size / VST3 checkSizeConstraint)
        // cannot force a stretch reflow. LX Slint UIs are fixed-layout.
        self.can_resize = value;
        let z = self.ui_zoom.zoomed_size();
        self.min_size = z;
        self.max_size = z;
        self
    }

    #[must_use]
    pub fn with_min_size(mut self, min: (u32, u32)) -> Self {
        self.min_size = min;
        self
    }

    #[must_use]
    pub fn with_max_size(mut self, max: (u32, u32)) -> Self {
        self.max_size = max;
        self
    }

    fn size_policy(&self) -> SizePolicy {
        // Plugins: content scale is host set_scale × ui_zoom (default 1.0),
        // never raw OS DPI (Xft.dpi / GetDpiForWindow). CLAP/VST3 report HWND
        // size as size()*host_scale where size() is design×ui_zoom; physical
        // child is design × (host×ui_zoom) = size() × host_scale.
        // Layout logical size stays at design (no control reflow).
        // Linux: lx-slint-baseview skips continuous host request_resize
        // push-back (Bitwig grows the frame in a fight). Standalone may opt
        // into system scale via set_uses_system_scale.
        let host_driven_scale = !self.use_system_scale;
        let zoomed = self.ui_zoom.zoomed_size();
        SizePolicy {
            // Slint layout coordinates = design (not host frame).
            design_size: self.design_size,
            min_size: zoomed,
            max_size: zoomed,
            can_resize: self.can_resize,
            scale: self.ui_zoom.scale(),
            host_driven_scale,
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
        // Host frame = design × ui_zoom (layout still uses design_size).
        self.ui_zoom.zoomed_size()
    }

    fn open(&mut self, parent: RawWindowHandle, context: PluginContext) {
        // Drop previous window if host re-opens without close.
        self.close();

        // Reset stale set_size from a previous open.
        self.pending_size.store(0, Ordering::Relaxed);
        self.sync_minmax_to_zoom();

        let ctx = context.with_params(self.params.clone());
        let parent_window = parent::ParentedWindow::from_raw(parent);

        // Physical = design × (host_scale × ui_zoom) = zoomed_size × host_scale.
        let (dw, dh) = self.design_size;
        let content_scale = self.ui_zoom.scale().get();
        let phys_w = to_physical_px(dw, content_scale);
        let phys_h = to_physical_px(dh, content_scale);
        // FemtoVG needs OpenGL (baseview opengl feature).
        // alpha_bits=8 helps embedded plugin windows in DAW hosts.
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

        // Host push-back must use zoomed frame size, not design logical.
        // Baseview reconcile passes design (layout) size into this callback.
        let request_resize = {
            let resize_ctx = ctx.clone();
            let zoom = self.ui_zoom.clone();
            Some(Arc::new(move |_rw: u32, _rh: u32| {
                let (w, h) = zoom.zoomed_size();
                resize_ctx.request_resize(w, h)
            }) as lx_slint_baseview::RequestResizeFn)
        };

        let window = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            SlintWindow::open_parented_with_policy(
                &parent_window,
                options,
                ctx.clone(),
                move |state: &mut PluginContext<P>| build(state.clone()),
                move |component: &C, state: &mut PluginContext<P>| {
                    sync(component, state);
                },
                policy,
                request_resize,
            )
        }));

        match window {
            Ok(Ok(w)) => {
                // Snap host frame to zoomed size after open (outside host open).
                let (zw, zh) = self.ui_zoom.zoomed_size();
                let _ = ctx.request_resize(zw, zh);
                let _ = w.resize(baseview::dpi::Size::Physical(
                    baseview::dpi::PhysicalSize::new(phys_w, phys_h),
                ));
                self.window = Some(w);
            }
            // Soft-fail: editor stays closed; DSP continues. Typical on old Linux/mac
            // without OpenGL 3.2 Core / broken GLX embeds in REAPER.
            Ok(Err(e)) => {
                log::error!(
                    "LX UI: OpenGL 3.2 Core unavailable or FemtoVG init failed — \
                     editor left closed (audio still runs). Detail: {e}"
                );
            }
            Err(_) => {
                log::error!(
                    "LX UI: OpenGL/FemtoVG open panicked — editor left closed. \
                     Need OpenGL 3.2 Core (or newer). Audio still runs."
                );
            }
        }
    }

    fn close(&mut self) {
        if let Some(window) = self.window.take() {
            window.close();
        }
    }

    fn can_resize(&self) -> bool {
        // Fixed-layout UIs keep min==max==design; report non-resizable so
        // hosts use the fixed-size path (no stretch negotiation).
        self.can_resize && self.min_size != self.max_size
    }

    fn min_size(&self) -> (u32, u32) {
        self.min_size
    }

    fn max_size(&self) -> (u32, u32) {
        self.max_size
    }

    fn set_scale_factor(&mut self, factor: f64) {
        // Host HiDPI only — ui_zoom multiplies on top (shared effective cell).
        if factor.is_finite() && factor > 0.0 {
            self.host_scale_set = true;
            self.ui_zoom.set_host_scale(factor);
        }
    }

    fn set_uses_system_scale(&mut self, yes: bool) {
        self.use_system_scale = yes;
    }

    fn set_size(&mut self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        // LX UIs are fixed-layout (stretch bands, absolute chrome). Host
        // set_size from multi-monitor / pane chrome must NOT reflow the
        // scene — that was the "nice stretching" regression. Keep design
        // layout; host frame is zoomed size only.
        let zoomed = self.ui_zoom.zoomed_size();
        if (width, height) != zoomed {
            // Re-assert design logical for Slint; host push-back uses zoomed.
            self.pending_size
                .store(pack_size(self.design_size), Ordering::Release);
            return false;
        }
        true
    }

    fn screenshot(
        &mut self,
        _params: Arc<dyn truce_params::Params>,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let state = truce_core::editor::for_test_params(self.params.clone())
            .with_params(self.params.clone());

        lx_slint_baseview::platform::ensure_platform();

        // Create software renderer window, keep Rc for direct draw_if_needed.
        // MinimalSoftwareWindow::new() already returns Rc<MinimalSoftwareWindow>.
        let msw = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        // Hand off a clone to the platform so Component::new() attaches to it.
        let adapter: Rc<dyn slint::platform::WindowAdapter> = msw.clone();
        lx_slint_baseview::platform::set_next_adapter(adapter);

        // Build the Slint component (attaches to the MinimalSoftwareWindow via platform).
        let component = (self.build)(state.clone());

        // Sync host params into the component so labels show defaults.
        (self.sync)(&component, &state);

        // Scale: prefer live effective scale, else DEFAULT_SCREENSHOT_SCALE (2.0).
        let scale = {
            let s = self.ui_zoom.scale().get();
            if s.is_finite() && s > 0.0 && (s - 1.0).abs() > 1.0e-6 {
                s
            } else {
                2.0
            }
        };
        // Screenshot renders design logical at the effective scale.
        let (w, h) = self.design_size;
        let phys_w = to_physical_px(w, scale);
        let phys_h = to_physical_px(h, scale);

        let slint_window = component.window();
        slint_window.set_size(slint::WindowSize::Physical(
            slint::PhysicalSize::new(phys_w, phys_h),
        ));
        slint_window.dispatch_event(
            slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: scale as f32,
            },
        );

        // Render via the MinimalSoftwareWindow directly.
        let pixel_count = (phys_w * phys_h) as usize;
        let mut px_buf = vec![PremultipliedRgbaColor::default(); pixel_count];

        let drew = msw.draw_if_needed(|renderer| {
            renderer.render(&mut px_buf, phys_w as usize);
        });
        if !drew {
            return None;
        }

        // Un-premultiply to straight RGBA.
        let inv_lut: [u32; 256] = {
            let mut lut = [0u32; 256];
            let mut i = 1u32;
            while i < 256 {
                lut[i as usize] = u32::MAX / i;
                i += 1;
            }
            lut
        };

        let mut rgba = Vec::with_capacity(pixel_count * 4);
        for px in &px_buf {
            if px.alpha == 0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else if px.alpha == 255 {
                rgba.extend_from_slice(&[px.red, px.green, px.blue, 255]);
            } else {
                #[allow(clippy::cast_possible_truncation)]
                let inv_a = inv_lut[px.alpha as usize];
                rgba.push(((u32::from(px.red) * inv_a) >> 16) as u8);
                rgba.push(((u32::from(px.green) * inv_a) >> 16) as u8);
                rgba.push(((u32::from(px.blue) * inv_a) >> 16) as u8);
                rgba.push(px.alpha);
            }
        }

        Some((rgba, phys_w, phys_h))
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


