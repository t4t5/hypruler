use crate::edge_detection::Edges;
use tiny_skia::{
    Color, FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform,
};

const LINE_WIDTH: f32 = 2.0;
const END_CAP_SIZE: f32 = 16.0;
const CROSSHAIR_SIZE: f32 = 15.0;
const FONT_SIZE: f32 = 24.0;
const LABEL_PADDING: (f32, f32) = (12.0, 6.0);
const LABEL_RADIUS: f32 = 6.0;
const LABEL_OFFSET: (f32, f32) = (95.0, 40.0);

// How close to screen edges before flipping label position:
const EDGE_THRESHOLD_X: f32 = 200.0;
const EDGE_THRESHOLD_Y: f32 = 100.0;

fn get_label_position(cx: f32, cy: f32, screen_w: u32, screen_h: u32) -> (f32, f32) {
    let x = if cx > screen_w as f32 - EDGE_THRESHOLD_X {
        cx - LABEL_OFFSET.0
    } else {
        cx + LABEL_OFFSET.0
    };
    let y = if cy > screen_h as f32 - EDGE_THRESHOLD_Y {
        cy - LABEL_OFFSET.1
    } else {
        cy + LABEL_OFFSET.1
    };
    (x, y)
}

fn line_color() -> Color {
    Color::from_rgba8(231, 76, 60, 255)
}

fn fill_color() -> Color {
    Color::from_rgba8(231, 76, 60, 60)
}

fn label_bg_color() -> Color {
    Color::from_rgba8(40, 40, 40, 230)
}

fn stroke_line(
    pixmap: &mut Pixmap,
    paint: &Paint,
    stroke: &Stroke,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) {
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, paint, stroke, Transform::identity(), None);
    }
}

pub fn draw_measurements(
    pixmap: &mut Pixmap,
    edges: &Edges,
    cursor_x: u32,
    cursor_y: u32,
    font: Option<&fontdue::Font>,
    scale: f64,
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

    // Dimension label (convert physical pixels to logical pixels)
    // Add 1 because distance from pixel N to pixel M is M - N + 1 pixels
    let h_distance = ((edges.right.saturating_sub(edges.left) + 1) as f64 / scale).round() as u32;
    let v_distance = ((edges.down.saturating_sub(edges.up) + 1) as f64 / scale).round() as u32;
    let (lx, ly) = get_label_position(cx, cy, pixmap.width(), pixmap.height());
    draw_label(
        pixmap,
        &format!("{} x {}", h_distance, v_distance),
        lx,
        ly,
        font,
    );
}

