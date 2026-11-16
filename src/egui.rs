//! Optional egui integration on the overlay's existing Vulkan/wgpu surface.
//!
//! The application owns the UI thread. [`EguiOverlay::run`] drains input, builds
//! the UI and waits for command submission (not GPU completion or scanout). Call it
//! regularly, including for passive HUDs. No winit window or second surface is used.
//! Clipboard, URL opening, accessibility and native IME surrounding-text deletion
//! are not implemented by this adapter. See `docs/CPP.md` for its input contract.
use crate::{
    gpu::wgpu, ButtonState, InputResetScope, OverlayError, OverlayEvent, OverlayWindow, Result,
    ScrollAxis, ScrollUnit,
};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

pub use ::egui;

/// UI context, input translation and renderer for one GPU overlay.
///
/// The window must have GPU rendering enabled. Configure keyboard and pointer
/// interaction with `OverlayBuilder` before passing it here. Do not drain the
/// same window's event queue elsewhere. Run and drop on your application thread.
pub struct EguiOverlay {
    window: OverlayWindow,
    context: egui::Context,
    renderer: Arc<Mutex<egui_wgpu::Renderer>>,
    input: egui::RawInput,
    pointer: egui::Pos2,
    started: Instant,
    completion: Option<mpsc::Receiver<()>>,
    free: Vec<egui::TextureId>,
}

impl EguiOverlay {
    /// Attaches egui to an existing GPU overlay, without changing input policy.
    pub fn new(window: OverlayWindow) -> Result<Self> {
        let gpu = window.gpu_context()?;
        let renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false);
        Ok(Self {
            window,
            context: egui::Context::default(),
            renderer: Arc::new(Mutex::new(renderer)),
            input: egui::RawInput {
                focused: false,
                ..Default::default()
            },
            pointer: egui::Pos2::ZERO,
            started: Instant::now(),
            completion: None,
            free: Vec::new(),
        })
    }

    /// Underlying window for geometry, capability inspection and input policy.
    pub fn window(&self) -> &OverlayWindow {
        &self.window
    }

    /// UI context for styling and font configuration. Native scale is negotiated
    /// each frame; egui zoom is accounted for when translating pointer input.
    pub fn context(&self) -> &egui::Context {
        &self.context
    }

    fn finish_previous(&mut self) -> Result<()> {
        if let Some(rx) = self.completion.take() {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.completion = Some(rx);
                    return Err(OverlayError::Timeout("encoding the egui frame"));
                }
            }
        }
        let mut renderer = self.renderer.lock().map_err(|_| OverlayError::Poisoned)?;
        for id in self.free.drain(..) {
            renderer.free_texture(&id);
        }
        Ok(())
    }

    /// Builds and queues one UI frame. Returned platform output lets the caller
    /// handle clipboard/URLs if desired. This adapter does not perform those actions.
    ///
    /// At most one draw is outstanding. Superseded/resized frames may be discarded;
    /// texture updates survive that discard. A timeout retains the pending job and
    /// the next call waits again before modifying renderer resources. UI actions
    /// may already have occurred when drawing reports an error.
    pub fn run(&mut self, mut ui: impl FnMut(&egui::Context)) -> Result<egui::FullOutput> {
        self.finish_previous()?;
        let info = self.window.info()?;
        let zoom = self.context.zoom_factor();
        self.input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(
                info.logical_size.width as f32 / zoom,
                info.logical_size.height as f32 / zoom,
            ),
        ));
        self.input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(info.scale_factor.as_f64() as f32);
        self.input.time = Some(self.started.elapsed().as_secs_f64());
        for event in self.window.poll_events()? {
            translate(&mut self.input, &mut self.pointer, event, zoom);
        }
        if !self.window.is_running() {
            return Err(OverlayError::Closed);
        }
        let output = self.context.run(self.input.take(), |ctx| ui(ctx));
        let jobs = self
            .context
            .tessellate(output.shapes.clone(), output.pixels_per_point);
        let gpu = self.window.gpu_context()?;
        {
            let mut renderer = self.renderer.lock().map_err(|_| OverlayError::Poisoned)?;
            for (id, delta) in &output.textures_delta.set {
                renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        self.free.extend(output.textures_delta.free.iter().copied());
        let renderer = Arc::clone(&self.renderer);
        let pixels_per_point = output.pixels_per_point;
        let (tx, rx) = mpsc::sync_channel(1);
        self.window.draw_gpu(move |frame| {
            let mut renderer = renderer.lock().map_err(|_| OverlayError::Poisoned)?;
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [
                    frame.info.physical_size.width,
                    frame.info.physical_size.height,
                ],
                pixels_per_point,
            };
            let commands = renderer.update_buffers(
                &frame.context.device,
                &frame.context.queue,
                frame.encoder,
                &jobs,
                &screen,
            );
            {
                let pass = frame
                    .encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("liboverlay egui"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: frame.view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
            }
            // Submit before acknowledging: the next UI iteration may write to
            // these same textures/buffers. Leaving submission to the runtime
            // after waking the caller would permit those writes to overtake us.
            let encoded = std::mem::replace(
                frame.encoder,
                frame
                    .context
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("egui runtime continuation"),
                    }),
            )
            .finish();
            frame
                .context
                .queue
                .submit(commands.into_iter().chain(std::iter::once(encoded)));
            // The renderer lock is released before waking the application thread.
            drop(renderer);
            let _ = tx.send(());
            Ok(())
        })?;
        self.completion = Some(rx);
        // Wait here as well, so input/UI cannot outrun the display command queue.
        // Texture deletion is deferred until the next iteration.
        if let Some(rx) = self.completion.take() {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.completion = Some(rx);
                    return Err(OverlayError::Timeout("encoding the egui frame"));
                }
            }
        }
        Ok(output)
    }
}

