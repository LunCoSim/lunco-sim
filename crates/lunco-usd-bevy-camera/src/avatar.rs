//! USD-authored avatar camera contracts.
//!
//! Standard `UsdGeomCamera` owns photographic and projection facts. The
//! `LunCoAvatarAPI` adds the behavior parameters that USD has no standard
//! vocabulary for. This module decodes that authored contract into the
//! renderer-independent [`lunco_avatar_core::camera::AvatarCameraIntent`].
//! Runtime realization remains in `lunco-avatar`; scenario policy remains in
//! Rhai.

use bevy::prelude::Transform;
use openusd::sdf::{Path as SdfPath, Value};

use crate::camera::read_camera_exposure_ev100;
use lunco_avatar_core::camera::{
    AvatarCameraIntent, AvatarCameraMode, AvatarFlightSettings, FollowAttitude,
};
use lunco_usd_bevy_core::read::{read_authored_bool_strict, read_vec3_f64, UsdReadObject};

/// An authored avatar camera contract is malformed at the named USD boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarCameraIntentError {
    code: &'static str,
    message: String,
}

impl AvatarCameraIntentError {
    /// Stable diagnostic code for the authored contract failure.
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// Human-readable explanation of the authored contract failure.
    pub fn message(&self) -> &str {
        &self.message
    }
}

fn invalid(code: &'static str, message: impl Into<String>) -> AvatarCameraIntentError {
    AvatarCameraIntentError {
        code,
        message: message.into(),
    }
}

/// Read the authored avatar camera contract from one composed USD prim.
///
/// `Ok(None)` means that the prim is not an avatar. Missing values use the
/// defaults declared by [`AvatarCameraIntent::default`], which mirror the
/// registered `LunCoAvatarAPI` schema fallbacks. An authored value with the
/// wrong type or range returns an error; it is never replaced by a Rust guess.
pub fn read_avatar_camera_intent(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    existing_transform: &Transform,
) -> Result<Option<AvatarCameraIntent>, AvatarCameraIntentError> {
    let is_avatar = read_authored_bool_strict(reader, path, "lunco:avatar").map_err(|_| {
        invalid(
            "avatar-attribute",
            "lunco:avatar must be an authored boolean",
        )
    })?;
    if !is_avatar.unwrap_or(false) {
        return Ok(None);
    }

    let defaults = AvatarCameraIntent::default();
    let mode = read_camera_mode(reader, path, defaults.mode)?;
    let mut yaw = read_camera_real(reader, path, "lunco:cameraYaw", defaults.yaw, "camera-yaw")?;
    let mut pitch = read_camera_real(
        reader,
        path,
        "lunco:cameraPitch",
        defaults.pitch,
        "camera-pitch",
    )?;

    if reader.has_authored_attribute(path, "lunco:cameraLookAt") {
        let look_at = read_vec3_f64(reader, path, "lunco:cameraLookAt").ok_or_else(|| {
            invalid(
                "camera-look-at",
                "authored lunco:cameraLookAt must be a finite double3",
            )
        })?;
        if !look_at.iter().all(|value| value.is_finite()) {
            return Err(invalid(
                "camera-look-at",
                "authored lunco:cameraLookAt must be a finite double3",
            ));
        }
        let eye = read_vec3_f64(reader, path, "xformOp:translate")
            .filter(|value| value.iter().all(|value| value.is_finite()))
            .map(|[x, y, z]| bevy::math::DVec3::new(x, y, z))
            .unwrap_or_else(|| existing_transform.translation.as_dvec3());
        let target = bevy::math::DVec3::from_array(look_at);
        if let Some(direction) = (target - eye).try_normalize() {
            pitch = direction.y.clamp(-1.0, 1.0).asin() as f32;
            yaw = (-direction.x).atan2(-direction.z) as f32;
        }
    }

    let flight_settings = read_flight_settings(reader, path, defaults.flight_settings)?;
    let orbit_distance = read_positive_real(
        reader,
        path,
        "lunco:camera:orbitDistance",
        defaults.orbit_distance,
        "camera-orbit-distance",
    )?;
    let spring_arm_distance = read_positive_real(
        reader,
        path,
        "lunco:camera:springArmDistance",
        defaults.spring_arm_distance,
        "camera-spring-arm-distance",
    )?;
    let spring_arm_vertical_offset = read_camera_real(
        reader,
        path,
        "lunco:camera:springArmVerticalOffset",
        defaults.spring_arm_vertical_offset,
        "camera-spring-arm-offset",
    )?;
    let spring_arm_track_heading = read_camera_bool(
        reader,
        path,
        "lunco:camera:springArmTrackHeading",
        defaults.spring_arm_track_heading,
        "camera-spring-arm-heading",
    )?;
    let spring_arm_attitude = read_attitude(reader, path, defaults.spring_arm_attitude)?;
    let exposure_ev100 = read_camera_exposure_ev100(reader, path).map_err(|_| {
        invalid(
            "avatar-exposure",
            "avatar camera exposure must be a finite EV100 value",
        )
    })?;

    Ok(Some(AvatarCameraIntent {
        mode,
        yaw,
        pitch,
        orbit_distance,
        spring_arm_distance,
        spring_arm_vertical_offset,
        spring_arm_track_heading,
        spring_arm_attitude,
        flight_settings,
        exposure_ev100,
    }))
}

