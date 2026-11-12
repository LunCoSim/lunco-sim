---
name: geo-assets
description: Download and process lunar geo assets (DEMs, stable material albedo, ortho/slope/shade maps, normal maps) with the LunCoSim asset pipeline — Assets.toml entries, ROI cropping, terrain layer wiring in USD, quality presets, bake keys. Use when adding a terrain site to a twin, baking layer maps, or debugging the asset pipeline.
---

# Geo assets: download & process lunar terrain for a Twin

The application pipeline is composed by `crates/lunco-assets`; the heavy native
processors live in `crates/lunco-assets-processing`. Pure Rust — no GDAL.
Sources may be **GeoTIFF or PDS3 `.IMG`**. Spherical polar-stereographic
(north/south) and equirectangular GeoTIFFs use their own CRS and sample
registration. Raw PDS/plain rasters use their declared geographic metadata.
Read [artifact processing](../../docs/architecture/asset-processing.md) for
ordered `kind = "pipeline"` recipes, additional raw sources, declared outputs,
Rhai customization, WESL image kernels, Modelica results, and external stages.
Keep reusable operations native; site parameters and specific policy stay authored.

Wire baked terrain outputs into a scene through LunCoSim's USD document
authoring commands. Open the exact USD source, inspect its layer and target,
apply typed `ApplyUsdOp(s)` or the existing terrain-material planner, save, and
read back the authored value. Do not patch `.usd*` text directly; if the
available command surface cannot author a required standard field, add the
smallest typed operation at the owning USD layer first.

The native texture decoder defaults to a bounded 1 GiB allocation budget.
Users can edit Settings > Data & libraries > Texture preparation, or author
`"assets.texture_decode_max_bytes" = 2147483648` in a Twin's `[settings]`
for 2 GiB. Existing Twin values are editable in the native Twin settings panel.
Precedence is explicit asset `max_decode_bytes`, owning Twin setting, then
application setting. Changes affect the next job; Rebuild reruns cached output.
For a large ordinary texture source that fails decoding with `Memory limit
exceeded`, author a larger bounded positive `max_decode_bytes` in that entry's
`[asset.process]` recipe, then rebuild through native `ProcessDataset`. For the
506 MiB LOLA unsigned TIFF, 1 GiB is the declared decode budget. Preserve
`color_space = "linear"` for elevation and verify the prepared output remains
16-bit. Never retry with unlimited allocation or silently substitute a smaller
source. See the bounded texture decoding section in the processing contract.

For distance-dependent missing tiles after asset readiness, use `render-quality`.
`TerrainLodStatus` describes CPU cover readiness; native GPU buffer layout and
actual drawing still require visual acceptance. The visual metric uses each
node's sampled height envelope in the shared `big_space` terrain frame.

## Quick commands (run from this repo's root)

```bash
cargo run -p lunco-assets -- list     --twin <TWIN>
cargo run -p lunco-assets -- download --twin <TWIN>            # ALL entries (can be GBs)
cargo run -p lunco-assets -- download --twin <TWIN> -a <key>   # one entry
cargo run -p lunco-assets -- process  --twin <TWIN> -a <key> --quality coarse|good
```

`<TWIN>` = a folder holding `Assets.toml` + `twin.toml`. `-a <key>` = the
`[section]` name in its Assets.toml. The same entries appear in-app under
Settings ▸ Data & libraries and the Twin inspector once the Twin is open
(scanned on open). Asset-consuming domains begin only after the asset owner
publishes `TwinAssetMounted`; they must not depend on observer registration
order. If a declared dataset is missing, the interactive app asks
which entries to download before terrain generation; nothing downloads until
the user confirms there or uses a CLI run. Closing a Twin retires its dataset
rows under the shared download/process commit barrier and cancels their
cooperative tasks before another Twin can reuse the authority. A failed mount
or poisoned lifecycle lock is surfaced as an error, never reported as a
successful asset postcondition. `--quality coarse` quarters
`target_resolution` (floor 64) for a seconds-fast quick-start bake; re-run
with `good` (default) for full res.