fn modifiers(value: crate::KeyboardModifiers) -> egui::Modifiers {
    let mask = value.effective;
    egui::Modifiers {
        alt: mask & 8 != 0,
        ctrl: mask & 4 != 0,
        shift: mask & 1 != 0,
        mac_cmd: false,
        command: mask & 4 != 0,
    }
}

fn key(sym: u32) -> Option<egui::Key> {
    use egui::Key;
    Some(match sym {
        0xff08 => Key::Backspace,
        0xff09 | 0xfe20 => Key::Tab,
        0xff0d | 0xff8d => Key::Enter,
        0xff1b => Key::Escape,
        0x20 => Key::Space,
        0xff50 | 0xff95 => Key::Home,
        0xff51 | 0xff96 => Key::ArrowLeft,
        0xff52 | 0xff97 => Key::ArrowUp,
        0xff53 | 0xff98 => Key::ArrowRight,
        0xff54 | 0xff99 => Key::ArrowDown,
        0xff55 | 0xff9a => Key::PageUp,
        0xff56 | 0xff9b => Key::PageDown,
        0xff57 | 0xff9c => Key::End,
        0xff63 | 0xff9e => Key::Insert,
        0xffff | 0xff9f => Key::Delete,
        0xffbe..=0xffd5 => return Key::from_name(&format!("F{}", sym - 0xffbe + 1)),
        0x21..=0x7e => {
            return Key::from_name(&char::from_u32(sym)?.to_ascii_uppercase().to_string())
        }
        _ => return None,
    })
}

