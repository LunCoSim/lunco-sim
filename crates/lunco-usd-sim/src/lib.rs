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
use bevy::math::DVec3;
use avian3d::prelude::*;
use big_space::prelude::{CellCoord, FloatingOrigin, Grid};
pub use lunco_usd_bevy::{UsdPreviewOnly, UsdPrimPath, UsdStageAsset};
use openusd::sdf::{Path as SdfPath, AbstractData, Value};
use openusd::usda::TextReader;
use lunco_mobility::{WheelRaycast, DifferentialDrive, AckermannSteer};
use lunco_fsw::FlightSoftware;
use lunco_core::architecture::{DigitalPort, PhysicalPort, Wire};
use lunco_hardware::MotorActuator;
use lunco_core::RoverVessel;
use lunco_avatar::{FreeFlightCamera, OrbitCamera, SpringArmCamera, AdaptiveNearPlane};
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
           .add_observer(on_add_usd_sim_prim)
           // `try_wire_wheel` runs in PreUpdate so that Wire entities exist
           // before `wire_system` (Update) propagates values through them.
           .add_systems(PreUpdate, try_wire_wheel)
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

/// Helper to check if a prim has a specific API schema applied.
///
/// Handles both `TokenVec` (resolved) and `TokenListOp` (with prepend/append ops)
/// since the USD parser stores apiSchemas as a list operation.
fn has_api_schema(reader: &mut TextReader, path: &SdfPath, schema_name: &str) -> bool {
    if let Ok(val) = reader.get(path, "apiSchemas") {
        match val.as_ref() {
            Value::TokenVec(tokens) => {
                return tokens.iter().any(|s| s == schema_name);
            }
            Value::TokenListOp(list_op) => {
                let mut all_items = list_op.explicit_items.iter()
                    .chain(list_op.prepended_items.iter())
                    .chain(list_op.appended_items.iter())
                    .chain(list_op.added_items.iter());
                return all_items.any(|s| s.as_str() == schema_name);
            }
            _ => {}
        }
    }
    false
}

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

