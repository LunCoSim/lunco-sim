//! Canonical lunar-sun shadow configuration — the single source of truth for
//! "what a sun's shadows look like at lunar scale".
//!
//! Before this module the same cascade split, biases and shadow-map size were
//! copy-pasted into several Rust sun-spawn paths that had silently drifted
//! apart — the worst offender spawned a `DirectionalLight` with *no*
//! `CascadeShadowConfig` at all, so it rendered with Bevy's single-cascade
//! default (wrong terrain self-shadowing, clipped low-sun streaks).
//!
//! Nothing spawns a sun from Rust any more: the sun is authored USD
//! (`lunco://lighting/sun.usda`) and there is exactly one instantiation path.
//! Two consumers remain, and both start from [`LunarSunShadow`]:
//!
//! - the USD `DistantLight` loader (`lunco-usd-bevy`), which overrides only
//!   what the prim authors,
//! - the `SetEnvironmentLight` runtime tuner (`lunco-environment`).
//!
//! Renderer defaults for cascade ranges, count, and bias live in persisted
//! Graphics settings. Explicit USD shadow-range attributes still override
//! those defaults for the individual scene light.

use bevy::light::{
    CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLight, DirectionalLightShadowMap,
};
use bevy::prelude::Color;

use crate::{RenderQualityProfile, RenderingQuality};

/// Camera EV100 matched to the ~128 klx lunar sun — the value
/// `lunco_environment::LunarSun` defaults to and that the celestial sun
/// (`update_sun_light_system`) is calibrated against.
///
/// Lives in this render-free intent crate because it is the lowest crate on
/// the graph that BOTH dependents can reach: `lunco-usd-bevy` (which spawns
/// USD `Camera` prims and must expose them for the real sun from frame one,
/// not Bevy's `Exposure::default()` = EV 9.7) cannot see
/// `lunco_environment::LunarSun`, and `lunco-environment` cannot see
/// `lunco-render-bevy`. Keeping the matched pair's exposure here means a
/// freshly-spawned camera and the late celestial/`SetEnvironmentLight`
/// exposure writes all agree on one number, so there is no window in which
/// the 131 klx sun renders against an EV-9.7 camera (a ~5-stop blowout).
pub const LUNAR_SUN_EXPOSURE_EV100: f32 = 16.0;

/// Sol's apparent angular **diameter** in degrees, from the Moon or the Earth
/// (the two are indistinguishable at this precision).
///
/// Here for the same reason as [`LUNAR_SUN_EXPOSURE_EV100`]: it is the lowest
/// crate both readers reach. `lunco_environment::LunarSun` publishes it as
/// environmental state, and the `lunco-usd-bevy` `DistantLight` loader — which
/// sits BELOW environment — needs it as the fallback for an unauthored
/// `inputs:angle`. Two literals would be two numbers that can drift, and the
/// symptom of drift is a penumbra width that changes depending on whether a
/// scene authored the attribute.
///
/// It is also exactly `UsdLuxDistantLight`'s schema fallback for
/// `inputs:angle`, so an unauthored prim lands where USD says it lands rather
/// than on an engine opinion.
pub const SOLAR_ANGULAR_DIAMETER_DEG: f32 = 0.53;

/// How to **render** a lunar sun's shadows — the cascade split, biases and
/// atlas size. This is render-side *presentation* config only; the sun's
/// physical identity (illuminance, angular size, matched camera exposure) lives
/// in `lunco_environment::LunarSun` (environmental state), and is passed *in* to
/// the builders here so render stays a low presentation crate with no dependency
/// on environment.
///
/// Construct with [`LunarSunShadow::default`] for the balanced Graphics
/// settings, override individual fields for an authored scene (the USD loader
/// does this from shadow attributes), then build the Bevy components with
/// [`cascade_config`](Self::cascade_config),
/// [`directional_light`](Self::directional_light) (which takes the illuminance)
/// and [`shadow_map`](Self::shadow_map).
///
/// The defaults are tuned for the airless hard-shadow terminator with the
/// near-cascade / far-march split (see `terrain_shadow.wgsl`): a tight first
/// cascade keeps rover contact shadows crisp while the far cascades carry
/// mesh-accurate terrain self-shadow out to `maximum_distance`, beyond which
/// the heightfield ray-march takes over.
#[derive(Debug, Clone, Copy)]
pub struct LunarSunShadow {
    /// Number of shadow cascades (near→far split inside one light).
    pub num_cascades: usize,
    /// Nearest shadow-casting distance, metres.
    pub minimum_distance: f32,
    /// Far bound of the first (sharpest) cascade, metres.
    pub first_cascade_far_bound: f32,
    /// Total shadow-casting range, metres.
    pub maximum_distance: f32,
    /// Cascade-to-cascade cross-fade. Low ⇒ crisper transitions (hard look).
    pub overlap_proportion: f32,
    /// Shadow depth bias — raise to suppress self-shadow acne stripes.
    pub depth_bias: f32,
    /// Shadow normal bias, in shadow-texel units — the main acne killer under
    /// grazing lunar light.
    pub normal_bias: f32,
    /// Directional shadow atlas size per cascade. The default is 2048² to keep
    /// the shared-GPU allocation bounded.
    pub shadow_map_size: u32,
}

