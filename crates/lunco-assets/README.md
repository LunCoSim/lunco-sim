# lunco-assets

Application-edge dataset provisioning for LunCoSim. The package is deliberately
small: the manifest/state contract is in
[`lunco-assets-datasets`](../lunco-assets-datasets), byte transport is in
[`lunco-assets-transport`](../lunco-assets-transport), manifest-aware download
and installation is in [`lunco-assets-download`](../lunco-assets-download), and
native raster/glTF baking is in
[`lunco-assets-processing`](../lunco-assets-processing). Lightweight asset
identity, URI, storage, discovery, and embedded-source APIs remain in
[`lunco-assets-core`](../lunco-assets-core).

## What This Crate Does

- **Composes the explicit dataset lifecycle** — user-authorised download,
  processing, cancellation, and installed status
- **Provides the Bevy worker boundary** used by the GUI application
- **Provides the `lunco-assets` CLI**, which composes the download and processing
  crates without making ordinary runtime asset readers link them

## Package boundary

Use `lunco-assets-core` for normal runtime asset access:

- canonical `lunco://` and `twin://` identities and readers
- cache and Twin-root resolution
- embedded Modelica, mission, tutorial, and Rhai sources
- discovery/catalog data and the shared asset-source registration plugin

Use `lunco-assets` only where the application explicitly provisions datasets.
Use the smaller packages directly when the owner is narrower:

- `lunco-assets-datasets` for manifests, registry state, artifact identity, and
  typed request/cancel events; it is safe for headless/browser-safe readers.
- `lunco-assets-transport` for shared native HTTP retry/resume primitives.
- `lunco-assets-download` for manifest verification, archive extraction, and
  atomic installation without Bevy.
- `lunco-assets-processing` for native decode, raster math, and baking. Its
  `ProcessorRegistry` is the extension seam for new heavy processors.

This dependency direction keeps HTTP/archive/raster/glTF tool dependencies out
of ordinary runtime readers and makes changes to worker policy, transport, or
manifest state recompile only the affected layer.

## Extensible baking contract

`Assets.toml` keeps the shared fields needed for cache identity and atomic
commit (`kind`, `output`, `output_root`) and flattens additional processor
parameters into `process.parameters`. A registered native processor receives
the complete `ProcessConfig`; Rust owns decoding, math, cancellation, staging,
and commit, while Rhai chooses and sequences authored dataset policy. New
processors register a `ProcessorSpec` with the explicit sidecars they publish;
they do not add another central `match`, cache key, or commit path.

Built-in kinds are `texture`, `gltf`, `dem`, `map`, `albedo`, and `normalmap`.
The reusable Rhai policy library is `assets::scripting::tools::assets` and
provides dataset listing, selection, request, cancellation, and recommended
dataset orchestration. It does not perform I/O or duplicate registry state.

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
kind = "texture"
target_resolution = [4096, 2048]
output = "textures/earth.png"

[moon]
name = "Moon Color Map (NASA CGI Moon Kit)"
url = "https://svs.gsfc.nasa.gov/vis/a000000/a004700/a004720/lroc_color_16bit_srgb_4k.tif"
dest = "textures/moon_source.tif"

[moon.process]
kind = "texture"
target_resolution = [4096, 2048]
output = "textures/moon.png"
```

```toml
# crates/lunco-modelica-ui/Assets.toml

[library]
name = "Modelica Standard Library"
version = "4.1.0"
url = "https://github.com/modelica/ModelicaStandardLibrary/archive/refs/tags/v4.1.0.tar.gz"
dest = "library"
```

## Cache Directory

All worktrees and Twins share the OS-global cache returned by `cache_dir()`:

```
~/.cache/lunco/            # Linux; OS equivalent on macOS/Windows
├── textures/               (downloaded and processed)
├── library/                (extracted source library)
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
1. declare → 2. request → 3. download → 4. process → 5. use
   (Assets.toml) (Rhai/UI/CLI) (download) (processing) (core / Bevy at runtime)
   global cache/           global cache/
   earth_source.jpg        textures/earth.png
   moon_source.tif         textures/moon.png
   library/4.1.0/
```

## Testing

```bash
cargo test -p lunco-assets-transport -p lunco-assets-download \
  -p lunco-assets-processing -p lunco-assets-datasets
```

The application worker composition is checked separately:

```bash
cargo check -p lunco-assets
```

The low-level asset identity and source tests remain in the lightweight package:

```bash
cargo test -p lunco-assets-core
```
