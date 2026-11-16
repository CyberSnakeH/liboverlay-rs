//! Optional Vulkan swapchain rendering, without CPU image readback.
//!
//! Callbacks run on the display runtime. Prepare pipelines and other expensive
//! resources ahead of time with `OverlayWindow::gpu_context()`. Keep callbacks
//! short so input commands remain responsive. The surface texture and native
//! display handles never leave the runtime.
use crate::{Result, WindowInfo};
pub use wgpu;
#[derive(Debug, Clone)]
/// Cloneable device resources; these can safely outlive their overlay window.
pub struct GpuContext {
    /// Device used by this overlay. Destroying it terminates the overlay runtime.
    pub device: wgpu::Device,
    /// Queue to which encoded drawing commands are submitted.
    pub queue: wgpu::Queue,
    /// Swapchain texture format to use when building render pipelines.
    pub format: wgpu::TextureFormat,
    /// Negotiated surface alpha mode; colors must be premultiplied when required.
    pub alpha_mode: wgpu::CompositeAlphaMode,
}

/// One acquired swapchain image, borrowed only for a display-thread callback.
pub struct GpuFrame<'a> {
    /// Device, queue and target format.
    pub context: &'a GpuContext,
    /// Current render target, initially cleared to transparent.
    pub view: &'a wgpu::TextureView,
    /// Encoder submitted by the runtime after the callback succeeds.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// Geometry of this acquired image.
    pub info: WindowInfo,
}
pub(crate) type GpuDraw = Box<dyn for<'a> FnOnce(GpuFrame<'a>) -> Result<()> + Send + 'static>;

#[cfg(feature = "x11")]
pub(crate) use surface::xcb;
#[cfg(any(feature = "wayland", feature = "x11"))]
pub(crate) use surface::GpuSurface;

#[cfg(any(feature = "wayland", feature = "x11"))]
mod surface {
    use super::{wgpu, GpuContext, GpuDraw, GpuFrame};
    use crate::{
        GpuAdapterInfo, OverlayBuilder, OverlayError, Requirement, Result, Size, WindowInfo,
    };
    use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
    use std::{
        any::Any,
        future::Future,
        sync::{Arc, Mutex, OnceLock},
        task::{Context, Poll, Wake, Waker},
        time::Instant,
    };

    pub(crate) struct GpuSurface {
        // Drop order is intentional: destroy the Vulkan surface before the native
        // connection keepalive. Parent backends drop this object before their window.
        surface: wgpu::Surface<'static>,
        context: GpuContext,
        config: wgpu::SurfaceConfiguration,
        info: GpuAdapterInfo,
        errors: Arc<Mutex<Option<String>>>,
        _instance: wgpu::Instance,
        _native: Box<dyn Any>,
    }

    fn error(error: impl std::fmt::Display) -> OverlayError {
        OverlayError::Gpu(error.to_string())
    }

