---
name: geo-assets
description: Download and process lunar geo assets (DEMs, ortho/slope/shade maps, normal maps) with lunco-assets — Assets.toml entries, ROI cropping, terrain layer wiring in USD, quality presets, bake keys. Use when adding a terrain site to a twin, baking layer maps, or debugging the asset pipeline.
---

# Geo assets: download & process lunar terrain for a Twin

The pipeline is `crates/lunco-assets` in this repo. Pure Rust — no GDAL.
Sources may be **GeoTIFF or PDS3 `.IMG`** (attached or detached `.LBL`;
`src/pds_img.rs`); polar-stereographic products are refused loudly (the crop
affine is equirectangular-only). Worked example twin with 24 authored
entries: `~/Documents/models/summer_space_school` (its
`scripts/fetch_territory.sh` wraps everything below).

## Quick commands (run from this repo's root)

```bash
cargo run -p lunco-assets -- list     --twin <TWIN>
cargo run -p lunco-assets -- download --twin <TWIN>            # ALL entries (can be GBs)
cargo run -p lunco-assets -- download --twin <TWIN> -a <key>   # one entry
cargo run -p lunco-assets -- process  --twin <TWIN> -a <key> --quality coarse|good
```

`<TWIN>` = a folder holding `Assets.toml` + `twin.toml`. `-a <key>` = the
`[section]` name in its Assets.toml. `--quality coarse` quarters
`target_resolution` (floor 64) for a seconds-fast quick-start bake; re-run
with `good` (default) for full res.

## Where files live (cache resolution)

- **Shared cache** — `LUNCOSIM_CACHE` env → OS cache dir
  (`~/.cache/lunco` on Linux) → CWD `.cache`. The worktrees pin
  `LUNCOSIM_CACHE` to ONE absolute workspace-level `.cache/` in their
  `.cargo/config.toml`, so every worktree and twin shares a single pool of
  regenerable data (MSL, textures, ephemeris, downloaded sources).
- **Raw downloads**: entries without `dest` (the norm) land in
  `<cache>/sources/<sha256(url)[..16]>/<basename>` — one download per URL,
  reused by every entry, twin, and worktree (`apollo15_dtm` and
  `apollo15_normal` share one file). Author `dest` only when a file must
  sit at a specific path (twin-relative under `--twin`, cache-relative
  otherwise).
- **Baked outputs** (`output_root = "twin"`): inside the twin at `output`,
  where the scene's `demSource`/layer attrs expect them. Per-twin, always.

## Process kinds (in `[key.process]`)

| kind | Input | Output |
|---|---|---|
| `dem` | DTM (GeoTIFF/.IMG) | `<output>/materials/textures/heightmap.tif` — square float32, georef in tags. `output` is a FOLDER; scenes reference it as `demSource = @terrain/<site>@` |
| `map` | co-registered raster (ortho `.IMG`, `_SHADE`/`_SLOPE`/`_CLRGRAD` `.TIF`) | 8-bit RGB PNG at `output` (a FILE). Gray sources get a 1–99 percentile stretch |
| `normalmap` | DTM | world-space normal PNG (`RGB = n*0.5+0.5`, `n = normalize(-dh/dx, 1, -dh/dz)` — the `terrain_layered.wgsl` decode) |
| `texture` | any image | resized PNG (non-geo default) |
| `gltf` | .glb | Bevy-clean .glb (needs npx) |

Shared ROI fields: `center_lat`, `center_lon`, `window_m`,
`target_resolution = [n, n]`, `pixel_scale_m`, `src_min/max_lat`,
`src_min/max_lon`, `frame = "MOON_ME"`, `output_root = "twin"`.

## Adding a new territory to a twin

1. Find the product: `https://data.lroc.im-ldi.com/lroc/view_rdr/NAC_DTM_<SITE>`;
   files under `https://pds.lroc.im-ldi.com/data/LRO-L-LROC-5-RDR-V1.0/LROLRC_2001/DATA/SDP/NAC_DTM/<SITE>/`.
2. Read its `.LBL`: `MAP_PROJECTION_TYPE` (EQUIRECTANGULAR → processable;
   POLARSTEREOGRAPHIC → download-only entry, no `[*.process]`),
   `MAP_SCALE` → `pixel_scale_m`, `MIN/MAXIMUM_LATITUDE` +
   `EASTERNMOST/WESTERNMOST_LONGITUDE` → the four `src_*` fields.
   **Label longitudes are 0–360 °E — author `center_lon` in the same
   convention.** Never trust `CENTER_LONGITUDE` (body-frame quirk).
3. Pick `center_lat/lon` (the POI), `window_m` (scene size),
   `target_resolution ≈ window_m / native m-per-px` (square).
4. `sha256 = ""` on first download → the tool prints the hash; paste it in.
5. PDS3 `.IMG` sources declare extent/scale in their label — `src_*` may be
   omitted (an authored manifest extent wins when all four are set).

## Wiring maps as terrain layers (USD)

On a Terrain prim (live example:
`summer_space_school/sim/scenes/traverse.usda`):

```usda
custom string lunco:terrain:layer:albedo:map  = "terrain/<site>/materials/textures/ortho.png"
custom float  lunco:terrain:layer:albedo:weight = 1.0
custom string lunco:terrain:layer:normal:map  = "terrain/<site>/materials/textures/normal.png"
custom float  lunco:terrain:layer:normal:weight = 0.5
custom string lunco:terrain:layer:mineral:map = "terrain/<site>/materials/textures/slope.png"
custom float  lunco:terrain:layer:mineral:weight = 0.0   # raise for a classification drape
```

Paths are twin-root-relative strings, read by `read_authored_layer_maps`
(`lunco-sandbox`). Roles: `albedo`, `mineral`, `surface` (packed R=rough
G=AO B=rockDens), `normal`. They bind on the static-mesh path
(`terrain_layered.wgsl`); streamed LOD tiles derive normal/AO from the DEM
at runtime. Future direction (UsdShade node graph, unlit overlays): the
twin's `18_VIZ_NODEGRAPH_DESIGN.md`.

## Caching & staleness

- Downloads skip when the resolved file exists with matching `sha256`.
- Bakes stamp a `.bakekey` =
  `sha256(source bytes ‖ effective config ‖ PIPELINE_VERSION)` beside each
  output; a matching stamp skips the bake before the expensive decode.
  Changing the source, ROI, `--quality`, or bumping `PIPELINE_VERSION`
  (`src/process.rs`) rebakes exactly what changed. Never time-based.
- Baked artifacts are gitignored in twins by policy
  (`terrain/*/materials/`, `.bakekey` stamps) — never commit them; raw
  downloads never enter the twin at all.

## Gotchas

- `*_50CM`/`*_2M` `.IMG` companions are ORTHOPHOTOS (brightness), never
  elevation — `kind = "map"`, never `kind = "dem"`.
- LROC elevations are metres vs the **1737.4 km** sphere; the engine
  registry uses 1737.0 km (~400 m bias — open issue).
- Heights are absolute body-datum metres: prims on a surface must be
  authored at the DEM's own elevation.
- The runtime DEM reader requires square rasters; keep the scene's
  `windowM`/`targetRes` in step with the manifest ROI.
- Optional QGIS/GDAL extras (custom-sun hillshade, slope ramps, contours):
  the twin's `scripts/make_maps_qgis.sh` — needs `gdaldem` on PATH.
