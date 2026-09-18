use avian3d::prelude::*;
use bevy::prelude::*;
use lunco_usd_bevy_runtime::UsdPlugins;
use lunco_usd_bevy_scene::UsdPrimPath;

#[derive(Resource)]
struct RoverAsset(String);

fn main() {
    let rover_asset = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: usd_rover_visual <asset-path-relative-to-assets>");
        std::process::exit(2);
    });
    let rover_asset = lunco_assets_path::relative_path(&rover_asset)
        .map(|path| lunco_assets_core::engine_asset_uri(&path.to_string_lossy()))
        .unwrap_or_else(|| {
            eprintln!("error: rover asset must be a safe path relative to the asset library");
            std::process::exit(2);
        });
    let assets_root = lunco_assets_core::assets_dir_abs();
    App::new()
        .add_plugins((
            DefaultPlugins.set(AssetPlugin {
                file_path: assets_root.to_string_lossy().into_owned(),
                ..default()
            }),
            PhysicsPlugins::default(),
            lunco_core_runtime::LunCoCoreRuntimePlugin,
            lunco_telemetry_core::LunCoTelemetryCorePlugin,
            lunco_mobility::LunCoMobilityPlugin,
            UsdPlugins,
        ))
        .insert_resource(RoverAsset(rover_asset))
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
            shadow_maps_enabled: false,
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

fn orbit_camera(time: Res<Time>, mut query: Query<&mut Transform, With<OrbitCamera>>) {
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

fn setup_rover(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    rover_asset: Res<RoverAsset>,
) {
    let stage_handle = asset_server.load(rover_asset.0.clone());

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
    ));
}

fn debug_rover_presence(query: Query<(&Name, &UsdPrimPath, Option<&Mesh3d>), Added<UsdPrimPath>>) {
    for (name, path, mesh) in query.iter() {
        if mesh.is_some() {
            println!(
                "SUCCESS: Entity '{}' ({}) has a visual mesh attached.",
                name, path.path
            );
        } else {
            println!(
                "INFO: Entity '{}' ({}) discovered (no mesh yet).",
                name, path.path
            );
        }
    }
}
