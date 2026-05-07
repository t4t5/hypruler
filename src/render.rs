use crate::capture::Screenshot;
use crate::edge_detection::find_edges;
use crate::ui::{draw_crosshair, draw_label, draw_measurements, draw_rectangle_measurement};
use tiny_skia::Pixmap;

#[derive(Debug, Clone, Copy)]
pub struct FrameOverlay {
    pub pointer_x: f64,
    pub pointer_y: f64,
    pub scale: f64,
    pub drag_start: Option<(f64, f64)>,
    pub drag_rect: Option<(u32, u32, u32, u32)>,
    pub is_dragging: bool,
    /// Smoothed frames-per-second to draw as a debug overlay. `None` disables it.
    pub debug_fps: Option<f64>,
}

fn normalize_rect(x1: u32, y1: u32, x2: u32, y2: u32) -> (u32, u32, u32, u32) {
    (x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2))
}

fn to_physical(logical: f64, scale: f64) -> u32 {
    (logical * scale) as u32
}

pub fn compose_frame(
    canvas: &mut [u8],
    cached_pixmap: &mut Option<Pixmap>,
    screenshot: &Screenshot,
    overlay: FrameOverlay,
    font: Option<&fontdue::Font>,
) -> Result<(), &'static str> {
    let phys_width = screenshot.width;
    let phys_height = screenshot.height;
    let size = phys_width
        .checked_mul(phys_height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("frame dimensions overflow")? as usize;

    if canvas.len() < size {
        return Err("canvas is smaller than screenshot");
    }

    let cursor_phys_x = to_physical(overlay.pointer_x, overlay.scale);
    let cursor_phys_y = to_physical(overlay.pointer_y, overlay.scale);

    let bgra = screenshot.bgra_data();
    let bgra_size = bgra.len().min(size);
    canvas[..bgra_size].copy_from_slice(&bgra[..bgra_size]);

    let needs_new_pixmap = cached_pixmap
        .as_ref()
        .map(|p| p.width() != phys_width || p.height() != phys_height)
        .unwrap_or(true);

    if needs_new_pixmap {
        *cached_pixmap = Pixmap::new(phys_width, phys_height);
    }

    let pixmap = cached_pixmap.as_mut().ok_or("failed to allocate pixmap")?;
    pixmap.fill(tiny_skia::Color::TRANSPARENT);

    if overlay.is_dragging {
        if let Some((start_x, start_y)) = overlay.drag_start {
            let (left, top, right, bottom) = normalize_rect(
                to_physical(start_x, overlay.scale),
                to_physical(start_y, overlay.scale),
                cursor_phys_x,
                cursor_phys_y,
            );
            draw_rectangle_measurement(pixmap, left, top, right, bottom, font, overlay.scale);
        }
    } else if cursor_phys_x < screenshot.width && cursor_phys_y < screenshot.height {
        if let Some((x1, y1, x2, y2)) = overlay.drag_rect {
            draw_rectangle_measurement(pixmap, x1, y1, x2, y2, font, overlay.scale);
        }

        let edges = find_edges(screenshot, cursor_phys_x, cursor_phys_y);
        draw_measurements(
            pixmap,
            &edges,
            cursor_phys_x,
            cursor_phys_y,
            font,
            overlay.scale,
        );
        draw_crosshair(pixmap, cursor_phys_x as f32, cursor_phys_y as f32);
    }

    if let Some(fps) = overlay.debug_fps {
        draw_label(pixmap, &format!("FPS: {fps:.1}"), 80.0, 30.0, font);
    }

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

    Ok(())
}
