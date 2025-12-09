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
use std::sync::atomic::{AtomicBool, Ordering};
use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform};
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_shm_pool, wl_surface, wl_buffer, wl_registry},
    Connection, Dispatch, QueueHandle, Proxy,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};
use memmap2::MmapMut;
use rustix::fs::{self, SealFlags};

const LINE_WIDTH: f32 = 2.0;
const END_CAP_SIZE: f32 = 8.0;
const EDGE_THRESHOLD: i32 = 25; // Luminance difference threshold for edge detection

fn line_color() -> Color {
    Color::from_rgba8(231, 76, 60, 255) // #E74C3C
}

fn label_bg_color() -> Color {
    Color::from_rgba8(40, 40, 40, 230)
}

// ============================================================================
// Screen Capture Types
// ============================================================================

#[derive(Debug, Clone, Copy)]
struct FrameFormat {
    format: wl_shm::Format,
    width: u32,
    height: u32,
    stride: u32,
}

struct CaptureState {
    format: Option<FrameFormat>,
    done: AtomicBool,
    ready: AtomicBool,
    failed: AtomicBool,
}

impl CaptureState {
    fn new() -> Self {
        Self {
            format: None,
            done: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        }
    }
}

// Dispatch implementations for screen capture
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrScreencopyManagerV1,
        _event: <ZwlrScreencopyManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer { format, width, height, stride } => {
                if let wayland_client::WEnum::Value(fmt) = format {
                    // Prefer common formats
                    if matches!(fmt, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 | wl_shm::Format::Xbgr8888) {
                        state.format = Some(FrameFormat { format: fmt, width, height, stride });
                    } else if state.format.is_none() {
                        state.format = Some(FrameFormat { format: fmt, width, height, stride });
                    }
                }
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => {
                state.done.store(true, Ordering::SeqCst);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                state.ready.store(true, Ordering::SeqCst);
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.failed.store(true, Ordering::SeqCst);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm_pool::WlShmPool,
        _event: <wl_shm_pool::WlShmPool as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<wl_buffer::WlBuffer, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_buffer::WlBuffer,
        _event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<wl_output::WlOutput, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_output::WlOutput,
        _event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {}
}

// ============================================================================
// Screen Capture Functions
// ============================================================================

fn create_shm_fd() -> std::io::Result<OwnedFd> {
    loop {
        match fs::memfd_create(
            CString::new("pixelsnap-capture")?.as_c_str(),
            fs::MemfdFlags::CLOEXEC | fs::MemfdFlags::ALLOW_SEALING,
        ) {
            Ok(fd) => {
                let _ = fs::fcntl_add_seals(&fd, SealFlags::SHRINK | SealFlags::SEAL);
                return Ok(fd);
            }
            Err(rustix::io::Errno::INTR) => continue,
            Err(errno) => return Err(std::io::Error::from(errno)),
        }
    }
}

struct Screenshot {
    bgra_data: Vec<u8>, // Pre-converted to BGRA for fast canvas copy
    width: u32,
    height: u32,
    luminance: Vec<u8>, // Pre-computed luminance for edge detection
}

impl Screenshot {
    fn get_luminance(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.luminance[(y * self.width + x) as usize]
    }
}

fn capture_screen(conn: &Connection) -> Result<Screenshot, String> {
    let (globals, mut event_queue) = registry_queue_init::<CaptureState>(conn)
        .map_err(|e| format!("Failed to init registry: {}", e))?;

    let qh = event_queue.handle();
    let mut state = CaptureState::new();

    // Get the screencopy manager
    let screencopy_manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 3..=3, ())
        .map_err(|_| "wlr-screencopy protocol not available. Is your compositor wlroots-based?")?;

    // Get the first output
    let output: wl_output::WlOutput = globals
        .bind(&qh, 1..=4, ())
        .map_err(|_| "No output available")?;

    // Get shm
    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "wl_shm not available")?;

    // Request a frame capture (without cursor)
    let frame = screencopy_manager.capture_output(0, &output, &qh, ());

    // Wait for buffer format info
    while !state.done.load(Ordering::SeqCst) {
        event_queue.blocking_dispatch(&mut state)
            .map_err(|e| format!("Dispatch error: {}", e))?;
    }

    let format = state.format.ok_or("No suitable buffer format received")?;

    // Create shm buffer
    let fd = create_shm_fd().map_err(|e| format!("Failed to create shm fd: {}", e))?;
    let file = File::from(fd);
    let size = (format.stride * format.height) as u64;
    file.set_len(size).map_err(|e| format!("Failed to set file size: {}", e))?;

    let shm_pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
    let buffer = shm_pool.create_buffer(
        0,
        format.width as i32,
        format.height as i32,
        format.stride as i32,
        format.format,
        &qh,
        (),
    );

    // Copy the frame
    frame.copy(&buffer);

    // Wait for ready or failed
    while !state.ready.load(Ordering::SeqCst) && !state.failed.load(Ordering::SeqCst) {
        event_queue.blocking_dispatch(&mut state)
            .map_err(|e| format!("Dispatch error: {}", e))?;
    }

    if state.failed.load(Ordering::SeqCst) {
        return Err("Screen capture failed".to_string());
    }

    // Memory map and copy data
    let mmap = unsafe { MmapMut::map_mut(&file) }
        .map_err(|e| format!("Failed to mmap: {}", e))?;

    let data = mmap.to_vec();

    // Pre-compute luminance for fast edge detection AND convert to BGRA for fast canvas copy
    let pixel_count = (format.width * format.height) as usize;
    let mut luminance = vec![0u8; pixel_count];
    let mut bgra_data = vec![0u8; pixel_count * 4];

    for y in 0..format.height {
        for x in 0..format.width {
            let src_idx = (y * format.stride + x * 4) as usize;
            let dst_idx = (y * format.width + x) as usize;

            if src_idx + 3 < data.len() {
                // Extract RGB based on source format
                let (r, g, b) = match format.format {
                    wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => {
                        // BGRX in memory (little endian)
                        (data[src_idx + 2], data[src_idx + 1], data[src_idx])
                    }
                    wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 => {
                        // RGBX in memory
                        (data[src_idx], data[src_idx + 1], data[src_idx + 2])
                    }
                    _ => (data[src_idx + 2], data[src_idx + 1], data[src_idx]),
                };

                // Compute luminance
                luminance[dst_idx] = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as u8;

                // Convert to BGRA (Wayland's ARGB8888 format, which is BGRA in memory)
                let bgra_idx = dst_idx * 4;
                bgra_data[bgra_idx] = b;
                bgra_data[bgra_idx + 1] = g;
                bgra_data[bgra_idx + 2] = r;
                bgra_data[bgra_idx + 3] = 255; // Full alpha
            }
        }
    }

    // Clean up
    buffer.destroy();
    shm_pool.destroy();
    frame.destroy();

    Ok(Screenshot {
        bgra_data,
        width: format.width,
        height: format.height,
        luminance,
    })
}

