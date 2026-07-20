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
//!   it via `rel physics:body1`. Motor torque comes from the joint's
//!   `drive:angular:physics:maxForce` (`UsdPhysicsDriveAPI:angular`); the
//!   constraint is built by `lunco-usd-avian`. The wheel becomes a full
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

use bevy::prelude::*;
use bevy::math::{DQuat, DVec3};
use avian3d::prelude::*;
use big_space::prelude::{CellCoord, FloatingOrigin, Grid};
pub use lunco_usd_bevy::{UsdPreviewOnly, UsdPrimPath, UsdStageAsset, UsdInstanceRoot};
use lunco_usd_bevy::{instance_key, CanonicalStages, UsdRead};
use lunco_usd_avian::ShouldBeDynamic;
// Appearance + camera **intent** — this crate must never name `MeshMaterial3d`,
// `StandardMaterial`, `ShaderMaterial` or `Camera3d` (all `bevy_pbr` /
// `bevy_core_pipeline` → wgpu + naga). `lunco-render-bevy` binds these.
// See docs/architecture/render-decoupling.md.
use lunco_materials::ShaderLook;
use lunco_render::{PbrLook, SceneCamera};
use openusd::sdf::Path as SdfPath;
use lunco_mobility::{WheelRaycast, DifferentialCoupling, SuspensionPiston, SuspensionSpring, Suspension};
use lunco_mobility::kernels::DriveMix;
use lunco_mobility::wheel_kinematics::{wheel_hub_pose, wheel_hub_velocity, wheel_roll_rate};
use lunco_core::architecture::Port;
use lunco_cosim::{ports::PORT_NAME, SimConnection};
use lunco_hardware::{MotorActuator, SteeringActuator};
use lunco_avatar::{FreeFlightCamera, OrbitCamera, SpringArmCamera, AdaptiveNearPlane, ProvisionalAvatarCamera};
use lunco_core::{Avatar, LocalAvatar};
use lunco_core::architecture::IntentAnalogState;
use leafwing_input_manager::prelude::ActionState;
use lunco_controller::get_avatar_input_map;
use std::collections::HashMap;

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
///   `physics:body1` rel → joint-based path. Motor torque comes from the
///   joint's `drive:angular:physics:maxForce` (`UsdPhysicsDriveAPI:angular`).
///   The joint constraint itself is built by `lunco-usd-avian`.
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
/// (`lunco-sandbox`'s headless boot, `lunco-usd`'s integration tests), and because
/// it remains a correct, cheap "don't wait" switch. Removing it is a separate,
/// cross-crate change.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct NoRenderVisuals;

pub struct UsdSimPlugin;

impl Plugin for UsdSimPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<PhysicalWheel>()
           // Client-only: reconstruct a remote rover's wheels from its chassis
           // (kinematic followers — wheels are no longer replicated), then re-derive
           // the cosmetic visual roll. Chained so the visual spin layers on the
           // freshly-placed body. Same `relative_speed > 0` gate as raycast wheels.
           .add_systems(FixedUpdate, (reconstruct_proxy_wheels, animate_proxy_physical_wheels)
               .chain()
               .run_if(|t: Res<Time<Virtual>>| !t.is_paused() && t.relative_speed_f64() > 0.0))
            .add_observer(on_add_usd_sim_prim)
           // `try_wire_wheel` runs in PreUpdate so that the `SimConnection` entities
           // exist before cosim propagation pushes values through them.
           .add_systems(
               PreUpdate,
               (try_wire_wheel, resolve_differential_coupling, resolve_behavior_targets),
           )
           // USD → ShaderMaterial authoring. Ordered AFTER the visuals exist
           // and BEFORE `process_usd_sim_prims` consumes them, so the material
           // is always present before a wheel is split onto its visual child
           // (Bevy auto-inserts the sync point). Race-free by construction —
           // see `shader.rs`.
           .add_systems(Update, shader::apply_usd_shader_materials
               .after(lunco_usd_bevy::sync_usd_visuals)
               .before(process_usd_sim_prims))
           // `process_usd_sim_prims` does a per-stage joint scan + per-
           // entity dispatch — too coupled to fit cleanly into a single
           // `OnAdd<UsdVisualSynced>` observer. Gating with `run_if`
           // skips the system entirely on frames with no unprocessed
           // USD prim (archetype-level check, near-zero cost).
           .init_resource::<GroundColliderPending>()
           .add_systems(Update, (
                process_usd_sim_prims
                    .run_if(any_unprocessed_usd_sim)
                    .after(lunco_usd_bevy::sync_usd_visuals),
                // Independent link/celestial projector — runs for EVERY prim (cosim,
                // wheel, plain), gated by its own marker, blocked by nothing.
                project_celestial_comms_prims
                    .run_if(any_unprojected_celestial)
                    .after(lunco_usd_bevy::sync_usd_visuals),
                activate_dynamic_bodies
                    .run_if(any_with_component::<ShouldBeDynamic>),
            ));
        // Self-healing watchdog: a USD prim that stays unprocessed forever means
        // an unmet dependency is silently deadlocking setup (historically the
        // wheel-shader bug: physics deferred until a render-only `ShaderMaterial`
        // that never arrived headless — structurally impossible now that the waits
        // are on render-free intent, see `NoRenderVisuals`). This turns that class
        // of invisible deadlock into a loud `error!` AND recovers by building the
        // physics without the missing visual.
        app.add_systems(Update, recover_stuck_usd_prims);
        // USD → cosim wiring (`lunco:modelicaModel`, `lunco:scriptModel`,
        // `lunco:simWires`) — see `cosim.rs`.
        cosim::install(app);
    }
}

/// USD-authored screen-facing text labels (`lunco:billboard*`) — a prim
/// declares its own label content, including live geolocation.
pub mod billboard;
pub mod celestial;
pub mod cosim;
pub mod powertrain;
pub use cosim::{CosimStatusProvider, UsdSourcedCosim};

/// USD → [`ShaderMaterial`](lunco_materials::ShaderMaterial) authoring,
/// deterministically ordered so it can never race a downstream consumer.
pub mod shader;

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
    pub index: i32,
    pub p_drive: Entity,
    pub p_steer: Entity,
    /// G4: USD-authored actuator binding — the port name resolved from the
    /// wheel's `inputs:drive.connect` connection (the target property minus its
    /// `outputs:` prefix). When `Some`, this wheel wires to that FSW port
    /// verbatim — overriding the index-parity default — so arbitrary topologies
    /// (per-wheel drive, 6-wheel, rocker-bogie) are declared in USD rather than
    /// hardcoded in `try_wire_wheel`.
    pub drive_port_name: Option<String>,
    /// G4: USD-authored steer binding, resolved the same way from the wheel's
    /// `inputs:steer.connect`. `Some` overrides the `index < 2` front-steer default.
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
    /// Hinge axis in the chassis-local frame.
    pub axis: DVec3,
    /// Authored `physxGearJoint:gearRatio` — the `r` in `θ_a = r·θ_b`. Was read
    /// and logged but THROWN AWAY before this field existed, so every geared
    /// joint silently behaved as the `-1` mirror case no matter what the scene
    /// authored.
    pub ratio: f64,
    pub rest_offset: f64,
    pub stiffness: f64,
    pub damping: f64,
}

