//! `SlintEditor`: truce `Editor` using baseview + FemtoVG OpenGL.

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use baseview::{
    Event, EventStatus, Size, Window, WindowHandle, WindowHandler, WindowOpenOptions,
    WindowScalePolicy,
};
use slint::platform::WindowEvent;
use slint::{LogicalPosition, SharedString};
use truce_core::editor::{Editor, PluginContext, RawWindowHandle};
use truce_params::Params;

use crate::parent::ParentWindow;
use crate::platform::{
    default_gl_config, ensure_platform, BaseviewSlintAdapter, CurrentAdapterGuard,
};
use crate::translate;

/// Per-frame host→UI sync. Lives on the baseview UI thread only.
pub type SyncFn<P> = Box<dyn Fn(&PluginContext<P>)>;

/// Called on each editor open (UI thread). Build Slint UI + return sync.
pub type SetupFn<P> = Arc<dyn Fn(PluginContext<P>) -> SyncFn<P> + Send + Sync>;

pub struct SlintEditor<P: Params + ?Sized> {
    params: Arc<P>,
    size: (u32, u32),
    setup: SetupFn<P>,
    window: Option<WindowHandle>,
    scale: f64,
    host_scale_set: bool,
    pending_size: Arc<AtomicU64>,
    can_resize: bool,
    can_maximize: bool,
    min_size: (u32, u32),
    max_size: (u32, u32),
    aspect_ratio: Option<(u32, u32)>,
    prefers_pow2: bool,
}

// SAFETY: same as truce-slint — host only uses Editor on one GUI thread.
unsafe impl<P: Params + ?Sized> Send for SlintEditor<P> {}

impl<P: Params + 'static> SlintEditor<P> {
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
            scale: 1.0,
            host_scale_set: false,
            pending_size: Arc::new(AtomicU64::new(0)),
            can_resize: false,
            can_maximize: false,
            min_size: (1, 1),
            max_size: (u32::MAX, u32::MAX),
            aspect_ratio: None,
            prefers_pow2: false,
        }
    }

    #[must_use]
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.can_resize = resizable;
        self
    }

    #[must_use]
    pub fn maximizable(mut self, maximizable: bool) -> Self {
        self.can_maximize = maximizable;
        self
    }

    #[must_use]
    pub fn min_size(mut self, min: (u32, u32)) -> Self {
        self.min_size = min;
        self
    }

    #[must_use]
    pub fn max_size(mut self, max: (u32, u32)) -> Self {
        self.max_size = max;
        self
    }

    #[must_use]
    pub fn aspect_ratio(mut self, ratio: Option<(u32, u32)>) -> Self {
        self.aspect_ratio = ratio;
        self
    }

    #[must_use]
    pub fn prefers_pow2(mut self, prefers: bool) -> Self {
        self.prefers_pow2 = prefers;
        self
    }
}

fn pack_size(size: (u32, u32)) -> u64 {
    (u64::from(size.0) << 32) | u64::from(size.1)
}

fn unpack_size(packed: u64) -> (u32, u32) {
    #[allow(clippy::cast_possible_truncation)]
    {
        ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32)
    }
}

