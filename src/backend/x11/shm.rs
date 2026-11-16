//! File-descriptor MIT-SHM uploads on the local X11 transport.
use super::XConnection;
use crate::{Frame, Result, Size};
use memmap2::{MmapMut, MmapOptions};
use std::fs::File;
use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::{
        shm::{self, ConnectionExt as _},
        xproto::{Gcontext, ImageFormat, ImageOrder, Pixmap},
    },
};

pub(super) struct Upload {
    segment: shm::Seg,
    map: MmapMut,
    size: Size,
    busy: bool,
}

impl Upload {
    pub(super) fn supported(conn: &XConnection) -> Result<bool> {
        if conn
            .extension_information(shm::X11_EXTENSION_NAME)?
            .is_none()
        {
            return Ok(false);
        }
        let version = conn.shm_query_version()?.reply()?;
        Ok((version.major_version, version.minor_version) >= (1, 2))
    }

    pub(super) fn create(conn: &XConnection, size: Size, limit: usize) -> Result<Self> {
        let bytes = size.byte_len(limit)?;
        let file = File::from(
            rustix::fs::memfd_create(
                c"liboverlay-x11",
                rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
            )
            .map_err(std::io::Error::from)?,
        );
        file.set_len(bytes as u64)?;
        rustix::fs::fcntl_add_seals(
            &file,
            rustix::fs::SealFlags::SHRINK | rustix::fs::SealFlags::GROW,
        )
        .map_err(std::io::Error::from)?;
        // SAFETY: this private, fixed-length memfd has no writers other than us.
        // The server receives a read-only mapping; completion gates every reuse.
        let map = unsafe { MmapOptions::new().len(bytes).map_mut(&file) }?;
        let segment = conn.generate_id()?;
        conn.shm_attach_fd(segment, std::os::fd::OwnedFd::from(file), true)?
            .check()?;
        Ok(Self {
            segment,
            map,
            size,
            busy: false,
        })
    }

    pub(super) fn busy(&self) -> bool {
        self.busy
    }
    pub(super) fn size(&self) -> Size {
        self.size
    }

    pub(super) fn completed(&mut self, segment: shm::Seg) {
        if self.segment == segment {
            self.busy = false;
        }
    }

    pub(super) fn put(
        &mut self,
        conn: &XConnection,
        pixmap: Pixmap,
        gc: Gcontext,
        depth: u8,
        frame: &Frame,
        rectangle: crate::damage::Rectangle,
    ) -> Result<()> {
        debug_assert!(!self.busy);
        debug_assert_eq!(frame.size, self.size);
        let little = conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        for y in rectangle.y..rectangle.y + rectangle.height {
            let first = (y * self.size.width + rectangle.x) as usize;
            let last = first + rectangle.width as usize;
            for (destination, pixel) in self.map[first * 4..last * 4]
                .chunks_exact_mut(4)
                .zip(&frame.pixels[first..last])
            {
                destination.copy_from_slice(&if little {
                    pixel.to_le_bytes()
                } else {
                    pixel.to_be_bytes()
                });
            }
        }
        conn.shm_put_image(
            pixmap,
            gc,
            self.size.width as u16,
            self.size.height as u16,
            rectangle.x as u16,
            rectangle.y as u16,
            rectangle.width as u16,
            rectangle.height as u16,
            rectangle.x as i16,
            rectangle.y as i16,
            depth,
            u8::from(ImageFormat::Z_PIXMAP),
            true,
            self.segment,
            0,
        )?;
        self.busy = true;
        Ok(())
    }

    pub(super) fn detach(self, conn: &XConnection) -> Result<()> {
        // Ordered after all uploads on the same connection. The server retains
        // its mapping even if our mapping is dropped before it reads this request.
        conn.shm_detach(self.segment)?;
        Ok(())
    }
}
