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
//! | `PhysxVehicleWheelAPI` | `WheelRaycast`, `RayCaster`, visual child | Raycast wheel |
//!
//! ## Wheel Entity Splitting
//!
//! USD defines each wheel as a **single entity** with a mesh and a rotation (90° Z for
//! wheel orientation). However, LunCoSim's raycast wheels need two entities:
//!
//! 1. **Physics entity** — identity rotation so `RayCaster::new(Dir3::NEG_Y)` casts
//!    straight down (local space). If rotated, rays go sideways and hit the chassis.
//! 2. **Visual child entity** — 90° Z rotation + mesh so the cylinder renders as a
//!    rolling wheel (not a flat pancake).
//!
//! The `process_usd_sim_prims` system performs this split at runtime:
//!
//! ```text
//! USD Wheel Entity (has mesh + 90° Z rotation)
//! ├── Before processing: single entity, mesh visible but rotated wrong for physics
//! └── After processing:
//!     ├── Physics entity: identity rotation, NO mesh, WheelRaycast + RayCaster
//!     └── Visual child: 90° Z rotation, mesh, CellCoord (visible, correctly oriented)
//! ```
//!
//! This matches the procedural `spawn_raycast_rover` pattern exactly.
//!
//! ## Raycast Exclusion Filter
//!
//! Wheel raycasters use `SpatialQueryFilter::from_excluded_entities([rover_entity])` so
//! wheels don't hit their own chassis. Without this filter, downward rays immediately
//! collide with the chassis collider, pushing the rover into the sky (jiggling bug).
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

/// Plugin for mapping simulation-specific USD schemas (like NVIDIA PhysX Vehicles)
/// to LunCo's optimized simulation models.
///
/// # Processing Order
///
/// The plugin registers three systems that run in the `Update` schedule:
///
/// 1. `process_usd_sim_prims` — maps schemas to components (runs after sync_usd_visuals)
/// 2. `swap_raycast_to_joint` — converts raycast wheels to physical joint wheels
/// 3. `try_wire_wheel` — connects wheel drive ports to FSW digital ports
///
/// The observer `on_add_usd_sim_prim` intentionally does minimal work — it only detects
/// physics joints. All other processing is deferred to ensure assets are loaded first.
pub struct UsdSimPlugin;

impl Plugin for UsdSimPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_usd_sim_prim)
           .add_systems(Update, (
               process_usd_sim_prims,
               swap_raycast_to_joint,
               try_wire_wheel,
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

/// Marker for wheels that are physically connected via joints.
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
/// 4. **Detects `PhysxVehicleWheelAPI`** → Splits the wheel entity into:
///    - Physics entity: `WheelRaycast`, `RayCaster` (identity rotation, exclusion filter)
///    - Visual child: mesh + 90° Z rotation for correct wheel rendering
///
/// # Why Not Just Use the Observer?
///
/// The observer fires when the entity is spawned, but the USD asset may not be loaded
/// yet. This system retries every frame until the asset is available, then marks the
/// entity with `UsdSimProcessed` to prevent re-processing.
fn process_usd_sim_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath, Option<&Transform>, Option<&Mesh3d>, Option<&MeshMaterial3d<StandardMaterial>>, Option<&ChildOf>), Without<UsdSimProcessed>>,
    q_fsw: Query<(&UsdPrimPath, &FlightSoftware)>,
    stages: Res<Assets<UsdStageAsset>>,
) {
    for (entity, prim_path, maybe_tf, maybe_mesh, maybe_mat, maybe_child_of) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        let mut reader = (*stage.reader).clone();
        let existing_tf = maybe_tf.cloned().unwrap_or_default();

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
                lunco_core::Vessel,
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

        // 3. Detect Physics Joints
        if let Ok(val) = reader.get(&sdf_path, "typeName") {
            if let Value::Token(type_name) = val.as_ref() {
                if type_name == "PhysicsRevoluteJoint" {
                    if let Ok(body1_val) = reader.get(&sdf_path.append_property("physics:body1").unwrap(), "targetPaths") {
                        if let Value::PathListOp(op) = body1_val.as_ref() {
                            if let Some(target_path) = op.explicit_items.first().or(op.prepended_items.first()) {
                                for (wheel_ent, wheel_path, _, _, _, _) in query.iter() {
                                    if wheel_path.path == target_path.as_str() && wheel_path.stage_handle == prim_path.stage_handle {
                                        commands.entity(wheel_ent).insert(PhysicalWheel);
                                        info!("Marked {} as PhysicalWheel based on joint {}", wheel_path.path, prim_path.path);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. Detect PhysxVehicleWheelAPI (The Wheel Intercept)
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

            let mut wheel = WheelRaycast {
                wheel_radius: radius as f64,
                visual_entity: Some(entity), // Will be updated below after visual child is spawned
                drive_port: p_drive,
                steer_port: p_steer,
                ..default()
            };

            // Read suspension parameters from USD
            if let Some(rest_len) = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:restLength") {
                wheel.rest_length = rest_len as f64;
            }
            if let Some(k) = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springStiffness") {
                wheel.spring_k = k as f64;
            }
            if let Some(d) = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springDamping") {
                wheel.damping_c = d as f64;
            }

            // --- Wheel Entity Splitting ---
            //
            // The USD file defines each wheel as a single entity with a mesh and a rotation
            // (90° Z for wheel orientation). We need to split this into:
            //
            // 1. Physics entity: identity rotation (for correct raycasting), NO mesh
            // 2. Visual child entity: 90° Z rotation + mesh (for correct rendering)
            //
            // This matches the procedural spawn_raycast_rover pattern exactly.
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

                // Add material if the physics entity had one
                if let Some(mat) = maybe_mat.cloned() {
                    commands.entity(visual_entity).insert(mat);
                }

                commands.entity(entity).add_child(visual_entity);
                // CRITICAL: Update WheelRaycast.visual_entity to point to the visual child.
                // The mobility system moves visual_entity to track suspension compression.
                wheel.visual_entity = Some(visual_entity);
                // Remove Mesh3d and material from physics entity to avoid duplicate rendering
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
            // The wheel's parent entity (via ChildOf) is the rover chassis.
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

            // Remove any physics components that were added by the Avian plugin
            // (wheels are raycast, not physical rigid bodies)
            commands.entity(entity)
                .remove::<Collider>()
                .remove::<RigidBody>()
                .remove::<Mass>();
        }

        commands.entity(entity).insert(UsdSimProcessed);
    }
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
                info!("Wired wheel {} drive to FSW port {}", prim_path.path, drive_port_name);
            }

            if is_front {
                if let Some(&s_port) = fsw.port_map.get("steering") {
                    commands.spawn((
                        Wire { source: s_port, target: pending.p_steer, scale: 1.0 },
                        Name::new("Wire_Steering"),
                    ));
                    info!("Wired wheel {} steering to FSW port steering", prim_path.path);
                }
            }
            commands.entity(ent).remove::<PendingWheelWiring>();
        }
    }
}

/// Converts raycast wheels to physical joint-based wheels.
///
/// This system watches for `PhysicalWheel` markers (created when a joint is detected
/// in the USD file) and swaps the raycast wheel components for physical body components
/// with a motor actuator.
///
/// This is used for joint-based rovers that use physical collision wheels instead of
/// raycast suspension.
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
