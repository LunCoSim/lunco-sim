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
                shadow_minimum_distance: 0.1,
                shadow_first_cascade_far_bound: 40.0,
                shadow_maximum_distance: 1500.0,
                shadow_cascade_overlap: 0.1,
                shadow_depth_bias: 0.06,
                shadow_normal_bias: 2.5,
                distant_light_default_illuminance: 128_000.0,
                local_light_default_intensity: 1_000.0,
                rect_light_default_intensity: 10_000.0,
                local_light_default_range: 30.0,
                local_shadow_map_near_z: 0.1,
                terrain_mesh_cache_bytes: 640 * 1024 * 1024,
                terrain_lod_tile_resolution: 49,
                terrain_lod_cinematic_resolution: 2049,
                terrain_lod_pixel_error: 2.0,
                terrain_lod_max_depth: 8,
                terrain_lod_probe_resolution: 9,
                terrain_lod_bakes_per_frame: 24,
                terrain_lod_max_inflight_bakes: 64,
                terrain_lod_tile_budget: 768,
            },
            Self::Low => RenderQualityProfile {
                directional_shadow_map_size: 512,
                point_shadow_map_size: 512,
                directional_cascades: 1,
                max_directional_shadow_casters: 1,
                max_point_shadow_casters: 2,
                max_spot_shadow_casters: 2,
                shadow_budget_bytes: 8 * 1024 * 1024,
                shadow_minimum_distance: 0.1,
                shadow_first_cascade_far_bound: 20.0,
                shadow_maximum_distance: 600.0,
                shadow_cascade_overlap: 0.1,
                shadow_depth_bias: 0.1,
                shadow_normal_bias: 4.0,
                distant_light_default_illuminance: 128_000.0,
                local_light_default_intensity: 1_000.0,
                rect_light_default_intensity: 10_000.0,
                local_light_default_range: 20.0,
                local_shadow_map_near_z: 0.2,
                terrain_mesh_cache_bytes: 256 * 1024 * 1024,
                terrain_lod_tile_resolution: 33,
                terrain_lod_cinematic_resolution: 1025,
                terrain_lod_pixel_error: 4.0,
                terrain_lod_max_depth: 6,
                terrain_lod_probe_resolution: 5,
                terrain_lod_bakes_per_frame: 8,
                terrain_lod_max_inflight_bakes: 16,
                terrain_lod_tile_budget: 256,
            },
            Self::High => RenderQualityProfile {
                directional_shadow_map_size: 2048,
                point_shadow_map_size: 2048,
                directional_cascades: 2,
                max_directional_shadow_casters: 2,
                max_point_shadow_casters: 8,
                max_spot_shadow_casters: 8,
                shadow_budget_bytes: 64 * 1024 * 1024,
                shadow_minimum_distance: 0.1,
                shadow_first_cascade_far_bound: 80.0,
                shadow_maximum_distance: 3000.0,
                shadow_cascade_overlap: 0.1,
                shadow_depth_bias: 0.03,
                shadow_normal_bias: 1.5,
                distant_light_default_illuminance: 128_000.0,
                local_light_default_intensity: 1_000.0,
                rect_light_default_intensity: 10_000.0,
                local_light_default_range: 50.0,
                local_shadow_map_near_z: 0.05,
                terrain_mesh_cache_bytes: 1024 * 1024 * 1024,
                terrain_lod_tile_resolution: 65,
                terrain_lod_cinematic_resolution: 2049,
                terrain_lod_pixel_error: 1.0,
                terrain_lod_max_depth: 10,
                terrain_lod_probe_resolution: 13,
                terrain_lod_bakes_per_frame: 48,
                terrain_lod_max_inflight_bakes: 128,
                terrain_lod_tile_budget: 1536,
            },
        }
    }
}

