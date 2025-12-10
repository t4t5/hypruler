use crate::capture::Screenshot;

const EDGE_THRESHOLD: i32 = 25;

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
