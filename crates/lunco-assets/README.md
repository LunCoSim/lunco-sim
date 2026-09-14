# lunco-assets

Dataset provisioning and offline asset processing for LunCoSim. The lightweight
identity, URI, storage, discovery, and embedded-source APIs live in
[`lunco-assets-core`](../lunco-assets-core); this package is intentionally the
heavy application/CLI boundary.

## What This Crate Does

- **Downloads external assets** from `Assets.toml` declarations with SHA-256 verification
- **Processes textures** — resize/convert source images (JPEG, PNG, TIFF, SVG → PNG)
- **Processes DEM, map, normal-map, PDS3, GeoTIFF, and glTF products** in the
  native offline pipeline
- **Owns the explicit dataset lifecycle** — declaration, user-authorised
  download, processing, cancellation, and installed status

## Package boundary

Use `lunco-assets-core` for normal runtime asset access:

- canonical `lunco://` and `twin://` identities and readers
- cache and Twin-root resolution
- embedded Modelica, mission, tutorial, and Rhai sources
- discovery/catalog data and the shared asset-source registration plugin

Use `lunco-assets` only where the application or tool explicitly provisions
datasets or runs the native processing pipeline. Keeping those consumers out of
the core package prevents archive, HTTP, raster, SVG, GeoTIFF, and `npx`
dependencies from propagating through every asset-reading crate.

## CLI Usage

```bash
# Download all external assets declared in workspace Assets.toml files
cargo run -p lunco-assets -- download

# Download for a specific crate only
cargo run -p lunco-assets -- download -p lunco-celestial

# Process downloaded textures (resize, convert)
cargo run -p lunco-assets -- process

# List asset status across all crates
cargo run -p lunco-assets -- list
```

## Assets.toml Format

Each crate declares its own `Assets.toml`, mirroring `Cargo.toml`:

```toml
# crates/lunco-celestial/Assets.toml

[earth]
name = "Earth Blue Marble (NASA Next Generation)"
url = "https://eoimages.gsfc.nasa.gov/images/..."
dest = "textures/earth_source.jpg"
# sha256 = ""  # fill after first download for integrity

[earth.process]
target_resolution = [4096, 2048]
output = "textures/earth.png"

[moon]
name = "Moon Color Map (NASA CGI Moon Kit)"
url = "https://svs.gsfc.nasa.gov/vis/a000000/a004700/a004720/lroc_color_16bit_srgb_4k.tif"
dest = "textures/moon_source.tif"

[moon.process]
target_resolution = [4096, 2048]
output = "textures/moon.png"
```

```toml
# crates/lunco-modelica-ui/Assets.toml

[msl]
name = "Modelica Standard Library"
version = "4.1.0"
url = "https://github.com/modelica/ModelicaStandardLibrary/archive/refs/tags/v4.1.0.tar.gz"
dest = "msl"
```

## Cache Directory

All worktrees and Twins share the OS-global cache returned by `cache_dir()`:

```
~/.cache/lunco/            # Linux; OS equivalent on macOS/Windows
├── textures/               (downloaded and processed)
├── msl/                    (extracted library)
└── ephemeris/              (runtime-generated CSVs)
```

The user configuration path is owned by `lunco-settings`; it is not another
asset-cache path. `settings.json` contains the shared `DownloadSettings`
section. The CLI and every runtime downloader use that section for the same
bounded transport policy: `max_attempts` is the total request count, and
retry delays grow exponentially from the configured initial delay up to the
configured cap. The in-app Data & libraries panel is the settings editor.

## Workflow

```
1. download  → 2. process  →  3. use
   (lunco-assets) (lunco-assets) (lunco-assets-core / Bevy at runtime)
   global cache/           global cache/
   earth_source.jpg        textures/earth.png
   moon_source.tif         textures/moon.png
   msl/4.1.0/
```

## Testing

```bash
cargo test -p lunco-assets
```

The low-level asset identity and source tests are in the lightweight package:

```bash
cargo test -p lunco-assets-core
```