/// Process USD prims for sim mapping AFTER their assets are loaded.
///
/// This is the core system that maps USD schemas to LunCoSim components. It runs in the
/// `Update` schedule **after** `sync_usd_visuals` to ensure meshes and transforms exist.
///
/// # What It Does
///
/// 1. **Detects `PhysxVehicleContextAPI`** → Creates `ActuatorPorts` with 4 digital ports
///    (`drive_left`, `drive_right`, `steering`, `brake`), plus `Vessel`.
/// 2. **Detects `PhysxVehicleTankDifferentialAPI`** → `DriveMix { kernel: "skid" }`.
/// 3. **Detects `PhysxVehicleAckermannSteeringAPI`** → `DriveMix { kernel: "linear" }` + steering.
///    (A `lunco:driveKernel` attribute overrides both → `DriveMix { kernel: <hook_id> }`,
///    a scripted rhai kernel — the imperative analog of an Omniverse OmniGraph controller.)
/// 4. **Detects `PhysxVehicleWheelAPI`** → Sets up wheel based on whether an authored
///    `PhysicsRevoluteJoint` targets the wheel:
///    - **Joint-based** (joint authored): `RigidBody`, `Collider`, `MotorActuator` (constraint built by `lunco-usd-avian`)
///    - **Raycast** (no joint): `WheelRaycast`, `RayCaster` (entity split into physics + visual child)
/// Run condition: true when any `UsdPrimPath` entity still lacks
/// `UsdSimProcessed`. Lets `process_usd_sim_prims` stay dormant after
/// scene-load is complete instead of running every frame.
fn any_unprocessed_usd_sim(
    q: Query<(), (With<UsdPrimPath>, Without<UsdSimProcessed>)>,
) -> bool {
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
    query: Query<(Entity, &UsdPrimPath, Option<&Transform>, Option<&Mesh3d>, Option<&PbrLook>, Option<&ShaderLook>, Option<&ChildOf>, Option<&ForceBuildNoVisual>), Without<UsdSimProcessed>>,
    q_all_prims: Query<&UsdPrimPath>,
    q_grids: Query<(Entity, &Grid)>,
    q_existing_floating_origins: Query<Entity, With<FloatingOrigin>>,
    q_provisional_cameras: Query<Entity, With<ProvisionalAvatarCamera>>,
    q_prior_avatars: Query<Entity, With<Avatar>>,
    q_child_of: Query<&ChildOf>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    stages: Res<Assets<UsdStageAsset>>,
    // Read the LIVE canonical stage (source of truth), built on demand from
    // the asset recipe.
    mut canonical: NonSendMut<CanonicalStages>,
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
    // --- Pass 1: collect authored revolute joints by their `body1` target ---
    //
    // Standard OpenUSD: a `def PhysicsRevoluteJoint` declares `rel
    // physics:body1 = </path/to/wheel>`. Presence of such a joint
    // targeting a wheel prim is the discriminator between joint-based
    // and raycast wheels — no custom `lunco:` tokens are involved.
    //
    // We also remember the joint's prim path so the joint-based wheel
    // setup can read `drive:angular:physics:maxForce` (the motor stall
    // torque, authored via `UsdPhysicsDriveAPI:angular`) from it.
    let mut joint_targets: HashMap<(Handle<UsdStageAsset>, String), String> = HashMap::new();
    // Articulated ROOTS, derived from the SAME joint scan: a `PhysicsRevoluteJoint`'s
    // `physics:body0` is the chassis the wheel hinges to. Keyed identically to
    // `joint_targets` so a prim's own `(stage, path)` looks up in both. This (plus
    // any `PhysicsArticulationRootAPI` prim) is the declarative source of truth for
    // `ArticulatedVehicle`, replacing the old `setup_physical_wheel` side-effect +
    // runtime `ChildOf` walk. See `crates/lunco-networking/USD_REPLICATION_POLICY.md`.
    let mut articulation_roots: std::collections::HashSet<(Handle<UsdStageAsset>, String)> =
        Default::default();
    // WheelAttachment → (wheel, suspension) target pairs, scanned in the same
    // Pass 1. The CANONICAL (Omniverse) suspension binding: a
    // `PhysxVehicleWheelAttachmentAPI` prim references a wheel prim via
    // `physxVehicleWheelAttachment:wheel` and a suspension prim via
    // `physxVehicleWheelAttachment:suspension`. Keyed by (stage, WHEEL path) —
    // the same key shape as `joint_targets`, since a prim path is unique only
    // within its stage — so the wheel branch (Pass 2) can look up its suspension
    // prim in O(1). Empty for
    // LunCo's flat composition (wheel references suspension directly) — see
    // doc 53 §3.2.
    let mut wheel_attachment_targets: HashMap<(Handle<UsdStageAsset>, String), String> =
        HashMap::new();

    // Scan the **stage data** rather than spawned entities. Joint and
    // wheel prims may be spawned on different frames; reading from the
    // SDF data directly avoids the race where a wheel is processed
    // before its joint sibling has an entity in the ECS.
    //
    // TODO(CQ-212): this Pass-1 re-scans every spec of every stage on
    // *every frame* to rebuild `joint_targets` / `articulation_roots`,
    // even when no stage SDF changed. Cache a per-stage joint index
    // (keyed by stage `Handle` + an asset-change/generation stamp) and
    // only rescan a stage when its SDF actually mutates; readers then do
    // a direct path→spec lookup. (Sibling spots: `shader.rs` reads scan
    // the whole stage per prim; `loaded_stages.rs` `prim_type_name` is an
    // O(n²) tree render.) Deferred per request — not modifying USD here.
    // See docs/code-quality-remediation.md (CQ-212).
    let mut seen_stages: std::collections::HashSet<Handle<UsdStageAsset>> = Default::default();
    for prim_path in q_all_prims.iter() {
        if !seen_stages.insert(prim_path.stage_handle.clone()) { continue; }
        // Scan the live canonical stage, built on demand from the recipe.
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else { continue };
        collect_joint_scan_read(
            &cs.view(), &prim_path.stage_handle, &mut joint_targets, &mut articulation_roots,
            &mut wheel_attachment_targets,
        );
    }

    // --- Pass 2: Process all prims ---
    for (entity, prim_path, maybe_tf, maybe_mesh, maybe_mat, maybe_shader_mat, maybe_child_of, force_build) in query.iter() {
        // Per-prim escape hatch: the recovery watchdog stamped this prim after it
        // was deferred too long, so stop waiting for its visual (as if headless).
        let wait_for_visuals = visuals_coming && force_build.is_none();
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

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
            if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else { continue };
        process_usd_sim_prim_read(
            &cs.view(), entity, prim_path, sdf_path.clone(), maybe_tf, maybe_mesh, maybe_mat,
            maybe_shader_mat, maybe_child_of, wait_for_visuals, &joint_targets,
            &articulation_roots, &wheel_attachment_targets, &q_existing_floating_origins,
            &q_provisional_cameras, &q_prior_avatars, &q_grids, active_sun.as_deref(),
            &mut commands,
        );
    }
}

/// Per-stage joint scan (Pass 1), generic over the read source ([`UsdRead`]):
/// collects `PhysicsRevoluteJoint` `body1` targets (wheel dispatch) and `body0`
/// targets (articulation roots) off either the live canonical `StageView` or the
/// flattened `sdf::Data`, identically. Also collects the canonical
/// `PhysxVehicleWheelAttachmentAPI` wheel→suspension bindings (doc 53 §3.2).
fn collect_joint_scan_read(
    reader: &lunco_usd_bevy::StageView<'_>,
    stage_handle: &Handle<UsdStageAsset>,
    joint_targets: &mut HashMap<(Handle<UsdStageAsset>, String), String>,
    articulation_roots: &mut std::collections::HashSet<(Handle<UsdStageAsset>, String)>,
    wheel_attachment_targets: &mut HashMap<(Handle<UsdStageAsset>, String), String>,
) {
    for path in reader.prim_paths() {
        if reader.type_name(&path).as_deref() == Some("PhysicsRevoluteJoint") {
            if let Some(body1) = reader.rel_target(&path, "physics:body1") {
                debug!("USD joint dispatch: {} → wheel {}", path.as_str(), body1);
                joint_targets.insert(
                    (stage_handle.clone(), body1),
                    path.as_str().to_string(),
                );
            }
            if let Some(body0) = reader.rel_target(&path, "physics:body0") {
                articulation_roots.insert((stage_handle.clone(), body0));
            }
        }
        // Canonical wheel-attachment binding: an attachment prim names its wheel
        // (`physxVehicleWheelAttachment:wheel`) and its suspension
        // (`physxVehicleWheelAttachment:suspension`) via USD relationships. We
        // record wheel→suspension so Pass 2 can resolve a wheel's compliance from
        // the referenced suspension prim rather than only from attrs on the wheel
        // itself. See doc 53 §3.2.
        if reader.has_api_schema(&path, "PhysxVehicleWheelAttachmentAPI") {
            if let (Some(wheel), Some(suspension)) = (
                reader.rel_target(&path, "physxVehicleWheelAttachment:wheel"),
                reader.rel_target(&path, "physxVehicleWheelAttachment:suspension"),
            ) {
                debug!(
                    "USD wheel attachment: wheel {} → suspension {} (via {})",
                    wheel,
                    suspension,
                    path.as_str()
                );
                wheel_attachment_targets
                    .insert((stage_handle.clone(), wheel), suspension);
            }
        }
    }
}

