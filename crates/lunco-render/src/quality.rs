//! Persisted render-quality intent and the conservative shadow allocation
//! policy shared by the scene projectors and the workbench.

use bevy::prelude::{Component, Resource};
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};

/// The user-facing rendering-quality choices.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RenderingQuality {
    /// Suggested low-cost shadow and lighting preset.
    Low,
    /// Suggested balanced shadow and lighting preset.
    #[default]
    Balanced,
    /// Suggested high-detail shadow and lighting preset.
    High,
}

impl RenderingQuality {
    /// Stable text used by the Graphics settings menu.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Balanced => "Balanced",
            Self::High => "High",
        }
    }

    /// Every selectable value in menu order.
    pub const fn all() -> [Self; 3] {
        [Self::Low, Self::Balanced, Self::High]
    }

    /// The suggested concrete settings for this preset.
    pub const fn profile(self) -> RenderQualityProfile {
        match self {
            Self::Balanced => RenderQualityProfile {
                directional_shadow_map_size: 1024,
                point_shadow_map_size: 1024,
                directional_cascades: 2,
                max_directional_shadow_casters: 1,
                max_point_shadow_casters: 4,
                max_spot_shadow_casters: 4,
                shadow_budget_bytes: 16 * 1024 * 1024,
            },
            Self::Low => RenderQualityProfile {
                directional_shadow_map_size: 512,
                point_shadow_map_size: 512,
                directional_cascades: 1,
                max_directional_shadow_casters: 1,
                max_point_shadow_casters: 2,
                max_spot_shadow_casters: 2,
                shadow_budget_bytes: 8 * 1024 * 1024,
            },
            Self::High => RenderQualityProfile {
                directional_shadow_map_size: 2048,
                point_shadow_map_size: 2048,
                directional_cascades: 2,
                max_directional_shadow_casters: 2,
                max_point_shadow_casters: 8,
                max_spot_shadow_casters: 8,
                shadow_budget_bytes: 64 * 1024 * 1024,
            },
        }
    }
}

/// The concrete shadow-map parameters selected by [`RenderingQuality`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RenderQualityProfile {
    pub directional_shadow_map_size: u32,
    pub point_shadow_map_size: u32,
    pub directional_cascades: usize,
    pub max_directional_shadow_casters: usize,
    pub max_point_shadow_casters: usize,
    pub max_spot_shadow_casters: usize,
    pub shadow_budget_bytes: u64,
}

/// Persisted user settings for shadow and local-light presentation quality.
///
/// [`RenderingQuality`] supplies suggested values only. Once a preset is
/// selected, these fields are the authoritative values used by the renderer;
/// the runtime never silently replaces them with a lower preset because an
/// adapter has less memory than requested.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct RenderingQualitySettings {
    #[serde(default = "default_directional_shadow_map_size")]
    pub directional_shadow_map_size: u32,
    #[serde(default = "default_point_shadow_map_size")]
    pub point_shadow_map_size: u32,
    #[serde(default = "default_directional_cascades")]
    pub directional_cascades: usize,
    #[serde(default = "default_max_directional_shadow_casters")]
    pub max_directional_shadow_casters: usize,
    #[serde(default = "default_max_point_shadow_casters")]
    pub max_point_shadow_casters: usize,
    #[serde(default = "default_max_spot_shadow_casters")]
    pub max_spot_shadow_casters: usize,
    #[serde(default = "default_shadow_budget_bytes")]
    pub shadow_budget_bytes: u64,
}

const fn balanced_profile() -> RenderQualityProfile {
    RenderingQuality::Balanced.profile()
}

const fn default_directional_shadow_map_size() -> u32 {
    balanced_profile().directional_shadow_map_size
}

const fn default_point_shadow_map_size() -> u32 {
    balanced_profile().point_shadow_map_size
}

const fn default_directional_cascades() -> usize {
    balanced_profile().directional_cascades
}

const fn default_max_directional_shadow_casters() -> usize {
    balanced_profile().max_directional_shadow_casters
}

const fn default_max_point_shadow_casters() -> usize {
    balanced_profile().max_point_shadow_casters
}

const fn default_max_spot_shadow_casters() -> usize {
    balanced_profile().max_spot_shadow_casters
}

const fn default_shadow_budget_bytes() -> u64 {
    balanced_profile().shadow_budget_bytes
}

impl RenderingQualitySettings {
    /// Return the currently authoritative values, including custom edits.
    pub const fn profile(self) -> RenderQualityProfile {
        RenderQualityProfile {
            directional_shadow_map_size: self.directional_shadow_map_size,
            point_shadow_map_size: self.point_shadow_map_size,
            directional_cascades: self.directional_cascades,
            max_directional_shadow_casters: self.max_directional_shadow_casters,
            max_point_shadow_casters: self.max_point_shadow_casters,
            max_spot_shadow_casters: self.max_spot_shadow_casters,
            shadow_budget_bytes: self.shadow_budget_bytes,
        }
    }

    /// Identify whether the current values still equal one of the suggestions.
    pub fn preset(self) -> Option<RenderingQuality> {
        RenderingQuality::all()
            .into_iter()
            .find(|quality| quality.profile() == self.profile())
    }

    /// Apply a preset as an explicit user action. Runtime systems do not call
    /// this in response to adapter memory or scene contents.
    pub fn apply_preset(&mut self, quality: RenderingQuality) {
        let profile = quality.profile();
        self.directional_shadow_map_size = profile.directional_shadow_map_size;
        self.point_shadow_map_size = profile.point_shadow_map_size;
        self.directional_cascades = profile.directional_cascades;
        self.max_directional_shadow_casters = profile.max_directional_shadow_casters;
        self.max_point_shadow_casters = profile.max_point_shadow_casters;
        self.max_spot_shadow_casters = profile.max_spot_shadow_casters;
        self.shadow_budget_bytes = profile.shadow_budget_bytes;
    }

