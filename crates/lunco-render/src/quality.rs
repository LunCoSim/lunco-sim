//! Persisted render-quality intent and the conservative shadow allocation
//! policy shared by the scene projectors and the workbench.

use bevy::prelude::{Component, Resource};
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};

/// The user-facing rendering-quality choices.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RenderingQuality {
    /// Select the highest profile that fits the adapter's conservative budget.
    #[default]
    Auto,
    /// Lowest shadow-map resolution and one sun cascade.
    Low,
    /// Balanced resolution with two sun cascades.
    Balanced,
    /// Highest resolution and longest-lived shadow detail.
    High,
}

impl RenderingQuality {
    /// Stable text used by the Graphics settings menu.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Low => "Low",
            Self::Balanced => "Balanced",
            Self::High => "High",
        }
    }

    /// Every selectable value in menu order.
    pub const fn all() -> [Self; 4] {
        [Self::Auto, Self::Low, Self::Balanced, Self::High]
    }

    /// Resolve the requested choice against the available shadow budget.
    ///
    /// The budget is deliberately conservative: wgpu does not expose a
    /// portable "free VRAM" value, and integrated adapters share their memory
    /// with the process and desktop. The estimate includes a 50% allocation
    /// allowance for texture views, alignment, and driver bookkeeping.
    pub fn effective_for_shadow_budget(
        self,
        budget_bytes: u64,
        directional_light_count: usize,
    ) -> Self {
        let candidates = match self {
            Self::Auto | Self::High => [Self::High, Self::Balanced, Self::Low],
            Self::Balanced => [Self::Balanced, Self::Low, Self::Low],
            Self::Low => [Self::Low, Self::Low, Self::Low],
        };

        candidates
            .into_iter()
            .find(|quality| {
                estimate_directional_shadow_bytes(*quality, directional_light_count) <= budget_bytes
            })
            .unwrap_or(Self::Low)
    }

    /// The concrete profile for this choice. `Auto` is only unresolved before
    /// an adapter budget is available; its safe concrete fallback is Balanced.
    pub const fn profile(self) -> RenderQualityProfile {
        match self {
            Self::Auto | Self::Balanced => RenderQualityProfile {
                directional_shadow_map_size: 1024,
                point_shadow_map_size: 1024,
                directional_cascades: 2,
            },
            Self::Low => RenderQualityProfile {
                directional_shadow_map_size: 512,
                point_shadow_map_size: 512,
                directional_cascades: 1,
            },
            Self::High => RenderQualityProfile {
                directional_shadow_map_size: 2048,
                point_shadow_map_size: 2048,
                directional_cascades: 2,
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
}

/// Persisted user preference for the renderer's presentation quality.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RenderingQualitySettings {
    pub quality: RenderingQuality,
}

impl SettingsSection for RenderingQualitySettings {
    const KEY: &'static str = "rendering_quality";
}

/// Main-world projection of the adapter's conservative shadow allocation limit.
///
/// The workbench fills this from the render-world adapter probe. The default is
/// intentionally safe for an integrated adapter so a persisted High choice
/// cannot allocate its large atlas before that probe is published.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct GpuShadowBudget {
    pub limit_bytes: u64,
}

impl Default for GpuShadowBudget {
    fn default() -> Self {
        Self {
            limit_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Records the user/scene shadow intent while the render-recovery ladder has
/// temporarily disabled a map. It makes re-arm lossless and preserves an
/// authored `shadow:enable = false` value.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShadowMapSuppressed {
    pub was_enabled: bool,
}

/// Conservative estimate for directional shadow textures and their views.
pub fn estimate_directional_shadow_bytes(
    quality: RenderingQuality,
    directional_light_count: usize,
) -> u64 {
    let profile = quality.profile();
    let texels = u64::from(profile.directional_shadow_map_size)
        .saturating_mul(u64::from(profile.directional_shadow_map_size))
        .saturating_mul(profile.directional_cascades as u64)
        .saturating_mul(directional_light_count as u64);
    // Depth32 plus a conservative 50% allowance for views/alignment/driver use.
    texels.saturating_mul(4).saturating_mul(3) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_selects_balanced_on_the_safe_integrated_budget() {
        assert_eq!(
            RenderingQuality::Auto.effective_for_shadow_budget(16 * 1024 * 1024, 1),
            RenderingQuality::Balanced
        );
    }

    #[test]
    fn high_is_retained_when_the_budget_can_hold_it() {
        assert_eq!(
            RenderingQuality::High.effective_for_shadow_budget(64 * 1024 * 1024, 1),
            RenderingQuality::High
        );
    }

    #[test]
    fn multiple_directional_lights_are_budgeted_together() {
        assert_eq!(
            RenderingQuality::High.effective_for_shadow_budget(64 * 1024 * 1024, 2),
            RenderingQuality::Balanced
        );
    }
}
