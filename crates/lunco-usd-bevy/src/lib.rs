//! # LunCoSim USD → Bevy Visual Sync
//!
//! Responsible for spawning child entities for USD prims and attaching visual components
//! (meshes, materials, transforms). This is the **first** plugin in the USD processing
//! pipeline — it must run before the animation, Avian physics, and Sim
//! simulation plugins.
//!
//! ## How It Works
//!
//! 1. The asset loader (`UsdLoader`) reads a `.usda` file and fetches its complete
//!    composed layer closure through `lunco-usd-compose`.
//! 2. The asset loader composes and snapshots the full default-time read surface on
//!    the async asset path; `process_queued_usd_visuals` only binds that owned data
//!    to Bevy over bounded frames.
//! 3. For each renderable prim, it uses the prepared authored structure and creates or schedules
//!    geometry based on the prim type (`Cube`, `Cylinder`, `Sphere`) using explicit dimensions
//!    from the USD file. A prim explicitly marked as a procedural camera background has no
//!    geometry projection; its appearance intent is consumed by the background render pass.
//! 4. It spawns the planned child hierarchy with pre-populated transforms so
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
//! asset may not be loaded yet (async loading). The observer and the loaded-stage
//! event both publish the same queue marker; `process_queued_usd_visuals` is the
//! single reader and marks each projected entity with `UsdSceneProjected`.

use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use big_space::prelude::CellCoord;
// Appearance **intent**, not a material: this crate must never name
// `MeshMaterial3d`/`StandardMaterial` (they live in `bevy_pbr` → wgpu + naga).
// `lunco-render-bevy` observes these and binds the real material.
// See docs/architecture/render-decoupling.md.
use lunco_materials::ProceduralSkybox;
use lunco_render::{PbrLook, PbrTextures, SurfaceAlpha};
use openusd::sdf::Path as SdfPath;
use openusd::sdf::Value;

/// Light and transform ports — the port backend for what `light`/`compose` spawn.
pub mod scene_ports;
use lunco_usd_bevy_core::animation::{prim_is_animated, ANIMATED_SHADER_INPUTS};
use lunco_usd_bevy_core::point_instancer::read_point_instancer;
use lunco_usd_bevy_core::read::{
    attr_has_time_samples, read_authored_bool_strict, read_primvar_f32_strict,
    read_primvar_vec3_strict, read_vec3_f64,
};
use lunco_usd_bevy_core::source::{UsdSourceText, UsdSourceTextLoader};
use lunco_usd_bevy_core::{
    canonical, program, read, UsdInstanceMember, UsdInstanceProjection, UsdInstanceRoot, UsdLoader,
    UsdStageAsset,
};
use lunco_usd_bevy_core::{
    canonical::CanonicalStages, local_transform_at, parent_prim_path, read_transform_from_usd,
    resolve_bound_shader, resolve_stage_prim_path, stage_convention, UsdRead, UsdReadObject,
};
use lunco_usd_bevy_lathe as lathe;
use lunco_usd_bevy_light::light;
use lunco_usd_bevy_scene::{
    is_preview_only, read_primitive_axis, read_shape_dims, read_usd_mesh_points,
    read_usd_mesh_topology, scene_root_ancestor, usd_axis_to_quat, GlbPlaceholder,
    PlaceholderAssetUri, ShapeDims, UsdAnimated, UsdPointInstance, UsdPointInstancer,
    UsdPreviewOnly, UsdPrimPath, UsdSceneAwaitingStage, UsdSceneGeometryPending, UsdScenePlugin,
    UsdSceneProjected, UsdSceneProjectionFailed, UsdSceneProjectionQueued, UsdSceneRoot,
    UsdSceneSyncSet, UsdStageRevision, UsdVisualMeshTarget, UsdVisualProjectionSet,
};
use openusd::schemas::geom::tokens as gtok;
/// Bevy plugin for USD visual synchronization.
///
/// Registers the `UsdStageAsset` type, the USD asset loader, and the `sync_usd_visuals`
/// system that processes USD prims into Bevy entities with meshes and transforms.
pub struct UsdVisualPlugin;

impl Plugin for UsdVisualPlugin {
    fn build(&self, app: &mut App) {
        // USD mesh and light projection consumes the authoritative graphics
        // settings. Initialise the documented default at this boundary so
        // projectors never invent a separate quality profile.
        app.init_resource::<lunco_render::RenderingQualitySettings>();
        app.add_plugins((
            UsdScenePlugin,
            lunco_usd_bevy_camera::UsdCameraPlugin,
            lunco_usd_bevy_light::UsdLightPlugin,
        ))
        .configure_sets(
            Update,
            lunco_usd_bevy_camera::UsdCameraProjectionSet.after(UsdVisualProjectionSet),
        );

        // Core glTF/USD scene component types. The workspace runs bevy with
        // `default-features = false`, so bevy's `reflect_auto_register` is OFF
        // and these are NOT auto-registered. Any glTF `WorldAssetRoot` we spawn (USD
        // payload overlay, terrain, rovers) is deserialized via
        // `Scene::write_to_world_with`, which panics on the first unregistered
        // component type. Register the bounded set a glTF scene can contain so
        // the registry is complete WITHOUT pulling the inventory-based
        // auto-register closure into the link (it overflowed clang's command
        // line — see the bevy dep note in `lunco-luncosim/Cargo.toml`).
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
            // NOTE: `MeshMaterial3d<StandardMaterial>` used to be registered here for
            // the glTF-scene deserializer. It is `bevy_pbr` — unnameable in this crate
            // now — and Bevy's own `MaterialPlugin<StandardMaterial>` (added by
            // `PbrPlugin`) already registers it in every render build, which is the
            // only build where a glTF scene carries one.
            // Skinned/morph meshes — glTF rover payloads are skinned.
            .register_type::<bevy::mesh::skinning::SkinnedMesh>()
            .register_type::<bevy::mesh::morph::MorphWeights>()
            .register_type::<bevy::mesh::morph::MeshMorphWeights>()
            // Lights the glTF loader may embed (USD-authored lights take a
            // separate path, but a glTF can carry its own).
            .register_type::<DirectionalLight>()
            // Light and transform ports. Registered HERE, beside the systems that spawn the
            // components they read, so a wire into a light lands in a headless build too —
            // the value is scene data, not a render resource.
            .add_plugins(scene_ports::ScenePortsPlugin)
            .register_type::<PointLight>()
            .register_type::<SpotLight>()
            .register_type::<bevy::gltf::GltfExtras>()
            .register_type::<bevy::gltf::GltfSceneExtras>()
            .register_type::<bevy::gltf::GltfMeshExtras>()
            .register_type::<bevy::gltf::GltfMeshName>()
            // bevy 0.19: the glTF loader also stamps the scene name on spawned
            // roots — unregistered, `world_instance_spawner_system` PANICS the
            // frame a glb component (habitat, lander) instantiates.
            .register_type::<bevy::gltf::GltfSceneName>()
            .register_type::<bevy::gltf::GltfMaterialExtras>()
            .register_type::<bevy::gltf::GltfMaterialName>();
        app.init_asset::<UsdStageAsset>()
            .register_asset_loader(UsdLoader)
            // E1b: raw-source asset so a scene document's base layer can be read
            // through the same (web-ready) asset source the live world uses.
            .init_asset::<UsdSourceText>()
            .register_asset_loader(UsdSourceTextLoader)
            .register_type::<UsdPrimPath>()
            .register_type::<lunco_core::UsdPrimKind>()
            .register_type::<UsdAnimated>()
            .register_type::<UsdResetXformStack>()
            // The retained NurbsPatch definition + its parametric layer. Registered
            // (not merely derived) because registration is what makes them reachable
            // from a script: `set(id, "UsdLathe.profile.exit_radius", 1.6)` resolves
            // through `AppTypeRegistry` by short type path. An unregistered component
            // fails there with `unknown type`, which is exactly the error the old
            // `set(me, "NurbsPatch.points", ...)` actuator died on.
            .register_type::<lathe::NurbsSurface>()
            .register_type::<lathe::UsdLathe>()
            .register_type::<lathe::LatheProfile>()
            .init_resource::<UsdVisualProjectionSettings>()
            // The live canonical stages are main-thread `NonSend` resources
            // because OpenUSD `Stage` is `!Send`. Initial projection uses each
            // asset's worker-produced `UsdStageProjectionPlan`; this resource
            // serves authoring and incremental edits.
            .init_non_send::<canonical::CanonicalStages>()
            // The one "USD projection changed" signal every derived consumer
            // gates on. `PreUpdate` runs before the `Update` producers, so a
            // spawn is observed here the frame AFTER it is applied and the
            // view-model re-derives one frame later — the right trade for a
            // panel, not for anything a simulation step depends on.
            .add_observer(on_usd_prim_added)
            .add_observer(on_cell_coord_added)
            // Rover/vehicle-mounted cameras: a nested `def Camera` is realised
            // as a grid-direct follower. `resolve` rigs it once during load; `follow`
            // tracks the mount each frame, before transform propagation.
            // `!resetXformStack!` detachment. In `Update`, so the reparent has
            // flushed long before `PostUpdate` propagates transforms — a prim
            // never renders one frame with the ancestor chain still applied.
            // Runs every frame rather than once: the ancestry a prim must be
            // lifted out of is itself spawned asynchronously during load.
            .add_systems(Update, detach_reset_xform_stack_prims)
            // Parametric lathes. BOTH are `Changed`-filtered, so on a scene nobody is
            // editing they iterate NOTHING — a nozzle's shape does not change while
            // the engine burns, and re-lathing it per frame is the exact mistake the
            // deleted rhai actuator made.
            //
            // `.chain()` matters: a parameter edit must reach the mesh in the SAME
            // frame. Unchained, `relathe_changed`'s write to `NurbsSurface` would be
            // seen by `regenerate_patch_meshes` only on the next run, so every
            // parameter change would render one frame stale — invisible when dragging
            // a slider, and a real off-by-one-frame artefact in a recorded take.
            .add_systems(
                Update,
                (lathe::relathe_changed, lathe::regenerate_patch_meshes)
                    .chain()
                    .after(poll_pending_usd_meshes),
            )
            .add_systems(
                Update,
                lathe::retessellate_patch_meshes_on_quality_change
                    .after(lathe::regenerate_patch_meshes),
            )
            .add_systems(
                Update,
                retessellate_primitive_meshes_on_quality_change
                    .after(lathe::retessellate_patch_meshes_on_quality_change),
            )
            .add_systems(
                Update,
                refresh_curve_meshes_on_stage_or_quality_change
                    .after(retessellate_primitive_meshes_on_quality_change),
            )
            // `sync_usd_visuals` runs only on frames where a stage's
            // `LoadedWithDependencies` event was emitted. Idle frames
            // skip it entirely (run-condition short-circuits).
            .add_systems(
                Update,
                (
                    // Initial projection is served by the worker-produced plan.
                    // This system only rebuilds the !Send live stage after an
                    // authored asset modification, when incremental readers need
                    // the new document.
                    canonical::sync_canonical_stages.run_if(
                        bevy::ecs::schedule::common_conditions::on_message::<
                            AssetEvent<UsdStageAsset>,
                        >,
                    ),
                    sync_usd_visuals
                        .run_if(
                            bevy::ecs::schedule::common_conditions::on_message::<
                                AssetEvent<UsdStageAsset>,
                            >,
                        )
                        .after(canonical::sync_canonical_stages)
                        .in_set(UsdSceneSyncSet),
                    process_queued_usd_visuals
                        .run_if(any_queued_usd_visuals)
                        .after(sync_usd_visuals)
                        .in_set(UsdVisualProjectionSet),
                    poll_pending_usd_meshes
                        .run_if(any_pending_usd_meshes)
                        .after(process_queued_usd_visuals)
                        .in_set(UsdVisualProjectionSet),
                    resolve_point_instancer_meshes
                        .after(poll_pending_usd_meshes)
                        .in_set(UsdVisualProjectionSet),
                    hide_point_instancer_prototypes
                        .after(process_queued_usd_visuals)
                        .in_set(UsdVisualProjectionSet),
                    ensure_point_instancer_prototypes
                        .after(process_queued_usd_visuals)
                        .in_set(UsdVisualProjectionSet),
                    retry_awaiting_usd_visuals_after_quality_change
                        .run_if(resource_changed::<lunco_render::RenderingQualitySettings>)
                        // A quality update can coincide with the asset-loaded
                        // event that drains the same awaiting queue.  The
                        // loaded-stage projection is authoritative for that
                        // frame; let its deferred marker land before the
                        // quality retry observes the queue, otherwise one
                        // prim can be instantiated twice.
                        .after(sync_usd_visuals),
                    // The other half of the same queue: `sync_usd_visuals` drains
                    // prims whose stage arrived, this one drains prims whose stage
                    // never will. Both must exist or the queue has an outcome it
                    // cannot leave.
                    // Upgrades parked runtime-instance descendants to a
                    // hierarchical `Derived` id (gap G2/B.1) once their root id
                    // is allocated. Cheap: the query is empty unless a runtime
                    // spawn is mid-flight.
                    resolve_usd_instance_identities,
                ),
            );
    }
}

/// Main-thread handle for one asynchronous CPU-generated USD mesh build.
///
/// The task owns only Send-safe extracted data. The live OpenUSD stage remains
/// on the main thread and is never captured by the worker.
#[derive(Component)]
struct PendingUsdMesh {
    task: Task<Option<Mesh>>,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    path: SdfPath,
    stage_generation: u64,
    profile: lunco_render::RenderQualityProfile,
}

/// Marker: this prim's `xformOpOrder` begins with the `!resetXformStack!`
/// sentinel, so UsdGeomXformable defines its local-to-world as its OWN op stack
/// alone — the ancestor chain is not part of it.
///
/// Composition of the prim's local transform already honours the sentinel
/// ([`compose_xform_order_at`]); the marker is what carries that fact into the
/// ECS, where "world" is an accumulated `GlobalTransform` and the only way to
/// ignore ancestors is to stop being their descendant.
/// [`detach_reset_xform_stack_prims`] does the reparent.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct UsdResetXformStack;

/// Main-thread budget for USD structural projection.
///
/// Canonical OpenUSD stages are `!Send` and Bevy render assets are main-thread
/// resources, so projection cannot be moved wholesale to a worker. This
/// resource keeps the UI responsive while still admitting a complete scene in
/// a small number of frames. The budget is measured in wall-clock time because
/// prim projection cost is not uniform; a prim may still overshoot the budget
/// because its USD read and child scheduling are atomic. CPU geometry that can
/// be detached from the `!Send` stage is dispatched to the async compute pool
/// instead of extending this main-thread slice.
#[derive(Resource, Debug, Clone, Copy)]
pub struct UsdVisualProjectionSettings {
    /// Maximum time the projector may start work in one `Update` pass.
    ///
    /// A zero budget is invalid and is reported by the projector rather than
    /// silently changing the pacing contract.
    pub frame_budget: std::time::Duration,
}

impl Default for UsdVisualProjectionSettings {
    fn default() -> Self {
        Self {
            // Eight milliseconds leaves the rest of a 60 Hz frame for input,
            // simulation, and UI while avoiding a hundreds-of-frames load for
            // ordinary scenes.
            frame_budget: std::time::Duration::from_millis(8),
        }
    }
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

/// Select the identity scope for one projected USD prim.
///
/// Render-only previews and descendants of runtime-spawned instances are local
/// entities. Authored live-scene prims derive deterministic content identity
/// only when the asset server can provide their stable source path.
fn usd_projection_provenance(
    preview_only: bool,
    inherited_member: bool,
    source: Option<String>,
    resolved_path: &str,
) -> Option<lunco_core::Provenance> {
    if preview_only || inherited_member {
        Some(lunco_core::Provenance::Local)
    } else {
        source.map(|source| lunco_core::Provenance::Content {
            namespace: "usd".into(),
            source,
            path: resolved_path.into(),
        })
    }
}

/// Translates a single USD prim into Bevy/big_space/avian components on
/// `entity`. The caller has already verified that the stage is loaded.
///
/// **Steady-state cost: zero** — this is invoked exactly once per projection
/// generation, by `process_queued_usd_visuals` after the entity's queue marker
/// has been admitted. The `Add<UsdPrimPath>` observer and the loaded-stage event
/// only publish that marker; they do not read or instantiate USD.
///
/// 1. Looks up the prim's attributes through the composed reader selected by
///    the stage generation: the worker-produced plan initially, then the live
///    canonical stage after an authored change.
/// 2. Creates a mesh based on prim type (Cube, Cylinder, Sphere), or schedules
///    detached CPU geometry through the async mesh phase. Procedural camera
///    backgrounds are the explicit exception: they are render intents only and
///    never create or schedule a mesh.
/// 3. Applies the prim's transform (position + rotation + scale).
/// 4. Spawns each prim child below its USD parent. A top-level child of the
///    nested scene Grid carries its own `CellCoord`; deeper descendants remain
///    ordinary children rooted in the prim's low-precision subtree.
/// 5. Marks the entity with `UsdSceneProjected` to prevent re-processing.
///
/// Custom materials (solar panels, blueprint grids, etc.) are applied
/// by independent material plugins in `lunco-materials` that observe
/// the `UsdSceneProjected` insertion.
#[allow(clippy::too_many_arguments)]
fn instantiate_usd_prim(
    entity: Entity,
    prim_path: &UsdPrimPath,
    existing_vis: Option<&Visibility>,
    existing_tf: Option<&Transform>,
    is_instance_root: bool,
    inherited_member: Option<&UsdInstanceMember>,
    instance_projection: Option<&UsdInstanceProjection>,
    is_high_precision_parent: bool,
    parent_is_grid: bool,
    is_grid_entity: bool,
    preview_only: bool,
    commands: &mut Commands,
    stages: &Assets<UsdStageAsset>,
    canonical: &CanonicalStages,
    asset_server: &AssetServer,
    meshes: &mut Assets<Mesh>,
    quality: lunco_render::RenderQualityProfile,
) {
    let id = prim_path.stage_handle.id();
    let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
        warn!(
            "[usd-bevy] no stage asset for {} — skipping visual instantiate",
            prim_path.path
        );
        return;
    };
    let (reader, stage_generation) =
        canonical.reader_for_entity(id, stage_asset, instance_projection);
    instantiate_usd_prim_from_reader(
        &reader,
        entity,
        prim_path,
        existing_vis,
        existing_tf,
        is_instance_root,
        inherited_member,
        instance_projection,
        is_high_precision_parent,
        parent_is_grid,
        is_grid_entity,
        preview_only,
        commands,
        asset_server,
        meshes,
        quality,
        stage_generation,
    );
}

