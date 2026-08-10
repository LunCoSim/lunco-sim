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
//! | `PhysxVehicleContextAPI` | `ActuatorPorts` | Rover root entity (kind is topology-derived, no `RoverVessel` marker) |
//! | `PhysxVehicleTankDifferentialAPI` | `DriveMix { kernel: "skid" }` | Skid/tank steering |
//! | `PhysxVehicleAckermannSteeringAPI` | `DriveMix { kernel: "linear" }` + steering port | Ackermann steering |
//! | `DriveMix` child scope | `DriveMix { kernel: "linear" }` | Arbitrary per-wheel linear mix — one prim per sink port, `lunco:factor:<source>` per command source |
//! | `lunco:driveKernel` (hook id) | `DriveMix { kernel: <hook_id> }` | Scripted (rhai) drive kernel — hook computes per-port outputs |
//! | `PhysxVehicleWheelAPI` | `WheelRaycast` *or* `MotorActuator` + `RigidBody` | Wheel — kind decided by joint authoring |
//!
//! ## Wheel kind: discriminated by standard authoring
//!
//! No custom `lunco:` tokens. Each `PhysxVehicleWheelAPI` wheel becomes:
//!
//! - **Joint-based** if any `def PhysicsRevoluteJoint` in the stage targets
//!   it via `rel physics:body1`. Motor torque comes from the composed
//!   `LunCoMotorAPI` + optional `LunCoGearboxAPI` powertrain; the constraint is
//!   built by `lunco-usd-avian`. The wheel becomes a full
//!   rigid body with collider and `MotorActuator`.
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
//! in the `Update` schedule **after** `sync_usd_visuals` to ensure:
//! 1. The USD asset is fully loaded
//! 2. Meshes exist so we can split wheel entities into physics + visual
//! 3. No duplicate processing or duplicate FSW ports

use avian3d::prelude::*;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, FloatingOrigin, Grid};
use lunco_usd_avian::{
    AuthoredInitialVelocity, PendingJointAdmission, SharedTireContact, ShouldBeDynamic,
};
use lunco_usd_bevy::{instance_key, CanonicalStages, UsdRead};
pub use lunco_usd_bevy::{UsdInstanceRoot, UsdPreviewOnly, UsdPrimPath, UsdStageAsset};
// Appearance + camera **intent** — this crate must never name `MeshMaterial3d`,
// `StandardMaterial`, `ShaderMaterial` or `Camera3d` (all `bevy_pbr` /
// `bevy_core_pipeline` → wgpu + naga). `lunco-render-bevy` binds these.
// See docs/architecture/render-decoupling.md.
use leafwing_input_manager::prelude::ActionState;
use lunco_avatar::{
    AdaptiveNearPlane, FreeFlightCamera, OrbitCamera, ProvisionalAvatarCamera, SpringArmCamera,
};
use lunco_controller::get_avatar_input_map;
use lunco_core::architecture::IntentAnalogState;
use lunco_core::architecture::Port;
use lunco_core::{Avatar, LocalAvatar};
use lunco_cosim::{
    avian_queries::RaycastObservation, joint::PassivePrismaticSuspension, ports::PORT_NAME,
    ForceActuator, SimConnection, TorqueActuator,
};
use lunco_hardware::{MotorReadback, MotorReadbackTarget, SteeringActuator};
use lunco_materials::ShaderLook;
use lunco_mobility::kernels::DriveMix;
use lunco_mobility::wheel_kinematics::{wheel_hub_pose, wheel_hub_velocity, wheel_roll_rate};
use lunco_mobility::{
    DifferentialCoupling, JointedWheelTire, Suspension, SuspensionPiston, SuspensionSpring,
    WheelRaycast,
};
use lunco_render::{PbrLook, SceneCamera};
use openusd::sdf::Path as SdfPath;
use std::collections::{HashMap, HashSet};

pub mod wheel_params;
use wheel_params::{SuspensionParams, WheelParams};

/// Plugin for mapping simulation-specific USD schemas (like NVIDIA PhysX Vehicles)
/// to LunCo's optimized simulation models.
///
/// # Processing Order
///
/// 1. `process_usd_sim_prims` — maps schemas to components (runs after sync_usd_visuals)
/// 2. `try_wire_wheel` — connects wheel drive ports to FSW digital ports
///
/// The observer `on_add_usd_sim_prim` intentionally does minimal work. All processing
/// is deferred to the `process_usd_sim_prims` system to ensure assets are loaded first.
///
/// # Wheel kind dispatch (no custom schemas)
///
/// Each wheel prim with `PhysxVehicleWheelAPI` becomes either a raycast wheel
/// (suspension simulation) or a joint-based wheel (full rigid body + revolute
/// joint), discriminated entirely by **standard OpenUSD authoring**:
///
/// - If any `PhysicsRevoluteJoint` in the stage targets the wheel via its
///   `physics:body1` rel → joint-based path. Motor torque and speed come from
///   the composed `LunCoMotorAPI` and optional `LunCoGearboxAPI`; the joint
///   constraint itself is built by `lunco-usd-avian`.
/// - Otherwise → raycast path.
///
/// No custom `lunco:` tokens drive this dispatch.
/// Marker resource present **only** on a headless build with no GPU renderer
/// (the `--no-ui` server): "do not wait for visual components before building
/// wheel physics".
///
/// **Largely redundant since the render decoupling.** The things
/// [`process_usd_sim_prims`] waits on are now `Mesh3d` (`bevy_mesh`) and the
/// appearance *intent* (`PbrLook` / `ShaderLook`), all of which this crate and
/// `lunco-usd-bevy` author with plain systems that run headless. The old deadlock
/// — waiting for a `ShaderMaterial` that only a GPU-side observer could produce —
/// is structurally gone.
///
/// It is kept because it is `pub` and inserted outside this crate
/// (`lunco-luncosim`'s headless boot, `lunco-usd`'s integration tests), and because
/// it remains a correct, cheap "don't wait" switch. Removing it is a separate,
/// cross-crate change.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct NoRenderVisuals;

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
    reader: &lunco_usd_bevy::StageView<'_>,
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
/// properties on the same prim.
pub(crate) fn force_actuator_from_usd(
    reader: &lunco_usd_bevy::StageView<'_>,
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
        .scalar::<[f32; 3]>(actuator_path, FORCE_DIRECTION_ATTR)
        .or_else(|| {
            reader
                .scalar::<[f64; 3]>(actuator_path, FORCE_DIRECTION_ATTR)
                .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32])
        })
        .map(Vec3::from_array)
        .filter(|v| v.is_finite() && v.length_squared() > f32::EPSILON);
    let Some(direction_local) = direction else {
        warn!(
            "[usd-cosim] force actuator {} has no finite non-zero {}",
            actuator_path, FORCE_DIRECTION_ATTR
        );
        return None;
    };
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
    reader: &lunco_usd_bevy::StageView<'_>,
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
        .scalar::<[f32; 3]>(actuator_path, TORQUE_AXIS_ATTR)
        .or_else(|| {
            reader
                .scalar::<[f64; 3]>(actuator_path, TORQUE_AXIS_ATTR)
                .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32])
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
/// The application assembly uses [`UsdSimSet::ActivateDynamicBodies`] to place
/// terrain readiness between terrain inspection and the first dynamic physics
/// tick. Keeping this boundary public prevents another first-load ordering race.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsdSimSet {
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
    /// Supported authored joints and their body endpoints. This is a
    /// composition fact, not an ECS observation: the body entities can be
    /// promoted before the joint observer's deferred command has landed.
    authored_joints: HashMap<String, (String, String)>,
    articulation_roots: HashSet<String>,
    wheel_attachment_targets: HashMap<String, String>,
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
        reader: &lunco_usd_bevy::StageView<'_>,
    ) {
        let topology = self.by_stage.entry(stage).or_default();
        if topology.canonical_generation == Some(generation)
            && topology.projection_revision == Some(projection_revision)
        {
            return;
        }
        topology.joint_targets.clear();
        topology.authored_joints.clear();
        topology.articulation_roots.clear();
        topology.wheel_attachment_targets.clear();
        topology.wheel_attachment_indices.clear();
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
        app.add_systems(
            lunco_usd_bevy::scene_lifecycle::SceneTeardown,
            reset_scene_runtime_safety,
        );

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
        lunco_usd_bevy::scene_lifecycle::run_scene_teardown(app.world_mut());
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
        lunco_usd_bevy::scene_lifecycle::run_scene_teardown(app.world_mut());
        assert!(!app.world().resource::<lunco_core::RuntimeFaults>().active());
    }
}

