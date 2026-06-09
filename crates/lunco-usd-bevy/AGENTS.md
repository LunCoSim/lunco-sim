# lunco-usd-bevy — AI Agent Notes

This crate is the **first** plugin in the USD pipeline (Layer 2 / domain). It
turns USD prims into Bevy entities with meshes, materials, and transforms.
Everything physics- or sim-related lives in `lunco-usd-avian` and
`lunco-usd-sim` and runs `.after(sync_usd_visuals)`.

Read alongside `crates/lunco-usd-bevy/src/lib.rs` (the only source file) and
`docs/usd-system.md` (the system-level overview).

## Adding a new prim type

`sync_usd_visuals` matches on `typeName` (`"Cube"`, `"Sphere"`, `"Cylinder"`).
Adding a new primitive shape:

1. Add a match arm that reads explicit dimensions via
   `reader.prim_attribute_value::<f64>(&sdf_path, "<attr>")`.
2. Build the mesh via Bevy's mesh builders (`Cuboid::new`, etc.). USD stores
   **full dimensions**, not half-extents — pass them straight to Bevy.
3. The existing material/transform/child code runs unchanged.

## glTF / external mesh assets

USD has standard `payload`/`references` syntax for pointing at external
layers. Pixar's distribution handles `.gltf`/`.glb` via the `UsdGltf`
SdfFileFormat plugin — `prepend payload = @./body.glb@` *just works*. Our
`openusd-rs 0.2` fork has no plugin system, so we approximate:

1. **`lunco-usd-composer`** detects non-USD extensions (`glb`, `gltf`, `obj`,
   `stl`) on `payload`/`references`, skips the USD-text read, and synthesises
   a `lunco:resolvedAsset` attribute on the referencing prim with the
   resolved URI.
2. **`sync_usd_visuals`** reads `lunco:resolvedAsset` and dispatches:
   - `lunco:assetMode = "mesh"` → `asset_server.load::<Mesh>("<uri>#Mesh0/Primitive0")`,
     attached as `Mesh3d`. Compatible with `lunco-usd-avian` collider construction.
   - `lunco:assetMode = "scene"` (default) → `asset_server.load::<Scene>("<uri>#Scene0")`,
     attached as a child `SceneRoot`. Preserves multi-mesh hierarchy, materials, lights.
   - `lunco:assetLabel` overrides the `#…` suffix when the file isn't laid out
     as the default labels.

The synthesised `lunco:resolvedAsset` is an internal contract between the
composer and this plugin — **don't author it by hand**. Author the standard
USD `payload` instead, and the composer fills it in. A hand-written value is
respected (composer doesn't overwrite), but it's a sharp tool.

## Asset URI schemes

The composer passes through three URI shapes for the resolved attribute:

| Shape | Meaning | Example |
|---|---|---|
| `lunco-lib://...` | LunCoSim shipped library — registered in `lunco-client/src/main.rs`, points at `<cache>/`. Populated by `cargo run -p lunco-assets -- download`. | `lunco-lib://models/perseverance.glb` |
| `/abs/path` (filesystem) | `/`-prefixed USD asset path resolved against the workspace `assets/` root. | `/ws/assets/models/x.glb` |
| `./relative.glb` | Relative to the layer's parent directory. | `./body.glb` |

**Reserved**: `lunco://` is held for the future LunCoSim asset/scene service
(multi-user, collaborative, network-backed — analogous to Omniverse's
Nucleus). It is **not** a synonym for `lunco-lib://`. Do not register it as
an alias today; that would constrain the future protocol's URI grammar.

## When the asset is missing

`AssetServer::load` returns a `Handle` immediately and surfaces load failure
through `AssetEvent::Failed`. Today we don't observe those — a missing
`lunco-lib://...` results in the prim having no visible geometry but no
crash. If you change this to surface user-facing errors, do it via observers
on `AssetEvent`, not a system that polls every frame.

## Testing

Tests in `crates/lunco-usd-bevy/tests/` run headless (`MinimalPlugins`).
glTF loading needs `bevy::scene::ScenePlugin` + `bevy_gltf::GltfPlugin` —
add them explicitly when a test exercises the scene-mode path. The
default-plugins test path covers it implicitly.
