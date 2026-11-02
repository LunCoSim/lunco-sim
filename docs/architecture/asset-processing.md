# Artifact processing recipes

> Status: Active · Audience: asset pipeline and Twin authors

`Assets.toml` owns source declarations, processing parameters, ordered steps,
and delivered artifacts. `lunco-assets` composes downloads and workers;
`lunco-assets-processing` executes operations. CLI and application commands use
the same processors and publication transaction. No processing view is required.

## Ownership

| Concern | Owner |
| --- | --- |
| Source identity, root order, containment | `lunco-assets-core` and `lunco-storage` |
| Recipe types, validation, receipts and installed state | `lunco-assets-datasets` |
| Download, integrity, archive install | `lunco-assets-download` |
| Decode, projection, raster math and artifact transaction | `lunco-assets-processing` |
| Source GeoTIFF projection and sample registration | `lunco-geotiff` |
| Custom step selection and parameter decisions | Authored Rhai |
| Custom pixel calculation | Authored WESL |
| Physical calculation | Authored Modelica through the existing compiler/batch runner |
| Craters, overzoom, edits, scatter and composed-surface products | Existing USD terrain layers and terrain owners |

Asset preparation produces the base DEM and source-backed material maps.
Procedural terrain layers stay in USD. The native terrain runtime composes
analytic crater/overzoom contributions with the base in `SurfaceOracle`.
Visual meshes, colliders, queries and geometry-dependent derived maps consume
that composed surface. Rasterizing the worker's crater stamp helper in an
asset recipe does not reproduce the native analytic model. Exporting a complete
composed surface is a separate terrain-owner operation, not an implicit asset bake.

## Manifest contract

A leaf `[asset.process]` selects one registered native processor. A compound
recipe selects `kind = "pipeline"`, one output directory, and a typed
`[asset.process.pipeline]` table:

```toml
[surface]
name = "Prepared study surface"
url = "https://example.org/height.tif"
dest = "sources/height.tif"
sha256 = "<source SHA-256>"
recommended = true

[surface.process]
kind = "pipeline"
output_root = "cache"
output = "terrain/study"
center_lat = -85.0
center_lon = 30.0
window_m = 1024.0
target_resolution = [257, 257]
frame = "MOON_ME"

[surface.process.pipeline]
artifacts = [
  { from = "height", source = "materials/textures/heightmap.tif", path = "materials/textures/heightmap.tif", kind = "dem", dimensions = [257, 257] },
]

[[surface.process.pipeline.steps]]
id = "height"
input = "source"
kind = "dem"
output = "height"
```

`source` is the entry's primary download. `pipeline.sources.<id>` declares
additional raw `AssetEntry` downloads with their own URL, checksum, destination
and shared-cache choice. One request obtains the complete input set, including
when the primary source is already cached. Nested processing declarations are
rejected: ordered steps express computation within this recipe.

Steps inherit the enclosing process's scalar settings, then apply their own
overrides. A DEM and orthophoto therefore share crop coordinates while choosing
different output resolutions. Step inputs name `source`, a declared source/code
input, or an earlier step. Scratch outputs must be distinct and cannot overlap.
Each artifact names its producing step, an optional file inside its directory,
the final relative path, format (`dem`, `image`, `file`) and optional dimensions.
The artifact table is the final consumer contract; a planner cannot replace it.

`coverage` emits a node-aligned PNG mask: 255 where source resampling has finite
coverage, 0 where the DEM processor must interpolate or the source is absent.
It describes coverage, not additional measurements. `zero_is_nodata = true`
explicitly selects an orthophoto-style zero mask. DEM, albedo, map and normalmap
operations share projection-aware sampling. Source polar stereographic
(north/south) and equirectangular GeoTIFFs use their standard spherical CRS,
false origin, scale, tiepoint and area/point registration. Unsupported CRS or
ellipsoidal metadata is rejected. Plain rasters/PDS products retain their
explicit extent-based processing contract. Output crops use the existing
crop-centred equirectangular GeoTIFF contract; a crop crossing a pole is rejected.
DEM artifacts retain f64 values through processing and float64 storage.

## Dynamic code

`pipeline.files` maps input names to portable authored paths. These are resolved
through the same owner roots as other assets, read on the worker, hashed and
copied into a per-job snapshot. Code retains its relative directory layout.
Declared raw files that are siblings remain siblings in that snapshot, including
PDS image/label pairs. All dependencies must be declared; no code reader falls
back to the process CWD. A file may use `lunco://` to select reusable engine code.

### Rhai planning hook

`planner = { source = "policy", entry = "plan" }` selects a declared code file.
This is the recipe-local `assets.process.plan(facts: Map) -> Map` seam, compiled
with `lunco-hooks-rhai::RhaiHook` and the existing typed `HookValue` ABI. It does
not install a second application/Twin policy manifest or a live-World engine.
Its scope/lifetime is one immutable asset-preparation attempt, with no simulation
clock. The reflected declaration is non-installable because the recipe owns
the exact source binding. The supplied facts contain `plan`, inherited
`settings`, input hashes and native `processors`. Return the modified `plan`:

