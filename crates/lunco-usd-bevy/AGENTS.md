# lunco-usd-bevy — AI Agent Notes

This crate is the **first** plugin in the USD pipeline (Layer 2 / domain). It
turns USD prims into Bevy entities with meshes, materials, and transforms.
Everything physics- or sim-related lives in `lunco-usd-avian` and
`lunco-usd-sim` and runs `.after(sync_usd_visuals)`.

Read alongside `crates/lunco-usd-bevy/src/lib.rs` (the visual sync systems),
`src/read.rs` (the `UsdRead` read seam), `src/view.rs` / `src/canonical.rs` (the
live canonical stage), and `docs/architecture/21-domain-usd.md` (the
system-level overview).

## Reading USD attributes

All composed runtime reads go through the `UsdRead` trait (`src/read.rs`) and its
live canonical `StageView` implementation. Authoring-layer reads use
`UsdDataExt` separately; extractors must not switch between those authorities.

**Real-valued reads use the `real` family, never `scalar::<f64>`/`scalar::<f32>`
directly.** A bare typed scalar matches only one authored precision, so a value
authored `float` where you read `double` (or vice versa) reads as `None` and is
silently dropped — a wrong-magnitude bug. Use:

- `reader.real(prim, name) -> Option<f64>` / `reader.real_f32(prim, name) -> Option<f32>`
- `reader.real_at(prim, name, time)` / `reader.real_f32_at(prim, name, time)` for animated channels

## Adding a new prim type

`sync_usd_visuals` matches on `typeName` (`"Cube"`, `"Sphere"`, `"Cylinder"`).
Adding a new primitive shape:

1. Add a match arm that reads explicit dimensions via `reader.real(&sdf_path, "<attr>")`
   (or `real_f32` for a mesh-builder `f32`).
2. Build the mesh via Bevy's mesh builders (`Cuboid::new`, etc.). USD stores
   **full dimensions**, not half-extents — pass them straight to Bevy.
3. The existing material/transform/child code runs unchanged.

## glTF / external mesh assets

USD has standard `payload`/`references` syntax for pointing at external
layers. Pixar's distribution handles `.gltf`/`.glb` via the `UsdGltf`
SdfFileFormat plugin — `prepend payload = @./body.glb@` *just works*. Our
`openusd` fork (v0.5) has no plugin system, so we approximate:

1. **The compose path (folded into `lunco-usd-bevy`)** detects binary
   extensions on `payload`/`references`, skips the USD-text read, and composes
   the arc through an empty stub because the Rust USD fork has no file-format
   plugin API. The render projection reads the authored arc live from the
   composed prim stack and canonicalizes it relative to its authoring layer.
2. **`sync_usd_visuals`** reads that live binary payload/reference and dispatches:
   - `lunco:assetMode = "mesh"` → `asset_server.load::<Mesh>("<uri>#Mesh0/Primitive0")`,
     attached as `Mesh3d`. Compatible with `lunco-usd-avian` collider construction.
   - `lunco:assetMode = "scene"` (default) → `asset_server.load::<Scene>("<uri>#Scene0")`,
     attached as a child `SceneRoot`. Preserves multi-mesh hierarchy, materials, lights.
   - `lunco:assetLabel` overrides the `#…` suffix when the file isn't laid out
     as the default labels.

## Asset URI schemes

The composer passes through three URI shapes for the resolved attribute:

| Shape | Meaning | Example |
|---|---|---|
| `lunco://...` | LunCoSim asset **library** — a logical address. The reader resolves `assets/` first, then `<cache>/` (populated by `cargo run -p lunco-assets -- download`), so git-tracked content and downloaded binaries share one address space. | `lunco://models/perseverance.glb` |
| `/abs/path` (filesystem) | `/`-prefixed USD asset path resolved against the workspace `assets/` root. | `/ws/assets/models/x.glb` |
| `./relative.glb` | Relative to the layer's parent directory. | `./body.glb` |

The cache is a **resolution step, not an address** — no scheme points at it, or
an authored `.usda` would carry a machine-local storage location and resolve
only inside our pipeline. A future collaborative/Nucleus-like protocol should
take a distinct scheme (e.g. `lunco-net://`).

**Resolution itself belongs to `lunco-assets`.** This crate owns `canonicalize`
and `LuncoUsdResolver` — anchoring a *relative* reference to its referencing
layer, which is USD composition semantics. It must not re-derive where a scheme
points: ask `lunco_assets` (`has_scheme`, `local_path`, `shipped_asset_root`,
`id_to_disk_path`, `engine_asset_uri`/`_rel`).

## When the asset is missing

`AssetServer::load` returns a `Handle` immediately and surfaces load failure
through `AssetEvent::Failed`. Today we don't observe those — a missing
`lunco://...` results in the prim having no visible geometry but no
crash. If you change this to surface user-facing errors, do it via observers
on `AssetEvent`, not a system that polls every frame.

## Testing

Tests in `crates/lunco-usd-bevy/tests/` run headless (`MinimalPlugins`).
glTF loading needs `bevy::scene::ScenePlugin` + `bevy_gltf::GltfPlugin` —
add them explicitly when a test exercises the scene-mode path. The
default-plugins test path covers it implicitly.
