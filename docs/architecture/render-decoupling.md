# Render decoupling — the material is the boundary

**Status: DONE (2026-07-13).** The `--no-ui` server links no GPU stack:

```
$ cargo tree -p lunco-sandbox-server -i wgpu               # package ID not found
$ cargo tree -p lunco-sandbox-server -i bevy_render        # package ID not found
$ cargo tree -p lunco-sandbox-server -i bevy_pbr           # package ID not found
$ cargo tree -p lunco-sandbox-server -i bevy_core_pipeline # package ID not found
$ cargo tree -p lunco-sandbox-server -i egui               # package ID not found
$ cargo tree -p lunco-sandbox-server -i winit              # package ID not found
```

`naga` remains, via `bevy_shader` — the WGSL **compiler**, kept for live shader editing
(`SetShaderSource` / `CreateShader` compile WGSL into `Assets<Shader>` so an edit renders without a
disk round-trip). A compiler, not a GPU stack. Moving it behind the gate is a separate, smaller job.

The [`render-decoupling` CI job](../../.github/workflows/lint.yml) enforces all of the above. **Do not
delete it** — see [Why this needs a machine, not vigilance](#why-this-needs-a-machine-not-vigilance).

**Goal:** a headless build (`--no-ui`, the wasm worker, every integration test) that does not link
wgpu, naga, or `bevy_render` — achieved **without a single `#[cfg(feature = "render")]` inside domain
code.** The gate is *which plugins you add*, not conditional compilation sprinkled through the
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

## The three intents

`lunco-render` (render-free) and `lunco-materials` (render-free) hold the whole vocabulary.
`lunco-render-bevy` is the **only** crate in the graph that names `bevy_pbr`, and it binds them.

| intent | in | binds to | notes |
|---|---|---|---|
| **`PbrLook`** | `lunco-render` | `MeshMaterial3d<StandardMaterial>` | a plain surface as data: colour, roughness, metallic, emissive, IOR, clearcoat, specular tint, `SurfaceAlpha::{Opaque,Mask,Blend,Add}`, and all five `Handle<Image>` texture channels (`bevy_image` is render-free) |

`PbrLook` states the **physics**, not one backend's parameterisation of it. IOR is the
worked example: `PbrLook` carries `ior` (as does `UsdPreviewSurface`), while Bevy's
`reflectance` — Filament's remap of the same quantity, `F0 = 0.16·r²` against Fresnel's
`F0 = ((1−ior)/(1+ior))²` — is derived in `lunco-render-bevy::bevy_reflectance_from_ior`
and known nowhere else. A second backend would remap from the same `ior`. Carrying both
would let a look reflect like diamond and refract like glass, and would need to persist a
`reflectance` that USD has no attribute for.
| **`ShaderLook`** | `lunco-materials` | `MeshMaterial3d<ShaderMaterial>` | a custom `.wgsl` with an **open, user-defined parameter set** — see [shader-layers-and-params.md](shader-layers-and-params.md) |
| **`SceneCamera`** | `lunco-render` | `Camera3d` + tonemapping + MSAA + bloom | because `Camera3d` was being used as the *query filter* for "which camera is the scene one?", which made domain crates link a GPU stack **merely to ask a question** |
| **`WorldLabel`** | `lunco-render` | `Text2d` + font + colour | a spacecraft's *name* is simulation data; the glyphs are not |

### Two rules you must not break

1. **Identical looks share one material.** The binders cache by the look's *content*
   (`PbrLook::key()` / `ShaderLook::key()`, floats quantised to 1e-4 so a rounding error cannot mint a
   second material). 6000 rocks that look alike cost **one** material and **one** bind group. The old
   code preserved this by hand-threading a single `Handle` through the scatter loop — easy to forget,
   and it *was* forgotten in one of the two rock paths. Now it cannot be.
   **Corollary: never vary a look per-instance.** Bucket the value first (see the terrain LOD band and
   the rock radius buckets for the pattern), or you get a material and a draw call per instance.

2. **Anything ANIMATED must set `unshared: true`.** A look whose value changes every frame re-keys the
   content cache every frame — minting a material per frame and freeing none. That is an unbounded leak
   that presents as a slow memory climb, not a crash. `unshared` gives it a private material the binder
   mutates in place. (USD `displayColor` timeSamples hit this immediately.)

3. **An entity must never carry `PbrLook` and a shader material at once**, or the mesh **draws twice**.
   Taking the shader path must `remove::<PbrLook>()`, not merely replace the material.

## What has no intent form — and moved bodily instead

Not everything visual is *appearance*. Three things had no honest intent representation and were
**moved** into `lunco-render-bevy` rather than tortured into a component:

- **`horizon_shade`** — a per-frame heightfield/sun **uniform feed** into the terrain shader (from
  `lunco-environment`). Not a look; a data pump. The horizon **maths** stayed behind — it was already
  render-free, and `lunco-sandbox` imports exactly that half.
- **`env_light`** — the `bloom` arm of `SetEnvironmentLight`.
- **`terrain_maps`** — the derived-layer bind onto the terrain material that `lunco-usd-sim` authors
  asynchronously (no component to restate).

Screenshots do **not** live there either — but not for the reason this document used to give.
`CaptureScreenshot` needs `bevy_render`, and that dependency sat inside **`lunco-api`** behind a
`render` feature that was **on by default**. So render-free was the *non-default* path, every consumer
had to remember `default-features = false`, and **three forgot** (`lunco-doc-bevy`,
`lunco-celestial`/`lunco-tutorial`, `lunco-telemetry`) — each silently re-linking wgpu into the
`--no-ui` server. **An unsafe default is a trap that fires forever.**

The GPU half now lives in **`lunco-workbench::screenshot`**, and `lunco-api` has no `render` feature at
all: it *cannot* link a renderer. Not `lunco-render-bevy`, because **`lunica` takes screenshots and has
no 3D renderer** — it links `bevy_render` through egui but never adds `LuncoRenderPlugin`. The right
home was the smallest crate for which "this binary can render *something*" is already true, and both
GUI binaries already add it.

`lunco-api` keeps only the DISPATCH, because raw-PNG mode must defer the HTTP response until the capture
lands and only the executor knows the correlation id. That deferral is now **generic**
(`register_deferred_command::<T>()`), so the substrate no longer names a domain command — and a
`DeferredRequests` watchdog turns a handler that forgets to answer into an error instead of a hung
caller.

**Feature placement trap:** enable `lunco-workbench/api` from a crate's **`ui`** feature, never from its
`lunco-api` feature. The headless server enables `lunco-api` too, and hanging the workbench off it drags
egui + wgpu straight back in. (This regression happened, and the guard caught it.)

## Why this needs a machine, not vigilance

The failure mode is **invisible to code review**. Cargo unifies features across the whole graph, so a
single missing `default-features = false` anywhere silently re-links wgpu into every binary. It has
already happened **twice in this repo**:

1. `lunco-workspace` → `lunco-doc-bevy` (whose default `ui` feature pulls `bevy_egui`), putting egui +
   winit + wgpu into the headless server. CI had even **rationalised the symptom** — `integration.yml`
   carried a comment explaining why Linux windowing headers were needed *"even for the headless
   binary."* Nobody suspected the comma.
2. `lunco-celestial` → `lunco-api` (whose default `render` feature pulls `bevy_render`). This was found
   **at the very end of the decoupling**, after every material, camera and shader had already been
   moved — the same trap, one layer deeper, still invisible.

And the last edge before that one was a **single billboard `Text2d` label on a spacecraft** — because
`bevy_sprite_render` pulls `bevy_render`. Nobody would guess the server links a GPU driver because of a
text label. **Only `cargo tree` sees any of this.**

Hence the `render-decoupling` job in [`.github/workflows/lint.yml`](../../.github/workflows/lint.yml):
it asserts the server links none of `wgpu`/`bevy_render`/`bevy_pbr`/`bevy_core_pipeline`/`egui`/`winit`,
and that no crate other than `lunco-render-bevy` enables `bevy_pbr`. The review's central lesson was
*the craft is high, the enforcement is absent.* This is the enforcement.

## What this is not

It is **not** a "headless renderer" or a second render path. `lunco-render-bevy` is the only consumer of
`bevy_pbr`, and the GUI build renders exactly as before. The one observable difference is that material
binding happens in an observer, a frame-boundary after the spawn — if any code depended on the material
existing in the same tick as the mesh, that was a latent ordering bug worth surfacing anyway.

The GUI is **not** feature-gated into the domain: there is exactly **one** `#[cfg(feature = "ui")]` in
the whole scheme, in `SandboxCorePlugin::build`, and it exists only because `lunco-render-bevy` is an
optional dependency. The simulation crates contain none.
