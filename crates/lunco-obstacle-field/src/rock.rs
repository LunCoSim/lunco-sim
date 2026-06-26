//! Procedural faceted rock meshes.
//!
//! A boulder is built from several overlapping, randomly-oriented boxes merged
//! into one mesh — angular and varied like real rubble, but cheap, flat-shaded,
//! and deterministic from a seed. Generated once per (size-bucket, variant) and
//! shared across all instances, so thousands of rocks cost a handful of meshes.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy_mesh::Indices;
use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;

fn axis(i: usize) -> Vec3 {
    match i % 3 {
        0 => Vec3::X,
        1 => Vec3::Y,
        _ => Vec3::Z,
    }
}

/// Append one oriented box (flat-shaded, per-face normals) to the buffers.
fn append_box(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    half: Vec3,
    offset: Vec3,
    rot: Quat,
) {
    let h = [half.x, half.y, half.z];
    for a in 0..3 {
        let b = (a + 1) % 3;
        let c = (a + 2) % 3;
        let na = axis(a);
        for &sign in &[-1.0f32, 1.0] {
            let center = na * sign * h[a];
            let du = axis(b) * h[b];
            let dv = axis(c) * h[c];
            let corners = [
                center - du - dv,
                center + du - dv,
                center + du + dv,
                center - du + dv,
            ];
            let normal = rot * (na * sign);
            let start = positions.len() as u32;
            for corner in corners {
                let p = rot * corner + offset;
                positions.push([p.x, p.y, p.z]);
                normals.push([normal.x, normal.y, normal.z]);
                uvs.push([0.0, 0.0]);
            }
            // Material is double-sided, so winding is cosmetic — keep it
            // consistent per face direction anyway.
            if sign > 0.0 {
                indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
            } else {
                indices.extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
            }
        }
    }
}

/// Build a faceted rock mesh of roughly `radius` metres from `cube_count` merged
/// boxes. `seed` makes the shape deterministic (and varied across seeds).
pub fn faceted_rock_mesh(seed: u64, cube_count: usize, radius: f32) -> Mesh {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for _ in 0..cube_count.max(1) {
        let s = radius * rng.gen_range(0.45..0.95);
        let half = Vec3::new(
            s * rng.gen_range(0.6..1.0),
            s * rng.gen_range(0.5..0.9),
            s * rng.gen_range(0.6..1.0),
        ) * 0.5;
        let offset = Vec3::new(
            rng.gen_range(-0.4..0.4),
            rng.gen_range(-0.15..0.35),
            rng.gen_range(-0.4..0.4),
        ) * radius;
        let rot = Quat::from_euler(
            EulerRot::XYZ,
            rng.gen_range(0.0..std::f32::consts::TAU),
            rng.gen_range(0.0..std::f32::consts::TAU),
            rng.gen_range(0.0..std::f32::consts::TAU),
        );
        append_box(&mut positions, &mut normals, &mut uvs, &mut indices, half, offset, rot);
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}
