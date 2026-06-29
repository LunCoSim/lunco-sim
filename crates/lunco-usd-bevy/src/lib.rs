//! # LunCoSim USD → Bevy Visual Sync
//!
//! Responsible for spawning child entities for USD prims and attaching visual components
//! (meshes, materials, transforms). This is the **first** plugin in the USD processing
//! pipeline — it must run before the Avian physics and Sim simulation plugins.
//!
//! ## How It Works
//!
//! 1. The asset loader (`UsdLoader`) reads a `.usda` file, parses it, and resolves all
//!    external references (e.g., wheel component files) via `UsdComposer::flatten()`.
//! 2. The `sync_usd_visuals` system iterates over all entities with `UsdPrimPath` that
//!    haven't been processed yet (`Without<UsdVisualSynced>`).
//! 3. For each prim, it creates a mesh based on the prim type (`Cube`, `Cylinder`, `Sphere`)
//!    using explicit dimensions from the USD file.
//! 4. It spawns child entities for each prim child, pre-populating their transforms so
//!    physics systems see them in the correct positions.
//!
//! ## Coordinate Systems
//!
//! USD uses Y-up, +Z-forward. Bevy uses Y-up, -Z-forward. The USD files store rotation
//! in degrees via `xformOp:rotateXYZ`. This system converts them to radians and applies
//! them as Bevy quaternions.
//!
//! ## Mesh Dimensions
//!
//! Bevy's `Cuboid::new()` and `Collider::cuboid()` take **full dimensions**, not
//! half-extents. The USD files store full dimensions (`width`, `height`, `depth`),
//! so no scaling is needed.
//!
//! ## Why Not Use the Observer?
//!
//! The `On<Add, UsdPrimPath>` observer fires when the entity is spawned, but the USD
//! asset may not be loaded yet (async loading). The `sync_usd_visuals` system runs in
//! the `Update` schedule and retries every frame until the asset is available, then
//! marks the entity with `UsdVisualSynced` to prevent re-processing.

use bevy::prelude::*;
use bevy::asset::{AssetLoader, LoadContext, io::Reader};
pub use openusd::sdf::Path as SdfPath;
// openusd `main` removed `TextReader`. The composed stage is flattened to a
// Send-safe `sdf::Data` (see `compose`), queried via the `usd_data` helpers.
pub use openusd::sdf::Data as UsdData;
use openusd::sdf::Value;
use big_space::prelude::CellCoord;
use std::sync::Arc;

mod resolver;
mod compose;
mod light;
pub mod author;
pub mod usd_data;
use usd_data::UsdDataExt;
pub use compose::compose_native_fs;
#[cfg(not(target_arch = "wasm32"))]
pub use compose::compose_file;
pub use light::{FallbackSceneLight, UsdAuthoredLight};

/// Bevy plugin for USD visual synchronization.
///
/// Registers the `UsdStageAsset` type, the USD asset loader, and the `sync_usd_visuals`
/// system that processes USD prims into Bevy entities with meshes and transforms.
pub struct UsdBevyPlugin;

impl Plugin for UsdBevyPlugin {
    fn build(&self, app: &mut App) {
        // Core glTF/USD scene component types. The workspace runs bevy with
        // `default-features = false`, so bevy's `reflect_auto_register` is OFF
        // and these are NOT auto-registered. Any glTF `SceneRoot` we spawn (USD
        // payload overlay, terrain, rovers) is deserialized via
        // `Scene::write_to_world_with`, which panics on the first unregistered
        // component type. Register the bounded set a glTF scene can contain so
        // the registry is complete WITHOUT pulling the inventory-based
        // auto-register closure into the link (it overflowed clang's command
        // line — see the bevy dep note in `lunco-sandbox/Cargo.toml`).
        app.register_type::<Transform>()
            .register_type::<GlobalTransform>()
            .register_type::<Visibility>()
            .register_type::<InheritedVisibility>()
            .register_type::<ViewVisibility>()
            .register_type::<Name>()
            .register_type::<ChildOf>()
            .register_type::<Children>()
            .register_type::<bevy::camera::primitives::Aabb>()
            .register_type::<Mesh3d>()
            .register_type::<MeshMaterial3d<StandardMaterial>>()
            // Skinned/morph meshes — glTF rover payloads are skinned.
            .register_type::<bevy::mesh::skinning::SkinnedMesh>()
            .register_type::<bevy::mesh::morph::MorphWeights>()
            .register_type::<bevy::mesh::morph::MeshMorphWeights>()
            // Lights the glTF loader may embed (USD-authored lights take a
            // separate path, but a glTF can carry its own).
            .register_type::<DirectionalLight>()
            .register_type::<PointLight>()
            .register_type::<SpotLight>()
            .register_type::<bevy::gltf::GltfExtras>()
            .register_type::<bevy::gltf::GltfSceneExtras>()
            .register_type::<bevy::gltf::GltfMeshExtras>()
            .register_type::<bevy::gltf::GltfMeshName>()
            .register_type::<bevy::gltf::GltfMaterialExtras>()
            .register_type::<bevy::gltf::GltfMaterialName>();
        app.init_asset::<UsdStageAsset>()
            .register_asset_loader(UsdLoader)
            // E1b: raw-source asset so a scene document's base layer can be read
            // through the same (web-ready) asset source the live world uses.
            .init_asset::<UsdSourceText>()
            .register_asset_loader(UsdSourceTextLoader)
            .register_type::<UsdPrimPath>()
            .init_resource::<DiagnosticLabelFont>()
            .init_resource::<DiagnosticLabelConfig>()
            .add_systems(Startup, load_diagnostic_label_font)
            .add_observer(on_usd_prim_added)
            .add_observer(light::on_usd_light_added)
            // `sync_usd_visuals` runs only on frames where a stage's
            // `LoadedWithDependencies` event was emitted. Idle frames
            // skip it entirely (run-condition short-circuits).
            .add_systems(
                Update,
                (
                    sync_usd_visuals.run_if(bevy::ecs::schedule::common_conditions::on_message::<AssetEvent<UsdStageAsset>>),
                    // Upgrades parked runtime-instance descendants to a
                    // hierarchical `Derived` id (gap G2/B.1) once their root id
                    // is allocated. Cheap: the query is empty unless a runtime
                    // spawn is mid-flight.
                    resolve_usd_instance_identities,
                    hide_glb_placeholder_meshes,
                    poll_diagnostic_label_font,
                    reveal_placeholder_on_failure,
                    bake_pending_labels,
                ),
            );
    }
}

/// A Bevy Asset representing a loaded USD Stage.
///
/// Contains a flattened USD reader with all external references resolved.
/// Created by the `UsdLoader` asset loader when a `.usda` file is loaded.
#[derive(Asset, TypePath, Clone)]
pub struct UsdStageAsset {
    /// Flattened, composed scene data (all references resolved). Send-safe
    /// `sdf::Data`; query it with the [`usd_data`] helpers.
    pub reader: Arc<UsdData>,
}

#[derive(Default, TypePath)]
pub struct UsdLoader;

impl AssetLoader for UsdLoader {
    type Asset = UsdStageAsset;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        // Read raw bytes from the .usda file.
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        // Source-qualified path of this layer — the composition root and the
        // pre-fetch BFS anchor. `LoadContext::path()` drops the asset *source*
        // (Bevy tracks it separately), so a layer loaded from a NAMED source
        // (e.g. an external Twin scene under `abs://`) would lose its scheme and
        // its relative refs (the co-located terrain glb) would wrongly resolve
        // against the default `assets/` source. Re-attach `scheme://` so every
        // relative arc stays under the layer's own source.
        let lc_path = load_context.path();
        let root_asset_path = match lc_path.source() {
            bevy::asset::io::AssetSourceId::Name(name) => {
                format!("{}://{}", name, lc_path.path().to_string_lossy())
            }
            bevy::asset::io::AssetSourceId::Default => {
                lc_path.path().to_string_lossy().into_owned()
            }
        };

        // Compose with openusd's PCP engine — references, payloads, variant
        // selection, and relationship-target translation — through our in-memory
        // `LuncoUsdResolver` (filesystem-free, native + wasm), then flatten the
        // composed stage into a Send-safe `sdf::Data` for the downstream visual /
        // physics / cosim readers (`Stage` is `!Send`, so it can't cross here).
        let data = compose::compose_to_data(load_context, &root_asset_path, bytes).await?;

        Ok(UsdStageAsset {
            reader: Arc::new(data),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["usda"]
    }
}

/// A USD layer's **raw source text**, read through the `AssetServer` without
/// composition.
///
/// Distinct from [`UsdStageAsset`], which is the *composed + flattened* stage:
/// this is just the bytes of one `.usda` layer, decoded to a `String`. E1b uses
/// it to open a scene document's base layer **through the same asset source the
/// live world loads from** (e.g. `twin://`) — so the read is web-ready (it rides
/// whatever the source supports) instead of going through native `std::fs`.
#[derive(Asset, TypePath, Clone)]
pub struct UsdSourceText(pub String);

/// Loader producing [`UsdSourceText`] — reads bytes, decodes UTF-8, no
/// composition. Shares the `.usda` extension with [`UsdLoader`]; the requested
/// asset type (`load::<UsdSourceText>` vs `load::<UsdStageAsset>`) selects the
/// loader.
#[derive(Default, TypePath)]
pub struct UsdSourceTextLoader;

impl AssetLoader for UsdSourceTextLoader {
    type Asset = UsdSourceText;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(UsdSourceText(String::from_utf8(bytes)?))
    }

    fn extensions(&self) -> &[&str] {
        &["usda"]
    }
}

/// Marks an entity as representing a USD prim path.
///
/// This component is added to every entity that corresponds to a USD prim. The system
/// uses it to look up the prim's attributes from the loaded USD stage.
///
/// # Fields
/// - `stage_handle`: Handle to the loaded `UsdStageAsset`
/// - `path`: USD prim path (e.g., `/SandboxRover` or `/SandboxRover/Wheel_FL`)
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct UsdPrimPath {
    /// Handle to the loaded USD stage asset.
    pub stage_handle: Handle<UsdStageAsset>,
    /// USD prim path within the stage (e.g., `/SandboxRover/Wheel_FL`).
    pub path: String,
}

