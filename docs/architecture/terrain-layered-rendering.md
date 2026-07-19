# Terrain Layered Rendering & Analysis Overlays

> Audience: contributors working on terrain rendering, scientific overlays, or the
> comms/illumination map layers. Companion to
> [`terrain-substrate.md`](terrain-substrate.md) (the height oracle) — this doc is
> about what gets **drawn on** the surface, not how the surface is shaped.

How LunCoSim renders **many layers on one terrain** — textures, scientific
overlays (slope, minerals), a lat/lon graticule, and **time-dependent maps**
(connectivity, illumination) — as a single extensible, live-tunable, USD-authored
system, without a bespoke shader per layer.

## The principle: one pipeline, not one shader per layer

A slope overlay, a mineral map, an elevation ramp, and a comms-connectivity map
look different but are the *same three decoupled pieces*:

```
layer = Data source  →  Transfer function  →  Blend
        (a field)        (field → RGBA)        (over the base)
```

You never write "a slope shader" and "a mineral shader" and "a connectivity
shader." You implement a **fixed, small set of transfer functions once**, and every
scientific layer is a `(data source, transfer function, parameters)` triple. Adding
temperature or illumination later is a data channel plus a colormap — no new code.

This split is also what makes **real-time tuning free**: bake the *data* once (a
texture), evaluate the *transfer* every frame from **uniforms**. Changing a critical
slope angle, a colormap, or an opacity is a uniform write — instant, no re-bake.

## Fields are data; render is one consumer

The single most important rule: **an analysis layer's field is computed CPU-side as
data, and rendering is one downstream consumer of it — never its definition.** A
slope map is not "a shader that colours by `normal.y`"; it is a **`SurfaceField`** —
`value_at(x, z) -> f32` (scalar or categorical), evaluated purely on the
`SurfaceOracle`, headless, deterministic, content-keyed. Slope, aspect, elevation,
AO, hazard, and connectivity are all `SurfaceField`s — the data-side parallel to the
height channel's `HeightSource`.

This is the same relationship height already has: the mesh is the *shadow* of the
oracle, not the source of truth. One field, many consumers:

| Consumer | Uses the field for |
|---|---|
| **render** | materialise to overlay tiles (the tiled model) → transfer → colour |
| **query** (API / rhai / MCP) | point / region sample — **headless, no GPU** |
| **planner / cosim** | slope → traversability, hazard → no-go, connectivity → comms windows |
| **USD** | a `lunco:layer = "field"` prim *describes* it → addressable / composable / replicated |

Consequences that shape everything below:

- **Author the field once, in Rust, on the oracle** — not in WGSL. It then runs
  headless (the field exists with no renderer), is unit-testable and deterministic,
  and bakes to tiles for the GPU. `derive.rs::slope_map` + the oracle's `normal_at`
  are already exactly this; `query.rs` already returns `slope` for a point with no GPU.
- **WGSL only samples + colourises.** For a purely geometric field a shader *may*
  re-derive it live (zero-bake), but the Rust stays authoritative — a determinism
  contract like the tile bake, not a second definition.
- **A field is a USD layer, describe-don't-store** — `{ field, params }`, no pixels;
  any tool resolves it by prim path, queries it headless, or renders it. A dynamic
  field (connectivity) adds only a time binding.

So a slope "view" is the *last, optional* step: the field is real data first, useful
to headless tools, the planner, cosim, and scripting whether or not anything draws it.

## Two planes: shading vs annotation

An "overlay" is composited in one of two places. Choosing the right plane per layer
is the first design decision.

| Plane | What it is | Belongs here | Mechanism |
|---|---|---|---|
| **1 — in-material** | recolours the surface *per pixel*, lit + occlusion-correct, inside the terrain tile fragment shader | albedo/mosaic texture, mineral map, slope/AO/roughness/hazard, elevation ramp, connectivity coverage | a data texture + transfer uniforms on the tile `ShaderMaterial`; live via `Changed<…>` re-apply (as `TerrainDerivedMaps` already does) |
| **2 — on-top** | *marks/annotates* over the terrain as its own geometry, independent of the terrain material | lat/lon graticule, region/ROI boundaries, traverse paths, landing rings, coordinate labels | `Gizmos` (enabled) for lines; an unlit overlay mesh for conformal fills; egui world-anchored labels (the rover name-tag pattern) |