impl Plugin for UsdSimPlugin {
    fn build(&self, app: &mut App) {
        crate::shader_ports::build(app);
        app.configure_sets(Update, UsdSimSet::ActivateDynamicBodies);
        app.add_systems(
            lunco_usd_bevy::scene_lifecycle::SceneTeardown,
            reset_scene_runtime_safety,
        );
        app.add_systems(
            lunco_usd_bevy::scene_lifecycle::SceneTeardown,
            retire_scene_cameras,
        );
        // Autopilot actors claim scene vessels and hold compiled trees of the scene's
        // route — scene-derived state, so the shared teardown boundary retires them
        // with the rest of the scene (despawn + release the claim, so the respawned
        // vessel can be re-engaged and its waypoints reset cleanly).
        app.add_systems(
            lunco_usd_bevy::scene_lifecycle::SceneTeardown,
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
            .add_observer(on_add_usd_sim_prim)
            // `try_wire_wheel` runs in PreUpdate so that the `SimConnection` entities
            // exist before cosim propagation pushes values through them.
            .add_systems(PreUpdate, (try_wire_wheel, resolve_differential_coupling))
            // USD → ShaderMaterial authoring. Ordered AFTER the visuals exist
            // and BEFORE `process_usd_sim_prims` consumes them, so the material
            // is always present before a wheel is split onto its visual child
            // (Bevy auto-inserts the sync point). Race-free by construction —
            // see `shader.rs`.
            .add_systems(
                Update,
                shader::apply_usd_shader_materials
                    .after(lunco_usd_bevy::sync_usd_visuals)
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
            .add_systems(
                Update,
                (
                    process_usd_sim_prims
                        .run_if(any_unprocessed_usd_sim)
                        .after(lunco_usd_bevy::sync_usd_visuals),
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
                    activate_dynamic_bodies
                        .in_set(UsdSimSet::ActivateDynamicBodies)
                        .run_if(any_with_component::<ShouldBeDynamic>),
                ),
            );
        // Self-healing watchdog: a USD prim that stays unprocessed forever means
        // an unmet dependency is silently deadlocking setup (historically the
        // wheel-shader bug: physics deferred until a render-only `ShaderMaterial`
        // that never arrived headless — structurally impossible now that the waits
        // are on render-free intent, see `NoRenderVisuals`). This turns that class
        // of invisible deadlock into a loud `error!` AND recovers by building the
        // physics without the missing visual.
        app.add_systems(Update, recover_stuck_usd_prims);
        // Screen-constant markers. `PostUpdate` before transform propagation:
        // the scale is a function of the camera's position THIS frame, and the
        // markers sit on other bodies' grids, which `place_celestial_bound_entities`
        // may have just re-parented.
        app.add_systems(
            PostUpdate,
            marker::scale_screen_constant_markers.before(TransformSystems::Propagate),
        );
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
/// USD-authored screen-constant markers (`lunco:marker:*`) — geometry that
/// subtends a fixed angle so a physically sub-pixel thing still reads on screen.
pub mod marker;
pub mod powertrain;
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
/// `MotorActuator` (on its joint) instead of `WheelRaycast` + `RayCaster`.
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
    /// Wheel mount offset in the **chassis** local frame (the authored wheel
    /// translation). The client reconstructs a proxy wheel's world position as
    /// `chassis_pos + chassis_rot · mount_local` instead of replicating it — the
    /// axle is rigid, so this offset is constant. See `reconstruct_proxy_wheels`.
    pub mount_local: Vec3,
    /// Whether this wheel steers (front wheel of an Ackermann rover). The client
    /// derives the steer angle from the chassis yaw-rate/speed for these.
    pub steers: bool,
    /// Front-to-rear axle distance (m), for the Ackermann steer reconstruction.
    pub wheelbase: f64,
}

/// Marker for wheels waiting for their FSW root to be spawned to complete wiring.
#[derive(Component)]
pub struct PendingWheelWiring {
    pub p_drive: Entity,
    pub p_steer: Entity,
    /// USD-authored actuator binding — the port name resolved from the wheel's
    /// required `inputs:drive.connect` connection (the target property minus its
    /// `outputs:` prefix). Drive topology is authored, never inferred from wheel
    /// order or parity.
    pub drive_port_name: String,
    /// USD-authored steer binding, resolved the same way from the optional
    /// `inputs:steer.connect`. Unsteered wheels leave this absent explicitly.
    pub steer_port_name: Option<String>,
}

/// An authored `PhysxPhysicsGearJoint`, held until the bodies it gears together have
/// spawned + been admitted by Avian. `resolve_differential_coupling` matches the
/// prim-path strings → entities (same deferred pattern as `try_wire_wheel` / USD
/// joints) then attaches the [`DifferentialCoupling`].
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
    pub stiffness: f64,
}

/// Process USD prims for sim mapping AFTER their assets are loaded.
///
/// This is the core system that maps USD schemas to LunCoSim components. It runs in the
/// `Update` schedule **after** `sync_usd_visuals` to ensure meshes and transforms exist.
///
/// # What It Does
///
/// 1. **Detects `PhysxVehicleContextAPI`** → Creates `ActuatorPorts` from the
///    vehicle root's authored numeric `outputs:*` attributes, plus `Vessel`.
/// 2. **Detects `PhysxVehicleTankDifferentialAPI`** → `DriveMix { kernel: "skid" }`.
/// 3. **Detects `PhysxVehicleAckermannSteeringAPI`** → `DriveMix { kernel: "linear" }` + steering.
///    (A `lunco:driveKernel` attribute overrides both → `DriveMix { kernel: <hook_id> }`,
///    a scripted rhai kernel — the imperative analog of an Omniverse OmniGraph controller.)
/// 4. **Detects `PhysxVehicleWheelAPI`** → Sets up wheel based on whether an authored
///    `PhysicsRevoluteJoint` targets the wheel:
///    - **Joint-based** (joint authored): `RigidBody`, `Collider`, `MotorActuator` (constraint built by `lunco-usd-avian`; torque/speed come from the composed motor/gearbox)
///    - **Raycast** (no joint): `WheelRaycast`, `RayCaster` (entity split into physics + visual child)
/// Run condition: true when any `UsdPrimPath` entity still lacks
/// `UsdSimProcessed`. Lets `process_usd_sim_prims` stay dormant after
/// scene-load is complete instead of running every frame.
fn any_unprocessed_usd_sim(q: Query<(), (With<UsdPrimPath>, Without<UsdSimProcessed>)>) -> bool {
    !q.is_empty()
}

/// Seconds a USD prim may remain unprocessed before the watchdog treats it as a
/// real deadlock and recovers. Every prim `process_usd_sim_prims` touches is
/// marked `UsdSimProcessed` in the same frame; the *only* prims that linger are
/// ones it deliberately defers waiting on a dependency (a wheel waiting for its
/// `Mesh3d` / `PbrLook` / `ShaderLook`). Async scene loads settle in well under this.
const STUCK_PRIM_DEADLINE_SECS: f32 = 10.0;

/// Stamped by [`recover_stuck_usd_prims`] on a prim that has been deferred too
/// long. [`process_usd_sim_prims`] treats it like the headless `NoRenderVisuals`
/// path for that one prim: stop waiting for the (never-arriving) visual and build
/// the physics anyway. This is the self-heal — a forgotten `NoRenderVisuals`, or a
/// future render-coupled gate, can no longer silently freeze a rover forever.
#[derive(Component)]
struct ForceBuildNoVisual;

/// Self-healing watchdog (structural guard against the wheel-shader class of bug).
/// `process_usd_sim_prims` defers a prim by `continue`-ing without marking it
/// `UsdSimProcessed`; if the awaited dependency never arrives (historically: a
/// render-only material on the headless server) the prim defers FOREVER and nothing
/// complains — the rover silently never gets wheels. Once the unprocessed set has
/// been **stuck (non-decreasing) for [`STUCK_PRIM_DEADLINE_SECS`]**, this:
/// 1. logs a loud `error!` to the console (the built-in `tracing` system), and
/// 2. **recovers** — stamps [`ForceBuildNoVisual`] on each stuck prim so the next
///    `process_usd_sim_prims` builds its physics without the missing visual.
///
/// The app keeps running with drivable rovers instead of a silent deadlock. The
/// query excludes already-recovered prims, and progress (a shrinking set) resets
/// the timer, so a slow async load never trips it.
fn recover_stuck_usd_prims(
    time: Res<Time>,
    q: Query<(Entity, &UsdPrimPath), (Without<UsdSimProcessed>, Without<ForceBuildNoVisual>)>,
    mut commands: Commands,
    mut stuck_for: Local<f32>,
    mut last_count: Local<usize>,
) {
    let count = q.iter().count();
    if count == 0 {
        *stuck_for = 0.0;
        *last_count = 0;
        return;
    }
    if count < *last_count {
        *stuck_for = 0.0; // progress — a normal async load, not a stall
    } else {
        *stuck_for += time.delta_secs();
    }
    *last_count = count;
    if *stuck_for > STUCK_PRIM_DEADLINE_SECS {
        let sample: Vec<String> = q.iter().take(8).map(|(_, p)| p.path.clone()).collect();
        error!(
            "[usd-sim] {count} USD prim(s) stuck unprocessed for >{:.0}s — an unmet \
             dependency (most likely a render-only visual component that a \
             headless/no-GPU build never produces) was deadlocking sim setup. \
             RECOVERING: building physics without the missing visual. Paths: {sample:?}",
            STUCK_PRIM_DEADLINE_SECS,
        );
        for (e, _) in q.iter() {
            commands.entity(e).try_insert(ForceBuildNoVisual);
        }
        // Recovered prims leave the query next frame; reset so any genuinely-new
        // stuck prim starts its own grace period cleanly.
        *stuck_for = 0.0;
        *last_count = 0;
    }
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
            Option<&ChildOf>,
            Option<&ForceBuildNoVisual>,
        ),
        Without<UsdSimProcessed>,
    >,
    all_prims: Query<(Entity, &UsdPrimPath, Option<&Transform>)>,
    q_grids: Query<(Entity, &Grid, Has<lunco_celestial::SiteAlignGrid>)>,
    q_existing_floating_origins: Query<Entity, With<FloatingOrigin>>,
    q_provisional_cameras: Query<Entity, With<ProvisionalAvatarCamera>>,
    q_child_of: Query<&ChildOf>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    stages: Res<Assets<UsdStageAsset>>,
    // Read the LIVE canonical stage (source of truth), built on demand from
    // the asset recipe.
    mut canonical: NonSendMut<CanonicalStages>,
    mut topology_index: ResMut<JointTopologyIndex>,
    stage_revision: Res<lunco_usd_bevy::UsdStageRevision>,
    // The active-scene sun: the avatar camera's exposure is read from the SAME
    // resource the sun illuminance comes from, so they can't drift (a dimmed
    // sun under a bright-tuned camera blacked the viewport). `Option` so the
    // loader still works in a stripped app without `EnvironmentPlugin`.
    active_sun: Option<Res<lunco_environment::LunarSun>>,
    // Inserted by a headless (`--no-ui`) boot. When set, do NOT wait for visual
    // components (`Mesh3d` / `PbrLook` / `ShaderLook`) before building wheel
    // PHYSICS, and skip the visual-only wheel split.
    //
    // Since the render decoupling all three of those ARE authored headless (they
    // are render-free intent, not GPU handles), so this is no longer load-bearing
    // against a deadlock — it is a cheap "don't bother with the visual half"
    // switch. The historical bug it was added for (waiting on a `ShaderMaterial`
    // only a GPU-side observer could mint) is structurally gone. See
    // `NoRenderVisuals` and `docs/architecture/render-decoupling.md`.
    no_render_visuals: Option<Res<NoRenderVisuals>>,
) {
    // Whether visual components will ever arrive. `false` headless ⇒ build the
    // physics now and skip the visual-only split.
    let visuals_coming = no_render_visuals.is_none();
    // Build (or refresh) each involved stage's immutable topology once. The
    // canonical generation is the authored-composition invalidation signal;
    // waiting for a mesh or another sibling no longer re-scans every spec.
    let mut seen_stages = HashSet::new();
    for (_, prim_path, ..) in query.iter() {
        let id = prim_path.stage_handle.id();
        if !seen_stages.insert(id) {
            continue;
        }
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages
                .get(&prim_path.stage_handle)
                .and_then(|asset| asset.recipe.clone())
            {
                canonical.get_or_build(id, &recipe);
            }
        }
        if let Some(stage) = canonical.get(id) {
            topology_index.refresh_if_stale(
                id,
                stage.generation(),
                stage_revision.0,
                &stage.view(),
            );
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
        maybe_child_of,
        force_build,
    ) in query.iter()
    {
        // Per-prim escape hatch: the recovery watchdog stamped this prim after it
        // was deferred too long, so stop waiting for its visual (as if headless).
        let wait_for_visuals = visuals_coming && force_build.is_none();
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

        // Read the live canonical stage, built on demand from the recipe.
        // Acquired per entity — `get_or_build` is cached, so the whole prim
        // cascade shares one composed stage.
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages
                .get(&prim_path.stage_handle)
                .and_then(|a| a.recipe.clone())
            {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else {
            continue;
        };
        let Some(topology) = topology_index.get(id) else {
            continue;
        };
        process_usd_sim_prim_read(
            &cs.view(),
            entity,
            prim_path,
            sdf_path.clone(),
            maybe_tf,
            maybe_mesh,
            maybe_mat,
            maybe_shader_mat,
            maybe_child_of,
            wait_for_visuals,
            topology,
            &all_prims,
            &q_child_of,
            &q_existing_floating_origins,
            &q_provisional_cameras,
            &q_grids,
            active_sun.as_deref(),
            &mut commands,
        );
    }
}

/// Per-stage joint scan (Pass 1), generic over the read source ([`UsdRead`]):
/// collects `PhysicsRevoluteJoint` `body1` targets (wheel dispatch) and the matching
/// `body0` targets (articulation roots) only when `body1` is a declared vehicle wheel.
/// Generic revolute mechanisms must not change a host's vehicle classification.
/// Also collects the canonical
/// `PhysxVehicleWheelAttachmentAPI` wheel→suspension bindings (doc 53 §3.2).
fn collect_joint_scan_read(
    reader: &lunco_usd_bevy::StageView<'_>,
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
            // motor on the wheel entity's actual chassis mount. Keeping the
            // authored identity in the generic readiness set would wait forever,
            // because that synthesized joint intentionally has no USD prim path.
            if !is_physical_wheel_joint {
                topology
                    .authored_joints
                    .insert(path.as_str().to_string(), (body0, body1));
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
        // Canonical wheel-attachment binding: an attachment prim names its wheel
        // (`physxVehicleWheelAttachment:wheel`) and its suspension
        // (`physxVehicleWheelAttachment:suspension`) via USD relationships. The
        // standard schema also permits wheel/tire/suspension APIs directly on the
        // attachment prim; that direct form is explicit and is handled by mapping
        // the attachment to itself. There is no flat legacy fallback.
        if reader.has_api_schema(&path, "PhysxVehicleWheelAttachmentAPI") {
            let wheel = reader
                .rel_target(&path, "physxVehicleWheelAttachment:wheel")
                .or_else(|| {
                    reader
                        .has_api_schema(&path, "PhysxVehicleWheelAPI")
                        .then(|| path.as_str().to_string())
                });
            let suspension = reader
                .rel_target(&path, "physxVehicleWheelAttachment:suspension")
                .or_else(|| {
                    reader
                        .has_api_schema(&path, "PhysxVehicleSuspensionAPI")
                        .then(|| path.as_str().to_string())
                });
            if let (Some(wheel), Some(suspension)) = (wheel, suspension) {
                debug!(
                    "USD wheel attachment: wheel {} → suspension {} (via {})",
                    wheel,
                    suspension,
                    path.as_str()
                );
                topology
                    .wheel_attachment_targets
                    .insert(wheel.clone(), suspension);
                if let Some(index) =
                    reader.scalar::<i32>(&path, "physxVehicleWheelAttachment:index")
                {
                    topology.wheel_attachment_indices.insert(wheel, index);
                }
            }
        }
    }
}

/// Per-prim sim-schema extractor (Pass 2) over the live composed [`UsdRead`]
/// surface — maps one composed prim's authored `lunco:*` / PhysX-vehicle
/// schemas to its sim/avatar/wheel components.
#[allow(clippy::too_many_arguments)]
/// Collect behavior-tree program children below a vehicle, including a
/// namespace such as `OBC`. Program discovery is capability-based and recursive;
/// the namespace's spelling and depth are authoring choices, not runtime rules.
fn collect_behavior_sources(
    reader: &lunco_usd_bevy::StageView<'_>,
    parent: &SdfPath,
    out: &mut Vec<(String, Option<String>, Option<String>)>,
) {
    for child in reader.children(parent) {
        if reader.has_api_schema(&child, "LunCoProgramAPI") {
            if let Some(xml) = reader
                .scalar::<String>(&child, "info:sourceCode")
                .filter(|s| s.trim_start().starts_with('<'))
            {
                out.push((child.as_str().to_string(), Some(xml), None));
            } else if let Some(path) = reader
                .asset(&child, "info:sourceAsset")
                .filter(|s| lunco_core::programs::is_behavior_tree_asset(s))
            {
                out.push((child.as_str().to_string(), None, Some(path)));
            }
        }
        collect_behavior_sources(reader, &child, out);
    }
}

/// Project the explicitly classified passive prismatic suspension API onto its
/// generic co-simulation physics component. A bilateral `PhysicsDriveAPI` is
/// intentionally not inferred here: elevators and actuators are also
/// prismatic joints, and only the applied suspension API means "compression
/// only".
fn passive_prismatic_suspension_from_usd(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
) -> Option<PassivePrismaticSuspension> {
    if !reader.has_api_schema(prim, "LunCoPrismaticSuspensionAPI") {
        return None;
    }
    if reader.type_name(prim).as_deref() != Some("PhysicsPrismaticJoint") {
        warn!(
            "USD prim {} applies LunCoPrismaticSuspensionAPI but is not a PhysicsPrismaticJoint",
            prim.as_str()
        );
        return None;
    }

    let rest_position = reader
        .real_f32(prim, "lunco:prismaticSuspension:restPosition")
        .unwrap_or(0.0) as f64;
    let spring_k = reader.real_f32(prim, "lunco:prismaticSuspension:stiffness")? as f64;
    let damping_c = reader.real_f32(prim, "lunco:prismaticSuspension:damping")? as f64;
    let max_force = reader
        .real_f32(prim, "lunco:prismaticSuspension:maxForce")
        .unwrap_or(f32::INFINITY) as f64;
    if !spring_k.is_finite()
        || spring_k <= 0.0
        || !damping_c.is_finite()
        || damping_c < 0.0
        || !rest_position.is_finite()
        || (!max_force.is_finite() && max_force != f64::INFINITY)
        || max_force <= 0.0
    {
        warn!(
            "USD passive suspension {} has invalid parameters: rest={} m, k={} N/m, c={} N*s/m, max={} N",
            prim.as_str(), rest_position, spring_k, damping_c, max_force
        );
        return None;
    }
    Some(PassivePrismaticSuspension {
        rest_position,
        spring_k,
        damping_c,
        max_force,
    })
}

fn process_usd_sim_prim_read(
    reader: &lunco_usd_bevy::StageView<'_>,
    entity: Entity,
    prim_path: &UsdPrimPath,
    sdf_path: SdfPath,
    maybe_tf: Option<&Transform>,
    maybe_mesh: Option<&Mesh3d>,
    maybe_mat: Option<&PbrLook>,
    maybe_shader_mat: Option<&ShaderLook>,
    maybe_child_of: Option<&ChildOf>,
    wait_for_visuals: bool,
    topology: &StageJointTopology,
    all_prims: &Query<(Entity, &UsdPrimPath, Option<&Transform>)>,
    q_child_of: &Query<&ChildOf>,
    q_existing_floating_origins: &Query<Entity, With<FloatingOrigin>>,
    q_provisional_cameras: &Query<Entity, With<ProvisionalAvatarCamera>>,
    q_grids: &Query<(Entity, &Grid, Has<lunco_celestial::SiteAlignGrid>)>,
    active_sun: Option<&lunco_environment::LunarSun>,
    mut commands: &mut Commands,
) {
    let existing_tf = maybe_tf.cloned().unwrap_or_default();

    // A passive landing suspension is a physical capability claimed by an
    // applied USD API on the joint. It is not inferred from a prim name,
    // damping values, or the fact that the joint happens to be prismatic.
    if let Some(suspension) = passive_prismatic_suspension_from_usd(reader, &sdf_path) {
        commands.entity(entity).try_insert(suspension);
    }

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
    if reader.scalar::<bool>(&sdf_path, "lunco:billboard") == Some(true) {
        let default = billboard::UsdBillboard::default();
        commands.entity(entity).try_insert(billboard::UsdBillboard {
            template: reader
                .text(&sdf_path, "lunco:billboard:text")
                .unwrap_or(default.template),
            offset_y: reader
                .real_f32(&sdf_path, "lunco:billboard:offsetY")
                .unwrap_or(default.offset_y),
            fade_end: reader
                .real_f32(&sdf_path, "lunco:billboard:fadeEnd")
                .unwrap_or(default.fade_end),
        });
    }
    if reader
        .scalar::<bool>(&sdf_path, "lunco:waypoint")
        .unwrap_or(false)
    {
        commands.entity(entity).try_insert(marker::WaypointMarker);
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
    if let Some(angular_deg) = reader.real_f32(&sdf_path, "lunco:marker:angularSizeDeg") {
        let default = marker::ScreenConstantMarker::default();
        commands
            .entity(entity)
            .try_insert(marker::ScreenConstantMarker {
                angular_deg,
                show_beyond_m: reader
                    .real_f32(&sdf_path, "lunco:marker:showBeyondM")
                    .unwrap_or(default.show_beyond_m),
            });
    }

    let net_replicate = reader.scalar::<bool>(&sdf_path, "lunco:net:replicate");
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
                // reparents it to the chassis; raycast wheels leave it in the
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
        let axis = reader
            .text(&sdf_path, "lunco:raycast:axis")
            .and_then(|axis| match axis.as_str() {
                "X" => Some(DVec3::X),
                "-X" => Some(DVec3::NEG_X),
                "Y" => Some(DVec3::Y),
                "-Y" => Some(DVec3::NEG_Y),
                "Z" => Some(DVec3::Z),
                "-Z" => Some(DVec3::NEG_Z),
                _ => None,
            });
        let max_distance = reader.real(&sdf_path, "lunco:raycast:maxDistance");
        if let (Some(axis), Some(max_distance)) = (axis, max_distance) {
            let offset = lunco_usd_bevy::read_vec3_f64(reader, &sdf_path, "lunco:raycast:offset")
                .map(|v| DVec3::new(v[0], v[1], v[2]))
                .unwrap_or_default();
            commands.entity(entity).try_insert(RaycastObservation {
                offset,
                axis,
                max_distance,
                ..default()
            });
        } else {
            warn!(
                "USD raycast {} is missing a valid axis or maxDistance",
                sdf_path
            );
        }
    }

    // (Link/celestial vocabulary is projected by the independent
    // `project_celestial_comms_prims` system, NOT here — see its doc. Bundling it
    // in this system made a cosim prim, which skips this system, lose its LinkNode.)

    // 0. Detect Avatar prim. `lunco:avatar` is a marker flag, but scenes author it
    // with EITHER type — `bool true` (moonbase) or `string "true"` (sandbox). A
    // `scalar::<String>` read silently misses the `bool`, so the avatar's camera is
    // never set up and the viewport is blank after a scene swap. Read it
    // type-tolerantly (same principle as the `text`/`real` reader family).
    let is_avatar = reader
        .scalar::<bool>(&sdf_path, "lunco:avatar")
        .unwrap_or_else(|| reader.text(&sdf_path, "lunco:avatar").as_deref() == Some("true"));
    if is_avatar {
        info!(
            "Detected Avatar prim at {}, setting up camera",
            prim_path.path
        );
        // `big_space` enforces "exactly one `FloatingOrigin` per
        // `BigSpace`". Other crates (e.g. `lunco-celestial`'s
        // Observer Camera) may have already spawned one at startup.
        // The USD Avatar is the user's intended perspective, so it
        // takes over: remove `FloatingOrigin` from every prior
        // holder before we add it to this entity. Without this we
        // get a per-frame `multiple floating origins → resetting
        // this big space` error from big_space and broken
        // transform propagation.
        for prior in q_existing_floating_origins.iter() {
            if prior != entity {
                commands.entity(prior).remove::<FloatingOrigin>();
            }
        }
        // Complete the takeover: retire any PROVISIONAL stand-in camera
        // (spawned by the sandbox while the scene was still loading) in THIS
        // same command flush, so it never coexists with the authored camera
        // as a second order-0 window `Camera3d` — which would otherwise
        // produce camera-order ambiguity (double scene render) and a
        // duplicate `GizmoCamera`. The marker is a separate entity from this
        // avatar prim, so `entity` is never among them; the guard is belt-
        // and-braces. See `ProvisionalAvatarCamera`.
        for prov in q_provisional_cameras.iter() {
            if prov != entity {
                commands.entity(prov).try_despawn();
            }
        }
        // PRIOR AVATARS are not this code's problem. `LocalAvatar` is singular
        // by construction (`lunco_core`'s component hook): inserting it below
        // demotes whatever held it, and `lunco_avatar::demote_former_avatar`
        // strips that entity's camera/control roles and deactivates its camera.
        // This used to be a loop right here, which is precisely why the OTHER
        // ways an avatar appears (the app's fallback camera, a recomposed prim)
        // could leave a second live one.
        // `token`, per luncoSchema — so `text`, not `scalar::<String>`, which
        // matches `Value::String` alone and reads every token as `None`.
        let camera_mode = reader
            .text(&sdf_path, "lunco:cameraMode")
            .unwrap_or_else(|| "freeflight".to_string());
        let mut yaw = reader
            .real_f32(&sdf_path, "lunco:cameraYaw")
            .unwrap_or(std::f32::consts::PI * 0.8);
        let mut pitch = reader
            .real_f32(&sdf_path, "lunco:cameraPitch")
            .unwrap_or(-0.3);

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
        if let Some([lx, ly, lz]) =
            lunco_usd_bevy::read_vec3_f64(reader, &sdf_path, "lunco:cameraLookAt")
        {
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

        // Avatar position from USD transform — placed CELL-GRID AWARE. The
        // authored translate is a WORLD position (e.g. ~1990 m on an elevated
        // site); split it into (CellCoord, local) via the grid instead of
        // dropping it in cell (0,0,0) with a multi-km local and leaning on
        // big_space's `recenter_large_transforms` to migrate it a frame later.
        // Because this camera carries the `FloatingOrigin`, that one-frame
        // cell-0 state put the origin — and the whole world composed off it —
        // at the wrong place on the first rendered frame at elevation.
        let target_grid = q_grids
            .iter()
            .find(|(_, _, is_site)| *is_site)
            .or_else(|| q_grids.iter().next());
        let (avatar_cell, avatar_local) = match target_grid {
            Some((_, grid, _)) => grid.translation_to_grid(existing_tf.translation.as_dvec3()),
            None => (CellCoord::default(), existing_tf.translation),
        };
        let avatar_tf = Transform {
            translation: avatar_local,
            rotation: existing_tf.rotation,
            scale: existing_tf.scale,
        };

        // Shared render-look for the avatar camera: SMAA post-process AA,
        // MSAA off (can't touch shader-internal regolith speckle), and
        // physical lunar exposure (ev100 15 ≈ SUNLIGHT) to pair with the
        // ~128k lx sun. Same look as the sandbox fallback camera; without it
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
        let ev100 = lunco_usd_bevy::read_camera_exposure_ev100(reader, &sdf_path)
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
            )
        };

        // Build camera based on mode, then parent to Grid for FloatingOrigin
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
                    FloatingOrigin,
                    avatar_cell,
                    Avatar,
                    LocalAvatar,
                    IntentAnalogState::default(),
                    ActionState::<lunco_core::UserIntent>::default(),
                    get_avatar_input_map(),
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
                    FloatingOrigin,
                    avatar_cell,
                    Avatar,
                    LocalAvatar,
                    IntentAnalogState::default(),
                    ActionState::<lunco_core::UserIntent>::default(),
                    get_avatar_input_map(),
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
                    FloatingOrigin,
                    avatar_cell,
                    Avatar,
                    LocalAvatar,
                    IntentAnalogState::default(),
                    ActionState::<lunco_core::UserIntent>::default(),
                    get_avatar_input_map(),
                ));
            }
            _ => {
                warn!(
                    "Unknown camera mode '{}' for avatar at {}, using freeflight",
                    camera_mode, prim_path.path
                );
                commands.entity(entity).try_insert((
                    camera_look(),
                    FreeFlightCamera {
                        yaw,
                        pitch,
                        damping: None,
                    },
                    AdaptiveNearPlane,
                    avatar_tf,
                    FloatingOrigin,
                    avatar_cell,
                    Avatar,
                    LocalAvatar,
                    IntentAnalogState::default(),
                    ActionState::<lunco_core::UserIntent>::default(),
                    get_avatar_input_map(),
                ));
            }
        }
        // This applies to both `def Camera` and `def Xform` avatar prims. The
        // avatar controller is the sole pose writer; a hierarchy-derived mount
        // follower must never claim it during an async reload.
        commands
            .entity(entity)
            .try_insert(lunco_usd_bevy::UsdCameraPose::Avatar);
        // Parent to Grid (preferring SiteAlignGrid when present) so FloatingOrigin works
        let target_grid = q_grids
            .iter()
            .find(|(_, _, is_site)| *is_site)
            .or_else(|| q_grids.iter().next());
        if let Some((g, _, _)) = target_grid {
            commands.entity(entity).try_insert(ChildOf(g));
        }
    }

    // 1. Detect PhysxVehicleContextAPI (The Rover Root)
    // Stamps `ActuatorPorts` exclusively from numeric `outputs:` attributes
    // authored on the vehicle root. A vehicle without an authored actuator
    // surface is under-specified; Rust does not fabricate drive/steer/brake
    // ports from the fact that a PhysX vehicle schema is present.
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
        // channels without a Rust-side compatibility vocabulary.
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
            if !port_names.iter().any(|n| n == name) {
                port_names.push(name.to_string());
            }
        }
        if port_names.is_empty() {
            error!(
                "USD vehicle {} applies PhysxVehicleContextAPI but authors no numeric outputs:* actuator ports",
                prim_path.path
            );
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        }
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
            .try_insert(lunco_core::SelectableRoot);

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
        // Only `ActuatorPorts` is stamped here. The `InputPorts` surface is
        // stamped beside the `ControlBinding` (lunco-usd-bevy, the `Controls`
        // branch) — ONE site, because `try_insert` OVERWRITES: stamping a fresh
        // empty surface from two different systems would let a live re-run of
        // either one wipe the keys `sync_input_ports` had already seeded.
        //
        // `ActuatorPorts` is a different thing and is NOT the input surface: it
        // maps ACTUATOR names to their `Port` entities, built above from the
        // vessel prim's authored `outputs:` attributes. The
        // two stay separate components on purpose — both carry a `"brake"`, and
        // they are not the same value (analog command vs discretized gate).
        commands
            .entity(entity)
            .try_insert(lunco_core::ActuatorPorts::new(port_map));
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
    collect_behavior_sources(reader, &sdf_path, &mut behavior_sources);
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
                    lunco_autopilot::usd_tree::BehaviorProgramSource(source_path.clone()),
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

    // 2. Detect the drive allocation → a `DriveMix { kernel, ports, entries }`
    // (`lunco_mobility::kernels`). The kernel is selected by the differential /
    // steering schema the asset declares (Omniverse PhysX Vehicle names) or an
    // authored `DriveMix` child scope. There is NO per-arch Rust
    // component/branch — `apply_drive_mix` looks the named kernel up and runs it.
    let drive_mix = derive_drive_mix(reader, &sdf_path, &prim_path.path);
    if let Some(mix) = drive_mix {
        commands.entity(entity).try_insert(mix);
    }

    // 2b. A GEAR JOINT — `PhysxPhysicsGearJoint`, the PhysX schema for two hinges
    // geared to each other. A rocker-bogie's differential is one of these: gear the
    // left and right rocker hinges at −1 and the chassis rides the AVERAGE of them,
    // which is what keeps the body level over rough ground.
    //
    // Nothing here is rocker-bogie code. A gear joint is a gear joint, and any
    // geared linkage authored this way gets the same coupling with no new Rust.
    // A gear joint is a zero-compliance relation. A positive authored angular
    // stiffness enables the relation; zero disables it for an explicit control
    // scene.
    //
    // Defer-resolved once both geared bodies spawn.
    if reader.type_name(&sdf_path).as_deref() == Some("PhysxPhysicsGearJoint") {
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
            let read_f = |name: &str, dflt: f64| reader.real(&sdf_path, name).unwrap_or(dflt);
            let ratio = read_f("physxGearJoint:gearRatio", -1.0);
            info!(
                "Gear joint {} couples {} / {} (ratio {})",
                prim_path.path, body_a, body_b, ratio,
            );
            commands.entity(entity).try_insert(PendingDifferential {
                chassis: frame,
                rocker_a: body_a,
                rocker_b: body_b,
                ratio,
                rest_offset: read_f("drive:angular:physics:targetPosition", 0.0),
                stiffness: read_f("drive:angular:physics:stiffness", 200_000.0),
            });
        }
    }

    // 3. Detect PhysxVehicleWheelAPI (The Wheel Intercept)
    //
    // By the APPLIED schema, like every other vehicle API here (`…ContextAPI`,
    // `…TankDifferentialAPI`, `…AckermannSteeringAPI`). Applying the API is
    // what makes a prim a wheel; authoring a radius is not. Sniffing for
    // `physxVehicleWheel:radius` conflated "declares itself a wheel" with
    // "happens to carry a wheel-ish attr" — any prim with a stray radius was
    // a wheel, and a wheel could not be authored without one.
    if reader.has_api_schema(&sdf_path, "PhysxVehicleWheelAPI") {
        // Skip if mesh doesn't exist yet — sync_usd_visuals may not have processed
        // this prim. We'll retry next frame (not marking UsdSimProcessed).
        // Headless (no renderer) or recovered (watchdog): the mesh never
        // comes, so don't wait — build the physics wheel without a visual
        // (`setup_raycast_wheel` handles a `None` mesh: it skips the visual child).
        if maybe_mesh.is_none() && wait_for_visuals {
            debug!(
                "Wheel {} has no mesh yet, skipping until next frame",
                prim_path.path
            );
            return;
        }

        // Backstop for the USD-authored shader. `apply_usd_shader_materials`
        // (see shader.rs) is ordered `before` this system, and Bevy's
        // automatic sync-point insertion normally flushes its `ShaderLook`
        // insert before we run — so in the default configuration this guard
        // never fires. It exists to keep the wheel split correct even if that
        // ordering guarantee is ever weakened (e.g. `auto_insert_apply_deferred`
        // disabled): without it we'd split the wheel carrying only
        // the plain `PbrLook` and lose the shader. If a wheel wants
        // a shader but it hasn't landed, retry next frame (don't mark
        // UsdSimProcessed).
        let wants_shader = reader.rel_target(&sdf_path, "material:binding").is_some();
        // Since the decoupling the `ShaderLook` is authored by a plain system
        // that runs headless too (it is intent, not a GPU material), so this no
        // longer deadlocks a `--no-ui` server. The wait is kept because the
        // ordering backstop above still wants it, and `wait_for_visuals`
        // (headless / watchdog-recovered) still short-circuits it.
        if wants_shader && maybe_shader_mat.is_none() && wait_for_visuals {
            debug!("Wheel {} awaits ShaderLook, deferring", prim_path.path);
            return;
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
        let powertrain = match powertrain::find_binding_for_wheel(reader, &sdf_path) {
            Ok(binding) => binding,
            Err(missing) => {
                error!(
                    "USD wheel {} has an invalid or ambiguous motor topology — refusing to spawn: {:?}",
                    sdf_path.as_str(),
                    missing
                );
                commands.entity(entity).try_insert(UsdSimProcessed);
                return;
            }
        };
        let motor_entities: Vec<Entity> = powertrain
            .as_ref()
            .map(|binding| {
                binding
                    .motors
                    .iter()
                    .filter_map(|motor| {
                        all_prims
                            .iter()
                            .find(|(_, candidate, _)| {
                                candidate.stage_handle == prim_path.stage_handle
                                    && candidate.path == motor.as_str()
                            })
                            .map(|(entity, _, _)| entity)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let params = match WheelParams::read(
            reader,
            &sdf_path,
            attachment_susp.as_ref(),
            powertrain.as_ref().map(|binding| &binding.params),
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
        if powertrain.is_some() && motor_entities.len() != powertrain.as_ref().unwrap().motors.len()
        {
            // The motor is an authored USD peer. Retry after its visual projection
            // arrives; silently falling back to the wheel would lose the composed
            // topology and split one drivetrain across two UI branches.
            return;
        }
        for motor in &motor_entities {
            // Native drivetrain state is a port on the authored motor. The
            // wheel/joint only holds this resolved handle, so runtime discovery
            // exposes torque and axle speed without an authored telemetry relay.
            commands.entity(*motor).try_insert(MotorReadback::default());
        }

        // Create the actuator-side ports for drive and steering. Owned by the wheel via
        // `ChildOf` so the single recursive scene-clear reclaims them with the
        // wheel — synthesized backing entities are never left detached at the root
        // (the general lifecycle contract; see `setup_physical_wheel`'s joint).
        let p_drive = commands
            .spawn((Port::default(), Name::new("Port_Drive"), ChildOf(entity)))
            .id();
        let p_steer = commands
            .spawn((Port::default(), Name::new("Port_Steer"), ChildOf(entity)))
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
        let connected_port = |attr: &str| -> Option<String> {
            let source = reader.connection_source(&sdf_path, attr)?;
            let (_, property) = source.rsplit_once('.')?;
            property.strip_prefix("outputs:").map(str::to_string)
        };
        let Some(drive_port_name) = connected_port("inputs:drive") else {
            error!(
                "USD wheel {} has no inputs:drive connection — drive topology must be authored",
                sdf_path.as_str()
            );
            commands.entity(entity).try_insert(UsdSimProcessed);
            return;
        };
        let steer_port_name = connected_port("inputs:steer");

        // Mark for wiring — the try_wire_wheel system will connect ports once FSW exists
        commands.entity(entity).try_insert(PendingWheelWiring {
            p_drive,
            p_steer,
            drive_port_name,
            steer_port_name: steer_port_name.clone(),
        });

        // Standard-USD discriminator: an authored `PhysicsRevoluteJoint`
        // pointing at this wheel via `physics:body1` ⇒ joint-based.
        // Front wheels (index < 2) of an Ackermann rover steer. Gate on the
        // rover's drive type — a skid rover keeps all wheels fixed (it steers
        // by skidding), so only wire the steering port when the wheel's VEHICLE
        // carries `PhysxVehicleAckermannSteeringAPI` (Omniverse steering
        // schema). Same for both wheel kinds: each attaches a shared
        // `SteeringActuator` (joint or raycast), so the model is identical.
        let steering_vehicle = steering_vehicle_of(reader, &prim_path.path);
        let steer_for_wheel = steer_port_name.as_ref().map(|_| p_steer);
        // The steer lock belongs to the VEHICLE's steering system, not to a
        // wheel: PhysX deprecated the per-wheel `physxVehicleWheel:maxSteerAngle`
        // in favour of the steering APIs, and a skid rover's wheels have no such
        // angle at all. RADIANS, as everywhere in PhysX (only the Kit authoring
        // wizard's UI field is in degrees).
        let max_steer_angle = match &steering_vehicle {
            Some(vehicle) => {
                let Some(rad) = reader.real(vehicle, "physxVehicleAckermannSteering:maxSteerAngle")
                else {
                    error!(
                        "USD vehicle {} applies PhysxVehicleAckermannSteeringAPI but \
                             authors no physxVehicleAckermannSteering:maxSteerAngle — \
                             refusing to spawn wheel {}. A steering rover must say how \
                             far it steers.",
                        vehicle.as_str(),
                        sdf_path.as_str()
                    );
                    commands.entity(entity).try_insert(UsdSimProcessed);
                    return;
                };
                rad
            }
            // Not a steering vehicle: no lock, because there is no steering.
            None => 0.0,
        };
        if topology.joint_targets.contains_key(&prim_path.path) {
            setup_physical_wheel(
                &mut commands,
                entity,
                prim_path,
                &existing_tf,
                maybe_mesh,
                maybe_mat,
                maybe_shader_mat,
                maybe_child_of,
                physical_suspension_visuals(
                    reader,
                    prim_path,
                    entity,
                    Transform {
                        translation: existing_tf.translation,
                        rotation: Quat::IDENTITY,
                        scale: existing_tf.scale,
                    },
                    &all_prims,
                    &q_child_of,
                ),
                &params,
                &motor_entities,
                p_drive,
                steer_for_wheel,
                max_steer_angle,
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
                &mut commands,
                entity,
                prim_path,
                &existing_tf,
                maybe_mesh,
                maybe_mat,
                maybe_shader_mat,
                maybe_child_of,
                &params,
                &motor_entities,
                &suspension,
                p_drive,
                p_steer,
                steer_for_wheel,
                max_steer_angle,
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

/// Read the vessel's authored `DriveMix` child scope into `linear` mix terms.
///
/// ONE PRIM PER SINK PORT, named for the actuator port it writes, carrying a
/// `double lunco:factor:<source>` per command source it responds to:
///
/// ```usda
/// def "DriveMix"
/// {
///     def "drive_w0" { double lunco:factor:throttle = 1.0
///                      double lunco:factor:steer    = 1.0 }
/// }
/// ```
///
/// This is SSP's `<Connection><LinearTransformation factor/></Connection>` in
/// USD form: the term prim IS the connection, so the transform is PER
/// CONNECTION rather than per sink. Keying it any other way would mean encoding
/// the source inside an attribute name on the sink attribute — and a connection
/// source is an `SdfPath`, whose `/` and `.` are illegal in USD property names.
/// (Index-aligning a `double[]` against the sink's `.connect` array is the other
/// tempting scheme and is worse: `.connect` is a list-op, so a stronger layer
/// prepending one connection silently shifts every factor.)
///
/// `lunco:factor:<source>` reuses the connection-transform vocabulary the
/// co-simulation port graph already reads (see `cosim.rs`), and the source names
/// are the command ports the vessel's OBC publishes — `throttle`/`steer`/`brake`
/// — not a private set of words.
///
/// An absent factor is `0`, so a term states only the sources it actually
/// responds to. A term prim naming NO known source is a typo, not a coast
/// command, so it is skipped loudly. Terms are sorted by port so the derived
/// component is independent of USD child order (which is hash-ordered).
fn read_drive_mix_scope(
    reader: &lunco_usd_bevy::StageView<'_>,
    scope: &SdfPath,
) -> Option<Vec<lunco_mobility::kernels::MixEntry>> {
    let terms = reader.children(scope);
    if terms.is_empty() {
        error!("DriveMix scope {} has no terms", scope.as_str());
        return None;
    }

    let mut valid = true;
    let mut entries: Vec<lunco_mobility::kernels::MixEntry> = terms
        .into_iter()
        .filter_map(|term| {
            if !reader.has_api_schema(&term, "LunCoDriveMixTermAPI") {
                error!(
                    "DriveMix term {} does not apply LunCoDriveMixTermAPI",
                    term.as_str()
                );
                valid = false;
                return None;
            }
            let port = term.name()?.to_string();
            let forward = reader.real(&term, "lunco:factor:throttle");
            let steer = reader.real(&term, "lunco:factor:steer");
            let brake = reader.real(&term, "lunco:factor:brake");
            if forward.is_none() && steer.is_none() && brake.is_none() {
                error!(
                    "DriveMix term {} declares no `lunco:factor:<throttle|steer|brake>`; \
                     the allocation is invalid",
                    term.as_str()
                );
                valid = false;
                return None;
            }
            Some(lunco_mobility::kernels::MixEntry {
                port,
                forward: forward.unwrap_or(0.0),
                steer: steer.unwrap_or(0.0),
                brake: brake.unwrap_or(0.0),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.port.cmp(&b.port));
    valid.then_some(entries)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DriveOutputOwnership {
    Imperative,
    Authored,
    Partial { connected: Vec<String> },
}

/// Decide ownership against the exact set of ports the selected allocator
/// writes. Output names are never classified by spelling: both lists come from
/// the derived allocator and composed USD connections respectively.
fn drive_output_ownership(expected: &[String], connected: &[String]) -> DriveOutputOwnership {
    let mut connected_expected: Vec<String> = expected
        .iter()
        .filter(|port| connected.iter().any(|candidate| candidate == *port))
        .cloned()
        .collect();
    connected_expected.sort();
    connected_expected.dedup();

    if connected_expected.is_empty() {
        DriveOutputOwnership::Imperative
    } else if connected_expected.len() == expected.len() {
        DriveOutputOwnership::Authored
    } else {
        DriveOutputOwnership::Partial {
            connected: connected_expected,
        }
    }
}

/// Derive the vehicle-root `DriveMix` from its authored schema — the kernel is
/// selected by the differential / steering schema the asset declares (Omniverse
/// PhysX Vehicle names), an authored `DriveMix` child scope, or a scripted
/// `lunco:driveKernel` hook. A connected drive output is already owned by an
/// authored producer (for example a Modelica drive law), so no imperative mix is
/// derived for that vehicle. Shared by the spawn path and the live wheel-param
/// resync so an edited allocation re-derives identically.
fn derive_drive_mix(
    reader: &lunco_usd_bevy::StageView<'_>,
    sdf_path: &SdfPath,
    prim_path_str: &str,
) -> Option<DriveMix> {
    let attrs = reader.attr_names(sdf_path);
    let has_kernel_attr = attrs.iter().any(|attr| attr == "lunco:driveKernel");
    let has_kernel_api = reader.has_api_schema(sdf_path, "LunCoDriveKernelAPI");
    if has_kernel_attr && !has_kernel_api {
        error!(
            "{} authors lunco:driveKernel without LunCoDriveKernelAPI; allocation disabled",
            prim_path_str
        );
        return None;
    }

    let scripted_hook = has_kernel_api
        .then(|| reader.text(sdf_path, "lunco:driveKernel"))
        .flatten()
        .filter(|id| !id.is_empty());
    let mix = if let Some(hook_id) = scripted_hook {
        // Scripted (rhai) kernel: the hook computes the per-port outputs, so it
        // takes precedence over the built-in skid/linear schemas. `apply_drive_mix`
        // falls back to the `lunco_hooks` hook named by `DriveMix.kernel`.
        info!("Scripted drive kernel '{}' for {}", hook_id, prim_path_str);
        DriveMix::scripted(&hook_id)
    } else if let Some(scope) = reader
        .children(sdf_path)
        .into_iter()
        .find(|c| c.name() == Some("DriveMix"))
    {
        let entries = read_drive_mix_scope(reader, &scope)?;
        info!(
            "Authored linear DriveMix for {} ({} ports)",
            prim_path_str,
            entries.len()
        );
        DriveMix::linear(entries)
    } else if reader.has_api_schema(sdf_path, "PhysxVehicleTankDifferentialAPI") {
        info!("Tank differential (skid kernel) for {}", prim_path_str);
        DriveMix::skid("drive_left", "drive_right")
    } else if reader.has_api_schema(sdf_path, "PhysxVehicleAckermannSteeringAPI") {
        // Ackermann: non-differential drive (both sides get throttle) + a
        // dedicated steering port; the front wheels castor (see steering gate).
        info!("Ackermann steering (linear kernel) for {}", prim_path_str);
        DriveMix::linear(vec![
            lunco_mobility::kernels::MixEntry {
                port: "drive_left".to_string(),
                forward: 1.0,
                steer: 0.0,
                brake: 0.0,
            },
            lunco_mobility::kernels::MixEntry {
                port: "drive_right".to_string(),
                forward: 1.0,
                steer: 0.0,
                brake: 0.0,
            },
            lunco_mobility::kernels::MixEntry {
                port: "steering".to_string(),
                forward: 0.0,
                steer: 1.0,
                brake: 0.0,
            },
        ])
    } else {
        return None;
    };

    // A scripted hook owns an open port set by contract. Explicitly selecting it
    // is sufficient; exact port derivation is intentionally unavailable.
    let expected: Vec<String> = if !mix.ports.is_empty() {
        mix.ports.clone()
    } else {
        mix.entries.iter().map(|entry| entry.port.clone()).collect()
    };
    if expected.is_empty() {
        return Some(mix);
    }

    let connected: Vec<String> = expected
        .iter()
        .filter(|port| {
            !reader
                .connections(sdf_path, &format!("outputs:{port}"))
                .is_empty()
        })
        .cloned()
        .collect();
    match drive_output_ownership(&expected, &connected) {
        DriveOutputOwnership::Imperative => Some(mix),
        DriveOutputOwnership::Authored => {
            info!(
                "Authored connections own all drive outputs {:?} for {}",
                expected, prim_path_str
            );
            None
        }
        DriveOutputOwnership::Partial { connected } => {
            error!(
                "{} connects only drive outputs {:?} of expected {:?}; partial allocation ownership is invalid and disabled",
                prim_path_str, connected, expected
            );
            None
        }
    }
}

#[cfg(test)]
mod drive_output_ownership_tests {
    use super::{drive_output_ownership, DriveOutputOwnership};

    #[test]
    fn ownership_is_derived_from_the_allocator_exact_port_set() {
        let expected = vec!["left".to_string(), "right".to_string()];
        assert_eq!(
            drive_output_ownership(&expected, &[]),
            DriveOutputOwnership::Imperative
        );
        assert_eq!(
            drive_output_ownership(&expected, &expected),
            DriveOutputOwnership::Authored
        );
        assert_eq!(
            drive_output_ownership(&expected, &["left".to_string(), "soc".to_string()]),
            DriveOutputOwnership::Partial {
                connected: vec!["left".to_string()]
            }
        );
    }
}

/// The vehicle whose steering system this wheel belongs to: the nearest ancestor
/// prim carrying `PhysxVehicleAckermannSteeringAPI`.
///
/// `None` ⇒ the wheel does not steer. That is the normal case, not a failure: a
/// skid rover steers by driving its sides at different speeds, and its wheels are
/// fixed. Steering geometry is a property of the VEHICLE — NVIDIA puts the lock
/// angle on this API, applied to the vehicle prim — so the wheel asks upward for
/// it instead of carrying a copy.
///
/// Walks ANCESTORS, not the immediate parent: a wheel need not be a direct child of
/// its vehicle. A rocker-bogie wheel hangs off a rocker link (`/Rover/RockerL/Wheel_FL`),
/// so a parent-only check silently reports "does not steer" for a rover that does.
fn steering_vehicle_of(
    reader: &lunco_usd_bevy::StageView<'_>,
    wheel_path: &str,
) -> Option<SdfPath> {
    let mut path = wheel_path;
    while let Some(cut) = path.rfind('/') {
        // `cut == 0` ⇒ the next ancestor is the pseudo-root; stop.
        if cut == 0 {
            break;
        }
        path = &path[..cut];
        if let Ok(prim) = SdfPath::new(path) {
            if reader.has_api_schema(&prim, "PhysxVehicleAckermannSteeringAPI") {
                return Some(prim);
            }
        }
    }
    None
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
    maybe_child_of: Option<&ChildOf>,
    params: &WheelParams,
    motor_entities: &[Entity],
    susp: &SuspensionParams,
    p_drive: Entity,
    p_steer: Entity,
    steer: Option<Entity>,
    max_steer_angle: f64,
) {
    info!("Setting up RAYCAST wheel {}", prim_path.path);

    let mut wheel = params.to_wheel_raycast(p_drive, p_steer, Some(entity));

    // --- Wheel Entity Splitting (always) ---
    // The physics entity needs identity rotation so `RayCaster::NEG_Y`
    // casts straight down. The visual mesh is moved to a child entity
    // so `apply_wheel_suspension` can reposition it to ground-level
    // each frame — its `q_visual` query filters out `WheelRaycast`,
    // so it can only operate on a separate visual entity.
    let wheel_mesh = maybe_mesh.map(|m| m.clone());
    let wheel_rotation = existing_tf.rotation;

    if wheel_mesh.is_some() {
        // Atomic spawn: `ChildOf(entity)` in the bundle so parent + transform
        // land together — same contract as `migrate_to_grid`.
        let mut visual = commands.spawn((
            Name::new(format!(
                "{}_visual",
                prim_path.path.split('/').next_back().unwrap_or("wheel")
            )),
            Transform {
                translation: Vec3::ZERO,
                rotation: wheel_rotation,
                scale: existing_tf.scale,
            },
            Visibility::Inherited,
            InheritedVisibility::default(),
            ViewVisibility::default(),
            wheel_mesh.unwrap(),
            ChildOf(entity),
        ));
        // Move whichever appearance INTENT the prim received onto the visual child;
        // `lunco-render-bevy` rebinds the material there. A USD
        // `materialType="shader"` prim gets a `ShaderLook` (authored by
        // `apply_usd_shader_materials`, ordered before this split) — prefer it over
        // the plain `PbrLook` so USD-authored shaders survive the wheel split. The
        // two are mutually exclusive on one entity (an entity carrying both would
        // draw twice), so `remove` BOTH from the physics entity.
        if let Some(sm) = maybe_shader_mat.cloned() {
            visual.try_insert(sm);
        } else if let Some(mat) = maybe_mat.cloned() {
            visual.try_insert(mat);
        }
        wheel.visual_entity = Some(visual.id());
        commands.entity(entity).remove::<Mesh3d>();
        commands.entity(entity).remove::<PbrLook>();
        commands.entity(entity).remove::<ShaderLook>();
    }

    // Physics entity: identity rotation, position preserved
    let wheel_tf = Transform {
        translation: existing_tf.translation,
        rotation: Quat::IDENTITY,
        scale: existing_tf.scale,
    };

    // Build RayCaster with exclusion filter to prevent wheels from raycasting
    // against their own rover chassis (causes jiggling/jumping bug).
    let rover_entity = maybe_child_of.map(|c| c.parent());
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
    .with_max_distance(susp.rest_length)
    .with_max_hits(1);
    // Mask out the non-physical layers so suspension rays ignore trigger-zone
    // sensors (else the wheels ride up on an invisible waypoint sphere) and
    // celestial body spheres (a planet-sized collider that CONTAINS the scene
    // returns distance 0 — see `NON_PHYSICAL_QUERY_LAYERS`). Excludes the
    // rover's own chassis by entity as before.
    let mut filter = avian3d::prelude::SpatialQueryFilter::from_mask(avian3d::prelude::LayerMask(
        !lunco_core::NON_PHYSICAL_QUERY_LAYERS,
    ));
    if let Some(rover_ent) = rover_entity {
        filter.excluded_entities.insert(rover_ent);
    }
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
    if !motor_entities.is_empty() {
        commands
            .entity(entity)
            .try_insert(MotorReadbackTarget(motor_entities.to_vec()));
    }

    // Front Ackermann wheel: attach the SHARED steering servo. The same
    // `SteeringActuator` + system the physical joint uses computes this wheel's
    // rate-limited Ackermann angle into `output_angle`; `apply_wheel_steering`
    // rotates the raycast wheel to it — identical steering across wheel kinds.
    if let Some(steer_port) = steer {
        let mount = existing_tf.translation.as_dvec3();
        commands.entity(entity).try_insert(SteeringActuator {
            port_entity: steer_port,
            max_steer_angle,
            current_ref: 0.0,
            lateral: mount.x,
            wheelbase: 2.0 * mount.z.abs(),
            output_angle: 0.0,
        });
    }

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
    reader: &lunco_usd_bevy::StageView<'_>,
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

/// Sets up a wheel as a full rigid body bound to the chassis by a revolute
/// joint, mirroring the standard `PhysicsRevoluteJoint` authored in USD.
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
    maybe_child_of: Option<&ChildOf>,
    suspension_visuals: Vec<(Entity, Transform)>,
    params: &WheelParams,
    motor_entities: &[Entity],
    p_drive: Entity,
    steer: Option<Entity>,
    max_steer_angle: f64,
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
    let mut visual_id: Option<Entity> = None;
    if let Some(mesh) = maybe_mesh.cloned() {
        let mut visual = commands.spawn((
            Name::new(format!(
                "{}_visual",
                prim_path.path.split('/').next_back().unwrap_or("wheel")
            )),
            Transform::from_rotation(visual_axis_rot),
            Visibility::Inherited,
            InheritedVisibility::default(),
            ViewVisibility::default(),
            mesh,
            ChildOf(entity),
        ));
        visual_id = Some(visual.id());
        // Move whichever appearance INTENT the prim received onto the visual child
        // (see `setup_raycast_wheel` for the full rationale): the `ShaderLook` wins
        // over the plain `PbrLook`, and both are removed from the physics entity.
        if let Some(sm) = maybe_shader_mat.cloned() {
            visual.try_insert(sm);
        } else if let Some(mat) = maybe_mat.cloned() {
            visual.try_insert(mat);
        }
        commands.entity(entity).remove::<Mesh3d>();
        commands.entity(entity).remove::<PbrLook>();
        commands.entity(entity).remove::<ShaderLook>();
    }

    commands
        .entity(entity)
        .remove::<WheelRaycast>()
        .remove::<RayCaster>()
        .remove::<RayHits>();

    // Wheel mass via DENSITY, not a forced `Mass` — see
    // `WheelParams::wheel_density` for why a forced mass desyncs mass from
    // angular inertia and sinks the rover through the terrain.
    let wheel_density = params.wheel_density();

    commands.entity(entity).insert((
        PhysicalWheel {
            visual_entity: visual_id,
            wheel_radius: radius,
            wheel_width: params.width as f32,
            axis_rot: wheel_axis_rot,
            spin_angle: 0.0,
            // Authored wheel offset in the chassis frame (the wheel is a child of the
            // chassis, so its local translation IS the mount). `steers`/`wheelbase`
            // mirror the `SteeringActuator` geometry below — used by the client's
            // `reconstruct_proxy_wheels` to place + steer the wheel without replicating it.
            mount_local: existing_tf.translation,
            steers: steer.is_some(),
            wheelbase: 2.0 * existing_tf.translation.as_dvec3().z.abs(),
        },
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
        // `d = dampingRate / I_axle`; the live authored inertia includes any
        // reflected rotor term. One authored number, two realizations, each in
        // its own units.
        //
        // `LinearDamping(0.1)` is GONE with no replacement. A wheel hinged to the
        // chassis travels at the chassis's speed, so a linear damper on it was a
        // second, unauthored aerodynamic-style drag on the vehicle (≈22 N at
        // cruise) that the raycast rover — whose wheels are not bodies — could not
        // have. Rolling drag is the bearing term above; there is no air on the Moon.
        AngularDamping(params.bearing_damping / params.axle_inertia().max(1e-6)),
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

    // The authored wheel MOI and the motor's reflected rotor inertia are the
    // rotational contract for BOTH realizations.  Collider density derives the
    // physical mass, but it cannot express either an authored non-solid-cylinder
    // MOI or a spin-only reflected rotor term.  Stamp the complete tensor even
    // when the reflected term is zero; otherwise undriven wheels silently use a
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
    let Some(child_of) = maybe_child_of else {
        warn!(
            "Physical wheel {} has no chassis parent; skipping revolute joint",
            prim_path.path
        );
        return;
    };
    let chassis = child_of.parent();
    // The wheel body rotates about its axle. Keep the authored suspension strut
    // on the chassis carrier so its casing, piston, and spring remain visually
    // connected to the mount instead of spinning with the tire. The transforms
    // were converted from wheel-local to chassis-local by the caller.
    for (visual, transform) in suspension_visuals {
        commands
            .entity(visual)
            .insert((transform, ChildOf(chassis)));
    }
    // NOTE: `ArticulatedVehicle` (the articulated-root guard) is no longer stamped
    // here. It is derived declaratively from the USD joint graph in
    // `process_usd_sim_prims` (a prim that is a joint `physics:body0` target, or
    // carries `PhysicsArticulationRootAPI`) — see USD_REPLICATION_POLICY.md. That
    // removes this build-order side-effect (the membership pass used to depend on it).
    // Wheel mount point in the chassis local frame (the wheel is a child of
    // the chassis, so its Transform translation is already chassis-local).
    let mount_local = existing_tf.translation.as_dvec3();
    // Axle direction — the same line the drive torque acts about. Chassis-local
    // (the wheel/hub frames are aligned to the chassis), so it is also the
    // hub→wheel revolute axis.
    let axle = axle_local;
    // Hinge the wheel to the chassis at its authored offset.
    //
    // An articulated chassis→prismatic(spring)→hub→revolute→wheel *suspension*
    // was prototyped and rejected: avian's joint SpringDamper is fragile bearing
    // the chassis weight — it rings the pitch/roll mode down for 15-20 s after
    // the scene's 5 m spawn drop, can't be damped harder (high damping_ratio
    // diverges), and its effective tuning shifts with substep count. The fix for
    // *vertical* travel is therefore the rigid axle below + `SubstepCount(16)` at
    // the app; joint rovers are rigid-axle. See `project_physical_rover_suspension`.
    //
    // Steering is a yaw of the front wheel about the vertical. A physical
    // steering KNUCKLE (an intermediate body on a second revolute) was tried and
    // rejected: a knuckle heavy enough to hold the wheel makes the
    // chassis→knuckle→wheel chain ill-conditioned and avian 0.6.1's solver
    // INJECTS energy (the idle rover spins and drifts metres with zero throttle);
    // a knuckle light enough to be stable can't hold the steer and the response
    // is pure noise. Verified across mass, inertia, motor stiffness and drive
    // mode with the headless `rover_turn` probe.
    //
    // Instead every wheel hangs off the chassis by a SINGLE revolute (stable,
    // like the rigid rear axle). The drive is a velocity-controlled motor on that
    // joint (see MotorActuator). Front wheels are STEERED by rotating the joint's
    // chassis-side frame about Y (`SteeringActuator`): the alignment constraint
    // yaws the wheel into the steered heading, so it physically turns and its grip
    // carries the rover into an arc — geometric Ackermann through one constraint.
    //
    // (A spring suspension was also rejected — avian's joint SpringDamper is
    // fragile bearing the chassis weight; the fix for vertical travel is the rigid
    // axle + `SubstepCount(16)`. See `project_physical_rover_suspension`.)

    // The joint starts with the authored motor model disabled. MotorActuator is
    // the sole owner of its velocity-motor drive and evaluates the same
    // authored axle torque-speed curve as the raycast path. The shared mobility
    // tire system owns wheel-ground tangent force; Avian supplies normal
    // contact and joint constraints.
    let drive_motor = params.drive_motor();

    // Joint construction lives in `lunco-usd-avian` (the single home for all
    // Avian joint-building); we add the mobility/hardware actuators on top.
    let mut joint_cmd = commands.spawn((
        // GENERAL LIFECYCLE CONTRACT — every entity the USD build *synthesizes* to back a
        // scene (avian joints, actuator ports, cosim wires) is parented into the grid
        // subtree via `ChildOf`, so the ONE hierarchy-recursive `clear_scene_entities`
        // reclaims it exactly once, in the same flush as its bodies. Authored joints (any
        // depth of `Physics*Joint` prim in a robot arm / lander / crane) already satisfy
        // this — they ARE prim entities under the scene. This is the *synthesized* joint,
        // the only one not authored, so it is the one that must opt in explicitly here.
        //
        // A wheel joint links two bodies, so it sits in nobody's TRANSFORM subtree — but
        // it must die WITH the rover. `ChildOf` puts it in the chassis's despawn subtree;
        // avian resolves the constraint from the joint's body anchors, never from this
        // entity's transform, so the parenting is physics-inert. Left detached, the joint
        // outlived its bodies on a scene swap and was double-removed from avian's island
        // bookkeeping — a `joint_count` underflow that corrupted the solver. Owning it here
        // makes that structurally impossible: no orphans, no reaper, no mask.
        ChildOf(chassis),
        lunco_usd_avian::ScenePhysicsOwned,
        // The shared torque-speed parameters still come from the composed
        // motor/gearbox. Avian applies the resulting axle drive through this
        // revolute motor; no wheel-local torque or speed values are read.
        lunco_hardware::MotorActuator {
            port_entity: p_drive,
            max_omega: params.max_rotation_speed,
            peak_torque: params.peak_torque,
            brake_torque: params.brake_torque_max,
            // The wheel hinge is authored about +X. Negative +X rotation is the
            // demand-positive rolling sense for a chassis-forward -Z wheel;
            // this is the convention used by both the Avian motor and shared
            // tire solve.
            drive_sign: -1.0,
        },
        Name::new(format!("PhysicalWheelJoint_{}", prim_path.path)),
    ));
    if !motor_entities.is_empty() {
        joint_cmd.try_insert(MotorReadbackTarget(motor_entities.to_vec()));
    }
    let joint_entity = joint_cmd.id();
    // Front wheels of an Ackermann rover also steer (frame rotation about Y).
    if let Some(steer_port) = steer {
        joint_cmd.try_insert(SteeringActuator {
            port_entity: steer_port,
            max_steer_angle,
            current_ref: 0.0,
            // Chassis-local geometry for the Ackermann correction. `mount_local`
            // is the wheel's offset from the chassis origin: X = lateral (+left),
            // Z = longitudinal. Wheelbase = front-to-rear axle distance = 2·|z|
            // for the symmetric layout.
            lateral: mount_local.x,
            wheelbase: 2.0 * mount_local.z.abs(),
            output_angle: 0.0,
        });
    }

    // Project the complete authored tire contract onto the physical wheel.
    // `lunco-mobility` consumes this in the same fixed-step force system as the
    // raycast wheel; there is no physical-only lateral coefficient.
    commands.entity(entity).insert(JointedWheelTire {
        drive_joint: joint_entity,
        radius: params.radius,
        axle_inertia: params.axle_inertia(),
        slip_stiffness: params.slip_stiffness,
        cornering_stiffness: params.cornering_stiffness,
        min_validated_speed: params.min_validated_speed,
        friction_mu: params.friction_mu,
        bearing_damping: params.bearing_damping,
        axle_axis_local: params.axle_axis,
        heading_local: existing_tf.rotation.as_dquat() * DVec3::NEG_Z,
    });

    // The constraint itself goes through the ONE door every joint in the
    // workspace uses. `attach_joint` takes the two BODIES, so it — not this call
    // site — decides WHEN the joint may enter avian's graph (both bodies admitted
    // to the island graph) and WHAT rides its bundle (`JointCollisionDisabled`).
    // Inserting a joint component here directly is what "Neither body … is in an
    // island" was: the wheel and its chassis are spawned by this very pass, so on
    // a scene swap they are routinely not yet admitted at this exact moment.
    lunco_usd_avian::attach_joint(
        commands,
        joint_entity,
        chassis,
        entity,
        lunco_usd_avian::wheel_revolute_joint(chassis, entity, mount_local, axle, drive_motor),
    );

    // The wheel↔chassis link is the wheel's `ChildOf(chassis)` — set by USD projection
    // and read back here as `chassis = child_of.parent()`. It is the ONE canonical link:
    // transform propagation, despawn cascade, AND parent lookup (the proxy systems below
    // read `ChildOf` to find the chassis). No separate ownership relationship is needed.
}

/// Build the physical wheel's authored inertia tensor in the entity's local
/// frame.  `WheelParams::axle_inertia` is the corresponding scalar used by the
/// raycast integrator; keeping this conversion here makes the two realizations
/// consume the same authored MOI and reflected motor inertia.
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
/// The axle is rigid, so a wheel's offset from the chassis is constant (`mount_local`)
/// and its only motion is cosmetic axle-spin (handled visually by
/// `animate_proxy_physical_wheels`) + front-wheel steer (derived here from the chassis
/// yaw-rate/speed). So a remote rover replicates **only its chassis**; each wheel is a
/// kinematic follower whose world pose = `chassis ∘ steer` at `mount_local`. This puts
/// the wheel collider in the right place for contact (the original "free wheel collider"
/// bug) at ~zero wire cost — no per-wheel snapshot.
///
/// Runs only on a **client**, only for wheels whose chassis is a **kinematic proxy**
/// (a remote rover); the host and the rover this client owns run real local wheel
/// physics (Dynamic + joint + motor). A kinematic child body's world pose is not
/// auto-derived from its parent, so it must be driven every tick or it freezes in world
/// space as the chassis moves away.
/// Ackermann steer angle (radians, about the chassis +Y axis) for a rigid-axle
/// proxy wheel, derived from the replicated chassis motion: `tan δ = wheelbase ·
/// yaw_rate / speed`. Rear wheels (`steers == false`) and a near-stationary
/// chassis (ground speed ≤ 0.25 m/s, where the ratio is numerically meaningless)
/// return 0. Cosmetic-grade; clamped to ±0.6 rad so a spike in the hint can't
/// snap the wheel sideways.
///
/// Pure extract of the steer math in [`reconstruct_proxy_wheels`]; `lin`/`ang`
/// are the chassis linear/angular velocity in world space and only the planar
/// (x,z) speed and yaw rate (`ang.y`) are used.
fn proxy_wheel_steer(steers: bool, wheelbase: f64, lin: DVec3, ang: DVec3) -> f64 {
    if !steers {
        return 0.0;
    }
    let speed = (lin.x * lin.x + lin.z * lin.z).sqrt();
    if speed > 0.25 {
        (wheelbase * ang.y / speed).atan().clamp(-0.6, 0.6)
    } else {
        0.0
    }
}

/// World pose of a rigid-axle proxy wheel: the chassis pose composed with the
/// authored mount offset and the (front-wheel) steer rotation. The axle is rigid,
/// so the wheel rides at a constant `mount_local` offset in the chassis frame and
/// only front wheels add a yaw about +Y. Returns `(position, rotation)`; the
/// rotation is normalized.
///
/// Pure extract of the pose math in [`reconstruct_proxy_wheels`].
fn proxy_wheel_pose(
    chassis_pos: DVec3,
    chassis_rot: DQuat,
    mount_local: DVec3,
    steer: f64,
) -> (DVec3, DQuat) {
    let pos = chassis_pos + chassis_rot * mount_local;
    let rot = (chassis_rot * DQuat::from_rotation_y(steer)).normalize();
    (pos, rot)
}

fn reconstruct_proxy_wheels(
    // Optional: with no network context (standalone / a minimal test harness that
    // ticks the fixed schedule without the full core plugin) there are no
    // replicated proxies to reconstruct, so no-op instead of panicking on a missing
    // resource. Only `NetworkRole::Client` does work here anyway.
    role: Option<Res<lunco_core::NetworkRole>>,
    q_chassis: Query<
        (
            &RigidBody,
            &Position,
            &Rotation,
            Option<&lunco_core::ReplicatedChassisMotion>,
        ),
        (With<lunco_core::ActuatorPorts>, Without<PhysicalWheel>),
    >,
    mut q_wheels: Query<
        (
            Entity,
            &PhysicalWheel,
            &ChildOf,
            &RigidBody,
            &mut Position,
            &mut Rotation,
        ),
        Without<lunco_core::OwnedLocally>,
    >,
    mut commands: Commands,
) {
    let Some(role) = role else { return };
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    for (e, wheel, child_of, rb, mut pos, mut rot) in q_wheels.iter_mut() {
        // The wheel's `ChildOf` parent IS its chassis (set by USD projection).
        let Ok((c_rb, c_pos, c_rot, motion)) = q_chassis.get(child_of.parent()) else {
            continue;
        };
        if !matches!(c_rb, RigidBody::Kinematic) {
            continue; // host / owned rover — real local wheel physics
        }
        if !matches!(rb, RigidBody::Kinematic) {
            commands.entity(e).try_insert(RigidBody::Kinematic);
        }
        // Front wheels: Ackermann steer from the chassis motion. Cosmetic-grade;
        // rear wheels δ = 0.
        let (lin, ang) = motion
            .map(|m| (m.lin, m.ang))
            .unwrap_or((DVec3::ZERO, DVec3::ZERO));
        let steer = proxy_wheel_steer(wheel.steers, wheel.wheelbase, lin, ang);
        // World pose = chassis ∘ steer, at the rigid mount offset. The cylinder
        // collider (axis baked into its compound) lands correctly for contact; the
        // visual child's spin is layered on by `animate_proxy_physical_wheels`.
        let (p, q) = proxy_wheel_pose(c_pos.0, c_rot.0, wheel.mount_local.as_dvec3(), steer);
        pos.0 = p;
        rot.0 = q;
    }
}

/// Client-only **fallback**: spin a joint-wheel's **visual** on a replicated proxy
/// when the wheel body itself is NOT per-link replicated.
///
/// Superseded for replicated wheels: with full articulated per-link replication
/// (wheels carry `NetReplicate`, applied by `apply_net_replication`) the wheel **body** carries
/// the host's true world rotation and the visual child (`ChildOf(wheel)`) inherits
/// it — so this system would *double-apply* spin. It therefore skips
/// `With<NetReplicate>` wheels (`Without<NetReplicate>` below) and only animates any
/// wheel that lacks per-link replication.
///
/// (Original behaviour, kept for the non-replicated case: on a client proxy the
/// chassis is kinematic and the motor is held at zero, so the body never turns — it
/// re-derives the rolling angle from the chassis's [`ReplicatedChassisMotion`] and
/// authors the visual child directly, reconstructing the host's `body_spin · axis_rot`.)
///
/// Guarded to a **kinematic** chassis so it is a no-op on the host/owned rover and
/// never fights the joint-driven body there.
fn animate_proxy_physical_wheels(
    // The wheel's `ChildOf` parent is its chassis. `Without<NetReplicate>`: replicated
    // wheels carry their own spin via the body's world rotation, so skip them (see docstring).
    mut q_wheels: Query<
        (&mut PhysicalWheel, &GlobalTransform, &ChildOf),
        Without<lunco_core::NetReplicate>,
    >,
    q_chassis: Query<
        (
            &RigidBody,
            &Position,
            &Rotation,
            Option<&lunco_core::ReplicatedChassisMotion>,
        ),
        With<lunco_core::ActuatorPorts>,
    >,
    mut q_visual: Query<&mut Transform, Without<PhysicalWheel>>,
    time: Res<Time>,
) {
    use std::f64::consts::TAU;
    // Sign mapping rolling speed → roll about the axle so the contact patch
    // tracks the ground (matches the host's motor-driven body spin). Mirrors the
    // `drive_sign = -1` axle convention used by `MotorActuator`.
    const ROLL_SIGN: f64 = -1.0;

    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }

    for (mut wheel, gtf, child_of) in q_wheels.iter_mut() {
        let Ok((body, pos, rot, motion)) = q_chassis.get(child_of.parent()) else {
            continue;
        };
        // Display proxies only; the host/owned rover spins the body via the joint.
        if !matches!(body, RigidBody::Kinematic) {
            continue;
        }
        // Chassis velocity arrives via the delivered hint (the proxy's avian
        // velocity is force-zeroed). Ground speed of the hub along the wheel's
        // forward axis → rolling rate ω = v_long / r.
        let (vlin, vang) = motion
            .map(|m| (m.lin, m.ang))
            .unwrap_or((DVec3::ZERO, DVec3::ZERO));
        // Reconstruct the hub in the AVIAN cell-local frame from the chassis pose +
        // the authored `mount_local` offset (the rigid axle), exactly as
        // `proxy_wheel_pose`/`reconstruct_proxy_wheels` do. The old code read
        // `gtf.translation()` (big_space render frame) against `pos.0` (avian) — the
        // same CQ-201 frame-mix as the raycast spin integrator, which drifted the
        // rolling rate once the proxy drove ~km from the floating origin. Rotation is
        // frame-safe, so `forward` keeps using `gtf` (it already carries the steer).
        let chassis_pos = lunco_core::coords::GridPos(pos.0);
        let (hub_pos, _) = wheel_hub_pose(
            chassis_pos,
            lunco_core::coords::GridRot(rot.0),
            wheel.mount_local.as_dvec3(),
            DQuat::IDENTITY,
        );
        let hub_vel = wheel_hub_velocity(vlin, vang, hub_pos, chassis_pos);
        let forward = gtf.rotation().mul_vec3(Vec3::NEG_Z).as_dvec3();
        let r = (wheel.wheel_radius as f64).max(1e-3);
        let w = wheel_roll_rate(hub_vel, forward, r);

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

/// Marker: this prim's link/celestial vocabulary has been projected to components.
#[derive(Component)]
struct CelestialProjected;

fn any_unprojected_celestial(
    q: Query<(), (With<UsdPrimPath>, Without<CelestialProjected>)>,
) -> bool {
    !q.is_empty()
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
    mut canonical: NonSendMut<CanonicalStages>,
) {
    for (entity, prim_path) in query.iter() {
        // Read the live canonical stage, built on demand from the recipe — the same
        // source `process_usd_sim_prims` reads.
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages
                .get(&prim_path.stage_handle)
                .and_then(|a| a.recipe.clone())
            {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else {
            continue;
        };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
            continue;
        };
        celestial::insert_celestial_comms_components(
            &cs.view(),
            entity,
            &prim_path.path,
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

/// Walks `entity`'s `ChildOf` ancestry looking for a `UsdPreviewOnly`
/// marker. Stops at the first ancestor that has the marker or when the
/// chain runs out. Bounded by USD scene depth, which is small.
fn is_preview_only(
    entity: Entity,
    q_child_of: &Query<&ChildOf>,
    q_preview_only: &Query<(), With<UsdPreviewOnly>>,
) -> bool {
    let mut cursor = entity;
    loop {
        if q_preview_only.get(cursor).is_ok() {
            return true;
        }
        match q_child_of.get(cursor) {
            Ok(parent) => cursor = parent.parent(),
            Err(_) => return false,
        }
    }
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

/// System that wires wheel drive/steer ports to FSW digital ports.
///
/// Runs every frame, checking for `PendingWheelWiring` markers. Once the FSW root entity
/// exists (has `ActuatorPorts`), it creates [`SimConnection`] entities connecting the
/// wheel's physical ports to the appropriate digital ports. Each end addresses a bare
/// [`Port`] entity through the cosim port backend's `value` connector.
///
/// # Wiring Rules
///
/// USD is the only topology authority:
/// - `inputs:drive.connect = </Rover.outputs:<name>>` on the wheel → wire its
///   drive to that FSW port. The connection is PCP-resolved and path-translated
///   through reference arcs, so a referenced wheel binds to its own instance's port.
/// - `inputs:steer.connect` likewise wires the wheel's steer.
///
/// A wheel without a drive connection is rejected during projection; there is no
/// inferred drive or steering mapping. A named port that is absent from the
/// rover's `ActuatorPorts` is reported and left pending until the authored port
/// exists.
fn try_wire_wheel(
    q_pending: Query<(Entity, &UsdPrimPath, &PendingWheelWiring)>,
    // `ActuatorPorts` does double duty here: it LOCATES the vehicle root (only a rover
    // root carries one) and it is the actuator index the wiring below looks ports up in.
    q_fsw: Query<(Entity, &UsdPrimPath, &lunco_core::ActuatorPorts)>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    mut commands: Commands,
) {
    for (ent, prim_path, pending) in q_pending.iter() {
        let wheel_root = instance_key(ent, &q_provenance, &q_gid, &q_instance_root);
        let vehicle_root = q_fsw.iter().find(|(root_ent, path, _)| {
            path.stage_handle == prim_path.stage_handle
                && prim_path.path.starts_with(&path.path)
                && instance_key(*root_ent, &q_provenance, &q_gid, &q_instance_root) == wheel_root
        });

        if let Some((_, _, actuators)) = vehicle_root {
            // Drive is an authored USD connection. No inferred mapping.
            let drive_port_name = pending.drive_port_name.clone();
            if let Some(d_port) = actuators.get(&drive_port_name) {
                // Owned by the wheel (`ChildOf(ent)`) so it dies with the rover subtree
                // on scene swap — the same general lifecycle contract the ports/joint use.
                commands.spawn((
                    SimConnection {
                        start_element: d_port,
                        start_connector: PORT_NAME.to_string(),
                        end_element: pending.p_drive,
                        end_connector: PORT_NAME.to_string(),
                        ..Default::default()
                    },
                    Name::new(format!("Conn_Drive_{}", drive_port_name)),
                    ChildOf(ent),
                ));
                debug!(
                    "Wired wheel {} drive to FSW port {}",
                    prim_path.path, drive_port_name
                );
            } else {
                warn!(
                    "Wheel {} drive port '{}' not in rover ActuatorPorts; skipping",
                    prim_path.path, drive_port_name
                );
            }

            // Steering is optional, but if present it is also an authored USD
            // connection. An unsteered wheel has no steering endpoint.
            if let Some(name) = pending.steer_port_name.clone() {
                if let Some(s_port) = actuators.get(&name) {
                    commands.spawn((
                        SimConnection {
                            start_element: s_port,
                            start_connector: PORT_NAME.to_string(),
                            end_element: pending.p_steer,
                            end_connector: PORT_NAME.to_string(),
                            ..Default::default()
                        },
                        Name::new(format!("Conn_Steer_{}", name)),
                        ChildOf(ent),
                    ));
                    info!(
                        "Wired wheel {} steering to FSW port {}",
                        prim_path.path, name
                    );
                } else {
                    warn!(
                        "Wheel {} steer port '{}' not in rover ActuatorPorts; skipping",
                        prim_path.path, name
                    );
                }
            }
            commands.entity(ent).remove::<PendingWheelWiring>();
        } else {
            debug!(
                "Wheel {} FSW not found yet, retrying next frame",
                prim_path.path
            );
        }
    }
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
/// Re-runs when a tree's XML changes or when any prim spawns (a waypoint may spawn
/// after the vessel that names it — prim order is not guaranteed). Unresolved paths
/// are simply left out of the map; the compiler refuses a tree with a dangling
/// target rather than driving to the origin.
fn resolve_behavior_targets(
    q_trees: Query<(
        Entity,
        &lunco_autopilot::usd_tree::BehaviorXml,
        Option<&UsdPrimPath>,
    )>,
    q_prims: Query<(Entity, &UsdPrimPath)>,
    q_new_prims: Query<(), Added<UsdPrimPath>>,
    q_changed_xml: Query<(), Changed<lunco_autopilot::usd_tree::BehaviorXml>>,
    q_new_ids: Query<(), Added<lunco_core::GlobalEntityId>>,
    q_existing_bindings: Query<&lunco_autopilot::usd_tree::TargetBindings>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    mut commands: Commands,
) {
    if q_trees.is_empty()
        || (q_new_prims.is_empty() && q_changed_xml.is_empty() && q_new_ids.is_empty())
    {
        return;
    }
    for (vessel, xml, vessel_path) in q_trees.iter() {
        let vessel_instance = instance_key(vessel, &q_provenance, &q_gid, &q_instance_root);
        let mut bindings = lunco_autopilot::usd_tree::TargetBindings::default();
        let mut missing = false;
        let targets = lunco_autopilot::usd_tree::target_paths(&xml.0);
        debug!(
            "[resolve_behavior_targets] vessel {:?} ({}) has {} targets: {:?}",
            vessel,
            vessel_path
                .map(|p| p.path.as_str())
                .unwrap_or("no-usd-path"),
            targets.len(),
            targets
        );
        for path in targets {
            let found = q_prims.iter().find(|(e, p)| {
                let match_path = p.path == path || p.path.ends_with(&path) || path.ends_with(&p.path);
                let match_stage = vessel_path
                    .map(|vp| p.stage_handle == vp.stage_handle)
                    .unwrap_or(true);
                let is_behavior = path.contains("/Route/");
                let inst = instance_key(*e, &q_provenance, &q_gid, &q_instance_root);
                let match_inst = is_behavior
                    || inst.is_none()
                    || vessel_instance.is_none()
                    || inst == vessel_instance;
                if match_path {
                    debug!("[resolve_behavior_targets] candidate {:?} ({}) match_stage={} match_inst={} (is_behavior={})", e, p.path, match_stage, match_inst, is_behavior);
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
        // A binding is an atomic projection: replacing a complete map with a
        // partial map makes the compiler observe a transient dangling target,
        // which used to send the rover toward a default origin for one frame.
        // Keep the previous complete map until the next prim projection gives
        // us every target. The compiler then either has a complete new map or
        // refuses the tree without changing vehicle pose.
        if !missing {
            commands.entity(vessel).try_insert(bindings);
        } else if q_existing_bindings.get(vessel).is_ok() {
            debug!(
                "[resolve_behavior_targets] retaining prior complete bindings for vessel {:?}",
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
    mut commands: Commands,
) {
    for (joint, joint_path, pending) in q_pending.iter() {
        let joint_root = instance_key(joint, &q_provenance, &q_gid, &q_instance_root);
        let find = |target: &str| {
            q_bodies
                .iter()
                .find(|(e, p)| {
                    p.path == target
                        && p.stage_handle == joint_path.stage_handle
                        && instance_key(*e, &q_provenance, &q_gid, &q_instance_root) == joint_root
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
            stiffness: pending.stiffness,
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
        (Entity, &UsdPrimPath, Option<&AuthoredInitialVelocity>),
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
    let mut promoted = false;
    for (entity, path, authored_velocity) in q_kinematic.iter() {
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
        let blocked = ground_pending.0
            || has_pending_joint
            || has_pending_admission
            || has_unready_authored_joint
            || has_pending_diff;
        if !blocked {
            // Despawn-safe: scene-load churn / doc-backed reload can despawn a
            // ShouldBeDynamic entity between this queue and `apply_deferred`; a plain
            // `insert` then panics on the invalid entity. `try_insert`/`try_remove`
            // no-op at apply time if the entity is gone (a `get_entity` guard here
            // would not help — it only proves validity at queue time, not apply).
            commands
                .entity(entity)
                .try_insert((RigidBody::Dynamic, lunco_core::PhysicsStateReady));
            commands
                .entity(entity)
                .try_remove::<lunco_core::PhysicsStatePending>();
            if let Some(velocity) = authored_velocity {
                if let Some(linear) = velocity.linear {
                    commands.entity(entity).try_insert(LinearVelocity(linear));
                }
                if let Some(angular) = velocity.angular {
                    commands.entity(entity).try_insert(AngularVelocity(angular));
                }
                commands
                    .entity(entity)
                    .try_remove::<AuthoredInitialVelocity>();
            }
            commands.entity(entity).try_remove::<ShouldBeDynamic>();
            // A physical wheel is part of the chassis' joint-connected assembly,
            // so marking it moves the whole vehicle as one.
            if q_wheel.contains(entity) {
                commands
                    .entity(entity)
                    .try_insert(lunco_core::NeedsGroundSettle);
            }
            debug!(
                "Activated RigidBody::Dynamic for stage: {:?}",
                path.stage_handle
            );
            promoted = true;
        }
    }
    if promoted {
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
        rel physxVehicleWheelAttachment:suspension = </Rover/Suspension>
        int physxVehicleWheelAttachment:index = 7
    }
    def Xform "Suspension" {}
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
                lunco_core::ReplicatedChassisMotion {
                    lin: DVec3::new(0.0, 0.0, -2.0), // 2 m/s along chassis forward (−Z)
                    ang: DVec3::ZERO,
                },
                lunco_core::ActuatorPorts::default(),
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
                steers: false,
                wheelbase: 0.0,
            },
            GlobalTransform::IDENTITY,
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
        // rotation and the visual child inherits it; this fallback animator must
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
                lunco_core::ReplicatedChassisMotion {
                    lin: DVec3::new(0.0, 0.0, -2.0),
                    ang: DVec3::ZERO,
                },
                lunco_core::ActuatorPorts::default(),
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
                steers: false,
                wheelbase: 0.0,
            },
            GlobalTransform::IDENTITY,
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
                lunco_core::ReplicatedChassisMotion {
                    lin: DVec3::ZERO,
                    ang,
                },
                lunco_core::ActuatorPorts::default(),
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
                steers: false,
                wheelbase: 0.0,
            },
            GlobalTransform::from(Transform::from_translation(wheel_gtf_translation)),
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
    fn rear_wheel_never_steers() {
        // steers=false ⇒ δ=0 regardless of motion.
        let s = super::proxy_wheel_steer(false, 2.0, DVec3::new(3.0, 0.0, 0.0), DVec3::Y);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn front_wheel_below_speed_threshold_holds_straight() {
        // Ground speed ≤ 0.25 m/s ⇒ yaw/speed ratio is meaningless ⇒ δ=0.
        let s = super::proxy_wheel_steer(true, 2.0, DVec3::new(0.0, 0.0, -0.2), DVec3::Y);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn front_wheel_ackermann_angle() {
        // tan δ = wheelbase · yaw_rate / speed. wheelbase=2, yaw=0.5, speed=2 (along −Z)
        // ⇒ δ = atan(2·0.5/2) = atan(0.5).
        let wheelbase = 2.0;
        let yaw = 0.5;
        let s = super::proxy_wheel_steer(
            true,
            wheelbase,
            DVec3::new(0.0, 0.0, -2.0),
            DVec3::new(0.0, yaw, 0.0),
        );
        let expected = (wheelbase * yaw / 2.0_f64).atan();
        assert!((s - expected).abs() < 1e-12, "δ={s}, expected {expected}");
        // Vertical (y) velocity must not leak into the planar speed used for the ratio.
        let s_with_vy = super::proxy_wheel_steer(
            true,
            wheelbase,
            DVec3::new(0.0, 9.0, -2.0),
            DVec3::new(0.0, yaw, 0.0),
        );
        assert!(
            (s_with_vy - expected).abs() < 1e-12,
            "vy leaked: δ={s_with_vy}"
        );
    }

    #[test]
    fn front_wheel_steer_is_clamped() {
        // A huge yaw/speed ratio saturates at ±0.6 rad, and sign tracks yaw.
        let hi = super::proxy_wheel_steer(
            true,
            100.0,
            DVec3::new(0.0, 0.0, -1.0),
            DVec3::new(0.0, 5.0, 0.0),
        );
        assert!((hi - 0.6).abs() < 1e-12, "δ={hi}");
        let lo = super::proxy_wheel_steer(
            true,
            100.0,
            DVec3::new(0.0, 0.0, -1.0),
            DVec3::new(0.0, -5.0, 0.0),
        );
        assert!((lo + 0.6).abs() < 1e-12, "δ={lo}");
    }

    #[test]
    fn proxy_pose_at_identity_chassis_is_mount_offset() {
        // Chassis at origin, no rotation, no steer ⇒ wheel sits exactly at mount_local.
        let mount = DVec3::new(0.8, -0.3, 1.2);
        let (p, q) = super::proxy_wheel_pose(DVec3::ZERO, DQuat::IDENTITY, mount, 0.0);
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
        let (p, q) = super::proxy_wheel_pose(chassis_pos, chassis_rot, mount, 0.0);
        let expected = chassis_pos + DVec3::new(1.0, 0.0, 0.0);
        assert!(
            (p - expected).length() < 1e-9,
            "p={p:?}, expected {expected:?}"
        );
        // No steer ⇒ wheel rotation equals the chassis rotation.
        assert!(q.angle_between(chassis_rot) < 1e-9, "q={q:?}");
    }

    #[test]
    fn proxy_pose_steer_composes_after_chassis() {
        // The steer yaw is applied in the chassis frame (chassis ∘ steer), so the
        // resulting wheel yaw is the sum of the two about a shared +Y axis, and the
        // mount position is unaffected by steer.
        let chassis_rot = DQuat::from_rotation_y(0.3);
        let mount = DVec3::new(0.5, 0.0, 1.0);
        let steer = 0.2;
        let (p, q) = super::proxy_wheel_pose(DVec3::ZERO, chassis_rot, mount, steer);
        let expected_rot = DQuat::from_rotation_y(0.3 + 0.2);
        assert!(q.angle_between(expected_rot) < 1e-9, "q={q:?}");
        // Position depends only on chassis pose + mount, not the steer angle.
        let (p0, _) = super::proxy_wheel_pose(DVec3::ZERO, chassis_rot, mount, 0.0);
        assert!(
            (p - p0).length() < 1e-12,
            "steer moved the hub: {p:?} vs {p0:?}"
        );
    }
}