/// The concrete shadow-map parameters selected by [`RenderingQuality`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RenderQualityProfile {
    pub directional_shadow_map_size: u32,
    pub point_shadow_map_size: u32,
    pub directional_cascades: usize,
    pub max_directional_shadow_casters: usize,
    pub max_point_shadow_casters: usize,
    pub max_spot_shadow_casters: usize,
    pub shadow_budget_bytes: u64,
    /// Default nearest distance for unauthored directional shadow cascades.
    pub shadow_minimum_distance: f32,
    /// Default far bound of the first directional shadow cascade.
    pub shadow_first_cascade_far_bound: f32,
    /// Default total range for unauthored directional shadows.
    pub shadow_maximum_distance: f32,
    /// Default cross-fade overlap between directional cascades.
    pub shadow_cascade_overlap: f32,
    /// Depth bias used by native directional/local-light shadow maps.
    pub shadow_depth_bias: f32,
    /// Normal bias, in shadow texels, used to suppress grazing-angle acne.
    pub shadow_normal_bias: f32,
    /// Illuminance used when a DistantLight omits `inputs:intensity`.
    pub distant_light_default_illuminance: f32,
    /// Luminous power used when a local SphereLight omits `inputs:intensity`.
    pub local_light_default_intensity: f32,
    /// Luminous power used when a RectLight omits `inputs:intensity`.
    pub rect_light_default_intensity: f32,
    /// Range used when a local light leaves `lunco:light:range` at its schema
    /// default of zero (the explicit USD meaning is engine default).
    pub local_light_default_range: f32,
    /// Near Z plane used by local-light shadow maps.
    pub local_shadow_map_near_z: f32,
    /// Maximum estimated GPU upload footprint retained by streamed terrain
    /// meshes. This is a requested cache limit, not an automatic quality
    /// downgrade; eviction is the cache's explicit response when it is full.
    pub terrain_mesh_cache_bytes: u64,
    /// Vertices per side of one streamed terrain tile.
    pub terrain_lod_tile_resolution: usize,
    /// Vertices per side of a frozen/cinematic terrain tile.
    pub terrain_lod_cinematic_resolution: usize,
    /// Screen-space terrain error in pixels that triggers refinement.
    pub terrain_lod_pixel_error: f64,
    /// Deepest streamed terrain quadtree level.
    pub terrain_lod_max_depth: u8,
    /// Samples per side used to measure terrain-node error.
    pub terrain_lod_probe_resolution: usize,
    /// New streamed terrain bakes admitted per interactive frame.
    pub terrain_lod_bakes_per_frame: usize,
    /// Maximum streamed terrain bakes allowed in flight.
    pub terrain_lod_max_inflight_bakes: usize,
    /// Maximum selected streamed terrain tiles per terrain.
    pub terrain_lod_tile_budget: usize,
}

/// Persisted user settings for shadow and local-light presentation quality.
///
/// [`RenderingQuality`] supplies suggested values only. Once a preset is
/// selected, these fields are the authoritative values used by the renderer;
/// the runtime never silently replaces them with a lower preset because a
/// scene or adapter cannot satisfy the request.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
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
    #[serde(default = "default_shadow_minimum_distance")]
    pub shadow_minimum_distance: f32,
    #[serde(default = "default_shadow_first_cascade_far_bound")]
    pub shadow_first_cascade_far_bound: f32,
    #[serde(default = "default_shadow_maximum_distance")]
    pub shadow_maximum_distance: f32,
    #[serde(default = "default_shadow_cascade_overlap")]
    pub shadow_cascade_overlap: f32,
    #[serde(default = "default_shadow_depth_bias")]
    pub shadow_depth_bias: f32,
    #[serde(default = "default_shadow_normal_bias")]
    pub shadow_normal_bias: f32,
    #[serde(default = "default_distant_light_default_illuminance")]
    pub distant_light_default_illuminance: f32,
    #[serde(default = "default_local_light_default_intensity")]
    pub local_light_default_intensity: f32,
    #[serde(default = "default_rect_light_default_intensity")]
    pub rect_light_default_intensity: f32,
    #[serde(default = "default_local_light_default_range")]
    pub local_light_default_range: f32,
    #[serde(default = "default_local_shadow_map_near_z")]
    pub local_shadow_map_near_z: f32,
    #[serde(default = "default_terrain_mesh_cache_bytes")]
    pub terrain_mesh_cache_bytes: u64,
    #[serde(default = "default_terrain_lod_tile_resolution")]
    pub terrain_lod_tile_resolution: usize,
    #[serde(default = "default_terrain_lod_cinematic_resolution")]
    pub terrain_lod_cinematic_resolution: usize,
    #[serde(default = "default_terrain_lod_pixel_error")]
    pub terrain_lod_pixel_error: f64,
    #[serde(default = "default_terrain_lod_max_depth")]
    pub terrain_lod_max_depth: u8,
    #[serde(default = "default_terrain_lod_probe_resolution")]
    pub terrain_lod_probe_resolution: usize,
    #[serde(default = "default_terrain_lod_bakes_per_frame")]
    pub terrain_lod_bakes_per_frame: usize,
    #[serde(default = "default_terrain_lod_max_inflight_bakes")]
    pub terrain_lod_max_inflight_bakes: usize,
    #[serde(default = "default_terrain_lod_tile_budget")]
    pub terrain_lod_tile_budget: usize,
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

