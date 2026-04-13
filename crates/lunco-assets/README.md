# lunco-assets

Unified asset management for LunCoSim — the single source of truth for cache directory resolution, versioned downloads, texture processing, and cross-platform asset loading.

## What This Crate Does

- **Resolves the shared cache directory** — all git worktrees point to the same cache location
- **Downloads external assets** from `Assets.toml` declarations with SHA-256 verification
- **Processes textures** — resize/convert source images (JPEG, PNG, TIFF, SVG → PNG)
- **Eliminates hardcoded paths** — no more scattered `.cache/` and `assets/` strings

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
# crates/lunco-modelica/Assets.toml

[msl]
name = "Modelica Standard Library"
version = "4.1.0"
url = "https://github.com/modelica/ModelicaStandardLibrary/archive/refs/tags/v4.1.0.tar.gz"
dest = "msl"
```

## Cache Directory

All worktrees share the same cache directory via `LUNCOSIM_CACHE` in the workspace root `.cargo/config.toml`:

```
luncosim-workspace/
├── .cargo/config.toml    ← sets LUNCOSIM_CACHE
├── .cache/               ← SHARED across all worktrees
│   ├── textures/
│   │   ├── earth_source.jpg   (downloaded)
│   │   ├── earth.png          (processed)
│   │   ├── moon_source.tif    (downloaded)
│   │   └── moon.png           (processed)
│   ├── msl/
│   │   └── 4.1.0/             (extracted library)
│   └── ephemeris/             (runtime-generated CSVs)
```

## Workflow

```
1. download  → 2. process  →  3. use
   (lunco-assets) (lunco-assets) (Bevy at runtime)
   cache/                  cache/processed/
   earth_source.jpg        textures/earth.png
   moon_source.tif         textures/moon.png
   msl/4.1.0/
```

## Testing

```bash
cargo test -p lunco-assets
```
