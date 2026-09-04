//! # LunCoSim USD → Simulation Mapping
//!
//! Detects USD simulation schemas (NVIDIA PhysX Vehicles) and maps them to LunCoSim
//! simulation components. This is the **third** plugin in the USD processing pipeline,
//! running after `UsdBevyPlugin` and alongside `UsdAvianPlugin`.
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
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_usd_avian::{
    AuthoredInitialVelocity, PendingJointAdmission, SharedTireContact, ShouldBeDynamic,
};
use lunco_usd_bevy::{
    instance_key, is_preview_only, resolve_stage_prim_path, CanonicalStages, UsdInstanceProjection,
};
pub use lunco_usd_bevy::{UsdInstanceRoot, UsdPreviewOnly, UsdPrimPath, UsdStageAsset};
// Appearance + camera **intent** — this crate must never name `MeshMaterial3d`,
// `StandardMaterial`, `ShaderMaterial` or `Camera3d` (all `bevy_pbr` /
// `bevy_core_pipeline` → wgpu + naga). `lunco-render-bevy` binds these.
// See docs/architecture/render-decoupling.md.
use leafwing_input_manager::prelude::ActionState;
use lunco_autopilot::usd_tree::BehaviorXml;
use lunco_avatar::{
    AdaptiveNearPlane, AvatarFlightSettings, FreeFlightCamera, OrbitCamera, SpringArmCamera,
};
use lunco_controller::InputBindingsSettings;
use lunco_core::architecture::{IntentAnalogState, Port, PortSurface};
use lunco_core::coords::{GridPos, GridRot, VehicleFrame};
use lunco_core::{Avatar, LocalAvatar};
use lunco_cosim::{
    avian_queries::RaycastObservation, ForceActuator, JointTorqueActuator, TorqueActuator,
};
use lunco_materials::ShaderLook;
use lunco_mobility::wheel_kinematics::{body_point_velocity, wheel_hub_pose, wheel_roll_rate};
use lunco_mobility::{
    DifferentialCoupling, DifferentialDriveType, JointedWheelTire, Suspension, SuspensionPiston,
    SuspensionSpring, WheelRaycast,
};
use lunco_render::{GraphicsCameraDefaults, PbrLook, SceneCamera};
use openusd::sdf::{Path as SdfPath, Value};
use std::collections::{HashMap, HashSet};

pub mod wheel_params;
use wheel_params::{SuspensionParams, WheelParams};

/// Plugin for mapping simulation-specific USD schemas (like NVIDIA PhysX Vehicles)
/// to LunCo's optimized simulation models.
///
/// # Processing Order
///
/// 1. `process_usd_sim_prims` — maps schemas to components after visual projection
/// 2. Generic USD connection derivation — connects authored controller outputs to
///    wheel and joint ports through the common co-simulation fabric
///
/// The observer `on_add_usd_sim_prim` intentionally does minimal work. All processing
/// is deferred to the `process_usd_sim_prims` system so the canonical stage and
/// render-free appearance intent are available at the projection boundary.
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

const FORCE_ACTUATOR_API: &str = "LunCoForceActuatorAPI";
const FORCE_DIRECTION_ATTR: &str = "lunco:forceActuator:direction";
const FORCE_MAX_ATTR: &str = "lunco:forceActuator:maxForce";
const TORQUE_ACTUATOR_API: &str = "LunCoTorqueActuatorAPI";
const TORQUE_AXIS_ATTR: &str = "lunco:torqueActuator:axis";
const TORQUE_MAX_ATTR: &str = "lunco:torqueActuator:maxTorque";

/// Find the USD rigid-body frame that owns a physical actuator. Ownership is
/// structural: the actuator is a prim under the body, just like a collider or
/// a joint endpoint. No vessel name or subsystem slot is embedded in Rust.
fn actuator_body_path(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    actuator_path: &SdfPath,
) -> Option<SdfPath> {
    let mut current = actuator_path.parent();
    while let Some(path) = current {
        if path.is_abs_root() {
            return None;
        }
        if reader.has_api_schema(&path, "PhysicsRigidBodyAPI") {
            return Some(path);
        }
        current = path.parent();
    }
    None
}

/// Read a force actuator's generic description. Position comes from the
/// composed prim transform; direction and force capacity are authored
/// properties on the same prim. The direction is authored in the actuator
/// prim's local frame (the USD schema contract) and is converted once into the
/// owning body's frame before it enters the generic Avian actuator component.
pub(crate) fn force_actuator_from_usd(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    actuator_path: &SdfPath,
) -> Option<ForceActuator> {
    if !reader.has_api_schema(actuator_path, FORCE_ACTUATOR_API) {
        return None;
    }
    let Some(body_path) = actuator_body_path(reader, actuator_path) else {
        warn!(
            "[usd-cosim] force actuator {} has no PhysicsRigidBodyAPI ancestor; actuator ignored",
            actuator_path
        );
        return None;
    };
    let Some(relative) =
        lunco_usd_avian::transform_in_body_frame(reader, &body_path, actuator_path)
    else {
        warn!(
            "[usd-cosim] force actuator {} could not derive its body-frame transform",
            actuator_path
        );
        return None;
    };
    let direction = reader
        .attr_value(actuator_path, FORCE_DIRECTION_ATTR)
        .and_then(|value| {
            value.clone().get::<[f32; 3]>().or_else(|| {
                value
                    .get::<[f64; 3]>()
                    .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32])
            })
        })
        .map(Vec3::from_array)
        .filter(|v| v.is_finite() && v.length_squared() > f32::EPSILON);
    let Some(direction_in_prim_frame) = direction else {
        warn!(
            "[usd-cosim] force actuator {} has no finite non-zero {}",
            actuator_path, FORCE_DIRECTION_ATTR
        );
        return None;
    };
    let direction_local = relative.rotation * direction_in_prim_frame;
    if !direction_local.is_finite() || direction_local.length_squared() <= f32::EPSILON {
        warn!(
            "[usd-cosim] force actuator {} produced an invalid body-frame direction",
            actuator_path
        );
        return None;
    }
    let Some(max_force_n) = reader
        .real(actuator_path, FORCE_MAX_ATTR)
        .filter(|v| v.is_finite() && *v > 0.0)
    else {
        warn!(
            "[usd-cosim] force actuator {} has no positive {}",
            actuator_path, FORCE_MAX_ATTR
        );
        return None;
    };
    Some(ForceActuator {
        local_position: relative.translation,
        direction_local,
        max_force_n,
    })
}

/// Read a torque actuator's generic description. Reaction wheels and control
/// moment gyros use the same scalar torque command and axis contract.
pub(crate) fn torque_actuator_from_usd(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    actuator_path: &SdfPath,
) -> Option<TorqueActuator> {
    if !reader.has_api_schema(actuator_path, TORQUE_ACTUATOR_API) {
        return None;
    }
    if actuator_body_path(reader, actuator_path).is_none() {
        warn!(
            "[usd-cosim] torque actuator {} has no PhysicsRigidBodyAPI ancestor; actuator ignored",
            actuator_path
        );
        return None;
    }
    let axis = reader
        .attr_value(actuator_path, TORQUE_AXIS_ATTR)
        .and_then(|value| {
            value.clone().get::<[f32; 3]>().or_else(|| {
                value
                    .get::<[f64; 3]>()
                    .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32])
            })
        })
        .map(Vec3::from_array)
        .filter(|v| v.is_finite() && v.length_squared() > f32::EPSILON);
    let Some(axis_local) = axis else {
        warn!(
            "[usd-cosim] torque actuator {} has no finite non-zero {}",
            actuator_path, TORQUE_AXIS_ATTR
        );
        return None;
    };
    let Some(max_torque_nm) = reader
        .real(actuator_path, TORQUE_MAX_ATTR)
        .filter(|v| v.is_finite() && *v > 0.0)
    else {
        warn!(
            "[usd-cosim] torque actuator {} has no positive {}",
            actuator_path, TORQUE_MAX_ATTR
        );
        return None;
    };
    Some(TorqueActuator {
        axis_local,
        max_torque_nm,
    })
}

/// Ordered phases of the USD-to-simulation projection.
///
/// `Projection` is the publication boundary for composed scene components;
/// camera handoff systems are ordered after it. `ActivateDynamicBodies` places
/// terrain readiness between terrain inspection and the first dynamic physics
/// tick. Keeping both boundaries public prevents first-load ordering races.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsdSimSet {
    /// Publishes the composed USD simulation and celestial components.
    Projection,
    /// Converts `ShouldBeDynamic` bodies only after their ground is known ready.
    ActivateDynamicBodies,
}

/// Immutable USD topology facts used by the simulation projector for one
/// composed stage revision.  This is deliberately separate from ECS entities:
/// a wheel and its sibling joint can arrive on different frames, while their
/// relationships are already complete in the canonical stage.
#[derive(Default)]
struct StageJointTopology {
    canonical_generation: Option<u64>,
    projection_revision: Option<u64>,
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
/// The canonical stage generation catches live authored changes; the USD
/// projection revision catches a replacement stage whose local generation
/// starts at zero again after an asset reload. A stage is scanned once for that
/// combined stamp rather than once for every frame that a prim waits for its
/// visuals.
#[derive(Resource, Default)]
struct JointTopologyIndex {
    by_stage: HashMap<bevy::asset::AssetId<UsdStageAsset>, StageJointTopology>,
}

impl JointTopologyIndex {
    fn refresh_if_stale(
        &mut self,
        stage: bevy::asset::AssetId<UsdStageAsset>,
        generation: u64,
        projection_revision: u64,
        reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    ) {
        let topology = self.by_stage.entry(stage).or_default();
        if topology.canonical_generation == Some(generation)
            && topology.projection_revision == Some(projection_revision)
        {
            return;
        }
        topology.joint_targets.clear();
        topology.physical_wheel_bodies.clear();
        topology.authored_joints.clear();
        topology.articulation_roots.clear();
        topology.wheel_attachment_targets.clear();
        topology.wheel_attachment_tires.clear();
        topology.wheel_attachment_indices.clear();
        topology.invalid_wheel_attachments.clear();
        collect_joint_scan_read(reader, topology);
        topology.canonical_generation = Some(generation);
        topology.projection_revision = Some(projection_revision);
    }

    fn get(&self, stage: bevy::asset::AssetId<UsdStageAsset>) -> Option<&StageJointTopology> {
        self.by_stage.get(&stage)
    }
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
        assert!(!app
            .world()
            .resource::<lunco_physics::PhysicsHolds>()
            .holds(lunco_physics::PhysicsHolds::SAFETY_FAILURE));

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

#[cfg(test)]
mod authored_sun_tests {
    use super::*;

    #[test]
    fn authored_sun_state_reads_the_propagated_world_rotation() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::transform::TransformPlugin));
        app.init_resource::<lunco_environment::SunState>();