impl Default for UsdPrimPath {
    fn default() -> Self {
        Self {
            stage_handle: Handle::default(),
            path: "/".to_string(),
        }
    }
}

/// Marker component indicating that an entity has been processed by `sync_usd_visuals`.
///
/// Prevents the system from re-processing the same entity on subsequent frames.
#[derive(Component)]
pub struct UsdVisualSynced;

/// Marker placed on a USD scene root that exists purely to render a
/// preview thumbnail. Plugins that activate simulation side-effects on
/// USD prims (avatar cameras, vehicle FSW, wheel physics) should walk
/// each candidate prim's `ChildOf` ancestry and bail if any ancestor
/// carries this marker — preview-only stages must show geometry but
/// must not spawn cameras into the window or insert physics bodies
/// into the live world.
#[derive(Component, Default, Debug, Clone, Copy)]
pub struct UsdPreviewOnly;

/// Attached to a scene-root entity to tell the USD instantiator where to
/// place top-level USD prims. When this component is present, each
/// direct USD child spawns as a `GridAnchor` parented to the target
/// Grid — *not* as a Bevy child of this entity.
///
/// This is what enforces the architectural rule: top-level USD prims
/// (rovers, balls, terrain) become Grid-direct entities so big_space's
/// `propagate_high_precision` runs on them; their own descendants
/// remain plain-`Transform` children of their USD parent's Bevy entity.
#[derive(Component, Debug, Clone, Copy)]
pub struct LoadIntoGrid(pub Entity);

/// Marker placed on an entity whose `UsdPrimPath` was added before the
/// referenced `UsdStageAsset` finished loading. `on_stage_loaded`
/// processes it once the asset becomes available.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdAwaitingStage;

/// Seed marker for hierarchical instance identity (gap G2/B.1). Placed
/// **atomically** (in the same spawn bundle as `UsdPrimPath`) on the root of a
/// runtime-spawned USD instance — a palette/API spawn, never authored scene
/// content. The loader reads it to start propagating [`UsdInstanceMember`] down
/// the subtree.
///
/// Why a dedicated marker rather than reusing `SkipContentStamp`: that stamp is
/// inserted in a *separate* command after the root spawn, so the
/// `Add<UsdPrimPath>` observer can fire before it lands. The loader needs the
/// signal to be present the instant the root is instantiated, which only an
/// atomic bundle component guarantees.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdInstanceRoot;

/// Propagated down a runtime-spawned USD instance subtree so each descendant
/// derives its identity from the instance root rather than taking a `Content`
/// id (gap G2/B.1: two spawns of the same asset compose identical prim paths,
/// so their descendants' content ids would collide).
///
/// `root` is the instance-root entity — it owns a unique, replicated
/// `GlobalEntityId`. `root_path` is the root's composed prim path; a member's
/// *role* is its own prim path relative to it. The loader parks each descendant
/// as [`lunco_core::Provenance::Local`] and `resolve_usd_instance_identities`
/// upgrades it to a deterministic `Derived` provenance once the root id exists.
#[derive(Component, Debug, Clone)]
pub struct UsdInstanceMember {
    /// The instance-root entity this member descends from.
    pub root: Entity,
    /// The instance root's composed prim path (the prefix to strip for `role`).
    pub root_path: String,
}

/// A USD instance member's *role*: its prim path relative to the instance root.
/// `/SolarPanel` + `/SolarPanel/Frame/Bolt` → `Frame/Bolt`. Falls back to the
/// full (leading-slash-trimmed) path if the prefix doesn't match.
fn instance_role(root_path: &str, prim_path: &str) -> String {
    prim_path
        .strip_prefix(root_path)
        .map(|s| s.trim_start_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| prim_path.trim_start_matches('/').to_string())
}