/// Per-prim sim-schema extractor (Pass 2), generic over the read source
/// ([`UsdRead`]) — maps one composed prim's authored `lunco:*` / PhysX-vehicle
/// schemas to its sim/avatar/wheel components off either the live canonical
/// `StageView` or the flattened `sdf::Data`, identically.
#[allow(clippy::too_many_arguments)]
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
    joint_targets: &HashMap<(Handle<UsdStageAsset>, String), String>,
    articulation_roots: &std::collections::HashSet<(Handle<UsdStageAsset>, String)>,
    wheel_attachment_targets: &HashMap<(Handle<UsdStageAsset>, String), String>,
    q_existing_floating_origins: &Query<Entity, With<FloatingOrigin>>,
    q_provisional_cameras: &Query<Entity, With<ProvisionalAvatarCamera>>,
    q_prior_avatars: &Query<Entity, With<Avatar>>,
    q_grids: &Query<(Entity, &Grid)>,
    active_sun: Option<&lunco_environment::LunarSun>,
    mut commands: &mut Commands,
) {
        let existing_tf = maybe_tf.cloned().unwrap_or_default();

        // --- Network replication policy, derived from USD ---
        // Structure from the joint graph (Pass 1) + `lunco:net:*` overrides. Stamps
        // the structural markers (`ArticulatedVehicle`/`ArticulatedLink`) and any
        // explicit opt-out / opacity override; the DEFAULT "replicate every non-static
        // rigid body" is applied downstream by `apply_net_replication` (it needs the
        // live avian `RigidBody`, which materialises later). Runs once per prim (this
        // pass is gated `Without<UsdSimProcessed>`). Replaces the old runtime `ChildOf`
        // walk + `setup_physical_wheel` side-effect. See USD_REPLICATION_POLICY.md.
        let net_key = (prim_path.stage_handle.clone(), prim_path.path.clone());
        if articulation_roots.contains(&net_key)
            || reader.has_api_schema(&sdf_path, "PhysicsArticulationRootAPI")
        {
            commands.entity(entity).try_insert(lunco_core::ArticulatedVehicle);
        }
        if joint_targets.contains_key(&net_key) {
            commands.entity(entity).try_insert(lunco_core::ArticulatedLink);
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

        let net_replicate = reader.scalar::<bool>(&sdf_path, "lunco:net:replicate");
        let net_authority = reader.text(&sdf_path, "lunco:net:authority");
        let (net_excluded, net_opaque) =
            net_override_markers(net_replicate, net_authority.as_deref());
        if net_excluded {
            commands.entity(entity).try_insert(lunco_core::NetExcluded);
        }
        if net_opaque {
            commands.entity(entity).try_insert(lunco_core::NotPredictable);
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
            match reader.text(&sdf_path, "lunco:suspensionVisual:role").as_deref() {
                Some("piston") => {
                    commands.entity(entity).try_insert(SuspensionPiston {
                        initial_y: existing_tf.translation.y,
                    });
                }
                Some("spring") => {
                    commands.entity(entity).try_insert(SuspensionSpring);
                }
                // The API's whole purpose is the role; applying it without one (or
                // with a token outside `allowedTokens`) is an authoring mistake.
                other => warn!(
                    "USD prim {} applies LunCoSuspensionVisualAPI but its \
                     lunco:suspensionVisual:role is {:?} — expected \"piston\" or \
                     \"spring\"; no visual will be animated.",
                    sdf_path.as_str(),
                    other.unwrap_or("<unauthored>")
                ),
            }
        }

        // USD-authored sensors → cosim telemetry ports (lunco-cosim::sensors).
        // Each marker turns the body's port surface on for that sensor kind; the
        // sensor systems fill the values each tick. `lunco:sensor:offset` is the
        // shared body-local mount point (lever arm from the COM).
        let sensor_offset = lunco_usd_bevy::read_vec3_f64(reader, &sdf_path, "lunco:sensor:offset")
            .map(|v| DVec3::new(v[0], v[1], v[2]))
            .unwrap_or(DVec3::ZERO);
        if reader.scalar::<bool>(&sdf_path, "lunco:sensor:imu").is_some() {
            commands.entity(entity).try_insert(lunco_cosim::sensors::ImuSensor::mounted(sensor_offset));
        }
        if reader.scalar::<bool>(&sdf_path, "lunco:sensor:range").is_some() {
            let axis = match reader.text(&sdf_path, "lunco:sensor:rangeAxis").as_deref() {
                Some("X") => DVec3::X,
                Some("-X") => DVec3::NEG_X,
                Some("Y") => DVec3::Y,
                Some("Z") => DVec3::Z,
                Some("-Z") => DVec3::NEG_Z,
                // Default and explicit "-Y": a downward altimeter.
                _ => DVec3::NEG_Y,
            };
            let max_distance = reader.real(&sdf_path, "lunco:sensor:rangeMax").unwrap_or(100.0);
            let out_of_range_mode = match reader
                .text(&sdf_path, "lunco:sensor:rangeOutOfRangeMode")
                .as_deref()
            {
                Some("NegativeOne") => lunco_cosim::sensors::OutOfRangeMode::NegativeOne,
                Some("NaN") => lunco_cosim::sensors::OutOfRangeMode::NaN,
                Some("IdealAltitude") => lunco_cosim::sensors::OutOfRangeMode::IdealAltitude,
                _ => lunco_cosim::sensors::OutOfRangeMode::MaxDistance,
            };
            commands.entity(entity).try_insert(lunco_cosim::sensors::RangeSensor {
                offset: sensor_offset,
                axis,
                max_distance,
                distance: max_distance,
                out_of_range_mode,
                ..default()
            });
        }
        if reader.scalar::<bool>(&sdf_path, "lunco:sensor:contact").is_some() {
            commands.entity(entity).try_insert(lunco_cosim::sensors::ContactSensor::default());
        }

        // USD-authored TELEMETRY channel → `lunco_core::telemetry::Parameter`.
        //
        // A channel is a named, rate-limited, clock-bound view of one live value. The
        // source is either a PORT (the fast path — and note the sensors authored just
        // above already expose ports, so `lunco:telemetry:port` can simply name one of
        // them) or a reflection path (the escape hatch, for a field no port exposes).
        //
        // Only `retention` is measured in samples rather than seconds: it is what bounds
        // memory, and letting someone raise the rate must not silently multiply the
        // buffer. See docs/architecture/telemetry-subsystem.md.
        if reader.scalar::<bool>(&sdf_path, "lunco:telemetry").unwrap_or(false) {
            let port = reader.text(&sdf_path, "lunco:telemetry:port");
            let reflect = reader.text(&sdf_path, "lunco:telemetry:reflect");
            let source = match (port, reflect) {
                (Some(p), _) => Some(lunco_core::telemetry::ChannelSource::Port(p)),
                (None, Some(r)) => Some(lunco_core::telemetry::ChannelSource::Reflect(r)),
                (None, None) => {
                    warn!(
                        "{sdf_path}: lunco:telemetry is set but neither lunco:telemetry:port \
                         nor lunco:telemetry:reflect names a source — no channel authored"
                    );
                    None
                }
            };
            if let Some(source) = source {
                // Default the mnemonic to the port/field name rather than refusing: a
                // channel whose name you didn't bother to pick is still a channel.
                let name = reader
                    .text(&sdf_path, "lunco:telemetry:name")
                    .unwrap_or_else(|| match &source {
                        lunco_core::telemetry::ChannelSource::Port(p) => p.clone(),
                        lunco_core::telemetry::ChannelSource::Reflect(r) => r.clone(),
                        // Not authorable from USD — a Diagnostic is engine-global, not a
                        // property of a prim. `lunco-telemetry` publishes those itself.
                        lunco_core::telemetry::ChannelSource::Diagnostic(d) => d.clone(),
                    });
                commands.entity(entity).try_insert(lunco_core::telemetry::Parameter {
                    name,
                    // The tag sits on the prim it measures — no indirection needed. (A channel
                    // created through the API is its own entity and sets `target`, because a
                    // Component caps an entity at one channel.)
                    target: None,
                    unit: reader.text(&sdf_path, "lunco:telemetry:unit").unwrap_or_default(),
                    source,
                    rate_hz: reader.real(&sdf_path, "lunco:telemetry:rateHz"),
                    // Absent ⇒ enabled. An authored channel is a live one; you turn it off
                    // by saying so, not by forgetting to say anything.
                    enabled: reader
                        .scalar::<bool>(&sdf_path, "lunco:telemetry:enabled")
                        .unwrap_or(true),
                    deadband: reader.real(&sdf_path, "lunco:telemetry:deadband"),
                    retention: reader
                        .scalar::<i64>(&sdf_path, "lunco:telemetry:retention")
                        .map(|n| n.max(1) as usize),
                });
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
            info!("Detected Avatar prim at {}, setting up camera", prim_path.path);
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
            // Same takeover for PRIOR AVATAR entities. A stage recompose can
            // hand this prim a FRESH ECS entity while an earlier pass's avatar
            // entity lives on (this system's `Without<UsdSimProcessed>` marker
            // proves each pass processes a new entity). Two live
            // `Avatar`+`Camera3d` entities render ambiguously and SPLIT the
            // input/possession path: a click binds the chase camera on one
            // avatar while the window renders the other ("possessed but the
            // camera is frozen"), keyboard drives every avatar's linked vessel
            // at once, and Backspace releases twice. Strip the avatar role off
            // every prior holder — the newest authored pass wins.
            for prior in q_prior_avatars.iter() {
                if prior != entity {
                    warn!(
                        "[avatar] stripping avatar role from prior entity {prior} \
                         (superseded by re-composed prim {})",
                        prim_path.path
                    );
                    commands.entity(prior).try_remove::<(
                        SceneCamera,
                        Avatar,
                        LocalAvatar,
                        FreeFlightCamera,
                        OrbitCamera,
                        SpringArmCamera,
                        lunco_avatar::SurfaceRelativeMode,
                        lunco_controller::ControllerLink,
                        IntentAnalogState,
                        ActionState<lunco_core::UserIntent>,
                    )>();
                    // DEACTIVATE the prior camera — do NOT remove `Camera`.
                    //
                    // The old code REMOVED `Camera`/`RenderTarget`/`Projection` to
                    // kill a GHOST second active window camera (a bare active camera
                    // that `reconcile_scene_viewport`, filtered `With<SceneCamera>`,
                    // could no longer reach once its `SceneCamera` was stripped). But
                    // removing `Camera` from a still-ACTIVE, still-extracted window
                    // camera in the SAME frame the new scene's shadow-casting sun
                    // initialises orphaned its render-world view for one frame:
                    // `build_directional_light_cascades` (main world) had already
                    // dropped that entity's cascade, so `bevy_pbr::prepare_lights`
                    // unwrapped `None` and hard-crashed the render app — deterministically
                    // on every elevated scene load (the moonbase Sun casts shadows; the
                    // flat sandbox sun does not, which is why it "used to work").
                    //
                    // Setting `is_active = false` reaches the SAME goal without the
                    // race: an inactive camera is not extracted as a view (so it needs
                    // no cascade) and does not render (so no ghost). Deactivation is a
                    // normal per-frame operation bevy handles cleanly; component
                    // REMOVAL of a live camera is what raced. `SceneCamera`
                    // is still stripped above, so `reconcile_scene_viewport` leaves it
                    // alone and it stays off for good.
                    commands.queue(move |world: &mut World| {
                        if let Some(mut cam) = world.get_mut::<bevy::camera::Camera>(prior) {
                            cam.is_active = false;
                        }
                    });
                }
            }
            // `token`, per luncoSchema — so `text`, not `scalar::<String>`, which
            // matches `Value::String` alone and reads every token as `None`.
            let camera_mode = reader
                .text(&sdf_path, "lunco:cameraMode")
                .unwrap_or_else(|| "freeflight".to_string());
            let mut yaw = reader
                .real_f32(&sdf_path, "lunco:cameraYaw")
                .unwrap_or(std::f32::consts::PI * 0.8);
            let mut pitch = reader.real_f32(&sdf_path, "lunco:cameraPitch").unwrap_or(-0.3);

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
            if let Some([lx, ly, lz]) = lunco_usd_bevy::read_vec3_f64(reader, &sdf_path, "lunco:cameraLookAt") {
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
            let (avatar_cell, avatar_local) = match q_grids.iter().next() {
                Some((_, grid)) => grid.translation_to_grid(existing_tf.translation.as_dvec3()),
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
            let ev100 = active_sun
                .copied()
                .unwrap_or_default()
                .exposure_ev100;
            // AgX tonemapping: a filmic curve that rolls off the blown highlights
            // and lifts the toe of the brutal grazing-sun terminator (vs the hard
            // clip that read as pure white/black), while keeping the realistic
            // high-contrast lunar exposure (ev100 stays lunar-calibrated).
            let camera_look = move || {
                (
                    // Spawn INACTIVE. `reconcile_scene_viewport` is the ONE
                    // writer of `Camera::is_active` and turns the bound camera
                    // on within a frame — but a `Camera` left at its default is
                    // active the moment it spawns, so a
                    // stage recompose that re-instantiates this prim renders as
                    // a SECOND active order-0 window camera (Bevy's per-frame
                    // "camera order ambiguities" warning + the whole scene
                    // rendered twice) until the arbiter and the prior avatar's
                    // deferred despawn catch up.
                    bevy::camera::Camera { is_active: false, ..Default::default() },
                    bevy::camera::Exposure { ev100 },
                    // Camera INTENT: `lunco-render-bevy` binds `Camera3d` +
                    // `Tonemapping::AgX` + MSAA. Render-free here, and it is what
                    // every "which entity is the scene camera?" query filters on.
                    SceneCamera::agx(),
                )
            };

            // Build camera based on mode, then parent to Grid for FloatingOrigin
            match camera_mode.as_str() {
                "freeflight" => {
                    commands.entity(entity).try_insert((
                        camera_look(),
                        FreeFlightCamera { yaw, pitch, damping: None },
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
                    warn!("Unknown camera mode '{}' for avatar at {}, using freeflight", camera_mode, prim_path.path);
                    commands.entity(entity).try_insert((
                        camera_look(),
                        FreeFlightCamera { yaw, pitch, damping: None },
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
            // Parent to Grid so FloatingOrigin works
            if let Some((g, _)) = q_grids.iter().next() {
                commands.entity(entity).try_insert(ChildOf(g));
            }
        }

        // 1. Detect PhysxVehicleContextAPI (The Rover Root)
        // Stamps `ActuatorPorts`: the 4 canonical digital actuator ports, plus any the
        // vessel declares as `outputs:` attributes.
        if reader.has_api_schema(&sdf_path, "PhysxVehicleContextAPI") {
            info!("Intercepted PhysxVehicleContextAPI for {}, initializing vessel control surface", prim_path.path);

            let mut port_map = HashMap::new();
            // Canonical actuator ports the built-in skid/Ackermann mix drives.
            let mut port_names: Vec<String> =
                ["drive_left", "drive_right", "steering", "brake"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            // Extra actuator ports the vessel DECLARES, as authored `outputs:`
            // attributes on the vessel prim — a port is an attribute, the same way a
            // command is an `inputs:` attribute. That lets a dynamic vehicle expose
            // custom per-wheel actuators (`outputs:drive_w0` …) that wheels bind to
            // with a USD CONNECTION and a wire/rhai/Modelica mix drives: arbitrary
            // topology authored in USD, not hardcoded here. Deduped against the
            // canonical set, so a vessel may re-declare a canonical port harmlessly.
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
            for name in &port_names {
                // `ChildOf(entity)`: the actuator ports are owned by the vehicle so the
                // recursive scene-clear reclaims them with it — no detached-at-root
                // survivors across a scene swap (general lifecycle contract).
                let port_ent = commands.spawn((
                    Port::default(),
                    Name::new(format!("Port_{}", name)),
                    ChildOf(entity),
                )).id();
                port_map.insert(name.clone(), port_ent);
            }

            commands.entity(entity).try_insert((
                lunco_core::SelectableRoot,
                // Rovers have a meaningful "upright" — opt into overturn
                // recovery (see `lunco_terrain_surface::collider_ring`).
                lunco_core::KeepUpright,
            ));

            // The command surface is AUTHORED, in the vessel's `Controls` scope: the
            // intents it binds name exactly the ports this vessel accepts.
            // `sync_command_surface` seeds them from the `ControlBinding`, so the
            // vocabulary is never decided here — it used to be the literal
            // `&["throttle", "steer", "brake"]`, which meant the engine decided what
            // could command a vehicle by knowing what kind of vehicle it was.
            //
            // `CommandInputs` is seeded EMPTY: the command backend is strict, so a
            // vessel whose `Controls` scope is absent accepts nothing and every write is
            // refused. That is how you author a wreck or an un-crewed chassis — by
            // composition, not a check.
            //
            // Only `ActuatorPorts` is stamped here. The `CommandInputs` surface is
            // stamped beside the `ControlBinding` (lunco-usd-bevy, the `Controls`
            // branch) — ONE site, because `try_insert` OVERWRITES: stamping a fresh
            // empty surface from two different systems would let a live re-run of
            // either one wipe the keys `sync_command_surface` had already seeded.
            //
            // `ActuatorPorts` is a different thing and is NOT the command surface: it
            // maps ACTUATOR names to their `Port` entities, built above from the
            // canonical set plus the vessel prim's authored `outputs:` attributes. The
            // two stay separate components on purpose — both carry a `"brake"`, and
            // they are not the same value (analog command vs discretized gate).
            commands
                .entity(entity)
                .try_insert(lunco_core::ActuatorPorts::new(port_map));

        }

        // 1b. Mission behaviour: a BT.CPP v4 XML tree, carried by a `LunCoProgram`
        // child of this prim — the vessel OWNS the tree, so the tree is read from
        // here, its owner. Inline source wins over a file: an author editing a tree in
        // place means it. The tree's spatial leaves reference WAYPOINT PRIMS by path;
        // `resolve_behavior_targets` binds those, and `lunco_autopilot::usd_tree` bakes
        // their live positions into the compiled tree.
        //
        // A `.xml` is the one program with a role of its own: a declarative tree is
        // not a script, it is compiled and ticked by the behaviour engine. Extension
        // picks the engine, exactly as it does for `.mo` and `.rhai`.
        for child in reader.children(&sdf_path) {
            if reader.type_name(&child).as_deref() != Some("LunCoProgram") {
                continue;
            }
            if let Some(xml) = reader
                .scalar::<String>(&child, "info:sourceCode")
                .filter(|s| s.trim_start().starts_with('<'))
            {
                commands
                    .entity(entity)
                    .try_insert(lunco_autopilot::usd_tree::BehaviorXml(xml));
            } else if let Some(path) = reader
                .asset(&child, "info:sourceAsset")
                .filter(|s| s.ends_with(".xml"))
            {
                commands
                    .entity(entity)
                    .try_insert(lunco_autopilot::usd_tree::BehaviorXmlPath(path));
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
        // The compliance is the joint's own `PhysicsDriveAPI` — a rigid gear would fight
        // the terrain and chatter, so it is enforced as a strong spring.
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
            if let (Some((body_a, frame)), Some((body_b, _))) =
                (geared(&hinges.0), geared(&hinges.1))
            {
                let read_f = |name: &str, dflt: f64| reader.real(&sdf_path, name).unwrap_or(dflt);
                // The hinges turn about a shared axis; take it from the first.
                let axis = SdfPath::new(hinges.0.as_deref().unwrap_or_default())
                    .ok()
                    .and_then(|h| reader.text(&h, "physics:axis"))
                    .as_deref()
                    .map(|a| match a {
                        "Y" => DVec3::Y,
                        "Z" => DVec3::Z,
                        _ => DVec3::X,
                    })
                    .unwrap_or(DVec3::X);
                let ratio = read_f("physxGearJoint:gearRatio", -1.0);
                info!(
                    "Gear joint {} couples {} / {} (ratio {})",
                    prim_path.path, body_a, body_b, ratio,
                );
                commands.entity(entity).try_insert(PendingDifferential {
                    chassis: frame,
                    rocker_a: body_a,
                    rocker_b: body_b,
                    axis,
                    ratio,
                    rest_offset: read_f("drive:angular:physics:targetPosition", 0.0),
                    stiffness: read_f("drive:angular:physics:stiffness", 200_000.0),
                    damping: read_f("drive:angular:physics:damping", 20_000.0),
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
                debug!("Wheel {} has no mesh yet, skipping until next frame", prim_path.path);
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
            // drivetrain/tire/inertia number plus suspension, resolved via the
            // canonical two-step path (attachment prim, else flat off the wheel
            // prim). Strict — all missing required attrs are collected and the
            // wheel refuses to spawn; the authored defaults live in
            // components/mobility/wheel.usda, which every wheel composes.
            // Read BEFORE spawning the port entities so an invalid wheel
            // synthesizes nothing.
            let attachment_susp =
                wheel_params::attachment_suspension_path(prim_path, wheel_attachment_targets);
            let powertrain = powertrain::find_for_wheel(reader, &sdf_path);
            let params = match WheelParams::read(
                reader,
                &sdf_path,
                attachment_susp.as_ref(),
                powertrain.as_ref(),
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

            // Create the actuator-side ports for drive and steering. Owned by the wheel via
            // `ChildOf` so the single recursive scene-clear reclaims them with the
            // wheel — synthesized backing entities are never left detached at the root
            // (the general lifecycle contract; see `setup_physical_wheel`'s joint).
            let p_drive = commands.spawn((Port::default(), Name::new("Port_Drive"), ChildOf(entity))).id();
            let p_steer = commands.spawn((Port::default(), Name::new("Port_Steer"), ChildOf(entity))).id();

            // `index` is LunCo-specific: NVIDIA's `physxVehicleWheelAttachment:index`
            // lives on the WheelAttachment prim, which our flat composition doesn't
            // have (the wheel prim composes both wheel + suspension directly). So
            // we author the wheel's logical index under a LunCo namespace rather
            // than squatting a non-existent `physxVehicleWheel:index` attr.
            let index = reader.scalar::<i32>(&sdf_path, "lunco:wheel:index").unwrap_or(0);

            // Optional per-wheel actuator binding, as a USD CONNECTION:
            //   float inputs:drive.connect = </Rover.outputs:drive_left>
            // This extracts the rover's wiring topology out of `try_wire_wheel`'s
            // hardcoded index parity and into USD, enabling per-wheel drive and
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
            let drive_port_name = connected_port("inputs:drive");
            let steer_port_name = connected_port("inputs:steer");

            // Mark for wiring — the try_wire_wheel system will connect ports once FSW exists
            commands.entity(entity).try_insert(PendingWheelWiring {
                index,
                p_drive,
                p_steer,
                drive_port_name,
                steer_port_name,
            });

            // Standard-USD discriminator: an authored `PhysicsRevoluteJoint`
            // pointing at this wheel via `physics:body1` ⇒ joint-based.
            let key = (prim_path.stage_handle.clone(), prim_path.path.clone());
            // Front wheels (index < 2) of an Ackermann rover steer. Gate on the
            // rover's drive type — a skid rover keeps all wheels fixed (it steers
            // by skidding), so only wire the steering port when the wheel's VEHICLE
            // carries `PhysxVehicleAckermannSteeringAPI` (Omniverse steering
            // schema). Same for both wheel kinds: each attaches a shared
            // `SteeringActuator` (joint or raycast), so the model is identical.
            let steering_vehicle = steering_vehicle_of(reader, &prim_path.path);
            let steer_for_wheel =
                if index < 2 && steering_vehicle.is_some() { Some(p_steer) } else { None };
            // The steer lock belongs to the VEHICLE's steering system, not to a
            // wheel: PhysX deprecated the per-wheel `physxVehicleWheel:maxSteerAngle`
            // in favour of the steering APIs, and a skid rover's wheels have no such
            // angle at all. RADIANS, as everywhere in PhysX (only the Kit authoring
            // wizard's UI field is in degrees).
            let max_steer_angle = match &steering_vehicle {
                Some(vehicle) => {
                    let Some(rad) = reader
                        .real(vehicle, "physxVehicleAckermannSteering:maxSteerAngle")
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
            if joint_targets.contains_key(&key) {
                setup_physical_wheel(
                    &mut commands, entity, prim_path, &existing_tf,
                    maybe_mesh, maybe_mat, maybe_shader_mat, maybe_child_of,
                    &params, p_drive,
                    steer_for_wheel, max_steer_angle,
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
                    &mut commands, entity, prim_path, &existing_tf,
                    maybe_mesh, maybe_mat, maybe_shader_mat, maybe_child_of,
                    &params, &suspension,
                    p_drive, p_steer, steer_for_wheel, max_steer_angle,
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
) -> Vec<lunco_mobility::kernels::MixEntry> {
    let mut entries: Vec<lunco_mobility::kernels::MixEntry> = reader
        .children(scope)
        .into_iter()
        .filter_map(|term| {
            let port = term.name()?.to_string();
            let forward = reader.real(&term, "lunco:factor:throttle");
            let steer = reader.real(&term, "lunco:factor:steer");
            let brake = reader.real(&term, "lunco:factor:brake");
            if forward.is_none() && steer.is_none() && brake.is_none() {
                warn!(
                    "DriveMix term {} declares no `lunco:factor:<throttle|steer|brake>`; \
                     skipped — the port would never be driven",
                    term.as_str()
                );
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
    entries
}

/// Derive the vehicle-root `DriveMix` from its authored schema — the kernel is
/// selected by the differential / steering schema the asset declares (Omniverse
/// PhysX Vehicle names), an authored `DriveMix` child scope, or a scripted
/// `lunco:driveKernel` hook. Shared by the spawn path and the live wheel-param
/// resync so an edited kernel re-derives identically.
fn derive_drive_mix(
    reader: &lunco_usd_bevy::StageView<'_>,
    sdf_path: &SdfPath,
    prim_path_str: &str,
) -> Option<DriveMix> {
    if let Some(hook_id) = reader.text(sdf_path, "lunco:driveKernel") {
        // Scripted (rhai) kernel: the hook computes the per-port outputs, so it
        // takes precedence over the built-in skid/linear schemas. `apply_drive_mix`
        // falls back to the `lunco_hooks` hook named by `DriveMix.kernel`.
        info!("Scripted drive kernel '{}' for {}", hook_id, prim_path_str);
        Some(DriveMix::scripted(&hook_id))
    } else if let Some(scope) = reader
        .children(sdf_path)
        .into_iter()
        .find(|c| c.name() == Some("DriveMix"))
    {
        let entries = read_drive_mix_scope(reader, &scope);
        info!(
            "Authored linear DriveMix for {} ({} ports)",
            prim_path_str,
            entries.len()
        );
        Some(DriveMix::linear(entries))
    } else if reader.has_api_schema(sdf_path, "PhysxVehicleTankDifferentialAPI") {
        info!("Tank differential (skid kernel) for {}", prim_path_str);
        Some(DriveMix::skid("drive_left", "drive_right"))
    } else if reader.has_api_schema(sdf_path, "PhysxVehicleAckermannSteeringAPI") {
        // Ackermann: non-differential drive (both sides get throttle) + a
        // dedicated steering port; the front wheels castor (see steering gate).
        info!("Ackermann steering (linear kernel) for {}", prim_path_str);
        Some(DriveMix::linear(vec![
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
        ]))
    } else {
        None
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
            Name::new(format!("{}_visual", prim_path.path.split('/').next_back().unwrap_or("wheel"))),
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
    let mut ray_caster = RayCaster::new(DVec3::ZERO, Dir3::NEG_Y);
    // Mask out the TRIGGER layer so suspension rays ignore trigger-zone sensors
    // (else the wheels ride up on an invisible waypoint sphere). Excludes the
    // rover's own chassis by entity as before.
    let mut filter = avian3d::prelude::SpatialQueryFilter::from_mask(
        avian3d::prelude::LayerMask(!lunco_core::TRIGGER_COLLISION_LAYER),
    );
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
    commands.entity(entity)
        .remove::<Collider>()
        .remove::<RigidBody>()
        .remove::<Mass>();
}

/// Sets up a wheel as a full rigid body bound to the chassis by a revolute
/// joint, mirroring the standard `PhysicsRevoluteJoint` authored in USD.
///
/// The joint is spawned **synchronously** from the authored USD attributes
/// (`physics:axis`, `physics:localPos0/1`) alongside the wheel's rigid-body
/// init; drive authority comes from the engine `peakTorque`. Doing it lazily — letting
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
    params: &WheelParams,
    p_drive: Entity,
    steer: Option<Entity>,
    max_steer_angle: f64,
) {
    info!("Setting up PHYSICAL wheel {}", prim_path.path);
    let radius = params.radius as f32;

    // `params.peak_torque` (N·m at full throttle) is the engine's `peakTorque`,
    // the SAME drive authority the raycast wheel uses — NOT the joint's
    // `drive:angular:physics:maxForce`. That joint attribute is a PhysX
    // joint-drive *saturation* limit (authored at 12000 in the demo scenes);
    // feeding it straight into the motor made the rover apply ~30× its lunar
    // weight in traction at full throttle and wheelie/launch on every forward
    // input. Using the engine peakTorque keeps joint and raycast rovers
    // consistent. See `project_physical_rover_suspension`.

    // The wheel body keeps **identity rotation**. The cylinder's
    // visible axis (from `UsdGeomCylinder.axis`) lives on a visual
    // child + the collider's compound-local rotation, so the wheel's
    // local frame stays aligned with the chassis — required for the
    // authored revolute joint's `physics:axis` token to be unambiguous.
    let wheel_axis_rot = existing_tf.rotation;
    let wheel_tf = Transform {
        translation: existing_tf.translation,
        rotation: Quat::IDENTITY,
        scale: existing_tf.scale,
    };

    let cyl = Collider::cylinder(radius as f64, (radius * 0.5) as f64);
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
            Transform::from_rotation(wheel_axis_rot),
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

    commands.entity(entity).remove::<WheelRaycast>()
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
        // Heavier wheels (100 kg default vs the raycast 25) damp the joint↔solver
        // impulse echo. Set via density so mass AND angular inertia stay consistent
        // (see the `wheel_density` note above) — a forced `Mass` desynced them and
        // the rover sank through the terrain.
        avian3d::prelude::ColliderDensity(wheel_density),
        // The drive is an axle TORQUE on the wheel (see MotorActuator); wheel↔ground
        // friction propels the rover. μ is a COMPROMISE: high μ gives traction +
        // Ackermann cornering grip, but also high LATERAL grip that resists a skid
        // rover's sideways scrub (skid steering needs the wheels to slide). μ=0.9
        // lets the skid differential actually yaw the body while still moving + (with
        // AWD) cornering the Ackermann. `AngularDamping(0.3)` = wheel-bearing drag.
        Friction::new(0.9),
        LinearDamping(0.1),
        AngularDamping(0.3),
        // Continuous collision detection: a thin, fast-falling wheel cylinder can
        // pass THROUGH the one-sided terrain heightfield in a single step (and once
        // below a one-sided surface, no contact ever pushes it back — it falls
        // forever). CCD sweeps the wheel's motion against the collider so it can
        // never tunnel, even across a one-frame collider-warmup gap. This is what
        // lets the tunnel-rescue safety net be deleted — the wheel physically
        // cannot end up below the terrain.
        avian3d::prelude::SweptCcd::default(),
        wheel_tf,
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
    let axle = (existing_tf.rotation * Vec3::Y).as_dvec3();

    // Hinge the wheel to the chassis at its authored offset.
    //
    // An articulated chassis→prismatic(spring)→hub→revolute→wheel *suspension*
    // was prototyped and rejected: avian's joint SpringDamper is fragile bearing
    // the chassis weight — it rings the pitch/roll mode down for 15-20 s after
    // the scene's 5 m spawn drop, can't be damped harder (high damping_ratio
    // diverges), and its effective tuning shifts with substep count. The fix for
    // *vertical* travel is therefore the rigid axle below + `SubstepCount(12)` at
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
    // axle + `SubstepCount(12)`. See `project_physical_rover_suspension`.)

    // Velocity-controlled axle drive — THE one drive-motor definition lives in
    // `WheelParams::drive_motor` (raw torque sat in avian's low-slip friction
    // dead-zone; commanding spin rate is stable and self-limits top speed at
    // traction; the high stall torque is what lets a skid turn enforce its
    // left/right speed split). The no-load speed it targets is
    // `physxVehicleEngine:maxRotationSpeed` — the SAME attribute the raycast
    // wheel's torque–speed rolloff uses, so both kinds top out at `ω_max · r`.
    // Further USD-tunable via `lunco:wheel:driveDamping` / `:stallTorqueGain`.
    let drive_motor = params.drive_motor();

    // Joint construction lives in `lunco-usd-avian` (the single home for all
    // Avian joint-building); we add the mobility/hardware actuators on top.
    let mut joint_cmd = commands.spawn((
        // `joint_bundle` carries the `JointCollisionDisabled` policy — the marker
        // MUST share the joint's bundle so the `JointGraphEdge` is born
        // collision-disabled and the pair never reaches the narrow phase. Never
        // insert the marker separately here; see `lunco_usd_avian::joint_bundle`.
        lunco_usd_avian::joint_bundle(lunco_usd_avian::wheel_revolute_joint(
            chassis,
            entity,
            mount_local,
            axle,
            drive_motor,
        )),
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
        // All-wheel drive. The throttle port already carries the skid rover's
        // per-side differential (drive_left/drive_right), so a single mapping here
        // yaws the skid body; on the Ackermann rover all wheels share one throttle
        // and the front frame-steer does the turning.
        MotorActuator {
            port_entity: p_drive,
            max_omega: params.max_rotation_speed,
            drive_sign: -1.0,
        },
        Name::new(format!("PhysicalWheelJoint_{}", prim_path.path)),
    ));
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

    // The wheel↔chassis link is the wheel's `ChildOf(chassis)` — set by USD projection
    // and read back here as `chassis = child_of.parent()`. It is the ONE canonical link:
    // transform propagation, despawn cascade, AND parent lookup (the proxy systems below
    // read `ChildOf` to find the chassis). No separate ownership relationship is needed.
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
        (With<DriveMix>, Without<PhysicalWheel>),
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
        (&RigidBody, &Position, &Rotation, Option<&lunco_core::ReplicatedChassisMotion>),
        With<DriveMix>,
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
        let Ok((body, pos, rot, motion)) = q_chassis.get(child_of.parent()) else { continue };
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
        let (hub_pos, _) = wheel_hub_pose(pos.0, rot.0, wheel.mount_local.as_dvec3(), DQuat::IDENTITY);
        let hub_vel = wheel_hub_velocity(vlin, vang, hub_pos, pos.0);
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
struct UsdSimProcessed;

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
            if let Some(recipe) =
                stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone())
            {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else { continue };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue };
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
/// USD authority first (G4 — topology authored, not hardcoded):
/// - `inputs:drive.connect = </Rover.outputs:<name>>` on the wheel → wire its
///   drive to that FSW port. The connection is PCP-resolved and path-translated
///   through reference arcs, so a referenced wheel binds to its own instance's port.
/// - `inputs:steer.connect` likewise wires the wheel's steer.
///
/// Default when unauthored (the canonical skid/Ackermann layout):
/// - **Even index** → `drive_left`, **odd index** → `drive_right`.
/// - **Index < 2** (front) → `steering` (only meaningful for Ackermann).
///
/// A named port that is absent from the rover's `ActuatorPorts` warns and is skipped —
/// declare custom ports as `outputs:<name>` attributes on the rover root.
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
            // Drive: authored binding wins, else even/odd index parity.
            let drive_port_name = pending.drive_port_name.clone().unwrap_or_else(|| {
                if pending.index % 2 == 0 { "drive_left" } else { "drive_right" }.to_string()
            });
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
                    Name::new(format!("Wire_Drive_{}", drive_port_name)),
                    ChildOf(ent),
                ));
                debug!("Wired wheel {} drive to FSW port {}", prim_path.path, drive_port_name);
            } else {
                warn!(
                    "Wheel {} drive port '{}' not in rover ActuatorPorts; skipping",
                    prim_path.path, drive_port_name
                );
            }

            // Steer: authored binding wins, else front wheels (index < 2) steer.
            // An unauthored rear/skid wheel has no steer port — leave it unwired.
            let steer_port_name = pending
                .steer_port_name
                .clone()
                .or_else(|| (pending.index < 2).then(|| "steering".to_string()));
            if let Some(name) = steer_port_name {
                if let Some(s_port) = actuators.get(&name) {
                    commands.spawn((
                        SimConnection {
                            start_element: s_port,
                            start_connector: PORT_NAME.to_string(),
                            end_element: pending.p_steer,
                            end_connector: PORT_NAME.to_string(),
                            ..Default::default()
                        },
                        Name::new(format!("Wire_Steer_{}", name)),
                        ChildOf(ent),
                    ));
                    info!("Wired wheel {} steering to FSW port {}", prim_path.path, name);
                } else if pending.steer_port_name.is_some() {
                    // Only warn when the author asked for a port that's missing;
                    // a defaulted front wheel on a skid rover legitimately has none.
                    warn!(
                        "Wheel {} steer port '{}' not in rover ActuatorPorts; skipping",
                        prim_path.path, name
                    );
                }
            }
            commands.entity(ent).remove::<PendingWheelWiring>();
        } else {
            debug!("Wheel {} FSW not found yet, retrying next frame", prim_path.path);
        }
    }
}

/// Bind the waypoint prims a vessel's behaviour tree references (`<Action ID="drive_to"
/// target="/World/Behaviors/RoverPatrol/wp0"/>`) to their live entities, so
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
    q_trees: Query<(Entity, &lunco_autopilot::usd_tree::BehaviorXml, &UsdPrimPath)>,
    q_prims: Query<(Entity, &UsdPrimPath)>,
    q_new_prims: Query<(), Added<UsdPrimPath>>,
    q_changed_xml: Query<(), Changed<lunco_autopilot::usd_tree::BehaviorXml>>,
    q_new_ids: Query<(), Added<lunco_core::GlobalEntityId>>,
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
        let targets = lunco_autopilot::usd_tree::target_paths(&xml.0);
        debug!("[resolve_behavior_targets] vessel {:?} ({}) has {} targets: {:?}", vessel, vessel_path.path, targets.len(), targets);
        for path in targets {
            let found = q_prims.iter().find(|(e, p)| {
                let match_path = p.path == path;
                let match_stage = p.stage_handle == vessel_path.stage_handle;
                let is_behavior = path.contains("/Behaviors/");
                let inst = instance_key(*e, &q_provenance, &q_gid, &q_instance_root);
                let match_inst = is_behavior || inst == vessel_instance;
                if match_path {
                    debug!("[resolve_behavior_targets] candidate {:?} ({}) match_stage={} match_inst={} (is_behavior={})", e, p.path, match_stage, match_inst, is_behavior);
                }
                match_path && match_stage && match_inst
            });
            if let Some((e, _)) = found {
                info!("[resolve_behavior_targets] resolved target {} -> entity {:?}", path, e);
                bindings.0.insert(path, e);
            } else {
                warn!("[resolve_behavior_targets] failed to resolve target {} for vessel {:?}", path, vessel);
            }
        }
        commands.entity(vessel).try_insert(bindings);
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
            axis: pending.axis,
            ratio: pending.ratio,
            rest_offset: pending.rest_offset,
            stiffness: pending.stiffness,
            damping: pending.damping,
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
/// `lunco-sandbox`). While `true`, [`activate_dynamic_bodies`] holds bodies
/// kinematic so a rover spawned over not-yet-collidable terrain doesn't
/// free-fall through the surface during the multi-second collider bake.
#[derive(Resource, Default)]
pub struct GroundColliderPending(pub bool);

fn activate_dynamic_bodies(
    mut commands: Commands,
    ground_pending: Res<GroundColliderPending>,
    q_kinematic: Query<(Entity, &UsdPrimPath), With<ShouldBeDynamic>>,
    q_pending_joints: Query<&UsdPrimPath, With<lunco_usd_avian::PendingUsdJoint>>,
    q_pending_diffs: Query<&UsdPrimPath, With<PendingDifferential>>,
    // Physical wheels only: they get a one-time drop-onto-terrain settle. Free
    // Dynamic bodies (balloons, etc.) must NOT be pinned to the ground.
    q_wheel: Query<(), With<PhysicalWheel>>,
) {
    // Ground still building → gravity would win the race; keep everything
    // kinematic until the terrain collider lands.
    if ground_pending.0 {
        return;
    }
    for (entity, path) in q_kinematic.iter() {
        let has_pending_joint = q_pending_joints.iter().any(|j_path| j_path.stage_handle == path.stage_handle);
        let has_pending_diff = q_pending_diffs.iter().any(|d_path| d_path.stage_handle == path.stage_handle);
        if !has_pending_joint && !has_pending_diff {
            // Despawn-safe: scene-load churn / doc-backed reload can despawn a
            // ShouldBeDynamic entity between this queue and `apply_deferred`; a plain
            // `insert` then panics on the invalid entity. `try_insert`/`try_remove`
            // no-op at apply time if the entity is gone (a `get_entity` guard here
            // would not help — it only proves validity at queue time, not apply).
            commands.entity(entity).try_insert(RigidBody::Dynamic);
            commands.entity(entity).try_remove::<ShouldBeDynamic>();
            // A physical wheel drops from the authored pose (chassis at the surface,
            // wheels hanging below it) — which starts the wheels embedded in the
            // one-sided terrain heightfield. Flag the assembly for a one-time
            // ground-settle that lifts it so the wheels clear the surface.
            if q_wheel.contains(entity) {
                commands.entity(entity).try_insert(lunco_core::NeedsGroundSettle);
            }
            debug!("Activated RigidBody::Dynamic for stage: {:?}", path.stage_handle);
        }
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
                DriveMix::default(),
            ))
            .id();
        let visual = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn((
            PhysicalWheel {
                visual_entity: Some(visual),
                wheel_radius: 0.5,
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
        let rot = app.world().entity(visual).get::<Transform>().unwrap().rotation;
        (spin, rot)
    }

    #[test]
    fn kinematic_proxy_spins_and_rotates_visual() {
        // v_long = 2 m/s, r = 0.5 → ω = 4 rad/s; one 0.1 s tick ⇒ |Δθ| = 0.4.
        let (spin, rot) = run_once(RigidBody::Kinematic);
        // spin_angle is wrapped to [0, TAU); measure the minimal circular distance.
        let wrapped = spin.rem_euclid(std::f32::consts::TAU);
        let circ = wrapped.min(std::f32::consts::TAU - wrapped);
        assert!((circ - 0.4).abs() < 1e-3, "expected |spin|≈0.4, got {spin} (circ {circ})");
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
                DriveMix::default(),
            ))
            .id();
        let visual = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn((
            PhysicalWheel {
                visual_entity: Some(visual),
                wheel_radius: 0.5,
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
        let rot = app.world().entity(visual).get::<Transform>().unwrap().rotation;
        assert_eq!(spin, 0.0, "replicated wheel must be a no-op, got spin {spin}");
        assert_eq!(rot, Quat::IDENTITY, "replicated wheel visual must be untouched");
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
                lunco_core::ReplicatedChassisMotion { lin: DVec3::ZERO, ang },
                DriveMix::default(),
            ))
            .id();
        let visual = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn((
            PhysicalWheel {
                visual_entity: Some(visual),
                wheel_radius: 0.5,
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
        let far = run_spin_with(ang, mount, /* rebased 1 km along the sensitive axis */ mount - Vec3::new(1000.0, 0.0, 0.0));

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
        assert!((circ - 0.2).abs() < 1e-3, "expected |Δθ|≈0.2, got {near} (circ {circ})");
    }

    #[test]
    fn net_override_vocabulary() {
        // Default / server / predictable: replicated, predictable (no override markers).
        assert_eq!(super::net_override_markers(None, None), (false, false));
        assert_eq!(super::net_override_markers(None, Some("server")), (false, false));
        assert_eq!(super::net_override_markers(None, Some("predictable")), (false, false));
        // Opt-out: excluded, not opaque.
        assert_eq!(super::net_override_markers(Some(false), None), (true, false));
        assert_eq!(super::net_override_markers(None, Some("local")), (true, false));
        // Opaque: replicated but never predicted.
        assert_eq!(super::net_override_markers(None, Some("opaque")), (false, true));
        // Explicit include is not an exclusion.
        assert_eq!(super::net_override_markers(Some(true), None), (false, false));
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
        let s = super::proxy_wheel_steer(true, wheelbase, DVec3::new(0.0, 0.0, -2.0), DVec3::new(0.0, yaw, 0.0));
        let expected = (wheelbase * yaw / 2.0_f64).atan();
        assert!((s - expected).abs() < 1e-12, "δ={s}, expected {expected}");
        // Vertical (y) velocity must not leak into the planar speed used for the ratio.
        let s_with_vy = super::proxy_wheel_steer(true, wheelbase, DVec3::new(0.0, 9.0, -2.0), DVec3::new(0.0, yaw, 0.0));
        assert!((s_with_vy - expected).abs() < 1e-12, "vy leaked: δ={s_with_vy}");
    }

    #[test]
    fn front_wheel_steer_is_clamped() {
        // A huge yaw/speed ratio saturates at ±0.6 rad, and sign tracks yaw.
        let hi = super::proxy_wheel_steer(true, 100.0, DVec3::new(0.0, 0.0, -1.0), DVec3::new(0.0, 5.0, 0.0));
        assert!((hi - 0.6).abs() < 1e-12, "δ={hi}");
        let lo = super::proxy_wheel_steer(true, 100.0, DVec3::new(0.0, 0.0, -1.0), DVec3::new(0.0, -5.0, 0.0));
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
        assert!((p - expected).length() < 1e-9, "p={p:?}, expected {expected:?}");
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
        assert!((p - p0).length() < 1e-12, "steer moved the hub: {p:?} vs {p0:?}");
    }
}

