//! FemtoVG OpenGL adapter + Slint Platform hand-off.
//! Adapted from slint-baseview (ISC).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use baseview::Window;
use once_cell::unsync::OnceCell;
use slint::platform::femtovg_renderer::FemtoVGRenderer;
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

thread_local! {
    static CURRENT_ADAPTER: RefCell<Option<Rc<BaseviewSlintAdapter>>> =
        const { RefCell::new(None) };
}

pub struct CurrentAdapterGuard;

impl CurrentAdapterGuard {
    pub fn set(adapter: Rc<BaseviewSlintAdapter>) -> Self {
        CURRENT_ADAPTER.with(|c| *c.borrow_mut() = Some(adapter));
        Self
    }
}

impl Drop for CurrentAdapterGuard {
    fn drop(&mut self) {
        CURRENT_ADAPTER.with(|c| *c.borrow_mut() = None);
    }
}

struct BaseviewSlintPlatform;

impl Platform for BaseviewSlintPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        CURRENT_ADAPTER.with(|a| {
            a.borrow()
                .clone()
                .map(|x| x as Rc<dyn WindowAdapter>)
                .ok_or_else(|| PlatformError::Other("No adapter set".into()))
        })
    }
}

/// Register platform once per thread (first open wins).
pub fn ensure_platform() {
    let _ = slint::platform::set_platform(Box::new(BaseviewSlintPlatform));
}

#[derive(Clone)]
struct BaseviewOpenGLInterface {
    get_proc_address: Arc<dyn Fn(&str) -> *const core::ffi::c_void + Send + Sync>,
}

impl BaseviewOpenGLInterface {
    fn new(window: &Window) -> Self {
        // SAFETY: GlContext lives as long as the Window; renderer dies first.
        let ctx_addr =
            window.gl_context().expect("OpenGL context required") as *const baseview::gl::GlContext
                as usize;
        Self {
            get_proc_address: Arc::new(move |name: &str| {
                let ctx = ctx_addr as *const baseview::gl::GlContext;
                unsafe { &*ctx }.get_proc_address(name)
            }),
        }
    }
}

unsafe impl slint::platform::femtovg_renderer::OpenGLInterface for BaseviewOpenGLInterface {
    fn ensure_current(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    fn swap_buffers(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    fn resize(
        &self,
        _width: core::num::NonZeroU32,
        _height: core::num::NonZeroU32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    fn get_proc_address(&self, name: &core::ffi::CStr) -> *const core::ffi::c_void {
        (self.get_proc_address)(name.to_str().unwrap_or(""))
    }
}

pub struct BaseviewSlintAdapter {
    window: slint::Window,
    renderer: OnceCell<FemtoVGRenderer>,
    physical_size: RefCell<PhysicalSize>,
    scale_factor: RefCell<f32>,
    gl_interface: OnceCell<BaseviewOpenGLInterface>,
}

impl BaseviewSlintAdapter {
    pub fn new(physical_width: u32, physical_height: u32, scale_factor: f32) -> Rc<Self> {
        Rc::new_cyclic(|weak| Self {
            window: slint::Window::new(weak.clone() as _),
            renderer: OnceCell::new(),
            physical_size: RefCell::new(PhysicalSize::new(physical_width, physical_height)),
            scale_factor: RefCell::new(scale_factor),
            gl_interface: OnceCell::new(),
        })
    }

    pub fn set_gl_context(&self, window: &Window) {
        let _ = self.gl_interface.set(BaseviewOpenGLInterface::new(window));
    }

    pub fn update_size(&self, physical_width: u32, physical_height: u32, scale_factor: f32) {
        *self.physical_size.borrow_mut() = PhysicalSize::new(physical_width, physical_height);
        *self.scale_factor.borrow_mut() = scale_factor;
    }

    pub fn slint_window(&self) -> &slint::Window {
        &self.window
    }

    pub fn ensure_renderer(&self) -> &FemtoVGRenderer {
        self.renderer.get_or_init(|| {
            let iface = self
                .gl_interface
                .get()
                .expect("GL context must be set before renderer init");
            FemtoVGRenderer::new(iface.clone()).expect("Failed to create FemtoVG renderer")
        })
    }
}

impl WindowAdapter for BaseviewSlintAdapter {
    fn window(&self) -> &slint::Window {
        &self.window
    }

    fn size(&self) -> PhysicalSize {
        *self.physical_size.borrow()
    }

    fn renderer(&self) -> &dyn slint::platform::Renderer {
        self.ensure_renderer()
    }

    fn request_redraw(&self) {}
}

pub fn default_gl_config() -> baseview::gl::GlConfig {
    baseview::gl::GlConfig {
        version: (3, 2),
        red_bits: 8,
        blue_bits: 8,
        green_bits: 8,
        alpha_bits: 8,
        depth_bits: 24,
        stencil_bits: 8,
        samples: None,
        srgb: true,
        double_buffer: true,
        vsync: false,
        ..Default::default()
    }
}
