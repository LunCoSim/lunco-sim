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
mod camera;
pub mod camera_mount;
pub mod camera_switch;
pub mod camera_track;
pub use camera_switch::SetActiveCamera;
pub mod author;
pub mod usd_data;
pub mod view;
pub mod canonical;
pub mod read;
use usd_data::UsdDataExt;
pub use view::StageView;
pub use canonical::{CanonicalStage, CanonicalStages, RawStageChange, StageRecipe};
pub use read::UsdRead;
pub use compose::compose_native_fs;
#[cfg(not(target_arch = "wasm32"))]
pub use compose::compose_file_to_stage;
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
            .register_type::<UsdAnimated>()
            .register_type::<camera_track::CameraTrack>()
            .init_resource::<DiagnosticLabelFont>()
            .init_resource::<DiagnosticLabelConfig>()
            // Guarantee the viewport substrate exists wherever these camera
            // systems run: `cycle_active_camera`/`reconcile_scene_viewport`
            // read `SceneViewport`, so a host that adds this plugin without
            // lunco-core's `register_core_resources` (e.g. a focused test app)
            // still has it. Idempotent — a no-op if core already registered it.
            .init_resource::<lunco_core::SceneViewport>()
            // Ph0′: the live canonical stages, built main-thread from each
            // loaded `UsdStageAsset`'s `StageRecipe` (`sync_canonical_stages`).
            // `NonSend` — holds `!Send` openusd `Stage`s.
            .init_non_send_resource::<canonical::CanonicalStages>()
            .add_systems(Startup, load_diagnostic_label_font)
            .add_observer(on_usd_prim_added)
            .add_observer(light::on_usd_light_added)
            // Active-camera switch (avatar-free): the `SetActiveCamera` command
            // + `KeyC` cycle both fire the internal `ActivateCamera` trigger,
            // which enforces the one-active-window-camera invariant and
            // relocates the big_space FloatingOrigin. Works in a static,
            // input-less world (the command path needs neither).
            .add_observer(camera_switch::on_activate_camera)
            .add_observer(camera_switch::bind_avatar_camera_on_add)
            // The viewport-camera reconciler: the SINGLE authority over
            // window-camera `is_active` + `viewport`. Reads `SceneViewport`
            // (bound camera + visibility + rect, written by the switch and the
            // workbench) and actuates it. Runs every frame so async spawns and
            // provisional→avatar takeover stay coherent.
            .add_systems(
                Update,
                (
                    camera_switch::cycle_active_camera,
                    camera_switch::reconcile_scene_viewport.after(camera_switch::cycle_active_camera),
                ),
            )
            // Rover/vehicle-mounted cameras: a nested `def Camera` is realised
            // as a grid-direct follower (so it can host the FloatingOrigin at
            // full precision). `resolve` rigs it once during load; `follow`
            // tracks the mount each frame, before transform propagation.
            .add_systems(Update, camera_mount::resolve_camera_mounts)
            .add_systems(
                PostUpdate,
                camera_mount::follow_mounted_cameras
                    .before(bevy::transform::TransformSystems::Propagate),
            )
            // `sync_usd_visuals` runs only on frames where a stage's
            // `LoadedWithDependencies` event was emitted. Idle frames
            // skip it entirely (run-condition short-circuits).
            .add_systems(
                Update,
                (
                    sync_usd_visuals.run_if(bevy::ecs::schedule::common_conditions::on_message::<AssetEvent<UsdStageAsset>>),
                    // Ph0′: build the live canonical stage from each loaded
                    // asset's recipe (additive; legacy path untouched).
                    canonical::sync_canonical_stages.run_if(bevy::ecs::schedule::common_conditions::on_message::<AssetEvent<UsdStageAsset>>),
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
                    camera_track::bind_camera_tracks_to_preview,
                    camera_track::clear_camera_track_plans_on_stage_reload.run_if(
                        bevy::ecs::schedule::common_conditions::on_message::<
                            AssetEvent<UsdStageAsset>,
                        >,
                    ),
                    camera_track::plan_camera_tracks,
                    camera_track::sample_camera_tracks.after(lunco_time::DomainResolveSet),
                )
                    .chain(),
            );
    }
}

// Generates `register_all_commands(app)` (register_type + add_observer for the
// listed command handlers). Called from `UsdBevyPlugin::build`.
lunco_core::register_commands!(camera_switch::on_set_active_camera);

/// A Bevy Asset representing a loaded USD Stage.
///
/// Contains a flattened USD reader with all external references resolved.
/// Created by the `UsdLoader` asset loader when a `.usda` file is loaded.
#[derive(Asset, TypePath, Clone)]
pub struct UsdStageAsset {
    /// Flattened, composed scene data (all references resolved). Send-safe
    /// `sdf::Data`; query it with the [`usd_data`] helpers.
    pub reader: Arc<UsdData>,
    /// The `Send` layer-closure recipe for rebuilding the live canonical
    /// [`Stage`](openusd::usd::Stage) on the main thread (Ph0′). `Some` for
    /// assets produced by the async [`UsdLoader`]; `None` for in-memory /
    /// preview constructions that never fetched a closure. A main-thread system
    /// ([`canonical::sync_canonical_stages`]) turns this into a `CanonicalStage`.
    pub recipe: Option<StageRecipe>,
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
        // `LuncoUsdResolver` (filesystem-free, native + wasm). Fetch the layer
        // closure into a `Send` recipe FIRST (so the canonical live `Stage` can be
        // rebuilt on the main thread — `Stage` is `!Send`, can't cross here), then
        // build+flatten it into a Send-safe `sdf::Data` for the downstream visual /
        // physics / cosim readers. The recipe rides along in the asset (Ph0′).
        let recipe = compose::fetch_layer_closure(load_context, &root_asset_path, bytes).await?;
        let stage = compose::build_stage_from_closure(&recipe)?;
        let data = compose::flatten_stage(&stage)?;

