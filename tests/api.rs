use liboverlay::{OverlayBuilder, OverlayError};

#[cfg(not(feature = "gpu"))]
#[test]
fn required_gpu_needs_the_explicit_feature() {
    assert!(matches!(
        OverlayBuilder::new()
            .with_renderer(liboverlay::RendererPreference::GpuRequired)
            .build(),
        Err(OverlayError::Unsupported(_))
    ));
}

#[test]
fn invalid_configurations_fail_before_opening_a_display() {
    assert!(matches!(
        OverlayBuilder::new().with_size(0, 2).build(),
        Err(OverlayError::InvalidSize(_))
    ));
    assert!(matches!(
        OverlayBuilder::new()
            .with_size(800, 600)
            .with_memory_limit(1024)
            .build(),
        Err(OverlayError::MemoryLimit { .. })
    ));
    assert!(matches!(
        OverlayBuilder::new().with_title("bad\0title").build(),
        Err(OverlayError::InvalidConfiguration(_))
    ));
    assert!(matches!(
        OverlayBuilder::new()
            .with_command_timeout(std::time::Duration::ZERO)
            .build(),
        Err(OverlayError::InvalidConfiguration(_))
    ));
}
