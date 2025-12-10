use crate::capture::Screenshot;

const EDGE_THRESHOLD: i32 = 1;

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

/// Max distance to search for an edge when snapping
const SNAP_DISTANCE: u32 = 200;

/// Luminance difference threshold for snapping (higher = less sensitive)
const SNAP_THRESHOLD: i32 = 10;

/// Snap a vertical edge (left or right) to nearby content.
/// Scans inward from every pixel along the edge and returns the outermost hit.
pub fn snap_edge_x(
    screenshot: &Screenshot,
    x: u32,
    y_start: u32,
    y_end: u32,
    direction: i32,
) -> u32 {
    (y_start..=y_end)
        .filter_map(|y| find_edge_x(screenshot, x, y, direction))
        .reduce(|a, b| if direction > 0 { a.min(b) } else { a.max(b) })
        .unwrap_or(x)
}

/// Snap a horizontal edge (top or bottom) to nearby content.
/// Scans inward from every pixel along the edge and returns the outermost hit.
pub fn snap_edge_y(
    screenshot: &Screenshot,
    x_start: u32,
    x_end: u32,
    y: u32,
    direction: i32,
) -> u32 {
    (x_start..=x_end)
        .filter_map(|x| find_edge_y(screenshot, x, y, direction))
        .reduce(|a, b| if direction > 0 { a.min(b) } else { a.max(b) })
        .unwrap_or(y)
}

/// Find first pixel with significant luminance change horizontally
fn find_edge_x(screenshot: &Screenshot, start_x: u32, y: u32, direction: i32) -> Option<u32> {
    let start_lum = screenshot.get_luminance(start_x, y) as i32;
    let mut x = start_x as i32;

    for _ in 0..SNAP_DISTANCE {
        x += direction;
        if x < 0 || x >= screenshot.width as i32 {
            return None;
        }
        if (screenshot.get_luminance(x as u32, y) as i32 - start_lum).abs() > SNAP_THRESHOLD {
            return Some(x as u32);
        }
    }
    None
}

/// Find first pixel with significant luminance change vertically
fn find_edge_y(screenshot: &Screenshot, x: u32, start_y: u32, direction: i32) -> Option<u32> {
    let start_lum = screenshot.get_luminance(x, start_y) as i32;
    let mut y = start_y as i32;

    for _ in 0..SNAP_DISTANCE {
        y += direction;
        if y < 0 || y >= screenshot.height as i32 {
            return None;
        }
        if (screenshot.get_luminance(x, y as u32) as i32 - start_lum).abs() > SNAP_THRESHOLD {
            return Some(y as u32);
        }
    }
    None
}
