//! # LunCoSim USD → Simulation Mapping
//!
//! Detects USD simulation schemas (NVIDIA PhysX Vehicles) and maps them to LunCoSim
//! simulation components. This is the **third** plugin in the USD processing pipeline,
//! running after `UsdVisualPlugin` and alongside `UsdAvianPlugin`.
//!
//! ## Detected Schemas
//!
//! | USD Schema | LunCoSim Components | Description |
//! |---|---|---|
//! | `PhysxVehicleContextAPI` | `MobilityRoot` + `OutputPorts` | Topology-derived mobility owner plus its runtime actuator output surface |
//! | `PhysxVehicleWheelAPI` | `WheelRaycast` *or* a rigid body plus generic joint ports | Wheel — kind decided by standard joint authoring |
//!
//! ## Wheel kind: discriminated by standard authoring
//!
//! No custom `lunco:` tokens. Each `PhysxVehicleWheelAPI` wheel becomes:
//!
//! - **Joint-based** if any `def PhysicsRevoluteJoint` in the stage targets
//!   it via `rel physics:body1`. Motor torque comes from the authored Modelica
//!   electrical/mechanical network through its solved shaft boundary; the
//!   constraint is built by `lunco-usd-avian`. The wheel becomes a full
//!   rigid body with collider and the generic solved joint torque boundary.
//! - **Raycast** otherwise. The wheel entity is split into a physics
//!   entity (identity rotation, `RayCaster::new(Dir3::NEG_Y)`) plus a
//!   visual child carrying the cylinder rotation.
//!
//! ## Wheel Entity Splitting (Raycast Only)
//!
//! USD defines each wheel as a **single entity** with a mesh and a rotation (90° Z for
//! wheel orientation). However, LunCoSim's raycast wheels need two entities:
//!
//! 1. **Physics entity** — identity rotation so `RayCaster::new(Dir3::NEG_Y)` casts
//!    straight down (local space). If rotated, rays go sideways and hit the chassis.
//! 2. **Visual child entity** — 90° Z rotation + mesh so the cylinder renders as a
//!    rolling wheel (not a flat pancake).
//!
//! The `process_usd_sim_prims` system performs this split at runtime for raycast wheels.
//! Physical wheels keep the USD entity as-is (mesh + rotation are correct for rendering).
//!
//! ## Why Deferred Processing?
//!
//! The `On<Add, UsdPrimPath>` observer fires when the entity is spawned, but the USD
//! asset may not be loaded yet (async loading). The `process_usd_sim_prims` system runs
//! in the `Update` schedule **after** `sync_usd_visuals` so the canonical stage and
//! render-free simulation intent are available before physics projection. Visual
//! products remain owned by the render pipeline and are not a simulation prerequisite.

use avian3d::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_usd_avian_contracts::{
    AuthoredInitialVelocity, PendingJointAdmission, PendingUsdJoint, ScenePhysicsOwned,
    ShouldBeDynamic,
};
use lunco_usd_avian_filters::filtered_pairs::SharedTireContact;
use lunco_usd_bevy_core::live_edit::{UsdLiveEditOwner, UsdLiveEditRegistry};
use lunco_usd_bevy_scene::{
    UsdPreviewOnly, UsdPrimPath, UsdSceneChangeBatch, UsdSceneGeometryPending, UsdSceneRoot,
    instance_key, is_preview_only,
};
use lunco_usd_bevy_stage::read::{read_authored_bool_strict, read_vec3_f64};
use lunco_usd_bevy_stage::{
    UsdInstanceProjection, UsdInstanceRoot, UsdStageAsset, UsdStageProjectionPlan,
    canonical::CanonicalStages,
};
// Appearance + camera **intent** — this crate must never name `MeshMaterial3d`,
// `StandardMaterial`, `ShaderMaterial` or `Camera3d` (all `bevy_pbr` /
// `bevy_core_pipeline` → wgpu + naga). `lunco-render-bevy` binds these.
// See docs/architecture/render-decoupling.md.
use lunco_materials::ShaderLook;
use lunco_mobility::wheel_kinematics::{body_point_velocity, wheel_hub_pose, wheel_roll_rate};
use lunco_mobility::{
    DifferentialCoupling, JointedWheelTire, Suspension, SuspensionPiston, SuspensionSpring,
    WheelRaycast,
};
use lunco_physics::joint::JointTorqueActuator;
use lunco_physics::raycast::RaycastObservation;
use lunco_port_core::ports::PortDirection;
use lunco_port_core::{Port, PortSurface, PortSurfacePort};
use lunco_render::{PbrLook, SceneCamera};
use lunco_spatial::coords::{GridPos, GridRot, VehicleFrame};
use lunco_usd_sim_authoring::{
    GearDriveValues, SuspensionParams, WheelParams, is_gear_drive, read_gear_drive_type,
    read_gear_drive_values, read_gear_ratio,
};
use lunco_usd_sim_core::{
    GroundColliderPending, PendingDifferential, PendingEntityWork, PhysicalWheel, UsdSimProcessed,
    UsdSimSet,
};
use openusd::schemas::physics::tokens as ptok;
use openusd::sdf::{Path as SdfPath, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};

mod wheel_runtime;

/// Plugin for mapping simulation-specific USD schemas (like NVIDIA PhysX Vehicles)
/// to LunCo's optimized simulation models.
///
/// # Processing Order
///
/// 1. `process_usd_sim_prims` — maps schemas to components after visual projection
/// 2. Generic USD connection derivation — connects authored controller outputs to
///    wheel and joint ports through the common co-simulation fabric
///
/// Lifecycle observers coalesce candidate entity IDs. Processing stays in
/// `process_usd_sim_prims` after visual projection, where canonical stage data
/// and render-free appearance intent are available.
///
/// # Wheel kind dispatch (no custom schemas)
///
/// Each wheel prim with `PhysxVehicleWheelAPI` becomes either a raycast wheel
/// (suspension simulation) or a joint-based wheel (full rigid body + revolute
/// joint), discriminated entirely by **standard OpenUSD authoring**:
///
/// - If any `PhysicsRevoluteJoint` in the stage targets the wheel via its
///   `physics:body1` rel → joint-based path. Motor torque and speed come from
///   the authored Modelica network's solved shaft boundary; the joint
///   constraint itself is built by `lunco-usd-avian`.
/// - Otherwise → raycast path.
///
/// No custom `lunco:` tokens drive this dispatch.
pub struct UsdSimPlugin;

/// Immutable USD topology facts used by the simulation projector for one
/// composed stage revision.  This is deliberately separate from ECS entities:
/// a wheel and its sibling joint can arrive on different frames, while their
/// relationships are already complete in the canonical stage.
#[derive(Default)]
struct StageJointTopology {
    canonical_generation: Option<u64>,
    dirty: bool,
    /// Prim paths whose composed schemas or relationships feed this cache.
    source_paths: HashSet<String>,
    /// Prim paths with authored inputs consumed by this projection owner.
    simulation_candidates: HashSet<String>,
    simulation_candidates_ready: bool,
    /// Prepared vehicle actuator outputs, keyed by the authored mobility root.
    /// Attribute and Modelica collection inspection is done with the immutable
    /// stage plan before per-prim UI projection.
    vehicle_output_ports: HashMap<String, Vec<String>>,
    joint_targets: HashMap<String, String>,
    /// Physical wheel revolute joints and their authored carrier body. The
    /// wheel projector uses this composed relationship instead of assuming a
    /// wheel's immediate parent is the vehicle body.
    physical_wheel_bodies: HashMap<String, String>,
    /// Supported authored joints and their body endpoints. This is a
    /// composition fact, not an ECS observation: the body entities can be
    /// promoted before the joint observer's deferred command has landed.
    authored_joints: HashMap<String, (String, String)>,
    articulation_roots: HashSet<String>,
    wheel_attachment_targets: HashMap<String, String>,
    /// Standard attachment tire bindings, keyed by the referenced wheel path.
    /// The tire may be a separate prim named by the attachment relationship or
    /// the attachment itself when the standard direct-API form is authored.
    wheel_attachment_tires: HashMap<String, String>,
    /// Wheels whose attachment topology is malformed or ambiguous. Keeping the
    /// rejection in the composed-stage scan prevents a first-target or
    /// last-attachment heuristic from silently selecting different tire and
    /// suspension data.
    invalid_wheel_attachments: HashSet<String>,
    /// Standard attachment index, keyed by the referenced wheel path. The
    /// index is authored on the attachment prim, never inferred from wheel
    /// order or copied into a wheel-local field.
    wheel_attachment_indices: HashMap<String, i32>,
}

/// Per-canonical-stage cache of immutable wheel/joint topology.
///
/// The canonical stage generation catches live authored changes; stage-asset
/// events retire an entry when the prepared stage is replaced. Scene-entity
/// projection revisions do not change authored topology and must not trigger a
/// new stage-wide scan as each prim materializes.
#[derive(Resource, Default)]
struct JointTopologyIndex {
    by_stage: HashMap<bevy::asset::AssetId<UsdStageAsset>, StageJointTopology>,
    refresh_pending: HashSet<bevy::asset::AssetId<UsdStageAsset>>,
}

#[derive(Resource, Default)]
struct PreparedJointTopologyTasks {
    pending: HashMap<bevy::asset::AssetId<UsdStageAsset>, PreparedJointTopologyTask>,
    failed: HashMap<bevy::asset::AssetId<UsdStageAsset>, FailedJointTopologyPreparation>,
    next_operation: u64,
    capacity_wait_revision: Option<u64>,
}

#[derive(SystemParam)]
struct JointTopologyPreparationParams<'w> {
    index: ResMut<'w, JointTopologyIndex>,
    tasks: ResMut<'w, PreparedJointTopologyTasks>,
    admission: ResMut<'w, lunco_core_runtime::AsyncWorkAdmission>,
}

#[derive(SystemParam)]
struct UsdStageIdentityParams<'w> {
    stages: Res<'w, Assets<UsdStageAsset>>,
    asset_server: Res<'w, AssetServer>,
}

struct FailedJointTopologyPreparation {
    generation: u64,
    error: String,
}

struct PreparedJointTopologyTask {
    source: PreparedJointTopologySource,
    work_key: Option<lunco_core_runtime::AsyncWorkKey>,
    completion: Arc<Mutex<Option<Result<StageJointTopology, String>>>>,
}

#[derive(Clone)]
enum PreparedJointTopologySource {
    AssetPlan {
        plan: Arc<UsdStageProjectionPlan>,
        generation: u64,
    },
    CanonicalSnapshot {
        asset_plan: Arc<UsdStageProjectionPlan>,
        generation: u64,
    },
}

impl PreparedJointTopologySource {
    fn generation(&self) -> u64 {
        match self {
            Self::AssetPlan { generation, .. } | Self::CanonicalSnapshot { generation, .. } => {
                *generation
            }
        }
    }

    fn asset_plan(&self) -> &Arc<UsdStageProjectionPlan> {
        match self {
            Self::AssetPlan { plan, .. } => plan,
            Self::CanonicalSnapshot { asset_plan, .. } => asset_plan,
        }
    }

    fn is_current(
        &self,
        stage: bevy::asset::AssetId<UsdStageAsset>,
        stage_asset: &UsdStageAsset,
        canonical: &CanonicalStages,
    ) -> bool {
        if self.generation() != canonical.generation_for(stage)
            || !Arc::ptr_eq(self.asset_plan(), &stage_asset.projection_plan)
        {
            return false;
        }
        match self {
            Self::AssetPlan { plan, generation } => {
                *generation == 0 || canonical.prepared_plan_is_current(stage, plan)
            }
            Self::CanonicalSnapshot { .. } => true,
        }
    }
}

impl PreparedJointTopologyTasks {
    fn failed_for_generation(
        &self,
        stage: bevy::asset::AssetId<UsdStageAsset>,
        generation: u64,
    ) -> bool {
        self.failed
            .get(&stage)
            .is_some_and(|failure| failure.generation == generation)
    }

    fn allocate_work_key(&mut self, generation: u64) -> Option<lunco_core_runtime::AsyncWorkKey> {
        let operation = self.next_operation;
        self.next_operation = operation.checked_add(1)?;
        Some(lunco_core_runtime::AsyncWorkKey::new(
            lunco_core_runtime::AsyncWorkKind::UsdPreparation,
            0,
            u128::from_be_bytes(*b"usd-sim-topology"),
            generation,
            operation,
        ))
    }
}

// Keep this owner's requests bounded before they enter shared CPU admission.
const MAX_PREPARED_JOINT_TOPOLOGY_TASKS: usize = 4;

/// Lifecycle-queued USD prims awaiting simulation projection.
#[derive(Resource)]
struct PendingUsdSimPrimWork(PendingEntityWork, Vec<lunco_core::RuntimeDiagnostic>);

// Deferred Commands are applied at the end of this Update system. Admit the
// lowest authored paths while limiting selection and command work per UI frame.
const MAX_USD_SIM_PRIM_PROJECTIONS_PER_UPDATE: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
struct StableUsdSimWork<T> {
    stage_source: String,
    prim_path: String,
    item: T,
}

fn compare_stable_usd_sim_work<T>(
    left: &StableUsdSimWork<T>,
    right: &StableUsdSimWork<T>,
) -> std::cmp::Ordering {
    left.stage_source
        .cmp(&right.stage_source)
        .then_with(|| left.prim_path.cmp(&right.prim_path))
}

fn duplicate_nonpreview_usd_sim_work_keys<T>(
    candidates: &[StableUsdSimWork<T>],
    mut entity_of: impl FnMut(&T) -> Entity,
    mut is_preview: impl FnMut(Entity) -> bool,
    preview_cache: &mut HashMap<Entity, bool>,
) -> HashSet<(String, String)> {
    let mut entities_by_key = HashMap::<(String, String), Vec<Entity>>::new();
    for candidate in candidates {
        entities_by_key
            .entry((candidate.stage_source.clone(), candidate.prim_path.clone()))
            .or_default()
            .push(entity_of(&candidate.item));
    }
    let mut duplicates = HashSet::new();
    for (key, entities) in entities_by_key {
        if entities.len() < 2 {
            continue;
        }
        let mut nonpreview_count = 0;
        for entity in entities {
            let preview = is_preview(entity);
            preview_cache.insert(entity, preview);
            if !preview {
                nonpreview_count += 1;
            }
        }
        if nonpreview_count > 1 {
            duplicates.insert(key);
        }
    }
    duplicates
}

fn stable_stage_source(
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    asset: &UsdStageAsset,
    asset_server: &AssetServer,
) -> Result<String, String> {
    let source = match asset.recipe.as_deref() {
        Some(recipe) if !recipe.root_id.is_empty() => recipe.root_id.clone(),
        Some(_) => return Err("USD stage recipe has an empty root identifier".to_owned()),
        None => asset_server
            .get_path(stage_id)
            .map(|path| path.to_string())
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                "USD stage has neither a recipe root identifier nor an AssetServer source path"
                    .to_owned()
            })?,
    };
    Ok(source)
}

fn append_order_segment(key: &mut String, segment: &str) {
    key.push_str(&segment.len().to_string());
    key.push(':');
    key.push_str(segment);
}

fn stable_usd_physics_order_key(
    stage_source: &str,
    instance_root_path: Option<&str>,
    prim_path: &str,
) -> Result<lunco_physics::PhysicsOrderKey, String> {
    if stage_source.is_empty() {
        return Err("USD stage has no stable source identity".to_owned());
    }
    if prim_path.is_empty() {
        return Err("USD prim has no authored path identity".to_owned());
    }
    let mut key = String::new();
    append_order_segment(&mut key, stage_source);
    if let Some(root_path) = instance_root_path {
        if root_path.is_empty() {
            return Err("USD instance root has no authored prim path".to_owned());
        }
        append_order_segment(&mut key, root_path);
    }
    append_order_segment(&mut key, prim_path);
    Ok(lunco_physics::PhysicsOrderKey(key))
}

/// Find the lowest-ranked simulation rows without charging preview rows to the
/// bounded simulation-work prefix.
fn select_bounded_sim_prim_work<T>(
    mut candidates: Vec<T>,
    maximum: usize,
    mut entity_of: impl FnMut(&T) -> Entity,
    mut compare: impl FnMut(&T, &T) -> std::cmp::Ordering,
    mut is_preview: impl FnMut(Entity) -> bool,
) -> (Vec<T>, Vec<Entity>, Vec<Entity>) {
    if maximum == 0 {
        return (
            Vec::new(),
            candidates.iter().map(&mut entity_of).collect(),
            Vec::new(),
        );
    }

    let candidate_count = candidates.len();
    let mut prefix_count = maximum.min(candidate_count);
    // Prefix expansion reuses membership so each candidate's hierarchy is
    // checked at most once.
    let mut checked = HashSet::new();
    let mut preview_set = HashSet::new();
    let mut previews = Vec::new();
    loop {
        if prefix_count < candidate_count {
            candidates.select_nth_unstable_by(prefix_count, &mut compare);
        }
        let mut eligible_count = 0;
        for candidate in candidates.iter().take(prefix_count) {
            let entity = entity_of(candidate);
            if checked.insert(entity) && is_preview(entity) {
                preview_set.insert(entity);
                previews.push(entity);
            }
            if !preview_set.contains(&entity) {
                eligible_count += 1;
            }
        }
        if eligible_count >= maximum || prefix_count == candidate_count {
            break;
        }
        // Earlier authored paths were previews; widen the window to find the
        // same bounded count of simulation-owned work.
        prefix_count = prefix_count
            .saturating_mul(2)
            .max(prefix_count + 1)
            .min(candidate_count);
    }

    let mut selected = Vec::with_capacity(maximum);
    let mut deferred = Vec::new();
    let mut prefix = candidates.drain(..prefix_count).collect::<Vec<_>>();
    prefix.sort_by(&mut compare);
    for candidate in prefix {
        let entity = entity_of(&candidate);
        if preview_set.contains(&entity) {
            continue;
        }
        if selected.len() < maximum {
            selected.push(candidate);
        } else {
            deferred.push(entity);
        }
    }
    deferred.extend(candidates.iter().map(&mut entity_of));
    (selected, deferred, previews)
}

impl Default for PendingUsdSimPrimWork {
    fn default() -> Self {
        Self(PendingEntityWork::with_initial_discovery(), Vec::new())
    }
}

impl JointTopologyIndex {
    fn observe_scene_change(
        &mut self,
        change: &UsdSceneChangeBatch,
        reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    ) -> bool {
        let Some(topology) = self.by_stage.get_mut(&change.stage_id) else {
            return false;
        };
        let invalidates = topology_change_affects(change, &topology.source_paths, reader);
        for path in change
            .resynced_prim_paths
            .iter()
            .chain(change.info_prim_paths.iter())
        {
            update_simulation_candidate(reader, path, &mut topology.simulation_candidates);
        }
        if invalidates {
            topology.dirty = true;
            self.refresh_pending.insert(change.stage_id);
        } else if !topology.dirty {
            topology.canonical_generation = Some(change.stage_generation);
        }
        true
    }

    fn is_current(&self, stage: bevy::asset::AssetId<UsdStageAsset>, generation: u64) -> bool {
        self.by_stage.get(&stage).is_some_and(|topology| {
            !topology.dirty
                && topology.simulation_candidates_ready
                && topology.canonical_generation == Some(generation)
        })
    }

    fn has_committed(&self, stage: bevy::asset::AssetId<UsdStageAsset>) -> bool {
        self.by_stage.get(&stage).is_some_and(|topology| {
            topology.simulation_candidates_ready && topology.canonical_generation.is_some()
        })
    }

    fn commit_prepared(
        &mut self,
        stage: bevy::asset::AssetId<UsdStageAsset>,
        generation: u64,
        mut topology: StageJointTopology,
    ) {
        topology.canonical_generation = Some(generation);
        topology.dirty = false;
        self.by_stage.insert(stage, topology);
        self.refresh_pending.remove(&stage);
    }

    fn invalidate_stage(&mut self, stage: bevy::asset::AssetId<UsdStageAsset>) {
        self.by_stage.remove(&stage);
        self.refresh_pending.remove(&stage);
    }

    fn mark_stale(&mut self, stage: bevy::asset::AssetId<UsdStageAsset>) {
        self.by_stage.entry(stage).or_default().dirty = true;
        self.refresh_pending.insert(stage);
    }

    fn request_refresh(&mut self, stage: bevy::asset::AssetId<UsdStageAsset>) {
        self.refresh_pending.insert(stage);
    }

    fn stop_refresh(&mut self, stage: bevy::asset::AssetId<UsdStageAsset>) {
        self.refresh_pending.remove(&stage);
    }

    fn get(&self, stage: bevy::asset::AssetId<UsdStageAsset>) -> Option<&StageJointTopology> {
        self.by_stage.get(&stage)
    }
}

fn track_joint_topology_changes(
    mut changes: MessageReader<UsdSceneChangeBatch>,
    mut topology: ResMut<JointTopologyIndex>,
    mut tasks: ResMut<PreparedJointTopologyTasks>,
    mut admission: ResMut<lunco_core_runtime::AsyncWorkAdmission>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
) {
    for change in changes.read() {
        if topology.get(change.stage_id).is_none() {
            continue;
        }
        let Some(stage_asset) = stages.get(change.stage_id) else {
            continue;
        };
        let (reader, _) = canonical.reader_for(change.stage_id, stage_asset);
        topology.observe_scene_change(change, &reader);
        if tasks
            .pending
            .get(&change.stage_id)
            .is_some_and(|pending| pending.source.generation() != change.stage_generation)
        {
            retire_joint_topology_task(change.stage_id, &mut tasks, &mut admission);
        }
    }
}

fn invalidate_joint_topology_on_stage_asset_event(
    mut events: MessageReader<AssetEvent<UsdStageAsset>>,
    mut topology: ResMut<JointTopologyIndex>,
    mut tasks: ResMut<PreparedJointTopologyTasks>,
    mut admission: ResMut<lunco_core_runtime::AsyncWorkAdmission>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let mut failures_changed = false;
    for event in events.read() {
        let (stage, invalidate) = match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => (*id, false),
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => (*id, true),
        };
        if invalidate {
            topology.invalidate_stage(stage);
        } else {
            // Keep the last committed facts available to already-admitted
            // simulation while the replacement generation is prepared.
            topology.mark_stale(stage);
        }
        retire_joint_topology_task(stage, &mut tasks, &mut admission);
        failures_changed |= tasks.failed.remove(&stage).is_some();
    }
    if failures_changed && let Some(diagnostics) = diagnostics.as_deref_mut() {
        publish_topology_preparation_diagnostics(&tasks, diagnostics);
    }
}

fn reset_joint_topology_state(
    mut topology: ResMut<JointTopologyIndex>,
    mut tasks: ResMut<PreparedJointTopologyTasks>,
    mut admission: ResMut<lunco_core_runtime::AsyncWorkAdmission>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    mut progress: Option<ResMut<lunco_core_runtime::SimulationProgress>>,
) {
    topology.by_stage.clear();
    topology.refresh_pending.clear();
    for task in tasks.pending.values() {
        if let Some(key) = task.work_key {
            admission.cancel_queued(key);
        }
    }
    tasks.pending.clear();
    tasks.failed.clear();
    tasks.capacity_wait_revision = None;
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.replace_producer("usd-sim-topology", std::iter::empty());
    }
    if let Some(progress) = progress.as_deref_mut() {
        release_usd_simulation_topology_progress(progress, None);
    }
}

fn retire_joint_topology_task(
    stage: bevy::asset::AssetId<UsdStageAsset>,
    tasks: &mut PreparedJointTopologyTasks,
    admission: &mut lunco_core_runtime::AsyncWorkAdmission,
) {
    let mut capacity_changed = false;
    if let Some(task) = tasks.pending.remove(&stage)
        && let Some(key) = task.work_key
    {
        capacity_changed = admission.cancel_queued(key);
    }
    if capacity_changed {
        tasks.capacity_wait_revision = None;
    }
}

fn release_usd_simulation_topology_progress(
    progress: &mut lunco_core_runtime::SimulationProgress,
    keep: Option<lunco_core_runtime::SimulationProgressKey>,
) {
    let stale: Vec<_> = progress
        .blockers()
        .filter(|blocker| {
            blocker.key.owner == lunco_core_runtime::SimulationProgressOwner::UsdSimulationTopology
                && Some(blocker.key) != keep
        })
        .map(|blocker| blocker.key)
        .collect();
    for key in stale {
        progress.release(key);
    }
}