```rhai
fn plan(facts) {
    let plan = facts.plan;
    plan.steps[0].source_height_offset_m = -10.0;
    plan
}
```

The owner validates and consumes the returned topology. Input/code ownership
and final artifacts cannot change. No planner means execute the declared plan.
An authored compile error, thrown exception or malformed result is a terminal
job failure. The shared hook interpreter rejects filesystem imports; helper
functions may live in the declared source. Cancellation interrupts evaluation.

### WESL image steps

Native desktop composition enables `shader-processing`; standalone CLI builds
can opt into the same feature. A `shader` step selects `shader = "kernel"`
from declared WESL/WGSL inputs, `uniforms = [1.0]`, explicit
`input_color_space` / `output_color_space` (`linear` or `srgb`), target dimensions,
and `timeout_s`. The dynamic file supplies:

```wgsl
fn process_pixel(p: vec2<u32>, size: vec2<u32>, color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(color.rgb * parameter(0u), color.a);
}
```

The engine-owned compute ABI supplies `input_pixel`, `parameter`, image sizes,
dispatch and readback. Declared shader file names are also virtual WESL module
names for imports. Only those captured sources enter compilation. The default
input colour passed to the function uses nearest sample mapping between input
and output dimensions; a kernel can explicitly select other input samples.
The result is an RGBA8 image artifact. Conversion to GPU f32 and image encoding
are explicit rendering boundaries; this stage does not replace canonical f64
DEM preparation. Device limits, compilation, output finiteness and dimensions
are checked. A missing GPU or feature is a visible failure, not a CPU fallback.
GPU artifact bytes are recorded by hash; cross-device bitwise regeneration is
not promised.

### Modelica steps

Desktop composition enables `modelica-processing`; the CLI can enable it too.
A `modelica` step reads a declared `.mo` input, explicit `class`, `bounds`
(`t_start`, `t_end`, optional `dt`, tolerance/solver), finite Real `inputs` and
`overrides`, `timeout_s`, and optional shared `RunResultLimits` in `limits`.
Other declared `.mo` files are the explicit source set. The admitted compiler
does not discover undeclared source libraries. `ModelicaCompiler` and the shared
`drive_run` supply compilation, solver selection, numeric execution, result
budgets and cancellation. The artifact is the typed `RunResult` serialized as
TOML, including f64 time series and metadata. It is suitable for calibration,
lookup tables and physical preprocessing. No second solver is implemented.

### External steps

An `external` step declares `program`, a string-array `args`, positive
`timeout_s`, explicit `tool_version`, and optional string-table `env`.
Whole arguments/environment values `{input}`, `{output}` or `{declared_id}`
resolve to snapshot/scratch paths. Arguments are passed literally without a
shell. The child receives only the declared environment; dependencies and tool
version belong to the recipe. Logs are captured and a bounded tail accompanies
failure. Nonzero exit, cancellation, timeout, absent outputs or invalid artifacts
prevent publication. External code is trusted native execution; staging is an
artifact admission boundary, not an operating-system sandbox.

## Publication and readiness

All steps run inside the existing bounded host worker mechanism. The complete
bundle is staged outside its destination. Output/source overlap, escaping
links, missing/non-finite DEMs, bad dimensions and incomplete images fail at the
owner. Input bytes are checked again before publication. The existing shared
commit barrier prevents a closed Twin's job from publishing late.

The bundle contains `.bakekey` and `.pipeline.toml`. The latter records the
declaration hash, hashes of all source/code inputs, resolved executed step
settings and output checksums. The bake key also covers the effective plan and
processor version. The registry reports installed only for a complete bundle
matching the declaration, current inputs and receipt. An incomplete authored
directory continues to shadow its cached counterpart according to the common
resolver; it is reported incomplete rather than silently bypassed. The worker
validates that same consumer-facing selection before reporting installed.
Publication reloads declared bundle members and dynamically consumed archive
members through the shared asset server. Failed handles are requested again
once at publication, synchronously entering Loading before consumers observe
the installed declaration. Loaded handles use ordinary reload. Consumers retain
their original request through source decoding and dependency publication;
a fresh loader failure remains terminal. No timeout, frame delay or repeated
retry substitutes for the asset server's load state.

`RequestDataset`, `ProcessDataset`, `CancelDataset` and `ListDatasets` remain the
public workflow. `ProcessDataset { id, force: true }` and CLI `process --force`
rerun stages even when the receipt is current; normal preparation reuses valid
work. Both preserve the last valid bundle on failure. Processing state names the active step. The query's `recipe`
field reports declared steps and logical artifact URIs. `lunco-assets verify
--twin <root> [-a <key>]` uses the same registry checks; without a key it checks
recommended datasets. Packaging must include the cache, receipt and declared
inputs, then verify the extracted Twin. Missing source for an explicitly
selected CLI process is an error.

Validation combines pure projection/parser/transaction tests with authored
Rhai verdicts through the production application, including failed processing,
code changes, artifact integrity and cold-cache terrain admission. The native
shader and Modelica feature paths must be exercised when changing their adapters.