Rule of thumb: **does the layer change how the ground *looks under light* (Plane 1),
or does it *draw a mark on top* (Plane 2)?** The lat/lon grid is Plane 2 — it must
work in every shader mode, toggle per-viewer, and needs no material change.

## Data sources — including dynamic, time-dependent ones

A **data source** is a field sampled over the terrain with a **type** (scalar /
categorical / vector) and a **refresh policy**:

| Kind | Example | Refresh |
|---|---|---|
| **static baked** | slope, elevation, AO, hazard | bake once off-thread, content-addressed by `SurfaceOracle::surface_key` (like `derived_layers`) |
| **authored raster** | mineral classification, FeO/TiO₂ abundance, NASA albedo mosaic | load a USD-referenced image once |
| **in-shader derived** | slope = `acos(normal.y)` | a *render optimisation* of a geometric field — zero bake, but the field's authoritative definition stays the CPU `SurfaceField` (headless), which the shader must match |
| **dynamic / sim-driven** | **connectivity, illumination-over-time, temperature** | recompute off-thread on a **cadence**, driven by sim time / ephemeris / observer state |
| **manual / painted** | user-drawn regions, a custom hazard / no-go map, hand annotations | edited in-app via the **brush pipeline**; persisted as a USD layer — a sparse paint field or an authored raster (see *Manual maps*) |

The dynamic kind is the only structural addition beyond a static texture stack: a
`Dynamic { driver, cadence }` provider re-evaluates its data texture on a throttled
cadence (the same async-bake + debounce machinery the horizon and derived-map bakes
use, re-triggered by *time* instead of an oracle swap). Everything downstream —
transfer, blend, legend, controls — is identical to a static layer.

### Connectivity ≡ the horizon-shadow cache, generalised

The sun-visibility `HorizonShadowCache` (an R8 "is the sun above the local horizon
in direction D at time t" texture) **is already a dynamic, time-dependent coverage
map.** A **connectivity map is the same computation with a comms target instead of
the sun**: per terrain cell, is the relay satellite (position from the deterministic
ephemeris) / Earth / a base station above the local horizon — the `HorizonMap`
oracle already gives the skyline per direction — and within range/SNR at sim-time
`t`? Output a coverage scalar `0..1` (or categorical connected / marginal / blocked).

So connectivity, illumination-over-time, and Earth-visibility all reuse the existing
**horizon oracle + ephemeris**, and connectivity specifically should **derive from
the existing comms/connectivity substrate** (`comms-connectivity` design), not
reimplement line-of-sight.

### Determinism & caching for dynamic maps

A dynamic map is keyed on `(surface_key, time-bucket, targets)` → content-addressable
**per time bucket** → cacheable and **peer-deterministic**: at the same virtual
sim-time every peer computes the identical map, so it replicates as spec+hash, never
pixels (the ephemeris is deterministic). Cadence tracks how fast the *driver* moves
(satellite motion), not the frame rate; scrubbing `lunco-time` updates the map on
cadence (respecting pause ≠ speed-0). Aggregation variants fall out for free:
instantaneous coverage, time-integrated ("% of a lunar day with comms"), or forecast
("coverage over the next orbit") — all just how the provider reduces over time.

## Tiled overlay layers — LOD, coverage, and memory in one move

The naïve delivery — one full-terrain draped texture per layer — fails three ways at
once: it can't match the terrain's sub-5 m *near* tessellation (blur), it can't express
a layer that covers only part of the site, and N such textures blow the memory budget.
**Tiling the overlay data the way the terrain is already tiled solves all three
together** — the Cesium / planetary-renderer approach: an imagery quadtree over the
terrain quadtree.

Each layer's data is a **tile pyramid keyed by the terrain's `QuadCoord`.** A resident
terrain tile at `(depth, x, z)` samples its layer's tile at the matching coord (or the
nearest coarser ancestor the layer provides). Consequences:

- **LOD parity — free.** The overlay tile's resolution tracks the terrain tile's
  tessellation depth: crisp near, coarse far, refining as you move. No draped-texture
  blur, no giant full-res texture.
