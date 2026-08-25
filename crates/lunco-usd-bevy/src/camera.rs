//! `UsdGeomCamera` (`def Camera`) → render-free camera intent.
//!
//! Scene files author cameras as **standard** USD `def Camera` prims; this
//! translator projects each to a Bevy `Camera` that keeps the prim's `Name`
//! and gets a `Projection` derived from the USD film-back attributes. The
//! render *pipeline* half (`Camera3d`, its render graph, tonemapping, MSAA, bloom) is attached by
//! `lunco-render-bevy` when it observes [`SceneCamera`] — so a headless world
//! still holds a fully-formed camera and this crate links no wgpu. A
//! "switchable scene camera" is a [`SceneCamera`] whose `RenderTarget` is a
//! window. Which one renders is Bevy's own `Camera::is_active`; the switch
//! mechanism (`camera_switch`) toggles it while the persistent world origin
//! tracker follows the selected camera's authoritative f64 pose.
//!
//! Cameras therefore spawn **inactive** — exactly one window camera renders at
//! a time, and an authored camera request or cutscene selection must explicitly
//! activate the presentation camera.
//!
//! ## Attribute mapping (UsdGeomCamera)
//! - `focalLength`, `verticalAperture` (mm) → perspective **vertical** FOV:
//!   `2·atan(verticalAperture / (2·focalLength))` (Bevy's `fov` is vertical).
//! - `horizontalAperture` (mm) → authored aspect, conformed per USD's default
//!   `aspectRatioConformPolicy = "expandAperture"`: on a window narrower than
//!   the authored aspect the vertical FOV expands so the authored horizontal
//!   FOV stays visible (Bevy already expands horizontally on wider windows).
//! - orthographic apertures are **tenths of scene units** →
//!   `aperture / 10 × metersPerUnit` world units; mapped to
//!   `ScalingMode::AutoMin` so neither authored aperture is ever cropped.
//! - `clippingRange` (float2, in stage units) → near / far in canonical metres.
//! - `projection` token (`perspective` | `orthographic`) → `Projection` variant.
//!
//! The prim's authored transform + visibility come from the shared path in
//! `instantiate_usd_prim`. An explicitly authored `cameraPose = "mounted"`
//! camera is then realised by `camera_mount` as a grid-direct presentation
//! follower; ordinary cameras retain their authored USD hierarchy.

use bevy::prelude::*;
use openusd::sdf::{Path as SdfPath, Value};

use crate::read::UsdRead;
use crate::units::StageMetrics;

/// `UsdGeomCamera` spec defaults (Pixar), so an unauthored attribute matches a
/// standard ~50 mm full-frame camera rather than Bevy's 45° default FOV.
const DEFAULT_FOCAL_LENGTH_MM: f32 = 50.0;
const DEFAULT_VERTICAL_APERTURE_MM: f32 = 15.2908;
const DEFAULT_HORIZONTAL_APERTURE_MM: f32 = 20.955;
/// USD's schema defaults. Keep these aligned with `UsdGeomCamera` rather than
/// introducing an importer-only camera profile.
const DEFAULT_NEAR: f32 = 1.0;
const DEFAULT_FAR: f32 = 1.0e6;

/// A USD camera has exactly one writer for its pose. This is projected from
/// `LunCoAvatarAPI` or `LunCoCameraAPI`; systems dispatch from this explicit
/// role rather than inferring intent from the prim hierarchy.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsdCameraPose {
    /// USD transform composition and animation own the pose.
    #[default]
    Authored,
    /// The avatar rig owns the grid-local pose interactively.
    Avatar,
    /// A rigid follower owns the grid-local pose from the authored parent.
    Mounted,
    /// A cinematic path owns the grid-local pose.
    Path,
}

/// A camera whose rendered output belongs to an instrument rather than the
/// main window. It deliberately does not carry [`SceneCamera`].
#[derive(Component, Default, Debug, Clone, Copy)]
pub struct UsdSensorCamera;

/// Convert standard `UsdGeomCamera` photographic exposure into Bevy EV100.
///
/// This is deliberately shared by imported cameras and `LunCoAvatarAPI`
/// cameras: a camera's ISO, shutter time and f-stop have one USD spelling and
/// therefore one conversion. `exposure` is a post-photographic compensation in
/// USD, so positive compensation opens the effective exposure (lowers EV).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraExposureError {
    /// An authored exposure field is not a finite value of the required sign,
    /// has an unsupported type, or produces a non-finite EV.
    InvalidAuthoredValue,
}

