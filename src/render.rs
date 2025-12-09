use crate::capture::Screenshot;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform};

const LINE_WIDTH: f32 = 4.0;
const END_CAP_SIZE: f32 = 16.0;
const CROSSHAIR_SIZE: f32 = 15.0;
const EDGE_THRESHOLD: i32 = 25;
const FONT_SIZE: f32 = 24.0;
const LABEL_PADDING: (f32, f32) = (12.0, 6.0);
const LABEL_RADIUS: f32 = 6.0;
const LABEL_OFFSET: (f32, f32) = (120.0, 50.0);

fn line_color() -> Color {
    Color::from_rgba8(231, 76, 60, 255)
}

fn label_bg_color() -> Color {
    Color::from_rgba8(40, 40, 40, 230)
}

fn stroke_line(pixmap: &mut Pixmap, paint: &Paint, stroke: &Stroke, x1: f32, y1: f32, x2: f32, y2: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, paint, stroke, Transform::identity(), None);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Edges {
    pub left: u32,
    pub right: u32,
    pub up: u32,
    pub down: u32,
}

pub fn find_edges(screenshot: &Screenshot, cursor_x: u32, cursor_y: u32) -> Edges {
    Edges {
        left: scan_horizontal(screenshot, cursor_x, cursor_y, -1).unwrap_or(0),
        right: scan_horizontal(screenshot, cursor_x, cursor_y, 1).unwrap_or(screenshot.width - 1),
        up: scan_vertical(screenshot, cursor_x, cursor_y, -1).unwrap_or(0),
        down: scan_vertical(screenshot, cursor_x, cursor_y, 1).unwrap_or(screenshot.height - 1),
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
        if (lum - prev_lum).abs() > EDGE_THRESHOLD {
            return Some(if direction < 0 { (x + 1) as u32 } else { (x - 1) as u32 });
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
        if (lum - prev_lum).abs() > EDGE_THRESHOLD {
            return Some(if direction < 0 { (y + 1) as u32 } else { (y - 1) as u32 });
        }
        prev_lum = lum;
    }
}

pub fn draw_measurements(
    pixmap: &mut Pixmap,
    edges: &Edges,
    cursor_x: u32,
    cursor_y: u32,
    font: Option<&fontdue::Font>,
) {
    let mut paint = Paint::default();
    paint.set_color(line_color());
    paint.anti_alias = true;

    let stroke = Stroke {
        width: LINE_WIDTH,
        ..Default::default()
    };

    let left = edges.left as f32;
    let right = edges.right as f32;
    let up = edges.up as f32;
    let down = edges.down as f32;
    let cx = cursor_x as f32;
    let cy = cursor_y as f32;

    // Horizontal measurement line
    stroke_line(pixmap, &paint, &stroke, left, cy, right, cy);
    draw_end_cap(pixmap, &paint, &stroke, left, cy, true);
    draw_end_cap(pixmap, &paint, &stroke, right, cy, true);

    // Vertical measurement line
    stroke_line(pixmap, &paint, &stroke, cx, up, cx, down);
    draw_end_cap(pixmap, &paint, &stroke, cx, up, false);
    draw_end_cap(pixmap, &paint, &stroke, cx, down, false);

    // Dimension label
    let h_distance = edges.right.saturating_sub(edges.left);
    let v_distance = edges.down.saturating_sub(edges.up);
    draw_label(
        pixmap,
        &format!("{} x {}", h_distance, v_distance),
        cx + LABEL_OFFSET.0,
        cy + LABEL_OFFSET.1,
        font,
    );
}

fn draw_end_cap(pixmap: &mut Pixmap, paint: &Paint, stroke: &Stroke, x: f32, y: f32, vertical: bool) {
    let half = END_CAP_SIZE / 2.0;
    if vertical {
        stroke_line(pixmap, paint, stroke, x, y - half, x, y + half);
    } else {
        stroke_line(pixmap, paint, stroke, x - half, y, x + half, y);
    }
}

pub fn draw_crosshair(pixmap: &mut Pixmap, x: f32, y: f32) {
    let mut paint = Paint::default();
    paint.set_color(line_color());
    paint.anti_alias = true;

    let stroke = Stroke {
        width: 2.0,
        ..Default::default()
    };

    stroke_line(pixmap, &paint, &stroke, x - CROSSHAIR_SIZE, y, x + CROSSHAIR_SIZE, y);
    stroke_line(pixmap, &paint, &stroke, x, y - CROSSHAIR_SIZE, x, y + CROSSHAIR_SIZE);
}

fn draw_label(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, font: Option<&fontdue::Font>) {
    let text_width = text.len() as f32 * FONT_SIZE * 0.6;
    let text_height = FONT_SIZE;

    let label_width = text_width + LABEL_PADDING.0 * 2.0;
    let label_height = text_height + LABEL_PADDING.1 * 2.0;

    let label_x = x - label_width / 2.0;
    let label_y = y - label_height / 2.0;

    // Rounded rectangle background
    let mut bg_paint = Paint::default();
    bg_paint.set_color(label_bg_color());
    bg_paint.anti_alias = true;

    let r = LABEL_RADIUS;
    let mut pb = PathBuilder::new();
    pb.move_to(label_x + r, label_y);
    pb.line_to(label_x + label_width - r, label_y);
    pb.quad_to(label_x + label_width, label_y, label_x + label_width, label_y + r);
    pb.line_to(label_x + label_width, label_y + label_height - r);
    pb.quad_to(label_x + label_width, label_y + label_height, label_x + label_width - r, label_y + label_height);
    pb.line_to(label_x + r, label_y + label_height);
    pb.quad_to(label_x, label_y + label_height, label_x, label_y + label_height - r);
    pb.line_to(label_x, label_y + r);
    pb.quad_to(label_x, label_y, label_x + r, label_y);
    pb.close();

    if let Some(path) = pb.finish() {
        pixmap.fill_path(&path, &bg_paint, FillRule::Winding, Transform::identity(), None);
    }

    // Render text
    let Some(font) = font else { return };

    let mut cursor_x = label_x + LABEL_PADDING.0;
    let baseline_y = label_y + LABEL_PADDING.1 + FONT_SIZE * 0.8;

    let pixmap_width = pixmap.width() as i32;
    let pixmap_height = pixmap.height() as i32;
    let width = pixmap_width as usize;
    let pixels = pixmap.pixels_mut();

    for c in text.chars() {
        let (metrics, bitmap) = font.rasterize(c, FONT_SIZE);

        for py in 0..metrics.height {
            for px in 0..metrics.width {
                let alpha = bitmap[py * metrics.width + px];
                if alpha == 0 {
                    continue;
                }

                let draw_x = cursor_x as i32 + px as i32 + metrics.xmin;
                let draw_y = baseline_y as i32 + py as i32 - metrics.height as i32 - metrics.ymin;

                if draw_x >= 0 && draw_x < pixmap_width && draw_y >= 0 && draw_y < pixmap_height {
                    let idx = draw_y as usize * width + draw_x as usize;
                    if idx < pixels.len() {
                        let pixel = &pixels[idx];
                        let a = alpha as f32 / 255.0;
                        let r = ((1.0 - a) * pixel.red() as f32 + a * 255.0).min(pixel.alpha() as f32) as u8;
                        let g = ((1.0 - a) * pixel.green() as f32 + a * 255.0).min(pixel.alpha() as f32) as u8;
                        let b = ((1.0 - a) * pixel.blue() as f32 + a * 255.0).min(pixel.alpha() as f32) as u8;
                        let new_a = ((1.0 - a) * pixel.alpha() as f32 + a * 255.0) as u8;

                        if let Some(new_pixel) = PremultipliedColorU8::from_rgba(r, g, b, new_a) {
                            pixels[idx] = new_pixel;
                        }
                    }
                }
            }
        }

        cursor_x += metrics.advance_width;
    }
}
