use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
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
use std::sync::Arc;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect, Stroke, Transform};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

const LINE_WIDTH: f32 = 2.0;
const END_CAP_SIZE: f32 = 8.0;

fn line_color() -> Color {
    Color::from_rgba8(231, 76, 60, 255) // #E74C3C
}

fn label_bg_color() -> Color {
    Color::from_rgba8(40, 40, 40, 230)
}


#[derive(Debug, Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Copy)]
enum MeasureDirection {
    Horizontal,
    Vertical,
}

#[derive(Clone)]
struct MeasureState {
    start: Point,
    end: Point,
    direction: MeasureDirection,
}

struct PixelSnap {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    layer_shell: LayerShell,

    exit: bool,
    width: u32,  // Logical width
    height: u32, // Logical height
    scale: i32,  // HiDPI scale factor
    layer_surface: Option<LayerSurface>,
    pool: Option<SlotPool>,

    // Input state
    pointer_pos: Point,
    dragging: bool,
    measure: Option<MeasureState>,

    // Font for rendering
    font: Option<Arc<fontdue::Font>>,

    // Rendering optimization
    needs_redraw: bool,
    cached_pixmap: Option<Pixmap>,
}

impl PixelSnap {
    fn new(
        registry_state: RegistryState,
        seat_state: SeatState,
        output_state: OutputState,
        compositor_state: CompositorState,
        shm: Shm,
        layer_shell: LayerShell,
    ) -> Self {
        // Load a simple font for text rendering
        let font_data = include_bytes!("/usr/share/fonts/noto/NotoSans-Regular.ttf");
        let font = fontdue::Font::from_bytes(font_data as &[u8], fontdue::FontSettings::default()).ok();

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
            pointer_pos: Point { x: 0.0, y: 0.0 },
            dragging: false,
            measure: None,
            font: font.map(Arc::new),
            needs_redraw: true,
            cached_pixmap: None,
        }
    }

    fn draw(&mut self, _qh: &QueueHandle<Self>) {
        if self.layer_surface.is_none() || self.pool.is_none() {
            return;
        }
        if self.width == 0 || self.height == 0 {
            return;
        }
        if !self.needs_redraw {
            return;
        }
        self.needs_redraw = false;

        // Physical pixel dimensions (for HiDPI)
        let phys_width = self.width * self.scale as u32;
        let phys_height = self.height * self.scale as u32;
        let scale = self.scale as f32;

        // Create or reuse pixmap at physical resolution
        let needs_new_pixmap = self.cached_pixmap.as_ref()
            .map(|p| p.width() != phys_width || p.height() != phys_height)
            .unwrap_or(true);

        if needs_new_pixmap {
            self.cached_pixmap = Pixmap::new(phys_width, phys_height);
        }

        let pixmap = self.cached_pixmap.as_mut().unwrap();

        // Clear with subtle red tinted background
        pixmap.fill(Color::from_rgba8(255, 0, 0, 12));

        // Draw measurement if we have one (scale coordinates for HiDPI)
        if let Some(measure) = &self.measure {
            Self::draw_measurement_static(pixmap, measure, self.font.as_ref(), scale);
        }

        // Now get the pool and buffer
        let pool = self.pool.as_mut().unwrap();
        let stride = phys_width as i32 * 4;
        let size = (stride * phys_height as i32) as usize;

        let (buffer, canvas) = pool
            .create_buffer(
                phys_width as i32,
                phys_height as i32,
                stride,
                wl_shm::Format::Argb8888,
            )
            .expect("Failed to create buffer");

        // Copy pixmap data to wayland buffer (RGBA -> BGRA conversion)
        // Use direct copy for speed
        let pixmap_data = self.cached_pixmap.as_ref().unwrap().data();
        for (i, chunk) in canvas[..size].chunks_exact_mut(4).enumerate() {
            let src_idx = i * 4;
            chunk[0] = pixmap_data[src_idx + 2]; // B
            chunk[1] = pixmap_data[src_idx + 1]; // G
            chunk[2] = pixmap_data[src_idx + 0]; // R
            chunk[3] = pixmap_data[src_idx + 3]; // A
        }

        let layer_surface = self.layer_surface.as_ref().unwrap();
        let surface = layer_surface.wl_surface();

        // Tell compositor about the buffer scale
        surface.set_buffer_scale(self.scale);

        buffer.attach_to(surface).expect("Failed to attach buffer");
        surface.damage_buffer(0, 0, phys_width as i32, phys_height as i32);
        surface.commit();
    }

    fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    fn draw_measurement_static(pixmap: &mut Pixmap, measure: &MeasureState, font: Option<&Arc<fontdue::Font>>, scale: f32) {
        let mut paint = Paint::default();
        paint.set_color(line_color());
        paint.anti_alias = true;

        let stroke = Stroke {
            width: LINE_WIDTH * scale,
            ..Default::default()
        };

        // Scale logical coordinates to physical pixels
        let (x1, y1) = (measure.start.x as f32 * scale, measure.start.y as f32 * scale);
        let (x2, y2) = (measure.end.x as f32 * scale, measure.end.y as f32 * scale);

        // Draw main line
        let mut pb = PathBuilder::new();
        pb.move_to(x1, y1);
        pb.line_to(x2, y2);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }

        // Draw end caps
        match measure.direction {
            MeasureDirection::Horizontal => {
                Self::draw_end_cap(pixmap, &paint, &stroke, x1, y1, true, scale);
                Self::draw_end_cap(pixmap, &paint, &stroke, x2, y2, true, scale);
            }
            MeasureDirection::Vertical => {
                Self::draw_end_cap(pixmap, &paint, &stroke, x1, y1, false, scale);
                Self::draw_end_cap(pixmap, &paint, &stroke, x2, y2, false, scale);
            }
        }

        // Calculate distance in LOGICAL pixels (what user cares about)
        let distance = match measure.direction {
            MeasureDirection::Horizontal => (measure.end.x - measure.start.x).abs(),
            MeasureDirection::Vertical => (measure.end.y - measure.start.y).abs(),
        };

        let label = format!("{}", distance.round() as i32);
        let mid_x = (x1 + x2) / 2.0;
        let mid_y = (y1 + y2) / 2.0;

        Self::draw_label(pixmap, &label, mid_x, mid_y, measure.direction, font, scale);
    }

    fn draw_end_cap(pixmap: &mut Pixmap, paint: &Paint, stroke: &Stroke, x: f32, y: f32, vertical: bool, scale: f32) {
        let cap_size = END_CAP_SIZE * scale;
        let mut pb = PathBuilder::new();
        if vertical {
            pb.move_to(x, y - cap_size / 2.0);
            pb.line_to(x, y + cap_size / 2.0);
        } else {
            pb.move_to(x - cap_size / 2.0, y);
            pb.line_to(x + cap_size / 2.0, y);
        }
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, paint, stroke, Transform::identity(), None);
        }
    }

    fn draw_label(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, direction: MeasureDirection, font: Option<&Arc<fontdue::Font>>, scale: f32) {
        let font_size = 14.0 * scale;
        let padding_x = 8.0 * scale;
        let padding_y = 4.0 * scale;

        // Calculate text dimensions
        let text_width = text.len() as f32 * font_size * 0.6;
        let text_height = font_size;

        let label_width = text_width + padding_x * 2.0;
        let label_height = text_height + padding_y * 2.0;

        // Position label offset from the line
        let offset = 12.0 * scale;
        let (label_x, label_y) = match direction {
            MeasureDirection::Horizontal => (x - label_width / 2.0, y - label_height - offset),
            MeasureDirection::Vertical => (x + offset, y - label_height / 2.0),
        };

        // Draw rounded rectangle background
        let rect = Rect::from_xywh(label_x, label_y, label_width, label_height);
        if let Some(_rect) = rect {
            let mut bg_paint = Paint::default();
            bg_paint.set_color(label_bg_color());
            bg_paint.anti_alias = true;

            // Draw rounded rect using path
            let radius = 4.0 * scale;
            let mut pb = PathBuilder::new();
            pb.move_to(label_x + radius, label_y);
            pb.line_to(label_x + label_width - radius, label_y);
            pb.quad_to(label_x + label_width, label_y, label_x + label_width, label_y + radius);
            pb.line_to(label_x + label_width, label_y + label_height - radius);
            pb.quad_to(label_x + label_width, label_y + label_height, label_x + label_width - radius, label_y + label_height);
            pb.line_to(label_x + radius, label_y + label_height);
            pb.quad_to(label_x, label_y + label_height, label_x, label_y + label_height - radius);
            pb.line_to(label_x, label_y + radius);
            pb.quad_to(label_x, label_y, label_x + radius, label_y);
            pb.close();

            if let Some(path) = pb.finish() {
                pixmap.fill_path(&path, &bg_paint, FillRule::Winding, Transform::identity(), None);
            }
        }

        // Draw text using fontdue if available
        if let Some(font) = font {
            let mut cursor_x = label_x + padding_x;
            let baseline_y = label_y + padding_y + font_size * 0.8;

            let pixmap_width = pixmap.width() as i32;
            let pixmap_height = pixmap.height() as i32;
            let width = pixmap_width as usize;
            let pixels = pixmap.pixels_mut();

            for c in text.chars() {
                let (metrics, bitmap) = font.rasterize(c, font_size);

                if !bitmap.is_empty() {
                    for py in 0..metrics.height {
                        for px in 0..metrics.width {
                            let alpha = bitmap[py * metrics.width + px];
                            if alpha > 0 {
                                let draw_x = cursor_x as i32 + px as i32 + metrics.xmin;
                                let draw_y = baseline_y as i32 + py as i32 - metrics.height as i32 - metrics.ymin;

                                if draw_x >= 0 && draw_x < pixmap_width
                                   && draw_y >= 0 && draw_y < pixmap_height {
                                    let idx = draw_y as usize * width + draw_x as usize;
                                    if idx < pixels.len() {
                                        let pixel = &pixels[idx];
                                        let a = alpha as f32 / 255.0;
                                        // Blend white text onto existing pixel
                                        let bg_r = pixel.red() as f32;
                                        let bg_g = pixel.green() as f32;
                                        let bg_b = pixel.blue() as f32;
                                        let bg_a = pixel.alpha() as f32;

                                        let r = ((1.0 - a) * bg_r + a * 255.0) as u8;
                                        let g = ((1.0 - a) * bg_g + a * 255.0) as u8;
                                        let b = ((1.0 - a) * bg_b + a * 255.0) as u8;
                                        let new_a = ((1.0 - a) * bg_a + a * 255.0) as u8;

                                        // For premultiplied alpha, RGB must be <= A
                                        let r = r.min(new_a);
                                        let g = g.min(new_a);
                                        let b = b.min(new_a);

                                        if let Some(new_pixel) = PremultipliedColorU8::from_rgba(r, g, b, new_a) {
                                            pixels[idx] = new_pixel;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                cursor_x += metrics.advance_width;
            }
        }
    }

    fn update_measurement(&mut self, current_pos: Point) {
        if let Some(ref mut measure) = self.measure {
            let dx = (current_pos.x - measure.start.x).abs();
            let dy = (current_pos.y - measure.start.y).abs();

            // Constrain to horizontal or vertical based on larger delta
            if dx > dy {
                measure.direction = MeasureDirection::Horizontal;
                measure.end = Point {
                    x: current_pos.x,
                    y: measure.start.y,
                };
            } else {
                measure.direction = MeasureDirection::Vertical;
                measure.end = Point {
                    x: measure.start.x,
                    y: current_pos.y,
                };
            }
        }
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
            // Invalidate cached pixmap since dimensions changed
            self.cached_pixmap = None;
            self.request_redraw();
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        self.draw(qh);
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for PixelSnap {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for PixelSnap {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.width = configure.new_size.0;
        self.height = configure.new_size.1;

        // Calculate physical size for the buffer pool (account for HiDPI)
        let phys_width = self.width * self.scale as u32;
        let phys_height = self.height * self.scale as u32;
        let pool_size = (phys_width * phys_height * 4) as usize;

        if self.pool.is_none() {
            let pool = SlotPool::new(pool_size, &self.shm)
                .expect("Failed to create pool");
            self.pool = Some(pool);
        }

        self.request_redraw();
        self.draw(qh);
    }
}

impl SeatHandler for PixelSnap {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.seat_state.get_keyboard(qh, &seat, None).is_err() {
            // Keyboard capability added
        }
        if capability == Capability::Pointer && self.seat_state.get_pointer(qh, &seat).is_err() {
            // Pointer capability added
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        _capability: Capability,
    ) {
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}
}

impl KeyboardHandler for PixelSnap {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        if event.keysym == Keysym::Escape {
            self.exit = true;
        }
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _modifiers: Modifiers,
        _layout: u32,
    ) {
    }
}

impl PointerHandler for PixelSnap {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match event.kind {
                PointerEventKind::Motion { .. } => {
                    self.pointer_pos = Point {
                        x: event.position.0,
                        y: event.position.1,
                    };

                    if self.dragging {
                        self.update_measurement(self.pointer_pos);
                        self.request_redraw();
                        self.draw(qh);
                    }
                }
                PointerEventKind::Press { button, .. } => {
                    if button == 272 {
                        // Left mouse button
                        if self.measure.is_some() && !self.dragging {
                            // Second click exits
                            self.exit = true;
                        } else {
                            self.dragging = true;
                            self.measure = Some(MeasureState {
                                start: self.pointer_pos,
                                end: self.pointer_pos,
                                direction: MeasureDirection::Horizontal,
                            });
                        }
                    }
                }
                PointerEventKind::Release { button, .. } => {
                    if button == 272 {
                        // Left mouse button
                        self.dragging = false;
                    }
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
    let (globals, mut event_queue) = registry_queue_init(&conn).expect("Failed to init registry");
    let qh = event_queue.handle();

    let compositor_state =
        CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("layer shell not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");
    let seat_state = SeatState::new(&globals, &qh);
    let output_state = OutputState::new(&globals, &qh);
    let registry_state = RegistryState::new(&globals);

    let mut pixelsnap = PixelSnap::new(
        registry_state,
        seat_state,
        output_state,
        compositor_state,
        shm,
        layer_shell,
    );

    // Create layer surface
    let surface = pixelsnap.compositor_state.create_surface(&qh);
    let layer_surface = pixelsnap.layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Overlay,
        Some("pixelsnap"),
        None,
    );

    // Configure the layer surface
    layer_surface.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer_surface.set_exclusive_zone(-1);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    layer_surface.commit();

    pixelsnap.layer_surface = Some(layer_surface);

    // Main event loop
    while !pixelsnap.exit {
        event_queue.blocking_dispatch(&mut pixelsnap).unwrap();
    }
}
