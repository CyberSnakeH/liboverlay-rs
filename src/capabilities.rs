#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendType {
    Wayland,
    X11,
}

/// Describes the protocol route, not a claim about pixels observed on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackingSupport {
    LayerShellOverlay,
    WindowManagerHint,
    WindowManagerControlled,
    UnmanagedRaise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePacing {
    CompositorCallback,
    XPresent,
    Timer,
    GpuFifo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererType {
    Cpu,
    Gpu,
}

#[derive(Debug, Clone)]
pub struct GpuAdapterInfo {
    pub name: String,
    pub driver: String,
    pub driver_info: String,
    pub software: bool,
    pub alpha_mode: String,
    pub present_mode: String,
}

#[derive(Debug, Clone)]
pub struct Capabilities {
    pub renderer: RendererType,
    pub transparency: Support,
    pub click_through: Support,
    pub keyboard_on_demand: Support,
    pub stacking: StackingSupport,
    pub frame_pacing: FramePacing,
    pub integer_scaling: bool,
    pub fractional_scaling: bool,
}

#[derive(Debug, Clone)]
pub struct BackendAttempt {
    pub backend: BackendType,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct BackendReport {
    pub gpu: Option<GpuAdapterInfo>,
    pub backend: BackendType,
    pub capabilities: Capabilities,
    pub fallback_attempts: Vec<BackendAttempt>,
    pub warnings: Vec<String>,
}
