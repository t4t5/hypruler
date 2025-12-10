mod capture;
mod render;

use capture::{capture_screen, Screenshot};
use render::{draw_crosshair, draw_measurements, find_edges};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{
            cursor_shape::CursorShapeManager, PointerEvent, PointerEventKind, PointerHandler,
        },
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use tiny_skia::Pixmap;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    self, WpCursorShapeDeviceV1,
};

struct PixelSnap {
    // Wayland protocol state (required by smithay-client-toolkit)
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    layer_shell: LayerShell,

    // Overlay surface
    layer_surface: Option<LayerSurface>,
    pool: Option<SlotPool>,
    width: u32,
    height: u32,
    scale: i32,

    // Cursor and input
    pointer_x: f64,
    pointer_y: f64,
    cursor_shape_manager: Option<CursorShapeManager>,
    cursor_shape_device: Option<WpCursorShapeDeviceV1>,

    // Rendering
    font: Option<fontdue::Font>,
    needs_redraw: bool,
    cached_pixmap: Option<Pixmap>,
    screenshot: Screenshot,

    // App state
    exit: bool,
}

impl PixelSnap {
    #[allow(clippy::too_many_arguments)]
    fn new(
        registry_state: RegistryState,
        seat_state: SeatState,
        output_state: OutputState,
        compositor_state: CompositorState,
        shm: Shm,
        layer_shell: LayerShell,
        screenshot: Screenshot,
        cursor_shape_manager: Option<CursorShapeManager>,
    ) -> Self {
        let font_data = include_bytes!("/usr/share/fonts/noto/NotoSans-Regular.ttf");
        let font =
            fontdue::Font::from_bytes(font_data as &[u8], fontdue::FontSettings::default()).ok();

        Self {
            registry_state,
            seat_state,
            output_state,
            compositor_state,
            shm,
            layer_shell,
            exit: false,
            width: 0,
            height: 0,
            scale: 1,
            layer_surface: None,
            pool: None,
            pointer_x: 0.0,
            pointer_y: 0.0,
            cursor_shape_manager,
            cursor_shape_device: None,
            font,
            needs_redraw: true,
            cached_pixmap: None,
            screenshot,
        }
    }

    fn draw(&mut self, _qh: &QueueHandle<Self>) {
        if self.layer_surface.is_none() || self.pool.is_none() {
            return;
        }
        if self.width == 0 || self.height == 0 || !self.needs_redraw {
            return;
        }
        self.needs_redraw = false;

        let phys_width = self.screenshot.width;
        let phys_height = self.screenshot.height;
        let scale = self.scale as f32;

        let cursor_phys_x = (self.pointer_x * scale as f64) as u32;
        let cursor_phys_y = (self.pointer_y * scale as f64) as u32;

        let pool = self.pool.as_mut().unwrap();
        let stride = phys_width as i32 * 4;
        let size = (stride * phys_height as i32) as usize;

        if pool.len() < size {
            pool.resize(size).expect("Failed to resize pool");
        }

        let (buffer, canvas) = pool
            .create_buffer(
                phys_width as i32,
                phys_height as i32,
                stride,
                wl_shm::Format::Argb8888,
            )
            .expect("Failed to create buffer");

        // Copy pre-converted BGRA background
        let bgra_size = self.screenshot.bgra_data.len().min(size);
        canvas[..bgra_size].copy_from_slice(&self.screenshot.bgra_data[..bgra_size]);

        // Draw overlay
        if cursor_phys_x < self.screenshot.width && cursor_phys_y < self.screenshot.height {
            let needs_new_pixmap = self
                .cached_pixmap
                .as_ref()
                .map(|p| p.width() != phys_width || p.height() != phys_height)
                .unwrap_or(true);

            if needs_new_pixmap {
                self.cached_pixmap = Pixmap::new(phys_width, phys_height);
            }

            let pixmap = self.cached_pixmap.as_mut().unwrap();
            pixmap.fill(tiny_skia::Color::TRANSPARENT);

            let edges = find_edges(&self.screenshot, cursor_phys_x, cursor_phys_y);
            draw_measurements(
                pixmap,
                &edges,
                cursor_phys_x,
                cursor_phys_y,
                self.font.as_ref(),
            );
            draw_crosshair(pixmap, cursor_phys_x as f32, cursor_phys_y as f32);

            // Composite overlay onto canvas
            let overlay_data = pixmap.data();
            for (i, chunk) in canvas[..size].chunks_exact_mut(4).enumerate() {
                let src_idx = i * 4;
                let alpha = overlay_data[src_idx + 3];
                if alpha > 0 {
                    let src_r = overlay_data[src_idx] as u32;
                    let src_g = overlay_data[src_idx + 1] as u32;
                    let src_b = overlay_data[src_idx + 2] as u32;
                    let src_a = alpha as u32;

                    let dst_b = chunk[0] as u32;
                    let dst_g = chunk[1] as u32;
                    let dst_r = chunk[2] as u32;

                    let inv_a = 255 - src_a;
                    chunk[0] = ((src_b * src_a + dst_b * inv_a) / 255) as u8;
                    chunk[1] = ((src_g * src_a + dst_g * inv_a) / 255) as u8;
                    chunk[2] = ((src_r * src_a + dst_r * inv_a) / 255) as u8;
                    chunk[3] = 255;
                }
            }
        }

        let layer_surface = self.layer_surface.as_ref().unwrap();
        let surface = layer_surface.wl_surface();

        surface.set_buffer_scale(self.scale);
        buffer.attach_to(surface).expect("Failed to attach buffer");
        surface.damage_buffer(0, 0, phys_width as i32, phys_height as i32);
        surface.commit();
    }
}

