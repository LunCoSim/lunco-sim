use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_usd::{UsdPlugins, UsdStageAsset, UsdPrimPath};

fn main() {
    println!("--- LunCo Model Viewer (Composition Refactored) ---");
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: "../../assets".to_string(),
            ..default()
        }))
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(UsdPlugins)
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate_camera, model_timer))
        .run();
}

#[derive(Resource)]
struct ModelTimer(Timer);

#[derive(Resource)]
struct LoadedStages {
    orion: Handle<UsdStageAsset>,
    rucheyok: Handle<UsdStageAsset>,
}

#[derive(Component)]
struct ViewerCamera;

#[derive(Component)]
struct ModelRoot;

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
        brightness: 0.5,
        ..default()
    });

    // Camera
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(-25.0, 15.0, 35.0).looking_at(Vec3::ZERO, Vec3::Y),
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

    commands.insert_resource(ModelTimer(Timer::from_seconds(5.0, TimerMode::Repeating)));
    
    // LOAD MISSIONS
    let orion_handle = asset_server.load("vessels/spacecrafts/orion/orion.usda");
    let rucheyok_handle = asset_server.load("vessels/rovers/rucheyok/rucheyok.usda");

    commands.insert_resource(LoadedStages {
        orion: orion_handle.clone(),
        rucheyok: rucheyok_handle.clone(),
    });

    commands.spawn((
        Name::new("Orion Root"),
        ModelRoot,
        UsdPrimPath {
            stage_handle: orion_handle,
            path: "/".to_string(),
        },
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
        Transform::default(),
    ));

    commands.spawn((
        Name::new("Rucheyok Root"),
        ModelRoot,
        UsdPrimPath {
            stage_handle: rucheyok_handle,
            path: "/".to_string(),
        },
        Visibility::Hidden,
        InheritedVisibility::default(),
        ViewVisibility::default(),
        Transform::default(),
    ));
}

fn rotate_camera(time: Res<Time>, mut query: Query<&mut Transform, With<ViewerCamera>>) {
    for mut transform in query.iter_mut() {
        let rotation = Quat::from_rotation_y(0.2 * time.delta_secs());
        let pos = transform.translation;
        transform.translation = rotation * pos;
        transform.look_at(Vec3::ZERO, Vec3::Y);
    }
}

fn model_timer(
    time: Res<Time>,
    mut timer: ResMut<ModelTimer>,
    mut query: Query<(&mut Visibility, &Name), With<ModelRoot>>,
) {
    if timer.0.tick(time.delta()).just_finished() {
        for (mut visibility, name) in query.iter_mut() {
            *visibility = match *visibility {
                Visibility::Visible => Visibility::Hidden,
                _ => Visibility::Visible,
            };
            if *visibility == Visibility::Visible {
                info!("SWITCH: Now showing {}", name);
            }
        }
    }
}