// ============================================================================
// Edge Detection
// ============================================================================

#[derive(Debug, Clone, Copy)]
struct Edges {
    left: u32,
    right: u32,
    up: u32,
    down: u32,
}

fn find_edges(screenshot: &Screenshot, cursor_x: u32, cursor_y: u32) -> Edges {
    let width = screenshot.width;
    let height = screenshot.height;

    // Scan left from cursor
    let left = scan_horizontal(screenshot, cursor_x, cursor_y, -1);
    // Scan right from cursor
    let right = scan_horizontal(screenshot, cursor_x, cursor_y, 1);
    // Scan up from cursor
    let up = scan_vertical(screenshot, cursor_x, cursor_y, -1);
    // Scan down from cursor
    let down = scan_vertical(screenshot, cursor_x, cursor_y, 1);

    Edges {
        left: left.unwrap_or(0),
        right: right.unwrap_or(width.saturating_sub(1)),
        up: up.unwrap_or(0),
        down: down.unwrap_or(height.saturating_sub(1)),
    }
}

fn scan_horizontal(screenshot: &Screenshot, start_x: u32, y: u32, direction: i32) -> Option<u32> {
    let mut x = start_x as i32;
    let mut prev_lum = screenshot.get_luminance(start_x, y) as i32;

    loop {
        x += direction;
        if x < 0 || x >= screenshot.width as i32 {
            return None;
        }

        let lum = screenshot.get_luminance(x as u32, y) as i32;
        let diff = (lum - prev_lum).abs();

        if diff > EDGE_THRESHOLD {
            // Found an edge - return the position just before the edge
            if direction < 0 {
                return Some((x + 1) as u32);
            } else {
                return Some((x - 1) as u32);
            }
        }

        prev_lum = lum;
    }
}

fn scan_vertical(screenshot: &Screenshot, x: u32, start_y: u32, direction: i32) -> Option<u32> {
    let mut y = start_y as i32;
    let mut prev_lum = screenshot.get_luminance(x, start_y) as i32;

    loop {
        y += direction;
        if y < 0 || y >= screenshot.height as i32 {
            return None;
        }

        let lum = screenshot.get_luminance(x, y as u32) as i32;
        let diff = (lum - prev_lum).abs();

        if diff > EDGE_THRESHOLD {
            // Found an edge - return the position just before the edge
            if direction < 0 {
                return Some((y + 1) as u32);
            } else {
                return Some((y - 1) as u32);
            }
        }

        prev_lum = lum;
    }
}

