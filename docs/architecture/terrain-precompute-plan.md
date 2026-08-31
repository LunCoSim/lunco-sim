# Terrain: precomputed tiles + monotone progressive refinement

> Status: Design · Audience: contributors on terrain streaming, LOD, and bakes
>
> The measurement steps are live tests:
> `lunco-terrain-surface/tests/precompute_sparse_set.rs` (step 1) and
> `precompute_bake_time.rs` (step 2).

Target architecture for the streamed terrain. It replaces the current
runtime-bake selection path with a coarse resident cover and progressive
refinement.

## Constraints this must satisfy

These are load-bearing and they eliminate most of the design space:

| constraint | consequence |
|---|---|
| Runs **headless on machines with no GPU** | geometry truth cannot live in a vertex shader |
| **Physics fidelity** where bodies touch ground | colliders sampled from the real surface, never from interpolated parent data |
| **Queries anywhere** (mouse-spawn at distance) | the surface must be answerable where no geometry is realised |
| **Weak machines** | triangle count must fall with distance; no main-thread bakes (wasm has no worker threads) |
| **Deterministic across peers** | surface is a pure function; derived artifacts content-addressed |
| **Caves / overhangs later** | the 2.5D `height_at(x,z)` assumption must be replaceable |

`SurfaceOracle` — one pure CPU analytic surface every consumer samples — satisfies all of them.
It stays. Anything that makes the GPU authoritative breaks headless, far queries, and
physics agreement simultaneously.

The problem was never the oracle. It is the **runtime bake pipeline layered on top of it**.

## What the industry actually does

The design is backed by the live sparse-set and bake-time measurements named
above, plus the terrain streaming implementation and its existing external
references (Cesium, MSFS, and geometry clipmaps).

- **Cesium `ForbidHoles`** — *"unrefine back to a parent tile when a child isn't done
  loading… never rendered with holes, though the tile rendered instead may have low
  resolution."* Draw the parent **instead of** its children, so the cover stays exact and
  disjoint. Not an underlay.
- **MSFS / Asobo (GDC)** — *"Draw tiles using the best currently available data ● Tiles can
  use data from a parent."* Blur, never nothing. Asobo explicitly **deny** pre-downloading:
  the answer to latency is fallback, not prefetch.
- **Geometry clipmaps (Losasso & Hoppe)** — coarse levels are **always resident**; only the
  *finest* levels are deactivated, and for aliasing reasons rather than memory.
- **Cesium `wasCreatedByUpsampling()`** — **blur the picture, never the ground.** A collider
  must never be cooked from upsampled parent data.

Convergent answer: *always have a coarse cover; substitute a parent when a child is not
ready; never let physics see the substitute.*

## Design

### 1. Precompute, not runtime bake

Tile geometry is already a pure function of `(oracle, coord, res)` and the cache is already
content-addressed — so the runtime bake is not necessary, it is merely the precompute
happening at the worst possible moment. Precompute writes the same cache the runtime reads.