fn panic_message(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

struct LiveHandler<P: Params + ?Sized> {
    adapter: Rc<BaseviewSlintAdapter>,
    sync_fn: SyncFn<P>,
    state: PluginContext<P>,
    pending_size: Arc<AtomicU64>,
    logical_w: u32,
    logical_h: u32,
    scale: f32,
    last_cursor: RefCell<LogicalPosition>,
    ready: bool,
}

enum HandlerState<P: Params + ?Sized> {
    Dead,
    Live(LiveHandler<P>),
}

impl<P: Params + 'static> WindowHandler for HandlerState<P> {
    fn on_frame(&mut self, window: &mut Window) {
        let HandlerState::Live(h) = self else {
            return;
        };

        unsafe {
            if let Some(ctx) = window.gl_context() {
                ctx.make_current();
            }
        }

        let packed = h.pending_size.swap(0, Ordering::AcqRel);
        if packed != 0 {
            let (w, hgt) = unpack_size(packed);
            h.logical_w = w;
            h.logical_h = hgt;
            let scale = h.scale.max(0.1);
            let pw = ((w as f32) * scale).round().max(1.0) as u32;
            let ph = ((hgt as f32) * scale).round().max(1.0) as u32;
            h.adapter.update_size(pw, ph, scale);
            h.adapter
                .slint_window()
                .dispatch_event(WindowEvent::Resized {
                    size: slint::LogicalSize::new(w as f32, hgt as f32),
                });
            window.resize(Size {
                width: f64::from(w),
                height: f64::from(hgt),
            });
        }

        if !h.ready {
            h.adapter.set_gl_context(window);
            let _ = h.adapter.ensure_renderer();
            h.adapter
                .slint_window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: h.scale,
                });
            h.ready = true;
        }

        (h.sync_fn)(&h.state);
        slint::platform::update_timers_and_animations();
        h.adapter.slint_window().request_redraw();
        let _ = h.adapter.ensure_renderer().render();
        if let Some(ctx) = window.gl_context() {
            ctx.swap_buffers();
        }
    }

    fn on_event(&mut self, _window: &mut Window, event: Event) -> EventStatus {
        let HandlerState::Live(h) = self else {
            return EventStatus::Ignored;
        };
        let win = h.adapter.slint_window();

        match event {
            Event::Mouse(mouse_event) => {
                let slint_event = match mouse_event {
                    baseview::MouseEvent::CursorMoved { position, .. } => {
                        let pos = LogicalPosition::new(position.x as f32, position.y as f32);
                        *h.last_cursor.borrow_mut() = pos;
                        WindowEvent::PointerMoved { position: pos }
                    }
                    baseview::MouseEvent::ButtonPressed { button, .. } => {
                        let button = match button {
                            baseview::MouseButton::Left => {
                                slint::platform::PointerEventButton::Left
                            }
                            baseview::MouseButton::Right => {
                                slint::platform::PointerEventButton::Right
                            }
                            baseview::MouseButton::Middle => {
                                slint::platform::PointerEventButton::Middle
                            }
                            _ => return EventStatus::Ignored,
                        };
                        WindowEvent::PointerPressed {
                            button,
                            position: *h.last_cursor.borrow(),
                        }
                    }
                    baseview::MouseEvent::ButtonReleased { button, .. } => {
                        let button = match button {
                            baseview::MouseButton::Left => {
                                slint::platform::PointerEventButton::Left
                            }
                            baseview::MouseButton::Right => {
                                slint::platform::PointerEventButton::Right
                            }
                            baseview::MouseButton::Middle => {
                                slint::platform::PointerEventButton::Middle
                            }
                            _ => return EventStatus::Ignored,
                        };
                        WindowEvent::PointerReleased {
                            button,
                            position: *h.last_cursor.borrow(),
                        }
                    }
                    baseview::MouseEvent::WheelScrolled { delta, .. } => {
                        let (delta_x, delta_y) = match delta {
                            baseview::ScrollDelta::Lines { x, y } => (x * 20.0, y * 20.0),
                            baseview::ScrollDelta::Pixels { x, y } => (x, y),
                        };
                        WindowEvent::PointerScrolled {
                            position: *h.last_cursor.borrow(),
                            delta_x,
                            delta_y,
                        }
                    }
                    baseview::MouseEvent::CursorLeft => WindowEvent::PointerExited,
                    baseview::MouseEvent::CursorEntered => return EventStatus::Captured,
                    _ => return EventStatus::Ignored,
                };
                win.dispatch_event(slint_event);
                EventStatus::Captured
            }
            Event::Keyboard(key_event) => {
                let text: SharedString = if let keyboard_types::Key::Character(c) = key_event.key {
                    c.into()
                } else {
                    match translate::key_text_from_code(key_event.code) {
                        Some(t) => t,
                        None => return EventStatus::Ignored,
                    }
                };
                if text.is_empty() {
                    return EventStatus::Ignored;
                }
                match key_event.state {
                    keyboard_types::KeyState::Down => {
                        if key_event.repeat {
                            win.dispatch_event(WindowEvent::KeyPressRepeated { text });
                        } else {
                            win.dispatch_event(WindowEvent::KeyPressed { text });
                        }
                    }
                    keyboard_types::KeyState::Up => {
                        win.dispatch_event(WindowEvent::KeyReleased { text });
                    }
                }
                EventStatus::Ignored
            }
            Event::Window(baseview::WindowEvent::Resized(info)) => {
                h.scale = info.scale() as f32;
                let physical = info.physical_size();
                let logical = info.logical_size();
                h.logical_w = logical.width as u32;
                h.logical_h = logical.height as u32;
                h.adapter
                    .update_size(physical.width, physical.height, h.scale);
                win.dispatch_event(WindowEvent::Resized {
                    size: slint::LogicalSize::new(logical.width as f32, logical.height as f32),
                });
                win.dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: h.scale,
                });
                EventStatus::Captured
            }
            Event::Window(_) => EventStatus::Ignored,
        }
    }
}