        let frame = app
            .world_mut()
            .spawn((Transform::default(), GlobalTransform::default()))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(frame));

        let authored_rotation = Quat::from_euler(
            EulerRot::XYZ,
            -35.0_f32.to_radians(),
            40.0_f32.to_radians(),
            0.0,
        );
        app.world_mut().spawn((
            Transform::from_rotation(authored_rotation),
            GlobalTransform::default(),
            bevy::light::DirectionalLight::default(),
            lunco_usd_bevy::UsdAuthoredLight,
            ChildOf(frame),
        ));

        install_authored_sun_state_seed(&mut app);
        app.update();

        let expected = -(authored_rotation * Vec3::NEG_Z);
        let actual = app
            .world()
            .resource::<lunco_environment::SunState>()
            .direction_to_sun
            .expect("the authored light must seed semantic sun state");
        assert!(actual.abs_diff_eq(expected.normalize(), 1.0e-5));
        assert!(
            actual.y > 0.25,
            "the authored sun must be above the ground: {actual:?}"
        );
    }
}

impl Plugin for UsdSimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<lunco_core::RuntimeFaults>();
        app.init_resource::<lunco_core::RuntimeDiagnostics>();
        app.init_resource::<InputBindingsSettings>();
        crate::shader_ports::build(app);
        app.configure_sets(
            Update,
            (
                UsdSimSet::Projection.before(lunco_avatar::AvatarSceneHandoffSet),
                UsdSimSet::ActivateDynamicBodies,
            ),
        )
        .configure_sets(PreUpdate, UsdSimSet::ActivateDynamicBodies);
        app.add_systems(lunco_core::SceneTeardown, reset_scene_runtime_safety);
        app.add_systems(lunco_core::SceneTeardown, retire_scene_cameras);
        // Autopilot actors claim scene vessels and hold compiled trees of the scene's
        // route — scene-derived state, so the shared teardown boundary retires them
        // with the rest of the scene (despawn + release the claim, so the respawned
        // vessel can be re-engaged and its waypoints reset cleanly).
        app.add_systems(
            lunco_core::SceneTeardown,
            lunco_autopilot::teardown_autopilot_actors,
        );
        app.register_type::<PhysicalWheel>()
            // Client-only: reconstruct a remote rover's wheels from its chassis
            // (kinematic followers — wheels are no longer replicated), then re-derive
            // the cosmetic visual roll. Chained so the visual spin layers on the
            // freshly-placed body. Same `relative_speed > 0` gate as raycast wheels.
            .add_systems(
                FixedUpdate,
                (reconstruct_proxy_wheels, animate_proxy_physical_wheels)
                    .chain()
                    .run_if(|t: Res<Time<Virtual>>| !t.is_paused() && t.relative_speed_f64() > 0.0),
            )
            .add_systems(
                FixedPostUpdate,
                physics_telemetry::retain_physics_telemetry.after(PhysicsSystems::StepSimulation),
            )
            .add_observer(on_add_usd_sim_prim)
            .add_systems(PreUpdate, resolve_differential_coupling)
            // USD → ShaderMaterial authoring. Ordered AFTER the bounded visual
            // projection and BEFORE `process_usd_sim_prims` consumes the prims,
            // so the material is present before a wheel is split onto its visual
            // child. The completed projection boundary also prevents the visual
            // projector from restoring a cylinder-axis rotation after the
            // simulator has established the wheel's identity physics frame.
            // See `shader.rs`.
            .add_systems(
                Update,
                shader::apply_usd_shader_materials
                    .after(lunco_usd_bevy::process_queued_usd_visuals)
                    .before(process_usd_sim_prims),
            )
            // `process_usd_sim_prims` does a per-stage joint scan + per-
            // entity dispatch — too coupled to fit cleanly into a single
            // `OnAdd<UsdVisualSynced>` observer. Gating with `run_if`
            // skips the system entirely on frames with no unprocessed
            // USD prim (archetype-level check, near-zero cost).
            .init_resource::<GroundColliderPending>()
            .init_resource::<GroundActivationInFlight>()
            .init_resource::<JointTopologyIndex>()
            .init_resource::<physics_telemetry::PhysicsTelemetryState>()
            .add_systems(
                Update,
                (
                    process_usd_sim_prims
                        .run_if(any_unprocessed_usd_sim)
                        .after(lunco_usd_bevy::process_queued_usd_visuals),
                    // Resolve behavior targets only after this frame's USD
                    // prim projection has admitted newly spawned waypoint
                    // entities. Running in PreUpdate raced the projection and
                    // replaced a valid binding with an incomplete map.
                    resolve_behavior_targets
                        .after(process_usd_sim_prims)
                        .after(lunco_usd_bevy::sync_usd_visuals),
                    // Independent link/celestial projector — runs for EVERY prim (cosim,
                    // wheel, plain), gated by its own marker, blocked by nothing.
                    project_celestial_comms_prims
                        .run_if(any_unprojected_celestial)
                        .after(lunco_usd_bevy::sync_usd_visuals),
                    remove_nested_link_nodes
                        .run_if(any_nested_link_nodes)
                        .after(project_celestial_comms_prims),
                )
                    .in_set(UsdSimSet::Projection),
            );
        // Dynamic admission must happen before the fixed loop.  The main loop runs
        // FixedUpdate before Update, so admitting a body from Update gives it one
        // live solver tick at its authored loading pose before the terrain placement
        // pass can observe it.  USD projection still publishes `ShouldBeDynamic` in
        // Update; this pre-fixed pass consumes it on the following frame, after the
        // terrain readiness state is authoritative.
        app.add_systems(
            PreUpdate,
            activate_dynamic_bodies
                .in_set(UsdSimSet::ActivateDynamicBodies)
                .before(lunco_physics::apply_physics_holds)
                .run_if(any_with_component::<ShouldBeDynamic>),
        );
        // Screen-constant markers. `PostUpdate` before transform propagation:
        // the scale is a function of the camera's position THIS frame, and the
        // markers sit on other bodies' grids, which `place_celestial_bound_entities`
        // may have just re-parented.
        app.add_systems(
            PostUpdate,
            marker::scale_screen_constant_markers.before(TransformSystems::Propagate),
        );
        // Waypoint progress is runtime session state. Keep it on the projected
        // appearance intent rather than routing a material edit back through the
        // live USD stage (which would rebuild the scene during an active mission).
        app.add_systems(
            Update,
            marker::sync_waypoint_visuals.after(process_usd_sim_prims),
        );
        // The authored light's `Transform` is installed during Update, while
        // its composed world rotation is produced by Bevy/big_space transform
        // propagation. Read that world fact only after propagation; sampling it
        // in Update sees the default identity GlobalTransform for a newly
        // admitted light and would publish a horizontal semantic sun on the
        // following frame.
        install_authored_sun_state_seed(app);
        // USD → cosim wiring through native `connectionPaths` — see `cosim.rs`.
        cosim::install(app);
        // `GET /api/diagnostics` read side — exposes the cosim dangling-wire report.
        cosim_diagnostics::register(app);
    }
}

/// USD-authored screen-facing text labels (`lunco:billboard*`) — a prim
/// declares its own label content, including live geolocation.
pub mod billboard;
pub mod celestial;
pub mod cosim;
pub mod cosim_diagnostics;
pub mod domain_projection;
pub mod lint;
/// USD-authored screen-constant markers (`lunco:marker:*`) — geometry that
/// subtends a fixed angle so a physically sub-pixel thing still reads on screen.
pub mod marker;
pub mod physics_telemetry;
pub mod readiness;
pub use cosim::{CosimStatusProvider, UsdSourcedCosim};

/// USD → [`ShaderMaterial`](lunco_materials::ShaderMaterial) authoring,
/// deterministically ordered so it can never race a downstream consumer.
pub mod shader;

/// Shader parameters as connection targets — the port backend for what
/// [`shader`] authors.
pub mod shader_ports;

/// A joint-based wheel: a full rigid body that interacts with terrain through
/// collision, not raycast suspension. It gets `RigidBody`, `Collider`, and a
/// solved `JointTorqueActuator` boundary instead of `WheelRaycast` + `RayCaster`.
///
/// On the host (and the rover this client owns) the visible spin comes from the
/// avian joint motor rotating the wheel **body**; the visual mesh is a child and
/// inherits that rotation. On a networked **client proxy** the chassis is
/// kinematic and the joint motor is held at zero, so the body never spins — the
/// fields below let [`animate_proxy_physical_wheels`] re-derive the roll from the
/// replicated chassis motion and author the visual child directly, mirroring how
/// raycast wheels are animated on the client.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct PhysicalWheel {
    /// The visual mesh child (the entity whose local rotation we author on a
    /// client proxy). `None` if the wheel prim carried no mesh.
    pub visual_entity: Option<Entity>,
    /// Rolling radius (m); the proxy roll rate is `ω = v_long / r`.
    pub wheel_radius: f32,
    /// Authored wheel width (m), retained so a live width edit can rebuild the
    /// collider instead of changing density while leaving the old shape in place.
    pub wheel_width: f32,
    /// Visual base orientation (the USD cylinder `axis`). The roll axle is
    /// `axis_rot · Y` and the visual base composes as `roll · axis_rot`, exactly
    /// reconstructing the host's `body_spin · axis_rot`.
    pub axis_rot: Quat,
    /// Integrated roll angle (rad), wrapped to `[0, 2π)`. Client display state;
    /// unused on the host (the body carries the real rotation there).
    pub spin_angle: f32,
    /// Wheel mount offset in the enclosing vehicle frame. A client proxy can
    /// reconstruct the wheel's position as `chassis_pos + chassis_rot · mount_local`
    /// instead of replicating a static mount offset.
    pub mount_local: Vec3,
}

