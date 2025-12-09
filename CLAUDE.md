# PixelSnap for Linux

A screen measurement tool for Hyprland/Sway (wlroots-based compositors), inspired by PixelSnap 2 for macOS.

## How it works

1. **Screen Capture**: On launch, captures a screenshot using `zwlr_screencopy_manager_v1` protocol
2. **Edge Detection**: Pre-computes luminance values for fast edge detection (threshold-based)
3. **Overlay**: Creates a fullscreen layer-shell surface on the `overlay` layer showing the frozen screenshot
4. **Measurement**: As you move the cursor, automatically detects edges and draws measurement lines between them
5. **Rendering**: Uses tiny-skia for drawing lines/labels, with pre-converted BGRA data for fast background rendering

## Architecture

```
src/
  main.rs      - App state, Wayland handlers, entry point
  capture.rs   - Screen capture via wlr-screencopy protocol
  render.rs    - Edge detection + drawing with tiny-skia
```

- **Screen capture** at physical resolution (e.g., 2880x1920 for HiDPI)
- **Pre-computed data** at startup:
  - `luminance[]` - grayscale values for edge detection
  - `bgra_data[]` - screenshot pre-converted to Wayland's buffer format
- **Edge detection** scans from cursor position in 4 directions, looking for luminance changes > threshold
- **Crosshair cursor** via `wp_cursor_shape_v1` protocol

## Usage

1. Launch via keybind (add to `~/.config/hypr/hyprland.conf`):
   ```
   bind = $mainMod, M, exec, /path/to/pixelsnap
   ```
2. Move cursor to measure between detected edges
3. Dimensions shown as `{width} x {height}` near cursor
4. Press any key or click to exit

## Building

```bash
cargo build --release
# Binary at target/release/pixelsnap
```

## Dependencies

- `smithay-client-toolkit` - Wayland client library with layer-shell support
- `wayland-protocols-wlr` - wlroots screencopy protocol
- `wayland-protocols` - cursor shape protocol
- `tiny-skia` - 2D rendering (lines, shapes)
- `fontdue` - Font rasterization for labels
- `memmap2` / `rustix` - Shared memory for screen capture
- Font: `/usr/share/fonts/noto/NotoSans-Regular.ttf` (embedded at compile time)

## Limitations

- Only works on wlroots-based compositors (Hyprland, Sway, etc.)
- Edge detection is luminance-based, may not detect all UI boundaries perfectly