/// Translates a single USD prim into Bevy/big_space/avian components on
/// `entity`. The caller has already verified that the stage is loaded.
///
/// **Steady-state cost: zero** — this is invoked exactly once per entity,
/// either by `on_usd_prim_added` (entity spawned after stage loaded) or
/// by `on_stage_loaded` (entity spawned before stage loaded; drained
/// from the `UsdAwaitingStage` queue when the asset becomes ready). No
/// per-frame polling.
///
/// 1. Looks up the prim's attributes from the loaded USD stage.
/// 2. Creates a mesh based on prim type (Cube, Cylinder, Sphere).
/// 3. Applies the prim's transform (position + rotation + scale).
/// 4. Spawns child entities for each prim child, applying the natural
///    anchor rule via `LoadIntoGrid` (top-level → `GridAnchor`).
/// 5. Marks the entity with `UsdVisualSynced` to prevent re-processing.
///
/// Custom materials (solar panels, blueprint grids, etc.) are applied
/// by independent material plugins in `lunco-materials` that observe
/// the `UsdVisualSynced` insertion.
#[allow(clippy::too_many_arguments)]
fn instantiate_usd_prim(
    entity: Entity,
    prim_path: &UsdPrimPath,
    existing_vis: Option<&Visibility>,
    existing_tf: Option<&Transform>,
    load_into_grid: Option<&LoadIntoGrid>,
    is_instance_root: bool,
    inherited_member: Option<&UsdInstanceMember>,
    commands: &mut Commands,
    stages: &Assets<UsdStageAsset>,
    asset_server: &AssetServer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { return; };

        // Borrow — `stage.reader` is `Arc<sdf::Data>`; deep-cloning it copies
        // the whole stage `HashMap`. Every read below is `&self`.
        let reader = &*stage.reader;

        // Deferred `defaultPrim` resolution. A scene-root spawned with an
        // empty path is the "use the stage's defaultPrim" sentinel
        // (`resolve_root_prim` no longer reads the file with `std::fs` —
        // that always returned `None` on wasm, so every web scene load
        // mounted the whole stage at `/` instead of the defaultPrim
        // subtree). The stage is parsed via the `AssetServer` (works on
        // web), so we read it here, where the reader is guaranteed loaded,
        // and write the concrete path back so downstream consumers
        // (cosim prim scan, dedup) see a real prim instead of the sentinel.
        let resolved_path = if prim_path.path.is_empty() {
            let p = match stage_default_prim(reader) {
                Some(name) => format!("/{name}"),
                None => {
                    warn!(
                        "[usd] stage has no `defaultPrim` — mounting whole stage at `/`. \
                         Add `( defaultPrim = \"Name\" )` to the stage header if this \
                         file will be referenced from other USD files."
                    );
                    "/".to_string()
                }
            };
            commands.entity(entity).insert(UsdPrimPath {
                stage_handle: prim_path.stage_handle.clone(),
                path: p.clone(),
            });
            p
        } else {
            prim_path.path.clone()
        };
        let Ok(sdf_path) = SdfPath::new(&resolved_path) else { return; };

        // M1 identity (Ph1). Two regimes:
        //
        //  * **Descendant of a runtime-spawned instance** (`inherited_member`):
        //    a palette/API spawn of the same asset composes identical prim
        //    paths, so a `Content` id would collide across instances (gap
        //    G2/B.1). Park it as `Provenance::Local` now and let
        //    `resolve_usd_instance_identities` upgrade it to a deterministic
        //    `Derived` id (parent = the instance root's unique, replicated id;
        //    role = path relative to the root) once that root id is allocated.
        //    The root id isn't minted yet during this synchronous instantiation,
        //    so the upgrade must be deferred.
        //
        //  * **Authored scene prim** (and the instance root itself): stamp the
        //    deterministic `Provenance::Content`. The `source` is the stage's
        //    **stable logical asset path** (NOT the content-hash `AssetId` —
        //    D3b in DECISIONS.md), so the same prim derives the same
        //    `GlobalEntityId` on every peer. The instance root *also* takes a
        //    `Content` stamp here, but `assign_global_entity_ids` ignores it
        //    (the root carries `SkipContentStamp` → authoritative id). Stages
        //    not loaded from a path (`get_path` → None) get no stamp, so the
        //    core assignment fallback allocates instead.
        if inherited_member.is_some() {
            commands
                .entity(entity)
                .insert(lunco_core::Provenance::Local);
        } else if let Some(source) = asset_server.get_path(prim_path.stage_handle.id()) {
            commands.entity(entity).insert(lunco_core::Provenance::Content {
                namespace: "usd".into(),
                source: source.path().to_string_lossy().into_owned(),
                path: resolved_path.clone(),
            });
        }

        // Membership to hand down to children: inherited if we're mid-subtree,
        // or freshly rooted at *this* entity if it is the instance root. `None`
        // for ordinary scene prims (their descendants keep `Content` identity).
        let child_member: Option<UsdInstanceMember> = inherited_member.cloned().or_else(|| {
            is_instance_root.then(|| UsdInstanceMember {
                root: entity,
                root_path: resolved_path.clone(),
            })
        });

        // Skip inactive prims
        if !reader.prim_is_active(&sdf_path) {
            commands.entity(entity).insert(UsdVisualSynced);
            return;
        }

        // Get prim type (Cube, Cylinder, Sphere, etc.)
        let prim_type = reader.prim_type_name(&sdf_path);

        // UsdLux light prims (`DistantLight` sun / `DomeLight` ambient —
        // see `light.rs`). A light produces no mesh; the shared transform
        // path below still applies, which is how a DistantLight gets its
        // orientation from `xformOp:rotateXYZ`.
        light::instantiate_light_prim(reader, &sdf_path, prim_type.as_deref(), commands, entity);

        // Horizon-map terrain self-shadowing (consumed by
        // `lunco-environment`'s horizon system). Authors opt a terrain prim
        // in with `custom bool lunco:terrain:horizonShadows = true`; the
        // bake grid is tunable via `int lunco:terrain:horizonMapResolution`
        // and `int lunco:terrain:horizonMapAzimuths`.
        if light::get_attribute_as_bool(reader, &sdf_path, "lunco:terrain:horizonShadows")
            .unwrap_or(false)
        {
            let mut cfg = lunco_core::HorizonShadowTerrain::default();
            if let Some(r) =
                get_attribute_as_f32(reader, &sdf_path, "lunco:terrain:horizonMapResolution")
            {
                cfg.resolution = (r as u32).clamp(64, 4096);
            }
            if let Some(a) =
                get_attribute_as_f32(reader, &sdf_path, "lunco:terrain:horizonMapAzimuths")
            {
                cfg.azimuths = (a as u32).clamp(4, 64);
            }
            commands.entity(entity).insert(cfg);
        }

        // Visibility — honour standard USD `token visibility`.
        // `invisible` suppresses mesh creation entirely (used for
        // collider-only Cube prims hidden behind a glTF visual, and
        // raycast wheel cylinders that have no visible representation).
        let invisible = matches!(
            get_attribute_as_string(reader, &sdf_path, "visibility").as_deref(),
            Some("invisible")
        );

        // Placeholder for an async-loading glTF payload. Authors set
        // `bool lunco:placeholder = true` on a Cube prim that lives as
        // a sibling of an `Xform "Visual" (payload = @lunco-lib://...@)`.
        // Third-party USD tools render it (they don't know our
        // attribute or the `lunco-lib://` scheme); our pipeline starts
        // it `Visibility::Hidden` so the user doesn't see a brief
        // tan-cube flash before the photoreal glTF replaces it. Mesh
        // is still built — visibility is the toggle. (Future: reveal
        // on `AssetServer::load_state(...).is_failed()`.)
        let is_placeholder = reader
            .prim_attribute_value::<bool>(&sdf_path, "lunco:placeholder")
            .unwrap_or(false);

        // **Placeholder + payload pattern**: when `lunco:resolvedAsset`
        // is present, we still build the primitive Cube/Sphere/Cylinder
        // mesh so the prim has a fallback visual until the glTF Scene
        // finishes loading. Once Bevy reports the Scene asset loaded,
        // `hide_glb_placeholder_meshes` (below) hides the primitive
        // Mesh3d so the photoreal glTF replaces it cleanly.
        //
        // Authors size the placeholder Cube ≈ glTF bbox; mismatched
        // scales briefly show a tan border around the rover during
        // loading and as fallback when the asset is missing.

        // Create mesh based on prim type and **spec-compliant** USD
        // attributes:
        //   * `Cube`     : `double size` (default 2.0) — UsdGeomCube
        //   * `Sphere`   : `double radius` (default 1.0) — UsdGeomSphere
        //   * `Cylinder` : `double radius`, `double height` — UsdGeomCylinder
        // Authors compose non-uniform dimensions via `xformOp:scale`
        // — exactly how Pixar USD / Houdini / Blender expect it.
        //
        // **Legacy fallback**: `width`/`height`/`depth` on Cube prims is
        // still accepted so older `.usda` files keep working during the
        // migration. New authoring should use `size` + `xformOp:scale`.
        // Shape dimensions (+ their magic defaults) come from the
        // canonical `read_shape_dims` so the visual mesh and the avian
        // collider can't desync. The mesh-quality params (sphere UV
        // tessellation, cylinder/cone radial resolution, capsule
        // lat/long) stay here — they're rendering-only and don't affect
        // physics.
        let mesh_handle: Option<Handle<Mesh>> = if invisible {
            None
        } else {
            match prim_type.as_deref().and_then(|ty| read_shape_dims(reader, &sdf_path, ty)) {
                Some(ShapeDims::Cube { size, legacy_extents }) => match legacy_extents {
                    // Legacy form — width/height/depth are *full* extents
                    // and bake into the mesh directly.
                    Some([w, h, d]) => Some(meshes.add(Cuboid::new(w as f32, h as f32, d as f32))),
                    // Spec form: unit-ish Cube; xformOp:scale handles
                    // non-uniform dimensions (set on Transform below).
                    None => Some(meshes.add(Cuboid::new(size as f32, size as f32, size as f32))),
                },
                Some(ShapeDims::Sphere { radius }) => {
                    // Lat-long (UV) sphere, NOT an icosphere: a UV sphere has a
                    // clean rectangular UV unwrap (uv.x = longitude, uv.y =
                    // pole-to-pole latitude), which our ShaderMaterial checker
                    // (e.g. shaders/balloon.wgsl) needs to tile across the whole
                    // surface. An icosphere's UVs are distorted/seamed and leave
                    // large uncovered-looking patches.
                    Some(meshes.add(Sphere::new(radius as f32).mesh().uv(48, 32)))
                }
                Some(ShapeDims::Cylinder { radius, height }) => {
                    // Bump radial resolution well above the default so the tire
                    // silhouette reads as round, not faceted — the low-poly
                    // barrel made the top edge of the wheel look chunky.
                    Some(meshes.add(Cylinder::new(radius as f32, height as f32).mesh().resolution(64)))
                }
                Some(ShapeDims::Cone { radius, height }) => {
                    Some(meshes.add(Cone::new(radius as f32, height as f32).mesh().resolution(64)))
                }
                Some(ShapeDims::Capsule { radius, height }) => {
                    let half_length = (height / 2.0) as f32;
                    Some(meshes.add(
                        Capsule3d::new(radius as f32, half_length)
                            .mesh()
                            .latitudes(16)
                            .longitudes(32),
                    ))
                }
                Some(ShapeDims::Plane { width, length }) => {
                    Some(meshes.add(Plane3d::default().mesh().size(width as f32, length as f32)))
                }
                None => None,
            }
        };

        // Apply standard PBR material with USD color
        if let Some(ref m) = mesh_handle {
            apply_standard_material(
                reader,
                &sdf_path,
                m,
                materials,
                &mut commands.entity(entity),
                asset_server,
                prim_path.stage_handle.id(),
            );
        }

        // Embedded per-entity scenario: a `custom string lunco:script` on the
        // prim carries a rhai scenario. Stamp the lunco-core marker so
        // `lunco-scripting` attaches + runs it (the two crates stay decoupled —
        // neither depends on the other). Scenarios thus travel with the scene.
        if let Some(src) = get_attribute_as_string(reader, &sdf_path, "lunco:script") {
            if !src.trim().is_empty() {
                commands
                    .entity(entity)
                    .insert(lunco_core::EmbeddedScenarioSource(src));
            }
        }

        // glTF / external-mesh branch.
        //
        // The composer writes `lunco:resolvedAsset` onto any prim whose
        // `payload`/`references` point at a non-USD binary (`.glb`,
        // `.gltf`, `.obj`, `.stl`). We hand the URI to Bevy's
        // `AssetServer` directly — the registered asset sources
        // (`lunco-lib://` for shipped fixtures, default `assets://` for
        // in-tree paths) handle the lookup.
        //
        // - `lunco:assetMode = "mesh"` (default `"scene"`): pull a
        //   single primitive out of the glTF and attach as `Mesh3d`.
        //   Used when the prim should also drive a physics collider —
        //   stays compatible with `lunco-usd-avian` mesh-collider
        //   pipelines.
        // - `lunco:assetMode = "scene"`: load the full glTF scene and
        //   attach as a `SceneRoot` child. Preserves hierarchy,
        //   materials, and lights at the cost of being opaque to the
        //   USD prim-path tree.
        if let Some(asset_uri) = get_attribute_as_string(reader, &sdf_path, "lunco:resolvedAsset") {
            let mode = get_attribute_as_string(reader, &sdf_path, "lunco:assetMode")
                .unwrap_or_else(|| "scene".to_string());
            let label = get_attribute_as_string(reader, &sdf_path, "lunco:assetLabel");

            match mode.as_str() {
                "mesh" => {
                    let label = label.unwrap_or_else(|| "Mesh0/Primitive0".to_string());
                    let path = format!("{asset_uri}#{label}");
                    let mesh_h: Handle<Mesh> = asset_server.load(&path);
                    // Single-mesh path keeps `lunco-usd-avian` collider
                    // construction unchanged — the entity ends up with
                    // a `Mesh3d` exactly like the Cube/Sphere branches.
                    apply_standard_material(
                        reader,
                        &sdf_path,
                        &mesh_h,
                        materials,
                        &mut commands.entity(entity),
                        asset_server,
                        prim_path.stage_handle.id(),
                    );
                }
                _ => {
                    let label = label.unwrap_or_else(|| "Scene0".to_string());
                    let path = format!("{asset_uri}#{label}");
                    let scene_h: Handle<Scene> = asset_server.load(&path);
                    // Mark the entity so `hide_glb_placeholder_meshes`
                    // can drop the placeholder Mesh3d once this Scene
                    // finishes loading. The marker is harmless if the
                    // entity has no Mesh3d (e.g. `def Xform` without a
                    // primitive fallback).
                    commands.entity(entity)
                        .insert(SceneRoot(scene_h))
                        .insert(GlbPlaceholder)
                        .insert(PlaceholderAssetUri(path.clone()));
                }
            }
        }

        // Transform (position and rotation)
        // Preserve any existing transform set by the spawning code (e.g., rover position).
        // Only override position/rotation if the USD prim has explicit NON-ZERO values.
        // A zero translation in USD means "no offset" — it shouldn't overwrite a spawn position.
        let mut transform = existing_tf.cloned().unwrap_or_default();
        if let Some(v) = get_attribute_as_vec3(reader, &sdf_path, "xformOp:translate") {
            // Only apply USD translation if it's non-zero (to avoid overwriting spawn positions)
            if v.length_squared() > 1e-6 {
                transform.translation = v;
            }
        }
        if let Some(v) = get_attribute_as_vec3(reader, &sdf_path, "xformOp:rotateXYZ") {
            // USD stores rotation in degrees; the canonical decoder
            // converts to radians. Only apply USD rotation if it's
            // non-zero (to preserve existing spawn rotation).
            let is_zero = v.x.abs() < 1e-6 && v.y.abs() < 1e-6 && v.z.abs() < 1e-6;
            if !is_zero {
                transform.rotation = euler_xyz_deg_to_quat(v);
            }
        }
        // UsdGeomCylinder.axis token (X|Y|Z, default Z). Compose the
        // axis-induced rotation onto the entity Transform so a Y-axis
        // Bevy `Cylinder` mesh appears along the authored axis without
        // an explicit `xformOp:rotateXYZ` hack. Goes after rotateXYZ so
        // it applies on top of any user-authored rotation.
        if matches!(prim_type.as_deref(), Some("Cylinder" | "Cone" | "Capsule" | "Plane")) {
            let axis = read_token(reader, &sdf_path, "axis").unwrap_or_else(|| "Z".to_string());
            if let Some(q) = usd_axis_to_quat(&axis) {
                transform.rotation = transform.rotation * q;
            }
            info!(
                "[usd-bevy] {} {} axis={} rot={:?}",
                sdf_path.as_str(),
                prim_type.as_deref().unwrap_or(""),
                axis,
                transform.rotation
            );
        }
        // `xformOp:scale` (UsdGeomXformable) — non-uniform scaling
        // composed with translate + rotate. Spec-compliant `Cube`
        // prims rely on this to express width/height/depth without
        // the legacy `width`/`height`/`depth` attributes.
        if let Some(v) = get_attribute_as_vec3(reader, &sdf_path, "xformOp:scale") {
            let nonzero = v.x.abs() > 1e-6 || v.y.abs() > 1e-6 || v.z.abs() > 1e-6;
            if nonzero {
                transform.scale = v;
            }
        }

        // Honour `token visibility = "invisible"` and the
        // `lunco:placeholder = true` author flag — both apply as
        // `Visibility::Hidden`. Children inherit unless they
        // override their own visibility (Placeholder Cubes have no
        // children, so propagation is a no-op).
        let final_vis = if invisible || is_placeholder {
            Visibility::Hidden
        } else {
            existing_vis.cloned().unwrap_or(Visibility::Inherited)
        };

        commands.entity(entity).insert((
            transform,
            UsdVisualSynced,
            final_vis,
            InheritedVisibility::default(),
            ViewVisibility::default(),
        ));

        // Spawn children with their transforms pre-populated so physics sees them correctly.
        // This is critical for wheel positions — they must be at the correct offsets from
        // the chassis center before the suspension system runs.
        for child_path in reader.prim_children(&sdf_path) {
            if !reader.prim_is_active(&child_path) {
                continue;
            }

            // Pre-read child transform from USD (canonical decoder).
            let child_tf = read_transform_from_usd(reader, &child_path);

            let base_components = (
                Name::new(child_path.to_string()),
                UsdPrimPath {
                    stage_handle: prim_path.stage_handle.clone(),
                    path: child_path.to_string(),
                },
                child_tf,
                GlobalTransform::default(),
                Visibility::Visible,
                InheritedVisibility::VISIBLE,
                ViewVisibility::default(),
            );

            // Top-level USD prims (children of a scene root tagged with
            // `LoadIntoGrid`) become Grid-direct anchors so big_space's
            // `propagate_high_precision` updates their GlobalTransform.
            // Anything deeper stays as plain `Transform` children of
            // their USD parent's Bevy entity.
            // ChildOf must be set atomically with UsdPrimPath so that
            // observers triggered by the spawn (on_usd_prim_added →
            // instantiate_usd_prim → UsdVisualSynced → process_usd_avian_prims)
            // see the established parentage. Setting ChildOf later via
            // add_child queues a separate command applied AFTER the
            // observer cascade, causing `q_child_of.get(entity).is_err()`
            // to take the root-collider branch and silently mark
            // collider-child prims (e.g. Chassis) as RigidBody::Static.
            // Bevy's relationship system fans the reverse `Children`
            // edge from ChildOf automatically.
            //
            // `UsdInstanceMember` (when present) must ALSO be in this atomic
            // bundle: the `on_usd_prim_added` observer reads it to decide the
            // child's identity regime, so a later `insert` would race the
            // observer and let the child take a colliding `Content` id.
            match (load_into_grid, &child_member) {
                (Some(LoadIntoGrid(grid)), Some(member)) => {
                    commands.spawn((
                        base_components,
                        CellCoord::default(),
                        lunco_core::GridAnchor,
                        ChildOf(*grid),
                        member.clone(),
                    ));
                }
                (Some(LoadIntoGrid(grid)), None) => {
                    commands.spawn((
                        base_components,
                        CellCoord::default(),
                        lunco_core::GridAnchor,
                        ChildOf(*grid),
                    ));
                }
                (None, Some(member)) => {
                    commands.spawn((base_components, ChildOf(entity), member.clone()));
                }
                (None, None) => {
                    commands.spawn((base_components, ChildOf(entity)));
                }
            }
        }
    }
}

