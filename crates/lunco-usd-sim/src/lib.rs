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
//! | `PhysxVehicleWheelAPI` | `WheelRaycast` or `MotorActuator` + `RigidBody` | Wheel (type from `lunco:wheelType`) |
//!
//! ## Wheel Type Declaration
//!
//! The `lunco:wheelType` attribute on the **chassis prim** determines how wheels are set up:
//!
//! - `"raycast"` (default) — Wheels use `WheelRaycast` + `RayCaster` for suspension simulation.
//!   The wheel entity is split into physics (identity rotation) + visual child (with rotation).
//! - `"physical"` — Wheels are full rigid bodies with `RigidBody`, `Collider`, and
//!   `MotorActuator`. They interact with terrain through physical collision, not raycasting.
//!
//! ```usda
//! def Cube "Rover" (
//!     prepend apiSchemas = ["PhysxVehicleContextAPI", "PhysxVehicleDriveSkidAPI"]
//! ) {
//!     string lunco:wheelType = "raycast"  // or "physical"
//!     // ...
//! }
//! ```
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
use big_space::prelude::CellCoord;
pub use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
use openusd::sdf::{Path as SdfPath, AbstractData, Value};
use openusd::usda::TextReader;
use lunco_mobility::{WheelRaycast, DifferentialDrive, AckermannSteer};
use lunco_fsw::FlightSoftware;
use lunco_core::architecture::{DigitalPort, PhysicalPort, Wire};
use lunco_hardware::MotorActuator;
use lunco_core::RoverVessel;
use std::collections::HashMap;

/// Determines how wheels interact with terrain.
///
/// Set via the `lunco:wheelType` attribute on the chassis prim.
/// Defaults to `Raycast` if not specified (backward compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WheelType {
    /// Wheels use raycast suspension simulation.
    /// The wheel entity is split into physics + visual child.
    #[default]
    Raycast,
    /// Wheels are full rigid bodies with physical collision.
    /// No raycasting — wheels interact with terrain colliders directly.
    Physical,
}

/// Plugin for mapping simulation-specific USD schemas (like NVIDIA PhysX Vehicles)
/// to LunCo's optimized simulation models.
///
/// # Processing Order
///
/// The plugin registers three systems that run in the `Update` schedule:
///
/// 1. `process_usd_sim_prims` — maps schemas to components (runs after sync_usd_visuals)
/// 2. `swap_raycast_to_joint` — legacy: converts raycast wheels to physical wheels
/// 3. `try_wire_wheel` — connects wheel drive ports to FSW digital ports
///
/// The observer `on_add_usd_sim_prim` intentionally does minimal work. All processing
/// is deferred to the `process_usd_sim_prims` system to ensure assets are loaded first.
pub struct UsdSimPlugin;

impl Plugin for UsdSimPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_usd_sim_prim)
           // `try_wire_wheel` runs in PreUpdate so that Wire entities exist
           // before `wire_system` (Update) propagates values through them.
           .add_systems(PreUpdate, try_wire_wheel)
           .add_systems(Update, (
               process_usd_sim_prims,
               swap_raycast_to_joint,
           ).chain().after(lunco_usd_bevy::sync_usd_visuals));
    }
}

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

