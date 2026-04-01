use bevy::prelude::*;
use avian3d::prelude::*;
use big_space::prelude::*;
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
            spawn_threshold: 50_000.0, // Alt below which we spawn a tile
        }
    }
}

#[derive(Component)]
pub struct ActiveTerrainTile;

pub fn terrain_spawn_system(
    mut commands: Commands,
    config: Res<TerrainTileConfig>,
    q_camera: Query<(&GlobalTransform, &ChildOf), With<Camera>>,
    q_bodies: Query<(&GlobalTransform, &CelestialBody, Entity)>,
    q_tiles: Query<Entity, With<ActiveTerrainTile>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some((cam_gtf, cam_child_of)) = q_camera.iter().next() else { return; };
    let cam_pos = cam_gtf.translation().as_dvec3();
    
    let mut nearest_body = None;
    let mut min_altitude = f64::MAX;
    
    for (body_gtf, body, ent) in q_bodies.iter() {
        let dist = cam_pos.distance(body_gtf.translation().as_dvec3());
        let alt = dist - body.radius_m;
        if alt < min_altitude {
            min_altitude = alt;
            nearest_body = Some((ent, body_gtf, body));
        }
    }
    
    if min_altitude < config.spawn_threshold {
        if q_tiles.is_empty() {
            if let Some((body_ent, body_gtf, body)) = nearest_body {
                // Spawn tile at surface under camera
                let dir = (cam_pos - body_gtf.translation().as_dvec3()).normalize_or_zero();
                let surface_pos_rel = dir * (body.radius_m + 0.01); 
                
                // Bevy Y-up vs Body local? For now, let's assume Bevy Y is "up" locally for simplicity 
                // in the tile mesh, but we should orient it correctly.
                let rot = Quat::from_rotation_arc(Vec3::Y, dir.as_vec3());

                commands.spawn((
                    ActiveTerrainTile,
                    RigidBody::Static,
                    Collider::cuboid(config.tile_size, 0.1, config.tile_size),
                    Mesh3d(meshes.add(Plane3d::default().mesh().size(config.tile_size as f32, config.tile_size as f32))),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb(0.5, 0.4, 0.3),
                        ..default()
                    })),
                    Transform::from_translation(surface_pos_rel.as_vec3()).with_rotation(rot),
                    Name::new("Terrain Tile"),
                )).set_parent_in_place(body_ent);
            }
        }
    } else {
        // Despawn if too high
        for ent in q_tiles.iter() {
            commands.entity(ent).despawn();
        }
    }
}