/// The visual extractor body over a composed [`UsdRead`] source.
/// Initial scene materialisation uses the worker-produced
/// [`UsdStageProjectionPlan`]; the canonical [`StageView`] is used explicitly
/// for later live edits. It maps one composed USD prim to its Bevy visual
/// components (mesh, material, light, camera, transform, and authored markers).
#[allow(clippy::too_many_arguments)]
fn instantiate_usd_prim_from_reader<R: UsdRead>(
    reader: &R,
    entity: Entity,
    prim_path: &UsdPrimPath,
    existing_vis: Option<&Visibility>,
    existing_tf: Option<&Transform>,
    is_instance_root: bool,
    inherited_member: Option<&UsdInstanceMember>,
    instance_projection: Option<&UsdInstanceProjection>,
    is_high_precision_parent: bool,
    parent_is_grid: bool,
    is_grid_entity: bool,
    preview_only: bool,
    commands: &mut Commands,
    asset_server: &AssetServer,
    meshes: &mut Assets<Mesh>,
    quality: lunco_render::RenderQualityProfile,
    stage_generation: u64,
) {
    let convention = match stage_convention(reader) {
        Ok(convention) => convention,
        Err(error) => {
            error!(
                "[usd-bevy] stage has invalid convention metadata: {error}; refusing visual projection"
            );
            commands.entity(entity).try_insert((
                UsdSceneProjectionFailed(error.to_string()),
                Visibility::Hidden,
            ));
            return;
        }
    };
    {
        // Resolve the empty scene-root sentinel against the live composed
        // stage, then publish the concrete path for every downstream projector.
        let resolved_path = match resolve_stage_prim_path(reader, &prim_path.path) {
            Some(path) => {
                if prim_path.path.is_empty() {
                    // `try_insert` (not `.insert`): one of these prims may have been
                    // despawned between sync's iterate (above) and ApplyDeferred — the
                    // moonbase autoload vs first-run tutorial race is the canonical case.
                    // See `sync_usd_visuals`'s panic-safe note below.
                    commands.entity(entity).try_insert(UsdPrimPath {
                        stage_handle: prim_path.stage_handle.clone(),
                        path: path.clone(),
                    });
                }
                path
            }
            None => {
                let message = format!(
                    "stage for {} has no `defaultPrim`; visual projection refused",
                    prim_path.stage_handle.id()
                );
                error!("[usd] {message}");
                commands.entity(entity).try_insert((
                    UsdSceneProjectionFailed(message.clone()),
                    Visibility::Hidden,
                ));
                lunco_core::trigger_error(commands, "usd-visual-sync-failed", message);
                return;
            }
        };
        let Ok(sdf_path) = SdfPath::new(&resolved_path) else {
            return;
        };
        project_usd_prim_kind(reader, &sdf_path, entity, commands);
        project_spawnable_selectable(reader, &sdf_path, entity, commands);

        // M1 identity (Ph1). Three projection scopes:
        //
        //  * **Presentation preview** (`preview_only`): a render-only view may
        //    project the same stage as the live scene, so it must never acquire
        //    the authored scene's deterministic identity. Local provenance is
        //    the core identity contract for non-networked presentation state.
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
        //    (the root carries `SkipContentStamp` → authoritative id). A stage
        //    with no stable source path receives no content provenance and
        //    therefore no derived identity.
        let source = asset_server
            .get_path(prim_path.stage_handle.id())
            .map(|source| source.path().to_string_lossy().into_owned());
        if let Some(provenance) = usd_projection_provenance(
            preview_only,
            inherited_member.is_some(),
            source,
            &resolved_path,
        ) {
            commands.entity(entity).try_insert(provenance);
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
        if !reader.is_active(&sdf_path) {
            commands
                .entity(entity)
                .try_insert((UsdSceneProjected, Visibility::Hidden));
            return;
        }

        // Get prim type (Cube, Cylinder, Sphere, etc.)
        let prim_type = reader.type_name(&sdf_path);

        // `UsdGeomPointInstancer` is an aggregate, not a Gprim. Its required
        // arrays and prototype relationship are decoded by the shared USD
        // reader, then projected into render-free instance children. The
        // children receive their prototype mesh/material handles after the
        // normal prototype prim has finished loading, which lets Bevy's
        // automatic instancing batch equal copies without changing authored
        // prototype-local coordinates.
        if prim_type.as_deref() == Some("PointInstancer") {
            if let Some(attribute) = point_instancer_array_time_samples(reader, &sdf_path) {
                let message = format!(
                    "{} has time-sampled {attribute}; animated PointInstancer arrays are not yet supported by the visual projection",
                    sdf_path.as_str()
                );
                error!("[usd-bevy] {message}");
                commands.entity(entity).try_insert((
                    UsdSceneProjectionFailed(message.clone()),
                    Visibility::Hidden,
                ));
                lunco_core::trigger_error(commands, "usd-visual-sync-failed", message);
                return;
            }
            let instances = match read_point_instancer(reader, &sdf_path, 0.0) {
                Ok(instances) => instances,
                Err(error) => {
                    let message = format!(
                        "{} has malformed UsdGeomPointInstancer data: {error}",
                        sdf_path.as_str()
                    );
                    error!("[usd-bevy] {message}");
                    commands.entity(entity).try_insert((
                        UsdSceneProjectionFailed(message.clone()),
                        Visibility::Hidden,
                    ));
                    lunco_core::trigger_error(commands, "usd-visual-sync-failed", message);
                    return;
                }
            };
            if let Err(error) = project_point_instancer(
                reader,
                entity,
                &sdf_path,
                &prim_path.stage_handle,
                instances,
                commands,
            ) {
                let message = format!(
                    "{} cannot project its UsdGeomPointInstancer prototypes: {error}",
                    sdf_path.as_str()
                );
                error!("[usd-bevy] {message}");
                commands.entity(entity).try_insert((
                    UsdSceneProjectionFailed(message.clone()),
                    Visibility::Hidden,
                ));
                lunco_core::trigger_error(commands, "usd-visual-sync-failed", message);
                return;
            }
        }

        // A procedural camera background is an Xform-level appearance intent,
        // not a USD gprim. Read the authored contract once at the USD
        // projection boundary and let the existing render-free marker carry it
        // to the shader binder. Geometry dispatch below is therefore never
        // entered for the background owner.
        let procedural_skybox =
            match read_authored_bool_strict(reader, &sdf_path, "lunco:surface:skybox") {
                Ok(value) => value.unwrap_or(false),
                Err(error) => {
                    let message = format!(
                        "{} has malformed authored attribute `lunco:surface:skybox`: {error}",
                        sdf_path.as_str()
                    );
                    error!("[usd-bevy] {message}");
                    commands.entity(entity).try_insert((
                        UsdSceneProjectionFailed(message.clone()),
                        Visibility::Hidden,
                    ));
                    lunco_core::trigger_error(commands, "usd-visual-sync-failed", message);
                    return;
                }
            };
        if procedural_skybox && prim_type.as_deref() != Some("Xform") {
            let message = format!(
                "{} authors `lunco:surface:skybox` on `{}`; the intent must be on an Xform",
                sdf_path.as_str(),
                prim_type.as_deref().unwrap_or("untyped prim")
            );
            error!("[usd-bevy] {message}");
            commands.entity(entity).try_insert((
                UsdSceneProjectionFailed(message.clone()),
                Visibility::Hidden,
            ));
            lunco_core::trigger_error(commands, "usd-visual-sync-failed", message);
            return;
        }
        if procedural_skybox {
            commands.entity(entity).try_insert(ProceduralSkybox);
        } else {
            commands.entity(entity).try_remove::<ProceduralSkybox>();
        }

        // UsdLux light prims (`DistantLight` sun / `DomeLight` sky — see
        // `light.rs`, and `dome.rs` for a DomeLight that carries an HDRI). A
        // light produces no mesh; the shared transform path below still
        // applies, which is how a DistantLight gets its orientation from
        // `xformOp:rotateXYZ` — and how a DomeLight gets the rotation that
        // spins its environment.
        light::instantiate_light_prim(
            reader,
            &sdf_path,
            prim_type.as_deref(),
            commands,
            entity,
            asset_server,
            prim_path.stage_handle.id(),
            quality,
            if preview_only {
                light::LightProjectionScope::Preview
            } else {
                light::LightProjectionScope::Scene
            },
        );

        // UsdGeomCamera (`def Camera`) → camera intent (see `camera.rs`). The
        // render binding turns viewport intent into an inactive Bevy `Camera3d`
        // with a complete render graph; which one renders is Bevy's
        // `Camera::is_active`, chosen by the switch mechanism in `lunco-avatar`.
        // A camera nested under a moving prim rides it via the shared transform
        // path below + `ChildOf` propagation ("camera on a rover").
        // Browser previews use their dedicated off-screen camera. Translating
        // an authored preview camera here would register a live SceneCamera
        // and let the avatar arbiter switch the main window to it on reload.
        if !preview_only {
            lunco_usd_bevy_camera::camera::instantiate_camera_prim(
                reader,
                &sdf_path,
                prim_type.as_deref(),
                commands,
                entity,
                quality,
            );
        }

        // Horizon-map terrain self-shadowing (consumed by
        // `lunco-environment`'s horizon system). Authors opt a terrain prim
        // in with `custom bool lunco:terrain:horizonShadows = true`; the
        // bake grid is tunable via `int lunco:terrain:horizonMapResolution`.
        //
        // `lunco:terrain:horizonMapAzimuths` is deliberately NOT read: the
        // shadow path ray-marches a heightfield and has no azimuth slices.
        // It was parsed into a field nothing ever read — see
        // `lunco_core::HorizonShadowTerrain`.
        if reader
            .boolean(&sdf_path, "lunco:terrain:horizonShadows")
            .unwrap_or(false)
        {
            let mut cfg = lunco_core::HorizonShadowTerrain::default();
            let valid_resolution = match reader
                .scalar::<i32>(&sdf_path, "lunco:terrain:horizonMapResolution")
            {
                Some(0) => true,
                Some(r) if (2..=4096).contains(&r) => {
                    cfg.resolution = r as u32;
                    true
                }
                Some(r) => {
                    error!(
                        "[usd-bevy] {} has invalid horizonMapResolution = {r}; expected 0 or an integer in [2, 4096]",
                        sdf_path.as_str()
                    );
                    false
                }
                None if reader
                    .has_authored_attribute(&sdf_path, "lunco:terrain:horizonMapResolution") =>
                {
                    error!(
                        "[usd-bevy] {} has authored horizonMapResolution with an unsupported value type",
                        sdf_path.as_str()
                    );
                    false
                }
                None => true,
            };
            if valid_resolution {
                commands.entity(entity).try_insert(cfg);
            }
        }

        // Visibility — honour standard USD `token visibility`.
        // `invisible` suppresses mesh creation entirely (used for
        // collider-only Cube prims hidden behind a glTF visual, and
        // raycast wheel cylinders that have no visible representation).
        //
        // `visibility` is INHERITED, exactly like `purpose` below —
        // `UsdGeomImageable` defines it as such, and the nearest ancestor that
        // authors `invisible` hides the whole subtree. Reading only the prim's
        // own path (as this did) means marking one `Xform` invisible leaves every
        // child drawing, which is both wrong per spec and a surprising way to
        // lose an afternoon: HAB-1's collider ring stayed on screen because the
        // `invisible` sat on the group rather than on each box.
        //
        // Visibility PRUNES: an `invisible` ancestor hides the subtree outright,
        // and a descendant cannot re-reveal itself. `inherited` means "take the
        // parent's answer", NOT "force visible" — so it keeps walking rather
        // than stopping. Getting that backwards would let a child override a
        // hidden parent, which USD does not permit.
        let invisible = reader.is_invisible_or_guide(&sdf_path);
        // `UsdGeomImageable.purpose = "guide"` — geometry that exists for authoring,
        // not for viewing: construction axes, alignment rigs, and (the HAB-1 case)
        // the boolean CUTTERS that define a shell's openings. Keeping them in the
        // file is what preserves the parametric intent — a porthole stays a
        // diameter and a position rather than becoming a hole someone has to
        // reverse-engineer — but a guide must never render.
        //
        // The inherited Imageable visibility/purpose result is resolved once by
        // the reader. Prepared readers compute it on the loader worker through
        // the same OpenUSD schema implementation.

        // Placeholder for an async-loading glTF payload. Authors set
        // `bool lunco:placeholder = true` on a Cube prim that lives as
        // a sibling of an `Xform "Visual" (payload = @lunco://...@)`.
        // Third-party USD tools render it (they don't know our
        // attribute or the `lunco://` scheme); our pipeline starts
        // it `Visibility::Hidden` so the user doesn't see a brief
        // tan-cube flash before the photoreal glTF replaces it. Mesh
        // is still built — visibility is the toggle. (Future: reveal
        // on `AssetServer::load_state(...).is_failed()`.)
        let is_placeholder = reader
            .boolean(&sdf_path, "lunco:placeholder")
            .unwrap_or(false);

        // **Placeholder + payload pattern**: when a binary payload/reference
        // is present, we still build the primitive Cube/Sphere/Cylinder
        // mesh so the prim has a fallback visual until the glTF Scene
        // finishes loading. Once Bevy reports the Scene asset loaded,
        // the diagnostics plugin hides the primitive
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
        // A Cube reads `size` and NOTHING else — `width`/`height`/`depth`
        // are not accepted on it (they are UsdGeomPlane's attributes, not
        // UsdGeomCube's). Non-uniform dimensions go through `xformOp:scale`.
        // Shape dimensions (+ their USD schema defaults) come from the
        // canonical `read_shape_dims` so the visual mesh and the avian
        // collider can't desync. Mesh-quality parameters come from the
        // Graphics profile — they're rendering-only and don't affect physics.
        let primitive_shape = if !invisible
            && !procedural_skybox
            && !matches!(
                prim_type.as_deref(),
                Some("Mesh") | Some("NurbsPatch") | Some("BasisCurves") | Some("NurbsCurves")
            ) {
            prim_type
                .as_deref()
                .and_then(|ty| read_shape_dims(reader, &sdf_path, ty))
        } else {
            None
        };
        let mut mesh_pending = false;
        let mesh_handle: Option<Handle<Mesh>> = if invisible || procedural_skybox {
            None
        } else if prim_type.as_deref() == Some("Mesh") {
            // Native UsdGeomMesh: decode points/faceVertexIndices/normals/st
            // into a Bevy mesh. (Falls through to `None` — no fallback
            // primitive — if the topology attrs are missing/malformed.)
            build_usd_mesh(reader, &sdf_path).map(|m| meshes.add(m))
        } else if prim_type.as_deref() == Some("NurbsPatch") {
            // Tensor-product rational surface — how USD spells a lathe, and the
            // only way to express a PARTIAL revolution (the gprims are complete
            // revolutions with no sweep-angle). `trimCurve:*` IS honoured; see the
            // fn doc.
            //
            // The patch's DEFINITION is retained on the entity rather than being
            // consumed and thrown away, which is what makes the surface editable at
            // all — `NurbsSurface` and `UsdLathe` are reflected components, so the
            // existing scripting bridge writes them with no new verb, and
            // `lunco-usd-bevy-lathe`'s `Changed`-filtered systems rebuild the mesh once per
            // edit instead of once per frame.
            if !has_authored_nurbs_trim(reader, &sdf_path) {
                if let Some((surface, lathe_params)) = read_patch_surface(reader, &sdf_path) {
                    let task_surface = surface.clone();
                    let task = AsyncComputeTaskPool::get()
                        .spawn(async move { task_surface.mesh(quality) });
                    let mut e = commands.entity(entity);
                    // Publish the definition before the mesh arrives. The
                    // change-filtered lathe systems then retain their normal
                    // editable state, but have no Mesh3d yet and cannot repeat
                    // the worker's tessellation on the load frame.
                    e.try_insert((
                        surface,
                        PendingUsdMesh {
                            task,
                            stage_id: prim_path.stage_handle.id(),
                            path: sdf_path.clone(),
                            stage_generation,
                            profile: quality,
                        },
                        UsdSceneGeometryPending,
                    ));
                    mesh_pending = true;
                    if let Some(l) = lathe_params {
                        e.try_insert(l);
                    }
                }
                None
            } else {
                build_usd_nurbs_patch_mesh(reader, &sdf_path, quality).map(|(mesh, def)| {
                    if let Some((surface, lathe_params)) = def {
                        let mut e = commands.entity(entity);
                        e.try_insert(surface);
                        if let Some(l) = lathe_params {
                            e.try_insert(l);
                        }
                    }
                    meshes.add(mesh)
                })
            }
        } else if matches!(
            prim_type.as_deref(),
            Some("BasisCurves") | Some("NurbsCurves")
        ) {
            // A curve prim with `widths` is a renderable surface: an unoriented
            // curve is a swept tube, while a curve with standard normals is a
            // flat ribbon. A pure path (for example a camera rail carrying
            // `lunco:path:camera`) authors no `widths` and remains non-rendered.
            build_usd_curve_mesh(reader, &sdf_path, quality).map(|m| {
                commands.entity(entity).try_insert(UsdCurveMesh);
                meshes.add(m)
            })
        } else {
            match primitive_shape {
                // `xformOp:scale` handles non-uniform dimensions (applied to the
                // Transform below) — that is how UsdGeomCube spells a box.
                Some(shape) => {
                    let task = AsyncComputeTaskPool::get()
                        .spawn(async move { build_primitive_mesh(shape, quality) });
                    commands.entity(entity).try_insert((
                        PendingUsdMesh {
                            task,
                            stage_id: prim_path.stage_handle.id(),
                            path: sdf_path.clone(),
                            stage_generation,
                            profile: quality,
                        },
                        UsdSceneGeometryPending,
                    ));
                    mesh_pending = true;
                    None
                }
                None => None,
            }
        };

        if let Some(shape) = primitive_shape {
            commands.entity(entity).try_insert(UsdPrimitiveMesh(shape));
        } else if mesh_handle.is_none() && !mesh_pending {
            commands
                .entity(entity)
                .remove::<Mesh3d>()
                .remove::<UsdPrimitiveMesh>()
                .remove::<UsdCurveMesh>()
                .remove::<PendingUsdMesh>()
                .remove::<UsdSceneGeometryPending>();
        }

        // Author the PBR appearance intent (`PbrLook`) with the USD
        // colour/textures. The intent is independent of mesh readiness, so a
        // worker-built mesh receives its authored appearance before the task
        // completes.
        let material_result = if let Some(ref m) = mesh_handle {
            apply_standard_material(
                reader,
                &sdf_path,
                m,
                &mut commands.entity(entity),
                asset_server,
                prim_path.stage_handle.id(),
            )
        } else if mesh_pending || is_procedural_terrain_visual_owner(reader, &sdf_path) {
            // DEM terrain is projected as an Xform and receives its mesh later from
            // lunco-terrain-surface. Preserve the authored USD appearance contract
            // across that asynchronous geometry boundary; the terrain assembler is
            // render-free and must not choose a second material itself.
            apply_standard_material_intent(
                reader,
                &sdf_path,
                &mut commands.entity(entity),
                asset_server,
                prim_path.stage_handle.id(),
            )
        } else {
            Ok(())
        };
        if let Err(err) = material_result {
            eprintln!(
                "[usd-bevy test-diagnostic] {} has malformed authored material attribute `{}`",
                sdf_path.as_str(),
                err.attribute
            );
            error!(
                "[usd-bevy] {} has malformed authored material attribute `{}`; no PbrLook was created",
                sdf_path.as_str(),
                err.attribute
            );
        }

        // Scripts are `LunCoProgramAPI` CHILD prims whose source is a `.rhai` — read
        // from here, the owner, because a script acts on behalf of the thing that
        // carries it: `me` is the vessel, not the program prim. The program prim is
        // what makes the binding composable (it arrives on a `references` arc and can
        // be deleted to take the behaviour away), and what gives the script its own
        // typed parameters, which live on it rather than on the owner.
        //
        // A program with a `.mo` source is owned by lunco-usd-sim; generic Rhai
        // programs are the policy surface projected here.
        // Preview stages are inert presentations, including generic programs
        // attached by this visual projection rather than the physics projector.
        if !preview_only {
            attach_programs(
                reader,
                &sdf_path,
                entity,
                prim_path.stage_handle.id(),
                commands,
            );
        }

        // There is deliberately NO "possessable" tag read here. The generic command
        // surface is authored by `Controls` and projected as `InputPorts`; the avatar
        // domain owns the semantic vessel boundary and rejects its own `Avatar`
        // endpoint before authority arbitration. What a non-avatar endpoint can do is
        // still decided by its authored capability — no vehicle-class branch belongs
        // in this translator.

        // `ui:displayName` — the STANDARD UsdUI attribute for a prim's human
        // name (SceneGraphPrimAPI), the field every DCC shows in its outliner.
        // Deliberately not a `lunco:*` invention: USD already has a word for
        // "what this thing is called", so we read that word. Ingested as
        // [`Callsign`]; driver-facing UI (HUD title) prefers it over `Name`,
        // which carries the prim PATH and reads as plumbing on camera. Read on
        // ANY prim — habitats and trailers deserve names too.
        if let Some(display) = reader.text(&sdf_path, "ui:displayName") {
            let trimmed = display.trim();
            if !trimmed.is_empty() {
                commands
                    .entity(entity)
                    .try_insert(lunco_core::markers::Callsign(trimmed.to_string()));
            }
        }

        // Per-vessel intent→port control map (stage 2 of control), authored as a
        // `Controls` child scope: each child prim's NAME is the intent, with
        // `string lunco:port` + `double lunco:factor`. Authored inline OR pulled in
        // from a shared profile class (`inherits = </_RoverControl>`); either way
        // it's already composed into this live stage. When absent, no keyboard
        // adapter is attached; direct named-port writes and authored programs
        // can still operate only on explicitly projected surfaces.
        if let Some(controls) = reader
            .children(&sdf_path)
            .into_iter()
            .find(|c| c.name() == Some("Controls"))
        {
            let entries: Vec<(String, String, f64)> = reader
                .children(&controls)
                .into_iter()
                .filter_map(|bind| {
                    let intent = bind.name()?.to_string();
                    let port = reader.scalar::<String>(&bind, "lunco:port")?;
                    let factor = reader.real(&bind, "lunco:factor")?;
                    Some((intent, port, factor))
                })
                .collect();
            if let Some(binding) = lunco_core::ControlBinding::from_intent_entries(&entries) {
                // Preserve authored `inputs:<port>` constants on the command
                // surface. The binding declares which names are writable; USD
                // remains the source of their initial state. Omitted inputs
                // use the semantic zero default.
                let inputs = lunco_core::InputPorts::with_defaults(binding.ports().map(|port| {
                    let value = reader
                        .real(&sdf_path, &format!("inputs:{port}"))
                        .unwrap_or(0.0);
                    (port.to_string(), value)
                }));
                // `InputPorts` rides along with the binding: the binding DECLARES
                // the accepted input ports, while the composed USD inputs provide
                // their initial values. The vocabulary is never a Rust literal.
                // (A rover also gets one at its `PhysxVehicleContextAPI` branch;
                // `try_insert` order is irrelevant because seeding is additive and
                // idempotent.)
                commands.entity(entity).try_insert((binding, inputs));
            }

            // Camera-follow mode is a property of how the vehicle moves, so it is
            // authored on the same control profile as the intent→port binding
            // (`uniform token lunco:cameraFollow` on the referenced profile, which
            // flattens onto this `Controls` prim). It answers "should the camera
            // rotate with the body?" — `heading` (yaw, surface vehicles), `orbit`
            // (stable frame for a 6-DOF flyer), `chase` (full attitude). Read here
            // and consumed by `on_possess_command`; absent → the `Heading` default.
            if let Some(mode) = reader
                .text(&controls, "lunco:cameraFollow")
                .and_then(|t| lunco_core::parse_camera_follow(&t))
            {
                commands.entity(entity).try_insert(mode);
            }
        }

        project_catalog_entry_id(reader, &sdf_path, entity, commands);

        // glTF / external-mesh branch.
        //
        // Read the authored binary `payload`/`references` directly from the
        // live composed prim stack. The pure-Rust USD resolver composes those
        // arcs through an empty stub, while this render projection hands the
        // canonical URI to Bevy's `AssetServer` — the registered asset sources
        // (`lunco://` for library assets, `twin://` for Twin-local ones,
        // default `assets://` for
        // in-tree paths) handle the lookup.
        //
        // - `lunco:assetMode = "mesh"` (default `"scene"`): pull a
        //   single primitive out of the glTF and attach as `Mesh3d`.
        //   Used when the prim should also drive a physics collider —
        //   stays compatible with `lunco-usd-avian` mesh-collider
        //   pipelines.
        // - `lunco:assetMode = "scene"`: load the full glTF scene and
        //   attach as a `WorldAssetRoot` child. Preserves hierarchy,
        //   materials, and lights at the cost of being opaque to the
        //   USD prim-path tree.
        if let Some(asset_uri) = reader.binary_asset_uri(&sdf_path) {
            let mode = reader
                .text(&sdf_path, "lunco:assetMode")
                .unwrap_or_else(|| "scene".to_string());
            let label = reader.text(&sdf_path, "lunco:assetLabel");

            match mode.as_str() {
                "mesh" => {
                    let label = label.unwrap_or_else(|| "Mesh0/Primitive0".to_string());
                    let path = format!("{asset_uri}#{label}");
                    let mesh_h: Handle<Mesh> = asset_server.load(&path);
                    // Single-mesh path keeps `lunco-usd-avian` collider
                    // construction unchanged — the entity ends up with
                    // a `Mesh3d` exactly like the Cube/Sphere branches.
                    if let Err(err) = apply_standard_material(
                        reader,
                        &sdf_path,
                        &mesh_h,
                        &mut commands.entity(entity),
                        asset_server,
                        prim_path.stage_handle.id(),
                    ) {
                        error!(
                            "[usd-bevy] {} has malformed authored material attribute `{}`; no PbrLook was created",
                            sdf_path.as_str(),
                            err.attribute
                        );
                    }
                }
                _ => {
                    let label = label.unwrap_or_else(|| "Scene0".to_string());
                    let path = format!("{asset_uri}#{label}");
                    let scene_h: Handle<WorldAsset> = asset_server.load(&path);
                    // Mark the entity so the diagnostics plugin can drop the
                    // placeholder Mesh3d once this Scene
                    // finishes loading. The marker is harmless if the
                    // entity has no Mesh3d (e.g. `def Xform` without a
                    // primitive fallback).
                    commands
                        .entity(entity)
                        .try_insert(WorldAssetRoot(scene_h))
                        .try_insert(GlbPlaceholder)
                        .try_insert(PlaceholderAssetUri(path));
                }
            }
        }

        // Full local transform: the authoritative USD `xformOpOrder` stack. An
        // omitted stack is the USD identity and preserves the code-set spawn
        // pose; malformed authored data rejects this prim instead of being guessed.
        let usd_tf = match local_transform_at(reader, &sdf_path, 0.0) {
            Ok(transform) => transform,
            Err(error) => {
                error!(
                    "[usd-bevy] {} has malformed authored transform; visual projection rejected: {}",
                    sdf_path.as_str(),
                    error
                );
                commands.entity(entity).try_insert((
                    UsdSceneProjectionFailed(error.to_string()),
                    Visibility::Hidden,
                ));
                return;
            }
        };
        // An authored stack owns the complete pose, including zero translation,
        // identity rotation and authored scale. Apply the primitive axis exactly once
        // to that pose, not to the previous visual projection.
        let mut transform = usd_tf.unwrap_or_else(|| existing_tf.cloned().unwrap_or_default());
        // UsdGeomCylinder.axis token (X|Y|Z, default Z). Compose the
        // axis-induced rotation onto the entity Transform so a Y-axis
        // Bevy `Cylinder` mesh appears along the authored axis without
        // an explicit `xformOp:rotateXYZ` hack. Goes after rotateXYZ so
        // it applies on top of any user-authored rotation.
        if matches!(
            prim_type.as_deref(),
            Some("Cylinder" | "Cone" | "Capsule" | "Plane")
        ) {
            if let Some(axis) = prim_type
                .as_deref()
                .and_then(|type_name| read_primitive_axis(reader, &sdf_path, type_name))
            {
                // The `axis` token names an axis of the STAGE's frame, while the Bevy
                // primitive is generated in the canonical one — so the axis rotation is
                // pre-rotated by the stage convention (`Q·q_axis`). On a Z-up stage an
                // `axis = "Z"` cylinder therefore stands up along canonical +Y, as it
                // did along the stage's +Z. Identity on a Y-up stage.
                let q_axis = convention.orient(usd_axis_to_quat(&axis).unwrap_or(Quat::IDENTITY));
                if !q_axis.abs_diff_eq(Quat::IDENTITY, 1e-6) {
                    transform.rotation *= q_axis;
                }
                debug!(
                    "[usd-bevy] {} {} axis={} rot={:?}",
                    sdf_path.as_str(),
                    prim_type.as_deref().unwrap_or(""),
                    axis,
                    transform.rotation
                );
            }
        }
        // UsdGeomCamera aim by target point: when a `def Camera` authors
        // `lunco:cameraLookAt` (double3, in the camera's PARENT-local space),
        // orient it to look from its `xformOp:translate` toward that point.
        // The ergonomic way to point a scene/cutscene camera at an object —
        // move either the camera or the object and the aim stays correct.
        // Overrides any authored rotation and produces a standard rotation
        // (same convenience the avatar camera has, but pure `Transform`).
        // Parent-local on both sides, so a camera nested under a rover aims in
        // rover-local space and the aim rides the rover.
        if prim_type.as_deref() == Some("Camera") {
            if let Some([tx, ty, tz]) = read_vec3_f64(reader, &sdf_path, "lunco:cameraLookAt") {
                // A point in the camera's PARENT-local (stage-frame) space →
                // canonical, exactly like every other authored point.
                let target = convention.point(Vec3::new(tx as f32, ty as f32, tz as f32));
                let eye = transform.translation;
                if (target - eye).length_squared() > 1e-6 {
                    transform.rotation = Transform::from_translation(eye)
                        .looking_at(target, Vec3::Y)
                        .rotation;
                }
            }
        }
        // Honour `token visibility = "invisible"` and the
        // `lunco:placeholder = true` author flag — both apply as
        // `Visibility::Hidden`.
        //
        // `invisible` is the ANCESTOR-RESOLVED answer (see where it is computed),
        // and that is load-bearing rather than redundant with Bevy's propagation.
        // Bevy lets a child holding `Visibility::Visible` override a hidden parent;
        // USD does not — an `invisible` ancestor PRUNES the subtree and no
        // descendant can re-reveal itself. Because every descendant re-walks and
        // reaches `Hidden` on its own, USD's rule holds no matter what Bevy's
        // propagation would have done.
        let final_vis = if invisible || is_placeholder {
            Visibility::Hidden
        } else {
            existing_vis.cloned().unwrap_or(Visibility::Inherited)
        };

        if parent_is_grid {
            commands.entity(entity).try_insert(CellCoord::default());
        }
        commands.entity(entity).try_insert((
            transform,
            UsdSceneProjected,
            final_vis,
            InheritedVisibility::default(),
            ViewVisibility::default(),
        ));

        // Tag entities carrying ANY animated channel (xform, visibility, or a
        // bound-shader / displayColor material input) so the animation adapter's
        // per-frame samplers drive them (doc 19). The query stays empty for static scenes.
        // `lunco_usd_bevy_animation::bind_animated_to_preview` then binds the tagged entity to the
        // animation-preview domain so the transport (play/pause/scrub/rate) reaches it.
        if prim_is_animated(reader, &sdf_path) {
            commands.entity(entity).try_insert(UsdAnimated);
        }

        // UsdGeomXformable's `!resetXformStack!`: composition above already
        // yielded the sentinel-honouring LOCAL transform, but "ignores its
        // ancestors" is a statement about parentage, which only
        // `detach_reset_xform_stack_prims` can act on. Tag now, while the reader
        // is in hand — the ancestry it needs may not exist yet this frame.
        if prim_resets_xform_stack(reader, &sdf_path) {
            commands.entity(entity).try_insert(UsdResetXformStack);
        }

        // Tag a prim authoring `lunco:activeCamera` timeSamples as an editorial
        // camera track (doc 35): its keys drive `SetActiveCamera` cuts over time.
        // `bind_camera_tracks_to_preview` then binds it to the animation-preview
        // domain so the transport scrubs the cuts.
        if lunco_usd_bevy_camera::camera_track::prim_is_camera_track(reader, &sdf_path) {
            commands
                .entity(entity)
                .try_insert(lunco_usd_bevy_camera::camera_track::CameraTrack);
        }

        // Commit the direct-child portion of the hierarchy plan prepared by
        // the asset loader. Each child is itself admitted to the projection
        // queue and commits its own children when its turn arrives; walking
        // the whole subtree here would enqueue the same descendants once per
        // ancestor.
        commit_usd_children(
            entity,
            &prim_path.stage_handle,
            reader,
            &sdf_path,
            &child_member,
            instance_projection,
            is_high_precision_parent,
            is_grid_entity,
            commands,
        );
    }
}

/// The initial visual projection is deliberately static. Reject time-sampled
/// PointInstancer arrays instead of sampling their default/initial value and
/// silently freezing a standards-valid animated asset.
fn point_instancer_array_time_samples<R: UsdRead>(
    reader: &R,
    path: &SdfPath,
) -> Option<&'static str> {
    [
        "positions",
        "protoIndices",
        "orientations",
        "orientationsf",
        "scales",
        "ids",
        "invisibleIds",
    ]
    .into_iter()
    .find(|attribute| reader.has_time_samples(path, attribute))
}

