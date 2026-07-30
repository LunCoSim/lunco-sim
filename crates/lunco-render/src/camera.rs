//! Camera **intent** — "this is the scene camera, and this is how it should look",
//! stated without naming a render pipeline.
//!
//! # Why
//!
//! `Camera3d`, `Tonemapping` and `Bloom` all live in `bevy_core_pipeline`, which
//! pulls `bevy_render` → wgpu + naga. And `Camera3d` is not just *spawned* — it is
//! used as the **query filter that identifies a scene camera** across
//! `camera_switch`, `camera_mount`, `camera_track` and their tests. So even a
//! perfect material decoupling leaves those crates linking wgpu, purely because
//! they ask "which entity is the 3D camera?".
//!
//! [`SceneCamera`] is that question, asked in a render-free way. `Camera` itself,
//! `Projection`, `Exposure` and `Visibility` are all `bevy_camera` and already free —
//! it is only the *pipeline* components that cost. So:
//!
//! - domain crates spawn `Camera` + [`SceneCamera`] and filter on `With<SceneCamera>`;
//! - `lunco-render-bevy` observes `SceneCamera` and attaches `Camera3d`, the
//!   tonemapper, MSAA and bloom.
//!
//! Headless keeps a fully-formed camera entity — pose, projection, tracking, mounts,
//! the lot — it simply never gets a render pipeline. That is what makes an offscreen
//! or server-side camera meaningful at all.
//!
//! # This also fixes two long-standing render bugs (review `R4`)
//!
//! 1. **MSAA was never configured anywhere in the workspace** — grep found zero
//!    `Msaa`. So Bevy's default `Sample4` was on *everywhere*, including WebGL2,
//!    where a 4× multisampled colour+depth target for a full-screen terrain is the
//!    single most expensive default in the build. [`SceneCamera::default`] picks
//!    [`MsaaLevel::Off`] on wasm and `X2` on native, deliberately.
//! 2. **Bloom was configured on non-HDR cameras** in four crates. `hdr` is set true
//!    nowhere in the repo, and bloom on a non-HDR view is at best a no-op with a
//!    downsample/upsample chain bolted on. Here bloom is `Option`, and the binder
//!    **refuses to attach it without `hdr`** rather than silently wasting the passes.

use bevy::camera::Exposure;
use bevy::prelude::*;

/// Projection used by the interactive camera when no authored USD camera
/// supplies a film back. These are the `UsdGeomCamera` specification defaults,
/// expressed as Bevy's vertical perspective FOV; the render pipeline never
/// owns or replaces this component, so Inspector edits remain authoritative.
pub fn usd_default_perspective_projection() -> Projection {
    const FOCAL_LENGTH_MM: f32 = 50.0;
    const VERTICAL_APERTURE_MM: f32 = 15.2908;
    let fov = 2.0 * (VERTICAL_APERTURE_MM / (2.0 * FOCAL_LENGTH_MM)).atan();
    Projection::Perspective(PerspectiveProjection {
        fov,
        near: 0.1,
        far: 1.0e6,
        ..default()
    })
}

/// A world-space text label — stated as data, so a domain crate can say "this thing
/// is labelled X" without linking a text/sprite render pipeline.
///
/// `Text2d` lives in `bevy_sprite`, whose `bevy_sprite_render` feature pulls
/// `bevy_render` → wgpu + naga. A spacecraft's *name* is simulation data; the
/// glyphs are not. So the name stays here and `lunco-render-bevy` builds the
/// `Text2d` + `TextFont` + `TextColor` in render builds.
///
/// This was the LAST edge dragging wgpu into the `--no-ui` server: one billboard
/// label on a spacecraft.
#[derive(Component, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct WorldLabel {
    pub text: String,
    /// Font size in pixels.
    pub size_px: f32,
    pub color: LinearRgba,
}

impl WorldLabel {
    pub fn new(text: impl Into<String>, size_px: f32) -> Self {
        Self {
            text: text.into(),
            size_px,
            color: LinearRgba::WHITE,
        }
    }
}

