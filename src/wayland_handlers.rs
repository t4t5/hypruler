use crate::capture::{MonitorInfo, MultiMonitorCapture};
use crate::edge_detection::{find_edges, snap_edge_x_ignoring_rect, snap_edge_y_ignoring_rect};
use crate::fps::{FrameClock, debug_clock_if_enabled};
use crate::ui::{draw_crosshair, draw_label, draw_measurements, draw_rectangle_measurement};
use tiny_skia::Color;
use std::process::Command;

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

const BTN_LEFT: u32 = 272;
const CURSOR_SNAP_IGNORE_RADIUS: u32 = 48;

fn find_system_font() -> Option<Vec<u8>> {
    let output = Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif"])
        .output()
        .ok()?;
    let path = String::from_utf8(output.stdout).ok()?;
    std::fs::read(path.trim()).ok()
}

fn get_hyprland_cursor_position() -> Option<(f64, f64)> {
    let output = Command::new("hyprctl").arg("cursorpos").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let (x, y) = stdout.trim().split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// Borrowed references to the shared Wayland globals needed to construct per-monitor surfaces.
struct WaylandContext<'a> {
    compositor_state: &'a CompositorState,
    layer_shell: &'a LayerShell,
    shm: &'a Shm,
    qh: &'a QueueHandle<WaylandApp>,
    fractional_scale_manager: &'a Option<WpFractionalScaleManagerV1>,
    viewporter: &'a Option<WpViewporter>,
}

// Per-monitor surface state
struct MonitorSurface {
    layer_surface: LayerSurface,
    pool: SlotPool,
    width: u32,
    height: u32,
    scale: f64,
    viewport: Option<WpViewport>,
    fractional_scale: Option<WpFractionalScaleV1>,
    cached_pixmap: Option<Pixmap>,
    needs_redraw: bool,
    // Global coordinates of this monitor
    global_x: i32,
    global_y: i32,
    // Physical size of the screenshot
    phys_width: u32,
    phys_height: u32,
}

impl MonitorSurface {
    fn new(
        ctx: &WaylandContext,
        output: &wl_output::WlOutput,
        monitor_info: &MonitorInfo,
        idx: usize,
    ) -> Self {
        let surface = ctx.compositor_state.create_surface(ctx.qh);

        // Set up fractional scaling if available (pass index as data)
        let fractional_scale = ctx
            .fractional_scale_manager
            .as_ref()
            .map(|m| m.get_fractional_scale(&surface, ctx.qh, idx));

        // Set up viewport if available
        let viewport = ctx
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(&surface, ctx.qh, ()));

        let layer_surface = ctx.layer_shell.create_layer_surface(
            ctx.qh,
            surface,
            Layer::Overlay,
            Some("hypruler"),
            Some(output),
        );

        layer_surface.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer_surface.commit();

        // Get physical dimensions from screenshot
        let phys_width = monitor_info
            .screenshot
            .as_ref()
            .map(|s| s.width)
            .unwrap_or(monitor_info.width);
        let phys_height = monitor_info
            .screenshot
            .as_ref()
            .map(|s| s.height)
            .unwrap_or(monitor_info.height);

        // Initial pool size (will be resized on configure)
        let pool_size = (phys_width * phys_height * 4) as usize;
        let pool = SlotPool::new(pool_size, ctx.shm).expect("Failed to create pool");

