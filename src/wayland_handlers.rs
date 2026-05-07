use crate::capture::Screenshot;
use crate::edge_detection::{snap_edge_x, snap_edge_y};
use crate::render::{FrameOverlay, compose_frame};
use std::process::Command;
use std::time::Instant;

/// EMA-smoothed FPS counter. Active only when `HYPRULER_DEBUG=1` in a debug build.
struct FrameClock {
    last: Option<Instant>,
    smoothed_fps: f64,
}

impl FrameClock {
    fn new() -> Self {
        Self {
            last: None,
            smoothed_fps: 0.0,
        }
    }

    /// Record a frame. Returns `(dt_ms, instantaneous_fps)` if a previous tick exists.
    fn tick(&mut self) -> Option<(f64, f64)> {
        let now = Instant::now();
        let result = self.last.map(|prev| {
            let dt_ms = now.duration_since(prev).as_secs_f64() * 1000.0;
            let inst_fps = if dt_ms > 0.0 { 1000.0 / dt_ms } else { 0.0 };
            let alpha = 0.1;
            self.smoothed_fps = if self.smoothed_fps == 0.0 {
                inst_fps
            } else {
                alpha * inst_fps + (1.0 - alpha) * self.smoothed_fps
            };
            (dt_ms, inst_fps)
        });
        self.last = Some(now);
        result
    }

    fn fps(&self) -> f64 {
        self.smoothed_fps
    }
}

fn debug_clock_if_enabled() -> Option<FrameClock> {
    if cfg!(debug_assertions) && std::env::var("HYPRULER_DEBUG").is_ok() {
        Some(FrameClock::new())
    } else {
        None
    }
}

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{
            PointerEvent, PointerEventKind, PointerHandler, cursor_shape::CursorShapeManager,
        },
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use tiny_skia::Pixmap;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    self, WpCursorShapeDeviceV1,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

fn find_system_font() -> Option<Vec<u8>> {
    let output = Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif"])
        .output()
        .ok()?;
    let path = String::from_utf8(output.stdout).ok()?;
    std::fs::read(path.trim()).ok()
}

pub struct WaylandApp {
    // Wayland protocol state
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
    scale: f64,
    target_output_name: Option<String>,

    // Fractional scaling support
    fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    fractional_scale: Option<WpFractionalScaleV1>,
    viewporter: Option<WpViewporter>,
    viewport: Option<WpViewport>,

    // Cursor
    cursor_shape_manager: Option<CursorShapeManager>,
    cursor_shape_device: Option<WpCursorShapeDeviceV1>,

    // Core app state
    pointer_x: f64,
    pointer_y: f64,
    font: Option<fontdue::Font>,
    needs_redraw: bool,
    cached_pixmap: Option<Pixmap>,
    screenshot: Screenshot,

    // Drag-to-measure state
    drag_start: Option<(f64, f64)>,
    drag_rect: Option<(u32, u32, u32, u32)>,
    is_dragging: bool,

    // Debug FPS overlay (None unless HYPRULER_DEBUG=1 in a debug build)
    debug_clock: Option<FrameClock>,

    // Control
    exit: bool,
}

fn normalize_rect(x1: u32, y1: u32, x2: u32, y2: u32) -> (u32, u32, u32, u32) {
    (x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2))
}

fn to_physical(logical: f64, scale: f64) -> u32 {
    (logical * scale) as u32
}

