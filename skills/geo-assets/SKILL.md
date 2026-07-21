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
`[section]` name in its Assets.toml. The same entries appear in-app under
Settings ▸ Downloadable data once the twin is open (scanned on open); nothing
downloads without a click there or a CLI run. `--quality coarse` quarters
`target_resolution` (floor 64) for a seconds-fast quick-start bake; re-run
with `good` (default) for full res.

## Where files live (cache resolution)

- **Shared cache** — `LUNCOSIM_CACHE` env → OS cache dir
  (`~/.cache/lunco` on Linux) → CWD `.cache`. The worktrees pin
  `LUNCOSIM_CACHE` to ONE absolute workspace-level `.cache/` in their
  `.cargo/config.toml`, so every worktree and twin shares a single pool of
  regenerable data (MSL, textures, ephemeris, downloaded sources).
- **Twin cache** — `<TWIN>/.cache`. A twin's downloads land beside the twin
  by DEFAULT, so the folder is self-contained: copy it and the data travels,
  delete it and nothing is orphaned. `twin://` reads resolve `<twin>/<rel>`
  first, then `<twin>/.cache/<rel>`.
- **`shared = true`** on an entry sends it to the global pool instead
  (`<cache>/sources/<sha256(url)[..16]>/<basename>`) — one download per URL,
  reused by every twin and worktree. Use it for multi-GB upstream products
  several twins reuse: the LROC DTM entries all set it, so `apollo15_dtm` and
  `apollo15_normal` still share one file.
- **Raw downloads**: entries without `dest` land in
  `<owner cache>/sources/<url-hash>/<basename>` — owner being the twin
  (`--twin`) or the shared cache (crate manifest). Author `dest` only when a
  file must sit at a specific path; it is then resolved against that same
  owner cache.
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

Maps bind through a **stock UsdShade Material network** — the only authoring
path. Bind the Terrain prim to a Material, whose surface output connects to a
Shader carrying one `asset inputs:<role>_map` + `float inputs:weight_<role>`
per layer (live example:
`summer_space_school/sim/scenes/traverse.usda`):

```usda
def Xform "Terrain" ( prepend apiSchemas = ["LunCoTerrainAPI"] )
{
    string lunco:assetMode = "layered"
    rel material:binding = </Traverse/Looks/TerrainLook>
    # … dem/overzoom/rocks layers …
}

def Scope "Looks"
{
    def Material "TerrainLook"
    {
        token outputs:surface.connect = </Traverse/Looks/TerrainLook/Surface.outputs:surface>

        def Shader "Surface"
        {
            uniform asset info:wgsl:sourceAsset = @lunco://shaders/terrain_layered.wgsl@
            asset inputs:albedo_map  = @terrain/<site>/materials/textures/ortho.png@
            float inputs:weight_albedo = 1.0
            asset inputs:normal_map  = @terrain/<site>/materials/textures/normal.png@
            float inputs:weight_normal = 0.5
            asset inputs:mineral_map = @terrain/<site>/materials/textures/slope.png@
            float inputs:weight_mineral = 0.0   # raise for a classification drape
        }
    }
}
```

Asset paths are **scene-root-relative** and resolve through `twin://`, so they
travel with the twin. Read by `read_material_network_layer_maps`
(`lunco-sandbox`), which walks `material:binding` → Material →
`outputs:surface.connect` → Shader. Roles: `albedo`, `mineral`, `surface`
(packed R=rough G=AO B=rockDens), `normal`.

- Every `inputs:*` is a live-tunable, journaled knob (networked, undoable) and
  the network is inspectable in usdview/Blender.
- **CONNECTED** map inputs are skipped — a connected port is fed by a producer
  node (doc 18 Tier B), not an authored file.
- `mineral` composites **UNLIT after lighting**, so a slope/classification
  drape stays readable inside shadow — its entire job.
- Layers bind on the static-mesh path (`terrain_layered.wgsl`); streamed LOD
  tiles derive normal/AO from the DEM at runtime, and the derived bake only
  fills slots an authored map left empty.
- For multi-site scenes, author these inputs **inside a terrain variant** —
  see `14_SCENARIO_DESIGN.md` §4a in the twin, and verify with
  `cargo run -p lunco-usd --example variant_probe -- <scene.usda>`.

Roadmap (bake nodes, node-graph editor): the twin's
`18_VIZ_NODEGRAPH_DESIGN.md`.

## Caching & staleness

- Downloads skip when the resolved file exists with matching `sha256`.
- Bakes stamp a `.bakekey` =
  `sha256(source bytes ‖ effective config ‖ PIPELINE_VERSION)` beside each
  output; a matching stamp skips the bake before the expensive decode.
  Changing the source, ROI, `--quality`, or bumping `PIPELINE_VERSION`
  (`src/process.rs`) rebakes exactly what changed. Never time-based.
- Baked artifacts and the twin cache are gitignored by policy
  (`terrain/*/materials/`, `.bakekey` stamps, `.cache/`) — never commit them.

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
