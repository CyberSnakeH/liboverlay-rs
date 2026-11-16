//! Virtual pointer fixture. Refuses every display except an owned test server.
use std::io::{self, BufRead, Write};
use wayland_client::{
    delegate_noop,
    protocol::{wl_output, wl_registry, wl_seat},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1 as manager, zwlr_virtual_pointer_v1 as pointer,
};

#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<manager::ZwlrVirtualPointerManagerV1>,
    output: Option<wl_output::WlOutput>,
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
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, version.min(5), qh, ()))
                }
                "wl_output" if state.output.is_none() => {
                    state.output = Some(registry.bind(name, version.min(2), qh, ()))
                }
                "zwlr_virtual_pointer_manager_v1" => {
                    state.manager = Some(registry.bind(name, version.min(2), qh, ()))
                }
                _ => {}
            }
        }
    }
}
delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore wl_output::WlOutput);
delegate_noop!(State: ignore manager::ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ignore pointer::ZwlrVirtualPointerV1);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::var("XDG_RUNTIME_DIR")?;
    if std::env::var("LIBOVERLAY_TEST_PRIVATE_INPUT").as_deref() != Ok("1")
        || !directory.starts_with("/tmp/liboverlay-sway-")
    {
        return Err("virtual input is restricted to scripts/check_cpp.py's private server".into());
    }
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue();
    let qh = queue.handle();
    let mut state = State::default();
    connection.display().get_registry(&qh, ());
    queue.roundtrip(&mut state)?;
    let pointer = state
        .manager
        .as_ref()
        .ok_or("no virtual-pointer protocol")?
        .create_virtual_pointer_with_output(state.seat.as_ref(), state.output.as_ref(), &qh, ());
    queue.roundtrip(&mut state)?;
    println!("ready");
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        let values: Vec<_> = line.split_whitespace().collect();
        match values.as_slice() {
            ["move", x, y] => pointer.motion_absolute(1, x.parse()?, y.parse()?, 640, 480),
            ["down"] => pointer.button(
                1,
                272,
                wayland_client::protocol::wl_pointer::ButtonState::Pressed,
            ),
            ["up"] => pointer.button(
                1,
                272,
                wayland_client::protocol::wl_pointer::ButtonState::Released,
            ),
            _ => return Err("unknown test input command".into()),
        }
        pointer.frame();
        queue.roundtrip(&mut state)?;
        println!("done");
        io::stdout().flush()?;
    }
    pointer.destroy();
    connection.flush()?;
    Ok(())
}
