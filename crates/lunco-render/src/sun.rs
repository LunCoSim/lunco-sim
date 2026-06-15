//! Canonical lunar-sun shadow configuration — the single source of truth for
//! "what a sun's shadows look like at lunar scale".
//!
//! Before this module the same cascade split, biases and shadow-map size were
//! copy-pasted into four places that had silently drifted apart:
//!
//! - the celestial bootstrap fallback sun (`lunco-celestial`),
//! - the sandbox binary fallback sun (`lunco-client`),
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
use lunco_core::{LunarSun, SunAngularDiameter};

/// How to **render** a lunar sun's shadows — the cascade split, biases and
/// atlas size — paired with the sun's physical identity ([`LunarSun`], which
/// owns the illuminance / angular size / exposure and lives in `lunco-core`).
///
/// This is the render-side adapter: construct with [`LunarSunShadow::default`]
/// for the standard look, override individual fields for an authored scene (the
/// USD loader does this from `lunco:shadow:*` attributes), then build the Bevy
/// components with [`cascade_config`](Self::cascade_config),
/// [`directional_light`](Self::directional_light) and [`shadow_map`](Self::shadow_map).
/// The physical numbers come from `self.sun` so they have exactly one home.
///
/// The defaults are tuned for the airless hard-shadow terminator with the
/// near-cascade / far-march split (see `terrain_shadow.wgsl`): a tight first
/// cascade keeps rover contact shadows crisp while the far cascades carry
/// mesh-accurate terrain self-shadow out to `maximum_distance`, beyond which
/// the heightfield ray-march takes over.
#[derive(Debug, Clone, Copy)]
pub struct LunarSunShadow {
    /// The sun's physical identity (illuminance, angular size, matched exposure).
    /// The single source of truth — see [`lunco_core::space_entities`].
    pub sun: LunarSun,
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
            sun: LunarSun::default(),
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

    /// Build the shadow-casting [`DirectionalLight`] with the given color.
    /// Illuminance comes from the physical [`sun`](Self::sun); biases are render
    /// config.
    pub fn directional_light(&self, color: Color) -> DirectionalLight {
        DirectionalLight {
            color,
            illuminance: self.sun.illuminance_lux,
            shadows_enabled: true,
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

    /// The [`SunAngularDiameter`] component, from the physical [`sun`](Self::sun).
    pub fn angular_diameter(&self) -> SunAngularDiameter {
        SunAngularDiameter(self.sun.angular_diameter_deg)
    }
}
