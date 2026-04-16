//! A standalone sandbox for rapid testing of ground mobility and physics.
//!
//! Loads the entire scene from USD **synchronously** during Startup,
//! so all entities (rover chassis + wheels) exist before physics runs.
//! This matches the original rover_sandbox behavior exactly.

use bevy::prelude::*;
use bevy::asset::AssetPlugin;
use bevy::pbr::wireframe::WireframePlugin;
use big_space::prelude::*;
use avian3d::prelude::PhysicsPlugins;
use leafwing_input_manager::prelude::*;

use lunco_mobility::LunCoMobilityPlugin;
use lunco_usd::{UsdPlugins, UsdPrimPath};
use lunco_sandbox_edit::SandboxEditPlugin;
use lunco_controller::LunCoControllerPlugin;
use lunco_avatar::{LunCoAvatarPlugin, IntentAnalogState, FreeFlightCamera, AdaptiveNearPlane};
use lunco_celestial::GravityPlugin;
use lunco_environment::EnvironmentPlugin;
use lunco_core::Avatar;
use lunco_cosim::CoSimPlugin;
use lunco_cosim::systems::propagate::CosimSet as PropagateCosimSet;
use lunco_cosim::systems::apply_forces::CosimSet as ApplyForcesCosimSet;
use lunco_modelica::{ModelicaPlugin, ModelicaSet};
use big_space::prelude::Grid;
use lunco_materials::{BlueprintMaterialPlugin, SolarPanelMaterialPlugin};

#[path = "../center_spacer.rs"]
mod center_spacer;
#[path = "../balloon_setup.rs"]
mod balloon_setup;
#[path = "../python_balloon_setup.rs"]
mod python_balloon_setup;

/// Parse API port from CLI args.
/// 
/// Supports:
fn main() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(lunco_core::TimeWarpState { physics_enabled: true, ..default() })
        .insert_resource(avian3d::prelude::Gravity::ZERO)
        .insert_resource(lunco_celestial::Gravity::flat(9.81, bevy::math::DVec3::NEG_Y))
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: std::env::current_dir().unwrap_or_default().join("assets").to_string_lossy().to_string(),
            ..default()
        }).build().disable::<TransformPlugin>())
        .add_plugins(BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>())
        .add_plugins(WireframePlugin::default())
        .add_plugins(PhysicsPlugins::default().set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all()))
        .add_plugins(CoSimPlugin)
        .add_plugins(bevy_workbench::WorkbenchPlugin {
            config: bevy_workbench::WorkbenchConfig {
                show_menu_bar: false,    // No menu bar
                show_toolbar: false,     // No play/stop toolbar
                enable_game_view: false, // GameView disabled — CenterSpacer provides viewport
                show_console: false,     // No console panel
                ..default()
            },
        })
        .add_plugins(ModelicaPlugin)
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(GravityPlugin)
        .add_plugins(EnvironmentPlugin)
        .add_plugins(LunCoMobilityPlugin)
        .add_plugins(UsdPlugins)
        .add_plugins(SandboxEditPlugin)
        .add_plugins(lunco_sandbox_edit::ui::SandboxEditUiPlugin)
        .add_plugins(center_spacer::CenterSpacerPlugin)
        .add_plugins(LunCoControllerPlugin)
        .add_plugins(LunCoAvatarPlugin)
        .add_plugins(BlueprintMaterialPlugin)
        .add_plugins(SolarPanelMaterialPlugin)
        .add_plugins(lunco_scripting::LunCoScriptingPlugin)
        .init_resource::<SandboxSettings>()
        .add_systems(Startup, setup_sandbox)
        .add_systems(Update, apply_sandbox_settings)
        // One-shot setup systems stay in Update (fire only on Added<BalloonModelMarker>)
        .add_systems(Update, balloon_setup::compile_balloon_model)
        .add_systems(Update, balloon_setup::setup_balloon_wires)
        .add_systems(Update, python_balloon_setup::setup_python_balloon)
        // Per-tick sync systems run in FixedUpdate, ordered within the cosim pipeline:
        //   HandleResponses → sync_outputs → Propagate → ApplyForces → sync_inputs → SpawnRequests
        .configure_sets(FixedUpdate, (
            ModelicaSet::HandleResponses,
            PropagateCosimSet::Propagate,
            ApplyForcesCosimSet::ApplyForces,
            ModelicaSet::SpawnRequests,
        ).chain())
        .add_systems(FixedUpdate,
            balloon_setup::sync_modelica_outputs
                .after(ModelicaSet::HandleResponses)
                .before(PropagateCosimSet::Propagate))
        .add_systems(FixedUpdate,
            python_balloon_setup::sync_script_outputs
                .after(ModelicaSet::HandleResponses)
                .before(PropagateCosimSet::Propagate))
        .add_systems(FixedUpdate,
            balloon_setup::sync_inputs_to_modelica
                .after(ApplyForcesCosimSet::ApplyForces)
                .before(ModelicaSet::SpawnRequests))
        .add_systems(FixedUpdate,
            python_balloon_setup::sync_inputs_to_script
                .after(ApplyForcesCosimSet::ApplyForces)
                .before(ModelicaSet::SpawnRequests))
        // Selection must run before avatar possession so DragModeActive flag is set
        .add_systems(Update, lunco_sandbox_edit::selection::handle_entity_selection.before(lunco_avatar::avatar_raycast_possession))
        .add_systems(PreUpdate, global_transform_propagation_system)
        .add_systems(PostUpdate, (
            global_transform_propagation_system,
            camera_render_propagation_system,
            spawn_fallback_avatar,
        ).chain().after(avian3d::prelude::PhysicsSystems::Writeback))
        .add_plugins(lunco_api::LunCoApiPlugin::default());

    app.run();
}