        Self {
            layer_surface,
            pool,
            width: 0,
            height: 0,
            scale: monitor_info.scale,
            viewport,
            fractional_scale,
            cached_pixmap: None,
            needs_redraw: true,
            global_x: monitor_info.x,
            global_y: monitor_info.y,
            phys_width,
            phys_height,
        }
    }

    /// Check if global coordinates are within this monitor
    fn contains(&self, x: f64, y: f64) -> bool {
        let x = x as i32;
        let y = y as i32;
        x >= self.global_x
            && x < self.global_x + self.width as i32
            && y >= self.global_y
            && y < self.global_y + self.height as i32
    }

    fn effective_scale(&self) -> f64 {
        if self.width > 0 {
            self.phys_width as f64 / self.width as f64
        } else if self.scale > 0.0 {
            self.scale
        } else {
            1.0
        }
    }

    /// Convert global logical coordinates to local physical coordinates.
    fn to_local_physical(&self, global_x: f64, global_y: f64) -> (u32, u32) {
        let scale = self.effective_scale();
        let local_x =
            ((global_x - self.global_x as f64) * scale).clamp(0.0, self.phys_width as f64);
        let local_y =
            ((global_y - self.global_y as f64) * scale).clamp(0.0, self.phys_height as f64);
        (local_x as u32, local_y as u32)
    }

    fn intersects_global_rect(&self, x1: f64, y1: f64, x2: f64, y2: f64) -> bool {
        let left = x1.min(x2);
        let top = y1.min(y2);
        let right = x1.max(x2);
        let bottom = y1.max(y2);
        let monitor_left = self.global_x as f64;
        let monitor_top = self.global_y as f64;
        let monitor_right = (self.global_x + self.width as i32) as f64;
        let monitor_bottom = (self.global_y + self.height as i32) as f64;

        right >= monitor_left
            && left <= monitor_right
            && bottom >= monitor_top
            && top <= monitor_bottom
    }

    /// Clip a global logical rectangle to this monitor and convert it to local physical coordinates.
    fn global_rect_to_local_physical(
        &self,
        gx1: i32,
        gy1: i32,
        gx2: i32,
        gy2: i32,
    ) -> Option<(u32, u32, u32, u32)> {
        let left = gx1.min(gx2).max(self.global_x);
        let top = gy1.min(gy2).max(self.global_y);
        let right = gx1.max(gx2).min(self.global_x + self.width as i32);
        let bottom = gy1.max(gy2).min(self.global_y + self.height as i32);

        if right <= left || bottom <= top {
            return None;
        }

        let (local_left, local_top) = self.to_local_physical(left as f64, top as f64);
        let (local_right, local_bottom) = self.to_local_physical(right as f64, bottom as f64);
        Some((local_left, local_top, local_right, local_bottom))
    }
}

pub struct WaylandApp {
    // Wayland protocol state
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    layer_shell: LayerShell,

    // Multi-monitor surfaces
    monitor_surfaces: Vec<MonitorSurface>,
    monitors: Vec<MonitorInfo>,

    // Fractional scaling support
    fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,

    // Cursor
    cursor_shape_manager: Option<CursorShapeManager>,
    cursor_shape_device: Option<WpCursorShapeDeviceV1>,

    // Core app state
    pointer_x: f64,
    pointer_y: f64,
    pointer_valid: bool,
    font: Option<fontdue::Font>,

    // Drag-to-measure state (in global coordinates)
    drag_start: Option<(f64, f64)>,
    drag_rect: Option<(i32, i32, i32, i32)>, // Global coordinates (x1, y1, x2, y2)
    is_dragging: bool,

    // Debug FPS overlay (None unless HYPRULER_DEBUG=1 in a debug build)
    debug_clock: Option<FrameClock>,

    // Control
    exit: bool,
}

fn normalize_rect(x1: u32, y1: u32, x2: u32, y2: u32) -> (u32, u32, u32, u32) {
    (x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2))
}

fn cursor_ignore_rect(x: u32, y: u32, width: u32, height: u32) -> (u32, u32, u32, u32) {
    (
        x.saturating_sub(CURSOR_SNAP_IGNORE_RADIUS),
        y.saturating_sub(CURSOR_SNAP_IGNORE_RADIUS),
        (x + CURSOR_SNAP_IGNORE_RADIUS).min(width.saturating_sub(1)),
        (y + CURSOR_SNAP_IGNORE_RADIUS).min(height.saturating_sub(1)),
    )
}

impl WaylandApp {
    pub fn new(conn: &Connection, multi_capture: MultiMonitorCapture) -> (Self, EventQueue<Self>) {
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

        let monitors = multi_capture.monitors;

        let app = Self {
            registry_state,
            seat_state,
            output_state,
            compositor_state,
            shm,
            layer_shell,
            monitor_surfaces: Vec::new(),
            monitors,
            fractional_scale_manager,
            viewporter,
            cursor_shape_manager,
            cursor_shape_device: None,
            pointer_x: 0.0,
            pointer_y: 0.0,
            pointer_valid: false,
            font,
            drag_start: None,
            drag_rect: None,
            is_dragging: false,
            debug_clock: debug_clock_if_enabled(),
            exit: false,
        };

        (app, event_queue)
    }