// ============================================================================
// Main Application State
// ============================================================================

#[derive(Debug, Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

struct PixelSnap {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    layer_shell: LayerShell,

    exit: bool,
    width: u32,
    height: u32,
    scale: i32,
    layer_surface: Option<LayerSurface>,
    pool: Option<SlotPool>,

    // Input state
    pointer_pos: Point,

    // Font for rendering
    font: Option<Arc<fontdue::Font>>,

    // Rendering optimization
    needs_redraw: bool,
    cached_pixmap: Option<Pixmap>,

    // Screenshot data for edge detection
    screenshot: Screenshot,
}

impl PixelSnap {
    fn new(
        registry_state: RegistryState,
        seat_state: SeatState,
        output_state: OutputState,
        compositor_state: CompositorState,
        shm: Shm,
        layer_shell: LayerShell,
        screenshot: Screenshot,
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
            font: font.map(Arc::new),
            needs_redraw: true,
            cached_pixmap: None,
            screenshot,
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

        // Screenshot is already at physical resolution from screencopy
        let phys_width = self.screenshot.width;
        let phys_height = self.screenshot.height;
        let scale = self.scale as f32;

        // Get values we need
        let pointer_pos = self.pointer_pos;
        let screenshot = &self.screenshot;
        let font = self.font.clone();

        // The pointer position is in logical coordinates, convert to physical
        let cursor_phys_x = (pointer_pos.x * scale as f64) as u32;
        let cursor_phys_y = (pointer_pos.y * scale as f64) as u32;

        // Copy to wayland buffer
        let pool = self.pool.as_mut().unwrap();
        let stride = phys_width as i32 * 4;
        let size = (stride * phys_height as i32) as usize;

        // Resize pool if needed
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

        // FAST: Copy pre-converted BGRA background directly to canvas
        let bgra_size = screenshot.bgra_data.len().min(size);
        canvas[..bgra_size].copy_from_slice(&screenshot.bgra_data[..bgra_size]);

        // Draw overlay (measurements, crosshair) on transparent pixmap
        if cursor_phys_x < screenshot.width && cursor_phys_y < screenshot.height {
            // Create or reuse overlay pixmap
            let needs_new_pixmap = self.cached_pixmap.as_ref()
                .map(|p| p.width() != phys_width || p.height() != phys_height)
                .unwrap_or(true);

            if needs_new_pixmap {
                self.cached_pixmap = Pixmap::new(phys_width, phys_height);
            }

            let pixmap = self.cached_pixmap.as_mut().unwrap();
            pixmap.fill(tiny_skia::Color::TRANSPARENT);

            let edges = find_edges(screenshot, cursor_phys_x, cursor_phys_y);
            Self::draw_measurements_static(pixmap, &edges, cursor_phys_x, cursor_phys_y, font.as_ref());
            Self::draw_crosshair_static(pixmap, cursor_phys_x as f32, cursor_phys_y as f32);

            // Composite overlay onto canvas (only non-transparent pixels)
            let overlay_data = pixmap.data();
            for (i, chunk) in canvas[..size].chunks_exact_mut(4).enumerate() {
                let src_idx = i * 4;
                let alpha = overlay_data[src_idx + 3];
                if alpha > 0 {
                    // Blend overlay onto background (RGBA -> BGRA with alpha blend)
                    let src_r = overlay_data[src_idx] as u32;
                    let src_g = overlay_data[src_idx + 1] as u32;
                    let src_b = overlay_data[src_idx + 2] as u32;
                    let src_a = alpha as u32;

                    let dst_b = chunk[0] as u32;
                    let dst_g = chunk[1] as u32;
                    let dst_r = chunk[2] as u32;

                    // Simple alpha blend: out = src * alpha + dst * (1 - alpha)
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

    fn draw_measurements_static(pixmap: &mut Pixmap, edges: &Edges, cursor_x: u32, cursor_y: u32, font: Option<&Arc<fontdue::Font>>) {
        let mut paint = Paint::default();
        paint.set_color(line_color());
        paint.anti_alias = true;

        // Fixed line width (we're at physical resolution)
        let stroke = Stroke {
            width: LINE_WIDTH * 2.0, // Slightly thicker for HiDPI
            ..Default::default()
        };

        // All coordinates are in physical pixels
        let left = edges.left as f32;
        let right = edges.right as f32;
        let up = edges.up as f32;
        let down = edges.down as f32;

        let cursor_x = cursor_x as f32;
        let cursor_y = cursor_y as f32;

        // Draw horizontal measurement line (left edge to right edge at cursor y)
        let mut pb = PathBuilder::new();
        pb.move_to(left, cursor_y);
        pb.line_to(right, cursor_y);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }

        // Draw end caps for horizontal line
        Self::draw_end_cap_static(pixmap, &paint, &stroke, left, cursor_y, true);
        Self::draw_end_cap_static(pixmap, &paint, &stroke, right, cursor_y, true);

        // Draw vertical measurement line (top edge to bottom edge at cursor x)
        let mut pb = PathBuilder::new();
        pb.move_to(cursor_x, up);
        pb.line_to(cursor_x, down);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }

        // Draw end caps for vertical line
        Self::draw_end_cap_static(pixmap, &paint, &stroke, cursor_x, up, false);
        Self::draw_end_cap_static(pixmap, &paint, &stroke, cursor_x, down, false);

        // Calculate distances in physical pixels (edge-to-edge)
        let h_distance = edges.right.saturating_sub(edges.left);
        let v_distance = edges.down.saturating_sub(edges.up);

        // Draw combined label near cursor (offset to bottom-right)
        Self::draw_label_impl(
            pixmap,
            &format!("{} x {}", h_distance, v_distance),
            cursor_x + 30.0,
            cursor_y + 30.0,
            font,
        );
    }

    fn draw_end_cap_static(pixmap: &mut Pixmap, paint: &Paint, stroke: &Stroke, x: f32, y: f32, vertical: bool) {
        let cap_size = END_CAP_SIZE * 2.0; // Slightly larger for HiDPI
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

    fn draw_crosshair_static(pixmap: &mut Pixmap, x: f32, y: f32) {
        let mut paint = Paint::default();
        paint.set_color(line_color());
        paint.anti_alias = true;

        let stroke = Stroke {
            width: 2.0, // Fixed for HiDPI
            ..Default::default()
        };

        let size = 15.0; // Fixed for HiDPI

        // Horizontal part of crosshair
        let mut pb = PathBuilder::new();
        pb.move_to(x - size, y);
        pb.line_to(x + size, y);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }

        // Vertical part of crosshair
        let mut pb = PathBuilder::new();
        pb.move_to(x, y - size);
        pb.line_to(x, y + size);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    fn draw_label_impl(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, font: Option<&Arc<fontdue::Font>>) {
        let font_size = 24.0; // Fixed for HiDPI
        let padding_x = 12.0;
        let padding_y = 6.0;

        let text_width = text.len() as f32 * font_size * 0.6;
        let text_height = font_size;

        let label_width = text_width + padding_x * 2.0;
        let label_height = text_height + padding_y * 2.0;

        let label_x = x - label_width / 2.0;
        let label_y = y - label_height / 2.0;

        // Draw rounded rectangle background
        let mut bg_paint = Paint::default();
        bg_paint.set_color(label_bg_color());
        bg_paint.anti_alias = true;

        let radius = 6.0;
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

        // Draw text
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
                                        let bg_r = pixel.red() as f32;
                                        let bg_g = pixel.green() as f32;
                                        let bg_b = pixel.blue() as f32;
                                        let bg_a = pixel.alpha() as f32;

                                        let r = ((1.0 - a) * bg_r + a * 255.0) as u8;
                                        let g = ((1.0 - a) * bg_g + a * 255.0) as u8;
                                        let b = ((1.0 - a) * bg_b + a * 255.0) as u8;
                                        let new_a = ((1.0 - a) * bg_a + a * 255.0) as u8;

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

    fn request_redraw(&mut self) {
        self.needs_redraw = true;
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
        }
        if capability == Capability::Pointer && self.seat_state.get_pointer(qh, &seat).is_err() {
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
                    self.request_redraw();
                    self.draw(qh);
                }
                PointerEventKind::Press { button, .. } => {
                    if button == 272 {
                        // Left click exits
                        self.exit = true;
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

    // Capture screen FIRST, before creating overlay
    eprintln!("Capturing screen...");
    let screenshot = match capture_screen(&conn) {
        Ok(s) => {
            eprintln!("Captured {}x{} screenshot", s.width, s.height);
            s
        }
        Err(e) => {
            eprintln!("Failed to capture screen: {}", e);
            std::process::exit(1);
        }
    };

    // Now set up the overlay
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
        screenshot,
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
