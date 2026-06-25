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
//! | `PhysxVehicleContextAPI` | `FlightSoftware`, `RoverVessel`, `Vessel` | Rover root entity |
//! | `PhysxVehicleDriveSkidAPI` | `DifferentialDrive` | Skid steering |
//! | `PhysxVehicleDrive4WAPI` | `AckermannSteer` | Ackermann steering |
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
pub use lunco_usd_bevy::{UsdPreviewOnly, UsdPrimPath, UsdStageAsset};
use lunco_usd_bevy::{has_api_schema, read_rel_target};
use openusd::sdf::{Path as SdfPath, AbstractData, Value};
use lunco_mobility::{WheelRaycast, DifferentialDrive, AckermannSteer};
use lunco_fsw::FlightSoftware;
use lunco_core::architecture::{DigitalPort, PhysicalPort, Wire};
use lunco_hardware::{MotorActuator, SteeringActuator};
use lunco_core::RoverVessel;
use lunco_avatar::{FreeFlightCamera, OrbitCamera, SpringArmCamera, AdaptiveNearPlane, ProvisionalAvatarCamera};
use lunco_core::Avatar;
use lunco_core::architecture::IntentAnalogState;
use leafwing_input_manager::prelude::ActionState;
use lunco_controller::get_avatar_input_map;
use std::collections::HashMap;

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
pub struct UsdSimPlugin;

impl Plugin for UsdSimPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<WheelOf>()
           .register_type::<RoverWheels>()
           .register_type::<ArticulationRoot>()
           .register_type::<PhysicalWheel>()
           // Client-only: reconstruct a remote rover's wheels from its chassis
           // (kinematic followers — wheels are no longer replicated), then re-derive
           // the cosmetic visual roll. Chained so the visual spin layers on the
           // freshly-placed body. Same `tw.is_running()` gate as raycast wheels.
           .add_systems(FixedUpdate, (reconstruct_proxy_wheels, animate_proxy_physical_wheels)
               .chain()
               .run_if(|tw: Res<lunco_core::TimeWarpState>| tw.is_running()))
           .add_observer(on_add_usd_sim_prim)
           // `try_wire_wheel` runs in PreUpdate so that Wire entities exist
           // before `wire_system` (Update) propagates values through them.
           .add_systems(PreUpdate, try_wire_wheel)
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
           .add_systems(Update, process_usd_sim_prims
               .run_if(any_unprocessed_usd_sim)
               .after(lunco_usd_bevy::sync_usd_visuals));
        // USD → cosim wiring (`lunco:modelicaModel`, `lunco:scriptModel`,
        // `lunco:simWires`) — see `cosim.rs`.
        cosim::install(app);
    }
}

pub mod cosim;
pub use cosim::{CosimStatusProvider, UsdSourcedCosim};

/// USD → [`ShaderMaterial`](lunco_materials::ShaderMaterial) authoring,
/// deterministically ordered so it can never race a downstream consumer.
pub mod shader;

/// Logical link from a joint-based wheel rigid body up to its rover.
///
/// Decouples ownership ("this wheel belongs to that rover") from the
/// Bevy parent-child hierarchy, which is reserved for transform
/// propagation. Used for selection ("click on a wheel, focus the
/// rover"), camera follow, and to find the matching `RoverWheels`
/// list when teleporting / despawning the rover.
///
/// Set in `setup_physical_wheel`; mirrors the standard OpenUSD
/// `PhysicsArticulationRootAPI` link declared on the rover Xform —
/// when Avian gains articulation support, this component becomes a
/// runtime reflection of the authored articulation graph.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct WheelOf(pub Entity);

/// On a rover root: the wheel rigid bodies the rover owns.
///
/// Populated alongside [`WheelOf`] for the inverse lookup — iterating
/// a single rover's wheels without scanning every wheel in the world.
#[derive(Component, Debug, Default, Clone, Reflect)]
#[reflect(Component)]
pub struct RoverWheels(pub Vec<Entity>);

/// Marker for rovers authored with `PhysicsArticulationRootAPI`.
///
/// Standard OpenUSD schema declaring "this Xform plus everything joint-
/// connected below it is **one** articulated multibody, not loose
/// rigid bodies that happen to be linked." Avian 0.6's XPBD-impulse
/// solver doesn't natively articulate; we honour the declaration by
/// reparenting wheels to top-level and tracking the link via
/// `WheelOf`/`RoverWheels`. The day Avian gains articulation, this
/// marker becomes the trigger for the engine-native path.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct ArticulationRoot;

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
}

