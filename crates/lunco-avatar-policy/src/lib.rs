//! Twin-scoped avatar safety policy and physical controller settings.
//!
//! This package interprets the generic workspace setting that governs whether
//! an avatar may pass through projected soil colliders. The runtime and its UI
//! consume the same reader; neither keeps a second cache or policy default.

use bevy::prelude::*;

/// Twin-scoped policy key for intentionally allowing the local avatar to pass
/// through colliders. The safe behavior is the omitted-value default.
pub const AVATAR_ALLOW_THROUGH_SOIL_SETTING: &str = "avatar.allow_through_soil";

/// Runtime interpretation of the active Twin's avatar collision policy.
///
/// `Unavailable` is distinct from `CollisionEnabled`: a plain folder or a
/// host without a workspace has no Twin settings contract, but it still gets
/// the safe collision behavior. The value is read directly from the active
/// Twin, so no policy survives `TwinClosed` or a scene replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarSoilCollisionPolicy {
    /// The setting is absent or explicitly false; colliders are enforced.
    CollisionEnabled,
    /// The active Twin explicitly opted into unsafe traversal.
    ThroughSoilAllowed,
    /// No active Twin manifest is available; collision remains enforced.
    Unavailable,
}

/// Camera navigation envelope and bounded sweep/slide precision.
/// The sphere encloses the lens and near-plane corners. Scene rigid bodies
/// retain their independently authored physical shapes.
#[derive(Resource, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Resource)]
pub struct AvatarCollisionSettings {
    /// Minimum lens-centred camera radius, in metres.
    pub radius_m: f64,
    /// Separation maintained around both props and analytic terrain.
    pub skin_m: f64,
    /// Maximum geometric separation from a witnessed camera contact, in metres.
    pub sweep_tolerance_m: f64,
    /// Bounded number of contact-plane projections per input step.
    pub max_slides: usize,
}

impl Default for AvatarCollisionSettings {
    fn default() -> Self {
        Self {
            radius_m: 0.025,
            skin_m: 0.002,
            sweep_tolerance_m: 0.0001,
            max_slides: 4,
        }
    }
}

impl AvatarCollisionSettings {
    /// Enclose all near-plane corners using the current runtime projection.
    /// Invalid or absent lens state is a visible unavailable movement contract.
    pub fn camera_radius(&self, projection: &bevy::camera::Projection) -> Result<f64, String> {
        if !self.radius_m.is_finite()
            || self.radius_m <= 0.0
            || !self.skin_m.is_finite()
            || self.skin_m <= 0.0
            || !self.sweep_tolerance_m.is_finite()
            || self.sweep_tolerance_m <= 0.0
            || self.sweep_tolerance_m >= self.skin_m
            || self.max_slides == 0
            || self.max_slides > 16
        {
            return Err("invalid camera collision envelope settings".into());
        }
        let inverse = projection.get_clip_from_view().inverse();
        if !inverse.is_finite() {
            return Err("camera collision projection is singular".into());
        }
        let mut radius = self.radius_m;
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                let corner = inverse.project_point3(Vec3::new(x, y, 1.0));
                if !corner.is_finite() {
                    return Err("camera collision near plane is invalid".into());
                }
                radius = radius.max(corner.as_dvec3().length());
            }
        }
        Ok(radius)
    }
}

/// Interpret one generic Twin setting without coercing unrelated scalar types.
pub fn avatar_soil_collision_policy_from_setting(
    setting: Option<&lunco_workspace::TwinSettingValue>,
) -> Result<AvatarSoilCollisionPolicy, String> {
    match setting {
        None => Ok(AvatarSoilCollisionPolicy::CollisionEnabled),
        Some(lunco_workspace::TwinSettingValue::Bool(true)) => {
            Ok(AvatarSoilCollisionPolicy::ThroughSoilAllowed)
        }
        Some(lunco_workspace::TwinSettingValue::Bool(false)) => {
            Ok(AvatarSoilCollisionPolicy::CollisionEnabled)
        }
        Some(value) => Err(format!(
            "Twin setting `{AVATAR_ALLOW_THROUGH_SOIL_SETTING}` must be boolean, got {value:?}"
        )),
    }
}

/// Read the avatar traversal policy from the active Twin's existing settings
/// boundary. Missing workspace/session state is visible to the UI through
/// `Unavailable` and remains fail-closed in the movement owner.
pub fn avatar_soil_collision_policy(
    workspace: Option<&lunco_workspace::WorkspaceResource>,
) -> Result<AvatarSoilCollisionPolicy, String> {
    let Some(workspace) = workspace else {
        return Ok(AvatarSoilCollisionPolicy::Unavailable);
    };
    let Some(twin_id) = workspace.active_twin else {
        return Ok(AvatarSoilCollisionPolicy::Unavailable);
    };
    let Some(twin) = workspace.twin(twin_id) else {
        return Err(format!("active Twin {twin_id:?} is no longer present"));
    };
    let Some(manifest) = twin.manifest.as_ref() else {
        return Ok(AvatarSoilCollisionPolicy::Unavailable);
    };
    avatar_soil_collision_policy_from_setting(manifest.setting(AVATAR_ALLOW_THROUGH_SOIL_SETTING))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_envelope_encloses_near_plane_and_rejects_invalid_lens() {
        let settings = AvatarCollisionSettings::default();
        let projection =
            bevy::camera::Projection::Perspective(bevy::camera::PerspectiveProjection {
                near: 0.1,
                fov: std::f32::consts::FRAC_PI_2,
                aspect_ratio: 2.0,
                ..default()
            });
        let radius = settings.camera_radius(&projection).unwrap();
        assert!((radius - 0.1_f64 * 6.0_f64.sqrt()).abs() < 1e-6);
        let invalid = AvatarCollisionSettings {
            skin_m: f64::NAN,
            ..settings
        };
        assert!(invalid.camera_radius(&projection).is_err());
    }

    #[test]
    fn omitted_policy_is_safe_and_missing_session_is_visible() {
        assert_eq!(
            avatar_soil_collision_policy_from_setting(None),
            Ok(AvatarSoilCollisionPolicy::CollisionEnabled)
        );
        assert_eq!(
            avatar_soil_collision_policy(None),
            Ok(AvatarSoilCollisionPolicy::Unavailable)
        );
    }

    #[test]
    fn only_an_explicit_boolean_true_enables_traversal() {
        assert_eq!(
            avatar_soil_collision_policy_from_setting(Some(
                &lunco_workspace::TwinSettingValue::Bool(false),
            )),
            Ok(AvatarSoilCollisionPolicy::CollisionEnabled)
        );
        assert_eq!(
            avatar_soil_collision_policy_from_setting(Some(
                &lunco_workspace::TwinSettingValue::Bool(true),
            )),
            Ok(AvatarSoilCollisionPolicy::ThroughSoilAllowed)
        );
    }

    #[test]
    fn invalid_policy_type_is_an_explicit_error() {
        let error = avatar_soil_collision_policy_from_setting(Some(
            &lunco_workspace::TwinSettingValue::Text("true".into()),
        ))
        .expect_err("text must not be coerced into an unsafe opt-out");
        assert!(error.contains(AVATAR_ALLOW_THROUGH_SOIL_SETTING));
    }
}
