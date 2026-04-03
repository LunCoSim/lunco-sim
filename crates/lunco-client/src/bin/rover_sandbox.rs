use bevy::{prelude::*, asset::io::AssetSourceBuilder, math::DVec3};
use avian3d::prelude::*;
use big_space::prelude::CellCoord;

use lunco_core::{Avatar, TimeWarpState};
use lunco_physics::{
    LunCoPhysicsPlugin, 
    spawn_joint_skid_rover, 
    spawn_joint_ackermann_rover,
};
use lunco_rover_raycast::{
    LunCoRoverRaycastPlugin, 
    spawn_raycast_skid_rover, 
    spawn_raycast_ackermann_rover
};
use lunco_controller::LunCoControllerPlugin;
use lunco_avatar::LunCoAvatarPlugin;
use lunco_celestial::{BlueprintMaterial, BlueprintExtension, ObserverCamera, ObserverMode, CelestialClock};

fn main() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(CelestialClock::default())
        // Enable physics by default for sandbox
        .insert_resource(TimeWarpState { physics_enabled: true, ..default() })
        .register_asset_source(
            "cached_textures",
            AssetSourceBuilder::platform_default("../../.cache/textures", None),
        )
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(PhysicsPlugins::default())
        // .add_plugins(PhysicsDebugPlugin::default()) // Enabled for visibility
        .add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(LunCoPhysicsPlugin)
        .add_plugins(LunCoRoverRaycastPlugin)
        .add_plugins(LunCoControllerPlugin)
        .add_plugins(LunCoAvatarPlugin);

    // THE UNIVERSAL SYNC BRIDGE (Crucial for big_space/avian sync)
    app.add_systems(PreUpdate, global_transform_propagation_system);
    app.add_systems(PostUpdate, global_transform_propagation_system.after(PhysicsSystems::Writeback));

    app.add_systems(Startup, setup_sandbox);
    
    app.run();
}

fn setup_sandbox(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut blueprint_materials: ResMut<Assets<BlueprintMaterial>>,
) {
    // 1. Camera & Avatar (Starting in Free-cam Mode)
    commands.spawn((
        Name::new("Sandbox_Avatar"),
        Avatar,
        Camera3d::default(),
        Transform::from_xyz(0.0, 10.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
        ObserverCamera { 
            mode: ObserverMode::Orbital,
            ..default()
        },
    ));

    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_4)),
    ));

    // 2. Blueprint Ground Plane
    let blueprint_mat = blueprint_materials.add(BlueprintMaterial {
        base: StandardMaterial {
            base_color: Color::srgb(0.05, 0.1, 0.2),
            perceptual_roughness: 0.8,
            ..default()
        },
        extension: BlueprintExtension {
            high_color: LinearRgba::new(0.2, 0.4, 0.8, 1.0),
            low_color: LinearRgba::new(0.1, 0.2, 0.4, 1.0),
            grid_scale: 1.0,
            line_width: 1.5,
            subdivisions: Vec2::new(10.0, 10.0),
            ..default()
        },
    });

    commands.spawn((
        Name::new("Blueprint_Ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(200.0, 200.0))),
        MeshMaterial3d(blueprint_mat),
        RigidBody::Static,
        Collider::half_space(DVec3::Y),
    ));

    // 3. Testing Ramp
    let ramp_mesh = meshes.add(Cuboid::new(10.0, 1.0, 20.0));
    let ramp_mat = materials.add(Color::srgb(0.4, 0.4, 0.4));
    commands.spawn((
        Name::new("Ramp"),
        Mesh3d(ramp_mesh),
        MeshMaterial3d(ramp_mat),
        Transform::from_xyz(10.0, 2.0, -10.0).with_rotation(Quat::from_rotation_x(0.15)),
        RigidBody::Static,
        Collider::cuboid(10.0, 1.0, 20.0),
        Friction::new(0.8),
    ));

    // 4. Populate 4 Rovers
    let wheel_mesh = meshes.add(Cylinder::new(0.5, 0.4));
    let root = commands.spawn((Name::new("Rovers_Root"), Transform::default(), Visibility::default())).id();
    
    // Joint Skid
    spawn_joint_skid_rover(
        &mut commands, 
        root, 
        wheel_mesh.clone(), 
        Vec3::new(-10.0, 2.0, 0.0), 
        "Joint_Skid", 
        Color::srgb(0.8, 0.2, 0.2)
    );

    // Joint Ackermann
    spawn_joint_ackermann_rover(
        &mut commands, 
        root, 
        wheel_mesh.clone(), 
        Vec3::new(-10.0, 2.0, 10.0), 
        "Joint_Ackermann", 
        Color::srgb(0.2, 0.8, 0.2)
    );

    // Raycast Skid
    spawn_raycast_skid_rover(
        &mut commands, 
        wheel_mesh.clone(), 
        Vec3::new(10.0, 2.0, 0.0), 
        "Raycast_Skid", 
        Color::srgb(0.2, 0.2, 0.8)
    );

    // Raycast Ackermann
    spawn_raycast_ackermann_rover(
        &mut commands, 
        wheel_mesh.clone(), 
        Vec3::new(10.0, 2.0, 10.0), 
        "Raycast_Ackermann", 
        Color::srgb(0.8, 0.8, 0.2)
    );
}

/// A robust multi-pass system to propagate GlobalTransform & Visibility across grids.
/// Adapted from main.rs to ensure physics/render sync in the sandbox.
fn global_transform_propagation_system(
    mut commands: Commands,
    q_visibility_needs: Query<Entity, (Without<InheritedVisibility>, Or<(With<Visibility>, With<Mesh3d>, With<Text>)>, Without<CellCoord>)>,
    mut q_visibility: Query<(Entity, &mut InheritedVisibility, &mut ViewVisibility, &Visibility, Option<&ChildOf>)>,
) {
    for ent in q_visibility_needs.iter() {
        commands.entity(ent).insert((
            InheritedVisibility::default(),
            ViewVisibility::default(),
        ));
    }

    for _ in 0..4 {
        let mut vis_cache = std::collections::HashMap::new();
        for (ent, inherited, _, _, _) in q_visibility.iter() {
            vis_cache.insert(ent, inherited.get());
        }

        for (_, mut inherited, _view, visibility, child_of_opt) in q_visibility.iter_mut() {
            let parent_visible = if let Some(child_of) = child_of_opt {
                *vis_cache.get(&child_of.parent()).unwrap_or(&true)
            } else {
                true
            };
            
            let is_visible = parent_visible && visibility != Visibility::Hidden;
            if inherited.get() != is_visible {
                *inherited = if is_visible { InheritedVisibility::VISIBLE } else { InheritedVisibility::HIDDEN };
            }
        }
    }
}
