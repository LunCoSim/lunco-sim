# Assets Manager

The **Assets Manager** (`lunco-assets`) is a command-line tool for managing external assets (textures, MSL, models) used by LunCoSim. It handles downloading, SHA-256 verification, and processing (resizing/conversion).

## What it does

- **Unified Download**: Reads `Assets.toml` files from all crates and fetches remote assets into a shared cache.
- **Integrity Check**: Verifies downloads against SHA-256 hashes.
- **Texture Processing**: Converts and resizes raw textures (JPEG, TIFF, SVG) into optimized PNGs for the engine.
- **Cache Management**: Ensures all git worktrees share a single cache directory, avoiding redundant downloads.

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

Assets are stored in a shared `.cache/` directory (typically at the workspace root or `~/.cache/lunco/` depending on configuration):

```
.cache/
├── textures/
│   ├── earth_source.jpg   (raw download)
│   └── earth.png          (processed for engine)
├── msl/
│   └── 4.1.0/             (Modelica Standard Library)
└── models/                (External glTF/USD assets)
```

## See also

- [**Asset IO Architecture**](../../architecture/40-asset-io.md) — how the engine loads these assets at runtime.
