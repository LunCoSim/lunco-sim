use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_usd::UsdPlugins;
use lunco_usd_bevy::{UsdStageResource, UsdPrimPath};
use openusd::usda::TextReader;

fn main() {
    println!("--- LunCo Model Viewer (Truly Recursive) ---");
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(UsdPlugins) // Standard + LunCo specific plugins
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate_camera, model_timer))
        .run();
}

#[derive(Resource)]
struct ModelTimer(Timer);

#[derive(Component)]
struct ViewerCamera;

#[derive(Component)]
struct ModelRoot;

fn setup(
    mut commands: Commands,
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
    
    load_usd_model(&mut commands, "assets/vessels/spacecrafts/orion/orion.usda", "/Orion", true);
    load_usd_model(&mut commands, "assets/vessels/rovers/rucheyok/rucheyok.usda", "/Rucheyok", false);
}

fn load_usd_model(commands: &mut Commands, path: &str, root_prim: &str, visible: bool) {
    info!("Loading USDA model: {}", path);
    match TextReader::read(path) {
        Ok(reader) => {
            let stage_id = commands.spawn((
                Name::new(format!("USD Stage: {}", path)),
                ModelRoot,
                UsdStageResource { reader },
                Transform::default(),
                if visible { Visibility::Visible } else { Visibility::Hidden },
            )).id();

            // Spawn the root Prim
            let root_entity = commands.spawn((
                Name::new(root_prim.to_string()),
                UsdPrimPath {
                    stage_id,
                    path: root_prim.to_string(),
                },
                Transform::default(),
                Visibility::Inherited,
            )).id();
            commands.entity(stage_id).add_child(root_entity);
        }
        Err(e) => {
            error!("CRITICAL: Failed to load USDA {}: {}", path, e);
        }
    }
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
