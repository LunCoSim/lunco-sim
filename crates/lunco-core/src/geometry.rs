//! Precision-preserving bounds shared by authored geometry and verification.
//!
//! These values stay in `f64` through SysML, Rhai, and geometry verification.
//! Adapters lower them only when the target USD schema or renderer requires it.

use crate::DTransform;
use bevy::math::{DQuat, DVec3};

/// An axis-aligned box in one explicitly shared coordinate frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds3 {
    min: DVec3,
    max: DVec3,
}

impl Bounds3 {
    /// Construct bounds from ordered, finite corners.
    pub fn new(min: DVec3, max: DVec3) -> Option<Self> {
        if !min.is_finite()
            || !max.is_finite()
            || !min.cmple(max).all()
            || !(max * 0.5 - min * 0.5).is_finite()
            || !(min * 0.5 + max * 0.5).is_finite()
        {
            return None;
        }
        Some(Self { min, max })
    }

    /// Construct bounds from a center and non-negative half extents.
    pub fn from_center_half_extents(center: DVec3, half_extents: DVec3) -> Option<Self> {
        if !center.is_finite() || !half_extents.is_finite() || half_extents.cmplt(DVec3::ZERO).any()
        {
            return None;
        }
        let min = center - half_extents;
        let max = center + half_extents;
        Self::new(min, max)
    }

    /// Lower corner in the box's declared frame.
    pub const fn min(self) -> DVec3 {
        self.min
    }

    /// Upper corner in the box's declared frame.
    pub const fn max(self) -> DVec3 {
        self.max
    }

    /// Midpoint computed without overflowing the intermediate sum.
    pub fn center(self) -> DVec3 {
        self.min * 0.5 + self.max * 0.5
    }

    /// Half-size on each local axis.
    pub fn half_extents(self) -> DVec3 {
        self.max * 0.5 - self.min * 0.5
    }
}

/// An oriented box represented by a center, local half extents, and rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrientedBounds3 {
    center: DVec3,
    half_extents: DVec3,
    rotation: DQuat,
}

impl OrientedBounds3 {
    /// Construct an oriented box, normalizing its finite non-zero rotation.
    pub fn new(center: DVec3, half_extents: DVec3, rotation: DQuat) -> Option<Self> {
        let rotation_length_squared = rotation.length_squared();
        if !center.is_finite()
            || !half_extents.is_finite()
            || half_extents.cmplt(DVec3::ZERO).any()
            || !rotation.is_finite()
            || !rotation_length_squared.is_finite()
            || rotation_length_squared <= 1.0e-24
        {
            return None;
        }
        let rotation = rotation.normalize();
        if !rotation.is_finite() {
            return None;
        }
        Some(Self {
            center,
            half_extents,
            rotation,
        })
    }

    /// Transform local axis-aligned bounds into a world-oriented box.
    pub fn from_local(bounds: Bounds3, pose: DTransform) -> Option<Self> {
        if !pose.is_finite() {
            return None;
        }
        Self::new(
            pose.transform_point(bounds.center())?,
            bounds.half_extents() * pose.scale.abs(),
            pose.rotation,
        )
    }

    /// World-space center.
    pub const fn center(self) -> DVec3 {
        self.center
    }

    /// Half extents in the box's local axes.
    pub const fn half_extents(self) -> DVec3 {
        self.half_extents
    }

    /// Unit rotation from the box's local axes into its declared world frame.
    pub const fn rotation(self) -> DQuat {
        self.rotation
    }