/// Project a standard point instancer into render-free children.
///
/// The first production renderer supported by this crate can share one mesh
/// and one material across direct-Gprim prototypes. A prototype subtree is
/// valid OpenUSD, but needs a flattened multi-mesh render batch and is rejected
/// here with an explicit projection failure until that renderer boundary is
/// implemented. This keeps unsupported authored structure visible instead of
/// silently drawing only part of a prototype.
fn project_point_instancer<R: UsdRead>(
    reader: &R,
    parent: Entity,
    path: &SdfPath,
    stage_handle: &Handle<UsdStageAsset>,
    instances: Vec<lunco_usd_bevy_core::point_instancer::UsdPointInstancePlan>,
    commands: &mut Commands,
) -> Result<(), String> {
    let prototype_paths = reader
        .rel_targets(path, "prototypes")
        .into_iter()
        .map(|prototype| prototype.to_string())
        .collect::<Vec<_>>();
    let convention = stage_convention(reader as &dyn UsdReadObject)
        .map_err(|error| format!("invalid stage convention: {error}"))?;

    for prototype in &prototype_paths {
        let prototype_path = SdfPath::new(prototype)
            .map_err(|error| format!("invalid prototype target {prototype}: {error}"))?;
        let prototype_type = reader.type_name(&prototype_path).unwrap_or_default();
        if !matches!(
            prototype_type.as_str(),
            "Mesh"
                | "Cube"
                | "Sphere"
                | "Cylinder"
                | "Cone"
                | "Capsule"
                | "Plane"
                | "NurbsPatch"
                | "BasisCurves"
                | "NurbsCurves"
        ) {
            return Err(format!(
                "prototype {prototype} has type `{prototype_type}`; only direct renderable Gprims are currently supported"
            ));
        }
        if prim_is_animated(reader, &prototype_path) {
            return Err(format!(
                "prototype {prototype} is animated; animated PointInstancer prototypes are not yet supported by the visual projection"
            ));
        }
        if let Some(child) = reader.children(&prototype_path).into_iter().next() {
            return Err(format!(
                "prototype {prototype} has child {child}; arbitrary prototype subtrees require a multi-mesh instancing batch"
            ));
        }
    }

    commands.entity(parent).try_insert(UsdPointInstancer {
        stage_handle: stage_handle.clone(),
        prototype_paths,
    });
    for instance in instances {
        let prototype_path = SdfPath::new(&instance.prototype_path).map_err(|error| {
            format!(
                "invalid prototype path {}: {error}",
                instance.prototype_path
            )
        })?;
        let prototype_type = reader.type_name(&prototype_path).unwrap_or_default();
        let mut transform = instance.transform;
        if matches!(
            prototype_type.as_str(),
            "Cylinder" | "Cone" | "Capsule" | "Plane"
        ) {
            if let Some(axis) = read_primitive_axis(reader, &prototype_path, &prototype_type) {
                transform.rotation *=
                    convention.orient(usd_axis_to_quat(&axis).unwrap_or(Quat::IDENTITY));
            }
        }
        commands.spawn((
            Name::new(format!("{}[{}]", path.as_str(), instance.index)),
            ChildOf(parent),
            transform,
            GlobalTransform::default(),
            if instance.visible {
                Visibility::Visible
            } else {
                Visibility::Hidden
            },
            InheritedVisibility::VISIBLE,
            ViewVisibility::default(),
            UsdPointInstance {
                stage_id: stage_handle.id(),
                index: instance.index,
                id: instance.id,
                prototype_path: instance.prototype_path,
            },
        ));
    }
    Ok(())
}

/// Copy a ready prototype mesh and appearance intent onto point-instancer
/// children. Handles are deliberately shared: Bevy's automatic instancing
/// requires equal `Handle<Mesh>` and `Handle<Material>` values, while the
/// authored `UsdPointInstance` id remains independent of unstable GPU batch
/// ordering.
fn resolve_point_instancer_meshes(
    mut commands: Commands,
    prototypes: Query<(&UsdPrimPath, Option<&Mesh3d>, Option<&PbrLook>)>,
    instances: Query<(Entity, &UsdPointInstance), Without<Mesh3d>>,
) {
    let mut ready = std::collections::HashMap::new();
    for (path, mesh, look) in &prototypes {
        if let (Some(mesh), Some(look)) = (mesh, look) {
            ready.insert(
                (path.stage_handle.id(), path.path.clone()),
                (mesh.0.clone(), look.clone()),
            );
        }
    }
    for (entity, instance) in &instances {
        let Some((mesh, look)) = ready.get(&(instance.stage_id, instance.prototype_path.clone()))
        else {
            continue;
        };
        commands
            .entity(entity)
            .try_insert((Mesh3d(mesh.clone()), look.clone()));
    }
}

/// Keep prototype source prims out of the visible scene traversal. OpenUSD
/// permits prototypes anywhere in the scenegraph, so this is relationship-
/// driven rather than a name/path convention. Descendants inherit the hidden
/// prototype root's visibility in Bevy.
fn hide_point_instancer_prototypes(
    instancers: Query<&UsdPointInstancer>,
    mut prims: Query<(&UsdPrimPath, &mut Visibility)>,
) {
    for instancer in &instancers {
        for prototype in &instancer.prototype_paths {
            for (path, mut visibility) in &mut prims {
                if (path.path == *prototype
                    || path
                        .path
                        .strip_prefix(prototype)
                        .is_some_and(|suffix| suffix.starts_with('/')))
                    && *visibility != Visibility::Hidden
                {
                    *visibility = Visibility::Hidden;
                }
            }
        }
    }
}

/// Materialize a hidden source projection when a relationship targets a prim
/// outside the currently mounted traversal subtree. OpenUSD permits prototype
/// roots anywhere in the scenegraph; this keeps that legal arrangement working
/// without fabricating a second mesh or altering the authored path.
fn ensure_point_instancer_prototypes(
    instancers: Query<(Entity, &UsdPointInstancer)>,
    prims: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let mut existing = prims
        .iter()
        .map(|path| (path.stage_handle.id(), path.path.clone()))
        .collect::<std::collections::HashSet<_>>();
    for (parent, instancer) in &instancers {
        for prototype in &instancer.prototype_paths {
            let key = (instancer.stage_handle.id(), prototype.clone());
            if !existing.insert(key) {
                continue;
            }
            queue_usd_child_spawn(
                &mut commands,
                parent,
                (
                    Name::new(prototype.clone()),
                    UsdPrimPath {
                        stage_handle: instancer.stage_handle.clone(),
                        path: prototype.clone(),
                    },
                    Transform::default(),
                    GlobalTransform::default(),
                    Visibility::Visible,
                    InheritedVisibility::VISIBLE,
                    ViewVisibility::default(),
                    UsdSceneAwaitingStage,
                    UsdSceneProjectionQueued,
                ),
                (),
                None,
            );
        }
    }
}

/// Project the authored catalog identity from one USD prim onto its ECS owner.
///
/// The same USD attribute is valid on an instance root and on a child prim;
/// both are projected through this one boundary so identity ownership does not
/// depend on which part of the composed asset owns the authored opinion.
fn project_catalog_entry_id(
    reader: &impl UsdRead,
    path: &SdfPath,
    entity: Entity,
    commands: &mut Commands,
) {
    if let Some(entry_id) = reader
        .text(path, "lunco:catalogId")
        .filter(|id| !id.trim().is_empty())
    {
        commands
            .entity(entity)
            .try_insert(lunco_core::CatalogEntryId(entry_id));
    }
}

/// Project the standard USD kind token used for identity/category reporting.
/// This read stays on the composed stage and follows references and variants
/// exactly like the visual projection.
fn project_usd_prim_kind<R: UsdRead>(
    reader: &R,
    path: &SdfPath,
    entity: Entity,
    commands: &mut Commands,
) {
    if let Some(kind) = reader.kind(path).filter(|kind| !kind.is_empty()) {
        commands
            .entity(entity)
            .try_insert(lunco_core::UsdPrimKind(kind));
    }
}

/// Project the authored spawnable marker onto the prim's selectable root.
///
/// `lunco:spawnable` is the authored identity boundary for selectable asset
/// roots.  It applies equally to a stage root and to a nested child, so both
/// projection paths use this helper rather than maintaining separate policy.
fn project_spawnable_selectable(
    reader: &impl UsdRead,
    path: &SdfPath,
    entity: Entity,
    commands: &mut Commands,
) {
    if reader.boolean(path, "lunco:spawnable").unwrap_or(false) {
        commands
            .entity(entity)
            .try_insert(lunco_core::SelectableRoot);
    }
}

/// Record the direct USD children from an owned read source.
///
/// Initial loads pass the worker-produced [`UsdStageProjectionPlan`], so this
/// function performs only cheap map reads and ECS command recording on the main
/// thread. Each child enters the same queue and owns the next direct-child
/// commit. A later live structural edit explicitly passes the canonical
/// [`StageView`] and follows the same ownership boundary for that edit.
fn commit_usd_children<R: UsdRead>(
    parent: Entity,
    stage_handle: &Handle<UsdStageAsset>,
    reader: &R,
    parent_path: &SdfPath,
    child_member: &Option<UsdInstanceMember>,
    instance_projection: Option<&UsdInstanceProjection>,
    is_high_precision_parent: bool,
    is_grid_entity: bool,
    commands: &mut Commands,
) {
    for child_path in reader.children(parent_path) {
        if !reader.is_active(&child_path) {
            continue;
        }

        let child_tf = match read_transform_from_usd(reader, &child_path) {
            Ok(transform) => transform,
            Err(error) => {
                error!(
                    "[usd-bevy] {} has malformed authored transform; visual projection rejected: {}",
                    child_path.as_str(),
                    error
                );
                commands.entity(parent).try_insert((
                    UsdSceneProjectionFailed(error.to_string()),
                    Visibility::Hidden,
                ));
                return;
            }
        };

        let base_components = (
            Name::new(child_path.to_string()),
            UsdPrimPath {
                stage_handle: stage_handle.clone(),
                path: child_path.to_string(),
            },
            child_tf,
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::VISIBLE,
            ViewVisibility::default(),
            UsdSceneAwaitingStage,
            UsdSceneProjectionQueued,
        );
        let is_low_precision_root_target = is_high_precision_parent && !is_grid_entity;
        let child_entity = match child_member {
            Some(member) if is_low_precision_root_target => queue_usd_child_spawn(
                commands,
                parent,
                base_components,
                (
                    member.clone(),
                    big_space::grid::propagation::LowPrecisionRoot,
                ),
                instance_projection.cloned(),
            ),
            Some(member) if is_grid_entity => queue_usd_child_spawn(
                commands,
                parent,
                base_components,
                (member.clone(), CellCoord::default()),
                instance_projection.cloned(),
            ),
            Some(member) => queue_usd_child_spawn(
                commands,
                parent,
                base_components,
                (member.clone(),),
                instance_projection.cloned(),
            ),
            None if is_low_precision_root_target => queue_usd_child_spawn(
                commands,
                parent,
                base_components,
                (big_space::grid::propagation::LowPrecisionRoot,),
                instance_projection.cloned(),
            ),
            None if is_grid_entity => queue_usd_child_spawn(
                commands,
                parent,
                base_components,
                (CellCoord::default(),),
                instance_projection.cloned(),
            ),
            None => queue_usd_child_spawn(
                commands,
                parent,
                base_components,
                (),
                instance_projection.cloned(),
            ),
        };

        project_spawnable_selectable(reader, &child_path, child_entity, commands);
        project_catalog_entry_id(reader, &child_path, child_entity, commands);
    }
}

