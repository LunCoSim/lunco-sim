//! One sampled terrain collision surface for discrete and swept contacts.

use avian3d::parry::shape::{HeightField, SharedShape, TriMeshFlags};
use avian3d::parry::utils::Array2;
use avian3d::prelude::Collider;
use bevy::math::DVec3;

/// Cook the exact Parry grid triangulation, including internal-edge normals.
/// The resulting native triangle mesh supports rotational CCD as well as
/// contact manifolds. Sampling, quantization, and rebasing stay with the oracle.
pub(crate) fn terrain_collider(heights: Vec<Vec<f64>>, scale: DVec3) -> Result<Collider, String> {
    let rows = heights.len();
    let cols = heights.first().map_or(0, Vec::len);
    if rows < 2
        || cols < 2
        || heights.iter().any(|column| column.len() != cols)
        || heights.iter().flatten().any(|height| !height.is_finite())
        || !scale.is_finite()
        || scale.min_element() <= 0.0
    {
        return Err(
            "terrain collider requires a finite rectangular grid and positive scale".into(),
        );
    }
    let grid = Array2::new(rows, cols, heights.into_iter().flatten().collect());
    let field = HeightField::new(grid, scale);
    let (vertices, indices) = field.to_trimesh();
    SharedShape::trimesh_with_flags(vertices, indices, TriMeshFlags::FIX_INTERNAL_EDGES)
        .map(Collider::from)
        .map_err(|error| format!("terrain triangle collision cook failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::parry::math::Pose;
    use avian3d::parry::query::{NonlinearRigidMotion, Ray, RayCast, cast_shapes_nonlinear};

    #[test]
    fn terrain_collision_supports_rotating_offset_pad_sweeps_and_preserves_samples() {
        // An asymmetric slope detects transposed grid data and lost rebasing.
        let heights = vec![
            vec![0.0, 0.2, 0.4],
            vec![0.5, 0.7, 0.9],
            vec![1.0, 1.2, 1.4],
        ];
        let field = HeightField::new(
            Array2::new(3, 3, heights.clone().into_iter().flatten().collect()),
            DVec3::new(8.0, 1.0, 8.0),
        );
        let collider = terrain_collider(heights, DVec3::new(8.0, 1.0, 8.0)).unwrap();
        for x in [-3.0, 0.0, 3.0] {
            for z in [-3.0, 0.0, 3.0] {
                let ray = Ray::new(DVec3::new(x, 5.0, z), -DVec3::Y);
                let original = field.cast_local_ray(&ray, 10.0, true).unwrap();
                let cooked = collider.shape().cast_local_ray(&ray, 10.0, true).unwrap();
                assert!((original - cooked).abs() < 1e-12);
            }
        }
        let pad = Collider::compound(vec![(
            DVec3::new(0.0, -1.32, 0.0),
            avian3d::prelude::Rotation::IDENTITY,
            Collider::cylinder(0.35, 0.04),
        )]);
        for angular_speed in [0.0, 0.3] {
            let moving = NonlinearRigidMotion::new(
                Pose::from_translation(DVec3::new(0.0, 3.0, 0.0)),
                DVec3::new(0.0, -1.26, 0.0),
                DVec3::new(0.0, -14.0, 0.0),
                DVec3::new(angular_speed, 0.0, 0.0),
            );
            let ground =
                NonlinearRigidMotion::new(Pose::IDENTITY, DVec3::ZERO, DVec3::ZERO, DVec3::ZERO);
            let hit = cast_shapes_nonlinear(
                &moving,
                pad.shape().as_ref(),
                &ground,
                collider.shape().as_ref(),
                0.0,
                0.25,
                false,
            )
            .unwrap()
            .expect("a descending pad must hit the sampled terrain");
            assert!(hit.time_of_impact > 0.0 && hit.time_of_impact < 0.25);
        }
        for invalid in [
            vec![],
            vec![vec![0.0]],
            vec![vec![0.0, 1.0], vec![0.0]],
            vec![vec![0.0, f64::NAN], vec![0.0, 0.0]],
        ] {
            assert!(terrain_collider(invalid, DVec3::ONE).is_err());
        }
    }
}