/// Observer: fires the moment a new `UsdPrimPath` is added to an entity.
/// If the referenced `UsdStageAsset` is already loaded, the prim is
/// instantiated immediately. Otherwise the entity is tagged
/// `UsdAwaitingStage` and waits for `sync_usd_visuals` to drain it once
/// the asset becomes ready.
///
/// This is the **happy path** in steady state — once a scene is loaded,
/// any newly-spawned `UsdPrimPath` entity (API command, attach
/// operation, recursive child spawn) is processed in the same frame
/// without per-frame polling.
fn on_usd_prim_added(
    trigger: On<Add, UsdPrimPath>,
    q: Query<
        (
            &UsdPrimPath,
            Option<&Visibility>,
            Option<&Transform>,
            Option<&LoadIntoGrid>,
            Has<UsdInstanceRoot>,
            Option<&UsdInstanceMember>,
        ),
        Without<UsdVisualSynced>,
    >,
    mut commands: Commands,
    stages: Res<Assets<UsdStageAsset>>,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let entity = trigger.entity;
    let Ok((prim_path, vis, tf, load_into, is_instance_root, member)) = q.get(entity) else { return; };

    if stages.get(&prim_path.stage_handle).is_none() {
        commands.entity(entity).insert(UsdAwaitingStage);
        return;
    }

    instantiate_usd_prim(
        entity,
        prim_path,
        vis,
        tf,
        load_into,
        is_instance_root,
        member,
        &mut commands,
        &stages,
        &asset_server,
        &mut meshes,
        &mut materials,
    );
}

/// Drains the `UsdAwaitingStage` queue when a stage finishes loading.
/// Each entity whose `UsdPrimPath.stage_handle` matches the newly-loaded
/// asset gets processed exactly once.
///
/// Registered with `run_if(on_message::<AssetEvent<UsdStageAsset>>())`
/// so the system body executes only on frames where an asset event
/// actually fires — zero per-frame cost in steady state.
///
/// **Name retained for compatibility**: downstream systems
/// (`lunco-materials`, `lunco-usd-sim`, `lunco-usd-avian`) order
/// themselves with `.after(sync_usd_visuals)` to ensure they see
/// USD-spawned components. The deferred-stage path now goes through
/// this system; the eager path goes through the `on_usd_prim_added`
/// observer (which fires synchronously during command application, so
/// downstream `.after()` ordering covers it too).
pub fn sync_usd_visuals(
    mut ev: MessageReader<AssetEvent<UsdStageAsset>>,
    q: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&Visibility>,
            Option<&Transform>,
            Option<&LoadIntoGrid>,
            Has<UsdInstanceRoot>,
            Option<&UsdInstanceMember>,
        ),
        (With<UsdAwaitingStage>, Without<UsdVisualSynced>),
    >,
    mut commands: Commands,
    stages: Res<Assets<UsdStageAsset>>,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    use bevy::asset::AssetId;
    let mut loaded: Vec<AssetId<UsdStageAsset>> = Vec::new();
    for event in ev.read() {
        if let AssetEvent::LoadedWithDependencies { id } = event {
            loaded.push(*id);
        }
    }
    if loaded.is_empty() { return; }

    for (entity, prim_path, vis, tf, load_into, is_instance_root, member) in q.iter() {
        if loaded.iter().any(|id| prim_path.stage_handle.id() == *id) {
            commands.entity(entity).remove::<UsdAwaitingStage>();
            instantiate_usd_prim(
                entity,
                prim_path,
                vis,
                tf,
                load_into,
                is_instance_root,
                member,
                &mut commands,
                &stages,
                &asset_server,
                &mut meshes,
                &mut materials,
            );
        }
    }
}

/// Upgrades parked runtime-instance descendants (gap G2/B.1) from their
/// placeholder [`lunco_core::Provenance::Local`] to a deterministic
/// [`lunco_core::Provenance::Derived`] once their instance root has been
/// allocated a [`lunco_core::GlobalEntityId`].
///
/// The loader parks each descendant the instant it is instantiated — the root
/// id is not minted yet at that point. Here we read the root's (authoritative
/// on the server, replicated on clients) id and the member's prim path to mint
/// `Derived{ parent: root_id, role: <path relative to root> }`. Two spawns of
/// the same asset have distinct root ids, so their descendants get distinct
/// ids; and because `derive_id` is a pure function of `(parent, role)`, every
/// peer computes the same id with zero coordination.
///
/// Convergence is at most one frame behind the root's id allocation: the member
/// stays parked (`Local` is a no-op in `assign_global_entity_ids`, so it is
/// never given a colliding auto-allocated id) until this runs, after which the
/// same-frame `assign_global_entity_ids` (PostUpdate) derives the real id.
/// `UsdInstanceMember` is removed on upgrade so each member resolves once.
fn resolve_usd_instance_identities(
    mut commands: Commands,
    members: Query<
        (Entity, &UsdInstanceMember, &UsdPrimPath),
        Without<lunco_core::GlobalEntityId>,
    >,
    roots: Query<&lunco_core::GlobalEntityId>,
) {
    for (entity, member, prim_path) in members.iter() {
        let Ok(root_gid) = roots.get(member.root) else { continue };
        let role = instance_role(&member.root_path, &prim_path.path);
        commands
            .entity(entity)
            .insert(lunco_core::Provenance::Derived {
                parent: root_gid.get(),
                role,
            })
            .remove::<UsdInstanceMember>();
    }
}

/// Resolves a USD texture asset path relative to the stage it belongs to.
fn resolve_texture_path(
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    asset_path: &str,
) -> Option<String> {
    if asset_path.contains("://") {
        return Some(asset_path.to_string());
    }
    let stage_path = asset_server.get_path(stage_id)?;
    let parent = stage_path.path().parent()?;
    let resolved_path = parent.join(asset_path);
    
    match stage_path.source() {
        bevy::asset::io::AssetSourceId::Name(name) => {
            Some(format!("{}://{}", name, resolved_path.to_string_lossy()))
        }
        bevy::asset::io::AssetSourceId::Default => {
            Some(resolved_path.to_string_lossy().into_owned())
        }
    }
}