    fn instance() -> wgpu::Instance {
        // Vulkan loader/adapter discovery belongs to the process, not a window.
        // In particular, repeated instance discovery on NVIDIA 580 leaves driver
        // descriptors open even without a surface/device (see stability evidence).
        // Reuse one thread-safe instance; devices/surfaces remain per window.
        static INSTANCE: OnceLock<wgpu::Instance> = OnceLock::new();
        INSTANCE
            .get_or_init(|| {
                wgpu::Instance::new(&wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::VULKAN,
                    ..Default::default()
                })
            })
            .clone()
    }

    impl GpuSurface {
        /// Native objects must remain valid until this object is dropped. `native`
        /// retains their connection; the parent backend must drop us before issuing
        /// a window/surface destruction request. No surface texture escapes this API.
        pub(crate) unsafe fn create(
            display: RawDisplayHandle,
            window: RawWindowHandle,
            native: Box<dyn Any>,
            config: &OverlayBuilder,
            size: Size,
            native_alpha: bool,
            deadline: Instant,
        ) -> Result<Self> {
            let instance = instance();
            // SAFETY: the caller owns the window, retains the display in `native`,
            // and explicitly drops GpuSurface before destroying either native object.
            let surface = unsafe {
                instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: display,
                    raw_window_handle: window,
                })
            }
            .map_err(error)?;
            let adapter = block_on_until(
                instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: Some(&surface),
                }),
                deadline,
            )?
            .map_err(error)?;
            let adapter_info = adapter.get_info();
            let software = adapter_info.device_type == wgpu::DeviceType::Cpu;
            if software && !config.allow_software_gpu {
                return Err(OverlayError::Unsupported(
                "only a software Vulkan adapter is available; software GPU fallback is disabled",
            ));
            }
            let caps = surface.get_capabilities(&adapter);
            let format = [
                wgpu::TextureFormat::Bgra8Unorm,
                wgpu::TextureFormat::Rgba8Unorm,
            ]
            .into_iter()
            .find(|format| caps.formats.contains(format))
            .ok_or(OverlayError::Unsupported("non-sRGB 8-bit swapchain format"))?;
            let alpha_mode = choose_alpha(&caps.alpha_modes, native_alpha, config.transparency)?;
            if !caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
                return Err(OverlayError::Unsupported("FIFO GPU presentation"));
            }
            let (device, queue) = block_on_until(
                adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("liboverlay GPU"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                }),
                deadline,
            )?
            .map_err(error)?;
            let errors = Arc::new(Mutex::new(None));
            let uncaptured = errors.clone();
            device.on_uncaptured_error(Box::new(move |err| {
                if let Ok(mut slot) = uncaptured.lock() {
                    if slot.is_none() {
                        *slot = Some(err.to_string());
                    }
                }
            }));
            let lost = errors.clone();
            device.set_device_lost_callback(move |reason, message| {
                if let Ok(mut slot) = lost.lock() {
                    if slot.is_none() {
                        *slot = Some(format!("device lost ({reason:?}): {message}"));
                    }
                }
            });
            let configuration = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: size.width,
                height: size.height,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode,
                view_formats: vec![],
            };
            let context = GpuContext {
                device,
                queue,
                format,
                alpha_mode,
            };
            let info = GpuAdapterInfo {
                name: adapter_info.name,
                driver: adapter_info.driver,
                driver_info: adapter_info.driver_info,
                software,
                alpha_mode: format!("{alpha_mode:?}"),
                present_mode: "Fifo".into(),
            };
            let mut result = Self {
                surface,
                context,
                config: configuration,
                info,
                errors,
                _instance: instance,
                _native: native,
            };
            result.resize(size)?;
            Ok(result)
        }
        pub(crate) fn info(&self) -> GpuAdapterInfo {
            self.info.clone()
        }
        pub(crate) fn context(&self) -> GpuContext {
            self.context.clone()
        }
        pub(crate) fn check(&self) -> Result<()> {
            self.context
                .device
                .poll(wgpu::PollType::Poll)
                .map_err(error)?;
            if let Some(message) = self
                .errors
                .lock()
                .map_err(|_| OverlayError::Poisoned)?
                .take()
            {
                return Err(OverlayError::Gpu(message));
            }
            Ok(())
        }
        fn resize(&mut self, size: Size) -> Result<()> {
            let limit = self.context.device.limits().max_texture_dimension_2d;
            if size.width == 0 || size.height == 0 || size.width > limit || size.height > limit {
                return Err(OverlayError::Unsupported("GPU texture dimension limit"));
            }
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.context.device, &self.config);
            self.check()
        }
        pub(crate) fn acquire(&mut self, size: Size) -> Result<Option<wgpu::SurfaceTexture>> {
            self.check()?;
            if self.config.width != size.width || self.config.height != size.height {
                self.resize(size)?;
            }
            match self.surface.get_current_texture() {
                Ok(texture) => Ok(Some(texture)),
                Err(wgpu::SurfaceError::Timeout) => Ok(None),
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    self.resize(size)?;
                    // Retry on the next runtime iteration without retaining a
                    // texture from an obsolete swapchain or consuming the callback.
                    Ok(None)
                }
                Err(err) => Err(error(err)),
            }
        }
        pub(crate) fn draw(
            &self,
            texture: wgpu::SurfaceTexture,
            draw: &mut Option<GpuDraw>,
            info: WindowInfo,
        ) -> Result<()> {
            // Keep the acquired surface texture outside the unwind boundary.
            // wgpu deliberately skips discard while panicking; unwinding past
            // it can leave swapchain semaphores live during surface destruction.
            let encoded =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                    let view = texture
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    let mut encoder = self.context.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor {
                            label: Some("liboverlay GPU frame"),
                        },
                    );
                    // Start every acquired image transparent; never expose undefined pixels
                    // from an old swapchain image when a caller paints only part of the HUD.
                    {
                        let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("transparent clear"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });
                    }
                    let draw = draw.take().ok_or_else(|| {
                        OverlayError::Gpu("render callback was already consumed".into())
                    })?;
                    draw(GpuFrame {
                        context: &self.context,
                        view: &view,
                        encoder: &mut encoder,
                        info,
                    })?;
                    self.check()?;
                    self.context.queue.submit([encoder.finish()]);
                    self.check()
                }))
                .unwrap_or(Err(OverlayError::RuntimePanic));
            // wgpu 25 validates the device before releasing an acquired image
            // in both present and discard. After device loss that validation can
            // unwind while the surface still retains its native image. Retain a
            // texture handle so its resource can be released before teardown.
            let cleanup = texture.texture.clone();
            if let Err(error) = encoded {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(texture))).is_err()
                {
                    cleanup.destroy();
                }
                return Err(error);
            }
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| texture.present())).is_err()
            {
                cleanup.destroy();
                return self.check().and(Err(OverlayError::RuntimePanic));
            }
            self.check()
        }
    }

    fn choose_alpha(
        modes: &[wgpu::CompositeAlphaMode],
        native_alpha: bool,
        requirement: Requirement,
    ) -> Result<wgpu::CompositeAlphaMode> {
        if native_alpha {
            for mode in [
                wgpu::CompositeAlphaMode::PreMultiplied,
                wgpu::CompositeAlphaMode::Inherit,
            ] {
                if modes.contains(&mode) {
                    return Ok(mode);
                }
            }
        }
        if requirement == Requirement::Preferred
            && modes.contains(&wgpu::CompositeAlphaMode::Opaque)
        {
            return Ok(wgpu::CompositeAlphaMode::Opaque);
        }
        Err(OverlayError::Gpu(format!("premultiplied/native-alpha surface unavailable; modes={modes:?}, native_alpha={native_alpha}")))
    }

    fn block_on_until<F: Future>(future: F, deadline: Instant) -> Result<F::Output> {
        struct ThreadWake(std::thread::Thread);
        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(OverlayError::Timeout("initializing the GPU"))?;
            match future.as_mut().poll(&mut context) {
                Poll::Ready(result) => return Ok(result),
                Poll::Pending => {
                    std::thread::park_timeout(remaining.min(std::time::Duration::from_millis(50)))
                }
            }
        }
    }

    #[cfg(feature = "x11")]
    pub(crate) mod xcb {
        use super::*;
        use std::{
            ffi::{c_char, c_int, c_void, CString},
            ptr::NonNull,
        };
        type Connect = unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void;
        type HasError = unsafe extern "C" fn(*mut c_void) -> c_int;
        type Disconnect = unsafe extern "C" fn(*mut c_void);
        pub(crate) struct NativeXcb {
            pub pointer: NonNull<c_void>,
            pub screen: c_int,
            disconnect: Disconnect,
            _library: libloading::Library,
        }
        impl NativeXcb {
            pub(crate) fn connect(display: &str) -> Result<Self> {
                let display = CString::new(display).map_err(error)?;
                // SAFETY: load the standard system libxcb ABI; its handle is kept
                // alive until after its connection has been disconnected.
                let library = unsafe { libloading::Library::new("libxcb.so.1") }.map_err(error)?;
                // SAFETY: these signatures are the documented libxcb public C ABI.
                let (connect, has_error, disconnect) = unsafe {
                    (
                        *library.get::<Connect>(b"xcb_connect\0").map_err(error)?,
                        *library
                            .get::<HasError>(b"xcb_connection_has_error\0")
                            .map_err(error)?,
                        *library
                            .get::<Disconnect>(b"xcb_disconnect\0")
                            .map_err(error)?,
                    )
                };
                let mut screen = 0;
                // SAFETY: CString and the writable screen pointer live for this call.
                let pointer = NonNull::new(unsafe { connect(display.as_ptr(), &mut screen) })
                    .ok_or_else(|| OverlayError::Gpu("xcb_connect returned null".into()))?;
                // SAFETY: pointer was returned by the matching library's connect.
                if unsafe { has_error(pointer.as_ptr()) } != 0 {
                    // SAFETY: this failed connection still requires one disconnect.
                    unsafe { disconnect(pointer.as_ptr()) };
                    return Err(OverlayError::Gpu("native XCB GPU connection failed".into()));
                }
                Ok(Self {
                    pointer,
                    screen,
                    disconnect,
                    _library: library,
                })
            }
        }
        impl Drop for NativeXcb {
            fn drop(&mut self) {
                // SAFETY: unique owned connection, library retained, surface already
                // dropped by GpuSurface's field order before this keepalive is dropped.
                unsafe { (self.disconnect)(self.pointer.as_ptr()) };
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn alpha_negotiation_never_silently_selects_opaque() {
            use wgpu::CompositeAlphaMode as A;
            assert!(choose_alpha(&[A::Opaque], true, Requirement::Required).is_err());
            assert_eq!(
                choose_alpha(&[A::Opaque, A::Inherit], true, Requirement::Required).unwrap(),
                A::Inherit
            );
            assert_eq!(
                choose_alpha(&[A::PreMultiplied, A::Inherit], true, Requirement::Required).unwrap(),
                A::PreMultiplied
            );
            assert!(choose_alpha(&[A::Inherit], false, Requirement::Required).is_err());
            assert_eq!(
                choose_alpha(&[A::Opaque], false, Requirement::Preferred).unwrap(),
                A::Opaque
            );
        }
    }
}