/// Queue a USD child spawn with a final parent-liveness check.
///
/// A scene replacement can invalidate a parent after the visual extractor has
/// queued work but before Bevy applies that work.  `Commands::spawn((..., ChildOf
/// (parent)))` would still create an unparented orphan when the parent is gone;
/// worse, its `UsdPrimPath` observer would continue projecting the orphan.  The
/// empty allocation is harmlessly reclaimed when the check fails, and the
/// authored bundle is inserted only while the parent is live. Projection
/// uniqueness comes from the queue marker and USD parent/child traversal; this
/// command deliberately does not scan the world for a path that may be valid in
/// another scene mount or runtime instance.
fn queue_usd_child_spawn<Base: Bundle, Extra: Bundle>(
    commands: &mut Commands,
    parent: Entity,
    base: Base,
    extra: Extra,
    projection: Option<UsdInstanceProjection>,
) -> Entity {
    let child = commands.spawn_empty().id();
    commands.queue(move |world: &mut World| {
        if world.get_entity(parent).is_err() || !scene_mount_entity_is_live(world, parent) {
            let _ = world.despawn(child);
            return;
        }
        let Ok(mut entity) = world.get_entity_mut(child) else {
            return;
        };
        entity.insert((base, ChildOf(parent), extra));
        if let Some(projection) = projection {
            entity.insert(projection);
        }
    });
    child
}

/// Check the scene ownership fence from inside a deferred command.
///
/// The command is the last point before a child bundle would trigger its USD
/// observers. A replacement may have invalidated the root while the parent
/// entity is still present in the deferred-despawn window, so a parent-liveness
/// check alone is insufficient.
fn scene_mount_entity_is_live(world: &World, entity: Entity) -> bool {
    let Some(state) = world.get_resource::<lunco_core::SceneMountState>() else {
        return true;
    };
    let mut current = entity;
    for _ in 0..1024 {
        if world.get::<UsdSceneRoot>(current).is_some() {
            return state.contains_root(current);
        }
        let Some(parent) = world.get::<ChildOf>(current).map(ChildOf::parent) else {
            return true;
        };
        if world.get_entity(parent).is_err() {
            return false;
        }
        current = parent;
    }
    false
}

/// Observer: fires the moment a new `UsdPrimPath` is added to an entity.
/// If the referenced `UsdStageAsset` is already loaded, the prim is queued for
/// bounded projection. Otherwise the entity is tagged `UsdSceneAwaitingStage` and
/// waits for `sync_usd_visuals` to move it once the asset becomes ready.
///
/// This is the **happy path** in steady state — once a scene is loaded,
/// any newly-spawned `UsdPrimPath` entity (API command, attach
/// operation, recursive child spawn) enters the same bounded queue. The queue
/// is dormant when no prim is waiting.
fn on_usd_prim_added(
    trigger: On<Add, UsdPrimPath>,
    q: Query<
        &UsdPrimPath,
        (
            Without<UsdSceneProjected>,
            Without<UsdSceneProjectionFailed>,
        ),
    >,
    mut commands: Commands,
    stages: Res<Assets<UsdStageAsset>>,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = q.get(entity) else {
        return;
    };

    if stages.get(&prim_path.stage_handle).is_none() {
        commands.entity(entity).try_insert(UsdSceneAwaitingStage);
        return;
    }

    // Do not perform USD reads, mesh generation, or recursive child spawning
    // from an Add observer. Child observers run while Bevy applies the
    // previous projection's command buffer; doing the work here made one
    // heavy scene occupy the entire update before the window could repaint.
    // The bounded queue below owns the same projection path for both authored
    // scene children and runtime-added prims.
    commands
        .entity(entity)
        .try_insert((UsdSceneAwaitingStage, UsdSceneProjectionQueued));
}

/// Observer: fires when `CellCoord` is added to an entity.
/// Stamping `LowPrecisionRoot` on its direct spatial children ensures that when an
/// entity receives a `CellCoord` (e.g. site anchor or DEM placement), its children
/// immediately satisfy big_space's hierarchy validation rules (`ChildRootSpatialLowPrecision`).
fn on_cell_coord_added(
    trigger: On<Add, big_space::prelude::CellCoord>,
    q_children: Query<&Children>,
    q_spatial_child: Query<
        (),
        (
            With<Transform>,
            With<GlobalTransform>,
            Without<big_space::prelude::CellCoord>,
            Without<big_space::grid::propagation::LowPrecisionRoot>,
        ),
    >,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if let Ok(children) = q_children.get(entity) {
        for child in children.iter() {
            if q_spatial_child.contains(child) {
                commands
                    .entity(child)
                    .try_insert(big_space::grid::propagation::LowPrecisionRoot);
            }
        }
    }
}

/// Moves the `UsdSceneAwaitingStage` queue into bounded visual projection when a
/// stage finishes loading. Each matching entity remains marked as awaiting
/// until `process_queued_usd_visuals` commits it.
pub fn sync_usd_visuals(
    mut ev: MessageReader<AssetEvent<UsdStageAsset>>,
    q: Query<
        (Entity, &UsdPrimPath),
        (
            With<UsdSceneAwaitingStage>,
            Without<UsdSceneProjectionQueued>,
            Without<UsdSceneProjected>,
            Without<UsdSceneProjectionFailed>,
        ),
    >,
    q_child_of: Query<&ChildOf>,
    q_entities: Query<Entity>,
    q_scene_root: Query<(), With<UsdSceneRoot>>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    mount_state: Res<lunco_core::SceneMountState>,
    mut commands: Commands,
) {
    use bevy::asset::AssetId;
    let mut loaded: Vec<AssetId<UsdStageAsset>> = Vec::new();
    for event in ev.read() {
        if let AssetEvent::LoadedWithDependencies { id } = event {
            loaded.push(*id);
        }
    }
    if loaded.is_empty() {
        return;
    }

    for (entity, prim_path) in q.iter() {
        if !loaded.iter().any(|id| prim_path.stage_handle.id() == *id) {
            continue;
        }

        let preview_only = is_preview_only(entity, &q_child_of, &q_preview_only);
        let stale_mount = match scene_root_ancestor(entity, &q_scene_root, &q_child_of, &q_entities)
        {
            Ok(Some(root)) => !mount_state.contains_root(root),
            Ok(None) => false,
            Err(_) => true,
        };
        if !preview_only && stale_mount {
            // The stage event is real, but this entity belongs to a root that
            // was invalidated by a newer load.  Do not enqueue even one
            // deferred command against it: teardown is intentionally deferred
            // too, and this is the window that previously produced Bevy's
            // invalid-entity panic in `sync_usd_visuals`.
            continue;
        }

        // Keep the stage marker until the bounded projection pass commits the
        // prim. This is what prevents the scene transaction from reporting
        // success while descendants are still waiting for a frame.
        commands.entity(entity).try_insert(UsdSceneProjectionQueued);
    }
}

fn any_queued_usd_visuals(q: Query<(), With<UsdSceneProjectionQueued>>) -> bool {
    !q.is_empty()
}

fn any_pending_usd_meshes(q: Query<(), With<PendingUsdMesh>>) -> bool {
    !q.is_empty()
}

/// Project USD prims until the configured wall-clock budget is exhausted.
///
/// The queue is the only structural projection boundary: each admitted prim is
/// bound once, its prepared direct children are queued, and the next frame
/// continues from that ownership fence. Initial reads use the worker-produced
/// plan; later generations use the canonical live reader. CPU geometry retains
/// the existing async compute path, while ECS and Bevy asset mutation stay on
/// the main thread.
#[allow(clippy::too_many_arguments)]
fn process_queued_usd_visuals(
    q: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&Visibility>,
            Option<&Transform>,
            Has<UsdInstanceRoot>,
            Option<&UsdInstanceMember>,
            Option<&UsdInstanceProjection>,
        ),
        (
            With<UsdSceneProjectionQueued>,
            Without<UsdSceneProjected>,
            Without<UsdSceneProjectionFailed>,
            Without<PendingUsdMesh>,
        ),
    >,
    q_high_precision: Query<
        (),
        Or<(
            With<big_space::prelude::Grid>,
            With<big_space::prelude::CellCoord>,
        )>,
    >,
    q_grid: Query<(), With<big_space::prelude::Grid>>,
    q_child_of: Query<&ChildOf>,
    q_scene_root: Query<(), With<UsdSceneRoot>>,
    q_entities: Query<Entity>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    mount_state: Res<lunco_core::SceneMountState>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    quality: Res<lunco_render::RenderingQualitySettings>,
    settings: Res<UsdVisualProjectionSettings>,
    mut commands: Commands,
) {
    let requested_profile = match quality.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!("[usd-bevy] invalid Graphics quality; deferring USD visual projection: {reason}");
            return;
        }
    };
    if settings.frame_budget.is_zero() {
        error!(
            "[usd-bevy] USD visual projection requires a non-zero frame budget; refusing invalid configuration"
        );
        return;
    }
    let started = web_time::Instant::now();
    let mut projected = 0usize;

    for (entity, prim_path, vis, tf, is_instance_root, member, instance_projection) in q.iter() {
        if projected != 0 && started.elapsed() >= settings.frame_budget {
            break;
        }
        if stages.get(&prim_path.stage_handle).is_none() {
            continue;
        }

        let preview_only = is_preview_only(entity, &q_child_of, &q_preview_only);
        if !preview_only {
            let stale_mount =
                match scene_root_ancestor(entity, &q_scene_root, &q_child_of, &q_entities) {
                    Ok(Some(root)) => !mount_state.contains_root(root),
                    Ok(None) => false,
                    Err(_) => true,
                };
            if stale_mount {
                // The replacement already invalidated this root. Reclaim the
                // queued entity instead of allowing it to instantiate after the
                // new scene has mounted.
                commands.entity(entity).try_despawn();
                projected += 1;
                continue;
            }
        }

        commands
            .entity(entity)
            .try_remove::<UsdSceneProjectionQueued>()
            .try_remove::<UsdSceneAwaitingStage>();
        let is_high_precision_parent = q_high_precision.contains(entity)
            || q_child_of
                .get(entity)
                .ok()
                .is_some_and(|c| q_high_precision.contains(c.parent()));
        let parent_is_grid = q_child_of
            .get(entity)
            .ok()
            .is_some_and(|c| q_grid.contains(c.parent()));
        instantiate_usd_prim(
            entity,
            prim_path,
            vis,
            tf,
            is_instance_root,
            member,
            instance_projection,
            is_high_precision_parent,
            parent_is_grid,
            q_grid.contains(entity),
            preview_only,
            &mut commands,
            &stages,
            &canonical,
            &asset_server,
            &mut meshes,
            requested_profile,
        );
        projected += 1;
    }
    if projected > 0 {
        debug!(
            "[usd-bevy] projected {projected} prim(s) in {:.2} ms",
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
}

/// Commit completed CPU-generated USD meshes without reading USD again.
///
/// The extraction side owns the live-stage read and publishes the editable
/// definition. This side only validates the stage generation and quality
/// snapshot, inserts the worker-produced Bevy mesh, and binds the already
/// authored appearance intent. A live edit, quality change, or scene replacement
/// cancels the result and returns the entity to the canonical projection queue.
fn poll_pending_usd_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    quality: Res<lunco_render::RenderingQualitySettings>,
    mut q: Query<(
        Entity,
        &UsdPrimPath,
        Has<UsdSceneProjected>,
        Option<&UsdVisualMeshTarget>,
        &mut PendingUsdMesh,
    )>,
) {
    let current_profile = match quality.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!("[usd-bevy] invalid Graphics quality; retaining pending USD meshes: {reason}");
            return;
        }
    };

    for (entity, prim_path, visual_synced, visual_target, mut pending) in &mut q {
        let Some(_stage_asset) = stages.get(pending.stage_id) else {
            continue;
        };
        let stage_generation = canonical.generation_for(pending.stage_id);
        let stale = !visual_synced
            || prim_path.stage_handle.id() != pending.stage_id
            || SdfPath::new(&prim_path.path).ok().as_ref() != Some(&pending.path)
            || stage_generation != pending.stage_generation
            || current_profile != pending.profile;
        if stale {
            commands
                .entity(entity)
                .try_remove::<PendingUsdMesh>()
                .try_remove::<UsdSceneGeometryPending>()
                .try_insert(UsdSceneProjectionQueued);
            continue;
        }

        let Some(result) = block_on(future::poll_once(&mut pending.task)) else {
            continue;
        };
        let Some(result) = result else {
            warn!(
                "[usd-bevy] {} CPU mesh build produced no geometry; visual mesh was not created",
                pending.path.as_str()
            );
            commands
                .entity(entity)
                .try_remove::<PendingUsdMesh>()
                .try_remove::<UsdSceneGeometryPending>()
                .try_remove::<UsdPrimitiveMesh>()
                .try_remove::<UsdCurveMesh>()
                .try_remove::<lathe::NurbsSurface>()
                .try_remove::<lathe::UsdLathe>();
            continue;
        };

        let mesh_handle = meshes.add(result);
        let render_entity = visual_target.map_or(entity, |target| target.0);
        commands
            .entity(render_entity)
            .try_insert(Mesh3d(mesh_handle.clone()));
        commands
            .entity(entity)
            .try_remove::<PendingUsdMesh>()
            .try_remove::<UsdSceneGeometryPending>();
    }
}

/// Retry USD prims that were deliberately parked because Graphics settings were
/// invalid. The settings UI rejects such values before insertion, but scripts,
/// tests, and a host may still mutate the resource directly. A corrected value
/// is therefore a complete recovery event; requiring a scene reload here would
/// turn a rejected edit into a lifecycle leak.
fn retry_awaiting_usd_visuals_after_quality_change(
    q: Query<
        (Entity, &UsdPrimPath),
        (
            With<UsdSceneAwaitingStage>,
            Without<UsdSceneProjectionQueued>,
            Without<UsdSceneProjected>,
            Without<UsdSceneProjectionFailed>,
        ),
    >,
    q_child_of: Query<&ChildOf>,
    q_scene_root: Query<(), With<UsdSceneRoot>>,
    q_entities: Query<Entity>,
    mount_state: Res<lunco_core::SceneMountState>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    mut commands: Commands,
    stages: Res<Assets<UsdStageAsset>>,
    quality: Res<lunco_render::RenderingQualitySettings>,
) {
    if let Err(reason) = quality.validated_profile() {
        warn!(
            "[usd-bevy] invalid Graphics quality; USD visual projection remains parked: {reason}"
        );
        return;
    }

    for (entity, prim_path) in &q {
        if stages.get(&prim_path.stage_handle).is_none() {
            continue;
        }
        let preview_only = is_preview_only(entity, &q_child_of, &q_preview_only);
        let stale_mount = match scene_root_ancestor(entity, &q_scene_root, &q_child_of, &q_entities)
        {
            Ok(Some(root)) => !mount_state.contains_root(root),
            Ok(None) => false,
            Err(_) => true,
        };
        if !preview_only && stale_mount {
            continue;
        }

        // Quality changes only release the queue. The bounded projection pass
        // remains the sole owner of USD reads and mesh/material generation.
        commands.entity(entity).try_insert(UsdSceneProjectionQueued);
    }
}

/// Convergence is at most one frame behind the root's id allocation: the member
/// stays parked (`Local` is a no-op in `assign_global_entity_ids`, so it is
/// never given a colliding auto-allocated id) until this runs, after which the
/// same-frame `assign_global_entity_ids` (PostUpdate) derives the real id.
/// `UsdInstanceMember` is removed on upgrade so each member resolves once.
fn resolve_usd_instance_identities(
    mut commands: Commands,
    members: Query<(Entity, &UsdInstanceMember, &UsdPrimPath), Without<lunco_core::GlobalEntityId>>,
    roots: Query<&lunco_core::GlobalEntityId>,
) {
    for (entity, member, prim_path) in members.iter() {
        let Ok(root_gid) = roots.get(member.root) else {
            continue;
        };
        let role = instance_role(&member.root_path, &prim_path.path);
        commands
            .entity(entity)
            .try_insert(lunco_core::Provenance::Derived {
                parent: root_gid.get(),
                role,
            })
            .remove::<UsdInstanceMember>();
    }
}

/// Maps a `UsdUVTexture` `inputs:wrapS`/`inputs:wrapT` token to a Bevy sampler
/// address mode. USD's `"useMetadata"` (and absent) use the documented
/// projection default `Repeat` because the image-header metadata is not part
/// of the material intent reader. An authored token outside the USD schema is
/// rejected instead of becoming a repeat sampler by accident.
fn usd_wrap_to_address(
    wrap: Option<&str>,
    attribute: &str,
) -> Result<bevy::image::ImageAddressMode, MaterialReadError> {
    use bevy::image::ImageAddressMode;
    match wrap.unwrap_or("useMetadata") {
        "useMetadata" | "repeat" => Ok(ImageAddressMode::Repeat),
        "clamp" => Ok(ImageAddressMode::ClampToEdge),
        "mirror" => Ok(ImageAddressMode::MirrorRepeat),
        "black" => Ok(ImageAddressMode::ClampToBorder),
        _ => Err(MaterialReadError::new(attribute)),
    }
}

/// Identifies an authored material property that cannot be represented by the
/// render intent.  The caller must reject the whole look: substituting a
/// plausible value would make a typo or a wrong USD type look like a valid
/// material and would leave the ECS projection disagreeing with the stage.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MaterialReadError {
    attribute: String,
}

impl MaterialReadError {
    fn new(attribute: &str) -> Self {
        Self {
            attribute: attribute.to_string(),
        }
    }
}

