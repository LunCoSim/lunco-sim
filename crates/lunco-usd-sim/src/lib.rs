use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
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
           .add_systems(Update, (swap_raycast_to_joint, try_wire_wheel));
    }
}

/// Helper to check if a prim has a specific API schema applied.
fn has_api_schema(reader: &mut TextReader, path: &SdfPath, schema_name: &str) -> bool {
    if let Ok(val) = reader.get(path, "apiSchemas") {
        if let Value::TokenVec(tokens) = val.as_ref() {
            return tokens.iter().any(|s| s == schema_name);
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

fn on_add_usd_sim_prim(
    trigger: On<Add, UsdPrimPath>,
    query: Query<(Entity, &UsdPrimPath)>,
    q_fsw: Query<(&UsdPrimPath, &FlightSoftware)>,
    stages: Res<Assets<UsdStageAsset>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok((_, prim_path)) = query.get(entity) else { return; };
    let Some(stage) = stages.get(&prim_path.stage_handle) else { return; };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

    let mut reader = (*stage.reader).clone();

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
                            for (wheel_ent, wheel_path) in query.iter() {
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
        info!("Intercepted PhysxVehicleWheelAPI for {}", prim_path.path);

        let p_drive = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Drive"))).id();
        let p_steer = commands.spawn((PhysicalPort::default(), Name::new("PhysicalPort_Steer"))).id();

        let index = reader.prim_attribute_value::<i32>(&sdf_path, "physxVehicleWheel:index").unwrap_or(0);

        // Immediate or Deferred Wiring
        let fsw_root = q_fsw.iter().find(|(path, _)| {
            path.stage_handle == prim_path.stage_handle && prim_path.path.starts_with(&path.path)
        });

        if fsw_root.is_some() {
            // Can wire immediately
            commands.entity(entity).insert(PendingWheelWiring { index, p_drive, p_steer });
        } else {
            // Defer wiring until root exists
            commands.entity(entity).insert(PendingWheelWiring { index, p_drive, p_steer });
            debug!("Deferred wiring for {}", prim_path.path);
        }

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

        commands.entity(entity).insert((
            wheel,
            RayCaster::new(DVec3::ZERO, Dir3::NEG_Y),
            RayHits::default(),
        ));

        commands.entity(entity)
            .remove::<Collider>()
            .remove::<RigidBody>()
            .remove::<Mass>();
    }
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