fn sync_joint_topology_progress(
    mount: Option<Res<lunco_core::SceneMountState>>,
    primary_roots: Query<&UsdPrimPath, With<UsdSceneRoot>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut topology: ResMut<JointTopologyIndex>,
    tasks: Res<PreparedJointTopologyTasks>,
    mut progress: ResMut<lunco_core_runtime::SimulationProgress>,
    mut previous: Local<Option<(lunco_core_runtime::SimulationProgressKey, &'static str)>>,
) {
    let desired = mount
        .as_deref()
        .and_then(lunco_core::SceneMountState::active_root)
        .and_then(|root| {
            let primary = primary_roots.get(root).ok()?;
            let stage = primary.stage_handle.id();
            let generation = canonical.generation_for(stage);
            let stage_is_loaded = stages.get(&primary.stage_handle).is_some();
            if stage_is_loaded {
                if topology.is_current(stage, generation) {
                    if topology.refresh_pending.contains(&stage) {
                        topology.stop_refresh(stage);
                    }
                    return None;
                }
                if topology.has_committed(stage) {
                    if !topology.refresh_pending.contains(&stage)
                        && !tasks.failed_for_generation(stage, generation)
                    {
                        topology.request_refresh(stage);
                    }
                    return None;
                }
                if tasks.failed_for_generation(stage, generation) {
                    if topology.refresh_pending.contains(&stage) {
                        topology.stop_refresh(stage);
                    }
                } else if !topology.refresh_pending.contains(&stage) {
                    topology.request_refresh(stage);
                }
            } else if !topology.has_committed(stage) {
                if topology.refresh_pending.contains(&stage) {
                    topology.stop_refresh(stage);
                }
            } else {
                return None;
            }
            Some((
                lunco_core_runtime::SimulationProgressKey::usd_simulation_topology(root),
                if !stage_is_loaded {
                    "Waiting for the mounted USD stage needed by simulation topology"
                } else if tasks.failed_for_generation(stage, generation) {
                    "USD simulation topology preparation failed; scene simulation is held"
                } else {
                    "Preparing current USD simulation topology"
                },
            ))
        });
    if *previous != desired || desired.is_some_and(|(key, _)| !progress.contains(key)) {
        if let Some((previous_key, _)) = *previous
            && desired.is_none_or(|(key, _)| key != previous_key)
        {
            progress.release(previous_key);
        }
        if let Some((key, reason)) = desired {
            if progress.contains(key) {
                progress.update_reason(key, reason);
            } else {
                progress.acquire(key, reason);
            }
        }
        *previous = desired;
    }
}

fn submit_joint_topology_preparation(
    stage: bevy::asset::AssetId<UsdStageAsset>,
    source: PreparedJointTopologySource,
    generation: u64,
    priority: lunco_core_runtime::AsyncWorkPriority,
    order: u64,
    work: impl FnOnce() -> Result<StageJointTopology, String> + Send + 'static,
    tasks: &mut PreparedJointTopologyTasks,
    admission: &mut lunco_core_runtime::AsyncWorkAdmission,
) {
    if tasks.pending.contains_key(&stage)
        || tasks.failed_for_generation(stage, generation)
        || tasks.pending.len() >= MAX_PREPARED_JOINT_TOPOLOGY_TASKS
        || tasks.capacity_wait_revision.is_some()
    {
        return;
    }
    let completion = Arc::new(Mutex::new(None));
    let worker_completion = Arc::clone(&completion);
    let job = move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
            .unwrap_or_else(|_| Err("USD simulation topology worker panicked".to_owned()));
        *worker_completion
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(result);
    };
    let capacity_revision = admission.capacity_revision();
    let work_key = tasks.allocate_work_key(generation);
    let mut admitted_key = None;
    if let Some(work_key) = work_key {
        match admission.submit_ordered(priority, work_key, order, job) {
            Ok(()) => admitted_key = Some(work_key),
            Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                tasks.capacity_wait_revision = Some(capacity_revision);
                return;
            }
            Err(rejection) => {
                let detail = match rejection {
                    lunco_core_runtime::AsyncWorkRejection::DuplicateKey => {
                        "shared USD topology admission rejected a duplicate operation key"
                    }
                    lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable => {
                        "USD simulation topology preparation requires a native worker transport"
                    }
                    lunco_core_runtime::AsyncWorkRejection::QueueFull => unreachable!(),
                };
                *completion.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(Err(detail.to_owned()));
            }
        }
    } else {
        *completion.lock().unwrap_or_else(PoisonError::into_inner) = Some(Err(
            "USD simulation topology operation id exhausted".to_owned(),
        ));
    }
    tasks.pending.insert(
        stage,
        PreparedJointTopologyTask {
            source,
            work_key: admitted_key,
            completion,
        },
    );
}

fn try_take_topology_completion(
    completion: &Mutex<Option<Result<StageJointTopology, String>>>,
) -> Option<Result<StageJointTopology, String>> {
    match completion.try_lock() {
        Ok(mut completion) => completion.take(),
        Err(TryLockError::Poisoned(error)) => error.into_inner().take(),
        Err(TryLockError::WouldBlock) => None,
    }
}

fn build_joint_topology(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
) -> StageJointTopology {
    let mut topology = StageJointTopology::default();
    let candidates = collect_stage_candidate_paths(reader);
    collect_joint_scan_read(reader, &mut topology, &candidates);
    collect_simulation_candidate_paths(&candidates, &mut topology.simulation_candidates);
    topology.simulation_candidates_ready = true;
    topology
}

fn poll_prepared_joint_topology(
    tasks: &mut PreparedJointTopologyTasks,
    stages: &Assets<UsdStageAsset>,
    canonical: &CanonicalStages,
    topology_index: &mut JointTopologyIndex,
    primary_stages: &HashSet<bevy::asset::AssetId<UsdStageAsset>>,
    mut faults: Option<&mut lunco_core::RuntimeFaults>,
    mut holds: Option<&mut lunco_physics::PhysicsHolds>,
    mut diagnostics: Option<&mut lunco_core::RuntimeDiagnostics>,
) {
    let mut completed = Vec::new();
    for (stage, pending) in &tasks.pending {
        if let Some(result) = try_take_topology_completion(&pending.completion) {
            completed.push((*stage, pending.source.clone(), result));
        }
    }

    let mut failures_changed = false;
    for (stage, source, result) in completed {
        tasks.pending.remove(&stage);
        let Some(stage_asset) = stages.get(stage) else {
            continue;
        };
        if !source.is_current(stage, stage_asset, canonical) {
            continue;
        }

        let generation = source.generation();
        if topology_index.is_current(stage, generation) {
            continue;
        }
        match result {
            Ok(prepared) => topology_index.commit_prepared(stage, generation, prepared),
            Err(error) => {
                if tasks
                    .failed
                    .insert(
                        stage,
                        FailedJointTopologyPreparation {
                            generation,
                            error: error.clone(),
                        },
                    )
                    .is_none_or(|previous| previous.generation != generation)
                {
                    failures_changed = true;
                    if primary_stages.contains(&stage) {
                        if let Some(faults) = faults.as_deref_mut() {
                            faults.raise(
                                "usd-sim-topology-preparation",
                                None,
                                format!("{stage:?}"),
                                error,
                            );
                        }
                        if let Some(holds) = holds.as_deref_mut() {
                            holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                        }
                    }
                }
                topology_index.stop_refresh(stage);
            }
        }
    }
    if failures_changed && let Some(diagnostics) = diagnostics.as_deref_mut() {
        publish_topology_preparation_diagnostics(tasks, diagnostics);
    }
}

fn publish_topology_preparation_diagnostics(
    tasks: &PreparedJointTopologyTasks,
    diagnostics: &mut lunco_core::RuntimeDiagnostics,
) {
    diagnostics.replace_producer(
        "usd-sim-topology",
        tasks
            .failed
            .iter()
            .map(|(stage, failure)| lunco_core::RuntimeDiagnostic {
                code: "usd-sim-topology-preparation".to_owned(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "usd-sim-topology".to_owned(),
                subject: format!("{stage:?}"),
                message: failure.error.clone(),
            }),
    );
}

/// Retire authored cameras at the shared scene-teardown boundary. The scene
/// entity despawn and the render-world extraction are not the same instant; a
/// camera left active until the subtree is flushed can render alongside the
/// replacement avatar during RestartScene.
fn retire_scene_cameras(
    mut cameras: Query<(&mut bevy::camera::Camera, Entity), (With<SceneCamera>, With<UsdPrimPath>)>,
    mut commands: Commands,
) {
    for (mut camera, entity) in &mut cameras {
        camera.is_active = false;
        commands.entity(entity).try_remove::<SceneCamera>();
    }
}

/// Reset scene-faulted simulation state before the outgoing entities are
/// reclaimed. A terminal fault deliberately stops physics for the bad scene,
/// but it must not become a process-wide lock that prevents the next tutorial
/// or scenario from loading. The fault and its safety hold have the same scene
/// ownership, so they are cleared together at the one teardown boundary.
fn reset_scene_runtime_safety(
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
) {
    if let Some(faults) = faults.as_deref_mut() {
        if faults.active() {
            info!("[scene] clearing terminal runtime fault for replacement scene");
            faults.clear();
        }
    }
    if let Some(holds) = holds.as_deref_mut() {
        holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, false);
    }
}

#[cfg(test)]
mod runtime_safety_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[derive(Resource, Debug, PartialEq, Eq)]
    struct LoadedScene(&'static str);

    #[test]
    fn scene_teardown_clears_only_scene_terminal_safety_state() {
        let mut world = World::new();
        let mut faults = lunco_core::RuntimeFaults::default();
        faults.raise("physics-body-escaped", None, "rover", "out of bounds");
        world.insert_resource(faults);

        let mut holds = lunco_physics::PhysicsHolds::default();
        holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
        holds.set(lunco_physics::PhysicsHolds::TERRAIN_READY, true);
        world.insert_resource(holds);

        world.run_system_once(reset_scene_runtime_safety).unwrap();

        assert!(!world.resource::<lunco_core::RuntimeFaults>().active());
        let holds = world.resource::<lunco_physics::PhysicsHolds>();
        assert!(!holds.holds(lunco_physics::PhysicsHolds::SAFETY_FAILURE));
        assert!(holds.holds(lunco_physics::PhysicsHolds::TERRAIN_READY));
    }

    #[test]
    fn fault_then_scene_reload_can_admit_a_replacement_runtime() {
        let mut app = App::new();
        app.init_resource::<lunco_core::RuntimeFaults>();
        app.init_resource::<lunco_physics::PhysicsHolds>();
        app.insert_resource(LoadedScene("escape-containment"));
        app.add_systems(lunco_core::SceneTeardown, reset_scene_runtime_safety);

        app.world_mut()
            .resource_mut::<lunco_core::RuntimeFaults>()
            .raise("physics-body-escaped", None, "escapee", "out of bounds");
        app.world_mut()
            .resource_mut::<lunco_physics::PhysicsHolds>()
            .set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);

        // This is the same lifecycle edge used by LoadScene/ClearScene. The
        // replacement is deliberately admitted only after the edge, proving a
        // terminal fault is scoped to the outgoing scene rather than latched in
        // the process.
        lunco_core::run_scene_teardown(app.world_mut());
        assert!(!app.world().resource::<lunco_core::RuntimeFaults>().active());
        assert!(
            !app.world()
                .resource::<lunco_physics::PhysicsHolds>()
                .holds(lunco_physics::PhysicsHolds::SAFETY_FAILURE)
        );

        app.insert_resource(LoadedScene("replacement"));
        assert_eq!(
            app.world().resource::<LoadedScene>(),
            &LoadedScene("replacement")
        );
        // A later scene can still raise its own fault and be torn down again;
        // the first scene's record is not reused as a process-wide lock.
        app.world_mut()
            .resource_mut::<lunco_core::RuntimeFaults>()
            .raise("physics-body-escaped", None, "replacement", "out of bounds");
        assert!(app.world().resource::<lunco_core::RuntimeFaults>().active());
        lunco_core::run_scene_teardown(app.world_mut());
        assert!(!app.world().resource::<lunco_core::RuntimeFaults>().active());
    }
}

impl Plugin for UsdSimPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_core_runtime::AsyncWorkAdmissionPlugin>() {
            app.add_plugins(lunco_core_runtime::AsyncWorkAdmissionPlugin);
        }
        app.init_resource::<UsdLiveEditRegistry>();
        app.world_mut()
            .resource_mut::<UsdLiveEditRegistry>()
            .register(UsdLiveEditOwner::new(
                "usd-sim.wheels",
                wheel_runtime::claims_edit,
                invalidate_usd_sim_projection,
                wheel_runtime::resync_wheels_for_stage,
            ));
        if !app.is_plugin_added::<lunco_embodiment_core::roles::EmbodimentCorePlugin>() {
            app.add_plugins(lunco_embodiment_core::roles::EmbodimentCorePlugin);
        }
        app.init_resource::<lunco_core::RuntimeFaults>();
        app.init_resource::<lunco_core::RuntimeDiagnostics>();
        app.configure_sets(
            Update,
            (
                UsdSimSet::ProjectionPrepare.before(UsdSimSet::Projection),
                UsdSimSet::Projection.before(lunco_spatial::SceneSpatialHandoffSet),
                lunco_usd_avian_joints::JointAdmission.after(UsdSimSet::Projection),
                UsdSimSet::ActivateDynamicBodies,
            ),
        )
        .configure_sets(PreUpdate, UsdSimSet::ActivateDynamicBodies);
        app.add_systems(lunco_core::SceneTeardown, reset_scene_runtime_safety);
        app.add_systems(lunco_core::SceneTeardown, retire_scene_cameras);
        app.init_resource::<PendingUsdSimPrimWork>();
        app.add_systems(lunco_core::SceneTeardown, reset_usd_sim_prim_work);
        app.add_systems(lunco_core::SceneTeardown, reset_joint_topology_state);
        // Client-only: reconstruct a remote rover's wheels from its chassis
        // (kinematic followers — wheels are no longer replicated), then re-derive
        // the cosmetic visual roll. Chained so the visual spin layers on the
        // freshly-placed body. Same `relative_speed > 0` gate as raycast wheels.
        app.add_systems(
            FixedUpdate,
            (reconstruct_proxy_wheels, animate_proxy_physical_wheels)
                .chain()
                .run_if(|t: Res<Time<Virtual>>| !t.is_paused() && t.relative_speed_f64() > 0.0),
        )
        .add_observer(queue_added_usd_sim_prim)
        .add_observer(queue_projected_usd_sim_prim)
        .add_observer(forget_removed_usd_sim_prim)
        .add_observer(forget_unprojected_usd_sim_prim)
        .add_observer(queue_invalidated_usd_sim_prim)
        .add_systems(PreUpdate, resolve_differential_coupling)
        .init_resource::<GroundColliderPending>()
        .init_resource::<JointTopologyIndex>()
        .init_resource::<PreparedJointTopologyTasks>()
        .init_resource::<lunco_core_runtime::SimulationProgress>()
        .add_systems(
            PreUpdate,
            (
                invalidate_joint_topology_on_stage_asset_event,
                track_joint_topology_changes,
            )
                .chain()
                .after(lunco_core::RuntimeCycleSet::Lifecycle),
        )
        .add_systems(
            PreUpdate,
            sync_joint_topology_progress
                .after(track_joint_topology_changes)
                .before(lunco_core_runtime::SimulationProgressAdmissionSet),
        )
        .add_systems(
            Update,
            (process_usd_sim_prims
                .run_if(any_pending_usd_sim)
                .after(lunco_usd_bevy_scene::UsdVisualProjectionSet),)
                .in_set(UsdSimSet::Projection),
        );
        // Dynamic admission must happen before the fixed loop. The body remains
        // kinematic until the initialization policy has accepted its composed
        // authored state; no terrain system can move it across this boundary.
        app.add_systems(
            PreUpdate,
            activate_dynamic_bodies
                .in_set(UsdSimSet::ActivateDynamicBodies)
                .before(lunco_physics::apply_physics_holds)
                .run_if(any_with_component::<ShouldBeDynamic>),
        );
        // Bodies and constraints can become admissible on different updates
        // after asynchronous scene projection. Do not let early bodies enter
        // the solver while another part of that authored admission set is
        // still pending; the hold affects physics only, so preparation, UI,
        // telemetry, and authored readiness clocks continue to run.
        app.add_systems(
            PreUpdate,
            sync_physics_body_admission_hold
                .after(activate_dynamic_bodies)
                .before(lunco_physics::apply_physics_holds),
        );
        // Screen-constant markers. `PostUpdate` before transform propagation:
        // the scale is a function of the camera's position THIS frame, and the
        // markers sit on other bodies' grids, which `place_celestial_bound_entities`
        // may have just re-parented.
        app.add_systems(
            PostUpdate,
            marker::scale_screen_constant_markers.before(TransformSystems::Propagate),
        );
        // The authored light's `Transform` is installed during Update, while
        // its composed world rotation is produced by Bevy/big_space transform
        // propagation. Read that world fact only after propagation; sampling it
        // in Update sees the default identity GlobalTransform for a newly
        // admitted light and would publish a horizontal semantic sun on the
        // following frame.
        install_authored_sun_state_seed(app);
    }
}

/// USD-authored screen-constant markers (`lunco:marker:*`) — geometry that
/// subtends a fixed angle so a physically sub-pixel thing still reads on screen.
pub mod marker;
/// Process USD prims for sim mapping AFTER their assets are loaded.
///
/// This is the core system that maps USD schemas to LunCoSim components. It runs in the
/// `Update` schedule **after** `sync_usd_visuals` to ensure meshes and transforms exist.
///
/// # What It Does
///
/// 1. **Detects `PhysxVehicleContextAPI`** → Creates a `MobilityRoot` and an
///    `OutputPorts` surface from the vehicle root's authored numeric `outputs:*`
///    attributes.
/// 2. **Detects vehicle metadata schemas** → leaves their motion law to the
///    authored Modelica/Rhai controller network.
/// 3. **Detects `PhysxVehicleWheelAPI`** → Sets up wheel based on whether an authored
///    `PhysicsRevoluteJoint` targets the wheel:
///    - **Joint-based** (joint authored): `RigidBody`, `Collider`, `JointTorqueActuator` (constraint built by `lunco-usd-avian`; torque/speed come from the authored Modelica network)
///    - **Raycast** (no joint): `WheelRaycast`, `RayCaster` (entity split into physics + visual child)
///
/// The observer-fed set makes settled-scene admission constant-time. The
/// bootstrap flag covers entities that predate plugin installation.
fn any_pending_usd_sim(
    pending: Res<PendingUsdSimPrimWork>,
    topology: Res<JointTopologyIndex>,
) -> bool {
    pending.0.has_work() || !topology.refresh_pending.is_empty()
}

