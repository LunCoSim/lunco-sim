//! # Space entities — the sky's light sources, as physical parameters
//!
//! The single, documented source of truth for **what the bodies in the lunar
//! sky *are*** as far as lighting and rendering are concerned: how bright they
//! shine ([illuminance](LunarSun::illuminance_lux)), how big they appear
//! ([angular diameter](LunarSun::angular_diameter_deg)), and the camera
//! exposure that pairs with the key light.
//!
//! Why this lives in `lunco-core` (and not `lunco-celestial`, which models the
//! bodies' *gravity* and orbital placement): these parameters are consumed by
//! the **lowest** crates — the USD `DistantLight` loader (`lunco-usd-bevy`),
//! the shadow-render builders (`lunco-render`), and every camera spawn. Core is
//! the one crate they all already depend on, so putting the canonical values
//! here gives a single home without any dependency cycle. The data is pure
//! (plain `f32`s, no Bevy/render types); `lunco-render` turns it into the actual
//! `DirectionalLight` / `CascadeShadowConfig` / `Exposure` components.
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

/// The Sun as seen from the lunar surface (Sol) — the hard key light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LunarSun {
    /// Direct solar illuminance on a surface facing the Sun, **lux**.
    /// ~128 000 lx on the airless Moon (vs ~100 000 lx through Earth's
    /// atmosphere). This is the scene's key-light brightness.
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
            exposure_ev100: 15.0,
        }
    }
}

/// Earthshine — Earth's reflected sunlight, the Moon's only other natural light.
/// A faint, cool-blue, **shadowless** fill that lifts sun-shadowed regolith into
/// readable relief without washing the shadow cores grey (which a flat ambient
/// would). The runtime light is spawned by `lunco-environment` from these values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Earthshine {
    /// Fill illuminance, **lux** (~10–15 lx, ≈ 1/10 000 of the Sun).
    pub illuminance_lux: f32,
    /// Fill colour, **linear RGB** — cool blue (Earth's albedo skews blue).
    pub color: [f32; 3],
}

impl Default for Earthshine {
    fn default() -> Self {
        Self {
            illuminance_lux: 12.0,
            color: [0.6, 0.75, 1.0],
        }
    }
}