/// Process USD prims for sim mapping AFTER their assets are loaded.
///
/// This is the core system that maps USD schemas to LunCoSim components. It runs in the
/// `Update` schedule **after** `sync_usd_visuals` to ensure meshes and transforms exist.
///
/// # What It Does
///
/// 1. **Detects `PhysxVehicleContextAPI`** → Creates `FlightSoftware` with 4 digital ports
///    (`drive_left`, `drive_right`, `steering`, `brake`), plus `RoverVessel` and `Vessel`.
/// 2. **Detects `PhysxVehicleDriveSkidAPI`** → Creates `DifferentialDrive` with port names.
/// 3. **Detects `PhysxVehicleDrive4WAPI`** → Creates `AckermannSteer` with port names.
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

fn process_usd_sim_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath, Option<&Transform>, Option<&Mesh3d>, Option<&MeshMaterial3d<StandardMaterial>>, Option<&MeshMaterial3d<lunco_materials::ShaderMaterial>>, Option<&ChildOf>), Without<UsdSimProcessed>>,
    q_all_prims: Query<&UsdPrimPath>,
    q_grids: Query<Entity, With<Grid>>,
    q_existing_floating_origins: Query<Entity, With<FloatingOrigin>>,
    q_provisional_cameras: Query<Entity, With<ProvisionalAvatarCamera>>,
    q_child_of: Query<&ChildOf>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    stages: Res<Assets<UsdStageAsset>>,
    // The active-scene sun: the avatar camera's exposure is read from the SAME
    // resource the sun illuminance comes from, so they can't drift (a dimmed
    // sun under a bright-tuned camera blacked the viewport). `Option` so the
    // loader still works in a stripped app without `EnvironmentPlugin`.
    active_sun: Option<Res<lunco_environment::LunarSun>>,
) {
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

    // Scan the **stage data** rather than spawned entities. Joint and
    // wheel prims may be spawned on different frames; reading from the
    // SDF data directly avoids the race where a wheel is processed
    // before its joint sibling has an entity in the ECS.
    let mut seen_stages: std::collections::HashSet<Handle<UsdStageAsset>> = Default::default();
    for prim_path in q_all_prims.iter() {
        if !seen_stages.insert(prim_path.stage_handle.clone()) { continue; }
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        // Borrow — `stage.reader` is `Arc<TextReader>`; `(*…).clone()` deep-copied
        // the whole stage `HashMap<String, sdf::Value>` once per stage. The
        // reads below (`iter`/`get`/`read_rel_target`) only need `&self`.
        let reader = &*stage.reader;
        for (path, _spec) in reader.iter() {
            let Ok(val) = reader.get(path, "typeName") else { continue; };
            let type_name = match &*val {
                Value::Token(t) => Some(t.as_str().to_string()),
                Value::String(s) => Some(s.clone()),
                _ => None,
            };
            if type_name.as_deref() == Some("PhysicsRevoluteJoint") {
                if let Some(body1) = read_rel_target(reader, path, "physics:body1") {
                    debug!("USD joint dispatch: {} → wheel {}", path.as_str(), body1);
                    joint_targets.insert(
                        (prim_path.stage_handle.clone(), body1),
                        path.as_str().to_string(),
                    );
                }
                if let Some(body0) = read_rel_target(reader, path, "physics:body0") {
                    articulation_roots.insert((prim_path.stage_handle.clone(), body0));
                }
            }
        }
    }

    // --- Pass 2: Process all prims ---
    for (entity, prim_path, maybe_tf, maybe_mesh, maybe_mat, maybe_shader_mat, maybe_child_of) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        // Bail when this prim lives under a `UsdPreviewOnly` scene
        // root. Preview viewports render geometry only — they must
        // not spawn Avatar Camera3d, FlightSoftware, or wheel raycasts
        // into the main world. Walking up the `ChildOf` chain catches
        // every prim because `sync_usd_visuals` parents each spawned
        // prim entity to its USD-parent entity, which itself chains
        // back to the workbench-owned scene_root.
        if is_preview_only(entity, &q_child_of, &q_preview_only) {
            commands.entity(entity).insert(UsdSimProcessed);
            continue;
        }

        // Borrow, not deep-clone (per prim, every frame until the scene
        // settles — see the pass-1 note above). All reads below are `&self`.
        let reader = &*stage.reader;
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
            || has_api_schema(reader, &sdf_path, "PhysicsArticulationRootAPI")
        {
            commands.entity(entity).insert(lunco_core::ArticulatedVehicle);
        }
        if joint_targets.contains_key(&net_key) {
            commands.entity(entity).insert(lunco_core::ArticulatedLink);
        }
        let net_replicate = reader.prim_attribute_value::<bool>(&sdf_path, "lunco:net:replicate");
        let net_authority = reader.prim_attribute_value::<String>(&sdf_path, "lunco:net:authority");
        let (net_excluded, net_opaque) =
            net_override_markers(net_replicate, net_authority.as_deref());
        if net_excluded {
            commands.entity(entity).insert(lunco_core::NetExcluded);
        }
        if net_opaque {
            commands.entity(entity).insert(lunco_core::NotPredictable);
        }

        // 0. Detect Avatar prim
        if reader.prim_attribute_value::<String>(&sdf_path, "lunco:avatar").is_some() {
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
                    commands.entity(prov).despawn();
                }
            }
            let camera_mode = reader.prim_attribute_value::<String>(&sdf_path, "lunco:cameraMode")
                .unwrap_or_else(|| "freeflight".to_string());
            let yaw = reader.prim_attribute_value::<f32>(&sdf_path, "lunco:cameraYaw")
                .unwrap_or(std::f32::consts::PI * 0.8);
            let pitch = reader.prim_attribute_value::<f32>(&sdf_path, "lunco:cameraPitch")
                .unwrap_or(-0.3);

            // Avatar position from USD transform
            let avatar_tf = Transform {
                translation: existing_tf.translation,
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
            // a lighting/camera bug. Keep workbench cameras SMAA-free; MSAA (the
            // `Camera3d` default) handles geometry-edge AA.
            let ev100 = active_sun
                .as_deref()
                .copied()
                .unwrap_or_default()
                .exposure_ev100;
            let camera_look = move || (bevy::camera::Exposure { ev100 },);

            // Build camera based on mode, then parent to Grid for FloatingOrigin
            match camera_mode.as_str() {
                "freeflight" => {
                    commands.entity(entity).insert((
                        Camera3d::default(),
                        camera_look(),
                        FreeFlightCamera { yaw, pitch, damping: None },
                        AdaptiveNearPlane,
                        avatar_tf,
                        FloatingOrigin,
                        CellCoord::default(),
                        Avatar,
                        IntentAnalogState::default(),
                        ActionState::<lunco_core::UserIntent>::default(),
                        get_avatar_input_map(),
                    ));
                }
                "orbit" => {
                    commands.entity(entity).insert((
                        Camera3d::default(),
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
                        CellCoord::default(),
                        Avatar,
                        IntentAnalogState::default(),
                        ActionState::<lunco_core::UserIntent>::default(),
                        get_avatar_input_map(),
                    ));
                }
                "springarm" => {
                    commands.entity(entity).insert((
                        Camera3d::default(),
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
                        },
                        avian3d::prelude::TranslationInterpolation,
                        avian3d::prelude::RotationInterpolation,
                        AdaptiveNearPlane,
                        avatar_tf,
                        FloatingOrigin,
                        CellCoord::default(),
                        Avatar,
                        IntentAnalogState::default(),
                        ActionState::<lunco_core::UserIntent>::default(),
                        get_avatar_input_map(),
                    ));
                }
                _ => {
                    warn!("Unknown camera mode '{}' for avatar at {}, using freeflight", camera_mode, prim_path.path);
                    commands.entity(entity).insert((
                        Camera3d::default(),
                        camera_look(),
                        FreeFlightCamera { yaw, pitch, damping: None },
                        AdaptiveNearPlane,
                        avatar_tf,
                        FloatingOrigin,
                        CellCoord::default(),
                        Avatar,
                        IntentAnalogState::default(),
                        ActionState::<lunco_core::UserIntent>::default(),
                        get_avatar_input_map(),
                    ));
                }
            }
            // Parent to Grid so FloatingOrigin works
            if let Some(g) = q_grids.iter().next() {
                commands.entity(entity).insert(ChildOf(g));
            }
        }

        // 1. Detect PhysxVehicleContextAPI (The Rover Root)
        // Creates FlightSoftware with 4 digital ports + RoverVessel + Vessel markers
        if has_api_schema(reader, &sdf_path, "PhysxVehicleContextAPI") {
            info!("Intercepted PhysxVehicleContextAPI for {}, initializing Flight Software", prim_path.path);

            let mut port_map = HashMap::new();
            for name in ["drive_left", "drive_right", "steering", "brake"] {
                let port_ent = commands.spawn((
                    DigitalPort::default(),
                    Name::new(format!("Port_{}", name)),
                )).id();
                port_map.insert(name.to_string(), port_ent);
            }

            commands.entity(entity).insert((
                FlightSoftware {
                    port_map,
                    brake_active: false,
                },
                RoverVessel,
                lunco_core::SelectableRoot,
                lunco_core::Vessel,
                RoverWheels::default(),
            ));

            // OpenUSD-standard `PhysicsArticulationRootAPI` declares
            // the rover as an articulated multibody. We mark it for
            // downstream code that needs to know wheels and chassis
            // are kinematically coupled even after the wheels are
            // reparented out of the Bevy hierarchy.
            if has_api_schema(reader, &sdf_path, "PhysicsArticulationRootAPI") {
                commands.entity(entity).insert(ArticulationRoot);
                info!("Detected PhysicsArticulationRootAPI on {}", prim_path.path);
            }

            info!("Successfully initialized FSW for {}", prim_path.path);
        }

        // 2. Detect Drive Schemas (Chassis Logic)
        if has_api_schema(reader, &sdf_path, "PhysxVehicleDriveSkidAPI") {
            info!("Detected Skid Drive for {}", prim_path.path);
            commands.entity(entity).insert(DifferentialDrive {
                left_port: "drive_left".to_string(),
                right_port: "drive_right".to_string(),
            });
        } else if has_api_schema(reader, &sdf_path, "PhysxVehicleDrive4WAPI") {
            info!("Detected Ackermann Drive for {}", prim_path.path);
            commands.entity(entity).insert(AckermannSteer {
                drive_left_port: "drive_left".to_string(),
                drive_right_port: "drive_right".to_string(),
                steer_port: "steering".to_string(),
                max_steer_angle: 0.5,
            });
        }

        // 3. Detect PhysxVehicleWheelAPI (The Wheel Intercept)
        if let Some(radius) = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleWheel:radius") {
            // Skip if mesh doesn't exist yet — sync_usd_visuals may not have processed
            // this prim. We'll retry next frame (not marking UsdSimProcessed).
            if maybe_mesh.is_none() {
                debug!("Wheel {} has no mesh yet, skipping until next frame", prim_path.path);
                continue;
            }

            // Backstop for the USD-authored shader. `apply_usd_shader_materials`
            // (see shader.rs) is ordered `before` this system, and Bevy's
            // automatic sync-point insertion normally flushes its `ShaderMaterial`
            // insert before we run — so in the default configuration this guard
            // never fires. It exists to keep the wheel split correct even if that
            // ordering guarantee is ever weakened (e.g. `auto_insert_apply_deferred`
            // disabled): without the material we'd split the wheel carrying only
            // the default `StandardMaterial` and lose the shader. If a wheel wants
            // a shader but it hasn't landed, retry next frame (don't mark
            // UsdSimProcessed).
            let wants_shader = matches!(
                reader.prim_attribute_value::<String>(&sdf_path, "primvars:materialType").as_deref(),
                Some("shader") | Some("usd_shader")
            ) && reader.prim_attribute_value::<String>(&sdf_path, "primvars:shaderPath").is_some();
            if wants_shader && maybe_shader_mat.is_none() {
                debug!("Wheel {} awaits ShaderMaterial from observer, deferring", prim_path.path);
                continue;
            }
            info!("Intercepted PhysxVehicleWheelAPI for {}", prim_path.path);

            // Create physical ports for drive and steering
            let p_drive = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Drive"))).id();
            let p_steer = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Steer"))).id();

            let index = reader.prim_attribute_value::<i32>(&sdf_path, "physxVehicleWheel:index").unwrap_or(0);

            // Mark for wiring — the try_wire_wheel system will connect ports once FSW exists
            commands.entity(entity).insert(PendingWheelWiring { index, p_drive, p_steer });

            // Suspension parameters — read ONCE here (the single
            // `physxVehicleSuspension:*` reading path) and handed to whichever
            // wheel implementation we build below. The raycast wheel emulates
            // this spring analytically (`suspension_force_mag`); the joint
            // wheel realises it as a real prismatic spring-damper. Same
            // authored data, two constructions.
            let suspension = SuspensionParams {
                rest_length: reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:restLength")
                    .unwrap_or(0.7) as f64,
                spring_k: reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springStiffness")
                    .unwrap_or(15000.0) as f64,
                damping_c: reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springDamping")
                    .unwrap_or(3000.0) as f64,
            };

            // Tire spin dynamics — read from the standard Omniverse PhysX
            // vehicle schema (`PhysxVehicleWheelAPI` / `PhysxVehicleEngineAPI` /
            // `PhysxVehicleTireAPI`) plus standard UsdPhysics `physics:mass`.
            let read_f = |name: &str| -> Option<f64> {
                reader.prim_attribute_value::<f32>(&sdf_path, name)
                    .map(|v| v as f64)
                    .or_else(|| reader.prim_attribute_value::<f64>(&sdf_path, name))
            };
            // Mass (UsdPhysicsMassAPI) → rotational inertia. `physxVehicleWheel:moi`
            // overrides the derived ½·m·r² if explicitly authored.
            let wheel_mass = read_f("physics:mass").unwrap_or(25.0);
            let wheel_moi = read_f("physxVehicleWheel:moi").unwrap_or(0.0);
            // Engine peak torque drives the axle; max rotation speed bounds the
            // free spin. Bearing drag uses the wheel's own `dampingRate` when
            // authored, else is derived so the airborne spin terminates at the
            // engine's max rotation speed (peakTorque / maxRotationSpeed).
            let drive_torque_max = read_f("physxVehicleEngine:peakTorque").unwrap_or(220.0);
            let max_rotation_speed = read_f("physxVehicleEngine:maxRotationSpeed")
                .unwrap_or(600.0)
                .max(1e-3);
            let bearing_damping = read_f("physxVehicleWheel:dampingRate")
                .filter(|&d| d > 0.0)
                .unwrap_or(drive_torque_max / max_rotation_speed);
            // Tire longitudinal stiffness → grip toward v/r before saturation.
            let slip_stiffness = read_f("physxVehicleTire:longitudinalStiffness").unwrap_or(8000.0);
            // Wheel brake torque caps the lock-up authority.
            let brake_torque_max = read_f("physxVehicleWheel:maxBrakeTorque")
                .unwrap_or(drive_torque_max * 3.0);
            // Coulomb μ for the drive-traction model (apply_wheel_drive). The
            // PhysX tire friction table is ground-material dependent and not a
            // single wheel scalar, so we read our own `lunco:frictionCoefficient`
            // (unit-friction default when unauthored).
            let friction_mu = read_f("lunco:frictionCoefficient").unwrap_or(1.0);
            // Ackermann steering lock at full input (rad); drives the front
            // steering-knuckle motor. Skid/rear wheels ignore it.
            let max_steer_angle = read_f("lunco:maxSteerAngle").unwrap_or(0.5);
            // Chassis-contact grip stiffness (slope of contact friction vs slip
            // before the Coulomb cone). USD: `lunco:contactGripStiffness`.
            let contact_grip_stiffness = read_f("lunco:contactGripStiffness").unwrap_or(50.0);

            // Standard-USD discriminator: an authored `PhysicsRevoluteJoint`
            // pointing at this wheel via `physics:body1` ⇒ joint-based.
            let key = (prim_path.stage_handle.clone(), prim_path.path.clone());
            // Front wheels (index < 2) of an Ackermann rover steer. Gate on the
            // rover's drive type — a skid rover keeps all wheels fixed (it steers
            // by skidding), so only wire the steering port when the PARENT rover
            // prim carries `PhysxVehicleDrive4WAPI` (Ackermann). Same for both
            // wheel kinds: each attaches a shared `SteeringActuator` (joint or
            // raycast), so the steering model is identical.
            let parent_prim = &prim_path.path[..prim_path.path.rfind('/').unwrap_or(0)];
            let is_ackermann = SdfPath::new(parent_prim)
                .map(|p| has_api_schema(reader, &p, "PhysxVehicleDrive4WAPI"))
                .unwrap_or(false);
            let steer_for_wheel = if index < 2 && is_ackermann { Some(p_steer) } else { None };
            if joint_targets.contains_key(&key) {
                setup_physical_wheel(
                    &mut commands, entity, prim_path, &existing_tf,
                    maybe_mesh, maybe_mat, maybe_shader_mat, maybe_child_of,
                    radius, p_drive,
                    drive_torque_max,
                    steer_for_wheel, max_steer_angle,
                );
            } else {
                setup_raycast_wheel(
                    &mut commands, entity, prim_path, &existing_tf,
                    maybe_mesh, maybe_mat, maybe_shader_mat, maybe_child_of,
                    radius, index, &suspension,
                    p_drive, p_steer, steer_for_wheel, max_steer_angle,
                    WheelSpinParams {
                        mass: wheel_mass,
                        moment_of_inertia: wheel_moi,
                        drive_torque_max,
                        bearing_damping,
                        friction_mu,
                        slip_stiffness,
                        contact_grip_stiffness,
                        brake_torque_max,
                    },
                );
            }
        }

        commands.entity(entity).insert(UsdSimProcessed);
    }
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