impl CompositorHandler for PixelSnap {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if self.scale != new_factor {
            self.scale = new_factor;
            self.cached_pixmap = None;
            self.needs_redraw = true;
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.draw(qh);
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for PixelSnap {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for PixelSnap {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.width = configure.new_size.0;
        self.height = configure.new_size.1;

        let phys_width = self.width * self.scale as u32;
        let phys_height = self.height * self.scale as u32;
        let pool_size = (phys_width * phys_height * 4) as usize;

        if self.pool.is_none() {
            self.pool = Some(SlotPool::new(pool_size, &self.shm).expect("Failed to create pool"));
        }

        self.needs_redraw = true;
        self.draw(qh);
    }
}

impl SeatHandler for PixelSnap {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Ok(pointer) = self.seat_state.get_pointer(qh, &seat) {
                if let Some(ref manager) = self.cursor_shape_manager {
                    self.cursor_shape_device = Some(manager.get_shape_device(&pointer, qh));
                }
            }
        }
        if capability == Capability::Keyboard {
            let _ = self.seat_state.get_keyboard(qh, &seat, None);
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for PixelSnap {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
        self.exit = true;
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: u32,
    ) {
    }
}

impl PointerHandler for PixelSnap {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    if let Some(ref device) = self.cursor_shape_device {
                        device.set_shape(serial, wp_cursor_shape_device_v1::Shape::Crosshair);
                    }
                }
                PointerEventKind::Motion { .. } => {
                    self.pointer_x = event.position.0;
                    self.pointer_y = event.position.1;
                    self.needs_redraw = true;
                    self.draw(qh);
                }
                PointerEventKind::Press { button: 272, .. } => {
                    self.exit = true;
                }
                _ => {}
            }
        }
    }
}

impl ShmHandler for PixelSnap {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for PixelSnap {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(PixelSnap);
delegate_output!(PixelSnap);
delegate_shm!(PixelSnap);
delegate_seat!(PixelSnap);
delegate_keyboard!(PixelSnap);
delegate_pointer!(PixelSnap);
delegate_layer!(PixelSnap);
delegate_registry!(PixelSnap);

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");

    let screenshot = match capture_screen(&conn) {
        Ok(s) => s,
        Err(_) => std::process::exit(1),
    };

    let (globals, mut event_queue) = registry_queue_init(&conn).expect("Failed to init registry");
    let qh = event_queue.handle();

    let compositor_state =
        CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("layer shell not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");
    let seat_state = SeatState::new(&globals, &qh);
    let output_state = OutputState::new(&globals, &qh);
    let registry_state = RegistryState::new(&globals);
    let cursor_shape_manager = CursorShapeManager::bind(&globals, &qh).ok();

    let mut pixelsnap = PixelSnap::new(
        registry_state,
        seat_state,
        output_state,
        compositor_state,
        shm,
        layer_shell,
        screenshot,
        cursor_shape_manager,
    );

    let surface = pixelsnap.compositor_state.create_surface(&qh);
    let layer_surface = pixelsnap.layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Overlay,
        Some("pixelsnap"),
        None,
    );

    layer_surface.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer_surface.set_exclusive_zone(-1);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    layer_surface.commit();

    pixelsnap.layer_surface = Some(layer_surface);

    while !pixelsnap.exit {
        event_queue.blocking_dispatch(&mut pixelsnap).unwrap();
    }
}