/// Tonemapping curve, named without depending on `bevy_core_pipeline`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum ToneMap {
    None,
    /// Bevy's default.
    TonyMcMapface,
    /// Filmic; default for LunCoSim (high contrast solar/lunar lighting).
    #[default]
    AgX,
    AcesFitted,
    Reinhard,
}

/// Multisample level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum MsaaLevel {
    Off,
    X2,
    X4,
}

/// Bloom, which is only meaningful on an HDR camera.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct BloomLook {
    pub intensity: f32,
    pub low_frequency_boost: f32,
}

impl Default for BloomLook {
    fn default() -> Self {
        Self {
            intensity: 0.15,
            low_frequency_boost: 0.7,
        }
    }
}

/// **The scene camera marker.** Filter on `With<SceneCamera>`, not `With<Camera3d>`.
///
/// Spawn it next to a `Camera` (and `Transform`/`Projection`, all render-free).
/// `lunco-render-bevy` attaches `Camera3d` + tonemapping + MSAA + bloom in render
/// builds; headless leaves the entity as pure scene data.
#[derive(Component, Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct SceneCamera {
    pub tone_map: ToneMap,
    /// Multisampling. **Off on wasm by default** — see the module docs.
    pub msaa: MsaaLevel,
    /// HDR render target. Required for [`bloom`](Self::bloom) to do anything.
    pub hdr: bool,
    /// Bloom. **Ignored (with a warning) unless `hdr` is true** — bloom on an LDR
    /// target is a no-op that still pays for the downsample/upsample chain.
    pub bloom: Option<BloomLook>,
}

impl Default for SceneCamera {
    fn default() -> Self {
        Self {
            tone_map: ToneMap::default(),
            // R4: MSAA was never set, so WebGL2 silently ran 4×. Off on the web —
            // the terrain shader's own footprint fades already do the AA that
            // actually matters at this scale.
            msaa: if cfg!(target_arch = "wasm32") {
                MsaaLevel::Off
            } else {
                MsaaLevel::X2
            },
            hdr: false,
            bloom: None,
        }
    }
}

impl SceneCamera {
    /// A camera with AgX tonemapping — what the USD scene cameras author.
    pub fn agx() -> Self {
        Self {
            tone_map: ToneMap::AgX,
            ..Default::default()
        }
    }

    /// Enable HDR **and** bloom together. They are one decision, not two: bloom
    /// without HDR is the exact bug this API exists to make unrepresentable.
    pub fn with_bloom(mut self, bloom: BloomLook) -> Self {
        self.hdr = true;
        self.bloom = Some(bloom);
        self
    }
}

/// **The one photometric setup every scene camera gets** — grade + exposure, as a
/// single pair no spawn path may split.
///
/// Exposure is not a per-camera preference: it is calibrated against the physical
/// sun the celestial system drives (`LUNAR_SUN_EXPOSURE_EV100`, the same number
/// `lunco_environment::LunarSun` defaults to). A camera that picks its own is
/// simply wrong by a measurable number of stops.
///
/// It exists because there were two spawn paths and only one of them was
/// calibrated: the USD camera projection paired `agx()` with the calibrated
/// exposure, while `lunco-avatar`'s "Avatar Camera" spawned `SceneCamera::default()`
/// and NO `Exposure` at all — so it rendered at Bevy's default EV 9.7 against a
/// ~131 klx sun, about five stops open, and every surface blew out to white. It
/// could not be repaired downstream either: `project_env_settings` writes exposure
/// only to cameras that already carry the component, so the avatar camera was
/// unreachable by the very system meant to keep exposure consistent.
///
/// `authored_ev100` is the scene's opinion (a `UsdGeomCamera`'s exposure attribute)
/// and wins when present — engine calibration is the fallback, never an override.
pub fn scene_camera_look(authored_ev100: Option<f32>) -> (SceneCamera, Exposure) {
    (
        // Keep the camera HDR so high-intensity emitters (especially the Sun)
        // produce a controlled bloom halo instead of clipping as hard white.
        SceneCamera::agx().with_bloom(BloomLook::default()),
        Exposure {
            ev100: authored_ev100.unwrap_or(crate::LUNAR_SUN_EXPOSURE_EV100),
        },
    )
}
