//! `UsdGeomCamera` (`def Camera`) → Bevy `Camera` + [`SceneCamera`] intent.
//!
//! Scene files author cameras as **standard** USD `def Camera` prims; this
//! translator projects each to a Bevy `Camera` that keeps the prim's `Name`
//! and gets a `Projection` derived from the USD film-back attributes. The
//! render *pipeline* half (`Camera3d`, tonemapping, MSAA, bloom) is attached by
//! `lunco-render-bevy` when it observes [`SceneCamera`] — so a headless world
//! still holds a fully-formed camera and this crate links no wgpu. A
//! "switchable scene camera" is a [`SceneCamera`] whose `RenderTarget` is a
//! window. Which one renders is Bevy's own `Camera::is_active`; the switch
//! mechanism (`camera_switch`) toggles it and relocates the big_space
//! `FloatingOrigin`.
//!
//! Cameras therefore spawn **inactive** — exactly one window camera renders at
//! a time, and the avatar/free camera stays the default view until the user
//! (or a cutscene script) switches.
//!
//! ## Attribute mapping (UsdGeomCamera)
//! - `focalLength`, `verticalAperture` (mm) → perspective **vertical** FOV:
//!   `2·atan(verticalAperture / (2·focalLength))` (Bevy's `fov` is vertical).
//! - `clippingRange` (float2) → near / far.
//! - `projection` token (`perspective` | `orthographic`) → `Projection` variant.
//!
//! The prim's transform + visibility come from the shared path in
//! `instantiate_usd_prim`, so a camera nested under a moving prim (e.g. a
//! `def Camera "ChaseCam"` under a rover Xform) rides it via normal `ChildOf`
//! transform propagation — that's "camera on a rover" for free.

use bevy::camera::Exposure;
use bevy::prelude::*;
use lunco_render::SceneCamera;
use openusd::sdf::Path as SdfPath;

use crate::read::UsdRead;

/// `UsdGeomCamera` spec defaults (Pixar), so an unauthored attribute matches a
/// standard ~50 mm full-frame camera rather than Bevy's 45° default FOV.
const DEFAULT_FOCAL_LENGTH_MM: f32 = 50.0;
const DEFAULT_VERTICAL_APERTURE_MM: f32 = 15.2908;
/// USD's spec default `clippingRange` is `(1, 1_000_000)`; we tighten the near
/// plane a touch for close-up scene work (far stays huge for planet-scale views).
const DEFAULT_NEAR: f32 = 0.1;
const DEFAULT_FAR: f32 = 1.0e6;

/// Convert standard `UsdGeomCamera` photographic exposure into Bevy EV100.
///
/// This is deliberately shared by imported cameras and `LunCoAvatarAPI`
/// cameras: a camera's ISO, shutter time and f-stop have one USD spelling and
/// therefore one conversion. `exposure` is a post-photographic compensation in
/// USD, so positive compensation opens the effective exposure (lowers EV).
pub fn read_camera_exposure_ev100(reader: &crate::StageView<'_>, path: &SdfPath) -> Option<f32> {
    let authored = [
        "exposure:iso",
        "exposure:time",
        "exposure:fStop",
        "exposure:responsivity",
        "exposure",
    ]
    .iter()
    .any(|name| reader.real_f32(path, name).is_some());
    if !authored {
        return None;
    }

    let iso = reader.real_f32(path, "exposure:iso").unwrap_or(100.0);
    let time = reader.real_f32(path, "exposure:time").unwrap_or(1.0);
    let f_stop = reader.real_f32(path, "exposure:fStop").unwrap_or(1.0);
    let responsivity = reader
        .real_f32(path, "exposure:responsivity")
        .unwrap_or(1.0);
    let compensation = reader.real_f32(path, "exposure").unwrap_or(0.0);
    if !(iso.is_finite()
        && time.is_finite()
        && f_stop.is_finite()
        && responsivity.is_finite()
        && compensation.is_finite()
        && iso > 0.0
        && time > 0.0
        && f_stop > 0.0
        && responsivity > 0.0)
    {
        warn!(
            "[usd-bevy] {path} has invalid UsdGeomCamera exposure; using calibrated scene exposure"
        );
        return None;
    }
    Some((f_stop * f_stop / time * (100.0 / iso) / responsivity).log2() - compensation)
}