/// Read a schema-declared USD token without treating a wrong value type as an
/// omitted token.  Texture controls use this rather than `text`, whose broad
/// textual coercion is appropriate for display labels but not for enum inputs.
fn read_material_token(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<String>, MaterialReadError> {
    match reader.attr_value(path, attribute) {
        Some(Value::Token(value)) => Ok(Some(value.to_string())),
        Some(_) => Err(MaterialReadError::new(attribute)),
        None if reader.has_authored_attribute(path, attribute) => {
            Err(MaterialReadError::new(attribute))
        }
        None => Ok(None),
    }
}

/// Read one authored real material input while preserving omission as a
/// semantic default.  Connections are deliberately rejected here because the
/// scalar PBR path has no graph evaluator; texture-capable inputs go through
/// `load_tex` first and scalar-only inputs must be authored values.
fn read_material_real(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<f32>, MaterialReadError> {
    if !reader.connections(path, attribute).is_empty() {
        return Err(MaterialReadError::new(attribute));
    }
    match reader.real_f32(path, attribute) {
        Some(value) if value.is_finite() => Ok(Some(value)),
        Some(_) => Err(MaterialReadError::new(attribute)),
        None if reader.has_authored_attribute(path, attribute) => {
            Err(MaterialReadError::new(attribute))
        }
        None => Ok(None),
    }
}

/// Read one authored scalar USD color input.  `UsdPreviewSurface` declares
/// these as scalar `color3f`/`color3d`, not array primvars; accepting an array
/// here would reintroduce the type confusion that the display-primvar reader
/// intentionally avoids.
fn read_material_vec3(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<Vec3>, MaterialReadError> {
    if !reader.connections(path, attribute).is_empty() {
        return Err(MaterialReadError::new(attribute));
    }
    let value = reader.attr_value(path, attribute).and_then(|value| {
        value
            .clone()
            .get::<[f32; 3]>()
            .map(|v| Vec3::new(v[0], v[1], v[2]))
            .or_else(|| {
                value
                    .get::<[f64; 3]>()
                    .map(|v| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32))
            })
    });
    match value {
        Some(value) if value.is_finite() => Ok(Some(value)),
        Some(_) => Err(MaterialReadError::new(attribute)),
        None if reader.has_authored_attribute(path, attribute) => {
            Err(MaterialReadError::new(attribute))
        }
        None => Ok(None),
    }
}

/// Read an authored boolean surface flag.  The shared USD boolean reader keeps
/// its documented integer spelling support, while malformed strings/arrays are
/// refused instead of becoming `false`.
fn read_material_bool(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<bool>, MaterialReadError> {
    read_authored_bool_strict(reader, path, attribute)
        .map_err(|_| MaterialReadError::new(attribute))
}

/// Validate a `UsdPreviewSurface` value whose schema meaning is a unit
/// interval.  The USD default remains available through `None`; an authored
/// value outside the interval is malformed and is never clamped.
fn material_unit_interval(
    value: Option<f32>,
    attribute: &str,
) -> Result<Option<f32>, MaterialReadError> {
    match value {
        Some(value) if (0.0..=1.0).contains(&value) => Ok(Some(value)),
        Some(_) => Err(MaterialReadError::new(attribute)),
        None => Ok(None),
    }
}

/// Authors the PBR appearance **intent** ([`lunco_render::PbrLook`]) for an
/// entity, resolving material bindings and shader networks if present, or
/// falling back to direct prim attributes.
///
/// This crate never names `StandardMaterial` — `lunco-render-bevy` observes the
/// `PbrLook` and binds the real material (see
/// `docs/architecture/render-decoupling.md`). Texture *loading* stays here: it
/// is `AssetServer` + `bevy_image` (sRGB per channel, `wrapS`/`wrapT` sampler
/// address modes), all render-free.
///
/// **Animated prims get an `unshared` look**: the material sampler
/// (`lunco-usd-bevy-animation::sample_usd_material_animation`) mutates the `PbrLook` every frame, and a
/// shared (content-keyed) look would mint a fresh material per frame and free
/// none. `unshared` gives it a private material the binder mutates in place.
fn read_standard_material(
    reader: &dyn read::UsdReadObject,
    sdf_path: &SdfPath,
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
) -> Result<PbrLook, MaterialReadError> {
    let mut base_color_texture = None;
    let mut emissive_texture = None;
    let mut metallic_roughness_texture = None;
    let mut normal_map_texture = None;
    let mut occlusion_texture = None;

    // Direct geometry attributes form the baseline. USD `color3f` values are
    // linear scene-referred, and the inspector writes `displayColor` from
    // `base_color.to_linear()`, so read them back as linear (not sRGB) to keep
    // the edit/save/reload round-trip stable.
    //
    // ARRAY-valued: `UsdGeomGprim` declares `color3f[] primvars:displayColor`, so
    // this reads `Vec3fVec`, not a scalar `color3f`. See `read_primvar_vec3`.
    let mut base_color = read_primvar_vec3_strict(reader, sdf_path, "primvars:displayColor")
        .map_err(|_| MaterialReadError::new("primvars:displayColor"))?
        .map(|v| Color::linear_rgb(v[0] as f32, v[1] as f32, v[2] as f32))
        .unwrap_or(Color::WHITE);

    // Emissive, metallic and roughness are **shader** inputs. They are NOT read from
    // the geometry, deliberately.
    //
    // This used to accept `inputs:metallic` (and bare `metallic`, `roughness`,
    // `reflectance`, `emissiveColor`, `inputs:perceptual_roughness`) authored
    // straight onto the Gprim. That is not valid USD — `inputs:*` is the
    // UsdShade namespace, and a `float inputs:metallic` on a Sphere is a value no
    // other DCC will ever read. The Inspector happily authored it, this read it
    // back, and the two bugs hid each other: scenes looked correct here and lost
    // their materials the moment they were opened anywhere else.
    //
    // Now there is exactly ONE way to have a material — bind one
    // (`lunco_usd_core::material::ensure_preview_surface_ops` builds it) — and exactly
    // one place these values come from: the bound `UsdPreviewSurface` below.
    // Deleting the fallback is the point: with it, nothing forces the correct
    // form; without it, the wrong form visibly does nothing.
    let mut emissive = LinearRgba::BLACK;
    let mut metallic = 0.0f32;
    let mut roughness = 0.5f32;

    // UsdPreviewSurface transparency + refraction. Default opaque (alpha 1) and
    // the glass-ish `ior` 1.5 USD uses; overridden only when a bound shader
    // authors `inputs:opacity` / `inputs:opacityThreshold` / `inputs:ior`.
    //
    // Geometry-baseline transparency: the standard UsdGeomGprim
    // `primvars:displayOpacity` lets a simple prim be translucent WITHOUT a
    // bound shader network. A bound shader's `inputs:opacity` still wins below.
    // A sub-1 value flips `AlphaMode::Blend` via the rule further down; opaque
    // marker assets omit this optional primvar.
    // `primvars:displayOpacity` ONLY — the bare `displayOpacity` alias is gone.
    // It is not a UsdGeomGprim attribute, and accepting it meant a typo'd primvar
    // still worked here and nowhere else. ARRAY-valued (`float[]`) by schema.
    let mut alpha = material_unit_interval(
        read_primvar_f32_strict(reader, sdf_path, "primvars:displayOpacity")
            .map_err(|_| MaterialReadError::new("primvars:displayOpacity"))?,
        "primvars:displayOpacity",
    )?
    .unwrap_or(1.0);
    let mut ior = 1.5f32;
    let mut opacity_threshold = 0.0f32;

    // Clearcoat layer.
    let mut clearcoat = 0.0f32;
    let mut clearcoat_roughness = 0.0f32;

    // Specular tint — only meaningful under `useSpecularWorkflow = 1`. White (untinted)
    // unless the shader says otherwise, matching `StandardMaterial`'s default.
    let mut specular_tint = LinearRgba::WHITE;

    // A bound material shader network overrides individual channels where it
    // authors them. Channels the shader omits — or whose texture connection
    // fails to resolve — keep the geometry baseline above rather than reverting
    // to a flat-white default.
    if let Some(shader_path) = resolve_bound_shader(reader, sdf_path) {
        use bevy::image::{ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor};

        // Resolve a shader input's connected `UsdUVTexture` to a loadable image
        // handle, or `None` if it has no connection / file / resolvable path.
        // `is_color` is the channel's default color space (true = sRGB for
        // albedo/emissive, false = linear data for metallic/roughness/normal/AO);
        // a `UsdUVTexture inputs:sourceColorSpace` of `raw`/`sRGB` overrides it.
        // `inputs:wrapS`/`wrapT` drive the sampler address modes at load time.
        let load_tex =
            |input: &str, is_color: bool| -> Result<Option<Handle<Image>>, MaterialReadError> {
                let conn = match reader.connection_source(&shader_path, input) {
                    Some(conn) => conn,
                    None if reader.connections(&shader_path, input).is_empty() => return Ok(None),
                    None => return Err(MaterialReadError::new(input)),
                };
                let texture_path =
                    parent_prim_path(&conn).ok_or_else(|| MaterialReadError::new(input))?;
                let asset_path = reader
                    .asset(&texture_path, "inputs:file")
                    .ok_or_else(|| MaterialReadError::new(input))?;
                let resolved = lunco_usd_bevy_core::asset::resolve_stage_asset_path(
                    asset_server,
                    stage_id,
                    &asset_path,
                );

                let is_srgb =
                    match read_material_token(reader, &texture_path, "inputs:sourceColorSpace")?
                        .as_deref()
                    {
                        Some("sRGB") => true,
                        Some("raw") => false,
                        Some("auto") | None => is_color,
                        Some(_) => {
                            return Err(MaterialReadError::new("inputs:sourceColorSpace"));
                        }
                    };
                let addr_u = usd_wrap_to_address(
                    read_material_token(reader, &texture_path, "inputs:wrapS")?.as_deref(),
                    "inputs:wrapS",
                )?;
                let addr_v = usd_wrap_to_address(
                    read_material_token(reader, &texture_path, "inputs:wrapT")?.as_deref(),
                    "inputs:wrapT",
                )?;

                Ok(Some(
                    asset_server
                        .load_builder()
                        .with_settings(move |s: &mut ImageLoaderSettings| {
                            s.is_srgb = is_srgb;
                            let mut d = ImageSamplerDescriptor::linear();
                            d.address_mode_u = addr_u;
                            d.address_mode_v = addr_v;
                            s.sampler = ImageSampler::Descriptor(d);
                        })
                        .load::<Image>(resolved),
                ))
            };

        // diffuseColor: texture, else authored value, else geometry baseline.
        base_color_texture = load_tex("inputs:diffuseColor", true)?;
        if base_color_texture.is_none() {
            if let Some(c) = read_material_vec3(reader, &shader_path, "inputs:diffuseColor")? {
                base_color = Color::linear_rgb(c.x, c.y, c.z);
            }
        }

        // emissiveColor
        emissive_texture = load_tex("inputs:emissiveColor", true)?;
        if emissive_texture.is_none() {
            if let Some(c) = read_material_vec3(reader, &shader_path, "inputs:emissiveColor")? {
                emissive = LinearRgba::new(c.x, c.y, c.z, 1.0);
            }
        }

        // metallic
        let metallic_texture = load_tex("inputs:metallic", false)?;
        if metallic_texture.is_none() {
            if let Some(m) = material_unit_interval(
                read_material_real(reader, &shader_path, "inputs:metallic")?,
                "inputs:metallic",
            )? {
                metallic = m;
            }
        }

        // roughness. `inputs:roughness` ONLY — `inputs:perceptual_roughness` is
        // Bevy's field name, not a UsdPreviewSurface input, and accepting it here
        // just taught callers to author a value usdview will never read.
        let roughness_texture = load_tex("inputs:roughness", false)?;
        if roughness_texture.is_none() {
            if let Some(r) = material_unit_interval(
                read_material_real(reader, &shader_path, "inputs:roughness")?,
                "inputs:roughness",
            )? {
                roughness = r;
            }
        }

        metallic_roughness_texture = roughness_texture.or(metallic_texture);

        normal_map_texture = load_tex("inputs:normal", false)?;
        occlusion_texture = load_tex("inputs:occlusion", false)?;

        // NOTE: there is no `inputs:reflectance`. `UsdPreviewSurface` has no such
        // input — its specular strength is `inputs:ior` (read below), and Bevy's
        // `reflectance` is derived from it in `lunco-render-bevy`. We used to author
        // and read a private `inputs:reflectance` inside UsdShade's reserved `inputs:`
        // namespace: writer and reader agreed with each other and with nothing else,
        // so scenes looked right here and lost the value everywhere else. Same bug as
        // the bare `metallic`/`roughness` on the Gprim, described above.

        // Specular workflow: `useSpecularWorkflow = 1` describes a dielectric by
        // `specularColor` instead of metalness → force metallic 0 (USD's specular
        // workflow has no metalness channel), and carry the tint.
        if read_material_real(reader, &shader_path, "inputs:useSpecularWorkflow")?.unwrap_or(0.0)
            >= 0.5
        {
            metallic = 0.0;
            if let Some(c) = read_material_vec3(reader, &shader_path, "inputs:specularColor")? {
                specular_tint = LinearRgba::rgb(c[0], c[1], c[2]);
            }
        }

        // Clearcoat layer (UsdPreviewSurface ↔ StandardMaterial 1:1).
        if let Some(c) = material_unit_interval(
            read_material_real(reader, &shader_path, "inputs:clearcoat")?,
            "inputs:clearcoat",
        )? {
            clearcoat = c;
        }
        if let Some(cr) = material_unit_interval(
            read_material_real(reader, &shader_path, "inputs:clearcoatRoughness")?,
            "inputs:clearcoatRoughness",
        )? {
            clearcoat_roughness = cr;
        }

        // Transparency: scalar `inputs:opacity` drives base-color alpha; a
        // connected opacity cannot be represented by the PBR intent because it
        // has no separate opacity texture slot. Reject it rather than claiming
        // Blend while retaining an unrelated opaque alpha.
        if let Some(o) = material_unit_interval(
            read_material_real(reader, &shader_path, "inputs:opacity")?,
            "inputs:opacity",
        )? {
            alpha = o;
        }
        opacity_threshold = material_unit_interval(
            read_material_real(reader, &shader_path, "inputs:opacityThreshold")?,
            "inputs:opacityThreshold",
        )?
        .unwrap_or(0.0);

        if let Some(i) = read_material_real(reader, &shader_path, "inputs:ior")? {
            if i <= 0.0 {
                return Err(MaterialReadError::new("inputs:ior"));
            }
            ior = i;
        }
    }

    // UsdPreviewSurface alpha semantics → `SurfaceAlpha`: a non-zero
    // `opacityThreshold` is a cutout (`Mask`); otherwise any sub-1 opacity is
    // alpha-blended; fully-opaque stays `Opaque` so
    // the depth-sorted transparent pass is only paid for when needed.
    //
    // `lunco:surface:additive` is a gprim-level USD surface policy, not a
    // shader opacity. It is the standard authored meaning of an emissive volume
    // such as an engine plume: add radiance without occluding the terrain behind
    // it. Read it here, at the USD material boundary, so every additive surface
    // (not only this episode's plume) gets the same render semantics.
    let additive = read_material_bool(reader, sdf_path, "lunco:surface:additive")?.unwrap_or(false);
    let alpha_mode = if additive {
        SurfaceAlpha::Add
    } else if opacity_threshold > 0.0 {
        SurfaceAlpha::Mask(opacity_threshold)
    } else if alpha < 1.0 {
        SurfaceAlpha::Blend
    } else {
        SurfaceAlpha::Opaque
    };
    // Entity-level cast intent is authored on the gprim, beside the standard
    // material network. It remains outside material sharing: two prims may use
    // the same UsdPreviewSurface while differing in shadow casting.
    let no_shadow_cast =
        read_material_bool(reader, sdf_path, "primvars:doNotCastShadows")?.unwrap_or(false);

    // An animated material channel means the sampler rewrites this look every
    // frame → it MUST NOT share a content-keyed material (that leaks one material
    // per distinct value, forever). `unshared` = a private material the binder
    // mutates in place.
    let animated = read::attr_has_time_samples(reader, sdf_path, "primvars:displayColor")
        || attr_has_time_samples(reader, sdf_path, "primvars:displayOpacity")
        || resolve_bound_shader(reader, sdf_path).is_some_and(|shader| {
            ANIMATED_SHADER_INPUTS
                .iter()
                .any(|i| attr_has_time_samples(reader, &shader, i))
        });

    Ok(PbrLook {
        base_color: base_color.with_alpha(alpha).to_linear(),
        emissive,
        perceptual_roughness: roughness,
        metallic,
        ior,
        clearcoat,
        clearcoat_perceptual_roughness: clearcoat_roughness,
        specular_tint,
        alpha: alpha_mode,
        textures: PbrTextures {
            base_color: base_color_texture,
            emissive: emissive_texture,
            metallic_roughness: metallic_roughness_texture,
            normal_map: normal_map_texture,
            occlusion: occlusion_texture,
        },
        unshared: animated,
        // `doubleSided` — core `UsdGeomGprim`, and it was not being read at all.
        //
        // It matters most for TRIMMED surfaces. A trim cuts a genuine hole, and
        // the moment there is a hole you can see the far side of the shell
        // through it. Single-sided, those backfaces are culled and the opening
        // reads as a hole from outside but as nothing at all from within — which
        // is exactly how HAB-1's arched doorway presented: visible from one side
        // only, with a black interior.
        //
        // USD's fallback is `false`, and that is kept: back-face culling is the
        // right default for closed solids and halves the fragment work. An asset
        // that opens itself up asks for the other behaviour explicitly.
        double_sided: read_material_bool(reader, sdf_path, "doubleSided")?.unwrap_or(false),
        // `primvars:doNotCastShadows` — OMNIVERSE'S name, not one of ours. RTX
        // reads it on the gprim and Composer surfaces it as the mesh's "Cast
        // Shadows" toggle, so a scene authored there arrives here with its shadow
        // intent intact. Its polarity already matches `no_shadow_cast`.
        //
        // Alpha does NOT answer this question: a blended surface is still
        // rasterised opaquely into the shadow map, so a translucent plume throws a
        // hard shadow until this says otherwise. Read on the GPRIM, not the shader
        // — two prims sharing one material can disagree about casting, and
        // `material:binding` is not the place to say so.
        no_shadow_cast,
        ..default()
    })
}

/// A DEM terrain owns a procedural mesh even when the initial USD projection has
/// no `UsdGeomGprim` shape to queue. Keep its standard USD appearance intent on
/// the owner so the later static mesh assembly is immediately renderable. This is
/// deliberately restricted to the explicit terrain API plus DEM asset modes; an
/// arbitrary Xform must not acquire a material merely because it may gain geometry
/// from another subsystem.
fn is_procedural_terrain_visual_owner(
    reader: &dyn read::UsdReadObject,
    sdf_path: &SdfPath,
) -> bool {
    reader.has_api_schema(sdf_path, "LunCoTerrainAPI")
        && matches!(
            reader.text(sdf_path, "lunco:assetMode").as_deref(),
            Some("dem") | Some("layered")
        )
}

/// Attach one prepared PBR intent together with a ready mesh.
fn apply_standard_material(
    reader: &dyn read::UsdReadObject,
    sdf_path: &SdfPath,
    mesh_handle: &Handle<Mesh>,
    entity_cmd: &mut EntityCommands,
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
) -> Result<(), MaterialReadError> {
    let look = read_standard_material(reader, sdf_path, asset_server, stage_id)?;
    entity_cmd.try_insert((Mesh3d(mesh_handle.clone()), look));
    Ok(())
}

/// Attach the authored PBR intent before its CPU mesh is available.
///
/// `PbrLook` is an appearance contract independent of geometry. Keeping it on
/// the USD entity during mesh preparation lets render binding and simulation
/// observe the authored look without waiting for a worker result.
fn apply_standard_material_intent(
    reader: &dyn read::UsdReadObject,
    sdf_path: &SdfPath,
    entity_cmd: &mut EntityCommands,
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
) -> Result<(), MaterialReadError> {
    let look = read_standard_material(reader, sdf_path, asset_server, stage_id)?;
    entity_cmd.try_insert(look);
    Ok(())
}

/// Attach the generic script/driver programs a prim carries to `entity`.
///
/// Program resolution happens before the one-program-per-owner check. Modelica
/// facets in a `CollectionAPI:components` network are owned by their domain
/// projection; they are not generic script siblings.
/// This is the boundary that prevents a physical network's component count from
/// becoming a false duplicate-program diagnostic.
fn attach_programs<R: UsdRead>(
    reader: &R,
    owner: &SdfPath,
    entity: Entity,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    commands: &mut Commands,
) {
    let network_members = program::modelica_network_member_paths(reader);
    let mut candidates: Vec<_> = reader
        .children(owner)
        .into_iter()
        .filter(|child| reader.is_active(child))
        .filter(|child| reader.has_api_schema(child, "LunCoProgramAPI"))
        .collect();
    // Intrinsic behavior is authored directly on a physical prim. A separable
    // program is a child Scope and is attached by its owner above, never to the
    // implementation Scope itself.
    if reader.type_name(owner).as_deref() != Some("Scope")
        && reader.has_api_schema(owner, "LunCoProgramAPI")
    {
        candidates.push(owner.clone());
    }

    let mut programs = Vec::new();
    for child in candidates {
        // Collection membership is the explicit ownership transfer to the
        // generated Modelica network. Its source is validated by the domain
        // projector, so it must not enter this generic executor at all.
        if network_members.contains(child.as_str()) {
            continue;
        }
        let resolved = match program::resolve_program(reader, &child) {
            Ok(resolved) => resolved,
            Err(issue) => {
                warn!(
                    "[usd] program {} is unresolved at {}: {}",
                    child.as_str(),
                    issue.property,
                    issue.message
                );
                continue;
            }
        };
        if program::is_generic_program_backend(resolved.backend) {
            programs.push((child, resolved));
        }
    }

    if programs.len() > 1 {
        warn!(
            "[usd] {} has {} generic executable LunCoProgramAPI children; one generic program per owner is the runtime contract, so none was attached",
            owner.as_str(),
            programs.len()
        );
        return;
    }
    for (child, resolved) in programs {
        // A program's parameters are typed attributes on its own program prim, one
        // per key — `float lunco:param:width = 1.05`. Read by `param(me, key,
        // default)`, which is how one reusable program drives many prims, each from
        // its own numbers.
        //
        // A Rust driver reads them the same way, from `ScriptParams`, and NOT off the
        // reader: a driver is an ordinary Bevy system, and a system has no USD reader.
        // Everything a driver needs must be projected into the ECS here, at load. That
        // is why the f64-ness of `ScriptParams` binds drivers too — a colour cannot
        // ride through it, and belongs in a bound `Material` regardless.
        let params: std::collections::HashMap<String, f64> = reader
            .attr_names(&child)
            .iter()
            .filter_map(|name| {
                let key = name.strip_prefix("lunco:param:")?;
                Some((key.to_string(), reader.real(&child, name)?))
            })
            .collect();
        if !params.is_empty() {
            commands
                .entity(entity)
                .try_insert(lunco_core::ScriptParams(params));
        } else {
            commands.entity(entity).remove::<lunco_core::ScriptParams>();
        }

        // Remember WHICH program this scenario came from. The script runs for the
        // owner, but its source belongs to the program prim — that is where a live
        // edit is saved back to.
        commands
            .entity(entity)
            .try_insert(lunco_core::ScenarioProgramPrim(child.as_str().to_string()));

        commands.queue(move |world: &mut World| {
            lunco_usd_bevy_core::program::apply_program_resolution(
                world,
                entity,
                stage_id,
                Some(resolved),
            );
        });
    }
}

const MAX_USD_ANCESTRY_DEPTH: usize = 64;

/// Internal lifecycle marker for the one-time ECS re-expression performed by
/// [`detach_reset_xform_stack_prims`]. A reset prim may already be directly
/// under the stage root, so parent equality alone cannot distinguish "already
/// corrected" from "still carrying the root's authored transform".
#[derive(Component)]
struct ResetXformStackApplied;

/// True iff the prim's `xformOpOrder` **begins** with `!resetXformStack!`.
///
/// Position matters: UsdGeomXformable gives the sentinel meaning only as the
/// first entry — anywhere else it is a malformed stack, which
/// [`lunco_usd_bevy_core::compose_xform_order_at`] already rejects with
/// [`TransformReadError`].
fn prim_resets_xform_stack<R: UsdRead>(reader: &R, path: &SdfPath) -> bool {
    lunco_usd_bevy_core::read_xform_op_order(reader, path).is_some_and(|order| {
        order
            .first()
            .is_some_and(|op| op == lunco_usd_bevy_core::RESET_XFORM_STACK)
    })
}

/// Re-anchor every `!resetXformStack!` prim onto its stage's world frame.
///
/// UsdGeomXformable: a prim whose op order opens with the sentinel is
/// **world-anchored, not parent-relative** — its local-to-world is its own op
/// stack and nothing else. A projection that leaves it parented under its USD
/// parent silently multiplies the ancestor chain back in, which is the exact
/// value the author asked to drop (the canonical use is a prop authored
/// underneath a moving rig that must nonetheless stay put in the world).
///
/// The anchor is the topmost ancestor still belonging to the SAME stage: the
/// entity that carries this stage's world frame in the projection. Reparenting
/// to that entity rather than to nothing keeps the stage's own placement — a
/// twin mounted into a grid at a spawn pose stays inside its twin, and the
/// grid/`big_space` frame contract is preserved: a prim directly below the
/// nested scene Grid remains grid-direct, while deeper descendants remain
/// ordinary children. What the sentinel drops
/// is everything USD-authored between the prim and that root, which is the whole
/// of the ancestor chain the stage itself defines.
///
/// Idempotent: the applied marker prevents re-expressing the local matrix on
/// every frame, while a prim spawned before its ancestry finished materialising
/// remains unmarked and gets fixed when the root appears.
fn detach_reset_xform_stack_prims(
    mut commands: Commands,
    q_reset: Query<
        (Entity, &UsdPrimPath, &ChildOf, &Transform),
        (With<UsdResetXformStack>, Without<ResetXformStackApplied>),
    >,
    q_prims: Query<(&UsdPrimPath, Option<&ChildOf>, &Transform)>,
    q_grids: Query<&big_space::prelude::Grid>,
) {
    for (entity, prim, child_of, local) in q_reset.iter() {
        let mut anchor = None;
        let mut node = child_of.parent();
        for _ in 0..MAX_USD_ANCESTRY_DEPTH {
            let Ok((ancestor, ancestor_parent, _)) = q_prims.get(node) else {
                // Above the stage's own prims — the grid/mount anchor. Whatever
                // we found last is the stage root.
                break;
            };
            if ancestor.stage_handle != prim.stage_handle {
                // A different stage mounted above this one; its frame is the
                // mount, not this stage's world.
                break;
            }
            anchor = Some(node);
            match ancestor_parent {
                Some(p) => node = p.parent(),
                None => break,
            }
        }
        let Some(anchor) = anchor else {
            // Already a child of something that is not a USD prim of this stage
            // ⇒ nothing of the stage's chain is being applied. Nothing to drop.
            continue;
        };
        let Ok((_, _, stage_root_local)) = q_prims.get(anchor) else {
            continue;
        };
        // The stage root entity carries the authored transform of the root
        // prim.  Reparenting below it would therefore still apply that
        // transform, even though USD's reset sentinel drops *the entire* USD
        // ancestor stack.  Keep the entity under the stage root (so its mount
        // and big-space frame remain intact), but express this prim's local
        // matrix in the root entity's frame.
        let root_inverse = stage_root_local.to_matrix().inverse();
        let reset_local = Transform::from_matrix(root_inverse * local.to_matrix());
        info!(
            "[usd-bevy] {} opens with {} — detaching from its USD ancestry \
             onto the stage world frame ({anchor:?})",
            prim.path,
            lunco_usd_bevy_core::RESET_XFORM_STACK,
        );
        let mut entity_commands = commands.entity(entity);
        if let Ok(grid) = q_grids.get(anchor) {
            let (cell, local_translation) =
                grid.translation_to_grid(reset_local.translation.as_dvec3());
            entity_commands.try_insert((
                ChildOf(anchor),
                Transform {
                    translation: local_translation,
                    ..reset_local
                },
                cell,
                ResetXformStackApplied,
            ));
        } else {
            entity_commands.try_insert((ChildOf(anchor), reset_local, ResetXformStackApplied));
            entity_commands.try_remove::<CellCoord>();
        }
    }
}

#[cfg(test)]
mod reset_xform_stack_tests {
    use super::*;

    #[test]
    fn reset_ignores_stage_root_authored_transform_but_keeps_mount() {
        let mut app = App::new();
        app.add_systems(Update, detach_reset_xform_stack_prims);

        let mount = app.world_mut().spawn((Transform::default(),)).id();
        let stage = Handle::<UsdStageAsset>::default();
        let root = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Root".into(),
                },
                Transform::from_xyz(10.0, 0.0, 0.0),
                ChildOf(mount),
            ))
            .id();
        let parent = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Root/Parent".into(),
                },
                Transform::from_xyz(20.0, 0.0, 0.0),
                ChildOf(root),
            ))
            .id();
        let reset = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage,
                    path: "/Root/Parent/Reset".into(),
                },
                Transform::from_xyz(3.0, 0.0, 0.0),
                UsdResetXformStack,
                ChildOf(parent),
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<ChildOf>(reset).unwrap().parent(), root);
        let local = app.world().get::<Transform>(reset).unwrap();
        assert!((local.translation.x + 7.0).abs() < 1e-5);
    }

    #[test]
    fn reset_corrects_a_direct_stage_root_child_once() {
        let mut app = App::new();
        app.add_systems(Update, detach_reset_xform_stack_prims);

        let mount = app.world_mut().spawn((Transform::default(),)).id();
        let stage = Handle::<UsdStageAsset>::default();
        let root = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Root".into(),
                },
                Transform::from_xyz(10.0, 0.0, 0.0),
                ChildOf(mount),
            ))
            .id();
        let reset = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage,
                    path: "/Root/Reset".into(),
                },
                Transform::from_xyz(3.0, 0.0, 0.0),
                UsdResetXformStack,
                ChildOf(root),
            ))
            .id();

        app.update();
        let first = app.world().get::<Transform>(reset).unwrap().translation.x;
        assert!((first + 7.0).abs() < 1e-5);
        assert!(app.world().get::<ResetXformStackApplied>(reset).is_some());

        app.update();
        let second = app.world().get::<Transform>(reset).unwrap().translation.x;
        assert!((second - first).abs() < 1e-5);
    }
}

