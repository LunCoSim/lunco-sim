use bevy::prelude::*;
use bevy::math::DVec3;
use bevy::render::render_resource::PrimitiveTopology;
use bevy_mesh::Indices;
use avian3d::prelude::*;
use crate::registry::CelestialBody;
use crate::camera::{ObserverCamera, ObserverMode};

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct TerrainTileConfig {
    pub tile_size_m: f64,
    pub tile_resolution: u32,
    pub grid_radius: i32,
    pub spawn_threshold: f64,
    pub max_lod: u32,
    pub lod_distance_factor: f64,
    pub physics_lod_threshold: u32,
}

impl Default for TerrainTileConfig {
    fn default() -> Self {
        Self {
            tile_size_m: 500.0,
            tile_resolution: 32,
            grid_radius: 4,
            spawn_threshold: 100_000.0,
            max_lod: 12,
            lod_distance_factor: 2.0,
            physics_lod_threshold: 8,
        }
    }
}

#[derive(Resource, Default)]
pub struct TerrainMapRegistry {
    pub maps: Vec<CustomMap>,
}

pub struct CustomMap {
    pub name: String,
    pub body_entity: Entity,
    pub center_lat_long: Vec2,
    pub radius_m: f32,
    pub height_offset: f32,
}

#[derive(Component)]
pub struct ActiveTerrainTile;

#[derive(Component)]
pub struct ENUFrame;

#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[reflect(Component)]
pub struct TileCoord {
    pub body: Entity,
    pub face: u8,
    pub level: u32,
    pub i: i32,
    pub j: i32,
}

#[derive(Component)]
pub struct TerrainTile;

pub fn terrain_spawn_system(
    mut commands: Commands,
    config: Res<TerrainTileConfig>,
    registry: Res<TerrainMapRegistry>,
    q_camera: Query<(&GlobalTransform, &ObserverCamera), With<Camera>>,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_tiles: Query<(Entity, &TileCoord)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<crate::blueprint::BlueprintMaterial>>,
) {
    let Some((cam_gtf, _obs)) = q_camera.iter().next() else { return; };
    let camera_pos = cam_gtf.translation().as_dvec3();
    
    let mut nearest_body = None;
    let mut min_altitude = f64::MAX;
    
    for (body_ent, body_gtf, body) in q_bodies.iter() {
        let dist = camera_pos.distance(body_gtf.translation().as_dvec3());
        let alt = dist - body.radius_m;
        if alt < min_altitude {
            min_altitude = alt;
            nearest_body = Some((body_ent, body_gtf, body));
        }
    }

    let Some((body_ent, body_gtf, body)) = nearest_body else { return; };
    let body_pos = body_gtf.translation().as_dvec3();

    if min_altitude < config.spawn_threshold {
        let mut desired_tiles = std::collections::HashSet::new();
        for face in 0..6 {
            subdivide_face(&mut desired_tiles, body_ent, face, 0, 0, 0, camera_pos, body_pos, body.radius_m, &config);
        }

        for (tile_ent, coord) in q_tiles.iter() {
            if !desired_tiles.contains(coord) {
                 // Use simple despawn if hierarchy helper is not found
                commands.entity(tile_ent).despawn();
            } else {
                desired_tiles.remove(coord);
            }
        }

        for coord in desired_tiles {
            let tiles_at_level = 1 << coord.level;
            let step = 2.0 / tiles_at_level as f64;
            let u_mid = -1.0 + (coord.i as f64 + 0.5) * step;
            let v_mid = -1.0 + (coord.j as f64 + 0.5) * step;
            let tile_center_dir = cube_to_sphere(coord.face, u_mid, v_mid);
            let tile_center_pos = tile_center_dir * body.radius_m;
            
            let mesh = create_quadsphere_tile_mesh(coord.body, coord.face, coord.level, coord.i, coord.j, body.radius_m, config.tile_resolution, &registry, tile_center_pos);
            
            let tile_ent = commands.spawn((
                ActiveTerrainTile,
                TerrainTile,
                coord,
                Mesh3d(meshes.add(mesh.clone())),
                MeshMaterial3d(materials.add(crate::blueprint::BlueprintMaterial {
                    base: StandardMaterial {
                        base_color: if body.name == "Moon" { Color::srgb(0.2, 0.2, 0.2) } else { Color::from(LinearRgba::new(0.005, 0.02, 0.05, 1.0)) },
                        perceptual_roughness: 0.8,
                        ..default()
                    },
                    extension: crate::blueprint::BlueprintExtension {
                        high_color: if body.name == "Earth" { LinearRgba::from(Color::srgb(0.05, 0.15, 0.8)) } else { LinearRgba::new(0.01, 0.01, 0.01, 1.0) },
                        low_color: if body.name == "Earth" { LinearRgba::from(Color::srgb(0.05, 0.15, 0.8)) } else { LinearRgba::new(0.01, 0.01, 0.01, 1.0) },
                        grid_scale: 100.0, // 100m grid for surface
                        line_width: 1.0,
                        subdivisions: Vec2::new(360.0, 180.0),
                        transition: (1.0 - (min_altitude / 50_000.0)).clamp(0.0, 1.0) as f32,
                        body_radius: body.radius_m as f32,
                        ..default()
                    },
                })),
                Transform::from_translation(tile_center_pos.as_vec3()),
                GlobalTransform::default(),
                Name::new(format!("Tile f{} l{} i{} j{}", coord.face, coord.level, coord.i, coord.j)),
            )).id();
            
            commands.entity(body_ent).add_child(tile_ent);
            if coord.level >= config.physics_lod_threshold {
                commands.entity(tile_ent).insert((RigidBody::Static, Collider::trimesh_from_mesh(&mesh).unwrap()));
            }
        }
    } else {
        for (ent, _) in q_tiles.iter() {
            commands.entity(ent).despawn(); 
        }
    }
}

