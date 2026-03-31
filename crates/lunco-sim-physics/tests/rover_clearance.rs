use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_physics::*;
use bevy::time::TimeUpdateStrategy;

fn setup_headless_app() -> App {
    println!("App Heartbeat Running");
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::log::LogPlugin::default());
    app.add_plugins(bevy::diagnostic::DiagnosticsPlugin);

    app.add_plugins(PhysicsPlugins::default());
    app.add_plugins(LunCoSimPhysicsPlugin);
    
    app.insert_resource(Gravity((Vec3::NEG_Y * 9.81).as_dvec3()));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    
    app.finish();
    app.cleanup();

    app.update();
    app
}

fn simulate(app: &mut App, ticks: u32) {
    for _ in 0..ticks {
        app.update();
    }
}

#[test]
fn test_joint_rover_standing_clearance() {
    println!("Test Heartbeat: Standing Clearance Starting");
    let mut app = setup_headless_app();

    // Ground at y=0
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    let spawn_pos = Vec3::new(0.0, 2.0, 0.0);
    
    let rover_id = {
        let mut commands = app.world_mut().commands();
        spawn_joint_skid_rover(
            &mut commands,
            Handle::default(),
            spawn_pos,
            "TestRover",
            Color::WHITE,
        )
    };

    simulate(&mut app, 600);

    let tf = app.world().get::<Transform>(rover_id).expect("Rover should exist");
    
    // Chassis is 0.5m high, middle is at y. Bottom is y - 0.25.
    let chassis_bottom_y = tf.translation.y - 0.25; 
    println!("Rover Y: {}, Bottom Y: {}", tf.translation.y, chassis_bottom_y);
    
    assert!(chassis_bottom_y > 0.1, "Rover chassis is scraping the ground! Bottom Y: {}", chassis_bottom_y);
    assert!(tf.translation.y < 1.0, "Rover is flying! Y: {}", tf.translation.y);
}

#[test]
fn test_joint_rover_suspension_travel() {
    let mut app = setup_headless_app();

    // Ground
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    let spawn_pos = Vec3::new(0.0, 3.0, 0.0);
    
    let rover_id = {
        let mut commands = app.world_mut().commands();
        spawn_joint_skid_rover(
            &mut commands,
            Handle::default(),
            spawn_pos,
            "TestRover",
            Color::WHITE,
        )
    };

    // 1. Measure height with 1000kg
    simulate(&mut app, 300);
    let y1 = app.world().get::<Transform>(rover_id).unwrap().translation.y;
    
    // 2. Increase mass to 5000kg
    app.world_mut().entity_mut(rover_id).insert(Mass(5000.0));
    simulate(&mut app, 300);
    let y2 = app.world().get::<Transform>(rover_id).unwrap().translation.y;
    
    println!("Y with 1000kg: {}, Y with 5000kg: {}", y1, y2);
    
    assert!(y2 < y1 - 0.01, "Suspension did not compress under load! y1: {}, y2: {}", y1, y2);
}