- ~~**Sparse and error-driven.**~~ **MEASURED AND REJECTED.** See
  [the measurement](#measured-there-is-no-sparsity) — error-driven sparsity does not exist on
  this terrain, so precompute covers the COARSE BASE ONLY and deep levels stay runtime-baked.
- **Coarse first.** Bake depths `0..=N` before anything deeper, so the terrain is *complete*
  at coarse quality almost immediately and sharpens from there.
- **Resumable and shareable for free.** Content-addressed keys ⇒ resume = skip existing keys,
  and a prebaked cache ships with the twin, so a weak or headless machine never bakes at all.
- **A real job:** tiles done/total, %, ETA, cancellable — surfaced, not a silent stall.
- **Covers all oracle-derived artifacts:** tile meshes, collider-ring tiles (this is what
  removes far-spawn collider latency), derived layer maps, horizon heightfield.
- **Incrementally re-runnable** over an edited region — `invalidate_region`'s bounds logic
  already expresses the "what did this edit touch" query.

### 2. Runtime: monotone refinement toward the camera

1. **Coarse base always resident.** Depths `0..=N` never evicted ⇒ the terrain is *always
   completely covered*, from the first frame, on any machine. Holes become structurally
   impossible instead of something to compensate for.
2. **Refine by priority, nearest-first** — a queue ordered by projected screen error ×
   proximity. Pull the next deeper tile from cache, swap it in. Quality improves outward
   from the camera.
3. **Only improve.** Never coarsen in reaction to a per-frame budget wobble — only when the
   camera has genuinely moved away (hysteresis) or under memory pressure, evicting
   furthest-first. *Monotone improvement is what makes it look stable.*
4. **Swap atomically.** Parent → its four children only when all four are resident. No
   partial refinement. Trivially satisfiable once tiles are cache hits.
5. **Physics never sees a substitute.** Visual tiles may fall back to a coarse parent;
   collider tiles are always real. Carry Cesium's upsampling flag: a tile created by
   substitution is display-only.

### 3. Budget enforced incrementally, not globally

This is the crux, and it is the fix for the worst bug measured in the old path.

Today `pixel_error` is re-derived every frame to satisfy a tile budget, and **any** change to
it re-selects the entire cover — measured on moonbase as `wanted` alternating **349 ↔ 532
every frame**, with `vis_flips` spiking to 55: hundreds of tiles despawned and respawned per
frame. A dwell timer damps that, but it is a band-aid on a design that should not exist.

With a priority queue the budget is enforced by **adding or evicting a few nodes per frame**.
There is no global metric to change, so there is no mass republish — the failure mode is gone
by construction rather than suppressed.

## What this deletes

All of it exists only to hide bake latency, and none of it survives having no latency:

- covers / hand-off / readiness gating / coarse backdrop
- the global budget-fit loop, its hysteresis, and its dwell timer
- bake backlog on the visible path (runtime bake stays only as a cache-miss fallback)

## Measured: there is no sparsity

`tests/precompute_sparse_set.rs`, against the real moonbase DEM (3200², ±8000 m, 5.00 m
posting), error floor 0.05 m, max depth 8:

```
 depth        tiles     node m   cumulative
     0            1    16000.0        0.2 MB
     1            4     8000.0        0.8 MB
     2           16     4000.0        3.2 MB
     3           64     2000.0       13.0 MB
     4          256     1000.0       52.3 MB
     5         1024      500.0      209.5 MB
     6         4096      250.0      838.2 MB
     7        16384      125.0     3352.9 MB
     8        65536       62.5    13411.6 MB

TOTAL 87381 tiles ≈ 13411.6 MB   (157 KB/tile at 49² verts)
uniform tree to depth 8 = 87381 tiles — sparsity is 1.0× smaller
```

**Every node clears the error floor, so the tree refines fully everywhere.** Lunar regolith is
fractal — rough at every scale — so there is no "flat enough to stop" region. The premise that
error-driven refinement yields a content-shaped sparse tileset is **false for this terrain**.

Two consequences:

1. **Precomputing the whole tileset is dead.** 13.4 GB per site is not shippable, and it does
   not shrink with cleverness: the camera can walk anywhere, so any node may be viewed close.
2. **Runtime baking cannot be deleted.** Deep levels must stay on-demand. Therefore the
   latency-hiding machinery still has a job — but it now has something it never had before:
   *a guaranteed coarse fallback*.

### Corrected design: precompute the coarse base only

| N | tiles | disk | node size |
|---|---|---|---|
| 3 | 85 | 13 MB | 2000 m |
| **4** | **341** | **52 MB** | **1000 m** |
| 5 | 1365 | 210 MB | 500 m |

`N = 4` (341 tiles, 52 MB) is the working choice: small enough to ship with a twin and hold
resident forever, deep enough that the fallback is 1 km tiles rather than a 16 km blur.

This keeps the parts of the design the measurement did not touch — always-resident coarse
base, unrefinement to a ready ancestor, monotone refinement toward the camera, incremental
budget — and drops only the claim that *everything* could be precomputed. What the coarse base
buys is the thing that was actually broken: **the fallback always exists**, so a not-yet-baked
deep tile degrades to blurry (MSFS's "best currently available data") instead of to black.

## Measured: bake the coarse base at scene open, async

`tests/precompute_bake_time.rs`, real DEM, real `bake_tile_mesh`, single-threaded:

```
 depth    tiles     total ms      ms/tile
     0        1          0.6         0.61
     4      256        159.7         0.62

TOTAL 341 tiles in 236 ms serial (0.69 ms/tile)

native, 32 cores (ideal)   ≈   7 ms
native, 4 cores (weak)     ≈  59 ms
wasm, single-threaded MAIN ≈ 236 ms   (no worker pool)
wasm at 3x native slowdown ≈ 707 ms
```

Worst case — wasm, main thread, 3× slowdown — is **~0.7 s against a ~2 s scene-open budget**.

**Decision: bake the coarse base at scene open, asynchronously.** No prebaked artifact ships,
so there is no cache-invalidation or staleness story: a terrain edit simply re-bakes 341 cheap
tiles. Async so even 236 ms never lands in one frame; on wasm it spreads across frames on the
main thread, and until it completes the terrain renders whatever depth has landed (coarse
first, so it is complete and blurry rather than absent).

## Open questions still to measure

1. **Deep-level working set**: how many depth 5–8 tiles are live near a camera at once? That
   sizes the runtime cache and the eviction policy.
2. **Tile resolution.** 157 KB/tile at 49² is heavy. Larger tiles = fewer, fatter tiles and
   less per-tile overhead; worth a sweep before fixing the format.

## Runtime readiness and derived visual work

The DEM build is the ground-readiness boundary. Once its composed heightfield, visual mesh,
and collider are published, the Twin has usable ground; optional derived surface/normal maps
are visual refinement and must not hold scene presentation or replacement hostage.

The derived baker uses the existing `RenderingQualitySettings` profile, but a static terrain's
effective map resolution is also bounded by its authored visual target and source grid. A
32×32 static product therefore does not launch a 1024² derived bake that cannot add visible
detail. Streamed terrain keeps the quality-profile resolution because its geometry refines
independently of the static product. The cache key includes this effective profile, so changing
the target or quality creates the correct artifact rather than reusing an incompatible one.

Every derived task is scene-owned. `SceneTeardown` drops task handles, published maps, stale
markers, failure markers, and the derived status before the replacement Twin mounts. Each task
owns its liveness start time, so the shared terrain work bound applies independently to every
terrain rather than making a late-queued terrain inherit an older task's age. A bake that stops
making progress is cancelled and emits a warning; the ground remains usable and the failure is
terminal for that product until the terrain or quality changes. The status bus mirrors terrain
streaming and derived preparation through separate sources and appends phase transitions, while
queued work remains indeterminate instead of displaying a fabricated percentage.

The native and Web Worker DEM paths also share one `HeightGrid` content-key owner. The worker
returns the key alongside its transferred full grid, so the Web path does not fold a
multi-million-sample DEM again on the main thread when it publishes the refined surface.

The status mirror exposes the active tile set through one progress entry and emits a terminal
event with the current resident/wanted counts when that entry completes. It does not retain a
historical start message as the live result; after progress removal the status bar otherwise
falls back to an obsolete count and makes a settled stream look blocked.

## Sequencing

1. **Measure the sparse set** (read-only; cannot regress anything). Answers all three open
   questions and fixes `N`.
2. **Precompute job** — coarse-first, sparse, resumable, progress-reported.
3. **Runtime swap** — resident coarse base + priority refine queue + incremental budget.
4. **Delete the latency machinery** — justified by a *measured* zero backlog under motion.

Step 4 is last on purpose: the deletion has to be earned by evidence, not by assuming the
new path works.

## Later: caves and non-heightfield detail

`height_at(x,z) -> y` cannot express an overhang, so caves are not an extension of the
heightfield — they replace it locally, with `field(x,y,z) -> density` and surface extraction
(dual contouring / marching cubes; cf. No Man's Sky).

What matters is that **the pipeline shape survives**:

```
field → extraction → cached mesh → { visuals, colliders, queries }
```

That is what exists today (`oracle → bake_tile_mesh → cached TileMesh → tiles + collider
ring`). A volumetric region swaps the *field type* and the *extraction algorithm*; caching,
determinism, LOD selection, and the physics path keep working. Heightfield stays the fast
path for the ~99% that is ground.

The thing to design early is therefore not voxels — it is the **seam**: how a region declares
"I am volumetric here" so field type and extractor can vary per region without the consumers
knowing.