    pub fn create_surfaces(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        let monitor_indices: Vec<_> = self
            .monitors
            .iter()
            .enumerate()
            .filter(|(_, monitor)| monitor.screenshot.is_some())
            .map(|(idx, _)| idx)
            .collect();

        let ctx = WaylandContext {
            compositor_state: &self.compositor_state,
            layer_shell: &self.layer_shell,
            shm: &self.shm,
            qh,
            fractional_scale_manager: &self.fractional_scale_manager,
            viewporter: &self.viewporter,
        };

        // Create a surface for each monitor that has a screenshot.
        for monitor_idx in monitor_indices {
            let monitor = &self.monitors[monitor_idx];

            // Find the wl_output for this monitor.
            let output = self.output_state.outputs().find(|o| {
                self.output_state
                    .info(o)
                    .map(|i| i.name.as_deref() == Some(&monitor.name))
                    .unwrap_or(false)
            });

            if let Some(ref output) = output {
                let surface_idx = self.monitor_surfaces.len();
                let surface = MonitorSurface::new(&ctx, output, monitor, surface_idx);
                self.monitor_surfaces.push(surface);
            }
        }

        if self.monitor_surfaces.is_empty() {
            return Err("No monitor surfaces created".to_string());
        }

        Ok(())
    }

    pub fn should_exit(&self) -> bool {
        self.exit
    }

    fn update_pointer_from_event(&mut self, event: &PointerEvent) -> Option<usize> {
        let monitor_idx = self
            .monitor_surfaces
            .iter()
            .position(|m| m.layer_surface.wl_surface() == &event.surface)?;
        let monitor = &self.monitor_surfaces[monitor_idx];
        self.pointer_x = monitor.global_x as f64 + event.position.0;
        self.pointer_y = monitor.global_y as f64 + event.position.1;
        self.pointer_valid = true;
        Some(monitor_idx)
    }

    fn update_pointer_from_hyprland(&mut self) -> bool {
        let Some((x, y)) = get_hyprland_cursor_position() else {
            return false;
        };
        self.pointer_x = x;
        self.pointer_y = y;
        self.pointer_valid = true;
        true
    }