/// If `prim_type` is `Camera`, attach an **inactive** Bevy camera to `entity`
/// and return `true`. Called from `instantiate_usd_prim`; the prim's transform
/// and visibility are applied by the shared path there.
pub(crate) fn instantiate_camera_prim(
    reader: &crate::StageView<'_>,
    sdf_path: &SdfPath,
    prim_type: Option<&str>,
    commands: &mut Commands,
    entity: Entity,
) -> bool {
    if prim_type != Some("Camera") {
        return false;
    }

    let projection = read_projection(reader, sdf_path);
    let kind = match &projection {
        Projection::Orthographic(_) => "orthographic",
        _ => "perspective",
    };

    // Spawn INACTIVE: exactly one window scene camera renders at a time, and the
    // switch mechanism (lunco-avatar) chooses it by toggling `is_active`.
    //
    // `SceneCamera::agx()` (AgX tonemapping) + a calibrated `Exposure` mirror
    // the avatar camera's filmic look so a switch doesn't jump the grade. The
    // exposure is the shared `LUNAR_SUN_EXPOSURE_EV100` (EV 15) — the SAME
    // number `lunco_environment::LunarSun` defaults to and the celestial sun is
    // calibrated against — so the camera is exposed for the real ~131 klx sun
    // from frame one. Spawning at Bevy's `Exposure::default()` (EV 9.7) instead
    // left a load-time window in which the celestial system had already raised
    // the sun to 131 klux but the camera still sat ~5 stops too open, blowing
    // out the terrain until the late `project_env_settings`/celestial EV write
    // caught up (and on stage re-composition that window re-opened).
    //
    // NO `CellCoord` here — deliberately. `resolve_camera_mounts` re-parents
    // every nested camera to its enclosing grid and inserts the cell + `ChildOf`
    // ATOMICALLY. Stamping a cell now, while the camera still sits under its
    // USD parent, creates the one class big_space cannot propagate (a
    // cell-entity under a non-grid parent, doc 45 class 2) — it was the sole
    // source of the validator's spawn-frame reports. Until the resolver runs
    // (next Update at the latest), the camera is a plain Transform child of a
    // cell-entity: valid, propagated, and inactive anyway.
    commands.entity(entity).try_insert((
        Camera {
            is_active: false,
            ..default()
        },
        // The render-free scene-camera marker. `lunco-render-bevy` turns this
        // into `Camera3d` + `Tonemapping::AgX` + MSAA in render builds; headless
        // it stays pure scene data. Every "which entity is the scene camera?"
        // query filters `With<SceneCamera>`.
        SceneCamera::agx(),
        projection,
        Exposure {
            ev100: read_camera_exposure_ev100(reader, sdf_path)
                .unwrap_or(lunco_render::LUNAR_SUN_EXPOSURE_EV100),
        },
    ));

    info!(
        "[usd-bevy] {} Camera → inactive SceneCamera ({kind})",
        sdf_path.as_str()
    );
    true
}

/// Build a Bevy `Projection` from a `UsdGeomCamera`'s film-back + clip attrs.
fn read_projection(reader: &crate::StageView<'_>, path: &SdfPath) -> Projection {
    // `clippingRange` is a `float2` (accept `double2` authoring too).
    let [near, far] = reader
        .scalar::<[f32; 2]>(path, "clippingRange")
        .or_else(|| {
            reader
                .scalar::<[f64; 2]>(path, "clippingRange")
                .map(|[n, f]| [n as f32, f as f32])
        })
        .unwrap_or([DEFAULT_NEAR, DEFAULT_FAR]);

    let is_ortho = reader
        .text(path, "projection")
        .map(|t| t == "orthographic")
        .unwrap_or(false);

    if is_ortho {
        // A full mapping of USD orthographic aperture → Bevy's `ScalingMode` is
        // deferred (TODO): honour the clip range and use Bevy's default framing
        // for now so an authored ortho camera at least renders.
        Projection::Orthographic(OrthographicProjection {
            near,
            far,
            ..OrthographicProjection::default_3d()
        })
    } else {
        let focal = reader
            .real_f32(path, "focalLength")
            .unwrap_or(DEFAULT_FOCAL_LENGTH_MM);
        let v_aperture = reader
            .real_f32(path, "verticalAperture")
            .unwrap_or(DEFAULT_VERTICAL_APERTURE_MM);
        // Bevy's `PerspectiveProjection::fov` is the **vertical** field of view.
        let fov = if focal > 1e-3 {
            2.0 * (v_aperture / (2.0 * focal)).atan()
        } else {
            std::f32::consts::FRAC_PI_4
        };
        Projection::Perspective(PerspectiveProjection {
            fov,
            near,
            far,
            ..default()
        })
    }
}
