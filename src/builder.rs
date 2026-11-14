use crate::{OverlayError, OverlayWindow, Result, Size};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendPreference {
    Auto,
    WaylandOnly,
    X11Only,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    Required,
    Preferred,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererPreference {
    Cpu,
    GpuPreferred,
    GpuRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackingPolicy {
    PreferOverlay,
    RequireOverlayLayer,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardMode {
    None,
    OnDemand,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSelector {
    Automatic,
    Name(String),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
    Fill,
}

#[derive(Debug, Clone)]
pub struct OverlayBuilder {
    pub(crate) renderer: RendererPreference,
    #[cfg(feature = "gpu")]
    pub(crate) allow_software_gpu: bool,
    pub(crate) title: String,
    pub(crate) size: Size,
    pub(crate) backend: BackendPreference,
    pub(crate) transparency: Requirement,
    pub(crate) stacking: StackingPolicy,
    pub(crate) keyboard: KeyboardMode,
    pub(crate) click_through: bool,
    pub(crate) output: OutputSelector,
    pub(crate) anchor: Anchor,
    pub(crate) margin: u32,
    pub(crate) max_frame_bytes: usize,
    pub(crate) startup_timeout: Duration,
    pub(crate) command_timeout: Duration,
    pub(crate) frame_interval: Duration,
}

impl Default for OverlayBuilder {
    fn default() -> Self {
        Self {
            renderer: RendererPreference::Cpu,
            #[cfg(feature = "gpu")]
            allow_software_gpu: false,
            title: "liboverlay".into(),
            size: Size::new(800, 600),
            backend: BackendPreference::Auto,
            transparency: Requirement::Required,
            stacking: StackingPolicy::PreferOverlay,
            keyboard: KeyboardMode::None,
            click_through: true,
            output: OutputSelector::Automatic,
            anchor: Anchor::Center,
            margin: 0,
            max_frame_bytes: 64 * 1024 * 1024,
            startup_timeout: Duration::from_secs(5),
            command_timeout: Duration::from_secs(2),
            frame_interval: Duration::from_secs_f64(1.0 / 60.0),
        }
    }
}
impl OverlayBuilder {
    pub fn with_renderer(mut self, renderer: RendererPreference) -> Self {
        self.renderer = renderer;
        self
    }
    /// Software adapters are rejected by default; enable only for diagnostics
    /// or environments where software Vulkan is explicitly desired.
    #[cfg(feature = "gpu")]
    pub fn with_software_gpu(mut self, allow: bool) -> Self {
        self.allow_software_gpu = allow;
        self
    }
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.size = Size::new(width, height);
        self
    }
    pub fn with_backend(mut self, backend: BackendPreference) -> Self {
        self.backend = backend;
        self
    }
    pub fn with_transparency(mut self, value: Requirement) -> Self {
        self.transparency = value;
        self
    }
    pub fn with_stacking(mut self, value: StackingPolicy) -> Self {
        self.stacking = value;
        self
    }
    pub fn with_keyboard_mode(mut self, value: KeyboardMode) -> Self {
        self.keyboard = value;
        self
    }
    pub fn with_click_through(mut self, value: bool) -> Self {
        self.click_through = value;
        self
    }
    pub fn with_output(mut self, value: OutputSelector) -> Self {
        self.output = value;
        self
    }
    pub fn with_anchor(mut self, value: Anchor) -> Self {
        self.anchor = value;
        self
    }
    pub fn with_margin(mut self, value: u32) -> Self {
        self.margin = value;
        self
    }
    pub fn with_memory_limit(mut self, bytes_per_frame: usize) -> Self {
        self.max_frame_bytes = bytes_per_frame;
        self
    }
    pub fn with_startup_timeout(mut self, value: Duration) -> Self {
        self.startup_timeout = value;
        self
    }
    pub fn with_command_timeout(mut self, value: Duration) -> Self {
        self.command_timeout = value;
        self
    }
    /// Sets the X11 timer interval used when Present is unavailable.
    /// Wayland and GPU renderers follow their presentation protocols instead.
    pub fn with_frame_interval(mut self, value: Duration) -> Self {
        self.frame_interval = value;
        self
    }
    pub fn build(self) -> Result<OverlayWindow> {
        self.validate()?;
        crate::runtime::spawn(self)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.renderer == RendererPreference::GpuRequired && !cfg!(feature = "gpu") {
            return Err(OverlayError::Unsupported(
                "GPU support: enable the gpu Cargo feature",
            ));
        }
        self.size.byte_len(self.max_frame_bytes)?;
        if self.title.contains('\0') || self.title.len() > 4096 || self.margin > 32767 {
            return Err(OverlayError::InvalidConfiguration(
                "invalid title or margin".into(),
            ));
        }
        for timeout in [
            self.startup_timeout,
            self.command_timeout,
            self.frame_interval,
        ] {
            if timeout.is_zero() || timeout > Duration::from_secs(60) {
                return Err(OverlayError::InvalidConfiguration(
                    "timeouts/interval must be in (0, 60s]".into(),
                ));
            }
        }
        if let OutputSelector::Name(name) = &self.output {
            if name.is_empty() || name.contains('\0') {
                return Err(OverlayError::InvalidConfiguration(
                    "invalid output name".into(),
                ));
            }
        }
        Ok(())
    }
}
