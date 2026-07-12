# Render decoupling — the material is the boundary

**Goal:** a headless build (`--no-ui`, the wasm worker, every integration test) that does not link
wgpu, naga, or `bevy_render` — achieved **without a single `#[cfg(feature = "render")]` inside domain
code.** The gate must be *which plugins you add*, not conditional compilation sprinkled through the
simulation.

## The finding that makes this cheap

Bevy 0.19 already split its render stack. Measured with `cargo tree -p <crate> --depth 1`:

| render-FREE — safe below the line | forces `bevy_render` → wgpu/naga |
|---|---|
| `bevy_mesh` — `Mesh`, **`Mesh3d`**, `Indices` (depends only on `wgpu-types`) | **`bevy_pbr`** — `MeshMaterial3d<M>`, `StandardMaterial` |
| `bevy_camera` — **`Camera`, `Projection`, `Visibility`** | `bevy_core_pipeline` — `Camera3d`, `Bloom`, `Tonemapping` |
| `bevy_light` — `DirectionalLight`, `PointLight`, cascades | `bevy_shader` — pulls `naga` |
| `bevy_image`, `bevy_asset`, `bevy_scene`, `bevy_gltf`, `bevy_picking` | |

So a headless world can hold geometry, transforms, cameras, lights, visibility, glTF and picking.
**The one thing in the common spawn path that drags in the GPU is the material.**

> **Rule: a domain crate may name `Mesh3d`. It may not name `MeshMaterial3d`.**

That single rule is the whole architecture. It is checkable by `cargo tree`, so it cannot rot.

## The shape

Today, domain crates spawn geometry *and* bind its material in the same breath:

```rust
// crates/lunco-terrain-surface/src/stream_viz.rs — today
commands.spawn((
    Mesh3d(mesh),
    MeshMaterial3d(materials.add(ShaderMaterial { .. })),   // ← forces bevy_pbr on the whole graph
    Transform::from_translation(pos),
));
```

Instead, the domain crate states *intent* and stops:

```rust
// domain crate — render-free
commands.spawn((
    Mesh3d(mesh),                                    // bevy_mesh — free
    TerrainTileVisual { mode, band_bucket, maps },   // a plain Component: our data, no bevy_pbr
    Transform::from_translation(pos),
));
```

and a render crate binds it:

```rust
// lunco-render — the only place that names a material
fn bind_terrain_material(
    trigger: On<Add, TerrainTileVisual>,
    ...
) { commands.entity(e).insert(MeshMaterial3d(materials.add(..))); }
```

Headless simply never adds `LuncoRenderPlugin`. No `#[cfg]` anywhere in the domain crates. The
appearance components stay in the world either way — they are simulation-visible *intent*, which is
also what makes them serializable to USD and replicable over the wire.

This is the same move the codebase already made for panels (a trait-object registry, not an enum) and
for materials (WGSL-reflected, not a hardcoded table). It is not a new idea here — it is the existing
doctrine applied to the last place that dodged it.

## Where the work is

21 files insert `MeshMaterial3d`. That is the entire surface:

| crate | files | intent component to introduce |
|---|---|---|
| `lunco-terrain-surface` | `stream_viz.rs`, `terrain.rs`, `derived_layers.rs`, `terrain_layers/{mod,rocks}.rs` | `TerrainTileVisual`, `RockVisual` |
| `lunco-celestial` | `big_space_setup.rs`, `globe_lod.rs`, `systems.rs`, `missions.rs`, `trajectories.rs` | `BodyVisual`, `TrajectoryVisual` |
| `lunco-usd-bevy` | `lib.rs` (`instantiate_prim`), `camera*.rs` | `PrimVisual { color, shader }` |
| `lunco-sandbox-edit` | `spawn.rs`, `commands.rs`, `terrain_tools.rs`, `ui/*` | reuse `PrimVisual`; UI is already gated |
| `lunco-usd-sim` | `lib.rs`, `shader.rs` | reuse `PrimVisual` |
| `lunco-obstacle-field` | `plugin.rs`, `rock.rs` | reuse `RockVisual` |
| `lunco-environment` | `horizon.rs` | `SkyVisual` |
| `lunco-avatar` | `screenshot.rs`, `ui/mod.rs` | render-only already — move wholesale |
| `lunco-materials` | all | **render-only by nature.** Not split — just stops being a dependency of domain crates. |

`lunco-render` already exists and already declares itself "the future home for exposure/AA/sky look
settings". It becomes the home for all material binding, `Camera3d`/post-processing setup, and
screenshot capture.

`lunco-robotics` and `lunco-terrain-globe` were already render-free in fact but declared the full PBR
feature list; both are fixed (2026-07-12).

## Order of work

1. **`lunco-render` grows the binding layer** — `LuncoRenderPlugin`, and the `Appearance` intent
   components (they live here, or in `lunco-core` if a domain crate must construct them without
   depending on render; prefer `lunco-core`).
2. **One domain crate at a time**, easiest first, each landing green:
   `lunco-obstacle-field` → `lunco-environment` → `lunco-terrain-surface` → `lunco-usd-bevy` →
   `lunco-celestial` → `lunco-usd-sim` → `lunco-sandbox-edit`.
3. **Drop `bevy_pbr` from each crate's `bevy` feature list** as it goes clean. The compiler enforces
   the rest — if the crate still names a material, it will not build.
4. **CI guard, so it cannot regress:**
   ```bash
   cargo tree -p lunco-sandbox-server -i wgpu    # must FAIL: "package ID not found"
   cargo tree -p lunco-sandbox-server -i naga    # must FAIL
   ```
   Same guard that now protects `bevy_egui` (A1). This is the part that makes the architecture real
   rather than aspirational — the review's core lesson was *the craft is high, the enforcement is
   absent.*

## What this is not

It is **not** a "headless renderer" or a second render path. The render crate is the only consumer of
`bevy_pbr`, and the GUI build is byte-for-byte the same as today. The only observable difference in
the GUI is that material binding happens in an observer one frame-boundary later than the spawn — if
any code depends on the material existing in the same tick as the mesh, that is a latent ordering bug
worth surfacing anyway.