impl WaylandApp {
    pub fn new(
        conn: &Connection,
        screenshot: Screenshot,
        target_output_name: Option<String>,
    ) -> (Self, EventQueue<Self>) {
        let (globals, event_queue) = registry_queue_init(conn).expect("Failed to init registry");
        let qh = event_queue.handle();

        let compositor_state =
            CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
        let layer_shell = LayerShell::bind(&globals, &qh).expect("layer shell not available");
        let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");
        let seat_state = SeatState::new(&globals, &qh);
        let output_state = OutputState::new(&globals, &qh);
        let registry_state = RegistryState::new(&globals);
        let cursor_shape_manager = CursorShapeManager::bind(&globals, &qh).ok();

        let fractional_scale_manager: Option<WpFractionalScaleManagerV1> =
            globals.bind(&qh, 1..=1, ()).ok();
        let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();

        let font = find_system_font().and_then(|data| {
            fontdue::Font::from_bytes(data, fontdue::FontSettings::default()).ok()
        });

        let app = Self {
            registry_state,
            seat_state,
            output_state,
            compositor_state,
            shm,
            layer_shell,
            layer_surface: None,
            pool: None,
            width: 0,
            height: 0,
            scale: 1.0,
            target_output_name,
            fractional_scale_manager,
            fractional_scale: None,
            viewporter,
            viewport: None,
            cursor_shape_manager,
            cursor_shape_device: None,
            pointer_x: 0.0,
            pointer_y: 0.0,
            font,
            needs_redraw: true,
            cached_pixmap: None,
            screenshot,
            drag_start: None,
            drag_rect: None,
            is_dragging: false,
            debug_clock: debug_clock_if_enabled(),
            exit: false,
        };

        (app, event_queue)
    }

    pub fn create_surface(&mut self, qh: &QueueHandle<Self>) {
        // Find the target output by name using OutputState
        let target_output = self.target_output_name.as_ref().and_then(|name| {
            self.output_state.outputs().find(|o| {
                self.output_state
                    .info(o)
                    .map(|i| i.name.as_deref() == Some(name))
                    .unwrap_or(false)
            })
        });

        let surface = self.compositor_state.create_surface(qh);

        // Set up fractional scaling if available
        if let Some(ref manager) = self.fractional_scale_manager {
            self.fractional_scale = Some(manager.get_fractional_scale(&surface, qh, ()));
        }

        // Set up viewport if available
        if let Some(ref viewporter) = self.viewporter {
            self.viewport = Some(viewporter.get_viewport(&surface, qh, ()));
        }

        let layer_surface = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("hypruler"),
            target_output.as_ref(),
        );

        layer_surface.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer_surface.commit();

        self.layer_surface = Some(layer_surface);
    }

    pub fn should_exit(&self) -> bool {
        self.exit
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

        // Derive scale from screenshot vs surface dimensions if fractional scale not set
        if self.scale == 1.0 && self.width > 0 {
            self.scale = phys_width as f64 / self.width as f64;
        }

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

        let debug_fps = self.debug_clock.as_mut().map(|clock| {
            if let Some((dt_ms, inst_fps)) = clock.tick() {
                eprintln!("[hypruler-debug] dt={dt_ms:.2}ms fps={inst_fps:.1}");
            }
            clock.fps()
        });

        compose_frame(
            canvas,
            &mut self.cached_pixmap,
            &self.screenshot,
            FrameOverlay {
                pointer_x: self.pointer_x,
                pointer_y: self.pointer_y,
                scale: self.scale,
                drag_start: self.drag_start,
                drag_rect: self.drag_rect,
                is_dragging: self.is_dragging,
                debug_fps,
            },
            self.font.as_ref(),
        )
        .expect("Failed to compose frame");

        let layer_surface = self.layer_surface.as_ref().unwrap();
        let surface = layer_surface.wl_surface();

        // Use viewport for fractional scaling, fall back to buffer_scale for integer
        if let Some(ref viewport) = self.viewport {
            viewport.set_destination(self.width as i32, self.height as i32);
        } else {
            surface.set_buffer_scale(self.scale.round() as i32);
        }

        buffer.attach_to(surface).expect("Failed to attach buffer");
        surface.damage_buffer(0, 0, phys_width as i32, phys_height as i32);
        surface.commit();
    }
}

// --- Wayland Handler Implementations ---

