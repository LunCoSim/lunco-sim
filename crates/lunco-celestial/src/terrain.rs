use bevy::prelude::*;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::math::DVec3;
use bevy::render::render_resource::PrimitiveTopology;
use bevy_mesh::Indices;
use bevy::tasks::{Task, AsyncComputeTaskPool};
use futures_lite::future;
use std::sync::Arc;
use avian3d::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use crate::registry::CelestialBody;

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
            grid_radius: 4, // Limit tile spawning to ~4 grid cells around camera
            spawn_threshold: 100_000.0, // 100 km — tiles visible from low orbit
            max_lod: 12,
            lod_distance_factor: 2.0,
            physics_lod_threshold: 8,
        }
    }
}

#[derive(Resource, Default, Clone)]
pub struct TerrainMapRegistry {
    pub maps: Arc<Vec<CustomMap>>,
}

#[derive(Clone)]
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

pub struct TileMeshData {
    pub mesh: Mesh,
    pub collider: Option<Collider>,
}

#[derive(Component)]
pub struct PendingTile(pub Task<TileMeshData>);

pub fn terrain_spawn_system(
    mut commands: Commands,
    config: Res<TerrainTileConfig>,
    registry: Res<TerrainMapRegistry>,
    q_camera: Query<(Entity, &GlobalTransform, &CellCoord, &Transform, &ChildOf), (With<Camera>, With<lunco_core::Avatar>)>,
    q_bodies: Query<(Entity, &GlobalTransform, &CellCoord, &Transform, &ChildOf, &CelestialBody)>,
    q_tiles: Query<(Entity, &TileCoord)>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(&CellCoord, &Transform)>,
) {
    let Some((cam_ent, _, cam_cell, cam_tf, _)) = q_camera.iter().next() else { return; };

    // Use absolute coordinates for both camera and bodies so altitudes are correct.
    // GlobalTransform alone is insufficient because big_space splits world position
    // across CellCoord (integer cell index) and Transform (local remainder).
    let camera_abs = crate::coords::get_absolute_pos_in_root_double_ghost_aware(
        cam_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial,
    );

    let mut nearest_body = None;
    let mut min_altitude = f64::MAX;

    for (body_ent, _, b_cell, b_tf, _, body) in q_bodies.iter() {
        let body_abs = crate::coords::get_absolute_pos_in_root_double_ghost_aware(
            body_ent, b_cell, b_tf, &q_parents, &q_grids, &q_spatial,
        );
        let dist = (camera_abs - body_abs).length();
        let alt = dist - body.radius_m;
        if alt < min_altitude {
            min_altitude = alt;
            nearest_body = Some((body_ent, body_abs, body.radius_m));
        }
    }

    let Some((body_ent, body_abs, body_radius)) = nearest_body else { return; };

    // Debug: log altitude every ~2 seconds
    static mut DEBUG_TIMER: f32 = 0.0;
    unsafe { DEBUG_TIMER += 1.0 / 60.0; }
    if unsafe { DEBUG_TIMER > 2.0 } {
        unsafe { DEBUG_TIMER = 0.0; }
        warn!("TERRAIN: alt={:.0}m threshold={} tiles_on_screen={}",
              min_altitude, config.spawn_threshold, q_tiles.iter().count());
    }

    if min_altitude < config.spawn_threshold {
        let mut desired_tiles = std::collections::HashSet::new();
        for face in 0..6 {
            subdivide_face(&mut desired_tiles, body_ent, face, 0, 0, 0, camera_abs, body_abs, body_radius, &config);
        }

        let new_tile_count = desired_tiles.len();
        info!("TERRAIN: spawning {} tiles", new_tile_count);

        for (tile_ent, coord) in q_tiles.iter() {
            if !desired_tiles.contains(coord) {
                commands.entity(tile_ent).despawn();
            } else {
                desired_tiles.remove(coord);
            }
        }

        let pool = AsyncComputeTaskPool::get();

        for coord in desired_tiles {
            let tiles_at_level = 1 << coord.level;
            let step = 2.0 / tiles_at_level as f64;
            let u_mid = -1.0 + (coord.i as f64 + 0.5) * step;
            let v_mid = -1.0 + (coord.j as f64 + 0.5) * step;
            let tile_center_dir = cube_to_sphere(coord.face, u_mid, v_mid);
            let tile_center_pos = tile_center_dir * body_radius;

            // Task parameters
            let body_ent_inner = coord.body;
            let face_inner = coord.face;
            let level_inner = coord.level;
            let i_inner = coord.i;
            let j_inner = coord.j;
            let radius_inner = body_radius;
            let res_inner = config.tile_resolution;
            let registry_inner = registry.clone();
            let tile_center_inner = tile_center_pos;
            let physics_threshold = config.physics_lod_threshold;

            let task = pool.spawn(async move {
                let mesh = create_quadsphere_tile_mesh(body_ent_inner, face_inner, level_inner, i_inner, j_inner, radius_inner, res_inner, Some(&registry_inner), tile_center_inner);
                let mut collider = None;
                if level_inner >= physics_threshold {
                    collider = Collider::trimesh_from_mesh(&mesh);
                }
                TileMeshData { mesh, collider }
            });
            
            let tile_ent = commands.spawn((
                ActiveTerrainTile,
                TerrainTile,
                coord,
                PendingTile(task),
                Transform::from_translation(tile_center_pos.as_vec3()),
                GlobalTransform::default(),
                Visibility::Visible,
                InheritedVisibility::default(),
                NoFrustumCulling,
                Name::new(format!("Tile f{} l{} i{} j{}", coord.face, coord.level, coord.i, coord.j)),
            )).id();

            // Parent tiles to the Body entity so they inherit rotation.
            // The Body is a child of the Grid, so tiles share the same
            // big_space Grid ancestor as the camera.
            commands.entity(body_ent).add_child(tile_ent);
        }

        warn!("TERRAIN: spawned {} tile entities, {} already on screen",
              new_tile_count, q_tiles.iter().count());
    } else {
        for (ent, _) in q_tiles.iter() {
            commands.entity(ent).despawn(); 
        }
    }
}

