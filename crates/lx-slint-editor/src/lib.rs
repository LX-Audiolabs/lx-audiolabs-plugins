//! `LxSlintEditor` — truce `Editor` adapter for lx-slint-baseview.
//!
//! Default: FemtoVG + OpenGL (baseview 0.3, slint 1.17.1). Stable in Windows
//! DAW hosts; better cross-compile story than Skia. Swap Cargo feature later
//! for `backend-skia` or `backend-wgpu` A/B.
//!
//! HiDPI / multi-monitor: mirrors truce-slint — shared [`EditorScale`],
//! `set_scale_factor` / `set_size`, open-time scale announce, and resize
//! reconcile that keeps logical layout stable across DPI changes.

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

/// Re-export for plugin bind macros (replaces `truce_slint::paste`).
pub use paste::paste;
/// Re-export so plugins need not depend on truce-core editor types directly.
pub use truce_core::editor::PluginContext;
/// OS clipboard helper (vault PASTE button, Ctrl+V inject, etc.).
pub use lx_slint_baseview::platform::clipboard_get_retry;
/// Shared content-scale cell (also used by the window handler).
pub use lx_slint_baseview::EditorScale;

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
    size: (u32, u32),
    build: BuildFn<P, C>,
    sync: SyncFn<P, C>,
    window: Option<Window>,
    can_resize: bool,
    min_size: (u32, u32),
    max_size: (u32, u32),
    /// Live content scale shared with the baseview handler.
    scale: EditorScale,
    /// Host announced a content scale via [`Editor::set_scale_factor`].
    host_scale_set: bool,
    /// Standalone hosts set this so Linux honors desktop scale.
    use_system_scale: bool,
    /// Packed logical size for `Editor::set_size` → handler `on_frame`.
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
        Self {
            params,
            size,
            build: Arc::new(build),
            sync: Arc::new(sync),
            window: None,
            can_resize: false,
            // Fixed-size default: min = max = design size (no stretch).
            min_size: size,
            max_size: size,
            scale: EditorScale::new(1.0),
            host_scale_set: false,
            use_system_scale: false,
            pending_size: Arc::new(AtomicU64::new(0)),
        }
    }

    #[must_use]
    pub fn resizable(mut self, value: bool) -> Self {
        // Report can_resize to the host if requested, but keep min=max=design
        // so format wrappers (CLAP fit_logical_size / VST3 checkSizeConstraint)
        // cannot force a stretch reflow. LX Slint UIs are fixed-layout.
        self.can_resize = value;
        self.min_size = self.size;
        self.max_size = self.size;
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
        // Plugins: content scale is host set_scale only (default 1.0), never
        // raw OS DPI. CLAP/VST3 report HWND size as size()*host_scale; the
        // child must use the same pixel size. Following GetDpiForWindow made
        // a larger child than the host frame → clipped top + gray margins.
        // Standalone may opt into system scale via set_uses_system_scale.
        let host_driven_scale = !self.use_system_scale;
        SizePolicy {
            design_size: self.size,
            min_size: self.min_size,
            max_size: self.max_size,
            can_resize: self.can_resize,
            scale: self.scale.clone(),
            host_driven_scale,
            pending_size: Arc::clone(&self.pending_size),
        }
    }
}

impl<P, C> Editor for LxSlintEditor<P, C>
where
    P: Params + 'static,
    C: ComponentHandle + 'static,
{
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn open(&mut self, parent: RawWindowHandle, context: PluginContext) {
        // Drop previous window if host re-opens without close.
        self.close();

        // Reset stale set_size from a previous open.
        self.pending_size.store(0, Ordering::Relaxed);

        let ctx = context.with_params(self.params.clone());
        let parent_window = parent::ParentedWindow::from_raw(parent);

        let (w, h) = self.size;
        // Physical pixels = design × content scale (host_scale, default 1.0).
        // Do NOT pass LogicalSize — baseview would multiply by OS DPI and the
        // child would no longer match CLAP get_size (logical × host_scale).
        let content_scale = self.scale.get();
        let phys_w = to_physical_px(w, content_scale);
        let phys_h = to_physical_px(h, content_scale);
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

        // Host resize callback (corrective DPI / fixed-size push-back).
        // Cloned PluginContext is Arc-based and Send.
        let request_resize = {
            let resize_ctx = ctx.clone();
            Some(Arc::new(move |rw: u32, rh: u32| resize_ctx.request_resize(rw, rh))
                as lx_slint_baseview::RequestResizeFn)
        };

        let window = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Prefer host scale as fallback when the platform has none yet
            // (old Win10 / X11 without Xft.dpi). No-op when OS already knows.
            let host_scale = self.scale.get();
            let opened = SlintWindow::open_parented_with_policy(
                &parent_window,
                options,
                ctx.clone(),
                move |state: &mut PluginContext<P>| build(state.clone()),
                move |component: &C, state: &mut PluginContext<P>| {
                    sync(component, state);
                },
                policy,
                request_resize,
            );
            // Do not suggest_fallback_scale_factor here: that would reintroduce
            // OS DPI into a surface that must stay size()*host_scale pixels.
            let _ = host_scale;
            opened
        }));

        match window {
            Ok(Ok(w)) => {
                // Snap host frame to design size after open (outside host open).
                let (lw, lh) = self.size;
                let _ = ctx.request_resize(lw, lh);
                let _ = w.resize(baseview::dpi::Size::Physical(
                    baseview::dpi::PhysicalSize::new(phys_w, phys_h),
                ));
                self.window = Some(w);
            }
            Ok(Err(e)) => log::error!("LxSlintEditor::open failed: {e}"),
            Err(_) => log::error!("LxSlintEditor::open panicked"),
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
        // Shared cell; the live handler reconciles Slint on the next frame.
        if factor.is_finite() && factor > 0.0 {
            self.host_scale_set = true;
            self.scale.set(factor);
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
        // size; tell the host we did not accept a different size.
        if (width, height) != self.size {
            // Re-assert design size so the handler pushes the child/host back.
            self.pending_size
                .store(pack_size(self.size), Ordering::Release);
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

        // Scale: prefer live content scale, else DEFAULT_SCREENSHOT_SCALE (2.0).
        let scale = {
            let s = self.scale.get();
            if s.is_finite() && s > 0.0 && (s - 1.0).abs() > 1.0e-6 {
                s
            } else {
                2.0
            }
        };
        let (w, h) = self.size;
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


