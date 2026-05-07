use std::time::Instant;

/// EMA-smoothed FPS counter. Active only when `HYPRULER_DEBUG=1` in a debug build.
pub struct FrameClock {
    last: Option<Instant>,
    smoothed_fps: f64,
}

impl FrameClock {
    fn new() -> Self {
        Self {
            last: None,
            smoothed_fps: 0.0,
        }
    }

    /// Record a frame. Returns `(dt_ms, instantaneous_fps)` if a previous tick exists.
    pub fn tick(&mut self) -> Option<(f64, f64)> {
        let now = Instant::now();
        let result = self.last.map(|prev| {
            let dt_ms = now.duration_since(prev).as_secs_f64() * 1000.0;
            let inst_fps = if dt_ms > 0.0 { 1000.0 / dt_ms } else { 0.0 };
            let alpha = 0.1;
            self.smoothed_fps = if self.smoothed_fps == 0.0 {
                inst_fps
            } else {
                alpha * inst_fps + (1.0 - alpha) * self.smoothed_fps
            };
            (dt_ms, inst_fps)
        });
        self.last = Some(now);
        result
    }

    pub fn fps(&self) -> f64 {
        self.smoothed_fps
    }
}

pub fn debug_clock_if_enabled() -> Option<FrameClock> {
    if cfg!(debug_assertions) && std::env::var("HYPRULER_DEBUG").is_ok() {
        Some(FrameClock::new())
    } else {
        None
    }
}
