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
//! values (the camera spawns in `lunco-celestial` / `lunco-luncosim` /
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
//! Both are described here so they read as one coherent picture — though only
//! the Sun keeps a resource, because only the Sun's values are still static.
//!
//! ## Earthshine is already realtime; the Sun is not
//! [`drive_earthshine_from_phase`] computes the fill from the live Sun–Earth–site
//! geometry each frame, so [`FULL_EARTH_EARTHSHINE_LUX`] is a calibration
//! (the value at full Earth) rather than the value in use.
//!
//! [`LunarSun`] is still **static almanac values** for the Shackleton-region
//! surface. The intended end state is the same one earthshine reached: Sun
//! direction + distance (hence illuminance and angular size) from sim time and
//! orbital position, at which point the constants here become the fallback.
//! `lunco-celestial`'s `update_sun_light_system` already does the 1/r² part for
//! an anchored scene.

use bevy::prelude::*;

/// The Sun as seen from the lunar surface (Sol) — the hard key light.
///
/// Also the one active-scene **`Resource`**: the sun spawn and every camera's
/// [`Exposure`](bevy::camera::Exposure) read it, so illuminance (lux) and
/// exposure (EV100) always move together. A scene that dims the sun therefore
/// cannot leave a camera over-/under-exposed — that exact mismatch produced a
/// black viewport (a 10 klx sandbox sun under a 128 klx-tuned EV16 camera).
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
    /// 16 lands 0.13-albedo regolith at mid-gray under
    /// the ~128 k lx Sun; raise it to darken the image, lower it to brighten.
    pub exposure_ev100: f32,
}

impl Default for LunarSun {
    fn default() -> Self {
        Self {
            illuminance_lux: 128_000.0,
            // Same one-constant rule as `exposure_ev100` below: the USD
            // `DistantLight` loader sits under this crate and needs the same
            // number as the fallback for an unauthored `inputs:angle`.
            angular_diameter_deg: lunco_core::SOLAR_ANGULAR_DIAMETER_DEG,
            // The balanced Graphics profile owns the unauthored camera
            // exposure. A live scene may replace it with an authored
            // environment/camera opinion through the normal command path.
            exposure_ev100: lunco_render::RenderingQuality::Balanced
                .profile()
                .camera_exposure_ev100,
        }
    }
}

/// Earthshine at **full Earth** — the peak of the fill, in lux.
///
/// A full Earth seen from the Moon is roughly 50× brighter than a full Moon
/// seen from Earth (≈0.25 lx), which puts the peak in the low tens of lux. This
/// is the calibration; the value the light actually carries is this scaled by
/// Earth's illuminated fraction, which is what [`drive_earthshine_from_phase`]
/// computes.
///
/// The tint is NOT here: it is `inputs:color` on the authored fill prim
/// (`lunco://lighting/earthshine.usda`), because a colour is a fact about the
/// light and USD already spells it.
pub const FULL_EARTH_EARTHSHINE_LUX: f32 = 12.0;

/// Drives the earthshine fill's illuminance from **Earth's phase**.
///
/// ## Why this is derived and not authored
///
/// Earthshine is sunlight that hit Earth and bounced. How much arrives
/// therefore depends on how much of Earth's lit face the site can see, which
/// swings from nothing to the full ~12 lx over a lunar month — and it is
/// ANTI-correlated with local daylight, since a full Earth stands opposite the
/// Sun. A single authored number cannot express that; it would be right on one
/// day of the month and wrong on the rest, and wrong in the direction that
/// matters (a fill that stays lit through lunar noon, washing out the shadows
/// the sun is casting).
///
/// It is also why there is no slider. The quantity has one writer, this system,
/// because a knob and a driver on the same field is the two-writer bug that
/// `lunco:env:ambientBrightness` already paid for once.
///
/// ## The geometry
///
/// With `s` the unit direction to the Sun and `e` the unit direction to Earth,
/// both from the site, the Sun–Earth–site angle `α` has `cos α = −(s · e)`, and
/// the illuminated fraction of the disc the site sees is `(1 + cos α) / 2`.
/// Sun and Earth in the same part of the sky ⇒ new Earth ⇒ 0; opposite ⇒ full
/// ⇒ 1. The far-source approximation (Earth→Sun ∥ site→Sun) is exact to well
/// under a degree at 1 AU.
///
/// No-data is a real state and is respected: [`EarthDirectionWorld`] holds a
/// zero vector until an ephemeris resolves, and a scene with no celestial
/// hierarchy never writes it at all. Both leave the fill at whatever the USD
/// authored — 0 — rather than at a guess.
pub fn drive_earthshine_from_phase(
    earth_dir: Option<Res<crate::EarthDirectionWorld>>,
    sun: crate::horizon::SunQuery,
    mut q_fill: Query<&mut DirectionalLight, With<crate::Earthshine>>,
) {
    if q_fill.is_empty() {
        return;
    }
    let Some(earth_dir) = earth_dir else { return };
    let e = earth_dir.0;
    if !e.is_finite() || e.length_squared() < 1e-12 {
        return;
    }
    let Some((sun_gt, _, _)) = crate::horizon::pick_sun(&sun) else {
        return;
    };
    // `back()` is the direction the light points *from* → toward the sun, the
    // same convention `compute_local_solar` reads.
    let s: Vec3 = *sun_gt.back();
    if !s.is_finite() || s.length_squared() < 1e-12 {
        return;
    }

    let cos_alpha = -(s.normalize().dot(e.normalize()));
    let lit_fraction = ((1.0 + cos_alpha) * 0.5).clamp(0.0, 1.0);
    let lux = FULL_EARTH_EARTHSHINE_LUX * lit_fraction;

    for mut fill in &mut q_fill {
        // Change-driven: `DirectionalLight` is in the render extract, and the
        // phase moves by ~0.5°/day, so an unconditional write would dirty the
        // component every frame to restate the same number.
        if (fill.illuminance - lux).abs() > f32::EPSILON {
            fill.illuminance = lux;
        }
    }
}
