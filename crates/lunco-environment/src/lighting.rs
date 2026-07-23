//! # Lighting — the sky's light sources, as physical parameters
//!
//! The single, documented source of truth for **what the bodies in the lunar
//! sky *are*** as far as lighting and rendering are concerned: how bright they
//! shine ([illuminance](LunarSun::illuminance_lux)), how big they appear
//! ([angular diameter](LunarSun::angular_diameter_deg)), and the camera
//! exposure that pairs with the key light.
//!
//! This lives in `lunco-environment` because **lighting is environmental
//! state** — the lighting analog of gravity. Every consumer that reads these
//! values (the camera spawns in `lunco-celestial` / `lunco-sandbox` /
//! `lunco-usd-sim`, and the runtime `SetEnvironmentLight` tuner here) already
//! sits at or above this crate. The lone exception is the `lunco-usd-bevy`
//! `DistantLight` loader, which sits *below* environment and therefore cannot
//! read these — but it never needs to: it builds its light from *authored* USD
//! attributes (`intensity`/`exposure`/`inputs:angle`), with its own local
//! fallbacks. The render-side `lunco_render::LunarSunShadow` (cascade/bias/atlas)
//! is the separate shadow-config home.
//!
//! ## Two real light sources
//! The airless Moon's surface is lit by exactly two things: the **Sun** (the
//! hard key light) and **earthshine** (Earth's faint blue reflected fill).
//! Both are defined here so they read as one coherent picture.
//!
//! ## TODO — make this realtime
//! These are **static almanac values** for the Shackleton-region surface. The
//! intended end state is ephemeris-driven: Sun direction + distance (hence
//! illuminance and angular size) and Earth phase (hence earthshine) computed
//! from sim time / orbital position by a runtime `Sun`/`Earth` entity. When
//! that lands, the constants here become the **fallback/default** and the live
//! values flow from that entity.

use bevy::prelude::*;

/// The Sun as seen from the lunar surface (Sol) — the hard key light.
///
/// Also the one active-scene **`Resource`**: the sun spawn and every camera's
/// [`Exposure`](bevy::camera::Exposure) read it, so illuminance (lux) and
/// exposure (EV100) always move together. A scene that dims the sun therefore
/// cannot leave a camera over-/under-exposed — that exact mismatch produced a
/// black viewport (a 10 klx sandbox sun under a 128 klx-tuned EV15 camera).
/// [`Default`] is the canonical lunar calibration; a non-lunar scene (the
/// sandbox) `insert_resource`s its own studio values before plugins are added.
#[derive(Debug, Clone, Copy, PartialEq, Resource)]
pub struct LunarSun {
    /// Direct solar illuminance on a surface facing the Sun, **lux**.
    /// ~128 000 lx on the airless Moon (vs ~100 000 lx through Earth's
    /// atmosphere). This is the scene's key-light brightness — the **1 AU
    /// calibration**: in ephemeris-driven scenes `update_sun_light_system`
    /// (lunco-celestial) scales the live light by 1/r² of the site body's
    /// actual solar distance.
    pub illuminance_lux: f32,
    /// Apparent angular **diameter** of the Sun, **degrees** (~0.53° from the
    /// Moon — essentially identical to the view from Earth). Sets the
    /// soft-shadow penumbra width in the horizon ray-march.
    pub angular_diameter_deg: f32,
    /// Camera exposure (**EV100**) matched to [`illuminance_lux`](Self::illuminance_lux).
    /// Bevy renders physically (final pixel ≈ luminance ÷ 2^ev100), so exposure
    /// and key-light lux **must move together** — that is why the matched value
    /// is stored alongside the lux rather than hard-coded at each camera. ev100
    /// 15 (≈ `Exposure::SUNLIGHT`) lands 0.13-albedo regolith at mid-gray under
    /// the ~128 k lx Sun; raise it to darken the image, lower it to brighten.
    pub exposure_ev100: f32,
}

impl Default for LunarSun {
    fn default() -> Self {
        Self {
            illuminance_lux: 128_000.0,
            angular_diameter_deg: 0.53,
            // Shared with `lunco-usd-bevy`'s USD camera spawn via the one
            // constant both crates can reach (`lunco_render::
            // LUNAR_SUN_EXPOSURE_EV100`). Inlining a literal here would let it
            // drift from the camera spawn and re-open the load-time blowout
            // window where the 131 klx sun renders against an EV-9.7 camera.
            exposure_ev100: lunco_render::LUNAR_SUN_EXPOSURE_EV100,
        }
    }
}

/// Earthshine — Earth's reflected sunlight, the Moon's only other natural light.
/// A faint, cool-blue, **shadowless** fill that lifts sun-shadowed regolith into
/// readable relief without washing the shadow cores grey (which a flat ambient
/// would). The runtime fill light is spawned from these values by
/// [`spawn_earthshine`](crate::spawn_earthshine).
///
/// Named `EarthshineParams` (not `Earthshine`) to stay distinct from the
/// [`Earthshine`](crate::Earthshine) *marker component* on the spawned light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EarthshineParams {
    /// Fill illuminance, **lux** (~10–15 lx, ≈ 1/10 000 of the Sun).
    pub illuminance_lux: f32,
    /// Fill colour, **linear RGB** — cool blue (Earth's albedo skews blue).
    pub color: [f32; 3],
}

impl Default for EarthshineParams {
    fn default() -> Self {
        Self {
            illuminance_lux: 12.0,
            color: [0.6, 0.75, 1.0],
        }
    }
}
