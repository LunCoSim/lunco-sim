//! A standalone sandbox for rapid testing of ground mobility and physics.
//!
//! This binary bypasses the full celestial ephemeris system to provide a 
//! stable, flat-ground environment for debugging rovers, actuators, and FSW.

use bevy::prelude::*;
use bevy::diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin};
use bevy::pbr::wireframe::WireframePlugin;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use big_space::prelude::*;
use avian3d::prelude::{RigidBody, Collider, Friction, PhysicsPlugins};
use leafwing_input_manager::prelude::*;

use lunco_mobility::{LunCoMobilityPlugin, Suspension};
use lunco_robotics::{LunCoRoboticsPlugin, rover};
use lunco_controller::LunCoControllerPlugin;
use lunco_avatar::{LunCoAvatarPlugin, IntentAnalogState, FreeFlightCamera, SpringArmCamera, OrbitCamera, AdaptiveNearPlane, CameraScroll};
use lunco_celestial::{BlueprintMaterial, BlueprintExtension};
use lunco_core::{Vessel, architecture::CommandMessage};

fn main() {
    App::new()
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(lunco_core::TimeWarpState { physics_enabled: true, ..default() })
        .insert_resource(avian3d::prelude::Gravity(bevy::math::DVec3::NEG_Y * 9.81))
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>())
        .add_plugins(LogDiagnosticsPlugin::default())
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(WireframePlugin::default())
        .add_plugins(EguiPlugin::default())
        .add_plugins(PhysicsPlugins::default().set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all()))
        .add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(LunCoMobilityPlugin)
        .add_plugins(LunCoRoboticsPlugin)
        .add_plugins(LunCoControllerPlugin)
        .add_plugins(LunCoAvatarPlugin)
        .init_resource::<SandboxSettings>()
        .add_systems(Startup, setup_sandbox)
        .add_systems(Update, apply_sandbox_settings)
        .add_systems(Update, apply_blueprint_grid_settings)
        .add_systems(PreUpdate, global_transform_propagation_system)
        .add_systems(PostUpdate, (
            global_transform_propagation_system,
            camera_render_propagation_system,
        ).chain().after(avian3d::prelude::PhysicsSystems::Writeback))
        .add_systems(EguiPrimaryContextPass, sandbox_ui_system)
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

/// Tunable blueprint grid parameters exposed in the sandbox UI.
#[derive(Resource)]
struct BlueprintGridSettings {
    /// Entity holding the BlueprintMaterial asset.
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
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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

