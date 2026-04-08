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
pub struct UsdSimPlugin;

impl Plugin for UsdSimPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_usd_sim_prim)
           .add_systems(Update, (process_usd_sim_prims, swap_raycast_to_joint, try_wire_wheel).chain().after(lunco_usd_bevy::sync_usd_visuals));
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
/// Runs in Update, checking for prims that haven't been processed yet.
/// This is needed because asset loading is async - the observer fires
/// before the asset is ready, so we retry here.
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
            // Skip if mesh doesn't exist yet — sync_usd_visuals may not have processed this prim.
            // We'll retry next frame (not marking UsdSimProcessed).
            if maybe_mesh.is_none() {
                debug!("Wheel {} has no mesh yet, skipping until next frame", prim_path.path);
                continue;
            }
            info!("Intercepted PhysxVehicleWheelAPI for {}", prim_path.path);

            let p_drive = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Drive"))).id();
            let p_steer = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Steer"))).id();

            let index = reader.prim_attribute_value::<i32>(&sdf_path, "physxVehicleWheel:index").unwrap_or(0);

            // Wire wheels to FSW ports
            let _fsw_root = q_fsw.iter().find(|(path, _)| {
                path.stage_handle == prim_path.stage_handle && prim_path.path.starts_with(&path.path)
            });

            commands.entity(entity).insert(PendingWheelWiring { index, p_drive, p_steer });

            let mut wheel = WheelRaycast {
                wheel_radius: radius as f64,
                visual_entity: Some(entity),
                drive_port: p_drive,
                steer_port: p_steer,
                ..default()
            };

            if let Some(rest_len) = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:restLength") {
                wheel.rest_length = rest_len as f64;
            }
            if let Some(k) = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springStiffness") {
                wheel.spring_k = k as f64;
            }
            if let Some(d) = reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springDamping") {
                wheel.damping_c = d as f64;
            }

            // CRITICAL: Split wheel entity into physics (no rotation) + visual (with rotation).
            // The entity has rotation from USD (e.g., 90° Z for wheel alignment).
            // RayCaster::new(Dir3::NEG_Y) uses LOCAL space direction.
            // If entity is rotated, the ray direction gets rotated too → rays go sideways!
            // Solution: Match procedural code - physics entity has identity rotation,
            //           separate visual child has the rotation AND the mesh.

            let wheel_mesh = maybe_mesh.map(|m| m.clone());
            let wheel_rotation = existing_tf.rotation;

            // Spawn visual child entity with rotation
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
                // Update WheelRaycast.visual_entity to point to the visual child
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

fn on_add_usd_sim_prim(
    _trigger: On<Add, UsdPrimPath>,
    _query: Query<(Entity, &UsdPrimPath)>,
    _stages: Res<Assets<UsdStageAsset>>,
    mut _commands: Commands,
) {
    // Intentionally empty — all processing is handled by process_usd_sim_prims
    // in the Update schedule, AFTER sync_usd_visuals creates meshes.
    // This ensures:
    // 1. Assets are fully loaded before processing
    // 2. Meshes exist so we can split wheel entities into physics + visual
    // 3. No duplicate processing or duplicate FSW ports
}

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