const fn default_shadow_minimum_distance() -> f32 {
    balanced_profile().shadow_minimum_distance
}

const fn default_shadow_first_cascade_far_bound() -> f32 {
    balanced_profile().shadow_first_cascade_far_bound
}

const fn default_shadow_maximum_distance() -> f32 {
    balanced_profile().shadow_maximum_distance
}

const fn default_shadow_cascade_overlap() -> f32 {
    balanced_profile().shadow_cascade_overlap
}

const fn default_terrain_mesh_cache_bytes() -> u64 {
    balanced_profile().terrain_mesh_cache_bytes
}

const fn default_shadow_depth_bias() -> f32 {
    balanced_profile().shadow_depth_bias
}

const fn default_shadow_normal_bias() -> f32 {
    balanced_profile().shadow_normal_bias
}

const fn default_local_light_default_range() -> f32 {
    balanced_profile().local_light_default_range
}

const fn default_distant_light_default_illuminance() -> f32 {
    balanced_profile().distant_light_default_illuminance
}

const fn default_local_light_default_intensity() -> f32 {
    balanced_profile().local_light_default_intensity
}

const fn default_rect_light_default_intensity() -> f32 {
    balanced_profile().rect_light_default_intensity
}

const fn default_local_shadow_map_near_z() -> f32 {
    balanced_profile().local_shadow_map_near_z
}

const fn default_terrain_lod_tile_resolution() -> usize {
    balanced_profile().terrain_lod_tile_resolution
}

const fn default_terrain_lod_cinematic_resolution() -> usize {
    balanced_profile().terrain_lod_cinematic_resolution
}

const fn default_terrain_lod_pixel_error() -> f64 {
    balanced_profile().terrain_lod_pixel_error
}

const fn default_terrain_lod_max_depth() -> u8 {
    balanced_profile().terrain_lod_max_depth
}

const fn default_terrain_lod_probe_resolution() -> usize {
    balanced_profile().terrain_lod_probe_resolution
}

const fn default_terrain_lod_bakes_per_frame() -> usize {
    balanced_profile().terrain_lod_bakes_per_frame
}

const fn default_terrain_lod_max_inflight_bakes() -> usize {
    balanced_profile().terrain_lod_max_inflight_bakes
}

