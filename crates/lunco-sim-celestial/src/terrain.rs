use bevy::prelude::*;
use bevy::ecs::relationship::Relationship;
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
            spawn_threshold: 500_000.0, 
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
    q_camera: Query<(&GlobalTransform, Option<&crate::ObserverCamera>), With<Camera>>,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_tiles: Query<(Entity, &ActiveTerrainTile, &ChildOf)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<crate::blueprint::BlueprintMaterial>>,
    q_targets: Query<&GlobalTransform>,
) {
    let Some((cam_gtf, obs_opt)) = q_camera.iter().next() else { return; };
    
    // Prioritize focused target for centering the terrain
    let center_pos = if let Some(target_ent) = obs_opt.and_then(|o| o.focus_target) {
        if let Ok(t_gtf) = q_targets.get(target_ent) { t_gtf.translation().as_dvec3() } else { cam_gtf.translation().as_dvec3() }
    } else {
        cam_gtf.translation().as_dvec3()
    };
    
    let mut nearest_body = None;
    let mut min_altitude = f64::MAX;
    
    for (body_ent, body_gtf, body) in q_bodies.iter() {
        let dist = center_pos.distance(body_gtf.translation().as_dvec3());
        let alt = dist - body.radius_m;
        if alt < min_altitude {
            min_altitude = alt;
            nearest_body = Some((body_ent, body_gtf, body));
        }
    }
    
    if min_altitude < config.spawn_threshold {
        if let Some((body_ent, body_gtf, body)) = nearest_body {
            let body_pos = body_gtf.translation().as_dvec3();
            let dir = (center_pos - body_pos).normalize_or_zero();
            let surface_pos_rel = dir * (body.radius_m + 0.1); 
            let rot = Quat::from_rotation_arc(Vec3::Y, dir.as_vec3());

            let mut existing_tile = None;
            for (tile_ent, _, child_of) in q_tiles.iter() {
                if child_of.get() == body_ent { existing_tile = Some(tile_ent); break; }
            }

            if let Some(tile_ent) = existing_tile {
                commands.entity(tile_ent).insert(Transform::from_translation(surface_pos_rel.as_vec3()).with_rotation(rot));
            } else {
                let subs = if min_altitude < 20000.0 { 128 } else if min_altitude < 50000.0 { 64 } else { 16 };
                commands.spawn((
                    ActiveTerrainTile,
                    RigidBody::Static,
                    Collider::cuboid(config.tile_size, 0.5, config.tile_size),
                    Mesh3d(meshes.add(Plane3d::default().mesh().size(config.tile_size as f32, config.tile_size as f32).subdivisions(subs))),
                    MeshMaterial3d(materials.add(crate::blueprint::BlueprintMaterial {
                        base: StandardMaterial {
                            base_color: Color::from(LinearRgba::new(0.0, 0.1, 0.3, 1.0)),
                            emissive: LinearRgba::new(0.0, 0.2, 0.6, 1.0),
                            metallic: 0.9,
                            perceptual_roughness: 0.1,
                            ..default()
                        },
                        extension: crate::blueprint::BlueprintExtension {
                            high_color: if body.name == "Earth" { LinearRgba::from(Color::srgb(0.05, 0.15, 0.8)) } else { LinearRgba::new(0.1, 0.1, 0.1, 1.0) },
                            low_color: if body.name == "Earth" { LinearRgba::from(Color::srgb(0.05, 0.15, 0.8)) } else { LinearRgba::new(0.1, 0.1, 0.1, 1.0) },
                            high_line_color: if body.name == "Earth" { LinearRgba::new(0.0, 0.5, 1.0, 1.0) } else { LinearRgba::new(0.6, 0.6, 0.6, 1.0) },
                            low_line_color: if body.name == "Earth" { LinearRgba::new(0.0, 0.5, 1.0, 1.0) } else { LinearRgba::new(0.6, 0.6, 0.6, 1.0) },
                            grid_scale: 100.0,
                            line_width: 1.0,
                            transition: 1.0, // Surface tiles are "low/blueprint" style
                            body_radius: body.radius_m as f32,
                            ..default()
                        },
                    })),
                    Transform::from_translation(surface_pos_rel.as_vec3()).with_rotation(rot),
                    GlobalTransform::default(),
                    Name::new("Blueprint Surface Tile"),
                )).set_parent_in_place(body_ent);
            }
        }
    } else {
        for (ent, _, _) in q_tiles.iter() {
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