pub fn read_camera_exposure_ev100(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
) -> Result<Option<f32>, CameraExposureError> {
    let authored = [
        "exposure:iso",
        "exposure:time",
        "exposure:fStop",
        "exposure:responsivity",
        "exposure",
    ]
    .iter()
    .any(|name| reader.has_authored_attribute(path, name));
    if !authored {
        // The standard USD schema's fallback values are not a photographic
        // override. The renderer calibrates omitted exposure against the active
        // physical sun instead.
        return Ok(None);
    }

    let iso = read_camera_exposure_real(reader, path, "exposure:iso", 100.0, true)?;
    let time = read_camera_exposure_real(reader, path, "exposure:time", 1.0, true)?;
    let f_stop = read_camera_exposure_real(reader, path, "exposure:fStop", 1.0, true)?;
    let responsivity = read_camera_exposure_real(reader, path, "exposure:responsivity", 1.0, true)?;
    let compensation = read_camera_exposure_real(reader, path, "exposure", 0.0, false)?;
    let ev100 = (f_stop * f_stop / time * (100.0 / iso) / responsivity).log2() - compensation;
    if !ev100.is_finite() {
        error!("[usd-bevy] {path} has an authored camera exposure that produces a non-finite EV");
        return Err(CameraExposureError::InvalidAuthoredValue);
    }
    Ok(Some(ev100))
}

fn read_camera_exposure_real(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    name: &str,
    schema_default: f32,
    must_be_positive: bool,
) -> Result<f32, CameraExposureError> {
    let value = match reader.real_f32(path, name) {
        Some(value) => value,
        None if reader.has_authored_attribute(path, name) => {
            error!(
                "[usd-bevy] {path} has an authored camera exposure {name} with an unsupported value type"
            );
            return Err(CameraExposureError::InvalidAuthoredValue);
        }
        None => schema_default,
    };
    if !(value.is_finite() && (!must_be_positive || value > 0.0)) {
        error!(
            "[usd-bevy] {path} has invalid camera exposure {name} = {value}; expected a finite {} value",
            if must_be_positive { "positive" } else { "real" }
        );
        return Err(CameraExposureError::InvalidAuthoredValue);
    }
    Ok(value)
}

