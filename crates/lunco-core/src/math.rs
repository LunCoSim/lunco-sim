//! Precision-preserving geometric values shared by simulation, authoring, and
//! scripting adapters.
//!
//! The renderer lowers these values to Bevy's `f32` transform only at the
//! presentation boundary.  Requirements, USD authoring, physics, and
//! Modelica-facing geometry keep the `f64` representation.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::Transform as BevyTransform;
use lunco_engineering_values::{CoordinateFrameId, FrameIdError};
use std::fmt;

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

/// A rigid transform that maps coordinates from one identified frame into
/// another. Length-unit conversion remains a separate engineering-value
/// operation; a coordinate transform never guesses the units of its frames.
#[derive(Clone, Debug, PartialEq)]
pub struct CoordinateTransform {
    source: CoordinateFrameId,
    target: CoordinateFrameId,
    transform: DTransform,
}

impl CoordinateTransform {
    /// Construct a finite rigid transform. Frame transforms cannot contain
    /// scale: callers must convert units explicitly before applying geometry.
    pub fn new(
        source: impl Into<String>,
        target: impl Into<String>,
        transform: DTransform,
    ) -> Result<Self, CoordinateTransformError> {
        let source = CoordinateFrameId::new(source).map_err(CoordinateTransformError::FrameId)?;
        let target = CoordinateFrameId::new(target).map_err(CoordinateTransformError::FrameId)?;
        if !transform.is_finite() {
            return Err(CoordinateTransformError::InvalidTransform);
        }
        if (transform.scale - DVec3::ONE).abs().max_element() > 1.0e-12 {
            return Err(CoordinateTransformError::ScaledFrameTransform);
        }
        let mut transform = DTransform::new(transform.translation, transform.rotation, DVec3::ONE)
            .ok_or(CoordinateTransformError::InvalidTransform)?;
        if source == target
            && (transform.translation.abs().max_element() > 1.0e-12
                || transform.rotation.dot(DQuat::IDENTITY).abs() < 1.0 - 1.0e-12)
        {
            return Err(CoordinateTransformError::NonIdentitySelfTransform);
        }
        if source == target {
            transform = DTransform::IDENTITY;
        }
        Ok(Self {
            source,
            target,
            transform,
        })
    }

    /// Frame whose coordinates are accepted by this transform.
    pub fn source(&self) -> &CoordinateFrameId {
        &self.source
    }

    /// Frame expressed by the transformed coordinates.
    pub fn target(&self) -> &CoordinateFrameId {
        &self.target
    }

    /// Apply this transform to a point tagged with the matching source frame.
    pub fn apply_position(
        &self,
        position: &FramedPosition,
    ) -> Result<FramedPosition, CoordinateTransformError> {
        require_frame(&self.source, &position.frame)?;
        let transformed = self
            .transform
            .transform_point(position.position)
            .ok_or(CoordinateTransformError::InvalidTransform)?;
        FramedPosition::new(self.target.as_str(), transformed)
    }

    /// Apply this transform to a pose tagged with the matching source frame.
    pub fn apply_pose(&self, pose: &FramedPose) -> Result<FramedPose, CoordinateTransformError> {
        require_frame(&self.source, &pose.frame)?;
        let transformed = self
            .transform
            .compose(pose.pose)
            .ok_or(CoordinateTransformError::InvalidTransform)?;
        FramedPose::new(self.target.as_str(), transformed)
    }

    /// Compose this transform followed by `next` when their frames connect.
    pub fn then(&self, next: &Self) -> Result<Self, CoordinateTransformError> {
        require_frame(&self.target, &next.source)?;
        let transform = next
            .transform
            .compose(self.transform)
            .ok_or(CoordinateTransformError::InvalidTransform)?;
        Self::new(self.source.as_str(), next.target.as_str(), transform)
    }

