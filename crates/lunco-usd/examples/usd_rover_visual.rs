use bevy::prelude::*;
use lunco_usd::*;
use avian3d::prelude::*;
use lunco_mobility::*;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins,
            PhysicsPlugins::default(),
            lunco_core::LunCoCorePlugin,
            lunco_mobility::LunCoMobilityPlugin,
            UsdPlugins,
        ))
        .insert_resource(lunco_core::TimeWarpState { speed: 1.0, physics_enabled: true })
        .insert_resource(lunco_core::CelestialClock::default())
        .add_systems(Startup, (setup_scene, setup_rover))
        .add_systems(Update, (orbit_camera, debug_rover_presence))
        .run();
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Ground
    commands.spawn((
        Name::new("Ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(2000.0, 2000.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.1, 0.1, 0.1),
            ..default()
        })),
        Collider::half_space(bevy::math::DVec3::Y),
        RigidBody::Static,
    ));

    // Simple Light
    commands.spawn((
        DirectionalLight {
            illuminance: 50000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(100.0, 200.0, 100.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Camera
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(400.0, 200.0, 400.0).looking_at(Vec3::ZERO, Vec3::Y),
        OrbitCamera,
    ));
    
    println!("\n--- Visual Inspection ---");
}

#[derive(Component)]
struct OrbitCamera;

fn orbit_camera(
    time: Res<Time>,
    mut query: Query<&mut Transform, With<OrbitCamera>>,
) {
    if let Ok(mut transform) = query.single_mut() {
        let angle = time.elapsed_secs() * 0.15;
        let distance = 500.0;
        let height = 200.0;
        
        let target = Vec3::ZERO;
        transform.translation.x = target.x + angle.cos() * distance;
        transform.translation.z = target.z + angle.sin() * distance;
        transform.translation.y = height;
        transform.look_at(target, Vec3::Y);
    }
}

fn setup_rover(mut commands: Commands, asset_server: Res<AssetServer>) {
    let stage_handle = asset_server.load("vessels/rovers/rucheyok/rucheyok.usda");
    
    commands.spawn((
        Name::new("RucheyokRover"),
        UsdPrimPath {
            stage_handle,
            path: "/Rucheyok".to_string(),
        },
        Transform::from_xyz(0.0, 100.0, 0.0), // Spawn high
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
        lunco_core::RoverVessel,
    ));
}

fn debug_rover_presence(
    query: Query<(&Name, &UsdPrimPath, Option<&Mesh3d>), Added<UsdPrimPath>>,
) {
    for (name, path, mesh) in query.iter() {
        if mesh.is_some() {
            println!("SUCCESS: Entity '{}' ({}) has a visual mesh attached.", name, path.path);
        } else {
            println!("INFO: Entity '{}' ({}) discovered (no mesh yet).", name, path.path);
        }
    }
}
