use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_rover_raycast::*;
use lunco_sim_physics::LunCoSimPhysicsPlugin;

#[test]
fn test_raycast_rover_idle_stability() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        PhysicsPlugins::default(),
        LunCoSimPhysicsPlugin,
        LunCoSimRoverRaycastPlugin,
    ));
    app.init_resource::<Assets<Mesh>>();
    app.init_resource::<Assets<StandardMaterial>>();

    // Ground
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    // Assets
    let wheel_mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(Cylinder::new(0.4, 0.4));
    
    let spawn_pos = Vec3::new(0.0, 1.0, 0.0);
    
    let rover_id = app.world_mut().resource_scope::<Assets<Mesh>, Entity>(|world, mut meshes| {
        world.resource_scope::<Assets<StandardMaterial>, Entity>(|world, mut materials| {
            let mut commands = world.commands();
            spawn_raycast_skid_rover(
                &mut commands,
                &mut meshes,
                &mut materials,
                wheel_mesh,
                spawn_pos,
                "TestRaycast",
                Color::WHITE,
            )
        })
    });

    app.update();

    let mut last_y = 1.0;
    for _ in 0..120 {
        app.update();
        let mut q_rover = app.world_mut().query::<&Transform>();
        let current_y = q_rover.get(app.world_mut(), rover_id).unwrap().translation.y;
        
        assert!(current_y < 2.0, "Raycast rover jumping! Y={}", current_y);
        
        last_y = current_y;
    }

    assert!((last_y - 0.4).abs() < 0.6, "Raycast rover not at stable height: {}", last_y);
}