    /// Validate persisted or UI-edited settings before they reach Bevy.
    pub fn validate(&self) -> Result<(), &'static str> {
        let profile = self.profile();
        if profile.directional_shadow_map_size == 0
            || !profile.directional_shadow_map_size.is_power_of_two()
        {
            return Err("directional shadow-map size must be a non-zero power of two");
        }
        if profile.point_shadow_map_size == 0 || !profile.point_shadow_map_size.is_power_of_two() {
            return Err("point shadow-map size must be a non-zero power of two");
        }
        if profile.directional_cascades == 0 {
            return Err("directional shadow cascade count must be greater than zero");
        }
        if profile.shadow_budget_bytes == 0 {
            return Err("shadow byte ceiling must be greater than zero");
        }
        Ok(())
    }
}

impl Default for RenderingQualitySettings {
    fn default() -> Self {
        let profile = balanced_profile();
        Self {
            directional_shadow_map_size: profile.directional_shadow_map_size,
            point_shadow_map_size: profile.point_shadow_map_size,
            directional_cascades: profile.directional_cascades,
            max_directional_shadow_casters: profile.max_directional_shadow_casters,
            max_point_shadow_casters: profile.max_point_shadow_casters,
            max_spot_shadow_casters: profile.max_spot_shadow_casters,
            shadow_budget_bytes: profile.shadow_budget_bytes,
        }
    }
}

impl SettingsSection for RenderingQualitySettings {
    const KEY: &'static str = "rendering_quality";
}

/// Main-world projection of the adapter's conservative shadow allocation limit.
///
/// This is a safety ceiling reported by the adapter probe, not a quality
/// selector. It is combined with the user-authored
/// [`RenderingQualitySettings::shadow_budget_bytes`] for admission control;
/// neither value changes the requested map resolution or cascade count.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct GpuShadowAdapterLimit {
    pub limit_bytes: u64,
}

impl Default for GpuShadowAdapterLimit {
    fn default() -> Self {
        Self {
            limit_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Why a shadow map is temporarily suppressed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShadowMapSuppressionReason {
    /// The explicit graphics admission settings excluded this caster.
    Budget,
    /// The render-recovery ladder disabled the map after a GPU fault.
    Recovery,
}

/// Records the user/scene shadow intent while a render policy temporarily
/// disables a map. It makes re-arm lossless and preserves an authored
/// `shadow:enable = false` value.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShadowMapSuppressed {
    pub was_enabled: bool,
    pub reason: ShadowMapSuppressionReason,
}

/// Conservative estimate for directional shadow textures and their views.
pub fn estimate_directional_shadow_bytes(
    quality: RenderingQuality,
    directional_light_count: usize,
) -> u64 {
    let profile = quality.profile();
    estimate_shadow_allocation_bytes(
        profile.directional_shadow_map_size as usize,
        profile.point_shadow_map_size as usize,
        profile.directional_cascades,
        directional_light_count,
        0,
        0,
    )
}

/// Conservative allocation estimate for Bevy's shadow resources.
///
/// Directional cascades and spot shadows use the directional shadow-map
/// texture. Point and spot lights are separate light classes in Bevy, but a
/// spot still consumes one layer of the directional atlas while a point light
/// consumes six faces of the point-light cubemap. The estimate is deliberately
/// expressed in terms of the resources and per-light layer counts so every preflight can
/// use the same accounting instead of maintaining a second approximation.
pub fn estimate_shadow_allocation_bytes(
    directional_map_size: usize,
    point_map_size: usize,
    directional_cascades_per_light: usize,
    directional_light_count: usize,
    point_light_count: usize,
    spot_light_count: usize,
) -> u64 {
    let directional_texels = (directional_map_size as u64)
        .saturating_mul(directional_map_size as u64)
        .saturating_mul(directional_cascades_per_light as u64)
        .saturating_mul(directional_light_count as u64)
        .saturating_add(
            (directional_map_size as u64)
                .saturating_mul(directional_map_size as u64)
                .saturating_mul(spot_light_count as u64),
        );
    let point_texels = (point_map_size as u64)
        .saturating_mul(point_map_size as u64)
        .saturating_mul(6)
        .saturating_mul(point_light_count as u64);

    // Depth32 plus a conservative 50% allowance for views/alignment/driver use.
    directional_texels
        .saturating_add(point_texels)
        .saturating_mul(4)
        .saturating_mul(3)
        / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_is_only_a_suggestion_until_explicitly_applied() {
        let mut settings = RenderingQualitySettings::default();
        settings.directional_shadow_map_size = 4096;
        settings.shadow_budget_bytes = 256 * 1024 * 1024;
        assert!(settings.preset().is_none());
        settings.apply_preset(RenderingQuality::Low);
        assert_eq!(settings.preset(), Some(RenderingQuality::Low));
        assert_eq!(settings.profile(), RenderingQuality::Low.profile());
    }

    #[test]
    fn requested_profile_is_not_replaced_by_a_budget() {
        let mut settings = RenderingQualitySettings::default();
        settings.apply_preset(RenderingQuality::High);
        assert_eq!(settings.profile().directional_shadow_map_size, 2048);
        assert!(
            estimate_directional_shadow_bytes(RenderingQuality::High, 1)
                > GpuShadowAdapterLimit::default().limit_bytes
        );
    }

    #[test]
    fn all_shadow_classes_share_one_conservative_estimate() {
        assert_eq!(
            estimate_shadow_allocation_bytes(1024, 512, 2, 1, 1, 1),
            27 * 1024 * 1024
        );
    }
}
