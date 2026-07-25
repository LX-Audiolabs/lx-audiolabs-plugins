//! `LxSlintEditor` — truce `Editor` adapter for slint-baseview.
//!
//! Default: FemtoVG + OpenGL (baseview 0.3, slint 1.17.1). Stable in Windows
//! DAW hosts; better cross-compile story than Skia. Swap Cargo feature later
//! for `backend-skia` or `backend-wgpu` A/B.

use std::sync::Arc;

use baseview::{dpi::LogicalSize, gl::GlConfig, Window, WindowSettings};
use slint::ComponentHandle;
use slint_baseview::slint_window::SlintWindow;
use truce_core::editor::{Editor, RawWindowHandle};
use truce_params::Params;

mod parent;

/// Re-export for plugin bind macros (replaces `truce_slint::paste`).
pub use paste::paste;
/// Re-export so plugins need not depend on truce-core editor types directly.
pub use truce_core::editor::PluginContext;

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