fn read_camera_mode(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    default: AvatarCameraMode,
) -> Result<AvatarCameraMode, AvatarCameraIntentError> {
    let Some(value) = reader.attr_value(path, "lunco:cameraMode") else {
        if reader.has_authored_attribute(path, "lunco:cameraMode") {
            return Err(invalid(
                "camera-mode",
                "authored lunco:cameraMode is malformed",
            ));
        }
        return Ok(default);
    };
    let Value::Token(value) = value else {
        return Err(invalid(
            "camera-mode",
            "lunco:cameraMode must be a token: `freeflight`, `orbit`, or `springarm`",
        ));
    };
    match value.as_str() {
        "freeflight" => Ok(AvatarCameraMode::FreeFlight),
        "orbit" => Ok(AvatarCameraMode::Orbit),
        "springarm" => Ok(AvatarCameraMode::SpringArm),
        value => Err(invalid(
            "camera-mode",
            format!(
                "avatar camera mode `{value}` is unsupported; use `freeflight`, `orbit`, or `springarm`"
            ),
        )),
    }
}

fn read_camera_real(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    name: &str,
    default: f32,
    code: &'static str,
) -> Result<f32, AvatarCameraIntentError> {
    match reader.real_f32(path, name) {
        Some(value) if value.is_finite() => Ok(value),
        Some(_) => Err(invalid(code, format!("authored {name} must be finite"))),
        None if reader.has_authored_attribute(path, name) => Err(invalid(
            code,
            format!("authored {name} must be a finite real"),
        )),
        None => Ok(default),
    }
}

fn read_positive_real(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    name: &str,
    default: f64,
    code: &'static str,
) -> Result<f64, AvatarCameraIntentError> {
    match reader.real(path, name) {
        Some(value) if value.is_finite() && value > 0.0 => Ok(value),
        Some(value) => Err(invalid(
            code,
            format!("authored {name} must be finite and greater than zero, got {value}"),
        )),
        None if reader.has_authored_attribute(path, name) => Err(invalid(
            code,
            format!("authored {name} must be a finite positive real"),
        )),
        None => Ok(default),
    }
}

fn read_camera_bool(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    name: &str,
    default: bool,
    code: &'static str,
) -> Result<bool, AvatarCameraIntentError> {
    match read_authored_bool_strict(reader, path, name) {
        Ok(Some(value)) => Ok(value),
        Ok(None) => Ok(default),
        Err(_) => Err(invalid(code, format!("{name} must be an authored boolean"))),
    }
}

fn read_attitude(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    default: FollowAttitude,
) -> Result<FollowAttitude, AvatarCameraIntentError> {
    let Some(value) = reader.attr_value(path, "lunco:camera:springArmAttitude") else {
        if reader.has_authored_attribute(path, "lunco:camera:springArmAttitude") {
            return Err(invalid(
                "camera-spring-arm-attitude",
                "authored spring-arm attitude is malformed",
            ));
        }
        return Ok(default);
    };
    let Value::Token(value) = value else {
        return Err(invalid(
            "camera-spring-arm-attitude",
            "lunco:camera:springArmAttitude must be a token",
        ));
    };
    match value.as_str() {
        "heading" => Ok(FollowAttitude::Heading),
        "world_locked" => Ok(FollowAttitude::WorldLocked),
        "full_attitude" => Ok(FollowAttitude::FullAttitude),
        value => Err(invalid(
            "camera-spring-arm-attitude",
            format!("unsupported spring-arm attitude `{value}`"),
        )),
    }
}