fn subdivide_face(desired: &mut std::collections::HashSet<TileCoord>, body_ent: Entity, face: u8, level: u32, i: i32, j: i32, camera_pos: DVec3, body_pos: DVec3, radius: f64, config: &TerrainTileConfig) {
    let tiles_at_level = 1 << level;
    let step = 2.0 / tiles_at_level as f64;
    let u = -1.0 + (i as f64 + 0.5) * step;
    let v = -1.0 + (j as f64 + 0.5) * step;
    let tile_center_sphere = cube_to_sphere(face, u, v);
    let tile_center_world = body_pos + tile_center_sphere * radius;
    let dist = camera_pos.distance(tile_center_world);
    let tile_size = (radius * std::f64::consts::PI * 0.5) / tiles_at_level as f64;
    
    if level < config.max_lod && dist < tile_size * config.lod_distance_factor {
        for di in 0..2 {
            for dj in 0..2 {
                subdivide_face(desired, body_ent, face, level + 1, i * 2 + di, j * 2 + dj, camera_pos, body_pos, radius, config);
            }
        }
    } else {
        desired.insert(TileCoord { body: body_ent, face, level, i, j });
    }
}

fn project_to_cube(v: DVec3) -> (u8, f64, f64) {
    let abs_v = v.abs();
    if abs_v.x >= abs_v.y && abs_v.x >= abs_v.z {
        if v.x > 0.0 { (0, -v.z / v.x, v.y / v.x) } else { (1, -v.z / v.x, -v.y / v.x) }
    } else if abs_v.y >= abs_v.x && abs_v.y >= abs_v.z {
        if v.y > 0.0 { (2, v.x / v.y, -v.z / v.y) } else { (3, -v.x / v.y, -v.z / v.y) }
    } else {
        if v.z > 0.0 { (4, v.x / v.z, v.y / v.z) } else { (5, v.x / v.z, -v.y / v.z) }
    }
}

fn cube_to_sphere(face: u8, u: f64, v: f64) -> DVec3 {
    let p = match face {
        0 => DVec3::new(1.0, v, -u),
        1 => DVec3::new(-1.0, v, u),
        2 => DVec3::new(u, 1.0, v),
        3 => DVec3::new(u, -1.0, -v),
        4 => DVec3::new(u, v, 1.0),
        5 => DVec3::new(-u, v, -1.0),
        _ => DVec3::ZERO,
    };
    p.normalize()
}