/// An authored `PhysxPhysicsGearJoint`, held until the bodies it gears together have
/// spawned + been admitted by Avian. `resolve_differential_coupling` matches the
/// prim-path strings → entities, then attaches the [`DifferentialCoupling`].
#[derive(Component)]
pub struct PendingDifferential {
    /// Composed prim path of the frame both hinges turn against — the gear's reaction
    /// target (`physics:body0` of the hinges; a rover's chassis).
    pub chassis: String,
    /// Composed prim path of the body the first hinge turns.
    pub rocker_a: String,
    /// Composed prim path of the body the second hinge turns.
    pub rocker_b: String,
    /// Authored `physxGearJoint:gearRatio` — the `r` in `θ_a = r·θ_b`.
    pub ratio: f64,
    pub rest_offset: f64,
    pub target_velocity: f64,
    pub stiffness: f64,
    pub damping: f64,
    pub max_force: f64,
    pub drive_type: DifferentialDriveType,
}

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
/// Run condition: true when any visually projected `UsdPrimPath` entity still
/// lacks `UsdSimProcessed`. The visual marker is the projection boundary: sim
/// processing must not race the bounded visual pass that applies primitive-axis
/// presentation transforms.
fn any_unprocessed_usd_sim(
    q: Query<
        (),
        (
            With<UsdPrimPath>,
            With<lunco_usd_bevy::UsdVisualSynced>,
            Without<UsdSimProcessed>,
        ),
    >,
) -> bool {
    !q.is_empty()
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
            Has<lunco_usd_bevy::UsdVisualMeshPending>,
            Has<lunco_usd_bevy::UsdVisualShaderBound>,
        ),
        (
            With<lunco_usd_bevy::UsdVisualSynced>,
            Without<UsdSimProcessed>,
        ),
    >,
    all_prims: Query<(Entity, &UsdPrimPath, Option<&Transform>)>,
    grid_components: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_child_of: Query<&ChildOf>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    stages: Res<Assets<UsdStageAsset>>,
    // Initial reads use the worker-produced plan; later authored generations
    // use the live canonical stage selected by the shared reader boundary.
    canonical: NonSend<CanonicalStages>,
    mut topology_index: ResMut<JointTopologyIndex>,
    stage_revision: Res<lunco_usd_bevy::UsdStageRevision>,
    // The active-scene sun: the avatar camera's exposure is read from the SAME
    // resource the sun illuminance comes from, so they can't drift (a dimmed
    // sun under a bright-tuned camera blacked the viewport). `Option` so the
    // loader still works in a stripped app without `EnvironmentPlugin`.
    active_sun: Option<Res<lunco_environment::LunarSun>>,
    input_bindings: Res<InputBindingsSettings>,
    mut runtime_diagnostics: ResMut<lunco_core::RuntimeDiagnostics>,
) {
    let started = web_time::Instant::now();
    let mut processed = 0usize;
    let mut authored_diagnostics = Vec::new();
    let Ok(input_map) = input_bindings.input_map() else {
        error!("[usd-sim] refusing to create avatar controllers from invalid input bindings");
        runtime_diagnostics.replace_producer("usd-sim", std::iter::empty());
        return;
    };
    // Build (or refresh) each involved stage's immutable topology once. The
    // canonical generation is the authored-composition invalidation signal;
    // waiting for a mesh or another sibling no longer re-scans every spec.
    let mut seen_stages = HashSet::new();
    for (_, prim_path, ..) in query.iter() {
        let id = prim_path.stage_handle.id();
        if !seen_stages.insert(id) {
            continue;
        }
        if let Some(stage_asset) = stages.get(&prim_path.stage_handle) {
            let (reader, generation) = canonical.reader_for(id, stage_asset);
            topology_index.refresh_if_stale(id, generation, stage_revision.0, &reader);
        }
    }

    // --- Pass 2: Process all prims ---
    for (
        entity,
        prim_path,
        maybe_tf,
        maybe_mesh,
        maybe_mat,
        maybe_shader_mat,
        instance_projection,
        mesh_pending,
        shader_bound,
    ) in query.iter()
    {
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
            continue;
        };

        // Bail when this prim lives under a `UsdPreviewOnly` scene
        // root. Preview viewports render geometry only — they must
        // not spawn Avatar Camera3d, actuator ports, or wheel raycasts
        // into the main world. Walking up the `ChildOf` chain catches
        // every prim because `sync_usd_visuals` parents each spawned
        // prim entity to its USD-parent entity, which itself chains
        // back to the workbench-owned scene_root.
        if is_preview_only(entity, &q_child_of, &q_preview_only) {
            commands.entity(entity).try_insert(UsdSimProcessed);
            continue;
        }

        let id = prim_path.stage_handle.id();
        let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
            continue;
        };
        let (reader, _generation) =
            canonical.reader_for_entity(id, stage_asset, instance_projection);
        let Some(topology) = topology_index.get(id) else {
            continue;
        };
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
            &grid_components,
            &q_spatial,
            active_sun.as_deref(),
            &input_map,
            &mut commands,
            &mut authored_diagnostics,
        );
        processed += 1;
    }
    runtime_diagnostics.replace_producer("usd-sim", authored_diagnostics);
    if processed > 0 {
        bevy::log::debug!(
            "[usd-sim] processed {processed} prim(s) in {:.2} ms",
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
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
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    topology: &mut StageJointTopology,
) {
    for path in reader.prim_paths() {
        let joint_type = reader.type_name(&path);
        let body1_target = reader.rel_target(&path, "physics:body1");
        if matches!(
            joint_type.as_deref(),
            Some(
                "PhysicsFixedJoint"
                    | "PhysicsRevoluteJoint"
                    | "PhysicsPrismaticJoint"
                    | "PhysicsSphericalJoint"
                    | "PhysicsDistanceJoint"
            )
        ) {
            let body0 = reader
                .rel_target(&path, "physics:body0")
                .and_then(|target| lunco_usd_avian::resolve_joint_body_path(reader, &target))
                .unwrap_or_default();
            let body1 = body1_target
                .clone()
                .and_then(|target| lunco_usd_avian::resolve_joint_body_path(reader, &target))
                .unwrap_or_default();
            let is_physical_wheel_joint = joint_type.as_deref() == Some("PhysicsRevoluteJoint")
                && body1_target.as_deref().is_some_and(|target| {
                    SdfPath::new(target)
                        .ok()
                        .is_some_and(|wheel| reader.has_api_schema(&wheel, "PhysxVehicleWheelAPI"))
                });
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
        }
        if joint_type.as_deref() == Some("PhysicsRevoluteJoint") {
            if let Some(body1) = body1_target {
                debug!("USD joint dispatch: {} → wheel {}", path.as_str(), body1);
                let is_vehicle_wheel = SdfPath::new(&body1)
                    .ok()
                    .is_some_and(|wheel| reader.has_api_schema(&wheel, "PhysxVehicleWheelAPI"));
                if is_vehicle_wheel {
                    topology
                        .joint_targets
                        .insert(body1, path.as_str().to_string());
                    if let Some(body0) = reader.rel_target(&path, "physics:body0") {
                        topology.articulation_roots.insert(body0);
                    }
                }
            }
        }
    }

    let attachments = wheel_params::collect_wheel_attachment_topology(reader);
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
#[allow(clippy::too_many_arguments)]
/// Collect behavior-tree program children below a vehicle, including a
/// namespace such as `OBC`. Program discovery is capability-based and recursive;
/// the namespace's spelling and depth are authoring choices, not runtime rules.
/// The source arm and backend come from the shared USD program resolver.
fn collect_behavior_sources(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    parent: &SdfPath,
    out: &mut Vec<(String, Option<String>, Option<String>)>,
) {
    if !reader.is_active(parent) {
        return;
    }
    for child in reader.children(parent) {
        // Inactive composed prims are not part of the scene contract. Do not
        // recurse through them: an inactive vessel may still carry a mission
        // program in a referenced layer, but that program must not be
        // projected onto an active assembly or vessel.
        if !reader.is_active(&child) {
            continue;
        }
        if reader.has_api_schema(&child, "LunCoProgramAPI") {
            match lunco_usd_bevy::program::resolve_behavior_tree_source(reader, &child) {
                Ok(Some(lunco_usd_bevy::program::BehaviorTreeSource::Code(xml))) => {
                    out.push((child.as_str().to_string(), Some(xml), None))
                }
                Ok(Some(lunco_usd_bevy::program::BehaviorTreeSource::Asset(path))) => {
                    out.push((child.as_str().to_string(), None, Some(path)))
                }
                Ok(_) => {}
                Err(issue) => warn!(
                    "[usd-sim] behavior program {} is unresolved at {}: {}",
                    child.as_str(),
                    issue.property,
                    issue.message
                ),
            }
        }
        collect_behavior_sources(reader, &child, out);
    }
}

fn read_gear_drive_real(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    prim: &SdfPath,
    name: &str,
    default: f64,
    allow_infinity: bool,
) -> Result<f64, ()> {
    match reader.real(prim, name) {
        Some(value) if value.is_finite() || (allow_infinity && value == f64::INFINITY) => Ok(value),
        Some(_) => Err(()),
        None if reader.has_authored_attribute(prim, name) => Err(()),
        None => Ok(default),
    }
}

pub(crate) fn is_gear_drive(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    prim: &SdfPath,
) -> bool {
    reader.type_name(prim).as_deref() == Some("PhysxPhysicsGearJoint")
        && reader.has_api_schema(prim, "PhysicsDriveAPI:angular")
}

pub(crate) fn read_gear_ratio(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    prim: &SdfPath,
) -> Option<f64> {
    reader
        .real(prim, "physxGearJoint:gearRatio")
        .filter(|value| value.is_finite() && *value != 0.0)
}

fn read_gear_drive_values(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    prim: &SdfPath,
) -> Result<(f64, f64, f64, f64, f64), ()> {
    let rest_offset = read_gear_drive_real(
        reader,
        prim,
        "drive:angular:physics:targetPosition",
        0.0,
        false,
    )?;
    let target_velocity = read_gear_drive_real(
        reader,
        prim,
        "drive:angular:physics:targetVelocity",
        0.0,
        false,
    )?;
    let stiffness =
        read_gear_drive_real(reader, prim, "drive:angular:physics:stiffness", 0.0, false)?;
    let damping = read_gear_drive_real(reader, prim, "drive:angular:physics:damping", 0.0, false)?;
    let max_force = read_gear_drive_real(
        reader,
        prim,
        "drive:angular:physics:maxForce",
        f64::INFINITY,
        true,
    )?;
    if stiffness < 0.0 || damping < 0.0 || max_force < 0.0 {
        return Err(());
    }
    Ok((rest_offset, target_velocity, stiffness, damping, max_force))
}

fn read_gear_drive_type(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    prim: &SdfPath,
) -> Option<DifferentialDriveType> {
    match reader.text(prim, "drive:angular:physics:type") {
        Some(value) if value == "force" => Some(DifferentialDriveType::Force),
        Some(value) if value == "acceleration" => Some(DifferentialDriveType::Acceleration),
        Some(_) => None,
        None if reader.has_authored_attribute(prim, "drive:angular:physics:type") => None,
        None => Some(DifferentialDriveType::Force),
    }
}

#[cfg(test)]
mod gear_drive_tests {
    use super::{read_gear_drive_type, read_gear_drive_values, DifferentialDriveType};
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};
    use openusd::sdf::Path as SdfPath;

    const FIXTURE: &str = r#"#usda 1.0
def PhysxPhysicsGearJoint "Differential" (
    prepend apiSchemas = ["PhysicsDriveAPI:angular"]
)
{
    float physxGearJoint:gearRatio = -1.0
    float drive:angular:physics:targetPosition = 0.25
    float drive:angular:physics:targetVelocity = 0.5
    float drive:angular:physics:stiffness = 8000.0
    float drive:angular:physics:damping = 1200.0
    float drive:angular:physics:maxForce = 100.0
    uniform token drive:angular:physics:type = "force"
}
"#;

    #[test]
    fn reads_standard_angular_drive_parameters_without_solver_defaults() {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("gear.usda", FIXTURE))
            .expect("gear fixture composes");
        let view = stage.view();
        let path = SdfPath::new("/Differential").expect("gear path");
        assert_eq!(
            read_gear_drive_values(&view, &path).expect("drive values"),
            (0.25, 0.5, 8000.0, 1200.0, 100.0)
        );
        assert_eq!(
            read_gear_drive_type(&view, &path),
            Some(DifferentialDriveType::Force)
        );
    }

    #[test]
    fn rejects_negative_authored_drive_coefficients() {
        let source = FIXTURE.replace("damping = 1200.0", "damping = -1.0");
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("gear.usda", &source))
            .expect("gear fixture composes");
        let view = stage.view();
        let path = SdfPath::new("/Differential").expect("gear path");
        assert!(read_gear_drive_values(&view, &path).is_err());
    }
}

fn read_authored_camera_look_at(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
) -> Result<Option<[f64; 3]>, ()> {
    if !reader.has_authored_attribute(path, "lunco:cameraLookAt") {
        return Ok(None);
    }
    match lunco_usd_bevy::read_vec3_f64(reader, path, "lunco:cameraLookAt") {
        Some(value) if value.iter().all(|value| value.is_finite()) => Ok(Some(value)),
        _ => Err(()),
    }
}

