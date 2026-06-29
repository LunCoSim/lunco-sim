//! Terrain tile mesh generation and sampling.

use bevy::prelude::*;
use bevy::math::DVec3;
use bevy::render::render_resource::PrimitiveTopology;
use bevy_mesh::Indices;
use crate::quad_sphere::cube_to_sphere;

/// Generate a mesh for a single QuadSphere tile.
pub fn create_quadsphere_tile_mesh(
    _body_ent: Entity,
    face: u8,
    level: u32,
    i: i32,
    j: i32,
    radius: f64,
    res: u32,
    tile_center: DVec3,
) -> Mesh {
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
            // Simple height: just radius (can be extended with noise/sampling)
            let h = radius;
            positions.push((pos_sphere * h - tile_center).as_vec3());
            normals.push(pos_sphere.as_vec3());

            // Equirectangular UV mapping
            let mut u_raw = (-pos_sphere.z).atan2(pos_sphere.x);
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
            uvs.push(Vec2::new(u_tex as f32, 1.0 - v_tex as f32));
        }
    }

    for y in 0..res {
        for x in 0..res {
            let i0 = y * (res + 1) + x;
            let i1 = i0 + 1;
            let i2 = (y + 1) * (res + 1) + x;
            let i3 = i2 + 1;

            // CCW for sides, CW for Top/Bottom
            if face == 2 || face == 3 {
                indices.push(i0); indices.push(i2); indices.push(i1);
                indices.push(i1); indices.push(i2); indices.push(i3);
            } else {
                indices.push(i0); indices.push(i1); indices.push(i2);
                indices.push(i1); indices.push(i3); indices.push(i2);
            }
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}