The consent modal retains transfer/preparation progress and offers background
work, cancellation, and retry with the failure detail. `ListDatasets` and
`assets::request_required(scope)` use `engine` or `twin:<name>` from
`DatasetScope::id()`. Recent Events records lifecycle transitions; live byte
progress stays in the registry. A required missing DEM holds simulation while
UI remains usable, then the existing terrain bridge automatically retries the
pending prim when the dataset is installed. Initial installation uses that bridge.
Settings offers Rebuild (`ProcessDataset { id, force: true }`) to rerun a valid
recipe; normal Prepare reuses valid outputs. A deliberate rebuild of an already
loaded terrain bundle needs Reload scene to apply it through the full scene
lifecycle. Neither preparation nor dismissal changes the user's transport pause.

For school startup acceptance, use the built production binary with
`scripts/api/test_school_asset_preparation.py --twin <school-root> --output
<fresh-evidence-directory> --port <owned-free-port>`. It stages a source-only
Twin from the current checkout and pinned cached raw inputs, then checks the
four composed scene dependencies, missing-ground tick hold, preserved manual pause,
and full rendered terrain cover through authored Rhai verdicts. Inspect the
captures separately from the API verdict. Every texture bound by the startup
material needs a delivered artifact, including an overlay whose authored weight
starts at zero; unrelated candidate sites remain optional.
For Griffin's NOBILE03 source, the Twin's `Assets.toml` defines a complete
native recipe. Requesting its `nobile03_dem` dataset obtains the DEM, orthophoto
and provenance label, then publishes float64 heightmap, albedo, coverage mask
and integrity receipt under `.cache/terrain/nobile03`. No custom Python adapter
or manual terrain copy is part of provisioning. `lunco-assets verify --twin
<TWIN> -a nobile03_dem` checks a prepared/extracted bundle.

The native processor preserves unresolved source coverage as NaN, performs
bounded minimum-curvature interpolation with measured samples fixed, and
rejects failed convergence or excessive missing coverage. Filled regions are
estimates. Output node span is `(resolution - 1) * spacing`. For crop changes,
compare retained heights through both the artifact and production `TerrainHeights`.
Procedural craters, overzoom and rocks remain USD terrain layers; derived
geometry-dependent maps consume the composed surface instead of a duplicate
asset-preparation crater pass.

For download UI acceptance, use `scripts/api/test_dataset_provisioning.py`
with `LUNCOSIM_BIN`, a free `--port`, a fresh `--output`, and exact prepared
`--summer-zip`/`--griffin-zip` inputs. The driver extracts task-owned copies,
serves paced local bytes, and exercises real window input for bulk download,
cancel, checksum failure, and retry. Inspect its captures and provide observed
logical coordinates at the driver prompts, or replay its recorded `--clicks`.
Authored Rhai verdicts require held terrain ticks, automatic recovery without
scene replacement, preserved pause, outgoing scope retirement, and advancing
Modelica participants in both prepared Twins. It runs each package's own
startup gate and verifies API Exit and port release. This Linux run does not
certify an installed Intel macOS build.

Downloads use the shared `download` section in the user settings file
(`lunco-settings::DownloadSettings`): attempts include the first request,
delays are exponential and capped, and a failed body read resumes from the
received prefix with HTTP `Range` when the source supports it. The CLI and the
interactive window therefore have one policy and one cache/path resolver.

## Joining a cropped DEM to the rendered globe

The site keeps its own cropped DEM as the only elevation input for its handoff.
The DEM asset and retained base grid remain unchanged, and the composed local
surface preserves their relief throughout the crop. The crop's border datum
sets the visible globe shell radius; celestial and physics state keep the
canonical body radius. A worker-prepared exterior collar continues the measured
edge profile from the exact crop boundary to the analytic sphere. This collar
is visual closure outside the crop; terrain queries and colliders remain on the
local surface. Globe tiles cut out the collar footprint with bounded
orthographic edge sampling. Do not copy the collar into each tile or make
global LOD follow DEM posting density. No body-wide raster asset is required.

Exterior smoothing preserves the first native posting and filters only the
continuation in its preparation worker, without increasing mesh density. Keep
the DEM appearance visibly distinct. See [the geometry contract](../../docs/architecture/60-curvature-elevation-and-gravity.md)
for ownership and sampling.

