//! An explicit bottom-layer input target for the private compositor fixture.
//! Same-layer stacking between two overlays is compositor policy, so it cannot
//! define the foreground/background relationship tested by the input suite.

use liboverlay::{ButtonState, OverlayEvent};
use std::{cell::RefCell, fs::File, io::Write, os::fd::AsFd};
use wayland_client::{
    delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
        wl_surface,
    },
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1 as shell, zwlr_layer_surface_v1 as layer,
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Default)]
struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    manager: Option<shell::ZwlrLayerShellV1>,
    outputs: Vec<(wl_output::WlOutput, String)>,
    surface: Option<wl_surface::WlSurface>,
    layer: Option<layer::ZwlrLayerSurfaceV1>,
    buffers: Vec<(File, wl_buffer::WlBuffer)>,
    size: Option<(u32, u32)>,
    events: Vec<OverlayEvent>,
    error: Option<String>,
}

impl State {
    fn render(&mut self, width: u32, height: u32, qh: &QueueHandle<Self>) -> Result {
        if self.size == Some((width, height)) {
            return Ok(());
        }
        let bytes = width as u64 * height as u64 * 4;
        if width == 0 || height == 0 || bytes > 64 * 1024 * 1024 || self.buffers.len() >= 8 {
            return Err("private background geometry exceeds fixture limits".into());
        }
        let mut file = File::from(rustix::fs::memfd_create(
            c"liboverlay-input-background",
            rustix::fs::MemfdFlags::CLOEXEC,
        )?);
        file.write_all(&vec![0; bytes as usize])?;
        let pool = self
            .shm
            .as_ref()
            .ok_or("private background has no SHM")?
            .create_pool(file.as_fd(), bytes as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            width as i32 * 4,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );
        pool.destroy();
        let surface = self
            .surface
            .as_ref()
            .ok_or("private background has no surface")?;
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.commit();
        self.size = Some((width, height));
        // Every configure size receives its own immutable backing allocation.
        self.buffers.push((file, buffer));
        Ok(())
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()))
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "zwlr_layer_shell_v1" => {
                    state.manager = Some(registry.bind(name, version.min(4), qh, ()))
                }
                "wl_seat" => {
                    let _: wl_seat::WlSeat = registry.bind(name, version.min(5), qh, ());
                }
                "wl_output" => state
                    .outputs
                    .push((registry.bind(name, version.min(4), qh, ()), String::new())),
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(qh, ());
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some((_, current)) = state
                .outputs
                .iter_mut()
                .find(|(candidate, _)| candidate == output)
            {
                *current = name;
            }
        }
    }
}

impl Dispatch<layer::ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer: &layer::ZwlrLayerSurfaceV1,
        event: layer::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            layer::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                if let Err(error) = state.render(width, height, qh) {
                    state.error = Some(error.to_string());
                }
            }
            layer::Event::Closed => state.error = Some("private background was closed".into()),
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            button,
            state: WEnum::Value(value),
            ..
        } = event
        {
            let value = match value {
                wl_pointer::ButtonState::Pressed => ButtonState::Pressed,
                wl_pointer::ButtonState::Released => ButtonState::Released,
                _ => return,
            };
            state.events.push(OverlayEvent::PointerButton {
                button,
                state: value,
            });
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore shell::ZwlrLayerShellV1);

pub struct Background {
    connection: Connection,
    queue: RefCell<EventQueue<State>>,
    state: RefCell<State>,
}

impl Background {
    pub fn new() -> Result<Self> {
        if std::env::var_os("LIBOVERLAY_TEST_PRIVATE_INPUT").is_none() {
            return Err("private compositor required".into());
        }
        let connection = Connection::connect_to_env()?;
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        let mut state = State::default();
        connection.display().get_registry(&qh, ());
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        let output = &state
            .outputs
            .iter()
            .find(|(_, name)| name == "HEADLESS-1")
            .ok_or("private background output unavailable")?
            .0;
        let surface = state
            .compositor
            .as_ref()
            .ok_or("private compositor unavailable")?
            .create_surface(&qh, ());
        let layer = state
            .manager
            .as_ref()
            .ok_or("private layer shell unavailable")?
            .get_layer_surface(
                &surface,
                Some(output),
                shell::Layer::Bottom,
                "liboverlay-input-background".into(),
                &qh,
                (),
            );
        layer.set_anchor(
            layer::Anchor::Top | layer::Anchor::Bottom | layer::Anchor::Left | layer::Anchor::Right,
        );
        layer.set_size(0, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(layer::KeyboardInteractivity::None);
        state.layer = Some(layer);
        state.surface = Some(surface.clone());
        surface.commit();
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        if let Some(error) = state.error.take() {
            return Err(error.into());
        }
        if state.size.is_none() {
            return Err("private background was not configured".into());
        }
        Ok(Self {
            connection,
            queue: RefCell::new(queue),
            state: RefCell::new(state),
        })
    }
    pub fn poll_events(&self) -> Result<Vec<OverlayEvent>> {
        let mut state = self.state.borrow_mut();
        self.queue.borrow_mut().roundtrip(&mut *state)?;
        if let Some(error) = state.error.take() {
            return Err(error.into());
        }
        Ok(std::mem::take(&mut state.events))
    }
    pub fn close(self) -> Result {
        drop(self);
        Ok(())
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        for (_, buffer) in &state.buffers {
            buffer.destroy();
        }
        if let Some(layer) = &state.layer {
            layer.destroy();
        }
        if let Some(surface) = &state.surface {
            surface.destroy();
        }
        let _ = self.connection.flush();
    }
}
