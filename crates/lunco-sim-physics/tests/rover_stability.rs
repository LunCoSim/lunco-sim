use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_physics::{spawn_joint_skid_rover, LunCoSimPhysicsPlugin};
use lunco_sim_core::architecture::CommandMessage;
use lunco_sim_fsw::LunCoSimFswPlugin;
use bevy::ecs::system::RunSystemOnce;

#[test]
fn test_rover_long_duration_stop() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.build()
        .disable::<bevy::winit::WinitPlugin>()
        .disable::<bevy::render::RenderPlugin>());
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
    assert!(pos.z < -0.1, "Rover should have moved forward (-Z)");
    assert!(vel.length() > 0.1, "Rover should have non-zero velocity");

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

    assert!(vel_stop.length() < 0.1, "Rover should be stopped (velocity < 0.1)");
    
    // Check if position is stable
    let pos_stop_before = pos_stop;
    for _ in 0..60 { app.update(); }
    let pos_stop_after = app.world().get::<Transform>(rover_id).unwrap().translation;
    assert!((pos_stop_before - pos_stop_after).length() < 0.01, "Rover should be stationary");
}