Derive the visual collar width from this crop's measured edge relief and
one-sided edge slope with a 0.60 relief-grade sizing target. Continue the
measured slope over one posting, then fade one nearest-perimeter signal to
the sphere over at least one full crop width. Broad relief must not collapse
into a steep perimeter rim; sizing is independent of Sun and camera position.
Geometry and appearance share one width sized from crop scale and maximum edge relief; the native
inner posting lattice tapers to an outer boundary with at least 32 segments per side. Keep
the DEM and all in-crop heights unchanged. Refine the globe only in the band
between the exact crop and the collar's outer cutout; set the local minimum tile
size from that handoff footprint, independent of DEM posting density.
The collar reads map roles from the cropped DEM prim and appearance from the
body's USD-selected `UsdShade` material. A continuation-capable WESL shader and
its USD Shader prim must declare the matching
`lunco.lunar-surface-continuation.v1` interface. Rhai admits compatible sources,
waits for reflection, and holds simulation on a mismatch. The compositor keeps
unadmitted collars hidden; the globe cutout waits for the composed material and
visibility. `RunLint` checks the composed declaration against reflected WESL.
Read the body's composed look from `GlobeLod`, with its authored declaration
entity as validation provenance. The shader asset path is never selected in Rust. Inspect the actual boundary
vertices through bounded `TerrainLodStatus` geometry pages; `mesh_entity` selects
a CPU-retained DEM boundary tile, including its morph positions. Use
`boundary_only: true` to page only the finite perimeter. When bake sampling math
changes, update the persistent visual tile cache revision in the same change.
Check both
rendered edges against mission queries before accepting a corner join.

The active crop supplies its own georeference, posting spacing, border datum,
and measured edge profile, so this works with Twin-local crops at different
sites and resolutions. A body currently has one built crop handoff; multiple
built crops for the same body are a visible input error, not a size-based
selection.

### Verify automatic lunar DEM registration

For automatic lunar DEM registration, verify both repository fixtures
`scenes/tests/lunar_dem_continuation.usda` (omitted coordinates) and
`scenes/tests/lunar_dem_georeferenced.usda` (DEM-authored coordinates) in an
owned headful production session. The shared continuation scenario waits for
material admission and settled DEM streaming. Headless scene tests do not
provide the GPU-retained boundary geometry required by this graphics verdict.
The scene-site projection and zero defaults are specified in the geometry
contract; no tutorial script installs the handoff.

## Where files live (cache resolution)

- **Shared cache** — `~/.lunco/cache` by default on native platforms.
  `LUNCOSIM_CACHE` remains an explicit CI/custom-install override. Every
  worktree and Twin therefore shares one pool of regenerable data (source libraries,
  textures, ephemeris, downloaded sources).
- **Twin cache** — `<TWIN>/.cache`. A Twin's default-owned downloads land
  beside the Twin. `twin://` reads resolve `<twin>/<rel>` first, then
  `<twin>/.cache/<rel>`, then the global cache `<cache>/<rel>`.
- Runtime code obtains this path from `lunco-assets-core::twin_cache_dir`;
  manifest ownership is selected by the shared cache contract in
  [asset resolution](../../docs/architecture/56-asset-resolution-and-cache.md#engine-and-twin-caches).
- **`shared = true`** on an entry sends its write to the global pool instead
  (`<cache>/sources/<sha256(url)[..16]>/<basename>`) — one download per URL,
  reused by every twin and worktree. Use it when several Twins intentionally
  share a multi-GB upstream product; a Twin that must be self-contained should
  set `shared = false`.
- **Raw downloads**: entries without `dest` land in
  `<owner cache>/sources/<url-hash>/<basename>` — owner being the twin
  (`--twin`) or the shared cache (crate manifest). Author `dest` only when a
  file must sit at a specific path; it is then resolved against that same
  owner cache.
- **Baked outputs** (`output_root = "twin"`): inside the twin at `output`,
  where the scene's `demSource`/layer attrs expect them. Per-twin, always.

## Git hygiene for downloaded and generated bytes

Before downloading a Twin dataset, add and verify ignore rules for the raw
cache and generated processing outputs. The canonical policy is:

```gitignore
twins/**/.cache/
twins/**/terrain/*/materials/
twins/**/terrain/*/.bakekey
```

Raw downloads belong under the Twin's `.cache` (or the shared global cache when
`shared = true`); processed DEMs, maps, and bake stamps are regenerable bytes.
Commit `Assets.toml`, processing/reprojection tools, provenance and parameter
reports, and USD references. Do not force-add cache or generated terrain bytes.
Use `git check-ignore -v <raw-file> <processed-file>` before staging. A source with unsupported or invalid projection metadata must fail at its
native owner. Do not reinterpret it as an equirectangular raster. Supported
GeoTIFF projections are selected from metadata, and the manifest owns the
output crop, frame and vertical datum.