    commands.spawn((
        Name::new("Blueprint_Ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(2000.0, 2000.0))),
        MeshMaterial3d(blueprint_mat_handle),
        RigidBody::Static,
        Collider::cuboid(2000.0, 0.1, 2000.0),
        CellCoord::default(),
        Visibility::default(),
    )).set_parent_in_place(grid);

    let ramp_mat = blueprint_materials.add(BlueprintMaterial {
        base: StandardMaterial {
            base_color: Color::srgb(0.25, 0.22, 0.18),
            perceptual_roughness: 0.9,
            ..default()
        },
        extension: BlueprintExtension {
            surface_color: LinearRgba::new(0.25, 0.22, 0.18, 1.0),
            high_line_color: LinearRgba::new(0.2, 0.18, 0.15, 1.0),
            low_line_color: LinearRgba::new(0.2, 0.18, 0.15, 1.0),
            major_grid_spacing: 1.0,
            minor_grid_spacing: 0.5,
            major_line_width: 1.0,
            minor_line_width: 0.5,
            minor_line_fade: 0.15,
            transition: 0.85,
            line_width: 2.0,
            subdivisions: Vec2::new(10.0, 10.0),
            ..default()
        },
    });
    commands.spawn((
        Name::new("Ramp"),
        Mesh3d(meshes.add(Cuboid::new(30.0, 1.0, 40.0))),
        MeshMaterial3d(ramp_mat),
        Transform::from_xyz(25.0, 4.0, 0.0).with_rotation(Quat::from_rotation_z(0.3)),
        RigidBody::Static,
        Collider::cuboid(30.0, 1.0, 40.0),
        Friction::new(1.0),
        CellCoord::default(),
        Visibility::default(),
    )).set_parent_in_place(grid);

    // Spawn a light source
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

    // Spawn a root for rovers
    let rovers_root = commands.spawn((
        Transform::from_xyz(0.0, 1.0, 0.0),
        GlobalTransform::default(),
        CellCoord::default(),
        Visibility::default(),
        Name::new("Rovers Root"),
    )).set_parent_in_place(grid).id();

    let _joint_skid = rover::spawn_joint_rover(
        &mut commands,
        &mut meshes,
        &mut materials,
        rovers_root,
        Vec3::new(-15.0, 5.0, -10.0),
        "Joint_Skid",
        Color::srgb(0.8, 0.2, 0.2),
        rover::SteeringType::Skid,
    );

    rover::spawn_joint_rover(
        &mut commands,
        &mut meshes,
        &mut materials,
        rovers_root,
        Vec3::new(-15.0, 5.0, 10.0),
        "Joint_Ackermann",
        Color::srgb(0.2, 0.8, 0.2),
        rover::SteeringType::Ackermann,
    );

    let r_skid = rover::spawn_raycast_rover(
        &mut commands,
        &mut meshes,
        &mut materials,
        Vec3::new(15.0, 5.0, -10.0),
        "Raycast_Skid",
        Color::srgb(0.2, 0.2, 0.8),
        rover::SteeringType::Skid,
    );
    commands.entity(r_skid).set_parent_in_place(grid);

    let r_ack = rover::spawn_raycast_rover(
        &mut commands,
        &mut meshes,
        &mut materials,
        Vec3::new(15.0, 5.0, 10.0),
        "Raycast_Ackermann",
        Color::srgb(0.8, 0.8, 0.2),
        rover::SteeringType::Ackermann,
    );
    commands.entity(r_ack).set_parent_in_place(grid);

    // Initialize the Avatar (Camera)
    // ActionState<UserIntent> is REQUIRED alongside InputMap for leafwing to process input.
    // Using the centralized input map from lunco_controller ensures Zoom is mapped.
    commands.spawn((
        Camera3d::default(),
        FreeFlightCamera {
            yaw: std::f32::consts::PI * 0.8,
            pitch: -0.3,
            damping: None,
        },
        AdaptiveNearPlane,
        Transform::from_translation(bevy::math::Vec3::new(-30.0, 15.0, -20.0)),
        GlobalTransform::default(),
        FloatingOrigin,
        CellCoord::default(),
        lunco_core::Avatar,
        IntentAnalogState::default(),
        ActionState::<lunco_core::UserIntent>::default(),
        lunco_controller::get_avatar_input_map(),
    )).set_parent_in_place(grid);
}

fn sandbox_ui_system(
    mut contexts: EguiContexts,
    mut settings: ResMut<SandboxSettings>,
    mut sens: ResMut<lunco_avatar::CameraScrollSensitivity>,
    mut grid_settings: ResMut<BlueprintGridSettings>,
    q_camera: Query<(Entity, &Transform, &CellCoord), With<lunco_core::Avatar>>,
    q_camera_spring: Query<&SpringArmCamera, With<lunco_core::Avatar>>,
    q_camera_orbit: Query<&OrbitCamera, With<lunco_core::Avatar>>,
    q_camera_ff: Query<&FreeFlightCamera, With<lunco_core::Avatar>>,
    q_vessels: Query<(Entity, &Name, &Vessel)>,
    q_children: Query<&Children>,
    mut q_suspension: Query<(Entity, &mut Suspension)>,
    mut commands: Commands,
    mut scroll_res: ResMut<CameraScroll>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    // Forward scroll input to the camera zoom resource when not hovering UI.
    if !ctx.is_pointer_over_area() {
        scroll_res.delta += ctx.input(|i| i.raw_scroll_delta.y);
    }

    egui::Window::new("Sandbox Control").show(ctx, |ui| {
        ui.heading("Environment");
        ui.add(egui::Slider::new(&mut settings.sun_yaw, 0.0..=6.28).text("Sun Yaw"));
        ui.add(egui::Slider::new(&mut settings.sun_pitch, -3.14..=0.0).text("Sun Pitch"));
        ui.add(egui::Slider::new(&mut settings.ambient_brightness, 0.0..=1000.0).text("Ambient"));
        ui.checkbox(&mut settings.wireframe, "Show Wireframe");

        ui.separator();
        ui.heading("Camera");
        ui.add(egui::Slider::new(&mut sens.value, 0.1..=5.0).text("Scroll Sensitivity (m)"));

        ui.separator();
        ui.heading("Grid");
        if ui.add(egui::Slider::new(&mut grid_settings.major_spacing, 0.1..=10.0).text("Major (m)")).changed() { grid_settings.dirty = true; }
        if ui.add(egui::Slider::new(&mut grid_settings.minor_spacing, 0.01..=1.0).text("Minor (m)")).changed() { grid_settings.dirty = true; }
        if ui.add(egui::Slider::new(&mut grid_settings.major_width, 0.5..=5.0).text("Major Width")).changed() { grid_settings.dirty = true; }
        if ui.add(egui::Slider::new(&mut grid_settings.minor_width, 0.1..=2.0).text("Minor Width")).changed() { grid_settings.dirty = true; }
        if ui.add(egui::Slider::new(&mut grid_settings.minor_fade, 0.0..=1.0).text("Minor Opacity")).changed() { grid_settings.dirty = true; }

        ui.separator();
        ui.heading("Avatar Telemetry");
        if let Some((_, tf, cell)) = q_camera.iter().next() {
            ui.label(format!("Position (BigSpace)\nCell: {:?}\nLocal: {:.2?}", cell, tf.translation));
            let (yaw, pitch, _): (f32, f32, f32) = tf.rotation.to_euler(EulerRot::YXZ);
            ui.label(format!("Orientation\nYaw: {:.2}°\nPitch: {:.2}°", yaw.to_degrees(), pitch.to_degrees()));
            // Display active behavior.
            let mode_str = if let Ok(arm) = q_camera_spring.single() {
                format!("SPRING ARM (dist: {:.1} m)", arm.distance)
            } else if let Ok(orbit) = q_camera_orbit.single() {
                format!("ORBIT (dist: {:.1} m)", orbit.distance)
            } else if q_camera_ff.single().is_ok() {
                "FREE FLIGHT".to_string()
            } else {
                "TRANSITION".to_string()
            };
            ui.label(format!("Mode: {}", mode_str));
        }

        ui.separator();
        ui.heading("Vessels");
        // Get the real avatar entity for POSSESS commands.
        let avatar_ent = q_camera.iter().next().map(|(e, _, _)| e);
        for (entity, name, _) in q_vessels.iter() {
            ui.collapsing(format!("{}", name), |ui| {
                if ui.button("Possess").clicked() {
                    if let Some(avatar) = avatar_ent {
                        commands.trigger(CommandMessage {
                            id: 0,
                            target: entity,
                            name: "POSSESS".to_string(),
                            args: Default::default(),
                            source: avatar,
                        });
                    }
                }
                ui.label("Mechanical Inspector");
                inspect_suspension_recursive(ui, entity, &q_children, &mut q_suspension);
            });
        }
    });
}

fn inspect_suspension_recursive(ui: &mut egui::Ui, entity: Entity, q_children: &Query<&Children>, q_suspension: &mut Query<(Entity, &mut Suspension)>) {
    if let Ok((_e, mut susp)) = q_suspension.get_mut(entity) {
        ui.label(format!("Hub: {:?}", entity));
        ui.add(egui::Slider::new(&mut susp.rest_length, 0.1..=2.0).text("Rest Length"));
        ui.add(egui::Slider::new(&mut susp.spring_k, 1000.0..=100000.0).text("Spring K"));
        ui.add(egui::Slider::new(&mut susp.damping_c, 100.0..=10000.0).text("Damping C"));
        ui.separator();
    }
    if let Ok(children) = q_children.get(entity) {
        for child in children.iter() {
            inspect_suspension_recursive(ui, child, q_children, q_suspension);
        }
    }
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
