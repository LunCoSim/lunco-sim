use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_physics::*;

fn setup_headless_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_once()),
        AssetPlugin::default(),
        TransformPlugin,
        PhysicsPlugins::default(),
        LunCoSimPhysicsPlugin,
    ));
    app.init_resource::<Assets<Mesh>>();
    app.init_resource::<Assets<StandardMaterial>>();
    app.insert_resource(Gravity((Vec3::NEG_Y * 9.81).as_dvec3()));
    
    // Initial update to trigger any internal setup
    app.update();
    
    app
}

fn simulate(app: &mut App, ticks: u32) {
    let delta = std::time::Duration::from_secs_f32(1.0 / 60.0);
    for _ in 0..ticks {
        app.world_mut().resource_mut::<Time>().advance_by(delta);
        app.update();
    }
}

#[test]
fn test_joint_rover_standing_clearance() {
    let mut app = setup_headless_app();

    // Ground at y=0
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    let wheel_mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(Cylinder::new(0.5, 0.4));
    let spawn_pos = Vec3::new(0.0, 2.0, 0.0);
    
    let rover_id = app.world_mut().resource_scope::<Assets<Mesh>, Entity>(|world, mut meshes| {
        world.resource_scope::<Assets<StandardMaterial>, Entity>(|world, mut materials| {
            let mut commands = world.commands();
            spawn_joint_skid_rover(
                &mut commands,
                &mut meshes,
                &mut materials,
                wheel_mesh,
                spawn_pos,
                "TestRover",
                Color::WHITE,
            )
        })
    });

    simulate(&mut app, 600);

    let tf = app.world().get::<Transform>(rover_id).expect("Rover should exist");
    
    let chassis_bottom_y = tf.translation.y - 0.25; 
    println!("Final Rover Y: {}, Chassis Bottom Y: {}", tf.translation.y, chassis_bottom_y);

    assert!(chassis_bottom_y > 0.4, "Rover is too low! Clearance: {}. It might be laying on its belly.", chassis_bottom_y);
}

#[test]
fn test_joint_rover_suspension_travel() {
    let mut app = setup_headless_app();

    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    let wheel_mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(Cylinder::new(0.5, 0.4));
    let spawn_pos = Vec3::new(0.0, 3.0, 0.0); // Start higher
    
    let rover_id = app.world_mut().resource_scope::<Assets<Mesh>, Entity>(|world, mut meshes| {
        world.resource_scope::<Assets<StandardMaterial>, Entity>(|world, mut materials| {
            let mut commands = world.commands();
            spawn_joint_skid_rover(
                &mut commands,
                &mut meshes,
                &mut materials,
                wheel_mesh,
                spawn_pos,
                "TestRover",
                Color::WHITE,
            )
        })
    });

    // 1. Settle
    simulate(&mut app, 600);
    let y1 = app.world().get::<Transform>(rover_id).unwrap().translation.y;

    // 2. Add mass to rover to see it compress
    app.world_mut().entity_mut(rover_id).insert(Mass(5000.0));
    
    simulate(&mut app, 600);
    let y2 = app.world().get::<Transform>(rover_id).unwrap().translation.y;

    println!("Y with 1000kg: {}, Y with 5000kg: {}", y1, y2);
    assert!(y1 > y2, "Suspension did not compress under load! y1: {}, y2: {}", y1, y2);
}
