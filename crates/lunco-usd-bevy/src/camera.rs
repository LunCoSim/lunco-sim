//! `UsdGeomCamera` (`def Camera`) → Bevy `Camera3d`.
//!
//! Scene files author cameras as **standard** USD `def Camera` prims; this
//! translator projects each to a Bevy `Camera3d` that keeps the prim's `Name`
//! and gets a `Projection` derived from the USD film-back attributes. There is
//! deliberately **no bespoke camera marker**: a "switchable scene camera" is
//! just a `Camera3d` whose `RenderTarget` is a window. Which one renders is
//! Bevy's own `Camera::is_active`; the switch mechanism (in `lunco-avatar`)
//! toggles it and relocates the big_space `FloatingOrigin`.
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
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
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

/// If `prim_type` is `Camera`, attach an **inactive** Bevy camera to `entity`
/// and return `true`. Called from `instantiate_usd_prim`; the prim's transform
/// and visibility are applied by the shared path there.
pub(crate) fn instantiate_camera_prim<R: UsdRead>(
    reader: &R,
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

    // Spawn INACTIVE: exactly one window `Camera3d` renders at a time, and the
    // switch mechanism (lunco-avatar) chooses it by toggling `is_active`.
    //
    // `Tonemapping::AgX` + a placeholder `Exposure` mirror the avatar camera's
    // filmic look so a switch doesn't jump the grade. The activation system
    // re-syncs `Exposure` to the active-scene sun (the same source as the sun
    // illuminance) so lux and EV move together — without it a lunar scene
    // camera renders at Blender-default ev9.7 and blows out the terrain.
    //
    // NO `CellCoord` here — deliberately. `resolve_camera_mounts` re-parents
    // every nested camera to its enclosing grid and inserts the cell + `ChildOf`
    // ATOMICALLY. Stamping a cell now, while the camera still sits under its
    // USD parent, creates the one class big_space cannot propagate (a
    // cell-entity under a non-grid parent, doc 45 class 2) — it was the sole
    // source of the validator's spawn-frame reports. Until the resolver runs
    // (next Update at the latest), the camera is a plain Transform child of a
    // cell-entity: valid, propagated, and inactive anyway.
    commands.entity(entity).insert((
        Camera3d::default(),
        Camera {
            is_active: false,
            ..default()
        },
        projection,
        Tonemapping::AgX,
        Exposure::default(),
    ));

    info!(
        "[usd-bevy] {} Camera → inactive Camera3d ({kind})",
        sdf_path.as_str()
    );
    true
}

/// Build a Bevy `Projection` from a `UsdGeomCamera`'s film-back + clip attrs.
fn read_projection<R: UsdRead>(reader: &R, path: &SdfPath) -> Projection {
    // `clippingRange` is a `float2` (accept `double2` authoring too).
    let [near, far] = reader
        .scalar::<[f32; 2]>(path, "clippingRange")
        .or_else(|| {
            reader
                .scalar::<[f64; 2]>(path, "clippingRange")
                .map(|[n, f]| [n as f32, f as f32])
        })
        .unwrap_or([DEFAULT_NEAR, DEFAULT_FAR]);

    let is_ortho = crate::read_token(reader, path, "projection")
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
