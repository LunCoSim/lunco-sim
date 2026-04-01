use bevy::prelude::*;
use big_space::prelude::*;
use crate::registry::{CelestialBodyRegistry, CelestialReferenceFrame, CelestialBody};

#[derive(Component)]
pub struct SolarSystemRoot;

#[derive(Component)]
pub struct EMBRoot;

#[derive(Component)]
pub struct EarthRoot;

#[derive(Component)]
pub struct MoonRoot;

/// Initialize the big_space hierarchy for the solar system.
pub fn setup_big_space_hierarchy(
    mut commands: Commands,
    registry: Res<CelestialBodyRegistry>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // 1. Root Solar System grid (Sun-centered)
    let solar_root = commands.spawn((
        SolarSystemRoot,
        CelestialReferenceFrame { ephemeris_id: 10 }, 
        BigSpaceRootBundle {
            grid: Grid::new(1.5e11, 1.0e10), 
            ..default()
        },
        // AD-9: No mesh for Sun. Use DirectionalLight.
        DirectionalLight {
            illuminance: 100_000.0,
            shadows_enabled: true,
            ..default()
        },
        CelestialBody { 
            name: "Sun".to_string(), 
            ephemeris_id: 10,
            radius_m: registry.bodies.iter().find(|d| d.ephemeris_id == 10).map(|d| d.radius_m).unwrap_or(695_700_000.0),
        },
        Name::new("Solar System Root (Sun)"),
    )).id();

    // 2. Earth-Moon Barycenter (child of Solar)
    let emb_root = commands.spawn((
        EMBRoot,
        CelestialReferenceFrame { ephemeris_id: 3 },
        BigGridBundle {
            grid: Grid::new(1.0e9, 1.0e8),
            ..default()
        },
        Name::new("EMB Frame"),
    )).id();
    commands.entity(emb_root).set_parent_in_place(solar_root);

    // 3. Earth Frame (child of EMB)
    let earth_frame = commands.spawn((
        EarthRoot,
        CelestialReferenceFrame { ephemeris_id: 399 },
        BigGridBundle {
            grid: Grid::new(1.0e7, 1.0e6),
            ..default()
        },
        // Visual for Earth
        Mesh3d(meshes.add(Sphere::new(6371.0e3).mesh().ico(4).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.0, 1.0, 0.0),
            ..default()
        })),
        CelestialBody { 
            name: "Earth".to_string(), 
            ephemeris_id: 399,
            radius_m: 6371.0e3,
        },
        Name::new("Earth Frame"),
    )).id();
    commands.entity(earth_frame).set_parent_in_place(emb_root);

    // 4. Moon Frame (child of EMB)
    let moon_frame = commands.spawn((
        MoonRoot,
        CelestialReferenceFrame { ephemeris_id: 301 },
        BigGridBundle {
            grid: Grid::new(1.0e6, 1.0e5),
            ..default()
        },
        // Visual for Moon
        Mesh3d(meshes.add(Sphere::new(1737.0e3).mesh().ico(4).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.8, 0.8, 0.8),
            ..default()
        })),
        CelestialBody { 
            name: "Moon".to_string(), 
            ephemeris_id: 301,
            radius_m: 1737.0e3,
        },
        Name::new("Moon Frame"),
    )).id();
    commands.entity(moon_frame).set_parent_in_place(emb_root);

    // 5. Spawn other planets directly into Solar Root
    for body_desc in registry.bodies.iter() {
        if [10, 399, 301].contains(&body_desc.ephemeris_id) { continue; }

        let mesh_size = body_desc.radius_m as f32;
        commands.spawn((
            CelestialBody {
                name: body_desc.name.clone(),
                ephemeris_id: body_desc.ephemeris_id,
                radius_m: body_desc.radius_m,
            },
            BigSpatialBundle::default(), 
            Mesh3d(meshes.add(Sphere::new(mesh_size).mesh().ico(4).unwrap())),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.5, 0.5, 0.5),
                ..default()
            })),
            Name::new(body_desc.name.clone()),
        )).set_parent_in_place(solar_root);
    }
}