/// If `prim_type` is `Camera`, attach the camera intent to `entity` and return
/// `true`. The render binding later adds an **inactive** Bevy `Camera3d` with a
/// complete render graph. Called from `instantiate_usd_prim`; the prim's
/// transform and visibility are applied by the shared path there.
pub(crate) fn instantiate_camera_prim(
    reader: &crate::StageView<'_>,
    sdf_path: &SdfPath,
    prim_type: Option<&str>,
    commands: &mut Commands,
    entity: Entity,
    quality: lunco_render::RenderQualityProfile,
) -> bool {
    if prim_type != Some("Camera") {
        return false;
    }

    let Some((projection, h_fov)) = read_projection(reader, sdf_path) else {
        // The prim is still handled as a USD camera, but it must not enter a
        // render role with an invalid Bevy projection. The authored value is
        // reported at the point where it becomes invalid; silently choosing a
        // perspective or 45° fallback would hide a broken scene contract.
        return true;
    };
    let kind = match &projection {
        Projection::Orthographic(_) => "orthographic",
        _ => "perspective",
    };

    // Spawn INACTIVE: exactly one window scene camera renders at a time, and the
    // switch mechanism (lunco-avatar) chooses it by toggling `is_active`.
    //
    // `SceneCamera::agx()` (AgX tonemapping) + the authoritative Graphics
    // profile's `Exposure` mirror the avatar camera's filmic look so a switch
    // doesn't jump the grade. Spawning at Bevy's `Exposure::default()` (EV 9.7) instead
    // left a load-time window in which the celestial system had already raised
    // the sun to 131 klux but the camera still sat ~5 stops too open, blowing
    // out the terrain until the late `project_env_settings`/celestial EV write
    // caught up (and on stage re-composition that window re-opened).
    //
    // Only an explicitly mounted camera is reparented to its grid. Authored,
    // path-driven, and avatar cameras keep their normal USD hierarchy.
    // Camera purpose and pose are authored semantics, never hierarchy guesses.
    // An avatar is a viewport camera with an interactive pose. Every other
    // participating camera must apply LunCoCameraAPI and state both roles.
    let is_avatar = if reader.has_api_schema(sdf_path, "LunCoAvatarAPI") {
        match read_camera_bool(reader, sdf_path, "lunco:avatar", false) {
            Some(value) => value,
            None => return true,
        }
    } else {
        false
    };
    let has_camera_api = reader.has_api_schema(sdf_path, "LunCoCameraAPI");
    let (is_viewport, is_sensor, pose) = if is_avatar {
        (true, false, UsdCameraPose::Avatar)
    } else if !has_camera_api {
        warn!(
            "[usd-bevy] {} Camera has no LunCoCameraAPI; it is not a viewport or sensor camera",
            sdf_path.as_str()
        );
        (false, false, UsdCameraPose::Authored)
    } else {
        let pose = match read_camera_token(
            reader,
            sdf_path,
            "lunco:cameraPose",
            "authored",
            &["authored", "mounted"],
        ) {
            Some(value) if value == "authored" => UsdCameraPose::Authored,
            Some(value) if value == "mounted" => UsdCameraPose::Mounted,
            None => return true,
            Some(_) => unreachable!("camera token helper validates its allowed values"),
        };
        // A mounted follower has a fixed camera-local offset. Time samples on
        // that same prim would create two pose authors, so reject the invalid
        // combination at projection rather than silently letting a later system
        // overwrite animation every frame.
        let invalid_mounted_animation = pose == UsdCameraPose::Mounted
            && reader
                .attr_names(sdf_path)
                .iter()
                .any(|name| name.starts_with("xformOp:") && name.ends_with(".timeSamples"));
        if invalid_mounted_animation {
            error!(
                "[usd-bevy] {} declares lunco:cameraPose=mounted and camera xform timeSamples; mounted cameras require a static local offset",
                sdf_path.as_str()
            );
            // Reject this camera from runtime roles. Reclassifying it as an
            // authored viewport would hide an invalid two-writer declaration.
            return true;
        } else {
            match read_camera_token(
                reader,
                sdf_path,
                "lunco:cameraRole",
                "viewport",
                &["viewport", "sensor"],
            ) {
                Some(value) if value == "viewport" => (true, false, pose),
                Some(value) if value == "sensor" => (false, true, pose),
                None => return true,
                Some(_) => unreachable!("camera token helper validates its allowed values"),
            }
        }
    };

    let camera_exposure = if is_viewport {
        match read_camera_exposure_ev100(reader, sdf_path) {
            Ok(exposure) => exposure,
            Err(_) => return true,
        }
    } else {
        None
    };

    // A scene camera is an intent until the render binding runs.  Do not add a
    // bare `Camera` here: Bevy 0.19 validates `Camera` at insertion time and
    // emits a missing-render-graph warning before `lunco-render-bevy` can add
    // `Camera3d`.  The render binder inserts `Camera3d` (and its required
    // `Camera`, `CameraRenderGraph`, and target) as one complete pipeline.
    // Keeping only the projection here also preserves the render-free USD
    // bridge and makes headless camera projection data available immediately.
    let mut camera = commands.entity(entity);
    camera.try_insert((projection, pose));
    if is_viewport {
        camera.try_insert((
            lunco_render::scene_camera_look_with_profile(camera_exposure, quality),
            lunco_render::GraphicsCameraDefaults,
        ));
    }
    if is_sensor {
        camera.try_insert(UsdSensorCamera);
    }

    // Conform the authored horizontal aperture against the real window aspect
    // (USD's default `aspectRatioConformPolicy = "expandAperture"`). Bevy takes
    // aspect from the window and expands *horizontally* on wide windows for
    // free; the missing half is a window NARROWER than the authored aspect,
    // where the vertical FOV must expand so the authored horizontal FOV stays
    // visible. Deferred to a queued command because the window isn't reachable
    // from the projection path (and headless there is none — skip).
    if let Some(h_fov) = h_fov {
        commands.queue(move |world: &mut World| {
            let mut windows =
                world.query_filtered::<&bevy::window::Window, With<bevy::window::PrimaryWindow>>();
            let aspect = windows
                .single(world)
                .ok()
                .map(|w| w.resolution.width() / w.resolution.height());
            let Some(aspect) = aspect else { return };
            if let Some(mut projection) = world.get_mut::<Projection>(entity) {
                if let Projection::Perspective(p) = &mut *projection {
                    p.fov = conform_vertical_fov(h_fov, p.fov, aspect);
                }
            }
        });
    }

    let role = if is_viewport {
        "viewport"
    } else if is_sensor {
        "sensor"
    } else {
        "unassigned"
    };
    info!("[usd-bevy] {} Camera → {role} ({kind})", sdf_path.as_str());
    true
}