- **Partial coverage — intrinsic.** A terrain tile with *no* tile in a layer → that
  layer contributes nothing there (transparent). **Missing tile = NoData — no separate
  mask.** Ragged edges *within* a tile use a per-texel **alpha / coverage** channel in
  the overlay tile. So: tile-presence = coarse coverage; per-texel alpha = fine coverage.
- **Memory — view-bounded.** Overlay-tile residency = terrain-tile residency: stream a
  layer's tiles with the terrain tiles that need them, evict on tile eviction. Memory
  scales with `view × active layers`, not `full terrain × all layers`, and reuses the
  terrain tile cache / OPFS / streaming machinery wholesale.
- **Multimap — natural.** Each layer owns its pyramid, extent, max-depth, and CRS. A
  landing-site mineral inset has tiles only there; a global map has them everywhere;
  each contributes only where it has coverage. Different source resolutions → different
  max pyramid depth per layer (a layer clamps / over-zooms to its finest tile beyond its
  own resolution, exactly like the DEM over-zoom).

The tile is the unit of **bake, cache, stream, and dirty** — identical to terrain tiles
— so every data-source kind slots in per-tile: *derived* (slope/AO baked per tile),
*authored* (sliced/resampled from a source raster into the tile), *painted* (the brush
writes the tiles under its footprint, dirtying only those), *dynamic* (connectivity
computed per tile per time-bucket, cached on `(tile, time-bucket)`). A simple small-site
layer degenerates to a **one-tile pyramid** (a single draped texture at depth 0), so
tiling never taxes the easy case.

**External geodata** maps straight onto this: an external tiled source (WMTS / slippy)
*is* a pyramid; an untiled GeoTIFF is reprojected + sliced into the terrain pyramid on
ingest (the georef reprojection step). Either way the renderer only ever sees
terrain-aligned tiles.

## Efficiency & performance — design invariants

- **Derive in-shader where the field is geometric** (slope/aspect from `normal`,
  elevation from position): zero bake, per-pixel, no texture, no LOD mismatch. Reserve
  baked/streamed tiles for data not recoverable from geometry (minerals, AO,
  connectivity, paint).
- **Connectivity reuses the baked horizon skyline, never a fresh ray-march.** Per cell:
  `target_elevation(t) > horizon[azimuth]` — a LUT read off the existing `HorizonMap`,
  on a coarse tile depth (coverage varies slowly), not per-pixel. A naïve per-cell
  ray-march per target per time-bucket would be ~1 M marches per recompute.
- **Dynamic-map recompute runs in the web worker on wasm**, not the AsyncCompute pool
  (which degrades to the page main thread and would freeze the tab — the DEM-decode trap).
- **One uniform-driven pipeline, not a permutation per active-layer-count / transfer.**
  Branch on an `active_layers` uniform; keep all transfer/blend params in uniforms so a
  tweak never mints a new material in the `(entity, mode, depth, band)` cache or triggers
  a first-use pipeline compile (stutter).
- **Paint writes partial tiles** — rasterize a stroke into the AABB of the tiles it
  touches (`write_texture` on the dirty rect); never re-upload a whole layer.
- **The graticule is a retained line mesh** rebuilt only on anchor/zoom change once it is
  dense; per-frame gizmos stay fine only for a sparse grid.
- **Layer residency is explicit** — a layer's tiles bake/upload only while it is visible;
  toggling a layer off evicts its tiles.

## Manual maps — drawing on the terrain

A hand-drawn map is the **same edit pipeline the height brush already uses**
(`terrain-substrate.md` → *Dynamic modification*), retargeted from the height channel
to a data channel (raster paint) or to overlay geometry (vector annotation). Two forms:

- **Raster paint (Plane 1).** A paintable data source. The brush — reusing
  `raycast_surface` onto the oracle, the tool palette + arm gate, and the footprint
  math — writes values into a **`SparsePaintField`**: a bounded sparse edit-raster of
  touched cells, the paint analogue of the deferred `SparseEditField` for height. It
  feeds any transfer (paint colours directly = Passthrough; paint a scalar "hazard
  0–1" → Ramp). This is how a user hand-authors a custom no-go / region / hazard map.
- **Vector annotation (Plane 2).** Click to drop vertices on the surface
  (`raycast_surface` → oracle hit) → a waypoint / traverse route / ROI polygon, stored
  as USD prims, drawn as gizmos / overlay mesh.

Everything the height-edit story provides comes for free:

| Editing need | Provided by |
|---|---|
| Undo / redo, history | the USD journal — revert the op |
| Multiplayer sync | the stroke is a journaled + replicated `lunco:layer` op |
| Permissions | layer-scoped RBAC on the terrain prim |
| Non-destructive | edits ride the session/runtime layer over an untouched base |
| Commit granularity | a paint drag edits the runtime texture live; authors **one** USD op on release (a click authors at once) — never per-frame |

**Replication caveat.** Parametric strokes (a circular brush at centre/radius/value)
replicate as a tiny **spec**, like the height brush. But **freeform per-pixel paint is
genuine data** — it cannot be spec'd — so a painted layer ships as a bounded sparse
raster / authored image asset (exactly like the mineral mosaic), not as parameters.
Keep it sparse (touched cells only) and content-addressed.

## Transfer functions — a fixed set, all uniforms

The transfer maps `data → RGBA`, fully parameterised by uniforms (so tuning is
instant). Four cover everything:

| Transfer | Params (uniforms) | For |
|---|---|---|
| **Ramp** | colormap LUT (256×1), domain `[min,max]`, gamma | continuous scalars — elevation, abundance, coverage |
| **Threshold / Classify** | stops, **critical value(s)**, discrete\|smooth | **slope hazard** (green < safe, amber < critical, red beyond), connectivity connected/marginal/blocked |
| **Palette** | class → colour table | categorical — mineral classes |
| **Passthrough** | — | an authored RGB mosaic, no remap |

Concretely: **slope-hazard = slope data + Threshold (critical angle uniform)**;
**minerals = classification raster + Palette**; **elevation = elevation data + Ramp**;
**connectivity = coverage data + Ramp or Threshold (min-SNR uniform)**.

Unify the slope-viz threshold with the sim's actual hazard classification
(`hazard_from_slope`'s safe/cliff angles in `lunco-terrain-core/src/derive.rs`) so the
coloured overlay shows *exactly* the hazard the rover planner uses — one source of
truth, live-tunable for both.

## Real-time parameter control — one param, three surfaces

Every transfer/blend parameter is a **reflected uniform** on the dynamic
`ShaderMaterial` (the `//!@`-annotated self-describing material, same path
`terrain_debug.wgsl` uses). Exposing a param as a reflected uniform gives, for free:

- **Inspector** — a slider (critical angle, opacity, domain, colormap picker);
- **Scripting / API** — `cmd("SetObjectProperty", { critical_angle: 0.35 })` over rhai / HTTP / MCP;
- **USD** — the layer prim carries defaults (`lunco:layer:slope:criticalAngle`); live tweaks ride the runtime/session layer, journaled + replicated like any edit.

Changing the value writes a uniform → instant, no re-bake. Changing the data channel
swaps the sampled texture; changing the colormap swaps the LUT.

## Legends

Each transfer function *implies* a legend: Ramp → gradient bar + domain labels;
Threshold → safe/caution/hazard bands with the live angle; Palette → mineral swatches
+ class names. An egui legend panel reads the active layer's transfer function and
re-renders as parameters change.

## Extensibility — reuse the USD layer stack

Layers ride the existing `TerrainLayerStack` (`lunco:layer` child prims, folded in
prim order). Extend the `TerrainLayer` contribution kinds — today height_modifier /
stamp / scatter / configure — with two rendering arms:

- `texture_layer() -> Option<TextureLayer>` → folds into the **in-material** stack (Plane 1);
- `overlay_layer() -> Option<OverlayLayer>` → spawns/updates an **on-top** entity (Plane 2).

A new layer type = impl `TerrainLayer` + a parser, registered via
`add_terrain_layer("<type>", parser)` — no changes to the build/scatter/regen
systems. `Changed<TerrainLayerStack>` makes add / remove / reorder **live**, and the
`LayerId` (USD prim path) is the addressable handle.

**The material constraint & fix.** `ShaderMaterial` today has fixed named
`#[texture(N)]` slots (height/albedo/mineral/surface/normal/shadow), already tight
against WebGPU binding limits — it cannot grow one slot per dynamic layer. For an
arbitrary-N in-material stack, replace the growing named slots with **one
`texture_2d_array` binding + a per-layer uniform array** `[{ weight, blend_mode,
transfer, params }; MAX_LAYERS]`, where — under the tiled model above — each array
slice is *this tile's* overlay tile for that layer (small, view-resident), not a
full-terrain texture. The fragment loops the active slices, generalising the current
fixed `mix()` sequence. Fold order = array z-order; a slice with no resident tile is
skipped (its layer is transparent here — intrinsic NoData).

## Georeferencing — matching real lunar terrain

Overlays that carry lat/lon (the graticule) or geographic data (minerals,
connectivity) need a projection that matches the source data. Do **not** pick one
arbitrarily — match the DEM's own CRS and the lunar cartographic standard, and store
it in `TerrainGeoref`:

- **Reference body:** a **sphere, R = 1737.4 km** (IAU mean lunar radius — the Moon
  is mapped as a sphere, not an ellipsoid), in the **Mean Earth / Polar Axis (ME)**
  body-fixed frame = `IAU_MOON`. Keep this consistent with the celestial body's frame
  (`CelestialBody::radius_m`) so the surface DEM registers onto the globe correctly.
- **Projection — read from the DEM, per site:**
  - global / equatorial products (LOLA LDEM) → **Equirectangular** (simple cylindrical);
  - **polar / Artemis south-pole sites** (Shackleton, Connecting Ridge) → **Polar
    Stereographic** (the LOLA polar-DEM standard). Equirectangular and the flat
    tangent-plane approximation both degenerate at the poles, so the site DEMs in play
    need real polar stereographic.
- `TerrainGeoref` should therefore carry: projection type
  (`Equirectangular | PolarStereographic { south }`), centre lat/lon, reference
  radius, scale / standard parallel, and implement **forward + inverse**. The DEM's
  GeoTIFF tags already declare all of it — read them
  (`lunco_geotiff` / `dem::read_geotiff_transform`), never restate them. The inverse
  (XZ → lat/lon) is what both the graticule *and* the orbit→surface
  `CompositeHeightSource` need, so implement it once here.

## Coordinate grid — Plane 2

A `TerrainGraticule` component + a system that steps integer lat/lon lines, maps them
to XZ via `TerrainGeoref`, samples the `SurfaceOracle` for Y so the lines hug relief,
and draws them with `Gizmos`. Per-viewer toggle, no material change, works in every
shader mode.

## Phasing

> **Implementation status (2026-07-12).** The headless **data + transfer** half of Phase 1
> plus the render VIEW are landed and verified on the streamed DEM:
> - `SurfaceField` (`lunco-terrain-core::field`) — slope / aspect / elevation as pure,
>   headless, deterministic fields; `field_map` materialises a region raster.
> - `TransferFn` (`lunco-terrain-core::transfer`) — Ramp / Threshold / **SlopeHazard**
>   (live critical-angle, reusing `hazard_from_slope`) / Palette; `sample(v) → Rgba`,
>   shared by the shader, the legend, and any headless export so they always agree.
> - `TerrainField` query (`lunco-terrain-surface::query`) —
>   `query("TerrainField", {field, x, z, half, res})` returns a headless region raster.
> - **Overlay VIEW** — `TerrainOverlayParams` + `SetTerrainOverlay` command +
>   `sync_terrain_overlay` live-tune, and overlay uniforms + the slope-hazard blend in
>   `terrain_geomorph.wgsl` / `_web.wgsl` (slope from the geometric normal, running the
>   same transfer math on-GPU as `transfer.rs`).
>
>   **Every `SetTerrainOverlay` field is `Option<T>`** — `enabled`, `safe_deg`,
>   `cliff_deg`, `opacity`. An omitted field means *leave it alone*. When `enabled` was
>   a plain `#[Command(default)]` bool it was written unconditionally on every call, so
>   `{"cliff_deg": 25}` **silently switched the overlay off** while appearing to tune
>   it. A partial-update command whose fields are not optional cannot express "change
>   only this" — it can only express "set everything, defaulting what you didn't say."
> - **Inspector legend + sliders** (`lunco-sandbox-edit::ui::inspector`) — enable +
>   Safe/Cliff/Opacity sliders + a gradient legend coloured by the same
>   `hazard_from_slope` + `hazard_color`, so the legend matches the terrain exactly.
>
> Remaining in Phase 1: the `lunco:layer = "field"` USD descriptor + tiled materialisation
> (the field is headless-queryable but not yet an addressable USD layer). Phases 2–5
> (graticule, tiled overlay stack, dynamic maps, manual paint) are unstarted. The overlay
> paints the **streamed CDLOD** terrain (`terrain_geomorph`); a statically-meshed DEM
> (`lod_viz = false`) would need the same uniforms wired into its material.

1. **Slope as a `SurfaceField` (data-first, headless).** Define slope as a field on the
   oracle (reuse `derive.rs::slope_map` + `normal_at`); the point query already returns
   it. Add a `lunco:layer = "field"` USD descriptor `{ field: slope }` so it's
   addressable / headless-queryable by other tools, and region/tile materialisation.
   *Then* the view: Threshold transfer (live **critical-angle** uniform defaulting to
   `hazard_from_slope`'s safe/cliff angles) → Blend, inspector slider + legend. The
   render is the last step; the field is usable headless first. Terrain crates + sandbox-edit.
2. **Lat/lon grid.** `TerrainGeoref` forward/inverse (equirectangular first) + gizmo
   graticule + toggle.
3. **Tiled overlay stack.** The per-tile pyramid + residency (reusing terrain tile
   streaming) + `texture_2d_array` + per-layer uniform array + the `texture_layer()`
   contribution kind + `lunco:layer = "texture"` parser + shader loop. Delivers LOD
   parity, intrinsic coverage/NoData, and view-bounded memory; unlocks minerals
   (authored raster tiled on ingest + Palette + legend) and any N tiled layers.
4. **Dynamic maps.** The `Dynamic { driver, cadence }` provider + connectivity layer
   (reusing the horizon oracle + ephemeris + comms substrate) + illumination-over-time.
   Polar-stereographic georef for the polar sites lands here if not before.
5. **Manual maps.** The paint brush (raster, reusing the height-brush pipeline —
   `SparsePaintField` + USD authoring) and vector-annotation drawing (Plane 2).
   Independent of Phase 3/4 — it can follow Phase 1 since it reuses the existing edit tools.

## Resolved by the tiled model

- **Overlay LOD parity** — overlay tiles refine with the terrain quadtree (was the
  biggest technical gap).
- **Coverage / NoData / partial maps** — missing tile = transparent; per-texel alpha for
  ragged edges. No separate mask system.
- **Memory for N layers** — view-bounded residency reusing terrain tile streaming.
- **Multiple simultaneous overlays** — per-tile `texture_2d_array` slices.

## Still open (designed-but-not-detailed / analyst-facing half)

- **External geodata ingest** — the reproject + crop + resample of a real mineral /
  compositional product (own CRS, resolution, extent) into the terrain tile pyramid. The
  practical blocker for authored geodata; needs a georef-aware tiler.
- **Data probe / readback** — hover/click reads the layer value ("slope 22°, anorthosite,
  comms blocked") and highlights the legend. Extends `query.rs` (height/slope today) to
  sample the active layers. Core analyst tool, not yet specced.
- **Layer-manager UI** — the GIS "layers panel": list / toggle / opacity / reorder / solo
  per layer, with legends. The stack + inspector exist; the management surface does not.
- **Lit-vs-unlit compositing** — analysis false-colour is usually **unlit** (slope reads
  the same regardless of sun) over the lit base; a per-layer lit/unlit toggle + the blend
  math need pinning down.
- **Time-control UX + range integration** — a scrubber tied to dynamic maps, and the cost
  model for "integrate coverage over an orbit / lunar day" (many time samples).
- **Dynamic-map cadence** — fixed sim-time step vs adaptive driver-motion threshold
  (satellite angular rate); likely the latter.
