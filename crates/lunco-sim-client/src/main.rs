use bevy::prelude::*;

mod blueprint_extension;
mod ui;
use blueprint_extension::BlueprintMaterial;
use ui::LunCoSimUiPlugin;

fn main() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(lunco_sim_core::LunCoSimCorePlugin);

    #[cfg(feature = "sandbox")]
    {
        // Sandbox features currently disabled to focus on celestial stabilization
    }

    #[cfg(not(feature = "sandbox"))]
    {
        app.add_plugins(lunco_sim_celestial::CelestialPlugin)
            .insert_resource(ClearColor(Color::BLACK))
            .add_systems(Update, (
                setup_celestial_scenario,
                initialize_camera_focus,
            ).chain());
    }

    app.add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(LunCoSimUiPlugin)
        .add_systems(Update, toggle_slow_motion)
        .run();
}

fn toggle_slow_motion(keyboard: Res<ButtonInput<KeyCode>>, mut time: ResMut<Time<Virtual>>) {
    if keyboard.just_pressed(KeyCode::KeyT) {
        if time.relative_speed() < 1.0 {
            time.set_relative_speed(1.0);
        } else {
            time.set_relative_speed(0.01);
        }
    }
}

#[cfg(not(feature = "sandbox"))]
fn setup_celestial_scenario(
    mut commands: Commands,
    q_solar: Query<Entity, With<lunco_sim_celestial::SolarSystemRoot>>,
    mut spawned: Local<bool>,
) {
    if *spawned { return; }
    let Some(solar_root) = q_solar.iter().next() else { return; };
    *spawned = true;

    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            far: 1.0e11,
            ..default()
        }),
        big_space::prelude::FloatingOrigin,
        big_space::prelude::BigSpatialBundle::default(),
        lunco_sim_celestial::ObserverCamera {
            focus_target: None,
            distance: 50_000_000.0,
        },
        lunco_sim_celestial::ActiveCamera,
        AmbientLight {
            brightness: 1000.0,
            ..default()
        },
        Name::new("Celestial Camera"),
    )).set_parent_in_place(solar_root);
}

#[cfg(not(feature = "sandbox"))]
fn initialize_camera_focus(
    mut commands: Commands,
    mut q_cam: Query<(Entity, &mut lunco_sim_celestial::ObserverCamera), Added<lunco_sim_celestial::ObserverCamera>>,
    q_earth: Query<(Entity, &ChildOf), With<lunco_sim_celestial::EarthRoot>>,
) {
    let Some((cam_entity, mut cam)) = q_cam.iter_mut().next() else { return; };
    let Some((earth, earth_child_of)) = q_earth.iter().next() else { return; };
    cam.focus_target = Some(earth);
    
    // Re-parent camera to Earth's parent grid (EMB Frame) to match ObserverCamera expectations
    commands.entity(cam_entity).set_parent_in_place(earth_child_of.parent());
}