fn create_quadsphere_tile_mesh(body_ent: Entity, face: u8, level: u32, i: i32, j: i32, radius: f64, res: u32, registry: &TerrainMapRegistry, tile_center: DVec3) -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let tiles_at_level = 1 << level;
    let step = 2.0 / tiles_at_level as f64;
    let start_u = -1.0 + (i as f64) * step;
    let start_v = -1.0 + (j as f64) * step;
    
    for y in 0..=res {
        for x in 0..=res {
            let u = start_u + (x as f64 / res as f64) * step;
            let v = start_v + (y as f64 / res as f64) * step;
            let pos_sphere = cube_to_sphere(face, u, v);
            let h = sample_height(body_ent, pos_sphere, radius, registry);
            positions.push((pos_sphere * h - tile_center).as_vec3());
            normals.push(pos_sphere.as_vec3());
        }
    }

    for y in 0..res {
        for x in 0..res {
            let i0 = y * (res + 1) + x;
            let i1 = i0 + 1;
            let i2 = (y + 1) * (res + 1) + x;
            let i3 = i2 + 1;
            indices.push(i0); indices.push(i2); indices.push(i1);
            indices.push(i1); indices.push(i2); indices.push(i3);
        }
    }

    let skirt_depth = (radius * std::f64::consts::PI / tiles_at_level as f64 / res as f64) * 5.0;
    let mut add_skirt = |indices_to_extrude: Vec<u32>| {
        let mut skirt_indices = Vec::new();
        for &idx in &indices_to_extrude {
            let pos = positions[idx as usize];
            let norm = normals[idx as usize];
            let skirt_pos = pos - norm * skirt_depth as f32;
            skirt_indices.push(positions.len() as u32);
            positions.push(skirt_pos);
            normals.push(norm);
        }
        for i in 0..(indices_to_extrude.len() as u32 - 1) {
            let a = indices_to_extrude[i as usize];
            let b = indices_to_extrude[i as usize + 1];
            let c = skirt_indices[i as usize];
            let d = skirt_indices[i as usize + 1];
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    };

    add_skirt((0..=res).collect());
    add_skirt((res*(res+1)..=(res+1)*(res+1)-1).collect());
    add_skirt((0..=res).map(|y| y * (res + 1)).collect());
    add_skirt((0..=res).map(|y| y * (res + 1) + res).collect());

    // Use direct crate imports to resolve private blockers, following compiler suggestions
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, Default::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sample_height(body_ent: Entity, pos_sphere: DVec3, radius: f64, registry: &TerrainMapRegistry) -> f64 {
    let mut h = radius;
    let lat = pos_sphere.y.asin().to_degrees() as f32;
    let long = pos_sphere.x.atan2(pos_sphere.z).to_degrees() as f32;
    let pos_geo = Vec2::new(lat, long);

    for map in &registry.maps {
        if map.body_entity != body_ent { continue; }
        let dist_deg = pos_geo.distance(map.center_lat_long);
        let radius_deg = (map.radius_m / radius as f32).to_degrees();
        if dist_deg < radius_deg {
            let t = smoothstep(0.8, 1.0, 1.0 - (dist_deg / radius_deg));
            h += (map.height_offset as f64) * (t as f64);
        }
    }
    h
}

pub fn setup_terrain_overrides(mut registry: ResMut<TerrainMapRegistry>, q_bodies: Query<(Entity, &CelestialBody)>) {
    for (ent, body) in q_bodies.iter() {
        if body.name == "Moon" {
            registry.maps.push(CustomMap {
                name: "Langrenus Crater".into(),
                body_entity: ent,
                center_lat_long: Vec2::new(-8.9, 61.1),
                radius_m: 66_000.0,
                height_offset: -2000.0,
            });
        }
    }
}

pub fn spawn_rover_at_camera_surface(commands: &mut Commands, cam_gtf: &GlobalTransform, body_gtf: &GlobalTransform, body: &CelestialBody, body_entity: Entity) -> Entity {
    let cam_pos = cam_gtf.translation().as_dvec3();
    let body_pos = body_gtf.translation().as_dvec3();
    let dir = (cam_pos - body_pos).normalize_or_zero();
    let surface_pos_rel = dir * (body.radius_m + 2.0); 
    let rot = Quat::from_rotation_arc(Vec3::Y, dir.as_vec3());
    commands.spawn((
        lunco_core::RoverVessel,
        ENUFrame, 
        RigidBody::Dynamic,
        Collider::cuboid(2.0, 1.0, 4.0),
        Transform::from_translation(surface_pos_rel.as_vec3()).with_rotation(rot),
        GlobalTransform::default(),
        Name::new(format!("Rover @ {}", body.name)),
    )).set_parent_in_place(body_entity).id()
}