/// Marks a chassis entity with its wheel type.
///
/// Added during chassis processing so wheels can look up their type
/// via the parent-child relationship.
#[derive(Component)]
pub struct ChassisWheelType(pub WheelType);

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
///    Also reads `lunco:wheelType` attribute (`"raycast"` or `"physical"`, defaults to raycast).
/// 2. **Detects `PhysxVehicleDriveSkidAPI`** → Creates `DifferentialDrive` with port names.
/// 3. **Detects `PhysxVehicleDrive4WAPI`** → Creates `AckermannSteer` with port names.
/// 4. **Detects `PhysxVehicleWheelAPI`** → Sets up wheel based on `lunco:wheelType`:
///    - **Raycast**: `WheelRaycast`, `RayCaster` (entity split into physics + visual child)
///    - **Physical**: `RigidBody`, `Collider`, `MotorActuator` (keeps original entity)
///
/// # Wheel Type Detection
///
/// The `lunco:wheelType` attribute on the **chassis prim** determines wheel behavior:
/// - `"raycast"` (default) — Raycast suspension simulation
/// - `"physical"` — Full rigid body with physical collision
///
/// Wheels read this from their parent chassis via the USD prim hierarchy.
fn process_usd_sim_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath, Option<&Transform>, Option<&Mesh3d>, Option<&MeshMaterial3d<StandardMaterial>>, Option<&ChildOf>), Without<UsdSimProcessed>>,
    stages: Res<Assets<UsdStageAsset>>,
) {
    // --- Pass 1: Detect all chassis types first ---
    // We need wheel types before processing wheels, but entities are processed
    // in arbitrary order. Collect chassis types first.
    let mut chassis_types: HashMap<(Handle<UsdStageAsset>, String), WheelType> = HashMap::new();

    for (_, prim_path, _, _, _, _) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };
        let mut reader = (*stage.reader).clone();

        if has_api_schema(&mut reader, &sdf_path, "PhysxVehicleContextAPI") {
            let wheel_type = read_wheel_type(&mut reader, &sdf_path);
            chassis_types.insert((prim_path.stage_handle.clone(), prim_path.path.clone()), wheel_type);
        }
    }

    // --- Pass 2: Process all prims ---
    for (entity, prim_path, maybe_tf, maybe_mesh, maybe_mat, maybe_child_of) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        let mut reader = (*stage.reader).clone();
        let existing_tf = maybe_tf.cloned().unwrap_or_default();

        // 1. Detect PhysxVehicleContextAPI (The Rover Root)
        // Creates FlightSoftware with 4 digital ports + RoverVessel + Vessel markers
        if has_api_schema(&mut reader, &sdf_path, "PhysxVehicleContextAPI") {
            info!("Intercepted PhysxVehicleContextAPI for {}, initializing Flight Software", prim_path.path);

            let wheel_type = read_wheel_type(&mut reader, &sdf_path);
            info!("Wheel type for {}: {:?}", prim_path.path, wheel_type);

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
                lunco_core::Vessel,
                ChassisWheelType(wheel_type),
            ));
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

            // Determine wheel type from parent chassis
            let wheel_type = resolve_wheel_type(&chassis_types, &mut reader, &sdf_path, &prim_path);

            // Create physical ports for drive and steering
            let p_drive = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Drive"))).id();
            let p_steer = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Steer"))).id();

            let index = reader.prim_attribute_value::<i32>(&sdf_path, "physxVehicleWheel:index").unwrap_or(0);

            // Mark for wiring — the try_wire_wheel system will connect ports once FSW exists
            commands.entity(entity).insert(PendingWheelWiring { index, p_drive, p_steer });

            // Read common wheel parameters
            let rest_length = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:restLength")
                .unwrap_or(0.7) as f64;
            let spring_k = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springStiffness")
                .unwrap_or(15000.0) as f64;
            let damping_c = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springDamping")
                .unwrap_or(3000.0) as f64;

            match wheel_type {
                WheelType::Raycast => {
                    setup_raycast_wheel(
                        &mut commands, entity, &prim_path, &existing_tf,
                        maybe_mesh, maybe_mat, maybe_child_of,
                        radius, index, rest_length, spring_k, damping_c,
                        p_drive, p_steer,
                    );
                }
                WheelType::Physical => {
                    setup_physical_wheel(
                        &mut commands, entity, &prim_path, &existing_tf,
                        maybe_mesh, maybe_mat,
                        radius, index, p_drive, p_steer,
                        &mut reader, &sdf_path,
                    );
                }
            }
        }

        commands.entity(entity).insert(UsdSimProcessed);
    }
}

/// Reads `lunco:wheelType` from a chassis prim.
///
/// Returns `WheelType::Raycast` (the default) if the attribute is absent,
/// ensuring backward compatibility with existing USD files.
fn read_wheel_type(reader: &mut TextReader, chassis_path: &SdfPath) -> WheelType {
    match reader.prim_attribute_value::<String>(chassis_path, "lunco:wheelType").as_deref() {
        Some("physical") => WheelType::Physical,
        _ => WheelType::Raycast,
    }
}