/// Marker for physical wheels awaiting full physical setup.
///
/// Physical wheels are full rigid bodies that interact with terrain through
/// collision, not raycast suspension. They get `RigidBody`, `Collider`, and
/// `MotorActuator` instead of `WheelRaycast` + `RayCaster`.
#[derive(Component)]
pub struct PhysicalWheel;

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
    query: Query<(Entity, &UsdPrimPath, Option<&Transform>, Option<&Mesh3d>, Option<&MeshMaterial3d<StandardMaterial>>, Option<&ChildOf>), Without<UsdSimProcessed>>,
    q_all_prims: Query<&UsdPrimPath>,
    q_grids: Query<Entity, With<Grid>>,
    q_existing_floating_origins: Query<Entity, With<FloatingOrigin>>,
    q_child_of: Query<&ChildOf>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    stages: Res<Assets<UsdStageAsset>>,
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

    // Scan the **stage data** rather than spawned entities. Joint and
    // wheel prims may be spawned on different frames; reading from the
    // SDF data directly avoids the race where a wheel is processed
    // before its joint sibling has an entity in the ECS.
    let mut seen_stages: std::collections::HashSet<Handle<UsdStageAsset>> = Default::default();
    for prim_path in q_all_prims.iter() {
        if !seen_stages.insert(prim_path.stage_handle.clone()) { continue; }
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let reader = (*stage.reader).clone();
        for (path, _spec) in reader.iter() {
            let Ok(val) = reader.get(path, "typeName") else { continue; };
            let type_name = match &*val {
                Value::Token(t) => Some(t.as_str().to_string()),
                Value::String(s) => Some(s.clone()),
                _ => None,
            };
            if type_name.as_deref() == Some("PhysicsRevoluteJoint") {
                if let Some(body1) = read_rel_target(&reader, path, "physics:body1") {
                    debug!("USD joint dispatch: {} → wheel {}", path.as_str(), body1);
                    joint_targets.insert(
                        (prim_path.stage_handle.clone(), body1),
                        path.as_str().to_string(),
                    );
                }
            }
        }
    }

    // --- Pass 2: Process all prims ---
    for (entity, prim_path, maybe_tf, maybe_mesh, maybe_mat, maybe_child_of) in query.iter() {
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

        let mut reader = (*stage.reader).clone();
        let existing_tf = maybe_tf.cloned().unwrap_or_default();

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

            // Build camera based on mode, then parent to Grid for FloatingOrigin
            match camera_mode.as_str() {
                "freeflight" => {
                    commands.entity(entity).insert((
                        Camera3d::default(),
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
                        SpringArmCamera {
                            target: Entity::PLACEHOLDER,
                            distance: 15.0,
                            yaw,
                            pitch,
                            damping: None,
                            vertical_offset: 2.0,
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
        if has_api_schema(&mut reader, &sdf_path, "PhysxVehicleContextAPI") {
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
            if has_api_schema(&mut reader, &sdf_path, "PhysicsArticulationRootAPI") {
                commands.entity(entity).insert(ArticulationRoot);
                info!("Detected PhysicsArticulationRootAPI on {}", prim_path.path);
            }

            info!("Successfully initialized FSW for {}", prim_path.path);
        }

        // 2. Detect Drive Schemas (Chassis Logic)
        if has_api_schema(&mut reader, &sdf_path, "PhysxVehicleDriveSkidAPI") {
            info!("Detected Skid Drive for {}", prim_path.path);
            commands.entity(entity).insert(DifferentialDrive {
                left_port: "drive_left".to_string(),
                right_port: "drive_right".to_string(),
            });
        } else if has_api_schema(&mut reader, &sdf_path, "PhysxVehicleDrive4WAPI") {
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
            info!("Intercepted PhysxVehicleWheelAPI for {}", prim_path.path);

            // Create physical ports for drive and steering
            let p_drive = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Drive"))).id();
            let p_steer = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Steer"))).id();

            let index = reader.prim_attribute_value::<i32>(&sdf_path, "physxVehicleWheel:index").unwrap_or(0);

            // Mark for wiring — the try_wire_wheel system will connect ports once FSW exists
            commands.entity(entity).insert(PendingWheelWiring { index, p_drive, p_steer });

            // Read common wheel parameters (used by raycast path only).
            let rest_length = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:restLength")
                .unwrap_or(0.7) as f64;
            let spring_k = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springStiffness")
                .unwrap_or(15000.0) as f64;
            let damping_c = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springDamping")
                .unwrap_or(3000.0) as f64;

            // Standard-USD discriminator: an authored `PhysicsRevoluteJoint`
            // pointing at this wheel via `physics:body1` ⇒ joint-based.
            let key = (prim_path.stage_handle.clone(), prim_path.path.clone());
            if let Some(joint_path_str) = joint_targets.get(&key).cloned() {
                let joint_sdf = SdfPath::new(&joint_path_str).ok();
                setup_physical_wheel(
                    &mut commands, entity, prim_path, &existing_tf,
                    maybe_mesh, maybe_mat, maybe_child_of,
                    radius, p_drive,
                    &mut reader, joint_sdf.as_ref(),
                );
            } else {
                setup_raycast_wheel(
                    &mut commands, entity, prim_path, &existing_tf,
                    maybe_mesh, maybe_mat, maybe_child_of,
                    radius, index, rest_length, spring_k, damping_c,
                    p_drive, p_steer,
                );
            }
        }

        commands.entity(entity).insert(UsdSimProcessed);
    }
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
    maybe_child_of: Option<&ChildOf>,
    radius: f32,
    _index: i32,
    rest_length: f64,
    spring_k: f64,
    damping_c: f64,
    p_drive: Entity,
    p_steer: Entity,
) {
    info!("Setting up RAYCAST wheel {}", prim_path.path);

    let mut wheel = WheelRaycast {
        wheel_radius: radius as f64,
        visual_entity: Some(entity),
        drive_port: p_drive,
        steer_port: p_steer,
        rest_length,
        spring_k,
        damping_c,
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
        if let Some(mat) = maybe_mat.cloned() {
            visual.insert(mat);
        }
        wheel.visual_entity = Some(visual.id());
        commands.entity(entity).remove::<Mesh3d>();
        commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>();
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
/// (`physics:axis`, `physics:localPos0/1`, `drive:angular:physics:maxForce`)
/// alongside the wheel's rigid-body init. Doing it lazily — letting
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
    maybe_child_of: Option<&ChildOf>,
    radius: f32,
    p_drive: Entity,
    reader: &mut TextReader,
    joint_sdf: Option<&SdfPath>,
) {
    info!("Setting up PHYSICAL wheel {}", prim_path.path);

    // Motor stall torque, in N·m. Read from `UsdPhysicsDriveAPI:angular`
    // applied to the joint prim — the OpenUSD-standard way to author an
    // angular drive on a revolute joint. `drive:angular:physics:maxForce`
    // is the maximum torque the drive can deliver; we treat it as the
    // motor's stall torque (port reads ±1.0 → ±maxForce N·m).
    let peak_torque = joint_sdf.and_then(|j| {
        reader.prim_attribute_value::<f64>(j, "drive:angular:physics:maxForce")
            .or_else(|| reader.prim_attribute_value::<f32>(j, "drive:angular:physics:maxForce").map(|v| v as f64))
    }).unwrap_or(360.0);

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
    // Sign chosen so a positive port value (W / DriveForward) rolls the
    // rover along its chassis-local -Z (Bevy's forward). With wheel body
    // identity-aligned to the chassis, axle = wheel_axis_rot * Y; torque
    // about +axle spins the wheel such that the contact point moves +Z
    // (i.e. rover moves -Z), so we negate to put forward command on -Z.
    let motor_axis = -(wheel_axis_rot * Vec3::Y).as_dvec3();

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
        if let Some(mat) = maybe_mat.cloned() {
            visual.insert(mat);
        }
        commands.entity(entity).remove::<Mesh3d>();
        commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>();
    }

    commands.entity(entity).remove::<WheelRaycast>()
        .remove::<RayCaster>()
        .remove::<RayHits>();

    commands.entity(entity).insert((
        PhysicalWheel,
        MotorActuator {
            port_entity: p_drive,
            axis: motor_axis,
            peak_torque,
        },
        RigidBody::Dynamic,
        collider,
        // Heavier wheels (100 kg vs the previous 25) damp the
        // joint↔solver impulse echo that produced visible idle wobble
        // when the rover was dropped from Y=5 onto the ground. With a
        // 1000 kg chassis the previous 40:1 mass ratio amplified
        // lateral float-precision noise into rolling drift.
        Mass(100.0),
        Friction::new(1.2),
        LinearDamping(2.0),
        AngularDamping(4.0),
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
    let anchor_chassis = existing_tf.translation.as_dvec3();
    let chassis_axis = (existing_tf.rotation * Vec3::Y).as_dvec3();
    // `JointCollisionDisabled` stops residual contact impulses between
    // wheel and chassis colliders that would otherwise drift the rover.
    commands.spawn((
        RevoluteJoint::new(chassis, entity)
            .with_local_anchor1(anchor_chassis)
            .with_local_anchor2(DVec3::ZERO)
            .with_hinge_axis(chassis_axis),
        JointCollisionDisabled,
        Name::new(format!("PhysicalWheelJoint_{}", prim_path.path)),
    ));

    // Logical wheel↔rover link, independent of Bevy hierarchy.
    // Reflects the OpenUSD `PhysicsArticulationRootAPI` graph.
    commands.entity(entity).insert(WheelOf(chassis));
    commands.queue(move |world: &mut World| {
        if let Some(mut rw) = world.get_mut::<RoverWheels>(chassis) {
            rw.0.push(entity);
        }
    });
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

/// Reads the first target path from a USD relationship (e.g. `physics:body1`).
///
/// USD relationship specs live at `<prim_path>.<rel_name>`. This mirrors
/// `lunco-usd-avian::read_rel_target` — kept local rather than re-exported
/// to avoid a public-API hop for one helper.
fn read_rel_target(reader: &TextReader, prim_path: &SdfPath, rel_name: &str) -> Option<String> {
    let rel_path_str = format!("{}.{}", prim_path.as_str(), rel_name);
    let Ok(rel_sdf) = SdfPath::new(&rel_path_str) else { return None; };
    if let Ok(val) = reader.get(&rel_sdf, "targetPaths") {
        if let Value::PathListOp(op) = &*val {
            if let Some(target) = op.explicit_items.first()
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
