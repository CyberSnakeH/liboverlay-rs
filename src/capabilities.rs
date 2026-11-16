#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Display protocol selected for this window.
pub enum BackendType {
    /// Native Wayland layer-shell surface.
    Wayland,
    /// Xorg or XWayland window.
    X11,
}

/// Describes the protocol route, not a claim about pixels observed on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// Negotiated with the backend or driver.
    Supported,
    /// Unavailable in this configuration.
    Unsupported,
    /// Not established by capability negotiation.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The mechanism used to request desktop layering.
pub enum StackingSupport {
    /// Wayland layer-shell Overlay layer.
    LayerShellOverlay,
    /// Managed X11 window with an advertised EWMH above-state hint.
    WindowManagerHint,
    /// Managed X11 window without an advertised above-state hint.
    WindowManagerControlled,
    /// Unmanaged X11 window raised above its X11 siblings.
    UnmanagedRaise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The negotiated presentation pacing mechanism.
pub enum FramePacing {
    /// Wayland frame callbacks, which may stop while the surface is hidden.
    CompositorCallback,
    /// X11 Present Complete/Idle notifications.
    XPresent,
    /// Local timer without a vertical synchronization guarantee.
    Timer,
    /// Vulkan FIFO swapchain, also gated by Wayland callbacks where applicable.
    GpuFifo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Renderer actually selected after startup and any fallback.
pub enum RendererType {
    /// Premultiplied CPU frame uploads.
    Cpu,
    /// Direct Vulkan command encoding and presentation.
    Gpu,
}

#[derive(Debug, Clone)]
/// Vulkan adapter and swapchain information recorded at startup.
pub struct GpuAdapterInfo {
    /// Driver-reported adapter name.
    pub name: String,
    /// Driver identifier.
    pub driver: String,
    /// Driver-supplied version or description.
    pub driver_info: String,
    /// Whether the adapter is a CPU/software implementation.
    pub software: bool,
    /// Negotiated wgpu composite alpha mode.
    pub alpha_mode: String,
    /// Negotiated presentation mode, currently FIFO.
    pub present_mode: String,
}

#[derive(Debug, Clone)]
/// Immutable startup capabilities; desktop composition remains compositor policy.
pub struct Capabilities {
    /// Selected renderer.
    pub renderer: RendererType,
    /// Whether the visual/surface supports the requested transparency.
    pub transparency: Support,
    /// Whether empty pointer input regions can be configured.
    pub click_through: Support,
    /// Whether on-demand keyboard interaction can be requested.
    pub keyboard_on_demand: Support,
    /// Whether custom logical hit regions are available.
    pub input_regions: Support,
    /// Whether XKB translation was initialized for the requested keyboard mode.
    pub keyboard_translation: Support,
    /// Whether XCompose tables or a desktop input-method route were found.
    pub text_composition: Support,
    /// Desktop IME protocol discovered at startup; activation is reported by events.
    pub input_method: Support,
    /// Whether the desktop protocol accepts surrounding text context.
    pub surrounding_text: Support,
    /// Whether touch was negotiated; this does not guarantee a physical device.
    pub touch: Support,
    /// Available layering mechanism.
    pub stacking: StackingSupport,
    /// Presentation scheduling mechanism.
    pub frame_pacing: FramePacing,
    /// Whether server presentation timestamps can be requested for this renderer.
    pub presentation_feedback: Support,
    /// Whether integer Wayland output scaling is supported.
    pub integer_scaling: bool,
    /// Whether both fractional-scale and viewporter were negotiated.
    pub fractional_scaling: bool,
}

#[derive(Debug, Clone)]
/// One unsuccessful backend startup attempt.
pub struct BackendAttempt {
    /// Backend that was attempted.
    pub backend: BackendType,
    /// Human-readable failure detail.
    pub reason: String,
}

#[derive(Debug, Clone)]
/// Startup decision and the negotiated limits of one overlay.
pub struct BackendReport {
    /// Selected Vulkan adapter, or `None` for CPU rendering.
    pub gpu: Option<GpuAdapterInfo>,
    /// Selected display protocol.
    pub backend: BackendType,
    /// Negotiated capabilities.
    pub capabilities: Capabilities,
    /// Earlier backend attempts, in execution order.
    pub fallback_attempts: Vec<BackendAttempt>,
    /// Fallback details and compositor-dependent limitations.
    pub warnings: Vec<String>,
}