fn process_usd_sim_prims(
    mut commands: Commands,
    // Appearance INTENT, not materials: the wheel split MOVES the `PbrLook` /
    // `ShaderLook` onto the visual child and `lunco-render-bevy` rebinds. Neither
    // component names `bevy_pbr`.
    query: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&Transform>,
            Option<&Mesh3d>,
            Option<&PbrLook>,
            Option<&ShaderLook>,
            Option<&UsdInstanceProjection>,
            Has<UsdSceneGeometryPending>,
            Has<lunco_usd_bevy_scene::UsdVisualShaderBound>,
        ),
        (
            With<lunco_usd_bevy_scene::UsdSceneProjected>,
            Without<UsdSimProcessed>,
        ),
    >,
    mut pending: ResMut<PendingUsdSimPrimWork>,
    all_prims: Query<(
        Entity,
        &UsdPrimPath,
        Option<&Transform>,
        Option<&UsdInstanceProjection>,
    )>,
    grid_components: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_child_of: Query<&ChildOf>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    primary_roots: Query<&UsdPrimPath, With<UsdSceneRoot>>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    stage_identity: UsdStageIdentityParams,
    // Initial reads use the worker-produced plan; later authored generations
    // use the live canonical stage selected by the shared reader boundary.
    canonical: NonSend<CanonicalStages>,
    topology_work: JointTopologyPreparationParams,
    mut runtime_faults: ResMut<lunco_core::RuntimeFaults>,
    mut physics_holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut runtime_diagnostics: ResMut<lunco_core::RuntimeDiagnostics>,
) {
    let JointTopologyPreparationParams {
        index: mut topology_index,
        tasks: mut topology_tasks,
        admission: mut async_work,
    } = topology_work;
    let UsdStageIdentityParams {
        stages,
        asset_server,
    } = stage_identity;
    let started = web_time::Instant::now();
    let mut processed = 0usize;
    let mut authored_diagnostics = Vec::new();
    let mut topology_failures_changed = false;
    let primary_stages: HashSet<_> = mount
        .as_deref()
        .and_then(lunco_core::SceneMountState::active_root)
        .and_then(|root| primary_roots.get(root).ok())
        .map(|prim| prim.stage_handle.id())
        .into_iter()
        .collect();
    let unprocessed = {
        let _span = bevy::log::info_span!("usd_sim_pending_collect_sort").entered();
        let mut entities = pending.0.take_queued().into_iter().collect::<Vec<_>>();
        if pending.0.take_initial_discovery() {
            // One bootstrap query covers prims that existed before this projector
            // was installed. Normal arrivals are supplied by lifecycle observers.
            entities.extend(query.iter().map(|(entity, ..)| entity));
        }
        entities.sort_unstable();
        entities.dedup();
        let mut candidates = Vec::new();
        let mut unidentified_prims = Vec::new();
        for entity in entities {
            let Ok(item) = query.get(entity) else {
                continue;
            };
            let stage_id = item.1.stage_handle.id();
            let Some(stage_asset) = stages.get(stage_id) else {
                pending.0.queue(entity);
                continue;
            };
            match stable_stage_source(stage_id, stage_asset, &asset_server) {
                Ok(stage_source) => candidates.push(StableUsdSimWork {
                    stage_source,
                    prim_path: item.1.path.clone(),
                    item,
                }),
                Err(error) => {
                    pending.0.queue(entity);
                    unidentified_prims.push((item.1.path.clone(), error));
                }
            }
        }
        unidentified_prims.sort();
        unidentified_prims.dedup();
        for (prim_path, error) in unidentified_prims {
            let message = format!(
                "USD simulation cannot order prim `{prim_path}` because its stage has no stable source identity: {error}"
            );
            runtime_faults.raise(
                "usd-sim-stage-identity",
                None,
                prim_path.clone(),
                message.clone(),
            );
            push_usd_sim_diagnostic(
                &mut authored_diagnostics,
                &prim_path,
                "stage-identity",
                message,
            );
        }
        let mut preview_cache = HashMap::new();
        let duplicate_keys = duplicate_nonpreview_usd_sim_work_keys(
            &candidates,
            |candidate| candidate.0,
            |entity| is_preview_only(entity, &q_child_of, &q_preview_only),
            &mut preview_cache,
        );
        if !duplicate_keys.is_empty() {
            for candidate in &candidates {
                let key = (candidate.stage_source.clone(), candidate.prim_path.clone());
                if duplicate_keys.contains(&key)
                    && !preview_cache
                        .get(&candidate.item.0)
                        .copied()
                        .unwrap_or(false)
                {
                    pending.0.queue(candidate.item.0);
                }
            }
            let mut ordered_duplicate_keys = duplicate_keys.iter().cloned().collect::<Vec<_>>();
            ordered_duplicate_keys.sort();
            for (stage_source, prim_path) in &ordered_duplicate_keys {
                let message = format!(
                    "USD stage `{stage_source}` projects multiple entities for `{prim_path}`; simulation admission requires one stable stage/path identity"
                );
                let mut fault_identity = String::new();
                append_order_segment(&mut fault_identity, stage_source);
                append_order_segment(&mut fault_identity, prim_path);
                runtime_faults.raise(
                    "usd-sim-duplicate-order-identity",
                    None,
                    fault_identity,
                    message.clone(),
                );
                push_usd_sim_diagnostic(
                    &mut authored_diagnostics,
                    prim_path,
                    "duplicate-stage-path",
                    message,
                );
            }
            candidates.retain(|candidate| {
                let key = (candidate.stage_source.clone(), candidate.prim_path.clone());
                !duplicate_keys.contains(&key)
                    || preview_cache
                        .get(&candidate.item.0)
                        .copied()
                        .unwrap_or(false)
            });
        }
        let (unprocessed, deferred, previews) = select_bounded_sim_prim_work(
            candidates,
            MAX_USD_SIM_PRIM_PROJECTIONS_PER_UPDATE,
            |candidate| candidate.item.0,
            compare_stable_usd_sim_work,
            |entity| {
                preview_cache
                    .get(&entity)
                    .copied()
                    .unwrap_or_else(|| is_preview_only(entity, &q_child_of, &q_preview_only))
            },
        );
        pending.0.extend(deferred);
        // Preview prims have no simulation owner and need no stage topology.
        for entity in previews {
            commands.entity(entity).try_insert(UsdSimProcessed);
        }
        unprocessed
    };

    // Initial topology uses the immutable asset plan. For later generations,
    // topology is prepared from a recipe snapshot of the exact canonical stage
    // through shared bounded admission; live OpenUSD traversal stays off Update.
    {
        let _span = bevy::log::info_span!("usd_sim_topology_refresh").entered();
        if topology_tasks
            .capacity_wait_revision
            .is_some_and(|revision| revision != async_work.capacity_revision())
        {
            topology_tasks.capacity_wait_revision = None;
        }
        let mut seen_stages = topology_index.refresh_pending.clone();
        let mut stage_ids: Vec<_> = seen_stages.iter().copied().collect();
        for candidate in &unprocessed {
            let prim_path = &candidate.item.1;
            let id = prim_path.stage_handle.id();
            if seen_stages.insert(id) {
                stage_ids.push(id);
            }
        }
        let mut ordered_stages = Vec::new();
        let mut unidentified_stages = Vec::new();
        for id in stage_ids {
            let Some(stage_asset) = stages.get(id) else {
                continue;
            };
            match stable_stage_source(id, stage_asset, &asset_server) {
                Ok(source) => ordered_stages.push((!primary_stages.contains(&id), source, id)),
                Err(error) => unidentified_stages.push(error),
            }
        }
        unidentified_stages.sort();
        unidentified_stages.dedup();
        for error in unidentified_stages {
            let message = format!(
                "USD simulation cannot order one or more stages because they have no stable source identity: {error}"
            );
            runtime_faults.raise(
                "usd-sim-stage-identity",
                None,
                error.clone(),
                message.clone(),
            );
            push_usd_sim_diagnostic(
                &mut authored_diagnostics,
                "usd-stage-identity",
                "stage-identity",
                message,
            );
        }
        ordered_stages.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
        let mut source_counts = HashMap::new();
        for (_, source, _) in &ordered_stages {
            *source_counts.entry(source.clone()).or_insert(0usize) += 1;
        }
        let mut duplicate_sources: Vec<_> = source_counts
            .into_iter()
            .filter_map(|(source, count)| (count > 1).then_some(source))
            .collect();
        duplicate_sources.sort();
        if !duplicate_sources.is_empty() {
            for source in &duplicate_sources {
                let message = format!(
                    "multiple loaded USD stages share stable root identifier `{source}`; simulation admission requires a unique stage source"
                );
                runtime_faults.raise(
                    "usd-sim-duplicate-stage-identity",
                    None,
                    source.clone(),
                    message.clone(),
                );
                push_usd_sim_diagnostic(
                    &mut authored_diagnostics,
                    source,
                    "duplicate-stage-identity",
                    message,
                );
            }
            ordered_stages
                .retain(|(_, source, _)| duplicate_sources.binary_search(source).is_err());
        }
        let stage_ids = ordered_stages
            .iter()
            .map(|(_, _, id)| *id)
            .collect::<Vec<_>>();
        for (stage_rank, (_, _, id)) in ordered_stages.iter().enumerate() {
            let id = *id;
            let Some(stage_asset) = stages.get(id) else {
                continue;
            };
            let generation = canonical.generation_for(id);
            if topology_tasks
                .failed
                .get(&id)
                .is_some_and(|failure| failure.generation != generation)
            {
                topology_tasks.failed.remove(&id);
                topology_failures_changed = true;
            }
            if topology_index.is_current(id, generation)
                || topology_tasks.pending.contains_key(&id)
                || topology_tasks.failed_for_generation(id, generation)
                || topology_tasks.pending.len() >= MAX_PREPARED_JOINT_TOPOLOGY_TASKS
                || topology_tasks.capacity_wait_revision.is_some()
            {
                continue;
            }
            let priority = if primary_stages.contains(&id) {
                lunco_core_runtime::AsyncWorkPriority::SimulationRequired
            } else {
                lunco_core_runtime::AsyncWorkPriority::Interactive
            };
            if generation > 0
                && !canonical.prepared_plan_is_current(id, &stage_asset.projection_plan)
            {
                let source = PreparedJointTopologySource::CanonicalSnapshot {
                    asset_plan: stage_asset.projection_plan.clone(),
                    generation,
                };
                let snapshot = if let Some(stage) = canonical.get(id) {
                    {
                        let _span = bevy::log::info_span!(
                            "usd_sim_canonical_topology_snapshot",
                            generation
                        )
                        .entered();
                        stage.recipe_snapshot().map_err(|error| error.to_string())
                    }
                } else {
                    Err(
                        "canonical stage is unavailable for its nonzero topology generation"
                            .to_owned(),
                    )
                };
                submit_joint_topology_preparation(
                    id,
                    source,
                    generation,
                    priority,
                    stage_rank as u64,
                    move || {
                        let _span =
                            bevy::log::info_span!("usd_sim_canonical_topology_prepare", generation)
                                .entered();
                        let snapshot = snapshot.map_err(|error| {
                            format!("cannot snapshot current canonical stage layers: {error}")
                        })?;
                        let plan = snapshot.prepare_projection_plan().map_err(|error| {
                            format!("cannot prepare current canonical stage facts: {error}")
                        })?;
                        Ok(build_joint_topology(&plan))
                    },
                    &mut topology_tasks,
                    &mut async_work,
                );
            } else {
                let plan = stage_asset.projection_plan.clone();
                submit_joint_topology_preparation(
                    id,
                    PreparedJointTopologySource::AssetPlan {
                        plan: plan.clone(),
                        generation,
                    },
                    generation,
                    priority,
                    stage_rank as u64,
                    move || Ok(build_joint_topology(plan.as_ref())),
                    &mut topology_tasks,
                    &mut async_work,
                );
            }
        }
        if topology_failures_changed {
            publish_topology_preparation_diagnostics(&topology_tasks, &mut runtime_diagnostics);
        }
        poll_prepared_joint_topology(
            &mut topology_tasks,
            &stages,
            &canonical,
            &mut topology_index,
            &primary_stages,
            Some(&mut runtime_faults),
            physics_holds.as_deref_mut(),
            Some(&mut runtime_diagnostics),
        );
        if unprocessed.iter().any(|candidate| {
            let stage = candidate.item.1.stage_handle.id();
            !stage_ids.contains(&stage)
                || !topology_index.is_current(stage, canonical.generation_for(stage))
        }) {
            for candidate in &unprocessed {
                let entity = candidate.item.0;
                let stage = candidate.item.1.stage_handle.id();
                if !topology_tasks.failed_for_generation(stage, canonical.generation_for(stage)) {
                    pending.0.queue(entity);
                }
            }
            return;
        }
    }

    // --- Pass 2: Process all prims ---
    // Query order and Bevy entity ids are allocation details. Order simulation
    // admission by the stage's stable logical root identifier and authored prim
    // path so identical paths in separate loaded stages do not race for the
    // bounded prefix.
    let projection_prim_count = unprocessed.len();
    let _projection_span = bevy::log::info_span!(
        "usd_sim_prim_projection_batch",
        prim_count = projection_prim_count
    )
    .entered();
    for candidate in unprocessed {
        let stage_source = candidate.stage_source;
        let (
            entity,
            prim_path,
            maybe_tf,
            maybe_mesh,
            maybe_mat,
            maybe_shader_mat,
            instance_projection,
            mesh_pending,
            shader_bound,
        ) = candidate.item;
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
            let message = format!(
                "USD simulation prim has an invalid path `{}`",
                prim_path.path
            );
            push_usd_sim_diagnostic(
                &mut authored_diagnostics,
                &prim_path.path,
                "prim-path",
                message.clone(),
            );
            warn!("[usd-sim] {message}");
            commands.entity(entity).try_insert(UsdSimProcessed);
            continue;
        };

        let id = prim_path.stage_handle.id();
        let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
            pending.0.queue(entity);
            continue;
        };
        let (reader, _generation) =
            canonical.reader_for_entity(id, stage_asset, instance_projection);
        let Some(topology) = topology_index.get(id) else {
            pending.0.queue(entity);
            continue;
        };
        if instance_projection.is_none()
            && !topology.simulation_candidates.contains(&prim_path.path)
        {
            // Keep readiness accounting complete without running USD readers
            // for a prim whose schemas and authored properties have no sim owner.
            commands.entity(entity).try_insert(UsdSimProcessed);
            processed += 1;
            continue;
        }
        let _span = bevy::log::info_span!(
            "usd_sim_prim_projection",
            prim_path = %prim_path.path
        )
        .entered();
        process_usd_sim_prim_read(
            &reader,
            entity,
            prim_path,
            sdf_path.clone(),
            maybe_tf,
            maybe_mesh,
            maybe_mat,
            maybe_shader_mat,
            mesh_pending,
            shader_bound,
            topology,
            &all_prims,
            &q_child_of,
            &q_preview_only,
            instance_projection,
            &stage_source,
            &grid_components,
            &q_spatial,
            &mut commands,
            &mut authored_diagnostics,
        );
        processed += 1;
    }
    pending.1.extend(authored_diagnostics);
    if !pending.0.has_work() {
        runtime_diagnostics.replace_producer("usd-sim", std::mem::take(&mut pending.1));
    }
    if processed > 0 {
        bevy::log::debug!(
            "[usd-sim] processed {processed} prim(s) in {:.2} ms",
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
}

const JOINT_TOPOLOGY_TYPES: &[&str] = &[
    "PhysicsFixedJoint",
    "PhysicsRevoluteJoint",
    "PhysicsPrismaticJoint",
    "PhysicsSphericalJoint",
    "PhysicsDistanceJoint",
];
const WHEEL_ATTACHMENT_API: &str = "PhysxVehicleWheelAttachmentAPI";
const VEHICLE_CONTEXT_API: &str = "PhysxVehicleContextAPI";
const VEHICLE_WHEEL_API: &str = "PhysxVehicleWheelAPI";
const SIMULATION_CANDIDATE_TYPES: &[&str] = &["PhysxPhysicsGearJoint"];
const SIMULATION_CANDIDATE_APIS: &[&str] = &[
    "LunCoAvatarAPI",
    "LunCoForceActuatorAPI",
    "LunCoMassContributionAPI",
    "LunCoPhysicsInitializationAPI",
    "LunCoRaycastAPI",
    "LunCoSuspensionVisualAPI",
    "LunCoTorqueActuatorAPI",
    "PhysicsArticulationRootAPI",
    "PhysicsRigidBodyAPI",
    "PhysxVehicleContextAPI",
    "PhysxVehicleWheelAPI",
];

fn is_simulation_candidate(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> bool {
    reader
        .type_name(path)
        .is_some_and(|name| SIMULATION_CANDIDATE_TYPES.contains(&name.as_str()))
        || SIMULATION_CANDIDATE_APIS
            .iter()
            .any(|schema| reader.has_api_schema(path, schema))
        || reader.any_attr_with_prefix(path, "lunco:")
}

fn collect_stage_candidate_paths(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
) -> Vec<lunco_usd_bevy_stage::read::UsdReadPrimFacts> {
    let types = JOINT_TOPOLOGY_TYPES
        .iter()
        .chain(SIMULATION_CANDIDATE_TYPES)
        .copied()
        .collect::<Vec<_>>();
    let apis = std::iter::once(WHEEL_ATTACHMENT_API)
        .chain(std::iter::once(VEHICLE_CONTEXT_API))
        .chain(std::iter::once(VEHICLE_WHEEL_API))
        .chain(SIMULATION_CANDIDATE_APIS.iter().copied())
        .collect::<Vec<_>>();
    reader.prim_schema_facts_matching(&types, &apis, "lunco:")
}

fn collect_simulation_candidate_paths(
    stage_candidates: &[lunco_usd_bevy_stage::read::UsdReadPrimFacts],
    candidates: &mut HashSet<String>,
) {
    candidates.extend(
        stage_candidates
            .iter()
            .filter(|candidate| {
                candidate
                    .type_name
                    .as_deref()
                    .is_some_and(|name| SIMULATION_CANDIDATE_TYPES.contains(&name))
                    || candidate
                        .api_schemas
                        .iter()
                        .any(|name| SIMULATION_CANDIDATE_APIS.contains(&name.as_str()))
                    || candidate.has_attr_prefix
            })
            .map(|candidate| candidate.path.as_str().to_owned()),
    );
}

fn update_simulation_candidate(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &str,
    candidates: &mut HashSet<String>,
) {
    let Ok(path_value) = SdfPath::new(path) else {
        candidates.remove(path);
        return;
    };
    if is_simulation_candidate(reader, &path_value) {
        candidates.insert(path.to_owned());
    } else {
        candidates.remove(path);
    }
}

fn topology_change_affects(
    change: &UsdSceneChangeBatch,
    source_paths: &HashSet<String>,
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
) -> bool {
    change
        .resynced_prim_paths
        .iter()
        .any(|path| topology_change_affects_path(path, source_paths, reader))
        || change.info_prim_paths.iter().any(|path| {
            source_paths.contains(path)
                && !(change.transform_only_prim_paths.contains(path)
                    && !change.resynced_prim_paths.contains(path))
        })
}

fn topology_change_affects_path(
    path: &str,
    source_paths: &HashSet<String>,
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
) -> bool {
    if source_paths.contains(path) {
        return true;
    }
    let Ok(path) = SdfPath::new(path) else {
        return true;
    };
    reader
        .type_name(&path)
        .is_some_and(|name| JOINT_TOPOLOGY_TYPES.contains(&name.as_str()))
        || reader.has_api_schema(&path, WHEEL_ATTACHMENT_API)
        || reader.has_api_schema(&path, VEHICLE_CONTEXT_API)
}

fn collect_vehicle_output_ports(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    root: &SdfPath,
) -> (Vec<String>, HashSet<String>) {
    let root_attributes = reader.attr_names(root);
    let network_members = reader
        .collection_members(root, "components")
        .unwrap_or_default();
    let network_boundary = lunco_usd_bevy_core::program::ModelicaNetworkBoundaryIndex::new(
        root,
        &root_attributes,
        &network_members,
    );
    let mut source_paths = HashSet::from([root.as_str().to_owned()]);
    source_paths.extend(
        network_members
            .iter()
            .filter(|path| !path.is_property_path())
            .map(|path| path.as_str().to_owned()),
    );
    let mut port_names = Vec::new();
    for attr in &root_attributes {
        let Some(name) = attr.strip_prefix("outputs:") else {
            continue;
        };
        // NUMERIC outputs only. `outputs:` is UsdShade's namespace too, so a
        // vessel root that also carries a material network would otherwise
        // mint a phantom actuator port from `token outputs:surface`.
        if reader.real(root, attr).is_none()
            || network_boundary.is_network_boundary_output(reader, attr)
            || port_names.iter().any(|existing| existing == name)
        {
            continue;
        }
        port_names.push(name.to_owned());
    }
    (port_names, source_paths)
}

/// Per-stage joint scan (Pass 1), generic over the read source ([`UsdRead`]):
/// collects `PhysicsRevoluteJoint` `body1` targets (wheel dispatch) and the matching
/// `body0` targets (articulation roots) only when `body1` is a declared vehicle wheel.
/// Generic revolute mechanisms must not change a host's vehicle classification.
/// Also collects the canonical
/// `PhysxVehicleWheelAttachmentAPI` wheel→tire/suspension bindings (doc 53 §3.2).
/// Every relationship is required to resolve to at most one target. A USD
/// relationship is a list-op, so taking `rel_target` here would silently turn
/// malformed fan-out authoring into a first-target choice.
fn collect_joint_scan_read(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    topology: &mut StageJointTopology,
    candidates: &[lunco_usd_bevy_stage::read::UsdReadPrimFacts],
) {
    let _joint_span = bevy::log::info_span!("usd_sim_joint_topology_scan").entered();
    let vehicle_wheel_paths = candidates
        .iter()
        .filter(|candidate| {
            candidate
                .api_schemas
                .iter()
                .any(|schema| schema == VEHICLE_WHEEL_API)
        })
        .map(|candidate| candidate.path.as_str().to_owned())
        .collect::<HashSet<_>>();
    let mut attachment_paths = Vec::new();
    for candidate in candidates {
        let path = &candidate.path;
        let joint_type = candidate.type_name.as_deref();
        if candidate
            .api_schemas
            .iter()
            .any(|schema| schema == WHEEL_ATTACHMENT_API)
        {
            attachment_paths.push(path.clone());
        }
        if candidate
            .api_schemas
            .iter()
            .any(|schema| schema == VEHICLE_CONTEXT_API)
        {
            let (ports, source_paths) = collect_vehicle_output_ports(reader, path);
            topology
                .vehicle_output_ports
                .insert(path.as_str().to_owned(), ports);
            topology.source_paths.extend(source_paths);
        }
        let Some(joint_type) = joint_type.filter(|name| JOINT_TOPOLOGY_TYPES.contains(name)) else {
            continue;
        };
        topology.source_paths.insert(path.as_str().to_owned());
        let body0_target = reader.rel_target(path, "physics:body0");
        let body1_target = reader.rel_target(path, "physics:body1");
        let resolve_body = |target: Option<&String>| {
            target
                .and_then(|target| {
                    lunco_usd_avian_reader::joint::resolve_joint_body_path(reader, target)
                })
                .unwrap_or_default()
        };
        let body0 = resolve_body(body0_target.as_ref());
        let body1 = resolve_body(body1_target.as_ref());
        topology.source_paths.extend(
            body0_target
                .iter()
                .cloned()
                .chain(body1_target.iter().cloned()),
        );
        let is_physical_wheel_joint = joint_type == "PhysicsRevoluteJoint"
            && body1_target
                .as_deref()
                .is_some_and(|target| vehicle_wheel_paths.contains(target));
        debug!(
            "USD authored joint topology: {} -> ({}, {})",
            path.as_str(),
            body0,
            body1
        );
        // A physical wheel's authored revolute joint is the USD identity of
        // the wheel attachment, but the mobility projector owns the runtime
        // constraint: it creates the admitted wheel joint with the actuator
        // motor on the wheel entity's actual carrier mount. Keeping the
        // authored identity in the generic readiness set would wait forever,
        // because that synthesized joint intentionally has no USD prim path.
        if !is_physical_wheel_joint {
            topology
                .authored_joints
                .insert(path.as_str().to_string(), (body0, body1));
        } else if !body0.is_empty() {
            topology.physical_wheel_bodies.insert(body1, body0);
        }
        if joint_type == "PhysicsRevoluteJoint" {
            if let Some(body1) = body1_target {
                topology.source_paths.insert(body1.clone());
                debug!("USD joint dispatch: {} → wheel {}", path.as_str(), body1);
                let is_vehicle_wheel = vehicle_wheel_paths.contains(&body1);
                if is_vehicle_wheel {
                    topology
                        .joint_targets
                        .insert(body1, path.as_str().to_string());
                    if let Some(body0) = body0_target {
                        topology.articulation_roots.insert(body0);
                    }
                }
            }
        }
    }

    drop(_joint_span);
    let _attachment_span = bevy::log::info_span!("usd_sim_wheel_attachment_scan").entered();
    let attachments = lunco_usd_sim_authoring::collect_wheel_attachment_topology_from_paths(
        reader,
        attachment_paths,
    );
    topology
        .source_paths
        .extend(attachments.source_paths().cloned());
    topology
        .source_paths
        .extend(attachments.invalid_wheels().cloned());
    topology
        .invalid_wheel_attachments
        .extend(attachments.invalid_wheels().cloned());
    for (wheel, binding) in attachments.bindings() {
        debug!(
            "USD wheel attachment: wheel {} → tire {} / suspension {}",
            wheel, binding.tire, binding.suspension
        );
        topology
            .wheel_attachment_targets
            .insert(wheel.clone(), binding.suspension.clone());
        topology
            .wheel_attachment_tires
            .insert(wheel.clone(), binding.tire.clone());
        topology
            .wheel_attachment_indices
            .insert(wheel.clone(), binding.index);
    }
}

/// Per-prim sim-schema extractor (Pass 2) over the live composed [`UsdRead`]
/// surface — maps one composed prim's authored `lunco:*` / PhysX-vehicle
/// schemas to its sim/avatar/wheel components.
fn read_raycast_observation(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> Result<RaycastObservation, ()> {
    let axis = match reader.text(path, "lunco:raycast:axis").as_deref() {
        Some("X") => DVec3::X,
        Some("-X") => DVec3::NEG_X,
        Some("Y") => DVec3::Y,
        Some("-Y") => DVec3::NEG_Y,
        Some("Z") => DVec3::Z,
        Some("-Z") => DVec3::NEG_Z,
        Some(_) | None => return Err(()),
    };
    let max_distance = match reader.real(path, "lunco:raycast:maxDistance") {
        Some(value) if value.is_finite() && value > 0.0 => value,
        Some(_) | None => return Err(()),
    };
    let offset = match read_vec3_f64(reader, path, "lunco:raycast:offset") {
        Some(value) if value.iter().all(|value| value.is_finite()) => {
            DVec3::new(value[0], value[1], value[2])
        }
        Some(_) | None => return Err(()),
    };
    Ok(RaycastObservation {
        offset,
        axis,
        max_distance,
        ..default()
    })
}

fn push_usd_sim_diagnostic(
    findings: &mut Vec<lunco_core::RuntimeDiagnostic>,
    subject: &str,
    code: &str,
    message: impl Into<String>,
) {
    findings.push(lunco_core::RuntimeDiagnostic {
        code: code.to_string(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: "usd-sim".to_string(),
        subject: subject.to_string(),
        message: message.into(),
    });
}

#[cfg(test)]
mod raycast_tests {
    use super::read_raycast_observation;
    use lunco_usd_bevy_stage::canonical::CanonicalStage;
    use lunco_usd_compose::recipe::StageRecipe;
    use openusd::sdf::Path as SdfPath;

    fn read(source: &str) -> Result<lunco_physics::raycast::RaycastObservation, ()> {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("ray.usda", source))
            .expect("raycast fixture composes");
        let path = SdfPath::new("/Sensor").expect("raycast path");
        read_raycast_observation(&stage.view(), &path)
    }

    #[test]
    fn malformed_authored_offset_is_rejected() {
        assert!(
            read(
                r#"#usda 1.0
def Xform "Sensor" (prepend apiSchemas = ["LunCoRaycastAPI"])
{
    string lunco:raycast:offset = "bad"
}
"#
            )
            .is_err()
        );
    }

    #[test]
    fn non_positive_authored_distance_is_rejected() {
        assert!(
            read(
                r#"#usda 1.0
def Xform "Sensor" (prepend apiSchemas = ["LunCoRaycastAPI"])
{
    float lunco:raycast:maxDistance = 0.0
}
"#
            )
            .is_err()
        );
    }

    #[test]
    fn standard_defaults_and_authored_values_are_read_together() {
        let observation = read(
            r#"#usda 1.0
def Xform "Sensor" (prepend apiSchemas = ["LunCoRaycastAPI"])
{
    token lunco:raycast:axis = "Z"
    float lunco:raycast:maxDistance = 12.5
    double3 lunco:raycast:offset = (1.0, 2.0, 3.0)
}
"#,
        )
        .expect("valid raycast");
        assert_eq!(observation.axis, bevy::math::DVec3::Z);
        assert_eq!(observation.max_distance, 12.5);
        assert_eq!(observation.offset, bevy::math::DVec3::new(1.0, 2.0, 3.0));
    }
}
fn process_usd_sim_prim_read(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    entity: Entity,
    prim_path: &UsdPrimPath,
    sdf_path: SdfPath,
    maybe_tf: Option<&Transform>,
    maybe_mesh: Option<&Mesh3d>,
    maybe_mat: Option<&PbrLook>,
    maybe_shader_mat: Option<&ShaderLook>,
    mesh_pending: bool,
    shader_bound: bool,
    topology: &StageJointTopology,
    all_prims: &Query<(
        Entity,
        &UsdPrimPath,
        Option<&Transform>,
        Option<&UsdInstanceProjection>,
    )>,
    q_child_of: &Query<&ChildOf>,
    q_preview_only: &Query<(), With<UsdPreviewOnly>>,
    instance_projection: Option<&UsdInstanceProjection>,
    stage_source: &str,
    grid_components: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    commands: &mut Commands,
    diagnostics: &mut Vec<lunco_core::RuntimeDiagnostic>,
) {
    let existing_tf = maybe_tf.cloned().unwrap_or_default();

    // Navigation consumes the authored steering capability, not an asset name
    // or a vehicle-specific branch. Project it on the simulation owner, which
    // is the same entity resolved by the generic pose/navigation bridge.
    // Omitted or invalid capability leaves navigation unavailable and therefore
    // fail-closed; no vehicle class is guessed here.
    match reader.text(&sdf_path, "lunco:steeringGeometry") {
        Some(value) => match lunco_spatial::parse_steering_geometry(&value) {
            Some(geometry) => {
                commands.entity(entity).try_insert(geometry);
            }
            None => {
                warn!(
                    "USD prim {} has invalid `lunco:steeringGeometry` `{}`; navigation capability refused",
                    sdf_path.as_str(),
                    value
                );
                commands
                    .entity(entity)
                    .try_remove::<lunco_spatial::SteeringGeometry>();
            }
        },
        None => {
            commands
                .entity(entity)
                .try_remove::<lunco_spatial::SteeringGeometry>();
        }
    }

    match raycast_mass_contribution_from_usd(
        reader,
        &sdf_path,
        prim_path.stage_handle.id(),
        stage_source,
        all_prims,
        instance_projection,
        q_child_of,
        q_preview_only,
    ) {
        Ok(Some(contribution)) => {
            commands.entity(entity).try_insert(contribution);
        }
        Ok(None) => {}
        Err(reason) => {
            error!(
                "USD mass contribution {} is invalid — refusing the reduced realization: {}",
                sdf_path.as_str(),
                reason
            );
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        }
    }
    let is_avatar = if reader.has_api_schema(&sdf_path, "LunCoAvatarAPI") {
        match read_authored_bool_strict(reader, &sdf_path, "lunco:avatar") {
            Ok(Some(value)) => value,
            Ok(None) => false,
            Err(error) => {
                let message = format!(
                    "{} has malformed authored `lunco:avatar`: {error}",
                    sdf_path.as_str()
                );
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "avatar-attribute",
                    message.clone(),
                );
                warn!("USD prim {message}");
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        }
    } else {
        false
    };

    // --- Network replication policy, derived from USD ---
    // Structure from the joint graph (Pass 1) + `lunco:net:*` overrides. Stamps
    // the structural markers (`ArticulatedVehicle`/`ArticulatedLink`) and any
    // explicit opt-out / opacity override; the DEFAULT "replicate every non-static
    // rigid body" is applied downstream by `apply_net_replication` (it needs the
    // live avian `RigidBody`, which materialises later). Runs once per prim (this
    // pass is gated `Without<UsdSimProcessed>`). Replaces the old runtime `ChildOf`
    // walk + `setup_physical_wheel` side-effect. See USD_REPLICATION_POLICY.md.
    if topology.articulation_roots.contains(&prim_path.path)
        || reader.has_api_schema(&sdf_path, "PhysicsArticulationRootAPI")
    {
        commands
            .entity(entity)
            .try_insert(lunco_core_session::ArticulatedVehicle);
    }
    if topology.joint_targets.contains_key(&prim_path.path) {
        commands
            .entity(entity)
            .try_insert(lunco_core_session::ArticulatedLink);
    }
    // Initialization is a pre-admission policy, not an implicit terrain
    // placement algorithm. The default is installed by the USD→Avian body owner.
    // Custom selection is a registered USD API field and dispatches through the
    // one declared physics.initialization Rhai seam. Missing schema or selector
    // state is retained as an error and leaves the body pending.
    if reader.has_api_schema(&sdf_path, "PhysicsRigidBodyAPI") {
        commands
            .entity(entity)
            .try_remove::<lunco_physics::PhysicsInitializationInvalid>();
    }
    if reader.has_api_schema(&sdf_path, "PhysicsRigidBodyAPI") {
        let has_policy_api = reader.has_api_schema(&sdf_path, "LunCoPhysicsInitializationAPI");
        let has_policy_attribute =
            reader.has_authored_attribute(&sdf_path, "lunco:physics:initializationPolicy");
        let policy_result: Result<lunco_physics::PhysicsInitializationPolicy, String> =
            match (has_policy_api, has_policy_attribute) {
            (false, false) => Ok(lunco_physics::PhysicsInitializationPolicy::default()),
            (true, true) => reader
                .text(&sdf_path, "lunco:physics:initializationPolicy")
                .ok_or_else(|| {
                    "LunCoPhysicsInitializationAPI selector is not a token".to_string()
                })
                .and_then(|name| {
                    lunco_physics::PhysicsInitializationPolicy::new(name)
                        .map_err(|error| error.to_string())
                }),
            (true, false) => Err(
                "LunCoPhysicsInitializationAPI requires an authored lunco:physics:initializationPolicy selector".to_string(),
            ),
            (false, true) => Err(
                "lunco:physics:initializationPolicy requires LunCoPhysicsInitializationAPI".to_string(),
            ),
        };
        match policy_result {
            Ok(policy) => {
                commands.entity(entity).try_insert(policy);
            }
            Err(error) => {
                commands
                    .entity(entity)
                    .try_insert(lunco_physics::PhysicsInitializationInvalid);
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "physics-initialization-policy",
                    &error,
                );
                warn!(
                    "USD prim {} has invalid physics initialization authoring: {}; dynamic admission remains held",
                    prim_path.path, error
                );
            }
        }
    }
    // Screen-facing label the PRIM asked for. Opt-in: only a prim that
    // authors `lunco:billboard = true` gets one, so adding the schema can
    // never make an existing scene sprout labels.
    let billboard_enabled = match read_authored_bool_strict(reader, &sdf_path, "lunco:billboard") {
        Ok(Some(value)) => value,
        Ok(None) => false,
        Err(_) => {
            push_usd_sim_diagnostic(
                diagnostics,
                &prim_path.path,
                "billboard-attribute",
                "lunco:billboard must be an authored boolean",
            );
            warn!(
                "USD prim {} has malformed `lunco:billboard`; label ignored",
                prim_path.path
            );
            false
        }
    };
    if billboard_enabled {
        let default = lunco_usd_bevy_scene::billboard::UsdBillboard::default();
        let billboard = (|| {
            let template = match reader.attr_value(&sdf_path, "lunco:billboard:text") {
                Some(Value::String(value)) => value,
                Some(_) if reader.has_authored_attribute(&sdf_path, "lunco:billboard:text") => {
                    return Err(());
                }
                _ => default.template.clone(),
            };
            let read_real = |name: &str, default_value: f32| -> Result<f32, ()> {
                match reader.real_f32(&sdf_path, name) {
                    Some(value) if value.is_finite() => Ok(value),
                    Some(_) => Err(()),
                    None if reader.has_authored_attribute(&sdf_path, name) => Err(()),
                    None => Ok(default_value),
                }
            };
            let offset_y = read_real("lunco:billboard:offsetY", default.offset_y)?;
            let fade_end = read_real("lunco:billboard:fadeEnd", default.fade_end)?;
            if fade_end <= 0.0 {
                return Err(());
            }
            Ok(lunco_usd_bevy_scene::billboard::UsdBillboard {
                template,
                offset_y,
                fade_end,
            })
        })();
        match billboard {
            Ok(billboard) => {
                commands.entity(entity).try_insert(billboard);
            }
            Err(_) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "billboard-contract",
                    "billboard attributes are malformed or outside their documented range",
                );
                warn!(
                    "USD prim {} has invalid billboard attributes; label ignored",
                    prim_path.path
                );
            }
        }
    }
    // Pointer behavior is scene intent, not a picking-backend concern.  The
    // render-free USD projection records it here; the GUI layer later maps the
    // primary-button pass-through part to Bevy's `Pickable` component.  This
    // keeps transparent markers usable by every scene and preserves the same
    // contract for future marker assets.
    if let Some(policy) = lunco_interaction_core::ScenePointerPolicy::from_usd(
        reader.text(&sdf_path, "lunco:interaction:left").as_deref(),
        reader.text(&sdf_path, "lunco:interaction:right").as_deref(),
    ) {
        commands.entity(entity).try_insert(policy);
    }

    // Physical actuators are generic USD descriptions. A force actuator and a
    // torque actuator publish ordinary scalar input ports; the cosim backend
    // later resolves those commands to Avian's force/torque writer. RCS names,
    // reaction-wheel names, and controller ownership do not appear here.
    if let Some(actuator) = lunco_usd_actuation::force_actuator_from_usd(reader, &sdf_path) {
        commands.entity(entity).try_insert(actuator);
    }
    if let Some(actuator) = lunco_usd_actuation::torque_actuator_from_usd(reader, &sdf_path) {
        commands.entity(entity).try_insert(actuator);
    }
    // Screen-constant marker, keyed on the size that IS the request: a prim
    // authoring no `angularSizeDeg` is not a half-declared marker, it is simply
    // not one. Same opt-in shape as the billboard above.
    if reader.has_authored_attribute(&sdf_path, "lunco:marker:angularSizeDeg") {
        let default = lunco_render::ScreenConstantMarker::default();
        let marker = (|| {
            let angular_deg = match reader.real_f32(&sdf_path, "lunco:marker:angularSizeDeg") {
                Some(value) if value.is_finite() && value > 0.0 => value,
                _ => return Err(()),
            };
            let show_beyond_m = match reader.real_f32(&sdf_path, "lunco:marker:showBeyondM") {
                Some(value) if value.is_finite() && value >= 0.0 => value,
                None if !reader.has_authored_attribute(&sdf_path, "lunco:marker:showBeyondM") => {
                    default.show_beyond_m
                }
                _ => return Err(()),
            };
            Ok(lunco_render::ScreenConstantMarker {
                angular_deg,
                show_beyond_m,
            })
        })();
        match marker {
            Ok(marker) => {
                commands.entity(entity).try_insert(marker);
            }
            Err(()) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "screen-marker-contract",
                    "screen marker attributes are malformed or outside their documented range",
                );
                warn!(
                    "USD prim {} has invalid screen marker attributes; marker ignored",
                    prim_path.path
                );
            }
        }
    }

    let net_replicate = reader.boolean(&sdf_path, "lunco:net:replicate");
    let net_authority = reader.text(&sdf_path, "lunco:net:authority");
    let (net_excluded, net_opaque) = net_override_markers(net_replicate, net_authority.as_deref());
    if net_excluded {
        commands
            .entity(entity)
            .try_insert(lunco_core_session::NetExcluded);
    }
    if net_opaque {
        commands
            .entity(entity)
            .try_insert(lunco_core_session::NotPredictable);
    }

    // --- Suspension visual roles: a prim that applies `LunCoSuspensionVisualAPI`
    // declares which moving part of a strut it is, and gets the Bevy component
    // the mobility system animates. Gated on the APPLIED schema, not on the
    // attr's presence — the API is the claim, the token is its parameter.
    //
    // The role is an authored attribute and NOT USD `kind` metadata: `kind` is
    // USD's regulated model taxonomy (component/assembly/subcomponent), and
    // "piston"/"spring" are not valid kinds. See
    // `assets/components/mobility/suspensions/standard.usda`.
    if reader.has_api_schema(&sdf_path, "LunCoSuspensionVisualAPI") {
        match reader
            .text(&sdf_path, "lunco:suspensionVisual:role")
            .as_deref()
        {
            Some("piston") => {
                commands.entity(entity).try_insert(SuspensionPiston {
                    initial_y: existing_tf.translation.y,
                });
            }
            Some("spring") => {
                commands.entity(entity).try_insert(SuspensionSpring);
            }
            Some("casing") => {
                // Static carrier-mounted housing. Physical-wheel projection
                // reparents it to the carrier; raycast wheels leave it in the
                // authored wheel hierarchy.
            }
            // The API's whole purpose is the role; applying it without one (or
            // with a token outside `allowedTokens`) is an authoring mistake.
            other => warn!(
                "USD prim {} applies LunCoSuspensionVisualAPI but its \
                     lunco:suspensionVisual:role is {:?} — expected \"casing\", \
                     \"piston\", or \"spring\"; no visual will be animated.",
                sdf_path.as_str(),
                other.unwrap_or("<unauthored>")
            ),
        }
    }

    // A raw Avian ray query is projected from its generic USD API. IMU,
    // altimeter, and contact conversions are ordinary Modelica/Avian wires;
    // this layer does not identify semantic sensor kinds.
    // A raycast prim is a generic Avian query description. It does not claim
    // that the result is an altimeter, range sensor, or touchdown detector;
    // those conversions are ordinary Modelica scopes authored in USD.
    if reader.has_api_schema(&sdf_path, "LunCoRaycastAPI") {
        match read_raycast_observation(reader, &sdf_path) {
            Ok(observation) => {
                // A raw ray is an Avian-backed output surface, not merely a
                // visual marker. Publish the generic readiness edge so the
                // USD connection projector can bind Modelica (or any other
                // authored consumer) to its ports even though this entity has
                // no SimComponent of its own.
                commands
                    .entity(entity)
                    .try_insert((observation, lunco_port_core::PortSurfaceReady));
            }
            Err(()) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "raycast-contract",
                    "raycast axis, offset, and maxDistance must be authored with finite valid values",
                );
                warn!(
                    "USD raycast {} has malformed or invalid axis, offset, or maxDistance",
                    sdf_path
                );
            }
        }
    }

    // Link/celestial vocabulary is projected by the independent
    // `lunco-usd-sim-celestial` plugin, not here. Bundling it in this system
    // made a cosim prim, which skips this system, lose its LinkNode.

    // Embodiment camera behavior is a presentation concern. USD simulation only
    // projects the avatar role and spatial identity; `lunco-avatar` realizes
    // the generic movement substrate and Rhai selects camera behavior.
    if is_avatar {
        info!(
            "Detected Embodiment prim at {}, publishing avatar role and spatial identity",
            prim_path.path
        );

        // Embodiment position from the live composed scene hierarchy. The USD
        // transform is local to its authored parent, so resolve the nearest
        // actual Grid in that parent chain and commit the complete spatial
        // handoff through the shared migration boundary.
        let Some((grid_entity, grid)) =
            lunco_spatial::coords::ancestor_grid(entity, q_child_of, grid_components)
        else {
            let message = format!(
                "{} is not below a BigSpace Grid; an explicit spatial frame is required",
                sdf_path.as_str()
            );
            push_usd_sim_diagnostic(
                diagnostics,
                &prim_path.path,
                "avatar-spatial-frame",
                message.clone(),
            );
            warn!("USD avatar {message}");
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        };
        let Some((position, rotation)) = lunco_spatial::coords::grid_relative_pose(
            entity,
            grid_entity,
            q_child_of,
            grid_components,
            q_spatial,
        ) else {
            let message = format!(
                "{} has an invalid spatial chain to Grid {grid_entity:?}",
                sdf_path.as_str()
            );
            push_usd_sim_diagnostic(
                diagnostics,
                &prim_path.path,
                "avatar-spatial-frame",
                message.clone(),
            );
            warn!("USD avatar {message}");
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        };
        let (avatar_cell, translation) = grid.translation_to_grid(position);
        let avatar_tf = Transform::from_translation(translation)
            .with_rotation(rotation.as_quat())
            .with_scale(existing_tf.scale);

        commands.entity(entity).try_insert((
            lunco_embodiment_core::roles::Embodiment,
            lunco_embodiment_core::roles::LocalEmbodiment,
        ));
        lunco_spatial::attach::migrate_to_grid(
            commands,
            entity,
            grid_entity,
            avatar_cell,
            avatar_tf,
        );
    }

    // 1. Detect PhysxVehicleContextAPI (the mobility root)
    // Stamps the generic mobility/selectable boundary from the PhysX schema. Numeric
    // outputs authored on the vehicle root become runtime actuator ports only when
    // they are not owned by a generated Modelica network. Outputs owned by that
    // network remain on its `SimComponent`; duplicating them into child `Port`s
    // would create a second, unwritten producer and make a cross-domain connection
    // read zero. The vehicle's command and drive behaviour therefore remains an
    // authored USD/Modelica contract; Rust does not fabricate a steering model.
    if reader.has_api_schema(&sdf_path, "PhysxVehicleContextAPI") {
        info!(
            "Intercepted PhysxVehicleContextAPI for {}, initializing vessel control surface",
            prim_path.path
        );

        let mut port_map = HashMap::new();
        let port_names = if instance_projection.is_some() {
            collect_vehicle_output_ports(reader, &sdf_path).0
        } else if let Some(ports) = topology.vehicle_output_ports.get(&prim_path.path) {
            ports.clone()
        } else {
            let message = format!(
                "{} applies PhysxVehicleContextAPI but its prepared actuator outputs are unavailable",
                prim_path.path
            );
            push_usd_sim_diagnostic(
                diagnostics,
                &prim_path.path,
                "vehicle-output-preparation",
                message.clone(),
            );
            warn!("USD vehicle {message}");
            Vec::new()
        };
        // A port is an authored numeric `outputs:` attribute, the same way a
        // command is an `inputs:` attribute. This supports conventional
        // drive_left/drive_right/steering/brake names and arbitrary per-wheel
        // channels without a Rust-side hard-coded vocabulary.
        commands
            .entity(entity)
            .try_insert((lunco_core::SelectableRoot, lunco_core::MobilityRoot))
            .remove::<lunco_port_core::OutputPorts>();

        if port_names.is_empty() {
            debug!(
                "USD vehicle {} has no external numeric outputs:* ports; authored generated network owns its actuator outputs",
                prim_path.path
            );
        } else {
            for name in &port_names {
                // `ChildOf(entity)`: the actuator ports are owned by the vehicle so the
                // recursive scene-clear reclaims them with it — no detached-at-root
                // survivors across a scene swap (general lifecycle contract).
                let port_ent = commands
                    .spawn((
                        Port::default(),
                        Name::new(format!("Port_{}", name)),
                        ChildOf(entity),
                    ))
                    .id();
                port_map.insert(name.clone(), port_ent);
            }

            commands
                .entity(entity)
                .try_insert(lunco_port_core::OutputPorts::new(port_map));
        }

        // The input surface is AUTHORED, in the vessel's `Controls` scope: the
        // intents it binds name exactly the ports this vessel accepts.
        // `sync_input_ports` seeds them from the `ControlBinding`, so the
        // vocabulary is never decided here — it used to be the literal
        // `&["throttle", "steer", "brake"]`, which meant the engine decided what
        // could command a vehicle by knowing what kind of vehicle it was.
        //
        // `InputPorts` is seeded EMPTY: the shared input backend is strict, so a
        // vessel whose `Controls` scope is absent accepts nothing and every write is
        // refused. That is how you author a wreck or an un-crewed chassis — by
        // composition, not a check.
        //
        // `MobilityRoot` is stamped here. The `InputPorts` surface is
        // stamped beside the `ControlBinding` (lunco-usd-bevy, the `Controls`
        // branch) — ONE site, because `try_insert` OVERWRITES: stamping a fresh
        // empty surface from two different systems would let a live re-run of
        // either one wipe the keys `sync_input_ports` had already seeded.
        //
        // `OutputPorts`, when authored, is a different thing and is NOT the input surface: it
        // maps ACTUATOR names to their `Port` entities, built above from the
        // vessel prim's authored `outputs:` attributes. The
        // two stay separate components on purpose — both carry a `"brake"`, and
        // they are not the same value (analog command vs discretized gate).
    }

    // 2b. A GEAR JOINT — `PhysxPhysicsGearJoint`, the PhysX schema for two hinges
    // geared to each other. A rocker-bogie's differential is one of these: gear the
    // left and right rocker hinges at −1 and the chassis rides the AVERAGE of them,
    // which is what keeps the body level over rough ground.
    //
    // Nothing here is rocker-bogie code. A gear joint is a gear joint, and any
    // geared linkage authored this way gets the same coupling with no new Rust.
    // The backend implements the standard angular drive on the gear relation;
    // an omitted drive therefore leaves the gear passive instead of inventing a
    // solver stiffness.
    //
    // Defer-resolved once both geared bodies spawn.
    if is_gear_drive(reader, &sdf_path) {
        let hinges = (
            reader.rel_target(&sdf_path, "physxGearJoint:hinge0"),
            reader.rel_target(&sdf_path, "physxGearJoint:hinge1"),
        );
        // The bodies the gear turns are the ones its hinges turn: a hinge's `body1`
        // is the part that moves, `body0` the frame it moves against. So the gear's
        // reaction goes into the hinges' shared frame — the chassis.
        let geared = |hinge: &Option<String>| -> Option<(String, String)> {
            let h = SdfPath::new(hinge.as_deref()?).ok()?;
            Some((
                reader.rel_target(&h, "physics:body1")?,
                reader.rel_target(&h, "physics:body0")?,
            ))
        };
        if let (Some((body_a, frame)), Some((body_b, _))) = (geared(&hinges.0), geared(&hinges.1)) {
            let Some(ratio) = read_gear_ratio(reader, &sdf_path) else {
                warn!(
                    "Gear joint {} has no valid non-zero physxGearJoint:gearRatio; coupling ignored",
                    prim_path.path
                );
                return;
            };
            let Ok(GearDriveValues {
                rest_offset,
                target_velocity,
                stiffness,
                damping,
                max_force,
            }) = read_gear_drive_values(reader, &sdf_path)
            else {
                warn!(
                    "Gear joint {} has malformed angular PhysicsDriveAPI values; coupling ignored",
                    prim_path.path
                );
                return;
            };
            let Some(drive_type) = read_gear_drive_type(reader, &sdf_path) else {
                warn!(
                    "Gear joint {} has an unsupported angular PhysicsDriveAPI type; coupling ignored",
                    prim_path.path
                );
                return;
            };
            info!(
                "Gear joint {} couples {} / {} (ratio {}, stiffness {}, damping {})",
                prim_path.path, body_a, body_b, ratio, stiffness, damping,
            );
            commands.entity(entity).try_insert(PendingDifferential {
                chassis: frame,
                rocker_a: body_a,
                rocker_b: body_b,
                ratio,
                rest_offset,
                target_velocity,
                stiffness,
                damping,
                max_force,
                drive_type,
            });
        }
    }

    // 3. Detect PhysxVehicleWheelAPI (The Wheel Intercept)
    //
    // By the APPLIED schema, like the vehicle context API here. Applying the
    // API is what makes a prim a wheel; authoring a radius is not. Sniffing for
    // `physxVehicleWheel:radius` conflated "declares itself a wheel" with
    // "happens to carry a wheel-ish attr" — any prim with a stray radius was
    // a wheel, and a wheel could not be authored without one.
    if reader.has_api_schema(&sdf_path, "PhysxVehicleWheelAPI") {
        if topology.invalid_wheel_attachments.contains(&prim_path.path) {
            error!(
                "USD wheel {} has malformed or ambiguous PhysxVehicleWheelAttachmentAPI topology — refusing to spawn",
                prim_path.path
            );
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        }
        // Appearance is render-free intent. The visual extractor and shader
        // projector run before this owner, while headless hosts simply leave
        // the optional visual components absent; neither case is allowed to
        // delay the authoritative physics projection.
        let wants_shader = reader.rel_target(&sdf_path, "material:binding").is_some();
        if wants_shader && maybe_shader_mat.is_none() {
            debug!(
                "Wheel {} has authored shader binding without a projected ShaderLook",
                prim_path.path
            );
        }
        info!("Intercepted PhysxVehicleWheelAPI for {}", prim_path.path);

        // ONE unified read for BOTH wheel kinds (see the authoring reader): every
        // drivetrain/tire/inertia number plus suspension, resolved through the
        // standard attachment relationship or explicit direct wheel/suspension
        // composition. Strict — all missing required attrs are collected and the
        // wheel refuses to spawn; the authored defaults live in
        // components/mobility/wheel.usda, which every wheel composes.
        // Read BEFORE spawning the port entities so an invalid wheel
        // synthesizes nothing.
        let attachment_susp = wheel_runtime::attachment_suspension_path(
            &prim_path.path,
            &topology.wheel_attachment_targets,
        );
        let attachment_tire =
            wheel_runtime::attachment_tire_path(&prim_path.path, &topology.wheel_attachment_tires);
        let params = match WheelParams::read(
            reader,
            &sdf_path,
            attachment_susp.as_ref(),
            attachment_tire.as_ref(),
        ) {
            Ok(p) => p,
            Err(missing) => {
                error!(
                    "USD wheel {} is missing required wheel attributes {:?} — \
                         refusing to spawn. They are authored in \
                         components/mobility/wheel.usda; a wheel that does not \
                         compose it has no handling to speak of.",
                    sdf_path.as_str(),
                    missing
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        };
        // A physical wheel is a separate body only in the physical realization. Its
        // collision shape is an authored USD geometry prim, not a collider synthesized
        // from wheel dynamics. Validate it before creating any synthesized ports so an
        // invalid authored body leaves no partial runtime projection behind.
        let is_physical = topology.joint_targets.contains_key(&prim_path.path);
        let authored_collider = if is_physical {
            if !reader.has_api_schema(&sdf_path, ptok::API_RIGID_BODY) {
                error!(
                    "USD physical wheel {} has no authored PhysicsRigidBodyAPI — refusing to spawn",
                    sdf_path.as_str()
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
            let collider = match lunco_usd_avian_reader::collider::authored_collider_from_usd(
                reader, &sdf_path,
            ) {
                Ok(collider) => collider,
                Err(error) => {
                    error!(
                        "USD physical wheel {} has invalid authored collision geometry — refusing to spawn: {}",
                        sdf_path.as_str(),
                        error
                    );
                    commands.entity(entity).try_insert(UsdSimProcessed);
                    return;
                }
            };
            Some(oriented_wheel_collider(collider, params.axle_axis))
        } else {
            None
        };

        // Create the actuator-side ports for drive and heading. Owned by the wheel via
        // `ChildOf` so the single recursive scene-clear reclaims them with the
        // wheel — synthesized backing entities are never left detached at the root
        // (the general lifecycle contract; see `setup_physical_wheel`'s joint).
        let p_drive = commands
            .spawn((Port::default(), Name::new("Port_Drive"), ChildOf(entity)))
            .id();
        let p_heading = commands
            .spawn((Port::default(), Name::new("Port_Heading"), ChildOf(entity)))
            .id();
        let p_speed = commands
            .spawn((
                Port::default(),
                Name::new("Port_ShaftSpeed"),
                ChildOf(entity),
            ))
            .id();

        // Wheel identity belongs to the standard attachment schema. The index
        // is looked up through the stage-local wheel→attachment map, so the
        // canonical relationship form reads the value from the attachment
        // prim and the direct self-composition form remains explicit. There is
        // no wheel-order or parity fallback.
        let Some(index) = topology
            .wheel_attachment_indices
            .get(&prim_path.path)
            .copied()
        else {
            error!(
                "USD wheel {} has no indexed PhysxVehicleWheelAttachmentAPI binding — refusing to spawn",
                sdf_path.as_str()
            );
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        };
        if index < 0 {
            error!(
                "USD wheel {} has the standard attachment index {} — vehicle wheels must author a non-negative index",
                sdf_path.as_str(),
                index
            );
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        }

        // Optional per-wheel actuator binding, as a USD CONNECTION:
        //   float inputs:drive.connect = </Rover.outputs:drive_left>
        // This keeps the rover's wiring topology in USD, enabling per-wheel drive and
        // non-2×N layouts.
        //
        // A connection, not a name: PCP resolves and PATH-TRANSLATES it through
        // reference arcs, so a wheel that arrives on a `references` arc points at
        // its own instance's port rather than at whatever prim happens to share
        // the name. The port it names is the property, so `outputs:drive_left`
        // resolves to the FSW port `drive_left`.
        let connected_source =
            |attr: &str| -> Option<String> { reader.connection_source(&sdf_path, attr) };
        let Some(_drive_source) = connected_source("inputs:drive") else {
            error!(
                "USD wheel {} has no inputs:drive connection — drive topology must be authored",
                sdf_path.as_str()
            );
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        };
        commands.entity(entity).try_insert((
            PortSurface::new(HashMap::from([
                (
                    "drive".to_owned(),
                    PortSurfacePort::new(p_drive, PortDirection::In),
                ),
                (
                    "heading".to_owned(),
                    PortSurfacePort::new(p_heading, PortDirection::In),
                ),
                (
                    "shaft_speed".to_owned(),
                    PortSurfacePort::new(p_speed, PortDirection::Out),
                ),
            ])),
            lunco_port_core::PortSurfaceReady,
        ));

        // A wheel receives only the scalar signals explicitly authored on its
        // own inputs. A connected `inputs:heading` is the final wheel heading;
        // no vehicle class or wheel index is consulted.
        let physical_body_path = if is_physical {
            let Some(path) = topology.physical_wheel_bodies.get(&prim_path.path) else {
                error!(
                    "USD physical wheel {} has no authored revolute body0 owner — refusing to spawn",
                    sdf_path.as_str()
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            };
            Some(path.as_str())
        } else {
            None
        };
        let Some(body_mount) = wheel_body_mount(
            reader,
            &sdf_path,
            physical_body_path,
            prim_path.stage_handle.id(),
            all_prims,
            instance_projection,
            q_child_of,
            q_preview_only,
        ) else {
            error!(
                "USD wheel {} has no resolved authored rigid-body owner — refusing to spawn",
                sdf_path.as_str()
            );
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        };
        let wheel_order_key = match usd_physics_order_key(
            stage_source,
            &prim_path.path,
            instance_projection,
            all_prims,
        ) {
            Ok(key) => key,
            Err(reason) => {
                error!(
                    "USD wheel {} has no stable physics identity — refusing to spawn: {}",
                    sdf_path.as_str(),
                    reason
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        };
        if is_physical {
            let Some(authored_collider) = authored_collider else {
                error!(
                    "USD physical wheel {} lost its validated authored collider — refusing to spawn",
                    sdf_path.as_str()
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            };
            let Some(vehicle_mount) = vehicle_mount_transform(reader, &sdf_path) else {
                error!(
                    "USD physical wheel {} has no resolved PhysxVehicleContextAPI owner — refusing to spawn",
                    sdf_path.as_str()
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            };
            setup_physical_wheel(
                commands,
                entity,
                prim_path,
                wheel_order_key.0.clone(),
                &existing_tf,
                maybe_mesh,
                maybe_mat,
                maybe_shader_mat,
                mesh_pending,
                shader_bound,
                physical_suspension_visuals(
                    reader,
                    prim_path,
                    entity,
                    Transform {
                        translation: existing_tf.translation,
                        rotation: Quat::IDENTITY,
                        scale: existing_tf.scale,
                    },
                    all_prims,
                    q_child_of,
                ),
                &params,
                body_mount,
                vehicle_mount,
                p_drive,
                p_speed,
                authored_collider,
            );
        } else {
            // Strict validation (doc 53 §4): a raycast wheel uses an
            // analytical spring-damper and CANNOT function without suspension
            // compliance params. No silent defaults — missing suspension is an
            // asset-composition bug, and we expose it loudly rather than
            // spawning a wheel with fabricated k/c/rest values. Joint/rigid
            // wheels took the `setup_physical_wheel` branch above and are
            // unaffected (§4.2).
            let Some(suspension) = params.suspension else {
                error!(
                    "USD raycast wheel {} has no suspension compliance \
                         (neither authored via physxVehicleSuspension:* nor resolvable \
                         via a PhysxVehicleWheelAttachmentAPI:suspension relationship) \
                         — refusing to spawn. Add a suspension reference to the wheel \
                         prim. See doc 53 §4.",
                    sdf_path.as_str()
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            };
            setup_raycast_wheel(
                commands,
                entity,
                prim_path,
                wheel_order_key,
                &existing_tf,
                maybe_mesh,
                maybe_mat,
                maybe_shader_mat,
                mesh_pending,
                shader_bound,
                &params,
                &suspension,
                body_mount,
                p_drive,
                p_speed,
                p_heading,
            );
        }
    }

    commands.entity(entity).try_insert(UsdSimProcessed);
}

/// Pure mapping of the `lunco:net:*` override attributes to replication markers,
/// factored out so the policy vocabulary is unit-testable without a USD/avian build.
///
/// Returns `(excluded, opaque)`:
/// - `excluded` ⇒ stamp [`lunco_core_session::NetExcluded`] (skip default replication):
///   `lunco:net:replicate = false` OR `lunco:net:authority = "local"`.
/// - `opaque` ⇒ stamp [`lunco_core_session::NotPredictable`] (never client-predicted):
///   `lunco:net:authority = "opaque"`.
///
/// `server`/`predictable`/absent ⇒ the default (replicated, predictable). See
/// `crates/lunco-networking/USD_REPLICATION_POLICY.md`.
fn net_override_markers(replicate: Option<bool>, authority: Option<&str>) -> (bool, bool) {
    let excluded = replicate == Some(false) || authority == Some("local");
    let opaque = authority == Some("opaque");
    (excluded, opaque)
}

/// Find the ECS entity for one exact composed USD prim path in this stage.
/// Entity identity is supplied by USD instantiation; ownership is never
/// inferred from a prim name or from an incidental Bevy parent.
fn usd_entity_for_path(
    all_prims: &Query<(
        Entity,
        &UsdPrimPath,
        Option<&Transform>,
        Option<&UsdInstanceProjection>,
    )>,
    stage: bevy::asset::AssetId<UsdStageAsset>,
    path: &str,
    instance_root: Option<Entity>,
    q_child_of: &Query<&ChildOf>,
    q_preview_only: &Query<(), With<UsdPreviewOnly>>,
) -> Option<Entity> {
    let mut matches = all_prims
        .iter()
        .filter(|(entity, prim, _, projection)| {
            prim.stage_handle.id() == stage
                && prim.path == path
                && projection.and_then(|projection| projection.root) == instance_root
                && !is_preview_only(*entity, q_child_of, q_preview_only)
        })
        .map(|(entity, _, _, _)| entity);
    let entity = matches.next()?;
    matches.next().is_none().then_some(entity)
}

fn usd_physics_order_key(
    stage_source: &str,
    prim_path: &str,
    instance_projection: Option<&UsdInstanceProjection>,
    all_prims: &Query<(
        Entity,
        &UsdPrimPath,
        Option<&Transform>,
        Option<&UsdInstanceProjection>,
    )>,
) -> Result<lunco_physics::PhysicsOrderKey, String> {
    let instance_root_path = if let Some(projection) = instance_projection {
        let root = projection
            .root
            .ok_or_else(|| "instanced prim has no projected instance root".to_owned())?;
        let (_, root_path, ..) = all_prims
            .get(root)
            .map_err(|_| "projected instance root has no USD prim identity".to_owned())?;
        Some(root_path.path.as_str())
    } else {
        None
    };
    stable_usd_physics_order_key(stage_source, instance_root_path, prim_path)
}

/// Resolve the nearest authored rigid body above a raycast wheel. The wheel
/// itself is a reusable rigid-body prim for the physical realization, so the
/// raycast realization starts at its parent and walks the composed topology to
/// the enclosing body.
fn raycast_body_path(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    wheel_path: &SdfPath,
) -> Option<SdfPath> {
    let mut path = wheel_path.parent()?;
    loop {
        if reader.has_api_schema(&path, "PhysicsRigidBodyAPI") {
            return Some(path);
        }
        path = path.parent()?;
    }
}

/// Resolve a wheel's body owner and body-local pose from authored USD
/// topology. A wheel may be nested under a non-body carrier, and a physical
/// wheel's owner is the body named by its authored revolute `body0` relation.
fn wheel_body_mount(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    wheel_path: &SdfPath,
    physical_body_path: Option<&str>,
    stage: bevy::asset::AssetId<UsdStageAsset>,
    all_prims: &Query<(
        Entity,
        &UsdPrimPath,
        Option<&Transform>,
        Option<&UsdInstanceProjection>,
    )>,
    instance_projection: Option<&UsdInstanceProjection>,
    q_child_of: &Query<&ChildOf>,
    q_preview_only: &Query<(), With<UsdPreviewOnly>>,
) -> Option<lunco_mobility::WheelBodyMount> {
    let body_path = if let Some(path) = physical_body_path {
        SdfPath::new(path).ok()?
    } else {
        raycast_body_path(reader, wheel_path)?
    };
    let body = usd_entity_for_path(
        all_prims,
        stage,
        body_path.as_str(),
        instance_projection.and_then(|projection| projection.root),
        q_child_of,
        q_preview_only,
    )?;
    let local = lunco_usd_bevy_stage::transform_in_body_frame(reader, &body_path, wheel_path)?;
    Some(lunco_mobility::WheelBodyMount { body, local })
}

/// Resolve the enclosing authored vehicle frame used by heading geometry.
/// This is separate from the wheel's immediate mechanical carrier: a physical
/// wheel's carrier owns the prismatic DOF, while the authored heading program
/// is evaluated in the vehicle context frame.
fn vehicle_mount_transform(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    wheel_path: &SdfPath,
) -> Option<Transform> {
    let mut path = wheel_path.clone();
    loop {
        if reader.has_api_schema(&path, "PhysxVehicleContextAPI") {
            let wheel = lunco_usd_bevy_stage::world_transform(reader, wheel_path).ok()?;
            let vehicle = lunco_usd_bevy_stage::world_transform(reader, &path).ok()?;
            let inverse = vehicle.rotation.inverse();
            return Some(Transform {
                translation: inverse * (wheel.translation - vehicle.translation),
                rotation: (inverse * wheel.rotation).normalize(),
                scale: Vec3::ONE,
            });
        }
        path = path.parent()?;
    }
}

/// Project a standard USD child mass into the reduced raycast realization.
/// The explicit applied API is the authoring contract; no asset name or
/// drivetrain string identifies a contribution. A full physical variant keeps
/// the same prim as its own `PhysicsRigidBodyAPI` and therefore does not fold it.
fn raycast_mass_contribution_from_usd(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    prim: &SdfPath,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    stage_source: &str,
    all_prims: &Query<(
        Entity,
        &UsdPrimPath,
        Option<&Transform>,
        Option<&UsdInstanceProjection>,
    )>,
    instance_projection: Option<&UsdInstanceProjection>,
    q_child_of: &Query<&ChildOf>,
    q_preview_only: &Query<(), With<UsdPreviewOnly>>,
) -> Result<Option<lunco_mobility::RaycastMassContribution>, String> {
    if !reader.has_api_schema(prim, "LunCoMassContributionAPI")
        || reader.has_api_schema(prim, "PhysicsRigidBodyAPI")
    {
        return Ok(None);
    }
    let mass = reader
        .real(prim, "physics:mass")
        .ok_or_else(|| "missing `physics:mass`".to_owned())?;
    let inertia = read_vec3_f64(reader, prim, "physics:diagonalInertia")
        .ok_or_else(|| "missing `physics:diagonalInertia`".to_owned())?;
    if !mass.is_finite()
        || mass <= 0.0
        || !inertia
            .iter()
            .all(|value| value.is_finite() && *value > 0.0)
    {
        return Err(format!(
            "invalid mass properties: mass={mass}, diagonalInertia={inertia:?}"
        ));
    }
    let convention = lunco_usd_bevy_stage::stage_convention(reader)
        .map_err(|reason| format!("invalid stage convention: {reason}"))?;
    let meters_per_unit = convention.length(1.0);
    let mut body_path = prim
        .parent()
        .ok_or_else(|| "mass contribution has no parent body".to_owned())?;
    while !body_path.is_empty() {
        if reader.has_api_schema(&body_path, "PhysicsRigidBodyAPI") {
            break;
        }
        body_path = body_path
            .parent()
            .ok_or_else(|| "no enclosing PhysicsRigidBodyAPI owner".to_owned())?;
    }
    if !reader.has_api_schema(&body_path, "PhysicsRigidBodyAPI") {
        return Err("no enclosing PhysicsRigidBodyAPI owner".into());
    }
    let owner = usd_entity_for_path(
        all_prims,
        stage_id,
        body_path.as_str(),
        instance_projection.and_then(|projection| projection.root),
        q_child_of,
        q_preview_only,
    )
    .ok_or_else(|| format!("owner entity {} is not projected", body_path.as_str()))?;
    let local = lunco_usd_bevy_stage::transform_in_body_frame(reader, &body_path, prim)
        .ok_or_else(|| "cannot resolve local transform".to_owned())?;
    let principal = convention.dir_d(DVec3::new(inertia[0], inertia[1], inertia[2]))
        * (meters_per_unit * meters_per_unit);
    let order_key =
        usd_physics_order_key(stage_source, prim.as_str(), instance_projection, all_prims)?;
    Ok(Some(lunco_mobility::RaycastMassContribution {
        owner,
        order_key,
        local,
        mass,
        principal,
    }))
}

/// Create the render side of a wheel split, including when its CPU mesh is
/// still pending. The USD entity remains the physics owner; the explicit target
/// lets `lunco-usd-bevy` commit the eventual mesh and material to this child.
fn spawn_wheel_visual(
    commands: &mut Commands,
    entity: Entity,
    prim_path: &UsdPrimPath,
    transform: Transform,
    maybe_mesh: Option<&Mesh3d>,
    maybe_mat: Option<&PbrLook>,
    maybe_shader_mat: Option<&ShaderLook>,
    mesh_pending: bool,
    shader_bound: bool,
) -> Option<Entity> {
    if maybe_mesh.is_none() && !mesh_pending {
        return None;
    }

    let mut visual = commands.spawn((
        Name::new(format!(
            "{}_visual",
            prim_path.path.split('/').next_back().unwrap_or("wheel")
        )),
        transform,
        Visibility::Inherited,
        InheritedVisibility::default(),
        ViewVisibility::default(),
        ChildOf(entity),
    ));
    if let Some(mesh) = maybe_mesh.cloned() {
        visual.try_insert(mesh);
    }
    // `ShaderLook` and `PbrLook` are mutually exclusive render intents. The
    // shader path wins, preserving the composed USD material through the split.
    match maybe_shader_mat.cloned() {
        Some(shader) => {
            visual.try_insert(shader);
        }
        _ => match maybe_mat.cloned() {
            Some(material) => {
                visual.try_insert(material);
            }
            _ => {}
        },
    }
    if shader_bound {
        visual.try_insert(lunco_usd_bevy_scene::UsdVisualShaderBound);
    }

    let visual_entity = visual.id();
    commands
        .entity(entity)
        .try_insert(lunco_usd_bevy_scene::UsdVisualMeshTarget(visual_entity));
    commands
        .entity(entity)
        .remove::<Mesh3d>()
        .remove::<PbrLook>()
        .remove::<ShaderLook>()
        .remove::<lunco_usd_bevy_scene::UsdVisualShaderBound>();
    Some(visual_entity)
}

/// Sets up a raycast wheel with entity splitting for correct raycasting.
///
/// Raycast wheels need two entities:
/// 1. **Physics entity**: identity rotation (for correct downward raycasting), NO mesh
/// 2. **Visual child entity**: 90° Z rotation + mesh (for correct rendering)
fn setup_raycast_wheel(
    commands: &mut Commands,
    entity: Entity,
    prim_path: &UsdPrimPath,
    order_key: lunco_physics::PhysicsOrderKey,
    existing_tf: &Transform,
    maybe_mesh: Option<&Mesh3d>,
    maybe_mat: Option<&PbrLook>,
    maybe_shader_mat: Option<&ShaderLook>,
    mesh_pending: bool,
    shader_bound: bool,
    params: &WheelParams,
    susp: &SuspensionParams,
    body_mount: lunco_mobility::WheelBodyMount,
    p_drive: Entity,
    p_speed: Entity,
    p_heading: Entity,
) {
    info!("Setting up RAYCAST wheel {}", prim_path.path);

    let mut wheel = params.to_wheel_raycast(p_drive, p_speed, p_heading, Some(entity));

    // --- Wheel Entity Splitting (always) ---
    // The physics entity needs identity rotation so `RayCaster::NEG_Y`
    // casts straight down. The visual mesh is moved to a child entity
    // so `apply_wheel_suspension` can reposition it to ground-level
    // each frame — its `q_visual` query filters out `WheelRaycast`,
    // so it can only operate on a separate visual entity.
    let wheel_rotation = existing_tf.rotation;
    let visual_id = spawn_wheel_visual(
        commands,
        entity,
        prim_path,
        Transform {
            translation: Vec3::ZERO,
            rotation: wheel_rotation,
            scale: existing_tf.scale,
        },
        maybe_mesh,
        maybe_mat,
        maybe_shader_mat,
        mesh_pending,
        shader_bound,
    );
    wheel.visual_entity = visual_id;

    // Physics entity: identity rotation, position preserved
    let wheel_tf = Transform {
        translation: existing_tf.translation,
        rotation: Quat::IDENTITY,
        scale: existing_tf.scale,
    };

    // Build the RayCaster with the non-physical layer mask. The mobility owner
    // projects the complete joint-connected assembly into the exclusion set
    // after Avian's runtime joint graph is available; setup cannot infer that
    // topology from one ChildOf edge.
    // THE RAY STARTS AT THE STRUT TOP, NOT AT THE PRIM. The wheel prim is the AXLE —
    // the same point the `physical` realization puts its wheel body at — so casting
    // from the prim itself would hang the hub a whole `rest_length` below the mount
    // and the two realizations would not share a ride height. `strut_offset` derives
    // the strut's rest extent (`rest_length − radius`) from the authored suspension,
    // which is what the drivetrain overlay used to fake with a 0.5 m difference in
    // the authored mount.
    let mut ray_caster = RayCaster::new(
        DVec3::new(
            0.0,
            lunco_mobility::strut_offset(susp.rest_length, params.radius),
            0.0,
        ),
        Dir3::NEG_Y,
    )
    // Suspension has no use for contacts beyond its authored travel. Avian's
    // default is an infinite ray, which makes an airborne/out-of-world wheel
    // traverse the entire collider tree every physics tick. The suspension
    // solver consumes only the nearest contact, so one bounded hit is the
    // complete physical query and keeps its cost independent of world extent.
    .with_max_distance(lunco_mobility::suspension_ray_max_distance(
        susp.rest_length,
    ))
    .with_max_hits(1);
    // Mask out the non-physical layers so suspension rays ignore trigger-zone
    // sensors (else the wheels ride up on an invisible waypoint sphere) and
    // celestial body spheres (a planet-sized collider that CONTAINS the scene
    // returns distance 0 — see `NON_PHYSICAL_QUERY_LAYERS`).
    let filter = avian3d::prelude::SpatialQueryFilter::from_mask(avian3d::prelude::LayerMask(
        !lunco_core::NON_PHYSICAL_QUERY_LAYERS,
    ));
    ray_caster = ray_caster.with_query_filter(filter);

    // avian's `update_ray_caster_positions` derives the ray's global origin from
    // the entity's own `Position`/`Rotation` when present, and ONLY falls back to
    // its `GlobalTransform` when they're absent. Without them the wheel casts from
    // its big_space RENDER-frame `GlobalTransform` (origin-relative, ≈ −53 m at a
    // 1945 m site) while the terrain collider lives in the grid-ABSOLUTE physics
    // frame (≈ +1945 m) — a ~2 km divergence that makes the ray miss the ground,
    // so `last_normal_force` stays 0 and `apply_wheel_drive` bails on its
    // `normal_force < 1.0` gate: the rover rests on its chassis collider but never
    // drives. Near the origin (flat sandbox) the two frames coincide and it works,
    // which is exactly the sandbox-vs-moonbase split. Carrying explicit
    // `Position`/`Rotation` (kept grid-absolute by `sync_raycast_wheel_physics_pose`
    // in `lunco-mobility`) makes the ray originate in the physics frame everywhere.
    // The wheel has no `RigidBody`/`Collider`, so avian's `position_to_transform`
    // never writes them back and the big_space bridge (BridgeShadow-gated) ignores
    // it — the mobility sync is the sole writer.
    commands.entity(entity).try_insert((
        wheel,
        order_key,
        body_mount,
        Suspension {
            rest_length: susp.rest_length,
            spring_k: susp.spring_k,
            damping_c: susp.damping_c,
            local_axis: DVec3::Y,
        },
        ray_caster,
        RayHits::default(),
        wheel_tf,
        avian3d::prelude::Position::default(),
        avian3d::prelude::Rotation::default(),
    ));
    // Remove any physics components added by the Avian plugin
    // (raycast wheels are not physical rigid bodies)
    commands
        .entity(entity)
        .remove::<Collider>()
        .remove::<RigidBody>()
        .remove::<Mass>();
}

/// Finds suspension visuals that USD authored below a wheel. Physical wheels
/// spin as rigid bodies, so these visuals must be moved to the wheel's carrier
/// before the wheel body is allowed to rotate. Their local transforms are
/// converted from wheel-local to carrier-local while preserving the authored
/// composed stage hierarchy.
fn physical_suspension_visuals(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    wheel_path: &UsdPrimPath,
    wheel_entity: Entity,
    wheel_tf: Transform,
    all_prims: &Query<(
        Entity,
        &UsdPrimPath,
        Option<&Transform>,
        Option<&UsdInstanceProjection>,
    )>,
    q_child_of: &Query<&ChildOf>,
) -> Vec<(Entity, Transform)> {
    let mut visuals = Vec::new();
    for (child, child_path, maybe_child_tf, _) in all_prims.iter() {
        if child == wheel_entity || child_path.stage_handle != wheel_path.stage_handle {
            continue;
        }
        let Ok(child_of) = q_child_of.get(child) else {
            continue;
        };
        if child_of.parent() != wheel_entity {
            continue;
        }
        let Ok(sdf_child_path) = SdfPath::new(&child_path.path) else {
            continue;
        };
        if !reader.has_api_schema(&sdf_child_path, "LunCoSuspensionVisualAPI") {
            continue;
        }
        let Some(role) = reader.text(&sdf_child_path, "lunco:suspensionVisual:role") else {
            continue;
        };
        if !matches!(role.as_str(), "casing" | "piston" | "spring") {
            continue;
        }
        let Some(child_tf) = maybe_child_tf.copied() else {
            continue;
        };
        visuals.push((child, wheel_tf.mul_transform(child_tf)));
    }
    visuals
}

/// Sets up a wheel as a full rigid body bound to its authored carrier by a
/// revolute joint, mirroring the standard `PhysicsRevoluteJoint` authored in USD.
///
/// The joint is spawned **synchronously** from the authored USD attributes
/// (`physics:axis`, `physics:localPos0/1`) alongside the wheel's rigid-body
/// init; drive authority comes from the composed motor/gearbox. Doing it lazily — letting
/// `lunco-usd-avian::build_usd_physics_joints` do it on a later frame —
/// raced narrow-phase contacts: the wheel's collider would meet the chassis
/// at the joint anchor before `JointCollisionDisabled` was in place,
/// crashing the Avian solver with "Head contact has no island".
/// `lunco-usd-avian` skips wheel-targeted joints (see `on_add_usd_prim`)
/// so we don't double-build.
fn setup_physical_wheel(
    commands: &mut Commands,
    entity: Entity,
    prim_path: &UsdPrimPath,
    order_key: String,
    existing_tf: &Transform,
    maybe_mesh: Option<&Mesh3d>,
    maybe_mat: Option<&PbrLook>,
    maybe_shader_mat: Option<&ShaderLook>,
    mesh_pending: bool,
    shader_bound: bool,
    suspension_visuals: Vec<(Entity, Transform)>,
    params: &WheelParams,
    body_mount: lunco_mobility::WheelBodyMount,
    vehicle_mount: Transform,
    p_drive: Entity,
    p_speed: Entity,
    authored_collider: Collider,
) {
    info!("Setting up PHYSICAL wheel {}", prim_path.path);
    let radius = params.radius as f32;

    // `params.peak_torque` (N·m at full throttle) is the composed motor/gearbox
    // axle torque, the SAME drive authority the raycast wheel uses — NOT the joint's
    // `drive:angular:physics:maxForce`. That joint attribute is a PhysX
    // joint-drive *saturation* limit (authored at 12000 in the demo scenes);
    // feeding it straight into the motor made the rover apply ~30× its lunar
    // weight in traction at full throttle and wheelie/launch on every forward
    // input. Using the motor/gearbox reduction keeps joint and raycast rovers
    // consistent. See `project_physical_rover_suspension`.

    // The wheel body keeps **identity rotation**. USD's authored cylinder axis
    // is the physical axle; Avian's primitive cylinder is conventionally +Y, so
    // rotate only the collider/inertia frame onto that axis. This is also the
    // exact axis passed to the revolute joint and tire torque law. The visible
    // mesh already carries its UsdGeomCylinder axis in generated vertices and
    // therefore keeps the authored prim rotation independently.
    let axle_local = params.axle_axis.normalize_or_zero();
    let wheel_axis_rot = Quat::from_rotation_arc(Vec3::Y, axle_local.as_vec3());
    let visual_axis_rot = existing_tf.rotation;
    let wheel_tf = Transform {
        translation: existing_tf.translation,
        rotation: Quat::IDENTITY,
        scale: existing_tf.scale,
    };

    // The specialized wheel owns the rigid-body/joint realization, but the
    // collision shape remains the one authored on the USD wheel prim. Avian
    // receives this projection from `lunco-usd-avian`; dynamics parameters never
    // become an implicit collision fallback.
    let collider = authored_collider;
    // Visual mesh child id, captured so the client-proxy animator
    // (`animate_proxy_physical_wheels`) can author its rotation directly.
    let visual_id = spawn_wheel_visual(
        commands,
        entity,
        prim_path,
        Transform::from_rotation(visual_axis_rot),
        maybe_mesh,
        maybe_mat,
        maybe_shader_mat,
        mesh_pending,
        shader_bound,
    );

    commands
        .entity(entity)
        .remove::<WheelRaycast>()
        .remove::<RayCaster>()
        .remove::<RayHits>();

    commands.entity(entity).try_insert((
        (
            PhysicalWheel {
                visual_entity: visual_id,
                wheel_radius: radius,
                wheel_width: params.width as f32,
                axis_rot: wheel_axis_rot,
                spin_angle: 0.0,
                // Authored wheel offset in the vehicle frame. The physical wheel is
                // nested under its suspension carrier, so this is separate from
                // the carrier-local joint pose.
                mount_local: vehicle_mount.translation,
            },
            lunco_physics::PhysicsOrderKey(order_key.clone()),
            body_mount,
        ),
        // Rebuild the vehicle support contract after replacing its raycast
        // wheel with this collider-backed realization.
        lunco_mobility::RaycastSupportGeometryDirty,
        // The standard wheel mass is a body mass, independent of the authored
        // collision shape. `NoAutoMass` prevents child/shape changes from
        // silently replacing the USD value during Avian recomputation.
        avian3d::prelude::Mass(params.mass as f32),
        avian3d::prelude::NoAutoMass,
        // Publish the complete authored wheel inertia before the rigid-body
        // observer runs, so Avian never derives a transient tensor from the
        // collision shape.
        physical_wheel_angular_inertia(params, wheel_axis_rot),
        avian3d::prelude::NoAutoAngularInertia,
        RigidBody::Kinematic,
        ShouldBeDynamic,
        collider,
        // The shared tire model owns tangential wheel-ground force. The Avian
        // collision hook removes its generic tangent impulse for this body;
        // Avian still owns the normal contact constraint and the wheel joint.
        Friction::new(params.friction_mu),
        SharedTireContact,
        // BEARING DRAG IS AUTHORED, in the wheel's own units. Was
        // `AngularDamping(0.3)` — again a Rust constant, and again one the raycast
        // wheel does not share: its spin integrator subtracts
        // `physxVehicleWheel:dampingRate · ω` (N·m·s, 0.45) from the axle torque.
        // avian's `AngularDamping` is not a torque coefficient but a per-second
        // decay applied to ω, i.e. τ ≈ d·I·ω, so the authored N·m·s converts as
        // `d = dampingRate / I_axle`; the live authored inertia is the complete
        // wheel assembly. One authored number, two realizations, each in its
        // own units.
        //
        // `LinearDamping(0.1)` is GONE with no replacement. A wheel hinged to the
        // chassis travels at the chassis's speed, so a linear damper on it was a
        // second, unauthored aerodynamic-style drag on the vehicle (≈22 N at
        // cruise) that the raycast rover — whose wheels are not bodies — could not
        // have. Rolling drag is the bearing term above; there is no air on the Moon.
        // `WheelParams::read` rejects every non-finite/non-positive wheel input,
        // so the authored inertia is already finite and positive here. Do not
        // hide an invalid projection behind a numerical floor.
        AngularDamping(params.bearing_damping / params.axle_inertia()),
        // Continuous collision detection: a thin, fast-falling wheel cylinder can
        // pass THROUGH the one-sided terrain heightfield in a single step (and once
        // below a one-sided surface, no contact ever pushes it back — it falls
        // forever). CCD sweeps the wheel's motion against the collider so it can
        // never tunnel, even across a one-frame collider-warmup gap. This is what
        // lets the tunnel-rescue safety net be deleted — the wheel physically
        // cannot end up below the terrain.
        // (Non-linear by default. `SweptCcd::LINEAR` was tried while hunting the
        // parity gap — the rotational sweep runs every substep on a permanently
        // spinning wheel — and measured as a byte-identical no-op, so the stronger
        // anti-tunneling guard stays.)
        avian3d::prelude::SweptCcd::default(),
        wheel_tf,
    ));

    // Spawn the avian joint. Anchors + axis are derived from the wheel's
    // own transform (which mirrors the USD `physics:localPos0` and
    // `physics:axis` of the authored joint, by construction). Reading
    // them straight from the USD joint prim caused `physics:axis` parse
    // mismatches in earlier iterations; the wheel-derived form has been
    // verified working for both raycast and joint-based rovers.
    let carrier = body_mount.body;
    // The wheel body rotates about its axle. Keep the authored suspension strut
    // on the carrier so its casing, piston, and spring remain visually
    // connected to the mount instead of spinning with the tire. The transforms
    // were converted from wheel-local to carrier-local by the caller.
    for (visual, transform) in suspension_visuals {
        commands
            .entity(visual)
            .try_insert((transform, ChildOf(carrier)));
    }
    // NOTE: `ArticulatedVehicle` (the articulated-root guard) is no longer stamped
    // here. It is derived declaratively from the USD joint graph in
    // `process_usd_sim_prims` (a prim that is a joint `physics:body0` target, or
    // carries `PhysicsArticulationRootAPI`) — see USD_REPLICATION_POLICY.md. That
    // removes this build-order side-effect (the membership pass used to depend on it).
    // Wheel mount point in the carrier-local frame. The vehicle-level mount is
    // authored once on the carrier; the revolute joint uses the wheel's
    // composed pose relative to that carrier.
    let mount_local = body_mount.local.translation.as_dvec3();
    // Axle direction — the same line the drive torque acts about. It is authored
    // in the wheel/carrier frame and is also the hub→wheel revolute axis.
    let axle = axle_local;
    // Hinge the wheel to the authored carrier. Steering, where present, is a
    // separate authored revolute joint on the carrier and is projected by the
    // generic USD/Avian joint path; this wheel joint owns only roll torque.
    let joint_cmd = commands.spawn((
        // GENERAL LIFECYCLE CONTRACT — every entity the USD build *synthesizes* to back a
        // scene (avian joints, actuator ports, cosim wires) is parented into the grid
        // subtree via `ChildOf`, so the ONE hierarchy-recursive `clear_scene_entities`
        // reclaims it exactly once, in the same flush as its bodies. Authored joints (any
        // depth of `Physics*Joint` prim in a robot arm / lander / crane) already satisfy
        // this — they ARE prim entities under the scene. This is the *synthesized* joint,
        // the only one not authored, so it is the one that must opt in explicitly here.
        //
        // A wheel joint links two bodies, so it sits in nobody's TRANSFORM subtree — but
        // it must die WITH the rover. `ChildOf` puts it in the carrier's despawn subtree;
        // avian resolves the constraint from the joint's body anchors, never from this
        // entity's transform, so the parenting is physics-inert. Left detached, the joint
        // outlived its bodies on a scene swap and was double-removed from avian's island
        // bookkeeping — a `joint_count` underflow that corrupted the solver. Owning it here
        // makes that structurally impossible: no orphans, no reaper, no mask.
        ChildOf(carrier),
        ScenePhysicsOwned,
        // Avian writes the solved revolute reaction here. The editor's wheel
        // gizmo reads this explicit boundary; it must not infer a per-wheel
        // force from the body's integration accumulator.
        JointForces::new(),
        // The solved mechanical network publishes physical shaft torque on the
        // wheel drive port. The generic co-simulation boundary applies that
        // scalar across this revolute joint; it never derives torque from a
        // command or from wheel speed.
        JointTorqueActuator {
            port_entity: p_drive,
            speed_port_entity: p_speed,
            brake_torque: params.brake_torque_max,
            rotational_inertia: params.axle_inertia(),
            // The wheel hinge is authored about +X. Negative +X rotation is the
            // demand-positive rolling sense for a chassis-forward -Z wheel;
            // this is the convention used by both the Avian motor and shared
            // tire solve.
            drive_sign: -1.0,
        },
        Name::new(format!("PhysicalWheelJoint_{}", prim_path.path)),
    ));
    let joint_entity = joint_cmd.id();
    // Project the complete authored tire contract onto the physical wheel.
    // `lunco-mobility` consumes this in the same fixed-step force system as the
    // raycast wheel; there is no physical-only lateral coefficient.
    commands.entity(entity).try_insert(JointedWheelTire {
        drive_joint: joint_entity,
        radius: params.radius,
        axle_inertia: params.axle_inertia(),
        slip_stiffness: params.slip_stiffness,
        lateral_stiffness_graph: params.lateral_stiffness_graph,
        min_validated_speed: params.min_validated_speed,
        friction_mu: params.friction_mu,
        bearing_damping: params.bearing_damping,
        heading_local: VehicleFrame::forward(GridRot(existing_tf.rotation.as_dquat())),
    });

    // The constraint itself goes through the ONE door every joint in the
    // workspace uses. `attach_joint` takes the two BODIES, so it — not this call
    // site — decides WHEN the joint may enter avian's graph (both bodies admitted
    // to the island graph) and WHAT rides its bundle (`JointCollisionDisabled`).
    // Inserting a joint component here directly is what "Neither body … is in an
    // island" was: the wheel and its carrier are spawned by this very pass, so on
    // a scene swap they are routinely not yet admitted at this exact moment.
    lunco_usd_avian_joints::attach_joint(
        commands,
        joint_entity,
        order_key,
        carrier,
        entity,
        lunco_usd_avian_joints::wheel_revolute_joint(carrier, entity, mount_local, axle),
    );

    // The wheel's `WheelBodyMount` is the canonical physics ownership boundary.
    // `ChildOf` remains the authored transform/despawn hierarchy; it is not used
    // to infer which body receives wheel torque, suspension, or mass.
}

/// Put an authored wheel shape into Avian's conventional +Y cylinder frame.
/// USD's `Cylinder.axis` is read from the same wheel parameter record that
/// drives the revolute joint and the visual projection. The shape itself still
/// comes exclusively from `PhysicsCollisionAPI` geometry.
pub(crate) fn oriented_wheel_collider(collider: Collider, axle_axis: DVec3) -> Collider {
    let axis = axle_axis.normalize_or_zero();
    let axis_rotation = Quat::from_rotation_arc(Vec3::Y, axis.as_vec3());
    if axis_rotation.abs_diff_eq(Quat::IDENTITY, 1e-5) {
        collider
    } else {
        Collider::compound(vec![(
            Position(DVec3::ZERO),
            Rotation(axis_rotation.as_dquat()),
            collider,
        )])
    }
}

/// Build the physical wheel's authored inertia tensor in the entity's local
/// frame.  `WheelParams::axle_inertia` is the corresponding scalar used by the
/// raycast integrator; keeping this conversion here makes the two realizations
/// consume the same authored complete assembly MOI.
pub(crate) fn physical_wheel_angular_inertia(
    params: &WheelParams,
    wheel_axis_rot: Quat,
) -> avian3d::prelude::AngularInertia {
    let m = params.mass;
    let r = params.radius;
    let i_perp = m * (3.0 * r * r + (params.width * params.width)) / 12.0;
    avian3d::prelude::AngularInertia {
        principal: bevy::math::Vec3::new(
            i_perp as f32,
            params.axle_inertia() as f32,
            i_perp as f32,
        ),
        local_frame: wheel_axis_rot,
    }
}

/// Client-only: place a remote rover's wheels by **reconstructing** them from the
/// chassis instead of replicating their poses over the wire.
///
/// The authored vehicle mount is constant (`mount_local`) and its only locally
/// reconstructed motion is cosmetic axle-spin (handled visually by
/// `animate_proxy_physical_wheels`). So a remote rover can replicate **only its
/// chassis**; each wheel is a kinematic follower at `mount_local`. This puts
/// the wheel collider in the right place for contact (the original "free wheel collider"
/// bug) at ~zero wire cost — no per-wheel snapshot.
///
/// Runs only on a **client**, only for wheels whose chassis is a **kinematic proxy**
/// (a remote rover); the host and the rover this client owns run real local wheel
/// physics (Dynamic + joint + motor). A kinematic child body's world pose is not
/// auto-derived from its parent, so it must be driven every tick or it freezes in world
/// World pose of a proxy wheel: the chassis pose composed with the
/// authored vehicle mount offset. Returns `(position, rotation)`; the
/// rotation is normalized.
///
/// Pure extract of the pose math in [`reconstruct_proxy_wheels`].
fn proxy_wheel_pose(chassis_pos: DVec3, chassis_rot: DQuat, mount_local: DVec3) -> (DVec3, DQuat) {
    let pos = chassis_pos + chassis_rot * mount_local;
    let rot = chassis_rot.normalize();
    (pos, rot)
}

fn reconstruct_proxy_wheels(
    // Optional: with no network context (standalone / a minimal test harness that
    // ticks the fixed schedule without the full core plugin) there are no
    // replicated proxies to reconstruct, so no-op instead of panicking on a missing
    // resource. Only `NetworkRole::Client` does work here anyway.
    role: Option<Res<lunco_core_session::NetworkRole>>,
    q_chassis: Query<
        (&RigidBody, &Position, &Rotation),
        (With<lunco_core::MobilityRoot>, Without<PhysicalWheel>),
    >,
    q_bodies: Query<(&RigidBody, &Position, &Rotation), Without<PhysicalWheel>>,
    mut q_wheels: Query<
        (
            Entity,
            &PhysicalWheel,
            &lunco_mobility::WheelBodyMount,
            &RigidBody,
            &mut Position,
            &mut Rotation,
        ),
        Without<lunco_core_session::OwnedLocally>,
    >,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Some(role) = role else { return };
    if !matches!(*role, lunco_core_session::NetworkRole::Client) {
        return;
    }
    for (e, wheel, mount, rb, mut pos, mut rot) in q_wheels.iter_mut() {
        let Ok((owner_rb, _, _)) = q_bodies.get(mount.body) else {
            continue;
        };
        if !matches!(owner_rb, RigidBody::Kinematic) {
            continue; // host / owned rover — real local wheel physics
        }
        let mut cursor = mount.body;
        let root = loop {
            if let Ok(root) = q_chassis.get(cursor) {
                break Some(root);
            }
            let Some(parent) = q_parents.get(cursor).ok().map(ChildOf::parent) else {
                break None;
            };
            cursor = parent;
        };
        let Some((c_rb, c_pos, c_rot)) = root else {
            continue;
        };
        if !matches!(c_rb, RigidBody::Kinematic) {
            continue;
        }
        if !matches!(rb, RigidBody::Kinematic) {
            commands.entity(e).try_insert(RigidBody::Kinematic);
        }
        // World pose at the rigid mount offset. The cylinder
        // collider (axis baked into its compound) lands correctly for contact; the
        // visual child's spin is layered on by `animate_proxy_physical_wheels`.
        let (p, q) = proxy_wheel_pose(c_pos.0, c_rot.0, wheel.mount_local.as_dvec3());
        pos.0 = p;
        rot.0 = q;
    }
}

/// Spin a joint-wheel's visual on a replicated proxy when the wheel body itself
/// is not per-link replicated.
///
/// With full articulated per-link replication
/// (wheels carry `NetReplicate`, applied by `apply_net_replication`) the wheel **body** carries
/// the host's true world rotation and the visual child (`ChildOf(wheel)`) inherits
/// it — so this system would *double-apply* spin. It therefore skips
/// `With<NetReplicate>` wheels (`Without<NetReplicate>` below) and only animates any
/// wheel that lacks per-link replication.
///
/// On a client proxy the chassis is kinematic and the motor is held at zero, so
/// the visual roll is derived from the authoritative [`ReplicatedChassisMotion`]
/// and the wheel's authored mount.
///
/// Guarded to a **kinematic** chassis so it is a no-op on the host/owned rover and
/// never fights the joint-driven body there.
fn animate_proxy_physical_wheels(
    // `Without<NetReplicate>`: replicated
    // wheels carry their own spin via the body's world rotation, so skip them (see docstring).
    mut q_wheels: Query<
        (
            &mut PhysicalWheel,
            &Rotation,
            &lunco_mobility::WheelBodyMount,
        ),
        Without<lunco_core_session::NetReplicate>,
    >,
    q_chassis: Query<
        (
            &RigidBody,
            &Position,
            &Rotation,
            &ComputedCenterOfMass,
            Option<&lunco_core_session::ReplicatedChassisMotion>,
        ),
        (With<lunco_core::MobilityRoot>, Without<PhysicalWheel>),
    >,
    q_bodies: Query<(&RigidBody, &Position, &Rotation), Without<PhysicalWheel>>,
    q_parents: Query<&ChildOf>,
    mut q_visual: Query<&mut Transform, Without<PhysicalWheel>>,
    time: Res<Time>,
) {
    use std::f64::consts::TAU;
    // Sign mapping rolling speed → roll about the axle so the contact patch
    // tracks the ground (matches the host's solved torque-driven body spin). Mirrors
    // the `drive_sign = -1` axle convention used by `JointTorqueActuator`.
    const ROLL_SIGN: f64 = -1.0;

    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }

    for (mut wheel, wheel_rot, mount) in q_wheels.iter_mut() {
        let Ok((owner, _, _)) = q_bodies.get(mount.body) else {
            continue;
        };
        if !matches!(owner, RigidBody::Kinematic) {
            continue;
        }
        let mut cursor = mount.body;
        let root = loop {
            if let Ok(root) = q_chassis.get(cursor) {
                break Some(root);
            }
            let Some(parent) = q_parents.get(cursor).ok().map(ChildOf::parent) else {
                break None;
            };
            cursor = parent;
        };
        let Some((_body, pos, rot, center_of_mass, motion)) = root else {
            continue;
        };
        // Chassis velocity arrives via the delivered hint (the proxy's avian
        // velocity is force-zeroed). Ground speed of the hub along the wheel's
        // forward axis → rolling rate ω = v_long / r.
        let Some(motion) = motion else {
            // A proxy without a delivered chassis-motion sample has no
            // authoritative rolling input. Leave its visual state untouched
            // until the replication boundary supplies one; inventing a zero
            // velocity here masks a broken transport and can make a stopped
            // wheel look like a valid simulation state.
            continue;
        };
        let (vlin, vang) = (motion.lin, motion.ang);
        // Reconstruct the hub in the Avian cell-local frame from the chassis pose +
        // the authored vehicle mount offset, exactly as
        // `proxy_wheel_pose`/`reconstruct_proxy_wheels` do. The old code read
        // `GlobalTransform` (big_space render frame) in this physics calculation.
        // The wheel's Avian rotation is authoritative and already includes proxy
        // steering, so no render projection crosses this boundary.
        let chassis_pos = GridPos(pos.0);
        let (hub_pos, _) = wheel_hub_pose(
            chassis_pos,
            GridRot(rot.0),
            wheel.mount_local.as_dvec3(),
            DQuat::IDENTITY,
        );
        let hub_vel = body_point_velocity(
            vlin,
            vang,
            hub_pos,
            chassis_pos,
            GridRot(rot.0),
            center_of_mass.0,
        );
        let forward = VehicleFrame::forward(GridRot(wheel_rot.0));
        let Some(w) = wheel_roll_rate(hub_vel, forward, wheel.wheel_radius as f64) else {
            continue;
        };

        let angle = (wheel.spin_angle as f64 + ROLL_SIGN * w * dt).rem_euclid(TAU);
        wheel.spin_angle = angle as f32;

        if let Some(visual_entity) = wheel.visual_entity {
            if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                // Roll about the wheel's axle (`axis_rot · Y`), composed over the
                // cylinder base — reconstructs the host's `body_spin · axis_rot`.
                let axle = (wheel.axis_rot * Vec3::Y).normalize();
                let rotation =
                    (Quat::from_axis_angle(axle, wheel.spin_angle) * wheel.axis_rot).normalize();
                if visual_tf.rotation != rotation {
                    visual_tf.rotation = rotation;
                }
            }
        }
    }
}

/// Allow a live prim to be projected again after its composed simulation
/// schemas change.
///
/// A runtime reference can initially arrive as a typeless visual root while
/// its referenced layer closure is still loading.  The sim projector marks
/// that root processed, so a later schema resync must clear the marker before
/// the normal projection pass can publish its authored control surface,
/// wheel wiring, or other simulation components.
fn invalidate_usd_sim_projection(world: &mut World, entity: Entity) -> bool {
    if world.get::<lunco_core::MobilityRoot>(entity).is_some() {
        return false;
    }
    if world.get::<UsdSimProcessed>(entity).is_none() {
        return false;
    }
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return false;
    };
    entity_mut.remove::<UsdSimProcessed>();
    true
}

fn install_authored_sun_state_seed(app: &mut App) {
    app.add_systems(
        PostUpdate,
        seed_authored_sun_state
            .after(TransformSystems::Propagate)
            .before(lunco_environment::finalize_sun_render_state),
    );
}

/// Avoids marking `EnvironmentDirections` changed on every update when the
/// celestial source already owns the sun direction.
fn clear_authored_sun_direction_if_present(
    directions: &mut ResMut<lunco_environment::EnvironmentDirections>,
) {
    if directions
        .get_named(lunco_environment::SUN_DIRECTION_SOURCE)
        .is_some()
    {
        directions.set_named(lunco_environment::SUN_DIRECTION_SOURCE, None);
    }
}

/// Select the static authored-light source only after the active scene root's
/// celestial projection confirms that no celestial source owns it. A celestial
/// source is authoritative even when its declaration or ephemeris is invalid;
/// in that case the `sun` direction source stays empty and the cosim projection
/// reports missing solar data instead of restoring a stale authored direction.
///
/// The static scene contract is one unscoped authored DistantLight below the
/// active USD scene root. Its composed rotation and illuminance define the
/// constant solar direction for manual/static scenes such as the sun tracker.
fn seed_authored_sun_state(
    sun_state: Option<ResMut<lunco_environment::SunState>>,
    directions: Option<ResMut<lunco_environment::EnvironmentDirections>>,
    mut authored_seed_revision: Local<Option<u64>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    scene_mount: Option<Res<lunco_core::SceneMountState>>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_scene_roots: Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    q_projected_roots: Query<
        (),
        (
            With<lunco_usd_bevy_scene::UsdSceneRoot>,
            With<lunco_usd_sim_celestial::CelestialProjected>,
        ),
    >,
    q_source_classifications: Query<
        &lunco_environment::CelestialSourceClassification,
        With<lunco_usd_bevy_scene::UsdSceneRoot>,
    >,
    q_parents: Query<&ChildOf>,
    q_entities: Query<Entity>,
    q_suns: Query<
        (Entity, &bevy::light::DirectionalLight),
        (
            With<lunco_usd_bevy_light::light::UsdAuthoredLight>,
            Without<lunco_environment::Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
) {
    let Some(mut sun_state) = sun_state else {
        return;
    };
    let Some(mut directions) = directions else {
        return;
    };
    let Some(active_root) = scene_mount.as_deref().and_then(|mount| mount.active_root()) else {
        *authored_seed_revision = None;
        return;
    };
    if q_scene_roots.get(active_root).is_err() || q_projected_roots.get(active_root).is_err() {
        return;
    }
    let Ok(source_classification) = q_source_classifications.get(active_root) else {
        return;
    };
    if source_classification.has_source {
        clear_authored_sun_direction_if_present(&mut directions);
        if authored_seed_revision
            .take()
            .is_some_and(|revision| sun_state.revision == revision)
        {
            sun_state.clear();
        }
        return;
    }
    if directions
        .get_named(lunco_environment::SUN_DIRECTION_SOURCE)
        .is_some()
    {
        return;
    }

    let mut authored_sun_entity = None;
    let mut authored_sun_count = 0;
    for (entity, _) in &q_suns {
        if lunco_usd_bevy_scene::scene_root_ancestor(
            entity,
            &q_scene_roots,
            &q_parents,
            &q_entities,
        ) == Ok(Some(active_root))
        {
            authored_sun_count += 1;
            authored_sun_entity = Some(entity);
        }
    }
    if authored_sun_count != 1 {
        return;
    }
    let Some(active_frame) = active_frame else {
        return;
    };
    let Some(authored_sun_entity) = authored_sun_entity else {
        return;
    };
    let Ok((_, light)) = q_suns.get(authored_sun_entity) else {
        return;
    };
    let Some((_, light_rotation_in_frame)) = lunco_spatial::coords::pose_in_grid(
        authored_sun_entity,
        active_frame.0,
        &q_parents,
        &q_grids,
        &q_spatial,
    ) else {
        return;
    };
    let Some(direction_to_sun) = lunco_spatial::coords::UnitDirection3::normalized(
        -(light_rotation_in_frame * bevy::math::DVec3::NEG_Z),
    ) else {
        return;
    };
    directions.set_named(
        lunco_environment::SUN_DIRECTION_SOURCE,
        Some(lunco_environment::FramedDirection {
            frame: active_frame.0,
            direction: direction_to_sun,
        }),
    );
    sun_state.publish(Some(light.illuminance));
    *authored_seed_revision = Some(sun_state.revision);
}

#[cfg(test)]
mod authored_sun_direction_tests {
    use super::*;

    #[derive(Resource, Default)]
    struct DirectionChangeObserved(bool);

    fn clear_authored_sun_direction(
        mut directions: ResMut<lunco_environment::EnvironmentDirections>,
    ) {
        super::clear_authored_sun_direction_if_present(&mut directions);
    }

    fn seed_authored_sun_direction(
        mut directions: ResMut<lunco_environment::EnvironmentDirections>,
    ) {
        let direction = lunco_spatial::coords::UnitDirection3::normalized(bevy::math::DVec3::NEG_Z)
            .expect("negative Z is a valid direction");
        directions.set_named(
            lunco_environment::SUN_DIRECTION_SOURCE,
            Some(lunco_environment::FramedDirection {
                frame: Entity::PLACEHOLDER,
                direction,
            }),
        );
    }

    fn record_direction_change(
        directions: Res<lunco_environment::EnvironmentDirections>,
        mut observed: ResMut<DirectionChangeObserved>,
    ) {
        observed.0 = directions.is_changed();
    }

    #[test]
    fn clearing_an_absent_sun_ray_does_not_mark_directions_changed() {
        let mut app = App::new();
        app.init_resource::<lunco_environment::EnvironmentDirections>()
            .init_resource::<DirectionChangeObserved>()
            .add_systems(
                Update,
                (clear_authored_sun_direction, record_direction_change).chain(),
            );

        app.update();
        app.world_mut().resource_mut::<DirectionChangeObserved>().0 = false;
        app.update();

        assert!(
            app.world()
                .resource::<lunco_environment::EnvironmentDirections>()
                .get_named(lunco_environment::SUN_DIRECTION_SOURCE)
                .is_none()
        );
        assert!(!app.world().resource::<DirectionChangeObserved>().0);
    }

    #[test]
    fn clearing_a_present_sun_ray_removes_it() {
        let mut app = App::new();
        app.init_resource::<lunco_environment::EnvironmentDirections>()
            .init_resource::<DirectionChangeObserved>()
            .add_systems(
                Update,
                (
                    seed_authored_sun_direction,
                    clear_authored_sun_direction,
                    record_direction_change,
                )
                    .chain(),
            );

        app.update();

        assert!(
            app.world()
                .resource::<lunco_environment::EnvironmentDirections>()
                .get_named(lunco_environment::SUN_DIRECTION_SOURCE)
                .is_none()
        );
    }
}

/// Queue prim identity before its render projection becomes available.
fn queue_added_usd_sim_prim(
    trigger: On<Add, UsdPrimPath>,
    unprocessed: Query<(), Without<UsdSimProcessed>>,
    mut pending: ResMut<PendingUsdSimPrimWork>,
) {
    if unprocessed.contains(trigger.entity) {
        pending.0.queue(trigger.entity);
    }
}

/// The visual projection marker is the readiness boundary for simulation
/// projection, so its arrival reopens work even if the path arrived earlier.
fn queue_projected_usd_sim_prim(
    trigger: On<Add, lunco_usd_bevy_scene::UsdSceneProjected>,
    eligible: Query<(), (With<UsdPrimPath>, Without<UsdSimProcessed>)>,
    mut pending: ResMut<PendingUsdSimPrimWork>,
) {
    if eligible.contains(trigger.entity) {
        pending.0.queue(trigger.entity);
    }
}

fn forget_removed_usd_sim_prim(
    trigger: On<Remove, UsdPrimPath>,
    mut pending: ResMut<PendingUsdSimPrimWork>,
) {
    pending.0.forget(trigger.entity);
}

fn forget_unprojected_usd_sim_prim(
    trigger: On<Remove, lunco_usd_bevy_scene::UsdSceneProjected>,
    mut pending: ResMut<PendingUsdSimPrimWork>,
) {
    pending.0.forget(trigger.entity);
}

/// Live USD edits invalidate the processed marker; queue that prim again only
/// while it still has the visual projection required by this owner.
fn queue_invalidated_usd_sim_prim(
    trigger: On<Remove, UsdSimProcessed>,
    eligible: Query<
        (),
        (
            With<UsdPrimPath>,
            With<lunco_usd_bevy_scene::UsdSceneProjected>,
        ),
    >,
    mut pending: ResMut<PendingUsdSimPrimWork>,
) {
    if eligible.contains(trigger.entity) {
        pending.0.queue(trigger.entity);
    }
}

fn reset_usd_sim_prim_work(mut pending: ResMut<PendingUsdSimPrimWork>) {
    pending.0.clear();
    pending.1.clear();
}

#[cfg(test)]
mod pending_sim_work_tests {
    use super::*;

    #[test]
    fn bounded_prefix_orders_equal_prim_paths_by_stable_stage_source() {
        let lower_entity = Entity::from_bits(2);
        let higher_entity = Entity::from_bits(900);
        let first_stage = StableUsdSimWork {
            stage_source: "twin://a/scene.usda".to_owned(),
            prim_path: "/World/Rover".to_owned(),
            item: (higher_entity, "first"),
        };
        let second_stage = StableUsdSimWork {
            stage_source: "twin://b/scene.usda".to_owned(),
            prim_path: "/World/Rover".to_owned(),
            item: (lower_entity, "second"),
        };

        for candidates in [
            vec![first_stage.clone(), second_stage.clone()],
            vec![second_stage.clone(), first_stage.clone()],
        ] {
            let (selected, deferred, _) = select_bounded_sim_prim_work(
                candidates,
                1,
                |candidate| candidate.item.0,
                compare_stable_usd_sim_work,
                |_| false,
            );
            assert_eq!(selected[0].item.1, "first");
            assert_eq!(deferred, [lower_entity]);
        }
    }

    #[test]
    fn duplicate_stage_and_prim_identity_is_rejected_before_selection() {
        let candidates = [1, 2]
            .into_iter()
            .map(|raw| StableUsdSimWork {
                stage_source: "twin://mission/scene.usda".to_owned(),
                prim_path: "/World/Rover".to_owned(),
                item: Entity::from_bits(raw),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            duplicate_nonpreview_usd_sim_work_keys(
                &candidates,
                |entity| *entity,
                |_| false,
                &mut HashMap::new(),
            ),
            HashSet::from([(
                "twin://mission/scene.usda".to_owned(),
                "/World/Rover".to_owned(),
            )])
        );
    }

    #[test]
    fn matching_preview_paths_do_not_make_the_live_identity_ambiguous() {
        let live = Entity::from_bits(1);
        let preview = Entity::from_bits(2);
        let candidates = [live, preview]
            .into_iter()
            .map(|entity| StableUsdSimWork {
                stage_source: "twin://mission/scene.usda".to_owned(),
                prim_path: "/World/Rover".to_owned(),
                item: entity,
            })
            .collect::<Vec<_>>();
        let mut preview_cache = HashMap::new();

        assert!(
            duplicate_nonpreview_usd_sim_work_keys(
                &candidates,
                |entity| *entity,
                |entity| entity == preview,
                &mut preview_cache,
            )
            .is_empty()
        );
        assert_eq!(preview_cache.get(&live), Some(&false));
        assert_eq!(preview_cache.get(&preview), Some(&true));
    }

    #[test]
    fn physics_order_identity_includes_stage_and_instance_scope() {
        let first =
            stable_usd_physics_order_key("twin://first/scene.usda", None, "/World/Rover/Wheel")
                .expect("stage-scoped physics key");
        let second =
            stable_usd_physics_order_key("twin://second/scene.usda", None, "/World/Rover/Wheel")
                .expect("other stage-scoped physics key");
        let instance = stable_usd_physics_order_key(
            "twin://first/scene.usda",
            Some("/World/Instances/RoverA"),
            "/World/Instances/RoverA/Wheel",
        )
        .expect("instance-scoped physics key");
        assert_ne!(first, second);
        assert_ne!(first, instance);
        assert!(stable_usd_physics_order_key("", None, "/World/Rover").is_err());
        assert!(stable_usd_physics_order_key("twin://first/scene.usda", None, "").is_err());
    }

    #[test]
    fn preview_prims_do_not_consume_bounded_simulation_admission() {
        let preview_entities = (0..40)
            .map(|rank| Entity::from_bits(rank + 1))
            .collect::<HashSet<_>>();
        let candidates = (0..40)
            .chain(100..105)
            .map(|rank| (Entity::from_bits(rank + 1), rank as usize))
            .collect();

        let (selected, deferred, preview_only) = select_bounded_sim_prim_work(
            candidates,
            2,
            |candidate| candidate.0,
            |left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)),
            |entity| preview_entities.contains(&entity),
        );

        assert_eq!(
            selected
                .into_iter()
                .map(|(_, rank)| rank)
                .collect::<Vec<_>>(),
            [100, 101],
        );
        assert_eq!(deferred.len(), 3);
        assert_eq!(preview_only.len(), 40);
    }

    #[test]
    fn bounded_admission_does_not_inspect_preview_rows_after_full_prefix() {
        let preview_entities = (2..42)
            .map(|rank| Entity::from_bits(rank + 1))
            .collect::<HashSet<_>>();
        let candidates = (0..42)
            .map(|rank| (Entity::from_bits(rank + 1), rank as usize))
            .collect();
        let preview_checks = std::cell::Cell::new(0);

        let (selected, deferred, previews) = select_bounded_sim_prim_work(
            candidates,
            2,
            |candidate| candidate.0,
            |left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)),
            |entity| {
                preview_checks.set(preview_checks.get() + 1);
                preview_entities.contains(&entity)
            },
        );

        assert_eq!(
            selected
                .into_iter()
                .map(|(_, rank)| rank)
                .collect::<Vec<_>>(),
            [0, 1],
        );
        assert_eq!(deferred.len(), 40);
        assert!(previews.is_empty());
        assert_eq!(preview_checks.get(), 2);
    }

    #[test]
    fn simulation_projection_work_tracks_readiness_and_invalidation_edges() {
        let mut app = App::new();
        app.init_resource::<PendingUsdSimPrimWork>();
        app.world_mut()
            .resource_mut::<PendingUsdSimPrimWork>()
            .0
            .take_initial_discovery();
        app.add_observer(queue_added_usd_sim_prim)
            .add_observer(queue_projected_usd_sim_prim)
            .add_observer(forget_removed_usd_sim_prim)
            .add_observer(forget_unprojected_usd_sim_prim)
            .add_observer(queue_invalidated_usd_sim_prim);

        let waiting_for_visuals = app.world_mut().spawn(UsdPrimPath::default()).id();
        assert!(
            app.world()
                .resource::<PendingUsdSimPrimWork>()
                .0
                .contains(waiting_for_visuals)
        );

        // Once a pass finds the path before visual projection, the visual
        // lifecycle edge must enqueue it again when the readiness boundary lands.
        app.world_mut()
            .resource_mut::<PendingUsdSimPrimWork>()
            .0
            .take_queued();
        app.world_mut()
            .entity_mut(waiting_for_visuals)
            .insert(lunco_usd_bevy_scene::UsdSceneProjected);
        assert!(
            app.world()
                .resource::<PendingUsdSimPrimWork>()
                .0
                .contains(waiting_for_visuals)
        );

        app.world_mut()
            .entity_mut(waiting_for_visuals)
            .remove::<UsdPrimPath>();
        assert!(
            !app.world()
                .resource::<PendingUsdSimPrimWork>()
                .0
                .contains(waiting_for_visuals)
        );

        let invalidated = app
            .world_mut()
            .spawn((
                UsdPrimPath::default(),
                lunco_usd_bevy_scene::UsdSceneProjected,
                UsdSimProcessed,
            ))
            .id();
        app.world_mut()
            .entity_mut(invalidated)
            .remove::<UsdSimProcessed>();
        assert!(
            app.world()
                .resource::<PendingUsdSimPrimWork>()
                .0
                .contains(invalidated)
        );
    }

    #[test]
    fn scene_teardown_clears_sim_projection_work() {
        let mut app = App::new();
        let mut pending = PendingUsdSimPrimWork::default();
        pending.0.queue(Entity::from_bits(1));
        app.insert_resource(pending)
            .add_systems(lunco_core::SceneTeardown, reset_usd_sim_prim_work);

        app.world_mut().run_schedule(lunco_core::SceneTeardown);

        assert!(!app.world().resource::<PendingUsdSimPrimWork>().0.has_work());
    }
}

/// Resolve an authored gear joint into a
/// [`DifferentialCoupling`] once every body it names is spawned and Avian-admitted
/// (the `With<Position>` gate, same as USD joints). Matches the authored prim-path
/// strings against live `UsdPrimPath`s, scoped by stage and instance root, so two
/// copies of the same rover in one scene each gear their OWN rockers.
///
/// The pending marker lives on the JOINT prim; the coupling is attached to the chassis,
/// which is the body the gear's reaction torque goes into and the one the coupling
/// system writes `Forces` through.
fn resolve_differential_coupling(
    q_pending: Query<(Entity, &UsdPrimPath, &PendingDifferential)>,
    q_bodies: Query<(Entity, &UsdPrimPath), With<Position>>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    q_instance_projection: Query<&UsdInstanceProjection>,
    mut commands: Commands,
) {
    for (joint, joint_path, pending) in q_pending.iter() {
        let joint_root = instance_key(
            joint,
            &q_provenance,
            &q_gid,
            &q_instance_root,
            &q_instance_projection,
        );
        let find = |target: &str| {
            q_bodies
                .iter()
                .find(|(e, p)| {
                    p.path == target
                        && p.stage_handle == joint_path.stage_handle
                        && instance_key(
                            *e,
                            &q_provenance,
                            &q_gid,
                            &q_instance_root,
                            &q_instance_projection,
                        ) == joint_root
                })
                .map(|(e, _)| e)
        };
        let (Some(chassis), Some(rocker_a), Some(rocker_b)) = (
            find(&pending.chassis),
            find(&pending.rocker_a),
            find(&pending.rocker_b),
        ) else {
            continue; // a geared body not admitted yet — retry next frame
        };
        commands.entity(chassis).try_insert(DifferentialCoupling {
            chassis,
            rocker_a,
            rocker_b,
            ratio: pending.ratio,
            rest_offset: pending.rest_offset,
            target_velocity: pending.target_velocity,
            stiffness: pending.stiffness,
            damping: pending.damping,
            max_force: pending.max_force,
            drive_type: pending.drive_type,
        });
        commands.entity(joint).remove::<PendingDifferential>();
        info!(
            "Resolved gear joint {} ({} <-> {})",
            joint_path.path, pending.rocker_a, pending.rocker_b
        );
    }
}

fn activate_dynamic_bodies(
    mut commands: Commands,
    ground_pending: Res<GroundColliderPending>,
    q_kinematic: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&AuthoredInitialVelocity>,
            Option<&avian3d::prelude::RigidBodyDisabled>,
        ),
        (
            With<ShouldBeDynamic>,
            Without<lunco_physics::PhysicsInitializationPending>,
            Without<lunco_physics::PhysicsInitializationInvalid>,
        ),
    >,
    q_pending_joints: Query<(Entity, &UsdPrimPath, &PendingUsdJoint), With<PendingUsdJoint>>,
    q_pending_admissions: Query<&PendingJointAdmission>,
    q_joint_states: Query<(
        Entity,
        &UsdPrimPath,
        Option<&PendingUsdJoint>,
        Option<&PendingJointAdmission>,
        Has<RevoluteJoint>,
        Has<PrismaticJoint>,
        Has<FixedJoint>,
        Has<SphericalJoint>,
        Has<DistanceJoint>,
    )>,
    q_pending_diffs: Query<(Entity, &UsdPrimPath), With<PendingDifferential>>,
    q_detached: Query<Option<&lunco_physics::PhysicsJointDetachSet>>,
    q_child_of: Query<&ChildOf>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    topology_index: Res<JointTopologyIndex>,
    mut binding_epoch: ResMut<lunco_cosim_core::BindingEpochDirty>,
) {
    // USD/Avian topology is built in the fixed schedule, while this admission
    // pass runs in Update. A body may not become dynamic until every authored
    // joint touching it has crossed both native boundaries:
    //
    //   USD schema -> typed PendingJoint -> Avian joint component
    //
    // The readiness hold pauses integration, not topology construction. The
    // joint builder and the outer Update admission system continue to run while
    // that hold is active, so waiting here cannot deadlock the scene. Promoting
    // first and hoping the parked constraint appears before the next solver tick
    // is precisely how an articulated pad escaped during warm-cache startup.
    let mut promoted = false;
    // Promotion is the final admission boundary before the first solver tick.
    // Keep it independent of ECS allocation order for the same reason as the
    // projection pass: async layer completion must not choose which rigid body
    // enters the native solver island first.
    let mut kinematic: Vec<_> = q_kinematic
        .iter()
        .filter(|(entity, ..)| !is_preview_only(*entity, &q_child_of, &q_preview_only))
        .collect();
    kinematic.sort_by(|left, right| left.1.path.cmp(&right.1.path));
    for (entity, path, authored_velocity, body_disabled) in kinematic {
        let has_pending_joint =
            q_pending_joints
                .iter()
                .any(|(joint_entity, joint_path, pending)| {
                    !is_preview_only(joint_entity, &q_child_of, &q_preview_only)
                        && joint_path.stage_handle == path.stage_handle
                        && (pending.body0_path == path.path || pending.body1_path == path.path)
                });
        let has_pending_admission = q_pending_admissions
            .iter()
            .any(|pending| pending.body0 == entity || pending.body1 == entity);
        let has_unready_authored_joint = topology_index
            .get(path.stage_handle.id())
            .and_then(|topology| {
                topology
                    .authored_joints
                    .iter()
                    .find_map(|(joint_path, (body0, body1))| {
                        if body0 != &path.path && body1 != &path.path {
                            return None;
                        }
                        if q_detached
                            .get(entity)
                            .ok()
                            .flatten()
                            .is_some_and(|detached| detached.contains(joint_path))
                        {
                            // A live DetachJoint deliberately invalidated this
                            // edge. The canonical stage still contains it until
                            // a persistent edit/reload, so it must not hold the
                            // released body at Kinematic forever.
                            return None;
                        }
                        let joint_ready = q_joint_states
                            .iter()
                            .find(|(joint_entity, joint, ..)| {
                                // Preview descendants can share a composed path with the live
                                // stage, but their joints are not part of its physics graph.
                                !is_preview_only(*joint_entity, &q_child_of, &q_preview_only)
                                    && joint.stage_handle == path.stage_handle
                                    && joint.path == *joint_path
                            })
                            .is_some_and(
                                |(
                                    _,
                                    _,
                                    pending_usd,
                                    pending_native,
                                    revolute,
                                    prismatic,
                                    fixed,
                                    spherical,
                                    distance,
                                )| {
                                    pending_usd.is_none()
                                        && pending_native.is_none()
                                        && (revolute || prismatic || fixed || spherical || distance)
                                },
                            );
                        (!joint_ready).then_some(())
                    })
            })
            .is_some();
        let has_pending_diff = q_pending_diffs.iter().any(|(entity, diff_path)| {
            !is_preview_only(entity, &q_child_of, &q_preview_only)
                && diff_path.stage_handle == path.stage_handle
        });
        // Readiness deliberately disables the body before the fixed physics
        // schedule can admit its island node. A native joint may therefore be
        // parked while this marker is present, but it must not keep the
        // authored body in `ShouldBeDynamic`: promotion while disabled is
        // inert, and release of the readiness marker then creates the island
        // node that `JointAdmission` needs. Outside that explicit freeze,
        // pending admission still blocks promotion so a live body can never
        // integrate before its constraint is installed.
        let blocked = ground_pending.0
            || has_pending_joint
            || (has_pending_admission && body_disabled.is_none())
            || has_unready_authored_joint
            || has_pending_diff;
        if !blocked {
            // Despawn-safe: scene-load churn / doc-backed reload can despawn a
            // ShouldBeDynamic entity between this queue and `apply_deferred`; a plain
            // `insert` then panics on the invalid entity. `try_insert`/`try_remove`
            // no-op at apply time if the entity is gone (a `get_entity` guard here
            // would not help — it only proves validity at queue time, not apply).
            // A kinematic loading body can carry a bridge-generated velocity from
            // the authored-pose/rebranch handoff. That is a render/pose transport
            // value, not a physical initial condition. Seed the dynamic body from
            // the only authoritative source: USD's explicitly authored velocity;
            // absent that, admission starts at rest.
            let linear = authored_velocity
                .and_then(|velocity| velocity.linear)
                .unwrap_or(DVec3::ZERO);
            let angular = authored_velocity
                .and_then(|velocity| velocity.angular)
                .unwrap_or(DVec3::ZERO);
            commands.entity(entity).try_insert((
                RigidBody::Dynamic,
                lunco_core::PhysicsStateReady,
                LinearVelocity(linear),
                AngularVelocity(angular),
            ));
            commands
                .entity(entity)
                .try_remove::<lunco_core::PhysicsStatePending>();
            commands
                .entity(entity)
                .try_remove::<AuthoredInitialVelocity>();
            commands.entity(entity).try_remove::<ShouldBeDynamic>();
            promoted = true;
        }
    }
    if promoted {
        // Promotion publishes the authored physical initial condition through
        // deferred component insertion. Reopen the co-sim binding epoch so the
        // next sealed pass seeds every already-valid sensor/actuator wire from
        // that finalized state rather than retaining its pre-admission zero.
        binding_epoch.0 = true;
    }
}

fn sync_physics_body_admission_hold(
    pending: Query<
        (Entity, Option<&UsdPrimPath>),
        Or<(
            With<ShouldBeDynamic>,
            With<lunco_core::PhysicsStatePending>,
            With<lunco_physics::PhysicsInitializationPending>,
            With<PendingJointAdmission>,
            With<PendingUsdJoint>,
            With<PendingDifferential>,
            With<lunco_usd_avian_joints::PendingJoint<RevoluteJoint>>,
            With<lunco_usd_avian_joints::PendingJoint<PrismaticJoint>>,
            With<lunco_usd_avian_joints::PendingJoint<FixedJoint>>,
            With<lunco_usd_avian_joints::PendingJoint<SphericalJoint>>,
            With<lunco_usd_avian_joints::PendingJoint<DistanceJoint>>,
        )>,
    >,
    parents: Query<&ChildOf>,
    preview_roots: Query<(), With<UsdPreviewOnly>>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    primary_roots: Query<&UsdPrimPath, With<UsdSceneRoot>>,
    holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
) {
    let Some(mut holds) = holds else { return };
    // A ready additive document or editor preview must not suspend the running
    // Twin. Only the pending closure below the currently mounted primary root
    // owns this simulation-wide admission boundary.
    let primary_root = mount
        .as_deref()
        .and_then(lunco_core::SceneMountState::active_root)
        .and_then(|root| primary_roots.get(root).ok());
    let pending_admission = primary_root.is_some_and(|primary| {
        pending.iter().any(|(entity, path)| {
            let belongs_to_primary = path.is_some_and(|path| {
                path.stage_handle.id() == primary.stage_handle.id()
                    && (path.path == primary.path
                        || path
                            .path
                            .strip_prefix(&primary.path)
                            .is_some_and(|suffix| suffix.starts_with('/')))
            });
            belongs_to_primary && !is_preview_only(entity, &parents, &preview_roots)
        })
    });
    if holds.holds(lunco_physics::PhysicsHolds::BODY_ADMISSION) != pending_admission {
        holds.set(
            lunco_physics::PhysicsHolds::BODY_ADMISSION,
            pending_admission,
        );
    }
}

#[cfg(test)]
mod topology_index_tests {
    use super::*;
    use lunco_usd_bevy_stage::canonical::CanonicalStage;
    use lunco_usd_compose::recipe::StageRecipe;

    const WHEEL_STAGE: &str = r#"#usda 1.0
def Xform "Rover" {
    def Xform "Chassis" {}
    def Xform "Wheel" (prepend apiSchemas = ["PhysxVehicleWheelAPI"]) {}
    def PhysicsRevoluteJoint "WheelJoint" {
        rel physics:body0 = </Rover/Chassis>
        rel physics:body1 = </Rover/Wheel>
    }
    def Xform "Attachment" (prepend apiSchemas = ["PhysxVehicleWheelAttachmentAPI"]) {
        rel physxVehicleWheelAttachment:wheel = </Rover/Wheel>
        rel physxVehicleWheelAttachment:tire = </Rover/Tire>
        rel physxVehicleWheelAttachment:suspension = </Rover/Suspension>
        int physxVehicleWheelAttachment:index = 7
    }
    def Xform "Suspension" (prepend apiSchemas = ["PhysxVehicleSuspensionAPI"]) {}
    def Xform "Tire" (prepend apiSchemas = ["PhysxVehicleTireAPI"]) {}
}
"#;

    #[test]
    fn topology_index_builds_stage_local_wheel_facts_once_per_generation() {
        let stage =
            CanonicalStage::from_recipe(&StageRecipe::from_source("topology.usda", WHEEL_STAGE))
                .expect("fixture composes");
        let id = Handle::<UsdStageAsset>::default().id();
        let mut index = JointTopologyIndex::default();

        index.commit_prepared(id, stage.generation(), build_joint_topology(&stage.view()));
        let topology = index.get(id).expect("first generation is indexed");
        assert_eq!(
            topology.joint_targets.get("/Rover/Wheel"),
            Some(&"/Rover/WheelJoint".to_string())
        );
        assert!(topology.articulation_roots.contains("/Rover/Chassis"));
        assert_eq!(
            topology.wheel_attachment_targets.get("/Rover/Wheel"),
            Some(&"/Rover/Suspension".to_string())
        );
        assert_eq!(
            topology.wheel_attachment_tires.get("/Rover/Wheel"),
            Some(&"/Rover/Tire".to_string())
        );
        assert_eq!(
            topology.wheel_attachment_indices.get("/Rover/Wheel"),
            Some(&7),
            "the standard index is read from the attachment, not copied onto the wheel"
        );
        assert!(
            !topology.authored_joints.contains_key("/Rover/WheelJoint"),
            "the physical-wheel projector owns the synthesized wheel constraint"
        );
        assert_eq!(topology.canonical_generation, Some(stage.generation()));

        // New ECS prims do not change composed topology. Keep the cached scan
        // until the canonical generation changes or the stage asset is replaced.
        index
            .by_stage
            .get_mut(&id)
            .expect("indexed stage")
            .joint_targets
            .clear();
        assert!(index.is_current(id, stage.generation()));
        assert!(
            index
                .get(id)
                .expect("indexed stage")
                .joint_targets
                .is_empty(),
            "an unchanged canonical stage must not rescan for ECS projection changes"
        );

        index.invalidate_stage(id);
        index.commit_prepared(id, stage.generation(), build_joint_topology(&stage.view()));
        assert_eq!(
            index
                .get(id)
                .expect("reindexed stage")
                .joint_targets
                .get("/Rover/Wheel"),
            Some(&"/Rover/WheelJoint".to_string()),
            "stage invalidation must rebuild replacement topology even at generation zero"
        );
    }

    #[test]
    fn topology_index_supports_the_standard_direct_api_attachment_form() {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source(
            "direct_attachment.usda",
            r#"#usda 1.0
def Xform "Wheel" (prepend apiSchemas = [
    "PhysxVehicleWheelAttachmentAPI",
    "PhysxVehicleWheelAPI",
    "PhysxVehicleTireAPI",
    "PhysxVehicleSuspensionAPI"
]) {
    int physxVehicleWheelAttachment:index = 2
}
"#,
        ))
        .expect("direct attachment fixture composes");
        let id = Handle::<UsdStageAsset>::default().id();
        let mut index = JointTopologyIndex::default();
        index.commit_prepared(id, stage.generation(), build_joint_topology(&stage.view()));
        let topology = index.get(id).expect("direct attachment is indexed");

        assert_eq!(
            topology.wheel_attachment_targets.get("/Wheel"),
            Some(&"/Wheel".to_string())
        );
        assert_eq!(
            topology.wheel_attachment_tires.get("/Wheel"),
            Some(&"/Wheel".to_string())
        );
        assert_eq!(topology.wheel_attachment_indices.get("/Wheel"), Some(&2));
        assert!(topology.invalid_wheel_attachments.is_empty());
    }

    #[test]
    fn topology_index_rejects_multi_target_tire_relationships() {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source(
            "ambiguous_attachment.usda",
            r#"#usda 1.0
def Xform "Wheel" (prepend apiSchemas = ["PhysxVehicleWheelAPI"]) {}
def Xform "Suspension" (prepend apiSchemas = ["PhysxVehicleSuspensionAPI"]) {}
def Xform "TireA" (prepend apiSchemas = ["PhysxVehicleTireAPI"]) {}
def Xform "TireB" (prepend apiSchemas = ["PhysxVehicleTireAPI"]) {}
def Xform "Attachment" (prepend apiSchemas = ["PhysxVehicleWheelAttachmentAPI"]) {
    rel physxVehicleWheelAttachment:wheel = </Wheel>
    rel physxVehicleWheelAttachment:tire = [</TireA>, </TireB>]
    rel physxVehicleWheelAttachment:suspension = </Suspension>
    int physxVehicleWheelAttachment:index = 0
}
"#,
        ))
        .expect("ambiguous attachment fixture composes");
        let id = Handle::<UsdStageAsset>::default().id();
        let mut index = JointTopologyIndex::default();
        index.commit_prepared(id, stage.generation(), build_joint_topology(&stage.view()));
        let topology = index.get(id).expect("ambiguous attachment is indexed");

        assert!(topology.invalid_wheel_attachments.contains("/Wheel"));
        assert!(!topology.wheel_attachment_tires.contains_key("/Wheel"));
    }
}

#[cfg(test)]
mod dynamic_activation_tests {
    use super::*;

    #[test]
    fn pending_typed_joint_keeps_only_its_bodies_kinematic_until_admission() {
        let mut app = App::new();
        app.init_resource::<GroundColliderPending>()
            .init_resource::<JointTopologyIndex>()
            .init_resource::<lunco_cosim_core::BindingEpochDirty>()
            .add_systems(Update, activate_dynamic_bodies);

        let stage = Handle::<UsdStageAsset>::default();
        let body = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Rover".into(),
                },
                RigidBody::Kinematic,
                ShouldBeDynamic,
            ))
            .id();
        let unrelated = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage,
                    path: "/Other".into(),
                },
                RigidBody::Kinematic,
                ShouldBeDynamic,
            ))
            .id();
        let admission = app
            .world_mut()
            .spawn(PendingJointAdmission {
                body0: body,
                body1: body,
            })
            .id();

        app.update();
        assert_eq!(
            app.world().get::<RigidBody>(body),
            Some(&RigidBody::Kinematic),
            "a body must not integrate while its typed joint is parked"
        );
        assert!(app.world().get::<ShouldBeDynamic>(body).is_some());
        assert_eq!(
            app.world().get::<RigidBody>(unrelated),
            Some(&RigidBody::Dynamic),
            "an unrelated articulated part must not wait on this joint"
        );

        app.world_mut()
            .entity_mut(admission)
            .remove::<PendingJointAdmission>();
        app.update();

        assert_eq!(
            app.world().get::<RigidBody>(body),
            Some(&RigidBody::Dynamic),
            "dynamic promotion resumes after joint admission"
        );
        assert!(app.world().get::<ShouldBeDynamic>(body).is_none());
    }

    #[test]
    fn authored_joint_topology_holds_bodies_before_joint_observer_state_lands() {
        let mut app = App::new();
        app.init_resource::<GroundColliderPending>()
            .init_resource::<JointTopologyIndex>()
            .init_resource::<lunco_cosim_core::BindingEpochDirty>()
            .add_systems(Update, activate_dynamic_bodies);

        let stage = Handle::<UsdStageAsset>::default();
        let chassis = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Rover/Chassis".into(),
                },
                RigidBody::Kinematic,
                ShouldBeDynamic,
            ))
            .id();
        let link = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Rover/Link".into(),
                },
                RigidBody::Kinematic,
                ShouldBeDynamic,
            ))
            .id();
        let joint = app
            .world_mut()
            .spawn(UsdPrimPath {
                stage_handle: stage.clone(),
                path: "/Rover/Joint".into(),
            })
            .id();

        let mut topology = StageJointTopology::default();
        topology.authored_joints.insert(
            "/Rover/Joint".into(),
            ("/Rover/Chassis".into(), "/Rover/Link".into()),
        );
        app.world_mut()
            .resource_mut::<JointTopologyIndex>()
            .by_stage
            .insert(stage.id(), topology);

        // The canonical stage already names the joint, but its observer command
        // has not yet added PendingUsdJoint to the joint entity. Neither body may
        // receive a dynamic physics step in that gap.
        app.update();
        assert_eq!(
            app.world().get::<RigidBody>(chassis),
            Some(&RigidBody::Kinematic)
        );
        assert_eq!(
            app.world().get::<RigidBody>(link),
            Some(&RigidBody::Kinematic)
        );

        // A live detach invalidates only this authored topology edge. The
        // canonical stage generation is intentionally unchanged, so the
        // endpoint marker is the generic handoff that lets admission proceed
        // without waiting for a scene reload.
        app.world_mut()
            .entity_mut(chassis)
            .insert(lunco_physics::PhysicsJointDetachSet {
                joint_paths: vec!["/Rover/Joint".into()],
            });
        app.world_mut()
            .entity_mut(link)
            .insert(lunco_physics::PhysicsJointDetachSet {
                joint_paths: vec!["/Rover/Joint".into()],
            });
        app.update();
        assert_eq!(
            app.world().get::<RigidBody>(chassis),
            Some(&RigidBody::Dynamic),
            "a released authored joint must not hold body0 kinematic"
        );
        assert_eq!(
            app.world().get::<RigidBody>(link),
            Some(&RigidBody::Dynamic),
            "a released authored joint must not hold body1 kinematic"
        );

        app.world_mut()
            .entity_mut(joint)
            .insert(RevoluteJoint::new(chassis, link));
        app.update();
        assert_eq!(
            app.world().get::<RigidBody>(chassis),
            Some(&RigidBody::Dynamic)
        );
        assert_eq!(
            app.world().get::<RigidBody>(link),
            Some(&RigidBody::Dynamic)
        );
    }
}