impl<P: Params + 'static> Editor for SlintEditor<P> {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn open(&mut self, parent: RawWindowHandle, context: PluginContext) {
        let (lw, lh) = self.size;
        self.pending_size.store(0, Ordering::Relaxed);
        let pending_size = Arc::clone(&self.pending_size);

        let scale_policy = if self.host_scale_set {
            WindowScalePolicy::ScaleFactor(self.scale)
        } else {
            WindowScalePolicy::SystemScaleFactor
        };

        let typed_ctx = context.with_params(self.params.clone());
        let setup = Arc::clone(&self.setup);
        let initial_scale = if self.host_scale_set {
            self.scale as f32
        } else {
            1.0
        };

        let options = WindowOpenOptions {
            title: String::from("lx-slint"),
            size: Size::new(f64::from(lw), f64::from(lh)),
            scale: scale_policy,
            gl_config: Some(default_gl_config()),
        };

        let parent_wrapper = ParentWindow(parent);

        let opened = panic::catch_unwind(AssertUnwindSafe(|| {
            baseview::Window::open_parented(
                &parent_wrapper,
                options,
                move |window: &mut Window| {
                    ensure_platform();
                    unsafe {
                        if let Some(ctx) = window.gl_context() {
                            ctx.make_current();
                        }
                    }

                    let scale = initial_scale.max(0.1);
                    let pw = ((lw as f32) * scale).round().max(1.0) as u32;
                    let ph = ((lh as f32) * scale).round().max(1.0) as u32;
                    let adapter = BaseviewSlintAdapter::new(pw, ph, scale);
                    adapter.set_gl_context(window);

                    let sync_fn = {
                        let _guard = CurrentAdapterGuard::set(adapter.clone());
                        match panic::catch_unwind(AssertUnwindSafe(|| setup(typed_ctx.clone()))) {
                            Ok(f) => f,
                            Err(e) => {
                                log::error!(
                                    "lx-slint-editor setup panicked: {}",
                                    panic_message(&*e)
                                );
                                return HandlerState::Dead;
                            }
                        }
                    };

                    HandlerState::Live(LiveHandler {
                        adapter,
                        sync_fn,
                        state: typed_ctx,
                        pending_size,
                        logical_w: lw,
                        logical_h: lh,
                        scale,
                        last_cursor: RefCell::new(LogicalPosition::new(0.0, 0.0)),
                        ready: false,
                    })
                },
            )
        }));

        match opened {
            Ok(handle) => self.window = Some(handle),
            Err(e) => log::error!("lx-slint-editor open panicked: {}", panic_message(&*e)),
        }
    }

    fn set_scale_factor(&mut self, factor: f64) {
        self.host_scale_set = true;
        self.scale = factor;
    }

    fn can_resize(&self) -> bool {
        self.can_resize
    }

    fn can_maximize(&self) -> bool {
        self.can_maximize
    }

    fn min_size(&self) -> (u32, u32) {
        self.min_size
    }

    fn max_size(&self) -> (u32, u32) {
        self.max_size
    }

    fn aspect_ratio(&self) -> Option<(u32, u32)> {
        self.aspect_ratio
    }

    fn prefers_pow2(&self) -> bool {
        self.prefers_pow2
    }

    fn set_size(&mut self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        self.size = (width, height);
        self.pending_size
            .store(pack_size((width, height)), Ordering::Release);
        true
    }

    fn close(&mut self) {
        if let Some(mut window) = self.window.take() {
            window.close();
        }
    }

    fn idle(&mut self) {}
}

impl<P: Params + ?Sized> Drop for SlintEditor<P> {
    fn drop(&mut self) {
        if let Some(mut window) = self.window.take() {
            window.close();
        }
    }
}