    /// Reverse the coordinate mapping.
    pub fn inverse(&self) -> Result<Self, CoordinateTransformError> {
        let rotation = self.transform.rotation.inverse();
        let transform =
            DTransform::new(rotation * -self.transform.translation, rotation, DVec3::ONE)
                .ok_or(CoordinateTransformError::InvalidTransform)?;
        Self::new(self.target.as_str(), self.source.as_str(), transform)
    }
}

/// A spatial position whose coordinate frame travels with its value.
#[derive(Clone, Debug, PartialEq)]
pub struct FramedPosition {
    frame: CoordinateFrameId,
    position: DVec3,
}

impl FramedPosition {
    /// Construct a finite position in a named coordinate frame.
    pub fn new(
        frame: impl Into<String>,
        position: DVec3,
    ) -> Result<Self, CoordinateTransformError> {
        if !position.is_finite() {
            return Err(CoordinateTransformError::InvalidPosition);
        }
        Ok(Self {
            frame: CoordinateFrameId::new(frame).map_err(CoordinateTransformError::FrameId)?,
            position,
        })
    }

    /// Frame in which `position` is expressed.
    pub fn frame(&self) -> &CoordinateFrameId {
        &self.frame
    }

    /// Position coordinates in `frame`.
    pub fn position(&self) -> DVec3 {
        self.position
    }
}

/// A rigid pose whose coordinate frame travels with its value.
#[derive(Clone, Debug, PartialEq)]
pub struct FramedPose {
    frame: CoordinateFrameId,
    pose: DTransform,
}

impl FramedPose {
    /// Construct a finite pose in a named coordinate frame.
    pub fn new(
        frame: impl Into<String>,
        pose: DTransform,
    ) -> Result<Self, CoordinateTransformError> {
        let pose = DTransform::new(pose.translation, pose.rotation, pose.scale)
            .ok_or(CoordinateTransformError::InvalidTransform)?;
        Ok(Self {
            frame: CoordinateFrameId::new(frame).map_err(CoordinateTransformError::FrameId)?,
            pose,
        })
    }

    /// Frame in which `pose` is expressed.
    pub fn frame(&self) -> &CoordinateFrameId {
        &self.frame
    }

    /// Pose expressed in `frame`.
    pub fn pose(&self) -> DTransform {
        self.pose
    }
}

fn require_frame(
    expected: &CoordinateFrameId,
    actual: &CoordinateFrameId,
) -> Result<(), CoordinateTransformError> {
    if expected == actual {
        Ok(())
    } else {
        Err(CoordinateTransformError::FrameMismatch {
            expected: expected.to_string(),
            actual: actual.to_string(),
        })
    }
}

/// A failed frame-tagged transform operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoordinateTransformError {
    /// The source or target frame name was empty.
    FrameId(FrameIdError),
    /// A pose contains non-finite or degenerate values.
    InvalidTransform,
    /// A frame conversion must be rigid; unit scale belongs in unit conversion.
    ScaledFrameTransform,
    /// A framed position contains non-finite coordinates.
    InvalidPosition,
    /// A transform from a frame to itself must be the identity.
    NonIdentitySelfTransform,
    /// Adjacent transforms or a value do not use the expected connecting frame.
    FrameMismatch { expected: String, actual: String },
}

impl fmt::Display for CoordinateTransformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameId(error) => fmt::Display::fmt(error, formatter),
            Self::InvalidTransform => {
                formatter.write_str("coordinate transform must be finite and non-degenerate")
            }
            Self::ScaledFrameTransform => formatter.write_str(
                "coordinate frame transforms cannot contain scale; convert units explicitly",
            ),
            Self::InvalidPosition => formatter.write_str("framed position must be finite"),
            Self::NonIdentitySelfTransform => {
                formatter.write_str("a coordinate frame cannot transform to itself non-identically")
            }
            Self::FrameMismatch { expected, actual } => write!(
                formatter,
                "coordinate frame mismatch: expected `{expected}`, got `{actual}`"
            ),
        }
    }
}

impl std::error::Error for CoordinateTransformError {}

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
