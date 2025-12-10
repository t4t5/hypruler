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
const SNAP_THRESHOLD: u32 = 50;

/// Find the nearest vertical edge (left or right) from a given x position.
/// Scans in both directions within the threshold and returns the closest edge.
pub fn snap_to_nearest_edge_x(screenshot: &Screenshot, x: u32, y: u32) -> u32 {
    let left_edge = scan_horizontal_limited(screenshot, x, y, -1, SNAP_THRESHOLD);
    let right_edge = scan_horizontal_limited(screenshot, x, y, 1, SNAP_THRESHOLD);

    match (left_edge, right_edge) {
        (Some(left), Some(right)) => {
            // Return the closer edge
            let left_dist = x.saturating_sub(left);
            let right_dist = right.saturating_sub(x);
            if left_dist <= right_dist { left } else { right }
        }
        (Some(left), None) => left,
        (None, Some(right)) => right,
        (None, None) => x, // No edge found, return original
    }
}

/// Find the nearest horizontal edge (up or down) from a given y position.
/// Scans in both directions within the threshold and returns the closest edge.
pub fn snap_to_nearest_edge_y(screenshot: &Screenshot, x: u32, y: u32) -> u32 {
    let up_edge = scan_vertical_limited(screenshot, x, y, -1, SNAP_THRESHOLD);
    let down_edge = scan_vertical_limited(screenshot, x, y, 1, SNAP_THRESHOLD);

    match (up_edge, down_edge) {
        (Some(up), Some(down)) => {
            // Return the closer edge
            let up_dist = y.saturating_sub(up);
            let down_dist = down.saturating_sub(y);
            if up_dist <= down_dist { up } else { down }
        }
        (Some(up), None) => up,
        (None, Some(down)) => down,
        (None, None) => y, // No edge found, return original
    }
}

/// Scan horizontally for an edge within a limited distance
fn scan_horizontal_limited(
    screenshot: &Screenshot,
    start_x: u32,
    y: u32,
    direction: i32,
    max_distance: u32,
) -> Option<u32> {
    let mut x = start_x as i32;
    let mut prev_lum = screenshot.get_luminance(start_x, y) as i32;
    let mut distance = 0u32;

    loop {
        x += direction;
        distance += 1;

        if distance > max_distance || x < 0 || x >= screenshot.width as i32 {
            return None;
        }

        let lum = screenshot.get_luminance(x as u32, y) as i32;
        if (lum - prev_lum).abs() > EDGE_THRESHOLD {
            // Return the position just before the edge (inside the element)
            return Some(if direction < 0 {
                (x + 1) as u32
            } else {
                (x - 1) as u32
            });
        }
        prev_lum = lum;
    }
}

/// Scan vertically for an edge within a limited distance
fn scan_vertical_limited(
    screenshot: &Screenshot,
    x: u32,
    start_y: u32,
    direction: i32,
    max_distance: u32,
) -> Option<u32> {
    let mut y = start_y as i32;
    let mut prev_lum = screenshot.get_luminance(x, start_y) as i32;
    let mut distance = 0u32;

    loop {
        y += direction;
        distance += 1;

        if distance > max_distance || y < 0 || y >= screenshot.height as i32 {
            return None;
        }

        let lum = screenshot.get_luminance(x, y as u32) as i32;
        if (lum - prev_lum).abs() > EDGE_THRESHOLD {
            // Return the position just before the edge (inside the element)
            return Some(if direction < 0 {
                (y + 1) as u32
            } else {
                (y - 1) as u32
            });
        }
        prev_lum = lum;
    }
}
