use bevy::prelude::*;
use bevy::math::{DVec3, DQuat};
use avian3d::prelude::*;
use big_space::prelude::CellCoord;
use std::collections::HashMap;

use lunco_core::{Vessel, RoverVessel};
use lunco_fsw::FlightSoftware;
use lunco_mobility::{Suspension, WheelRaycast, DifferentialDrive, AckermannSteer};
use lunco_hardware::{MotorActuator, BrakeActuator};

use crate::assembler::{spawn_digital_port, spawn_physical_port, connect_ports};

#[derive(PartialEq, Eq, Copy, Clone)]
pub enum SteeringType { Skid, Ackermann }

/// Spawns a Raycast-based rover using the new modular logic.
pub fn spawn_raycast_rover(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
    steering_type: SteeringType,
) -> Entity {
    let chassis_width = 2.0_f64;
    let chassis_height = 0.5_f64;
    let chassis_length = 3.5_f64;

    let mut rover_builder = commands.spawn((
        Name::new(name.to_string()),
        RoverVessel,
        Vessel,
        Transform::from_translation(spawn_pos),
        CellCoord::default(),
        RigidBody::Dynamic,
        Collider::cuboid(chassis_width, chassis_height, chassis_length),
        Mass(1000.0),
        Mesh3d(meshes.add(Cuboid::new(chassis_width as f32, chassis_height as f32, chassis_length as f32))),
        MeshMaterial3d(materials.add(StandardMaterial::from(color))),
    ));

    if steering_type == SteeringType::Ackermann {
        rover_builder.insert(AckermannSteer {
            drive_left_port: "drive_left".to_string(),
            drive_right_port: "drive_right".to_string(),
            steer_port: "steering".to_string(),
            max_steer_angle: 0.5,
        });
    } else {
        rover_builder.insert(DifferentialDrive {
            left_port: "drive_left".to_string(),
            right_port: "drive_right".to_string(),
        });
    }

    let rover_entity = rover_builder.id();

    // Flight Software Setup
    let drive_l_digital = spawn_digital_port(commands, rover_entity, "drive_l_reg");
    let drive_r_digital = spawn_digital_port(commands, rover_entity, "drive_r_reg");
    let steer_digital = spawn_digital_port(commands, rover_entity, "steer_reg");
    let brake_digital = spawn_digital_port(commands, rover_entity, "brake_reg");

    let mut port_map = HashMap::new();
    port_map.insert("drive_left".to_string(), drive_l_digital);
    port_map.insert("drive_right".to_string(), drive_r_digital);
    port_map.insert("steering".to_string(), steer_digital);
    port_map.insert("brake".to_string(), brake_digital);
    commands.entity(rover_entity).insert(FlightSoftware { port_map, brake_active: false });

    let wheel_configs = [
        ("FR", Vec3::new((chassis_width * 0.6) as f32, -0.4, (chassis_length * 0.4) as f32), false, true),
        ("FL", Vec3::new((-chassis_width * 0.6) as f32, -0.4, (chassis_length * 0.4) as f32), true, true),
        ("RR", Vec3::new((chassis_width * 0.6) as f32, -0.4, (-chassis_length * 0.4) as f32), false, false),
        ("RL", Vec3::new((-chassis_width * 0.6) as f32, -0.4, (-chassis_length * 0.4) as f32), true, false),
    ];

    let wheel_rot = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let wheel_mesh = meshes.add(Cylinder::new(0.4, 0.4));

    for (label, rel_pos, is_left, is_front) in wheel_configs {
        let drive_port = spawn_physical_port(commands, rover_entity, &format!("port_{}_drive", label));
        let steer_port = spawn_physical_port(commands, rover_entity, &format!("port_{}_steer", label));
        let brake_port = spawn_physical_port(commands, rover_entity, &format!("port_{}_brake", label));

        let digital_source = if is_left { drive_l_digital } else { drive_r_digital };
        connect_ports(commands, rover_entity, digital_source, drive_port, 1.0);
        connect_ports(commands, rover_entity, brake_digital, brake_port, 1.0);
        
        if is_front && steering_type == SteeringType::Ackermann {
            connect_ports(commands, rover_entity, steer_digital, steer_port, 1.0);
        }

        let visual_wheel = commands.spawn((
            Name::new(format!("{}_visual", label)),
            Transform::from_translation(rel_pos).with_rotation(wheel_rot), 
            CellCoord::default(),
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(materials.add(StandardMaterial::from(if is_front { Color::from(Srgba::RED) } else { Color::from(Srgba::BLUE) }))),
            ChildOf(rover_entity),
        )).id();

        commands.spawn((
            Name::new(format!("{}_{}", name, label)),
            WheelRaycast {
                suspension_port: Entity::PLACEHOLDER,
                drive_port,
                steer_port: if is_front { steer_port } else { Entity::PLACEHOLDER },
                visual_entity: Some(visual_wheel),
                ..default()
            },
            RayCaster::new(Vec3::new(0.0, 0.5, 0.0).as_dvec3(), Dir3::NEG_Y)
                .with_max_distance(1.2)
                .with_solidness(true)
                .with_query_filter(SpatialQueryFilter::from_excluded_entities([rover_entity])),
            Transform::from_translation(rel_pos),
            CellCoord::default(),
            ChildOf(rover_entity),
        ));
    }

    rover_entity
}

