use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::*;
use bevy::camera::visibility::NoFrustumCulling;
use crate::registry::{CelestialBodyRegistry, CelestialReferenceFrame, CelestialBody};
use crate::gravity::{GravityProvider, PointMassGravity};
use crate::soi::SOI;

#[derive(Component)]
pub struct SolarSystemRoot;

#[derive(Component)]
pub struct EMBRoot;

#[derive(Component)]
pub struct EarthRoot;

#[derive(Component)]
pub struct MoonRoot;

pub fn setup_big_space_hierarchy(
    mut commands: Commands,
    registry: Res<CelestialBodyRegistry>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut blueprint_materials: ResMut<Assets<crate::blueprint::BlueprintMaterial>>,
    asset_server: Res<AssetServer>,
) {
    // 1. Minimalist BigSpace Root (No Name, No standard spatial components)
    let big_space_root = commands.spawn(BigSpace::default()).id();

    // 2. Solar System Grid Anchor
    let solar_grid = commands.spawn((
        SolarSystemRoot,
        CelestialReferenceFrame { ephemeris_id: 10 }, 
        Grid::new(1.0e9, 1.0e30), 
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Universe Grid (Solar)"),
    )).set_parent_in_place(big_space_root).id();

    // All subsequent bodies/grids follow as children of solar_grid...
    // The Sun Body
    let _sun_body = commands.spawn((
        SolarSystemRoot, 
        CelestialBody { 
            name: "Sun".to_string(), 
            ephemeris_id: 10,
            radius_m: 696_340.0e3,
        },
        SOI { radius_m: 1.0e13 }, 
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::Visible, // Prevent f32 frustum culling bugs at scale
        InheritedVisibility::default(),
        Mesh3d(meshes.add(Sphere::new(696_340.0e3).mesh().ico(4).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::BLACK,
            emissive: LinearRgba::from(Color::srgb(1.0, 0.9, 0.4)) * 5.0,
            unlit: false, // Better stability with StandardMaterial flow
            ..default()
        })),
        Name::new("Sun Body"),
    )).set_parent_in_place(solar_grid).id();

    // 2. EMB Anchor
    let emb_grid = commands.spawn((
        EMBRoot,
        CelestialReferenceFrame { ephemeris_id: 3 },
        Grid::new(1.0e8, 1.0e30),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        Name::new("EMB Frame Grid"),
    )).set_parent_in_place(solar_grid).id();

    // 3. Earth Anchor
    let earth_grid = commands.spawn((
        EarthRoot,
        CelestialReferenceFrame { ephemeris_id: 399 },
        Grid::new(10_000.0, 1.0e30),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        Name::new("Earth Local Grid"),
    )).set_parent_in_place(emb_grid).id();

    // Earth Body
    let earth_body = commands.spawn((
        CelestialBody { 
            name: "Earth".to_string(), 
            ephemeris_id: 399,
            radius_m: 6371.0e3,
        },
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        NoFrustumCulling,
        GravityProvider {
            model: Box::new(PointMassGravity { gm: registry.bodies.iter().find(|d| d.ephemeris_id == 399).map(|d| d.gm).unwrap_or(3.986e14) }),
        },
        SOI { radius_m: registry.bodies.iter().find(|d| d.ephemeris_id == 399).and_then(|d| d.soi_radius_m).unwrap_or(924e6) },
        Name::new("Earth Body"),
    )).set_parent_in_place(earth_grid).id();

    // --- 24-Tile Body Architecture (LOD 1 - Definitively No-Stitch) ---
    for face in 0..6 {
        for i in 0..2 {
            for j in 0..2 {
                commands.spawn((
                    Mesh3d(meshes.add(crate::terrain::create_quadsphere_tile_mesh(earth_body, face, 1, i, j, 6371.0e3, 32, None, DVec3::ZERO))),
                    MeshMaterial3d(blueprint_materials.add(crate::blueprint::BlueprintMaterial {
                        base: StandardMaterial {
                            base_color: Color::WHITE,
                            base_color_texture: Some(asset_server.load("cached_textures://earth.png")),
                            unlit: false, 
                            ..default()
                        },
                        extension: crate::blueprint::BlueprintExtension {
                            high_color: LinearRgba::WHITE,
                            low_color: LinearRgba::WHITE,
                            high_line_color: LinearRgba::new(0.0, 0.5, 1.0, 1.0), 
                            low_line_color: LinearRgba::new(0.0, 0.5, 1.0, 1.0),
                            subdivisions: Vec2::new(36.0, 18.0), 
                            fade_range: Vec2::new(0.2, 0.6),
                            grid_scale: 1000.0,
                            line_width: 1.0, 
                            transition: 0.0,
                            body_radius: 6371.0e3,
                        },
                    })),
                    Transform::default(),
                    GlobalTransform::default(),
                    Visibility::Visible,
                    InheritedVisibility::default(),
                    NoFrustumCulling,
                    Name::new(format!("Earth Tile f{} i{} j{}", face, i, j)),
                )).set_parent_in_place(earth_body);
            }
        }
    }

    // 4. Moon Anchor
    let moon_grid = commands.spawn((
        MoonRoot,
        CelestialReferenceFrame { ephemeris_id: 301 },
        Grid::new(10_000.0, 1.0e30),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        Name::new("Moon Local Grid"),
    )).set_parent_in_place(emb_grid).id();

    // Moon Body
    let moon_body = commands.spawn((
        CelestialBody { 
            name: "Moon".to_string(), 
            ephemeris_id: 301,
            radius_m: 1737.0e3,
        },
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        NoFrustumCulling,
        GravityProvider {
            model: Box::new(PointMassGravity { gm: registry.bodies.iter().find(|d| d.ephemeris_id == 301).map(|d| d.gm).unwrap_or(4.904e12) }),
        },
        SOI { radius_m: registry.bodies.iter().find(|d| d.ephemeris_id == 301).and_then(|d| d.soi_radius_m).unwrap_or(66.1e6) },
        Name::new("Moon Body"),
    )).set_parent_in_place(moon_grid).id();

    // --- 24-Tile Body Architecture (Moon - Definitively No-Stitch) ---
    for face in 0..6 {
        for i in 0..2 {
            for j in 0..2 {
                commands.spawn((
                    Mesh3d(meshes.add(crate::terrain::create_quadsphere_tile_mesh(moon_body, face, 1, i, j, 1737.0e3, 32, None, DVec3::ZERO))),
                    MeshMaterial3d(blueprint_materials.add(crate::blueprint::BlueprintMaterial {
                        base: StandardMaterial {
                            base_color: Color::srgb(0.5, 0.5, 0.5),
                            base_color_texture: Some(asset_server.load("cached_textures://moon.png")),
                            metallic: 0.1, 
                            perceptual_roughness: 0.9,
                            ..default()
                        },
                        extension: crate::blueprint::BlueprintExtension {
                            high_color: LinearRgba::WHITE,
                            low_color: LinearRgba::WHITE,
                            high_line_color: LinearRgba::new(0.6, 0.6, 0.6, 1.0),
                            low_line_color: LinearRgba::new(0.6, 0.6, 0.6, 1.0),
                            subdivisions: Vec2::new(24.0, 12.0),
                            fade_range: Vec2::new(0.2, 0.6),
                            grid_scale: 1000.0, 
                            line_width: 2.0,
                            transition: 0.0,
                            body_radius: 1737_000.0,
                        },
                    })),
                    Transform::default(),
                    GlobalTransform::default(),
                    Visibility::Visible,
                    InheritedVisibility::default(),
                    NoFrustumCulling,
                    Name::new(format!("Moon Tile f{} i{} j{}", face, i, j)),
                )).set_parent_in_place(moon_body);
            }
        }
    }

    // Initial Observer Camera
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            near: 1.0,
            far: 1.0e15, 
            ..default()
        }),
        bevy::post_process::bloom::Bloom {
            intensity: 0.4,
            low_frequency_boost: 0.5,
            low_frequency_boost_curvature: 0.5,
            high_pass_frequency: 1.0,
            prefilter: bevy::post_process::bloom::BloomPrefilter {
                threshold: 2.0,
                threshold_softness: 0.5,
            },
            composite_mode: bevy::post_process::bloom::BloomCompositeMode::EnergyConserving,
            ..bevy::post_process::bloom::Bloom::NATURAL
        },
        bevy::core_pipeline::tonemapping::Tonemapping::TonyMcMapface,
        FloatingOrigin, 
        CellCoord::default(),
        Transform::from_translation(Vec3::new(0.0, 10_000_000.0, 10_000_000.0)),
        GlobalTransform::default(),
        lunco_camera::ObserverCamera {
            focus_target: Some(moon_body),
            mode: lunco_camera::ObserverMode::Flyby,
            distance: 2_137_000.0, // 400km alt
            pitch: -0.8,
            yaw: 0.0,
            local_flyby_pos: DVec3::new(0.0, 2_137_000.0, 0.0),
            altitude: 400_000.0,
            ..default()
        },
        lunco_camera::ActiveCamera,
        lunco_core::Avatar,
        Name::new("Observer Camera"),
    )).set_parent_in_place(moon_grid); // Parent camera back to moon_grid

    // 5. Other planets
    for body_desc in registry.bodies.iter() {
        if body_desc.ephemeris_id == 10 || body_desc.ephemeris_id == 399 || body_desc.ephemeris_id == 301 || body_desc.ephemeris_id == 3 {
            continue; 
        }
        commands.spawn((
            CelestialBody { 
                name: body_desc.name.clone(), 
                ephemeris_id: body_desc.ephemeris_id,
                radius_m: body_desc.radius_m,
            },
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Mesh3d(meshes.add(Sphere::new(body_desc.radius_m as f32).mesh().ico(2).unwrap())),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.5, 0.5, 0.5),
                ..default()
            })),
            Name::new(format!("{} Body", body_desc.name)),
        )).set_parent_in_place(solar_grid);
    }
    
    // Sun light
    commands.spawn((
        DirectionalLight {
            color: Color::WHITE,
            illuminance: 10_000.0, // Match Bevy standard for bright daylight
            shadows_enabled: true,
            ..default()
        },
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Name::new("Sun Light"),
    )).set_parent_in_place(solar_grid);
}

