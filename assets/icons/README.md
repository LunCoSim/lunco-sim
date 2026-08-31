# LunCoSim (LCS) app icons — night theme

The three SVGs in `svg/` are the source of truth. `scripts/build_native.sh`
asks the Rust build script to derive the platform-native outputs required by
the package consumer, so a clean GitHub runner does not depend on ImageMagick.
Derived files are generated under `target/package-icons/` and staged into the
native package; icon changes are made once in SVG and regenerated for every
target package.

Background #0A0A0F · letters #FFFFFF · S-glyph #4DE3F5. Mark only, no wordmark. All three platforms are full-bleed: the tile fills the canvas edge to edge, the mark occupies 72% of the canvas width, and only the corner radius differs per platform (macOS 26%, Windows 6%, Linux 24%).

## macOS
- The build generates a 16–1024 px `iconset` and converts it with macOS
  `iconutil` to the `.icns` passed to Velopack. Velopack places it in
  `YourApp.app/Contents/Resources/` and references it from `Info.plist`.
- Geometry: full-bleed squircle, r = 26% (Tahoe masks the icon itself — no baked margin).

## Windows
- The build generates a multi-resolution 16/24/32/48/64/128/256 `.ico` with
  32-bit PNG frames.
  The Windows build embeds it as the PE `ICON` resource and Velopack consumes
  the same file for Setup, Update, and shortcut branding.
- Geometry: full-bleed square, r = 6%.

## Linux
- The build generates 16/24/32/48/64/128/256 hicolor PNGs, which are staged
  into the AppImage/AppDir icon tree.
- `svg/lcs-night-linux.svg` — scalable version for
  `/usr/share/icons/hicolor/scalable/apps/luncosim.svg`.
- Direct archives and Velopack AppImages use `luncosim` as the desktop icon
  name and window identity. Velopack emits the single root `luncosim.desktop`
  entry from that package identity; the staging directory must not add another
  desktop entry. The final AppImage check requires the root filename, icon,
  `Exec`, `.DirIcon`, and `StartupWMClass` to agree.
- Geometry: full-bleed rounded square, r = 24%.

## Vector sources
`svg/lcs-night-mac.svg`, `svg/lcs-night-win.svg`, `svg/lcs-night-linux.svg` — 1024 px
square, per-platform shape already applied. Re-raster from these for any extra size.

Other packaging consumers should use the generated artifact from
`target/package-icons/<binary>-<platform>-<arch>/` rather than introduce a
second icon generator.