#[cfg(test)]
mod proxy_wheel_tests {
    use super::*;
    use bevy::time::Time;
    use std::time::Duration;

    /// Run `animate_proxy_physical_wheels` one tick against a chassis of the given
    /// body type moving along world −Z, returning the wheel's resulting
    /// `spin_angle` and the visual child's rotation.
    fn run_once(chassis_body: RigidBody) -> (f32, Quat) {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f64(0.1));
        app.insert_resource(time);

        let chassis = app
            .world_mut()
            .spawn((
                chassis_body,
                Position(DVec3::ZERO),
                // avian auto-adds `Rotation` to every RigidBody in the real app; the
                // hand-built test entity must carry it too now that the spin system
                // reconstructs the hub from the chassis pose (CQ-201 fix).
                Rotation::default(),
                ComputedCenterOfMass::default(),
                lunco_core_session::ReplicatedChassisMotion {
                    lin: DVec3::new(0.0, 0.0, -2.0), // 2 m/s along chassis forward (−Z)
                    ang: DVec3::ZERO,
                },
                lunco_core::MobilityRoot,
                lunco_port_core::OutputPorts::default(),
            ))
            .id();
        let visual = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn((
            PhysicalWheel {
                visual_entity: Some(visual),
                wheel_radius: 0.5,
                wheel_width: 0.3,
                axis_rot: Quat::IDENTITY,
                spin_angle: 0.0,
                mount_local: Vec3::ZERO,
            },
            lunco_mobility::WheelBodyMount {
                body: chassis,
                local: Transform::IDENTITY,
            },
            GlobalTransform::IDENTITY,
            Rotation::default(),
            ChildOf(chassis),
        ));