/// Extractor for parent prim path from property connection target (e.g. `/World/Material/Shader.output` -> `/World/Material/Shader`)
///
/// Delegates to openusd's `SdfPath::prim_path()` so namespaced render contexts
/// and variant selections (e.g. `/World/Mat/Shader{lod=hi}.outputs:surface`)
/// resolve to the correct owning prim rather than being mis-split on the first `.`.
pub fn parent_prim_path(target: &str) -> Option<SdfPath> {
    Some(SdfPath::new(target).ok()?.prim_path())
}

/// Resolves the surface shader prim bound to a geometry prim, following
/// `material:binding` → the material's `outputs:surface` connection → the
/// owning shader prim. Returns `None` if the geometry has no bound material or
/// the material authors no surface output.
///
/// Single source of truth for the bind→shader walk shared by the renderer
/// ([`apply_standard_material`]) and the inspector's material editor.
pub fn resolve_bound_shader(reader: &UsdData, mesh_path: &SdfPath) -> Option<SdfPath> {
    let mat_path_str = read_rel_target(reader, mesh_path, "material:binding")?;
    let mat_path = SdfPath::new(&mat_path_str).ok()?;
    let surf_conn = read_rel_target(reader, &mat_path, "outputs:surface")?;
    parent_prim_path(&surf_conn)
}

/// Applies a standard PBR material to an entity, resolving material bindings
/// and shader networks if present, or falling back to direct prim attributes.
fn apply_standard_material(
    reader: &UsdData,
    sdf_path: &SdfPath,
    mesh_handle: &Handle<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    entity_cmd: &mut EntityCommands,
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
) {
    let mut base_color_texture = None;
    let mut emissive_texture = None;
    let mut metallic_roughness_texture = None;
    let mut normal_map_texture = None;
    let mut occlusion_texture = None;

    // Direct geometry attributes form the baseline. USD `color3f` values are
    // linear scene-referred, and the inspector writes `displayColor` from
    // `base_color.to_linear()`, so read them back as linear (not sRGB) to keep
    // the edit/save/reload round-trip stable.
    let mut base_color = get_attribute_as_vec3(reader, sdf_path, "primvars:displayColor")
        .map(|v| Color::linear_rgb(v.x, v.y, v.z))
        .unwrap_or(Color::WHITE);

    let mut emissive = get_attribute_as_vec3(reader, sdf_path, "primvars:emissiveColor")
        .or_else(|| get_attribute_as_vec3(reader, sdf_path, "emissiveColor"))
        .map(|v| LinearRgba::new(v.x, v.y, v.z, 1.0))
        .unwrap_or(LinearRgba::BLACK);

    let mut metallic = get_attribute_as_f32(reader, sdf_path, "inputs:metallic")
        .or_else(|| get_attribute_as_f32(reader, sdf_path, "metallic"))
        .unwrap_or(0.0);

    let mut roughness = get_attribute_as_f32(reader, sdf_path, "inputs:roughness")
        .or_else(|| get_attribute_as_f32(reader, sdf_path, "roughness"))
        .or_else(|| get_attribute_as_f32(reader, sdf_path, "inputs:perceptual_roughness"))
        .unwrap_or(0.5);

    let mut reflectance = get_attribute_as_f32(reader, sdf_path, "inputs:reflectance")
        .or_else(|| get_attribute_as_f32(reader, sdf_path, "reflectance"))
        .unwrap_or(0.5);

    // A bound material shader network overrides individual channels where it
    // authors them. Channels the shader omits — or whose texture connection
    // fails to resolve — keep the geometry baseline above rather than reverting
    // to a flat-white default.
    if let Some(shader_path) = resolve_bound_shader(reader, sdf_path) {
        // Resolve a shader input's connected texture to a loadable image handle,
        // or `None` if it has no connection / file / resolvable path.
        let load_tex = |input: &str| -> Option<Handle<Image>> {
            let conn = read_rel_target(reader, &shader_path, input)?;
            let texture_path = parent_prim_path(&conn)?;
            let asset_path = get_attribute_as_string(reader, &texture_path, "inputs:file")?;
            let resolved = resolve_texture_path(asset_server, stage_id, &asset_path)?;
            Some(asset_server.load(resolved))
        };

        // diffuseColor: texture, else authored value, else geometry baseline.
        base_color_texture = load_tex("inputs:diffuseColor");
        if base_color_texture.is_none() {
            if let Some(c) = get_attribute_as_vec3(reader, &shader_path, "inputs:diffuseColor") {
                base_color = Color::linear_rgb(c.x, c.y, c.z);
            }
        }

        // emissiveColor
        emissive_texture = load_tex("inputs:emissiveColor");
        if emissive_texture.is_none() {
            if let Some(c) = get_attribute_as_vec3(reader, &shader_path, "inputs:emissiveColor") {
                emissive = LinearRgba::new(c.x, c.y, c.z, 1.0);
            }
        }

        // metallic
        let metallic_texture = load_tex("inputs:metallic");
        if metallic_texture.is_none() {
            if let Some(m) = get_attribute_as_f32(reader, &shader_path, "inputs:metallic") {
                metallic = m;
            }
        }

        // roughness
        let roughness_texture = load_tex("inputs:roughness");
        if roughness_texture.is_none() {
            if let Some(r) = get_attribute_as_f32(reader, &shader_path, "inputs:roughness")
                .or_else(|| get_attribute_as_f32(reader, &shader_path, "inputs:perceptual_roughness"))
            {
                roughness = r;
            }
        }

        metallic_roughness_texture = roughness_texture.or(metallic_texture);

        normal_map_texture = load_tex("inputs:normal");
        occlusion_texture = load_tex("inputs:occlusion");

        if let Some(r) = get_attribute_as_f32(reader, &shader_path, "inputs:reflectance") {
            reflectance = r;
        }
    }

    entity_cmd.insert((
        Mesh3d(mesh_handle.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color,
            base_color_texture,
            emissive,
            emissive_texture,
            metallic,
            perceptual_roughness: roughness,
            metallic_roughness_texture,
            normal_map_texture,
            occlusion_texture,
            reflectance,
            ..default()
        }))
    ));
}

