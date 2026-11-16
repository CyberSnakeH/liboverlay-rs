//! Optional full-frame comparison; the cached image is the last accepted submission.
use crate::{Frame, OverlayError, Result, Size};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rectangle {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl Rectangle {
    pub fn full(size: Size) -> Self {
        Self {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
        }
    }
}
#[derive(Default)]
pub(crate) struct Tracker {
    enabled: bool,
    previous: Vec<u32>,
    geometry: Option<(Size, u64)>,
}
impl Tracker {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }
    pub fn update(&mut self, frame: &Frame) -> Result<Option<Rectangle>> {
        if !self.enabled {
            return Ok(Some(Rectangle::full(frame.size)));
        }
        if self.geometry != Some((frame.size, frame.generation)) {
            if self.previous.len() != frame.pixels.len() {
                self.previous = Vec::new();
                self.previous
                    .try_reserve_exact(frame.pixels.len())
                    .map_err(|_| OverlayError::Allocation {
                        bytes: frame.pixels.len() * 4,
                    })?;
                self.previous.resize(frame.pixels.len(), 0);
            }
            self.previous.copy_from_slice(&frame.pixels);
            self.geometry = Some((frame.size, frame.generation));
            return Ok(Some(Rectangle::full(frame.size)));
        }
        let mut left = frame.size.width;
        let mut top = frame.size.height;
        let mut right = 0;
        let mut bottom = 0;
        let width = frame.size.width as usize;
        for (y, (old, new)) in self
            .previous
            .chunks_exact_mut(width)
            .zip(frame.pixels.chunks_exact(width))
            .enumerate()
        {
            if old == new {
                continue;
            }
            let first = old
                .iter()
                .zip(new)
                .position(|(a, b)| a != b)
                .expect("row differs");
            let last = old
                .iter()
                .zip(new)
                .rposition(|(a, b)| a != b)
                .expect("row differs");
            left = left.min(first as u32);
            right = right.max(last as u32 + 1);
            top = top.min(y as u32);
            bottom = y as u32 + 1;
            old[first..=last].copy_from_slice(&new[first..=last]);
        }
        Ok((right > left).then(|| Rectangle {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn damage_includes_erased_and_new_pixels_and_invalidates_after_resize() {
        let mut tracker = Tracker::new(true);
        let mut frame = Frame::new(Size::new(8, 6), 1, 1, 4096).unwrap();
        assert_eq!(
            tracker.update(&frame).unwrap(),
            Some(Rectangle::full(frame.size))
        );
        assert_eq!(tracker.update(&frame).unwrap(), None);
        frame.pixels[9] = 0xff112233;
        assert_eq!(
            tracker.update(&frame).unwrap(),
            Some(Rectangle {
                x: 1,
                y: 1,
                width: 1,
                height: 1
            })
        );
        frame.pixels[9] = 0;
        frame.pixels[38] = 0xff445566;
        assert_eq!(
            tracker.update(&frame).unwrap(),
            Some(Rectangle {
                x: 1,
                y: 1,
                width: 6,
                height: 4
            })
        );
        assert_eq!(tracker.update(&frame).unwrap(), None);
        frame.generation += 1;
        assert_eq!(
            tracker.update(&frame).unwrap(),
            Some(Rectangle::full(frame.size))
        );
    }
}