fn translate(input: &mut egui::RawInput, pointer: &mut egui::Pos2, event: OverlayEvent, zoom: f32) {
    use egui::Event;
    match event {
        OverlayEvent::PointerMoved { x, y } => {
            *pointer = egui::pos2(x as f32 / zoom, y as f32 / zoom);
            input.events.push(Event::PointerMoved(*pointer));
        }
        OverlayEvent::PointerLeft => input.events.push(Event::PointerGone),
        OverlayEvent::PointerButton { button, state } => {
            let button = match button {
                272 => egui::PointerButton::Primary,
                273 => egui::PointerButton::Secondary,
                274 => egui::PointerButton::Middle,
                275 => egui::PointerButton::Extra1,
                276 => egui::PointerButton::Extra2,
                _ => return,
            };
            input.events.push(Event::PointerButton {
                pos: *pointer,
                button,
                pressed: state == ButtonState::Pressed,
                modifiers: input.modifiers,
            });
        }
        OverlayEvent::PointerAxis { axis, value, unit } => {
            let delta = -(value as f32)
                / if unit == ScrollUnit::Pixels {
                    zoom
                } else {
                    1.0
                };
            input.events.push(Event::MouseWheel {
                unit: match unit {
                    ScrollUnit::Pixels => egui::MouseWheelUnit::Point,
                    ScrollUnit::Steps => egui::MouseWheelUnit::Line,
                },
                delta: match axis {
                    ScrollAxis::Horizontal => egui::vec2(delta, 0.0),
                    ScrollAxis::Vertical => egui::vec2(0.0, delta),
                },
                modifiers: input.modifiers,
            });
        }
        OverlayEvent::ModifiersChanged {
            modifiers: value, ..
        } => input.modifiers = modifiers(value),
        OverlayEvent::KeyboardInput(value) => {
            input.modifiers = modifiers(value.modifiers);
            if let Some(key) = value.keysyms.iter().find_map(|sym| key(*sym)) {
                input.events.push(Event::Key {
                    key,
                    physical_key: None,
                    pressed: value.state == ButtonState::Pressed,
                    repeat: value.repeat,
                    modifiers: input.modifiers,
                });
            }
        }
        OverlayEvent::TextCommitted { text, .. } => input.events.push(Event::Text(text)),
        OverlayEvent::KeyboardFocus { focused, .. } => {
            input.focused = focused;
            input.events.push(Event::WindowFocused(focused));
        }
        OverlayEvent::InputReset {
            scope: scope @ (InputResetScope::Keyboard | InputResetScope::All),
            ..
        } => {
            input.modifiers = egui::Modifiers::default();
            input.focused = false;
            input.events.push(Event::WindowFocused(false));
            if scope == InputResetScope::All {
                for button in [
                    egui::PointerButton::Primary,
                    egui::PointerButton::Secondary,
                    egui::PointerButton::Middle,
                    egui::PointerButton::Extra1,
                    egui::PointerButton::Extra2,
                ] {
                    input.events.push(Event::PointerButton {
                        pos: *pointer,
                        button,
                        pressed: false,
                        modifiers: input.modifiers,
                    });
                }
                input.events.push(Event::PointerGone);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_preserves_logical_coordinates_scroll_and_single_text_commit() {
        let mut input = egui::RawInput::default();
        let mut pointer = egui::Pos2::ZERO;
        translate(
            &mut input,
            &mut pointer,
            OverlayEvent::PointerMoved { x: 120.0, y: 60.0 },
            2.0,
        );
        translate(
            &mut input,
            &mut pointer,
            OverlayEvent::PointerAxis {
                axis: ScrollAxis::Vertical,
                value: 2.0,
                unit: ScrollUnit::Steps,
            },
            2.0,
        );
        translate(
            &mut input,
            &mut pointer,
            OverlayEvent::TextCommitted {
                source: crate::InputId {
                    seat: 1,
                    device: None,
                },
                text: "été".into(),
            },
            2.0,
        );
        assert_eq!(pointer, egui::pos2(60.0, 30.0));
        assert!(
            matches!(input.events[1], egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, .. } if delta.y == -2.0)
        );
        assert!(matches!(&input.events[2], egui::Event::Text(text) if text == "été"));
        assert_eq!(key(0xff51), Some(egui::Key::ArrowLeft));
        assert_eq!(key(b'a' as u32), Some(egui::Key::A));
    }
}
