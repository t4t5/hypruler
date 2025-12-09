# PixelSnap for Linux

A minimal screen measurement tool for Hyprland/Wayland, inspired by PixelSnap 2 for macOS.

## How it works

- Uses Wayland layer-shell to create a transparent fullscreen overlay on the `overlay` layer
- Captures mouse input for click-and-drag measurement
- Constrains movement to horizontal OR vertical (auto-detected based on drag direction)
- Renders with tiny-skia (software rendering) to a shared memory buffer
- Supports HiDPI displays by rendering at physical pixel resolution and using `set_buffer_scale`

## Usage

1. Launch via keybind (add to `~/.config/hypr/hyprland.conf`):
   ```
   bind = $mainMod, M, exec, /path/to/pixelsnap
   ```
2. Click and drag to measure
3. Release to see final measurement
4. Press Escape or click again to exit

## Building

```bash
cargo build --release
# Binary at target/release/pixelsnap (~2.4MB)
```

## Dependencies

- `smithay-client-toolkit` - Wayland client library with layer-shell support
- `tiny-skia` - 2D rendering (lines, shapes)
- `fontdue` - Font rasterization for labels
- Font: `/usr/share/fonts/noto/NotoSans-Regular.ttf` (embedded at compile time)