## Process kinds (in `[key.process]`)

| kind | Input | Output |
|---|---|---|
| `dem` | DTM (GeoTIFF/.IMG) | `<output>/materials/textures/heightmap.tif` — square float64, georef in tags. `output` is a FOLDER; scenes reference it as the search path `demSource = @terrain/<site>@`, found from any scene folder through the Twin root/cache |
| `map` | co-registered raster (ortho `.IMG`, `_SHADE`/`_SLOPE`/`_CLRGRAD` `.TIF`) | 8-bit RGB PNG at `output` (a FILE). Gray sources get a 1–99 percentile stretch in linear contrast space, then sRGB encoding for the runtime loader |
| `albedo` | grayscale PDS3 `.IMG` or georeferenced TIFF orthophoto | stable linear material-albedo PNG at `output` (a FILE) |
| `normalmap` | DTM | DEM-local ENU normal PNG (`RGB = n*0.5+0.5`, decoded by the shared terrain-surface shader kernel) |
| `texture` | any image | resized PNG (non-geo default) |
| `gltf` | .glb | Draco-normalized .glb; WebP extension conversion pending |

The built-in processors are registered through
`lunco-assets-processing::process::ProcessorRegistry`. A domain-specific
native baker should register a `ProcessorSpec` and publish its sidecars through
the shared staging/commit path. Put processor-specific manifest values in
`[key.process.parameters]`; do not add a new shared manifest field for every
domain. Keep selection, ordering, and onboarding policy in the reusable Rhai
`assets` tool library or a Twin-owned script; Rust remains the owner of
decoding, heavy math, cancellation, and atomic publication.

`DatasetRegistry` rejects process outputs that overlap another process output
or any declared download source, including file paths nested under a directory
output. The `dem` processor replaces its complete configured folder when it
commits. Store independent material maps in sibling paths and point the scene
at the DEM folder.

Use the existing `assets` Rhai library to process declared sources at runtime:
`assets::bake(id)` dispatches `ProcessDataset`, and
`assets::bake_scope(scope)` queues each idle processing declaration in a scope.
Both read source identity and pipeline settings from `Assets.toml`; the command
carries only the dataset id. `ListDatasets` reports completion through each
entry's `state`. Calling bake again is safe: the content bake key skips current
outputs. Adding another source or changing its output size/ROI therefore needs
manifest and Rhai edits only; add a Rust processor only for a new transform.

Shared ROI fields: `center_lat`, `center_lon`, `window_m`,
`target_resolution = [n, n]`, `pixel_scale_m`, `src_min/max_lat`,
`src_min/max_lon`, `frame = "MOON_ME"`, `output_root = "twin"`.

For a grayscale orthophoto used as terrain colour, add a separate `albedo`
process entry. Its optional native-only parameters are
`albedo_base_linear` (neutral material base, default `0.13`),
`albedo_detail_strength` (retained local contrast, default `0.35`), and
`albedo_illumination_radius_m` (low-frequency field radius, default `40`). The
processor excludes unmapped zero radiance before resampling, so missing coverage
cannot darken neighbouring measurements. It writes a stable `albedo.png`; it does
not claim to perform full photometric calibration. PDS3 `.IMG` sources supply
their projection extent and pixel scale from the attached or detached label.
A plain grayscale TIFF albedo source must author `pixel_scale_m` and all four
`src_*` bounds; a supported spherical GeoTIFF supplies these spatial facts
through its standard projection metadata.
Keep `map` for analysis/display outputs. In Rhai,
`assembly_builder::lunar_albedo_material_plan(...)` returns standard USD
`SetAttribute` operations for the produced albedo/normal assets; submit those
through `assembly_edit::batch` or `assembly_edit::propose`. This keeps heavy
image math in Rust while making the assembly policy replaceable and extensible.

