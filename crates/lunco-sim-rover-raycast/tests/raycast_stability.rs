use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use avian3d::prelude::*;
use lunco_sim_rover_raycast::*;
use lunco_sim_physics::LunCoSimPhysicsPlugin;

fn setup_headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::log::LogPlugin::default());
    app.add_plugins(TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::diagnostic::DiagnosticsPlugin);

    app.add_plugins(PhysicsPlugins::default());
    app.add_plugins(LunCoSimPhysicsPlugin);
    app.add_plugins(LunCoSimRoverRaycastPlugin);

    app.insert_resource(Gravity((Vec3::NEG_Y * 9.81).as_dvec3()));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    
    app.finish();
    app.cleanup();

    app.update();
    app.add_systems(Update, check_resources);
    app
}

fn check_resources(world: &World) {
    use avian3d::prelude::*;
    use avian3d::collider_tree::ColliderTrees;
    use avian3d::collision::narrow_phase::NarrowPhaseConfig;
    use avian3d::dynamics::solver::SolverConfig;
    use avian3d::dynamics::solver::SolverDiagnostics;
    use avian3d::collision::CollisionDiagnostics;
    use avian3d::spatial_query::SpatialQueryDiagnostics;
    
    println!("--- Resource Check ---");
    println!("Gravity: {}", world.contains_resource::<Gravity>());
    println!("Time<Physics>: {}", world.contains_resource::<Time<Physics>>());
    println!("Time<Substeps>: {}", world.contains_resource::<Time<Substeps>>());
    println!("SolverConfig: {}", world.contains_resource::<SolverConfig>());
    println!("NarrowPhaseConfig: {}", world.contains_resource::<NarrowPhaseConfig>());
    println!("ColliderTrees: {}", world.contains_resource::<ColliderTrees>());
    println!("SolverDiagnostics: {}", world.contains_resource::<SolverDiagnostics>());
    println!("CollisionDiagnostics: {}", world.contains_resource::<CollisionDiagnostics>());
    println!("SpatialQueryDiagnostics: {}", world.contains_resource::<SpatialQueryDiagnostics>());
    println!("----------------------");
}

#[test]
fn test_raycast_rover_idle_stability() {
    let mut app = setup_headless_app();

    // Ground
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    let spawn_pos = Vec3::new(0.0, 1.0, 0.0);
    
    let rover_id = {
        let mut commands = app.world_mut().commands();
        spawn_raycast_skid_rover(
            &mut commands,
            Handle::default(),
            spawn_pos,
            "TestRaycast",
            Color::WHITE,
        )
    };

    for _ in 0..120 {
        app.update();
    }

    let mut q_rover = app.world_mut().query::<&Transform>();
    let tf = q_rover.get(app.world_mut(), rover_id).expect("Rover should exist");
    let last_y = tf.translation.y;
    
    println!("Final Raycast Rover Y: {}", last_y);
    
    assert!(last_y < 2.0, "Raycast rover jumping! Y={}", last_y);
    assert!((last_y - 0.4).abs() < 0.6, "Raycast rover not at stable height: {}", last_y);
}