/// Reads a 3-component vector attribute from a USD prim.
///
/// Handles all common USD vector types:
/// - `color3f` → `Value::Vec3f`
/// - `double3` → `Value::Vec3d`
/// - `float3` → `Value::Vec3f`
/// - `Vec<f32>` / `Vec<f64>` array forms
///
/// Returns `None` if the attribute doesn't exist or can't be converted.
/// Reads a string-typed attribute from a USD prim.
///
/// Accepts every reasonable string-shaped USD value:
/// - `Value::String` — authored as `string foo = "..."`.
/// - `Value::Token` — authored as `token foo = "..."` (also the
///   parser's choice for several `lunco:*` attributes).
/// - `Value::AssetPath` — authored as `asset foo = @...@`. This
///   is what the composer emits for the synthesised
///   `lunco:resolvedAsset` so user-facing attributes carry the
///   correct USD type.
///
/// `prim_attribute_value::<String>` covers `String`/`Token` only,
/// so we go through `reader.get` for the attribute path directly
/// to also catch `AssetPath`.
/// Read the stage's `defaultPrim` metadata from the parsed pseudo-root.
///
/// This is the wasm-correct source for the default prim: it reads the
/// already-parsed `TextReader` (populated through the `AssetServer`,
/// which works on web) instead of re-reading the `.usda` file with
/// `std::fs`, which silently returns `None` on wasm. Returns the bare
/// prim name (no leading slash), or `None` when the stage declares no
/// `defaultPrim`. The metadata lives on the pseudo-root spec at the
/// absolute root path.
pub fn stage_default_prim(reader: &UsdData) -> Option<String> {
    // `defaultPrim` is authored as `Value::Token` (see compose.rs), and on
    // openusd `main` `String::try_from(Value::Token)` is an error — only the
    // `String` variant converts. `as_str` coerces `Token`/`String`/`AssetPath`
    // uniformly, so it is the correct reader here.
    let val = reader.field(&SdfPath::abs_root(), "defaultPrim")?;
    let name = val.as_str()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// True if the prim at `path` applies the named API schema, by exact
/// token match against its `apiSchemas` list (or list-op). Canonical
/// shared helper — `lunco-usd-avian` and `lunco-usd-sim` both call
/// this instead of keeping their own (previously diverged) copies.
///
/// Handles every form `apiSchemas` can take: a single `Token`/`String`,
/// a `TokenVec`, or a `TokenListOp` (explicit/prepended/appended/added).
pub fn has_api_schema(reader: &UsdData, path: &SdfPath, schema_name: &str) -> bool {
    let Some(val) = reader.field(path, "apiSchemas") else {
        return false;
    };
    match val {
        Value::Token(s) => s.as_str() == schema_name,
        Value::String(s) => s == schema_name,
        Value::TokenVec(ss) => ss.iter().any(|s| s.as_str() == schema_name),
        Value::TokenListOp(op) => op
            .explicit_items
            .iter()
            .chain(op.prepended_items.iter())
            .chain(op.appended_items.iter())
            .chain(op.added_items.iter())
            .any(|s| s.as_str() == schema_name),
        _ => false,
    }
}

/// First target path of relationship `rel_name` on `prim_path`, as a
/// string (`None` if the relationship is absent/empty). Canonical
/// shared helper — replaces the byte-identical copies that lived in
/// `lunco-usd-avian` and `lunco-usd-sim`.
pub fn read_rel_target(reader: &UsdData, prim_path: &SdfPath, rel_name: &str) -> Option<String> {
    let rel_path_str = format!("{}.{}", prim_path.as_str(), rel_name);
    let Ok(rel_sdf) = SdfPath::new(&rel_path_str) else {
        return None;
    };
    for field in &["targetPaths", "connectionPaths"] {
        if let Some(val) = reader.field(&rel_sdf, field) {
            if let Value::PathListOp(op) = val {
                if let Some(target) = op
                    .explicit_items
                    .first()
                    .or_else(|| op.prepended_items.first())
                    .or_else(|| op.appended_items.first())
                    .or_else(|| op.added_items.first())
                {
                    return Some(target.as_str().to_string());
                }
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────
// Canonical USD attribute / geometry readers (WP-3 — CQ-101..104)
//
// `lunco-usd-bevy` is the lowest USD layer that the other USD crates
// already depend on (`lunco-usd-avian` → here; `lunco-usd-sim` → here;
// the top-level `lunco-usd` aggregator → all three). So the shared
// parsing lives HERE — putting it in `lunco-usd` would be a dependency
// cycle. These functions are the single home for the vec3/token/shape/
// transform/axis parsing that used to be copy-pasted (and drifting)
// between this crate and `lunco-usd-avian`.
//
// `read_vec3_f64` keeps the full f64 4-branch fallback ladder; the
// `Vec3` (f32) and `DVec3` (f64, at the avian call site) wrappers cast
// at the boundary, so physics anchors (`physics:localPos*`) keep f64
// precision.
// ─────────────────────────────────────────────────────────────────────

/// THE canonical USD vec3 reader. Returns the raw `[f64; 3]` so callers
/// keep full precision (avian joint anchors need it; downcasting to f32
/// in the shared layer would silently lose precision).
///
/// Tries, in order: `[f32;3]` → `[f64;3]` → `Vec<f32>` → `Vec<f64>`.
/// **This 4-branch ladder MUST stay intact** — it exists to avoid the
/// documented silent-`None` "bodies launched into orbit" bug, where a
/// `point3f` anchor (parsed as `[f32;3]`) read through a single-type
/// path returned `None` and defaulted the joint anchor to zero.
pub fn read_vec3_f64(reader: &UsdData, path: &SdfPath, attr: &str) -> Option<[f64; 3]> {
    // Fixed-size array forms first (`point3f`/`float3` → `[f32;3]`,
    // `point3d`/`double3` → `[f64;3]`).
    if let Some(v) = reader.prim_attribute_value::<[f32; 3]>(path, attr) {
        return Some([v[0] as f64, v[1] as f64, v[2] as f64]);
    }
    if let Some(v) = reader.prim_attribute_value::<[f64; 3]>(path, attr) {
        return Some([v[0], v[1], v[2]]);
    }
    // `Vec<f32>`/`Vec<f64>` array forms (rare in authored USD).
    if let Some(v) = reader.prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some([v[0] as f64, v[1] as f64, v[2] as f64]); }
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some([v[0], v[1], v[2]]); }
    }
    None
}

/// Reads a 3-component vector attribute (`color3f` / `double3` / `float3`
/// and `Vec<f32>`/`Vec<f64>` array forms) from a USD prim as a Bevy
/// `Vec3` (f32). Thin wrapper over [`read_vec3_f64`] — reused by
/// downstream crates (e.g. `lunco-usd-sim`'s shader authoring) so there
/// is one canonical vec3 reader. `None` if absent or unconvertible.
pub fn get_attribute_as_vec3(reader: &UsdData, path: &SdfPath, attr: &str) -> Option<Vec3> {
    read_vec3_f64(reader, path, attr).map(|v| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32))
}

/// Canonical USD token/string attribute reader. Reads the attribute's
/// `default` value at `prim.attr` and returns it as a `String` for
/// `token`, `string`, and `asset` value types. `None` if absent or a
/// different type.
pub fn read_token(reader: &UsdData, path: &SdfPath, attr: &str) -> Option<String> {
    let attr_path = path.append_property(attr).ok()?;
    let val = reader.field(&attr_path, "default")?;
    match val {
        Value::String(s) => Some(s.clone()),
        Value::Token(s) => Some(s.to_string()),
        Value::AssetPath(a) => Some(a.as_str().to_string()),
        _ => None,
    }
}

/// Back-compat alias for the string/asset call sites (visibility,
/// `lunco:resolvedAsset`, …). Same impl as [`read_token`].
fn get_attribute_as_string(reader: &UsdData, path: &SdfPath, attr: &str) -> Option<String> {
    read_token(reader, path, attr)
}

/// USD `xformOp:rotateXYZ` (Euler XYZ, **degrees** as authored) → Bevy
/// `Quat` (radians). Canonical so the Euler order/units live in one
/// place across both consumers.
pub fn euler_xyz_deg_to_quat(deg: Vec3) -> Quat {
    Quat::from_euler(
        EulerRot::XYZ,
        deg.x.to_radians(),
        deg.y.to_radians(),
        deg.z.to_radians(),
    )
}

/// Canonical local-transform decode from `xformOp:translate` +
/// `xformOp:rotateXYZ`. Scale is left at `ONE` (callers that need
/// `xformOp:scale` compose it themselves; avian pre-applies scale onto
/// the collider instead). Avian downcasts the resulting `Transform` to
/// `DVec3`/`DQuat` at its call site.
pub fn read_transform_from_usd(reader: &UsdData, path: &SdfPath) -> Transform {
    let translation =
        get_attribute_as_vec3(reader, path, "xformOp:translate").unwrap_or(Vec3::ZERO);
    let rotation = get_attribute_as_vec3(reader, path, "xformOp:rotateXYZ")
        .map(euler_xyz_deg_to_quat)
        .unwrap_or(Quat::IDENTITY);
    Transform { translation, rotation, scale: Vec3::ONE }
}

/// Canonical `UsdGeom` `axis` token → quaternion folding. A Bevy/Avian
/// primitive (`Cylinder`/`Cone`/`Capsule`/`Plane`) is Y-axial; this
/// rotates it onto the authored `axis`. `None` for `"Y"` (already
/// aligned) or an unknown token — callers then leave the rotation
/// untouched. Adding an axis case touches exactly this one place.
pub fn usd_axis_to_quat(axis: &str) -> Option<Quat> {
    match axis {
        "X" => Some(Quat::from_rotation_arc(Vec3::Y, Vec3::X)),
        "Z" => Some(Quat::from_rotation_arc(Vec3::Y, Vec3::Z)),
        _ => None,
    }
}

/// Dimensions of a USD primitive shape prim, with the spec-compliant
/// defaults applied. One home (CQ-102) so the avian collider and the
/// bevy mesh never desync. `Cube::size` is the spec form (default 2.0);
/// `Cube::legacy_extents` carries the deprecated `width`/`height`/`depth`
/// **full-extent** form when all three are authored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeDims {
    Cube { size: f64, legacy_extents: Option<[f64; 3]> },
    Sphere { radius: f64 },
    Cylinder { radius: f64, height: f64 },
    Cone { radius: f64, height: f64 },
    Capsule { radius: f64, height: f64 },
    Plane { width: f64, length: f64 },
}

/// Read the dimensions of a USD primitive shape prim. `type_name` is the
/// prim's `typeName` token (callers already have it). Returns `None` for
/// an unsupported type. **The defaults here are the single source of
/// truth** for both `lunco-usd-avian` (→ `Collider`) and this crate
/// (→ `Mesh`); changing one here changes both, so they can't drift.
pub fn read_shape_dims(reader: &UsdData, path: &SdfPath, type_name: &str) -> Option<ShapeDims> {
    let dims = match type_name {
        "Cube" => {
            let size = reader.prim_attribute_value::<f64>(path, "size").unwrap_or(2.0);
            let legacy_extents = match (
                reader.prim_attribute_value::<f64>(path, "width"),
                reader.prim_attribute_value::<f64>(path, "height"),
                reader.prim_attribute_value::<f64>(path, "depth"),
            ) {
                (Some(w), Some(h), Some(d)) => Some([w, h, d]),
                _ => None,
            };
            ShapeDims::Cube { size, legacy_extents }
        }
        "Sphere" => ShapeDims::Sphere {
            radius: reader.prim_attribute_value::<f64>(path, "radius").unwrap_or(1.0),
        },
        "Cylinder" => ShapeDims::Cylinder {
            radius: reader.prim_attribute_value::<f64>(path, "radius").unwrap_or(1.0),
            height: reader.prim_attribute_value::<f64>(path, "height").unwrap_or(2.0),
        },
        "Cone" => ShapeDims::Cone {
            radius: reader.prim_attribute_value::<f64>(path, "radius").unwrap_or(1.0),
            height: reader.prim_attribute_value::<f64>(path, "height").unwrap_or(2.0),
        },
        "Capsule" => ShapeDims::Capsule {
            radius: reader.prim_attribute_value::<f64>(path, "radius").unwrap_or(0.5),
            height: reader.prim_attribute_value::<f64>(path, "height").unwrap_or(1.0),
        },
        "Plane" => ShapeDims::Plane {
            width: reader.prim_attribute_value::<f64>(path, "width").unwrap_or(2.0),
            length: reader.prim_attribute_value::<f64>(path, "length").unwrap_or(2.0),
        },
        _ => return None,
    };
    Some(dims)
}

/// Reads a float-like attribute (`float` / `double` / `int`) from a USD prim.
/// Falls back to the metadata-based "default" key if not found (needed for dome/light attributes).
pub fn get_attribute_as_f32(reader: &UsdData, path: &SdfPath, attr: &str) -> Option<f32> {
    if let Some(v) = reader.prim_attribute_value::<f32>(path, attr) {
        return Some(v);
    }
    if let Some(v) = reader.prim_attribute_value::<f64>(path, attr) {
        return Some(v as f32);
    }
    if let Some(v) = reader.prim_attribute_value::<i32>(path, attr) {
        return Some(v as f32);
    }
    light::get_attribute_as_f32(reader, path, attr)
}

/// Marker inserted on prim entities that own both a primitive Cube
/// fallback mesh **and** a glTF [`SceneRoot`]. Used by
/// [`hide_glb_placeholder_meshes`] to find these entities cheaply.
#[derive(Component)]
pub struct GlbPlaceholder;

/// Stores the URI of the GLB asset that this placeholder is waiting for.
/// Used for diagnostic labels if the asset fails to load.
#[derive(Component)]
pub struct PlaceholderAssetUri(pub String);

/// Marker for entities spawned as diagnostic stubs when asset loading fails.
#[derive(Component)]
pub struct DiagnosticStub;

/// Marker for the textured quad that displays the failed asset's filename.
#[derive(Component)]
pub struct DiagnosticStubLabel;

/// Attached to a freshly-spawned [`DiagnosticStub`] that still needs its
/// filename baked onto its faces. A separate pass ([`bake_pending_labels`])
/// does the baking once [`DiagnosticLabelFont`] is available — this decouples
/// *when the asset fails* from *when the font is ready*, which matters on web
/// where the font arrives asynchronously over HTTP.
#[derive(Component)]
pub struct PendingDiagnosticLabel {
    /// Full label text (prefix + file name).
    pub text: String,
    /// World size of the diagnostic box, for fitting the label per face.
    pub box_size: Vec3,
}