## Adding a new territory to a twin

1. Find the product: `https://data.lroc.im-ldi.com/lroc/view_rdr/NAC_DTM_<SITE>`;
   files under `https://pds.lroc.im-ldi.com/data/LRO-L-LROC-5-RDR-V1.0/LROLRC_2001/DATA/SDP/NAC_DTM/<SITE>/`.
2. For an LROC PDS3 source, read its `.LBL`: `MAP_PROJECTION_TYPE`
   (`.IMG` currently uses EQUIRECTANGULAR; spherical polar/equirectangular
   GeoTIFFs use standard tags), `MAP_SCALE` → `pixel_scale_m`, and
   `MIN/MAXIMUM_LATITUDE` + `EASTERNMOST/WESTERNMOST_LONGITUDE` → the four
   `src_*` fields. **Label longitudes are 0–360 °E — author `center_lon` in
   the same convention.** Never trust `CENTER_LONGITUDE` (body-frame quirk).
   For a GeoTIFF, verify its spherical CRS/geotransform. Standard polar
   stereographic and equirectangular metadata is consumed natively; plain TIFFs
   require explicit scale and extent declarations.
3. Pick `center_lat/lon` (the POI), `window_m` (scene size),
   `target_resolution ≈ window_m / native m-per-px` (square).
4. `sha256 = ""` on first download → the tool prints the hash; paste it in.
5. PDS3 `.IMG` sources declare extent/scale in their label — `src_*` may be
   omitted (an authored manifest extent wins when all four are set).

## Wiring maps as terrain layers (USD)

Maps bind through a **stock UsdShade Material network** — the only authoring
path. Bind the Terrain prim to a Material, whose surface output connects to a
Shader carrying one `asset inputs:<role>_map` + `float inputs:weight_<role>`
per layer. Inspect an existing terrain scene in the target Twin for its exact
prim paths before adding a new network:

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
            uniform token info:implementationSource = "sourceAsset"
            uniform asset info:wesl:sourceAsset = @lunco://shaders/terrain_layered.wesl@
            # Optional for streamed CDLOD; omit for a static/root mesh.
            uniform asset info:wesl:vertexAsset = @lunco://shaders/terrain_geomorph.wesl@
            asset inputs:albedo_map  = @terrain/<site>/materials/textures/albedo.png@
            float inputs:weight_albedo = 1.0
            asset inputs:normal_map  = @terrain/<site>/materials/textures/normal.png@
            float inputs:weight_normal = 0.5
            asset inputs:mineral_map = @terrain/<site>/materials/textures/slope.png@
            float inputs:weight_mineral = 0.0   # raise for a classification drape
        }
    }
}
```

For authored DEM terrain, `terrain_layered.wesl` owns the canonical material
fragment. `terrain_geomorph.wesl` is only its optional CDLOD vertex stage;
`terrain_shadow.wesl` is an explicit material for non-authored terrain and is
never an automatic recovery choice. Keep shared regolith detail and lunar
photometry in the imported `lunco::shaders::terrain_surface` and `lunco::shaders::lunar_brdf` shader modules.
See the [terrain rendering decision record](../../docs/architecture/terrain-layered-rendering.md)
before adding another terrain shader path.

Keep production albedo and normal rasters as authored assets. An illumination-
bearing grayscale orthophoto is not intrinsic albedo: declare it as
`kind = "albedo"` so native Rust removes its low-frequency acquisition-light
field and retains stable local variation around the authored neutral regolith
base. Bind the resulting `albedo.png`, which is lit once by the runtime sun
and shadows. A calibrated reflectance raster may use `texture` when its colour
contract is known. Do not replace an orthophoto with a hillshade, slope, or
elevation-colour diagnostic to hide minification aliasing; those remain
optional analysis products and their authored role/weight must stay explicit.
The render binder still builds missing RGBA8 mip levels once per image asset
version, off-thread and with role-aware filtering (linear-light colour, linear
scalars, renormalized normals) before enabling trilinear/anisotropic sampling.

### Physics parameters for a DEM generator

The `dem` child layer also owns the physics collider-ring lattice. Author these
beside `windowM`, `targetRes`, `lodViz`, and `colliderRing` when a Twin needs a
non-default contract:

For an authored `rocks` layer, `lunco:layer:regionM` is an optional
half-extent in metres: omit it or leave it at `0` to cover the whole composed
terrain; author a positive value only when a near-field scope is intentional.
`lunco:layer:density` is per hectare, and the rendering-quality profile owns
the total instance cap.

```usda
int lunco:layer:colliderDepth = 8
int lunco:layer:colliderResolution = 49
```

These values are copied into the typed terrain-generation request and used by
native, worker, GUI, and headless physics. They are deliberately independent
of `RenderingQualitySettings`, camera-driven visual LOD, and `targetRes`; a
graphics preset must never change collider tile count or resolution.
For terrain width `W`, contact spacing is
`W / (2^colliderDepth * (colliderResolution - 1))`. Overzoom crater radii below
twice that spacing are absent from contacts and reach full amplitude at six
times the spacing. Size this explicit lattice for the authored physical relief,
then compare native non-solid downward `Raycast` hits (excluding sensors) with
`TerrainHeight` in the body-driven ring. A visible crater can outresolve an
otherwise correctly composed collider band.

Visual CDLOD morph targets sample the parent's indexed triangles for height
and normal. Preserve that interpolation at interior vertices and stitched
edges, and advance the visual tile-cache revision when bake math changes.
The pure `morphed_child_matches_every_parent_triangle` regression covers this
contract without involving authored terrain assets or physics resolution.

Asset paths are **scene-root-relative** and resolve through `twin://`, so they
travel with the twin. The generic USD shader projection walks
`material:binding` → Material → `outputs:surface.connect` → Shader and publishes
one `ShaderLook`; the terrain source reconciler derives the typed roles from that
same look. Roles are `albedo`, `mineral`, `surface` (packed R=rough G=AO B=rockDens),
and `normal`.

