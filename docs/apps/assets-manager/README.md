# Assets Manager

The **Assets Manager** (`lunco-assets`) is a command-line tool for managing external assets (textures, MSL, models) used by LunCoSim. It handles downloading, SHA-256 verification, and processing (resizing/conversion).

## What it does

- **Unified Download**: Reads `Assets.toml` files from all crates and fetches remote assets into a shared cache.
- **Integrity Check**: Verifies downloads against SHA-256 hashes.
- **Texture Processing**: Converts and resizes raw textures (JPEG, TIFF, SVG) into optimized PNGs for the engine.
- **Cache Management**: Ensures all git worktrees share a single cache directory, avoiding redundant downloads.
- **Shared retry policy**: Uses `download` in the one `settings.json` owned by `lunco-settings`; the same total-attempt and exponential-backoff policy is used by the app and updater.

## CLI Usage

```bash
cargo run -p lunco-assets -- <ACTION> [FLAGS]
```

### Actions

| Action | Description |
|---|---|
| `download` | Download all workspace assets. |
| `list` | List all workspace assets and their status. |
| `process` | Process downloaded assets (e.g., texture conversion). |

### Flags

| Flag | Description |
|---|---|
| `-p, --package <NAME>` | Target a specific crate (e.g., `lunco-modelica`). |
| `-a, --asset <KEY>` | Download/Process a single asset by its key. |
| `--workspace-root <PATH>` | Override the workspace root directory. |

## Cache Layout

Assets are stored in the OS-global cache returned by `lunco_assets::cache_dir()` (typically `~/.cache/lunco/` on Linux):

```
.cache/
├── textures/
│   ├── earth_source.jpg   (raw download)
│   └── earth.png          (processed for engine)
├── msl/
│   └── 4.1.0/             (Modelica Standard Library)
└── models/                (External glTF/USD assets)
```

Retry settings are stored separately from this regenerable cache in
`lunco_settings::settings_path()` (`~/.config/lunco/settings.json` on Linux,
unless `LUNCOSIM_CONFIG` is set). Edit them in Settings → Data & libraries or
let the CLI read the same values; there is no second CLI-specific retry file.

## See also

- [**Asset IO Architecture**](../../architecture/40-asset-io.md) — how the engine loads these assets at runtime.
