//! `LxSlintEditor` — truce `Editor` adapter for slint-baseview.
//!
//! Uses our slint-baseview (baseview 0.3, slint 1.17.1, multi-backend)
//! instead of baseview-truce 0.1.1.

use std::sync::Arc;

use baseview::Window;
use truce_core::editor::{Editor, PluginContext, RawWindowHandle};
use truce_params::Params;

mod parent;

pub type SetupFn<P> = Arc<dyn Fn(PluginContext<P>) -> SyncFn<P> + Send + Sync>;
pub type SyncFn<P> = Box<dyn Fn(&PluginContext<P>)>;

#[allow(dead_code)]
pub struct LxSlintEditor<P: Params + ?Sized> {
    params: Arc<P>,
    size: (u32, u32),
    setup: SetupFn<P>,
    window: Option<Window>,
    can_resize: bool,
}

// SAFETY: baseview::Window holds raw native window pointers (HWND/NSView)
// and is not auto-Send. Hosts call Editor::open/close from a single GUI
// thread, never concurrently. Same pattern as truce-slint, truce-egui, truce-iced.
unsafe impl<P: Params + ?Sized> Send for LxSlintEditor<P> {}

impl<P: Params + ?Sized> LxSlintEditor<P> {
    pub fn new(
        params: Arc<P>,
        size: (u32, u32),
        setup: impl Fn(PluginContext<P>) -> SyncFn<P> + Send + Sync + 'static,
    ) -> Self {
        Self {
            params,
            size,
            setup: Arc::new(setup),
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

impl<P: Params + 'static> Editor for LxSlintEditor<P> {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn open(&mut self, _parent: RawWindowHandle, _context: PluginContext) {
        // ponytail: SlintWindow::open_parented needs concrete component type.
        // For now, stub — wiring comes when plugins migrate.
        log::info!("LxSlintEditor::open — skeleton, not yet wired");
    }

    fn close(&mut self) {
        self.window.take();
    }

    fn can_resize(&self) -> bool {
        self.can_resize
    }
}