The bound Shader also owns the render stages: `info:wesl:sourceAsset` must expose
`@fragment`, and optional `info:wesl:vertexAsset` must expose `@vertex`. A single
WESL module may provide both. Missing or invalid stages are reported as a
structured diagnostic and leave the material unbound. Rust never swaps in a
neutral or terrain shader to hide a bad asset; if a Twin wants an explicit
non-authored material, author that choice in USD/Rhai so it can be changed
without rebuilding the renderer.

- Every `inputs:*` is a live-tunable, journaled knob (networked, undoable) and
  the network is inspectable in usdview/Blender.
- **CONNECTED** map inputs are skipped — a connected port is fed by a producer
  node (doc 18 Tier B), not an authored file.
- `mineral` composites **UNLIT after lighting**, so a slope/classification
  drape stays readable inside shadow — its entire job.
- `albedo` is the terrain colour source at its authored `weight_albedo`; at
  full weight the layered shader disables its procedural dust/mottle colour,
  while relief normals, roughness, AO, and photometry remain active.
- The authored shader source and maps bind on both static and streamed terrain
  through the same `ShaderLook`; streamed LOD tiles add only their CDLOD
  geometry inputs, and the derived bake fills slots an authored map left empty.
- The runtime derived bake is optional visual refinement after the DEM ground
  is ready. Its effective resolution is bounded by a static terrain's authored
  visual target, so a low-resolution static product does not pay for an
  invisible high-resolution map. The task is cancelled at scene teardown or
  when its liveness bound expires; the terrain remains usable and the status
  bus reports the terminal warning. This refinement status is separate from
  terrain tile streaming, so it cannot hide tile progress or make a presentable
  ground scene wait for an optional map.
- For multi-site scenes, author these inputs **inside a terrain variant** and
  verify the composed variant through the production scene-test command; do
  not add a package-specific USD probe for this asset contract.

Node-graph authoring is outside this asset pipeline. Read the current
multi-domain architecture before introducing a new graph owner.

## Caching & staleness