    fn draw_monitor(&mut self, idx: usize, _qh: &QueueHandle<Self>) {
        // Check if redraw is needed
        {
            let monitor = &self.monitor_surfaces[idx];
            if monitor.width == 0 || monitor.height == 0 || !monitor.needs_redraw {
                return;
            }
        }

        // Borrow the screenshot instead of cloning it on every redraw.
        let monitor_info_idx = self.monitors.iter().position(|m| {
            m.x == self.monitor_surfaces[idx].global_x && m.y == self.monitor_surfaces[idx].global_y
        });
        let Some(screenshot) = monitor_info_idx.and_then(|i| self.monitors[i].screenshot.as_ref())
        else {
            return;
        };

        // Collect data we need from the monitor before mutable borrow
        let (
            phys_width,
            phys_height,
            scale,
            contains_cursor,
            cursor_phys,
            active_drag_rect,
            completed_drag_rect,
        ) = {
            let monitor = &self.monitor_surfaces[idx];
            let phys_width = monitor.phys_width;
            let phys_height = monitor.phys_height;
            let scale = monitor.effective_scale();

            let contains_cursor =
                self.pointer_valid && monitor.contains(self.pointer_x, self.pointer_y);
            let cursor_phys = monitor.to_local_physical(self.pointer_x, self.pointer_y);

            let active_drag_rect = if self.is_dragging {
                self.drag_start.and_then(|(start_x, start_y)| {
                    monitor.global_rect_to_local_physical(
                        start_x as i32,
                        start_y as i32,
                        self.pointer_x as i32,
                        self.pointer_y as i32,
                    )
                })
            } else {
                None
            };

            let completed_drag_rect = if self.is_dragging {
                None
            } else {
                self.drag_rect.and_then(|(gx1, gy1, gx2, gy2)| {
                    monitor.global_rect_to_local_physical(gx1, gy1, gx2, gy2)
                })
            };

            (
                phys_width,
                phys_height,
                scale,
                contains_cursor,
                cursor_phys,
                active_drag_rect,
                completed_drag_rect,
            )
        };

        // Mark as redrawn
        self.monitor_surfaces[idx].needs_redraw = false;

        // Now do the actual drawing with mutable borrow
        let monitor = &mut self.monitor_surfaces[idx];
        let stride = phys_width as i32 * 4;
        let size = (stride * phys_height as i32) as usize;

        if monitor.pool.len() < size {
            monitor.pool.resize(size).expect("Failed to resize pool");
        }

        let (buffer, canvas) = monitor
            .pool
            .create_buffer(
                phys_width as i32,
                phys_height as i32,
                stride,
                wl_shm::Format::Argb8888,
            )
            .expect("Failed to create buffer");

        // Copy pre-converted BGRA background
        let bgra = screenshot.bgra_data();
        let bgra_size = bgra.len().min(size);
        canvas[..bgra_size].copy_from_slice(&bgra[..bgra_size]);

        // Draw overlay
        let needs_new_pixmap = monitor
            .cached_pixmap
            .as_ref()
            .map(|p| p.width() != phys_width || p.height() != phys_height)
            .unwrap_or(true);

        if needs_new_pixmap {
            monitor.cached_pixmap = Pixmap::new(phys_width, phys_height);
        }

        let pixmap = monitor.cached_pixmap.as_mut().unwrap();
        pixmap.fill(tiny_skia::Color::TRANSPARENT);

        if self.is_dragging {
            // Draw the active rectangle clipped to this monitor.
            if let Some((left, top, right, bottom)) = active_drag_rect {
                draw_rectangle_measurement(
                    pixmap,
                    left,
                    top,
                    right,
                    bottom,
                    self.font.as_ref(),
                    scale,
                );
            }
        } else {
            // Draw the completed rectangle clipped to this monitor.
            if let Some((left, top, right, bottom)) = completed_drag_rect {
                draw_rectangle_measurement(
                    pixmap,
                    left,
                    top,
                    right,
                    bottom,
                    self.font.as_ref(),
                    scale,
                );
            }

            // Always show edge detection and crosshair when not dragging (only on active monitor)
            let (cursor_phys_x, cursor_phys_y) = cursor_phys;
            if contains_cursor
                && cursor_phys_x < screenshot.width
                && cursor_phys_y < screenshot.height
            {
                let edges = find_edges(screenshot, cursor_phys_x, cursor_phys_y);
                draw_measurements(
                    pixmap,
                    &edges,
                    cursor_phys_x,
                    cursor_phys_y,
                    self.font.as_ref(),
                    scale,
                );
                draw_crosshair(pixmap, cursor_phys_x as f32, cursor_phys_y as f32);
            }
        }

        if let Some(clock) = self.debug_clock.as_mut() {
            if let Some((dt_ms, inst_fps)) = clock.tick() {
                eprintln!("[hypruler-debug] dt={dt_ms:.2}ms fps={inst_fps:.1}");
            }
            let fps = clock.fps();
            let color = if fps >= 55.0 {
                Color::from_rgba8(46, 204, 113, 255) // green
            } else if fps >= 30.0 {
                Color::from_rgba8(243, 156, 18, 255) // yellow/orange
            } else {
                Color::from_rgba8(231, 76, 60, 255) // red
            };
            draw_label(
                pixmap,
                &format!("FPS: {:.1}", fps),
                80.0,
                30.0,
                self.font.as_ref(),
                color,
            );
        }

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

        let surface = monitor.layer_surface.wl_surface();

        // Use viewport for fractional scaling, fall back to buffer_scale for integer
        if let Some(ref viewport) = monitor.viewport {
            viewport.set_destination(monitor.width as i32, monitor.height as i32);
        } else {
            surface.set_buffer_scale(scale.round() as i32);
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
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        // Find the monitor surface that owns this surface
        for monitor in &mut self.monitor_surfaces {
            if monitor.layer_surface.wl_surface() == surface {
                // Only use integer scale if fractional scaling is not available
                if monitor.fractional_scale.is_none() && monitor.scale != new_factor as f64 {
                    monitor.scale = new_factor as f64;
                    monitor.cached_pixmap = None;
                    monitor.needs_redraw = true;
                }
                break;
            }
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

    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        // Find which monitor this surface belongs to and redraw
        if let Some(idx) = self
            .monitor_surfaces
            .iter()
            .position(|m| m.layer_surface.wl_surface() == surface)
        {
            self.draw_monitor(idx, qh);
        }
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
        layer_surface: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        // Find the monitor surface that owns this layer_surface
        if let Some(monitor) = self
            .monitor_surfaces
            .iter_mut()
            .find(|m| &m.layer_surface == layer_surface)
        {
            monitor.width = configure.new_size.0;
            monitor.height = configure.new_size.1;
            monitor.needs_redraw = true;

            // Resize pool if needed
            let scale = monitor.effective_scale();
            let phys_width = (monitor.width as f64 * scale) as u32;
            let phys_height = (monitor.height as f64 * scale) as u32;
            let pool_size = (phys_width * phys_height * 4) as usize;

            if monitor.pool.len() < pool_size {
                let _ = monitor.pool.resize(pool_size);
            }

            // Find index and draw
            if let Some(idx) = self
                .monitor_surfaces
                .iter()
                .position(|m| &m.layer_surface == layer_surface)
            {
                self.draw_monitor(idx, qh);
            }
        }
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
                PointerEventKind::Enter { serial, .. } => {
                    self.update_pointer_from_event(event);
                    if let Some(ref device) = self.cursor_shape_device {
                        device.set_shape(serial, wp_cursor_shape_device_v1::Shape::Crosshair);
                    }
                }
                PointerEventKind::Motion { .. } => {
                    let had_old_pointer = self.pointer_valid;
                    let old_x = self.pointer_x;
                    let old_y = self.pointer_y;

                    // Convert local surface coordinates to global desktop coordinates.
                    if let Some(monitor_idx) = self.update_pointer_from_event(event) {
                        self.monitor_surfaces[monitor_idx].needs_redraw = true;
                    }

                    // When dragging, redraw only monitors touched by the previous or current drag rect.
                    // Otherwise only redraw the monitor where the cursor was and where it is now.
                    let drag_start = self.drag_start;
                    let monitors_to_redraw: Vec<_> = self
                        .monitor_surfaces
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| {
                            if self.is_dragging {
                                drag_start
                                    .map(|(start_x, start_y)| {
                                        m.intersects_global_rect(start_x, start_y, old_x, old_y)
                                            || m.intersects_global_rect(
                                                start_x,
                                                start_y,
                                                self.pointer_x,
                                                self.pointer_y,
                                            )
                                    })
                                    .unwrap_or(false)
                            } else {
                                (had_old_pointer && m.contains(old_x, old_y))
                                    || m.contains(self.pointer_x, self.pointer_y)
                            }
                        })
                        .map(|(i, _)| i)
                        .collect();

                    for idx in monitors_to_redraw {
                        self.monitor_surfaces[idx].needs_redraw = true;
                        let surface = self.monitor_surfaces[idx]
                            .layer_surface
                            .wl_surface()
                            .clone();
                        self.monitor_surfaces[idx]
                            .layer_surface
                            .wl_surface()
                            .frame(qh, surface);
                        self.monitor_surfaces[idx]
                            .layer_surface
                            .wl_surface()
                            .commit();
                    }
                }
                PointerEventKind::Press {
                    button: BTN_LEFT, ..
                } => {
                    self.update_pointer_from_event(event);
                    self.update_pointer_from_hyprland();

                    // Start drag
                    let had_drag_rect = self.drag_rect.is_some();
                    self.drag_start = Some((self.pointer_x, self.pointer_y));
                    self.is_dragging = true;
                    self.drag_rect = None;

                    // Redraw all monitors only when clearing an existing rectangle; otherwise redraw the current monitor.
                    for monitor in &mut self.monitor_surfaces {
                        if had_drag_rect || monitor.contains(self.pointer_x, self.pointer_y) {
                            monitor.needs_redraw = true;
                            let surface = monitor.layer_surface.wl_surface().clone();
                            monitor.layer_surface.wl_surface().frame(qh, surface);
                            monitor.layer_surface.wl_surface().commit();
                        }
                    }
                }
                PointerEventKind::Release {
                    button: BTN_LEFT, ..
                } => {
                    self.update_pointer_from_event(event);
                    self.update_pointer_from_hyprland();

                    // End drag - finalize rectangle only if it has size
                    if let Some((start_x, start_y)) = self.drag_start {
                        let gx1 = start_x as i32;
                        let gy1 = start_y as i32;
                        let gx2 = self.pointer_x as i32;
                        let gy2 = self.pointer_y as i32;
                        let normalized_global =
                            (gx1.min(gx2), gy1.min(gy2), gx1.max(gx2), gy1.max(gy2));

                        if (gx2 - gx1).abs() > 1 && (gy2 - gy1).abs() > 1 {
                            let snapped_rect = self
                                .monitor_surfaces
                                .iter()
                                .position(|m| {
                                    m.contains(start_x, start_y)
                                        && m.contains(self.pointer_x, self.pointer_y)
                                })
                                .and_then(|monitor_idx| {
                                    let monitor = &self.monitor_surfaces[monitor_idx];
                                    let monitor_info = self.monitors.iter().find(|m| {
                                        m.x == monitor.global_x && m.y == monitor.global_y
                                    })?;
                                    let screenshot = monitor_info.screenshot.as_ref()?;
                                    let scale = monitor.effective_scale();

                                    let (local_x1, local_y1) =
                                        monitor.to_local_physical(start_x, start_y);
                                    let (local_x2, local_y2) =
                                        monitor.to_local_physical(self.pointer_x, self.pointer_y);
                                    let (left, top, right, bottom) =
                                        normalize_rect(local_x1, local_y1, local_x2, local_y2);
                                    let ignored_cursor_rect = Some(cursor_ignore_rect(
                                        local_x1,
                                        local_y1,
                                        screenshot.width,
                                        screenshot.height,
                                    ));

                                    // Snap each edge inward to nearby content, ignoring the captured cursor area.
                                    let snapped_left = snap_edge_x_ignoring_rect(
                                        screenshot,
                                        left,
                                        top,
                                        bottom,
                                        1,
                                        ignored_cursor_rect,
                                    );
                                    let snapped_right = snap_edge_x_ignoring_rect(
                                        screenshot,
                                        right,
                                        top,
                                        bottom,
                                        -1,
                                        ignored_cursor_rect,
                                    );
                                    let snapped_top = snap_edge_y_ignoring_rect(
                                        screenshot,
                                        left,
                                        right,
                                        top,
                                        1,
                                        ignored_cursor_rect,
                                    );
                                    let snapped_bottom = snap_edge_y_ignoring_rect(
                                        screenshot,
                                        left,
                                        right,
                                        bottom,
                                        -1,
                                        ignored_cursor_rect,
                                    );

                                    Some((
                                        monitor.global_x + (snapped_left as f64 / scale) as i32,
                                        monitor.global_y + (snapped_top as f64 / scale) as i32,
                                        monitor.global_x + (snapped_right as f64 / scale) as i32,
                                        monitor.global_y + (snapped_bottom as f64 / scale) as i32,
                                    ))
                                });

                            self.drag_rect = Some(snapped_rect.unwrap_or(normalized_global));
                        } else {
                            // Click without drag - clear rectangle
                            self.drag_rect = None;
                        }
                    }
                    let released_x = self.pointer_x;
                    let released_y = self.pointer_y;
                    let release_drag_start = self.drag_start;
                    let final_drag_rect = self.drag_rect;
                    self.drag_start = None;
                    self.is_dragging = false;

                    // Redraw only monitors touched by the active/final rectangle, plus the cursor monitor.
                    let monitors_to_redraw: Vec<_> = self
                        .monitor_surfaces
                        .iter()
                        .enumerate()
                        .filter(|(_, monitor)| {
                            monitor.contains(released_x, released_y)
                                || release_drag_start
                                    .map(|(start_x, start_y)| {
                                        monitor.intersects_global_rect(
                                            start_x, start_y, released_x, released_y,
                                        )
                                    })
                                    .unwrap_or(false)
                                || final_drag_rect
                                    .map(|(x1, y1, x2, y2)| {
                                        monitor.intersects_global_rect(
                                            x1 as f64, y1 as f64, x2 as f64, y2 as f64,
                                        )
                                    })
                                    .unwrap_or(false)
                        })
                        .map(|(idx, _)| idx)
                        .collect();

                    for idx in monitors_to_redraw {
                        self.monitor_surfaces[idx].needs_redraw = true;
                        let surface = self.monitor_surfaces[idx]
                            .layer_surface
                            .wl_surface()
                            .clone();
                        self.monitor_surfaces[idx]
                            .layer_surface
                            .wl_surface()
                            .frame(qh, surface);
                        self.monitor_surfaces[idx]
                            .layer_surface
                            .wl_surface()
                            .commit();
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

// Store index of monitor being configured (used for fractional scale events)
impl Dispatch<WpFractionalScaleV1, usize> for WaylandApp {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        data: &usize,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let idx = *data;
            if idx < state.monitor_surfaces.len() {
                let new_scale = scale as f64 / 120.0;
                let monitor = &mut state.monitor_surfaces[idx];
                if (monitor.scale - new_scale).abs() > 0.001 {
                    monitor.scale = new_scale;
                    monitor.cached_pixmap = None;
                    monitor.needs_redraw = true;
                }
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