        Ok(UsdStageAsset {
            reader: Arc::new(data),
            recipe: Some(recipe),
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
    /// A single `xformOp:transform` matrix drives the full pose.
    Matrix,
    /// Piecewise TRS: only the flagged channels carry `timeSamples`.
    Trs { translate: bool, rotate: bool, scale: bool },
    /// No animated transform channels (material/visibility-only prim).
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

        // UsdGeomCamera (`def Camera`) → an inactive Bevy `Camera3d` (see
        // `camera.rs`). Standard USD camera prim; which one renders is Bevy's
        // `Camera::is_active`, chosen by the switch mechanism in `lunco-avatar`.
        // A camera nested under a moving prim rides it via the shared transform
        // path below + `ChildOf` propagation ("camera on a rover").
        camera::instantiate_camera_prim(reader, &sdf_path, prim_type.as_deref(), commands, entity);

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
        } else if prim_type.as_deref() == Some("Mesh") {
            // Native UsdGeomMesh: decode points/faceVertexIndices/normals/st
            // into a Bevy mesh. (Falls through to `None` — no fallback
            // primitive — if the topology attrs are missing/malformed.)
            build_usd_mesh(reader, &sdf_path).map(|m| meshes.add(m))
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
        if let Some(src) =
            get_attribute_as_string(reader, &sdf_path, "lunco:script").filter(|s| !s.trim().is_empty())
        {
            commands
                .entity(entity)
                .insert(lunco_core::EmbeddedScenarioSource(src));
        } else if let Some(path) = get_attribute_as_string(reader, &sdf_path, "lunco:scriptPath")
            .filter(|s| !s.trim().is_empty())
        {
            // File-backed scenario: `lunco:scriptPath` references a `.rhai` asset.
            // Stamp a lunco_core path marker; lunco-scripting loads it via the
            // AssetServer (wasm-safe) and swaps in EmbeddedScenarioSource. Inline
            // `lunco:script` wins if both are present.
            commands
                .entity(entity)
                .insert(lunco_core::EmbeddedScenarioPath(path));
        }

        // Custom `lunco:vessel = "true"` marks this prim as possessable by
        // stamping `FlightSoftware` — the unified control-surface tag. There is
        // no separate `Vessel` marker: "possessable/controllable" is exactly
        // "has a control surface", so the avatar routes plain-clicks to
        // `PossessVessel` for anything carrying `FlightSoftware` (or a Modelica
        // `SimComponent`). Rovers get `FlightSoftware` from `PhysxVehicleContextAPI`
        // instead; a standalone rigid body (lander, spacecraft) needs this tag.
        // Empty `port_map` is fine — it means "possessable, no digital actuator
        // ports of its own" (a lander's actuation is its Modelica inputs).
        if let Some(val) = get_attribute_as_string(reader, &sdf_path, "lunco:vessel") {
            if val.eq_ignore_ascii_case("true") {
                commands.entity(entity).insert(lunco_fsw::FlightSoftware::default());
            }
        }

        // Per-vessel intent→port control map (stage 2 of control), authored as a
        // `Controls` child scope: each child prim's NAME is the intent, with
        // `string lunco:port` + `double lunco:scale`. Authored inline OR pulled in
        // from a shared profile class (`inherits = </_RoverControl>`); either way
        // it's already composed into this flattened data. When absent, the
        // controller stamps a topology default at possess. Fully data-driven: a
        // vessel declares what its inputs actuate with no Rust change.
        if let Some(controls) = reader
            .prim_children(&sdf_path)
            .into_iter()
            .find(|c| c.name() == Some("Controls"))
        {
            let entries: Vec<(String, String, f64)> = reader
                .prim_children(&controls)
                .into_iter()
                .filter_map(|bind| {
                    let intent = bind.name()?.to_string();
                    let port = reader.prim_attribute_value::<String>(&bind, "lunco:port")?;
                    let scale = reader.prim_attribute_value::<f64>(&bind, "lunco:scale")?;
                    Some((intent, port, scale))
                })
                .collect();
            if let Some(binding) = lunco_core::ControlBinding::from_intent_entries(&entries) {
                commands.entity(entity).insert(binding);
            }
        }

        // Per-prim script params: `lunco:params = "wmax=1.05, lmax=3.6"`. Parsed
        // into a `ScriptParams` map a reusable script reads via `param(me, key,
        // default)` — the typed, fast alternative to inferring config from a name.
        if let Some(spec) = get_attribute_as_string(reader, &sdf_path, "lunco:params") {
            let mut map = std::collections::HashMap::new();
            for entry in spec.split(',') {
                if let Some((k, v)) = entry.split_once('=') {
                    if let Ok(val) = v.trim().parse::<f64>() {
                        map.insert(k.trim().to_string(), val);
                    }
                }
            }
            if !map.is_empty() {
                commands.entity(entity).insert(lunco_core::ScriptParams(map));
            }
        }

        // Tutorial chain: `lunco:nextScene = "scenes/foo.usda"` declares the scene
        // to load when this scene's mission completes. Stamped as a `NextScene`
        // marker; a generic handler (lunco-tutorial) loads it on MISSION_COMPLETE.
        if let Some(next) = get_attribute_as_string(reader, &sdf_path, "lunco:nextScene")
            .filter(|s| !s.trim().is_empty())
        {
            commands.entity(entity).insert(lunco_core::NextScene(next));
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
        // Full local transform: `xformOpOrder` composition when authored, else a
        // full `xformOp:transform` matrix, else piecewise translate + the full
        // rotation set (Euler orders, `orient`, single-axis) + scale. Each
        // component is applied with a spawn-preservation guard so an identity/zero
        // USD value doesn't clobber a code-set spawn pose.
        let usd_tf = local_transform_at(reader, &sdf_path, 0.0);
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
                let target = Vec3::new(tx as f32, ty as f32, tz as f32);
                let eye = transform.translation;
                if (target - eye).length_squared() > 1e-6 {
                    transform.rotation = Transform::from_translation(eye)
                        .looking_at(target, Vec3::Y)
                        .rotation;
                }
            }
        }
        // `xformOp:scale` (UsdGeomXformable) — non-uniform scaling composed with
        // translate + rotate. Spec-compliant `Cube` prims rely on this to express
        // width/height/depth without the legacy `width`/`height`/`depth`
        // attributes. The composed transform (matrix / xformOpOrder) carries scale too.
        let usd_scale = usd_tf.map(|t| t.scale);
        if let Some(v) = usd_scale {
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

        // Tag entities carrying ANY animated channel (xform, visibility, or a
        // bound-shader / displayColor material input) so the per-frame samplers
        // drive them (doc 19). The query stays empty for static scenes.
        // `bind_animated_to_preview` then binds the tagged entity to the
        // animation-preview domain so the transport (play/pause/scrub/rate) reaches it.
        if prim_is_animated(reader, &sdf_path) {
            commands.entity(entity).insert(UsdAnimated);
        }

        // Tag a prim authoring `lunco:activeCamera` timeSamples as an editorial
        // camera track (doc 35): its keys drive `SetActiveCamera` cuts over time.
        // `bind_camera_tracks_to_preview` then binds it to the animation-preview
        // domain so the transport scrubs the cuts.
        if camera_track::prim_is_camera_track(reader, &sdf_path) {
            commands.entity(entity).insert(camera_track::CameraTrack);
        }

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
            let child_entity = match (load_into_grid, &child_member) {
                (Some(LoadIntoGrid(grid)), Some(member)) => commands
                    .spawn((
                        base_components,
                        CellCoord::default(),
                        lunco_core::GridAnchor,
                        ChildOf(*grid),
                        member.clone(),
                    ))
                    .id(),
                (Some(LoadIntoGrid(grid)), None) => commands
                    .spawn((base_components, CellCoord::default(), lunco_core::GridAnchor, ChildOf(*grid)))
                    .id(),
                (None, Some(member)) => {
                    commands.spawn((base_components, ChildOf(entity), member.clone())).id()
                }
                (None, None) => commands.spawn((base_components, ChildOf(entity))).id(),
            };

            // A prim that declares `lunco:spawnable = true` — authored on the prim
            // or COMPOSED from a referenced wrapper (the `structures/*.usda` model
            // wrappers all set it) — is a placeable "unit". Tag it `SelectableRoot`
            // so a click on a deep glb sub-mesh resolves UP to this prim (via
            // `find_selectable`), not the leaf. That matters for the transform
            // gizmo: the gizmo crate reads a target's LOCAL `Transform` and treats
            // it as world, but a glb leaf carries a parent-local (~0) transform, so
            // targeting the leaf drops the gizmo at the world origin. This prim
            // carries the authored placement transform (== world when its scene-root
            // ancestor sits at identity), so the gizmo lands on the object. Works
            // whether the prim is Grid-direct OR nested under a referenced scene.
            // Scenes author `spawnable = false` (never a target); terrain/props
            // without the flag fall through to the leaf as before.
            if light::get_attribute_as_bool(reader, &child_path, "lunco:spawnable").unwrap_or(false) {
                commands.entity(child_entity).insert(lunco_core::SelectableRoot);
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

/// Maps a `UsdUVTexture` `inputs:wrapS`/`inputs:wrapT` token to a Bevy sampler
/// address mode. USD's `"useMetadata"` (and absent) fall back to `Repeat` —
/// the common authored intent for tiled textures — rather than the spec's
/// metadata-then-black, which we can't read from the file header here.
fn usd_wrap_to_address(wrap: Option<&str>) -> bevy::image::ImageAddressMode {
    use bevy::image::ImageAddressMode;
    match wrap {
        Some("clamp") => ImageAddressMode::ClampToEdge,
        Some("mirror") => ImageAddressMode::MirrorRepeat,
        Some("black") => ImageAddressMode::ClampToBorder,
        _ => ImageAddressMode::Repeat,
    }
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

    // UsdPreviewSurface transparency + refraction. Default opaque (alpha 1) and
    // the glass-ish `ior` 1.5 USD uses; overridden only when a bound shader
    // authors `inputs:opacity` / `inputs:opacityThreshold` / `inputs:ior`.
    //
    // Geometry-baseline transparency: the standard UsdGeomGprim
    // `primvars:displayOpacity` lets a simple prim be translucent WITHOUT a
    // bound shader network (used by the waypoint-marker asset). A bound shader's
    // `inputs:opacity` still wins below. A sub-1 value flips `AlphaMode::Blend`
    // via the rule further down.
    let mut alpha = get_attribute_as_f32(reader, sdf_path, "primvars:displayOpacity")
        .or_else(|| get_attribute_as_f32(reader, sdf_path, "displayOpacity"))
        .unwrap_or(1.0);
    let mut ior = 1.5f32;
    let mut opacity_threshold = 0.0f32;
    let mut opacity_connected = false;

    // Specular-workflow tint (default white = untinted) + clearcoat layer.
    let mut specular_tint = Color::WHITE;
    let mut clearcoat = 0.0f32;
    let mut clearcoat_roughness = 0.0f32;

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
        let load_tex = |input: &str, is_color: bool| -> Option<Handle<Image>> {
            let conn = read_rel_target(reader, &shader_path, input)?;
            let texture_path = parent_prim_path(&conn)?;
            let asset_path = get_attribute_as_string(reader, &texture_path, "inputs:file")?;
            let resolved = resolve_texture_path(asset_server, stage_id, &asset_path)?;

            let is_srgb = match read_token(reader, &texture_path, "inputs:sourceColorSpace").as_deref() {
                Some("sRGB") => true,
                Some("raw") => false,
                _ => is_color, // "auto" / absent → channel default
            };
            let addr_u = usd_wrap_to_address(read_token(reader, &texture_path, "inputs:wrapS").as_deref());
            let addr_v = usd_wrap_to_address(read_token(reader, &texture_path, "inputs:wrapT").as_deref());

            Some(asset_server.load_with_settings::<Image, ImageLoaderSettings>(
                resolved,
                move |s: &mut ImageLoaderSettings| {
                    s.is_srgb = is_srgb;
                    let mut d = ImageSamplerDescriptor::linear();
                    d.address_mode_u = addr_u;
                    d.address_mode_v = addr_v;
                    s.sampler = ImageSampler::Descriptor(d);
                },
            ))
        };

        // diffuseColor: texture, else authored value, else geometry baseline.
        base_color_texture = load_tex("inputs:diffuseColor", true);
        if base_color_texture.is_none() {
            if let Some(c) = get_attribute_as_vec3(reader, &shader_path, "inputs:diffuseColor") {
                base_color = Color::linear_rgb(c.x, c.y, c.z);
            }
        }

        // emissiveColor
        emissive_texture = load_tex("inputs:emissiveColor", true);
        if emissive_texture.is_none() {
            if let Some(c) = get_attribute_as_vec3(reader, &shader_path, "inputs:emissiveColor") {
                emissive = LinearRgba::new(c.x, c.y, c.z, 1.0);
            }
        }

        // metallic
        let metallic_texture = load_tex("inputs:metallic", false);
        if metallic_texture.is_none() {
            if let Some(m) = get_attribute_as_f32(reader, &shader_path, "inputs:metallic") {
                metallic = m;
            }
        }

        // roughness
        let roughness_texture = load_tex("inputs:roughness", false);
        if roughness_texture.is_none() {
            if let Some(r) = get_attribute_as_f32(reader, &shader_path, "inputs:roughness")
                .or_else(|| get_attribute_as_f32(reader, &shader_path, "inputs:perceptual_roughness"))
            {
                roughness = r;
            }
        }

        metallic_roughness_texture = roughness_texture.or(metallic_texture);

        normal_map_texture = load_tex("inputs:normal", false);
        occlusion_texture = load_tex("inputs:occlusion", false);

        if let Some(r) = get_attribute_as_f32(reader, &shader_path, "inputs:reflectance") {
            reflectance = r;
        }

        // Specular workflow: `useSpecularWorkflow = 1` describes a dielectric by
        // `specularColor` instead of metalness → tint the specular and force
        // metallic 0 (USD's specular workflow has no metalness channel).
        if get_attribute_as_f32(reader, &shader_path, "inputs:useSpecularWorkflow").unwrap_or(0.0) >= 0.5 {
            metallic = 0.0;
            if let Some(c) = get_attribute_as_vec3(reader, &shader_path, "inputs:specularColor") {
                specular_tint = Color::linear_rgb(c.x, c.y, c.z);
            }
        }

        // Clearcoat layer (UsdPreviewSurface ↔ StandardMaterial 1:1).
        if let Some(c) = get_attribute_as_f32(reader, &shader_path, "inputs:clearcoat") {
            clearcoat = c;
        }
        if let Some(cr) = get_attribute_as_f32(reader, &shader_path, "inputs:clearcoatRoughness") {
            clearcoat_roughness = cr;
        }

        // Transparency: scalar `inputs:opacity` drives base-color alpha; a
        // *connected* opacity (texture) flips to blended even without a scalar.
        if let Some(o) = get_attribute_as_f32(reader, &shader_path, "inputs:opacity") {
            alpha = o;
        }
        opacity_threshold =
            get_attribute_as_f32(reader, &shader_path, "inputs:opacityThreshold").unwrap_or(0.0);
        opacity_connected = read_rel_target(reader, &shader_path, "inputs:opacity").is_some();

        if let Some(i) = get_attribute_as_f32(reader, &shader_path, "inputs:ior") {
            ior = i;
        }
    }

    // UsdPreviewSurface alpha semantics → Bevy `AlphaMode`: a non-zero
    // `opacityThreshold` is a cutout (`Mask`); otherwise any sub-1 opacity or a
    // connected opacity input is alpha-blended; fully-opaque stays `Opaque` so
    // the depth-sorted transparent pass is only paid for when needed.
    let alpha_mode = if opacity_threshold > 0.0 {
        AlphaMode::Mask(opacity_threshold)
    } else if alpha < 1.0 || opacity_connected {
        AlphaMode::Blend
    } else {
        AlphaMode::Opaque
    };

    entity_cmd.insert((
        Mesh3d(mesh_handle.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: base_color.with_alpha(alpha),
            base_color_texture,
            emissive,
            emissive_texture,
            metallic,
            perceptual_roughness: roughness,
            metallic_roughness_texture,
            normal_map_texture,
            occlusion_texture,
            reflectance,
            ior,
            alpha_mode,
            specular_tint,
            clearcoat,
            clearcoat_perceptual_roughness: clearcoat_roughness,
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

/// Parse a single USD layer's source text with openusd's USDA parser (no PCP
/// composition, no reference resolution) and read a `string`/`token` attribute
/// authored directly on the stage's `defaultPrim`. Returns `None` when the text
/// doesn't parse, the stage declares no `defaultPrim`, or the attribute is
/// absent.
///
/// For metadata that lives on the root prim (e.g. a scene's
/// `lunco:description` tooltip) this is a cheap, composition-free alternative
/// to [`compose_file`] / the async `AssetServer` loader — referenced sub-layers
/// are not consulted, which is correct for root-prim metadata but NOT for
/// attributes that a reference might override. The single parsed layer is the
/// same [`UsdData`] = `sdf::Data` the composed-stage flattener produces, so
/// [`stage_default_prim`] + [`read_token`] work on it unchanged.
pub fn read_default_prim_attr(text: &str, attr: &str) -> Option<String> {
    let data = openusd::usda::parse(text).ok()?;
    let prim_name = stage_default_prim(&data)?;
    let path = SdfPath::new(&format!("/{prim_name}")).ok()?;
    read_token(&data, &path, attr)
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
pub fn read_vec3_f64<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str) -> Option<[f64; 3]> {
    // Fixed-size array forms first (`point3f`/`float3` → `[f32;3]`,
    // `point3d`/`double3` → `[f64;3]`).
    if let Some(v) = reader.scalar::<[f32; 3]>(path, attr) {
        return Some([v[0] as f64, v[1] as f64, v[2] as f64]);
    }
    if let Some(v) = reader.scalar::<[f64; 3]>(path, attr) {
        return Some([v[0], v[1], v[2]]);
    }
    // `Vec<f32>`/`Vec<f64>` array forms (rare in authored USD).
    if let Some(v) = reader.scalar::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some([v[0] as f64, v[1] as f64, v[2] as f64]); }
    }
    if let Some(v) = reader.scalar::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some([v[0], v[1], v[2]]); }
    }
    None
}

/// Time-sampled twin of [`read_vec3_f64`]: evaluates the attribute's
/// `timeSamples` at `time` (held/linear via `openusd::usd::evaluate`), falling
/// back to `default` when there are no samples. Same value-type coverage
/// (`[f32;3]`/`[f64;3]` and the `Vec<f32>`/`Vec<f64>` forms).
pub fn read_vec3_f64_at<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str, time: f64) -> Option<[f64; 3]> {
    if let Some(v) = reader.scalar_at::<[f32; 3]>(path, attr, time) {
        return Some([v[0] as f64, v[1] as f64, v[2] as f64]);
    }
    if let Some(v) = reader.scalar_at::<[f64; 3]>(path, attr, time) {
        return Some([v[0], v[1], v[2]]);
    }
    if let Some(v) = reader.scalar_at::<Vec<f32>>(path, attr, time) {
        if v.len() >= 3 { return Some([v[0] as f64, v[1] as f64, v[2] as f64]); }
    }
    if let Some(v) = reader.scalar_at::<Vec<f64>>(path, attr, time) {
        if v.len() >= 3 { return Some([v[0], v[1], v[2]]); }
    }
    None
}

/// True iff `attr` on `path` actually carries `timeSamples` (not just a
/// `default`). The sampler uses this per-channel so it writes **only** animated
/// channels — a static `xformOp:rotateXYZ` is left exactly as instantiated.
pub fn attr_has_time_samples(reader: &UsdData, path: &SdfPath, attr: &str) -> bool {
    path.append_property(attr)
        .ok()
        .and_then(|ap| reader.field(&ap, "timeSamples"))
        .is_some_and(|v| matches!(v, Value::TimeSamples(_)))
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
pub fn prim_has_xform_time_samples(reader: &UsdData, path: &SdfPath) -> bool {
    attr_has_time_samples(reader, path, "xformOp:translate")
        || attr_has_time_samples(reader, path, "xformOp:scale")
        || attr_has_time_samples(reader, path, "xformOp:transform")
        || prim_rotation_animated(reader, path)
}

/// True iff the prim carries ANY channel the runtime samples per-frame: an
/// xform op, `visibility`, geom `primvars:displayColor`, or a bound surface
/// shader's [`ANIMATED_SHADER_INPUTS`]. Drives the [`UsdAnimated`] tag, so a
/// material-only or visibility-only animation is funnelled the same as xform.
pub fn prim_is_animated(reader: &UsdData, path: &SdfPath) -> bool {
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

/// The stage's `timeCodesPerSecond` (flattened onto the pseudo-root by
/// `flatten_stage`). USD maps a time code `t` to wall-clock `t / tcps` seconds,
/// so the samplers multiply their resolved time (seconds) by this to get the
/// time code to evaluate. Defaults to 24.0 (USD spec) when unauthored or
/// non-positive — the latter guards a malformed stage from freezing animation.
pub fn stage_time_codes_per_second(reader: &UsdData) -> f64 {
    reader
        .field_as::<f64>(&SdfPath::abs_root(), "timeCodesPerSecond")
        .filter(|t| *t > 0.0)
        .unwrap_or(24.0)
}

/// Held-sampled token/string attribute at time code `time` (USD tokens hold,
/// never interpolate). `None` when the attribute has no `timeSamples` or the
/// held sample isn't a token/string/asset value. The animated twin of
/// [`read_token`] — note tokens can't go through `prim_attribute_value_at::<String>`
/// because `String::try_from(Value::Token)` fails on openusd `main`.
pub(crate) fn read_token_at(reader: &UsdData, path: &SdfPath, attr: &str, time: f64) -> Option<String> {
    let attr_path = path.append_property(attr).ok()?;
    let Some(Value::TimeSamples(samples)) = reader.field(&attr_path, "timeSamples") else {
        return None;
    };
    match openusd::usd::evaluate(samples, time, openusd::usd::InterpolationType::Held)? {
        Value::String(s) => Some(s),
        Value::Token(s) => Some(s.to_string()),
        Value::AssetPath(a) => Some(a.as_str().to_string()),
        _ => None,
    }
}

/// Enumerate a token/string channel's authored keys as `(time_code, value)`
/// pairs, ascending. Reads the raw `timeSamples` key times, then resolves each
/// held value through [`read_token_at`] — so it doesn't depend on the inner
/// sample value type. `None`/empty when the attribute carries no token samples.
/// Used to build the [`camera_track::CameraTrackPlan`] key list once.
pub(crate) fn read_token_timesamples(
    reader: &UsdData,
    path: &SdfPath,
    attr: &str,
) -> Vec<(f64, String)> {
    let Ok(attr_path) = path.append_property(attr) else {
        return Vec::new();
    };
    let Some(Value::TimeSamples(samples)) = reader.field(&attr_path, "timeSamples") else {
        return Vec::new();
    };
    samples
        .iter()
        .filter_map(|s| read_token_at(reader, path, attr, s.0).map(|name| (s.0, name)))
        .collect()
}

/// The authored time-code span `(first, last)` of one attribute's `timeSamples`
/// (samples are stored ascending, so the ends are the first/last keys). `None`
/// when the attribute has no samples.
fn attr_sample_span(reader: &UsdData, path: &SdfPath, attr: &str) -> Option<(f64, f64)> {
    let attr_path = path.append_property(attr).ok()?;
    match reader.field(&attr_path, "timeSamples")? {
        Value::TimeSamples(s) if !s.is_empty() => Some((s.first()?.0, s.last()?.0)),
        _ => None,
    }
}

/// The authored time span `(start, end)` in **seconds** across all of `path`'s
/// animated channels (xform ops / `visibility` / geom `primvars:displayColor` /
/// bound-shader [`ANIMATED_SHADER_INPUTS`]), i.e. the time codes divided by the
/// stage `timeCodesPerSecond`. `None` when nothing is sampled. The transport
/// uses this to bound the preview playhead to the real clip length instead of a
/// guessed range.
pub fn animated_time_range(reader: &UsdData, path: &SdfPath) -> Option<(f64, f64)> {
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
fn read_f32_at(reader: &UsdData, path: &SdfPath, attr: &str, time: f64) -> Option<f32> {
    if !attr_has_time_samples(reader, path, attr) {
        return None;
    }
    reader
        .prim_attribute_value_at::<f32>(path, attr, time)
        .or_else(|| reader.prim_attribute_value_at::<f64>(path, attr, time).map(|v| v as f32))
}

/// Sample one xform-op channel **only if it is animated** (has `timeSamples`),
/// evaluated at `time`. Returns `None` for static channels so the caller leaves
/// the instantiated value untouched.
fn sample_animated_vec3(reader: &UsdData, path: &SdfPath, attr: &str, time: f64) -> Option<[f64; 3]> {
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
/// reference `LayerOffset`s are already baked into the composed sample times by
/// PCP at flatten, so no offset compose happens here.
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
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), (With<UsdAnimated>, Without<AnimationPlan>)>,
) {
    for (entity, prim) in &q {
        let Some(stage) = stages.get(&prim.stage_handle) else { continue };
        let reader = &*stage.reader;
        let Ok(sdf_path) = SdfPath::new(prim.path.as_str()) else { continue };

        // Transform: an authored `xformOpOrder` drives the whole stack; else a
        // single `xformOp:transform` matrix; else piecewise TRS with a per-channel
        // gate (a channel without `timeSamples` keeps its code-set spawn value).
        let xform = if has_xform_op_order(reader, &sdf_path) {
            XformDrive::OpOrder
        } else if attr_has_time_samples(reader, &sdf_path, "xformOp:transform") {
            XformDrive::Matrix
        } else {
            let drive = XformDrive::Trs {
                translate: attr_has_time_samples(reader, &sdf_path, "xformOp:translate"),
                rotate: prim_rotation_animated(reader, &sdf_path),
                scale: attr_has_time_samples(reader, &sdf_path, "xformOp:scale"),
            };
            match drive {
                XformDrive::Trs { translate: false, rotate: false, scale: false } => {
                    XformDrive::None
                }
                other => other,
            }
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
        let material = (diffuse || geom_color || opacity)
            .then(|| MaterialPlan { shader, diffuse, geom_color, opacity });

        commands.entity(entity).insert(AnimationPlan {
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
        let Some(stage) = stages.get(&prim.stage_handle) else { continue };
        let reader = &*stage.reader;
        let sdf_path = &plan.path;

        // Resolve this entity's clock — its bound `TimeDomain` (per-object /
        // selection / project / factory) or the world clock when unbound — and
        // convert seconds → USD time code (topology already resolved in the plan).
        let secs = lunco_time::domain_time(&resolved, binding, &world);
        let t = secs * plan.time_codes_per_second;

        // Drive the local transform per the plan's cached channel topology.
        match &plan.xform {
            XformDrive::OpOrder => {
                if let Some(m) = compose_xform_order_at(reader, &sdf_path, t) {
                    tf.translation = m.translation;
                    tf.rotation = m.rotation;
                    tf.scale = m.scale;
                }
            }
            XformDrive::Matrix => {
                if let Some(m) = read_matrix_transform_at(reader, &sdf_path, t) {
                    tf.translation = m.translation;
                    tf.rotation = m.rotation;
                    tf.scale = m.scale;
                }
            }
            XformDrive::Trs { translate, rotate, scale } => {
                if *translate {
                    if let Some(v) = sample_animated_vec3(reader, &sdf_path, "xformOp:translate", t) {
                        tf.translation = Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32);
                    }
                }
                // Any animated rotation channel → recompose the full local rotation
                // (Euler order / `orient` slerp / single-axis) at `t`.
                if *rotate {
                    if let Some(q) = local_rotation_at(reader, &sdf_path, t) {
                        tf.rotation = q;
                    }
                }
                if *scale {
                    if let Some(v) = sample_animated_vec3(reader, &sdf_path, "xformOp:scale", t) {
                        tf.scale = Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32);
                    }
                }
            }
            XformDrive::None => {}
        }

        // Animated `visibility` (token, held): `invisible` → `Hidden`, anything
        // else → `Inherited`. Skipped entirely unless the plan flags it, so a prim
        // animated only in xform/material never churns visibility change-detection.
        if plan.visibility {
            if let Some(tok) = read_token_at(reader, &sdf_path, "visibility", t) {
                let want =
                    if tok == "invisible" { Visibility::Hidden } else { Visibility::Inherited };
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
/// [`UsdAnimated`] entity that owns a `StandardMaterial`, sample the bound
/// surface shader's animated `inputs:diffuseColor` / `inputs:opacity` (or the
/// geom's `primvars:displayColor`) at the entity's resolved time code and write
/// them into the live material asset. Each channel is gated on
/// [`attr_has_time_samples`], so an entity animated only in xform/visibility
/// does a few cheap `HashMap` lookups and touches no material. Runs in `Update`
/// after [`lunco_time::DomainResolveSet`], like the transform sampler.
pub fn sample_usd_material_animation(
    world: Res<lunco_time::WorldTime>,
    resolved: Res<lunco_time::ResolvedDomains>,
    stages: Res<Assets<UsdStageAsset>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    q: Query<
        (
            &UsdPrimPath,
            &AnimationPlan,
            &MeshMaterial3d<StandardMaterial>,
            Option<&lunco_time::TimeBinding>,
        ),
        With<UsdAnimated>,
    >,
) {
    for (prim, plan, mat_handle, binding) in &q {
        // Cheap gate: the plan already resolved the shader + which channels move.
        let Some(mat) = &plan.material else { continue };
        let Some(stage) = stages.get(&prim.stage_handle) else { continue };
        let reader = &*stage.reader;
        let sdf_path = &plan.path;

        let secs = lunco_time::domain_time(&resolved, binding, &world);
        let t = secs * plan.time_codes_per_second;
        let Some(material) = materials.get_mut(&mat_handle.0) else { continue };

        // Base color: a shader `inputs:diffuseColor` wins over geom displayColor.
        // USD `color3f` is linear scene-referred (matches `apply_standard_material`).
        let color_src = if mat.diffuse {
            mat.shader.as_ref()
        } else if mat.geom_color {
            Some(sdf_path)
        } else {
            None
        };
        let color_attr = if mat.diffuse { "inputs:diffuseColor" } else { "primvars:displayColor" };
        if let Some(src) = color_src {
            if let Some(c) = read_vec3_f64_at(reader, src, color_attr, t) {
                let a = material.base_color.alpha();
                material.base_color =
                    Color::linear_rgb(c[0] as f32, c[1] as f32, c[2] as f32).with_alpha(a);
            }
        }

        // Opacity → base-color alpha. If a fully-opaque material starts being
        // animated below 1.0, promote it to `Blend` so the transparency shows.
        if mat.opacity {
            if let Some(o) =
                read_f32_at(reader, mat.shader.as_ref().unwrap_or(sdf_path), "inputs:opacity", t)
            {
                material.base_color = material.base_color.with_alpha(o);
                if o < 1.0 && material.alpha_mode == AlphaMode::Opaque {
                    material.alpha_mode = AlphaMode::Blend;
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
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), (Added<UsdAnimated>, Without<lunco_time::TimeBinding>)>,
    mut playback: Query<&mut lunco_time::Playback>,
) {
    let Some(preview) = preview else { return };
    let mut span: Option<(f64, f64)> = None;
    for (entity, prim) in &q {
        commands
            .entity(entity)
            .insert(lunco_time::TimeBinding { domain: preview.domain });
        // Union this clip's authored span into the range we'll grow the domain to.
        if let Some(stage) = stages.get(&prim.stage_handle) {
            if let Ok(sp) = SdfPath::new(prim.path.as_str()) {
                if let Some((a, b)) = animated_time_range(&stage.reader, &sp) {
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
        let (lo, hi) = if pb.bounded() { (pb.start.min(a), pb.end.max(b)) } else { (a, b) };
        pb.start = lo;
        pb.end = hi;
    }
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
pub fn read_token<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str) -> Option<String> {
    match reader.attr_value(path, attr)? {
        Value::String(s) => Some(s),
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

/// USD rotation xform-ops, in sampler precedence: the quaternion `orient`, then
/// the six Euler-order triples, then the single-axis scalars. A prim normally
/// authors exactly one; when several are present they compose in this order
/// (`local_rotation_at`).
pub const ROTATION_OPS: [&str; 10] = [
    "xformOp:orient",
    "xformOp:rotateXYZ", "xformOp:rotateXZY", "xformOp:rotateYXZ",
    "xformOp:rotateYZX", "xformOp:rotateZXY", "xformOp:rotateZYX",
    "xformOp:rotateX", "xformOp:rotateY", "xformOp:rotateZ",
];

/// Map a USD Euler-order op name + authored **degrees** (`float3`, each
/// component the angle about that axis) to a Bevy `Quat`. The op-name letter
/// order is the application sequence. `None` for a non-Euler-order op name.
fn euler_op_to_quat(op: &str, deg: Vec3) -> Option<Quat> {
    let (x, y, z) = (deg.x.to_radians(), deg.y.to_radians(), deg.z.to_radians());
    let q = match op {
        "xformOp:rotateXYZ" => Quat::from_euler(EulerRot::XYZ, x, y, z),
        "xformOp:rotateXZY" => Quat::from_euler(EulerRot::XZY, x, z, y),
        "xformOp:rotateYXZ" => Quat::from_euler(EulerRot::YXZ, y, x, z),
        "xformOp:rotateYZX" => Quat::from_euler(EulerRot::YZX, y, z, x),
        "xformOp:rotateZXY" => Quat::from_euler(EulerRot::ZXY, z, x, y),
        "xformOp:rotateZYX" => Quat::from_euler(EulerRot::ZYX, z, y, x),
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
        Value::Quatd(q) => Some(Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32)),
        Value::Quath(q) => Some(Quat::from_xyzw(q.x.to_f32(), q.y.to_f32(), q.z.to_f32(), q.w.to_f32())),
        _ => None,
    }
}

/// One attribute's value at time code `time` — `timeSamples` interpolated
/// (held/linear; quaternions slerp) when authored, else the `default` opinion.
/// `None` when the attribute is absent. The shared read for the rotation /
/// matrix composers, which serve both static decode and the animation sampler.
fn value_at(reader: &UsdData, path: &SdfPath, attr: &str, time: f64) -> Option<Value> {
    let ap = path.append_property(attr).ok()?;
    if let Some(Value::TimeSamples(s)) = reader.field(&ap, "timeSamples") {
        if let Some(v) = openusd::usd::evaluate(s, time, openusd::usd::InterpolationType::Linear) {
            return Some(v);
        }
    }
    reader.field(&ap, "default").cloned()
}

/// A scalar numeric attribute (`float`/`double`, or integer-authored angles) at
/// time `time` (timeSamples-or-default). The int fallback avoids the silent-`None`
/// trap when an angle is authored as a bare integer (`rotateZ = 90`). `None` when
/// absent or non-numeric.
fn read_scalar_f32_at<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str, time: f64) -> Option<f32> {
    reader
        .scalar_at::<f32>(path, attr, time)
        .or_else(|| reader.scalar_at::<f64>(path, attr, time).map(|v| v as f32))
        .or_else(|| match reader.attr_value_at(path, attr, time)? {
            Value::Int(v) => Some(v as f32),
            Value::Int64(v) => Some(v as f32),
            _ => None,
        })
}

/// Composed local **rotation** at time code `time` from whatever rotation
/// xform-op(s) the prim authors: quaternion `orient` (slerped), else an
/// Euler-order triple (`rotateXYZ`…`rotateZYX`), else single-axis `rotateX/Y/Z`
/// composed about X then Y then Z. Each channel reads its `default` when static,
/// so this serves both load-time decode (any `time`) and the animation sampler.
/// `None` when the prim authors no rotation op.
pub fn local_rotation_at<R: UsdRead>(reader: &R, path: &SdfPath, time: f64) -> Option<Quat> {
    // 1. Quaternion orient wins.
    if let Some(q) = reader.attr_value_at(path, "xformOp:orient", time).and_then(|v| quat_from_value(&v)) {
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
pub fn read_matrix_transform_at<R: UsdRead>(reader: &R, path: &SdfPath, time: f64) -> Option<Transform> {
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
fn prim_rotation_animated(reader: &UsdData, path: &SdfPath) -> bool {
    ROTATION_OPS.iter().any(|op| attr_has_time_samples(reader, path, op))
}

/// The prim's authored `xformOpOrder` (the ordered op-token list), or `None`
/// when unauthored or empty. When authored it is the **authoritative** op
/// sequence — [`compose_xform_order_at`] honors it exactly, including non-TRS
/// orders the piecewise decode can't express.
fn read_xform_op_order<R: UsdRead>(reader: &R, path: &SdfPath) -> Option<Vec<String>> {
    let order: Vec<String> = match reader.attr_value(path, "xformOpOrder")? {
        Value::TokenVec(v) => v.iter().map(|t| t.to_string()).collect(),
        Value::StringVec(v) => v,
        _ => return None,
    };
    (!order.is_empty()).then_some(order)
}

/// True iff the prim authors a non-empty `xformOpOrder` (so its local transform
/// is defined by the ordered op stack, not the implicit TRS fallback).
fn has_xform_op_order(reader: &UsdData, path: &SdfPath) -> bool {
    read_xform_op_order(reader, path).is_some()
}

/// The glam matrix for one `xformOp:*` token at time `time` (already in glam's
/// column-vector form). Handles every op kind — translate / scale / the six
/// Euler orders / single-axis / `orient` / the full `transform` matrix — keyed
/// by the type segment after `xformOp:` (so a named op like
/// `xformOp:translate:pivot` still resolves). `None` for an unknown or absent op.
fn op_matrix_at<R: UsdRead>(reader: &R, path: &SdfPath, token: &str, time: f64) -> Option<Mat4> {
    let kind = token.strip_prefix("xformOp:")?.split(':').next()?;
    let vec3 = |v: [f64; 3]| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32);
    let m = match kind {
        "translate" => Mat4::from_translation(vec3(read_vec3_f64_at(reader, path, token, time)?)),
        "scale" => Mat4::from_scale(vec3(read_vec3_f64_at(reader, path, token, time)?)),
        "orient" => Mat4::from_quat(quat_from_value(&reader.attr_value_at(path, token, time)?)?),
        "transform" => match reader.attr_value_at(path, token, time)? {
            Value::Matrix4d(m) => Mat4::from_cols_array(&std::array::from_fn(|i| m.0[i] as f32)),
            _ => return None,
        },
        "rotateX" => Mat4::from_rotation_x(read_scalar_f32_at(reader, path, token, time)?.to_radians()),
        "rotateY" => Mat4::from_rotation_y(read_scalar_f32_at(reader, path, token, time)?.to_radians()),
        "rotateZ" => Mat4::from_rotation_z(read_scalar_f32_at(reader, path, token, time)?.to_radians()),
        "rotateXYZ" | "rotateXZY" | "rotateYXZ" | "rotateYZX" | "rotateZXY" | "rotateZYX" => {
            let v = read_vec3_f64_at(reader, path, token, time)?;
            Mat4::from_quat(euler_op_to_quat(&format!("xformOp:{kind}"), vec3(v))?)
        }
        _ => return None,
    };
    Some(m)
}

/// Compose the prim's local `Transform` at time `time` from its `xformOpOrder`,
/// honoring op order and `!invert!` prefixes (USD §`UsdGeomXformable`). USD uses
/// row vectors with the composite `M = M(opₙ)·…·M(op₀)` (the **last** listed op
/// is applied first to the geometry — openusd's `Matrix4d::from_trs` builds
/// `S·R·T` for the standard `["translate","rotateXYZ","scale"]`). In glam's
/// column-vector form that is `m₀·m₁·…·mₙ`, so each op **right**-multiplies the
/// accumulator. `None` when no `xformOpOrder` is authored. A listed op that fails
/// to read is skipped (treated as identity), matching USD's lenient stack.
pub fn compose_xform_order_at<R: UsdRead>(reader: &R, path: &SdfPath, time: f64) -> Option<Transform> {
    let order = read_xform_op_order(reader, path)?;
    let mut m = Mat4::IDENTITY;
    for token in &order {
        let (token, invert) = match token.strip_prefix("!invert!") {
            Some(rest) => (rest, true),
            None => (token.as_str(), false),
        };
        let Some(op_mat) = op_matrix_at(reader, path, token, time) else { continue };
        m *= if invert { op_mat.inverse() } else { op_mat };
    }
    Some(Transform::from_matrix(m))
}

/// The prim's full local `Transform` at time `time`: `xformOpOrder` composition
/// when authored (authoritative), else a full `xformOp:transform` matrix, else
/// the implicit-order piecewise fallback (translate + [`local_rotation_at`] +
/// scale). `None` when the prim authors no xform op at all — the caller then
/// keeps the entity's existing transform. Shared by the static decoder and the
/// animation sampler so both agree.
pub fn local_transform_at<R: UsdRead>(reader: &R, path: &SdfPath, time: f64) -> Option<Transform> {
    if let Some(tf) = compose_xform_order_at(reader, path, time) {
        return Some(tf);
    }
    if let Some(tf) = read_matrix_transform_at(reader, path, time) {
        return Some(tf);
    }
    let t = read_vec3_f64_at(reader, path, "xformOp:translate", time);
    let r = local_rotation_at(reader, path, time);
    let s = read_vec3_f64_at(reader, path, "xformOp:scale", time);
    if t.is_none() && r.is_none() && s.is_none() {
        return None;
    }
    let vec3 = |v: [f64; 3]| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32);
    Some(Transform {
        translation: t.map(vec3).unwrap_or(Vec3::ZERO),
        rotation: r.unwrap_or(Quat::IDENTITY),
        scale: s.map(vec3).unwrap_or(Vec3::ONE),
    })
}

/// Canonical local-transform decode via [`local_transform_at`] — `xformOpOrder`
/// composition when authored, else a full `xformOp:transform`, else piecewise
/// translate + the full rotation set ([`local_rotation_at`]). Scale is forced to
/// `ONE` (callers that need `xformOp:scale` compose it themselves; avian
/// pre-applies scale onto the collider instead). Avian downcasts the resulting
/// `Transform` to `DVec3`/`DQuat` at its call site.
pub fn read_transform_from_usd<R: UsdRead>(reader: &R, path: &SdfPath) -> Transform {
    match local_transform_at(reader, path, 0.0) {
        Some(tf) => Transform { scale: Vec3::ONE, ..tf },
        None => Transform::IDENTITY,
    }
}

/// Spawn footprint of a vehicle, derived in real time from the composed USD
/// geometry — the single source of truth for spawn sizing (no hand-tuned
/// per-asset table, no `lunco:spawnLift` attribute that can drift from the
/// mesh). Computed by walking the same composed stage that `sync_usd_visuals`
/// instantiates, so the placement solver and the live entity can never disagree.
#[derive(Clone, Copy, Debug, Default)]
pub struct WheelFootprint {
    /// Half of the wheel-track width along X (metres). The placement solver
    /// samples terrain at `±half_w` from the click point to fit a slope normal.
    pub half_w: f64,
    /// Half of the wheelbase along Z (metres).
    pub half_l: f64,
    /// Rest height: signed distance from the root origin down to the lowest
    /// wheel ground contact. The solver lifts the root by this along the
    /// terrain normal so the wheels sit on — not in or above — the ground.
    pub contact_depth: f64,
}

/// Derive the [`WheelFootprint`] of a vehicle by walking the composed USD stage
/// from `root_prim` (e.g. `"/RockerBogie"`).
///
/// Recursively composes each prim's [`local_transform_at`] down the hierarchy;
/// any prim carrying `physxVehicleWheel:index` (the universal wheel signal —
/// present on every wheel across the raycast and physical drivetrains) is
/// treated as a wheel. Its ground contact is `center - radius·Y` in the root's
/// **level** frame; the placement solver re-orients this box to the terrain
/// normal afterwards. Wheel authoring never tilts the cylinder, so the level
/// frame is exact for the pre-slope-fit footprint.
///
/// Returns `None` when no wheel prims are found (non-vehicle assets use the
/// caller's default footprint).
pub fn wheel_footprint(reader: &UsdData, root_prim: &str) -> Option<WheelFootprint> {
    let Ok(root) = SdfPath::new(root_prim) else { return None };
    let mut contacts: Vec<bevy::math::DVec3> = Vec::new();
    let root_tf = local_transform_at(reader, &root, 0.0).unwrap_or_default();
    collect_wheel_contacts(reader, &root, root_tf, &mut contacts);
    if contacts.is_empty() {
        return None;
    }
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_z = f64::INFINITY;
    let mut max_z = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    for c in &contacts {
        min_x = min_x.min(c.x);
        max_x = max_x.max(c.x);
        min_z = min_z.min(c.z);
        max_z = max_z.max(c.z);
        min_y = min_y.min(c.y);
    }
    Some(WheelFootprint {
        half_w: (max_x - min_x) * 0.5,
        half_l: (max_z - min_z) * 0.5,
        contact_depth: -min_y,
    })
}

/// Recursive helper for [`wheel_footprint`]: DFS the prim tree composing
/// transforms, and record each wheel's ground contact in the root's level frame.
fn collect_wheel_contacts(
    reader: &UsdData,
    path: &SdfPath,
    parent_tf: Transform,
    contacts: &mut Vec<bevy::math::DVec3>,
) {
    for child in reader.prim_children(path) {
        if !reader.prim_is_active(&child) {
            continue;
        }
        // `parent_tf * local` composes the child's transform in the root frame
        // (Bevy `Transform * Transform` = parent ∘ child-local). Scale is kept
        // because USD propagates parent scale to child positions — stripping it
        // would misplace wheels under any scaled intermediate Xform.
        let local = local_transform_at(reader, &child, 0.0).unwrap_or_default();
        let world = parent_tf * local;
        if reader
            .prim_attribute_value::<i32>(&child, "physxVehicleWheel:index")
            .is_some()
        {
            let radius = reader
                .prim_attribute_value::<f64>(&child, "radius")
                .unwrap_or(0.25);
            let center = world.translation.as_dvec3();
            contacts.push(bevy::math::DVec3::new(
                center.x,
                center.y - radius,
                center.z,
            ));
        }
        collect_wheel_contacts(reader, &child, world, contacts);
    }
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
pub fn read_shape_dims<R: UsdRead>(reader: &R, path: &SdfPath, type_name: &str) -> Option<ShapeDims> {
    let dims = match type_name {
        "Cube" => {
            let size = reader.scalar::<f64>(path, "size").unwrap_or(2.0);
            let legacy_extents = match (
                reader.scalar::<f64>(path, "width"),
                reader.scalar::<f64>(path, "height"),
                reader.scalar::<f64>(path, "depth"),
            ) {
                (Some(w), Some(h), Some(d)) => Some([w, h, d]),
                _ => None,
            };
            ShapeDims::Cube { size, legacy_extents }
        }
        "Sphere" => ShapeDims::Sphere {
            radius: reader.scalar::<f64>(path, "radius").unwrap_or(1.0),
        },
        "Cylinder" => ShapeDims::Cylinder {
            radius: reader.scalar::<f64>(path, "radius").unwrap_or(1.0),
            height: reader.scalar::<f64>(path, "height").unwrap_or(2.0),
        },
        "Cone" => ShapeDims::Cone {
            radius: reader.scalar::<f64>(path, "radius").unwrap_or(1.0),
            height: reader.scalar::<f64>(path, "height").unwrap_or(2.0),
        },
        "Capsule" => ShapeDims::Capsule {
            radius: reader.scalar::<f64>(path, "radius").unwrap_or(0.5),
            height: reader.scalar::<f64>(path, "height").unwrap_or(1.0),
        },
        "Plane" => ShapeDims::Plane {
            width: reader.scalar::<f64>(path, "width").unwrap_or(2.0),
            length: reader.scalar::<f64>(path, "length").unwrap_or(2.0),
        },
        _ => return None,
    };
    Some(dims)
}

/// Reads an `int[]` / `int64[]` USD array attribute (`Value::IntVec` /
/// `Int64Vec`) as `Vec<i32>`. The fixed-array `TryFrom<Value>` impls don't
/// cover integer arrays, so mesh topology (`faceVertexCounts` /
/// `faceVertexIndices`) is matched directly. `None` if absent or not an int
/// array.
fn read_int_array<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str) -> Option<Vec<i32>> {
    match reader.attr_value(path, attr)? {
        Value::IntVec(v) => Some(v),
        Value::Int64Vec(v) => Some(v.iter().map(|&x| x as i32).collect()),
        _ => None,
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
pub fn read_usd_mesh_indexed<R: UsdRead>(
    reader: &R,
    path: &SdfPath,
) -> Option<(Vec<[f32; 3]>, Vec<[u32; 3]>)> {
    let points = reader.scalar::<Vec<[f32; 3]>>(path, "points")?;
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
pub fn build_usd_mesh(reader: &UsdData, path: &SdfPath) -> Option<Mesh> {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::PrimitiveTopology;

    let points = reader.prim_attribute_value::<Vec<[f32; 3]>>(path, "points")?;
    let counts = read_int_array(reader, path, "faceVertexCounts")?;
    let indices = read_int_array(reader, path, "faceVertexIndices")?;
    if points.is_empty() || counts.is_empty() || indices.is_empty() {
        return None;
    }

    // Optional vertex attributes. `primvars:st` is the de-facto UV channel;
    // accept the bare `st`/`st0` spellings some exporters emit.
    let normals = reader.prim_attribute_value::<Vec<[f32; 3]>>(path, "normals");
    let uvs = reader
        .prim_attribute_value::<Vec<[f32; 2]>>(path, "primvars:st")
        .or_else(|| reader.prim_attribute_value::<Vec<[f32; 2]>>(path, "primvars:st0"))
        .or_else(|| reader.prim_attribute_value::<Vec<[f32; 2]>>(path, "st"));

    let n_corners = indices.len();
    let normals_per_vertex = normals.as_ref().is_some_and(|n| n.len() == points.len());
    let normals_per_corner = normals.as_ref().is_some_and(|n| n.len() == n_corners);
    let uvs_per_vertex = uvs.as_ref().is_some_and(|u| u.len() == points.len());
    let uvs_per_corner = uvs.as_ref().is_some_and(|u| u.len() == n_corners);

    let left_handed =
        read_token(reader, path, "orientation").as_deref() == Some("leftHanded");

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
                let tri = if left_handed { [0, k + 1, k] } else { [0, k, k + 1] };
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

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
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

#[cfg(test)]
mod mesh_tests {
    //! Native UsdGeomMesh → Bevy [`Mesh`] decode ([`build_usd_mesh`]).
    use super::*;
    use openusd::sdf::Path as SdfPath;

    fn parse(usda: &str) -> UsdData {
        openusd::usda::parse(usda).expect("parse USDA")
    }

    /// A single quad fan-triangulates to 2 tris (6 unindexed verts); per-vertex
    /// `primvars:st` carries through and missing normals are computed.
    #[test]
    fn quad_triangulates_with_uvs_and_computed_normals() {
        let reader = parse(
            "#usda 1.0\n\
             def Mesh \"Quad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0),(0,1,0)]\n\
             int[] faceVertexCounts = [4]\n\
             int[] faceVertexIndices = [0,1,2,3]\n\
             texCoord2f[] primvars:st = [(0,0),(1,0),(1,1),(0,1)]\n}\n",
        );
        let mesh = build_usd_mesh(&reader, &SdfPath::new("/Quad").unwrap()).expect("mesh built");
        assert_eq!(mesh.count_vertices(), 6, "one quad → two triangles");
        assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some(), "st preserved");
        assert!(mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some(), "normals computed");
    }

    /// Two triangles, no optional attrs → 6 verts, a zeroed UV set, flat normals.
    #[test]
    fn bare_triangles_get_default_uvs() {
        let reader = parse(
            "#usda 1.0\n\
             def Mesh \"Tris\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(0,1,0),(1,1,0)]\n\
             int[] faceVertexCounts = [3,3]\n\
             int[] faceVertexIndices = [0,1,2,1,3,2]\n}\n",
        );
        let mesh = build_usd_mesh(&reader, &SdfPath::new("/Tris").unwrap()).expect("mesh built");
        assert_eq!(mesh.count_vertices(), 6);
        assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some(), "zeroed UVs inserted");
    }

    /// Missing topology attributes → `None` (caller falls back to no mesh).
    #[test]
    fn missing_topology_returns_none() {
        let reader = parse("#usda 1.0\ndef Mesh \"Empty\"\n{\n}\n");
        assert!(build_usd_mesh(&reader, &SdfPath::new("/Empty").unwrap()).is_none());
    }

    /// An index pointing past the end of `points` is rejected, not panicked on.
    #[test]
    fn out_of_range_index_is_rejected() {
        let reader = parse(
            "#usda 1.0\n\
             def Mesh \"Bad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0)]\n\
             int[] faceVertexCounts = [3]\n\
             int[] faceVertexIndices = [0,1,9]\n}\n",
        );
        assert!(build_usd_mesh(&reader, &SdfPath::new("/Bad").unwrap()).is_none());
    }

    /// The collider decode keeps the raw points (4) and fan-triangulates the
    /// quad into two index triples — the form `Collider::trimesh` consumes.
    #[test]
    fn indexed_decode_keeps_points_and_fans_quad() {
        let reader = parse(
            "#usda 1.0\n\
             def Mesh \"Quad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0),(0,1,0)]\n\
             int[] faceVertexCounts = [4]\n\
             int[] faceVertexIndices = [0,1,2,3]\n}\n",
        );
        let (verts, tris) =
            read_usd_mesh_indexed(&reader, &SdfPath::new("/Quad").unwrap()).expect("indexed mesh");
        assert_eq!(verts.len(), 4, "raw points kept (shared verts)");
        assert_eq!(tris, vec![[0, 1, 2], [0, 2, 3]], "fan (0,k,k+1)");
    }

    /// The collider decode rejects malformed topology the same as the render
    /// path, so no bad trimesh reaches the physics engine.
    #[test]
    fn indexed_decode_rejects_bad_topology() {
        let reader = parse(
            "#usda 1.0\n\
             def Mesh \"Bad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0)]\n\
             int[] faceVertexCounts = [3]\n\
             int[] faceVertexIndices = [0,1,9]\n}\n",
        );
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
        assert_eq!(usd_wrap_to_address(Some("clamp")), ImageAddressMode::ClampToEdge);
        assert_eq!(usd_wrap_to_address(Some("mirror")), ImageAddressMode::MirrorRepeat);
        assert_eq!(usd_wrap_to_address(Some("black")), ImageAddressMode::ClampToBorder);
        assert_eq!(usd_wrap_to_address(Some("repeat")), ImageAddressMode::Repeat);
        // "useMetadata" and absent both fall back to Repeat.
        assert_eq!(usd_wrap_to_address(Some("useMetadata")), ImageAddressMode::Repeat);
        assert_eq!(usd_wrap_to_address(None), ImageAddressMode::Repeat);
    }
}


#[cfg(test)]
mod animation_tests {
    //! The USD animation sampler read path: `timeSamples` detection, time-aware
    //! vec3 evaluation, and per-channel "animated only" sampling (doc 19).
    use super::*;
    use openusd::sdf::Path as SdfPath;

    fn parse(usda: &str) -> UsdData {
        openusd::usda::parse(usda).expect("parse USDA")
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
        let reader = parse(SCENE);
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
        let reader = parse(SCENE);
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
        let reader = parse(SCENE);
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
        let reader = parse(SCENE);
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
        let reader = parse(VIS_SCENE);
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
        let reader = parse(ORIENT_SCENE);
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

def Xform "HingeZ"
{
    float xformOp:rotateZ.timeSamples = {
        0: 0.0,
        4: 90.0,
    }
}

def Xform "EulerZYX"
{
    float3 xformOp:rotateZYX = (0, 0, 90)
}

def Xform "Matrixed"
{
    matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (3, 4, 5, 1) )
}
"#;

    #[test]
    fn single_axis_rotation_is_detected_and_composed() {
        let reader = parse(ROTATION_OPS_SCENE);
        let hinge = SdfPath::new("/HingeZ").unwrap();
        // A single-axis `rotateZ` time-sample marks the prim animated.
        assert!(prim_has_xform_time_samples(&reader, &hinge));
        // Held start = 0° → identity; midway (code 2) = 45° about Z.
        assert!(local_rotation_at(&reader, &hinge, 0.0).unwrap().abs_diff_eq(Quat::IDENTITY, 1e-6));
        let q = local_rotation_at(&reader, &hinge, 2.0).unwrap();
        assert!(q.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4), 1e-5));
    }

    #[test]
    fn euler_order_zyx_composes() {
        let reader = parse(ROTATION_OPS_SCENE);
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
        let reader = parse(scene);
        let q = local_rotation_at(&reader, &SdfPath::new("/HalfSpin").unwrap(), 0.0).unwrap();
        assert!(q.abs_diff_eq(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0), 1e-3));
    }

    const ORDER_SCENE: &str = r#"#usda 1.0

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
        let reader = parse(ORDER_SCENE);
        // `["scale","translate"]`: translate is the LAST op → applied first to the
        // geometry, then `scale` (first op) scales it → translation (2,0,0).
        let sf = compose_xform_order_at(&reader, &SdfPath::new("/ScaleFirst").unwrap(), 0.0).unwrap();
        assert!(sf.translation.abs_diff_eq(Vec3::new(2.0, 0.0, 0.0), 1e-5));
        assert!(sf.scale.abs_diff_eq(Vec3::splat(2.0), 1e-5));
        // `["translate","scale"]` (standard order): scale applied first, then the
        // unscaled translate → (1,0,0). Different result ⇒ op order is honored.
        let tf = compose_xform_order_at(&reader, &SdfPath::new("/TranslateFirst").unwrap(), 0.0).unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::splat(2.0), 1e-5));
    }

    #[test]
    fn xform_op_order_standard_matches_piecewise() {
        // Standard-order content (`["translate","rotateXYZ"]`, what the rover
        // assets author) decodes identically to the implicit piecewise path —
        // no regression.
        let reader = parse(ORDER_SCENE);
        let tf = local_transform_at(&reader, &SdfPath::new("/Std").unwrap(), 0.0).unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(5.0, 6.0, 7.0), 1e-5));
        assert!(tf.rotation.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-5));
    }

    #[test]
    fn matrix_transform_decomposes_translation() {
        let reader = parse(ROTATION_OPS_SCENE);
        // Identity rotation/scale, translation in the USD matrix's last row.
        let tf = read_matrix_transform_at(&reader, &SdfPath::new("/Matrixed").unwrap(), 0.0).unwrap();
        assert!(tf.translation.abs_diff_eq(Vec3::new(3.0, 4.0, 5.0), 1e-5));
        assert!(tf.rotation.abs_diff_eq(Quat::IDENTITY, 1e-5));
        assert!(tf.scale.abs_diff_eq(Vec3::ONE, 1e-5));
        // And `read_transform_from_usd` prefers the matrix.
        let full = read_transform_from_usd(&reader, &SdfPath::new("/Matrixed").unwrap());
        assert!(full.translation.abs_diff_eq(Vec3::new(3.0, 4.0, 5.0), 1e-5));
    }

    #[test]
    fn animated_time_range_spans_keys_in_seconds() {
        let reader = parse(SCENE);
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
        let reader = parse(VIS_SCENE);
        assert!(prim_is_animated(&reader, &SdfPath::new("/Blinker").unwrap()));
        // `Solid` keyframes nothing — visibility and translate are both defaults.
        assert!(!prim_is_animated(&reader, &SdfPath::new("/Solid").unwrap()));
        // The xform-animated `Mover` from SCENE is still caught by the broader gate.
        let mover_reader = parse(SCENE);
        assert!(prim_is_animated(&mover_reader, &SdfPath::new("/Mover").unwrap()));
        assert!(!prim_is_animated(&mover_reader, &SdfPath::new("/Static").unwrap()));
    }
}

#[cfg(test)]
mod default_prim_attr_tests {
    //! `read_default_prim_attr` — openusd-parse a single layer and read a
    //! `string`/`token` attribute off its `defaultPrim` (the path the scene
    //! `lunco:description` tooltip uses).
    use super::*;

    const SCENE: &str = "#usda 1.0\n\
        (\n\
            defaultPrim = \"SandboxScene\"\n\
            upAxis = \"Y\"\n\
        )\n\
        def Xform \"SandboxScene\"\n{\n\
            custom bool lunco:spawnable = false\n\
            custom string lunco:description = \"Two cubes joined together.\"\n\
            def Cube \"Ground\"\n{\n}\n\
        }\n";

    #[test]
    fn reads_string_attr_off_default_prim() {
        assert_eq!(
            read_default_prim_attr(SCENE, "lunco:description").as_deref(),
            Some("Two cubes joined together.")
        );
    }

    #[test]
    fn missing_attr_is_none() {
        assert!(read_default_prim_attr(SCENE, "lunco:notAuthored").is_none());
    }

    #[test]
    fn no_default_prim_is_none() {
        // Layer with no `defaultPrim` metadata — even if the attribute exists
        // on a prim, we don't know which prim is the root.
        let src = "#usda 1.0\ndef Xform \"Orphan\"\n{\n    custom string lunco:description = \"x\"\n}\n";
        assert!(read_default_prim_attr(src, "lunco:description").is_none());
    }

    #[test]
    fn unparseable_text_is_none() {
        assert!(read_default_prim_attr("this is not USDA", "lunco:description").is_none());
    }
}