/// Build a Bevy `Projection` from a `UsdGeomCamera`'s film-back + clip attrs.
///
/// Returns the projection plus, for perspective, the **horizontal** FOV
/// derived from `horizontalAperture` — the window aspect isn't known here, so
/// the caller conforms the vertical FOV against it (expandAperture).
///
/// `None` means an authored camera attribute is malformed or outside the USD
/// camera contract. Schema defaults are still accepted when an attribute is
/// genuinely unauthored; an invalid authored opinion is never converted into a
/// guessed projection.
fn read_projection(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
) -> Option<(Projection, Option<f32>)> {
    // `clippingRange` is a `float2` (accept `double2` authoring too).
    let meters_per_unit = StageMetrics::from_reader(reader).ok()?.meters_per_unit as f32;
    let resolved_clipping = reader
        .scalar::<[f32; 2]>(path, "clippingRange")
        .or_else(|| {
            reader
                .scalar::<[f64; 2]>(path, "clippingRange")
                .map(|[n, f]| [n as f32, f as f32])
        });
    let clipping = match resolved_clipping {
        Some(range) => Some(range),
        None if reader.has_authored_attribute(path, "clippingRange") => {
            error!(
                "[usd-bevy] {} has an authored clippingRange with an unsupported value type",
                path.as_str()
            );
            return None;
        }
        None => None,
    };
    // USD stores this range in world units. Bevy's projection consumes the
    // canonical metre frame, so the resolved USD range (including a schema
    // fallback) is scaled from the stage's declared units.
    let [near, far] = clipping
        .unwrap_or([DEFAULT_NEAR, DEFAULT_FAR])
        .map(|value| value * meters_per_unit);
    if !(near.is_finite() && far.is_finite() && near > 0.0 && far > near) {
        error!(
            "[usd-bevy] {} has invalid camera clippingRange ({near}, {far}); expected finite 0 < near < far",
            path.as_str()
        );
        return None;
    }

    let projection_token = read_camera_token(
        reader,
        path,
        "projection",
        "perspective",
        &["perspective", "orthographic"],
    )?;
    let is_ortho = projection_token == "orthographic";

    if is_ortho {
        // Orthographic apertures are **tenths of scene units** (USD's aperture
        // convention: aperture / 10 × metersPerUnit = world units). `AutoMin`
        // keeps at least the authored width AND height visible and expands the
        // other axis for the window aspect — Bevy's native expandAperture.
        let h_aperture = read_positive_camera_real(
            reader,
            path,
            "horizontalAperture",
            DEFAULT_HORIZONTAL_APERTURE_MM,
        )?;
        let v_aperture = read_positive_camera_real(
            reader,
            path,
            "verticalAperture",
            DEFAULT_VERTICAL_APERTURE_MM,
        )?;
        Some((
            Projection::Orthographic(OrthographicProjection {
                near,
                far,
                scaling_mode: ortho_scaling_mode(h_aperture, v_aperture, meters_per_unit),
                ..OrthographicProjection::default_3d()
            }),
            None,
        ))
    } else {
        let focal =
            read_positive_camera_real(reader, path, "focalLength", DEFAULT_FOCAL_LENGTH_MM)?;
        let v_aperture = read_positive_camera_real(
            reader,
            path,
            "verticalAperture",
            DEFAULT_VERTICAL_APERTURE_MM,
        )?;
        let h_aperture = read_positive_camera_real(
            reader,
            path,
            "horizontalAperture",
            DEFAULT_HORIZONTAL_APERTURE_MM,
        )?;
        // Bevy's `PerspectiveProjection::fov` is the **vertical** field of view.
        let fov = 2.0 * (v_aperture / (2.0 * focal)).atan();
        let h_fov = Some(2.0 * (h_aperture / (2.0 * focal)).atan());
        Some((
            Projection::Perspective(PerspectiveProjection {
                fov,
                near,
                far,
                ..default()
            }),
            h_fov,
        ))
    }
}

