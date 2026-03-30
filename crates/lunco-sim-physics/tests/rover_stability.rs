use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_physics::{spawn_joint_skid_rover, LunCoSimPhysicsPlugin};
use lunco_sim_core::architecture::CommandMessage;
use lunco_sim_fsw::LunCoSimFswPlugin;
use bevy::ecs::system::RunSystemOnce;

#[test]
fn test_rover_long_duration_stop() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    
    // In Bevy 0.18.1, MinimalPlugins is extremely lean.
    // We add the essential simulation core modules.
    // Hierarchy propagation is now part of TransformPlugin in 0.15/0.18.
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::input::InputPlugin);
    
    // Satisfy Avian's potential need for reflection/type registration
    app.init_resource::<AppTypeRegistry>();
    
    app.init_resource::<Assets<Mesh>>();
    app.init_resource::<Assets<StandardMaterial>>();
    
    // THE "SPECIAL PURE CI" MESSAGE INITIALIZATION:
    // Required to satisfy MessageReader<T> validation in external plugins like Avian and FSW.
    app.init_resource::<Messages<bevy::asset::AssetEvent<Mesh>>>();
    app.init_resource::<Messages<bevy::asset::AssetEvent<StandardMaterial>>>();
    app.init_resource::<Messages<bevy::input::mouse::MouseMotion>>();
    app.init_resource::<Messages<bevy::input::mouse::MouseWheel>>();
    app.init_resource::<Messages<bevy::input::keyboard::KeyboardInput>>();
    app.init_resource::<Messages<bevy::input::mouse::MouseButtonInput>>();
    
    // Physics messages from avian3d 0.6.1 in 0.18.1
    // (We use prelude to avoid private module access)
    app.init_resource::<Messages<avian3d::prelude::CollisionStart>>();
    app.init_resource::<Messages<avian3d::prelude::CollisionEnd>>();
    
    app.add_plugins(PhysicsPlugins::default());
    app.add_plugins(LunCoSimPhysicsPlugin);
    app.add_plugins(LunCoSimFswPlugin);

    // 1. Ground Plane
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    // 2. Spawn Rover
    let rover_id = app.world_mut().run_system_once(|mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>| {
        spawn_joint_skid_rover(
            &mut commands,
            &mut meshes,
            &mut materials,
            Handle::<Mesh>::default(),
            Vec3::new(0.0, 1.0, 0.0),
            "TestRover",
            Color::WHITE,
        )
    }).unwrap();

    // Initial updates to settle
    for _ in 0..10 { app.update(); }

    // 3. Drive Forward (-Z Forward)
    app.world_mut().trigger(CommandMessage {
        target: rover_id,
        name: "DRIVE_ROVER".to_string(),
        args: vec![1.0, 0.0],
        source: Entity::PLACEHOLDER,
    });

    // Run for 1 second (approx)
    for _ in 0..60 { app.update(); }

    let pos = app.world().get::<Transform>(rover_id).unwrap().translation;
    let vel = app.world().get::<LinearVelocity>(rover_id).unwrap().0;
    
    println!("Rover Position after 1s: {:?}", pos);
    assert!(pos.y > 0.4, "FAIL: Rover bottomed out! Y must be > 0.4. Current: {}", pos.y);
    assert!(pos.z < -0.1, "FAIL: Rover moved BACKWARD or didn't move! Expecting -Z movement. Current Z: {}", pos.z);
    assert!(vel.length() > 0.5, "FAIL: Rover velocity too low! Expecting > 0.5. Current: {}", vel.length());

    // 4. BRAKE_ROVER (Stateful Toggle)
    app.world_mut().trigger(CommandMessage {
        target: rover_id,
        name: "BRAKE_ROVER".to_string(),
        args: vec![1.0],
        source: Entity::PLACEHOLDER,
    });

    // Run for 3 seconds to ensure full stop
    for _ in 0..180 { app.update(); }

    let pos_stop = app.world().get::<Transform>(rover_id).unwrap().translation;
    let vel_stop = app.world().get::<LinearVelocity>(rover_id).unwrap().0;

    println!("Rover Position after Brake: {:?}", pos_stop);
    println!("Rover Velocity after Brake: {:?}", vel_stop);

    assert!(pos_stop.y > 0.4, "FAIL: Rover bottomed out after braking!");
    assert!(vel_stop.length() < 0.2, "FAIL: Rover braking failed! Velocity > 0.2. Current: {}", vel_stop.length());
    
    // 5. Steering Check (Direction)
    app.world_mut().trigger(CommandMessage {
        target: rover_id,
        name: "DRIVE_ROVER".to_string(),
        args: vec![0.0, 1.0], // Steer Right
        source: Entity::PLACEHOLDER,
    });
    for _ in 0..10 { app.update(); }
    
    // Check local rotation of a child with SteeringActuator or its Hub
    // For Skid rover, we just check if it begins to rotate CW (negative Y rotation)
    let rot = app.world().get::<Transform>(rover_id).unwrap().rotation;
    let (_, yaw, _) = rot.to_euler(EulerRot::YXZ);
    assert!(yaw < 0.0, "FAIL: Rover rotating WRONG DIRECTION (Left instead of Right). Yaw: {}", yaw);
}