/// Spawns a Joint-based rover using the new modular logic.
pub fn spawn_joint_rover(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    parent: Entity,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
    steering_type: SteeringType,
) -> Entity {
    let chassis_width = 1.8_f64;
    let chassis_height = 0.5_f64;
    let chassis_length = 3.0_f64;
    let wheel_radius = 0.5_f64;
    let wheel_width = 0.4_f64;
    let suspension_travel = 0.3_f64;

    let mut rover_builder = commands.spawn((
        Name::new(name.to_string()),
        RoverVessel,
        Vessel,
        Transform::from_translation(spawn_pos),
        CellCoord::default(),
        RigidBody::Dynamic,
        Collider::cuboid(chassis_width, chassis_height, chassis_length),
        Friction::new(0.5),
        Mass(1000.0), 
        CenterOfMass(Vec3::new(0.0, -0.2, 0.0)),
        LinearDamping(0.2), 
        AngularDamping(0.5),
        Mesh3d(meshes.add(Cuboid::new(chassis_width as f32, chassis_height as f32, chassis_length as f32))),
        MeshMaterial3d(materials.add(StandardMaterial::from(color))),
    ));

    if steering_type == SteeringType::Ackermann {
        rover_builder.insert(AckermannSteer {
            drive_left_port: "drive_left".to_string(),
            drive_right_port: "drive_right".to_string(),
            steer_port: "steering".to_string(),
            max_steer_angle: 0.6,
        });
    } else {
        rover_builder.insert(DifferentialDrive {
            left_port: "drive_left".to_string(),
            right_port: "drive_right".to_string(),
        });
    }

    let rover_entity = rover_builder.id();
    commands.entity(parent).add_child(rover_entity);

    // FSW Setup
    let drive_l_digital = spawn_digital_port(commands, rover_entity, "drive_l_reg");
    let drive_r_digital = spawn_digital_port(commands, rover_entity, "drive_r_reg");
    let steer_digital = spawn_digital_port(commands, rover_entity, "steer_reg");
    let brake_digital = spawn_digital_port(commands, rover_entity, "brake_reg");

    let mut port_map = HashMap::new();
    port_map.insert("drive_left".to_string(), drive_l_digital);
    port_map.insert("drive_right".to_string(), drive_r_digital);
    port_map.insert("steering".to_string(), steer_digital);
    port_map.insert("brake".to_string(), brake_digital);
    commands.entity(rover_entity).insert(FlightSoftware { port_map, brake_active: false });

    let wheel_configs = [
        ("fl", Vec3::new(-1.2, -0.6, 1.2), true, true), 
        ("rl", Vec3::new(-1.2, -0.6, -1.2), true, false), 
        ("fr", Vec3::new(1.2, -0.6, 1.2), false, true),
        ("rr", Vec3::new(1.2, -0.6, -1.2), false, false),
    ];

    let steer_port = spawn_physical_port(commands, rover_entity, "port_steer");
    connect_ports(commands, rover_entity, steer_digital, steer_port, 10.0);

    let wheel_tilt = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let wheel_tilt_d = DQuat::from_xyzw(wheel_tilt.x as f64, wheel_tilt.y as f64, wheel_tilt.z as f64, wheel_tilt.w as f64);
    let wheel_mesh = meshes.add(Cylinder::new(wheel_radius as f32, wheel_width as f32));

    for (label, rel_pos, is_left, is_front) in wheel_configs {
        let digital_source = if is_left { drive_l_digital } else { drive_r_digital };
        let motor_port = spawn_physical_port(commands, rover_entity, &format!("port_{}_drive", label));
        let brake_port = spawn_physical_port(commands, rover_entity, &format!("port_{}_brake", label));
        
        connect_ports(commands, rover_entity, digital_source, motor_port, 200.0);
        connect_ports(commands, rover_entity, brake_digital, brake_port, 1.0);

        let wheel_entity = commands.spawn((
            Name::new(format!("{}_wheel_{}", name, label)),
            Transform::from_translation(spawn_pos + rel_pos).with_rotation(wheel_tilt),
            RigidBody::Dynamic,
            Collider::cylinder(wheel_radius, wheel_width),
            Friction::new(1.0), 
            Mass(20.0), 
            LinearDamping(0.5), 
            AngularDamping(2.0),
            CellCoord::default(),
            MotorActuator { port_entity: motor_port, axis: DVec3::Y },
            BrakeActuator { port_entity: brake_port, max_force: 32767.0 },
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(materials.add(StandardMaterial::from(if is_front { Color::from(Srgba::RED) } else { Color::from(Srgba::BLUE) }))),
        )).id();
        commands.entity(parent).add_child(wheel_entity);

        let hub_entity = commands.spawn((
            Name::new(format!("{}_hub_{}", name, label)),
            RigidBody::Dynamic, 
            Mass(10.0), 
            Collider::sphere(0.05),
            CollisionLayers::from_bits(0, 0), // Hubs shouldn't collide with anything
            Transform::from_translation(spawn_pos + rel_pos),
            CellCoord::default(),
            ChildOf(rover_entity),
        )).id();

        commands.spawn((
            PrismaticJoint::new(rover_entity, hub_entity)
                .with_local_anchor1(rel_pos.as_dvec3())
                .with_local_anchor2(DVec3::ZERO)
                .with_slider_axis(DVec3::Y)
                .with_limits(-suspension_travel, suspension_travel),
            Suspension {
                local_axis: DVec3::Y,
                ..default()
            },
            CellCoord::default(),
            ChildOf(rover_entity),
        ));
        
        if is_front && steering_type == SteeringType::Ackermann {
            let steering_hub = commands.spawn((
                Name::new(format!("{}_steer_hub_{}", name, label)),
                RigidBody::Dynamic, 
                Mass(5.0), 
                Collider::sphere(0.04),
                CollisionLayers::from_bits(0, 0),
                Transform::from_translation(spawn_pos + rel_pos),
                CellCoord::default(),
                MotorActuator { port_entity: steer_port, axis: DVec3::Y },
                ChildOf(rover_entity),
            )).id();

            commands.spawn((
                RevoluteJoint::new(hub_entity, steering_hub)
                    .with_hinge_axis(DVec3::Y)
                    .with_angle_limits(-0.6, 0.6),
                CellCoord::default(),
                ChildOf(rover_entity),
            ));
                
            commands.spawn((
                RevoluteJoint::new(steering_hub, wheel_entity)
                    .with_hinge_axis(DVec3::X)
                    .with_local_basis2(wheel_tilt_d.inverse()),
                CellCoord::default(),
                ChildOf(rover_entity),
            ));
        } else {
            commands.spawn((
                RevoluteJoint::new(hub_entity, wheel_entity)
                    .with_hinge_axis(DVec3::X)
                    .with_local_basis2(wheel_tilt_d.inverse()),
                CellCoord::default(),
                ChildOf(rover_entity),
            ));
        }
    }
    rover_entity
}