        app.add_systems(Update, animate_proxy_physical_wheels);
        app.update();

        let spin = app
            .world_mut()
            .query::<&PhysicalWheel>()
            .iter(app.world())
            .next()
            .unwrap()
            .spin_angle;
        let rot = app
            .world()
            .entity(visual)
            .get::<Transform>()
            .unwrap()
            .rotation;
        (spin, rot)
    }

    #[test]
    fn kinematic_proxy_spins_and_rotates_visual() {
        // v_long = 2 m/s, r = 0.5 → ω = 4 rad/s; one 0.1 s tick ⇒ |Δθ| = 0.4.
        let (spin, rot) = run_once(RigidBody::Kinematic);
        // spin_angle is wrapped to [0, TAU); measure the minimal circular distance.
        let wrapped = spin.rem_euclid(std::f32::consts::TAU);
        let circ = wrapped.min(std::f32::consts::TAU - wrapped);
        assert!(
            (circ - 0.4).abs() < 1e-3,
            "expected |spin|≈0.4, got {spin} (circ {circ})"
        );
        assert!(
            rot.angle_between(Quat::IDENTITY) > 1e-3,
            "visual child should be rotated, got {rot:?}"
        );
    }

    #[test]
    fn host_dynamic_chassis_is_noop() {
        // On the host the joint motor spins the body; this system must not touch
        // the wheel (else the visual double-rotates).
        let (spin, rot) = run_once(RigidBody::Dynamic);
        assert_eq!(spin, 0.0, "host chassis must be a no-op, got spin {spin}");
        assert_eq!(rot, Quat::IDENTITY, "host visual must be untouched");
    }

    #[test]
    fn replicated_wheel_is_noop() {
        // With per-link replication the wheel BODY carries the host's true world
        // rotation and the visual child inherits it; the proxy animator must
        // skip a `NetReplicate` wheel (else the visual spin double-applies).
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f64(0.1));
        app.insert_resource(time);

        let chassis = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                Position(DVec3::ZERO),
                Rotation::default(),
                ComputedCenterOfMass::default(),
                lunco_core_session::ReplicatedChassisMotion {
                    lin: DVec3::new(0.0, 0.0, -2.0),
                    ang: DVec3::ZERO,
                },
                lunco_core::MobilityRoot,
                lunco_port_core::OutputPorts::default(),
            ))
            .id();
        let visual = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn((
            PhysicalWheel {
                visual_entity: Some(visual),
                wheel_radius: 0.5,
                wheel_width: 0.3,
                axis_rot: Quat::IDENTITY,
                spin_angle: 0.0,
                mount_local: Vec3::ZERO,
            },
            lunco_mobility::WheelBodyMount {
                body: chassis,
                local: Transform::IDENTITY,
            },
            GlobalTransform::IDENTITY,
            Rotation::default(),
            ChildOf(chassis),
            // The discriminator under test: a per-link-replicated wheel.
            lunco_core_session::NetReplicate,
        ));

        app.add_systems(Update, animate_proxy_physical_wheels);
        app.update();

        let spin = app
            .world_mut()
            .query::<&PhysicalWheel>()
            .iter(app.world())
            .next()
            .unwrap()
            .spin_angle;
        let rot = app
            .world()
            .entity(visual)
            .get::<Transform>()
            .unwrap()
            .rotation;
        assert_eq!(
            spin, 0.0,
            "replicated wheel must be a no-op, got spin {spin}"
        );
        assert_eq!(
            rot,
            Quat::IDENTITY,
            "replicated wheel visual must be untouched"
        );
    }

    /// Run the proxy spin one tick with an explicit chassis angular velocity, a
    /// non-zero wheel mount offset, and an arbitrary wheel `GlobalTransform`
    /// translation — returns the resulting `spin_angle`.
    ///
    /// The chassis pose is read from avian `Position`/`Rotation` (identity here);
    /// the wheel's `GlobalTransform.translation` is what big_space rebases away
    /// from the origin. Pre-fix the spin integrator built the lever arm as
    /// `wheel_gtf − chassis_pos` (render-frame minus avian-frame), so the returned
    /// spin depended on `wheel_gtf_translation`. Post-fix it reconstructs the hub
    /// from `chassis_pos + chassis_rot · mount_local` (pure avian), so the spin is
    /// **independent** of `wheel_gtf_translation` — which is what this drives.
    fn run_spin_with(ang: DVec3, mount_local: Vec3, wheel_gtf_translation: Vec3) -> f32 {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f64(0.1));
        app.insert_resource(time);

        let chassis = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                Position(DVec3::ZERO),
                Rotation::default(),
                ComputedCenterOfMass::default(),
                lunco_core_session::ReplicatedChassisMotion {
                    lin: DVec3::ZERO,
                    ang,
                },
                lunco_core::MobilityRoot,
                lunco_port_core::OutputPorts::default(),
            ))
            .id();
        let visual = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn((
            PhysicalWheel {
                visual_entity: Some(visual),
                wheel_radius: 0.5,
                wheel_width: 0.3,
                axis_rot: Quat::IDENTITY,
                spin_angle: 0.0,
                mount_local,
            },
            lunco_mobility::WheelBodyMount {
                body: chassis,
                local: Transform::from_translation(mount_local),
            },
            GlobalTransform::from(Transform::from_translation(wheel_gtf_translation)),
            Rotation::default(),
            ChildOf(chassis),
        ));

        app.add_systems(Update, animate_proxy_physical_wheels);
        app.update();
        app.world_mut()
            .query::<&PhysicalWheel>()
            .iter(app.world())
            .next()
            .unwrap()
            .spin_angle
    }

    #[test]
    fn proxy_spin_is_floating_origin_invariant() {
        // CQ-201 regression. Chassis yaws about +Y at 1 rad/s; the hub sits 1 m out
        // along +X, so the lever arm feeds the hub velocity (ω × r) and thus the
        // rolling rate. The ONLY difference between the two runs is the wheel's
        // `GlobalTransform` translation — "near origin" (the true world hub pos) vs
        // "≈1 km away" (rebased by a big_space origin offset). A frame-correct
        // integrator must give the SAME spin for both; the old `gtf − pos.0` lever
        // gave wildly different answers (that was the bug, invisible near origin).
        let ang = DVec3::Y; // yaw 1 rad/s about +Y
        let mount = Vec3::new(1.0, 0.0, 0.0);

        let near = run_spin_with(ang, mount, /* true hub world pos */ mount);
        let far = run_spin_with(
            ang,
            mount,
            /* rebased 1 km along the sensitive axis */ mount - Vec3::new(1000.0, 0.0, 0.0),
        );

        assert!(
            (near - far).abs() < 1e-6,
            "spin must be floating-origin invariant: near={near} far={far} (Δ={})",
            (near - far).abs()
        );

        // And it must be the physically-correct value, not just self-consistent:
        // lever=(1,0,0), ω×r=(0,1,0)×(1,0,0)=(0,0,−1) ⇒ v_long=(0,0,−1)·(0,0,−1)=1;
        // rate ω=v_long/r=1/0.5=2; one 0.1 s tick with ROLL_SIGN=−1 ⇒ Δθ=−0.2.
        let wrapped = near.rem_euclid(std::f32::consts::TAU);
        let circ = wrapped.min(std::f32::consts::TAU - wrapped);
        assert!(
            (circ - 0.2).abs() < 1e-3,
            "expected |Δθ|≈0.2, got {near} (circ {circ})"
        );
    }

    #[test]
    fn net_override_vocabulary() {
        // Default / server / predictable: replicated, predictable (no override markers).
        assert_eq!(super::net_override_markers(None, None), (false, false));
        assert_eq!(
            super::net_override_markers(None, Some("server")),
            (false, false)
        );
        assert_eq!(
            super::net_override_markers(None, Some("predictable")),
            (false, false)
        );
        // Opt-out: excluded, not opaque.
        assert_eq!(
            super::net_override_markers(Some(false), None),
            (true, false)
        );
        assert_eq!(
            super::net_override_markers(None, Some("local")),
            (true, false)
        );
        // Opaque: replicated but never predicted.
        assert_eq!(
            super::net_override_markers(None, Some("opaque")),
            (false, true)
        );
        // Explicit include is not an exclusion.
        assert_eq!(
            super::net_override_markers(Some(true), None),
            (false, false)
        );
    }

    #[test]
    fn proxy_pose_at_identity_chassis_is_mount_offset() {
        // Chassis at origin, no rotation, no steer ⇒ wheel sits exactly at mount_local.
        let mount = DVec3::new(0.8, -0.3, 1.2);
        let (p, q) = super::proxy_wheel_pose(DVec3::ZERO, DQuat::IDENTITY, mount);
        assert!((p - mount).length() < 1e-12, "p={p:?}");
        assert!(q.angle_between(DQuat::IDENTITY) < 1e-12, "q={q:?}");
    }

    #[test]
    fn proxy_pose_rotates_mount_into_world() {
        // Chassis yawed 90° about +Y at a translated origin: the mount offset must
        // be rotated into world space and added to the chassis position. A +90° yaw
        // maps local +Z → world +X (right-handed, Y-up).
        let chassis_pos = DVec3::new(10.0, 0.0, -5.0);
        let chassis_rot = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
        let mount = DVec3::new(0.0, 0.0, 1.0); // 1 m forward in chassis frame
        let (p, q) = super::proxy_wheel_pose(chassis_pos, chassis_rot, mount);
        let expected = chassis_pos + DVec3::new(1.0, 0.0, 0.0);
        assert!(
            (p - expected).length() < 1e-9,
            "p={p:?}, expected {expected:?}"
        );
        // The proxy wheel inherits the chassis orientation.
        assert!(q.angle_between(chassis_rot) < 1e-9, "q={q:?}");
    }
}
