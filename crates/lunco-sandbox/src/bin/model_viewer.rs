use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
use lunco_usd::{UsdPlugins, UsdPrimPath};

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: "../../assets".to_string(),
            ..default()
        }))
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Gravity(DVec3::ZERO))
        .add_plugins(UsdPlugins)
        .add_systems(Startup, setup)
        .add_systems(Update, rotate_camera)
        .add_plugins(lunco_api::LunCoApiPlugin::default());

    app.run();
}

#[derive(Component)]
struct ViewerCamera;

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Light
    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(10.0, 20.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn(AmbientLight {
        brightness: 1.0,
        ..default()
    });

    // Camera
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(-30.0, 20.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
        ViewerCamera,
    ));

    // Grid
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(100.0, 100.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.1, 0.1, 0.1),
            ..default()
        })),
        Transform::from_xyz(0.0, -10.0, 0.0),
    ));

    // Rover
    let rucheyok_handle = asset_server.load("vessels/rovers/rucheyok/rucheyok.usda");

    commands.spawn((
        Name::new("Rucheyok Root"),
        UsdPrimPath {
            stage_handle: rucheyok_handle,
            path: "/".to_string(),
        },
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
        Transform::default(),
    ));
}

fn rotate_camera(time: Res<Time>, mut query: Query<&mut Transform, With<ViewerCamera>>) {
    for mut transform in query.iter_mut() {
        let rotation = Quat::from_rotation_y(0.15 * time.delta_secs());
        let pos = transform.translation;
        transform.translation = rotation * pos;
        transform.look_at(Vec3::ZERO, Vec3::Y);
    }
}