fn read_flight_settings(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    defaults: AvatarFlightSettings,
) -> Result<AvatarFlightSettings, AvatarCameraIntentError> {
    let speed_mps = read_flight_real(reader, path, "lunco:avatar:flightSpeed", defaults.speed_mps)?;
    let boost_multiplier = read_flight_real(
        reader,
        path,
        "lunco:avatar:boostMultiplier",
        defaults.boost_multiplier,
    )?;
    let boost_threshold = read_flight_real(
        reader,
        path,
        "lunco:avatar:boostThreshold",
        defaults.boost_threshold,
    )?;
    let input_deadzone = read_flight_real(
        reader,
        path,
        "lunco:avatar:inputDeadzone",
        defaults.input_deadzone,
    )?;

    if speed_mps <= 0.0 {
        return Err(invalid(
            "avatar-flight-settings",
            "lunco:avatar:flightSpeed must be finite and greater than zero",
        ));
    }
    if boost_multiplier < 1.0 {
        return Err(invalid(
            "avatar-flight-settings",
            "lunco:avatar:boostMultiplier must be finite and at least one",
        ));
    }
    if !(0.0..=1.0).contains(&boost_threshold) {
        return Err(invalid(
            "avatar-flight-settings",
            "lunco:avatar:boostThreshold must be finite and within [0, 1]",
        ));
    }
    if !(0.0..1.0).contains(&input_deadzone) {
        return Err(invalid(
            "avatar-flight-settings",
            "lunco:avatar:inputDeadzone must be finite and within [0, 1)",
        ));
    }
    Ok(AvatarFlightSettings {
        speed_mps,
        boost_multiplier,
        boost_threshold,
        input_deadzone,
    })
}

fn read_flight_real(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    name: &str,
    default: f64,
) -> Result<f64, AvatarCameraIntentError> {
    match reader.real(path, name) {
        Some(value) if value.is_finite() => Ok(value),
        Some(_) => Err(invalid(
            "avatar-flight-settings",
            format!("{name} must be finite"),
        )),
        None if reader.has_authored_attribute(path, name) => Err(invalid(
            "avatar-flight-settings",
            format!("{name} must be a finite real"),
        )),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_bevy_core::canonical::CanonicalStage;
    use lunco_usd_compose::recipe::StageRecipe;

    fn read(source: &str) -> Result<Option<AvatarCameraIntent>, AvatarCameraIntentError> {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("avatar.usda", source))
            .expect("avatar fixture composes");
        let path = SdfPath::new("/Avatar").expect("avatar path");
        read_avatar_camera_intent(&stage.view(), &path, &Transform::default())
    }

    #[test]
    fn authored_avatar_uses_generic_camera_contract() {
        let intent = read(
            r#"#usda 1.0
def Xform "Avatar" (
    prepend apiSchemas = ["LunCoAvatarAPI"]
)
{
    bool lunco:avatar = true
    token lunco:cameraMode = "springarm"
    float lunco:camera:springArmDistance = 9.0
}
"#,
        )
        .expect("avatar intent reads")
        .expect("avatar is present");

        assert_eq!(intent.mode, AvatarCameraMode::SpringArm);
        assert_eq!(intent.spring_arm_distance, 9.0);
        assert_eq!(intent.spring_arm_attitude, FollowAttitude::Heading);
    }

    #[test]
    fn malformed_authored_look_at_is_rejected_at_usd_boundary() {
        let error = read(
            r#"#usda 1.0
def Xform "Avatar" (
    prepend apiSchemas = ["LunCoAvatarAPI"]
)
{
    bool lunco:avatar = true
    string lunco:cameraLookAt = "origin"
}
"#,
        )
        .expect_err("malformed look-at must fail");

        assert_eq!(error.code(), "camera-look-at");
    }
}