/// Dimensions are decoded by `lunco-usd-bevy-scene`, the shared owner used by
/// both the visual mesh and physics collider paths.
/// Rendering-only provenance for a USD built-in primitive mesh. The dimensions
/// remain owned by [`ShapeDims`] so a quality change can rebuild the mesh without
/// reopening the USD stage or duplicating the dimension reader.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct UsdPrimitiveMesh(ShapeDims);

/// Rendering-only marker for a USD curve mesh. The authored curve remains
/// addressable through [`UsdPrimPath`], so a Graphics quality change can rebuild
/// the mesh from the composed stage without duplicating USD geometry data in ECS.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
struct UsdCurveMesh;

/// Build one USD primitive's visual mesh from its resolved dimensions and the
/// current Graphics quality profile. USD has no attribute for these tessellation
/// counts; they are viewer policy, unlike the shape dimensions.
fn build_primitive_mesh(
    shape: ShapeDims,
    quality: lunco_render::RenderQualityProfile,
) -> Option<Mesh> {
    if quality.primitive_sphere_longitudes < 3
        || quality.primitive_sphere_latitudes < 2
        || quality.primitive_radial_segments < 3
        || quality.primitive_capsule_longitudes < 3
        || quality.primitive_capsule_latitudes < 2
    {
        return None;
    }

    match shape {
        ShapeDims::Cube { size } => Some(Cuboid::new(size as f32, size as f32, size as f32).into()),
        ShapeDims::Sphere { radius } => Some(Sphere::new(radius as f32).mesh().uv(
            quality.primitive_sphere_longitudes,
            quality.primitive_sphere_latitudes,
        )),
        ShapeDims::Cylinder { radius, height } => Some(
            Cylinder::new(radius as f32, height as f32)
                .mesh()
                .resolution(quality.primitive_radial_segments)
                .into(),
        ),
        ShapeDims::Cone { radius, height } => Some(
            Cone::new(radius as f32, height as f32)
                .mesh()
                .resolution(quality.primitive_radial_segments)
                .into(),
        ),
        ShapeDims::Capsule { radius, height } => Some(
            Capsule3d::new(radius as f32, (height / 2.0) as f32)
                .mesh()
                .latitudes(quality.primitive_capsule_latitudes)
                .longitudes(quality.primitive_capsule_longitudes)
                .into(),
        ),
        ShapeDims::Plane { width, length } => Some(
            Plane3d::default()
                .mesh()
                .size(width as f32, length as f32)
                .into(),
        ),
    }
}

/// Rebuild built-in USD primitive meshes when the user changes Graphics quality.
/// Dimensions are retained in [`UsdPrimitiveMesh`], so this is change-driven and
/// does not repeat USD traversal or touch physics colliders.
fn retessellate_primitive_meshes_on_quality_change(
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<(&UsdPrimitiveMesh, &Mesh3d, Option<&Name>)>,
    quality: Res<lunco_render::RenderingQualitySettings>,
) {
    if !quality.is_changed() {
        return;
    }
    let profile = match quality.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!(
                "[usd-bevy] invalid Graphics primitive quality; retaining current meshes: {reason}"
            );
            return;
        }
    };
    for (primitive, handle, name) in &q {
        let Some(mesh) = build_primitive_mesh(primitive.0, profile) else {
            warn!(
                "[usd-bevy] {} primitive mesh quality is invalid; retaining the previous mesh",
                name.map(|n| n.as_str()).unwrap_or("<unnamed>")
            );
            continue;
        };
        let Some(mut slot) = meshes.get_mut(&handle.0) else {
            continue;
        };
        *slot = mesh;
    }
}

/// Rebuild USD curve meshes when authored USD geometry or Graphics tessellation
/// changes. The live-stage revision is the generic invalidation signal for
/// authored curve points, topology, and widths; no route or waypoint knowledge
/// belongs in this renderer-owned path. Invalid settings leave the existing mesh
/// in place and are reported; no lower quality profile is selected implicitly.
fn refresh_curve_meshes_on_stage_or_quality_change(
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<(&UsdPrimPath, &Mesh3d, Option<&Name>), With<UsdCurveMesh>>,
    quality: Res<lunco_render::RenderingQualitySettings>,
    stage_revision: Res<UsdStageRevision>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
) {
    if !quality.is_changed() && !stage_revision.is_changed() {
        return;
    }
    let profile = match quality.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!("[usd-bevy] invalid Graphics curve quality; retaining current meshes: {reason}");
            return;
        }
    };
    for (prim_path, handle, name) in &q {
        let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(prim_path.stage_handle.id(), stage_asset);
        let Ok(path) = SdfPath::new(&prim_path.path) else {
            continue;
        };
        let Some(mesh) = build_usd_curve_mesh(&reader, &path, profile) else {
            warn!(
                "[usd-bevy] {} curve quality is invalid or its authored curve cannot be tessellated; retaining the previous mesh",
                name.map(|n| n.as_str()).unwrap_or("<unnamed>")
            );
            continue;
        };
        let Some(mut slot) = meshes.get_mut(&handle.0) else {
            continue;
        };
        *slot = mesh;
    }
}

#[cfg(test)]
mod curve_mesh_quality_tests {
    use super::*;
    use lunco_usd_bevy_core::canonical::CanonicalStage;

    fn stage(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&lunco_usd_core::StageRecipe::from_source(
            "curve.usda",
            source,
        ))
        .expect("build curve stage")
    }

    #[test]
    fn curve_tube_density_follows_graphics_quality() {
        let stage = stage(
            r#"#usda 1.0
(
    metersPerUnit = 1
)
def BasisCurves "Tube"
{
    uniform token type = "cubic"
    uniform token basis = "catmullRom"
    int[] curveVertexCounts = [4]
    point3f[] points = [(0, 0, 0), (1, 0, 1), (2, 0, -1), (3, 0, 0)]
    float[] widths = [0.2]
}
"#,
        );
        let reader = stage.view();
        let path = SdfPath::new("/Tube").unwrap();
        let low = build_usd_curve_mesh(
            &reader,
            &path,
            lunco_render::RenderingQuality::Low.profile(),
        )
        .expect("low curve mesh");
        let high = build_usd_curve_mesh(
            &reader,
            &path,
            lunco_render::RenderingQuality::High.profile(),
        )
        .expect("high curve mesh");
        assert!(high.count_vertices() > low.count_vertices());
    }

    #[test]
    fn malformed_curve_structure_is_rejected_instead_of_guessed() {
        let stage = stage(
            r#"#usda 1.0
def NurbsCurves "MissingKnots"
{
    int[] curveVertexCounts = [3]
    int[] order = [3]
    point3f[] points = [(0, 0, 0), (1, 0, 1), (2, 0, 0)]
    float[] widths = [0.2]
}
def BasisCurves "WrongBasis"
{
    uniform token type = "cubic"
    uniform token basis = "bspline"
    int[] curveVertexCounts = [4]
    point3f[] points = [(0, 0, 0), (1, 0, 1), (2, 0, -1), (3, 0, 0)]
    float[] widths = [0.2]
}
"#,
        );
        let reader = stage.view();
        assert!(build_usd_curve_mesh(
            &reader,
            &SdfPath::new("/MissingKnots").unwrap(),
            lunco_render::RenderingQuality::Balanced.profile(),
        )
        .is_none());
        assert!(build_usd_curve_mesh(
            &reader,
            &SdfPath::new("/WrongBasis").unwrap(),
            lunco_render::RenderingQuality::Balanced.profile(),
        )
        .is_none());
    }

    #[test]
    fn rover_nurbs_antenna_geometry_is_tessellated_as_a_tube() {
        // These are the authored structures used by the Summer Space School
        // rover: a four-control-point order-four NurbsCurves prim with an
        // explicit diameter.  Keep this as a renderer-path regression rather
        // than replacing the curve with a special antenna mesh.
        let stage = stage(
            r#"#usda 1.0
def NurbsCurves "MagnetometerBoom"
{
    int[] curveVertexCounts = [4]
    int[] order = [4]
    double[] knots = [0, 0, 0, 0, 1, 1, 1, 1]
    point3f[] points = [(0, 0.20, -0.82), (0, 0.19, -1.34), (0, 0.20, -1.80), (0, 0.21, -2.16)]
    float[] widths = [0.025]
}
def NurbsCurves "FeedArm"
{
    int[] curveVertexCounts = [4]
    int[] order = [4]
    double[] knots = [0, 0, 0, 0, 1, 1, 1, 1]
    point3f[] points = [(0.56, 0.25, 0), (0.44, 0.44, 0), (0.16, 0.47, 0), (0, 0.38, 0)]
    float[] widths = [0.03]
}
"#,
        );
        let reader = stage.view();
        for path in ["/MagnetometerBoom", "/FeedArm"] {
            let mesh = build_usd_curve_mesh(
                &reader,
                &SdfPath::new(path).unwrap(),
                lunco_render::RenderingQuality::Balanced.profile(),
            )
            .unwrap_or_else(|| panic!("authored rover curve {path} must produce a tube"));
            assert!(mesh.count_vertices() > 0);
            assert!(mesh.indices().is_some(), "tube must have triangle indices");
        }
    }
}

#[cfg(test)]
mod primitive_mesh_quality_tests {
    use super::*;

    #[test]
    fn primitive_mesh_density_follows_graphics_quality() {
        let shape = ShapeDims::Sphere { radius: 1.0 };
        let low = build_primitive_mesh(shape, lunco_render::RenderingQuality::Low.profile())
            .expect("low-quality sphere mesh");
        let high = build_primitive_mesh(shape, lunco_render::RenderingQuality::High.profile())
            .expect("high-quality sphere mesh");
        assert!(
            high.count_vertices() > low.count_vertices(),
            "primitive mesh quality must control sphere tessellation density"
        );
    }

    #[test]
    fn invalid_primitive_mesh_quality_is_rejected() {
        let mut quality = lunco_render::RenderingQuality::Balanced.profile();
        quality.primitive_radial_segments = 2;
        assert!(build_primitive_mesh(
            ShapeDims::Cylinder {
                radius: 1.0,
                height: 2.0
            },
            quality
        )
        .is_none());
    }
}

/// A `Mesh` prim's normals, rotated into the canonical frame (`n' = Q·n`) — a
/// direction, so never scaled. `primvars:normals` wins over the typed `normals`
/// attribute (UsdGeomPointBased gives the primvar precedence); the returned name
/// says which was read so the caller can resolve its interpolation/indices.
/// `None` when unauthored (the caller then computes flat normals).
fn read_mesh_normals(
    reader: &impl UsdRead,
    path: &SdfPath,
) -> Option<(Vec<[f32; 3]>, &'static str)> {
    // `points3` for the same reason as `points` above — `normal3d[]` is legal, and a
    // strict read of it means "unauthored", which here silently swaps authored
    // shading normals for computed flat ones (a faceted look, not an error).
    let (normals, attr) = {
        let pv = reader.points3(path, "primvars:normals");
        if pv.is_empty() {
            (reader.points3(path, "normals"), "normals")
        } else {
            (pv, "primvars:normals")
        }
    };
    if normals.is_empty() {
        return None;
    }
    let conv = stage_convention(reader).ok()?;
    if conv.is_identity() {
        return Some((normals, attr));
    }
    Some((
        normals
            .into_iter()
            .map(|n| conv.dir(Vec3::from_array(n)).to_array())
            .collect(),
        attr,
    ))
}