pub fn finalize_terrain_tiles(
    mut commands: Commands,
    mut q_pending: Query<(Entity, &TileCoord, &mut PendingTile)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<crate::blueprint::BlueprintMaterial>>,
    q_bodies: Query<(Entity, &CelestialBody, &CellCoord, &Transform, &ChildOf, &MeshMaterial3d<crate::blueprint::BlueprintMaterial>)>,
    q_camera: Query<(Entity, &CellCoord, &Transform, &ChildOf), (With<Camera>, With<lunco_core::Avatar>)>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(&CellCoord, &Transform)>,
) {
    let Some((cam_ent, cam_cell, cam_tf, _)) = q_camera.iter().next() else { return; };
    let camera_abs = crate::coords::get_absolute_pos_in_root_double_ghost_aware(
        cam_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial,
    );

    for (ent, coord, mut pending) in q_pending.iter_mut() {
        if let Some(data) = future::block_on(future::poll_once(&mut pending.0)) {
            let Ok((body_ent, body, b_cell, b_tf, _, body_mat_handle)) = q_bodies.get(coord.body) else {
                commands.entity(ent).despawn();
                continue;
            };

            let body_abs = crate::coords::get_absolute_pos_in_root_double_ghost_aware(
                body_ent, b_cell, b_tf, &q_parents, &q_grids, &q_spatial,
            );
            let dist = (camera_abs - body_abs).length();
            let altitude = (dist - body.radius_m).max(0.0);

            // Get texture from body material
            let mut base_color = if body.name == "Moon" { Color::srgb(0.2, 0.2, 0.2) } else { Color::from(LinearRgba::new(0.005, 0.02, 0.05, 1.0)) };
            let mut base_color_texture = None;
            
            if let Some(body_mat) = materials.get(body_mat_handle) {
                base_color = body_mat.base.base_color;
                base_color_texture = body_mat.base.base_color_texture.clone();
            }

            let mut entity_cmds = commands.entity(ent);
            entity_cmds.insert((
                Mesh3d(meshes.add(data.mesh)),
                MeshMaterial3d(materials.add(crate::blueprint::BlueprintMaterial {
                    base: StandardMaterial {
                        base_color,
                        base_color_texture,
                        perceptual_roughness: 0.8,
                        ..default()
                    },
                    extension: crate::blueprint::BlueprintExtension {
                        high_color: LinearRgba::WHITE,
                        low_color: LinearRgba::WHITE,
                        grid_scale: 100.0,
                        line_width: 1.0,
                        subdivisions: Vec2::new(360.0, 180.0),
                        transition: (1.0f64 - (altitude / 50_000.0f64)).clamp(0.0, 1.0) as f32,
                        body_radius: body.radius_m as f32,
                        surface_color: LinearRgba::new(0.3, 0.3, 0.3, 1.0),
                        ..default()
                    },
                })),
            ));
            
            if let Some(collider) = data.collider {
                entity_cmds.insert((RigidBody::Static, collider));
            }
            
            entity_cmds.remove::<PendingTile>();
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
pub fn create_quadsphere_tile_mesh(body_ent: Entity, face: u8, level: u32, i: i32, j: i32, radius: f64, res: u32, registry: Option<&TerrainMapRegistry>, tile_center: DVec3) -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let mut uvs = Vec::new();
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

            // Equirectangular UV mapping (Seam handling with Mirrored fix)
            let mut u_raw = (-pos_sphere.z).atan2(pos_sphere.x);
            
            // Compute tile's geometric center from face parameters (not tile_center which may be ZERO)
            let center_u = start_u + step * 0.5;
            let center_v = start_v + step * 0.5;
            let tile_center_dir = cube_to_sphere(face, center_u, center_v);
            let ref_lon = (-tile_center_dir.z).atan2(tile_center_dir.x);
            if (u_raw - ref_lon) > std::f64::consts::PI {
                u_raw -= 2.0 * std::f64::consts::PI;
            } else if (u_raw - ref_lon) < -std::f64::consts::PI {
                u_raw += 2.0 * std::f64::consts::PI;
            }

            let u_tex = (u_raw + std::f64::consts::PI) / (2.0 * std::f64::consts::PI);
            let v_tex = (pos_sphere.y.asin() + (std::f64::consts::PI / 2.0)) / std::f64::consts::PI;
            uvs.push(Vec2::new(u_tex as f32, 1.0 - v_tex as f32)); // Flip V for Bevy
        }
    }

    for y in 0..res {
        for x in 0..res {
            let i0 = y * (res + 1) + x;
            let i1 = i0 + 1;
            let i2 = (y + 1) * (res + 1) + x;
            let i3 = i2 + 1;
            
            // CCW Winding for sides, CW for Top/Bottom
            if face == 2 || face == 3 {
                indices.push(i0); indices.push(i2); indices.push(i1);
                indices.push(i1); indices.push(i2); indices.push(i3);
            } else {
                indices.push(i0); indices.push(i1); indices.push(i2);
                indices.push(i1); indices.push(i3); indices.push(i2);
            }
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
            uvs.push(uvs[idx as usize]); // Extend UVs to skirt
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
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sample_height(body_ent: Entity, pos_sphere: DVec3, radius: f64, registry: Option<&TerrainMapRegistry>) -> f64 {
    let mut h = radius;
    let lat = pos_sphere.y.asin().to_degrees() as f32;
    let long = pos_sphere.x.atan2(pos_sphere.z).to_degrees() as f32;
    let pos_geo = Vec2::new(lat, long);

    if let Some(registry) = registry {
        for map in registry.maps.iter() {
            if map.body_entity != body_ent { continue; }
            let dist_deg = pos_geo.distance(map.center_lat_long);
            let radius_deg = (map.radius_m / radius as f32).to_degrees();
            if dist_deg < radius_deg {
                let t = smoothstep(0.8, 1.0, 1.0 - (dist_deg / radius_deg));
                h += (map.height_offset as f64) * (t as f64);
            }
        }
    }
    h
}

pub fn setup_terrain_overrides(mut registry: ResMut<TerrainMapRegistry>, q_bodies: Query<(Entity, &CelestialBody)>) {
    let mut maps = (*registry.maps).clone();
    for (ent, body) in q_bodies.iter() {
        if body.name == "Moon" {
            maps.push(CustomMap {
                name: "Langrenus Crater".into(),
                body_entity: ent,
                center_lat_long: Vec2::new(-8.9, 61.1),
                radius_m: 66_000.0,
                height_offset: -2000.0,
            });
        }
    }
    registry.maps = Arc::new(maps);
}
