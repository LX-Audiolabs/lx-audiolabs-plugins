//! `LxSlintEditor` — truce `Editor` adapter for slint-baseview.
//!
//! Default: FemtoVG + OpenGL (baseview 0.3, slint 1.17.1). Stable in Windows
//! DAW hosts; better cross-compile story than Skia. Swap Cargo feature later
//! for `backend-skia` or `backend-wgpu` A/B.

use std::rc::Rc;
use std::sync::Arc;

use baseview::{dpi::LogicalSize, gl::GlConfig, Window, WindowSettings};
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint_baseview::slint_window::SlintWindow;
use truce_core::editor::{Editor, RawWindowHandle};
use truce_params::Params;

mod parent;

/// Re-export for plugin bind macros (replaces `truce_slint::paste`).
pub use paste::paste;
/// Re-export so plugins need not depend on truce-core editor types directly.
pub use truce_core::editor::PluginContext;
/// OS clipboard helpers (vault PASTE button, Ctrl+V inject, etc.).
pub use slint_baseview::platform::{clipboard_get, clipboard_get_retry, clipboard_set};

/// Build closure: creates the Slint component and wires UI callbacks.
pub type BuildFn<P, C> = Arc<dyn Fn(PluginContext<P>) -> C + Send + Sync>;

/// Per-frame sync closure: pushes host parameter values into the component.
pub type SyncFn<P, C> = Arc<dyn Fn(&C, &PluginContext<P>) + Send + Sync>;

/// Slint-based editor implementing truce's `Editor` trait.
///
/// Generic over the concrete Slint component type, because the GPU-backed
/// `slint_baseview::SlintWindow` needs to own the generated component.
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
        }
    }

    #[must_use]
    pub fn resizable(mut self, value: bool) -> Self {
        self.can_resize = value;
        self
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

        let ctx = context.with_params(self.params.clone());
        let parent_window = parent::ParentedWindow::from_raw(parent);

        let (w, h) = self.size;
        // FemtoVG needs OpenGL (baseview opengl feature).
        // alpha_bits=8 helps embedded plugin windows in DAW hosts.
        let options = WindowSettings::new()
            .with_title("LX Audiolabs")
            .with_size(LogicalSize::new(f64::from(w), f64::from(h)))
            .with_gl_config(GlConfig {
                alpha_bits: 8,
                ..GlConfig::default()
            });

        let build = Arc::clone(&self.build);
        let sync = Arc::clone(&self.sync);

        let window = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            SlintWindow::open_parented(
                &parent_window,
                options,
                ctx.clone(),
                move |state: &mut PluginContext<P>| build(state.clone()),
                move |component: &C, state: &mut PluginContext<P>| {
                    sync(component, state);
                },
            )
        }));

        match window {
            Ok(Ok(w)) => self.window = Some(w),
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
        self.can_resize
    }

    fn screenshot(
        &mut self,
        _params: Arc<dyn truce_params::Params>,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let state = truce_core::editor::for_test_params(self.params.clone())
            .with_params(self.params.clone());

        slint_baseview::platform::ensure_platform();

        // Create software renderer window, keep Rc for direct draw_if_needed.
        // MinimalSoftwareWindow::new() already returns Rc<MinimalSoftwareWindow>.
        let msw = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        // Hand off a clone to the platform so Component::new() attaches to it.
        let adapter: Rc<dyn slint::platform::WindowAdapter> = msw.clone();
        slint_baseview::platform::set_next_adapter(adapter);

        // Build the Slint component (attaches to the MinimalSoftwareWindow via platform).
        let component = (self.build)(state.clone());

        // Sync host params into the component so labels show defaults.
        (self.sync)(&component, &state);

        // Scale per DEFAULT_SCREENSHOT_SCALE (2.0).
        let scale: f64 = 2.0;
        let (w, h) = self.size;
        let phys_w = (w as f64 * scale).round() as u32;
        let phys_h = (h as f64 * scale).round() as u32;

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