fn camera_render_propagation_system(
    commands: Commands,
    q_needs: Query<Entity, (Or<(With<Visibility>, With<Mesh3d>, With<Text2d>, With<Transform>)>, Without<InheritedVisibility>, Without<CellCoord>)>,
    q_spatial: Query<(Entity, &mut GlobalTransform, &Transform, Option<&ChildOf>)>,
    q_visibility: Query<(Entity, &mut InheritedVisibility, &mut ViewVisibility, &Visibility, Option<&ChildOf>)>,
) {
    global_transform_propagation_system(commands, q_needs, q_spatial, q_visibility);
}

#[derive(Resource, Reflect)]
struct SandboxSettings {
    sun_yaw: f32,
    sun_pitch: f32,
    ambient_brightness: f32,
    ambient_color: LinearRgba,
    wireframe: bool,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            sun_yaw: 0.5,
            sun_pitch: -0.8,
            ambient_brightness: 400.0,
            ambient_color: LinearRgba::WHITE,
            wireframe: false,
        }
    }
}

fn setup_sandbox(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let big_space_root = commands.spawn(BigSpace::default()).id();
    let grid = commands.spawn((
        Grid::new(2000.0, 1.0e10),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Sandbox_Grid"),
    )).set_parent_in_place(big_space_root).id();

    // --- Sun (directional light) ---
    commands.spawn((
        DirectionalLight {
            illuminance: 10000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(10.0, 20.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        GlobalTransform::default(),
        CellCoord::default(),
        Name::new("Sun"),
    )).set_parent_in_place(grid);

    // --- Load scene from USD (ground + ramp + ALL rovers) ---
    // The scene file references rover definitions from external .usda files
    // with position overrides. The UsdComposer flattens everything into
    // a single stage, then sync_usd_visuals spawns entities for all prims.
    let scene_handle = asset_server.load("scenes/sandbox/sandbox_scene.usda");
    info!("Loading sandbox scene from USD");
    commands.spawn((
        Name::new("SandboxScene"),
        UsdPrimPath {
            stage_handle: scene_handle,
            path: "/SandboxScene".to_string(),
        },
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
        Transform::default(),
        CellCoord::default(),
    )).set_parent_in_place(grid);

    // --- Spawn Balloons ---
    // Red Balloon (Modelica)
    commands.spawn((
        Name::new("Red Balloon (Modelica)"),
        lunco_core::SelectableRoot,
        Transform::from_xyz(10.0, 5.0, 0.0),
        avian3d::prelude::RigidBody::Dynamic,
        avian3d::prelude::Collider::sphere(1.0),
        avian3d::prelude::Mass(4.5),
        Mesh3d(meshes.add(Sphere::new(1.0).mesh().ico(16).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.9, 0.2, 0.2),
            ..default()
        })),
        ChildOf(grid),
        lunco_sandbox_edit::catalog::BalloonModelMarker::default(),
    ));

    // Green Balloon (Python)
    commands.spawn((
        Name::new("Green Balloon (Python)"),
        lunco_core::SelectableRoot,
        Transform::from_xyz(-10.0, 5.0, 0.0),
        avian3d::prelude::RigidBody::Dynamic,
        avian3d::prelude::Collider::sphere(1.0),
        avian3d::prelude::Mass(4.5),
        Mesh3d(meshes.add(Sphere::new(1.0).mesh().ico(16).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.2, 0.9, 0.2),
            ..default()
        })),
        ChildOf(grid),
        lunco_sandbox_edit::catalog::PythonBalloonMarker::default(),
    ));
}