/// Build a mesh from a `UsdGeomBasisCurves` or `UsdGeomNurbsCurves` prim.
///
/// An unoriented curve with `widths` is a swept tube whose widths are diameters.
/// An oriented curve with standard `normals` is a flat ribbon whose widths are
/// strip widths. Both paths share the same centerline evaluation and are kept
/// here as one generic USD geometry reader.
///
/// Batches are honoured: `curveVertexCounts` partitions `points` into several
/// curves on one prim, and each is swept and merged into a single mesh so the
/// prim keeps its 1:1 entity mapping.
///
/// `widths` interpolation follows USD: one value is `constant`, otherwise it is
/// per-vertex. Absent `widths` means an infinitely thin curve, which has no
/// surface — returns `None` rather than inventing a radius, so a curve authored
/// as a pure path (a camera rail) does not silently become a visible pipe.
fn build_usd_curve_mesh(
    reader: &impl UsdRead,
    path: &SdfPath,
    quality: lunco_render::RenderQualityProfile,
) -> Option<Mesh> {
    use bevy::math::DVec3;
    use lunco_usd_bevy_camera::camera_path::CurveBasis;
    use lunco_usd_geometry::curve_sweep::sweep_tube;
    use lunco_usd_geometry::ribbon::{build_ribbon_mesh, RibbonPoint};

    // Canonical-frame points — same conversion the mesh path takes.
    let points = read_usd_mesh_points(reader, path)?;
    if points.is_empty() {
        return None;
    }
    if points
        .iter()
        .any(|point| point.iter().any(|value| !value.is_finite()))
    {
        error!(
            "[usd-bevy] {} has non-finite authored curve control points",
            path.as_str()
        );
        return None;
    }
    // No `widths` ⇒ no surface. Deliberately not defaulted: see the doc above.
    let widths = match read::read_curve_real_array(reader, path, gtok::A_WIDTHS) {
        Ok(Some(widths)) if !widths.is_empty() => widths,
        Ok(Some(_)) | Ok(None) => return None,
        Err(_) => {
            error!(
                "[usd-bevy] {} has authored curve widths with an unsupported value type",
                path.as_str()
            );
            return None;
        }
    };

    // USD `widths` means a ribbon width when the curve is oriented by its
    // standard `normals` attribute, and a tube diameter otherwise. Keep that
    // distinction at the generic USD geometry boundary so authored route,
    // camera, and annotation curves all receive the same interpretation.
    let oriented_normals = read_mesh_normals(reader, path).map(|(normals, _)| {
        normals
            .into_iter()
            .map(Vec3::from_array)
            .collect::<Vec<_>>()
    });
    if let Some(normals) = &oriented_normals {
        if normals.len() != points.len() || normals.iter().any(|normal| !normal.is_finite()) {
            error!(
                "[usd-bevy] {} has curve normals that do not match its point topology",
                path.as_str()
            );
            return None;
        }
    }

    // Radii are a LENGTH, so they scale with `metersPerUnit` — `conv.length`,
    // not `conv.point`. (`read_usd_mesh_points` already converted the centerline.)
    let conv = stage_convention(reader).ok()?;
    let radii: Vec<f32> = widths
        .iter()
        .map(|w| conv.length(*w / 2.0) as f32)
        .collect();
    if radii.iter().any(|r| !r.is_finite() || *r <= 0.0) {
        error!(
            "[usd-bevy] {} has curve widths that are not finite and positive",
            path.as_str()
        );
        return None;
    }

    // `NurbsCurves` carries its own basis: per-curve `order`, a concatenated
    // `knots` array, and optional rational `pointWeights`. `BasisCurves` carries a
    // `type`/`basis` token pair instead. Both are swept identically once each
    // curve is reduced to a centerline — the only difference is how that
    // centerline is produced.
    let is_nurbs = reader.type_name(path).as_deref() == Some(gtok::T_NURBS_CURVES);
    let counts = match read::read_curve_int_array(
        reader,
        path,
        openusd::schemas::geom::tokens::A_CURVE_VERTEX_COUNTS,
    ) {
        Ok(Some(counts)) if !counts.is_empty() => counts,
        Ok(Some(_)) | Ok(None) => {
            error!(
                "[usd-bevy] {} has no usable authored curveVertexCounts; USD requires this topology field",
                path.as_str()
            );
            return None;
        }
        Err(_) => {
            error!(
                "[usd-bevy] {} has authored curveVertexCounts with an unsupported value type",
                path.as_str()
            );
            return None;
        }
    };
    let total_control_points: usize = counts
        .iter()
        .filter_map(|count| usize::try_from(*count).ok())
        .sum();
    if total_control_points != points.len() || counts.iter().any(|count| *count < 2) {
        error!(
            "[usd-bevy] {} has curveVertexCounts inconsistent with its points",
            path.as_str()
        );
        return None;
    }
    if widths.len() != 1 && widths.len() != counts.len() && widths.len() != points.len() {
        error!(
            "[usd-bevy] {} has {} widths for {} curves and {} points; expected constant, uniform, or vertex widths",
            path.as_str(),
            widths.len(),
            counts.len(),
            points.len()
        );
        return None;
    }

    let (basis, periodic, orders, all_knots, point_weights) = if is_nurbs {
        let orders =
            match read::read_curve_int_array(reader, path, openusd::schemas::geom::tokens::A_ORDER)
            {
                Ok(Some(orders)) if !orders.is_empty() => orders,
                Ok(Some(_)) | Ok(None) => {
                    error!(
                        "[usd-bevy] {} has no usable authored NurbsCurves order",
                        path.as_str()
                    );
                    return None;
                }
                Err(_) => {
                    error!(
                    "[usd-bevy] {} has authored NurbsCurves order with an unsupported value type",
                    path.as_str()
                );
                    return None;
                }
            };
        if orders.len() != 1 && orders.len() != counts.len() {
            error!(
                "[usd-bevy] {} has {} NurbsCurves orders for {} curves",
                path.as_str(),
                orders.len(),
                counts.len()
            );
            return None;
        }
        let all_knots = match read::read_curve_real_array(
            reader,
            path,
            openusd::schemas::geom::tokens::A_KNOTS,
        ) {
            Ok(Some(knots)) if !knots.is_empty() => knots,
            Ok(Some(_)) | Ok(None) | Err(_) => {
                error!(
                    "[usd-bevy] {} has no usable authored NurbsCurves knots",
                    path.as_str()
                );
                return None;
            }
        };
        let point_weights = match read::read_curve_real_array(
            reader,
            path,
            openusd::schemas::geom::tokens::A_POINT_WEIGHTS,
        ) {
            Ok(Some(weights)) if weights.len() == points.len() => weights,
            Ok(Some(_)) => {
                error!(
                    "[usd-bevy] {} has pointWeights whose length does not match points",
                    path.as_str()
                );
                return None;
            }
            Ok(None) => Vec::new(),
            Err(_) => {
                error!(
                    "[usd-bevy] {} has authored pointWeights with an unsupported value type",
                    path.as_str()
                );
                return None;
            }
        };
        (CurveBasis::Linear, false, orders, all_knots, point_weights)
    } else {
        let ty = match read::read_curve_token(
            reader,
            path,
            openusd::schemas::geom::tokens::A_TYPE,
            "cubic",
            &["linear", "cubic"],
        ) {
            Ok(token) => token,
            Err(_) => return None,
        };
        let basis = if ty == "linear" {
            CurveBasis::Linear
        } else {
            match read::read_curve_token(
                reader,
                path,
                openusd::schemas::geom::tokens::A_BASIS,
                "bezier",
                &["bezier", "catmullRom"],
            ) {
                Ok(token) if token == "bezier" => CurveBasis::Bezier,
                Ok(_) => CurveBasis::CatmullRom,
                Err(_) => return None,
            }
        };
        let wrap = match read::read_curve_token(
            reader,
            path,
            openusd::schemas::geom::tokens::A_WRAP,
            "nonperiodic",
            &["nonperiodic", "periodic", "pinned"],
        ) {
            Ok(wrap) => wrap,
            Err(_) => return None,
        };
        (
            basis,
            wrap == "periodic",
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    };

    if is_nurbs {
        let expected_knots = counts
            .iter()
            .enumerate()
            .try_fold(0usize, |total, (index, count)| {
                let count = usize::try_from(*count).ok()?;
                let order = orders
                    .get(index)
                    .or_else(|| orders.first())
                    .and_then(|order| usize::try_from(*order).ok())?;
                total.checked_add(count.checked_add(order)?)
            });
        if expected_knots != Some(all_knots.len()) {
            error!(
                "[usd-bevy] {} has {} NurbsCurves knots, expected {:?}",
                path.as_str(),
                all_knots.len(),
                expected_knots
            );
            return None;
        }
    }

    if quality.curve_samples_per_segment == 0
        || (oriented_normals.is_none() && quality.curve_radial_segments < 3)
    {
        return None;
    }

    let mut merged: Option<Mesh> = None;
    let mut cursor = 0usize;
    // `knots` is one flat array for the whole batch: curve `i` owns
    // `vertexCount_i + order_i` of them, consumed in order. Tracked separately
    // from the point cursor because the strides differ.
    let mut knot_cursor = 0usize;
    let curve_count = counts.len();
    for (ci, c) in counts.into_iter().enumerate() {
        let Ok(n) = usize::try_from(c) else {
            error!(
                "[usd-bevy] {} has a negative curveVertexCounts entry",
                path.as_str()
            );
            return None;
        };
        if n < 2 || n > points.len().saturating_sub(cursor) {
            error!(
                "[usd-bevy] {} has curve topology outside its points",
                path.as_str()
            );
            return None;
        }
        // Captured before `cursor` advances — `pointWeights` is indexed by control
        // point, so it slices with the same offset the points did.
        let cursor_start = cursor;
        let cvs: Vec<Vec3> = points[cursor..cursor + n]
            .iter()
            .map(|p| Vec3::from_array(*p))
            .collect();
        let seg_radii: Vec<f32> = if radii.len() == 1 {
            vec![radii[0]]
        } else if radii.len() == curve_count {
            vec![radii[ci]]
        } else {
            radii
                .iter()
                .skip(cursor)
                .take(n)
                .copied()
                .collect::<Vec<_>>()
        };
        let seg_normals = oriented_normals.as_ref().map(|normals| {
            normals[cursor..cursor + n]
                .iter()
                .copied()
                .collect::<Vec<_>>()
        });
        cursor += n;

        let centerline: Vec<Vec3> = if is_nurbs {
            // Per-curve order; a single authored value applies to the whole batch.
            let Some(order) = orders
                .get(ci)
                .or_else(|| orders.first())
                .and_then(|order| usize::try_from(*order).ok())
                .filter(|order| *order >= 2 && *order <= n)
            else {
                error!(
                    "[usd-bevy] {} has an invalid NurbsCurves order",
                    path.as_str()
                );
                return None;
            };
            let need = n + order;
            let knot_end = knot_cursor.checked_add(need)?;
            if knot_end > all_knots.len() {
                error!(
                    "[usd-bevy] {} has insufficient NurbsCurves knots",
                    path.as_str()
                );
                return None;
            }
            let knots = all_knots[knot_cursor..knot_end].to_vec();
            knot_cursor = knot_end;
            let w: Vec<f64> = if point_weights.is_empty() {
                Vec::new()
            } else {
                point_weights[cursor_start..cursor_start + n].to_vec()
            };
            let steps = (n.saturating_sub(1)).max(1) * quality.curve_samples_per_segment;
            let pts: Vec<[f32; 3]> = cvs.iter().map(|p| p.to_array()).collect();
            let sampled =
                lunco_usd_geometry::nurbs::sample_nurbs_curve(&pts, &w, order, &knots, steps);
            if sampled.is_empty() {
                error!(
                    "[usd-bevy] {} has a NurbsCurves segment that cannot be evaluated",
                    path.as_str()
                );
                return None;
            }
            sampled.into_iter().map(Vec3::from_array).collect()
        } else if basis == CurveBasis::Linear {
            cvs.clone()
        } else {
            let steps = (n.saturating_sub(1)).max(1) * quality.curve_samples_per_segment;
            let Some(samples) = (0..=steps)
                .map(|i| {
                    lunco_usd_bevy_camera::camera_path::eval_curve(
                        &cvs,
                        basis,
                        periodic,
                        i as f32 / steps as f32,
                    )
                })
                .collect::<Option<Vec<_>>>()
            else {
                error!(
                    "[usd-bevy] {} has a BasisCurves segment that cannot be evaluated",
                    path.as_str()
                );
                return None;
            };
            samples
        };
        // Resampling changes the point count, so per-vertex radii must be
        // resampled with it or a tapered tube would snap back to its control-point
        // radii. Constant width (len 1) passes straight through.
        let seg_radii = if seg_radii.len() <= 1 || centerline.len() == cvs.len() {
            seg_radii
        } else {
            let last = cvs.len() - 1;
            (0..centerline.len())
                .map(|i| {
                    let t = i as f32 / (centerline.len() - 1).max(1) as f32 * last as f32;
                    let (a, f) = (t.floor() as usize, t.fract());
                    let b = (a + 1).min(last);
                    seg_radii[a] * (1.0 - f) + seg_radii[b] * f
                })
                .collect()
        };

        let segment_mesh = if let Some(seg_normals) = seg_normals {
            let seg_normals = if centerline.len() == cvs.len() {
                seg_normals
            } else {
                let last = cvs.len() - 1;
                (0..centerline.len())
                    .map(|i| {
                        let t = i as f32 / (centerline.len() - 1).max(1) as f32 * last as f32;
                        let (a, f) = (t.floor() as usize, t.fract());
                        let b = (a + 1).min(last);
                        seg_normals[a].lerp(seg_normals[b], f).normalize_or_zero()
                    })
                    .collect()
            };
            let points = centerline
                .iter()
                .zip(seg_normals)
                .map(|(position, normal)| RibbonPoint {
                    position: position.as_dvec3(),
                    normal: normal.as_dvec3(),
                })
                .collect::<Vec<_>>();
            build_ribbon_mesh(&points, DVec3::ZERO, &seg_radii, 0.0, periodic)?
        } else {
            sweep_tube(
                &centerline,
                &seg_radii,
                quality.curve_radial_segments,
                periodic,
            )?
        };
        merged = Some(match merged {
            None => segment_mesh,
            Some(mut acc) => {
                acc.merge(&segment_mesh).ok()?;
                acc
            }
        });
    }
    if is_nurbs && knot_cursor != all_knots.len() {
        return None;
    }
    merged
}

/// Build a mesh from a `UsdGeomNurbsPatch` prim.
///
/// A patch is a tensor-product rational surface: a `uVertexCount × vVertexCount`
/// control net with a knot vector and order per direction. It is how USD spells
/// every surface of revolution — which for HAB-1 is **80.4% of the habitat's
/// vertices** (261 lathe objects plus the ellipsoidal dome), and the only way to
/// express a *partial* revolution at all, since `Cylinder`/`Sphere`/`Cone` are
/// complete revolutions with no sweep-angle parameter.
///
/// Normals are analytic (`uder × vder`), not face-averaged — exact at the poles
/// and seams where averaging creases, which is precisely the dome apex.
///
/// **`trimCurve:*` IS honoured** — see [`lunco_usd_geometry::trim`]. A trimmed patch gets an
/// irregular triangulation of its surviving domain instead of a lattice, which is
/// what puts a genuine arched doorway in a wall.
///
/// A malformed authored trim definition refuses the patch. Rendering the
/// untrimmed surface would add geometry the USD scene explicitly removed.
///
/// (This paragraph previously said trimming was unimplemented and silently
/// ignored. It was stale, and it cost a debugging session: the claim was taken at
/// face value while the code underneath was working, so a missing surface was
/// blamed on trim support that in fact existed. A doc comment that describes a
/// capability the code no longer lacks is worse than no comment.)
fn has_authored_nurbs_trim(reader: &impl UsdRead, path: &SdfPath) -> bool {
    [
        "trimCurve:counts",
        "trimCurve:orders",
        "trimCurve:vertexCounts",
        "trimCurve:knots",
        "trimCurve:points",
        "trimCurve:ranges",
    ]
    .into_iter()
    .any(|attr| reader.has_authored_attribute(path, attr))
}

/// Read a `NurbsPatch` prim's definition — either GENERATED from a
/// `lunco:lathe:*` profile, or read from the authored control arrays.
///
/// This is the single place the two spellings of "what surface is this" meet, and
/// they are mutually exclusive by design: a prim that declares a lathe profile does
/// not author `points`, because the whole point of the parametric form is that the
/// control net is derived. Authoring both is the duplication that let the engine
/// bell's drawn contour (effective exponent ≈1.3) drift away from the contour its
/// own Modelica model declared (0.55) with nothing to catch it.
fn read_patch_surface(
    reader: &impl UsdRead,
    path: &SdfPath,
) -> Option<(lathe::NurbsSurface, Option<lathe::UsdLathe>)> {
    // Applying the parametric API is the ownership decision: its profile is the
    // only source of the surface. An empty/unknown profile is therefore an
    // invalid parametric definition, not permission to resurrect a competing
    // authored control net. Falling through here used to make a profile typo
    // render stale or unrelated `points` data and violated the schema's explicit
    // "unknown = no surface" contract.
    if reader.has_api_schema(path, "LunCoLatheAPI") {
        let l = lathe::read_lathe(reader, path)?;
        return Some((l.surface()?, Some(l)));
    }

    let points = read_usd_mesh_points(reader, path)?;
    let u_count = lathe::read_required_nurbs_int(reader, path, gtok::A_U_VERTEX_COUNT)?;
    let v_count = lathe::read_required_nurbs_int(reader, path, gtok::A_V_VERTEX_COUNT)?;
    let u_order = lathe::read_required_nurbs_int(reader, path, gtok::A_U_ORDER)?;
    let v_order = lathe::read_required_nurbs_int(reader, path, gtok::A_V_ORDER)?;
    if u_count < u_order || v_count < v_order {
        error!(
            "[usd-bevy] {} has NurbsPatch order/count mismatch: u {u_count}/{u_order}, v {v_count}/{v_order}",
            path.as_str()
        );
        return None;
    }
    let u_knots = match read::read_curve_real_array(reader, path, gtok::A_U_KNOTS) {
        Ok(Some(knots)) if knots.len() == u_count + u_order => knots,
        Ok(Some(_)) | Ok(None) | Err(_) => {
            error!(
                "[usd-bevy] {} has no usable authored uKnots for its NurbsPatch",
                path.as_str()
            );
            return None;
        }
    };
    let v_knots = match read::read_curve_real_array(reader, path, gtok::A_V_KNOTS) {
        Ok(Some(knots)) if knots.len() == v_count + v_order => knots,
        Ok(Some(_)) | Ok(None) | Err(_) => {
            error!(
                "[usd-bevy] {} has no usable authored vKnots for its NurbsPatch",
                path.as_str()
            );
            return None;
        }
    };
    let weights = match read::read_curve_real_array(reader, path, gtok::A_POINT_WEIGHTS) {
        Ok(Some(weights)) if weights.len() == points.len() => weights,
        Ok(Some(_)) => {
            error!(
                "[usd-bevy] {} has pointWeights whose length does not match its NurbsPatch points",
                path.as_str()
            );
            return None;
        }
        Ok(None) => Vec::new(),
        Err(_) => {
            error!(
                "[usd-bevy] {} has pointWeights with an unsupported value type",
                path.as_str()
            );
            return None;
        }
    };
    let orientation = match read::read_curve_token(
        reader,
        path,
        "orientation",
        "rightHanded",
        &["rightHanded", "leftHanded"],
    ) {
        Ok(orientation) => orientation == "leftHanded",
        Err(_) => return None,
    };

    Some((
        lathe::NurbsSurface {
            points,
            weights,
            u_count: u_count as u32,
            v_count: v_count as u32,
            u_order: u_order as u32,
            v_order: v_order as u32,
            u_knots,
            v_knots,
            left_handed: orientation,
        },
        None,
    ))
}

#[cfg(test)]
mod parametric_surface_tests {
    use super::*;

    #[test]
    fn lathe_api_owns_surface_even_when_profile_is_invalid() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "lathe.usda",
            r#"#usda 1.0
def NurbsPatch "Nozzle" (
    prepend apiSchemas = ["LunCoLatheAPI"]
)
{
    uniform token lunco:lathe:profile = "typo"
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
    int uVertexCount = 2
    int vVertexCount = 2
    int uOrder = 2
    int vOrder = 2
}
"#,
        );
        let stage = canonical::CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Nozzle").unwrap();
        assert!(
            read_patch_surface(&stage.view(), &path).is_none(),
            "an invalid parametric profile must not fall through to authored points"
        );
    }

    #[test]
    fn lathe_api_rejects_invalid_profile_parameters_without_clamping_them() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "lathe.usda",
            r#"#usda 1.0
def NurbsPatch "Nozzle" (
    prepend apiSchemas = ["LunCoLatheAPI"]
)
{
    uniform token lunco:lathe:profile = "paraboloid"
    float lunco:lathe:focalLength = 0
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
    int uVertexCount = 2
    int vVertexCount = 2
    int uOrder = 2
    int vOrder = 2
}
"#,
        );
        let stage = canonical::CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Nozzle").unwrap();
        assert!(
            read_patch_surface(&stage.view(), &path).is_none(),
            "an invalid focal length must not be replaced with a tiny denominator"
        );
    }

    #[test]
    fn lathe_api_requires_standard_sampling_fields() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "lathe.usda",
            r#"#usda 1.0
def NurbsPatch "Nozzle" (
    prepend apiSchemas = ["LunCoLatheAPI"]
)
{
    uniform token lunco:lathe:profile = "bell"
    float lunco:lathe:throatRadius = 0.35
    float lunco:lathe:exitRadius = 1.35
    float lunco:lathe:length = 1.90
    float lunco:lathe:contour = 0.55
}
"#,
        );
        let stage = canonical::CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Nozzle").unwrap();
        assert!(
            read_patch_surface(&stage.view(), &path).is_none(),
            "a parametric patch must author its standard sampling fields"
        );
    }

    #[test]
    fn authored_patch_requires_standard_sampling_fields() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "patch.usda",
            r#"#usda 1.0
