//! Monitor selection and checked placement in the X11 coordinate space.
use super::connection::XConnection;
use crate::{Anchor, OutputSelector, OverlayError, Result, Size};
use x11rb::{
    connection::RequestConnection,
    protocol::{
        randr::{self, ConnectionExt as _},
        xproto::{ConnectionExt as _, Window},
    },
};

pub(super) fn monitor_geometry(
    conn: &XConnection,
    root: Window,
    selector: &OutputSelector,
    warnings: &mut Vec<String>,
) -> Result<Option<(i32, i32, u32, u32)>> {
    if conn
        .extension_information(randr::X11_EXTENSION_NAME)?
        .is_some()
    {
        let version = conn.randr_query_version(1, 5)?.reply()?;
        if (version.major_version, version.minor_version) >= (1, 5) {
            let monitors = conn.randr_get_monitors(root, true)?.reply()?.monitors;
            for monitor in &monitors {
                let chosen = match selector {
                    OutputSelector::Automatic => {
                        monitor.primary
                            || (!monitors.iter().any(|m| m.primary)
                                && monitors.first().is_some_and(|m| m.name == monitor.name))
                    }
                    OutputSelector::Name(name) => {
                        conn.get_atom_name(monitor.name)?.reply()?.name == name.as_bytes()
                    }
                };
                if chosen {
                    return Ok(Some((
                        i32::from(monitor.x),
                        i32::from(monitor.y),
                        u32::from(monitor.width),
                        u32::from(monitor.height),
                    )));
                }
            }
        }
    }
    if matches!(selector, OutputSelector::Name(_)) {
        return Ok(None);
    }
    warnings.push("No RandR monitor geometry; placement uses the complete X11 screen.".into());
    let geometry = conn.get_geometry(root)?.reply()?;
    Ok(Some((
        0,
        0,
        u32::from(geometry.width),
        u32::from(geometry.height),
    )))
}

pub(super) fn anchored_size(
    anchor: Anchor,
    margin: u32,
    requested: Size,
    monitor: (i32, i32, u32, u32),
) -> Result<Size> {
    if anchor != Anchor::Fill {
        return Ok(requested);
    }
    let inset = margin
        .checked_mul(2)
        .ok_or(OverlayError::InvalidSize(requested))?;
    let size = Size::new(
        monitor
            .2
            .checked_sub(inset)
            .ok_or(OverlayError::InvalidSize(requested))?,
        monitor
            .3
            .checked_sub(inset)
            .ok_or(OverlayError::InvalidSize(requested))?,
    );
    size.byte_len(usize::MAX)?;
    Ok(size)
}
pub(super) fn placement(
    anchor: Anchor,
    margin: u32,
    size: Size,
    monitor: (i32, i32, u32, u32),
) -> Result<(i16, i16)> {
    let (x, y, w, h) = monitor;
    let dx = i64::from(w) - i64::from(size.width);
    let dy = i64::from(h) - i64::from(size.height);
    let margin = i64::from(margin);
    let px = match anchor {
        Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft => margin,
        Anchor::TopRight | Anchor::Right | Anchor::BottomRight => dx - margin,
        Anchor::Fill => margin,
        _ => dx / 2,
    };
    let py = match anchor {
        Anchor::TopLeft | Anchor::Top | Anchor::TopRight => margin,
        Anchor::BottomLeft | Anchor::Bottom | Anchor::BottomRight => dy - margin,
        Anchor::Fill => margin,
        _ => dy / 2,
    };
    Ok((
        (i64::from(x) + px)
            .try_into()
            .map_err(|_| OverlayError::Unsupported("X11 horizontal coordinate range"))?,
        (i64::from(y) + py)
            .try_into()
            .map_err(|_| OverlayError::Unsupported("X11 vertical coordinate range"))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn anchors_respect_negative_monitor_origins_and_margins() {
        assert_eq!(
            placement(
                Anchor::Center,
                0,
                Size::new(800, 600),
                (-1920, 0, 1920, 1080)
            )
            .unwrap(),
            (-1360, 240)
        );
        assert_eq!(
            placement(
                Anchor::BottomRight,
                10,
                Size::new(800, 600),
                (0, 0, 1920, 1080)
            )
            .unwrap(),
            (1110, 470)
        );
        assert!(placement(Anchor::Right, 0, Size::new(1, 1), (32767, 0, 100, 100)).is_err());
    }

    #[test]
    fn fill_geometry_tracks_monitor_changes_and_rejects_excessive_margins() {
        let requested = Size::new(800, 600);
        let monitor = (-1920, -1080, 1920, 1080);
        let size = anchored_size(Anchor::Fill, 16, requested, monitor).unwrap();
        assert_eq!(size, Size::new(1888, 1048));
        assert_eq!(
            placement(Anchor::Fill, 16, size, monitor).unwrap(),
            (-1904, -1064)
        );
        assert_eq!(
            anchored_size(Anchor::Center, 16, requested, monitor).unwrap(),
            requested
        );
        assert!(anchored_size(Anchor::Fill, 540, requested, monitor).is_err());
        assert!(anchored_size(Anchor::Fill, u32::MAX, requested, monitor).is_err());
    }
}
