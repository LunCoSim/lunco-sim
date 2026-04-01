use bevy::prelude::*;
use avian3d::prelude::*;
use crate::registry::CelestialBody;

#[derive(Resource)]
pub struct TerrainTileConfig {
    pub tile_size: f64,
    pub spawn_threshold: f64,
}

impl Default for TerrainTileConfig {
    fn default() -> Self {
        Self {
            tile_size: 10_000.0,
            spawn_threshold: 100_000.0, 
        }
    }
}

#[derive(Component)]
pub struct ActiveTerrainTile;

/// Local ENU (East-North-Up) frame for robotics and surface missions.
#[derive(Component)]
pub struct ENUFrame;

pub fn terrain_spawn_system(
    mut commands: Commands,
    config: Res<TerrainTileConfig>,
    q_camera: Query<(Entity, &GlobalTransform), With<Camera>>,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_tiles: Query<Entity, With<ActiveTerrainTile>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some((_cam_ent, cam_gtf)) = q_camera.iter().next() else { return; };
    let cam_pos = cam_gtf.translation().as_dvec3();
    
    let mut nearest_body = None;
    let mut min_altitude = f64::MAX;
    
    for (body_ent, body_gtf, body) in q_bodies.iter() {
        let dist = cam_pos.distance(body_gtf.translation().as_dvec3());
        let alt = dist - body.radius_m;
        if alt < min_altitude {
            min_altitude = alt;
            nearest_body = Some((body_ent, body_gtf, body));
        }
    }
    
    if min_altitude < config.spawn_threshold {
        if q_tiles.is_empty() {
            if let Some((body_ent, body_gtf, body)) = nearest_body {
                let body_pos = body_gtf.translation().as_dvec3();
                let dir = (cam_pos - body_pos).normalize_or_zero();
                let surface_pos_rel = dir * (body.radius_m + 0.1); 
                let rot = Quat::from_rotation_arc(Vec3::Y, dir.as_vec3());

                commands.spawn((
                    ActiveTerrainTile,
                    RigidBody::Static,
                    Collider::cuboid(config.tile_size, 0.5, config.tile_size),
                    Mesh3d(meshes.add(Plane3d::default().mesh().size(config.tile_size as f32, config.tile_size as f32))),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb(0.3, 0.3, 0.35),
                        metallic: 0.1,
                        perceptual_roughness: 0.9,
                        ..default()
                    })),
                    Transform::from_translation(surface_pos_rel.as_vec3()).with_rotation(rot),
                    GlobalTransform::default(),
                    Name::new("Surface Local Tile"),
                )).set_parent_in_place(body_ent);
            }
        }
    } else {
        for ent in q_tiles.iter() {
            commands.entity(ent).despawn();
        }
    }
}

pub fn spawn_rover_at_camera_surface(
    commands: &mut Commands,
    cam_gtf: &GlobalTransform,
    body_gtf: &GlobalTransform,
    body: &CelestialBody,
    body_entity: Entity,
) -> Entity {
    let cam_pos = cam_gtf.translation().as_dvec3();
    let body_pos = body_gtf.translation().as_dvec3();
    let dir = (cam_pos - body_pos).normalize_or_zero();
    let surface_pos_rel = dir * (body.radius_m + 2.0); 
    let rot = Quat::from_rotation_arc(Vec3::Y, dir.as_vec3());

    commands.spawn((
        lunco_sim_core::RoverVessel,
        ENUFrame, 
        RigidBody::Dynamic,
        Collider::cuboid(2.0, 1.0, 4.0),
        Transform::from_translation(surface_pos_rel.as_vec3()).with_rotation(rot),
        GlobalTransform::default(),
        Name::new(format!("Rover @ {}", body.name)),
    )).set_parent_in_place(body_entity).id()
}
