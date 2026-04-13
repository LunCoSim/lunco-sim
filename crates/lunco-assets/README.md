# lunco-assets

Unified asset management for LunCoSim — the single source of truth for cache directory resolution, asset path construction, and cross-platform asset loading.

## What This Crate Does

- **Resolves the shared cache directory** — all git worktrees point to the same cache location, avoiding redundant downloads
- **Provides typed helpers** for every asset subdirectory (textures, ephemeris, modelica, etc.)
- **Eliminates hardcoded paths** — no more scattered `.cache/` and `assets/` strings across the codebase
- **Supports desktop and wasm32** — paths resolve correctly regardless of target platform

## Cache Directory Strategy

All worktrees (`main`, `modelica`, `usd`, etc.) share a **single cache directory** to avoid:
- Redundant downloads of large binary assets (Earth texture ~5MB)
- Duplicated preprocessing output (USD parsing, Modelica compilation)
- Conflicting cache state between branches

### How It Works

Resolution order (set in the workspace root `.cargo/config.toml`):

1. **`LUNCOSIM_CACHE`** environment variable → shared workspace root
2. Fallback to **`./.cache/`** → worktree-local

```toml
# .cargo/config.toml (workspace root)
[env]
LUNCOSIM_CACHE = { value = "/home/rod/Documents/luncosim-workspace/.cache", force = false }
```

```
luncosim-workspace/
├── .cargo/
│   └── config.toml          ← sets LUNCOSIM_CACHE for ALL worktrees
├── .cache/                  ← SHARED cache directory
│   ├── textures/            ← Generated/downloaded textures
│   ├── ephemeris/           ← JPL Horizons CSV ephemeris data
│   ├── remote/              ← HTTP-downloaded assets
│   ├── processed/           ← AssetProcessor output (optimized USD, etc.)
│   ├── modelica/            ← Per-entity Modelica compilation output
│   └── msl/                 ← Modelica Standard Library cache
├── main/                    ← git worktree A
├── modelica/                ← git worktree B
└── usd/                     ← git worktree C (this crate)
```

### Benefits

| Scenario | Without shared cache | With shared cache |
|----------|---------------------|-------------------|
| Switch worktrees | Re-download everything | Assets already there |
| Parallel builds | Duplicate preprocessing | Single cache |
| Texture processing | Per-worktree copies | One shared output |
| Disk usage | N × cache size | 1 × cache size |

## API Reference

### Cache Directory Functions

```rust
use lunco_assets::{cache_dir, textures_dir, ephemeris_dir, modelica_dir, msl_dir, assets_dir};

// Primary: resolve the shared cache root
let cache = cache_dir();          // → /home/rod/Documents/luncosim-workspace/.cache

// Subdirectory helpers (auto-creates directories)
let textures = textures_dir();    // → ~/.cache/textures/
let ephemeris = ephemeris_dir();  // → ~/.cache/ephemeris/
let remote = remote_dir();        // → ~/.cache/remote/
let processed = processed_dir();  // → ~/.cache/processed/
let modelica = modelica_dir();    // → ~/.cache/modelica/
let msl = msl_dir();              // → ~/.cache/msl/

// Development source assets
let assets = assets_dir();        // → ./assets/
```

### Path Construction Helpers

```rust
// Texture loading URI
let path = cached_texture_path("earth.png");
// → "cached_textures://earth.png"

// Ephemeris CSV path
let path = ephemeris_path_for_target("-1024", "2026-04-02_0159", "2026-04-11_0001");
// → ~/.cache/ephemeris/target_-1024_2026-04-02_0159_2026-04-11_0001.csv

// Modelica entity output path
let dir = modelica_entity_dir("Battery");
// → ~/.cache/modelica/Battery/
```

### Custom Cache Subdirectory

```rust
use lunco_assets::cache_subdir;

let my_cache = cache_subdir("my_category");
// → ~/.cache/my_category/ (created if missing)
```

## Usage

### In a new crate

```toml
[dependencies]
lunco-assets = { workspace = true }
```