/// Read a positive USD camera scalar without mistaking an invalid authored
/// opinion for an omitted schema default.
fn read_positive_camera_real(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    name: &str,
    schema_default: f32,
) -> Option<f32> {
    match reader.real_f32(path, name) {
        Some(value) if value.is_finite() && value > 0.0 => Some(value),
        Some(value) => {
            error!(
                "[usd-bevy] {} has invalid camera {} = {value}; expected a finite positive value",
                path.as_str(),
                name
            );
            None
        }
        None if reader.has_authored_attribute(path, name) => {
            error!(
                "[usd-bevy] {} has an authored camera {} with an unsupported value type",
                path.as_str(),
                name
            );
            None
        }
        None => Some(schema_default),
    }
}

/// Read a schema token while preserving the distinction between an omitted
/// attribute (which may use its documented USD default) and malformed authored
/// data. Custom camera-role tokens are control-plane data, so falling through to
/// another role would create a second pose/viewport owner by accident.
fn read_camera_token(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    name: &str,
    schema_default: &str,
    allowed: &[&str],
) -> Option<String> {
    match reader.attr_value(path, name) {
        Some(Value::Token(value)) => {
            let value = value.to_string();
            if allowed.contains(&value.as_str()) {
                Some(value)
            } else {
                error!(
                    "[usd-bevy] {} has unsupported {} token `{}`",
                    path.as_str(),
                    name,
                    value
                );
                None
            }
        }
        Some(_) => {
            error!(
                "[usd-bevy] {} has authored {} with an unsupported value type",
                path.as_str(),
                name
            );
            None
        }
        None if reader.has_authored_attribute(path, name) => {
            error!(
                "[usd-bevy] {} has authored {} with an unsupported value type",
                path.as_str(),
                name
            );
            None
        }
        None => Some(schema_default.to_string()),
    }
}

/// Read the standard USD bool without turning an authored type mismatch into
/// `false`. The schema fallback remains available only when the attribute is
/// genuinely omitted.
fn read_camera_bool(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    name: &str,
    schema_default: bool,
) -> Option<bool> {
    match reader.attr_value(path, name) {
        Some(Value::Bool(value)) => Some(value),
        Some(_) => {
            error!(
                "[usd-bevy] {} has authored {} with an unsupported value type",
                path.as_str(),
                name
            );
            None
        }
        None if reader.has_authored_attribute(path, name) => {
            error!(
                "[usd-bevy] {} has authored {} with an unsupported value type",
                path.as_str(),
                name
            );
            None
        }
        None => Some(schema_default),
    }
}

/// USD orthographic aperture (tenths of scene units) → Bevy `ScalingMode`.
fn ortho_scaling_mode(
    h_aperture: f32,
    v_aperture: f32,
    meters_per_unit: f32,
) -> bevy::camera::ScalingMode {
    bevy::camera::ScalingMode::AutoMin {
        min_width: h_aperture / 10.0 * meters_per_unit,
        min_height: v_aperture / 10.0 * meters_per_unit,
    }
}

