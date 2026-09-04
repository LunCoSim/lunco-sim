//! # LunCoSim USD → Bevy Visual Sync
//!
//! Responsible for spawning child entities for USD prims and attaching visual components
//! (meshes, materials, transforms). This is the **first** plugin in the USD processing
//! pipeline — it must run before the Avian physics and Sim simulation plugins.
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
//! single reader and marks each projected entity with `UsdVisualSynced`.

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use big_space::prelude::CellCoord;
use lunco_usd_compose::parse_usda;
// Appearance **intent**, not a material: this crate must never name
// `MeshMaterial3d`/`StandardMaterial` (they live in `bevy_pbr` → wgpu + naga).
// `lunco-render-bevy` observes these and binds the real material.
// See docs/architecture/render-decoupling.md.
use lunco_materials::ProceduralSkybox;
use lunco_render::{PbrLook, PbrTextures, SurfaceAlpha};
pub use openusd::sdf::Path as SdfPath;
// `UsdData` remains the Send-safe authored-layer representation used by document
// authoring helpers. Initial runtime projection reads the prepared plan; live
// scene reads use `StageView` after an authored generation exists.
pub use openusd::sdf::Data as UsdData;
use openusd::sdf::Value;
use std::sync::Arc;

/// The standard `UsdGeomImageable.purpose` value resolved for a composed prim.
///
/// Physics and placement both need the same inherited purpose semantics. Keep
/// the reader at the USD boundary so downstream projections cannot drift into
/// separate path walks with different collision ownership rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Default,
    Render,
    Proxy,
    Guide,
}

/// Resolve the composed, inherited `UsdGeomImageable.purpose` value.
pub fn effective_purpose(reader: &dyn read::UsdReadObject, path: &SdfPath) -> Purpose {
    let mut cur = Some(path.clone());
    while let Some(p) = cur {
        if p.is_abs_root() {
            break;
        }
        match reader.text(&p, "purpose").as_deref() {
            Some("guide") => return Purpose::Guide,
            Some("proxy") => return Purpose::Proxy,
            Some("render") => return Purpose::Render,
            Some("default") => return Purpose::Default,
            _ => {}
        }
        cur = p.parent();
    }
    Purpose::Default
}

mod camera;
pub mod camera_mount;
pub mod camera_switch;
pub mod camera_track;
mod compose;
pub mod dome;
mod light;
/// Light and transform ports — the port backend for what `light`/`compose` spawn.
pub mod scene_ports;
pub use camera::{read_camera_exposure_ev100, CameraExposureError, UsdCameraPose, UsdSensorCamera};
pub use camera_switch::SetActiveCamera;
pub use light::{read_dome_intensity, read_intensity_with_exposure, DomeIntensity, LightReadError};
pub mod author;
pub mod camera_path;
pub mod canonical;
pub mod curve_sweep;
pub mod lathe;
pub mod mount;
pub mod nurbs;
pub mod program;
mod projection_plan;
pub mod read;
pub mod trim;
pub mod units;
pub mod usd_data;
pub mod variants;
pub mod view;
pub use canonical::{CanonicalStage, CanonicalStages, RawStageChange, StageProjector, StageRecipe};
#[cfg(not(target_arch = "wasm32"))]
pub use compose::{compose_file_to_stage, compose_file_to_stage_with_assets};
pub use light::UsdAuthoredLight;
pub use projection_plan::{UsdPrimProjectionPlan, UsdStageProjectionPlan};
pub use read::{AttrUiHint, UsdRead};
pub use units::{stage_convention, ConventionTransform, StageMetrics, UpAxis};
use usd_data::UsdDataExt;
pub use view::StageView;
// The ambient-fill solve. Uniform ambient is spelled as an untextured `DomeLight`
// and composed as a SUM, so a command that wants to set the composed TOTAL (the
// inspector's ambient slider) must solve for the one dome it owns. Exported
// because the WRITER lives in `lunco-scene-commands`, while the semantics — what
// counts as an ambient dome, and in what units — live here with the reader.
pub use light::{
    ambient_fill_intensity, ambient_fill_saturates, untextured_dome_intensity_sum,
    DOME_TEXTURE_ATTR,
};

/// Bevy plugin for USD visual synchronization.
///
/// Registers the `UsdStageAsset` type, the USD asset loader, and the `sync_usd_visuals`
/// system that processes USD prims into Bevy entities with meshes and transforms.
pub struct UsdBevyPlugin;

/// Systems in this set finish the bounded USD visual queue for the current
/// update. Consumers that expose a render-ready state must run after this set,
/// so a document generation cannot be reported ready between structural
/// projection and asynchronous CPU mesh commit.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct UsdVisualProjectionSet;

/// The authored camera pose and persistent origin projection are inserted into
/// BigSpace's propagation pipeline before floating-origin recentering and
/// high-precision propagation. Keeping these systems as sibling sets of
/// BigSpace's propagation phases is necessary because
/// `RecenterLargeTransforms` itself is a member of Bevy's `Propagate` set.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
struct CameraPathSet;

impl Plugin for UsdBevyPlugin {
    fn build(&self, app: &mut App) {
        // Failed-asset diagnostic labels load the shared fallback font on the
        // browser, so this standalone scene plugin also installs the common
        // network policy when no higher-level asset plugin is present.
        lunco_settings::ensure_download_settings(app);
        // USD mesh and light projection consumes the authoritative graphics
        // settings. Initialise the documented default at this boundary so
        // projectors never invent a separate quality profile.
        app.init_resource::<lunco_render::RenderingQualitySettings>();

        // `SetActiveCamera` (avatar-free camera switch). Registered here so a
        // static/headless USD world can switch cameras via the command bus/rhai
        // without pulling in the avatar plugin. The observer is generated +
        // wired by the `register_commands!` invocation at module scope below.
        register_all_commands(app);

        // The mission-time spine provides `WorldTime` (the world animation clock)
        // for `sample_usd_animation`. Guarded so a context that also adds it via
        // `CelestialPlugin` is fine; where neither celestial nor a real clock UI
        // runs, the spine still advances the world at the default 1× transport so
        // authored USD animation plays.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }

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
            .register_type::<camera_track::CameraTrack>()
            // The retained NurbsPatch definition + its parametric layer. Registered
            // (not merely derived) because registration is what makes them reachable
            // from a script: `set(id, "UsdLathe.profile.exit_radius", 1.6)` resolves
            // through `AppTypeRegistry` by short type path. An unregistered component
            // fails there with `unknown type`, which is exactly the error the old
            // `set(me, "NurbsPatch.points", ...)` actuator died on.
            .register_type::<lathe::NurbsSurface>()
            .register_type::<lathe::UsdLathe>()
            .register_type::<lathe::LatheProfile>()
            .init_resource::<DiagnosticLabelFont>()
            .init_resource::<DiagnosticLabelConfig>()
            // Guarantee the viewport substrate exists wherever these camera
            // systems run: `cycle_active_camera`/`reconcile_scene_viewport`
            // read `SceneViewport`, so a host that adds this plugin without
            // lunco-core's `register_core_resources` (e.g. a focused test app)
            // still has it. Idempotent — a no-op if core already registered it.
            .init_resource::<lunco_core::SceneViewport>()
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<UsdVisualProjectionSettings>()
            .init_resource::<lunco_core::TheLocalAvatar>()
            .init_resource::<camera_switch::ViewportCameraSelection>()
            .init_resource::<camera_switch::CameraSelectionStatus>()
            .init_resource::<camera_switch::CameraContractStatus>()
            .init_resource::<camera_switch::StandalonePresentationState>()
            .init_resource::<camera_switch::StandalonePresentationSettings>()
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
            .init_resource::<UsdStageRevision>()
            .add_systems(PreUpdate, bump_usd_stage_revision)
            .add_systems(Startup, load_diagnostic_label_font)
            .add_observer(on_usd_prim_added)
            .add_observer(on_cell_coord_added)
            .add_observer(light::on_usd_light_added)
            // Active-camera switch (avatar-free): the `SetActiveCamera` command
            // + `KeyC` cycle both fire the internal `ActivateCamera` trigger,
            // which enforces the one-active-window-camera invariant and updates
            // the persistent BigSpace origin tracker. Works in a static,
            // input-less world (the command path needs neither).
            .add_observer(camera_switch::on_activate_camera)
            .add_observer(camera_switch::on_request_local_avatar_view)
            // The viewport-camera reconciler: the SINGLE authority over
            // window-camera `is_active` + `viewport`. Reads `SceneViewport`
            // (bound camera + visibility + rect, written by the switch and the
            // workbench) and actuates it. Runs every frame so an explicitly
            // requested authored camera can be fulfilled after async projection.
            .add_systems(Update, camera_switch::cycle_active_camera)
            .configure_sets(
                PostUpdate,
                (
                    lunco_core::SceneViewportSet::Publish,
                    lunco_core::SceneViewportSet::Reconcile,
                )
                    .chain()
                    .before(bevy::camera::CameraUpdateSystems),
            )
            .add_systems(
                PostUpdate,
                (
                    camera_switch::reconcile_scene_viewport
                        .in_set(lunco_core::SceneViewportSet::Reconcile)
                        .before(camera_switch::update_camera_origin),
                    camera_switch::update_camera_selection_status
                        .after(camera_switch::reconcile_scene_viewport)
                        .run_if(camera_switch::camera_selection_status_changed),
                ),
            )
            .add_systems(
                lunco_core::SceneTeardown,
                camera_switch::reset_camera_selection,
            )
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
                retessellate_curve_meshes_on_quality_change
                    .after(retessellate_primitive_meshes_on_quality_change),
            )
            .add_systems(Update, camera_mount::resolve_camera_mounts)
            .add_systems(
                PostUpdate,
                camera_mount::follow_mounted_cameras
                    .before(bevy::transform::TransformSystems::Propagate),
            )
            // Camera paths (`UsdGeomBasisCurves` + `lunco:path:camera`). Sampled and
            // written once per RENDER frame, chained, before transform propagation.
            //
            // NOT on the fixed cadence, which is where this used to live. The sample
            // time comes from `ResolvedDomains`, which `lunco-time` fills in
            // `PreUpdate` — once per render frame — so a `FixedPostUpdate` driver
            // re-read the same `t` on multi-step frames and did not run at all on
            // zero-step ones. The render-rate interpolation that papered over it keyed
            // off `Time<Fixed>::overstep_fraction()`, a WALL-CLOCK residual, which made
            // offline recordings differ run to run. A path is an analytic function of
            // time and needs neither. See `camera_path`'s module doc.
            //
            // Ordering against `DomainResolveSet` is not spelled out because it cannot
            // be: that set lives in `PreUpdate`, and an `.after()` naming a set from
            // another schedule is silently vacuous — which is exactly how the old
            // `FixedPostUpdate` registration looked correct while ordering nothing.
            // `PostUpdate` runs after `PreUpdate` within the frame, so the sample sees
            // this frame's resolved clock by schedule order.
            .add_systems(
                Update,
                (
                    camera_path::resolve_camera_paths,
                    // After resolve, so an aim target that spawns on the very frame
                    // its path resolves binds immediately rather than one frame late.
                )
                    .chain(),
            )
            .configure_sets(
                PostUpdate,
                CameraPathSet
                    .in_set(bevy::transform::TransformSystems::Propagate)
                    .before(big_space::prelude::BigSpaceSystems::RecenterLargeTransforms),
            )
            .add_systems(
                PostUpdate,
                (
                    camera_path::drive_camera_paths,
                    camera_path::apply_camera_paths,
                )
                    .chain()
                    // The path is the complete pose owner. It runs after
                    // generic interaction interpolation and before BigSpace's
                    // recentering so the persistent origin tracker sees the
                    // same cell-local pose in this frame.
                    .in_set(CameraPathSet),
            )
            .add_systems(
                PostUpdate,
                camera_switch::update_camera_origin
                    .in_set(bevy::transform::TransformSystems::Propagate)
                    .after(CameraPathSet)
                    .before(big_space::prelude::BigSpaceSystems::RecenterLargeTransforms),
            )
            // HDRI environment: project an authored `DomeLight`'s equirect into
            // a cubemap and bind it to the cameras (`dome.rs`).
            .add_plugins(dome::DomePlugin)
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
                        .after(canonical::sync_canonical_stages),
                    process_queued_usd_visuals
                        .run_if(any_queued_usd_visuals)
                        .after(sync_usd_visuals)
                        .in_set(UsdVisualProjectionSet),
                    poll_pending_usd_meshes
                        .run_if(any_pending_usd_meshes)
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
                    fail_awaiting_stage_prims.run_if(
                        bevy::ecs::schedule::common_conditions::on_message::<
                            bevy::asset::AssetLoadFailedEvent<UsdStageAsset>,
                        >,
                    ),
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
            )
            // Per-frame USD animation: drive `UsdAnimated` transforms from authored
            // `timeSamples` at each entity's resolved domain time. After the domain
            // resolve so playheads/derived chains are current this frame; cheap
            // (query is empty without animated prims).
            .add_systems(
                Update,
                (
                    bind_animated_to_preview,
                    // Hot-reload: drop stale plans so the next `plan_usd_animation`
                    // re-derives topology against the new stage content.
                    clear_animation_plans_on_stage_reload.run_if(
                        bevy::ecs::schedule::common_conditions::on_message::<
                            AssetEvent<UsdStageAsset>,
                        >,
                    ),
                    // Derive each animated prim's `AnimationPlan` once (tier-1 memo),
                    // then sample values at `t` — both samplers read the cached plan.
                    plan_usd_animation,
                    (sample_usd_animation, sample_usd_material_animation)
                        .after(lunco_time::DomainResolveSet),
                )
                    .chain(),
            )
            // Editorial **camera track** (doc 35): a prim's `lunco:activeCamera`
            // timeSamples drive `SetActiveCamera` cuts over time. Same shape as
            // the animation funnel — bind to the preview domain, derive the key
            // plan once (re-derive on hot-reload), then sample the held camera at
            // `t` and fire a cut on change. Query empty for scenes with no track.
            .add_systems(
                Update,
                (
                    camera_switch::ensure_standalone_presentation,
                    camera_track::bind_camera_tracks_to_preview,
                    camera_track::clear_camera_track_plans_on_stage_reload.run_if(
                        bevy::ecs::schedule::common_conditions::on_message::<
                            AssetEvent<UsdStageAsset>,
                        >,
                    ),
                    camera_track::plan_camera_tracks,
                    camera_switch::validate_authored_camera_contract.run_if(
                        lunco_core::gate::tracked(
                            "usd::camera_contract",
                            camera_switch::camera_contract_inputs_changed,
                        ),
                    ),
                    camera_track::sample_camera_tracks.after(lunco_time::DomainResolveSet),
                )
                    .chain()
                    .after(sync_usd_visuals)
                    .after(UsdVisualProjectionSet),
            );
    }
}

// Generates `register_all_commands(app)` (register_type + add_observer for the
// listed command handlers). Called from `UsdBevyPlugin::build`.
lunco_core::register_commands!(
    camera_switch::on_set_active_camera,
    camera_switch::on_set_user_camera,
    camera_switch::on_observe_avatar,
    camera_switch::on_resume_camera_director,
    camera_path::camera_path_transport,
);

/// A Bevy Asset representing a loaded USD Stage.
///
/// Carries the worker-produced [`UsdStageProjectionPlan`] for initial composed
/// projection and, when available, the `Send` layer-closure [`StageRecipe`]
/// used to create the live canonical stage for authoring. The non-`Send`
/// canonical [`Stage`](openusd::usd::Stage) is never part of this asset;
/// initial hierarchy, transform, and material reads use the prepared plan.
#[derive(Asset, TypePath, Clone)]
pub struct UsdStageAsset {
    /// The `Send` layer-closure recipe shared by the prepared initial projection
    /// and the live canonical stage. It is absent only for an externally
    /// composed stage whose live canonical owner was supplied separately.
    pub recipe: Option<StageRecipe>,
    /// Structural hierarchy prepared from the composed stage at the async
    /// boundary. Every valid asset carries a plan, including externally composed
    /// stages created by [`Self::from_composed_stage`].
    pub projection_plan: Arc<UsdStageProjectionPlan>,
}

impl UsdStageAsset {
    /// Build an in-memory asset with the same prepared projection contract as
    /// the asynchronous loader. Tests and live-document adapters use this
    /// constructor so they cannot accidentally create a loaded-looking asset
    /// without the data required by initial visual materialisation.
    pub fn from_recipe(recipe: StageRecipe) -> anyhow::Result<Self> {
        let projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
        projection_plan.validate()?;
        Ok(Self {
            recipe: Some(recipe),
            projection_plan: Arc::new(projection_plan),
        })
    }

    /// Build an asset read surface from an already-composed stage supplied by a
    /// native adapter. The live stage remains owned by [`CanonicalStages`]; the
    /// asset receives only the same owned snapshot used by the async loader.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_composed_stage(stage: &openusd::usd::Stage) -> anyhow::Result<Self> {
        let projection_plan = UsdStageProjectionPlan::from_stage(stage)?;
        projection_plan.validate()?;
        Ok(Self {
            recipe: None,
            projection_plan: Arc::new(projection_plan),
        })
    }
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

        // Fetch the transitive layer closure, then compose and snapshot its
        // initial read surface before the asset crosses into Bevy. The live
        // `!Send` stage is opened by the canonical-stage owner from this same
        // recipe when authoring or incremental projection requires it.
        let recipe = compose::fetch_layer_closure(load_context, &root_asset_path, bytes).await?;
        Ok(UsdStageAsset::from_recipe(recipe)?)
    }

    fn extensions(&self) -> &[&str] {
        &["usda"]
    }
}

/// A USD layer's **raw source text**, read through the `AssetServer` without
/// composition.
///
/// Distinct from [`UsdStageAsset`], which carries the prepared composed read
/// surface and optional live-stage recipe: this is just the bytes of one
/// `.usda` layer, decoded to a `String`. E1b uses
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

/// Monotonic "the USD projection may have changed" signal.
///
/// USD is the source of truth and the ECS is its projection, so everything
/// derived *from* USD — a view-model, a wiring cache, a panel's graph — needs to
/// know when to re-derive. This resource is that one signal, and consumers gate
/// on it with `run_if(resource_changed::<UsdStageRevision>)`.
///
/// Why a counter and not a hash of the derived result: a revision is O(1) and
/// **cannot drift from the truth**, because it is stamped by the writers
/// themselves. A hash can only tell you *after* paying to produce the thing you
/// were deciding whether to produce — which is exactly the bug this replaces
/// (`produce_usd_canvas` spent 11 ms/frame building a graph and hashing it just
/// to discover the graph was unchanged). Keep hashes for assertions, never for
/// gates. See `docs/architecture/42-ui-frame-discipline.md` §6.
///
/// Bumped by [`bump_usd_stage_revision`] on prim spawn/despawn and stage asset
/// modification, and directly by the live-edit drain in `lunco-usd`
/// (`live_consume`) for `ApplyUsdOp` edits to already-spawned prims, which raise
/// no ECS-structural signal of their own.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsdStageRevision(pub u64);

impl UsdStageRevision {
    /// Mark the USD projection as changed. Consumers gated on
    /// `resource_changed::<UsdStageRevision>` re-derive on the next run.
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

/// Raise [`UsdStageRevision`] when the USD→ECS projection changes structurally.
///
/// Deliberately does NOT touch the resource when nothing happened: writing it
/// unconditionally would fire `resource_changed` every frame and defeat every
/// gate downstream.
pub fn bump_usd_stage_revision(
    mut rev: ResMut<UsdStageRevision>,
    added: Query<(), Added<UsdPrimPath>>,
    mut removed: RemovedComponents<UsdPrimPath>,
    mut stage_events: MessageReader<AssetEvent<UsdStageAsset>>,
) {
    let stage_changed = stage_events.read().any(|e| {
        matches!(
            e,
            AssetEvent::Modified { .. } | AssetEvent::LoadedWithDependencies { .. }
        )
    });
    // `removed.read()` must be drained unconditionally — an unread reader keeps
    // redelivering, and `||` short-circuiting past it is what let the old wiring
    // gate stay "structural" for frames after the fact.
    let any_removed = removed.read().next().is_some();
    if !added.is_empty() || any_removed || stage_changed {
        rev.bump();
    }
}

/// Marker component indicating that the entity's structural USD projection is
/// committed. Geometry may still be streaming through [`UsdVisualMeshPending`]
/// when a CPU-generated mesh is being built asynchronously.
///
/// Prevents the projection systems from re-processing the same entity on
/// subsequent frames and is the lifecycle signal consumed by physics.
#[derive(Component)]
pub struct UsdVisualSynced;

/// A NURBS visual whose CPU tessellation is running on Bevy's async compute
/// pool. Structural USD projection and physics may proceed while the mesh is
/// being built; the marker is removed when the render asset is committed.
///
/// This is an explicit loading phase, not a placeholder or a second loader:
/// the request is extracted from the live canonical stage once, and the task
/// only evaluates the already-owned [`lathe::NurbsSurface`] definition.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdVisualMeshPending;

/// Main-thread handle for one asynchronous CPU-generated USD mesh build.
///
/// The task owns only Send-safe extracted data. The live OpenUSD stage remains
/// on the main thread and is never captured by the worker.
#[derive(Component)]
pub struct PendingUsdMesh {
    task: Task<Option<Mesh>>,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    path: SdfPath,
    stage_generation: u64,
    profile: lunco_render::RenderQualityProfile,
}

/// Render entity selected by a simulation-side visual split.
///
/// A wheel keeps its USD entity as the physics owner, while the mesh belongs to
/// a child with the presentation transform. CPU-generated geometry may finish
/// after that split, so the mesh commit must use this explicit target instead of
/// assuming the USD entity is still renderable.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdVisualMeshTarget(pub Entity);

/// Marks the render target whose appearance is owned by a projected custom
/// shader. The marker is render-free so the async PBR commit can avoid adding a
/// second appearance intent without depending on the shader crate.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdVisualShaderBound;

/// The composed USD prim had malformed authored data and was intentionally not
/// projected into the visual/physics pipeline. Keeping this explicit failure
/// state prevents an identity/stale-transform retry from looking successful.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq)]
#[reflect(Component)]
pub struct UsdVisualSyncFailed(pub String);

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

/// Marker: this entity's local `Transform` is driven by USD `timeSamples` on its
/// xform ops (`xformOp:translate` / `xformOp:rotateXYZ` / `xformOp:scale`).
///
/// Stamped at instantiation (see [`prim_has_xform_time_samples`]) so the
/// per-frame [`sample_usd_animation`] sampler iterates **only** animated entities
/// (cheap query) rather than re-reading every prim. This is the entity half of
/// the doc-19 animation funnel; the time source is the `lunco-time` `WorldTime`
/// (world domain). Per-object / per-selection domains (a `TimeBinding` to a
/// driven `TimeDomain`) layer on top of this later (doc 19 — T5).
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct UsdAnimated;

/// Tier-1 RAM memo of an animated prim's **topology** — which channels carry
/// `timeSamples` and (for materials) the resolved bound-shader path.
///
/// The set of animated channels is a *structural* property of the composed
/// stage: it doesn't change frame to frame, only the sample time `t` does.
/// [`plan_usd_animation`] derives it **once** (when the entity's stage asset is
/// loaded) so the per-frame samplers ([`sample_usd_animation`] /
/// [`sample_usd_material_animation`]) skip the reader topology walks
/// (`has_xform_op_order`, `attr_has_time_samples`, `resolve_bound_shader`, …)
/// and go straight to the value read at `t`. Cleared on stage hot-reload so it
/// re-derives against the new content.
#[derive(Component, Debug, Clone)]
pub struct AnimationPlan {
    /// Parsed prim `SdfPath` (cached so the samplers skip the per-frame re-parse).
    pub path: SdfPath,
    /// Stage `timeCodesPerSecond` (constant per stage) — seconds × this = code.
    pub time_codes_per_second: f64,
    /// How this prim's local `Transform` is driven.
    pub xform: XformDrive,
    /// Whether `visibility` carries `timeSamples` (else the sampler skips it).
    pub visibility: bool,
    /// Material channels + resolved shader, when any color/opacity is animated.
    pub material: Option<MaterialPlan>,
}

/// The transform channel that drives an [`AnimationPlan`] prim's local pose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XformDrive {
    /// Authored `xformOpOrder` — recompose the whole stack honoring op order.
    OpOrder,
    /// No authored `xformOpOrder` (orderless ops contribute nothing, matching
    /// the static decode) — or no animated transform channels at all.
    None,
}