impl Default for LunarSunShadow {
    fn default() -> Self {
        Self {
            num_cascades: RenderingQuality::Balanced.profile().directional_cascades,
            minimum_distance: RenderingQuality::Balanced.profile().shadow_minimum_distance,
            first_cascade_far_bound: RenderingQuality::Balanced
                .profile()
                .shadow_first_cascade_far_bound,
            maximum_distance: RenderingQuality::Balanced.profile().shadow_maximum_distance,
            overlap_proportion: RenderingQuality::Balanced.profile().shadow_cascade_overlap,
            // The terrain needs a conservative bias under grazing lunar light:
            // smaller values turn its dense DEM triangles into visible shadow
            // acne. Contact detail remains with the native shadow maps: Bevy
            // 0.19 cannot safely switch a custom material pipeline to its
            // contact-shadow view layout after specialization.
            depth_bias: RenderingQuality::Balanced.profile().shadow_depth_bias,
            normal_bias: RenderingQuality::Balanced.profile().shadow_normal_bias,
            shadow_map_size: RenderingQuality::Balanced
                .profile()
                .directional_shadow_map_size,
        }
    }
}

impl LunarSunShadow {
    /// Build the canonical sun-shadow spec for a resolved quality profile.
    ///
    /// Authored USD may still override the standard maximum shadow distance and
    /// renderer-specific first-cascade split after this constructor returns.
    /// Omitted values, along with all other renderer quality values, come from
    /// the settings profile.
    pub fn for_quality(quality: RenderingQuality) -> Self {
        Self::for_profile(quality.profile())
    }

    /// Build the canonical sun-shadow spec from the authoritative graphics
    /// settings, including custom edits made after a preset was applied.
    pub fn for_profile(profile: RenderQualityProfile) -> Self {
        Self {
            num_cascades: profile.directional_cascades,
            minimum_distance: profile.shadow_minimum_distance,
            first_cascade_far_bound: profile.shadow_first_cascade_far_bound,
            maximum_distance: profile.shadow_maximum_distance,
            overlap_proportion: profile.shadow_cascade_overlap,
            depth_bias: profile.shadow_depth_bias,
            normal_bias: profile.shadow_normal_bias,
            shadow_map_size: profile.directional_shadow_map_size,
        }
    }

    /// Build the [`CascadeShadowConfig`] for this spec.
    pub fn cascade_config(&self) -> CascadeShadowConfig {
        CascadeShadowConfigBuilder {
            num_cascades: self.num_cascades.max(1),
            minimum_distance: self.minimum_distance,
            first_cascade_far_bound: self.first_cascade_far_bound,
            maximum_distance: self.maximum_distance,
            overlap_proportion: self.overlap_proportion,
        }
        .build()
    }

    /// Build the [`DirectionalLight`] with the given color and illuminance
    /// (lux). Illuminance is *physical* state — the caller passes it from
    /// `lunco_environment::LunarSun` (or an authored USD value); biases are this
    /// struct's render config.
    ///
    /// `casts_shadows` comes from `UsdLuxShadowAPI`'s `inputs:shadow:enable`. It
    /// is a PARAMETER and not a hardcoded `true` because not every
    /// `DistantLight` is the key light: a body's reflected fill (earthshine)
    /// authors `false`, and honouring that is the difference between one shadow
    /// pass and two over the whole scene for a contribution measured in single
    /// lux — which also double-darkens every contact shadow.
    pub fn directional_light(
        &self,
        color: Color,
        illuminance_lux: f32,
        casts_shadows: bool,
    ) -> DirectionalLight {
        DirectionalLight {
            color,
            illuminance: illuminance_lux,
            shadow_maps_enabled: casts_shadows,
            shadow_depth_bias: self.depth_bias,
            shadow_normal_bias: self.normal_bias,
            ..Default::default()
        }
    }

    /// The shadow-atlas resource for this spec. Insert as a resource; it is
    /// global (one atlas size for all directional lights).
    pub fn shadow_map(&self) -> DirectionalLightShadowMap {
        DirectionalLightShadowMap {
            size: self.shadow_map_size as usize,
        }
    }
}
