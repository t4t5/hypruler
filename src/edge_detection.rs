use crate::capture::Screenshot;

const EDGE_THRESHOLD: i32 = 1;
const SNAP_THRESHOLD: i32 = 10;
const SNAP_DISTANCE: u32 = 200;

#[derive(Debug, Clone, Copy)]
pub struct Edges {
    pub left: u32,
    pub right: u32,
    pub up: u32,
    pub down: u32,
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

/// Generic scan function for edge detection.
/// Scans along `axis` from starting position, looking for luminance changes.
/// Returns the pixel position just before the edge (for edge detection mode).
fn scan_for_edge(
    screenshot: &Screenshot,
    start_x: u32,
    start_y: u32,
    axis: Axis,
    direction: i32,
    threshold: i32,
    max_distance: Option<u32>,
) -> Option<u32> {
    let (mut pos, fixed, limit) = match axis {
        Axis::X => (start_x as i32, start_y, screenshot.width as i32),
        Axis::Y => (start_y as i32, start_x, screenshot.height as i32),
    };

    let get_lum = |p: i32| -> u8 {
        match axis {
            Axis::X => screenshot.get_luminance(p as u32, fixed),
            Axis::Y => screenshot.get_luminance(fixed, p as u32),
        }
    };

    let start_lum = get_lum(pos) as i32;
    let mut prev_lum = start_lum;
    let mut steps = 0u32;

    loop {
        pos += direction;
        steps += 1;

        if pos < 0 || pos >= limit {
            return None;
        }
        if let Some(max) = max_distance {
            if steps > max {
                return None;
            }
        }

        let lum = get_lum(pos) as i32;

        // For snap mode (max_distance set): compare against start luminance
        // For edge mode: compare against previous pixel (tracks gradient)
        let diff = if max_distance.is_some() {
            (lum - start_lum).abs()
        } else {
            (lum - prev_lum).abs()
        };

        if diff > threshold {
            // For edge detection: return pixel before the edge
            // For snap: return the edge pixel itself
            return Some(if max_distance.is_some() {
                pos as u32
            } else if direction < 0 {
                (pos + 1) as u32
            } else {
                (pos - 1) as u32
            });
        }
        prev_lum = lum;
    }
}

pub fn find_edges(screenshot: &Screenshot, cursor_x: u32, cursor_y: u32) -> Edges {
    Edges {
        left: scan_for_edge(
            screenshot,
            cursor_x,
            cursor_y,
            Axis::X,
            -1,
            EDGE_THRESHOLD,
            None,
        )
        .unwrap_or(0),
        right: scan_for_edge(
            screenshot,
            cursor_x,
            cursor_y,
            Axis::X,
            1,
            EDGE_THRESHOLD,
            None,
        )
        .unwrap_or(screenshot.width - 1),
        up: scan_for_edge(
            screenshot,
            cursor_x,
            cursor_y,
            Axis::Y,
            -1,
            EDGE_THRESHOLD,
            None,
        )
        .unwrap_or(0),
        down: scan_for_edge(
            screenshot,
            cursor_x,
            cursor_y,
            Axis::Y,
            1,
            EDGE_THRESHOLD,
            None,
        )
        .unwrap_or(screenshot.height - 1),
    }
}

/// Snap a vertical edge (left or right) to nearby content, skipping scanlines inside an ignored rectangle.
pub fn snap_edge_x_ignoring_rect(
    screenshot: &Screenshot,
    x: u32,
    y_start: u32,
    y_end: u32,
    direction: i32,
    ignored_rect: Option<(u32, u32, u32, u32)>,
) -> u32 {
    (y_start..=y_end)
        .filter(|&y| {
            ignored_rect
                .map(|(_, top, _, bottom)| y < top || y > bottom)
                .unwrap_or(true)
        })
        .filter_map(|y| {
            scan_for_edge(
                screenshot,
                x,
                y,
                Axis::X,
                direction,
                SNAP_THRESHOLD,
                Some(SNAP_DISTANCE),
            )
        })
        .reduce(|a, b| if direction > 0 { a.min(b) } else { a.max(b) })
        .unwrap_or(x)
}

/// Snap a horizontal edge (top or bottom) to nearby content, skipping scanlines inside an ignored rectangle.
pub fn snap_edge_y_ignoring_rect(
    screenshot: &Screenshot,
    x_start: u32,
    x_end: u32,
    y: u32,
    direction: i32,
    ignored_rect: Option<(u32, u32, u32, u32)>,
) -> u32 {
    (x_start..=x_end)
        .filter(|&x| {
            ignored_rect
                .map(|(left, _, right, _)| x < left || x > right)
                .unwrap_or(true)
        })
        .filter_map(|x| {
            scan_for_edge(
                screenshot,
                x,
                y,
                Axis::Y,
                direction,
                SNAP_THRESHOLD,
                Some(SNAP_DISTANCE),
            )
        })
        .reduce(|a, b| if direction > 0 { a.min(b) } else { a.max(b) })
        .unwrap_or(y)
}
