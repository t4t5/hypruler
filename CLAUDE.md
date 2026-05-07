# Hypruler

A screen measurement tool for Hyprland/Sway (wlroots-based compositors), inspired by PixelSnap 2 for macOS.

## How it works

1. **Screen Capture**: On launch, detects focused monitor via `hyprctl`, then captures that screen using `zwlr_screencopy_manager_v1` protocol
2. **Edge Detection**: Pre-computes luminance values for fast edge detection (threshold-based)
3. **Overlay**: Creates a fullscreen layer-shell surface on the `overlay` layer showing the frozen screenshot
4. **Measurement**: Two modes:
   - **Auto mode**: Move cursor to automatically detect edges and show measurement lines
   - **Manual mode**: Click and drag to draw a rectangle; edges auto-snap to nearby content on release
5. **Rendering**: Uses tiny-skia for drawing lines/labels/rectangles, with pre-converted BGRA data for fast background rendering. Redraws are throttled via Wayland frame callbacks to match display refresh rate

## Architecture

```
src/
  main.rs            - Entry point (minimal - just connects and runs event loop)
  lib.rs             - Library root, re-exports modules so main.rs can use them via `use hypruler::...`
  wayland_handlers.rs - WaylandApp struct and Wayland protocol handlers; delegates per-frame drawing to render::compose_frame
  render.rs          - Pure CPU composition (compose_frame), separated from Wayland I/O
  capture.rs         - Focused monitor detection (hyprctl) and screen capture (wlr-screencopy)
  edge_detection.rs  - Edge detection (luminance-based boundary finding)
  ui.rs              - Drawing with tiny-skia (lines, crosshair, labels, rectangles)
```

- **Screen capture** at physical resolution (e.g., 2880x1920 for HiDPI)
- **HiDPI support**: Fractional scaling via `wp_fractional_scale_v1` and `wp_viewporter` protocols. Dimensions displayed in logical pixels (physical pixels ÷ scale factor)
- **Pre-computed data** at startup:
  - `luminance[]` - grayscale values for edge detection
  - `bgra_data[]` - screenshot pre-converted to Wayland's buffer format
- **Edge detection** scans from cursor position in 4 directions, looking for luminance changes > threshold
- **Rectangle snapping** samples every pixel along each drawn edge, scanning inward to find content boundaries
- **Crosshair cursor** via `wp_cursor_shape_v1` protocol

## Usage

1. Launch via keybind (add to `~/.config/hypr/hyprland.conf`):
   ```
   bind = $mainMod, M, exec, /path/to/hypruler
   ```
2. Move cursor to measure between detected edges (auto mode)
3. Click and drag to draw a rectangle that snaps to content edges (manual mode)
4. Click without dragging to clear the rectangle
5. Dimensions shown as `{width} x {height}` centered on large rectangles, or below small rectangles
6. Press any key to exit

## Building

```bash
cargo build --release
# Binary at target/release/hypruler
```

## Performance

The per-frame composition runs on the drag hot path (every vsync) and dominates perceived lag. The CPU composition is split out as a pure function `render::compose_frame(canvas, cached_pixmap, screenshot, overlay, font)` with no Wayland deps; `wayland_handlers` calls into it.

To check performance in real Wayland environments, we can enable an FPS overlay with the `release-debug` Cargo profile and run with `HYPRULER_DEBUG=1`:

```bash
just build-debug
just start-debug
```

A debug-profile build (`cargo run`) would be slow enough that FPS measurements wouldn't reflect real user experience; the `release-debug` profile keeps optimizations.

A small "FPS: XX" label is drawn in the top-left of the overlay, and per-frame timings are logged to stderr in the form `[hypruler-debug] dt=14.20ms fps=70.4`.

The overlay is gated on both `cfg!(debug_assertions)` and the env var, so production release builds carry zero overhead and have no codepath to trigger it. Implementation: `FrameClock` (EMA-smoothed) ticks once per `frame()` callback in `wayland_handlers.rs`; the smoothed value is threaded into `FrameOverlay::debug_fps` and rendered by `compose_frame`. FPS is bounded by the compositor's vsync rate, so the useful signal is *drops* below it (e.g. 60 → 22 during a 4K drag).

## Dependencies

- `smithay-client-toolkit` - Wayland client library with layer-shell support
- `wayland-protocols-wlr` - wlroots screencopy protocol
- `wayland-protocols` - cursor shape, fractional scale, and viewporter protocols
- `tiny-skia` - 2D rendering (lines, shapes)
- `fontdue` - Font rasterization for labels
- `memmap2` / `rustix` - Shared memory for screen capture
- `serde` / `serde_json` - Parsing hyprctl JSON output for monitor detection
- Font: System sans-serif font discovered via `fc-match` at runtime

## Limitations

- Only works on wlroots-based compositors (Hyprland, Sway, etc.)
- Multi-monitor detection requires Hyprland (`hyprctl`); on other compositors falls back to first output
- Edge detection is luminance-based, may not detect all UI boundaries perfectly