def NurbsPatch "Patch"
{
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
}
"#,
        );
        let stage = canonical::CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Patch").unwrap();
        assert!(
            read_patch_surface(&stage.view(), &path).is_none(),
            "an authored patch must not receive renderer sampling defaults"
        );
    }

    #[test]
    fn authored_patch_requires_authored_knot_vectors() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "patch.usda",
            r#"#usda 1.0
def NurbsPatch "Patch"
{
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
    int uVertexCount = 2
    int vVertexCount = 2
    int uOrder = 2
    int vOrder = 2
}
"#,
        );
        let stage = canonical::CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Patch").unwrap();
        assert!(
            read_patch_surface(&stage.view(), &path).is_none(),
            "a patch must not receive guessed clamped knot vectors"
        );
    }

    #[test]
    fn authored_trim_data_cannot_fall_back_to_an_untrimmed_patch() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "patch.usda",
            r#"#usda 1.0
def NurbsPatch "Patch"
{
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
    int uVertexCount = 2
    int vVertexCount = 2
    int uOrder = 2
    int vOrder = 2
    float[] uKnots = [0, 0, 1, 1]
    float[] vKnots = [0, 0, 1, 1]
    int[] trimCurve:counts = [1]
}
"#,
        );
        let stage = canonical::CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Patch").unwrap();
        assert!(
            build_usd_nurbs_patch_mesh(
                &stage.view(),
                &path,
                lunco_render::RenderingQuality::Balanced.profile()
            )
            .is_none(),
            "partial authored trim data must refuse the patch instead of restoring its hole"
        );
    }
}

/// Build a `NurbsPatch`'s mesh AND the definition to retain alongside it.
///
/// The returned [`lathe::NurbsSurface`] is `None` for a TRIMMED patch. That is
/// deliberate: a trim loop lives in the patch's own `(u, v)` parameter space, and
/// re-deriving the trimmed triangulation from an edited control net is a different
/// problem from retessellating an untrimmed one. Withholding the component means a
/// trimmed patch is simply not live-editable, rather than editable-but-wrong — the
/// trim would silently stop matching the surface it cuts.
fn build_usd_nurbs_patch_mesh(
    reader: &impl UsdRead,
    path: &SdfPath,
    quality: lunco_render::RenderQualityProfile,
) -> Option<(Mesh, Option<(lathe::NurbsSurface, Option<lathe::UsdLathe>)>)> {
    use bevy::asset::RenderAssetUsages;
    use bevy_mesh::PrimitiveTopology;

    let (surface, lathe_params) = read_patch_surface(reader, path)?;
    let points = surface.points.clone();
    let weights = surface.weights.clone();
    let u_count = surface.u_count as usize;
    let v_count = surface.v_count as usize;
    let u_order = surface.u_order as usize;
    let v_order = surface.v_order as usize;
    let u_knots = surface.u_knots.clone();
    let v_knots = surface.v_knots.clone();

    // ── Trim curves ─────────────────────────────────────────────────────────
    // `trimCurve:*` IS applied — see `lunco_usd_geometry::trim`. A trimmed patch gets an
    // irregular triangulation of its surviving domain instead of a lattice.
    //
    // Two things that used to block this are handled there rather than guessed:
    // USD never states the keep/discard winding rule, so classification is
    // even-odd with the domain rectangle as an implicit outer loop
    // (orientation-independent); and the geometry crate handles constraint
    // crossings without panicking, so
    // loops are inserted with `add_constraint_and_split` rather than gated with
    // `can_add_constraint` — gating would silently drop part of a loop and leave
    // the hole with a missing side.
    let trim_authored = has_authored_nurbs_trim(reader, path);
    let trim_loops = if !trim_authored {
        None
    } else {
        let counts = match read::read_curve_int_array(reader, path, "trimCurve:counts") {
            Ok(Some(counts)) if !counts.is_empty() => counts,
            _ => {
                error!(
                    "[usd-bevy] {} has malformed trimCurve:counts; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let orders = match read::read_curve_int_array(reader, path, "trimCurve:orders") {
            Ok(Some(orders)) => orders,
            _ => {
                error!(
                    "[usd-bevy] {} has malformed trimCurve:orders; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let vertex_counts = match read::read_curve_int_array(reader, path, "trimCurve:vertexCounts")
        {
            Ok(Some(vertex_counts)) => vertex_counts,
            _ => {
                error!(
                    "[usd-bevy] {} has malformed trimCurve:vertexCounts; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let tknots = match read::read_curve_real_array(reader, path, "trimCurve:knots") {
            Ok(Some(tknots)) if !tknots.is_empty() => tknots,
            _ => {
                error!(
                    "[usd-bevy] {} has malformed trimCurve:knots; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let tpoints = reader.points3(path, "trimCurve:points");
        if tpoints.is_empty() {
            error!(
                "[usd-bevy] {} has malformed trimCurve:points; refusing the patch",
                path.as_str()
            );
            return None;
        }
        let ranges = match read::read_double2_array_strict(reader, path, "trimCurve:ranges") {
            Ok(Some(ranges)) => ranges,
            Ok(None) => Vec::new(),
            Err(_) => {
                error!(
                    "[usd-bevy] {} has malformed trimCurve:ranges; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };

        let u_span = [u_knots[u_order - 1], u_knots[u_count]];
        let v_span = [v_knots[v_order - 1], v_knots[v_count]];
        let loops = lunco_usd_geometry::trim::assemble_loops(
            &counts,
            &orders,
            &vertex_counts,
            &tknots,
            &ranges,
            &tpoints,
            u_span,
            v_span,
            quality.nurbs_trim_curve_samples,
        );
        if loops.is_empty() {
            error!(
                "[usd-bevy] {} has authored trimCurve data but no usable loop",
                path.as_str()
            );
            return None;
        }
        Some(loops)
    };

    if let Some(loops) = trim_loops {
        let grid = quality.nurbs_trim_subdivisions(u_count.max(v_count));
        bevy::log::info!(
            "[usd-bevy] {} trimming: {} loop(s), grid {}",
            path.as_str(),
            loops.loops.len(),
            grid
        );
        let Some(domain) = lunco_usd_geometry::trim::triangulate_trimmed(&loops, grid) else {
            error!(
                "[usd-bevy] {} authored trim could not be triangulated; refusing the patch",
                path.as_str()
            );
            return None;
        };
        bevy::log::info!(
            "[usd-bevy] {} trimmed domain: {} verts, {} tris",
            path.as_str(),
            domain.uvs.len(),
            domain.indices.len() / 3
        );
        let samples = lunco_usd_geometry::nurbs::sample_nurbs_patch_at(
            &points,
            &weights,
            u_count,
            v_count,
            u_order,
            v_order,
            &u_knots,
            &v_knots,
            &domain.uvs,
        );
        if samples.is_empty() {
            error!(
                "[usd-bevy] {} authored trim produced no surface samples; refusing the patch",
                path.as_str()
            );
            return None;
        }
        let mut positions = Vec::with_capacity(samples.len());
        let mut normals = Vec::with_capacity(samples.len());
        let mut uvs = Vec::with_capacity(samples.len());
        for s in &samples {
            positions.push(s.position);
            normals.push(s.normal);
            uvs.push(s.uv);
        }
        let mut indices = domain.indices;
        lathe::flip_if_left_handed(surface.left_handed, &mut normals, &mut indices);
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(bevy_mesh::Indices::U32(indices));
        // No `NurbsSurface` for a trimmed patch — see the fn doc.
        return Some((mesh, None));
    }

    // The untrimmed build now lives on `NurbsSurface` itself, because it is
    // EXACTLY the operation the regeneration system has to perform when a parameter
    // changes. Keeping a second copy here would be two tessellators that can
    // disagree — the same trap `lunco_usd_geometry::nurbs`' module doc describes for evaluators.
    let Some(mesh) = surface.mesh(quality) else {
        // `sample_nurbs_patch_at` has already warned WHICH guard fired; this
        // adds the prim path, which it has no way to know.
        bevy::log::warn!(
            "[usd-bevy] {} untrimmed patch produced no samples — no mesh",
            path.as_str()
        );
        return None;
    };
    // Parity with the trimmed branch above, which logs its vert/tri counts. The
    // untrimmed branch used to be completely SILENT, so a patch that reached
    // here and built correctly was indistinguishable in the log from one whose
    // prim was never traversed at all. Telling those two apart is exactly what
    // you need when a surface is missing from the render, and not being able to
    // is what made the HAB-1 dome expensive to diagnose.
    bevy::log::info!(
        "[usd-bevy] {} untrimmed patch: {}x{} net{}, {} verts",
        path.as_str(),
        surface.u_count,
        surface.v_count,
        match &lathe_params {
            Some(l) => format!(" (lathed, {:?})", l.profile),
            None => String::new(),
        },
        mesh.count_vertices()
    );
    Some((mesh, Some((surface, lathe_params))))
}

/// Build a Bevy [`Mesh`] from a native USD `Mesh` prim (UsdGeomMesh):
/// `point3f[] points`, `int[] faceVertexCounts`, `int[] faceVertexIndices`,
/// with optional `normal3f[] normals` and `texCoord2f[] primvars:st`.
///
/// Polygons are **fan-triangulated** into an *unindexed* triangle list — one
/// vertex per face-corner — so per-face-varying normals/uvs need no welding
/// and quads/n-gons render directly. Attribute interpolation is inferred by
/// array length: `== points.len()` → per-vertex (indexed by point), `==
/// faceVertexIndices.len()` → per-face-varying (indexed by corner); any other
/// length is ignored. `orientation = "leftHanded"` flips the winding (USD
/// default is right-handed = CCW, which matches Bevy). Missing `normals` are
/// computed flat; missing `primvars:st` get a zeroed UV set so the standard /
/// shader material paths don't choke.
///
/// Returns `None` if the required topology attributes are absent/empty or the
/// indices reference out-of-range points (malformed mesh). Rendering only —
/// native-mesh **colliders** are still the glTF side-channel's job
/// (see `resolver.rs` `TODO(glb-composability)`).
pub fn build_usd_mesh(reader: &impl UsdRead, path: &SdfPath) -> Option<Mesh> {
    use bevy::asset::RenderAssetUsages;
    // `bevy_mesh`, NOT `bevy::render::render_resource` — the latter is a
    // re-export through `bevy_render` (wgpu + naga). `bevy_mesh` depends only on
    // `wgpu-types`, so naming the topology here costs no GPU stack.
    // See docs/architecture/render-decoupling.md.
    use bevy_mesh::PrimitiveTopology;

    // Canonical-frame points/normals (Y-up, metres); identity for our stages.
    // Topology is decoded by the render-free scene contract so physics and
    // visual projection consume the same authored mesh facts.
    let topology = read_usd_mesh_topology(reader, path)?;
    let points = topology.points;
    let counts = topology.face_vertex_counts;
    let indices = topology.face_vertex_indices;

    // Optional vertex attributes. `primvars:st` is THE UV channel — the
    // `primvars:st0` / bare `st` spellings are gone. A UV set is a primvar, so it
    // is namespaced; a bare `st` is not one, and accepting it let a mesh carry UVs
    // in a form no other DCC binds.
    let normals = read_mesh_normals(reader, path).map(|(values, _source)| values);
    // `points2`, NOT `scalar::<Vec<[f32; 2]>>`: Maya and Houdini export
    // `texCoord2d[]`, Blender exports `texCoord2f[]`. A strict `2f` read of a `2d` UV
    // set yields "no UVs", and the documented response to that is a ZEROED UV set —
    // so the mesh samples its texture entirely at (0,0) and renders as one flat
    // colour. That misreads as a material/texture bug, which is the wrong place to
    // look. `None` (rather than empty) keeps the `uvs_per_vertex`/`per_corner` logic
    // below unchanged.
    let uvs = Some(reader.points2(path, "primvars:st")).filter(|v: &Vec<[f32; 2]>| !v.is_empty());

    let n_corners = indices.len();
    let normals_per_vertex = normals.as_ref().is_some_and(|n| n.len() == points.len());
    let normals_per_corner = normals.as_ref().is_some_and(|n| n.len() == n_corners);
    let uvs_per_vertex = uvs.as_ref().is_some_and(|u| u.len() == points.len());
    let uvs_per_corner = uvs.as_ref().is_some_and(|u| u.len() == n_corners);

    let left_handed = reader.text(path, "orientation").as_deref() == Some("leftHanded");

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(n_corners);
    let mut out_normals: Vec<[f32; 3]> = Vec::new();
    let mut out_uvs: Vec<[f32; 2]> = Vec::new();

    // Walk faces; `base` is the running offset of the face's first corner into
    // the flat `indices` (and per-corner attribute) arrays.
    let mut base = 0usize;
    for &count in &counts {
        let count = count as usize;
        if base + count > n_corners {
            return None; // counts/indices disagree → malformed
        }
        if count >= 3 {
            // Fan: triangle (0, k, k+1) for k in 1..count-1.
            for k in 1..count - 1 {
                let tri = if left_handed {
                    [0, k + 1, k]
                } else {
                    [0, k, k + 1]
                };
                for local in tri {
                    let corner = base + local;
                    let vidx = indices[corner] as usize;
                    if vidx >= points.len() {
                        return None; // index out of range → malformed
                    }
                    positions.push(points[vidx]);
                    if normals_per_vertex {
                        out_normals.push(normals.as_ref().unwrap()[vidx]);
                    } else if normals_per_corner {
                        out_normals.push(normals.as_ref().unwrap()[corner]);
                    }
                    if uvs_per_vertex {
                        out_uvs.push(uvs.as_ref().unwrap()[vidx]);
                    } else if uvs_per_corner {
                        out_uvs.push(uvs.as_ref().unwrap()[corner]);
                    }
                }
            }
        }
        base += count;
    }
    if positions.is_empty() {
        return None;
    }

    let have_normals = out_normals.len() == positions.len();
    let have_uvs = out_uvs.len() == positions.len();

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    if have_normals {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, out_normals);
    }
    if have_uvs {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, out_uvs);
    } else {
        // ShaderMaterial / StandardMaterial both expect a UV channel.
        let zero = vec![[0.0f32, 0.0]; mesh.count_vertices()];
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, zero);
    }
    if !have_normals {
        // Unindexed triangle soup → flat per-face normals.
        mesh.compute_flat_normals();
    }
    Some(mesh)
}

#[cfg(test)]
mod instance_identity_tests {
    //! Gap G2/B.1: descendants of a runtime-spawned USD instance must derive a
    //! hierarchical identity from the instance root, so two spawns of the same
    //! asset (identical composed prim paths) don't collide.
    use super::*;
    use lunco_core::{identity::derive_id, GlobalEntityId, Provenance};

    #[test]
    fn preview_projection_is_local_even_when_source_has_a_content_path() {
        assert_eq!(
            usd_projection_provenance(
                true,
                false,
                Some("assets/scenes/parts.usda".into()),
                "/Parts",
            ),
            Some(Provenance::Local)
        );
        assert_eq!(
            usd_projection_provenance(
                false,
                true,
                Some("assets/scenes/parts.usda".into()),
                "/Parts/Wheel",
            ),
            Some(Provenance::Local)
        );
        assert_eq!(
            usd_projection_provenance(
                false,
                false,
                Some("assets/scenes/parts.usda".into()),
                "/Parts/Wheel",
            ),
            Some(Provenance::Content {
                namespace: "usd".into(),
                source: "assets/scenes/parts.usda".into(),
                path: "/Parts/Wheel".into(),
            })
        );
    }

    #[test]
    fn role_is_path_relative_to_root() {
        assert_eq!(instance_role("/SolarPanel", "/SolarPanel/Frame"), "Frame");
        assert_eq!(
            instance_role("/SolarPanel", "/SolarPanel/Frame/Bolt"),
            "Frame/Bolt"
        );
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
                    UsdInstanceMember {
                        root,
                        root_path: "/Rover".into(),
                    },
                    UsdPrimPath {
                        stage_handle: Handle::default(),
                        path: "/Rover/Wheel_FL".into(),
                    },
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
        assert_eq!(
            pa,
            Provenance::Derived {
                parent: 1001,
                role: "Wheel_FL".into()
            }
        );
        assert_eq!(
            pb,
            Provenance::Derived {
                parent: 2002,
                role: "Wheel_FL".into()
            }
        );

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
                UsdInstanceMember {
                    root,
                    root_path: "/Rover".into(),
                },
                UsdPrimPath {
                    stage_handle: Handle::default(),
                    path: "/Rover/Wheel_FL".into(),
                },
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

#[cfg(test)]
mod mesh_tests {
    //! Native UsdGeomMesh → Bevy [`Mesh`] decode ([`build_usd_mesh`]).
    use super::*;
    use lunco_usd_bevy_core::canonical::CanonicalStage;
    use openusd::sdf::Path as SdfPath;

    /// Build a real composed stage. The extractors read through `StageView` — the
    /// live, PCP-composed stage — which is the ONLY read path now that the
    /// Runtime reads come from the live canonical stage. Tests read what the app reads.
    fn parse(usda: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&lunco_usd_core::StageRecipe::from_source("t.usda", usda))
            .expect("build canonical stage")
    }

    /// A single quad fan-triangulates to 2 tris (6 unindexed verts); per-vertex
    /// `primvars:st` carries through and missing normals are computed.
    #[test]
    fn quad_triangulates_with_uvs_and_computed_normals() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Quad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0),(0,1,0)]\n\
             int[] faceVertexCounts = [4]\n\
             int[] faceVertexIndices = [0,1,2,3]\n\
             texCoord2f[] primvars:st = [(0,0),(1,0),(1,1),(0,1)]\n}\n",
        );
        let reader = __cs.view();
        let mesh = build_usd_mesh(&reader, &SdfPath::new("/Quad").unwrap()).expect("mesh built");
        assert_eq!(mesh.count_vertices(), 6, "one quad → two triangles");
        assert!(
            mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some(),
            "st preserved"
        );
        assert!(
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some(),
            "normals computed"
        );
    }

    /// Two triangles, no optional attrs → 6 verts, a zeroed UV set, flat normals.
    #[test]
    fn bare_triangles_get_default_uvs() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Tris\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(0,1,0),(1,1,0)]\n\
             int[] faceVertexCounts = [3,3]\n\
             int[] faceVertexIndices = [0,1,2,1,3,2]\n}\n",
        );
        let reader = __cs.view();
        let mesh = build_usd_mesh(&reader, &SdfPath::new("/Tris").unwrap()).expect("mesh built");
        assert_eq!(mesh.count_vertices(), 6);
        assert!(
            mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some(),
            "zeroed UVs inserted"
        );
    }

    /// Missing topology attributes → `None` (caller falls back to no mesh).
    #[test]
    fn missing_topology_returns_none() {
        let __cs = parse("#usda 1.0\ndef Mesh \"Empty\"\n{\n}\n");
        let reader = __cs.view();
        assert!(build_usd_mesh(&reader, &SdfPath::new("/Empty").unwrap()).is_none());
    }

    /// An index pointing past the end of `points` is rejected, not panicked on.
    #[test]
    fn out_of_range_index_is_rejected() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Bad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0)]\n\
             int[] faceVertexCounts = [3]\n\
             int[] faceVertexIndices = [0,1,9]\n}\n",
        );
        let reader = __cs.view();
        assert!(build_usd_mesh(&reader, &SdfPath::new("/Bad").unwrap()).is_none());
    }
}

#[cfg(test)]
mod wrap_tests {
    //! `UsdUVTexture` wrap-token → Bevy sampler address-mode mapping.
    use super::*;
    use bevy::image::ImageAddressMode;

    #[test]
    fn usd_wrap_tokens_map_to_address_modes() {
        assert_eq!(
            usd_wrap_to_address(Some("clamp"), "inputs:wrapS").expect("clamp"),
            ImageAddressMode::ClampToEdge
        );
        assert_eq!(
            usd_wrap_to_address(Some("mirror"), "inputs:wrapS").expect("mirror"),
            ImageAddressMode::MirrorRepeat
        );
        assert_eq!(
            usd_wrap_to_address(Some("black"), "inputs:wrapS").expect("black"),
            ImageAddressMode::ClampToBorder
        );
        assert_eq!(
            usd_wrap_to_address(Some("repeat"), "inputs:wrapS").expect("repeat"),
            ImageAddressMode::Repeat
        );
        // "useMetadata" and absent both fall back to Repeat.
        assert_eq!(
            usd_wrap_to_address(Some("useMetadata"), "inputs:wrapS").expect("metadata"),
            ImageAddressMode::Repeat
        );
        assert_eq!(
            usd_wrap_to_address(None, "inputs:wrapS").expect("absent"),
            ImageAddressMode::Repeat
        );
        assert!(usd_wrap_to_address(Some("invalid"), "inputs:wrapS").is_err());
    }
}
