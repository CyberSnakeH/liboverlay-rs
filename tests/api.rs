//! Public configuration contracts that do not require a display connection.
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

#[test]
fn invalid_input_contracts_fail_before_opening_a_display() {
    use liboverlay::{InputRectangle, InputRegion, KeyboardMode, SurroundingText, TextInputState};
    assert!(matches!(
        OverlayBuilder::new()
            .with_input_region(InputRegion::Rectangles(vec![InputRectangle::new(
                0, 0, 0, 1
            )]))
            .build(),
        Err(OverlayError::InvalidConfiguration(_))
    ));
    assert!(matches!(
        OverlayBuilder::new()
            .with_text_input(TextInputState::enabled())
            .with_keyboard_mode(KeyboardMode::None)
            .build(),
        Err(OverlayError::InvalidConfiguration(_))
    ));
    let state = TextInputState {
        surrounding: Some(SurroundingText {
            text: "é".into(),
            cursor: 1,
            anchor: 2,
        }),
        ..TextInputState::enabled()
    };
    assert!(matches!(
        OverlayBuilder::new().with_text_input(state).build(),
        Err(OverlayError::InvalidConfiguration(_))
    ));
}
