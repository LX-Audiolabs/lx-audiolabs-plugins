//! Bridge from `aura_core` `RawWindowHandle` to `raw_window_handle 0.6`.

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, WindowHandle,
};
#[cfg(target_os = "macos")]
use raw_window_handle::AppKitWindowHandle;
#[cfg(target_os = "linux")]
use raw_window_handle::{RawDisplayHandle, XlibDisplayHandle, XlibWindowHandle};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::num::NonZero;
use aura_core::editor::RawWindowHandle as AuraRaw;

/// Window handle implementing `HasWindowHandle` for baseview parenting.
pub(crate) struct ParentedWindow(RawWindowHandle);

impl ParentedWindow {
    pub(crate) fn from_raw(raw: AuraRaw) -> Self {
        let handle = match raw {
            #[cfg(target_os = "windows")]
            AuraRaw::Win32(hwnd) => {
                let h = NonZero::new(hwnd as _).expect("HWND must not be null");
                RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(h))
            }
            #[cfg(target_os = "macos")]
            AuraRaw::AppKit(ns_view) => {
                let p = NonZero::new(ns_view).expect("NSView must not be null");
                RawWindowHandle::AppKit(AppKitWindowHandle::new(p))
            }
            #[cfg(target_os = "linux")]
            AuraRaw::X11(xid) => {
                assert!(xid != 0, "X11 Window ID must not be null");
                RawWindowHandle::Xlib(XlibWindowHandle::new(xid))
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unsupported platform for parent window"),
        };
        Self(handle)
    }
}

impl HasWindowHandle for ParentedWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: raw handle valid for host parent lifetime
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

impl HasDisplayHandle for ParentedWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        #[cfg(target_os = "linux")]
        {
            Ok(unsafe {
                DisplayHandle::borrow_raw(RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0)))
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(HandleError::Unavailable)
        }
    }
}