impl CompositorHandler for WaylandApp {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        // Only use integer scale if fractional scaling is not available
        if self.fractional_scale.is_none() && self.scale != new_factor as f64 {
            self.scale = new_factor as f64;
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

impl OutputHandler for WaylandApp {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for WaylandApp {
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

impl SeatHandler for WaylandApp {
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
        if capability == Capability::Pointer
            && let Ok(pointer) = self.seat_state.get_pointer(qh, &seat)
            && let Some(ref manager) = self.cursor_shape_manager
        {
            self.cursor_shape_device = Some(manager.get_shape_device(&pointer, qh));
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

impl KeyboardHandler for WaylandApp {
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
        _: RawModifiers,
        _: u32,
    ) {
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
}

impl PointerHandler for WaylandApp {
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
                    // Request frame callback - don't draw directly
                    if let Some(ref layer_surface) = self.layer_surface {
                        layer_surface
                            .wl_surface()
                            .frame(qh, layer_surface.wl_surface().clone());
                        layer_surface.wl_surface().commit();
                    }
                }
                PointerEventKind::Press { button: 272, .. } => {
                    // Start drag
                    self.drag_start = Some((self.pointer_x, self.pointer_y));
                    self.is_dragging = true;
                    self.drag_rect = None;
                    self.needs_redraw = true;
                    if let Some(ref layer_surface) = self.layer_surface {
                        layer_surface
                            .wl_surface()
                            .frame(qh, layer_surface.wl_surface().clone());
                        layer_surface.wl_surface().commit();
                    }
                }
                PointerEventKind::Release { button: 272, .. } => {
                    // End drag - finalize rectangle only if it has size
                    if let Some((start_x, start_y)) = self.drag_start {
                        let (left, top, right, bottom) = normalize_rect(
                            to_physical(start_x, self.scale),
                            to_physical(start_y, self.scale),
                            to_physical(self.pointer_x, self.scale),
                            to_physical(self.pointer_y, self.scale),
                        );
                        if right > left && bottom > top {
                            // Snap each edge inward to nearby content
                            let snapped_left = snap_edge_x(&self.screenshot, left, top, bottom, 1);
                            let snapped_right =
                                snap_edge_x(&self.screenshot, right, top, bottom, -1);
                            let snapped_top = snap_edge_y(&self.screenshot, left, right, top, 1);
                            let snapped_bottom =
                                snap_edge_y(&self.screenshot, left, right, bottom, -1);

                            self.drag_rect = Some(normalize_rect(
                                snapped_left,
                                snapped_top,
                                snapped_right,
                                snapped_bottom,
                            ));
                        } else {
                            // Click without drag - clear rectangle
                            self.drag_rect = None;
                        }
                    }
                    self.is_dragging = false;
                    self.needs_redraw = true;
                    if let Some(ref layer_surface) = self.layer_surface {
                        layer_surface
                            .wl_surface()
                            .frame(qh, layer_surface.wl_surface().clone());
                        layer_surface.wl_surface().commit();
                    }
                }
                _ => {}
            }
        }
    }
}

impl ShmHandler for WaylandApp {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for WaylandApp {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(WaylandApp);
delegate_output!(WaylandApp);
delegate_shm!(WaylandApp);
delegate_seat!(WaylandApp);
delegate_keyboard!(WaylandApp);
delegate_pointer!(WaylandApp);
delegate_layer!(WaylandApp);
delegate_registry!(WaylandApp);

// Fractional scaling protocol handlers
impl Dispatch<WpFractionalScaleManagerV1, ()> for WaylandApp {
    fn event(
        _state: &mut Self,
        _proxy: &WpFractionalScaleManagerV1,
        _event: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for WaylandApp {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let new_scale = scale as f64 / 120.0;
            if (state.scale - new_scale).abs() > 0.001 {
                state.scale = new_scale;
                state.cached_pixmap = None;
                state.needs_redraw = true;
            }
        }
    }
}

// Viewporter protocol handlers
impl Dispatch<WpViewporter, ()> for WaylandApp {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        _event: <WpViewporter as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for WaylandApp {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        _event: <WpViewport as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}
