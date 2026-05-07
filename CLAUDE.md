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
  lib.rs             - Library root, re-exports modules so benches/tests can use them
  wayland_handlers.rs - WaylandApp struct and Wayland protocol handlers; delegates per-frame drawing to render::compose_frame
  render.rs          - Pure CPU composition (compose_frame) — no Wayland deps, decoupled for benching
  capture.rs         - Focused monitor detection (hyprctl) and screen capture (wlr-screencopy)
  edge_detection.rs  - Edge detection (luminance-based boundary finding)
  ui.rs              - Drawing with tiny-skia (lines, crosshair, labels, rectangles)

benches/
  draw.rs            - Criterion bench for compose_frame at 1080p and 4K

tests/
  alloc_budget.rs    - Per-frame allocation regression test (CI gate)
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

## Benchmarks

The per-frame composition runs on the drag hot path (every vsync, per dirty monitor) and dominates perceived lag. To make it measurable and to catch future regressions (e.g. an accidental `Vec` clone in the inner loop), the CPU composition is split out as a pure function `render::compose_frame(canvas, cached_pixmap, screenshot, overlay, font)` with no Wayland deps. `wayland_handlers` calls into it; benches and tests can call it directly.

Run with:

```bash
just bench
```

`benches/draw.rs` builds synthetic 1080p and 4K screenshots, calls `compose_frame` in a drag-state configuration, and uses Criterion to detect statistical regressions across runs. Add new bench cases here when changing the rendering path; aim to keep `compose_frame` allocation-free on steady state (the cached `Pixmap` is reused, the canvas is borrowed).

### Allocation-budget CI gate

In order to prevent performance regression, we have a test that measures per-frame allocations (a deterministic, hardware-independent number) and asserts they stay under generous thresholds. Run locally:

```bash
just test-perf   # cargo test --test alloc_budget --release
```

How it works: an integration-test binary installs a `CountingAllocator` as `#[global_allocator]` that tracks bytes and allocation count. The test runs `compose_frame` in a 4K drag configuration 100 times, samples the counters before/after the loop (so test setup and warm-up don't pollute the measurement), and asserts both budgets. As of writing the steady-state cost is ~48 KiB / 51 allocs per frame; budgets are set at ~5× that. Tracks both axes because a bytes-only budget would miss death-by-many-small-allocations and a counts-only budget would miss a single 41 MB pixel-buffer clone — they cover different regression modes.

`.github/workflows/perf.yml` runs this test on every PR. A failure points at exactly the kind of regression that surfaced in the multi-monitor PR review (a `Screenshot::clone()` per frame inside `draw_monitor`). Tighten the budgets if you intentionally reduce per-frame allocations; loosen them only with a clear reason in the commit message.

`src/lib.rs` exists primarily so the bench (and any future integration tests) can import crate modules; `main.rs` uses the same library entrypoints rather than redeclaring modules.

### Debug FPS overlay

To check performance in real Wayland environments, we can enable an FPS overlay with the `release-debug` Cargo profile and run with `HYPRULER_DEBUG=1`:

```bash
just build-debug
just start-debug
```

A debug-profile build (`cargo run`) would be slow enough that FPS measurements wouldn't reflect real user experience; the `release-debug` profile keeps optimizations.

A small "FPS: XX" label is drawn in the top-left of the overlay, and per-frame timings are logged to stderr in the form `[hypruler-debug] dt=14.20ms fps=70.4`. The overlay is gated on both `cfg!(debug_assertions)` and the env var, so production release builds carry zero overhead and have no codepath to trigger it. Implementation: `FrameClock` (EMA-smoothed) ticks once per `frame()` callback in `wayland_handlers.rs`; the smoothed value is threaded into `FrameOverlay::debug_fps` and rendered by `compose_frame`. FPS is bounded by the compositor's vsync rate, so the useful signal is *drops* below it (e.g. 60 → 22 during a 4K drag).

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