const fn default_terrain_lod_tile_budget() -> usize {
    balanced_profile().terrain_lod_tile_budget
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
            shadow_minimum_distance: self.shadow_minimum_distance,
            shadow_first_cascade_far_bound: self.shadow_first_cascade_far_bound,
            shadow_maximum_distance: self.shadow_maximum_distance,
            shadow_cascade_overlap: self.shadow_cascade_overlap,
            shadow_depth_bias: self.shadow_depth_bias,
            shadow_normal_bias: self.shadow_normal_bias,
            distant_light_default_illuminance: self.distant_light_default_illuminance,
            local_light_default_intensity: self.local_light_default_intensity,
            rect_light_default_intensity: self.rect_light_default_intensity,
            local_light_default_range: self.local_light_default_range,
            local_shadow_map_near_z: self.local_shadow_map_near_z,
            terrain_mesh_cache_bytes: self.terrain_mesh_cache_bytes,
            terrain_lod_tile_resolution: self.terrain_lod_tile_resolution,
            terrain_lod_cinematic_resolution: self.terrain_lod_cinematic_resolution,
            terrain_lod_pixel_error: self.terrain_lod_pixel_error,
            terrain_lod_max_depth: self.terrain_lod_max_depth,
            terrain_lod_probe_resolution: self.terrain_lod_probe_resolution,
            terrain_lod_bakes_per_frame: self.terrain_lod_bakes_per_frame,
            terrain_lod_max_inflight_bakes: self.terrain_lod_max_inflight_bakes,
            terrain_lod_tile_budget: self.terrain_lod_tile_budget,
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
        self.shadow_minimum_distance = profile.shadow_minimum_distance;
        self.shadow_first_cascade_far_bound = profile.shadow_first_cascade_far_bound;
        self.shadow_maximum_distance = profile.shadow_maximum_distance;
        self.shadow_cascade_overlap = profile.shadow_cascade_overlap;
        self.shadow_depth_bias = profile.shadow_depth_bias;
        self.shadow_normal_bias = profile.shadow_normal_bias;
        self.distant_light_default_illuminance = profile.distant_light_default_illuminance;
        self.local_light_default_intensity = profile.local_light_default_intensity;
        self.rect_light_default_intensity = profile.rect_light_default_intensity;
        self.local_light_default_range = profile.local_light_default_range;
        self.local_shadow_map_near_z = profile.local_shadow_map_near_z;
        self.terrain_mesh_cache_bytes = profile.terrain_mesh_cache_bytes;
        self.terrain_lod_tile_resolution = profile.terrain_lod_tile_resolution;
        self.terrain_lod_cinematic_resolution = profile.terrain_lod_cinematic_resolution;
        self.terrain_lod_pixel_error = profile.terrain_lod_pixel_error;
        self.terrain_lod_max_depth = profile.terrain_lod_max_depth;
        self.terrain_lod_probe_resolution = profile.terrain_lod_probe_resolution;
        self.terrain_lod_bakes_per_frame = profile.terrain_lod_bakes_per_frame;
        self.terrain_lod_max_inflight_bakes = profile.terrain_lod_max_inflight_bakes;
        self.terrain_lod_tile_budget = profile.terrain_lod_tile_budget;
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
        if !profile.shadow_minimum_distance.is_finite() || profile.shadow_minimum_distance < 0.0 {
            return Err("shadow minimum distance must be finite and non-negative");
        }
        if !profile.shadow_first_cascade_far_bound.is_finite()
            || profile.shadow_first_cascade_far_bound <= profile.shadow_minimum_distance
        {
            return Err(
                "first shadow cascade bound must be finite and greater than the minimum distance",
            );
        }
        if !profile.shadow_maximum_distance.is_finite()
            || profile.shadow_maximum_distance <= profile.shadow_first_cascade_far_bound
        {
            return Err(
                "maximum shadow distance must be finite and greater than the first cascade bound",
            );
        }
        if !profile.shadow_cascade_overlap.is_finite()
            || !(0.0..1.0).contains(&profile.shadow_cascade_overlap)
        {
            return Err("shadow cascade overlap must be finite and in [0, 1)");
        }
        if !profile.shadow_depth_bias.is_finite() || profile.shadow_depth_bias < 0.0 {
            return Err("shadow depth bias must be finite and non-negative");
        }
        if !profile.shadow_normal_bias.is_finite() || profile.shadow_normal_bias < 0.0 {
            return Err("shadow normal bias must be finite and non-negative");
        }
        if !profile.distant_light_default_illuminance.is_finite()
            || profile.distant_light_default_illuminance <= 0.0
        {
            return Err("distant-light default illuminance must be finite and greater than zero");
        }
        if !profile.local_light_default_intensity.is_finite()
            || profile.local_light_default_intensity <= 0.0
        {
            return Err("local-light default intensity must be finite and greater than zero");
        }
        if !profile.rect_light_default_intensity.is_finite()
            || profile.rect_light_default_intensity <= 0.0
        {
            return Err("rect-light default intensity must be finite and greater than zero");
        }
        if !profile.local_light_default_range.is_finite()
            || profile.local_light_default_range <= 0.0
        {
            return Err("local-light default range must be finite and greater than zero");
        }
        if !profile.local_shadow_map_near_z.is_finite() || profile.local_shadow_map_near_z < 0.0 {
            return Err("local shadow-map near Z must be finite and non-negative");
        }
        if profile.terrain_mesh_cache_bytes == 0 {
            return Err("terrain mesh cache byte ceiling must be greater than zero");
        }
        if profile.terrain_lod_tile_resolution < 3 || profile.terrain_lod_tile_resolution > 4097 {
            return Err("terrain tile resolution must be between 3 and 4097");
        }
        if profile.terrain_lod_cinematic_resolution < 3
            || profile.terrain_lod_cinematic_resolution > 4097
        {
            return Err("cinematic terrain resolution must be between 3 and 4097");
        }
        if !profile.terrain_lod_pixel_error.is_finite() || profile.terrain_lod_pixel_error <= 0.0 {
            return Err("terrain LOD pixel error must be finite and greater than zero");
        }
        if profile.terrain_lod_max_depth == 0 || profile.terrain_lod_max_depth > 20 {
            return Err("terrain LOD max depth must be between 1 and 20");
        }
        if profile.terrain_lod_probe_resolution < 3 || profile.terrain_lod_probe_resolution > 257 {
            return Err("terrain LOD probe resolution must be between 3 and 257");
        }
        if profile.terrain_lod_bakes_per_frame == 0 {
            return Err("terrain LOD bakes per frame must be greater than zero");
        }
        if profile.terrain_lod_max_inflight_bakes == 0 {
            return Err("terrain LOD in-flight bake cap must be greater than zero");
        }
        if profile.terrain_lod_tile_budget == 0 {
            return Err("terrain LOD tile budget must be greater than zero");
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
            shadow_minimum_distance: profile.shadow_minimum_distance,
            shadow_first_cascade_far_bound: profile.shadow_first_cascade_far_bound,
            shadow_maximum_distance: profile.shadow_maximum_distance,
            shadow_cascade_overlap: profile.shadow_cascade_overlap,
            shadow_depth_bias: profile.shadow_depth_bias,
            shadow_normal_bias: profile.shadow_normal_bias,
            distant_light_default_illuminance: profile.distant_light_default_illuminance,
            local_light_default_intensity: profile.local_light_default_intensity,
            rect_light_default_intensity: profile.rect_light_default_intensity,
            local_light_default_range: profile.local_light_default_range,
            local_shadow_map_near_z: profile.local_shadow_map_near_z,
            terrain_mesh_cache_bytes: profile.terrain_mesh_cache_bytes,
            terrain_lod_tile_resolution: profile.terrain_lod_tile_resolution,
            terrain_lod_cinematic_resolution: profile.terrain_lod_cinematic_resolution,
            terrain_lod_pixel_error: profile.terrain_lod_pixel_error,
            terrain_lod_max_depth: profile.terrain_lod_max_depth,
            terrain_lod_probe_resolution: profile.terrain_lod_probe_resolution,
            terrain_lod_bakes_per_frame: profile.terrain_lod_bakes_per_frame,
            terrain_lod_max_inflight_bakes: profile.terrain_lod_max_inflight_bakes,
            terrain_lod_tile_budget: profile.terrain_lod_tile_budget,
        }
    }
}

