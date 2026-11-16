use crate::{OverlayError, Result};

/// Maximum number of rectangles accepted in a custom input region.
pub const MAX_INPUT_RECTANGLES: usize = 1024;

/// Half-open rectangle in surface-local logical coordinates.
/// Coordinates and dimensions fit both Wayland regions and X11 SHAPE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputRectangle {
    /// Left edge; negative coordinates are allowed and clipped by the surface.
    pub x: i16,
    /// Top edge; negative coordinates are allowed and clipped by the surface.
    pub y: i16,
    /// Positive logical width.
    pub width: u16,
    /// Positive logical height.
    pub height: u16,
}

impl InputRectangle {
    /// Defines a rectangle. Zero dimensions are rejected when applied.
    pub const fn new(x: i16, y: i16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(OverlayError::InvalidConfiguration(
                "input rectangles must have positive dimensions".into(),
            ));
        }
        Ok(())
    }
}

/// Pointer and touch hit region, intersected with the current surface extent.
/// Rectangles are a union, keep logical coordinates across scale/size changes,
/// and may overlap. An empty rectangle list is equivalent to `Empty`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum InputRegion {
    /// Entire current surface, including newly exposed areas after a resize.
    #[default]
    Full,
    /// No pointer or touch input.
    Empty,
    /// Union of at most `MAX_INPUT_RECTANGLES` half-open rectangles.
    Rectangles(Vec<InputRectangle>),
}

impl InputRegion {
    pub(crate) fn validate(&self) -> Result<()> {
        if let Self::Rectangles(rectangles) = self {
            if rectangles.len() > MAX_INPUT_RECTANGLES {
                return Err(OverlayError::InvalidConfiguration(
                    "too many input rectangles".into(),
                ));
            }
            for rectangle in rectangles {
                rectangle.validate()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_rectangles_have_checked_sizes_and_bounded_storage() {
        assert!(
            InputRegion::Rectangles(vec![InputRectangle::new(-10, -5, 20, 10)])
                .validate()
                .is_ok()
        );
        assert!(
            InputRegion::Rectangles(vec![InputRectangle::new(0, 0, 0, 10)])
                .validate()
                .is_err()
        );
        assert!(InputRegion::Rectangles(vec![
            InputRectangle::new(0, 0, 1, 1);
            MAX_INPUT_RECTANGLES + 1
        ])
        .validate()
        .is_err());
    }
}
