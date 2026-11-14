use crate::{OverlayError, Result};
use std::ops::{Deref, DerefMut};

/// An exact positive scale, using the Wayland protocol's denominator of 120.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScaleFactor(u32);

impl ScaleFactor {
    pub const ONE: Self = Self(120);
    pub fn from_120ths(numerator: u32) -> Result<Self> {
        if numerator == 0 {
            return Err(OverlayError::InvalidConfiguration(
                "scale must be positive".into(),
            ));
        }
        Ok(Self(numerator))
    }
    pub fn from_integer(scale: u32) -> Result<Self> {
        Self::from_120ths(
            scale.checked_mul(120).ok_or_else(|| {
                OverlayError::InvalidConfiguration("integer scale overflow".into())
            })?,
        )
    }
    pub const fn numerator(self) -> u32 {
        self.0
    }
    pub fn as_f64(self) -> f64 {
        f64::from(self.0) / 120.0
    }
    pub fn as_integer(self) -> Option<u32> {
        if self.0 % 120 == 0 {
            Some(self.0 / 120)
        } else {
            None
        }
    }
    /// Round positive dimensions halfway away from zero, as required by the
    /// fractional-scale protocol. Each buffer dimension remains at least one.
    pub fn physical_size(self, logical: Size) -> Result<Size> {
        if logical.width == 0 || logical.height == 0 {
            return Err(OverlayError::InvalidSize(logical));
        }
        let dimension = |value: u32| -> Result<u32> {
            let rounded = ((u64::from(value) * u64::from(self.0) + 60) / 120).max(1);
            u32::try_from(rounded).map_err(|_| OverlayError::InvalidSize(logical))
        };
        let physical = Size::new(dimension(logical.width)?, dimension(logical.height)?);
        physical.byte_len(usize::MAX)?;
        Ok(physical)
    }
}
impl std::fmt::Debug for ScaleFactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x ({}/120)", self.as_f64(), self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub(crate) fn byte_len(self, limit: usize) -> Result<usize> {
        // Both protocols have signed coordinates and SHM lengths. Restrict the
        // common API rather than truncating values on one backend.
        if self.width == 0 || self.height == 0 || self.width > 32767 || self.height > 32767 {
            return Err(OverlayError::InvalidSize(self));
        }
        let len = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(OverlayError::InvalidSize(self))?;
        if len > limit || len > i32::MAX as usize {
            return Err(OverlayError::MemoryLimit { limit });
        }
        Ok(len)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Argb8888Premultiplied,
}

/// Converts straight RGBA channels to a premultiplied numeric `0xAARRGGBB` pixel.
pub const fn premultiply(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let alpha = a as u32;
    (alpha << 24)
        | (((r as u32 * alpha + 127) / 255) << 16)
        | (((g as u32 * alpha + 127) / 255) << 8)
        | ((b as u32 * alpha + 127) / 255)
}

/// An owned CPU frame. Dropping it without presentation cancels that frame.
/// Raw pixels must satisfy `R <= A`, `G <= A`, and `B <= A`.
#[derive(Debug)]
pub struct Frame {
    pub(crate) pixels: Vec<u32>,
    pub(crate) size: Size,
    pub(crate) generation: u64,
    pub(crate) owner: u64,
}

impl Frame {
    pub(crate) fn new(size: Size, generation: u64, owner: u64, limit: usize) -> Result<Self> {
        let bytes = size.byte_len(limit)?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(bytes / 4)
            .map_err(|_| OverlayError::Allocation { bytes })?;
        pixels.resize(bytes / 4, 0);
        Ok(Self {
            pixels,
            size,
            generation,
            owner,
        })
    }
    pub fn size(&self) -> Size {
        self.size
    }
    pub fn format(&self) -> PixelFormat {
        PixelFormat::Argb8888Premultiplied
    }
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.pixels
    }
}
impl Deref for Frame {
    type Target = [u32];
    fn deref(&self) -> &Self::Target {
        &self.pixels
    }
}
impl DerefMut for Frame {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pixel_alpha_and_rounding() {
        assert_eq!(premultiply(255, 200, 99, 0), 0);
        assert_eq!(premultiply(255, 128, 0, 128), 0x80804000);
        assert_eq!(premultiply(12, 34, 56, 255), 0xff0c2238);
    }
    #[test]
    fn allocation_limits_and_protocol_bounds() {
        assert!(matches!(
            Size::new(0, 10).byte_len(1000),
            Err(OverlayError::InvalidSize(_))
        ));
        assert!(Size::new(u32::MAX, u32::MAX).byte_len(usize::MAX).is_err());
        assert!(matches!(
            Size::new(10, 10).byte_len(399),
            Err(OverlayError::MemoryLimit { .. })
        ));
        assert_eq!(Size::new(10, 10).byte_len(400).unwrap(), 400);
        assert!(ScaleFactor::from_integer(2)
            .unwrap()
            .physical_size(Size::new(u32::MAX, 2))
            .is_err());
    }
    #[test]
    fn fractional_scaling_rounds_ties_up_and_preserves_bounds() {
        let scale = ScaleFactor::from_120ths(150).unwrap();
        assert_eq!(
            scale.physical_size(Size::new(102, 101)).unwrap(),
            Size::new(128, 126)
        );
        assert_eq!(scale.as_f64(), 1.25);
        assert_eq!(ScaleFactor::ONE.as_integer(), Some(1));
        assert_eq!(scale.as_integer(), None);
        assert!(ScaleFactor::from_120ths(0).is_err());
        assert!(ScaleFactor::from_integer(u32::MAX).is_err());
        assert!(ScaleFactor::from_120ths(u32::MAX)
            .unwrap()
            .physical_size(Size::new(1000, 1000))
            .is_err());
        assert_eq!(
            ScaleFactor::from_120ths(1)
                .unwrap()
                .physical_size(Size::new(1, 1))
                .unwrap(),
            Size::new(1, 1)
        );
    }
}
