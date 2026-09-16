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

/// Physical shape used by the kinematic avatar controller.
///
/// This is engine geometry, not a user preference and not a USD fact: the
/// avatar is a runtime camera embodiment rather than an authored rigid body.
/// Keeping the dimensions in one resource gives the controller one owner for
/// its measured shape without duplicating a collider in every scene.
#[derive(Resource, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Resource)]
pub struct AvatarCollisionSettings {
    /// Capsule radius in stage metres.
    pub radius_m: f64,
    /// Length of the capsule's central segment in stage metres.
    pub capsule_length_m: f64,
}

impl Default for AvatarCollisionSettings {
    fn default() -> Self {
        Self {
            radius_m: 0.35,
            capsule_length_m: 1.1,
        }
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