/// The resolved material-animation topology cached in an [`AnimationPlan`].
#[derive(Debug, Clone)]
pub struct MaterialPlan {
    /// Resolved bound-shader prim path, when the color/opacity lives on a shader.
    pub shader: Option<SdfPath>,
    /// Shader `inputs:diffuseColor` is animated.
    pub diffuse: bool,
    /// Geom `primvars:displayColor` is animated (only when `diffuse` is false).
    pub geom_color: bool,
    /// Shader `inputs:opacity` is animated.
    pub opacity: bool,
}

/// Marker placed on a USD scene root that exists purely to render a
/// preview thumbnail. Plugins that activate simulation side-effects on
/// USD prims (avatar cameras, vehicle FSW, wheel physics) should walk
/// each candidate prim's `ChildOf` ancestry and bail if any ancestor
/// carries this marker — preview-only stages must show geometry but
/// must not spawn cameras into the window or insert physics bodies
/// into the live world.
#[derive(Component, Default, Debug, Clone, Copy)]
pub struct UsdPreviewOnly;

/// Root of a live USD scene mount.
///
/// This marker belongs to the USD projection boundary because visual
/// synchronization must identify the ownership root without depending on the
/// simulation translator.  Additive mounts use the same root marker; the
/// shared [`lunco_core::SceneMountState`] decides which roots are still valid
/// after a replacement.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdSceneRoot;

/// Returns whether an entity belongs to an off-screen USD preview stage.
///
/// Preview stages still need normal USD geometry, transforms, and materials so
/// their viewport can render them. Their authored `Camera` prims, however,
/// must never become Bevy window cameras: the viewport owns the one camera
/// that renders the preview target. Walk `ChildOf` rather than testing only
/// the entity because cameras are normally descendants of the preview root.
/// The bounded walk also keeps malformed hierarchy data from hanging a
/// lifecycle or error path.
pub fn is_preview_only(
    entity: Entity,
    q_child_of: &Query<&ChildOf>,
    q_preview_only: &Query<(), With<UsdPreviewOnly>>,
) -> bool {
    let mut current = entity;
    for _ in 0..1024 {
        if q_preview_only.contains(current) {
            return true;
        }
        let Ok(parent) = q_child_of.get(current) else {
            return false;
        };
        current = parent.parent();
    }
    warn!(
        "[usd] preview hierarchy exceeded 1024 ancestors at {:?}",
        entity
    );
    false
}

/// Returns whether an entity belongs to a render-only USD preview hierarchy.
///
/// The root marker is the ownership boundary for a preview lease. Consumers
/// that reconcile an already-live entity use this same boundary instead of
/// inferring preview state from a name, stage handle, or missing physics
/// components.
pub fn is_preview_only_entity(world: &World, entity: Entity) -> bool {
    let mut current = entity;
    for _ in 0..1024 {
        if world.get::<UsdPreviewOnly>(current).is_some() {
            return true;
        }
        let Some(parent) = world.get::<ChildOf>(current).map(ChildOf::parent) else {
            return false;
        };
        current = parent;
    }
    warn!(
        "[usd] preview hierarchy exceeded 1024 ancestors at {:?}",
        entity
    );
    false
}

/// Marker placed on an entity whose `UsdPrimPath` was added before the
/// referenced `UsdStageAsset` finished loading. `sync_usd_visuals` moves it
/// into the bounded projection queue once the asset becomes available.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdAwaitingStage;

/// Marker for a prim whose USD stage is available but whose visual projection
/// is waiting for the bounded projection pass.
///
/// Keeping [`UsdAwaitingStage`] until structural projection is committed makes
/// the scene transaction complete only after the canonical hierarchy exists.
/// CPU-generated geometry has its own [`UsdVisualMeshPending`] phase and is
/// reported separately while it streams. The marker is also the queue
/// ownership fence; a replacement can discard the entity without a late
/// observer trying to project it.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdVisualProjectionQueued;

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