- Downloads skip when the resolved file exists with matching `sha256`.
- Bakes stamp a `.bakekey` =
  `sha256(source bytes ‖ effective config ‖ PIPELINE_VERSION)` beside each
  output; a matching stamp skips the bake before the expensive decode.
  Changing the source, ROI, `--quality`, or bumping `PIPELINE_VERSION`
  (`src/process.rs`) rebakes exactly what changed. Never time-based.
- The dataset registry treats that stamp as the completion boundary: a DEM
  output directory without `.bakekey` is partial and remains downloadable/
  processable, even if the directory itself exists.
- Processing roots are strict (`cache`, `twin`, or `assets`); an unknown root or
  missing required owner fails visibly. Processing uses a unique staging output
  and an atomic commit barrier, so cancellation cannot publish stale terrain.
- The terrain derived bake keys through the oracle's canonical surface identity
  and does not re-hash the full DEM for every request.
- A queued DEM build is an indeterminate state until its owner admits a task;
  do not show a numeric percentage for that scheduler hand-off. Phase changes
  are discrete status events and active work uses the existing progress entry.
- Baked artifacts and the twin cache are gitignored by policy
  (`terrain/*/materials/`, any `.bakekey` stamp, `.cache/`, and `extern/`) —
  never commit downloaded or derived payloads. Keep only the `Assets.toml`
  declaration, source URL/hash, ROI, and processing configuration in Git.

## Gotchas

- `*_50CM`/`*_2M` `.IMG` companions are ORTHOPHOTOS (brightness), never
  elevation — `kind = "albedo"` for terrain colour, `kind = "map"` only for
  analysis/display, and never `kind = "dem"`.
- Confirm the DEM datum and the scene's celestial-body radius before combining
  terrain elevations with orbital or body-fixed coordinates. Do not encode a
  product-specific radius correction in the asset pipeline.
- Heights are absolute body-datum metres: prims on a surface must be
  authored at the DEM's own elevation.
- The runtime DEM reader requires square rasters; keep the scene's
  `windowM`/`targetRes` in step with the manifest ROI.
- Optional QGIS/GDAL extras (custom-sun hillshade, slope ramps, contours) are
  external to `lunco-assets`; verify that the Twin supplies and documents its
  own tooling before invoking it.

The provisioning dialog distinguishes verified source inputs from prepared
artifacts. `ListDatasets.inputs` records each source URL and verified state;
`inputs_ready` selects preparation without another transfer. Inspect both these
facts and `state` when diagnosing a failure. An authored override may hide a
valid generated cache: the failure names the selected and output paths. Correct
that authored state explicitly; do not change resolver precedence. Current
pipeline/UI acceptance is `scripts/api/test_asset_pipeline.py`, using authored
Rhai verdicts, source-only Griffin extraction and an owned windowed session.

Native scene requirements come from active composed USD asset values, including
arrays, time samples and binary asset references. `SceneDatasetRequirements`
(`lunco-assets-datasets`) is supplied by the USD runtime and exposed as
`ListDatasets.required_for_scene`. `recommended` ranks optional content only.
Do not add a duplicate scene-to-dataset list in a manifest or suppress required
consent. Settings uses the same row policy; closing the dialog does not release
`SceneDatasets` progress holds. Preparation jobs queue in request order under
`DownloadSettings.max_parallel_downloads`; cancellation works before admission.
The current source name accompanies per-file byte counts. Recent Events reports
source/phase changes, not byte updates. Native Windows acceptance must run the
nightly matrix's transfer recovery, publication and path tests on Windows.

Runtime consumers add concrete URI requirements through the same registry;
Rhai scenario/tool imports are discovered before execution. For a URI selected
by code, use `assets::require(uri)`, then consume data through the owning
loader's ready event after `DATASET_INSTALLED`. The acknowledgement does not
mean bytes are ready, and no partially executed script is replayed. Missing
declared data is recoverable; unknown URIs and fresh loader failures are visible
errors. Catalog/preload reads do not make optional data mandatory.

Use `scripts/api/test_scene_asset_requirements.py --port <owned-port> --output
<fresh-directory>` for the generic selected-scene gate. It authors fixture USD
through document commands, serves controlled HTTP bytes, and verifies
nonrecommended requirements, optional recommendations, queue cancellation,
checksum failure/retry, preserved manual pause, scene replacement and teardown.
