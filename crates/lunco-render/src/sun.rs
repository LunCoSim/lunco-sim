//! Canonical lunar-sun shadow configuration — the single source of truth for
//! "what a sun's shadows look like at lunar scale".
//!
//! Before this module the same cascade split, biases and shadow-map size were
//! copy-pasted into four places that had silently drifted apart:
//!
//! - the celestial bootstrap fallback sun (`lunco-celestial`),
//! - the sandbox binary fallback sun (`lunco-sandbox`),
//! - the USD `DistantLight` loader (`lunco-usd-bevy`),
//! - the `SetEnvironmentLight` runtime tuner (`lunco-environment`).
//!
//! The worst offender spawned a `DirectionalLight` with *no* `CascadeShadowConfig`
//! at all, so it rendered with Bevy's single-cascade default — wrong terrain
//! self-shadowing and clipped low-sun streaks. Now every spawn path builds its
//! light from [`LunarSunShadow`], so a tuning change lands everywhere by
//! construction and no path can forget the cascade/bias/map setup.

use bevy::light::{
    CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLight, DirectionalLightShadowMap,
};
use bevy::prelude::Color;

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

/// How to **render** a lunar sun's shadows — the cascade split, biases and
/// atlas size. This is render-side *presentation* config only; the sun's
/// physical identity (illuminance, angular size, matched camera exposure) lives
/// in `lunco_environment::LunarSun` (environmental state), and is passed *in* to
/// the builders here so render stays a low presentation crate with no dependency
/// on environment.
///
/// Construct with [`LunarSunShadow::default`] for the standard look, override
/// individual fields for an authored scene (the USD loader does this from
/// `lunco:shadow:*` attributes), then build the Bevy components with
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
    /// Directional shadow atlas size per cascade. 4096² is the safe ceiling
    /// (8192² × 4 cascades ≈ 1 GB VRAM).
    pub shadow_map_size: u32,
}

impl Default for LunarSunShadow {
    fn default() -> Self {
        Self {
            num_cascades: 4,
            minimum_distance: 0.1,
            first_cascade_far_bound: 40.0,
            maximum_distance: 1500.0,
            overlap_proportion: 0.1,
            // Favour acne-free terrain over the last centimetres of contact
            // tightness — grazing sun makes self-shadow acne the dominant
            // artifact. Live-tunable via `SetEnvironmentLight`.
            depth_bias: 0.06,
            normal_bias: 2.5,
            // WEB: 2048² shadow atlas — a quarter of the shadow-pass fill +
            // sampling cost on a WebGL iGPU, for a slightly softer terminator.
            // Native keeps the crisp 4096² ceiling. One place → every web app
            // (sandbox/lunica/luncosim) gets the cheaper atlas.
            #[cfg(target_arch = "wasm32")]
            shadow_map_size: 2048,
            #[cfg(not(target_arch = "wasm32"))]
            shadow_map_size: 4096,
        }
    }
}

impl LunarSunShadow {
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

    /// Build the shadow-casting [`DirectionalLight`] with the given color and
    /// illuminance (lux). Illuminance is *physical* state — the caller passes it
    /// from `lunco_environment::LunarSun` (or an authored USD value); biases are
    /// this struct's render config.
    pub fn directional_light(&self, color: Color, illuminance_lux: f32) -> DirectionalLight {
        DirectionalLight {
            color,
            illuminance: illuminance_lux,
            shadow_maps_enabled: true,
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