The existing provisioning dialog presents source verification separately from
artifact preparation. Source facts (`inputs`, `inputs_ready` in `ListDatasets`)
are captured on admission/download/processing workers and committed with the
operation's existing progress channel. UI/query reads never hash files. Rhai
chooses Download, Prepare, Cancel and retry wording from those typed facts;
HTML/CSS shows the source URL, verification count, active operation and full
failure detail. A prepared cache hidden by an invalid authored override names
both physical paths and requires explicit correction. No override is skipped.
The shared `OpenWebUrl` shell command validates absolute HTTP(S) addresses and
uses the existing egui browser adapter; runtime UI forwards a typed URL action.

Settings ▸ Data & libraries groups assets by owner, filters by asset/source name,
and uses the same Rhai row policy for source readiness, processing status, errors
and actions. Source details and transfer tuning are collapsed until requested.
Prepare, Rebuild and Cancel dispatch typed commands through the existing tool.
Rebuilding changes the artifact cache. Reload scene applies rebuilt terrain
through the supported full scene lifecycle, including normal physics admission.
Manifest changes are admitted when the Twin is opened.

The provisioning dialog also consumes typed readiness, runtime-fault, transport
and simulation-progress snapshots. Artifact readiness and simulation readiness
remain separate: required terrain holds ticks through source preparation, DEM
admission and resident collision support. Closing the dialog never changes
transport; a manual pause survives successful preparation.

Scene requirements and admission
-------------------------------

USD composition owns scene dependency facts. The stage projection worker
snapshots active asset values (scalar/array/time sampled) and binary references;
unselected variants and inactive prims are excluded. Live asset edits refresh
only affected properties/subtrees through the canonical stage owner. The native
USD runtime matches logical identities through `DatasetRegistry.declared_artifact`,
including explicit pipeline members and archive directory members, then publishes
scene-scoped `SceneDatasetRequirements`. It waits for manifest discovery before
asserting completeness. A `SceneDatasets` progress hold persists until every
required declaration is installed. Teardown releases the exact scene hold and
clears requirements; byte/worker completion never controls transport Play/Pause.

`ListDatasets.required_for_scene` and the provisioning hook consume that same
resource. Recommendations do not define scene requirements. The prompt shows
required data; Settings includes required, recommended and optional rows. Closing
the prompt keeps a missing scene paused. Required consent has no persistent
suppression setting. No network request starts without an explicit request.

Runtime dependencies use this same scene lease and registry. Actual Rhai
scenario and tool-library consumers traverse their loaded source/import graph
before execution; missing declared imports add their concrete URIs to
`RuntimeAssetRequirements`. Literal imports inside functions are included.
Catalog scans and preloads do not create requirements. Timeline text and dataset
text consumers also admit their requested identities. Dataset discovery and
artifact installation remain separate from source parsing and physics readiness.

For a URI selected by running code, `assets::require(uri)` dispatches the typed
`RequireAsset` command. It declares use, without starting a download or blocking
the calling Rhai function. Its acknowledgement means the requirement was
accepted, not that the bytes are ready. Event-driven code consumes the artifact
after `DATASET_INSTALLED` using its ordinary loader/ready event; the dataset text
reader retains and resumes its original request automatically. Arbitrary
partially executed scripts are never replayed. File-backed Rhai imports retain
the existing string-literal contract. An undeclared URI is a terminal command
rejection naming `Assets.toml`; malformed downloaded source faults the scene
with the loader diagnostic. New runtime requirements reopen the window even
when the user dismissed an earlier request. Scene teardown retires the exact
requirement set. Browser hosts keep their own fetch transport and reject the
desktop provisioning command explicitly.

Explicit requests enter a FIFO preparation queue limited by
`DownloadSettings.max_parallel_downloads`. Queued jobs can be cancelled without
starting a worker. Closing a Twin removes its queued work and retires active
workers behind the commit barrier. Transfer progress identifies the current
source; its percentage applies to that file. Recent Events records queue,
source and processing-step changes, completion, cancellation and full errors.

Native processing and directory transactions run on Linux, Windows and macOS.
The planner receives `platform.os` and `platform.arch` for explicit authored tool
selection. External programs receive literal arguments and declared environment,
without shell expansion. The native CI matrix exercises publication/replacement,
external processing, transfer cleanup and recovery, including Unicode/spaced
paths. Browser hosts retain their separate asset transport; this native worker
gate is not installed in browser builds.

Mounted preview/document authorities can share one canonical storage root with
an open Twin. Dataset discovery registers one store per root, preferring the
actual Workspace Twin's live authority. Other live asset authorities resolve
requirements to that same store. Retired or transferred scopes emit
`RetireDatasetScope`; the provisioning owner cancels queued work, crosses its
commit barrier, retires active workers, then removes registry/UI rows. A
registry-only host performs the same removal without workers. Late manifest
scans validate the current storage owner before admission. Closing an editor
preview therefore cannot poison a subsequent Twin catalog with stale output
conflicts.
