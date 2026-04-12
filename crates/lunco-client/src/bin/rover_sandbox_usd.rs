//! A standalone sandbox for rapid testing of ground mobility and physics.
//!
//! Loads the entire scene from USD **synchronously** during Startup,
//! so all entities (rover chassis + wheels) exist before physics runs.
//! This matches the original rover_sandbox behavior exactly.

use bevy::prelude::*;
use bevy::asset::AssetPlugin;
use bevy::pbr::wireframe::WireframePlugin;
use bevy_egui::EguiPlugin;
use big_space::prelude::*;
use avian3d::prelude::PhysicsPlugins;
use leafwing_input_manager::prelude::*;

use lunco_mobility::LunCoMobilityPlugin;
use lunco_usd::{UsdPlugins, UsdPrimPath, UsdStageAsset};
use lunco_usd_bevy::sync_usd_visuals;
use lunco_sandbox_edit::{SandboxEditPlugin, ui::SandboxEditUiPlugin};
use lunco_ui::LuncoUiPlugin;
use lunco_controller::LunCoControllerPlugin;
use lunco_avatar::{LunCoAvatarPlugin, IntentAnalogState, FreeFlightCamera, AdaptiveNearPlane};
use lunco_celestial::{BlueprintMaterial, BlueprintExtension, GravityPlugin, EmbeddedAssetsPlugin, BlueprintShaderPlugin};
use lunco_core::Avatar;
use big_space::prelude::Grid;

/// Marker for the sandbox scene entity.
#[derive(Component)]
struct SandboxScene;

mod center_spacer;

/// Marker applied to entities whose material has been swapped to BlueprintMaterial.
#[derive(Component)]
struct BlueprintMaterialApplied;

fn main() {
    App::new()
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(lunco_core::TimeWarpState { physics_enabled: true, ..default() })
        .insert_resource(avian3d::prelude::Gravity(bevy::math::DVec3::NEG_Y * 9.81))
        .insert_resource(lunco_celestial::Gravity::flat(9.81, bevy::math::DVec3::NEG_Y))
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: std::env::current_dir().unwrap_or_default().join("assets").to_string_lossy().to_string(),
            ..default()
        }).build().disable::<TransformPlugin>())
        .add_plugins(BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>())
        // Diagnostics disabled for cleaner output - uncomment to debug performance
        // .add_plugins(LogDiagnosticsPlugin::default())
        // .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(WireframePlugin::default())
        .add_plugins(EguiPlugin::default())
        .add_plugins(PhysicsPlugins::default().set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all()))
        .add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(lunco_core::LunCoCorePlugin)
        // EmbeddedAssetsPlugin is no-op on desktop, handles shaders/textures/missions on wasm32
        .add_plugins(EmbeddedAssetsPlugin)
        // Register blueprint shader on desktop (wasm32 handled by EmbeddedAssetsPlugin)
        .add_plugins(BlueprintShaderPlugin)
        .add_plugins(GravityPlugin)
        .add_plugins(LunCoMobilityPlugin)
        .add_plugins(UsdPlugins)
        .add_plugins(SandboxEditPlugin)
        .add_plugins(LuncoUiPlugin)
        .add_plugins(bevy_workbench::WorkbenchPlugin::default())
        .add_plugins(SandboxEditUiPlugin)
        .add_plugins(center_spacer::CenterSpacerPlugin)
        .add_plugins(LunCoControllerPlugin)
        .add_plugins(LunCoAvatarPlugin)
        .init_resource::<SandboxSettings>()
        .add_systems(Startup, setup_sandbox)
        .add_systems(Update, (apply_sandbox_settings, apply_blueprint_to_usd_terrain.after(sync_usd_visuals)))
        .add_systems(Update, apply_blueprint_grid_settings)
        // Selection must run before avatar possession so DragModeActive flag is set
        .add_systems(Update, lunco_sandbox_edit::selection::handle_entity_selection.before(lunco_avatar::avatar_raycast_possession))
        .add_systems(PreUpdate, global_transform_propagation_system)
        .add_systems(PostUpdate, (
            global_transform_propagation_system,
            camera_render_propagation_system,
            spawn_fallback_avatar,
        ).chain().after(avian3d::prelude::PhysicsSystems::Writeback))
        .run();
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

#[derive(Resource)]
struct BlueprintGridSettings {
    material_handle: Handle<BlueprintMaterial>,
    major_spacing: f32,
    minor_spacing: f32,
    major_width: f32,
    minor_width: f32,
    minor_fade: f32,
    dirty: bool,
}

impl Default for BlueprintGridSettings {
    fn default() -> Self {
        Self {
            material_handle: Handle::default(),
            major_spacing: 1.0,
            minor_spacing: 0.5,
            major_width: 1.0,
            minor_width: 0.5,
            minor_fade: 0.15,
            dirty: true,
        }
    }
}

