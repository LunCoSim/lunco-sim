use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_physics::*;

fn setup_headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.build()
        .disable::<bevy::render::RenderPlugin>()
        .disable::<bevy::winit::WinitPlugin>()
        .disable::<bevy::audio::AudioPlugin>()
    );
    app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_once());
    
    app.add_plugins(LunCoSimPhysicsPlugin);
    
    // assets still need to exist as resources for resource_scope
    app.init_resource::<Assets<Mesh>>();
    app.init_resource::<Assets<StandardMaterial>>();
    
    // Initialize the Shader asset type to avoid the panic when handles are created
    app.init_asset::<Shader>();
    
    app.insert_resource(Gravity((Vec3::NEG_Y * 9.81).as_dvec3()));
    
    app.update();
    app
}

#[test]
fn test_joint_rover_idle_stability() {
    let mut app = setup_headless_app();

    let spawn_pos = Vec3::new(0.0, 1.0, 0.0);
    
    let rover_id = app.world_mut().resource_scope::<Assets<Mesh>, Entity>(|world, mut meshes| {
        world.resource_scope::<Assets<StandardMaterial>, Entity>(|world, mut materials| {
            let mut commands = world.commands();
            spawn_joint_skid_rover(
                &mut commands,
                &mut meshes,
                &mut materials,
                Handle::default(),
                spawn_pos,
                "TestRover",
                Color::WHITE,
            )
        })
    });

    let delta = std::time::Duration::from_secs_f32(1.0 / 60.0);
    for _ in 0..120 {
        app.world_mut().resource_mut::<Time>().advance_by(delta);
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