fn read_avatar_flight_settings(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
) -> Result<AvatarFlightSettings, String> {
    let defaults = AvatarFlightSettings::default();
    let read = |name: &str, default: f64| -> Result<f64, String> {
        match reader.real_f32(path, name) {
            Some(value) if value.is_finite() => Ok(value as f64),
            Some(_) => Err(format!("{name} must be finite")),
            None if reader.has_authored_attribute(path, name) => {
                Err(format!("{name} must be a finite real"))
            }
            None => Ok(default),
        }
    };
    let settings = AvatarFlightSettings {
        speed_mps: read("lunco:avatar:flightSpeed", defaults.speed_mps)?,
        boost_multiplier: read("lunco:avatar:boostMultiplier", defaults.boost_multiplier)?,
        boost_threshold: read("lunco:avatar:boostThreshold", defaults.boost_threshold)?,
        input_deadzone: read("lunco:avatar:inputDeadzone", defaults.input_deadzone)?,
    };
    if !settings.speed_mps.is_finite() || settings.speed_mps <= 0.0 {
        return Err("lunco:avatar:flightSpeed must be finite and greater than zero".into());
    }
    if !settings.boost_multiplier.is_finite() || settings.boost_multiplier < 1.0 {
        return Err("lunco:avatar:boostMultiplier must be finite and at least one".into());
    }
    if !settings.boost_threshold.is_finite() || !(0.0..=1.0).contains(&settings.boost_threshold) {
        return Err("lunco:avatar:boostThreshold must be finite and within [0, 1]".into());
    }
    if !settings.input_deadzone.is_finite() || !(0.0..1.0).contains(&settings.input_deadzone) {
        return Err("lunco:avatar:inputDeadzone must be finite and within [0, 1)".into());
    }
    Ok(settings)
}

