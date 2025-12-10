<h1 align="center">hypruler</h1>

<p align="center">
  📏 Measure anything on your screen. Built for Linux + Hyprland.
</p>

<p align="center">
  <img src="assets/demo.gif" alt="Hypruler demo" width="500"/>
</p>

---

## Installation

### Pre-built AUR binary (fastest)

```bash
yay -S hypruler-bin
```

### Building from source

Using the AUR:

```bash
yay -S hypruler
```

Or manually from GitHub:

```bash
git clone https://github.com/t4t5/hypruler.git
cd hypruler
cargo build --release
cargo install --path .
```

## Usage

Add a keybind to your Hyprland config (`~/.config/hypr/hyprland.conf`):

```
bind = $mainMod, M, exec, hypruler
```

Or if you're using Omarchy (`~/.config/hypr/bindings.conf`):

```
bindd = SUPER, M, hypruler, exec, hypruler
```

## Requirements

- wlroots-based compositor (Hyprland, Sway, etc.)
- `wlr-screencopy-unstable-v1` protocol support

## Acknowledgments

Heavily inspired by [PixelSnap](https://pixelsnap.com/) for macOS.

## License

MIT