/// Spawns a default avatar if no USD-defined Avatar was loaded.
///
/// This acts as a fallback when the scene file doesn't contain an Avatar prim,
/// ensuring the user always has a controllable camera.
fn spawn_fallback_avatar(
    q_cameras: Query<Entity, With<Camera3d>>,
    q_grids: Query<Entity, With<Grid>>,
    mut commands: Commands,
    mut done: Local<bool>,
) {
    if *done { return; }
    // Check if ANY camera already exists (USD avatar or fallback)
    if q_cameras.iter().next().is_some() {
        *done = true;
        return;
    }
    let Some(grid) = q_grids.iter().next() else { return; };

    info!("No camera found, spawning fallback FreeFlightCamera");
    commands.spawn((
        Camera3d::default(),
        FreeFlightCamera {
            yaw: std::f32::consts::PI * 0.8,
            pitch: -0.3,
            damping: None,
        },
        AdaptiveNearPlane,
        Transform::from_translation(Vec3::new(-30.0, 15.0, -20.0)),
        GlobalTransform::default(),
        FloatingOrigin,
        CellCoord::default(),
        Avatar,
        IntentAnalogState::default(),
        ActionState::<lunco_core::UserIntent>::default(),
        lunco_controller::get_avatar_input_map(),
        ChildOf(grid),
    ));
    *done = true;
}


fn apply_sandbox_settings(
    settings: Res<SandboxSettings>,
    mut q_sun: Query<&mut Transform, With<DirectionalLight>>,
    mut q_ambient: Query<&mut AmbientLight>,
) {
    if settings.is_changed() {
        for mut tf in q_sun.iter_mut() {
            tf.rotation = Quat::from_euler(EulerRot::YXZ, settings.sun_yaw, settings.sun_pitch, 0.0);
        }
        for mut ambient in q_ambient.iter_mut() {
            ambient.brightness = settings.ambient_brightness;
            ambient.color = Color::Srgba(settings.ambient_color.into());
        }
    }
}

fn global_transform_propagation_system(
    mut commands: Commands,
    q_needs: Query<Entity, (Or<(With<Visibility>, With<Mesh3d>, With<Text2d>, With<Transform>)>, Without<InheritedVisibility>, Without<CellCoord>)>,
    mut q_spatial: Query<(Entity, &mut GlobalTransform, &Transform, Option<&ChildOf>)>,
    mut q_visibility: Query<(Entity, &mut InheritedVisibility, &mut ViewVisibility, &Visibility, Option<&ChildOf>)>,
) {
    for ent in q_needs.iter() {
        commands.entity(ent).insert((InheritedVisibility::default(), ViewVisibility::default(), GlobalTransform::default()));
    }
    for _ in 0..4 {
        let mut gtf_cache = std::collections::HashMap::new();
        for (ent, gtf, _, _) in q_spatial.iter() { gtf_cache.insert(ent, *gtf); }
        for (_ent, mut gtf, local_tf, child_of_opt) in q_spatial.iter_mut() {
            let parent_gtf = if let Some(child_of) = child_of_opt { gtf_cache.get(&child_of.parent()).cloned().unwrap_or_default() } else { GlobalTransform::default() };
            *gtf = parent_gtf.mul_transform(*local_tf);
        }
    }
    for _ in 0..4 {
        let mut vis_cache = std::collections::HashMap::new();
        for (ent, inherited, _, _, _) in q_visibility.iter() { vis_cache.insert(ent, inherited.get()); }
        for (_, mut inherited, _view, visibility, child_of_opt) in q_visibility.iter_mut() {
            // If entity is explicitly Visible, it's always visible regardless of parent
            if *visibility == Visibility::Visible {
                *inherited = InheritedVisibility::VISIBLE;
                continue;
            }
            // If entity is explicitly Hidden, it's always hidden
            if *visibility == Visibility::Hidden {
                *inherited = InheritedVisibility::HIDDEN;
                continue;
            }
            // Otherwise inherit from parent
            let parent_visible = if let Some(child_of) = child_of_opt { *vis_cache.get(&child_of.parent()).unwrap_or(&true) } else { true };
            *inherited = if parent_visible { InheritedVisibility::VISIBLE } else { InheritedVisibility::HIDDEN };
        }
    }
}