fn read_raycast_observation(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
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
    let offset = match lunco_usd_bevy::read_vec3_f64(reader, path, "lunco:raycast:offset") {
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
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};
    use openusd::sdf::Path as SdfPath;

    fn read(source: &str) -> Result<lunco_cosim::avian_queries::RaycastObservation, ()> {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("ray.usda", source))
            .expect("raycast fixture composes");
        let path = SdfPath::new("/Sensor").expect("raycast path");
        read_raycast_observation(&stage.view(), &path)
    }

    #[test]
    fn malformed_authored_offset_is_rejected() {
        assert!(read(
            r#"#usda 1.0
def Xform "Sensor" (prepend apiSchemas = ["LunCoRaycastAPI"])
{
    string lunco:raycast:offset = "bad"
}
"#
        )
        .is_err());
    }

    #[test]
    fn non_positive_authored_distance_is_rejected() {
        assert!(read(
            r#"#usda 1.0
def Xform "Sensor" (prepend apiSchemas = ["LunCoRaycastAPI"])
{
    float lunco:raycast:maxDistance = 0.0
}
"#
        )
        .is_err());
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
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
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
    all_prims: &Query<(Entity, &UsdPrimPath, Option<&Transform>)>,
    q_child_of: &Query<&ChildOf>,
    grid_components: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
    active_sun: Option<&lunco_environment::LunarSun>,
    input_map: &leafwing_input_manager::prelude::InputMap<lunco_core::UserIntent>,
    commands: &mut Commands,
    diagnostics: &mut Vec<lunco_core::RuntimeDiagnostic>,
) {
    let existing_tf = maybe_tf.cloned().unwrap_or_default();
    match raycast_mass_contribution_from_usd(
        reader,
        &sdf_path,
        prim_path.stage_handle.id(),
        all_prims,
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
    let is_avatar =
        match lunco_usd_bevy::read_authored_bool_strict(reader, &sdf_path, "lunco:avatar") {
            Ok(Some(value)) => value,
            Ok(None) => false,
            Err(_) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "avatar-attribute",
                    "lunco:avatar must be an authored boolean",
                );
                warn!(
                    "USD prim {} has malformed `lunco:avatar`; prim ignored",
                    prim_path.path
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        };
    let avatar_exposure = if is_avatar {
        match lunco_usd_bevy::read_camera_exposure_ev100(reader, &sdf_path) {
            Ok(exposure) => exposure,
            Err(_) => {
                // An invalid authored exposure is a broken camera contract, not
                // an invitation to replace it with a calibrated value. Mark the
                // prim complete so the scene does not retry the same bad opinion.
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "avatar-exposure",
                    "avatar camera exposure must be a finite EV100 value",
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        }
    } else {
        None
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
            .try_insert(lunco_core::ArticulatedVehicle);
    }
    if topology.joint_targets.contains_key(&prim_path.path) {
        commands
            .entity(entity)
            .try_insert(lunco_core::ArticulatedLink);
    }
    // Screen-facing label the PRIM asked for. Opt-in: only a prim that
    // authors `lunco:billboard = true` gets one, so adding the schema can
    // never make an existing scene sprout labels.
    let billboard_enabled =
        match lunco_usd_bevy::read_authored_bool_strict(reader, &sdf_path, "lunco:billboard") {
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
        let default = billboard::UsdBillboard::default();
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
            Ok(billboard::UsdBillboard {
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
    let waypoint =
        match lunco_usd_bevy::read_authored_bool_strict(reader, &sdf_path, "lunco:waypoint") {
            Ok(Some(value)) => value,
            Ok(None) => false,
            Err(_) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "waypoint-attribute",
                    "lunco:waypoint must be an authored boolean",
                );
                warn!(
                    "USD prim {} has malformed `lunco:waypoint`; marker ignored",
                    prim_path.path
                );
                false
            }
        };
    if waypoint {
        commands.entity(entity).try_insert(marker::WaypointMarker);
    }
    // Waypoint arrival is session state. Capture the composed active/inactive
    // looks on the projected visual child so `marker::sync_waypoint_visuals` can
    // update appearance in ECS without authoring a live USD material edit. A
    // material edit would rebuild every bound visual and tear down active
    // co-simulation participants while a route is running.
    if let Some(material) = maybe_mat {
        let mut owner = sdf_path.as_str().to_string();
        let marker_path = loop {
            let owner_sdf = SdfPath::new(&owner).ok();
            if owner_sdf
                .as_ref()
                .is_some_and(|path| reader.has_api_schema(path, "LunCoWaypointAPI"))
            {
                break Some(owner);
            }
            if owner == "/" {
                break None;
            }
            owner = owner
                .rsplit_once('/')
                .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
                .unwrap_or("/")
                .to_string();
        };
        if let Some(marker_path) = marker_path {
            if let Some(inactive) = lunco_usd_bevy::read_vec3_f64(
                reader,
                &SdfPath::new(&marker_path).expect("validated waypoint marker path"),
                "lunco:waypoint:inactiveColor",
            ) {
                if inactive.iter().all(|value| value.is_finite()) {
                    let mut inactive_look = material.clone();
                    inactive_look.base_color = LinearRgba::new(
                        inactive[0] as f32,
                        inactive[1] as f32,
                        inactive[2] as f32,
                        material.base_color.alpha,
                    );
                    inactive_look.emissive = LinearRgba::new(
                        inactive[0] as f32,
                        inactive[1] as f32,
                        inactive[2] as f32,
                        material.emissive.alpha,
                    );
                    commands
                        .entity(entity)
                        .try_insert(marker::WaypointVisualLook {
                            active: material.clone(),
                            inactive: inactive_look,
                            marker_path,
                        });
                }
            }
        }
    }
    // Pointer behavior is scene intent, not a picking-backend concern.  The
    // render-free USD projection records it here; the GUI layer later maps the
    // primary-button pass-through part to Bevy's `Pickable` component.  This
    // keeps transparent markers usable by every scene and preserves the same
    // contract for future marker assets.
    if let Some(policy) = lunco_core::ScenePointerPolicy::from_usd(
        reader.text(&sdf_path, "lunco:interaction:left").as_deref(),
        reader.text(&sdf_path, "lunco:interaction:right").as_deref(),
    ) {
        commands.entity(entity).try_insert(policy);
    }

    // Physical actuators are generic USD descriptions. A force actuator and a
    // torque actuator publish ordinary scalar input ports; the cosim backend
    // later resolves those commands to Avian's force/torque writer. RCS names,
    // reaction-wheel names, and controller ownership do not appear here.
    if let Some(actuator) = force_actuator_from_usd(reader, &sdf_path) {
        commands.entity(entity).try_insert(actuator);
    }
    if let Some(actuator) = torque_actuator_from_usd(reader, &sdf_path) {
        commands.entity(entity).try_insert(actuator);
    }
    // Screen-constant marker, keyed on the size that IS the request: a prim
    // authoring no `angularSizeDeg` is not a half-declared marker, it is simply
    // not one. Same opt-in shape as the billboard above.
    if reader.has_authored_attribute(&sdf_path, "lunco:marker:angularSizeDeg") {
        let default = marker::ScreenConstantMarker::default();
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
            Ok(marker::ScreenConstantMarker {
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
        commands.entity(entity).try_insert(lunco_core::NetExcluded);
    }
    if net_opaque {
        commands
            .entity(entity)
            .try_insert(lunco_core::NotPredictable);
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
                commands.entity(entity).try_insert(observation);
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

    // (Link/celestial vocabulary is projected by the independent
    // `project_celestial_comms_prims` system, NOT here — see its doc. Bundling it
    // in this system made a cosim prim, which skips this system, lose its LinkNode.)

    // 0. Avatar role and photographic exposure were validated before any
    // per-prim simulation components were projected.
    if is_avatar {
        info!(
            "Detected Avatar prim at {}, setting up camera",
            prim_path.path
        );
        // PRIOR AVATARS are not this code's problem. `LocalAvatar` is singular
        // by construction (`lunco_core`'s component hook): inserting it below
        // demotes whatever held it, and `lunco_avatar::demote_former_avatar`
        // strips that entity's camera/control roles and deactivates its camera.
        // This used to be a loop right here, which is precisely why the OTHER
        // ways an avatar appears (an explicit host camera, a recomposed prim)
        // could leave a second live one.
        // `token`, per luncoSchema — so `text`, not `scalar::<String>`, which
        // matches `Value::String` alone and reads every token as `None`.
        // `LunCoAvatarAPI` declares `freeflight` as the USD schema fallback.
        // That is an authored semantic default, not a Rust recovery path: a
        // malformed token must remain an explicit scene error below.
        let camera_mode = match reader.attr_value(&sdf_path, "lunco:cameraMode") {
            Some(Value::Token(value)) => {
                let value = value.to_string();
                if matches!(value.as_str(), "freeflight" | "orbit" | "springarm") {
                    value
                } else {
                    push_usd_sim_diagnostic(
                        diagnostics,
                        &prim_path.path,
                        "camera-mode",
                        format!(
                            "avatar camera mode `{value}` is unsupported; use `freeflight`, `orbit`, or `springarm`"
                        ),
                    );
                    warn!(
                        "USD avatar {} has unsupported camera mode `{}`; avatar ignored",
                        prim_path.path, value
                    );
                    commands.entity(entity).try_insert(UsdSimProcessed);
                    return;
                }
            }
            Some(_) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "camera-mode",
                    "lunco:cameraMode must be a token: `freeflight`, `orbit`, or `springarm`",
                );
                warn!(
                    "USD avatar {} has malformed `lunco:cameraMode`; avatar ignored",
                    prim_path.path
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
            None if reader.has_authored_attribute(&sdf_path, "lunco:cameraMode") => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "camera-mode",
                    "authored lunco:cameraMode is malformed",
                );
                warn!(
                    "USD avatar {} has malformed `lunco:cameraMode`; avatar ignored",
                    prim_path.path
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
            None => "freeflight".to_string(),
        };
        let read_camera_real = |name: &str, default_value: f32| -> Result<f32, ()> {
            match reader.real_f32(&sdf_path, name) {
                Some(value) if value.is_finite() => Ok(value),
                Some(_) => Err(()),
                None if reader.has_authored_attribute(&sdf_path, name) => Err(()),
                None => Ok(default_value),
            }
        };
        let mut yaw = match read_camera_real("lunco:cameraYaw", std::f32::consts::PI * 0.8) {
            Ok(value) => value,
            Err(()) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "camera-yaw",
                    "authored lunco:cameraYaw must be finite",
                );
                warn!(
                    "USD avatar {} has malformed `lunco:cameraYaw`; avatar ignored",
                    prim_path.path
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        };
        let mut pitch = match read_camera_real("lunco:cameraPitch", -0.3) {
            Ok(value) => value,
            Err(()) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "camera-pitch",
                    "authored lunco:cameraPitch must be finite",
                );
                warn!(
                    "USD avatar {} has malformed `lunco:cameraPitch`; avatar ignored",
                    prim_path.path
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        };

        // `lunco:cameraLookAt` (double3, scene-local): when authored,
        // derive yaw/pitch so the camera aims from its USD
        // `xformOp:translate` toward this point on start. Overrides any
        // authored `lunco:cameraYaw`/`lunco:cameraPitch` — expressing
        // "look at the main object" as a target point is more maintainable
        // than hand-tuned angles (move the camera or the object and the
        // aim stays correct). The math inverts `freeflight_system`'s
        // `Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0)`, whose forward
        // is `(-sin(yaw)·cos(pitch), sin(pitch), -cos(yaw)·cos(pitch))`:
        //   pitch = asin(dir.y),  yaw = atan2(-dir.x, -dir.z).
        let look_at = match read_authored_camera_look_at(reader, &sdf_path) {
            Ok(value) => value,
            Err(()) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "camera-look-at",
                    "authored lunco:cameraLookAt must be a finite double3",
                );
                warn!(
                    "USD avatar {} has malformed `lunco:cameraLookAt`; avatar ignored",
                    prim_path.path
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        };
        if let Some([lx, ly, lz]) = look_at {
            // The EYE must be the avatar's authored position, not `existing_tf`:
            // `maybe_tf` is `None` on this path, so `existing_tf` defaults to the
            // origin, and aiming from (0,0,0) instead of (e.g.) (14,6,12) points the
            // camera up at the sky. Read `xformOp:translate` directly.
            let eye = lunco_usd_bevy::read_vec3_f64(reader, &sdf_path, "xformOp:translate")
                .map(|[x, y, z]| DVec3::new(x, y, z))
                .unwrap_or(existing_tf.translation.as_dvec3());
            let dir = DVec3::new(lx, ly, lz) - eye;
            if let Some(n) = dir.try_normalize() {
                pitch = (n.y.clamp(-1.0, 1.0)).asin() as f32;
                yaw = (-n.x).atan2(-n.z) as f32;
            }
        }

        let flight_settings = match read_avatar_flight_settings(reader, &sdf_path) {
            Ok(settings) => settings,
            Err(error) => {
                push_usd_sim_diagnostic(
                    diagnostics,
                    &prim_path.path,
                    "avatar-flight-settings",
                    error.clone(),
                );
                warn!(
                    "USD avatar {} has malformed flight settings: {}",
                    prim_path.path, error
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        };

        // Avatar position from the LIVE composed scene hierarchy. The USD
        // transform is local to its authored parent (`/Traverse` here). Resolve
        // the nearest actual Grid in that parent chain: this
        // is the scene's frame owner (WorldGrid during bootstrap, or the body's
        // surface grid after celestial placement), never a marker-selected
        // parallel grid. Resolve the pose directly in that Grid's frame; a
        // root-world compose followed by an immediate inverse conversion is
        // both unnecessary and unstable when the distant root is re-pinned.
        let (grid_entity, grid) = lunco_core::coords::ancestor_grid(
            entity,
            q_child_of,
            grid_components,
        )
        .unwrap_or_else(|| {
            panic!(
                "USD avatar {sdf_path} is not below a BigSpace Grid; an explicit spatial frame is required"
            )
        });
        let (position, rotation) = lunco_core::coords::grid_relative_pose(
            entity,
            grid_entity,
            q_child_of,
            grid_components,
            q_spatial,
        )
        .unwrap_or_else(|| {
            panic!("USD avatar {sdf_path} has an invalid spatial chain to Grid {grid_entity:?}")
        });
        let (avatar_cell, translation) = grid.translation_to_grid(position);
        let avatar_tf = Transform::from_translation(translation)
            .with_rotation(rotation.as_quat())
            .with_scale(existing_tf.scale);

        // Shared render-look for the avatar camera: SMAA post-process AA,
        // MSAA off (can't touch shader-internal regolith speckle), and
        // physical lunar exposure (ev100 15 ≈ SUNLIGHT) to pair with the
        // ~128k lx sun. Same look as the standard scene camera; without it
        // a USD-authored Avatar camera renders at Blender-default ev9.7 and
        // the lunar terrain blows out. Tune live via SetEnvironmentLight.
        // Render-look for the avatar camera: physical exposure read from the
        // active-scene `LunarSun` resource — the SAME source as the sun
        // illuminance, so lux and EV move together (the point of bundling
        // them). A dimmed sun can therefore never leave the camera mis-
        // exposed (that mismatch blacked the viewport once).
        //
        // NB: NO SMAA here. SMAA is a per-camera post-process whose resolve
        // does not survive the workbench's full-window-3D + egui-overlay
        // compositing (egui paints over with `ClearColorConfig::None`), so a
        // workbench camera with `Smaa` renders a blank/black viewport — and
        // without the `smaa_luts` feature it additionally drops every frame
        // on a wgpu bind-group validation error. Both failure modes look like
        // a lighting/camera bug. Keep workbench cameras SMAA-free; MSAA (from
        // `SceneCamera`, bound by `lunco-render-bevy`) handles geometry-edge AA.
        // An authored camera is authoritative over the calibrated scene default.
        // Both paths use the one USD photographic conversion, so ISO/shutter/
        // f-stop never acquire a second spelling at the avatar boundary.
        let ev100 = avatar_exposure
            .unwrap_or_else(|| active_sun.copied().unwrap_or_default().exposure_ev100);
        // AgX tonemapping: a filmic curve that rolls off the blown highlights
        // and lifts the toe of the brutal grazing-sun terminator (vs the hard
        // clip that read as pure white/black), while keeping the realistic
        // high-contrast lunar exposure (ev100 stays lunar-calibrated).
        let camera_look = move || {
            (
                bevy::camera::Exposure { ev100 },
                // Camera INTENT: `lunco-render-bevy` binds `Camera3d` + its
                // render graph + `Tonemapping::AgX` + MSAA. Render-free here,
                // and it is what every "which entity is the scene camera?"
                // query filters on.
                SceneCamera::agx(),
                // This avatar camera is renderer-owned intent. The render
                // binder must keep it synchronized with live Graphics settings
                // just like canonical USD and native avatar cameras.
                GraphicsCameraDefaults,
            )
        };

        // Build the avatar camera in its explicit scene Grid. BigSpace's
        // persistent OriginAnchor owns FloatingOrigin; camera role and origin
        // ownership are separate contracts.
        match camera_mode.as_str() {
            "freeflight" => {
                commands.entity(entity).try_insert((
                    camera_look(),
                    FreeFlightCamera {
                        yaw,
                        pitch,
                        damping: None,
                    },
                    AdaptiveNearPlane,
                    avatar_tf,
                    avatar_cell,
                    Avatar,
                    LocalAvatar,
                    IntentAnalogState::default(),
                    ActionState::<lunco_core::UserIntent>::default(),
                    input_map.clone(),
                ));
            }
            "orbit" => {
                commands.entity(entity).try_insert((
                    camera_look(),
                    OrbitCamera {
                        target: Entity::PLACEHOLDER,
                        distance: 30.0,
                        yaw,
                        pitch,
                        damping: None,
                        vertical_offset: 0.0,
                    },
                    AdaptiveNearPlane,
                    avatar_tf,
                    avatar_cell,
                    Avatar,
                    LocalAvatar,
                    IntentAnalogState::default(),
                    ActionState::<lunco_core::UserIntent>::default(),
                    input_map.clone(),
                ));
            }
            "springarm" => {
                commands.entity(entity).try_insert((
                    camera_look(),
                    SpringArmCamera {
                        target: Entity::PLACEHOLDER,
                        distance: 15.0,
                        yaw,
                        pitch,
                        damping: None,
                        vertical_offset: 2.0,
                        // Authored chase cams target steerable vehicles.
                        track_heading: true,
                        attitude: lunco_avatar::FollowAttitude::Heading,
                    },
                    avian3d::prelude::TranslationInterpolation,
                    avian3d::prelude::RotationInterpolation,
                    AdaptiveNearPlane,
                    avatar_tf,
                    avatar_cell,
                    Avatar,
                    LocalAvatar,
                    IntentAnalogState::default(),
                    ActionState::<lunco_core::UserIntent>::default(),
                    input_map.clone(),
                ));
            }
            _ => {
                error!(
                    "Unknown camera mode '{}' for avatar at {}; refusing to create an avatar controller (allowed: freeflight, orbit, springarm)",
                    camera_mode, prim_path.path
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        }
        // This applies to both `def Camera` and `def Xform` avatar prims. The
        // avatar controller is the sole pose writer; a hierarchy-derived mount
        // follower must never claim it during an async reload.
        commands
            .entity(entity)
            .try_insert((lunco_usd_bevy::UsdCameraPose::Avatar, flight_settings));
        // Keep the camera in the scene's actual frame owner. The nearest Grid
        // was selected above from the authored parent chain, so this is not a
        // second celestial frame and does not detach the camera from the rover
        // scene during bootstrap or body-surface rebranching. The pose was
        // already composed in that Grid; commit the complete spatial handoff
        // through the shared migration boundary.
        lunco_core::attach::migrate_to_grid(commands, entity, grid_entity, avatar_cell, avatar_tf);
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
        let mut port_names: Vec<String> = Vec::new();
        // A port is an authored numeric `outputs:` attribute, the same way a
        // command is an `inputs:` attribute. This supports conventional
        // drive_left/drive_right/steering/brake names and arbitrary per-wheel
        // channels without a Rust-side hard-coded vocabulary.
        for attr in reader.attr_names(&sdf_path) {
            let Some(name) = attr.strip_prefix("outputs:") else {
                continue;
            };
            // NUMERIC outputs only. `outputs:` is UsdShade's namespace too, so a
            // vessel root that also carries a material network would otherwise
            // mint a phantom actuator port from `token outputs:surface`. An
            // actuator port carries a number; a shader terminal does not.
            if reader.real(&sdf_path, &attr).is_none() {
                continue;
            }
            if lunco_usd_bevy::program::is_network_boundary_output(reader, &sdf_path, &attr) {
                continue;
            }
            if !port_names.iter().any(|n| n == name) {
                port_names.push(name.to_string());
            }
        }
        commands
            .entity(entity)
            .try_insert((lunco_core::SelectableRoot, lunco_core::MobilityRoot))
            .remove::<lunco_core::OutputPorts>();

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
                .try_insert(lunco_core::OutputPorts::new(port_map));
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

    // 1b. Mission behaviour: a BT.CPP v4 XML tree, carried by a program-API
    // child of this prim — the vessel OWNS the tree, so the tree is read from
    // here, its owner. Inline source wins over a file: an author editing a tree in
    // place means it. The tree's spatial leaves reference WAYPOINT PRIMS by path;
    // `resolve_behavior_targets` binds those, and `lunco_autopilot::usd_tree` bakes
    // their live positions into the compiled tree.
    //
    // A `.btxml` (canonical) or interoperable `.xml` is the one program with a role
    // of its own: a declarative tree is
    // not a script, it is compiled and ticked by the behaviour engine. Extension
    // picks the engine, exactly as it does for `.mo` and `.rhai`.
    let mut behavior_sources: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    // A behavior program belongs to the prim that owns the command surface. A
    // scene/root may contain the vessel as a descendant, but recursively scanning
    // every prim would attach the descendant mission to that root and leave the
    // actual vessel without an autopilot. `Controls` is the authored, generic
    // ownership boundary shared by vehicles and other controllable assemblies.
    let owns_control_surface = reader
        .children(&sdf_path)
        .into_iter()
        .any(|child| child.name() == Some("Controls"));
    if owns_control_surface {
        if reader.has_api_schema(&sdf_path, "PhysxVehicleContextAPI") {
            // Steering geometry is a required authored capability of every
            // controllable vehicle. It is read at the same ownership boundary as
            // Controls, so native and scripted navigation cannot silently disagree
            // about whether the body can pivot or must roll through a turn.
            match reader
                .text(&sdf_path, "lunco:steeringGeometry")
                .and_then(|value| lunco_core::parse_steering_geometry(&value))
            {
                Some(geometry) => {
                    commands.entity(entity).try_insert(geometry);
                }
                None => {
                    commands
                        .entity(entity)
                        .remove::<lunco_core::SteeringGeometry>();
                    push_usd_sim_diagnostic(
                        diagnostics,
                        &prim_path.path,
                        "missing-steering-geometry",
                        "controllable vehicle must author lunco:steeringGeometry as differential or ackermann",
                    );
                }
            }
        }
        collect_behavior_sources(reader, &sdf_path, &mut behavior_sources);
    }
    // A BT.CPP file may contain several named BehaviorTree definitions; its
    // `main_tree_to_execute` is the explicit selection for that file. Several
    // sibling BT program children are different controllers, not an implicit
    // priority list: choosing one by traversal order would make Safety/Mission
    // arbitration a hidden last-writer race. Keep the projection fail-closed
    // until those programs are connected through an authored port arbiter.
    behavior_sources.sort_by(|a, b| a.0.cmp(&b.0));
    if behavior_sources.len() > 1 {
        warn!(
            "USD prim {} carries {} BT program children; no tree projected until an authored port arbiter selects one",
            prim_path.path,
            behavior_sources.len()
        );
    }
    if behavior_sources.len() != 1 {
        commands
            .entity(entity)
            .remove::<lunco_autopilot::usd_tree::BehaviorXml>()
            .remove::<lunco_autopilot::usd_tree::BehaviorXmlPath>()
            .remove::<lunco_autopilot::usd_tree::BehaviorXmlHandle>()
            .remove::<lunco_autopilot::usd_tree::BehaviorProgramSource>();
    }
    let selected_behavior = (behavior_sources.len() == 1)
        .then(|| behavior_sources.into_iter().next().expect("length checked"));
    if let Some((source_path, xml, asset)) = selected_behavior {
        if let Some(xml) = xml {
            commands
                .entity(entity)
                .try_insert((
                    lunco_autopilot::usd_tree::BehaviorXml(xml),
                    lunco_autopilot::usd_tree::BehaviorProgramSource(source_path),
                ))
                .remove::<lunco_autopilot::usd_tree::BehaviorXmlPath>()
                .remove::<lunco_autopilot::usd_tree::BehaviorXmlHandle>();
        } else if let Some(asset) = asset {
            commands
                .entity(entity)
                .try_insert((
                    lunco_autopilot::usd_tree::BehaviorXmlPath(asset),
                    lunco_autopilot::usd_tree::BehaviorProgramSource(source_path),
                ))
                .remove::<lunco_autopilot::usd_tree::BehaviorXml>()
                .remove::<lunco_autopilot::usd_tree::BehaviorXmlHandle>();
        }
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
            let Ok((rest_offset, target_velocity, stiffness, damping, max_force)) =
                read_gear_drive_values(reader, &sdf_path)
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

        // ONE unified read for BOTH wheel kinds (see `wheel_params`): every
        // drivetrain/tire/inertia number plus suspension, resolved through the
        // standard attachment relationship or explicit direct wheel/suspension
        // composition. Strict — all missing required attrs are collected and the
        // wheel refuses to spawn; the authored defaults live in
        // components/mobility/wheel.usda, which every wheel composes.
        // Read BEFORE spawning the port entities so an invalid wheel
        // synthesizes nothing.
        let attachment_susp = wheel_params::attachment_suspension_path(
            &prim_path.path,
            &topology.wheel_attachment_targets,
        );
        let attachment_tire =
            wheel_params::attachment_tire_path(&prim_path.path, &topology.wheel_attachment_tires);
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
                ("drive".to_owned(), p_drive),
                ("heading".to_owned(), p_heading),
                ("shaft_speed".to_owned(), p_speed),
            ])),
            lunco_core::PortSurfaceReady,
        ));

        // A wheel receives only the scalar signals explicitly authored on its
        // own inputs. A connected `inputs:heading` is the final wheel heading;
        // no vehicle class or wheel index is consulted.
        let is_physical = topology.joint_targets.contains_key(&prim_path.path);
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
        ) else {
            error!(
                "USD wheel {} has no resolved authored rigid-body owner — refusing to spawn",
                sdf_path.as_str()
            );
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        };
        if is_physical {
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
/// - `excluded` ⇒ stamp [`lunco_core::NetExcluded`] (skip default replication):
///   `lunco:net:replicate = false` OR `lunco:net:authority = "local"`.
/// - `opaque` ⇒ stamp [`lunco_core::NotPredictable`] (never client-predicted):
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
    all_prims: &Query<(Entity, &UsdPrimPath, Option<&Transform>)>,
    stage: bevy::asset::AssetId<UsdStageAsset>,
    path: &str,
) -> Option<Entity> {
    all_prims
        .iter()
        .find(|(_, prim, _)| prim.stage_handle.id() == stage && prim.path == path)
        .map(|(entity, _, _)| entity)
}

/// Resolve the nearest authored rigid body above a raycast wheel. The wheel
/// itself is a reusable rigid-body prim for the physical realization, so the
/// raycast realization starts at its parent and walks the composed topology to
/// the enclosing body.
fn raycast_body_path(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
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
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    wheel_path: &SdfPath,
    physical_body_path: Option<&str>,
    stage: bevy::asset::AssetId<UsdStageAsset>,
    all_prims: &Query<(Entity, &UsdPrimPath, Option<&Transform>)>,
) -> Option<lunco_mobility::WheelBodyMount> {
    let body_path = if let Some(path) = physical_body_path {
        SdfPath::new(path).ok()?
    } else {
        raycast_body_path(reader, wheel_path)?
    };
    let body = usd_entity_for_path(all_prims, stage, body_path.as_str())?;
    let local = lunco_usd_avian::transform_in_body_frame(reader, &body_path, wheel_path)?;
    Some(lunco_mobility::WheelBodyMount { body, local })
}

/// Resolve the enclosing authored vehicle frame used by heading geometry.
/// This is separate from the wheel's immediate mechanical carrier: a physical
/// wheel's carrier owns the prismatic DOF, while the authored heading program
/// is evaluated in the vehicle context frame.
fn vehicle_mount_transform(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    wheel_path: &SdfPath,
) -> Option<Transform> {
    let mut path = wheel_path.clone();
    loop {
        if reader.has_api_schema(&path, "PhysxVehicleContextAPI") {
            let wheel = lunco_usd_avian::world_transform(reader, wheel_path).ok()?;
            let vehicle = lunco_usd_avian::world_transform(reader, &path).ok()?;
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
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    prim: &SdfPath,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    all_prims: &Query<(Entity, &UsdPrimPath, Option<&Transform>)>,
) -> Result<Option<lunco_mobility::RaycastMassContribution>, String> {
    if !reader.has_api_schema(prim, "LunCoMassContributionAPI")
        || reader.has_api_schema(prim, "PhysicsRigidBodyAPI")
    {
        return Ok(None);
    }
    let mass = reader
        .real(prim, "physics:mass")
        .ok_or_else(|| "missing `physics:mass`".to_owned())?;
    let inertia = lunco_usd_bevy::read_vec3_f64(reader, prim, "physics:diagonalInertia")
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
    let convention = lunco_usd_bevy::stage_convention(reader)
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
    let owner = usd_entity_for_path(all_prims, stage_id, body_path.as_str())
        .ok_or_else(|| format!("owner entity {} is not projected", body_path.as_str()))?;
    let local = lunco_usd_avian::transform_in_body_frame(reader, &body_path, prim)
        .ok_or_else(|| "cannot resolve local transform".to_owned())?;
    let principal = convention.dir_d(DVec3::new(inertia[0], inertia[1], inertia[2]))
        * (meters_per_unit * meters_per_unit);
    Ok(Some(lunco_mobility::RaycastMassContribution {
        owner,
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
    if let Some(shader) = maybe_shader_mat.cloned() {
        visual.try_insert(shader);
    } else if let Some(material) = maybe_mat.cloned() {
        visual.try_insert(material);
    }
    if shader_bound {
        visual.try_insert(lunco_usd_bevy::UsdVisualShaderBound);
    }

    let visual_entity = visual.id();
    commands
        .entity(entity)
        .try_insert(lunco_usd_bevy::UsdVisualMeshTarget(visual_entity));
    commands
        .entity(entity)
        .remove::<Mesh3d>()
        .remove::<PbrLook>()
        .remove::<ShaderLook>()
        .remove::<lunco_usd_bevy::UsdVisualShaderBound>();
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
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    wheel_path: &UsdPrimPath,
    wheel_entity: Entity,
    wheel_tf: Transform,
    all_prims: &Query<(Entity, &UsdPrimPath, Option<&Transform>)>,
    q_child_of: &Query<&ChildOf>,
) -> Vec<(Entity, Transform)> {
    let mut visuals = Vec::new();
    for (child, child_path, maybe_child_tf) in all_prims.iter() {
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

    // Keep the rigid collider identical to the authored cylinder that produced
    // the visual mesh. `Collider::cylinder` takes the full height; deriving a
    // width from radius made the collider wider/narrower than the tire.
    let cyl = Collider::cylinder(params.radius, params.width);
    let collider = if wheel_axis_rot.abs_diff_eq(Quat::IDENTITY, 1e-5) {
        cyl
    } else {
        Collider::compound(vec![(
            Position(DVec3::ZERO),
            Rotation(wheel_axis_rot.as_dquat()),
            cyl,
        )])
    };
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

    // Wheel mass via DENSITY, not a forced `Mass` — see
    // `WheelParams::wheel_density` for why a forced mass desyncs mass from
    // angular inertia and sinks the rover through the terrain.
    let wheel_density = params.wheel_density();

    commands.entity(entity).try_insert((
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
        body_mount,
        RigidBody::Kinematic,
        ShouldBeDynamic,
        collider,
        // The authored wheel mass is applied through density so mass AND angular
        // inertia stay consistent (see the `wheel_density` note above). A forced
        // `Mass` desynced them and the rover sank through the terrain.
        avian3d::prelude::ColliderDensity(wheel_density),
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

    // The authored complete wheel-assembly MOI is the rotational contract for
    // BOTH realizations. Collider density derives the physical mass, but it
    // cannot express an authored non-solid-cylinder MOI. Stamp the complete
    // tensor even on undriven wheels; otherwise they silently use a
    // collider-derived inertia and diverge from the same wheel on the raycast
    // path.  The transverse terms remain the geometric cylinder tensor.
    commands.entity(entity).try_insert((
        physical_wheel_angular_inertia(params, wheel_axis_rot),
        avian3d::prelude::NoAutoAngularInertia,
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
        lunco_usd_avian::ScenePhysicsOwned,
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
    lunco_usd_avian::attach_joint(
        commands,
        joint_entity,
        carrier,
        entity,
        lunco_usd_avian::wheel_revolute_joint(carrier, entity, mount_local, axle),
    );

    // The wheel's `WheelBodyMount` is the canonical physics ownership boundary.
    // `ChildOf` remains the authored transform/despawn hierarchy; it is not used
    // to infer which body receives wheel torque, suspension, or mass.
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
    role: Option<Res<lunco_core::NetworkRole>>,
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
        Without<lunco_core::OwnedLocally>,
    >,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Some(role) = role else { return };
    if !matches!(*role, lunco_core::NetworkRole::Client) {
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
        Without<lunco_core::NetReplicate>,
    >,
    q_chassis: Query<
        (
            &RigidBody,
            &Position,
            &Rotation,
            &ComputedCenterOfMass,
            Option<&lunco_core::ReplicatedChassisMotion>,
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
                visual_tf.rotation =
                    (Quat::from_axis_angle(axle, wheel.spin_angle) * wheel.axis_rot).normalize();
            }
        }
    }
}

/// Marker to indicate a prim has been processed by the sim system.
#[derive(Component)]
pub struct UsdSimProcessed;

/// Allow a live prim to be projected again after its composed simulation
/// schemas change.
///
/// A runtime reference can initially arrive as a typeless visual root while
/// its referenced layer closure is still loading.  The sim projector marks
/// that root processed, so a later schema resync must clear the marker before
/// the normal projection pass can publish its authored control surface,
/// wheel wiring, or other simulation components.
pub fn invalidate_usd_sim_projection(world: &mut World, entity: Entity) -> bool {
    if world.get::<UsdSimProcessed>(entity).is_none() {
        return false;
    }
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return false;
    };
    entity_mut.remove::<UsdSimProcessed>();
    true
}

/// Marker: this prim's link/celestial vocabulary has been projected to components.
#[derive(Component)]
struct CelestialProjected;

fn any_unprojected_celestial(
    q: Query<(), (With<UsdPrimPath>, Without<CelestialProjected>)>,
) -> bool {
    !q.is_empty()
}

fn install_authored_sun_state_seed(app: &mut App) {
    app.add_systems(
        PostUpdate,
        seed_authored_sun_state
            .after(TransformSystems::Propagate)
            .before(lunco_environment::finalize_sun_render_state),
    );
}

/// Seed the semantic sun provider from an authored USD `DistantLight` in
/// scenes that do not declare a celestial site. The light's composed world
/// rotation is the USD fact; `DirectionalLight` is used only for the authored
/// illuminance carried by the same light. The direction is projected from the
/// light's composed world rotation into the explicitly bound physics frame,
/// so a parent transform cannot silently change the semantic axes. Ephemeris
/// owns SunState once a site exists, and this system never overwrites a
/// semantic sample already published by a provider or command.
fn seed_authored_sun_state(
    sun_state: Option<ResMut<lunco_environment::SunState>>,
    active_frame: Option<Res<lunco_core::ActivePhysicsFrame>>,
    q_frames: Query<&GlobalTransform>,
    q_suns: Query<
        (&GlobalTransform, &bevy::light::DirectionalLight),
        (
            With<lunco_usd_bevy::UsdAuthoredLight>,
            Without<lunco_environment::Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
) {
    let Some(mut sun_state) = sun_state else {
        return;
    };
    if sun_state.direction_to_sun.is_some() || q_suns.iter().count() != 1 {
        return;
    }
    let Some(active_frame) = active_frame else {
        return;
    };
    let Ok(frame_gt) = q_frames.get(active_frame.0) else {
        return;
    };
    let Ok((global_transform, light)) = q_suns.single() else {
        return;
    };
    let emit_direction_world = global_transform.rotation() * Vec3::NEG_Z;
    let direction_to_sun = frame_gt
        .rotation()
        .inverse()
        .mul_vec3(-emit_direction_world);
    if !direction_to_sun.is_finite() || direction_to_sun.length_squared() < 1.0e-12 {
        return;
    }
    sun_state.publish(direction_to_sun.normalize(), Some(light.illuminance));
}

/// Project a prim's USD-authored link/celestial vocabulary (geodetic anchors, Kepler
/// orbits, link nodes, occluders) to `lunco-celestial` components — as its OWN system,
/// independent of `process_usd_sim_prims` (wheels/joints/avatar) and
/// `process_usd_cosim_prims` (behaviour models).
///
/// These concerns are ORTHOGONAL: an antenna can be a link node AND run `CommsLink.mo`;
/// a lander can anchor to a site AND run guidance. It used to live inside
/// `process_usd_sim_prims`, so a cosim prim — which stamps `UsdSimProcessed` to skip that
/// system — silently lost its `LinkNode` and never joined the link graph. One projector,
/// one marker: every prim gets link/celestial projection exactly once and no projector
/// blocks another, the way USD API schemas compose.
fn project_celestial_comms_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<CelestialProjected>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
) {
    for (entity, prim_path) in query.iter() {
        // A scene mounted without an explicit root uses the empty path as the
        // documented defaultPrim-resolution sentinel.  The USD visual
        // projector replaces it with the concrete composed path once the stage
        // is loaded.  Do not consume the celestial projection marker while the
        // path is unresolved: the root carries the scene's SiteAnchor, so
        // marking it here would permanently lose the only frame from which
        // scene-local link nodes can derive their solar poses.
        if prim_path.path.is_empty() {
            continue;
        }
        let id = prim_path.stage_handle.id();
        let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(id, stage_asset);
        let Some(resolved_path) = resolve_stage_prim_path(&reader, &prim_path.path) else {
            warn!(
                stage = ?id,
                "USD stage root has no defaultPrim; celestial projection skipped"
            );
            continue;
        };
        let Ok(sdf_path) = SdfPath::new(&resolved_path) else {
            continue;
        };
        celestial::insert_celestial_comms_components(
            &reader,
            entity,
            &resolved_path,
            &sdf_path,
            &mut commands,
        );
        commands.entity(entity).try_insert(CelestialProjected);
    }
}

/// Keep one physical connectivity endpoint per authored assembly. A nested
/// `linkNode` is an authoring error (the usual case is a wrapper and its feed
/// aperture both being marked); remove the outer projection before the link
/// kernel sees it. The USD lint reports the source error, while this runtime
/// normalization keeps a malformed custom Twin from creating a self-link.
fn remove_nested_link_nodes(
    mut commands: Commands,
    nodes: Query<(Entity, &lunco_celestial::link::LinkNode)>,
    parents: Query<&ChildOf>,
) {
    for (entity, _) in &nodes {
        let mut cursor = entity;
        while let Ok(child_of) = parents.get(cursor) {
            cursor = child_of.parent();
            if nodes.get(cursor).is_ok() {
                commands
                    .entity(cursor)
                    .try_remove::<lunco_celestial::link::LinkNode>();
                commands
                    .entity(cursor)
                    .try_remove::<lunco_celestial::link::LinkState>();
                break;
            }
        }
    }
}

fn any_nested_link_nodes(
    nodes: Query<Entity, With<lunco_celestial::link::LinkNode>>,
    parents: Query<&ChildOf>,
) -> bool {
    for entity in &nodes {
        let mut cursor = entity;
        while let Ok(child_of) = parents.get(cursor) {
            cursor = child_of.parent();
            if nodes.get(cursor).is_ok() {
                return true;
            }
        }
    }
    false
}

/// Observer that fires when a USD prim entity is added.
///
/// **Intentionally minimal.** All processing is handled by `process_usd_sim_prims` in
/// the `Update` schedule to ensure assets are loaded first. This observer exists only
/// to satisfy the plugin structure — it does nothing.
fn on_add_usd_sim_prim(
    _trigger: On<Add, UsdPrimPath>,
    _query: Query<(Entity, &UsdPrimPath)>,
    _stages: Res<Assets<UsdStageAsset>>,
    mut _commands: Commands,
) {
    // All processing is handled by process_usd_sim_prims in the Update schedule,
    // AFTER sync_usd_visuals creates meshes. This ensures:
    // 1. Assets are fully loaded before processing
    // 2. Meshes exist so we can split wheel entities into physics + visual
    // 3. No duplicate processing or duplicate FSW ports
}

/// Bind the waypoint prims a vessel's behaviour tree references (`<Action ID="drive_to"
/// target="/World/Route/W0"/>`) to their live entities, so
/// `lunco_autopilot::usd_tree::compile_behavior_xml` can bake their world positions
/// into the compiled tree.
///
/// Prim-path → entity resolution is USD's job, which is why it lives HERE and not in
/// `lunco-autopilot` — that crate stays USD-free and merely compiles the bindings it
/// is handed.
///
/// Runs when a tree's XML or the USD identity projection changes. Target paths
/// are derived once per entity/XML change and cached in this resolver; the
/// compiler owns the separate active-frame pose bake. Unresolved paths produce
/// an explicitly empty binding set: the compiler then refuses the tree with a
/// dangling target rather than driving to a guessed origin. A pending route is
/// re-evaluated when the authoritative prim or identity publication changes.
fn resolve_behavior_targets(
    q_trees: Query<(
        Entity,
        Ref<lunco_autopilot::usd_tree::BehaviorXml>,
        Option<&UsdPrimPath>,
        Option<&lunco_autopilot::usd_tree::TargetBindings>,
    )>,
    q_prims: Query<(Entity, &UsdPrimPath)>,
    q_changed_trees: Query<(), Or<(Added<BehaviorXml>, Changed<BehaviorXml>)>>,
    q_changed_prims: Query<(), Or<(Added<UsdPrimPath>, Changed<UsdPrimPath>)>>,
    q_changed_ids: Query<
        (),
        Or<(
            Added<lunco_core::GlobalEntityId>,
            Changed<lunco_core::GlobalEntityId>,
        )>,
    >,
    mut removed_trees: RemovedComponents<BehaviorXml>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    q_instance_projection: Query<&UsdInstanceProjection>,
    mut target_cache: Local<bevy::ecs::entity::EntityHashMap<Vec<String>>>,
    mut commands: Commands,
) {
    for vessel in removed_trees.read() {
        target_cache.remove(&vessel);
    }
    if q_trees.is_empty() {
        return;
    }
    if q_changed_trees.is_empty() && q_changed_prims.is_empty() && q_changed_ids.is_empty() {
        return;
    }
    let mut xml_changed = false;
    for (vessel, xml, _, _) in q_trees.iter() {
        if xml.is_changed() || !target_cache.contains_key(&vessel) {
            xml_changed = true;
            let targets = lunco_autopilot::usd_tree::target_paths(&xml.0);
            target_cache.insert(vessel, targets);
        }
    }
    if !xml_changed && q_changed_prims.is_empty() && q_changed_ids.is_empty() {
        return;
    }
    for (vessel, _xml, vessel_path, current_bindings) in q_trees.iter() {
        let vessel_instance = instance_key(
            vessel,
            &q_provenance,
            &q_gid,
            &q_instance_root,
            &q_instance_projection,
        );
        let mut bindings = lunco_autopilot::usd_tree::TargetBindings::default();
        let mut missing = false;
        let targets = target_cache
            .get(&vessel)
            .expect("target cache entry exists after the change-detection pass");
        debug!(
            "[resolve_behavior_targets] vessel {:?} ({}) has {} targets: {:?}",
            vessel,
            vessel_path
                .map(|p| p.path.as_str())
                .unwrap_or("no-usd-path"),
            targets.len(),
            targets
        );
        for path in targets.iter().cloned() {
            let valid_target = SdfPath::new(&path).is_ok_and(|target| {
                target.is_abs()
                    && !target.is_property_path()
                    && !target.is_prim_variant_selection_path()
            });
            let found = q_prims.iter().find(|(e, p)| {
                let match_path = valid_target && p.path == path;
                let match_stage = vessel_path
                    .map(|vp| p.stage_handle == vp.stage_handle)
                    .unwrap_or(true);
                let inst = instance_key(
                    *e,
                    &q_provenance,
                    &q_gid,
                    &q_instance_root,
                    &q_instance_projection,
                );
                let match_inst = inst.is_none()
                    || vessel_instance.is_none()
                    || inst == vessel_instance;
                if match_path {
                    debug!(
                        "[resolve_behavior_targets] candidate {:?} ({}) match_stage={} match_inst={}",
                        e, p.path, match_stage, match_inst
                    );
                }
                match_path && match_stage && match_inst
            });
            if let Some((e, _)) = found {
                debug!(
                    "[resolve_behavior_targets] resolved target {} -> entity {:?}",
                    path, e
                );
                bindings.0.insert(path, e);
            } else {
                missing = true;
                debug!(
                    "[resolve_behavior_targets] target {} for vessel {:?} is pending USD projection",
                    path, vessel
                );
            }
        }
        if !missing {
            commands.entity(vessel).try_insert(bindings);
        } else if !current_bindings.is_some_and(|bindings| bindings.0.is_empty()) {
            // Empty is an explicit pending state, not a guessed route. This
            // change wakes the autopilot compiler, which refuses the route
            // until the resolver can publish the complete binding set.
            commands
                .entity(vessel)
                .try_insert(lunco_autopilot::usd_tree::TargetBindings::default());
            warn_once!(
                "[resolve_behavior_targets] vessel {:?} has unresolved route targets; waiting for composed prim projection",
                vessel
            );
        } else {
            debug!(
                "[resolve_behavior_targets] vessel {:?} still waiting for composed route targets",
                vessel
            );
        }
    }
}

/// Resolve a [`PendingDifferential`] — an authored gear joint — into a
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

/// Set while a ground provider's static collider is still building (the DEM
/// terrain build — tracked by the assembly crate that sees both worlds, e.g.
/// `lunco-luncosim`). While `true`, [`activate_dynamic_bodies`] holds bodies
/// kinematic so a rover spawned over not-yet-collidable terrain doesn't
/// free-fall through the surface during the multi-second collider bake.
#[derive(Resource, Default)]
pub struct GroundColliderPending(pub bool);

/// Raised for the frame boundary in which authored bodies become dynamic after
/// terrain loading. Terrain observes dynamic bodies in `Update`, while physics
/// can run in the fixed loop between updates; the application assembly clears
/// this only after the terrain gate has evaluated that promoted set.
#[derive(Resource, Default)]
pub struct GroundActivationInFlight(pub u8);

fn activate_dynamic_bodies(
    mut commands: Commands,
    ground_pending: Res<GroundColliderPending>,
    mut activation: ResMut<GroundActivationInFlight>,
    q_kinematic: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&AuthoredInitialVelocity>,
            Option<&avian3d::prelude::RigidBodyDisabled>,
        ),
        With<ShouldBeDynamic>,
    >,
    q_pending_joints: Query<
        (&UsdPrimPath, &lunco_usd_avian::PendingUsdJoint),
        With<lunco_usd_avian::PendingUsdJoint>,
    >,
    q_pending_admissions: Query<&PendingJointAdmission>,
    q_joint_states: Query<(
        &UsdPrimPath,
        Option<&lunco_usd_avian::PendingUsdJoint>,
        Option<&PendingJointAdmission>,
        Has<RevoluteJoint>,
        Has<PrismaticJoint>,
        Has<FixedJoint>,
        Has<SphericalJoint>,
        Has<DistanceJoint>,
    )>,
    q_pending_diffs: Query<&UsdPrimPath, With<PendingDifferential>>,
    topology_index: Res<JointTopologyIndex>,
    mut binding_epoch: ResMut<crate::cosim::BindingEpochDirty>,
    // Physical wheels arm their joint-connected assembly for one-time
    // drop-onto-terrain placement. Free dynamic bodies (balloons, etc.) must
    // not be pinned to the ground. Probe-based models publish their own
    // `PhysicsSupportFootprint` and placement policy from their physics owner.
    q_wheel: Query<(), With<PhysicalWheel>>,
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
    for (entity, path, authored_velocity, body_disabled) in q_kinematic.iter() {
        let has_pending_joint = q_pending_joints.iter().any(|(joint_path, pending)| {
            joint_path.stage_handle == path.stage_handle
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
                        let joint_ready = q_joint_states
                            .iter()
                            .find(|(joint, ..)| {
                                joint.stage_handle == path.stage_handle && joint.path == *joint_path
                            })
                            .is_some_and(
                                |(
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
        let has_pending_diff = q_pending_diffs
            .iter()
            .any(|d_path| d_path.stage_handle == path.stage_handle);
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
            // A physical wheel is part of the chassis' joint-connected assembly,
            // so marking it moves the whole vehicle as one.
            if q_wheel.contains(entity) {
                commands
                    .entity(entity)
                    .try_insert(lunco_core::NeedsGroundSettle);
            }
            promoted = true;
        }
    }
    if promoted {
        // The assembly root owns the ground-activation hold. This projector only
        // publishes the promotion boundary; a second physics-hold writer here
        // made the release order depend on plugin insertion order.
        // Promotion publishes the authored physical initial condition through
        // deferred component insertion. Reopen the co-sim binding epoch so the
        // next sealed pass seeds every already-valid sensor/actuator wire from
        // that finalized state rather than retaining its pre-admission zero.
        binding_epoch.0 = true;
        // Two Update passes: this one spans deferred insertion of Dynamic, the
        // next lets the terrain ring observe that inserted body before its
        // dedicated hold becomes the only gate.
        activation.0 = 2;
    }
}

#[cfg(test)]
mod topology_index_tests {
    use super::*;
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};

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

        index.refresh_if_stale(id, stage.generation(), 1, &stage.view());
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
        assert_eq!(topology.projection_revision, Some(1));

        // Same stamp is a cache hit; a new projection revision (for example an
        // asset reload that replaces a generation-zero canonical stage) must
        // rebuild instead of retaining stale topology.
        index
            .by_stage
            .get_mut(&id)
            .expect("indexed stage")
            .joint_targets
            .clear();
        index.refresh_if_stale(id, stage.generation(), 1, &stage.view());
        assert!(
            index
                .get(id)
                .expect("indexed stage")
                .joint_targets
                .is_empty(),
            "unchanged stamps must not rescan"
        );
        index.refresh_if_stale(id, stage.generation(), 2, &stage.view());
        assert_eq!(
            index
                .get(id)
                .expect("reindexed stage")
                .joint_targets
                .get("/Rover/Wheel"),
            Some(&"/Rover/WheelJoint".to_string()),
            "a projection revision must rebuild a replacement stage"
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
        index.refresh_if_stale(id, stage.generation(), 1, &stage.view());
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
        index.refresh_if_stale(id, stage.generation(), 1, &stage.view());
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
            .init_resource::<GroundActivationInFlight>()
            .init_resource::<JointTopologyIndex>()
            .init_resource::<crate::cosim::BindingEpochDirty>()
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
            .init_resource::<GroundActivationInFlight>()
            .init_resource::<JointTopologyIndex>()
            .init_resource::<crate::cosim::BindingEpochDirty>()
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
                lunco_core::ReplicatedChassisMotion {
                    lin: DVec3::new(0.0, 0.0, -2.0), // 2 m/s along chassis forward (−Z)
                    ang: DVec3::ZERO,
                },
                lunco_core::MobilityRoot,
                lunco_core::OutputPorts::default(),
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
                lunco_core::ReplicatedChassisMotion {
                    lin: DVec3::new(0.0, 0.0, -2.0),
                    ang: DVec3::ZERO,
                },
                lunco_core::MobilityRoot,
                lunco_core::OutputPorts::default(),
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
            lunco_core::NetReplicate,
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
                lunco_core::ReplicatedChassisMotion {
                    lin: DVec3::ZERO,
                    ang,
                },
                lunco_core::MobilityRoot,
                lunco_core::OutputPorts::default(),
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

#[cfg(test)]
mod authored_camera_tests {
    use super::*;
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};

    fn stage_view(source: &str) -> (CanonicalStage, SdfPath) {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("camera.usda", source))
            .expect("camera fixture composes");
        let path = SdfPath::new("/World/Avatar").expect("camera path");
        (stage, path)
    }

    #[test]
    fn omitted_camera_look_at_does_not_override_authored_angles() {
        let (stage, path) = stage_view(
            r#"#usda 1.0
def Xform "World"
{
    def Xform "Avatar" {}
}
"#,
        );
        assert_eq!(read_authored_camera_look_at(&stage.view(), &path), Ok(None));
    }

    #[test]
    fn malformed_authored_camera_look_at_is_rejected() {
        let (stage, path) = stage_view(
            r#"#usda 1.0
def Xform "World"
{
    def Xform "Avatar"
    {
        string lunco:cameraLookAt = "origin"
    }
}
"#,
        );
        assert!(read_authored_camera_look_at(&stage.view(), &path).is_err());
    }
}