    /// Compare two boxes using all face normals and edge-cross-edge axes.
    ///
    /// The returned signed margin is positive when separated, zero at contact,
    /// and negative when intersecting. It is the greatest SAT-axis margin, not
    /// Euclidean closest-point distance; callers requiring a minimum clearance
    /// can use it as a conservative bound.
    pub fn relation(self, other: Self) -> BoundsRelation {
        let axes_a = [
            self.rotation * DVec3::X,
            self.rotation * DVec3::Y,
            self.rotation * DVec3::Z,
        ];
        let axes_b = [
            other.rotation * DVec3::X,
            other.rotation * DVec3::Y,
            other.rotation * DVec3::Z,
        ];
        let mut best_margin = f64::NEG_INFINITY;
        let mut best_axis = DVec3::ZERO;

        for axis in axes_a.into_iter().chain(axes_b) {
            consider_axis(axis, self, other, &mut best_margin, &mut best_axis);
        }
        for axis_a in axes_a {
            for axis_b in axes_b {
                consider_axis(
                    axis_a.cross(axis_b),
                    self,
                    other,
                    &mut best_margin,
                    &mut best_axis,
                );
            }
        }

        BoundsRelation {
            greatest_axis_margin: best_margin,
            axis_from_a_to_b: best_axis,
            separated: best_margin > 0.0,
        }
    }
}

/// Signed separating-axis result for an oriented-box pair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundsRelation {
    /// Greatest signed margin over the fifteen SAT axes, in metres.
    pub greatest_axis_margin: f64,
    /// The selected unit axis, oriented from the first box toward the second.
    pub axis_from_a_to_b: DVec3,
    /// Whether a strictly positive separating margin exists.
    pub separated: bool,
}

fn consider_axis(
    candidate: DVec3,
    a: OrientedBounds3,
    b: OrientedBounds3,
    best_margin: &mut f64,
    best_axis: &mut DVec3,
) {
    let length_squared = candidate.length_squared();
    if !length_squared.is_finite() || length_squared <= 1.0e-24 {
        return;
    }
    let mut axis = candidate / length_squared.sqrt();
    let signed_distance = (b.center - a.center).dot(axis);
    let distance = signed_distance.abs();
    let radius_a = projected_radius(a, axis);
    let radius_b = projected_radius(b, axis);
    let margin = distance - radius_a - radius_b;
    if margin > *best_margin {
        if signed_distance < 0.0 {
            axis = -axis;
        }
        *best_margin = margin;
        *best_axis = axis;
    }
}

fn projected_radius(bounds: OrientedBounds3, axis: DVec3) -> f64 {
    let local_axes = [
        bounds.rotation * DVec3::X,
        bounds.rotation * DVec3::Y,
        bounds.rotation * DVec3::Z,
    ];
    bounds.half_extents.x * axis.dot(local_axes[0]).abs()
        + bounds.half_extents.y * axis.dot(local_axes[1]).abs()
        + bounds.half_extents.z * axis.dot(local_axes[2]).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oriented_bounds_preserve_f64_translation_and_detect_axis_separation() {
        let a = OrientedBounds3::new(
            DVec3::new(1.0e8, 0.0, 0.0),
            DVec3::splat(0.5),
            DQuat::IDENTITY,
        )
        .unwrap();
        let b = OrientedBounds3::new(
            DVec3::new(1.0e8 + 1.25, 0.0, 0.0),
            DVec3::splat(0.5),
            DQuat::IDENTITY,
        )
        .unwrap();

        let relation = a.relation(b);
        assert!(relation.separated);
        assert!((relation.greatest_axis_margin - 0.25).abs() < 1.0e-9);
        assert_eq!(relation.axis_from_a_to_b, DVec3::X);
    }

    #[test]
    fn oriented_bounds_detect_a_rotated_intersection() {
        let a =
            OrientedBounds3::new(DVec3::ZERO, DVec3::new(1.0, 0.2, 0.2), DQuat::IDENTITY).unwrap();
        let b = OrientedBounds3::new(
            DVec3::new(0.0, 0.0, 0.1),
            DVec3::new(1.0, 0.2, 0.2),
            DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
        )
        .unwrap();

        let relation = a.relation(b);
        assert!(!relation.separated);
        assert!(relation.greatest_axis_margin < 0.0);
    }
}