/// Authored `physxVehicleSuspension:*` parameters, read once and shared by
/// both wheel implementations. The raycast wheel emulates this spring
/// analytically; the joint wheel realises it as a real prismatic spring.
#[derive(Clone, Copy)]
struct SuspensionParams {
    /// Natural standoff of the wheel below its mount (raycast resting length).
    rest_length: f64,
    /// Spring stiffness, N/m.
    spring_k: f64,
    /// Spring damping, N·s/m.
    damping_c: f64,
}

/// USD-derived tire spin dynamics, forwarded onto the `WheelRaycast` so the
/// spin integrator (`lunco_mobility::update_wheel_spin`) runs on authored data.
struct WheelSpinParams {
    mass: f64,
    /// Explicit axle moment of inertia (kg·m²); 0 ⇒ derive ½·m·r².
    moment_of_inertia: f64,
    drive_torque_max: f64,
    bearing_damping: f64,
    friction_mu: f64,
    slip_stiffness: f64,
    contact_grip_stiffness: f64,
    brake_torque_max: f64,
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
    maybe_mat: Option<&MeshMaterial3d<StandardMaterial>>,
    maybe_shader_mat: Option<&MeshMaterial3d<lunco_materials::ShaderMaterial>>,
    maybe_child_of: Option<&ChildOf>,
    radius: f32,
    _index: i32,
    susp: &SuspensionParams,
    p_drive: Entity,
    p_steer: Entity,
    steer: Option<Entity>,
    max_steer_angle: f64,
    spin: WheelSpinParams,
) {
    info!("Setting up RAYCAST wheel {}", prim_path.path);

    let mut wheel = WheelRaycast {
        wheel_radius: radius as f64,
        visual_entity: Some(entity),
        drive_port: p_drive,
        steer_port: p_steer,
        rest_length: susp.rest_length,
        spring_k: susp.spring_k,
        damping_c: susp.damping_c,
        mass: spin.mass,
        moment_of_inertia: spin.moment_of_inertia,
        drive_torque_max: spin.drive_torque_max,
        bearing_damping: spin.bearing_damping,
        friction_mu: spin.friction_mu,
        slip_stiffness: spin.slip_stiffness,
        contact_grip_stiffness: spin.contact_grip_stiffness,
        brake_torque_max: spin.brake_torque_max,
        ..default()
    };

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
        // Move whichever material the prim received onto the visual child. A USD
        // `materialType="shader"` prim gets a `ShaderMaterial` (applied by the
        // material observer before this split runs) — prefer it over the default
        // `StandardMaterial` so USD-authored shaders survive the wheel split.
        if let Some(sm) = maybe_shader_mat.cloned() {
            visual.insert(sm);
        } else if let Some(mat) = maybe_mat.cloned() {
            visual.insert(mat);
        }
        wheel.visual_entity = Some(visual.id());
        commands.entity(entity).remove::<Mesh3d>();
        commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>();
        commands.entity(entity).remove::<MeshMaterial3d<lunco_materials::ShaderMaterial>>();
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
    if let Some(rover_ent) = rover_entity {
        ray_caster = ray_caster.with_query_filter(
            avian3d::prelude::SpatialQueryFilter::from_excluded_entities([rover_ent])
        );
    }

    commands.entity(entity).insert((
        wheel,
        ray_caster,
        RayHits::default(),
        wheel_tf,
    ));

    // Front Ackermann wheel: attach the SHARED steering servo. The same
    // `SteeringActuator` + system the physical joint uses computes this wheel's
    // rate-limited Ackermann angle into `output_angle`; `apply_wheel_steering`
    // rotates the raycast wheel to it — identical steering across wheel kinds.
    if let Some(steer_port) = steer {
        let mount = existing_tf.translation.as_dvec3();
        commands.entity(entity).insert(SteeringActuator {
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
    maybe_mat: Option<&MeshMaterial3d<StandardMaterial>>,
    maybe_shader_mat: Option<&MeshMaterial3d<lunco_materials::ShaderMaterial>>,
    maybe_child_of: Option<&ChildOf>,
    radius: f32,
    p_drive: Entity,
    peak_torque: f64,
    steer: Option<Entity>,
    max_steer_angle: f64,
) {
    info!("Setting up PHYSICAL wheel {}", prim_path.path);

    // `peak_torque` (N·m at full throttle) is the engine's `peakTorque`, the
    // SAME drive authority the raycast wheel uses — NOT the joint's
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
        // Move whichever material the prim received onto the visual child. A USD
        // `materialType="shader"` prim gets a `ShaderMaterial` (applied by the
        // material observer before this split runs) — prefer it over the default
        // `StandardMaterial` so USD-authored shaders survive the wheel split.
        if let Some(sm) = maybe_shader_mat.cloned() {
            visual.insert(sm);
        } else if let Some(mat) = maybe_mat.cloned() {
            visual.insert(mat);
        }
        commands.entity(entity).remove::<Mesh3d>();
        commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>();
        commands.entity(entity).remove::<MeshMaterial3d<lunco_materials::ShaderMaterial>>();
    }

    commands.entity(entity).remove::<WheelRaycast>()
        .remove::<RayCaster>()
        .remove::<RayHits>();

    commands.entity(entity).insert((
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
        RigidBody::Dynamic,
        collider,
        // Heavier wheels (100 kg vs the previous 25) damp the
        // joint↔solver impulse echo that produced visible idle wobble
        // when the rover was dropped from Y=5 onto the ground. With a
        // 1000 kg chassis the previous 40:1 mass ratio amplified
        // lateral float-precision noise into rolling drift.
        Mass(100.0),
        // The drive is an axle TORQUE on the wheel (see MotorActuator); wheel↔ground
        // friction propels the rover. μ is a COMPROMISE: high μ gives traction +
        // Ackermann cornering grip, but also high LATERAL grip that resists a skid
        // rover's sideways scrub (skid steering needs the wheels to slide). μ=0.9
        // lets the skid differential actually yaw the body while still moving + (with
        // AWD) cornering the Ackermann. `AngularDamping(0.3)` = wheel-bearing drag.
        Friction::new(0.9),
        LinearDamping(0.1),
        AngularDamping(0.3),
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

    // Velocity-controlled axle drive: pure velocity control (stiffness 0),
    // mass-auto-scaled. A raw constant axle torque sat in avian's low-slip
    // friction dead-zone (barely moved) at small values and broke traction at
    // large ones; commanding the spin rate instead is stable and self-limits the
    // top speed at traction.
    //
    // `max_torque` is the motor's STALL torque — how hard it can drive the wheel
    // toward the commanded spin. It must be well above the engine `peakTorque`
    // (the steady traction figure): for a SKID turn the inner wheels are
    // commanded to *reverse* while the body still carries forward momentum, and a
    // low cap lets them just keep rolling forward with the rover → no speed
    // differential → no yaw. A high stall torque lets the wheels actually enforce
    // their left/right speed split and pivot the body. Velocity control self-caps
    // the spin, so a high stall torque can't run away (unlike raw torque). Tunable
    // via USD later.
    const MAX_DRIVE_OMEGA: f64 = 12.0; // rad/s at full throttle (≈ 4.8 m/s at r=0.4)
    const DRIVE_DAMP: f64 = 30.0; // velocity-tracking aggressiveness (1/s)
    const STALL_TORQUE_GAIN: f64 = 6.0; // stall torque = peakTorque × this
    let drive_motor = AngularMotor::new(MotorModel::AccelerationBased {
        stiffness: 0.0,
        damping: DRIVE_DAMP,
    })
    .with_max_torque(peak_torque * STALL_TORQUE_GAIN);

    let mut joint_cmd = commands.spawn((
        RevoluteJoint::new(chassis, entity)
            .with_local_anchor1(mount_local)
            .with_local_anchor2(DVec3::ZERO)
            .with_hinge_axis(axle)
            .with_motor(drive_motor),
        JointCollisionDisabled,
        // All-wheel drive. The throttle port already carries the skid rover's
        // per-side differential (drive_left/drive_right), so a single mapping here
        // yaws the skid body; on the Ackermann rover all wheels share one throttle
        // and the front frame-steer does the turning.
        MotorActuator {
            port_entity: p_drive,
            max_omega: MAX_DRIVE_OMEGA,
            drive_sign: -1.0,
        },
        Name::new(format!("PhysicalWheelJoint_{}", prim_path.path)),
    ));
    // Front wheels of an Ackermann rover also steer (frame rotation about Y).
    if let Some(steer_port) = steer {
        joint_cmd.insert(SteeringActuator {
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

    // Logical wheel↔rover link, independent of Bevy hierarchy.
    // Reflects the OpenUSD `PhysicsArticulationRootAPI` graph.
    commands.entity(entity).insert(WheelOf(chassis));
    commands.queue(move |world: &mut World| {
        if let Some(mut rw) = world.get_mut::<RoverWheels>(chassis) {
            rw.0.push(entity);
        }
    });
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
    role: Res<lunco_core::NetworkRole>,
    q_chassis: Query<
        (
            &RigidBody,
            &Position,
            &Rotation,
            Option<&lunco_core::ReplicatedChassisMotion>,
        ),
        (With<RoverVessel>, Without<PhysicalWheel>),
    >,
    mut q_wheels: Query<
        (
            Entity,
            &PhysicalWheel,
            &WheelOf,
            &RigidBody,
            &mut Position,
            &mut Rotation,
        ),
        Without<lunco_core::OwnedLocally>,
    >,
    mut commands: Commands,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    for (e, wheel, wheel_of, rb, mut pos, mut rot) in q_wheels.iter_mut() {
        let Ok((c_rb, c_pos, c_rot, motion)) = q_chassis.get(wheel_of.0) else {
            continue;
        };
        if !matches!(c_rb, RigidBody::Kinematic) {
            continue; // host / owned rover — real local wheel physics
        }
        if !matches!(rb, RigidBody::Kinematic) {
            commands.entity(e).insert(RigidBody::Kinematic);
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
    // `WheelOf`, not `ChildOf`: the logical wheel→chassis link survives independent
    // of Bevy hierarchy. `Without<NetReplicate>`: replicated wheels carry their own
    // spin via the body's world rotation, so skip them (see docstring).
    mut q_wheels: Query<
        (&mut PhysicalWheel, &GlobalTransform, &WheelOf),
        Without<lunco_core::NetReplicate>,
    >,
    q_chassis: Query<
        (&RigidBody, &Position, Option<&lunco_core::ReplicatedChassisMotion>),
        With<RoverVessel>,
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

    for (mut wheel, gtf, wheel_of) in q_wheels.iter_mut() {
        let Ok((body, pos, motion)) = q_chassis.get(wheel_of.0) else { continue };
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
        let hub_world = gtf.translation().as_dvec3();
        let hub_vel = vlin + vang.cross(hub_world - pos.0);
        let forward = gtf.rotation().mul_vec3(Vec3::NEG_Z).as_dvec3();
        let v_long = hub_vel.dot(forward);
        let r = (wheel.wheel_radius as f64).max(1e-3);
        let w = v_long / r;

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
/// exists (has `FlightSoftware`), it creates `Wire` entities connecting the wheel's
/// physical ports to the appropriate digital ports.
///
/// # Wiring Rules
///
/// - **Left wheels** → `drive_left` digital port
/// - **Right wheels** → `drive_right` digital port
/// - **Front wheels** → `steering` digital port (for Ackermann)
/// - **All wheels** → brake (handled separately)
fn try_wire_wheel(
    q_pending: Query<(Entity, &UsdPrimPath, &PendingWheelWiring)>,
    q_fsw: Query<(&UsdPrimPath, &FlightSoftware)>,
    mut commands: Commands,
) {
    for (ent, prim_path, pending) in q_pending.iter() {
        let fsw_root = q_fsw.iter().find(|(path, _)| {
            path.stage_handle == prim_path.stage_handle && prim_path.path.starts_with(&path.path)
        });

        if let Some((_, fsw)) = fsw_root {
            let is_left = pending.index % 2 == 0;
            let is_front = pending.index < 2;

            let drive_port_name = if is_left { "drive_left" } else { "drive_right" };
            if let Some(&d_port) = fsw.port_map.get(drive_port_name) {
                commands.spawn((
                    Wire { source: d_port, target: pending.p_drive, scale: 1.0 },
                    Name::new(format!("Wire_Drive_{}", drive_port_name)),
                ));
                debug!("Wired wheel {} drive to FSW port {}", prim_path.path, drive_port_name);
            }

            if is_front {
                if let Some(&s_port) = fsw.port_map.get("steering") {
                    commands.spawn((
                        Wire { source: s_port, target: pending.p_steer, scale: 1.0 },
                        Name::new("Wire_Steering"),
                    ));
                    info!("Wired wheel {} STEERING to FSW port steering", prim_path.path);
                } else {
                    warn!("Wheel {} is front wheel but FSW has no 'steering' port!", prim_path.path);
                }
            }
            commands.entity(ent).remove::<PendingWheelWiring>();
        } else {
            debug!("Wheel {} FSW not found yet, retrying next frame", prim_path.path);
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
                lunco_core::ReplicatedChassisMotion {
                    lin: DVec3::new(0.0, 0.0, -2.0), // 2 m/s along chassis forward (−Z)
                    ang: DVec3::ZERO,
                },
                RoverVessel,
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
            WheelOf(chassis),
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
                lunco_core::ReplicatedChassisMotion {
                    lin: DVec3::new(0.0, 0.0, -2.0),
                    ang: DVec3::ZERO,
                },
                RoverVessel,
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
            WheelOf(chassis),
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

