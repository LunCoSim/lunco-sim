//! Precision-preserving geometric values shared by simulation, authoring, and
//! scripting adapters.
//!
//! The renderer lowers these values to Bevy's `f32` transform only at the
//! presentation boundary.  Requirements, USD authoring, physics, and
//! Modelica-facing geometry keep the `f64` representation.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::Transform as BevyTransform;

/// A finite, precision-preserving rigid pose with component-wise scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DTransform {
    pub translation: DVec3,
    pub rotation: DQuat,
    pub scale: DVec3,
}

impl DTransform {
    pub const IDENTITY: Self = Self {
        translation: DVec3::ZERO,
        rotation: DQuat::IDENTITY,
        scale: DVec3::ONE,
    };

    /// Construct a pose after normalizing its quaternion.
    pub fn new(translation: DVec3, rotation: DQuat, scale: DVec3) -> Option<Self> {
        if !translation.is_finite() || !rotation.is_finite() || !scale.is_finite() {
            return None;
        }
        if rotation.length_squared() == 0.0 {
            return None;
        }
        let rotation = rotation.normalize();
        if !rotation.is_finite() {
            return None;
        }
        Some(Self {
            translation,
            rotation,
            scale,
        })
    }

    /// Whether all components satisfy the finite-pose invariant.
    pub fn is_finite(self) -> bool {
        self.translation.is_finite()
            && self.rotation.is_finite()
            && self.scale.is_finite()
            && self.rotation.length_squared() > 0.0
    }

    /// Compose `self` with a child-local pose without lowering to `f32`.
    pub fn compose(self, local: Self) -> Option<Self> {
        Self::new(
            self.translation + self.rotation * (self.scale * local.translation),
            self.rotation * local.rotation,
            self.scale * local.scale,
        )
    }

    /// Apply this pose to a point in its local frame.
    pub fn transform_point(self, point: DVec3) -> Option<DVec3> {
        let result = self.translation + self.rotation * (self.scale * point);
        result.is_finite().then_some(result)
    }

    /// Explicit presentation boundary.  No simulation or requirement code
    /// should call this conversion merely to perform geometric calculations.
    pub fn as_bevy_transform(self) -> BevyTransform {
        BevyTransform {
            translation: self.translation.as_vec3(),
            rotation: self.rotation.as_quat(),
            scale: self.scale.as_vec3(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_stays_in_f64_until_explicit_bevy_lowering() {
        let parent =
            DTransform::new(DVec3::new(1.0e8, 2.0, 3.0), DQuat::IDENTITY, DVec3::ONE).unwrap();
        let local =
            DTransform::new(DVec3::new(0.25, 0.5, 0.75), DQuat::IDENTITY, DVec3::ONE).unwrap();
        let composed = parent.compose(local).unwrap();
        assert_eq!(composed.translation, DVec3::new(1.0e8 + 0.25, 2.5, 3.75));
        assert_eq!(composed.as_bevy_transform().translation.x as f64, 1.0e8);
    }
}
