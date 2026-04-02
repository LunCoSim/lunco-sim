use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::*;
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
        Mesh3d(meshes.add(Sphere::new(696_340.0e3).mesh().ico(4).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.9, 0.6),
            emissive: LinearRgba::from(Color::srgb(1.0, 0.8, 0.1)),
            unlit: true,
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
        Name::new("Earth Local Grid"),
    )).set_parent_in_place(emb_grid).id();

    // Earth Body
    let _earth_body = commands.spawn((
        CelestialBody { 
            name: "Earth".to_string(), 
            ephemeris_id: 399,
            radius_m: 6371.0e3,
        },
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Mesh3d(meshes.add(Sphere::new(6371.0e3).mesh().ico(4).unwrap())),
        MeshMaterial3d(blueprint_materials.add(crate::blueprint::BlueprintMaterial {
            base: StandardMaterial {
                base_color: Color::srgb(0.2, 0.4, 1.0),
                unlit: false, 
                ..default()
            },
            extension: crate::blueprint::BlueprintExtension {
                high_color: LinearRgba::from(Color::srgb(0.05, 0.15, 0.8)),
                low_color: LinearRgba::from(Color::srgb(0.05, 0.15, 0.8)),
                high_line_color: LinearRgba::new(0.0, 0.5, 1.0, 1.0), // Cyan for Earth Blueprint
                low_line_color: LinearRgba::new(0.0, 0.5, 1.0, 1.0),
                subdivisions: Vec2::new(36.0, 18.0), // Denser grid for Earth (10 deg)
                fade_range: Vec2::new(0.2, 0.6),
                grid_scale: 1000.0,
                line_width: 1.0, 
                transition: 0.0,
                body_radius: 6371.0e3,
            },
        })),
        GravityProvider {
            model: Box::new(PointMassGravity { gm: registry.bodies.iter().find(|d| d.ephemeris_id == 399).map(|d| d.gm).unwrap_or(3.986e14) }),
        },
        SOI { radius_m: registry.bodies.iter().find(|d| d.ephemeris_id == 399).and_then(|d| d.soi_radius_m).unwrap_or(924e6) },
        Name::new("Earth Body"),
    )).set_parent_in_place(earth_grid).id();

    // 4. Moon Anchor
    let moon_grid = commands.spawn((
        MoonRoot,
        CelestialReferenceFrame { ephemeris_id: 301 },
        Grid::new(10_000.0, 1.0e30),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
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
        Mesh3d(meshes.add(Sphere::new(1737.0e3).mesh().ico(6).unwrap())),
        MeshMaterial3d(blueprint_materials.add(crate::blueprint::BlueprintMaterial {
            base: StandardMaterial {
                base_color: Color::srgb(0.5, 0.5, 0.5),
                metallic: 0.2, // Less metallic for a moon
                perceptual_roughness: 0.8,
                ..default()
            },
            extension: crate::blueprint::BlueprintExtension {
                high_color: LinearRgba::new(0.1, 0.1, 0.1, 1.0),
                low_color: LinearRgba::new(0.1, 0.1, 0.1, 1.0),
                high_line_color: LinearRgba::new(0.6, 0.6, 0.6, 1.0), // Grey for Moon Blueprint
                low_line_color: LinearRgba::new(0.6, 0.6, 0.6, 1.0),
                subdivisions: Vec2::new(24.0, 12.0),
                fade_range: Vec2::new(0.2, 0.6),
                grid_scale: 1000.0, // Fine grid for blueprint
                line_width: 2.0,
                transition: 0.0, // Start with Lat/Long (High)
                body_radius: 1737_000.0,
            },
        })),
        GravityProvider {
            model: Box::new(PointMassGravity { gm: registry.bodies.iter().find(|d| d.ephemeris_id == 301).map(|d| d.gm).unwrap_or(4.904e12) }),
        },
        SOI { radius_m: registry.bodies.iter().find(|d| d.ephemeris_id == 301).and_then(|d| d.soi_radius_m).unwrap_or(66.1e6) },
        Name::new("Moon Body"),
    )).set_parent_in_place(moon_grid).id();

    // Initial Observer Camera
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            near: 1.0,
            far: 1.0e15, 
            ..default()
        }),
        FloatingOrigin, 
        CellCoord::default(),
        Transform::from_translation(Vec3::new(0.0, 10_000_000.0, 10_000_000.0)),
        GlobalTransform::default(),
        crate::ObserverCamera {
            focus_target: Some(moon_body),
            mode: crate::ObserverMode::Flyby,
            distance: 2_137_000.0, // 400km alt
            pitch: -0.8,
            yaw: 0.0,
            local_flyby_pos: DVec3::new(0.0, 2_137_000.0, 0.0),
            altitude: 400_000.0,
        },
        crate::ActiveCamera,
        lunco_core::Avatar,
        Name::new("Observer Camera"),
    )).set_parent_in_place(moon_grid);

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
            illuminance: 10_000.0,
            shadows_enabled: false,
            ..default()
        },
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Name::new("Sun Light"),
    )).set_parent_in_place(solar_grid);
}
