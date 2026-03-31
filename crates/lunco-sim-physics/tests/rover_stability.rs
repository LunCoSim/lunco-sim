use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_physics::*;
use bevy::time::TimeUpdateStrategy;

fn setup_headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::log::LogPlugin::default());
    app.add_plugins(TransformPlugin);
    app.add_plugins(AssetPlugin::default());
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

#[test]
fn test_joint_rover_idle_stability() {
    let mut app = setup_headless_app();

    let spawn_pos = Vec3::new(0.0, 1.0, 0.0);
    
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

    for _ in 0..120 {
        app.update();
    }

    let mut q_rover = app.world_mut().query::<(&Transform, &LinearVelocity)>();
    let (tf, vel) = q_rover.get(app.world_mut(), rover_id).expect("Rover should exist");

    println!("Final Rover Pos: {:?}, Velocity: {:?}", tf.translation, vel.0);

    assert!(vel.0.length() < 0.5, "Rover should be relatively stationary: vel={:?}", vel.0);
    assert!(tf.translation.y > 0.3, "Rover chassis dropped too low: {}", tf.translation.y);

    let rover_pos = tf.translation;
    let mut q_wheels = app.world_mut().query_filtered::<&GlobalTransform, With<MotorActuator>>();
    let mut wheel_count = 0;
    for wheel_tf in q_wheels.iter(app.world_mut()) {
        wheel_count += 1;
        let dist = wheel_tf.translation().distance(rover_pos);
        assert!(dist < 4.0, "Wheel detached! Distance: {}", dist);
    }
    assert_eq!(wheel_count, 4, "Should have 4 wheels attached");
}