/// USD's default `aspectRatioConformPolicy = "expandAperture"` for a Bevy
/// perspective camera: Bevy already expands **horizontally** when the window is
/// wider than the authored aspect (fov is vertical, aspect from the window), so
/// only the narrow case needs help — expand the vertical FOV until the authored
/// horizontal FOV fits. Neither authored aperture is ever cropped.
fn conform_vertical_fov(h_fov: f32, v_fov: f32, window_aspect: f32) -> f32 {
    let half_h = (h_fov * 0.5).tan();
    let half_v = (v_fov * 0.5).tan();
    if !(window_aspect.is_finite() && window_aspect > 0.0 && half_h > 0.0 && half_v > 0.0) {
        return v_fov;
    }
    let authored_aspect = half_h / half_v;
    if window_aspect >= authored_aspect {
        v_fov
    } else {
        2.0 * (half_h / window_aspect).atan()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ortho_aperture_is_tenths_of_scene_units_times_meters_per_unit() {
        // Spec defaults: 20.955 / 15.2908 tenths → 2.0955 × 1.52908 world units.
        let bevy::camera::ScalingMode::AutoMin {
            min_width,
            min_height,
        } = ortho_scaling_mode(
            DEFAULT_HORIZONTAL_APERTURE_MM,
            DEFAULT_VERTICAL_APERTURE_MM,
            1.0,
        )
        else {
            panic!("ortho aperture must map to AutoMin");
        };
        assert!((min_width - 2.0955).abs() < 1e-4);
        assert!((min_height - 1.52908).abs() < 1e-4);

        // A centimetre stage (metersPerUnit = 0.01) scales the viewport too.
        let bevy::camera::ScalingMode::AutoMin { min_height, .. } =
            ortho_scaling_mode(200.0, 100.0, 0.01)
        else {
            panic!("ortho aperture must map to AutoMin");
        };
        assert!((min_height - 0.1).abs() < 1e-6);
    }

    #[test]
    fn authored_clipping_range_converts_from_stage_units_to_metres() {
        let recipe = crate::canonical::StageRecipe::from_source(
            "camera.usda",
            "#usda 1.0\n(\n    metersPerUnit = 0.01\n)\ndef Camera \"Camera\"\n{\n    float2 clippingRange = (10, 1000)\n}\n",
        );
        let stage = crate::canonical::CanonicalStage::from_recipe(&recipe).expect("build camera");
        let path = SdfPath::new("/Camera").unwrap();
        let (projection, _) = read_projection(&stage.view(), &path).expect("valid camera");
        let Projection::Perspective(perspective) = projection else {
            panic!("camera defaults to perspective");
        };
        assert!((perspective.near - 0.1).abs() < 1e-6);
        assert!((perspective.far - 10.0).abs() < 1e-5);
    }

    #[test]
    fn omitted_clipping_range_uses_usd_defaults_in_stage_units() {
        let recipe = crate::canonical::StageRecipe::from_source(
            "camera.usda",
            "#usda 1.0\n(\n    metersPerUnit = 0.01\n)\ndef Camera \"Camera\"\n{}\n",
        );
        let stage = crate::canonical::CanonicalStage::from_recipe(&recipe).expect("build camera");
        let path = SdfPath::new("/Camera").unwrap();
        let (projection, _) = read_projection(&stage.view(), &path).expect("valid camera");
        let Projection::Perspective(perspective) = projection else {
            panic!("camera defaults to perspective");
        };
        assert!((perspective.near - 0.01).abs() < 1e-6);
        assert!((perspective.far - 10_000.0).abs() < 1e-3);
    }

    #[test]
    fn invalid_authored_clipping_range_is_rejected() {
        let recipe = crate::canonical::StageRecipe::from_source(
            "camera.usda",
            "#usda 1.0\ndef Camera \"Camera\"\n{\n    float2 clippingRange = (0, 100)\n}\n",
        );
        let stage = crate::canonical::CanonicalStage::from_recipe(&recipe).expect("build camera");
        let path = SdfPath::new("/Camera").unwrap();
        assert!(read_projection(&stage.view(), &path).is_none());
    }

    #[test]
    fn invalid_authored_focal_length_is_rejected_instead_of_using_a_heuristic_fov() {
        let recipe = crate::canonical::StageRecipe::from_source(
            "camera.usda",
            "#usda 1.0\ndef Camera \"Camera\"\n{\n    float focalLength = 0\n}\n",
        );
        let stage = crate::canonical::CanonicalStage::from_recipe(&recipe).expect("build camera");
        let path = SdfPath::new("/Camera").unwrap();
        assert!(read_projection(&stage.view(), &path).is_none());
    }

    #[test]
    fn authored_camera_projection_wrong_type_or_token_is_rejected() {
        for projection in [
            "string projection = \"orthographic\"",
            "uniform token projection = \"fisheye\"",
        ] {
            let source = format!("#usda 1.0\ndef Camera \"Camera\"\n{{\n    {projection}\n}}\n");
            let recipe = crate::canonical::StageRecipe::from_source("camera.usda", &source);
            let stage =
                crate::canonical::CanonicalStage::from_recipe(&recipe).expect("build camera");
            let path = SdfPath::new("/Camera").unwrap();
            assert!(read_projection(&stage.view(), &path).is_none());
        }
    }

    #[test]
    fn omitted_camera_exposure_uses_scene_calibration() {
        let recipe = crate::canonical::StageRecipe::from_source(
            "camera.usda",
            "#usda 1.0\ndef Camera \"Camera\"\n{\n}\n",
        );
        let stage = crate::canonical::CanonicalStage::from_recipe(&recipe).expect("build camera");
        let path = SdfPath::new("/Camera").unwrap();
        assert_eq!(read_camera_exposure_ev100(&stage.view(), &path), Ok(None));
    }

    #[test]
    fn authored_camera_exposure_converts_once_from_usd_fields() {
        let recipe = crate::canonical::StageRecipe::from_source(
            "camera.usda",
            "#usda 1.0\ndef Camera \"Camera\"\n{\n    float exposure:iso = 200\n    float exposure = 1\n}\n",
        );
        let stage = crate::canonical::CanonicalStage::from_recipe(&recipe).expect("build camera");
        let path = SdfPath::new("/Camera").unwrap();
        let ev100 = read_camera_exposure_ev100(&stage.view(), &path)
            .expect("valid exposure")
            .expect("authored exposure");
        assert!((ev100 + 2.0).abs() < 1e-6);
    }

    #[test]
    fn invalid_authored_camera_exposure_is_rejected() {
        let recipe = crate::canonical::StageRecipe::from_source(
            "camera.usda",
            "#usda 1.0\ndef Camera \"Camera\"\n{\n    float exposure:iso = 0\n}\n",
        );
        let stage = crate::canonical::CanonicalStage::from_recipe(&recipe).expect("build camera");
        let path = SdfPath::new("/Camera").unwrap();
        assert_eq!(
            read_camera_exposure_ev100(&stage.view(), &path),
            Err(CameraExposureError::InvalidAuthoredValue)
        );
    }

    #[test]
    fn authored_camera_exposure_with_wrong_type_is_rejected() {
        let recipe = crate::canonical::StageRecipe::from_source(
            "camera.usda",
            "#usda 1.0\ndef Camera \"Camera\"\n{\n    string exposure:iso = \"film\"\n}\n",
        );
        let stage = crate::canonical::CanonicalStage::from_recipe(&recipe).expect("build camera");
        let path = SdfPath::new("/Camera").unwrap();
        assert_eq!(
            read_camera_exposure_ev100(&stage.view(), &path),
            Err(CameraExposureError::InvalidAuthoredValue)
        );
    }

    #[test]
    fn conform_keeps_vertical_fov_on_wide_windows() {
        let h_fov = 0.9_f32;
        let v_fov = 0.7_f32;
        // Wider than authored: Bevy expands horizontally by itself.
        assert_eq!(conform_vertical_fov(h_fov, v_fov, 4.0), v_fov);
        // Exactly the authored aspect: unchanged.
        let authored = (h_fov * 0.5).tan() / (v_fov * 0.5).tan();
        assert!((conform_vertical_fov(h_fov, v_fov, authored) - v_fov).abs() < 1e-6);
    }

    #[test]
    fn conform_expands_vertical_fov_on_narrow_windows() {
        let h_fov = 0.9_f32;
        let v_fov = 0.7_f32;
        let narrow = 0.5_f32;
        let expanded = conform_vertical_fov(h_fov, v_fov, narrow);
        assert!(expanded > v_fov);
        // The authored horizontal FOV is exactly preserved at this aspect:
        // 2·atan(tan(v'/2) · aspect) == h_fov.
        let effective_h = 2.0 * ((expanded * 0.5).tan() * narrow).atan();
        assert!((effective_h - h_fov).abs() < 1e-5);
    }

    #[test]
    fn conform_ignores_degenerate_aspect() {
        assert_eq!(conform_vertical_fov(0.9, 0.7, 0.0), 0.7);
        assert_eq!(conform_vertical_fov(0.9, 0.7, f32::NAN), 0.7);
        assert_eq!(conform_vertical_fov(0.9, 0.7, -1.0), 0.7);
    }
}