pub fn draw_rectangle_measurement(
    pixmap: &mut Pixmap,
    x1: u32,
    y1: u32,
    x2: u32,
    y2: u32,
    font: Option<&fontdue::Font>,
    scale: f64,
) {
    let left = x1 as f32;
    let top = y1 as f32;
    let right = x2 as f32;
    let bottom = y2 as f32;

    // Draw filled rectangle
    let mut fill_paint = Paint::default();
    fill_paint.set_color(fill_color());
    fill_paint.anti_alias = true;

    let mut pb = PathBuilder::new();
    pb.move_to(left, top);
    pb.line_to(right, top);
    pb.line_to(right, bottom);
    pb.line_to(left, bottom);
    pb.close();
    if let Some(path) = pb.finish() {
        pixmap.fill_path(
            &path,
            &fill_paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    // Draw outline
    let mut stroke_paint = Paint::default();
    stroke_paint.set_color(line_color());
    stroke_paint.anti_alias = true;

    let stroke = Stroke {
        width: LINE_WIDTH,
        ..Default::default()
    };

    // Top edge
    stroke_line(pixmap, &stroke_paint, &stroke, left, top, right, top);
    // Bottom edge
    stroke_line(pixmap, &stroke_paint, &stroke, left, bottom, right, bottom);
    // Left edge
    stroke_line(pixmap, &stroke_paint, &stroke, left, top, left, bottom);
    // Right edge
    stroke_line(pixmap, &stroke_paint, &stroke, right, top, right, bottom);

    // Draw dimension label (convert physical pixels to logical pixels)
    let width = ((x2.saturating_sub(x1) + 1) as f64 / scale).round() as u32;
    let height = ((y2.saturating_sub(y1) + 1) as f64 / scale).round() as u32;
    // Use physical pixel sizes for layout threshold check
    let phys_width = x2.saturating_sub(x1) + 1;
    let phys_height = y2.saturating_sub(y1) + 1;
    let (lx, ly) = if phys_width >= 150 && phys_height >= 50 {
        // Center on rectangle if large enough
        ((left + right) / 2.0, (top + bottom) / 2.0)
    } else {
        // Position at bottom center of rectangle
        let center_x = (left + right) / 2.0;
        let offset_y = 30.0;
        let y = if bottom + offset_y > pixmap.height() as f32 - EDGE_THRESHOLD_Y {
            top - offset_y // Move above if near bottom edge
        } else {
            bottom + offset_y
        };
        (center_x, y)
    };
    draw_label(pixmap, &format!("{} x {}", width, height), lx, ly, font);
}

fn draw_end_cap(
    pixmap: &mut Pixmap,
    paint: &Paint,
    stroke: &Stroke,
    x: f32,
    y: f32,
    vertical: bool,
) {
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

    stroke_line(
        pixmap,
        &paint,
        &stroke,
        x - CROSSHAIR_SIZE,
        y,
        x + CROSSHAIR_SIZE,
        y,
    );
    stroke_line(
        pixmap,
        &paint,
        &stroke,
        x,
        y - CROSSHAIR_SIZE,
        x,
        y + CROSSHAIR_SIZE,
    );
}

fn draw_rounded_rect(pixmap: &mut Pixmap, x: f32, y: f32, width: f32, height: f32, radius: f32) {
    let mut paint = Paint::default();
    paint.set_color(label_bg_color());
    paint.anti_alias = true;

    let mut pb = PathBuilder::new();
    pb.move_to(x + radius, y);
    pb.line_to(x + width - radius, y);
    pb.quad_to(x + width, y, x + width, y + radius);
    pb.line_to(x + width, y + height - radius);
    pb.quad_to(x + width, y + height, x + width - radius, y + height);
    pb.line_to(x + radius, y + height);
    pb.quad_to(x, y + height, x, y + height - radius);
    pb.line_to(x, y + radius);
    pb.quad_to(x, y, x + radius, y);
    pb.close();

    if let Some(path) = pb.finish() {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn blend_pixel(pixel: &PremultipliedColorU8, alpha: f32) -> Option<PremultipliedColorU8> {
    let inv_a = 1.0 - alpha;
    let max_val = pixel.alpha() as f32;
    PremultipliedColorU8::from_rgba(
        ((inv_a * pixel.red() as f32 + alpha * 255.0).min(max_val)) as u8,
        ((inv_a * pixel.green() as f32 + alpha * 255.0).min(max_val)) as u8,
        ((inv_a * pixel.blue() as f32 + alpha * 255.0).min(max_val)) as u8,
        (inv_a * pixel.alpha() as f32 + alpha * 255.0) as u8,
    )
}

fn draw_text(pixmap: &mut Pixmap, font: &fontdue::Font, text: &str, start_x: f32, baseline_y: f32) {
    let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
    let stride = width as usize;
    let pixels = pixmap.pixels_mut();

    let mut cursor_x = start_x;
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

                if draw_x < 0 || draw_x >= width || draw_y < 0 || draw_y >= height {
                    continue;
                }

                let idx = draw_y as usize * stride + draw_x as usize;
                if let Some(new_pixel) = blend_pixel(&pixels[idx], alpha as f32 / 255.0) {
                    pixels[idx] = new_pixel;
                }
            }
        }
        cursor_x += metrics.advance_width;
    }
}

fn draw_label(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, font: Option<&fontdue::Font>) {
    let mut text_width = 0.0;
    if let Some(font) = font {
        for c in text.chars() {
            let metrics = font.metrics(c, FONT_SIZE);
            text_width += metrics.advance_width;
        }
    }
    let label_width = text_width + LABEL_PADDING.0 * 2.0;
    let label_height = FONT_SIZE + LABEL_PADDING.1 * 2.0;
    let label_x = x - label_width / 2.0;
    let label_y = y - label_height / 2.0;

    draw_rounded_rect(
        pixmap,
        label_x,
        label_y,
        label_width,
        label_height,
        LABEL_RADIUS,
    );

    if let Some(font) = font {
        let text_x = label_x + LABEL_PADDING.0;
        let baseline_y = label_y + LABEL_PADDING.1 + FONT_SIZE * 0.8;
        draw_text(pixmap, font, text, text_x, baseline_y);
    }
}