```rust
use lunco_assets::{cache_dir, textures_dir, assets_dir};

fn load_my_texture() {
    let tex_dir = textures_dir();  // auto-creates if missing
    let tex_path = tex_dir.join("my_texture.png");
    // ...
}
```

### Replace Hardcoded Paths

**Before:**
```rust
let csv_path = format!(".cache/ephemeris/target_{}_{}.csv", id, date);
let model_dir = "assets/models";
```

**After:**
```rust
use lunco_assets::{ephemeris_path_for_target, assets_dir};

let csv_path = ephemeris_path_for_target(id, start_date, end_date);
let model_dir = assets_dir().join("models");
```

## Integration with Bevy Asset System

This crate is the foundation for LunCoSim's asset architecture:

| Component | Status | Description |
|-----------|--------|-------------|
| **Cache resolution** | ✅ Done | `cache_dir()`, `textures_dir()`, etc. |
| **Path construction** | ✅ Done | URI builders, ephemeris paths |
| **Hardcoded path removal** | ✅ Done | All `.cache/` and `assets/` replaced |
| **bevy-cache integration** | 🔜 Future | Manifest-based caching with expiry |
| **Unified AssetSources** | 🔜 Future | Auto-register `cache://`, `user://`, `assets://` |
| **Mesh LOD helpers** | 🔜 Future | `VisibilityRange` builder from USD meshes |
| **Asset catalog** | 🔜 Future | Content index with tags and metadata |

### Current Asset Flow

```
┌─────────────────────────────────────────────────────────┐
│                    DEVELOPMENT TIME                       │
├─────────────────────────────────────────────────────────┤
│  Core Assets (shaders, models)                           │
│    assets/shaders/*.wgsl  → embedded_asset! (wasm32)    │
│    assets/models/*.mo     → include_str! (all targets)   │
│                                                           │
│  Large Binaries (textures, ephemeris)                    │
│    Download once → ~/.cache/textures/                     │
│    All worktrees share → no redundant downloads           │
│                                                           │
│  USD Content (scenes, components)                        │
│    Custom UsdLoader → async asset loading                │
│    Reference resolution via UsdComposer::flatten()        │
│                                                           │
│  Modelica Compilation                                    │
│    Source → ~/.cache/modelica/<entity>/model.mo          │
│    MSL → ~/.cache/msl/                                    │
└─────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────┐
│                    RUNTIME                                │
├──────────────┬──────────────────────────────────────────┤
│   Desktop    │   Web (wasm32)                           │
├──────────────┼──────────────────────────────────────────┤
│ assets://    │ embedded:// (core baked in)              │
│ cache://     │ https:// (download on demand)            │
│ user://      │ browser cache (web_asset_cache feature)  │
└──────────────┴──────────────────────────────────────────┘
```

## Asset Categories

| Category | Location | Preprocess? | Cache? | Examples |
|----------|----------|-------------|--------|----------|
| **Core** | `assets/shaders/`, `assets/models/` | No | wasm32: embedded | WGSL shaders, Modelica models |
| **USD** | `assets/**/*.usda` | Yes (parse) | No | Scenes, components, vessels |
| **Textures** | `.cache/textures/` | Yes (resize) | Shared | Earth, Moon, terrain maps |
| **Ephemeris** | `.cache/ephemeris/` | No | Shared | JPL Horizons CSVs |
| **Modelica** | `.cache/modelica/` | Yes (compile) | Per-entity | Compiled FMUs |
| **Remote** | `.cache/remote/` | Optional | HTTP key | Downloaded assets |
| **User** | TBD | Optional | N/A | Mods, custom scenes |

## Testing

```bash
cargo test -p lunco-assets
```

All public functions have doc tests. Run `cargo doc --open -p lunco-assets` for the full API documentation.

## Migration Checklist (for new crates)

- [ ] Add `lunco-assets = { workspace = true }` to `Cargo.toml`
- [ ] Replace `.cache/` strings with `cache_dir().join(...)` or specific helpers
- [ ] Replace `assets/` strings with `assets_dir().join(...)`
- [ ] Use `cached_texture_path()` for texture loading URIs
- [ ] Use `ephemeris_path_for_target()` for ephemeris CSV paths
- [ ] Use `modelica_entity_dir()` for compilation output
