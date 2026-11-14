//! Opt-in live checks, restricted to two windows owned by this test.
#![cfg(feature = "x11")]
use liboverlay::{Anchor, BackendPreference, ButtonState, OverlayBuilder, OverlayEvent};
use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    properties::WmHints,
    protocol::{xproto::*, xtest::ConnectionExt as _, Event},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const X: i16 = 80;
const Y: i16 = 80;
const WIDTH: u16 = 320;
const HEIGHT: u16 = 240;

struct Fixture {
    conn: RustConnection,
    root: Window,
    window: Window,
    original_pointer: (i16, i16),
    pointer_moved: bool,
    button_pressed: bool,
}
impl Fixture {
    fn new() -> TestResult<Self> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let visual = conn.setup().roots[screen].root_visual;
        let pointer = conn.query_pointer(root)?.reply()?;
        assert_eq!(
            u16::from(pointer.mask) & 0x1f00,
            0,
            "pointer buttons must be released before this test"
        );
        let window = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            X,
            Y,
            WIDTH,
            HEIGHT,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(0x204060)
                .event_mask(
                    EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::EXPOSURE,
                ),
        )?
        .check()?;
        let fixture = Self {
            conn,
            root,
            window,
            original_pointer: (pointer.root_x, pointer.root_y),
            pointer_moved: false,
            button_pressed: false,
        };
        fixture
            .conn
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"liboverlay owned visual/click fixture",
            )?
            .check()?;
        let mut hints = WmHints::new();
        hints.input = Some(false);
        hints.set(&fixture.conn, window)?.check()?;
        fixture.conn.map_window(window)?.check()?;
        let gc = fixture.conn.generate_id()?;
        fixture
            .conn
            .create_gc(gc, window, &CreateGCAux::new().foreground(0x90a0b0))?
            .check()?;
        let mut tiles = Vec::new();
        for row in 0..6_i16 {
            for column in 0..8_i16 {
                if (row + column) % 2 == 1 {
                    tiles.push(Rectangle {
                        x: column * 40,
                        y: row * 40,
                        width: 40,
                        height: 40,
                    });
                }
            }
        }
        fixture
            .conn
            .poly_fill_rectangle(window, gc, &tiles)?
            .check()?;
        fixture.conn.free_gc(gc)?.check()?;
        fixture
            .conn
            .configure_window(
                window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?
            .check()?;
        fixture.conn.flush()?;
        std::thread::sleep(Duration::from_millis(250));
        Ok(fixture)
    }
    fn overlay_id(&self, title: &str) -> TestResult<Window> {
        for child in self.conn.query_tree(self.root)?.reply()?.children {
            if self
                .conn
                .get_property(false, child, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)?
                .reply()?
                .value
                == title.as_bytes()
            {
                return Ok(child);
            }
        }
        Err("owned overlay window not found".into())
    }
    fn click_owned(&mut self, expected_window: Window) -> TestResult<()> {
        self.pointer_moved = true;
        self.conn
            .xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, self.root, X + 160, Y + 120, 0)?
            .check()?;
        self.conn.flush()?;
        std::thread::sleep(Duration::from_millis(150));
        let pointer = self.conn.query_pointer(self.root)?.reply()?;
        assert_eq!(
            pointer.child, expected_window,
            "click target must be this test's own window"
        );
        self.conn
            .xtest_fake_input(BUTTON_PRESS_EVENT, 1, 0, self.root, 0, 0, 0)?
            .check()?;
        self.button_pressed = true;
        self.conn
            .xtest_fake_input(BUTTON_RELEASE_EVENT, 1, 0, self.root, 0, 0, 0)?
            .check()?;
        self.button_pressed = false;
        self.conn.flush()?;
        Ok(())
    }
    fn button_events(&self) -> TestResult<(usize, usize)> {
        let mut presses = 0;
        let mut releases = 0;
        while let Some(event) = self.conn.poll_for_event()? {
            match event {
                Event::ButtonPress(e) if e.event == self.window && e.detail == 1 => presses += 1,
                Event::ButtonRelease(e) if e.event == self.window && e.detail == 1 => releases += 1,
                _ => {}
            }
        }
        Ok((presses, releases))
    }
    fn capture(
        &self,
        path: &std::path::Path,
        reference: Option<&std::path::Path>,
    ) -> TestResult<()> {
        assert_eq!(
            self.conn
                .get_window_attributes(self.window)?
                .reply()?
                .map_state,
            MapState::VIEWABLE
        );
        let helper =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/visual_input_capture.py");
        let mut command = Command::new("/usr/bin/python3");
        command
            .arg(helper)
            .args([
                X.to_string(),
                Y.to_string(),
                WIDTH.to_string(),
                HEIGHT.to_string(),
            ])
            .arg(path);
        if let Some(reference) = reference {
            command.arg("--reference").arg(reference);
        }
        let result = command.output()?;
        println!("{}", String::from_utf8_lossy(&result.stdout));
        if !result.status.success() {
            return Err(format!(
                "compositor capture unavailable or assertion failed: {}",
                String::from_utf8_lossy(&result.stderr)
            )
            .into());
        }
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.button_pressed {
            let _ = self
                .conn
                .xtest_fake_input(BUTTON_RELEASE_EVENT, 1, 0, self.root, 0, 0, 0);
        }
        if self.pointer_moved {
            let _ = self.conn.warp_pointer(
                x11rb::NONE,
                self.root,
                0,
                0,
                0,
                0,
                self.original_pointer.0,
                self.original_pointer.1,
            );
        }
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

#[test]
#[ignore = "moves/restores the pointer and sends three XTEST clicks only to owned windows"]
fn xwayland_click_routing_and_focus() -> TestResult {
    let mut fixture = Fixture::new()?;
    let original_focus = fixture.conn.get_input_focus()?.reply()?.focus;
    let title = format!("liboverlay click routing {}", std::process::id());
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_backend(BackendPreference::X11Only)
        .with_anchor(Anchor::TopLeft)
        .with_margin(X as u32)
        .with_size(WIDTH as u32, HEIGHT as u32)
        .build()?;
    let overlay_id = fixture.overlay_id(&title)?;
    for click_through in [true, false, true] {
        overlay.set_click_through(click_through)?;
        fixture.button_events()?;
        overlay.poll_events()?;
        fixture.click_owned(if click_through {
            fixture.window
        } else {
            overlay_id
        })?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut underlay = (0, 0);
        let mut received = (0, 0);
        loop {
            let events = fixture.button_events()?;
            underlay.0 += events.0;
            underlay.1 += events.1;
            for event in overlay.poll_events()? {
                match event {
                    OverlayEvent::PointerButton {
                        button: 272,
                        state: ButtonState::Pressed,
                    } => received.0 += 1,
                    OverlayEvent::PointerButton {
                        button: 272,
                        state: ButtonState::Released,
                    } => received.1 += 1,
                    _ => {}
                }
            }
            if (click_through && underlay == (1, 1)) || (!click_through && received == (1, 1)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "click routing timed out: underlay={underlay:?}, overlay={received:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(underlay, if click_through { (1, 1) } else { (0, 0) });
        assert_eq!(received, if click_through { (0, 0) } else { (1, 1) });
        assert_eq!(
            fixture.conn.get_input_focus()?.reply()?.focus,
            original_focus,
            "click changed focus"
        );
        println!("click_through={click_through}; underlay={underlay:?}; overlay={received:?}; focus=unchanged");
    }
    overlay.close()?;
    println!("XTEST routing passed; pointer restored on fixture destruction; native Wayland injection not tested");
    Ok(())
}

#[test]
#[ignore = "captures only an owned 320x240 rectangle through KWin ScreenShot2"]
fn compositor_alpha_x11() -> TestResult {
    let fixture = Fixture::new()?;
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/evidence/visual-input");
    std::fs::create_dir_all(&directory)?;
    let baseline = directory.join("x11-background.png");
    fixture.capture(&baseline, None)?;
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay alpha fixture X11")
        .with_backend(BackendPreference::X11Only)
        .with_anchor(Anchor::TopLeft)
        .with_margin(X as u32)
        .with_size(WIDTH as u32, HEIGHT as u32)
        .build()?;
    overlay.draw(|pixels, width, height| {
        for y in 20..height - 20 {
            for x in 30..110 {
                pixels[(y * width + x) as usize] = liboverlay::premultiply(40, 220, 100, 128);
            }
            for x in 125..205 {
                pixels[(y * width + x) as usize] = liboverlay::premultiply(220, 40, 80, 255);
            }
        }
    })?;
    std::thread::sleep(Duration::from_millis(350));
    fixture.capture(&directory.join("x11-composited.png"), Some(&baseline))?;
    overlay.close()?;
    Ok(())
}

#[cfg(all(feature = "gpu", feature = "wayland"))]
#[test]
#[ignore = "opens a hardware Wayland GPU overlay and captures an owned rectangle through KWin"]
fn compositor_alpha_wayland_gpu() -> TestResult {
    use liboverlay::RendererPreference;
    let fixture = Fixture::new()?;
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/evidence/visual-input");
    std::fs::create_dir_all(&directory)?;
    let baseline = directory.join("wayland-background.png");
    fixture.capture(&baseline, None)?;
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay alpha fixture Wayland GPU")
        .with_backend(BackendPreference::WaylandOnly)
        .with_renderer(RendererPreference::GpuRequired)
        .with_anchor(Anchor::TopLeft)
        .with_margin(X as u32)
        .with_size(WIDTH as u32, HEIGHT as u32)
        .with_startup_timeout(Duration::from_secs(20))
        .build()?;
    draw_gpu_pattern(&overlay, 0)?;
    std::thread::sleep(Duration::from_millis(500));
    fixture.capture(
        &directory.join("wayland-gpu-composited.png"),
        Some(&baseline),
    )?;
    overlay.close()?;
    Ok(())
}

#[cfg(all(feature = "gpu", feature = "wayland"))]
fn draw_gpu_pattern(overlay: &liboverlay::OverlayWindow, phase: u32) -> TestResult {
    use liboverlay::gpu::wgpu;
    let context = overlay.gpu_context()?;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("owned alpha fixture"), source: wgpu::ShaderSource::Wgsl(r#"
            const PHASE: u32 = PHASE_NUM;
            struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
            @vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
                var vertices = array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
                var out: Out; out.position=vec4(vertices[i],0.,1.);
                out.uv=vec2((vertices[i].x+1.)*.5,(1.-vertices[i].y)*.5); return out;
            }
            @fragment fn fs(in: Out) -> @location(0) vec4<f32> {
                let p=in.uv*vec2(320.,240.);
                if p.y>=20. && p.y<220. {
                    if p.x>=30. && p.x<110. { return vec4(40./255.,220./255.,100./255.,1.)*(128./255.); }
                    if p.x>=125. && p.x<205. { return vec4(220./255.,40./255.,80./255.,1.); }
                }
                if p.y>=226. && p.y<234. && p.x>=20. && p.x<80. {
                    let square=u32((p.x-20.)/20.);
                    if square<PHASE && p.x<20.+f32(square)*20.+10. { return vec4(1.); }
                }
                return vec4(0.);
            }
        "#.replace("PHASE_NUM", &phase.to_string()).into()) });
    let pipeline = context
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("owned alpha fixture"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: context.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
    overlay.draw_gpu(move |frame| {
        let mut pass = frame
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("owned alpha fixture"),
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
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, 0..1);
        Ok(())
    })?;
    Ok(())
}

#[cfg(all(feature = "gpu", feature = "wayland"))]
#[test]
#[ignore = "requires a user to click the owned Wayland GPU fixture three times"]
fn manual_wayland_visual_clicks() -> TestResult {
    use liboverlay::RendererPreference;
    let timeout = std::env::var("LIBOVERLAY_MANUAL_TIMEOUT_SECONDS")
        .ok()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(600)
        .clamp(10, 600);
    let fixture = Fixture::new()?;
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay manual Wayland GPU alpha/click fixture")
        .with_backend(BackendPreference::WaylandOnly)
        .with_renderer(RendererPreference::GpuRequired)
        .with_anchor(Anchor::TopLeft)
        .with_margin(X as u32)
        .with_size(WIDTH as u32, HEIGHT as u32)
        .with_startup_timeout(Duration::from_secs(20))
        .build()?;
    draw_gpu_pattern(&overlay, 1)?;
    std::thread::sleep(Duration::from_millis(300));
    println!("READY owned_fixture=80,80,320,240 click_target=240,200 native_backend=Wayland renderer=Gpu");
    println!("VISUAL: checkerboard background; left green semi-transparent patch; middle red opaque patch; transparent margins; 1/2/3 white stage markers.");
    println!(
        "USER ACTION: click the middle red patch once at each READY stage, three clicks total."
    );
    for (stage, click_through) in [true, false, true].into_iter().enumerate() {
        overlay.set_click_through(click_through)?;
        draw_gpu_pattern(&overlay, stage as u32 + 1)?;
        fixture.button_events()?;
        overlay.poll_events()?;
        println!(
            "READY stage={}/3 click_through={click_through} expected_receiver={}",
            stage + 1,
            if click_through {
                "owned-X11-underlay"
            } else {
                "owned-native-Wayland-overlay"
            }
        );
        let deadline = Instant::now() + Duration::from_secs(timeout);
        let mut underlay = (0, 0);
        let mut received = (0, 0);
        loop {
            let events = fixture.button_events()?;
            underlay.0 += events.0;
            underlay.1 += events.1;
            for event in overlay.poll_events()? {
                match event {
                    OverlayEvent::PointerButton {
                        button: 272,
                        state: ButtonState::Pressed,
                    } => received.0 += 1,
                    OverlayEvent::PointerButton {
                        button: 272,
                        state: ButtonState::Released,
                    } => received.1 += 1,
                    _ => {}
                }
            }
            if (click_through && underlay.0 > 0 && underlay.1 > 0)
                || (!click_through && received.0 > 0 && received.1 > 0)
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "manual stage {} timed out: underlay={underlay:?}, overlay={received:?}",
                    stage + 1
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            if click_through { received } else { underlay },
            (0, 0),
            "a click reached the wrong owned surface"
        );
        println!(
            "PASS stage={}/3 underlay={underlay:?} native_overlay={received:?}",
            stage + 1
        );
    }
    overlay.close()?;
    println!("Manual real-input routing passed; visual appearance still requires user's explicit observation.");
    Ok(())
}