/// Tunable appearance of the failed-asset diagnostic stub. Insert your own
/// before [`UsdBevyPlugin`] builds (or mutate the resource at runtime) to
/// override any field — nothing here is a hard-coded magic constant.
#[derive(Resource, Clone, Debug)]
pub struct DiagnosticLabelConfig {
    /// Glyph height used when rasterising the label, in texture pixels
    /// (higher = crisper text, larger texture).
    pub font_px: f32,
    /// Transparent border around the text, in texture pixels.
    pub padding_px: f32,
    /// Text colour, RGB 0-255.
    pub text_color: [u8; 3],
    /// Backdrop colour painted behind the text, RGBA 0-255.
    pub bg_color: [u8; 4],
    /// Fraction (0..1) of each box face the label may cover.
    pub face_coverage: f32,
    /// Colour of the semi-transparent diagnostic box itself.
    pub box_color: Color,
    /// String prepended to the file name (e.g. `"Missing: "`).
    pub prefix: String,
    /// `true` → label on all six faces; `false` → only the +Z front face.
    pub all_faces: bool,
    /// Seconds a placeholder may wait for its glTF scene before the stub is
    /// shown. Covers web, where a 404 may never report a clean `is_failed()`.
    pub grace_secs: f32,
}

impl Default for DiagnosticLabelConfig {
    fn default() -> Self {
        Self {
            font_px: 64.0,
            padding_px: 24.0,
            text_color: [255, 255, 255],
            bg_color: [20, 0, 0, 140],
            face_coverage: 0.85,
            box_color: Color::srgba(1.0, 0.0, 0.0, 0.7),
            prefix: "Missing: ".to_string(),
            all_faces: true,
            grace_secs: 8.0,
        }
    }
}

/// Caches the DejaVu Sans face used to bake filename labels into textures, so
/// the `.ttf` is loaded at most once (not per failed asset). `None` until the
/// font is loaded (native: read from storage at startup; web: fetched over
/// HTTP). If it never loads, stubs still show the red box, just without text.
#[derive(Resource, Default)]
pub struct DiagnosticLabelFont(pub Option<std::sync::Arc<ab_glyph::FontVec>>);

/// Holds the receiver from [`lunco_assets::font::load_dejavu_sans_bytes`]
/// until the bytes land. The same channel mechanism works on native (bytes
/// ready immediately) and web (bytes fetched async), so the plugin has no
/// platform branches. Removed once the font installs.
#[derive(Resource)]
struct DiagnosticFontLoad(std::sync::Mutex<std::sync::mpsc::Receiver<Vec<u8>>>);

/// Parses raw `.ttf` bytes into [`DiagnosticLabelFont`].
fn install_diagnostic_font(font: &mut DiagnosticLabelFont, bytes: Vec<u8>) {
    match ab_glyph::FontVec::try_from_vec(bytes) {
        Ok(f) => font.0 = Some(std::sync::Arc::new(f)),
        Err(e) => warn!("[usd-bevy] diagnostic label font parse failed: {e}"),
    }
}

/// Startup: kick off the DejaVu Sans load via `lunco-assets` (which owns the
/// native-read / web-fetch procedure) and stash the receiver for
/// [`poll_diagnostic_label_font`] to drain.
fn load_diagnostic_label_font(mut commands: Commands) {
    let rx = lunco_assets::font::load_dejavu_sans_bytes();
    commands.insert_resource(DiagnosticFontLoad(std::sync::Mutex::new(rx)));
}

/// Drains the font-load channel and installs the face once the bytes arrive
/// (frame 1 on native, whenever the fetch lands on web). Uniform across
/// platforms; removes the loader resource when done.
fn poll_diagnostic_label_font(
    load: Option<Res<DiagnosticFontLoad>>,
    mut font: ResMut<DiagnosticLabelFont>,
    mut commands: Commands,
) {
    if font.0.is_some() {
        return;
    }
    let Some(load) = load else { return };
    let received = load.0.lock().ok().and_then(|rx| rx.try_recv().ok());
    if let Some(bytes) = received {
        info!("[usd-bevy] diagnostic label font loaded ({} bytes)", bytes.len());
        install_diagnostic_font(&mut font, bytes);
        commands.remove_resource::<DiagnosticFontLoad>();
    }
}

/// CPU-rasterises `text` into an RGBA [`Image`] per [`DiagnosticLabelConfig`]:
/// coloured glyphs on a configurable backdrop. Baked once per failed asset —
/// no camera, no render pass, no per-frame work. `None` if `text` is empty.
fn rasterize_label(text: &str, font: &ab_glyph::FontVec, cfg: &DiagnosticLabelConfig) -> Option<Image> {
    use ab_glyph::{Font, ScaleFont, point, PxScale};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    use bevy::asset::RenderAssetUsages;

    if text.is_empty() {
        return None;
    }
    let px = cfg.font_px.max(1.0);
    let pad = cfg.padding_px.max(0.0);
    let scaled = font.as_scaled(PxScale::from(px));

    // Measure advance width (with kerning) for the whole string.
    let mut width = 0.0_f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let gid = font.glyph_id(c);
        if let Some(p) = prev {
            width += scaled.kern(p, gid);
        }
        width += scaled.h_advance(gid);
        prev = Some(gid);
    }
    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let img_w = (width + pad * 2.0).ceil().max(1.0) as usize;
    let img_h = (ascent - descent + pad * 2.0).ceil().max(1.0) as usize;

    // Configurable backdrop so the text reads over the box behind the quad.
    let mut buf = vec![0u8; img_w * img_h * 4];
    for px4 in buf.chunks_mut(4) {
        px4.copy_from_slice(&cfg.bg_color);
    }

    // Draw each glyph in the configured text colour, coverage-blended.
    let [tr, tg, tb] = cfg.text_color;
    let tc = [tr as u16, tg as u16, tb as u16];
    let mut caret = point(pad, pad + ascent);
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let gid = font.glyph_id(c);
        if let Some(p) = prev {
            caret.x += scaled.kern(p, gid);
        }
        let glyph = gid.with_scale_and_position(PxScale::from(px), caret);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bb = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let x = bb.min.x as i32 + gx as i32;
                let y = bb.min.y as i32 + gy as i32;
                if x < 0 || y < 0 || x as usize >= img_w || y as usize >= img_h {
                    return;
                }
                let idx = (y as usize * img_w + x as usize) * 4;
                let a = (cov * 255.0) as u16;
                for k in 0..3 {
                    let bg = buf[idx + k] as u16;
                    buf[idx + k] = ((tc[k] * a + bg * (255 - a)) / 255) as u8;
                }
                buf[idx + 3] = buf[idx + 3].max((cov * 255.0) as u8);
            });
        }
        caret.x += scaled.h_advance(gid);
        prev = Some(gid);
    }

    Some(Image::new(
        Extent3d {
            width: img_w as u32,
            height: img_h as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        buf,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    ))
}

/// Bakes the filename texture onto every (or just the front) face of each
/// pending diagnostic stub, once the label font is available. Runs each frame
/// but only touches stubs that still carry [`PendingDiagnosticLabel`].
fn bake_pending_labels(
    mut commands: Commands,
    cfg: Res<DiagnosticLabelConfig>,
    font: Res<DiagnosticLabelFont>,
    pending: Query<(Entity, &PendingDiagnosticLabel)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(font) = font.0.as_ref() else { return };
    for (stub, pending) in pending.iter() {
        let Some(image) = rasterize_label(&pending.text, font, &cfg) else {
            commands.entity(stub).remove::<PendingDiagnosticLabel>();
            continue;
        };
        let aspect = (image.width() as f32 / image.height().max(1) as f32).max(0.01);
        let tex = images.add(image);
        // One material shared across all faces.
        let label_mat = materials.add(StandardMaterial {
            base_color_texture: Some(tex),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            cull_mode: None, // readable from either side
            ..default()
        });
        let s = pending.box_size;
        let (hx, hy, hz) = (s.x / 2.0, s.y / 2.0, s.z / 2.0);
        let eps = 0.01;
        use std::f32::consts::{FRAC_PI_2, PI};
        // Each face: outward offset + a rotation that turns the default
        // +Z-facing `Rectangle` to face outward, plus the face's
        // (horizontal, vertical) extent for sizing.
        let faces: &[(Vec3, Quat, f32, f32)] = if cfg.all_faces {
            &[
                (Vec3::new(0.0, 0.0, hz + eps), Quat::IDENTITY, s.x, s.y),                 // +Z
                (Vec3::new(0.0, 0.0, -hz - eps), Quat::from_rotation_y(PI), s.x, s.y),     // -Z
                (Vec3::new(hx + eps, 0.0, 0.0), Quat::from_rotation_y(FRAC_PI_2), s.z, s.y), // +X
                (Vec3::new(-hx - eps, 0.0, 0.0), Quat::from_rotation_y(-FRAC_PI_2), s.z, s.y), // -X
                (Vec3::new(0.0, hy + eps, 0.0), Quat::from_rotation_x(-FRAC_PI_2), s.x, s.z), // +Y
                (Vec3::new(0.0, -hy - eps, 0.0), Quat::from_rotation_x(FRAC_PI_2), s.x, s.z), // -Y
            ]
        } else {
            &[(Vec3::new(0.0, 0.0, hz + eps), Quat::IDENTITY, s.x, s.y)]
        };
        let cover = cfg.face_coverage.clamp(0.05, 1.0);
        commands.entity(stub).with_children(|p| {
            for &(offset, rot, fw, fh) in faces {
                // Fit the label inside the face, keeping the texture aspect.
                let mut qw = (fw * cover).max(0.1);
                let mut qh = qw / aspect;
                if qh > fh * cover {
                    qh = (fh * cover).max(0.05);
                    qw = qh * aspect;
                }
                p.spawn((
                    Name::new("DiagnosticStubLabel"),
                    DiagnosticStubLabel,
                    Mesh3d(meshes.add(Rectangle::new(qw, qh))),
                    MeshMaterial3d(label_mat.clone()),
                    Transform::from_translation(offset).with_rotation(rot),
                ));
            }
        });
        commands.entity(stub).remove::<PendingDiagnosticLabel>();
    }
}