impl SettingsSection for RenderingQualitySettings {
    const KEY: &'static str = "rendering_quality";
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

/// Which directional shadow ranges were explicitly authored by USD.
///
/// Renderer settings provide defaults for omitted range attributes. This
/// provenance marker lets a live settings change update those defaults without
/// overwriting an explicit scene opinion.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ShadowRangeAuthorship {
    pub first_cascade_far_bound: bool,
    pub maximum_distance: bool,
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
        assert_eq!(settings.profile().shadow_budget_bytes, 64 * 1024 * 1024);
        assert!(estimate_directional_shadow_bytes(RenderingQuality::High, 1) > 0);
    }

    #[test]
    fn light_defaults_are_authoritative_settings_and_are_validated() {
        let mut settings = RenderingQualitySettings::default();
        settings.distant_light_default_illuminance = 90_000.0;
        settings.local_light_default_intensity = 700.0;
        settings.rect_light_default_intensity = 4_000.0;
        assert_eq!(
            settings.profile().distant_light_default_illuminance,
            90_000.0
        );
        assert_eq!(settings.profile().local_light_default_intensity, 700.0);
        assert_eq!(settings.profile().rect_light_default_intensity, 4_000.0);
        assert!(settings.validate().is_ok());

        settings.local_light_default_intensity = 0.0;
        assert_eq!(
            settings.validate(),
            Err("local-light default intensity must be finite and greater than zero")
        );
    }

    #[test]
    fn terrain_quality_is_authoritative_and_validated() {
        let mut settings = RenderingQualitySettings::default();
        assert!(settings.validate().is_ok());
        assert_eq!(
            settings.profile().terrain_lod_tile_resolution,
            RenderingQuality::Balanced
                .profile()
                .terrain_lod_tile_resolution
        );

        settings.terrain_lod_tile_resolution = 2;
        assert_eq!(
            settings.validate(),
            Err("terrain tile resolution must be between 3 and 4097")
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
