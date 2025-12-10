use crate::capture::Screenshot;

const EDGE_THRESHOLD: i32 = 5;

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
            return Some(if direction < 0 {
                (x + 1) as u32
            } else {
                (x - 1) as u32
            });
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
            return Some(if direction < 0 {
                (y + 1) as u32
            } else {
                (y - 1) as u32
            });
        }
        prev_lum = lum;
    }
}

/// Snap threshold in pixels - how close to an edge before snapping occurs
const SNAP_THRESHOLD: u32 = 200;

/// Find the nearest vertical edge from a given x position, preferring the inward direction.
/// Samples at multiple y positions to find the outermost edge.
/// `prefer_direction`: 1 = prefer rightward (for left edge), -1 = prefer leftward (for right edge)
pub fn snap_to_nearest_edge_x(
    screenshot: &Screenshot,
    x: u32,
    y_start: u32,
    y_end: u32,
    prefer_direction: i32,
) -> u32 {
    let mut best_edge: Option<u32> = None;

    // Sample every pixel along the edge for maximum accuracy
    for y in y_start..=y_end {
        if let Some(edge) = scan_horizontal_limited(screenshot, x, y, prefer_direction, SNAP_THRESHOLD) {
            best_edge = Some(match best_edge {
                // For left edge (prefer_direction=1), we want the leftmost (min)
                // For right edge (prefer_direction=-1), we want the rightmost (max)
                Some(prev) if prefer_direction > 0 => prev.min(edge),
                Some(prev) => prev.max(edge),
                None => edge,
            });
        }
    }

    if let Some(edge) = best_edge {
        return edge;
    }

    // Fallback: try opposite direction at center
    let center_y = (y_start + y_end) / 2;
    let fallback_edge =
        scan_horizontal_limited(screenshot, x, center_y, -prefer_direction, SNAP_THRESHOLD);
    fallback_edge.unwrap_or(x)
}

/// Find the nearest horizontal edge from a given y position, preferring the inward direction.
/// Samples at multiple x positions to find the outermost edge.
/// `prefer_direction`: 1 = prefer downward (for top edge), -1 = prefer upward (for bottom edge)
pub fn snap_to_nearest_edge_y(
    screenshot: &Screenshot,
    x_start: u32,
    x_end: u32,
    y: u32,
    prefer_direction: i32,
) -> u32 {
    let mut best_edge: Option<u32> = None;

    // Sample every pixel along the edge for maximum accuracy
    for x in x_start..=x_end {
        if let Some(edge) = scan_vertical_limited(screenshot, x, y, prefer_direction, SNAP_THRESHOLD) {
            best_edge = Some(match best_edge {
                // For top edge (prefer_direction=1), we want the topmost (min)
                // For bottom edge (prefer_direction=-1), we want the bottommost (max)
                Some(prev) if prefer_direction > 0 => prev.min(edge),
                Some(prev) => prev.max(edge),
                None => edge,
            });
        }
    }

    if let Some(edge) = best_edge {
        return edge;
    }

    // Fallback: try opposite direction at center
    let center_x = (x_start + x_end) / 2;
    let fallback_edge =
        scan_vertical_limited(screenshot, center_x, y, -prefer_direction, SNAP_THRESHOLD);
    fallback_edge.unwrap_or(y)
}

/// Minimum luminance difference from starting point to consider as content
/// Higher value = less sensitive, won't snap to subtle gradients
const SNAP_EDGE_THRESHOLD: i32 = 30;

/// Scan horizontally for an edge within a limited distance
/// Returns the position of the first pixel that differs significantly from the starting luminance
fn scan_horizontal_limited(
    screenshot: &Screenshot,
    start_x: u32,
    y: u32,
    direction: i32,
    max_distance: u32,
) -> Option<u32> {
    let mut x = start_x as i32;
    let start_lum = screenshot.get_luminance(start_x, y) as i32;
    let mut distance = 0u32;

    loop {
        x += direction;
        distance += 1;

        if distance > max_distance || x < 0 || x >= screenshot.width as i32 {
            return None;
        }

        let lum = screenshot.get_luminance(x as u32, y) as i32;
        // Compare against starting luminance, not previous pixel
        // This skips over anti-aliasing gradients
        if (lum - start_lum).abs() > SNAP_EDGE_THRESHOLD {
            return Some(x as u32);
        }
    }
}

/// Scan vertically for an edge within a limited distance
/// Returns the position of the first pixel that differs significantly from the starting luminance
fn scan_vertical_limited(
    screenshot: &Screenshot,
    x: u32,
    start_y: u32,
    direction: i32,
    max_distance: u32,
) -> Option<u32> {
    let mut y = start_y as i32;
    let start_lum = screenshot.get_luminance(x, start_y) as i32;
    let mut distance = 0u32;

    loop {
        y += direction;
        distance += 1;

        if distance > max_distance || y < 0 || y >= screenshot.height as i32 {
            return None;
        }

        let lum = screenshot.get_luminance(x, y as u32) as i32;
        // Compare against starting luminance, not previous pixel
        // This skips over anti-aliasing gradients
        if (lum - start_lum).abs() > SNAP_EDGE_THRESHOLD {
            return Some(y as u32);
        }
    }
}