/// Removes the primitive Cube/Sphere/Cylinder fallback mesh once its
/// sibling [`SceneRoot`] reports its glTF [`Scene`] asset fully loaded.
fn hide_glb_placeholder_meshes(
    mut commands: Commands,
    // `Option<...>` so the system no-ops (instead of panicking on param
    // validation) in minimal apps that never `init_asset::<Scene>()` — e.g.
    // headless tests that add `UsdBevyPlugin` without the full scene pipeline.
    // Production always registers `Scene`, so behaviour there is unchanged.
    events: Option<MessageReader<AssetEvent<Scene>>>,
    scene_roots: Query<(Entity, &SceneRoot, Option<&ChildOf>), With<GlbPlaceholder>>,
    children: Query<&Children>,
    has_mesh: Query<(), With<Mesh3d>>,
    mut visibility: Query<&mut Visibility>,
) {
    let Some(mut events) = events else { return };
    for ev in events.read() {
        if let AssetEvent::LoadedWithDependencies { id } = ev {
            for (e, root, parent) in scene_roots.iter() {
                if root.0.id() == *id {
                    if let Ok(mut vis) = visibility.get_mut(e) {
                        *vis = Visibility::Inherited;
                    }
                    commands.entity(e)
                        .remove::<Mesh3d>()
                        .remove::<MeshMaterial3d<StandardMaterial>>()
                        .remove::<GlbPlaceholder>()
                        .remove::<PlaceholderAssetUri>();

                    if let Some(parent) = parent {
                        if let Ok(siblings) = children.get(parent.0) {
                            for sib in siblings.iter() {
                                if sib != e && has_mesh.get(sib).is_ok() {
                                    commands.entity(sib)
                                        .remove::<Mesh3d>()
                                        .remove::<MeshMaterial3d<StandardMaterial>>();
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Reveals a red, semi-transparent diagnostic box when a [`GlbPlaceholder`]'s
/// glTF scene fails to load or never loads within
/// [`DiagnosticLabelConfig::grace_secs`] (the web case, where a 404 may not
/// surface a clean `is_failed()`). The filename label is baked on separately by
/// [`bake_pending_labels`] once the font is ready, via [`PendingDiagnosticLabel`].
pub fn reveal_placeholder_on_failure(
    mut commands: Commands,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    stages: Res<Assets<UsdStageAsset>>,
    cfg: Res<DiagnosticLabelConfig>,
    scene_roots: Query<(Entity, &SceneRoot, &GlobalTransform, &PlaceholderAssetUri, &UsdPrimPath), (With<GlbPlaceholder>, Without<DiagnosticStub>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    // Per-placeholder time spent waiting on its glTF scene. Used to trip the
    // grace timeout on web, where a broken load may never report `is_failed()`.
    mut waited: Local<std::collections::HashMap<Entity, f32>>,
) {
    for (e, root, global_transform, uri, prim_path) in scene_roots.iter() {
        let state = asset_server.load_state(root.0.id());
        // The asset arrived — stop tracking; `hide_glb_placeholder_meshes`
        // drops the marker on the next `LoadedWithDependencies` event.
        if state.is_loaded() {
            waited.remove(&e);
            continue;
        }
        let elapsed = waited.entry(e).or_insert(0.0);
        *elapsed += time.delta_secs();
        let timed_out = *elapsed >= cfg.grace_secs;
        if state.is_failed() || timed_out {
            waited.remove(&e);
            info!(
                "[usd-bevy] asset {} for {:?} ({}), spawning diagnostic stub",
                if timed_out { "did not load in time" } else { "load FAILED" },
                root.0.id(),
                uri.0,
            );

            // Spawn a new independent diagnostic entity in world space
            let transform = global_transform.compute_transform();

            // Default scale
            let mut scale = Vec3::ONE;

            // Attempt to resolve dimensions from USD prim attributes
            if let Some(stage) = stages.get(&prim_path.stage_handle) {
                let reader = &*stage.reader;

                // Navigate up from the current prim to its parent to find the sibling "Placeholder"
                let parent_path = prim_path.path.rsplitn(2, '/').nth(1).unwrap_or("");
                let sibling_placeholder_path = format!("{}/Placeholder", parent_path);

                // Helper to check attributes
                let check_path = |path: &str| -> Option<Vec3> {
                    if let Ok(sdf_path) = SdfPath::new(path) {
                        if let Some(s) = get_attribute_as_vec3(reader, &sdf_path, "xformOp:scale") {
                            Some(s)
                        } else if let Some(size) = reader.prim_attribute_value::<f64>(&sdf_path, "size") {
                            Some(Vec3::splat(size as f32))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };

                // Check sibling first, then parent prim path itself
                if let Some(s) = check_path(&sibling_placeholder_path)
                    .or_else(|| check_path(&prim_path.path))
                {
                    info!("[usd-bevy] Found scale: {:?}", s);
                    scale = s;
                } else {
                    info!("[usd-bevy] No scale or size found on paths: {:?} or {:?}", sibling_placeholder_path, prim_path.path);
                }
            }

            info!("[usd-bevy] Computed stub scale: {:?}", scale);

            // Just the filename — strip the `lunco-lib://…/` path prefix and
            // the `#Scene0` glTF sub-label.
            let file_name = uri
                .0
                .rsplit('/')
                .next()
                .unwrap_or(&uri.0)
                .split('#')
                .next()
                .unwrap_or(&uri.0);

            commands.spawn((
                Name::new("DiagnosticStub"),
                Mesh3d(meshes.add(Cuboid::from_size(scale))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: cfg.box_color,
                    emissive: LinearRgba::from(cfg.box_color),
                    alpha_mode: AlphaMode::Blend, // Support transparency
                    unlit: true, // readable even with no scene lighting
                    ..default()
                })),
                transform,
                Visibility::Visible,
                DiagnosticStub,
                // The label is baked on once the font is ready (frame 1 on
                // native, whenever the fetch lands on web).
                PendingDiagnosticLabel {
                    text: format!("{}{file_name}", cfg.prefix),
                    box_size: scale,
                },
            ));

            // Mark the original prim as having a diagnostic stub
            commands.entity(e).insert(DiagnosticStub);
        }
    }
}


#[cfg(test)]
mod instance_identity_tests {
    //! Gap G2/B.1: descendants of a runtime-spawned USD instance must derive a
    //! hierarchical identity from the instance root, so two spawns of the same
    //! asset (identical composed prim paths) don't collide.
    use super::*;
    use lunco_core::{identity::derive_id, GlobalEntityId, Provenance};

    #[test]
    fn role_is_path_relative_to_root() {
        assert_eq!(instance_role("/SolarPanel", "/SolarPanel/Frame"), "Frame");
        assert_eq!(instance_role("/SolarPanel", "/SolarPanel/Frame/Bolt"), "Frame/Bolt");
        // Prefix mismatch → fall back to the full (slash-trimmed) path.
        assert_eq!(instance_role("/SolarPanel", "/Other/Frame"), "Other/Frame");
        // Root itself (degenerate) → non-empty fallback, never "".
        assert_eq!(instance_role("/SolarPanel", "/SolarPanel"), "SolarPanel");
    }

    /// The core regression: two instances of the SAME asset compose identical
    /// prim paths, so the same role string — yet distinct root ids must yield
    /// distinct descendant ids. Drives the real resolver system.
    #[test]
    fn two_instances_of_same_asset_get_distinct_descendant_ids() {
        let mut app = App::new();

        // Two instance roots, each pinned to a unique (replicated) id.
        let root_a = app.world_mut().spawn(GlobalEntityId::from_raw(1001)).id();
        let root_b = app.world_mut().spawn(GlobalEntityId::from_raw(2002)).id();

        // A descendant of each — identical asset-local path "/Rover/Wheel_FL".
        let spawn_member = |app: &mut App, root: Entity| {
            app.world_mut()
                .spawn((
                    UsdInstanceMember { root, root_path: "/Rover".into() },
                    UsdPrimPath { stage_handle: Handle::default(), path: "/Rover/Wheel_FL".into() },
                ))
                .id()
        };
        let wheel_a = spawn_member(&mut app, root_a);
        let wheel_b = spawn_member(&mut app, root_b);

        app.world_mut()
            .run_system_cached(resolve_usd_instance_identities)
            .unwrap();

        let pa = app.world().get::<Provenance>(wheel_a).cloned().unwrap();
        let pb = app.world().get::<Provenance>(wheel_b).cloned().unwrap();

        // Hierarchical: same role, different parent.
        assert_eq!(pa, Provenance::Derived { parent: 1001, role: "Wheel_FL".into() });
        assert_eq!(pb, Provenance::Derived { parent: 2002, role: "Wheel_FL".into() });

        // The whole point: the derived ids are distinct (no collision) and
        // deterministic.
        let id_a = derive_id(&pa).unwrap();
        let id_b = derive_id(&pb).unwrap();
        assert_ne!(id_a, id_b, "two instances must not collide");
        assert_eq!(derive_id(&pa).unwrap(), id_a, "derive_id is deterministic");

        // Membership consumed → each member resolves exactly once.
        assert!(app.world().get::<UsdInstanceMember>(wheel_a).is_none());
    }

    /// A member whose root has no id yet stays parked (no premature/colliding
    /// id), so the upgrade is correctly deferred to a later frame.
    #[test]
    fn member_waits_for_root_id() {
        let mut app = App::new();
        let root = app.world_mut().spawn_empty().id(); // no GlobalEntityId yet
        let member = app
            .world_mut()
            .spawn((
                UsdInstanceMember { root, root_path: "/Rover".into() },
                UsdPrimPath { stage_handle: Handle::default(), path: "/Rover/Wheel_FL".into() },
            ))
            .id();

        app.world_mut()
            .run_system_cached(resolve_usd_instance_identities)
            .unwrap();

        // Still parked: no Derived stamped, membership retained for retry.
        assert!(app.world().get::<Provenance>(member).is_none());
        assert!(app.world().get::<UsdInstanceMember>(member).is_some());
    }
}