/// Prepared composed read data and identity scope for a referenced runtime instance.
///
/// The source asset is composed once on the asset worker. This remapped plan
/// lets every descendant read the same composed facts at its scene instance
/// path without reopening or repeatedly querying the live scene stage.
#[derive(Component, Debug, Clone)]
pub struct UsdInstanceProjection {
    /// The runtime instance root whose USD identity scopes this projection.
    /// It is assigned when the live-stage reconciliation creates the root and
    /// is inherited by every projected descendant.
    pub root: Option<Entity>,
    pub plan: Arc<UsdStageProjectionPlan>,
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

/// The **instance** an entity belongs to, named by its instance-root
/// [`GlobalEntityId`](lunco_core::GlobalEntityId).
///
/// This is THE disambiguator for any resolver that matches authored USD
/// prim-path strings to entities. Two runtime spawns of one asset compose
/// BYTE-IDENTICAL stage-relative paths (`/DescentLander`, `/DescentLander/Hull`,
/// …), so a resolver that matches on path alone binds across copies — a lander
/// flying on the other lander's model, a rover geared to the other rover's
/// rockers. Scope the match to the instance and the ambiguity is gone.
///
/// The instance-root GID is the right name for it: unique per spawn, identical
/// on every peer, and stable across entity churn (a descendant's id is
/// `derive_id(parent, role)`, a pure function of identity, so a hot-swapped
/// program re-resolves to the same endpoints). Returns:
/// - `Some(root_gid)` for a runtime instance — a descendant reports its
///   [`Provenance::Derived`](lunco_core::Provenance::Derived)`{ parent }` when
///   identity assignment has completed, or resolves the same root through its
///   durable [`UsdInstanceProjection`] while that projection is live; the root
///   itself (`Authoritative`, tagged [`UsdInstanceRoot`]) reports its own GID.
/// - `None` for authored scene prims, whose composed paths are already globally
///   unique, so they share one namespace safely.
///
/// `None` is also the answer in the one-frame window before identity is minted
/// (`assign_global_entity_ids`, PostUpdate). A resolver that runs every frame
/// until it succeeds simply DEFERS — a `None` key never equals a `Some` key, so
/// it can never mis-bind; a resolver gated on `Added` must also wake on
/// `Added<GlobalEntityId>` to pick the ids up (see `resolve_behavior_targets`).
pub fn instance_key(
    entity: Entity,
    q_provenance: &Query<&lunco_core::Provenance>,
    q_gid: &Query<&lunco_core::GlobalEntityId>,
    q_instance_root: &Query<(), With<UsdInstanceRoot>>,
    q_instance_projection: &Query<&UsdInstanceProjection>,
) -> Option<u64> {
    instance_key_from_projection(
        entity,
        q_provenance,
        q_gid,
        q_instance_root,
        q_instance_projection.get(entity).ok(),
    )
}

/// Resolve instance scope when the caller already fetched the entity's
/// projection as part of its primary query.
pub fn instance_key_from_projection(
    entity: Entity,
    q_provenance: &Query<&lunco_core::Provenance>,
    q_gid: &Query<&lunco_core::GlobalEntityId>,
    q_instance_root: &Query<(), With<UsdInstanceRoot>>,
    projection: Option<&UsdInstanceProjection>,
) -> Option<u64> {
    match q_provenance.get(entity) {
        Ok(lunco_core::Provenance::Derived { parent, .. }) => Some(*parent),
        _ => projection
            .and_then(|projection| projection.root)
            .and_then(|root| q_gid.get(root).map(|gid| gid.get()).ok())
            .or_else(|| {
                q_instance_root
                    .contains(entity)
                    .then(|| q_gid.get(entity).map(|gid| gid.get()).ok())
                    .flatten()
            }),
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
            commands
                .entity(entity)
                .try_insert((UsdVisualSyncFailed(error.to_string()), Visibility::Hidden));
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
                commands
                    .entity(entity)
                    .try_insert((UsdVisualSyncFailed(message.clone()), Visibility::Hidden));
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
                .try_insert((UsdVisualSynced, Visibility::Hidden));
            return;
        }

        // Get prim type (Cube, Cylinder, Sphere, etc.)
        let prim_type = reader.type_name(&sdf_path);

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
                    commands
                        .entity(entity)
                        .try_insert((UsdVisualSyncFailed(message.clone()), Visibility::Hidden));
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
            commands
                .entity(entity)
                .try_insert((UsdVisualSyncFailed(message.clone()), Visibility::Hidden));
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
            camera::instantiate_camera_prim(
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
            // `crate::lathe`'s `Changed`-filtered systems rebuild the mesh once per
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
                        UsdVisualMeshPending,
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
            // A curve prim with `widths` is a TUBE — swept geometry, not a line.
            // `build_usd_curve_mesh` returns `None` when `widths` is unauthored,
            // which is what keeps a pure path (a camera rail carrying
            // `lunco:path:camera`, see `camera_path.rs`) from silently becoming a
            // visible pipe. So the two readings coexist without a gate: a camera
            // path authors no `widths`, a conduit does.
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
                        UsdVisualMeshPending,
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
                .remove::<UsdVisualMeshPending>();
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
        // A program with a source this engine does not run — a `.mo` solved by
        // lunco-usd-sim, an `.xml` compiled by the behaviour-tree engine — is not
        // ours; extension picks the engine, exactly as USD picks a file format.
        attach_programs(
            reader,
            &sdf_path,
            entity,
            prim_path.stage_handle.id(),
            asset_server,
            commands,
        );

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

        // Tutorial chain: `lunco:nextScene = "scenes/foo.usda"` declares the scene
        // to load when this scene's mission completes. Stamped as a `NextScene`
        // marker; a generic handler (lunco-tutorial) loads it on MISSION_COMPLETE.
        if let Some(next) = reader
            .text(&sdf_path, "lunco:nextScene")
            .filter(|s| !s.trim().is_empty())
        {
            commands
                .entity(entity)
                .try_insert(lunco_core::NextScene(next));
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
                    // Mark the entity so `hide_glb_placeholder_meshes`
                    // can drop the placeholder Mesh3d once this Scene
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

        // Transform (position and rotation)
        // Preserve any existing transform set by the spawning code (e.g., rover position).
        // Only override position/rotation if the USD prim has explicit NON-ZERO values.
        // A zero translation in USD means "no offset" — it shouldn't overwrite a spawn position.
        let mut transform = existing_tf.cloned().unwrap_or_default();
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
                commands
                    .entity(entity)
                    .try_insert((UsdVisualSyncFailed(error.to_string()), Visibility::Hidden));
                return;
            }
        };
        if let Some(v) = usd_tf.map(|t| t.translation) {
            // Only apply USD translation if it's non-zero (avoid overwriting spawn positions).
            if v.length_squared() > 1e-6 {
                transform.translation = v;
            }
        }
        if let Some(q) = usd_tf.map(|t| t.rotation) {
            // Only apply a non-identity USD rotation (preserve spawn rotation otherwise).
            if !q.abs_diff_eq(Quat::IDENTITY, 1e-6) {
                transform.rotation = q;
            }
        }
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
        // `xformOp:scale` (UsdGeomXformable) — non-uniform scaling composed with
        // translate + rotate. `Cube` prims rely on this to express differing
        // width/height/depth, since UsdGeomCube itself has only `size`. The
        // composed transform (matrix / xformOpOrder) carries scale too.
        let usd_scale = usd_tf.map(|t| t.scale);
        if let Some(v) = usd_scale {
            let nonzero = v.x.abs() > 1e-6 || v.y.abs() > 1e-6 || v.z.abs() > 1e-6;
            if nonzero {
                transform.scale = v;
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
            UsdVisualSynced,
            final_vis,
            InheritedVisibility::default(),
            ViewVisibility::default(),
        ));

        // Tag entities carrying ANY animated channel (xform, visibility, or a
        // bound-shader / displayColor material input) so the per-frame samplers
        // drive them (doc 19). The query stays empty for static scenes.
        // `bind_animated_to_preview` then binds the tagged entity to the
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
        if camera_track::prim_is_camera_track(reader, &sdf_path) {
            commands
                .entity(entity)
                .try_insert(camera_track::CameraTrack);
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
                commands
                    .entity(parent)
                    .try_insert((UsdVisualSyncFailed(error.to_string()), Visibility::Hidden));
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
            UsdAwaitingStage,
            UsdVisualProjectionQueued,
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
/// bounded projection. Otherwise the entity is tagged `UsdAwaitingStage` and
/// waits for `sync_usd_visuals` to move it once the asset becomes ready.
///
/// This is the **happy path** in steady state — once a scene is loaded,
/// any newly-spawned `UsdPrimPath` entity (API command, attach
/// operation, recursive child spawn) enters the same bounded queue. The queue
/// is dormant when no prim is waiting.
fn on_usd_prim_added(
    trigger: On<Add, UsdPrimPath>,
    q: Query<&UsdPrimPath, (Without<UsdVisualSynced>, Without<UsdVisualSyncFailed>)>,
    mut commands: Commands,
    stages: Res<Assets<UsdStageAsset>>,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = q.get(entity) else {
        return;
    };

    if stages.get(&prim_path.stage_handle).is_none() {
        commands.entity(entity).try_insert(UsdAwaitingStage);
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
        .try_insert((UsdAwaitingStage, UsdVisualProjectionQueued));
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

/// Moves the `UsdAwaitingStage` queue into bounded visual projection when a
/// stage finishes loading. Each matching entity remains marked as awaiting
/// until `process_queued_usd_visuals` commits it.
pub fn sync_usd_visuals(
    mut ev: MessageReader<AssetEvent<UsdStageAsset>>,
    q: Query<
        (Entity, &UsdPrimPath),
        (
            With<UsdAwaitingStage>,
            Without<UsdVisualProjectionQueued>,
            Without<UsdVisualSynced>,
            Without<UsdVisualSyncFailed>,
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
        commands
            .entity(entity)
            .try_insert(UsdVisualProjectionQueued);
    }
}

fn any_queued_usd_visuals(q: Query<(), With<UsdVisualProjectionQueued>>) -> bool {
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
pub fn process_queued_usd_visuals(
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
            With<UsdVisualProjectionQueued>,
            Without<UsdVisualSynced>,
            Without<UsdVisualSyncFailed>,
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
            .try_remove::<UsdVisualProjectionQueued>()
            .try_remove::<UsdAwaitingStage>();
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
        Has<UsdVisualSynced>,
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
                .try_remove::<UsdVisualMeshPending>()
                .try_insert(UsdVisualProjectionQueued);
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
                .try_remove::<UsdVisualMeshPending>()
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
            .try_remove::<UsdVisualMeshPending>();
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
            With<UsdAwaitingStage>,
            Without<UsdVisualProjectionQueued>,
            Without<UsdVisualSynced>,
            Without<UsdVisualSyncFailed>,
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
        commands
            .entity(entity)
            .try_insert(UsdVisualProjectionQueued);
    }
}

/// Find the live-scene ownership root for a projected entity.
///
/// Entities in a preview or an additive/unrooted projection may have no live
/// scene root ancestor and are intentionally left to their own projection
/// policy.  A regular scene entity always reaches `UsdSceneRoot`, including
/// the root itself.  The bound prevents malformed relationship data from
/// turning a failed load into an infinite loop during error handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneRootAncestorError {
    /// A parent relationship points to an entity that no longer exists.
    MissingParentEntity,
    /// The hierarchy exceeded the traversal bound and is treated as malformed.
    DepthExceeded,
}

pub fn scene_root_ancestor(
    entity: Entity,
    q_scene_root: &Query<(), With<UsdSceneRoot>>,
    q_child_of: &Query<&ChildOf>,
    q_entities: &Query<Entity>,
) -> Result<Option<Entity>, SceneRootAncestorError> {
    let mut current = entity;
    for _ in 0..1024 {
        if q_scene_root.contains(current) {
            return Ok(Some(current));
        }
        let Ok(parent) = q_child_of.get(current) else {
            return Ok(None);
        };
        current = parent.parent();
        if !q_entities.contains(current) {
            return Err(SceneRootAncestorError::MissingParentEntity);
        }
    }
    warn!(
        "[usd] scene hierarchy exceeded 1024 ancestors at {:?}",
        entity
    );
    Err(SceneRootAncestorError::DepthExceeded)
}

#[cfg(test)]
mod scene_mount_tests {
    use super::*;

    #[derive(Resource)]
    struct ExpectedRoots {
        root: Entity,
        child: Entity,
        detached: Entity,
    }

    fn assert_root_ancestry(
        expected: Res<ExpectedRoots>,
        q_scene_root: Query<(), With<UsdSceneRoot>>,
        q_child_of: Query<&ChildOf>,
        q_entities: Query<Entity>,
    ) {
        assert_eq!(
            scene_root_ancestor(expected.child, &q_scene_root, &q_child_of, &q_entities),
            Ok(Some(expected.root))
        );
        assert_eq!(
            scene_root_ancestor(expected.detached, &q_scene_root, &q_child_of, &q_entities),
            Ok(None)
        );
    }

    #[derive(Resource)]
    struct ExpectedPreview {
        root: Entity,
        child: Entity,
        detached: Entity,
    }

    fn assert_preview_ancestry(
        expected: Res<ExpectedPreview>,
        q_preview_only: Query<(), With<UsdPreviewOnly>>,
        q_child_of: Query<&ChildOf>,
    ) {
        assert!(is_preview_only(expected.root, &q_child_of, &q_preview_only));
        assert!(is_preview_only(
            expected.child,
            &q_child_of,
            &q_preview_only
        ));
        assert!(!is_preview_only(
            expected.detached,
            &q_child_of,
            &q_preview_only
        ));
    }

    #[test]
    fn projected_descendants_resolve_their_scene_mount_root() {
        let mut app = App::new();
        let (root, child, detached) = {
            let world = app.world_mut();
            let root = world.spawn(UsdSceneRoot).id();
            let child = world.spawn(ChildOf(root)).id();
            let detached = world.spawn_empty().id();
            (root, child, detached)
        };
        app.insert_resource(ExpectedRoots {
            root,
            child,
            detached,
        })
        .add_systems(Update, assert_root_ancestry);
        app.update();
    }

    #[test]
    fn an_invalidated_root_is_no_longer_owned_by_the_mount() {
        let root = Entity::from_bits(41);
        let mut state = lunco_core::SceneMountState::default();
        state.register_root(root, true);
        assert!(state.contains_root(root));
        assert_eq!(state.active_root(), Some(root));

        state.begin_replacement();
        assert!(!state.contains_root(root));
        assert_eq!(state.active_root(), None);
    }

    #[test]
    fn projected_preview_descendants_resolve_their_preview_ownership_root() {
        let mut app = App::new();
        let (root, child, detached) = {
            let world = app.world_mut();
            let root = world.spawn(UsdPreviewOnly).id();
            let child = world.spawn(ChildOf(root)).id();
            let detached = world.spawn_empty().id();
            (root, child, detached)
        };
        app.insert_resource(ExpectedPreview {
            root,
            child,
            detached,
        })
        .add_systems(Update, assert_preview_ancestry);
        app.update();

        assert!(is_preview_only_entity(app.world(), child));
        assert!(!is_preview_only_entity(app.world(), detached));
    }
}

/// Terminal asset failure recorded by the USD asset boundary until the scene
/// transaction consumes it. Keeping the stage identity here prevents the
/// generic readiness/scene reconciler from mistaking a failed load for a
/// successful drain.
#[derive(Resource, Debug, Clone)]
pub struct FailedSceneLoad {
    pub stage_id: bevy::asset::AssetId<UsdStageAsset>,
    pub error: String,
}

/// Makes a failed stage load TERMINAL for the prims parked on it.
///
/// [`sync_usd_visuals`] drains `UsdAwaitingStage` on `LoadedWithDependencies`,
/// which is the only outcome it models. A stage that fails to load never emits
/// that event, so this boundary records the failure and closes the parked
/// entities explicitly. The scene transaction can then publish its terminal
/// failure and a later `LoadScene` can begin a new transaction.
///
/// A parked prim whose stage will never arrive cannot become anything, so it is
/// despawned rather than left as an inert husk that later passes for a mounted
/// scene. The failure is loud (`error!`) and consumed by the scene transaction
/// owner, which publishes the typed `SceneTransitionFailed` edge after the
/// parked entities have been reclaimed.
fn fail_awaiting_stage_prims(
    mut ev: MessageReader<bevy::asset::AssetLoadFailedEvent<UsdStageAsset>>,
    q: Query<(Entity, &UsdPrimPath), With<UsdAwaitingStage>>,
    mut commands: Commands,
) {
    for failure in ev.read() {
        let parked: Vec<Entity> = q
            .iter()
            .filter(|(_, prim_path)| prim_path.stage_handle.id() == failure.id)
            .map(|(entity, _)| entity)
            .collect();
        if parked.is_empty() {
            continue;
        }
        error!(
            "[usd] stage `{}` failed to load ({}) — {} prim(s) waiting on it will \
             never instantiate and are being dropped. The mount is abandoned; \
             later scene loads are free to proceed.",
            failure.path,
            failure.error,
            parked.len()
        );
        for entity in parked {
            // A replacement LoadScene/ClearScene may have reclaimed the
            // parked prim in the same command flush. Failure cleanup is
            // idempotent at the entity boundary.
            commands.entity(entity).try_despawn();
        }
        commands.insert_resource(FailedSceneLoad {
            stage_id: failure.id,
            error: failure.error.to_string(),
        });
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

/// Resolves an asset path relative to the stage it belongs to.
///
/// The rule is [`lunco_assets::asset_path::canonicalize`] — the same one USD layer
/// composition uses, so a texture, scenario, or layer reference spelled the same
/// way resolves the same way. Keeping the stage anchor lookup here prevents each
/// projection from inventing a second source-resolution path.
///
/// A stage need not have been loaded from a path — one composed in memory
/// (`StageRecipe::from_source`, runtime authoring) has none, which the provenance
/// stamp above already accounts for. That is the SAME "no anchoring document"
/// case openusd's resolver and rhai's importer hit, so it maps onto
/// [`canonicalize_root`] here too rather than failing the whole lookup: a
/// path-less stage referencing `@lunco://textures/foo.png@` still resolves,
/// which is what a `has_scheme` special case used to (partially) buy.
///
/// [`canonicalize_root`]: lunco_assets::asset_path::canonicalize_root
pub fn resolve_stage_asset_path(
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    asset_path: &str,
) -> String {
    use lunco_assets::asset_path::{anchor_of, canonicalize, canonicalize_root};
    match asset_server.get_path(stage_id) {
        Some(stage_path) => canonicalize(asset_path, &anchor_of(&stage_path)),
        None => canonicalize_root(asset_path),
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
pub fn resolve_bound_shader(
    reader: &dyn read::UsdReadObject,
    mesh_path: &SdfPath,
) -> Option<SdfPath> {
    let mat_path = reader.bound_material(mesh_path, MaterialPurpose::Render)?;
    let mat_path = SdfPath::new(&mat_path).ok()?;
    // `outputs:surface` is an attribute CONNECTION, not a relationship.
    let surf_conn = reader.connection_source(&mat_path, "outputs:surface")?;
    parent_prim_path(&surf_conn)
}

/// Which *purpose* of material binding to resolve.
///
/// USD binds a look and a physical surface with the SAME schema
/// (`UsdShadeMaterial`) and the SAME mechanism (a `material:binding`
/// relationship) — they differ only in the binding's **purpose** token. So a
/// single `Material` prim can carry a `UsdPreviewSurface` *and* an applied
/// `PhysicsMaterialAPI`, and one "Regolith" means both "looks like regolith" and
/// "grips like regolith". That is the right model for a simulator, and it is
/// USD's, so we don't invent a parallel one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialPurpose {
    /// All-purpose binding (`material:binding`) — the rendered look.
    Render,
    /// Purpose-specific binding (`material:binding:physics`) — friction,
    /// restitution, density (`UsdPhysicsMaterialAPI`).
    Physics,
}

impl MaterialPurpose {
    /// The USD binding *purpose* token. All-purpose is the empty token (the
    /// `material:binding` relationship); a restricted purpose names itself
    /// (`material:binding:physics`).
    pub fn token(self) -> &'static str {
        match self {
            MaterialPurpose::Render => openusd::schemas::shade::tokens::PURPOSE_ALL,
            MaterialPurpose::Physics => "physics",
        }
    }
}

/// Resolve the `Material` prim bound to `prim` for a given purpose —
/// `UsdShadeMaterialBindingAPI::ComputeBoundMaterial`, delegated to openusd.
///
/// The rules are openusd's, not ours: bindings inherit down namespace (nearest
/// ancestor wins, unless an ancestor is `strongerThanDescendants`), a restricted
/// purpose resolves across the WHOLE ancestor chain before falling back to
/// all-purpose, and a collection binding whose collection includes the prim beats
/// a direct binding. We used to re-derive the first two by hand and support
/// neither of the last two.
///
/// Resolution runs on the prim as it is — `MaterialBindingAPI::on` rather than
/// `::get` — because the prim being asked about (a mesh deep inside a rover)
/// normally authors no binding at all, and so carries no `MaterialBindingAPI` in
/// its `apiSchemas`. `::get` returns `None` there, which would silently drop
/// every *inherited* binding: the common case, not the corner case.
pub fn resolve_bound_material(
    reader: &StageView<'_>,
    prim: &SdfPath,
    purpose: MaterialPurpose,
) -> Option<SdfPath> {
    openusd::schemas::shade::MaterialBindingAPI::on(reader.stage(), prim.clone())
        .compute_bound_material(purpose.token())
        .ok()
        .flatten()
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
/// ([`sample_usd_material_animation`]) mutates the `PbrLook` every frame, and a
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
    // (`lunco_usd::material::ensure_preview_surface_ops` builds it) — and exactly
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
                let resolved = resolve_stage_asset_path(asset_server, stage_id, &asset_path);

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
    let animated = attr_has_time_samples(reader, sdf_path, "primvars:displayColor")
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
/// - `Value::AssetPath` — authored as `asset foo = @...@`, preserving the
///   standard USD asset-path type for user-facing attributes.
///
/// `prim_attribute_value::<String>` covers `String`/`Token` only,
/// so we go through `reader.get` for the attribute path directly
/// to also catch `AssetPath`.
/// Read the stage's `defaultPrim` metadata from the live composed
/// [`StageView`] pseudo-root. Returns the bare prim name (no leading slash),
/// or `None` when the stage declares no `defaultPrim`. The metadata lives on
/// the pseudo-root spec at the absolute root path.
/// The `defaultPrim` authored on a **layer** (`sdf::Data`), without composition.
///
/// The authored-layer twin of [`stage_default_prim`], which reads the *composed*
/// stage. A document's own root layer is the right place to ask "what prim do I
/// mount?" when authoring into it — no references need resolving to answer that,
/// and the two must not be conflated: runtime reads the composed stage, while
/// authoring asks the root layer directly.
pub fn layer_default_prim(layer: &UsdData) -> Option<String> {
    let name = layer.field(&SdfPath::abs_root(), "defaultPrim")?.as_str()?;
    (!name.is_empty()).then(|| name.to_string())
}

pub fn stage_default_prim(reader: &dyn read::UsdReadObject) -> Option<String> {
    // `defaultPrim` is authored as `Value::Token` (see compose.rs). The two
    // `StageView` resolves it through the composed stage.
    reader.default_prim()
}

/// Resolve a mounted prim path against the live composed stage.
///
/// Scene roots use an empty path until their asset is parsed; that sentinel
/// means the stage's composed `defaultPrim`. Every projection that reads a
/// [`UsdPrimPath`] must use this resolver so deferred visual projection cannot
/// race another domain projector and make the root permanently unaddressable.
pub fn resolve_stage_prim_path(reader: &dyn read::UsdReadObject, path: &str) -> Option<String> {
    if path.is_empty() {
        stage_default_prim(reader).map(|name| format!("/{name}"))
    } else {
        Some(path.to_owned())
    }
}

/// A single USD layer's source text, parsed once, positioned on the stage's
/// `defaultPrim` — with **typed** reads of the attributes authored there.
///
/// For data that lives on the root prim (a scene's `doc` metadata, an asset's
/// `lunco:spawnable`) this is a cheap, composition-free alternative to
/// [`compose_file`] / the async `AssetServer` loader — referenced sub-layers are
/// not consulted, which is correct for root-prim metadata but NOT for attributes
/// that a reference might override.
///
/// Reads the authored layer directly, NOT through [`UsdRead`]: `UsdRead` is the
/// *composed-stage* contract (`StageView`), and this exists precisely because
/// it does **not** want composition.
///
/// Parse ONCE, read many. A caller wanting three attributes off the same prim
/// (spawnable + lift + description) should not parse the file three times.
pub struct DefaultPrim {
    data: openusd::sdf::Data,
    path: SdfPath,
}

impl DefaultPrim {
    /// Parse `text` and locate its `defaultPrim`. `None` when the text doesn't
    /// parse or the stage declares no `defaultPrim`.
    pub fn parse(text: &str) -> Option<Self> {
        let data = parse_usda(text).ok()?;
        // `defaultPrim` is stage metadata on the pseudo-root, authored as a Token.
        let name = data
            .field(&SdfPath::abs_root(), "defaultPrim")?
            .as_str()?
            .to_string();
        if name.is_empty() {
            return None;
        }
        let path = SdfPath::new(&format!("/{name}")).ok()?;
        Some(Self { data, path })
    }

    /// The authored `defaultPrim` path, absolute in the source layer.
    ///
    /// Runtime reference authoring uses this path explicitly because the live
    /// stage must compose the source root's applied schemas onto a newly
    /// defined instance prim; an implicit default-prim reference composes the
    /// child namespace but leaves that instance root typeless.
    pub fn path(&self) -> &SdfPath {
        &self.path
    }

    /// Raw default-time value of `attr`, as authored.
    pub fn value(&self, attr: &str) -> Option<&Value> {
        let attr_path = self.path.append_property(attr).ok()?;
        self.data.field(&attr_path, "default")
    }

    /// The prim's `doc` metadata — USD's own human-readable "what is this thing"
    /// string, which usdview and every other DCC already display.
    ///
    /// Metadata on the prim, NOT an attribute on it, so it is read off the prim
    /// spec rather than through [`value`](Self::value). Authored in the metadata
    /// parens:
    ///
    /// ```usda
    /// def Xform "LandingPad" (
    ///     doc = "Blast-hardened landing pad — sintered disc with a centre hub."
    /// )
    /// ```
    ///
    /// This uses USD's `doc` metadata rather than a custom attribute, so the
    /// description is visible to OpenUSD tools without a LunCo-specific schema.
    pub fn documentation(&self) -> Option<String> {
        self.data
            .field(&self.path, "documentation")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    /// Typed read, via the same `TryFrom<Value>` conversion `UsdRead::scalar`
    /// uses — so a `bool` attribute reads as a `bool` and nothing else.
    pub fn scalar<T: TryFrom<Value>>(&self, attr: &str) -> Option<T> {
        self.value(attr).cloned()?.get::<T>()
    }

    /// The text of a `string`/`token`/`asset` attribute, via openusd's own
    /// [`Value::as_str`] — the one textual coercion (see [`UsdRead::text`]).
    pub fn text(&self, attr: &str) -> Option<String> {
        self.value(attr)?.as_str().map(str::to_string)
    }

    /// A real scalar tolerant of `float` **or** `double` authoring — the
    /// [`UsdRead::real_f32`] rule, so a value is never dropped for being
    /// authored in the other precision.
    pub fn real_f32(&self, attr: &str) -> Option<f32> {
        self.scalar::<f32>(attr)
            .or_else(|| self.scalar::<f64>(attr).map(|v| v as f32))
    }
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

/// Whether `path` is `root` or is below it in the USD namespace.
pub fn is_descendant_or_self(path: &SdfPath, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path.as_str() == root
        || path
            .as_str()
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
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
        if let Some(Value::PathListOp(op)) = reader.field(&rel_sdf, field) {
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
pub fn read_vec3_f64(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Option<[f64; 3]> {
    reader.vec3_f64(path, attr)
}

/// Read a **`UsdGeomGprim` display primvar** — `primvars:displayColor`.
///
/// These are ARRAY-valued by schema (`color3f[]`, i.e. `Vec3fVec`), not scalar
/// `color3f`. The interpolation defaults to `constant`, meaning one value for
/// the whole prim, so element 0 is the prim's colour.
///
/// Deliberately separate from [`read_vec3_f64`], which reads genuinely *scalar*
/// `color3f`/`float3` attributes (`inputs:diffuseColor`, `inputs:color`, the
/// xform ops). Two USD types, two readers — a single lenient one that took
/// either would let `color3f primvars:displayColor` (the wrong type, which every
/// asset here used to author) keep working, and that is the bug we are removing.
pub fn read_primvar_vec3<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str) -> Option<[f64; 3]> {
    let out = primvar_vec3_from(reader.attr_value(path, attr)?);
    if out.is_none() {
        // Authored, but not as the schema's array type (the classic mistake is a
        // scalar `color3f primvars:displayColor`). Say so ONCE instead of
        // silently rendering white forever.
        static NON_ARRAY_PRIMVAR: std::sync::Once = std::sync::Once::new();
        NON_ARRAY_PRIMVAR.call_once(|| {
            warn!(
                "[usd-bevy] {} authors `{attr}` with a non-array value — the schema type \
                 is `color3f[]`; the value is ignored (further cases not logged)",
                path.as_str()
            );
        });
    }
    out
}

/// Strict authored twin of [`read_primvar_vec3`].  `Ok(None)` means the
/// attribute is genuinely omitted; `Err` means an authored value has the wrong
/// USD type, is empty, or contains a non-finite component. Runtime material
/// boundaries use this distinction so malformed display data cannot turn into
/// a white surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrictAttributeError {
    /// Prim that owns the malformed authored value.
    pub path: String,
    /// Attribute whose authored value failed strict decoding.
    pub attribute: String,
}

impl StrictAttributeError {
    fn new(path: &SdfPath, attribute: &str) -> Self {
        Self {
            path: path.to_string(),
            attribute: attribute.to_owned(),
        }
    }
}

impl std::fmt::Display for StrictAttributeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} has an invalid authored `{}` value",
            self.path, self.attribute
        )
    }
}

impl std::error::Error for StrictAttributeError {}

pub fn read_primvar_vec3_strict(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<[f64; 3]>, StrictAttributeError> {
    match reader.attr_value(path, attr) {
        Some(value) => primvar_vec3_from(value)
            .filter(|values| values.iter().all(|value| value.is_finite()))
            .map(Some)
            .ok_or_else(|| StrictAttributeError::new(path, attr)),
        None if reader.has_authored_attribute(path, attr) => {
            Err(StrictAttributeError::new(path, attr))
        }
        None => Ok(None),
    }
}

/// Time-sampled twin of [`read_primvar_vec3`].
pub fn read_primvar_vec3_at(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
    time: f64,
) -> Option<[f64; 3]> {
    primvar_vec3_from(reader.attr_value_at(path, attr, time)?)
}

/// First element of an array-valued vec3 primvar (`constant` interpolation).
fn primvar_vec3_from(value: Value) -> Option<[f64; 3]> {
    match value {
        Value::Vec3fVec(v) => v.first().map(|c| [c.x as f64, c.y as f64, c.z as f64]),
        Value::Vec3dVec(v) => v.first().map(|c| [c.x, c.y, c.z]),
        Value::Vec3hVec(v) => v.first().map(|c| {
            [
                f32::from(c.x) as f64,
                f32::from(c.y) as f64,
                f32::from(c.z) as f64,
            ]
        }),
        _ => None,
    }
}

/// Read a **`UsdGeomGprim` display primvar** — `primvars:displayOpacity`.
/// `float[]` by schema, `constant` interpolation → element 0. See
/// [`read_primvar_vec3`] for why this is not merged with the scalar reader.
pub fn read_primvar_f32<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str) -> Option<f32> {
    primvar_f32_from(reader.attr_value(path, attr)?)
}

/// Strict authored twin of [`read_primvar_f32`].  It preserves omission while
/// rejecting authored wrong types, empty arrays, and non-finite values.
pub fn read_primvar_f32_strict(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<f32>, StrictAttributeError> {
    match reader.attr_value(path, attr) {
        Some(value) => primvar_f32_from(value)
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| StrictAttributeError::new(path, attr)),
        None if reader.has_authored_attribute(path, attr) => {
            Err(StrictAttributeError::new(path, attr))
        }
        None => Ok(None),
    }
}

/// Strict authored boolean reader for USD surface flags.  Integer spellings
/// remain supported by [`UsdRead::boolean`], but an authored value that is not
/// a USD boolean/integer is not treated as `false`.
pub fn read_authored_bool_strict(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<bool>, StrictAttributeError> {
    match reader.boolean(path, attr) {
        Some(value) => Ok(Some(value)),
        None if reader.has_authored_attribute(path, attr) => {
            Err(StrictAttributeError::new(path, attr))
        }
        None => Ok(None),
    }
}

/// Time-sampled twin of [`read_primvar_f32`].
pub fn read_primvar_f32_at(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
    time: f64,
) -> Option<f32> {
    primvar_f32_from(reader.attr_value_at(path, attr, time)?)
}

fn primvar_f32_from(value: Value) -> Option<f32> {
    match value {
        Value::FloatVec(v) => v.first().copied(),
        Value::DoubleVec(v) => v.first().map(|d| *d as f32),
        Value::HalfVec(v) => v.first().map(|h| f32::from(*h)),
        _ => None,
    }
}

/// Time-sampled twin of [`read_vec3_f64`]: evaluates the attribute's
/// `timeSamples` at `time` (held/linear via `openusd::usd::evaluate`), falling
/// back to `default` when there are no samples. Same value-type coverage
/// (`[f32;3]`/`[f64;3]` and the `Vec<f32>`/`Vec<f64>` forms).
pub fn read_vec3_f64_at(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
    time: f64,
) -> Option<[f64; 3]> {
    let value = reader.attr_value_at(path, attr, time)?;
    if let Some(v) = value.clone().get::<[f32; 3]>() {
        return Some([v[0] as f64, v[1] as f64, v[2] as f64]);
    }
    if let Some(v) = value.clone().get::<[f64; 3]>() {
        return Some([v[0], v[1], v[2]]);
    }
    if let Some(v) = value.clone().get::<Vec<f32>>() {
        if v.len() >= 3 {
            return Some([v[0] as f64, v[1] as f64, v[2] as f64]);
        }
    }
    if let Some(v) = value.get::<Vec<f64>>() {
        if v.len() >= 3 {
            return Some([v[0], v[1], v[2]]);
        }
    }
    None
}

/// True iff `attr` on `path` actually carries `timeSamples` (not just a
/// `default`). The sampler uses this per-channel so it writes **only** animated
/// channels — a static `xformOp:rotateXYZ` is left exactly as instantiated.
pub fn attr_has_time_samples(reader: &dyn read::UsdReadObject, path: &SdfPath, attr: &str) -> bool {
    reader.has_time_samples(path, attr)
}

/// The xform ops the animation sampler drives, in compose order (T, R, S).
pub const ANIMATED_XFORM_OPS: [&str; 3] =
    ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"];

/// The bound-shader inputs the material sampler drives. Base color and opacity
/// are the canonical animated `UsdPreviewSurface` channels.
pub const ANIMATED_SHADER_INPUTS: [&str; 2] = ["inputs:diffuseColor", "inputs:opacity"];

/// True iff any of the entity's xform ops carries `timeSamples` — i.e. the prim
/// is animated and the entity should get the [`UsdAnimated`] marker. Covers
/// translate / scale, the full matrix `xformOp:transform`, and every rotation
/// channel ([`ROTATION_OPS`]: Euler orders, `orient`, single-axis).
pub fn prim_has_xform_time_samples<R: UsdRead>(reader: &R, path: &SdfPath) -> bool {
    attr_has_time_samples(reader, path, "xformOp:translate")
        || attr_has_time_samples(reader, path, "xformOp:scale")
        || attr_has_time_samples(reader, path, "xformOp:transform")
        || prim_rotation_animated(reader, path)
}

/// True iff the prim carries ANY channel the runtime samples per-frame: an
/// xform op, `visibility`, geom `primvars:displayColor`, or a bound surface
/// shader's [`ANIMATED_SHADER_INPUTS`]. Drives the [`UsdAnimated`] tag, so a
/// material-only or visibility-only animation is funnelled the same as xform.
pub fn prim_is_animated<R: UsdRead>(reader: &R, path: &SdfPath) -> bool {
    if prim_has_xform_time_samples(reader, path)
        || attr_has_time_samples(reader, path, "visibility")
        || attr_has_time_samples(reader, path, "primvars:displayColor")
    {
        return true;
    }
    resolve_bound_shader(reader, path).is_some_and(|shader| {
        ANIMATED_SHADER_INPUTS
            .iter()
            .any(|i| attr_has_time_samples(reader, &shader, i))
    })
}

/// The stage's `timeCodesPerSecond`, read as stage metadata off the
/// pseudo-root. USD maps a time code `t` to wall-clock `t / tcps` seconds,
/// so the samplers multiply their resolved time (seconds) by this to get the
/// time code to evaluate. Defaults to 24.0 (USD spec) when unauthored or
/// non-positive — the latter guards a malformed stage from freezing animation.
pub fn stage_time_codes_per_second(reader: &dyn read::UsdReadObject) -> f64 {
    // `UsdRead::time_codes_per_second` already defaults to 24 when unauthored;
    // guard a malformed non-positive opinion (either source) so it can't freeze
    // animation (division by a zero/negative rate).
    let tcps = reader.time_codes_per_second();
    if tcps > 0.0 {
        tcps
    } else {
        24.0
    }
}

/// Held-sampled `token`/`string` attribute at time code `time` (USD tokens hold,
/// never interpolate) — the animated twin of [`UsdRead::text`], reading the same
/// [`Value::as_str`] coercion at a time code. `None` when the attribute has no
/// `timeSamples` or the held sample isn't textual. An `asset`-typed channel is
/// deliberately NOT read here: an asset reference is a different thing from a
/// token, and no animated channel we author is one.
pub(crate) fn read_token_at(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
    time: f64,
) -> Option<String> {
    // Gate on authored `timeSamples` (never fall back to `default`): a token
    // channel with no samples isn't animated. `attr_value_at` evaluates the
    // samples — for a non-lerpable token type openusd's Linear interpolation
    // falls back to Held (the nearest previous sample).
    if !reader.has_time_samples(path, attr) {
        return None;
    }
    reader
        .attr_value_at(path, attr, time)?
        .as_str()
        .map(str::to_string)
}

/// Enumerate a token/string channel's authored keys as `(time_code, value)`
/// pairs, ascending. Reads the raw `timeSamples` key times, then resolves each
/// held value through [`read_token_at`] — so it doesn't depend on the inner
/// sample value type. `None`/empty when the attribute carries no token samples.
/// Used to build the [`camera_track::CameraTrackPlan`] key list once.
pub(crate) fn read_token_timesamples(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Vec<(f64, String)> {
    reader
        .time_sample_times(path, attr)
        .into_iter()
        .filter_map(|t| read_token_at(reader, path, attr, t).map(|name| (t, name)))
        .collect()
}

/// The authored time-code span `(first, last)` of one attribute's `timeSamples`
/// (samples are stored ascending, so the ends are the first/last keys). `None`
/// when the attribute has no samples.
fn attr_sample_span(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Option<(f64, f64)> {
    let times = reader.time_sample_times(path, attr);
    Some((*times.first()?, *times.last()?))
}

/// The authored time span `(start, end)` in **seconds** across all of `path`'s
/// animated channels (xform ops / `visibility` / geom `primvars:displayColor` /
/// bound-shader [`ANIMATED_SHADER_INPUTS`]), i.e. the time codes divided by the
/// stage `timeCodesPerSecond`. `None` when nothing is sampled. The transport
/// uses this to bound the preview playhead to the real clip length instead of a
/// guessed range.
pub fn animated_time_range(reader: &dyn read::UsdReadObject, path: &SdfPath) -> Option<(f64, f64)> {
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for op in ["xformOp:translate", "xformOp:scale", "xformOp:transform"] {
        spans.extend(attr_sample_span(reader, path, op));
    }
    for op in ROTATION_OPS {
        spans.extend(attr_sample_span(reader, path, op));
    }
    spans.extend(attr_sample_span(reader, path, "visibility"));
    spans.extend(attr_sample_span(reader, path, "primvars:displayColor"));
    if let Some(shader) = resolve_bound_shader(reader, path) {
        for i in ANIMATED_SHADER_INPUTS {
            spans.extend(attr_sample_span(reader, &shader, i));
        }
    }
    let lo = spans.iter().map(|s| s.0).fold(f64::INFINITY, f64::min);
    let hi = spans.iter().map(|s| s.1).fold(f64::NEG_INFINITY, f64::max);
    if hi < lo {
        return None;
    }
    let tcps = stage_time_codes_per_second(reader);
    Some((lo / tcps, hi / tcps))
}

/// Time-sampled scalar float at time code `time`, accepting both `float` and
/// `double` authored types (`inputs:opacity` is commonly either). `None` for a
/// static channel so the caller leaves the material untouched.
fn read_f32_at(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
    time: f64,
) -> Option<f32> {
    if !attr_has_time_samples(reader, path, attr) {
        return None;
    }
    reader
        .attr_value_at(path, attr, time)
        .and_then(|value| match value {
            Value::Float(value) => Some(value),
            Value::Double(value) => Some(value as f32),
            Value::Int(value) => Some(value as f32),
            Value::Int64(value) => Some(value as f32),
            _ => None,
        })
}

/// Sample one xform-op channel **only if it is animated** (has `timeSamples`),
/// evaluated at `time`. Returns `None` for static channels so the caller leaves
/// the instantiated value untouched.
pub fn sample_animated_vec3(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
    time: f64,
) -> Option<[f64; 3]> {
    if !attr_has_time_samples(reader, path, attr) {
        return None;
    }
    read_vec3_f64_at(reader, path, attr, time)
}

/// Per-frame USD animation sampler (doc 19 — the animation funnel / T5).
///
/// For every [`UsdAnimated`] entity, resolve its clock — the [`TimeBinding`]'d
/// `TimeDomain` (per-object / per-selection / per-project / factory-scaled) via
/// [`ResolvedDomains`], or the world clock when unbound — then evaluate its
/// animated xform-op channels at that `local_t` and write the result to the
/// entity's local `Transform`. Only channels carrying `timeSamples` are written;
/// static channels keep their instantiated value. Runs in `Update` after the
/// domain resolve ([`lunco_time::DomainResolveSet`]) and before the `PostUpdate`
/// transform propagation (incl. big_space), so the pose is current before it
/// propagates.
///
/// Time convention: the entity's resolved domain time is in **seconds**; it is
/// mapped to USD time codes via the stage's `timeCodesPerSecond`
/// ([`stage_time_codes_per_second`], default 24 per USD spec). Sublayer /
/// reference `LayerOffset`s are already resolved into the composed sample times
/// by the shared reader, so no offset composition happens here.
/// Derive each animated prim's [`AnimationPlan`] once, as soon as its stage
/// asset is loaded (doc 19 — tier-1 memo of animation topology).
///
/// Gated on `Without<AnimationPlan>`, so it retries each frame only for
/// entities not yet planned (a stage may not be loaded the frame `UsdAnimated`
/// is added) and is **empty in steady state** once every animated prim carries
/// its plan. The topology walks (`has_xform_op_order`, `attr_has_time_samples`,
/// `resolve_bound_shader`, …) happen here — the per-frame samplers then just
/// read values at `t`. Re-derived after a stage hot-reload via
/// [`clear_animation_plans_on_stage_reload`].
pub fn plan_usd_animation(
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), (With<UsdAnimated>, Without<AnimationPlan>)>,
) {
    for (entity, prim) in &q {
        let Some(stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(prim.stage_handle.id(), stage_asset);
        let reader = &reader;
        let Ok(sdf_path) = SdfPath::new(prim.path.as_str()) else {
            continue;
        };

        // Transform: an authored `xformOpOrder` drives the whole stack. Without
        // one there is NO transform to drive — UsdGeomXformable gives orderless
        // `xformOp:*` attributes no meaning, and the static decode
        // (`local_transform_at_raw`) already treats them as inert data, so the
        // sampler must too or an animated prim would move where a static one
        // holds still.
        let xform = if has_xform_op_order(reader, &sdf_path) {
            XformDrive::OpOrder
        } else {
            XformDrive::None
        };

        // Material: resolve the bound shader once and record which channels move.
        let shader = resolve_bound_shader(reader, &sdf_path);
        let diffuse = shader
            .as_ref()
            .is_some_and(|s| attr_has_time_samples(reader, s, "inputs:diffuseColor"));
        let geom_color =
            !diffuse && attr_has_time_samples(reader, &sdf_path, "primvars:displayColor");
        let opacity = shader
            .as_ref()
            .is_some_and(|s| attr_has_time_samples(reader, s, "inputs:opacity"));
        let material = (diffuse || geom_color || opacity).then_some(MaterialPlan {
            shader,
            diffuse,
            geom_color,
            opacity,
        });

        commands.entity(entity).try_insert(AnimationPlan {
            time_codes_per_second: stage_time_codes_per_second(reader),
            xform,
            visibility: attr_has_time_samples(reader, &sdf_path, "visibility"),
            material,
            path: sdf_path,
        });
    }
}

/// Drop cached [`AnimationPlan`]s for entities whose stage was hot-reloaded, so
/// [`plan_usd_animation`] re-derives them against the new content. Runs only on
/// frames carrying a `UsdStageAsset` `Modified` event (else the query is skipped).
pub fn clear_animation_plans_on_stage_reload(
    mut ev: MessageReader<AssetEvent<UsdStageAsset>>,
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), With<AnimationPlan>>,
) {
    let reloaded: Vec<AssetId<UsdStageAsset>> = ev
        .read()
        .filter_map(|e| match e {
            AssetEvent::Modified { id } | AssetEvent::LoadedWithDependencies { id } => Some(*id),
            _ => None,
        })
        .collect();
    if reloaded.is_empty() {
        return;
    }
    for (entity, prim) in &q {
        if reloaded.contains(&prim.stage_handle.id()) {
            commands.entity(entity).remove::<AnimationPlan>();
        }
    }
}

pub fn sample_usd_animation(
    world: Res<lunco_time::WorldTime>,
    resolved: Res<lunco_time::ResolvedDomains>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut q: Query<
        (
            &UsdPrimPath,
            &AnimationPlan,
            &mut Transform,
            &mut Visibility,
            Option<&lunco_time::TimeBinding>,
        ),
        With<UsdAnimated>,
    >,
) {
    for (prim, plan, mut tf, mut vis, binding) in &mut q {
        let Some(stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(prim.stage_handle.id(), stage_asset);
        let reader = &reader;
        let sdf_path = &plan.path;

        // Resolve this entity's clock — its bound `TimeDomain` (per-object /
        // selection / project / factory) or the world clock when unbound — and
        // convert seconds → USD time code (topology already resolved in the plan).
        let secs = lunco_time::domain_time(&resolved, binding, &world);
        let t = secs * plan.time_codes_per_second;

        // Drive the local transform per the plan's cached topology. The result is
        // converted to the canonical frame by the stage's `ConventionTransform` —
        // the sampler drives the raw composer (not `local_transform_at`), so it
        // must convert explicitly or an animated prim on a Z-up/cm stage would
        // snap back to stage units every frame.
        let Ok(conv) = stage_convention(reader) else {
            error!(
                "[usd-bevy] animated prim {} has invalid stage convention metadata; refusing sample",
                sdf_path.as_str()
            );
            continue;
        };
        match &plan.xform {
            XformDrive::OpOrder => {
                if let Ok(Some(m)) = compose_xform_order_at(reader, sdf_path, t) {
                    let m = conv.local_transform(m);
                    tf.translation = m.translation;
                    tf.rotation = m.rotation;
                    tf.scale = m.scale;
                }
            }
            XformDrive::None => {}
        }

        // Animated `visibility` (token, held): `invisible` → `Hidden`, anything
        // else → `Inherited`. Skipped entirely unless the plan flags it, so a prim
        // animated only in xform/material never churns visibility change-detection.
        if plan.visibility {
            if let Some(tok) = read_token_at(reader, sdf_path, "visibility", t) {
                let want = if tok == "invisible" {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                };
                if *vis != want {
                    *vis = want;
                }
            }
        }
    }
}

/// Per-frame USD **material** animation (doc 19 — T5 material channels).
///
/// Sibling of [`sample_usd_animation`] for the visual-material path: for each
/// [`UsdAnimated`] entity that owns a [`PbrLook`], sample the bound
/// surface shader's animated `inputs:diffuseColor` / `inputs:opacity` (or the
/// geom's `primvars:displayColor`) at the entity's resolved time code and write
/// them into the look. Each channel is gated on
/// [`attr_has_time_samples`], so an entity animated only in xform/visibility
/// does a few cheap `HashMap` lookups and touches no material. Runs in `Update`
/// after [`lunco_time::DomainResolveSet`], like the transform sampler.
///
/// This writes **intent**, not a material asset — `lunco-render-bevy`'s
/// `rebind_changed_pbr_look` picks the change up. Those looks are authored
/// `unshared` (see [`apply_standard_material`]), so the binder mutates ONE
/// private material in place per prim instead of minting a fresh cached material
/// every frame (which would be an unbounded leak).
///
/// Change-detection note: `Mut<PbrLook>` is only dereferenced *mutably* when a
/// channel actually resolves a sample, so a static frame does not mark the look
/// changed.
pub fn sample_usd_material_animation(
    world: Res<lunco_time::WorldTime>,
    resolved: Res<lunco_time::ResolvedDomains>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut q: Query<
        (
            &UsdPrimPath,
            &AnimationPlan,
            &mut PbrLook,
            Option<&lunco_time::TimeBinding>,
        ),
        With<UsdAnimated>,
    >,
) {
    for (prim, plan, mut look, binding) in &mut q {
        // Cheap gate: the plan already resolved the shader + which channels move.
        let Some(mat) = &plan.material else { continue };
        let Some(stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(prim.stage_handle.id(), stage_asset);
        let reader = &reader;
        let sdf_path = &plan.path;

        let secs = lunco_time::domain_time(&resolved, binding, &world);
        let t = secs * plan.time_codes_per_second;

        // Base color: a shader `inputs:diffuseColor` wins over geom displayColor.
        // USD `color3f` is linear scene-referred (matches `apply_standard_material`).
        let color_src = if mat.diffuse {
            mat.shader.as_ref()
        } else if mat.geom_color {
            Some(sdf_path)
        } else {
            None
        };
        // Two different USD value types, so two different readers: a shader's
        // `inputs:diffuseColor` is a SCALAR `color3f`, while the geom's
        // `primvars:displayColor` is an ARRAY (`color3f[]`, constant
        // interpolation). Reading either with the other's reader silently yields
        // `None` and the animation just stops.
        if let Some(src) = color_src {
            let sampled = if mat.diffuse {
                read_vec3_f64_at(reader, src, "inputs:diffuseColor", t)
            } else {
                read_primvar_vec3_at(reader, src, "primvars:displayColor", t)
            };
            if let Some(c) = sampled {
                let a = look.base_color.alpha;
                look.base_color = LinearRgba::new(c[0] as f32, c[1] as f32, c[2] as f32, a);
            }
        }

        // Opacity → base-color alpha. If a fully-opaque material starts being
        // animated below 1.0, promote it to `Blend` so the transparency shows.
        if mat.opacity {
            if let Some(o) = read_f32_at(
                reader,
                mat.shader.as_ref().unwrap_or(sdf_path),
                "inputs:opacity",
                t,
            ) {
                look.base_color.alpha = o;
                if o < 1.0 && look.alpha == SurfaceAlpha::Opaque {
                    look.alpha = SurfaceAlpha::Blend;
                }
            }
        }
    }
}

/// Bind freshly-tagged [`UsdAnimated`] entities to the singleton
/// [`lunco_time::AnimationPreview`] domain so the animation transport
/// (play / pause / scrub / rate) drives them, while physics keeps following the
/// world clock. `Without<TimeBinding>` leaves any explicit binding (e.g. a
/// factory-replay domain) intact; when the time spine isn't installed (a
/// `MinimalPlugins` example) the resource is absent and animated prims simply
/// stay on the world clock. Change-driven via `Added` — empty in steady state.
///
/// Also grows the preview domain's [`Playback`](lunco_time::Playback) range to
/// cover the bound clips' authored span ([`animated_time_range`]), so the
/// transport scrub bar and clamp/loop track the real clip length.
pub fn bind_animated_to_preview(
    preview: Option<Res<lunco_time::AnimationPreview>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), (Added<UsdAnimated>, Without<lunco_time::TimeBinding>)>,
    mut playback: Query<&mut lunco_time::Playback>,
) {
    let Some(preview) = preview else { return };
    let mut span: Option<(f64, f64)> = None;
    for (entity, prim) in &q {
        commands.entity(entity).try_insert(lunco_time::TimeBinding {
            domain: preview.domain,
        });
        // Union this clip's authored span into the range we'll grow the domain to.
        if let Some(stage_asset) = stages.get(&prim.stage_handle) {
            let (reader, _generation) = canonical.reader_for(prim.stage_handle.id(), stage_asset);
            let reader = &reader;
            if let Ok(sp) = SdfPath::new(prim.path.as_str()) {
                if let Some((a, b)) = animated_time_range(reader, &sp) {
                    span = Some(match span {
                        Some((lo, hi)) => (lo.min(a), hi.max(b)),
                        None => (a, b),
                    });
                }
            }
        }
    }
    if let (Some((a, b)), Ok(mut pb)) = (span, playback.get_mut(preview.domain)) {
        // Grow (never shrink) the existing range so multiple stages coexist.
        let (lo, hi) = if pb.bounded() {
            (pb.start.min(a), pb.end.max(b))
        } else {
            (a, b)
        };
        pb.start = lo;
        pb.end = hi;
    }
}

/// Reads a 3-component vector attribute (`color3f` / `double3` / `float3`
/// and `Vec<f32>`/`Vec<f64>` array forms) from a USD prim as a Bevy
/// `Vec3` (f32). Thin wrapper over [`read_vec3_f64`] — reused by
/// downstream crates (e.g. `lunco-usd-sim`'s shader authoring) so there
/// is one canonical vec3 reader. `None` if absent or unconvertible.
pub fn get_attribute_as_vec3(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Option<Vec3> {
    read_vec3_f64(reader, path, attr).map(|v| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32))
}

/// Attach the generic script/driver programs a prim carries to `entity`.
///
/// Program resolution happens before the one-program-per-owner check. Modelica
/// facets in a `CollectionAPI:components` network and BehaviorTree programs are
/// owned by their respective projections; they are not generic script siblings.
/// This is the boundary that prevents a physical network's component count from
/// becoming a false duplicate-program diagnostic.
fn attach_programs<R: UsdRead>(
    reader: &R,
    owner: &SdfPath,
    entity: Entity,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    asset_server: &AssetServer,
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

        match (resolved.backend, resolved.source) {
            (program::ProgramBackend::Builtin, program::ProgramSource::Id(id)) => {
                commands
                    .entity(entity)
                    .try_insert(lunco_core::programs::ProgramDriverId(id));
            }
            (program::ProgramBackend::Rhai, program::ProgramSource::Code(source)) => {
                commands
                    .entity(entity)
                    .try_insert(lunco_core::EmbeddedScenarioSource(source));
            }
            (program::ProgramBackend::Rhai, program::ProgramSource::Asset(asset)) => {
                let asset = resolve_stage_asset_path(asset_server, stage_id, &asset);
                commands
                    .entity(entity)
                    .try_insert(lunco_core::EmbeddedScenarioPath(asset));
            }
            _ => unreachable!("generic program resolution returned a foreign source"),
        }
    }
}

/// USD `xformOp:rotateXYZ` (Euler XYZ, **degrees** as authored) → Bevy
/// `Quat` (radians). Canonical so the Euler order/units live in one
/// place across both consumers.
pub fn euler_xyz_deg_to_quat(deg: Vec3) -> Quat {
    Quat::from_euler(
        EulerRot::XYZEx,
        deg.x.to_radians(),
        deg.y.to_radians(),
        deg.z.to_radians(),
    )
}

/// USD rotation xform-ops, in sampler precedence: the quaternion `orient`, then
/// the six Euler-order triples, then the single-axis scalars. A prim normally
/// authors exactly one; when several are present they compose in this order
/// (`local_rotation_at`).
pub const ROTATION_OPS: [&str; 10] = [
    "xformOp:orient",
    "xformOp:rotateXYZ",
    "xformOp:rotateXZY",
    "xformOp:rotateYXZ",
    "xformOp:rotateYZX",
    "xformOp:rotateZXY",
    "xformOp:rotateZYX",
    "xformOp:rotateX",
    "xformOp:rotateY",
    "xformOp:rotateZ",
];

/// Map a USD Euler-order op name + authored **degrees** (`float3`, each
/// component the angle about that axis) to a Bevy `Quat`. The op-name letter
/// order is the application sequence, about the FIXED (extrinsic) axes — USD's
/// row-vector `rx*ry*rz` composition, so glam's `*Ex` orders. `None` for a
/// non-Euler-order op name.
fn euler_op_to_quat(op: &str, deg: Vec3) -> Option<Quat> {
    let (x, y, z) = (deg.x.to_radians(), deg.y.to_radians(), deg.z.to_radians());
    let q = match op {
        "xformOp:rotateXYZ" => Quat::from_euler(EulerRot::XYZEx, x, y, z),
        "xformOp:rotateXZY" => Quat::from_euler(EulerRot::XZYEx, x, z, y),
        "xformOp:rotateYXZ" => Quat::from_euler(EulerRot::YXZEx, y, x, z),
        "xformOp:rotateYZX" => Quat::from_euler(EulerRot::YZXEx, y, z, x),
        "xformOp:rotateZXY" => Quat::from_euler(EulerRot::ZXYEx, z, x, y),
        "xformOp:rotateZYX" => Quat::from_euler(EulerRot::ZYXEx, z, y, x),
        _ => return None,
    };
    Some(q)
}

/// A USD quaternion value (`quatf`/`quatd`/`quath`) → Bevy `Quat`. USD authors
/// `(w, x, y, z)`; Bevy is `(x, y, z, w)`. Half-precision components convert via
/// `f16::to_f32` (no raw `f16` arithmetic in this crate).
fn quat_from_value(v: &Value) -> Option<Quat> {
    match v {
        Value::Quatf(q) => Some(Quat::from_xyzw(q.x, q.y, q.z, q.w)),
        Value::Quatd(q) => Some(Quat::from_xyzw(
            q.x as f32, q.y as f32, q.z as f32, q.w as f32,
        )),
        Value::Quath(q) => Some(Quat::from_xyzw(
            q.x.to_f32(),
            q.y.to_f32(),
            q.z.to_f32(),
            q.w.to_f32(),
        )),
        _ => None,
    }
}

/// A scalar numeric attribute (`float`/`double`, or integer-authored angles) at
/// time `time` (timeSamples-or-default). The int fallback avoids the silent-`None`
/// trap when an angle is authored as a bare integer (`rotateZ = 90`). `None` when
/// absent or non-numeric.
fn read_scalar_f32_at(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
    time: f64,
) -> Option<f32> {
    match reader.attr_value_at(path, attr, time)? {
        Value::Float(value) => Some(value),
        Value::Double(value) => Some(value as f32),
        Value::Int(value) => Some(value as f32),
        Value::Int64(value) => Some(value as f32),
        _ => None,
    }
}

/// Composed local **rotation** at time code `time` from whatever rotation
/// xform-op(s) the prim authors: quaternion `orient` (slerped), else an
/// Euler-order triple (`rotateXYZ`…`rotateZYX`), else single-axis `rotateX/Y/Z`
/// composed about X then Y then Z. Each channel reads its `default` when static,
/// so this serves both load-time decode (any `time`) and the animation sampler.
/// `None` when the prim authors no rotation op.
pub fn local_rotation_at(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    time: f64,
) -> Option<Quat> {
    // 1. Quaternion orient wins.
    if let Some(q) = reader
        .attr_value_at(path, "xformOp:orient", time)
        .and_then(|v| quat_from_value(&v))
    {
        return Some(q);
    }
    // 2. An Euler-order triple (degrees).
    for op in &ROTATION_OPS[1..7] {
        if let Some(v) = read_vec3_f64_at(reader, path, op, time) {
            return euler_op_to_quat(op, Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32));
        }
    }
    // 3. Single-axis scalars, composed (rotate about X, then Y, then Z).
    let mut q = Quat::IDENTITY;
    let mut any = false;
    for (op, axis) in [
        ("xformOp:rotateX", Vec3::X),
        ("xformOp:rotateY", Vec3::Y),
        ("xformOp:rotateZ", Vec3::Z),
    ] {
        if let Some(a) = read_scalar_f32_at(reader, path, op, time) {
            q = Quat::from_axis_angle(axis, a.to_radians()) * q;
            any = true;
        }
    }
    any.then_some(q)
}

/// `xformOp:transform` (matrix4d) at time `time`, decomposed to a Bevy
/// `Transform`. USD matrices are row-major / row-vector with translation in the
/// last row — exactly glam's column-major / column-vector layout transposed, and
/// the two transposes cancel, so the raw 16 elements feed `Mat4::from_cols_array`
/// directly. `None` when no `xformOp:transform` is authored.
pub fn read_matrix_transform_at(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    time: f64,
) -> Option<Transform> {
    match reader.attr_value_at(path, "xformOp:transform", time)? {
        Value::Matrix4d(m) => {
            let cols: [f32; 16] = std::array::from_fn(|i| m.0[i] as f32);
            Some(Transform::from_matrix(Mat4::from_cols_array(&cols)))
        }
        _ => None,
    }
}

/// True iff any rotation xform-op carries `timeSamples` (so the sampler must
/// recompose the prim's rotation this frame).
fn prim_rotation_animated(reader: &impl UsdRead, path: &SdfPath) -> bool {
    ROTATION_OPS
        .iter()
        .any(|op| attr_has_time_samples(reader, path, op))
}

/// The prim's authored `xformOpOrder` (the ordered op-token list), or `None`
/// when unauthored or empty. When authored it is the **authoritative** op
/// sequence — [`compose_xform_order_at`] honors it exactly, including non-TRS
/// orders that no hand-written decomposition should guess.
fn read_xform_op_order(reader: &dyn read::UsdReadObject, path: &SdfPath) -> Option<Vec<String>> {
    let order: Vec<String> = match reader.attr_value(path, "xformOpOrder")? {
        Value::TokenVec(v) => v.iter().map(|t| t.to_string()).collect(),
        Value::StringVec(v) => v,
        Value::TokenListOp(op) => op.flatten().into_iter().map(|t| t.to_string()).collect(),
        Value::StringListOp(op) => op.flatten(),
        _ => return None,
    };
    (!order.is_empty()).then_some(order)
}

/// Return whether an ordered USD xform token names a standard `UsdGeomXformOp`
/// type. Suffixes are legal and identify independent ops of the same type, so
/// validation checks the type prefix rather than accepting only the handful of
/// unsuffixed spellings emitted by our authoring helpers.
fn is_valid_xform_op_token(op: &str, index: usize) -> bool {
    let (inverted, base) = match op.strip_prefix("!invert!") {
        Some(base) => (true, base),
        None => (false, op),
    };
    if base == RESET_XFORM_STACK {
        return !inverted && index == 0;
    }
    if inverted && base.starts_with('!') {
        return false;
    }
    [
        "xformOp:translate",
        "xformOp:scale",
        "xformOp:transform",
        "xformOp:orient",
        "xformOp:rotateX",
        "xformOp:rotateY",
        "xformOp:rotateZ",
        "xformOp:rotateXYZ",
        "xformOp:rotateXZY",
        "xformOp:rotateYXZ",
        "xformOp:rotateYZX",
        "xformOp:rotateZXY",
        "xformOp:rotateZYX",
    ]
    .iter()
    .any(|kind| base == *kind || base.strip_prefix(kind).is_some_and(|s| s.starts_with(':')))
}

/// Validate the structural part of an authored xform stack before delegating
/// numeric composition to OpenUSD. OpenUSD currently ignores an unknown op
/// token or an op whose attribute is absent in this code path, which would turn
/// malformed authored placement data into an identity transform. That is not a
/// USD semantic default and is unsafe for physics/spawn consumers.
fn valid_xform_op_order<R: UsdRead>(reader: &R, path: &SdfPath, order: &[String]) -> bool {
    order.iter().enumerate().all(|(index, op)| {
        if !is_valid_xform_op_token(op, index) {
            return false;
        }
        let base = op.strip_prefix("!invert!").unwrap_or(op);
        base == RESET_XFORM_STACK || reader.has_authored_attribute(path, base)
    })
}

/// True iff the prim authors a non-empty `xformOpOrder` (so its local transform
/// is defined by the ordered op stack, not the implicit TRS fallback).
fn has_xform_op_order(reader: &dyn read::UsdReadObject, path: &SdfPath) -> bool {
    read_xform_op_order(reader, path).is_some()
}

/// An [`openusd::schemas::geom::Xformable`] view over ANY prim, unchecked —
/// the transform decoders compose whatever prim carries an `xformOpOrder`, not
/// just those typed `Xform` (a `Mesh`, a `Camera`, a schema-less `over` are all
/// xformable). Mirrors the C++ `UsdGeomXformable(prim)` constructor.
struct XformablePrim(openusd::usd::Prim);

impl openusd::usd::SchemaBase for XformablePrim {
    const KIND: openusd::usd::SchemaKind = openusd::usd::SchemaKind::AbstractTyped;

    fn prim(&self) -> &openusd::usd::Prim {
        &self.0
    }
}
impl openusd::schemas::geom::Imageable for XformablePrim {}
impl openusd::schemas::geom::Xformable for XformablePrim {}

/// The `!resetXformStack!` sentinel, as UsdGeomXformable spells it in
/// `xformOpOrder`.
const RESET_XFORM_STACK: &str = "!resetXformStack!";

/// How far up a `ChildOf` chain the stage-root walk will look before giving up.
/// USD prim hierarchies are shallow; a chain deeper than this is a cycle or a
/// mid-load half-built ancestry, and either way is not worth spinning on.
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
/// [`compose_xform_order_at`] already rejects with [`TransformReadError`].
fn prim_resets_xform_stack<R: UsdRead>(reader: &R, path: &SdfPath) -> bool {
    read_xform_op_order(reader, path)
        .is_some_and(|order| order.first().is_some_and(|op| op == RESET_XFORM_STACK))
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
            "[usd-bevy] {} opens with {RESET_XFORM_STACK} — detaching from its USD ancestry \
             onto the stage world frame ({anchor:?})",
            prim.path
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

/// A USD transform stack was authored but could not be composed safely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformReadError {
    pub prim: String,
}

impl std::fmt::Display for TransformReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed authored transform at {}", self.prim)
    }
}

impl std::error::Error for TransformReadError {}

fn malformed_transform(path: &SdfPath) -> TransformReadError {
    TransformReadError {
        prim: path.as_str().to_owned(),
    }
}

/// Compose the prim's local `Transform` at time `time` from its `xformOpOrder`,
/// via openusd's spec implementation
/// ([`Xformable::local_to_parent_transform`](openusd::schemas::geom::Xformable::local_to_parent_transform)):
/// op order, `!invert!` prefixes, the leading `!resetXformStack!` sentinel, the
/// full op-kind set (translate/scale and their single-axis forms, the six Euler
/// orders, `orient`, `transform`), all composed in f64 before the one narrowing
/// to Bevy's `Transform`. USD matrices are row-major / row-vector — glam's
/// column-major / column-vector layout transposed, and the two transposes
/// cancel (see [`read_matrix_transform_at`]), so the raw 16 elements feed
/// `Mat4::from_cols_array` directly. `Ok(None)` means no transform stack is
/// authored. A malformed authored stack is an error; it must not be converted
/// into identity or the entity's previous transform.
pub fn compose_xform_order_at<R: UsdRead>(
    reader: &R,
    path: &SdfPath,
    time: f64,
) -> Result<Option<Transform>, TransformReadError> {
    reader.local_transform_at(path, time)
}

/// Compose a live OpenUSD transform. This is the StageView implementation of
/// the shared [`UsdRead::local_transform_at`] contract; the initial asset
/// projection uses its worker-produced owned result instead.
pub(crate) fn compose_live_xform_order_at(
    reader: &StageView<'_>,
    path: &SdfPath,
    time: f64,
) -> Result<Option<Transform>, TransformReadError> {
    use openusd::schemas::geom::Xformable as _;
    let Some(order) = read_xform_op_order(reader, path) else {
        return if reader.has_authored_attribute(path, "xformOpOrder")
            && !authored_empty_xform_op_order(reader, path)
        {
            Err(malformed_transform(path))
        } else {
            Ok(None)
        };
    };
    if !valid_xform_op_order(reader, path, &order) {
        return Err(malformed_transform(path));
    }
    let m = XformablePrim(reader.stage().prim(path.clone()))
        .local_to_parent_transform(time)
        .map_err(|_| malformed_transform(path))?;
    let cols: [f32; 16] = std::array::from_fn(|i| m.0[i] as f32);
    let raw = Transform::from_matrix(Mat4::from_cols_array(&cols));
    let convention = stage_convention(reader).map_err(|_| malformed_transform(path))?;
    Ok(Some(convention.local_transform(raw)))
}

/// The prim's full local `Transform` at time `time`, **in the canonical frame**:
/// `xformOpOrder` composition when authored (authoritative). `Ok(None)` when the
/// prim authors no transform stack; malformed authored data is an error.
/// Shared by the static decoder and the animation sampler so both agree.
///
/// **Units/axes convert here** (`docs/architecture/41-axes-and-units.md`): the
/// raw stage-frame transform is conjugated by the stage's
/// [`ConventionTransform`](units::ConventionTransform), so a Z-up / centimetre
/// stage (Omniverse, Isaac Sim) yields an upright, metre-scaled local transform.
/// A canonical stage (all our own assets) takes the identity path — unchanged.
/// Every downstream consumer (visual sync, avian colliders, mounts, the gizmo)
/// funnels through here, so none of them sees stage units.
pub fn local_transform_at(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    time: f64,
) -> Result<Option<Transform>, TransformReadError> {
    reader.local_transform_at(path, time)
}

/// [`local_transform_at`] **before** the canonical conversion — the transform as
/// authored, in the stage's own frame and units. Private: no consumer may hold a
/// raw spatial value (doc 41 — "visibility is the guardrail").
/// Canonical local-transform decode via [`local_transform_at`]. An omitted
/// transform stack is the USD identity; malformed authored data is returned to
/// the caller instead of becoming identity. The returned transform is complete:
/// translation, rotation, and scale are the result of the authored
/// `xformOpOrder`, after stage-axis and stage-unit conversion.
pub fn read_transform_from_usd(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
) -> Result<Transform, TransformReadError> {
    match local_transform_at(reader, path, 0.0) {
        Ok(Some(tf)) => Ok(tf),
        Ok(None) => Ok(Transform::IDENTITY),
        Err(error) => Err(error),
    }
}

/// Resolve the inherited USD Imageable visibility and purpose on a live stage.
/// The same result is captured by the worker-produced projection plan.
pub(crate) fn stage_prim_is_invisible_or_guide(reader: &StageView<'_>, path: &SdfPath) -> bool {
    use openusd::schemas::geom::Imageable as _;
    let imageable = XformablePrim(reader.stage().prim(path.clone()));
    imageable
        .compute_visibility()
        .map(|value| value == openusd::schemas::geom::Visibility::Invisible)
        .unwrap_or(false)
        || imageable
            .compute_purpose()
            .map(|value| value == openusd::schemas::geom::Purpose::Guide)
            .unwrap_or(false)
}

/// Small gap (metres) left between an asset's lowest collision point and the
/// terrain at spawn — a "skin width" so a body never spawns interpenetrating the
/// ground (which the solver would resolve by ejecting it). Physics is held until
/// the terrain collider is ready, so the object then settles this last gap gently.
/// Shared by every spawn path (GUI ghost + the authoritative `SpawnEntity`
/// handler) so they place identically.
pub const SPAWN_GROUND_CLEARANCE: f64 = 0.05;

/// Axis-aligned bounding box of an asset's COLLISION geometry, in the asset's own
/// root reference frame — the general, wheel-free basis for spawn placement.
///
/// Walks the composed USD stage from `root_prim` and, for every active gprim that
/// applies the standard `UsdPhysicsCollisionAPI` and whose
/// `physics:collisionEnabled` is not `false`, folds that shape's local bounding
/// box (its 8 corners transformed into the root frame) into a running min/max.
/// Nested rigid bodies and authored vehicle wheels are ownership boundaries and
/// are not folded into the root body. Shape dimensions come from the shared
/// [`read_shape_dims`], so the box can't drift from the avian collider built off
/// the same attributes. The result's
/// [`rest_depth`](ObjectAabb::rest_depth) (`-min.y`) is the distance from the root
/// origin down to the lowest collision point: lift a spawn by it and the object
/// rests ON the ground with no part buried — for ANY asset (lander, rover, prop),
/// no per-asset placement tuning, no dependency on wheels. Computed off the
/// same composed stage the live entity is instantiated from, so the placement
/// solver and the physics body can never disagree.
///
/// Returns `Ok(None)` when no collision geometry is found (a pure-visual prop).
/// Native `UsdGeomMesh` collision topology is included using the same indexed
/// points consumed by Avian. Malformed authored collision data is an error, not
/// an empty footprint: callers must not replace it with a spawn heuristic.
#[derive(Clone, Copy, Debug, Default)]
pub struct ObjectAabb {
    pub min: bevy::math::DVec3,
    pub max: bevy::math::DVec3,
}

/// A composed collision tree could not provide a trustworthy placement AABB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollisionAabbError {
    InvalidRootPath(String),
    MalformedTransform { prim: String },
    InvalidCollisionEnabled { prim: String },
    MalformedPrimitive { prim: String, type_name: String },
}

impl std::fmt::Display for CollisionAabbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRootPath(path) => write!(f, "invalid collision root path {path}"),
            Self::MalformedTransform { prim } => {
                write!(f, "malformed authored transform at {prim}")
            }
            Self::InvalidCollisionEnabled { prim } => {
                write!(f, "invalid authored physics:collisionEnabled at {prim}")
            }
            Self::MalformedPrimitive { prim, type_name } => {
                write!(f, "malformed {type_name} collision data at {prim}")
            }
        }
    }
}

impl std::error::Error for CollisionAabbError {}

impl ObjectAabb {
    /// Half-width along X (metres) — the placement solver samples terrain at
    /// `±half_w` from the click point to fit a slope normal.
    pub fn half_w(&self) -> f64 {
        (self.max.x - self.min.x) * 0.5
    }
    /// Half-length along Z (metres).
    pub fn half_l(&self) -> f64 {
        (self.max.z - self.min.z) * 0.5
    }
    /// Root origin → lowest collision point. Lift a spawn by this so the object
    /// rests on the surface. Non-negative for any geometry reaching below origin.
    pub fn rest_depth(&self) -> f64 {
        -self.min.y
    }
}

/// Derive the [`ObjectAabb`] of an asset by walking the composed USD stage from
/// `root_prim` (e.g. `"/DescentLander"`). See [`ObjectAabb`].
pub fn collision_aabb(
    reader: &StageView<'_>,
    root_prim: &str,
) -> Result<Option<ObjectAabb>, CollisionAabbError> {
    let root = SdfPath::new(root_prim)
        .map_err(|_| CollisionAabbError::InvalidRootPath(root_prim.to_owned()))?;
    let root_tf = collision_local_transform(reader, &root)?;
    let mut candidates = vec![(root.clone(), root_tf)];
    gather_collision_aabb_candidates(reader, &root, root_tf, &mut candidates)?;
    let has_proxy = candidates
        .iter()
        .any(|(path, _)| effective_purpose(reader, path) == Purpose::Proxy);
    let mut acc: Option<(bevy::math::DVec3, bevy::math::DVec3)> = None;
    for (path, world_tf) in candidates {
        if reader
            .text(&path, "lunco:triggerZone")
            .is_some_and(|zone| !zone.trim().is_empty())
        {
            continue;
        }
        let purpose = effective_purpose(reader, &path);
        if purpose == Purpose::Guide || (has_proxy && purpose == Purpose::Render) {
            continue;
        }
        if !reader.has_api_schema(&path, openusd::schemas::physics::tokens::API_COLLISION) {
            continue;
        }
        let collides = match reader.boolean(&path, "physics:collisionEnabled") {
            Some(value) => value,
            None if reader.has_authored_attribute(&path, "physics:collisionEnabled") => {
                return Err(CollisionAabbError::InvalidCollisionEnabled {
                    prim: path.as_str().to_owned(),
                });
            }
            None => true,
        };
        if !collides {
            continue;
        }
        let ty = reader.type_name(&path).unwrap_or_default();
        let corners = local_shape_corners(reader, &path, &ty).ok_or_else(|| {
            CollisionAabbError::MalformedPrimitive {
                prim: path.as_str().to_owned(),
                type_name: ty.clone(),
            }
        })?;
        for c in corners {
            let w = world_tf.transform_point(c.as_vec3()).as_dvec3();
            match acc.as_mut() {
                Some((min, max)) => {
                    *min = min.min(w);
                    *max = max.max(w);
                }
                None => acc = Some((w, w)),
            }
        }
    }
    Ok(acc.map(|(min, max)| ObjectAabb { min, max }))
}

/// Read a collision-tree transform while distinguishing USD's identity for an
/// unauthored xform stack from a malformed authored stack. The latter must not
/// become identity: doing so changes the computed rest depth and can bury a
/// spawned body while the malformed asset appears loadable.
fn collision_local_transform(
    reader: &StageView<'_>,
    path: &SdfPath,
) -> Result<Transform, CollisionAabbError> {
    match local_transform_at(reader, path, 0.0) {
        Ok(Some(transform)) => Ok(transform),
        Ok(None) => Ok(Transform::IDENTITY),
        Err(_) => Err(CollisionAabbError::MalformedTransform {
            prim: path.as_str().to_owned(),
        }),
    }
}

/// An authored empty `xformOpOrder` is the valid USD identity stack. It must be
/// distinguished from an authored value of the wrong type, which is malformed.
fn authored_empty_xform_op_order(reader: &StageView<'_>, path: &SdfPath) -> bool {
    match reader.attr_value(path, "xformOpOrder") {
        Some(Value::TokenVec(values)) => values.is_empty(),
        Some(Value::StringVec(values)) => values.is_empty(),
        Some(Value::TokenListOp(op)) => op.flatten().is_empty(),
        Some(Value::StringListOp(op)) => op.flatten().is_empty(),
        _ => false,
    }
}

/// DFS helper for [`collision_aabb`]: collect the same ownership candidates as
/// the Avian compound-body reader. Transforms are composed in the root frame;
/// nested rigid bodies and vehicle wheels remain their own physics owners.
fn gather_collision_aabb_candidates(
    reader: &StageView<'_>,
    path: &SdfPath,
    world_tf: Transform,
    out: &mut Vec<(SdfPath, Transform)>,
) -> Result<(), CollisionAabbError> {
    for child in reader.children(path) {
        if !reader.is_active(&child) {
            continue;
        }
        if reader.has_api_schema(&child, openusd::schemas::physics::tokens::API_RIGID_BODY)
            || reader
                .real_f32(&child, "physxVehicleWheel:radius")
                .is_some()
        {
            continue;
        }
        let local = collision_local_transform(reader, &child)?;
        let child_world = world_tf * local;
        out.push((child.clone(), child_world));
        gather_collision_aabb_candidates(reader, &child, child_world, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod collision_aabb_tests {
    use super::*;

    fn parse(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("collision.usda", source))
            .expect("build collision stage")
    }

    #[test]
    fn unauthored_transforms_use_usd_identity() {
        let stage = parse(
            r#"#usda 1.0
(
    metersPerUnit = 1
)
def Xform "Root"
{
    def Cube "Body" (
        prepend apiSchemas = ["PhysicsCollisionAPI"]
    )
    {
        double size = 2
    }
}
"#,
        );
        let aabb = collision_aabb(&stage.view(), "/Root")
            .expect("collision AABB derivation")
            .expect("cube collision AABB");
        assert_eq!(aabb.min, bevy::math::DVec3::splat(-1.0));
        assert_eq!(aabb.max, bevy::math::DVec3::splat(1.0));
    }

    #[test]
    fn visual_primitive_without_collision_api_is_not_a_collision_aabb() {
        let stage = parse(
            r#"#usda 1.0
(
    metersPerUnit = 1
)
def Xform "Root"
{
    def Cube "Visual"
    {
        double size = 2
    }
}
"#,
        );
        assert!(
            collision_aabb(&stage.view(), "/Root")
                .expect("collision AABB derivation")
                .is_none(),
            "visual geometry without UsdPhysicsCollisionAPI must not affect placement"
        );
    }

    #[test]
    fn proxy_collision_geometry_excludes_render_geometry() {
        let stage = parse(
            r#"#usda 1.0
(
    metersPerUnit = 1
)
def Xform "Root"
{
    def Cube "Render" (
        prepend apiSchemas = ["PhysicsCollisionAPI"]
    )
    {
        uniform token purpose = "render"
        double size = 10
    }
    def Cube "Proxy" (
        prepend apiSchemas = ["PhysicsCollisionAPI"]
    )
    {
        uniform token purpose = "proxy"
        double size = 2
    }
}
"#,
        );
        let aabb = collision_aabb(&stage.view(), "/Root")
            .expect("collision AABB derivation")
            .expect("proxy collision AABB");
        assert_eq!(aabb.min, bevy::math::DVec3::splat(-1.0));
        assert_eq!(aabb.max, bevy::math::DVec3::splat(1.0));
    }

    #[test]
    fn native_mesh_collision_geometry_is_used_for_placement() {
        let stage = parse(
            r#"#usda 1.0
(
    metersPerUnit = 1
)
def Xform "Root"
{
    def Mesh "Mesh" (
        prepend apiSchemas = ["PhysicsCollisionAPI"]
    )
    {
        point3f[] points = [(-2, -1, -3), (4, 5, 6), (1, 2, -1)]
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
    }
}
"#,
        );
        let aabb = collision_aabb(&stage.view(), "/Root")
            .expect("collision AABB derivation")
            .expect("mesh collision AABB");
        assert_eq!(aabb.min, bevy::math::DVec3::new(-2.0, -1.0, -3.0));
        assert_eq!(aabb.max, bevy::math::DVec3::new(4.0, 5.0, 6.0));
    }

    #[test]
    fn malformed_authored_transform_is_not_replaced_with_identity() {
        let stage = parse(
            r#"#usda 1.0
def Xform "Root"
{
    uniform token[] xformOpOrder = ["xformOp:unsupported"]
    def Cube "Body"
    {
        double size = 2
    }
}
"#,
        );
        assert!(
            collision_aabb(&stage.view(), "/Root").is_err(),
            "a malformed authored transform must reject spawn AABB derivation"
        );
        let root = SdfPath::new("/Root").unwrap();
        assert!(
            local_transform_at(&stage.view(), &root, 0.0).is_err(),
            "the shared transform reader must preserve malformed-data errors"
        );
        assert!(
            read_transform_from_usd(&stage.view(), &root).is_err(),
            "the identity-producing convenience path must not hide malformed data"
        );
    }

    #[test]
    fn malformed_authored_collision_data_is_not_treated_as_omitted() {
        let stage = parse(
            r#"#usda 1.0
def Xform "Root"
{
    def Cube "Body" (
        prepend apiSchemas = ["PhysicsCollisionAPI"]
    )
    {
        string size = "not a number"
    }
}
"#,
        );
        assert!(
            collision_aabb(&stage.view(), "/Root").is_err(),
            "malformed authored dimensions must reject spawn AABB derivation"
        );
    }
}

/// The 8 corners of a primitive shape's local bounding box, centred at its origin.
/// `None` for a non-shape prim (Xform/Scope/Mesh/…) so the walk skips it. Uses the
/// shared [`read_shape_dims`], and rotates a round shape's box onto its authored
/// `axis` token, so the box matches the collider the shape produces.
fn local_shape_corners(
    reader: &StageView<'_>,
    path: &SdfPath,
    ty: &str,
) -> Option<Vec<bevy::math::DVec3>> {
    if ty == "Mesh" {
        let approximation = reader.text(path, "physics:approximation");
        if approximation
            .as_deref()
            .is_some_and(|value| !matches!(value, "none" | "convexHull" | "convexDecomposition"))
            || (approximation.is_none()
                && reader.has_authored_attribute(path, "physics:approximation"))
        {
            return None;
        }
        let (vertices, _) = read_usd_mesh_indexed(reader, path)?;
        return Some(
            vertices
                .into_iter()
                .map(|[x, y, z]| bevy::math::DVec3::new(x as f64, y as f64, z as f64))
                .collect(),
        );
    }
    // Half-extents in the shape's Y-axial local frame; `axial` = the `axis` token
    // may re-orient it (round shapes only — box/sphere/plane are axis-agnostic).
    let (half, axial) = match read_shape_dims(reader, path, ty)? {
        ShapeDims::Cube { size } => (bevy::math::DVec3::splat(size * 0.5), false),
        ShapeDims::Sphere { radius } => (bevy::math::DVec3::splat(radius), false),
        ShapeDims::Cylinder { radius, height } | ShapeDims::Cone { radius, height } => {
            (bevy::math::DVec3::new(radius, height * 0.5, radius), true)
        }
        // Capsule bounds include the hemispherical caps: half-length = height/2 + r.
        ShapeDims::Capsule { radius, height } => (
            bevy::math::DVec3::new(radius, height * 0.5 + radius, radius),
            true,
        ),
        ShapeDims::Plane { width, length } => (
            bevy::math::DVec3::new(width * 0.5, 0.0005, length * 0.5),
            false,
        ),
    };
    let axis_q = if axial {
        read_primitive_axis(reader, path, ty)
            .and_then(|axis| usd_axis_to_quat(&axis))
            .unwrap_or(Quat::IDENTITY)
    } else {
        Quat::IDENTITY
    };
    let mut corners = Vec::with_capacity(8);
    for sx in [-1.0_f64, 1.0] {
        for sy in [-1.0_f64, 1.0] {
            for sz in [-1.0_f64, 1.0] {
                let local = axis_q
                    * Vec3::new(
                        (half.x * sx) as f32,
                        (half.y * sy) as f32,
                        (half.z * sz) as f32,
                    );
                corners.push(local.as_dvec3());
            }
        }
    }
    Some(corners)
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

/// Read the standard `UsdGeom` axis token for a primitive. The schema fallback
/// is `Z`; an authored value outside the schema's allowed tokens is invalid and
/// must not silently become an identity rotation.
pub fn read_primitive_axis(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    type_name: &str,
) -> Option<String> {
    if !matches!(type_name, "Cylinder" | "Cone" | "Capsule" | "Plane") {
        return Some("Z".to_string());
    }
    match reader.text(path, "axis") {
        Some(axis) if matches!(axis.as_str(), "X" | "Y" | "Z") => Some(axis),
        Some(axis) => {
            error!(
                "[usd-bevy] {} has invalid {} axis token `{axis}`; expected X, Y, or Z",
                path.as_str(),
                type_name
            );
            None
        }
        None if reader.has_authored_attribute(path, "axis")
            || !reader.connections(path, "axis").is_empty() =>
        {
            error!(
                "[usd-bevy] {} has an authored {} axis with an unsupported value type",
                path.as_str(),
                type_name
            );
            None
        }
        None => Some("Z".to_string()),
    }
}

/// Dimensions of a USD primitive shape prim, with the spec-compliant
/// defaults applied. One home (CQ-102) so the avian collider and the
/// bevy mesh never desync.
///
/// `Cube::size` is the ONLY form. `UsdGeomCube` declares exactly one dimension
/// attribute (`double size`, default 2.0) — a non-uniform box is a `size` plus an
/// `xformOp:scale`. `width`/`height`/`depth` on a Cube are not UsdGeomCube
/// attributes and are not read: accepting them would let a scene encode its box
/// dimensions in a form no other DCC reads. (`Plane::width`/`length` below ARE
/// real UsdGeomPlane attributes — that is the difference.)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeDims {
    Cube { size: f64 },
    Sphere { radius: f64 },
    Cylinder { radius: f64, height: f64 },
    Cone { radius: f64, height: f64 },
    Capsule { radius: f64, height: f64 },
    Plane { width: f64, length: f64 },
}

/// Rendering-only provenance for a USD built-in primitive mesh. The dimensions
/// remain owned by [`ShapeDims`] so a quality change can rebuild the mesh without
/// reopening the USD stage or duplicating the dimension reader.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct UsdPrimitiveMesh(ShapeDims);

/// Rendering-only marker for a USD curve tube. The authored curve remains
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

/// Rebuild curve-tube meshes when the user changes Graphics tessellation.
/// Invalid settings leave the existing mesh in place and are reported; no lower
/// quality profile is selected implicitly.
fn retessellate_curve_meshes_on_quality_change(
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<(&UsdPrimPath, &Mesh3d, Option<&Name>), With<UsdCurveMesh>>,
    quality: Res<lunco_render::RenderingQualitySettings>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
) {
    if !quality.is_changed() {
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

    fn stage(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("curve.usda", source))
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

#[cfg(test)]
mod primitive_attribute_tests {
    use super::*;

    fn parse(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("primitive.usda", source))
            .expect("build primitive stage")
    }

    #[test]
    fn omitted_dimensions_use_usd_defaults_but_invalid_authored_values_are_rejected() {
        let stage = parse(
            r#"#usda 1.0
(
    metersPerUnit = 1
)
def Xform "World"
{
    def Sphere "Default" {}
    def Cylinder "Negative"
    {
        double radius = -1
        double height = 2
    }
    def Cylinder "WrongType"
    {
        string radius = "not a number"
        double height = 2
    }
    def Cylinder "BadAxis"
    {
        double radius = 1
        double height = 2
        token axis = "Q"
    }
}
"#,
        );
        let reader = stage.view();
        assert_eq!(
            read_shape_dims(&reader, &SdfPath::new("/World/Default").unwrap(), "Sphere"),
            Some(ShapeDims::Sphere { radius: 1.0 })
        );
        assert!(read_shape_dims(
            &reader,
            &SdfPath::new("/World/Negative").unwrap(),
            "Cylinder"
        )
        .is_none());
        assert!(read_shape_dims(
            &reader,
            &SdfPath::new("/World/WrongType").unwrap(),
            "Cylinder"
        )
        .is_none());
        assert!(read_shape_dims(
            &reader,
            &SdfPath::new("/World/BadAxis").unwrap(),
            "Cylinder"
        )
        .is_none());
    }
}

/// Read the dimensions of a USD primitive shape prim, **in metres**. `type_name`
/// is the prim's `typeName` token (callers already have it). Returns `None` for
/// an unsupported type. **The defaults here are the single source of
/// truth** for both `lunco-usd-avian` (→ `Collider`) and this crate
/// (→ `Mesh`); changing one here changes both, so they can't drift.
///
/// Every dimension is scaled by the stage's `metersPerUnit`
/// ([`ConventionTransform::length`]) — a centimetre stage's `radius = 50` reads
/// back `0.5` m. Identity (and therefore unchanged) for a metre stage.
pub fn read_shape_dims(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    type_name: &str,
) -> Option<ShapeDims> {
    let dims = read_shape_dims_raw(reader, path, type_name)?;
    let conv = stage_convention(reader).ok()?;
    if conv.is_identity() {
        return Some(dims);
    }
    let m = |x: f64| conv.length(x);
    Some(match dims {
        ShapeDims::Cube { size } => ShapeDims::Cube { size: m(size) },
        ShapeDims::Sphere { radius } => ShapeDims::Sphere { radius: m(radius) },
        ShapeDims::Cylinder { radius, height } => ShapeDims::Cylinder {
            radius: m(radius),
            height: m(height),
        },
        ShapeDims::Cone { radius, height } => ShapeDims::Cone {
            radius: m(radius),
            height: m(height),
        },
        ShapeDims::Capsule { radius, height } => ShapeDims::Capsule {
            radius: m(radius),
            height: m(height),
        },
        ShapeDims::Plane { width, length } => ShapeDims::Plane {
            width: m(width),
            length: m(length),
        },
    })
}

/// [`read_shape_dims`] **before** the unit conversion — dimensions in the stage's
/// own linear unit. Private (doc 41: no public raw spatial accessor).
fn read_shape_dims_raw(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    type_name: &str,
) -> Option<ShapeDims> {
    read_primitive_axis(reader, path, type_name)?;
    let dims = match type_name {
        "Cube" => ShapeDims::Cube {
            size: read_shape_dimension(reader, path, "size", 2.0)?,
        },
        "Sphere" => ShapeDims::Sphere {
            radius: read_shape_dimension(reader, path, "radius", 1.0)?,
        },
        "Cylinder" => ShapeDims::Cylinder {
            radius: read_shape_dimension(reader, path, "radius", 1.0)?,
            height: read_shape_dimension(reader, path, "height", 2.0)?,
        },
        "Cone" => ShapeDims::Cone {
            radius: read_shape_dimension(reader, path, "radius", 1.0)?,
            height: read_shape_dimension(reader, path, "height", 2.0)?,
        },
        "Capsule" => ShapeDims::Capsule {
            radius: read_shape_dimension(reader, path, "radius", 0.5)?,
            height: read_shape_dimension(reader, path, "height", 1.0)?,
        },
        "Plane" => ShapeDims::Plane {
            width: read_shape_dimension(reader, path, "width", 2.0)?,
            length: read_shape_dimension(reader, path, "length", 2.0)?,
        },
        _ => return None,
    };
    Some(dims)
}

/// Read one positive USD primitive dimension, keeping omitted schema defaults
/// distinct from malformed authored values. A wrong type, non-finite value, or
/// non-positive size rejects the primitive instead of creating a plausible but
/// different mesh/collider.
fn read_shape_dimension(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    name: &str,
    schema_default: f64,
) -> Option<f64> {
    let authored =
        reader.has_authored_attribute(path, name) || !reader.connections(path, name).is_empty();
    match reader.real(path, name) {
        Some(value) if value.is_finite() && value > 0.0 => Some(value),
        Some(value) => {
            error!(
                "[usd-bevy] {} has invalid primitive {} = {value}; expected a finite positive value",
                path.as_str(),
                name
            );
            None
        }
        None if authored => {
            error!(
                "[usd-bevy] {} has authored primitive {} with an unsupported value type",
                path.as_str(),
                name
            );
            None
        }
        None => Some(schema_default),
    }
}

/// Reads an `int[]` / `int64[]` USD array attribute (`Value::IntVec` /
/// `Int64Vec`) as `Vec<i32>`. The fixed-array `TryFrom<Value>` impls don't
/// cover integer arrays, so mesh topology (`faceVertexCounts` /
/// `faceVertexIndices`) is matched directly. `None` if absent or not an int
/// array.
/// A `Mesh` prim's `points`, converted to the canonical frame: `p' = k·Q·p`
/// (see [`units`]). The one place mesh geometry crosses the unit/axis boundary —
/// both the render mesh ([`build_usd_mesh`]) and the physics trimesh
/// ([`read_usd_mesh_indexed`]) read through it, so they cannot disagree.
fn read_mesh_points(reader: &dyn read::UsdReadObject, path: &SdfPath) -> Option<Vec<[f32; 3]>> {
    // `points3`, NOT `scalar::<Vec<[f32; 3]>>`: USD's `points` is `point3f[]` by
    // convention but `point3d[]` is legal and exporters do emit it (coordinates feel
    // like they deserve the precision). A strict `point3f[]` read of a `point3d[]`
    // mesh returns `None`, i.e. "this prim has no geometry" — so the mesh silently
    // does not spawn and nothing is logged. An empty result is `None` here, matching
    // the old contract: a points-less mesh is not a mesh.
    let points = reader.points3(path, "points");
    if points.is_empty() {
        return None;
    }
    let conv = stage_convention(reader).ok()?;
    if conv.is_identity() {
        return Some(points);
    }
    Some(
        points
            .into_iter()
            .map(|p| conv.point(Vec3::from_array(p)).to_array())
            .collect(),
    )
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

/// Build a swept-tube mesh from a `UsdGeomBasisCurves` prim.
///
/// A curve prim with `widths` is a **tube**, not a line: `widths` is a diameter in
/// object space, so the curve is a centerline and the profile is a circle. See
/// [`crate::curve_sweep`] for why the frames are rotation-minimizing rather than
/// Frenet (short version: Frenet is undefined on straight runs, and flips as it
/// approaches them — a habitat is mostly straight pipe).
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
    use crate::camera_path::CurveBasis;
    use crate::curve_sweep::sweep_tube;

    // Canonical-frame points — same conversion the mesh path takes.
    let points = read_mesh_points(reader, path)?;
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
    let widths = match read_curve_real_array(reader, path, "widths") {
        Ok(Some(widths)) if !widths.is_empty() => widths,
        Ok(Some(_)) | Ok(None) => return None,
        Err(()) => {
            error!(
                "[usd-bevy] {} has authored curve widths with an unsupported value type",
                path.as_str()
            );
            return None;
        }
    };

    // Radii are a LENGTH, so they scale with `metersPerUnit` — `conv.length`,
    // not `conv.point`. (`read_mesh_points` already converted the centerline.)
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
    let is_nurbs = reader.type_name(path).as_deref() == Some("NurbsCurves");
    let counts = match read_curve_int_array(reader, path, "curveVertexCounts") {
        Ok(Some(counts)) if !counts.is_empty() => counts,
        Ok(Some(_)) | Ok(None) => {
            error!(
                "[usd-bevy] {} has no usable authored curveVertexCounts; USD requires this topology field",
                path.as_str()
            );
            return None;
        }
        Err(()) => {
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
        let orders = match read_curve_int_array(reader, path, "order") {
            Ok(Some(orders)) if !orders.is_empty() => orders,
            Ok(Some(_)) | Ok(None) => {
                error!(
                    "[usd-bevy] {} has no usable authored NurbsCurves order",
                    path.as_str()
                );
                return None;
            }
            Err(()) => {
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
        let all_knots = match read_curve_real_array(reader, path, "knots") {
            Ok(Some(knots)) if !knots.is_empty() => knots,
            Ok(Some(_)) | Ok(None) | Err(()) => {
                error!(
                    "[usd-bevy] {} has no usable authored NurbsCurves knots",
                    path.as_str()
                );
                return None;
            }
        };
        let point_weights = match read_curve_real_array(reader, path, "pointWeights") {
            Ok(Some(weights)) if weights.len() == points.len() => weights,
            Ok(Some(_)) => {
                error!(
                    "[usd-bevy] {} has pointWeights whose length does not match points",
                    path.as_str()
                );
                return None;
            }
            Ok(None) => Vec::new(),
            Err(()) => {
                error!(
                    "[usd-bevy] {} has authored pointWeights with an unsupported value type",
                    path.as_str()
                );
                return None;
            }
        };
        (CurveBasis::Linear, false, orders, all_knots, point_weights)
    } else {
        let ty = match read_curve_token(reader, path, "type", "cubic", &["linear", "cubic"]) {
            Ok(token) => token,
            Err(()) => return None,
        };
        let basis = if ty == "linear" {
            CurveBasis::Linear
        } else {
            match read_curve_token(reader, path, "basis", "bezier", &["bezier", "catmullRom"]) {
                Ok(token) if token == "bezier" => CurveBasis::Bezier,
                Ok(_) => CurveBasis::CatmullRom,
                Err(()) => return None,
            }
        };
        let wrap = match read_curve_token(
            reader,
            path,
            "wrap",
            "nonperiodic",
            &["nonperiodic", "periodic", "pinned"],
        ) {
            Ok(wrap) => wrap,
            Err(()) => return None,
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

    if quality.curve_samples_per_segment == 0 || quality.curve_radial_segments < 3 {
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
            let sampled = crate::nurbs::sample_nurbs_curve(&pts, &w, order, &knots, steps);
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
                    crate::camera_path::eval_curve(&cvs, basis, periodic, i as f32 / steps as f32)
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

        let Some(tube) = sweep_tube(
            &centerline,
            &seg_radii,
            quality.curve_radial_segments,
            periodic,
        ) else {
            error!(
                "[usd-bevy] {} has a curve segment that cannot be swept into a mesh",
                path.as_str()
            );
            return None;
        };
        merged = Some(match merged {
            None => tube,
            Some(mut acc) => {
                acc.merge(&tube).ok()?;
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
/// **`trimCurve:*` IS honoured** — see [`crate::trim`]. A trimmed patch gets an
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

    let points = read_mesh_points(reader, path)?;
    let u_count = lathe::read_required_nurbs_int(reader, path, "uVertexCount")?;
    let v_count = lathe::read_required_nurbs_int(reader, path, "vVertexCount")?;
    let u_order = lathe::read_required_nurbs_int(reader, path, "uOrder")?;
    let v_order = lathe::read_required_nurbs_int(reader, path, "vOrder")?;
    if u_count < u_order || v_count < v_order {
        error!(
            "[usd-bevy] {} has NurbsPatch order/count mismatch: u {u_count}/{u_order}, v {v_count}/{v_order}",
            path.as_str()
        );
        return None;
    }
    let u_knots = match read_curve_real_array(reader, path, "uKnots") {
        Ok(Some(knots)) if knots.len() == u_count + u_order => knots,
        Ok(Some(_)) | Ok(None) | Err(()) => {
            error!(
                "[usd-bevy] {} has no usable authored uKnots for its NurbsPatch",
                path.as_str()
            );
            return None;
        }
    };
    let v_knots = match read_curve_real_array(reader, path, "vKnots") {
        Ok(Some(knots)) if knots.len() == v_count + v_order => knots,
        Ok(Some(_)) | Ok(None) | Err(()) => {
            error!(
                "[usd-bevy] {} has no usable authored vKnots for its NurbsPatch",
                path.as_str()
            );
            return None;
        }
    };
    let weights = match read_curve_real_array(reader, path, "pointWeights") {
        Ok(Some(weights)) if weights.len() == points.len() => weights,
        Ok(Some(_)) => {
            error!(
                "[usd-bevy] {} has pointWeights whose length does not match its NurbsPatch points",
                path.as_str()
            );
            return None;
        }
        Ok(None) => Vec::new(),
        Err(()) => {
            error!(
                "[usd-bevy] {} has pointWeights with an unsupported value type",
                path.as_str()
            );
            return None;
        }
    };
    let orientation = match read_curve_token(
        reader,
        path,
        "orientation",
        "rightHanded",
        &["rightHanded", "leftHanded"],
    ) {
        Ok(orientation) => orientation == "leftHanded",
        Err(()) => return None,
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
        let recipe = canonical::StageRecipe::from_source(
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
        let recipe = canonical::StageRecipe::from_source(
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
        let recipe = canonical::StageRecipe::from_source(
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

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn shipped_parametric_assets_apply_their_lathe_schema() {
        let antenna = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/components/comms/antenna.usda");
        let stage = compose_file_to_stage(&antenna).expect("compose antenna.usda");
        let view = StageView::new(&stage);
        let reflector =
            SdfPath::new("/CommsAntenna/YawHead/DishGimbal/DishHead/Reflector").unwrap();
        assert!(
            view.has_api_schema(&reflector, "LunCoLatheAPI"),
            "the shipped reflector must opt into the parametric lathe contract"
        );
        let (surface, Some(lathe)) = read_patch_surface(&view, &reflector)
            .expect("the shipped reflector must produce a surface")
        else {
            panic!("the shipped reflector must retain its lathe parameters")
        };
        assert_eq!(surface.u_count, 9);
        assert_eq!(surface.v_count, 4);
        assert!(matches!(
            lathe.profile,
            lathe::LatheProfile::Paraboloid { .. }
        ));

        let lander = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/vessels/landers/descent_lander.usda");
        let stage = compose_file_to_stage(&lander).expect("compose descent_lander.usda");
        let view = StageView::new(&stage);
        let nozzle = SdfPath::new("/DescentLander/Nozzle").unwrap();
        assert!(
            view.has_api_schema(&nozzle, "LunCoLatheAPI"),
            "the shipped nozzle must opt into the parametric lathe contract"
        );
        let (surface, Some(lathe)) =
            read_patch_surface(&view, &nozzle).expect("the shipped nozzle must produce a surface")
        else {
            panic!("the shipped nozzle must retain its lathe parameters")
        };
        assert_eq!(surface.u_count, 9);
        assert_eq!(surface.v_count, 4);
        assert!(matches!(lathe.profile, lathe::LatheProfile::Bell { .. }));
    }

    #[test]
    fn authored_patch_requires_standard_sampling_fields() {
        let recipe = canonical::StageRecipe::from_source(
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
        let recipe = canonical::StageRecipe::from_source(
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
        let recipe = canonical::StageRecipe::from_source(
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
    // `trimCurve:*` IS applied — see `crate::trim`. A trimmed patch gets an
    // irregular triangulation of its surviving domain instead of a lattice.
    //
    // Two things that used to block this are handled there rather than guessed:
    // USD never states the keep/discard winding rule, so classification is
    // even-odd with the domain rectangle as an implicit outer loop
    // (orientation-independent); and `spade` panics when constraints cross, so
    // loops are inserted with `add_constraint_and_split` rather than gated with
    // `can_add_constraint` — gating would silently drop part of a loop and leave
    // the hole with a missing side.
    let trim_authored = has_authored_nurbs_trim(reader, path);
    let trim_loops = if !trim_authored {
        None
    } else {
        let counts = match read_curve_int_array(reader, path, "trimCurve:counts") {
            Ok(Some(counts)) if !counts.is_empty() => counts,
            _ => {
                error!(
                    "[usd-bevy] {} has malformed trimCurve:counts; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let orders = match read_curve_int_array(reader, path, "trimCurve:orders") {
            Ok(Some(orders)) => orders,
            _ => {
                error!(
                    "[usd-bevy] {} has malformed trimCurve:orders; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let vertex_counts = match read_curve_int_array(reader, path, "trimCurve:vertexCounts") {
            Ok(Some(vertex_counts)) => vertex_counts,
            _ => {
                error!(
                    "[usd-bevy] {} has malformed trimCurve:vertexCounts; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let tknots = match read_curve_real_array(reader, path, "trimCurve:knots") {
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
        let ranges = match read_double2_array_strict(reader, path, "trimCurve:ranges") {
            Ok(Some(ranges)) => ranges,
            Ok(None) => Vec::new(),
            Err(()) => {
                error!(
                    "[usd-bevy] {} has malformed trimCurve:ranges; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };

        let u_span = [u_knots[u_order - 1], u_knots[u_count]];
        let v_span = [v_knots[v_order - 1], v_knots[v_count]];
        let loops = crate::trim::assemble_loops(
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
        let Some(domain) = crate::trim::triangulate_trimmed(&loops, grid) else {
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
        let samples = crate::nurbs::sample_nurbs_patch_at(
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
        crate::lathe::flip_if_left_handed(surface.left_handed, &mut normals, &mut indices);
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
    // disagree — the same trap `crate::nurbs`' module doc describes for evaluators.
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

fn read_int_array(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Option<Vec<i32>> {
    match reader.attr_value(path, attr)? {
        Value::IntVec(v) => Some(v),
        Value::Int64Vec(v) => Some(v.iter().map(|&x| x as i32).collect()),
        _ => None,
    }
}

/// Read a curve integer array while preserving the distinction between an
/// omitted optional value and an authored value of the wrong type. Curve
/// topology is structural USD data; it must never be replaced by a guessed
/// single-curve layout.
fn read_curve_int_array(
    reader: &impl UsdRead,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<Vec<i32>>, ()> {
    match reader.attr_value(path, attr) {
        Some(Value::IntVec(values)) => Ok(Some(values)),
        Some(Value::Int64Vec(values)) => {
            Ok(Some(values.iter().map(|value| *value as i32).collect()))
        }
        Some(_) => Err(()),
        None if reader.has_authored_attribute(path, attr) => Err(()),
        None => Ok(None),
    }
}

/// Read a curve real array (`float[]` or `double[]`) without turning an
/// authored type mismatch into an omitted attribute.
fn read_curve_real_array(
    reader: &impl UsdRead,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<Vec<f64>>, ()> {
    match reader.attr_value(path, attr) {
        Some(Value::DoubleVec(values)) => Ok(Some(values)),
        Some(Value::FloatVec(values)) => Ok(Some(values.into_iter().map(f64::from).collect())),
        Some(_) => Err(()),
        None if reader.has_authored_attribute(path, attr) => Err(()),
        None => Ok(None),
    }
}

/// Read a schema-declared `token[]` array without treating an authored string
/// array or malformed value as an empty optional list.
fn read_curve_token_array(
    reader: &impl UsdRead,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<Vec<String>>, ()> {
    match reader.attr_value(path, attr) {
        Some(Value::TokenVec(values)) => Ok(Some(
            values.into_iter().map(|value| value.to_string()).collect(),
        )),
        Some(_) => Err(()),
        None if reader.has_authored_attribute(path, attr) => Err(()),
        None => Ok(None),
    }
}

/// Read one standard USD token with its schema fallback. An authored token
/// outside the schema's allowed set is malformed and is rejected, rather than
/// being interpreted as a different curve basis or wrap mode.
fn read_curve_token(
    reader: &impl UsdRead,
    path: &SdfPath,
    attr: &str,
    schema_default: &str,
    allowed: &[&str],
) -> Result<String, ()> {
    match reader.attr_value(path, attr) {
        Some(Value::Token(value)) => {
            let value = value.to_string();
            if allowed.contains(&value.as_str()) {
                Ok(value)
            } else {
                error!(
                    "[usd-bevy] {} has unsupported {} token `{}`",
                    path.as_str(),
                    attr,
                    value
                );
                Err(())
            }
        }
        Some(_) => {
            error!(
                "[usd-bevy] {} has authored {} with an unsupported value type",
                path.as_str(),
                attr
            );
            Err(())
        }
        None if reader.has_authored_attribute(path, attr) => {
            error!(
                "[usd-bevy] {} has authored {} with an unsupported value type",
                path.as_str(),
                attr
            );
            Err(())
        }
        None => Ok(schema_default.to_string()),
    }
}

/// Reads a `double2[]` / `float2[]` array as `Vec<[f64; 2]>`.
///
/// Tolerant of authored precision on the same principle as
/// [`points2`](read::UsdRead::points2), which this deliberately does NOT reuse:
/// `points2` narrows to `f32` because its consumers are vertex attributes, whereas
/// `trimCurve:ranges` is a pair of KNOT values. Those are compared against the
/// `f64` knot vector to decide where each curve's span starts and ends, so
/// round-tripping them through `f32` can move a span end just past a knot and drop
/// or duplicate a segment of a trim loop.
fn read_double2_array_strict(
    reader: &impl UsdRead,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<Vec<[f64; 2]>>, ()> {
    match reader.attr_value(path, attr) {
        Some(Value::Vec2dVec(v)) => Ok(Some(v.iter().map(|p| [p[0], p[1]]).collect())),
        Some(Value::Vec2fVec(v)) => {
            Ok(Some(v.iter().map(|p| [p[0] as f64, p[1] as f64]).collect()))
        }
        Some(_) => Err(()),
        None if reader.has_authored_attribute(path, attr) => Err(()),
        None => Ok(None),
    }
}

/// Triangulated topology of a native USD `Mesh` in the compact **indexed**
/// form a physics trimesh wants: the raw `points` as vertices, plus
/// fan-triangulated `faceVertexIndices` as triangle index triples.
///
/// This is the collider counterpart to [`build_usd_mesh`] (which expands to an
/// *unindexed* soup so per-face-varying normals/uvs survive). Here we keep
/// shared vertices — smaller, and exactly the `(Vec<vertex>, Vec<[u32;3]>)`
/// shape `Collider::trimesh` consumes. Triangle winding is irrelevant for
/// collision, so `orientation` is ignored. `None` if the topology attributes
/// are absent/empty or an index is out of range (malformed mesh).
pub fn read_usd_mesh_indexed(
    reader: &dyn read::UsdReadObject,
    path: &SdfPath,
) -> Option<(Vec<[f32; 3]>, Vec<[u32; 3]>)> {
    // Points are converted to the canonical frame (Y-up, metres) — the trimesh
    // collider must land where the rendered mesh lands. Identity for a canonical
    // stage. See `units`.
    let points = read_mesh_points(reader, path)?;
    let counts = read_int_array(reader, path, "faceVertexCounts")?;
    let indices = read_int_array(reader, path, "faceVertexIndices")?;
    if points.is_empty() || counts.is_empty() || indices.is_empty() {
        return None;
    }
    let n_points = points.len() as u32;
    let n_corners = indices.len();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut base = 0usize;
    for &count in &counts {
        let count = count as usize;
        if base + count > n_corners {
            return None; // counts/indices disagree → malformed
        }
        for k in 1..count.saturating_sub(1) {
            let tri = [
                indices[base] as u32,
                indices[base + k] as u32,
                indices[base + k + 1] as u32,
            ];
            if tri[0] >= n_points || tri[1] >= n_points || tri[2] >= n_points {
                return None; // index out of range → malformed
            }
            tris.push(tri);
        }
        base += count;
    }
    if tris.is_empty() {
        return None;
    }
    Some((points, tris))
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
    let points = read_mesh_points(reader, path)?;
    let counts = read_int_array(reader, path, "faceVertexCounts")?;
    let indices = read_int_array(reader, path, "faceVertexIndices")?;
    if points.is_empty() || counts.is_empty() || indices.is_empty() {
        return None;
    }

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

/// Marker inserted on prim entities that own both a primitive Cube
/// fallback mesh **and** a glTF [`WorldAssetRoot`]. Used by
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
fn load_diagnostic_label_font(
    mut commands: Commands,
    settings: Res<lunco_settings::DownloadSettings>,
) {
    let rx = lunco_assets::font::load_dejavu_sans_bytes(&settings);
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
        info!(
            "[usd-bevy] diagnostic label font loaded ({} bytes)",
            bytes.len()
        );
        install_diagnostic_font(&mut font, bytes);
        commands.remove_resource::<DiagnosticFontLoad>();
    }
}

/// CPU-rasterises `text` into an RGBA [`Image`] per [`DiagnosticLabelConfig`]:
/// coloured glyphs on a configurable backdrop. Baked once per failed asset —
/// no camera, no render pass, no per-frame work. `None` if `text` is empty.
fn rasterize_label(
    text: &str,
    font: &ab_glyph::FontVec,
    cfg: &DiagnosticLabelConfig,
) -> Option<Image> {
    use ab_glyph::{point, Font, PxScale, ScaleFont};
    // The POD texture descriptors, straight from `wgpu-types` — the same types
    // `bevy_image` itself takes. NOT `bevy::render::render_resource`, which is a
    // `bevy_render` re-export and would drag wgpu + naga into this crate.
    use bevy::asset::RenderAssetUsages;
    use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

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
        // One look shared across all faces — the binder's content-keyed cache
        // gives every face the same material handle, as the hand-shared
        // `label_mat` did. `double_sided` == the old `cull_mode: None`
        // (readable from either side).
        let label_look = PbrLook {
            // WHITE, explicitly: `PbrLook::default()`'s base colour is mid-grey,
            // which would tint the baked glyphs 50% dark. `StandardMaterial`'s
            // default (what this used to build) is white.
            base_color: LinearRgba::WHITE,
            textures: PbrTextures {
                base_color: Some(tex),
                ..default()
            },
            alpha: SurfaceAlpha::Blend,
            unlit: true,
            double_sided: true,
            ..default()
        };
        let s = pending.box_size;
        let (hx, hy, hz) = (s.x / 2.0, s.y / 2.0, s.z / 2.0);
        let eps = 0.01;
        use std::f32::consts::{FRAC_PI_2, PI};
        // Each face: outward offset + a rotation that turns the default
        // +Z-facing `Rectangle` to face outward, plus the face's
        // (horizontal, vertical) extent for sizing.
        let faces: &[(Vec3, Quat, f32, f32)] = if cfg.all_faces {
            &[
                (Vec3::new(0.0, 0.0, hz + eps), Quat::IDENTITY, s.x, s.y), // +Z
                (
                    Vec3::new(0.0, 0.0, -hz - eps),
                    Quat::from_rotation_y(PI),
                    s.x,
                    s.y,
                ), // -Z
                (
                    Vec3::new(hx + eps, 0.0, 0.0),
                    Quat::from_rotation_y(FRAC_PI_2),
                    s.z,
                    s.y,
                ), // +X
                (
                    Vec3::new(-hx - eps, 0.0, 0.0),
                    Quat::from_rotation_y(-FRAC_PI_2),
                    s.z,
                    s.y,
                ), // -X
                (
                    Vec3::new(0.0, hy + eps, 0.0),
                    Quat::from_rotation_x(-FRAC_PI_2),
                    s.x,
                    s.z,
                ), // +Y
                (
                    Vec3::new(0.0, -hy - eps, 0.0),
                    Quat::from_rotation_x(FRAC_PI_2),
                    s.x,
                    s.z,
                ), // -Y
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
                    label_look.clone(),
                    Transform::from_translation(offset).with_rotation(rot),
                ));
            }
        });
        commands.entity(stub).remove::<PendingDiagnosticLabel>();
    }
}

/// Removes the primitive Cube/Sphere/Cylinder fallback mesh once its
/// sibling [`WorldAssetRoot`] reports its glTF [`WorldAsset`] asset fully loaded.
fn hide_glb_placeholder_meshes(
    mut commands: Commands,
    // `Option<...>` so the system no-ops (instead of panicking on param
    // validation) in minimal apps that never `init_asset::<WorldAsset>()` — e.g.
    // headless tests that add `UsdBevyPlugin` without the full scene pipeline.
    // Production always registers `WorldAsset`, so behaviour there is unchanged.
    events: Option<MessageReader<AssetEvent<WorldAsset>>>,
    scene_roots: Query<(Entity, &WorldAssetRoot, Option<&ChildOf>), With<GlbPlaceholder>>,
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
                    // Dropping `Mesh3d` is what stops the placeholder drawing;
                    // dropping `PbrLook` retires its appearance intent (the binder
                    // owns the `MeshMaterial3d`, which is inert with no mesh).
                    commands
                        .entity(e)
                        .remove::<Mesh3d>()
                        .remove::<PbrLook>()
                        .remove::<GlbPlaceholder>()
                        .remove::<PlaceholderAssetUri>();

                    if let Some(parent) = parent {
                        if let Ok(siblings) = children.get(parent.0) {
                            for sib in siblings.iter() {
                                if sib != e && has_mesh.get(sib).is_ok() {
                                    commands.entity(sib).remove::<Mesh3d>().remove::<PbrLook>();
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
    canonical: NonSend<CanonicalStages>,
    cfg: Res<DiagnosticLabelConfig>,
    scene_roots: Query<
        (
            Entity,
            &WorldAssetRoot,
            &GlobalTransform,
            &PlaceholderAssetUri,
            &UsdPrimPath,
        ),
        (With<GlbPlaceholder>, Without<DiagnosticStub>),
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    // Per-placeholder time spent waiting on its glTF scene. Used to trip the
    // grace timeout on web, where a broken load may never report `is_failed()`.
    mut waited: Local<std::collections::HashMap<Entity, f32>>,
) {
    for (e, root, _global_transform, uri, prim_path) in scene_roots.iter() {
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
                if timed_out {
                    "did not load in time"
                } else {
                    "load FAILED"
                },
                root.0.id(),
                uri.0,
            );

            // Default scale
            let mut scale = Vec3::ONE;

            // Attempt to resolve dimensions from USD prim attributes
            if let Some(stage_asset) = stages.get(&prim_path.stage_handle) {
                let (reader, _generation) =
                    canonical.reader_for(prim_path.stage_handle.id(), stage_asset);

                // Navigate up from the current prim to its parent to find the sibling "Placeholder"
                let parent_path = prim_path.path.rsplit_once('/').map(|x| x.0).unwrap_or("");
                let sibling_placeholder_path = format!("{}/Placeholder", parent_path);

                // Helper to check attributes
                let check_path = |path: &str| -> Option<Vec3> {
                    if let Ok(sdf_path) = SdfPath::new(path) {
                        get_attribute_as_vec3(&reader, &sdf_path, "xformOp:scale").or_else(|| {
                            reader
                                .real(&sdf_path, "size")
                                .map(|size| Vec3::splat(size as f32))
                        })
                    } else {
                        None
                    }
                };

                // Check sibling first, then parent prim path itself
                if let Some(s) =
                    check_path(&sibling_placeholder_path).or_else(|| check_path(&prim_path.path))
                {
                    debug!("[usd-bevy] Found scale: {:?}", s);
                    scale = s;
                } else {
                    debug!(
                        "[usd-bevy] No scale or size found on paths: {:?} or {:?}",
                        sibling_placeholder_path, prim_path.path
                    );
                }
            }

            debug!("[usd-bevy] Computed stub scale: {:?}", scale);

            // Just the filename — strip the `lunco://…/` path prefix and
            // the `#Scene0` glTF sub-label.
            let file_name = uri
                .0
                .rsplit('/')
                .next()
                .unwrap_or(&uri.0)
                .split('#')
                .next()
                .unwrap_or(&uri.0);

            commands.entity(e).try_insert((
                Mesh3d(meshes.add(Cuboid::from_size(scale))),
                PbrLook {
                    base_color: cfg.box_color.to_linear(),
                    emissive: LinearRgba::from(cfg.box_color),
                    alpha: SurfaceAlpha::Blend, // Support transparency
                    unlit: true,                // readable even with no scene lighting
                    ..default()
                },
                // No `Transform` / `Visibility` insert here. `Mesh3d` pulls both in as
                // required components, and re-inserting a `Transform` built from
                // `GlobalTransform::compute_transform()` would overwrite the prim's LOCAL
                // transform with a world-space one — wrong for any entity with a parent.
                DiagnosticStub,
                // The label is baked on once the font is ready (frame 1 on
                // native, whenever the fetch lands on web).
                PendingDiagnosticLabel {
                    text: format!("{}{file_name}", cfg.prefix),
                    box_size: scale,
                },
            ));
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
    use openusd::sdf::Path as SdfPath;

    /// Build a real composed stage. The extractors read through `StageView` — the
    /// live, PCP-composed stage — which is the ONLY read path now that the
    /// Runtime reads come from the live canonical stage. Tests read what the app reads.
    fn parse(usda: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("t.usda", usda))
            .expect("build canonical stage")
    }

    /// `UsdRead::asset` reads an `asset`-typed attribute, and `scalar::<String>`
    /// does NOT.
    ///
    /// This is the type contract, pinned. A shader's source is an `asset`
    /// (`@shaders/wheel.wgsl@`) so USD's resolver — and anything walking a layer for
    /// the files a scene depends on — can see the `.wgsl`. As a `string` it is inert:
    /// the scene names a shader that will not travel with it.
    ///
    /// The second assertion is the important one. A reader tolerant of BOTH types
    /// would let the wrong authoring keep working, and writer and reader would go on
    /// concealing each other. `scalar::<String>` returning `None` on an `asset` is the
    /// property that makes the schema binding, rather than advisory.
    #[test]
    fn asset_typed_attribute_reads_as_asset_and_not_as_string() {
        let __cs = parse(
            "#usda 1.0\n\
             def Shader \"Shader\"\n{\n\
             uniform token info:implementationSource = \"sourceAsset\"\n\
             uniform asset info:wgsl:sourceAsset = @shaders/wheel.wgsl@\n}\n",
        );
        let reader = __cs.view();
        let panel = SdfPath::new("/Shader").unwrap();

        assert_eq!(
            reader.asset(&panel, "info:wgsl:sourceAsset").as_deref(),
            Some("shaders/wheel.wgsl"),
        );
        assert!(
            reader
                .scalar::<String>(&panel, "info:wgsl:sourceAsset")
                .is_none(),
            "an `asset` must NOT read back as a String — tolerating both is what let \
             the writer and reader hide each other's bugs",
        );
        // …and the sibling `token` reads through `text`, NOT through `scalar::<String>`.
        //
        // A `token` is its own `sdf::Value` variant, and `scalar::<String>` matches
        // `Value::String` alone — so a reader asking for a String reads every token as
        // `None`, for every prim, silently. A shader that never binds is a plain grey
        // surface, not an error, which is why this half is pinned in a test.
        assert_eq!(
            reader.text(&panel, "info:implementationSource").as_deref(),
            Some("sourceAsset"),
            "a `token` must read through `text`",
        );
        assert!(
            reader
                .scalar::<String>(&panel, "info:implementationSource")
                .is_none(),
            "`scalar::<String>` must NOT read a token — the whole point is that asking \
             for the wrong USD type fails loudly in a test rather than quietly at runtime",
        );
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

    /// The collider decode keeps the raw points (4) and fan-triangulates the
    /// quad into two index triples — the form `Collider::trimesh` consumes.
    #[test]
    fn indexed_decode_keeps_points_and_fans_quad() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Quad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0),(0,1,0)]\n\
             int[] faceVertexCounts = [4]\n\
             int[] faceVertexIndices = [0,1,2,3]\n}\n",
        );
        let reader = __cs.view();
        let (verts, tris) =
            read_usd_mesh_indexed(&reader, &SdfPath::new("/Quad").unwrap()).expect("indexed mesh");
        assert_eq!(verts.len(), 4, "raw points kept (shared verts)");
        assert_eq!(tris, vec![[0, 1, 2], [0, 2, 3]], "fan (0,k,k+1)");
    }

    /// The collider decode rejects malformed topology the same as the render
    /// path, so no bad trimesh reaches the physics engine.
    #[test]
    fn indexed_decode_rejects_bad_topology() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Bad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0)]\n\
             int[] faceVertexCounts = [3]\n\
             int[] faceVertexIndices = [0,1,9]\n}\n",
        );
        let reader = __cs.view();
        assert!(read_usd_mesh_indexed(&reader, &SdfPath::new("/Bad").unwrap()).is_none());
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

#[cfg(test)]
mod animation_tests {
    //! The USD animation sampler read path: `timeSamples` detection, time-aware
    //! vec3 evaluation, and per-channel "animated only" sampling (doc 19).
    use super::*;
    use openusd::sdf::Path as SdfPath;

    /// Build a real composed stage. The extractors read through `StageView` — the
    /// live, PCP-composed stage — which is the ONLY read path now that the
    /// Runtime reads come from the live canonical stage. Tests read what the app reads.
    fn parse(usda: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("t.usda", usda))
            .expect("build canonical stage")
    }

    /// translate is keyframed (animated); rotateXYZ has only a default (static);
    /// scale is absent.
    const SCENE: &str = r#"#usda 1.0

def Xform "Mover"
{
    double3 xformOp:translate.timeSamples = {
        0: (0, 0, 0),
        2: (20, 0, 0),
    }
    double3 xformOp:rotateXYZ = (0, 90, 0)
}

def Xform "Static"
{
    double3 xformOp:translate = (5, 0, 0)
}
"#;

    #[test]
    fn detects_animated_prims_by_xform_time_samples() {
        let __cs = parse(SCENE);
        let reader = __cs.view();
        let mover = SdfPath::new("/Mover").unwrap();
        let stat = SdfPath::new("/Static").unwrap();
        assert!(prim_has_xform_time_samples(&reader, &mover));
        assert!(!prim_has_xform_time_samples(&reader, &stat));
        // Per-channel: translate animated, rotateXYZ not.
        assert!(attr_has_time_samples(&reader, &mover, "xformOp:translate"));
        assert!(!attr_has_time_samples(&reader, &mover, "xformOp:rotateXYZ"));
    }

    #[test]
    fn samples_animated_channel_and_leaves_static_untouched() {
        let __cs = parse(SCENE);
        let reader = __cs.view();
        let mover = SdfPath::new("/Mover").unwrap();

        // Animated translate interpolates linearly: t=1.0 → halfway (10,0,0).
        assert_eq!(
            sample_animated_vec3(&reader, &mover, "xformOp:translate", 1.0),
            Some([10.0, 0.0, 0.0])
        );
        // On a key.
        assert_eq!(
            sample_animated_vec3(&reader, &mover, "xformOp:translate", 2.0),
            Some([20.0, 0.0, 0.0])
        );
        // Held past the last key (USD semantics).
        assert_eq!(
            sample_animated_vec3(&reader, &mover, "xformOp:translate", 99.0),
            Some([20.0, 0.0, 0.0])
        );
        // rotateXYZ has only a default → the sampler must NOT touch it (None),
        // so its instantiated pose is preserved.
        assert_eq!(
            sample_animated_vec3(&reader, &mover, "xformOp:rotateXYZ", 1.0),
            None
        );
    }

    #[test]
    fn read_vec3_f64_at_falls_back_to_default_for_static() {
        let __cs = parse(SCENE);
        let reader = __cs.view();
        let stat = SdfPath::new("/Static").unwrap();
        // The raw time-aware reader returns the default at any time (value
        // resolution), even though `sample_animated_vec3` gates it out.
        assert_eq!(
            read_vec3_f64_at(&reader, &stat, "xformOp:translate", 7.0),
            Some([5.0, 0.0, 0.0])
        );
    }

    #[test]
    fn time_codes_per_second_defaults_to_24_when_unauthored() {
        // A stage that authors no `timeCodesPerSecond` reads back the USD-spec
        // fallback of 24, so the sampler's seconds→time-code map is well-defined
        // even for content that never set it.
        let __cs = parse(SCENE);
        let reader = __cs.view();
        assert_eq!(stage_time_codes_per_second(&reader), 24.0);
    }

    /// Visibility is keyframed; a second prim is fully static.
    const VIS_SCENE: &str = r#"#usda 1.0

def Xform "Blinker"
{
    token visibility.timeSamples = {
        0: "inherited",
        5: "invisible",
    }
}

def Xform "Solid"
{
    token visibility = "inherited"
    double3 xformOp:translate = (1, 2, 3)
}
"#;

    #[test]
    fn read_token_at_holds_visibility_keyframes() {
        let __cs = parse(VIS_SCENE);
        let reader = __cs.view();
        let blinker = SdfPath::new("/Blinker").unwrap();
        // On the first key.
        assert_eq!(
            read_token_at(&reader, &blinker, "visibility", 0.0).as_deref(),
            Some("inherited")
        );
        // Between keys → held lower (tokens never interpolate).
        assert_eq!(
            read_token_at(&reader, &blinker, "visibility", 2.0).as_deref(),
            Some("inherited")
        );
        // Past the last key → held last.
        assert_eq!(
            read_token_at(&reader, &blinker, "visibility", 9.0).as_deref(),
            Some("invisible")
        );
        // A static-visibility prim has no samples → None (sampler leaves it).
        let solid = SdfPath::new("/Solid").unwrap();
        assert_eq!(read_token_at(&reader, &solid, "visibility", 1.0), None);
    }

    const ORIENT_SCENE: &str = r#"#usda 1.0

def Xform "Spinner"
{
    quatf xformOp:orient.timeSamples = {
        0: (1, 0, 0, 0),
        10: (0, 1, 0, 0),
    }
}
"#;

    #[test]
    fn orient_channel_slerps_and_is_detected() {
        let __cs = parse(ORIENT_SCENE);
        let reader = __cs.view();
        let spinner = SdfPath::new("/Spinner").unwrap();
        // The quaternion channel marks the prim animated.
        assert!(prim_has_xform_time_samples(&reader, &spinner));
        assert!(prim_is_animated(&reader, &spinner));
        // USD (w,x,y,z) = (1,0,0,0) → Bevy identity at the first key.
        let q0 = local_rotation_at(&reader, &spinner, 0.0).unwrap();
        assert!(q0.abs_diff_eq(Quat::IDENTITY, 1e-6));
        // Held past the last key → (0,1,0,0) = 180° about X.
        let q_end = local_rotation_at(&reader, &spinner, 99.0).unwrap();
        assert!(q_end.abs_diff_eq(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0), 1e-6));
        // Midway slerps to 90° about X (normalized) — not a component lerp.
        let q_mid = local_rotation_at(&reader, &spinner, 5.0).unwrap();
        assert!(q_mid.is_normalized());
        assert!(q_mid.abs_diff_eq(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), 1e-5));
    }

    const ROTATION_OPS_SCENE: &str = r#"#usda 1.0

(
    metersPerUnit = 1
)

def Xform "HingeZ"
{
    float xformOp:rotateZ.timeSamples = {
        0: 0.0,
        4: 90.0,
    }
    uniform token[] xformOpOrder = ["xformOp:rotateZ"]
}

def Xform "EulerZYX"
{
    float3 xformOp:rotateZYX = (0, 0, 90)
    uniform token[] xformOpOrder = ["xformOp:rotateZYX"]
}

def Xform "Matrixed"
{
    matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (3, 4, 5, 1) )
    uniform token[] xformOpOrder = ["xformOp:transform"]
}
"#;

    #[test]
    fn single_axis_rotation_is_detected_and_composed() {
        let __cs = parse(ROTATION_OPS_SCENE);
        let reader = __cs.view();
        let hinge = SdfPath::new("/HingeZ").unwrap();
        // A single-axis `rotateZ` time-sample marks the prim animated.
        assert!(prim_has_xform_time_samples(&reader, &hinge));
        // Held start = 0° → identity; midway (code 2) = 45° about Z.
        assert!(local_rotation_at(&reader, &hinge, 0.0)
            .unwrap()
            .abs_diff_eq(Quat::IDENTITY, 1e-6));
        let q = local_rotation_at(&reader, &hinge, 2.0).unwrap();
        assert!(q.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4), 1e-5));
    }

    #[test]
    fn euler_order_zyx_composes() {
        let __cs = parse(ROTATION_OPS_SCENE);
        let reader = __cs.view();
        // `rotateZYX = (0,0,90)` → 90° about Z (the X and Y angles are zero).
        let q = local_rotation_at(&reader, &SdfPath::new("/EulerZYX").unwrap(), 0.0).unwrap();
        assert!(q.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-5));
    }

    #[test]
    fn quath_orient_decodes() {
        // Half-precision quaternion orient: USD (w,x,y,z) = (0,1,0,0) → 180° about
        // X. Proves the `quath` arm (via `f16::to_f32`) decodes.
        let scene = r#"#usda 1.0
def Xform "HalfSpin"
{
    quath xformOp:orient = (0, 1, 0, 0)
}
"#;
        let __cs = parse(scene);
        let reader = __cs.view();
        let q = local_rotation_at(&reader, &SdfPath::new("/HalfSpin").unwrap(), 0.0).unwrap();
        assert!(q.abs_diff_eq(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0), 1e-3));
    }

    const ORDER_SCENE: &str = r#"#usda 1.0
(
    metersPerUnit = 1
)

def Xform "ScaleFirst"
{
    double3 xformOp:translate = (1, 0, 0)
    double3 xformOp:scale = (2, 2, 2)
    uniform token[] xformOpOrder = ["xformOp:scale", "xformOp:translate"]
}

def Xform "TranslateFirst"
{
    double3 xformOp:translate = (1, 0, 0)
    double3 xformOp:scale = (2, 2, 2)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:scale"]
}

def Xform "Std"
{
    double3 xformOp:translate = (5, 6, 7)
    float3 xformOp:rotateXYZ = (0, 0, 90)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ"]
}
"#;

    #[test]
    fn xform_op_order_is_honored() {
        let __cs = parse(ORDER_SCENE);
        let reader = __cs.view();
        // `["scale","translate"]`: translate is the LAST op → applied first to the
        // geometry, then `scale` (first op) scales it → translation (2,0,0).
        let sf = compose_xform_order_at(&reader, &SdfPath::new("/ScaleFirst").unwrap(), 0.0)
            .unwrap()
            .unwrap();
        assert!(sf.translation.abs_diff_eq(Vec3::new(2.0, 0.0, 0.0), 1e-5));
        assert!(sf.scale.abs_diff_eq(Vec3::splat(2.0), 1e-5));
        // `["translate","scale"]` (standard order): scale applied first, then the
        // unscaled translate → (1,0,0). Different result ⇒ op order is honored.
        let tf = compose_xform_order_at(&reader, &SdfPath::new("/TranslateFirst").unwrap(), 0.0)
            .unwrap()
            .unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::splat(2.0), 1e-5));
    }

    #[test]
    fn shared_transform_reader_preserves_usd_scale() {
        let __cs = parse(ORDER_SCENE);
        let reader = __cs.view();
        let tf =
            read_transform_from_usd(&reader, &SdfPath::new("/TranslateFirst").unwrap()).unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::splat(2.0), 1e-5));
    }

    #[test]
    fn xform_op_order_standard_composes_as_expected() {
        // Standard-order content (`["translate","rotateXYZ"]`) composes its
        // authored translation and rotation without a parallel decoder.
        let __cs = parse(ORDER_SCENE);
        let reader = __cs.view();
        let tf = local_transform_at(&reader, &SdfPath::new("/Std").unwrap(), 0.0)
            .unwrap()
            .unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(5.0, 6.0, 7.0), 1e-5));
        assert!(tf
            .rotation
            .abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-5));
    }

    #[test]
    fn matrix_transform_decomposes_translation() {
        let __cs = parse(ROTATION_OPS_SCENE);
        let reader = __cs.view();
        // Identity rotation/scale, translation in the USD matrix's last row.
        let tf =
            read_matrix_transform_at(&reader, &SdfPath::new("/Matrixed").unwrap(), 0.0).unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(3.0, 4.0, 5.0), 1e-5));
        assert!(tf.rotation.abs_diff_eq(Quat::IDENTITY, 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-5));
        // And `read_transform_from_usd` prefers the matrix.
        let full = read_transform_from_usd(&reader, &SdfPath::new("/Matrixed").unwrap()).unwrap();
        assert!(full.translation.abs_diff_eq(Vec3::new(3.0, 4.0, 5.0), 1e-5));
    }

    #[test]
    fn animated_time_range_spans_keys_in_seconds() {
        let __cs = parse(SCENE);
        let reader = __cs.view();
        // `/Mover` translate is keyed at codes 0 and 2; default tcps = 24, so the
        // span in seconds is [0, 2/24].
        let (lo, hi) = animated_time_range(&reader, &SdfPath::new("/Mover").unwrap()).unwrap();
        assert!(lo.abs() < 1e-9);
        assert!((hi - 2.0 / 24.0).abs() < 1e-9);
        // A static prim keyframes nothing → no range.
        assert!(animated_time_range(&reader, &SdfPath::new("/Static").unwrap()).is_none());
    }

    #[test]
    fn prim_is_animated_covers_visibility_and_xform_but_not_static() {
        let __cs = parse(VIS_SCENE);
        let reader = __cs.view();
        assert!(prim_is_animated(
            &reader,
            &SdfPath::new("/Blinker").unwrap()
        ));
        // `Solid` keyframes nothing — visibility and translate are both defaults.
        assert!(!prim_is_animated(&reader, &SdfPath::new("/Solid").unwrap()));
        // The xform-animated `Mover` from SCENE is still caught by the broader gate.
        let __mover = parse(SCENE);
        let mover_reader = __mover.view();
        assert!(prim_is_animated(
            &mover_reader,
            &SdfPath::new("/Mover").unwrap()
        ));
        assert!(!prim_is_animated(
            &mover_reader,
            &SdfPath::new("/Static").unwrap()
        ));
    }
}

#[cfg(test)]
mod stage_metrics_import_tests {
    //! **P7** — the importer honours the stage's `metersPerUnit` / `upAxis`
    //! (`docs/architecture/41-axes-and-units.md`: "convert once, at the
    //! importer"). Before this, an Omniverse / Isaac Sim stage — Z-up,
    //! centimetres, *their* defaults — imported rotated 90° and 100× too small,
    //! silently. These tests are the fixture doc 41 asks for: load a Z-up/cm
    //! stage, assert SI Y-up out.
    use super::*;
    use crate::units::{StageMetrics, UpAxis};
    use openusd::sdf::Path as SdfPath;

    /// Build a real composed stage. The extractors read through `StageView` — the
    /// live, PCP-composed stage — which is the ONLY read path now that the
    /// Runtime reads come from the live canonical stage. Tests read what the app reads.
    fn parse(usda: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("t.usda", usda))
            .expect("build canonical stage")
    }

    /// An Isaac-Sim-flavoured stage: Z-up, centimetres. `/Tower` sits 3 m up the
    /// stage's up-axis (+Z = 300 cm) and 1 m along +X; it is a Z-axial cylinder
    /// (upright in a Z-up world) of radius 0.5 m / height 2 m, authored in cm.
    const ZUP_CM: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    metersPerUnit = 0.01
    upAxis = "Z"
)

def Xform "World"
{
    def Cylinder "Tower"
    {
        double3 xformOp:translate = (100, 0, 300)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        token axis = "Z"
        double radius = 50
        double height = 200
    }

    def Mesh "Slab"
    {
        point3f[] points = [(0, 0, 100), (100, 0, 100), (0, 100, 100)]
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
    }
}
"#;

    /// The same scene in our canonical metrics (Y-up, metres) — the control.
    const YUP_M: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    metersPerUnit = 1
)

def Xform "World"
{
    def Cylinder "Tower"
    {
        double3 xformOp:translate = (1, 3, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        token axis = "Y"
        double radius = 0.5
        double height = 2
    }
}
"#;

    #[test]
    fn reads_stage_metrics() {
        let m = StageMetrics::from_reader(&parse(ZUP_CM).view()).expect("valid stage metrics");
        assert_eq!(m.up_axis, UpAxis::Z);
        assert_eq!(m.meters_per_unit, 0.01);
        assert!(!m.is_canonical());

        // Unauthored ⇒ the USD defaults, which are our canonical frame.
        let m = StageMetrics::from_reader(&parse(YUP_M).view()).expect("valid stage metrics");
        assert_eq!(m.up_axis, UpAxis::Y);
        assert_eq!(m.meters_per_unit, 1.0);
        assert!(
            m.is_canonical(),
            "a Y-up metre stage must convert to the identity"
        );
    }

    /// The headline regression: the Z-up centimetre stage imports **upright and
    /// at true scale**. Before the fix, `translate` read back `(100, 0, 300)` —
    /// 100× too large and with the up-axis on Z.
    #[test]
    fn zup_centimetre_stage_imports_upright_and_metre_scaled() {
        let __cs = parse(ZUP_CM);
        let reader = __cs.view();
        let tower = SdfPath::new("/World/Tower").unwrap();

        let tf = local_transform_at(&reader, &tower, 0.0)
            .expect("transform stack is valid")
            .expect("prim authors an xform");
        // (100, 0, 300) cm, Z-up  →  (1, 3, 0) m, Y-up: the stage's +Z (up) is now
        // canonical +Y (up); +X is untouched; the metre scale is 1/100.
        assert!(
            tf.translation.abs_diff_eq(Vec3::new(1.0, 3.0, 0.0), 1e-5),
            "expected (1, 3, 0) m Y-up, got {:?}",
            tf.translation
        );

        // Dimensions convert to metres — the collider and the mesh both read this.
        match read_shape_dims(&reader, &tower, "Cylinder") {
            Some(ShapeDims::Cylinder { radius, height }) => {
                assert!((radius - 0.5).abs() < 1e-9, "radius {radius} m");
                assert!((height - 2.0).abs() < 1e-9, "height {height} m");
            }
            other => panic!("expected Cylinder dims, got {other:?}"),
        }

        // Mesh points convert as points: (0,0,100)cm Z-up → (0,1,0)m Y-up, and
        // (0,100,100) → (0, 1, -1).
        let (points, tris) =
            read_usd_mesh_indexed(&reader, &SdfPath::new("/World/Slab").unwrap()).expect("mesh");
        assert_eq!(tris.len(), 1);
        assert!(Vec3::from_array(points[0]).abs_diff_eq(Vec3::new(0.0, 1.0, 0.0), 1e-5));
        assert!(Vec3::from_array(points[1]).abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-5));
        assert!(Vec3::from_array(points[2]).abs_diff_eq(Vec3::new(0.0, 1.0, -1.0), 1e-5));

        // The `axis` token is a STAGE-frame axis: a Z-axial cylinder stands up in a
        // Z-up world, so after conversion it must stand up along canonical +Y —
        // i.e. the composed geometry rotation maps the primitive's own +Y to +Y.
        let conv = stage_convention(&reader).expect("valid stage convention");
        let q = conv.orient(usd_axis_to_quat("Z").unwrap_or(Quat::IDENTITY));
        assert!(
            (q * Vec3::Y).abs_diff_eq(Vec3::Y, 1e-5),
            "a Z-axial cylinder on a Z-up stage must end up axial with canonical up, got {:?}",
            q * Vec3::Y
        );
    }

    /// The Z-up/cm stage and its hand-written canonical twin import to the SAME
    /// pose and dimensions — the round-trip guard doc 41 §"three holes" asks for.
    #[test]
    fn zup_cm_stage_matches_its_canonical_twin() {
        let __zup = parse(ZUP_CM);
        let __yup = parse(YUP_M);
        let zup = __zup.view();
        let yup = __yup.view();
        let tower = SdfPath::new("/World/Tower").unwrap();

        let a = local_transform_at(&zup, &tower, 0.0).unwrap().unwrap();
        let b = local_transform_at(&yup, &tower, 0.0).unwrap().unwrap();
        assert!(a.translation.abs_diff_eq(b.translation, 1e-5));

        assert_eq!(
            read_shape_dims(&zup, &tower, "Cylinder"),
            read_shape_dims(&yup, &tower, "Cylinder"),
        );
    }

    /// A canonical stage is bit-for-bit unaffected — every asset we ship takes
    /// this path, so the conversion cannot regress existing content.
    #[test]
    fn canonical_stage_is_untouched() {
        let __cs = parse(YUP_M);
        let reader = __cs.view();
        let tower = SdfPath::new("/World/Tower").unwrap();
        assert!(stage_convention(&reader)
            .expect("valid stage convention")
            .is_identity());
        let tf = local_transform_at(&reader, &tower, 0.0).unwrap().unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(1.0, 3.0, 0.0), 1e-6));
        assert!(tf.rotation.abs_diff_eq(Quat::IDENTITY, 1e-6));
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-6));
    }

    /// An unsupported declaration must not import silently-wrong: the stage is
    /// rejected instead of being replaced with the canonical frame.
    #[test]
    fn unsupported_declarations_are_rejected() {
        let bogus = parse(
            "#usda 1.0\n(\n    upAxis = \"X\"\n    metersPerUnit = 0\n)\ndef Xform \"W\"\n{\n}\n",
        );
        let error = StageMetrics::from_reader(&bogus.view()).expect_err("malformed metadata");
        assert!(matches!(
            error,
            crate::units::StageMetricsError::InvalidUpAxis(_)
        ));
        assert!(stage_convention(&bogus.view()).is_err());
        assert!(matches!(
            StageMetrics::from_stage(bogus.stage()),
            Err(crate::units::StageMetricsError::InvalidUpAxis(_))
        ));

        let malformed_units = parse(
            "#usda 1.0\n(\n    upAxis = \"Y\"\n    metersPerUnit = 0\n)\ndef Xform \"W\"\n{\n}\n",
        );
        assert!(matches!(
            StageMetrics::from_reader(&malformed_units.view()),
            Err(crate::units::StageMetricsError::InvalidMetersPerUnit(_))
        ));
        assert!(matches!(
            StageMetrics::from_stage(malformed_units.stage()),
            Err(crate::units::StageMetricsError::InvalidMetersPerUnit(_))
        ));
    }
}

#[cfg(test)]
mod default_prim_attr_tests {
    //! [`DefaultPrim`] — parse a single layer and read a `string`/`token`
    //! attribute off its `defaultPrim`.
    use super::*;

    fn attr(text: &str, name: &str) -> Option<String> {
        DefaultPrim::parse(text)?.text(name)
    }

    const SCENE: &str = "#usda 1.0\n\
        (\n\
            defaultPrim = \"SandboxScene\"\n\
            upAxis = \"Y\"\n\
        )\n\
        def Xform \"SandboxScene\"\n{\n\
            custom bool lunco:spawnable = false\n\
            custom string lunco:testLabel = \"Two cubes joined together.\"\n\
            def Cube \"Ground\"\n{\n}\n\
        }\n";

    #[test]
    fn reads_string_attr_off_default_prim() {
        assert_eq!(
            attr(SCENE, "lunco:testLabel").as_deref(),
            Some("Two cubes joined together.")
        );
    }

    #[test]
    fn missing_attr_is_none() {
        assert!(attr(SCENE, "lunco:notAuthored").is_none());
    }

    #[test]
    fn no_default_prim_is_none() {
        // Layer with no `defaultPrim` metadata — even if the attribute exists
        // on a prim, we don't know which prim is the root.
        let src =
            "#usda 1.0\ndef Xform \"Orphan\"\n{\n    custom string lunco:testLabel = \"x\"\n}\n";
        assert!(attr(src, "lunco:testLabel").is_none());
    }

    #[test]
    fn unparseable_text_is_none() {
        assert!(attr("this is not USDA", "lunco:testLabel").is_none());
    }
}

#[cfg(test)]
mod awaiting_stage_failure_tests {
    //! A stage load that fails must END the wait it started.
    //!
    //! The regression these guard is the one that made an app unable to load any
    //! scene after a single bad path: `sync_usd_visuals` only ever drains
    //! `UsdAwaitingStage` on success, so a failed load left its prims parked, and
    //! "a prim is still awaiting this stage" is what keeps `SceneLoadInFlight`
    //! set — which suppresses every subsequent `LoadScene`.
    use super::*;
    use bevy::asset::{AssetLoadError, AssetLoadFailedEvent, AssetPath};

    /// Bare app: the system reads a message and a query, and writes commands.
    /// Nothing here needs an asset pipeline, which is the point — the behaviour
    /// under test is what happens when the pipeline has already given up.
    fn app() -> App {
        let mut app = App::new();
        app.add_message::<AssetLoadFailedEvent<UsdStageAsset>>();
        app.add_systems(Update, fail_awaiting_stage_prims);
        app
    }

    fn parked(app: &mut App, handle: Handle<UsdStageAsset>) -> Entity {
        app.world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: handle,
                    path: "/Scene".into(),
                },
                UsdAwaitingStage,
            ))
            .id()
    }

    fn fail(app: &mut App, handle: &Handle<UsdStageAsset>, path: &str) {
        app.world_mut()
            .resource_mut::<Messages<AssetLoadFailedEvent<UsdStageAsset>>>()
            .write(AssetLoadFailedEvent {
                id: handle.id(),
                path: AssetPath::from(path.to_string()),
                error: AssetLoadError::EmptyPath(AssetPath::from(path.to_string())),
            });
    }

    #[test]
    fn a_failed_stage_drops_the_prims_parked_on_it() {
        let mut app = app();
        let handle = Handle::<UsdStageAsset>::default();
        let entity = parked(&mut app, handle.clone());

        app.update();
        assert!(
            app.world().get_entity(entity).is_ok(),
            "nothing has failed yet — the prim is still legitimately waiting"
        );

        fail(&mut app, &handle, "missing.usda");
        app.update();
        assert!(
            app.world().get_entity(entity).is_err(),
            "the stage will never arrive, so the prim can never instantiate; \
             leaving it parked is what pinned `SceneLoadInFlight` forever"
        );
    }

    #[test]
    fn a_different_stages_failure_leaves_this_prim_waiting() {
        let mut app = app();
        let mine = Handle::<UsdStageAsset>::default();
        let entity = parked(&mut app, mine);

        let other: Handle<UsdStageAsset> =
            bevy::asset::uuid_handle!("5ce7e000-0000-4000-8000-000000000001");
        fail(&mut app, &other, "someone_elses.usda");
        app.update();

        assert!(
            app.world().get_entity(entity).is_ok(),
            "one scene failing must not tear down prims waiting on another stage"
        );
    }
}
