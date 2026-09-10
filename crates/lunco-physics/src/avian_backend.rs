//! Admission checks for the Avian backend representation boundary.
//!
//! The simulation keeps physics poses in `f64`, while Avian's OBVHS spatial
//! backend consumes `f32` points and AABBs. This module owns that one shared
//! contract. Callers remain responsible for their lifecycle decision and
//! diagnostic subject; they must not invent a second representability rule.

use avian3d::prelude::{Collider, SimpleCollider};
use bevy::math::{DQuat, DVec3};

/// Check a vector before it crosses into an Avian backend value.
#[inline]
pub fn avian_backend_vector_is_valid(value: DVec3) -> bool {
    value.is_finite() && value.as_vec3().is_finite()
}

/// Check a point before it crosses into an Avian backend value.
#[inline]
pub fn avian_backend_point_is_valid(point: DVec3) -> bool {
    avian_backend_vector_is_valid(point)
}

/// Check a rotation before it crosses into Avian.
#[inline]
pub fn avian_backend_rotation_is_valid(rotation: DQuat) -> bool {
    rotation.is_finite()
        && rotation.as_quat().is_finite()
        && rotation.length_squared() > f64::EPSILON
}

/// Check a pose before it crosses into Avian.
#[inline]
pub fn avian_backend_pose_is_valid(position: DVec3, rotation: DQuat) -> bool {
    avian_backend_point_is_valid(position) && avian_backend_rotation_is_valid(rotation)
}

/// Check the AABB Avian's collider tree will store and grow.
#[inline]
pub fn avian_backend_aabb_is_valid(min: DVec3, max: DVec3) -> bool {
    avian_backend_point_is_valid(min)
        && avian_backend_point_is_valid(max)
        && min.cmple(max).all()
        && min.as_vec3().cmple(max.as_vec3()).all()
}

/// Check a collider's local shape bounds before ECS insertion.
#[inline]
pub fn avian_backend_collider_shape_is_valid(collider: &Collider) -> bool {
    let aabb = collider.aabb(DVec3::ZERO, DQuat::IDENTITY);
    avian_backend_aabb_is_valid(aabb.min, aabb.max)
}

/// Check the structural precondition for a Parry compound child.
///
/// Parry deliberately rejects nested composite shapes when constructing a
/// `Compound`. Composite colliders remain valid as standalone Avian colliders;
/// this predicate is only for callers that are about to aggregate a child into
/// another compound.
#[inline]
pub fn avian_backend_collider_is_leaf(collider: &Collider) -> bool {
    collider.shape_scaled().as_composite_shape().is_none()
}

/// Return the backend shape kind for a diagnostic at an admission boundary.
#[inline]
pub fn avian_backend_collider_shape_kind(collider: &Collider) -> String {
    format!("{:?}", collider.shape_scaled().shape_type())
}

#[cfg(test)]
mod tests {
    use super::{
        avian_backend_aabb_is_valid, avian_backend_collider_is_leaf,
        avian_backend_collider_shape_is_valid, avian_backend_point_is_valid,
        avian_backend_pose_is_valid, avian_backend_rotation_is_valid,
    };
    use avian3d::prelude::Collider;
    use bevy::math::{DQuat, DVec3};

    #[test]
    fn backend_admission_rejects_nonrepresentable_geometry() {
        assert!(avian_backend_point_is_valid(DVec3::ZERO));
        assert!(!avian_backend_point_is_valid(DVec3::new(
            f64::MAX,
            0.0,
            0.0
        )));
        assert!(avian_backend_rotation_is_valid(DQuat::IDENTITY));
        assert!(!avian_backend_rotation_is_valid(DQuat::from_xyzw(
            0.0, 0.0, 0.0, 0.0
        )));
        assert!(avian_backend_pose_is_valid(DVec3::ZERO, DQuat::IDENTITY));
        assert!(avian_backend_aabb_is_valid(DVec3::ZERO, DVec3::ONE));
        assert!(!avian_backend_aabb_is_valid(DVec3::ONE, DVec3::ZERO));
        assert!(!avian_backend_aabb_is_valid(
            DVec3::ZERO,
            DVec3::new(f64::MAX, 0.0, 0.0)
        ));
        assert!(avian_backend_collider_shape_is_valid(&Collider::cuboid(
            1.0, 1.0, 1.0
        )));
        assert!(avian_backend_collider_is_leaf(&Collider::cuboid(
            1.0, 1.0, 1.0
        )));
        assert!(!avian_backend_collider_is_leaf(&Collider::compound(vec![
            (
                DVec3::ZERO,
                DQuat::IDENTITY,
                Collider::cuboid(1.0, 1.0, 1.0)
            ),
        ])));
    }
}
