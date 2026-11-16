use crate::{OverlayError, OverlayWindow, Result, Size};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Display connection selection at startup.
pub enum BackendPreference {
    /// Try configured Wayland first, then configured X11.
    Auto,
    /// Require Wayland layer-shell; never fall back to X11.
    WaylandOnly,
    /// Require a local X11 or XWayland server.
    X11Only,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Whether startup may proceed without a requested capability.
pub enum Requirement {
    /// Reject a backend or renderer that cannot meet the requirement.
    Required,
    /// Allow a documented fallback and report it in the backend warnings.
    Preferred,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Pixel rendering and presentation selection.
pub enum RendererPreference {
    /// Render premultiplied ARGB pixels through shared memory or X11 uploads.
    Cpu,
    /// Try Vulkan, then use CPU rendering if initialization fails.
    GpuPreferred,
    /// Require Vulkan and the configured transparency policy.
    GpuRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Required strength of desktop layering.
pub enum StackingPolicy {
    /// Use layer-shell where available, or the best available X11 policy.
    PreferOverlay,
    /// Require the Wayland layer-shell Overlay layer.
    RequireOverlayLayer,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Keyboard interaction policy; pointer interaction is configured separately.
pub enum KeyboardMode {
    /// Do not request keyboard focus.
    None,
    /// Allow focus after user interaction, without forcing initial focus.
    OnDemand,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Output binding. A removed explicitly named output closes its overlay.
pub enum OutputSelector {
    /// Let Wayland choose, or follow the primary/first X11 monitor.
    Automatic,
    /// Bind to an exact wl_output v4 or RandR 1.5 monitor name.
    Name(String),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Placement within the selected output, in logical coordinates.
pub enum Anchor {
    /// Align the top and left edges.
    TopLeft,
    /// Center along the top edge.
    Top,
    /// Align the top and right edges.
    TopRight,
    /// Center along the left edge.
    Left,
    /// Center in both directions.
    Center,
    /// Center along the right edge.
    Right,
    /// Align the bottom and left edges.
    BottomLeft,
    /// Center along the bottom edge.
    Bottom,
    /// Align the bottom and right edges.
    BottomRight,
    /// Fill the selected output, inset by the configured margin.
    Fill,
}

#[derive(Debug, Clone)]
/// Overlay configuration, validated before a display connection is opened.
///
/// Defaults: 800×600 logical pixels, automatic backend, CPU rendering, required
/// transparency, passive pointer input, no keyboard request and centered placement.
pub struct OverlayBuilder {
    pub(crate) renderer: RendererPreference,
    pub(crate) presentation_feedback: bool,
    pub(crate) cpu_damage_tracking: bool,
    #[cfg(feature = "gpu")]
    pub(crate) allow_software_gpu: bool,
    pub(crate) title: String,
    pub(crate) size: Size,
    pub(crate) backend: BackendPreference,
    pub(crate) transparency: Requirement,
    pub(crate) stacking: StackingPolicy,
    pub(crate) keyboard: KeyboardMode,
    pub(crate) text_input: crate::TextInputState,
    pub(crate) click_through: bool,
    pub(crate) input_region: crate::InputRegion,
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
            presentation_feedback: false,
            cpu_damage_tracking: false,
            #[cfg(feature = "gpu")]
            allow_software_gpu: false,
            title: "liboverlay".into(),
            size: Size::new(800, 600),
            backend: BackendPreference::Auto,
            transparency: Requirement::Required,
            stacking: StackingPolicy::PreferOverlay,
            keyboard: KeyboardMode::None,
            text_input: crate::TextInputState::default(),
            click_through: true,
            input_region: crate::InputRegion::Full,
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
    /// Selects CPU, preferred Vulkan or required Vulkan presentation.
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
    /// Returns the default configuration.
    pub fn new() -> Self {
        Self::default()
    }
    /// Sets the window title and Wayland layer-shell namespace (at most 4096 bytes).
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }
    /// Sets a nonzero logical size. `Anchor::Fill` instead follows output geometry.
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.size = Size::new(width, height);
        self
    }
    /// Restricts startup backend selection.
    pub fn with_backend(mut self, backend: BackendPreference) -> Self {
        self.backend = backend;
        self
    }
    /// Sets whether an opaque fallback is permitted. Transparency is required by default.
    pub fn with_transparency(mut self, value: Requirement) -> Self {
        self.transparency = value;
        self
    }
    /// Sets the required desktop layering policy.
    pub fn with_stacking(mut self, value: StackingPolicy) -> Self {
        self.stacking = value;
        self
    }
    /// Compares complete CPU frames to upload and damage only their changed bounds.
    /// Disabled by default. Retains one additional CPU image and scans each frame;
    /// useful for sparse changes, but may cost more for full-surface animation.
    /// Frames still start transparent and must contain the complete desired image.
    /// GPU rendering is unaffected.
    pub fn with_cpu_damage_tracking(mut self, enabled: bool) -> Self {
        self.cpu_damage_tracking = enabled;
        self
    }
    /// Requests server presentation timestamps in addition to submission events.
    /// Disabled by default. Check the negotiated `presentation_feedback` capability;
    /// timestamps do not establish optical or physical input latency.
    pub fn with_presentation_feedback(mut self, enabled: bool) -> Self {
        self.presentation_feedback = enabled;
        self
    }
    /// Enables optional keyboard interaction and keymap-translated events.
    pub fn with_keyboard_mode(mut self, value: KeyboardMode) -> Self {
        self.keyboard = value;
        self
    }
    /// Configures a text field. An enabled field also selects on-demand keyboard
    /// interaction. The desktop IME is used when available, otherwise XCompose
    /// provides local composition when its locale tables are installed.
    pub fn with_text_input(mut self, state: crate::TextInputState) -> Self {
        if state.enabled {
            self.keyboard = KeyboardMode::OnDemand;
        }
        self.text_input = state;
        self
    }
    /// Sets whether pointer events pass to underlying windows (enabled by default).
    pub fn with_click_through(mut self, value: bool) -> Self {
        self.click_through = value;
        self
    }
    /// Applies a custom pointer/touch region and disables click-through.
    /// Later enabling click-through temporarily masks this region; disabling it
    /// restores the configured region. Coordinates remain logical after resizing.
    pub fn with_input_region(mut self, region: crate::InputRegion) -> Self {
        self.input_region = region;
        self.click_through = false;
        self
    }
    /// Selects an output automatically or binds to its exact name.
    pub fn with_output(mut self, value: OutputSelector) -> Self {
        self.output = value;
        self
    }
    /// Sets placement relative to the selected output.
    pub fn with_anchor(mut self, value: Anchor) -> Self {
        self.anchor = value;
        self
    }
    /// Sets the logical inset from anchored output edges, in 0..=32767 pixels.
    pub fn with_margin(mut self, value: u32) -> Self {
        self.margin = value;
        self
    }
    /// Limits each physical CPU frame/backing buffer (64 MiB by default).
    /// This is a per-buffer bound, not a bound on total process or GPU memory.
    pub fn with_memory_limit(mut self, bytes_per_frame: usize) -> Self {
        self.max_frame_bytes = bytes_per_frame;
        self
    }
    /// Sets the startup deadline, in (0, 60 seconds]; the default is five seconds.
    pub fn with_startup_timeout(mut self, value: Duration) -> Self {
        self.startup_timeout = value;
        self
    }
    /// Sets synchronous command and cleanup waits, in (0, 60 seconds].
    /// A timeout does not cancel a command already queued. Defaults to two seconds.
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
    /// Validates the configuration and starts a private display runtime.
    /// Capability negotiation failures retain the individual backend attempts.
    pub fn build(self) -> Result<OverlayWindow> {
        self.validate()?;
        crate::runtime::spawn(self)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        self.input_region.validate()?;
        self.text_input.validate()?;
        if self.text_input.enabled && self.keyboard != KeyboardMode::OnDemand {
            return Err(OverlayError::InvalidConfiguration(
                "text input requires on-demand keyboard interaction".into(),
            ));
        }
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
