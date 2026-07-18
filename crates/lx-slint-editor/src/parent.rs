//! Bridge truce `RawWindowHandle` → baseview's rwh 0.5 `HasRawWindowHandle`.

use raw_window_handle::{
    HasRawWindowHandle, RawWindowHandle as RwhRawWindowHandle,
};
use truce_core::editor::RawWindowHandle;

pub struct ParentWindow(pub RawWindowHandle);

// SAFETY: host guarantees the parent handle is valid for the editor lifetime.
unsafe impl HasRawWindowHandle for ParentWindow {
    fn raw_window_handle(&self) -> RwhRawWindowHandle {
        match self.0 {
            RawWindowHandle::AppKit(ptr) => {
                let mut handle = raw_window_handle::AppKitWindowHandle::empty();
                handle.ns_view = ptr;
                RwhRawWindowHandle::AppKit(handle)
            }
            RawWindowHandle::UiKit(ptr) => {
                let mut handle = raw_window_handle::UiKitWindowHandle::empty();
                handle.ui_view = ptr;
                RwhRawWindowHandle::UiKit(handle)
            }
            RawWindowHandle::Win32(ptr) => {
                let mut handle = raw_window_handle::Win32WindowHandle::empty();
                handle.hwnd = ptr;
                RwhRawWindowHandle::Win32(handle)
            }
            RawWindowHandle::X11(window_id) => {
                let mut handle = raw_window_handle::XlibWindowHandle::empty();
                #[allow(clippy::cast_possible_truncation)]
                {
                    handle.window = window_id as _;
                }
                RwhRawWindowHandle::Xlib(handle)
            }
        }
    }
}