/// Resolves the wheel type for a wheel prim by checking:
/// 1. The parent chassis in the collected chassis_types map
/// 2. The parent prim's `lunco:wheelType` attribute in the USD stage
/// 3. Defaults to `WheelType::Raycast` (backward compatible)
fn resolve_wheel_type(
    chassis_types: &HashMap<(Handle<UsdStageAsset>, String), WheelType>,
    reader: &mut TextReader,
    wheel_sdf: &SdfPath,
    prim_path: &UsdPrimPath,
) -> WheelType {
    // Try to find parent chassis path (wheel path is like "/Rover/Wheel_FL")
    let wheel_path_str = wheel_sdf.as_str();
    if let Some(last_slash) = wheel_path_str.rfind('/') {
        let parent_path_str = &wheel_path_str[..last_slash];
        if let Ok(parent_sdf) = SdfPath::new(parent_path_str) {
            // Check collected chassis types first
            if let Some(&wheel_type) = chassis_types.get(&(prim_path.stage_handle.clone(), parent_path_str.to_string())) {
                return wheel_type;
            }
            // Fall back to reading from USD stage directly
            return read_wheel_type(reader, &parent_sdf);
        }
    }
    WheelType::Raycast
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

    // --- Wheel Entity Splitting ---
    let wheel_mesh = maybe_mesh.map(|m| m.clone());
    let wheel_rotation = existing_tf.rotation;

    if wheel_mesh.is_some() && wheel_rotation != Quat::IDENTITY {
        let visual_entity = commands.spawn((
            Name::new(format!("{}_visual", prim_path.path.split('/').next_back().unwrap_or("wheel"))),
            Transform {
                translation: Vec3::ZERO,
                rotation: wheel_rotation,
                scale: existing_tf.scale,
            },
            CellCoord::default(),
            Visibility::Inherited,
            InheritedVisibility::default(),
            ViewVisibility::default(),
            wheel_mesh.unwrap(),
        )).id();

        if let Some(mat) = maybe_mat.cloned() {
            commands.entity(visual_entity).insert(mat);
        }

        commands.entity(entity).add_child(visual_entity);
        wheel.visual_entity = Some(visual_entity);
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

/// Sets up a physical wheel as a full rigid body with collision.
///
/// Physical wheels interact with terrain through physical collision, not raycasting.
/// The wheel entity keeps its mesh and rotation (no entity splitting needed).
fn setup_physical_wheel(
    commands: &mut Commands,
    entity: Entity,
    prim_path: &UsdPrimPath,
    existing_tf: &Transform,
    _maybe_mesh: Option<&Mesh3d>,
    _maybe_mat: Option<&MeshMaterial3d<StandardMaterial>>,
    radius: f32,
    _index: i32,
    p_drive: Entity,
    _p_steer: Entity,
    reader: &mut TextReader,
    sdf_path: &SdfPath,
) {
    info!("Setting up PHYSICAL wheel {}", prim_path.path);

    // Read motor parameters from USD (used later when wiring motors)
    let _motor_power = reader.prim_attribute_value::<f64>(sdf_path, "lunco:motorPower")
        .unwrap_or(2000.0);
    let _motor_efficiency = reader.prim_attribute_value::<f64>(sdf_path, "lunco:motorEfficiency")
        .unwrap_or(0.85);

    // Physical wheel: keep rotation (no raycast direction issue), keep mesh
    // but ensure it's a proper rigid body with collision
    let wheel_tf = Transform {
        translation: existing_tf.translation,
        rotation: existing_tf.rotation, // Keep USD rotation for physical wheels
        scale: existing_tf.scale,
    };

    // Remove raycast components if they were added by a previous pass
    commands.entity(entity).remove::<WheelRaycast>()
        .remove::<RayCaster>()
        .remove::<RayHits>();

    // Set up as physical body
    commands.entity(entity).insert((
        PhysicalWheel,
        MotorActuator {
            port_entity: p_drive,
            axis: DVec3::Y,
        },
        RigidBody::Dynamic,
        Collider::cylinder(radius as f64, (radius * 0.5) as f64),
        Mass(25.0),
        Friction::new(0.8),
        LinearDamping(0.5),
        AngularDamping(2.0),
        wheel_tf,
    ));
}

/// Marker to indicate a prim has been processed by the sim system.
#[derive(Component)]
struct UsdSimProcessed;

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

/// Converts raycast wheels to physical joint-based wheels.
///
/// **Legacy:** This system exists for backward compatibility with rovers that use
/// `PhysicsRevoluteJoint` in USD. New rover definitions should use `lunco:wheelType = "physical"`
/// on the chassis prim instead, which sets up physical wheels directly without this swap.
///
/// Watches for `PhysicalWheel` markers (created when a joint is detected in USD)
/// and swaps the raycast wheel components for physical body components with a motor actuator.
fn swap_raycast_to_joint(
    q_physical: Query<(Entity, &WheelRaycast, &PhysicalWheel), Added<PhysicalWheel>>,
    mut commands: Commands,
) {
    for (entity, wheel, _) in q_physical.iter() {
        info!("Swapping Raycast wheel to Physical Joint-Based wheel");
        commands.entity(entity)
            .remove::<WheelRaycast>()
            .remove::<RayCaster>()
            .remove::<RayHits>()
            .insert((
                MotorActuator {
                    port_entity: wheel.drive_port,
                    axis: DVec3::Y,
                },
                RigidBody::Dynamic,
                Collider::cylinder(wheel.wheel_radius, wheel.wheel_radius * 0.5),
            ));
    }
}
