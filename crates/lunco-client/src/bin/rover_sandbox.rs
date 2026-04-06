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
use leafwing_input_manager::prelude::MouseMove;

use lunco_mobility::{LunCoMobilityPlugin, Suspension};
use lunco_robotics::{LunCoRoboticsPlugin, rover};
use lunco_controller::LunCoControllerPlugin;
use lunco_avatar::{LunCoAvatarPlugin, IntentAnalogState, ObserverBehavior, ObserverMode, AdaptiveNearPlane};
use lunco_celestial::{BlueprintMaterial, BlueprintExtension};
use lunco_core::{Vessel, architecture::CommandMessage};

fn main() {
    App::new()
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(lunco_core::TimeWarpState { physics_enabled: true, ..default() })
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>())
        .add_plugins(LogDiagnosticsPlugin::default())
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(WireframePlugin::default())
        .add_plugins(EguiPlugin::default())
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(LunCoMobilityPlugin)
        .add_plugins(LunCoRoboticsPlugin)
        .add_plugins(LunCoControllerPlugin)
        .add_plugins(LunCoAvatarPlugin)
        .init_resource::<SandboxSettings>()
        .add_systems(Startup, setup_sandbox)
        .add_systems(Update, apply_sandbox_settings)
        .add_systems(PreUpdate, global_transform_propagation_system)
        .configure_sets(PostUpdate, lunco_avatar::AvatarCameraSet.after(avian3d::prelude::PhysicsSystems::Writeback))
        .add_systems(PostUpdate, global_transform_propagation_system.after(lunco_avatar::AvatarCameraSet))
        .add_systems(EguiPrimaryContextPass, sandbox_ui_system)
        .run();
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

    let blueprint_mat = blueprint_materials.add(BlueprintMaterial {
        base: StandardMaterial {
            base_color: Color::srgb(0.2, 0.2, 0.2), 
            perceptual_roughness: 0.9,
            ..default()
        },
        extension: BlueprintExtension {
            high_color: LinearRgba::new(0.5, 0.5, 0.5, 1.0),
            low_color: LinearRgba::new(0.1, 0.1, 0.1, 1.0),
            grid_scale: 1.0,
            line_width: 2.0,
            subdivisions: Vec2::new(10.0, 10.0),
            transition: 0.0, 
            ..default()
        },
    });

    commands.spawn((
        Name::new("Blueprint_Ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(2000.0, 2000.0))),
        MeshMaterial3d(blueprint_mat),
        RigidBody::Static,
        Collider::cuboid(2000.0, 0.1, 2000.0),
        CellCoord::default(),
        Visibility::default(),
    )).set_parent_in_place(grid);

    let ramp_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.7, 0.7, 0.7),
        ..default()
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

    let joint_skid = rover::spawn_joint_rover(
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
    commands.spawn((
        Camera3d::default(),
        ObserverBehavior {
            target: Some(rovers_root),
            mode: ObserverMode::Flyby,
            flyby_offset: bevy::math::DVec3::new(-30.0, 15.0, -20.0),
            yaw: std::f32::consts::PI * 0.8,
            pitch: -0.3,
            distance: 20.0,
            ..default()
        },
        AdaptiveNearPlane,
        Transform::default(),
        GlobalTransform::default(),
        FloatingOrigin,
        CellCoord::default(),
        lunco_core::Avatar,
        IntentAnalogState::default(),
        leafwing_input_manager::prelude::InputMap::<lunco_avatar::UserIntent>::default()
            .with_dual_axis(lunco_avatar::UserIntent::Look, MouseMove::default())
            .with(lunco_avatar::UserIntent::MoveForward, KeyCode::KeyW)
            .with(lunco_avatar::UserIntent::MoveBackward, KeyCode::KeyS)
            .with(lunco_avatar::UserIntent::MoveLeft, KeyCode::KeyA)
            .with(lunco_avatar::UserIntent::MoveRight, KeyCode::KeyD)
            .with(lunco_avatar::UserIntent::MoveUp, KeyCode::KeyE)
            .with(lunco_avatar::UserIntent::MoveDown, KeyCode::KeyQ)
            .with(lunco_avatar::UserIntent::Pause, KeyCode::Space),
    )).set_parent_in_place(grid);
}

fn sandbox_ui_system(
    mut contexts: EguiContexts,
    mut settings: ResMut<SandboxSettings>,
    q_camera: Query<(&Transform, &CellCoord, &ObserverBehavior)>,
    q_vessels: Query<(Entity, &Name, &Vessel)>,
    q_children: Query<&Children>,
    mut q_suspension: Query<(Entity, &mut Suspension)>,
    mut commands: Commands,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    egui::Window::new("Sandbox Control").show(ctx, |ui| {
        ui.heading("Environment");
        ui.add(egui::Slider::new(&mut settings.sun_yaw, 0.0..=6.28).text("Sun Yaw"));
        ui.add(egui::Slider::new(&mut settings.sun_pitch, -3.14..=0.0).text("Sun Pitch"));
        ui.add(egui::Slider::new(&mut settings.ambient_brightness, 0.0..=1000.0).text("Ambient"));
        ui.checkbox(&mut settings.wireframe, "Show Wireframe");

        ui.separator();
        ui.heading("Avatar Telemetry");
        if let Some((tf, cell, obs)) = q_camera.iter().next() {
            ui.label(format!("Position (BigSpace)\nCell: {:?}\nLocal: {:.2?}", cell, tf.translation));
            let (yaw, pitch, _): (f32, f32, f32) = tf.rotation.to_euler(EulerRot::YXZ);
            ui.label(format!("Orientation\nYaw: {:.2}°\nPitch: {:.2}°", yaw.to_degrees(), pitch.to_degrees()));
            ui.label(format!("Mode: {:?}", obs.mode));
        }

        ui.separator();
        ui.heading("Vessels");
        for (entity, name, _) in q_vessels.iter() {
            ui.collapsing(format!("{}", name), |ui| {
                if ui.button("Possess").clicked() {
                    let avatar = commands.spawn_empty().id(); // Placeholder approach
                    commands.trigger(CommandMessage {
                        id: 0,
                        target: entity,
                        name: "POSSESS".to_string(),
                        args: Default::default(),
                        source: avatar,
                    });
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