fn setup_sandbox(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut blueprint_materials: ResMut<Assets<BlueprintMaterial>>,
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

    let blueprint_mat = BlueprintExtension {
        high_color: LinearRgba::new(0.5, 0.5, 0.5, 1.0),
        low_color: LinearRgba::new(0.1, 0.1, 0.1, 1.0),
        high_line_color: LinearRgba::new(0.18, 0.18, 0.18, 1.0),
        low_line_color: LinearRgba::new(0.18, 0.18, 0.18, 1.0),
        surface_color: LinearRgba::new(0.15, 0.15, 0.18, 1.0),
        grid_scale: 1.0,
        line_width: 2.0,
        subdivisions: Vec2::new(10.0, 10.0),
        transition: 0.85,
        major_grid_spacing: 1.0,
        minor_grid_spacing: 0.5,
        major_line_width: 1.0,
        minor_line_width: 0.5,
        minor_line_fade: 0.15,
        ..default()
    };
    let blueprint_mat_handle = blueprint_materials.add(BlueprintMaterial {
        base: StandardMaterial {
            base_color: Color::srgb(0.2, 0.2, 0.2),
            perceptual_roughness: 0.9,
            ..default()
        },
        extension: blueprint_mat,
    });

    commands.insert_resource(BlueprintGridSettings {
        material_handle: blueprint_mat_handle.clone(),
        ..default()
    });

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
        SandboxScene,
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

/// Applies BlueprintMaterial to USD terrain entities (Ground and Ramp).
fn apply_blueprint_to_usd_terrain(
    mut commands: Commands,
    q_all_meshes: Query<(Entity, &Name, &UsdPrimPath), (With<Mesh3d>, Without<BlueprintMaterialApplied>)>,
    q_scene: Query<Entity, With<SandboxScene>>,
    stages: Res<Assets<UsdStageAsset>>,
    mut materials: ResMut<Assets<BlueprintMaterial>>,
) {
    if q_scene.is_empty() { return; }

    for (ent, name, prim_path) in q_all_meshes.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue };
        let Ok(sdf_path) = openusd::sdf::Path::new(&prim_path.path) else { continue };
        let reader = (*stage.reader).clone();

        let mat_type: Option<String> = reader.prim_attribute_value(&sdf_path, "lunco:material");
        if mat_type.as_deref() != Some("BlueprintGrid") { continue; }

        let surface_color = reader.prim_attribute_value::<Vec<f64>>(&sdf_path, "lunco:gridSurfaceColor")
            .unwrap_or_else(|| vec![0.2, 0.2, 0.2]);
        let major_spacing = reader.prim_attribute_value::<f64>(&sdf_path, "lunco:gridMajorSpacing")
            .unwrap_or(1.0) as f32;
        let minor_spacing = reader.prim_attribute_value::<f64>(&sdf_path, "lunco:gridMinorSpacing")
            .unwrap_or(0.5) as f32;
        let major_width = reader.prim_attribute_value::<f64>(&sdf_path, "lunco:gridMajorWidth")
            .unwrap_or(1.0) as f32;
        let minor_width = reader.prim_attribute_value::<f64>(&sdf_path, "lunco:gridMinorWidth")
            .unwrap_or(0.5) as f32;
        let minor_fade = reader.prim_attribute_value::<f64>(&sdf_path, "lunco:gridMinorFade")
            .unwrap_or(0.15) as f32;

        let r = surface_color.get(0).copied().unwrap_or(0.2) as f32;
        let g = surface_color.get(1).copied().unwrap_or(0.2) as f32;
        let b = surface_color.get(2).copied().unwrap_or(0.2) as f32;

        let bp_ext = BlueprintExtension {
            high_color: LinearRgba::new(0.5, 0.5, 0.5, 1.0),
            low_color: LinearRgba::new(0.1, 0.1, 0.1, 1.0),
            high_line_color: LinearRgba::new(r + 0.05, g + 0.05, b + 0.05, 1.0),
            low_line_color: LinearRgba::new(r + 0.05, g + 0.05, b + 0.05, 1.0),
            surface_color: LinearRgba::new(r, g, b, 1.0),
            grid_scale: 1.0,
            line_width: 2.0,
            subdivisions: Vec2::new(10.0, 10.0),
            transition: 0.85,
            major_grid_spacing: major_spacing,
            minor_grid_spacing: minor_spacing,
            major_line_width: major_width,
            minor_line_width: minor_width,
            minor_line_fade: minor_fade,
            ..Default::default()
        };
        let bp_mat = BlueprintMaterial {
            base: StandardMaterial {
                base_color: Color::srgb(r, g, b),
                perceptual_roughness: 0.9,
                ..default()
            },
            extension: bp_ext,
        };
        let mat_handle = materials.add(bp_mat);
        commands.entity(ent)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert((MeshMaterial3d(mat_handle), BlueprintMaterialApplied));
        info!("Applied BlueprintMaterial to {}", name.as_str());
    }
}

fn apply_blueprint_grid_settings(
    mut grid_settings: ResMut<BlueprintGridSettings>,
    mut materials: ResMut<Assets<BlueprintMaterial>>,
) {
    if grid_settings.dirty {
        grid_settings.dirty = false;
        if let Some(mat) = materials.get_mut(&grid_settings.material_handle) {
            mat.extension.major_grid_spacing = grid_settings.major_spacing;
            mat.extension.minor_grid_spacing = grid_settings.minor_spacing;
            mat.extension.major_line_width = grid_settings.major_width;
            mat.extension.minor_line_width = grid_settings.minor_width;
            mat.extension.minor_line_fade = grid_settings.minor_fade;
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
            let parent_visible = if let Some(child_of) = child_of_opt { *vis_cache.get(&child_of.parent()).unwrap_or(&true) } else { true };
            let is_visible = parent_visible && visibility != Visibility::Hidden;
            *inherited = if is_visible { InheritedVisibility::VISIBLE } else { InheritedVisibility::HIDDEN };
        }
    }
}
