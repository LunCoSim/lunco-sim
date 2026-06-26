# lunco-obstacle-field — procedural obstacle-field generator for rover testing

## Goal
Generate **crater + rock fields** on the fly with tunable distribution parameters
(density, size distribution, spatial pattern, seed) so rovers can be tested across
varied surface conditions. Must be **deterministic** (same spec+seed → same field,
so networking replicates only the spec), **efficient** (background generation,
size-bucketed shared assets, chunk streaming, LOD), and **bakeable** (cache to disk
so an experiment sweep reuses each generated field).

## Key facts (from codebase audit, 2026-06-26)
- Rover drives on a **flat 4 km Cube slab at y≈0** (`assets/scenes/sandbox/sandbox_scene.usda:16`),
  NOT a heightfield. The DEM-heightfield path (`lunco-usd-avian/src/lib.rs:355`) is dormant;
  the quadsphere (`lunco-terrain`) is render-only (no colliders).
  → We **generate the height array ourselves**. Craters = bowl functions written into it.
  Build the Avian `Collider::heightfield(Vec<Vec<f64>>, scale)` directly from that array.
  Analytic `height_at(x,z)` (bilinear) gives off-thread rock-placement heights — no raycasts.
- Async pattern: component/resource holds `bevy::tasks::Task<T>`, spawn on
  `AsyncComputeTaskPool::get()`, poll with `block_on(poll_once)` (`lunco-cache/src/lib.rs:55`,
  `lunco-usd/src/commands.rs:278`). ⚠️ wasm runs the pool on the main thread → chunk + yield.
- No `rand` in workspace yet → add `rand` + `rand_chacha` (seed `ChaCha8Rng` from u64,
  no getrandom at runtime → wasm-safe).
- Experiment spec (`lunco-experiments`) varies **Modelica scalar params only** — does NOT fit a
  geometry spec. ObstacleFieldSpec lives as a Bevy `Resource` in this crate, optionally
  authored from a USD prim attr (`lunco:obstacleField:*`) on the terrain prim.
- Tuning UI seam: `lunco-sandbox-edit/src/ui/inspector.rs` (egui `Panel` trait, `Slider` pattern).
- LOD: Bevy 0.18 `VisibilityRange` (used nowhere today). Inject per mesh tier.

## Architecture — layered, pure core + Bevy plugin
- `spec.rs`   — `ObstacleFieldSpec`, `SizeDist` (log-normal), `Pattern` (Uniform/PoissonDisk/Clustered),
                `CraterLayer`, `RockLayer`. serde + Reflect. THE knobs.
- `sampler.rs`— deterministic placement sampling (ChaCha8Rng) → `Vec<Placement>`. Pure, unit-tested.
- `field.rs`  — `HeightGrid` (heights `Vec<f64>`, dims, region) + crater bowl stamping +
                bilinear `height_at`. Pure, unit-tested.
- `assets.rs` — size→bucket quantization (N shared meshes/colliders, instances scale via Transform). Pure.
- `bake.rs`   — cache key = hash(spec+seed) → persist placements via `lunco-storage`. (later phase)
- `plugin.rs` — Bevy: `ObstacleFieldSpec` resource; background gen task; build heightfield collider +
                visual mesh; scatter rocks (static merged + dynamic_fraction as PredictedDynamic);
                chunk streaming; `VisibilityRange` LOD.
- `ui.rs`     — egui tuning panel (CollapsingHeader + sliders), feature `ui`.

## Phases
1. **Core (this commit)** — crate scaffold; `spec` + `sampler` + `field` + `assets` pure modules with
   unit tests; compiling `ObstacleFieldPlugin` skeleton that, from a spec resource, builds the
   heightfield collider + visual mesh and scatters bucketed static rocks with `VisibilityRange` LOD.
2. **Streaming + dynamics** — chunk grid, distance-based spawn/despawn, `dynamic_fraction` rocks as
   `PredictedDynamic` replicated props. Merge static rocks per chunk.
3. **Tuning UI** — live sliders (density/size/pattern/seed) → debounce → background regen → swap.
4. **Bake/cache** — disk cache keyed by spec hash; instant reload; sweep-friendly.
5. **Experiment sweep** — drive spec from scenario/batch so rover perf is measured per condition.
6. **USD authoring** — `lunco:obstacleField:*` prim attrs parsed alongside terrain build.
```
