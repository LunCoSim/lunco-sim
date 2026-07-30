# LunCoSim (LCS) app icons — night theme

The three SVGs in `svg/` are the source of truth. Run
`scripts/generate_icons.sh` to derive Linux PNGs, Windows ICO/PNGs, and the
macOS ICNS/PNGs. Derived raster and platform files are ignored by Git so icon
changes are made once, in SVG, and CI regenerates the complete bundle.

Background #0A0A0F · letters #FFFFFF · S-glyph #4DE3F5. Mark only, no wordmark. All three platforms are full-bleed: the tile fills the canvas edge to edge, the mark occupies 72% of the canvas width, and only the corner radius differs per platform (macOS 26%, Windows 6%, Linux 24%).

## macOS
- `macos/luncosim.icns` — drop into `YourApp.app/Contents/Resources/` and set
  `CFBundleIconFile = luncosim` in Info.plist. Contains 16–1024 px, @1x/@2x.
- `macos/luncosim.iconset/` — source set for `iconutil -c icns luncosim.iconset`.
  NOTE: this export writes retina files as `icon_16x16-2x.png`; rename `-2x` to `@2x`
  before running iconutil (`for f in *-2x.png; do mv "$f" "${f/-2x/@2x}"; done`).
- `macos/png/` — flat PNGs (16, 32, 64, 128, 256, 512, 1024).
- Geometry: full-bleed squircle, r = 26% (Tahoe masks the icon itself — no baked margin).

## Windows
- `windows/luncosim.ico` — multi-resolution 16/24/32/48/64/128/256, 32-bit PNG frames.
  Use as the executable icon (`ICON` resource / Electron `icon:`) and shortcut icon.
- `windows/png/` — individual PNGs if a build tool wants them separately.
- Geometry: full-bleed square, r = 6%.

## Linux
- `linux/hicolor/<size>x<size>/apps/luncosim.png` — install into
  `/usr/share/icons/hicolor/` (or `~/.local/share/icons/hicolor/`), then
  `gtk-update-icon-cache`.
- `svg/lcs-night-linux.svg` — scalable version for
  `/usr/share/icons/hicolor/scalable/apps/luncosim.svg`.
- `.desktop` file: `Icon=luncosim`.
- Geometry: full-bleed rounded square, r = 24%.

## Vector sources
`svg/lcs-night-mac.svg`, `svg/lcs-night-win.svg`, `svg/lcs-night-linux.svg` — 1024 px
square, per-platform shape already applied. Re-raster from these for any extra size.

## Electron / Tauri quick reference
- Electron builder: `mac.icon: macos/luncosim.icns`, `win.icon: windows/luncosim.ico`,
  `linux.icon: linux/hicolor`.
- Tauri: `bundle.icon = ["icons/windows/png/luncosim-32.png", "icons/macos/png/luncosim-128.png", "icons/macos/png/luncosim-256.png", "icons/windows/luncosim.ico", "icons/macos/luncosim.icns"]`.
