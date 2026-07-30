//! Bridge from truce `RawWindowHandle` to `raw_window_handle 0.6`.
//!
//! baseview 0.3 / slint-baseview expect rwh 0.6 `HasWindowHandle`.

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, WindowHandle,
};
#[cfg(target_os = "macos")]
use raw_window_handle::AppKitWindowHandle;
#[cfg(target_os = "linux")]
use raw_window_handle::{RawDisplayHandle, XlibDisplayHandle, XlibWindowHandle};
use std::num::NonZero;
use truce_core::editor::RawWindowHandle as TruceRaw;

/// A window handle that implements `HasWindowHandle` (rwh 0.6) for any
/// platform, constructed from truce's `RawWindowHandle` enum.
pub(crate) struct ParentedWindow(RawWindowHandle);

impl ParentedWindow {
    /// Create from truce's platform-specific raw handle.
    pub(crate) fn from_raw(raw: TruceRaw) -> Self {
        let handle = match raw {
            #[cfg(target_os = "windows")]
            TruceRaw::Win32(hwnd) => {
                let h = NonZero::new(hwnd as _).expect("HWND must not be null");
                RawWindowHandle::Win32(
                    raw_window_handle::Win32WindowHandle::new(h),
                )
            }
            #[cfg(target_os = "macos")]
            TruceRaw::AppKit(ns_view) => {
                let p = NonZero::new(ns_view).expect("NSView must not be null");
                RawWindowHandle::AppKit(
                    AppKitWindowHandle::new(p),
                )
            }
            #[cfg(target_os = "linux")]
            TruceRaw::X11(xid) => {
                let id = NonZero::new(xid as u32).expect("X11 Window ID must not be null");
                RawWindowHandle::Xlib(
                    XlibWindowHandle::new(id),
                )
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unsupported platform for parent window"),
        };
        Self(handle)
    }
}

impl HasWindowHandle for ParentedWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the raw handle is valid for the window's lifetime
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

impl HasDisplayHandle for ParentedWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        #[cfg(target_os = "linux")]
        {
            // X11 display — use a default display handle.
            // The actual display connection is owned by the host.
            Ok(unsafe {
                DisplayHandle::borrow_raw(RawDisplayHandle::Xlib(
                    XlibDisplayHandle::new(None, 0),
                ))
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            // On Windows/macOS, baseview doesn't need a display handle
            // for surface creation; wgpu uses the default.
            Err(HandleError::Unavailable)
        }
    }
}
