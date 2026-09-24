//! Persisted render-quality intent, the typed authored-profile boundary, and
//! the conservative shadow-allocation mechanism.

use bevy::prelude::{Component, Resource};
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::camera::{MsaaLevel, ToneMap};

/// Authored policy that supplies the concrete values for each quality choice.
pub const RENDER_QUALITY_PROFILE_HOOK: &str = "render.quality_profile";
/// Authored policy selecting the initial profile for fresh settings.
pub const RENDER_DEFAULT_QUALITY_PROFILE_HOOK: &str = "render.default_quality_profile";

lunco_hooks::declare_hook! {
    id: RENDER_QUALITY_PROFILE_HOOK,
    owner: "lunco-render",
    description: "Supply the concrete settings for a named rendering-quality profile.",
    signature: [id: String],
    output: Map,
    deterministic: true,
    required: true,
    installable: true,
}

lunco_hooks::declare_hook! {
    id: RENDER_DEFAULT_QUALITY_PROFILE_HOOK,
    owner: "lunco-render",
    description: "Select the initial rendering-quality profile for fresh settings.",
    signature: [],
    output: String,
    deterministic: true,
    required: true,
    installable: true,
}

const PROFILE_FIELD_NAMES: [&str; 69] = [
    "directional_shadow_map_size",
    "point_shadow_map_size",
    "directional_cascades",
    "shadow_filtering_quality",
    "max_directional_shadow_casters",
    "max_point_shadow_casters",
    "max_spot_shadow_casters",
    "shadow_budget_bytes",
    "horizon_shadow_cache_enabled",
    "horizon_shadow_cache_sun_threshold_deg",
    "horizon_march_steps",
    "horizon_cache_samples_per_axis",
    "shadow_minimum_distance",
    "shadow_first_cascade_far_bound",
    "shadow_maximum_distance",
    "shadow_cascade_overlap",
    "shadow_depth_bias",
    "shadow_normal_bias",
    "camera_tone_map",
    "camera_msaa",
    "camera_exposure_ev100",
    "render_failure_quiet_period_secs",
    "render_failure_give_up_after_secs",
    "camera_bloom_intensity",
    "camera_bloom_low_frequency_boost",
    "distant_light_default_illuminance",
    "local_light_default_intensity",
    "rect_light_default_intensity",
    "dome_default_intensity",
    "local_light_default_range",
    "local_shadow_map_near_z",
    "dome_cubemap_face_size",
    "primitive_sphere_longitudes",
    "primitive_sphere_latitudes",
    "primitive_radial_segments",
    "primitive_capsule_longitudes",
    "primitive_capsule_latitudes",
    "terrain_mesh_cache_bytes",
    "terrain_derived_map_resolution",
    "terrain_derived_ao_directions",
    "terrain_derived_ao_steps",
    "terrain_derived_ao_radius_fraction",
    "terrain_derived_roughness_base",
    "terrain_derived_roughness_saturation_radians",
    "terrain_derived_texture_anisotropy",
    "terrain_rock_max_instances",
    "terrain_rock_mesh_buckets",
    "terrain_rock_mesh_cube_count",
    "terrain_rock_lod_start_distance",
    "terrain_rock_lod_fade_distance",
    "terrain_lod_tile_resolution",
    "terrain_lod_cinematic_resolution",
    "terrain_lod_pixel_error",
    "terrain_lod_max_depth",
    "terrain_lod_probe_resolution",
    "terrain_lod_bakes_per_frame",
    "terrain_lod_max_inflight_bakes",
    "terrain_lod_tile_budget",
    "terrain_lod_cover_edits_per_frame",
    "terrain_lod_hysteresis_ratio",
    "terrain_lod_morph_start_ratio",
    "nurbs_surface_samples_per_control_span",
    "nurbs_surface_minimum_subdivisions",
    "nurbs_surface_maximum_subdivisions",
    "nurbs_trim_curve_samples",
    "nurbs_trim_minimum_subdivisions",
    "nurbs_trim_maximum_subdivisions",
    "curve_samples_per_segment",
    "curve_radial_segments",
];

/// Shadow-map sampling quality selected by the Graphics settings.
///
/// `Gaussian` uses Bevy's nine-sample filter and is the highest-quality mode
/// without temporal anti-aliasing. `Hardware2x2` is the lower-cost option.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShadowFilteringQuality {
    /// Four comparison samples with hardware filtering.
    Hardware2x2,
    /// A Gaussian filter with a wider nine-sample footprint.
    #[default]
    Gaussian,
}

impl ShadowFilteringQuality {
    /// Stable label used by the Graphics settings menu.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hardware2x2 => "Hardware 2×2 (fast)",
            Self::Gaussian => "Gaussian 5×5 (high quality)",
        }
    }

    /// Every selectable filter in menu order.
    pub const fn all() -> [Self; 2] {
        [Self::Hardware2x2, Self::Gaussian]
    }
}

/// The user-facing rendering-quality choices.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenderingQuality {
    /// Suggested low-cost shadow and lighting preset.
    Low,
    /// Suggested balanced shadow and lighting preset.
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

    /// Stable authored-policy key for this preset.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Balanced => "balanced",
            Self::High => "high",
        }
    }

    /// Parse a stable authored-policy key.
    pub fn parse_id(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "balanced" => Some(Self::Balanced),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// Concrete render settings supplied by the authored [`RenderingQuality`]
/// policy.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct RenderQualityProfile {
    pub directional_shadow_map_size: u32,
    pub point_shadow_map_size: u32,
    pub directional_cascades: usize,
    /// Sampling method used to smooth shadow-map edges.
    pub shadow_filtering_quality: ShadowFilteringQuality,
    pub max_directional_shadow_casters: usize,
    pub max_point_shadow_casters: usize,
    pub max_spot_shadow_casters: usize,
    /// Explicit logical `Depth32Float` shadow-storage ceiling. This is not a
    /// physical-GPU-memory guarantee; adapter limits and render failures are
    /// handled independently at the render boundary.
    pub shadow_budget_bytes: u64,
    /// Whether the renderer may replace the per-pixel horizon march with a
    /// pre-baked visibility cache. This is an explicit quality choice; it is
    /// never changed by the target platform or adapter.
    pub horizon_shadow_cache_enabled: bool,
    /// Sun-direction change in degrees that invalidates a horizon cache.
    pub horizon_shadow_cache_sun_threshold_deg: f32,
    /// Maximum iterations of the live horizon ray march.
    pub horizon_march_steps: usize,
    /// Cache bake supersamples per axis (1 means one ray; 2 means 2×2, etc.).
    pub horizon_cache_samples_per_axis: usize,
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
    /// Tonemapping curve for scene cameras.
    pub camera_tone_map: ToneMap,
    /// Multisampling level for scene cameras.
    pub camera_msaa: MsaaLevel,
    /// EV100 used by scene cameras when USD does not author exposure.
    pub camera_exposure_ev100: f32,
    /// Callback quiet period before a render failure is considered recovered.
    pub render_failure_quiet_period_secs: f64,
    /// Wall-clock grace period before a persistent render failure stops presentation.
    pub render_failure_give_up_after_secs: f64,
    /// Bloom intensity used when the USD environment omits bloom.
    pub camera_bloom_intensity: f32,
    /// Bloom low-frequency boost used when the USD environment omits bloom.
    pub camera_bloom_low_frequency_boost: f32,
    /// Illuminance used when a DistantLight omits `inputs:intensity`.
    pub distant_light_default_illuminance: f32,
    /// Luminous power used when a local SphereLight omits `inputs:intensity`.
    pub local_light_default_intensity: f32,
    /// Luminous power used when a RectLight omits `inputs:intensity`.
    pub rect_light_default_intensity: f32,
    /// Luminance used when a textured DomeLight omits `inputs:intensity`, in
    /// cd/m². This replaces UsdLux's 1.0 schema default because it is invisible
    /// at the calibrated scene exposure; authors can override it in USD.
    pub dome_default_intensity: f32,
    /// Range used when a local light leaves `lunco:light:range` at its schema
    /// default of zero (the explicit USD meaning is engine default).
    pub local_light_default_range: f32,
    /// Near Z plane used by local-light shadow maps.
    pub local_shadow_map_near_z: f32,
    /// Cubemap face size used when a textured USD dome omits its authored
    /// renderer-specific face-size override.
    pub dome_cubemap_face_size: u32,
    /// Longitudinal segments used for USD UV spheres.
    pub primitive_sphere_longitudes: u32,
    /// Latitudinal segments used for USD UV spheres.
    pub primitive_sphere_latitudes: u32,
    /// Radial segments used for USD cylinders and cones.
    pub primitive_radial_segments: u32,
    /// Longitudinal segments used for USD capsules.
    pub primitive_capsule_longitudes: u32,
    /// Latitudinal segments used for USD capsules.
    pub primitive_capsule_latitudes: u32,
    /// Maximum estimated GPU upload footprint retained by streamed terrain
    /// meshes. This is a requested cache limit, not an automatic quality
    /// downgrade; eviction is the cache's explicit response when it is full.
    pub terrain_mesh_cache_bytes: u64,
    /// Texels per side of terrain-derived roughness/AO/normal maps.
    pub terrain_derived_map_resolution: usize,
    /// Azimuth samples per texel for terrain-derived ambient occlusion.
    pub terrain_derived_ao_directions: usize,
    /// March samples per azimuth for terrain-derived ambient occlusion.
    pub terrain_derived_ao_steps: usize,
    /// Horizon-ray reach as a fraction of the terrain half-extent.
    pub terrain_derived_ao_radius_fraction: f64,
    /// Roughness of flat terrain in the derived surface map.
    pub terrain_derived_roughness_base: f32,
    /// Slope angle at which derived roughness reaches one, in radians.
    pub terrain_derived_roughness_saturation_radians: f32,
    /// Anisotropic filtering level for derived terrain textures.
    pub terrain_derived_texture_anisotropy: u16,
    /// Maximum procedural rock entities admitted from an authored density.
    pub terrain_rock_max_instances: usize,
    /// Number of shared size buckets used by procedural and placed rocks.
    pub terrain_rock_mesh_buckets: usize,
    /// Number of merged boxes used to build each shared faceted rock mesh.
    pub terrain_rock_mesh_cube_count: usize,
    /// Native distance at which procedural rocks begin their visibility fade.
    pub terrain_rock_lod_start_distance: f32,
    /// Native distance over which procedural rocks fade out.
    pub terrain_rock_lod_fade_distance: f32,
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
    /// Maximum persistent-cover split/merge edits applied per terrain per frame.
    pub terrain_lod_cover_edits_per_frame: usize,
    /// Coarsening/refinement dead-band multiplier for streamed terrain.
    pub terrain_lod_hysteresis_ratio: f64,
    /// Fraction of a tile's morph band at which geomorphing starts.
    pub terrain_lod_morph_start_ratio: f64,
    /// Samples per control-point span used for untrimmed NURBS surfaces.
    pub nurbs_surface_samples_per_control_span: usize,
    /// Minimum samples per direction used for untrimmed NURBS surfaces.
    pub nurbs_surface_minimum_subdivisions: usize,
    /// Maximum samples per direction used for untrimmed NURBS surfaces.
    pub nurbs_surface_maximum_subdivisions: usize,
    /// Samples used to approximate each NURBS trim curve.
    pub nurbs_trim_curve_samples: usize,
    /// Minimum grid subdivisions used for trimmed NURBS surfaces.
    pub nurbs_trim_minimum_subdivisions: usize,
    /// Maximum grid subdivisions used for trimmed NURBS surfaces.
    pub nurbs_trim_maximum_subdivisions: usize,
    /// Samples used for each non-linear USD curve control-point segment.
    pub curve_samples_per_segment: usize,
    /// Radial segments used to sweep USD curve tubes.
    pub curve_radial_segments: usize,
}

impl RenderQualityProfile {
    /// Read one profile from the typed authored-policy boundary.
    pub fn from_policy_value(value: &lunco_hooks::HookValue) -> Result<Self, String> {
        let lunco_hooks::HookValue::Map(entries) = value else {
            return Err(format!(
                "render profile policy returned {}, expected map",
                value.type_name()
            ));
        };

        let mut seen = HashSet::with_capacity(entries.len());
        for (key, _) in entries {
            if !PROFILE_FIELD_NAMES.contains(&key.as_str()) {
                return Err(format!(
                    "render profile policy returned unknown field '{key}'"
                ));
            }
            if !seen.insert(key.as_str()) {
                return Err(format!(
                    "render profile policy returned duplicate field '{key}'"
                ));
            }
        }
        for field in PROFILE_FIELD_NAMES {
            if !seen.contains(field) {
                return Err(format!(
                    "render profile policy omitted required field '{field}'"
                ));
            }
        }

        macro_rules! unsigned {
            ($field:ident, $ty:ty) => {
                <$ty>::try_from(profile_integer(entries, stringify!($field))?).map_err(|_| {
                    format!(
                        "render profile field '{}' is out of range",
                        stringify!($field)
                    )
                })?
            };
        }

        Ok(Self {
            directional_shadow_map_size: unsigned!(directional_shadow_map_size, u32),
            point_shadow_map_size: unsigned!(point_shadow_map_size, u32),
            directional_cascades: unsigned!(directional_cascades, usize),
            shadow_filtering_quality: match profile_string(entries, "shadow_filtering_quality")? {
                "hardware2x2" => ShadowFilteringQuality::Hardware2x2,
                "gaussian" => ShadowFilteringQuality::Gaussian,
                value => {
                    return Err(format!(
                        "render profile field 'shadow_filtering_quality' has unknown value '{value}'"
                    ));
                }
            },
            max_directional_shadow_casters: unsigned!(max_directional_shadow_casters, usize),
            max_point_shadow_casters: unsigned!(max_point_shadow_casters, usize),
            max_spot_shadow_casters: unsigned!(max_spot_shadow_casters, usize),
            shadow_budget_bytes: unsigned!(shadow_budget_bytes, u64),
            horizon_shadow_cache_enabled: profile_bool(entries, "horizon_shadow_cache_enabled")?,
            horizon_shadow_cache_sun_threshold_deg: profile_f32(
                entries,
                "horizon_shadow_cache_sun_threshold_deg",
            )?,
            horizon_march_steps: unsigned!(horizon_march_steps, usize),
            horizon_cache_samples_per_axis: unsigned!(horizon_cache_samples_per_axis, usize),
            shadow_minimum_distance: profile_f32(entries, "shadow_minimum_distance")?,
            shadow_first_cascade_far_bound: profile_f32(entries, "shadow_first_cascade_far_bound")?,
            shadow_maximum_distance: profile_f32(entries, "shadow_maximum_distance")?,
            shadow_cascade_overlap: profile_f32(entries, "shadow_cascade_overlap")?,
            shadow_depth_bias: profile_f32(entries, "shadow_depth_bias")?,
            shadow_normal_bias: profile_f32(entries, "shadow_normal_bias")?,
            camera_tone_map: match profile_string(entries, "camera_tone_map")? {
                "none" => ToneMap::None,
                "tony_mc_mapface" => ToneMap::TonyMcMapface,
                "agx" => ToneMap::AgX,
                "aces_fitted" => ToneMap::AcesFitted,
                "reinhard" => ToneMap::Reinhard,
                value => {
                    return Err(format!(
                        "render profile field 'camera_tone_map' has unknown value '{value}'"
                    ));
                }
            },
            camera_msaa: match profile_string(entries, "camera_msaa")? {
                "off" => MsaaLevel::Off,
                "x2" => MsaaLevel::X2,
                "x4" => MsaaLevel::X4,
                value => {
                    return Err(format!(
                        "render profile field 'camera_msaa' has unknown value '{value}'"
                    ));
                }
            },
            camera_exposure_ev100: profile_f32(entries, "camera_exposure_ev100")?,
            render_failure_quiet_period_secs: profile_f64(
                entries,
                "render_failure_quiet_period_secs",
            )?,
            render_failure_give_up_after_secs: profile_f64(
                entries,
                "render_failure_give_up_after_secs",
            )?,
            camera_bloom_intensity: profile_f32(entries, "camera_bloom_intensity")?,
            camera_bloom_low_frequency_boost: profile_f32(
                entries,
                "camera_bloom_low_frequency_boost",
            )?,
            distant_light_default_illuminance: profile_f32(
                entries,
                "distant_light_default_illuminance",
            )?,
            local_light_default_intensity: profile_f32(entries, "local_light_default_intensity")?,
            rect_light_default_intensity: profile_f32(entries, "rect_light_default_intensity")?,
            dome_default_intensity: profile_f32(entries, "dome_default_intensity")?,
            local_light_default_range: profile_f32(entries, "local_light_default_range")?,
            local_shadow_map_near_z: profile_f32(entries, "local_shadow_map_near_z")?,
            dome_cubemap_face_size: unsigned!(dome_cubemap_face_size, u32),
            primitive_sphere_longitudes: unsigned!(primitive_sphere_longitudes, u32),
            primitive_sphere_latitudes: unsigned!(primitive_sphere_latitudes, u32),
            primitive_radial_segments: unsigned!(primitive_radial_segments, u32),
            primitive_capsule_longitudes: unsigned!(primitive_capsule_longitudes, u32),
            primitive_capsule_latitudes: unsigned!(primitive_capsule_latitudes, u32),
            terrain_mesh_cache_bytes: unsigned!(terrain_mesh_cache_bytes, u64),
            terrain_derived_map_resolution: unsigned!(terrain_derived_map_resolution, usize),
            terrain_derived_ao_directions: unsigned!(terrain_derived_ao_directions, usize),
            terrain_derived_ao_steps: unsigned!(terrain_derived_ao_steps, usize),
            terrain_derived_ao_radius_fraction: profile_f64(
                entries,
                "terrain_derived_ao_radius_fraction",
            )?,
            terrain_derived_roughness_base: profile_f32(entries, "terrain_derived_roughness_base")?,
            terrain_derived_roughness_saturation_radians: profile_f32(
                entries,
                "terrain_derived_roughness_saturation_radians",
            )?,
            terrain_derived_texture_anisotropy: unsigned!(terrain_derived_texture_anisotropy, u16),
            terrain_rock_max_instances: unsigned!(terrain_rock_max_instances, usize),
            terrain_rock_mesh_buckets: unsigned!(terrain_rock_mesh_buckets, usize),
            terrain_rock_mesh_cube_count: unsigned!(terrain_rock_mesh_cube_count, usize),
            terrain_rock_lod_start_distance: profile_f32(
                entries,
                "terrain_rock_lod_start_distance",
            )?,
            terrain_rock_lod_fade_distance: profile_f32(entries, "terrain_rock_lod_fade_distance")?,
            terrain_lod_tile_resolution: unsigned!(terrain_lod_tile_resolution, usize),
            terrain_lod_cinematic_resolution: unsigned!(terrain_lod_cinematic_resolution, usize),
            terrain_lod_pixel_error: profile_f64(entries, "terrain_lod_pixel_error")?,
            terrain_lod_max_depth: unsigned!(terrain_lod_max_depth, u8),
            terrain_lod_probe_resolution: unsigned!(terrain_lod_probe_resolution, usize),
            terrain_lod_bakes_per_frame: unsigned!(terrain_lod_bakes_per_frame, usize),
            terrain_lod_max_inflight_bakes: unsigned!(terrain_lod_max_inflight_bakes, usize),
            terrain_lod_tile_budget: unsigned!(terrain_lod_tile_budget, usize),
            terrain_lod_cover_edits_per_frame: unsigned!(terrain_lod_cover_edits_per_frame, usize),
            terrain_lod_hysteresis_ratio: profile_f64(entries, "terrain_lod_hysteresis_ratio")?,
            terrain_lod_morph_start_ratio: profile_f64(entries, "terrain_lod_morph_start_ratio")?,
            nurbs_surface_samples_per_control_span: unsigned!(
                nurbs_surface_samples_per_control_span,
                usize
            ),
            nurbs_surface_minimum_subdivisions: unsigned!(
                nurbs_surface_minimum_subdivisions,
                usize
            ),
            nurbs_surface_maximum_subdivisions: unsigned!(
                nurbs_surface_maximum_subdivisions,
                usize
            ),
            nurbs_trim_curve_samples: unsigned!(nurbs_trim_curve_samples, usize),
            nurbs_trim_minimum_subdivisions: unsigned!(nurbs_trim_minimum_subdivisions, usize),
            nurbs_trim_maximum_subdivisions: unsigned!(nurbs_trim_maximum_subdivisions, usize),
            curve_samples_per_segment: unsigned!(curve_samples_per_segment, usize),
            curve_radial_segments: unsigned!(curve_radial_segments, usize),
        })
    }
}

fn profile_value<'a>(
    entries: &'a [(String, lunco_hooks::HookValue)],
    key: &str,
) -> Result<&'a lunco_hooks::HookValue, String> {
    entries
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value))
        .ok_or_else(|| format!("render profile policy omitted required field '{key}'"))
}

fn profile_integer(entries: &[(String, lunco_hooks::HookValue)], key: &str) -> Result<i64, String> {
    profile_value(entries, key)?
        .as_i64()
        .ok_or_else(|| format!("render profile field '{key}' must be an integer"))
}

fn profile_f64(entries: &[(String, lunco_hooks::HookValue)], key: &str) -> Result<f64, String> {
    let value = profile_value(entries, key)?
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("render profile field '{key}' must be a finite number"))?;
    Ok(value)
}

fn profile_f32(entries: &[(String, lunco_hooks::HookValue)], key: &str) -> Result<f32, String> {
    let value = profile_f64(entries, key)?;
    if value.abs() > f32::MAX as f64 {
        return Err(format!(
            "render profile field '{key}' is outside the f32 range"
        ));
    }
    Ok(value as f32)
}

fn profile_bool(entries: &[(String, lunco_hooks::HookValue)], key: &str) -> Result<bool, String> {
    match profile_value(entries, key)? {
        lunco_hooks::HookValue::Bool(value) => Ok(*value),
        _ => Err(format!("render profile field '{key}' must be a boolean")),
    }
}

fn profile_string<'a>(
    entries: &'a [(String, lunco_hooks::HookValue)],
    key: &str,
) -> Result<&'a str, String> {
    profile_value(entries, key)?
        .as_str()
        .ok_or_else(|| format!("render profile field '{key}' must be a string"))
}

impl RenderQualityProfile {
    /// Conservative allocation required when all configured caster limits are
    /// admitted at this profile's map sizes and cascade count.
    pub fn maximum_shadow_allocation_bytes(self) -> u64 {
        estimate_shadow_allocation_bytes(
            self.directional_shadow_map_size as usize,
            self.point_shadow_map_size as usize,
            self.directional_cascades,
            self.max_directional_shadow_casters,
            self.max_point_shadow_casters,
            self.max_spot_shadow_casters,
        )
    }

    /// Resolve the requested untrimmed NURBS sample count for one control-net
    /// direction. The profile is a rendering policy; USD remains the owner of
    /// the control net and its structural orders/counts.
    pub fn nurbs_surface_subdivisions(self, control_count: usize) -> usize {
        control_count
            .saturating_mul(self.nurbs_surface_samples_per_control_span)
            .clamp(
                self.nurbs_surface_minimum_subdivisions,
                self.nurbs_surface_maximum_subdivisions,
            )
    }

    /// Resolve the requested trimmed-surface grid count from its largest control
    /// direction.
    pub fn nurbs_trim_subdivisions(self, control_count: usize) -> usize {
        control_count
            .saturating_mul(self.nurbs_surface_samples_per_control_span)
            .clamp(
                self.nurbs_trim_minimum_subdivisions,
                self.nurbs_trim_maximum_subdivisions,
            )
    }
}

/// Persisted user settings for shadow and light presentation quality.
///
/// The authored quality-profile policy supplies suggested values only. Once a
/// profile is selected, these persisted fields are authoritative; the runtime
/// never silently replaces them with a lower profile because a scene or adapter
/// cannot satisfy the request.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct RenderingQualitySettings {
    pub directional_shadow_map_size: u32,
    pub point_shadow_map_size: u32,
    pub directional_cascades: usize,
    pub shadow_filtering_quality: ShadowFilteringQuality,
    pub max_directional_shadow_casters: usize,
    pub max_point_shadow_casters: usize,
    pub max_spot_shadow_casters: usize,
    pub shadow_budget_bytes: u64,
    pub horizon_shadow_cache_enabled: bool,
    pub horizon_shadow_cache_sun_threshold_deg: f32,
    pub horizon_march_steps: usize,
    pub horizon_cache_samples_per_axis: usize,
    pub shadow_minimum_distance: f32,
    pub shadow_first_cascade_far_bound: f32,
    pub shadow_maximum_distance: f32,
    pub shadow_cascade_overlap: f32,
    pub shadow_depth_bias: f32,
    pub shadow_normal_bias: f32,
    pub camera_tone_map: ToneMap,
    pub camera_msaa: MsaaLevel,
    pub camera_exposure_ev100: f32,
    pub render_failure_quiet_period_secs: f64,
    pub render_failure_give_up_after_secs: f64,
    pub camera_bloom_intensity: f32,
    pub camera_bloom_low_frequency_boost: f32,
    pub distant_light_default_illuminance: f32,
    pub local_light_default_intensity: f32,
    pub rect_light_default_intensity: f32,
    pub dome_default_intensity: f32,
    pub local_light_default_range: f32,
    pub local_shadow_map_near_z: f32,
    pub dome_cubemap_face_size: u32,
    pub primitive_sphere_longitudes: u32,
    pub primitive_sphere_latitudes: u32,
    pub primitive_radial_segments: u32,
    pub primitive_capsule_longitudes: u32,
    pub primitive_capsule_latitudes: u32,
    pub terrain_mesh_cache_bytes: u64,
    pub terrain_derived_map_resolution: usize,
    pub terrain_derived_ao_directions: usize,
    pub terrain_derived_ao_steps: usize,
    pub terrain_derived_ao_radius_fraction: f64,
    pub terrain_derived_roughness_base: f32,
    pub terrain_derived_roughness_saturation_radians: f32,
    pub terrain_derived_texture_anisotropy: u16,
    pub terrain_rock_max_instances: usize,
    pub terrain_rock_mesh_buckets: usize,
    pub terrain_rock_mesh_cube_count: usize,
    pub terrain_rock_lod_start_distance: f32,
    pub terrain_rock_lod_fade_distance: f32,
    pub terrain_lod_tile_resolution: usize,
    pub terrain_lod_cinematic_resolution: usize,
    pub terrain_lod_pixel_error: f64,
    pub terrain_lod_max_depth: u8,
    pub terrain_lod_probe_resolution: usize,
    pub terrain_lod_bakes_per_frame: usize,
    pub terrain_lod_max_inflight_bakes: usize,
    pub terrain_lod_tile_budget: usize,
    pub terrain_lod_cover_edits_per_frame: usize,
    pub terrain_lod_hysteresis_ratio: f64,
    pub terrain_lod_morph_start_ratio: f64,
    pub nurbs_surface_samples_per_control_span: usize,
    pub nurbs_surface_minimum_subdivisions: usize,
    pub nurbs_surface_maximum_subdivisions: usize,
    pub nurbs_trim_curve_samples: usize,
    pub nurbs_trim_minimum_subdivisions: usize,
    pub nurbs_trim_maximum_subdivisions: usize,
    pub curve_samples_per_segment: usize,
    pub curve_radial_segments: usize,
    /// Fresh settings wait for the authored default profile during startup.
    #[serde(skip)]
    profile_uninitialized: bool,
    /// Process-level override queued before the policy catalog is loaded.
    #[serde(skip)]
    requested_profile: Option<RenderingQuality>,
}

/// Validated profile values supplied by the authored application policy.
#[derive(Resource, Clone, Debug, Default)]
pub struct RenderingQualityProfiles {
    profiles: Vec<(RenderingQuality, RenderQualityProfile)>,
    default_quality: Option<RenderingQuality>,
    error: Option<String>,
    generation: Option<u64>,
}

impl RenderingQualityProfiles {
    /// Read the validated values for a stable profile id.
    pub fn get(&self, quality: RenderingQuality) -> Option<RenderQualityProfile> {
        self.profiles
            .iter()
            .find_map(|(id, profile)| (*id == quality).then_some(*profile))
    }

    /// Read the fresh-settings choice supplied by the authored policy.
    pub fn default_quality(&self) -> Option<RenderingQuality> {
        self.default_quality
    }

    /// Whether all shipped profile data was accepted.
    pub fn is_available(&self) -> bool {
        self.error.is_none() && self.profiles.len() == RenderingQuality::all().len()
    }

    /// Explain why no usable catalog is installed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether the hook registry has changed since this catalog was resolved.
    pub fn is_stale(&self) -> bool {
        self.generation != Some(lunco_hooks::generation())
    }

    /// Replace the complete catalog after the owner has validated every entry.
    pub fn install(
        &mut self,
        profiles: Vec<(RenderingQuality, RenderQualityProfile)>,
        default_quality: RenderingQuality,
        generation: u64,
    ) -> Result<(), String> {
        if profiles.len() != RenderingQuality::all().len()
            || RenderingQuality::all()
                .into_iter()
                .any(|quality| profiles.iter().filter(|(id, _)| *id == quality).count() != 1)
        {
            return Err("profile catalog must contain each stable quality id exactly once".into());
        }
        if !profiles.iter().any(|(id, _)| *id == default_quality) {
            return Err("default profile id is not present in the profile catalog".into());
        }
        self.profiles = profiles;
        self.default_quality = Some(default_quality);
        self.error = None;
        self.generation = Some(generation);
        Ok(())
    }

    /// Retain a visible error when the authored policy cannot be loaded.
    pub fn mark_unavailable(&mut self, error: impl Into<String>, generation: u64) {
        self.profiles.clear();
        self.default_quality = None;
        self.error = Some(error.into());
        self.generation = Some(generation);
    }
}

impl RenderingQualitySettings {
    /// Return the currently authoritative values, including custom edits.
    pub const fn profile(self) -> RenderQualityProfile {
        RenderQualityProfile {
            directional_shadow_map_size: self.directional_shadow_map_size,
            point_shadow_map_size: self.point_shadow_map_size,
            directional_cascades: self.directional_cascades,
            shadow_filtering_quality: self.shadow_filtering_quality,
            max_directional_shadow_casters: self.max_directional_shadow_casters,
            max_point_shadow_casters: self.max_point_shadow_casters,
            max_spot_shadow_casters: self.max_spot_shadow_casters,
            shadow_budget_bytes: self.shadow_budget_bytes,
            horizon_shadow_cache_enabled: self.horizon_shadow_cache_enabled,
            horizon_shadow_cache_sun_threshold_deg: self.horizon_shadow_cache_sun_threshold_deg,
            horizon_march_steps: self.horizon_march_steps,
            horizon_cache_samples_per_axis: self.horizon_cache_samples_per_axis,
            shadow_minimum_distance: self.shadow_minimum_distance,
            shadow_first_cascade_far_bound: self.shadow_first_cascade_far_bound,
            shadow_maximum_distance: self.shadow_maximum_distance,
            shadow_cascade_overlap: self.shadow_cascade_overlap,
            shadow_depth_bias: self.shadow_depth_bias,
            shadow_normal_bias: self.shadow_normal_bias,
            camera_tone_map: self.camera_tone_map,
            camera_msaa: self.camera_msaa,
            camera_exposure_ev100: self.camera_exposure_ev100,
            render_failure_quiet_period_secs: self.render_failure_quiet_period_secs,
            render_failure_give_up_after_secs: self.render_failure_give_up_after_secs,
            camera_bloom_intensity: self.camera_bloom_intensity,
            camera_bloom_low_frequency_boost: self.camera_bloom_low_frequency_boost,
            distant_light_default_illuminance: self.distant_light_default_illuminance,
            local_light_default_intensity: self.local_light_default_intensity,
            rect_light_default_intensity: self.rect_light_default_intensity,
            dome_default_intensity: self.dome_default_intensity,
            local_light_default_range: self.local_light_default_range,
            local_shadow_map_near_z: self.local_shadow_map_near_z,
            dome_cubemap_face_size: self.dome_cubemap_face_size,
            primitive_sphere_longitudes: self.primitive_sphere_longitudes,
            primitive_sphere_latitudes: self.primitive_sphere_latitudes,
            primitive_radial_segments: self.primitive_radial_segments,
            primitive_capsule_longitudes: self.primitive_capsule_longitudes,
            primitive_capsule_latitudes: self.primitive_capsule_latitudes,
            terrain_mesh_cache_bytes: self.terrain_mesh_cache_bytes,
            terrain_derived_map_resolution: self.terrain_derived_map_resolution,
            terrain_derived_ao_directions: self.terrain_derived_ao_directions,
            terrain_derived_ao_steps: self.terrain_derived_ao_steps,
            terrain_derived_ao_radius_fraction: self.terrain_derived_ao_radius_fraction,
            terrain_derived_roughness_base: self.terrain_derived_roughness_base,
            terrain_derived_roughness_saturation_radians: self
                .terrain_derived_roughness_saturation_radians,
            terrain_derived_texture_anisotropy: self.terrain_derived_texture_anisotropy,
            terrain_rock_max_instances: self.terrain_rock_max_instances,
            terrain_rock_mesh_buckets: self.terrain_rock_mesh_buckets,
            terrain_rock_mesh_cube_count: self.terrain_rock_mesh_cube_count,
            terrain_rock_lod_start_distance: self.terrain_rock_lod_start_distance,
            terrain_rock_lod_fade_distance: self.terrain_rock_lod_fade_distance,
            terrain_lod_tile_resolution: self.terrain_lod_tile_resolution,
            terrain_lod_cinematic_resolution: self.terrain_lod_cinematic_resolution,
            terrain_lod_pixel_error: self.terrain_lod_pixel_error,
            terrain_lod_max_depth: self.terrain_lod_max_depth,
            terrain_lod_probe_resolution: self.terrain_lod_probe_resolution,
            terrain_lod_bakes_per_frame: self.terrain_lod_bakes_per_frame,
            terrain_lod_max_inflight_bakes: self.terrain_lod_max_inflight_bakes,
            terrain_lod_tile_budget: self.terrain_lod_tile_budget,
            terrain_lod_cover_edits_per_frame: self.terrain_lod_cover_edits_per_frame,
            terrain_lod_hysteresis_ratio: self.terrain_lod_hysteresis_ratio,
            terrain_lod_morph_start_ratio: self.terrain_lod_morph_start_ratio,
            nurbs_surface_samples_per_control_span: self.nurbs_surface_samples_per_control_span,
            nurbs_surface_minimum_subdivisions: self.nurbs_surface_minimum_subdivisions,
            nurbs_surface_maximum_subdivisions: self.nurbs_surface_maximum_subdivisions,
            nurbs_trim_curve_samples: self.nurbs_trim_curve_samples,
            nurbs_trim_minimum_subdivisions: self.nurbs_trim_minimum_subdivisions,
            nurbs_trim_maximum_subdivisions: self.nurbs_trim_maximum_subdivisions,
            curve_samples_per_segment: self.curve_samples_per_segment,
            curve_radial_segments: self.curve_radial_segments,
        }
    }

    /// Return the authoritative profile only when every setting is usable by
    /// its runtime consumers. Runtime systems must use this boundary instead
    /// of turning an invalid setting into a different quality profile.
    pub fn validated_profile(self) -> Result<RenderQualityProfile, &'static str> {
        if self.profile_uninitialized {
            return Err("authored rendering-quality profiles have not loaded yet");
        }
        self.validate().map(|()| self.profile())
    }

    /// Identify whether the current values still equal one of the suggestions.
    pub fn preset(self, profiles: &RenderingQualityProfiles) -> Option<RenderingQuality> {
        if self.profile_uninitialized {
            return None;
        }
        RenderingQuality::all()
            .into_iter()
            .find(|quality| profiles.get(*quality) == Some(self.profile()))
    }

    /// Apply validated values chosen by the authored profile policy.
    pub fn apply_profile(&mut self, profile: RenderQualityProfile) {
        self.directional_shadow_map_size = profile.directional_shadow_map_size;
        self.point_shadow_map_size = profile.point_shadow_map_size;
        self.directional_cascades = profile.directional_cascades;
        self.shadow_filtering_quality = profile.shadow_filtering_quality;
        self.max_directional_shadow_casters = profile.max_directional_shadow_casters;
        self.max_point_shadow_casters = profile.max_point_shadow_casters;
        self.max_spot_shadow_casters = profile.max_spot_shadow_casters;
        self.shadow_budget_bytes = profile.shadow_budget_bytes;
        self.horizon_shadow_cache_enabled = profile.horizon_shadow_cache_enabled;
        self.horizon_shadow_cache_sun_threshold_deg =
            profile.horizon_shadow_cache_sun_threshold_deg;
        self.horizon_march_steps = profile.horizon_march_steps;
        self.horizon_cache_samples_per_axis = profile.horizon_cache_samples_per_axis;
        self.shadow_minimum_distance = profile.shadow_minimum_distance;
        self.shadow_first_cascade_far_bound = profile.shadow_first_cascade_far_bound;
        self.shadow_maximum_distance = profile.shadow_maximum_distance;
        self.shadow_cascade_overlap = profile.shadow_cascade_overlap;
        self.shadow_depth_bias = profile.shadow_depth_bias;
        self.shadow_normal_bias = profile.shadow_normal_bias;
        self.camera_tone_map = profile.camera_tone_map;
        self.camera_msaa = profile.camera_msaa;
        self.camera_exposure_ev100 = profile.camera_exposure_ev100;
        self.render_failure_quiet_period_secs = profile.render_failure_quiet_period_secs;
        self.render_failure_give_up_after_secs = profile.render_failure_give_up_after_secs;
        self.camera_bloom_intensity = profile.camera_bloom_intensity;
        self.camera_bloom_low_frequency_boost = profile.camera_bloom_low_frequency_boost;
        self.distant_light_default_illuminance = profile.distant_light_default_illuminance;
        self.local_light_default_intensity = profile.local_light_default_intensity;
        self.rect_light_default_intensity = profile.rect_light_default_intensity;
        self.dome_default_intensity = profile.dome_default_intensity;
        self.local_light_default_range = profile.local_light_default_range;
        self.local_shadow_map_near_z = profile.local_shadow_map_near_z;
        self.dome_cubemap_face_size = profile.dome_cubemap_face_size;
        self.primitive_sphere_longitudes = profile.primitive_sphere_longitudes;
        self.primitive_sphere_latitudes = profile.primitive_sphere_latitudes;
        self.primitive_radial_segments = profile.primitive_radial_segments;
        self.primitive_capsule_longitudes = profile.primitive_capsule_longitudes;
        self.primitive_capsule_latitudes = profile.primitive_capsule_latitudes;
        self.terrain_mesh_cache_bytes = profile.terrain_mesh_cache_bytes;
        self.terrain_derived_map_resolution = profile.terrain_derived_map_resolution;
        self.terrain_derived_ao_directions = profile.terrain_derived_ao_directions;
        self.terrain_derived_ao_steps = profile.terrain_derived_ao_steps;
        self.terrain_derived_ao_radius_fraction = profile.terrain_derived_ao_radius_fraction;
        self.terrain_derived_roughness_base = profile.terrain_derived_roughness_base;
        self.terrain_derived_roughness_saturation_radians =
            profile.terrain_derived_roughness_saturation_radians;
        self.terrain_derived_texture_anisotropy = profile.terrain_derived_texture_anisotropy;
        self.terrain_rock_max_instances = profile.terrain_rock_max_instances;
        self.terrain_rock_mesh_buckets = profile.terrain_rock_mesh_buckets;
        self.terrain_rock_mesh_cube_count = profile.terrain_rock_mesh_cube_count;
        self.terrain_rock_lod_start_distance = profile.terrain_rock_lod_start_distance;
        self.terrain_rock_lod_fade_distance = profile.terrain_rock_lod_fade_distance;
        self.terrain_lod_tile_resolution = profile.terrain_lod_tile_resolution;
        self.terrain_lod_cinematic_resolution = profile.terrain_lod_cinematic_resolution;
        self.terrain_lod_pixel_error = profile.terrain_lod_pixel_error;
        self.terrain_lod_max_depth = profile.terrain_lod_max_depth;
        self.terrain_lod_probe_resolution = profile.terrain_lod_probe_resolution;
        self.terrain_lod_bakes_per_frame = profile.terrain_lod_bakes_per_frame;
        self.terrain_lod_max_inflight_bakes = profile.terrain_lod_max_inflight_bakes;
        self.terrain_lod_tile_budget = profile.terrain_lod_tile_budget;
        self.terrain_lod_cover_edits_per_frame = profile.terrain_lod_cover_edits_per_frame;
        self.terrain_lod_hysteresis_ratio = profile.terrain_lod_hysteresis_ratio;
        self.terrain_lod_morph_start_ratio = profile.terrain_lod_morph_start_ratio;
        self.nurbs_surface_samples_per_control_span =
            profile.nurbs_surface_samples_per_control_span;
        self.nurbs_surface_minimum_subdivisions = profile.nurbs_surface_minimum_subdivisions;
        self.nurbs_surface_maximum_subdivisions = profile.nurbs_surface_maximum_subdivisions;
        self.nurbs_trim_curve_samples = profile.nurbs_trim_curve_samples;
        self.nurbs_trim_minimum_subdivisions = profile.nurbs_trim_minimum_subdivisions;
        self.nurbs_trim_maximum_subdivisions = profile.nurbs_trim_maximum_subdivisions;
        self.curve_samples_per_segment = profile.curve_samples_per_segment;
        self.curve_radial_segments = profile.curve_radial_segments;
        self.profile_uninitialized = false;
        self.requested_profile = None;
    }

    /// Queue a process-level preset request until authored profiles are loaded.
    pub fn request_profile(&mut self, quality: RenderingQuality) {
        self.requested_profile = Some(quality);
    }

    /// Whether the runtime still needs its first policy-backed profile.
    pub fn is_profile_initialized(&self) -> bool {
        !self.profile_uninitialized
    }

    /// Whether an explicit process-level profile is waiting to be applied.
    pub fn has_requested_profile(&self) -> bool {
        self.requested_profile.is_some()
    }

    /// Initialize fresh settings or apply a queued process-level choice.
    pub fn initialize_profile(
        &mut self,
        profiles: &RenderingQualityProfiles,
    ) -> Result<(), String> {
        let requested = self.requested_profile.or_else(|| {
            self.profile_uninitialized
                .then(|| profiles.default_quality())
                .flatten()
        });
        let Some(quality) = requested else {
            return if self.profile_uninitialized || self.requested_profile.is_some() {
                Err("authored default rendering-quality profile is unavailable".into())
            } else {
                Ok(())
            };
        };
        let profile = profiles.get(quality).ok_or_else(|| {
            format!(
                "authored rendering-quality profile '{}' is unavailable",
                quality.id()
            )
        })?;
        self.apply_profile(profile);
        Ok(())
    }

    /// Validate persisted or UI-edited settings before they reach Bevy.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.profile_uninitialized {
            return Err("authored rendering-quality profiles have not loaded yet");
        }
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
        if profile.shadow_budget_bytes < profile.maximum_shadow_allocation_bytes() {
            return Err("shadow byte ceiling is below the configured maximum shadow allocation");
        }
        if !profile.horizon_shadow_cache_sun_threshold_deg.is_finite()
            || profile.horizon_shadow_cache_sun_threshold_deg <= 0.0
            || profile.horizon_shadow_cache_sun_threshold_deg >= 180.0
        {
            return Err("horizon cache sun threshold must be finite and in (0, 180) degrees");
        }
        if profile.horizon_march_steps == 0 || profile.horizon_march_steps > 4096 {
            return Err("horizon march steps must be between 1 and 4096");
        }
        if profile.horizon_cache_samples_per_axis == 0 || profile.horizon_cache_samples_per_axis > 8
        {
            return Err("horizon cache samples per axis must be between 1 and 8");
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
        if !profile.camera_bloom_intensity.is_finite() || profile.camera_bloom_intensity < 0.0 {
            return Err("camera bloom intensity must be finite and non-negative");
        }
        if !profile.camera_exposure_ev100.is_finite() {
            return Err("camera exposure EV100 must be finite");
        }
        if !profile.render_failure_quiet_period_secs.is_finite()
            || profile.render_failure_quiet_period_secs <= 0.0
        {
            return Err("render failure quiet period must be finite and greater than zero");
        }
        if !profile.render_failure_give_up_after_secs.is_finite()
            || profile.render_failure_give_up_after_secs <= profile.render_failure_quiet_period_secs
        {
            return Err(
                "render failure give-up period must be finite and greater than the quiet period",
            );
        }
        if !profile.camera_bloom_low_frequency_boost.is_finite()
            || profile.camera_bloom_low_frequency_boost < 0.0
        {
            return Err("camera bloom boost must be finite and non-negative");
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
        if !profile.dome_default_intensity.is_finite() || profile.dome_default_intensity < 0.0 {
            return Err("dome default intensity must be finite and non-negative");
        }
        if profile.dome_cubemap_face_size == 0
            || !profile.dome_cubemap_face_size.is_power_of_two()
            || profile.dome_cubemap_face_size > 4096
        {
            return Err("dome cubemap face size must be a power of two between 1 and 4096");
        }
        if profile.primitive_sphere_longitudes < 3
            || profile.primitive_sphere_latitudes < 2
            || profile.primitive_radial_segments < 3
            || profile.primitive_capsule_longitudes < 3
            || profile.primitive_capsule_latitudes < 2
        {
            return Err("primitive mesh tessellation values are below their minimum");
        }
        if profile.primitive_sphere_longitudes > 4096
            || profile.primitive_sphere_latitudes > 4096
            || profile.primitive_radial_segments > 4096
            || profile.primitive_capsule_longitudes > 4096
            || profile.primitive_capsule_latitudes > 4096
        {
            return Err("primitive mesh tessellation values must be at most 4096");
        }
        if profile.terrain_mesh_cache_bytes == 0 {
            return Err("terrain mesh cache byte ceiling must be greater than zero");
        }
        if profile.terrain_derived_map_resolution == 0
            || !profile.terrain_derived_map_resolution.is_power_of_two()
            || profile.terrain_derived_map_resolution > 4096
        {
            return Err("terrain derived-map resolution must be a power of two between 1 and 4096");
        }
        if profile.terrain_derived_ao_directions == 0
            || profile.terrain_derived_ao_directions > 64
            || profile.terrain_derived_ao_steps == 0
            || profile.terrain_derived_ao_steps > 64
        {
            return Err("terrain derived ambient-occlusion samples must be between 1 and 64");
        }
        if !profile.terrain_derived_ao_radius_fraction.is_finite()
            || !(0.0..=1.0).contains(&profile.terrain_derived_ao_radius_fraction)
            || profile.terrain_derived_ao_radius_fraction == 0.0
        {
            return Err(
                "terrain derived ambient-occlusion radius fraction must be finite and in (0, 1]",
            );
        }
        if !profile.terrain_derived_roughness_base.is_finite()
            || !(0.0..=1.0).contains(&profile.terrain_derived_roughness_base)
        {
            return Err("terrain derived roughness base must be finite and in [0, 1]");
        }
        if !profile
            .terrain_derived_roughness_saturation_radians
            .is_finite()
            || !(0.0..=std::f32::consts::FRAC_PI_2)
                .contains(&profile.terrain_derived_roughness_saturation_radians)
            || profile.terrain_derived_roughness_saturation_radians == 0.0
        {
            return Err(
                "terrain derived roughness saturation angle must be finite and in (0, pi/2]",
            );
        }
        if !(1..=16).contains(&profile.terrain_derived_texture_anisotropy) {
            return Err("terrain derived texture anisotropy must be between 1 and 16");
        }
        if profile.terrain_rock_max_instances == 0 || profile.terrain_rock_max_instances > 1_000_000
        {
            return Err("terrain rock maximum instances must be between 1 and 1000000");
        }
        if profile.terrain_rock_mesh_buckets < 2 || profile.terrain_rock_mesh_buckets > 64 {
            return Err("terrain rock mesh buckets must be between 2 and 64");
        }
        if profile.terrain_rock_mesh_cube_count == 0 || profile.terrain_rock_mesh_cube_count > 64 {
            return Err("terrain rock mesh cube count must be between 1 and 64");
        }
        if !profile.terrain_rock_lod_start_distance.is_finite()
            || profile.terrain_rock_lod_start_distance < 0.0
        {
            return Err("terrain rock LOD start distance must be finite and non-negative");
        }
        if !profile.terrain_rock_lod_fade_distance.is_finite()
            || profile.terrain_rock_lod_fade_distance <= 0.0
        {
            return Err("terrain rock LOD fade distance must be finite and greater than zero");
        }
        if profile.terrain_lod_tile_resolution < 3 || profile.terrain_lod_tile_resolution > 4097 {
            return Err("terrain tile resolution must be between 3 and 4097");
        }
        if profile.terrain_lod_cinematic_resolution < 3
            || profile.terrain_lod_cinematic_resolution > 4097
        {
            return Err("cinematic terrain resolution must be between 3 and 4097");
        }
        if !profile.terrain_lod_pixel_error.is_finite()
            || !(0.1..=32.0).contains(&profile.terrain_lod_pixel_error)
        {
            return Err("terrain LOD pixel error must be finite and in [0.1, 32]");
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
        if profile.terrain_lod_cover_edits_per_frame == 0
            || profile.terrain_lod_cover_edits_per_frame > 4096
        {
            return Err("terrain LOD cover edits per frame must be between 1 and 4096");
        }
        if !profile.terrain_lod_hysteresis_ratio.is_finite()
            || profile.terrain_lod_hysteresis_ratio <= 1.0
            || profile.terrain_lod_hysteresis_ratio > 4.0
        {
            return Err("terrain LOD hysteresis ratio must be finite and in (1, 4]");
        }
        if !profile.terrain_lod_morph_start_ratio.is_finite()
            || !(0.0..1.0).contains(&profile.terrain_lod_morph_start_ratio)
        {
            return Err("terrain LOD morph start ratio must be finite and in [0, 1)");
        }
        if profile.nurbs_surface_samples_per_control_span == 0 {
            return Err("NURBS surface samples per control span must be greater than zero");
        }
        if profile.nurbs_surface_minimum_subdivisions == 0
            || profile.nurbs_surface_minimum_subdivisions
                > profile.nurbs_surface_maximum_subdivisions
        {
            return Err("NURBS surface subdivision minimum must not exceed its maximum");
        }
        if profile.nurbs_surface_maximum_subdivisions > 4096 {
            return Err("NURBS surface subdivision maximum must be at most 4096");
        }
        if profile.nurbs_trim_curve_samples == 0 {
            return Err("NURBS trim-curve samples must be greater than zero");
        }
        if profile.nurbs_trim_minimum_subdivisions == 0
            || profile.nurbs_trim_minimum_subdivisions > profile.nurbs_trim_maximum_subdivisions
        {
            return Err("NURBS trim subdivision minimum must not exceed its maximum");
        }
        if profile.nurbs_trim_maximum_subdivisions > 4096 {
            return Err("NURBS trim subdivision maximum must be at most 4096");
        }
        if profile.curve_samples_per_segment == 0 {
            return Err("curve samples per segment must be greater than zero");
        }
        if profile.curve_radial_segments < 3 {
            return Err("curve radial segments must be at least three");
        }
        if profile.curve_samples_per_segment > 4096 || profile.curve_radial_segments > 4096 {
            return Err("curve tessellation values must be at most 4096");
        }
        Ok(())
    }
}

impl Default for RenderingQualitySettings {
    fn default() -> Self {
        let profile = RenderQualityProfile::default();
        Self {
            directional_shadow_map_size: profile.directional_shadow_map_size,
            point_shadow_map_size: profile.point_shadow_map_size,
            directional_cascades: profile.directional_cascades,
            shadow_filtering_quality: profile.shadow_filtering_quality,
            max_directional_shadow_casters: profile.max_directional_shadow_casters,
            max_point_shadow_casters: profile.max_point_shadow_casters,
            max_spot_shadow_casters: profile.max_spot_shadow_casters,
            shadow_budget_bytes: profile.shadow_budget_bytes,
            horizon_shadow_cache_enabled: profile.horizon_shadow_cache_enabled,
            horizon_shadow_cache_sun_threshold_deg: profile.horizon_shadow_cache_sun_threshold_deg,
            horizon_march_steps: profile.horizon_march_steps,
            horizon_cache_samples_per_axis: profile.horizon_cache_samples_per_axis,
            shadow_minimum_distance: profile.shadow_minimum_distance,
            shadow_first_cascade_far_bound: profile.shadow_first_cascade_far_bound,
            shadow_maximum_distance: profile.shadow_maximum_distance,
            shadow_cascade_overlap: profile.shadow_cascade_overlap,
            shadow_depth_bias: profile.shadow_depth_bias,
            shadow_normal_bias: profile.shadow_normal_bias,
            camera_tone_map: profile.camera_tone_map,
            camera_msaa: profile.camera_msaa,
            camera_exposure_ev100: profile.camera_exposure_ev100,
            render_failure_quiet_period_secs: profile.render_failure_quiet_period_secs,
            render_failure_give_up_after_secs: profile.render_failure_give_up_after_secs,
            camera_bloom_intensity: profile.camera_bloom_intensity,
            camera_bloom_low_frequency_boost: profile.camera_bloom_low_frequency_boost,
            distant_light_default_illuminance: profile.distant_light_default_illuminance,
            local_light_default_intensity: profile.local_light_default_intensity,
            rect_light_default_intensity: profile.rect_light_default_intensity,
            dome_default_intensity: profile.dome_default_intensity,
            local_light_default_range: profile.local_light_default_range,
            local_shadow_map_near_z: profile.local_shadow_map_near_z,
            dome_cubemap_face_size: profile.dome_cubemap_face_size,
            primitive_sphere_longitudes: profile.primitive_sphere_longitudes,
            primitive_sphere_latitudes: profile.primitive_sphere_latitudes,
            primitive_radial_segments: profile.primitive_radial_segments,
            primitive_capsule_longitudes: profile.primitive_capsule_longitudes,
            primitive_capsule_latitudes: profile.primitive_capsule_latitudes,
            terrain_mesh_cache_bytes: profile.terrain_mesh_cache_bytes,
            terrain_derived_map_resolution: profile.terrain_derived_map_resolution,
            terrain_derived_ao_directions: profile.terrain_derived_ao_directions,
            terrain_derived_ao_steps: profile.terrain_derived_ao_steps,
            terrain_derived_ao_radius_fraction: profile.terrain_derived_ao_radius_fraction,
            terrain_derived_roughness_base: profile.terrain_derived_roughness_base,
            terrain_derived_roughness_saturation_radians: profile
                .terrain_derived_roughness_saturation_radians,
            terrain_derived_texture_anisotropy: profile.terrain_derived_texture_anisotropy,
            terrain_rock_max_instances: profile.terrain_rock_max_instances,
            terrain_rock_mesh_buckets: profile.terrain_rock_mesh_buckets,
            terrain_rock_mesh_cube_count: profile.terrain_rock_mesh_cube_count,
            terrain_rock_lod_start_distance: profile.terrain_rock_lod_start_distance,
            terrain_rock_lod_fade_distance: profile.terrain_rock_lod_fade_distance,
            terrain_lod_tile_resolution: profile.terrain_lod_tile_resolution,
            terrain_lod_cinematic_resolution: profile.terrain_lod_cinematic_resolution,
            terrain_lod_pixel_error: profile.terrain_lod_pixel_error,
            terrain_lod_max_depth: profile.terrain_lod_max_depth,
            terrain_lod_probe_resolution: profile.terrain_lod_probe_resolution,
            terrain_lod_bakes_per_frame: profile.terrain_lod_bakes_per_frame,
            terrain_lod_max_inflight_bakes: profile.terrain_lod_max_inflight_bakes,
            terrain_lod_tile_budget: profile.terrain_lod_tile_budget,
            terrain_lod_cover_edits_per_frame: profile.terrain_lod_cover_edits_per_frame,
            terrain_lod_hysteresis_ratio: profile.terrain_lod_hysteresis_ratio,
            terrain_lod_morph_start_ratio: profile.terrain_lod_morph_start_ratio,
            nurbs_surface_samples_per_control_span: profile.nurbs_surface_samples_per_control_span,
            nurbs_surface_minimum_subdivisions: profile.nurbs_surface_minimum_subdivisions,
            nurbs_surface_maximum_subdivisions: profile.nurbs_surface_maximum_subdivisions,
            nurbs_trim_curve_samples: profile.nurbs_trim_curve_samples,
            nurbs_trim_minimum_subdivisions: profile.nurbs_trim_minimum_subdivisions,
            nurbs_trim_maximum_subdivisions: profile.nurbs_trim_maximum_subdivisions,
            curve_samples_per_segment: profile.curve_samples_per_segment,
            curve_radial_segments: profile.curve_radial_segments,
            profile_uninitialized: true,
            requested_profile: None,
        }
    }
}

impl SettingsSection for RenderingQualitySettings {
    const KEY: &'static str = "rendering_quality";

    fn validate_section(&self) -> Result<(), String> {
        self.validate().map_err(str::to_owned)
    }
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

/// Records which photometric values came from Graphics defaults when a USD
/// light was projected. Live Graphics edits may update only those values;
/// authored USD intensity and range remain authoritative.
///
/// `intensity_scale` includes authored exposure and any UsdLux area-size
/// scale, so changing a default preserves the light's authored interpretation.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct LightGraphicsDefaults {
    pub intensity_uses_graphics_default: bool,
    pub intensity_scale: f32,
    pub range_uses_graphics_default: bool,
}

/// Conservative estimate for directional shadow textures and their views.
pub fn estimate_directional_shadow_bytes(
    profile: RenderQualityProfile,
    directional_light_count: usize,
) -> u64 {
    estimate_shadow_allocation_bytes(
        profile.directional_shadow_map_size as usize,
        profile.point_shadow_map_size as usize,
        profile.directional_cascades,
        directional_light_count,
        0,
        0,
    )
}

/// Logical allocation estimate for Bevy's shadow resources.
///
/// Directional cascades and spot shadows use the directional shadow-map
/// texture. Point and spot lights are separate light classes in Bevy, but a
/// spot still consumes one layer of the directional atlas while a point light
/// consumes six faces of the point-light cubemap. The estimate is deliberately
/// expressed in terms of the resources and per-light layer counts so every preflight can
/// use the same accounting instead of maintaining a second approximation. The estimate is
/// the requested depth-texture storage (`Depth32Float`, four bytes per texel); it is an
/// admission-policy number, not a promise about total physical GPU memory consumed by a
/// driver or by unrelated render resources.
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

    // Bevy's shadow maps use a depth texture. Keep the format size explicit rather than
    // hiding a safety multiplier in the byte-ceiling semantics: the ceiling is an
    // authoritative, user-authored logical allocation limit, while device capability
    // validation is performed separately at the render boundary.
    directional_texels
        .saturating_add(point_texels)
        .saturating_mul(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_profile() -> RenderQualityProfile {
        RenderQualityProfile {
            directional_shadow_map_size: 1024,
            point_shadow_map_size: 512,
            directional_cascades: 2,
            shadow_filtering_quality: ShadowFilteringQuality::Hardware2x2,
            max_directional_shadow_casters: 1,
            max_point_shadow_casters: 1,
            max_spot_shadow_casters: 1,
            shadow_budget_bytes: 1024 * 1024 * 1024,
            horizon_shadow_cache_enabled: false,
            horizon_shadow_cache_sun_threshold_deg: 0.2,
            horizon_march_steps: 24,
            horizon_cache_samples_per_axis: 1,
            shadow_minimum_distance: 0.1,
            shadow_first_cascade_far_bound: 20.0,
            shadow_maximum_distance: 600.0,
            shadow_cascade_overlap: 0.1,
            shadow_depth_bias: 0.1,
            shadow_normal_bias: 4.0,
            camera_tone_map: ToneMap::AgX,
            camera_msaa: MsaaLevel::Off,
            camera_exposure_ev100: 16.0,
            render_failure_quiet_period_secs: 0.5,
            render_failure_give_up_after_secs: 5.0,
            camera_bloom_intensity: 0.0,
            camera_bloom_low_frequency_boost: 0.0,
            distant_light_default_illuminance: 128_000.0,
            local_light_default_intensity: 1_000.0,
            rect_light_default_intensity: 10_000.0,
            dome_default_intensity: 1_000.0,
            local_light_default_range: 20.0,
            local_shadow_map_near_z: 0.2,
            dome_cubemap_face_size: 512,
            primitive_sphere_longitudes: 24,
            primitive_sphere_latitudes: 16,
            primitive_radial_segments: 32,
            primitive_capsule_longitudes: 16,
            primitive_capsule_latitudes: 8,
            terrain_mesh_cache_bytes: 256 * 1024 * 1024,
            terrain_derived_map_resolution: 512,
            terrain_derived_ao_directions: 4,
            terrain_derived_ao_steps: 4,
            terrain_derived_ao_radius_fraction: 0.1,
            terrain_derived_roughness_base: 0.6,
            terrain_derived_roughness_saturation_radians: 0.6,
            terrain_derived_texture_anisotropy: 1,
            terrain_rock_max_instances: 2_000,
            terrain_rock_mesh_buckets: 3,
            terrain_rock_mesh_cube_count: 2,
            terrain_rock_lod_start_distance: 1_500.0,
            terrain_rock_lod_fade_distance: 300.0,
            terrain_lod_tile_resolution: 33,
            terrain_lod_cinematic_resolution: 1025,
            terrain_lod_pixel_error: 4.0,
            terrain_lod_max_depth: 6,
            terrain_lod_probe_resolution: 5,
            terrain_lod_bakes_per_frame: 8,
            terrain_lod_max_inflight_bakes: 16,
            terrain_lod_tile_budget: 256,
            terrain_lod_cover_edits_per_frame: 16,
            terrain_lod_hysteresis_ratio: 1.2,
            terrain_lod_morph_start_ratio: 0.45,
            nurbs_surface_samples_per_control_span: 3,
            nurbs_surface_minimum_subdivisions: 6,
            nurbs_surface_maximum_subdivisions: 64,
            nurbs_trim_curve_samples: 12,
            nurbs_trim_minimum_subdivisions: 8,
            nurbs_trim_maximum_subdivisions: 48,
            curve_samples_per_segment: 4,
            curve_radial_segments: 6,
        }
    }

    fn valid_settings() -> RenderingQualitySettings {
        let mut settings = RenderingQualitySettings::default();
        settings.apply_profile(valid_profile());
        settings
    }

    #[test]
    fn quality_ids_are_stable_policy_keys() {
        assert_eq!(RenderingQuality::Low.id(), "low");
        assert_eq!(RenderingQuality::Balanced.id(), "balanced");
        assert_eq!(
            RenderingQuality::parse_id("high"),
            Some(RenderingQuality::High)
        );
        assert_eq!(RenderingQuality::parse_id("turbo"), None);
    }

    #[test]
    fn fresh_settings_wait_for_the_authored_catalog() {
        let mut settings = RenderingQualitySettings::default();
        assert!(settings.validated_profile().is_err());
        assert_eq!(
            settings.validated_profile(),
            Err("authored rendering-quality profiles have not loaded yet")
        );
        settings.request_profile(RenderingQuality::Balanced);
        assert!(settings.has_requested_profile());
    }

    #[test]
    fn validated_catalog_applies_and_identifies_selected_values() {
        let base = valid_profile();
        let mut low = base;
        low.directional_shadow_map_size = 512;
        let mut high = base;
        high.directional_shadow_map_size = 4096;
        let mut profiles = RenderingQualityProfiles::default();
        profiles
            .install(
                vec![
                    (RenderingQuality::Low, low),
                    (RenderingQuality::Balanced, base),
                    (RenderingQuality::High, high),
                ],
                RenderingQuality::High,
                lunco_hooks::generation(),
            )
            .unwrap();
        assert!(profiles.is_available());
        assert_eq!(profiles.default_quality(), Some(RenderingQuality::High));

        let mut fresh_settings = RenderingQualitySettings::default();
        fresh_settings.initialize_profile(&profiles).unwrap();
        assert_eq!(
            fresh_settings.preset(&profiles),
            Some(RenderingQuality::High)
        );

        let mut settings = valid_settings();
        settings.apply_profile(profiles.get(RenderingQuality::Balanced).unwrap());
        assert_eq!(settings.preset(&profiles), Some(RenderingQuality::Balanced));
        assert_eq!(settings.profile().directional_shadow_map_size, 1024);
        settings.directional_shadow_map_size = 2048;
        assert_eq!(settings.preset(&profiles), None);
    }

    #[test]
    fn catalog_rejects_duplicate_or_missing_profile_ids() {
        let mut profiles = RenderingQualityProfiles::default();
        assert!(
            profiles
                .install(
                    vec![
                        (RenderingQuality::Low, valid_profile()),
                        (RenderingQuality::Low, valid_profile()),
                        (RenderingQuality::High, valid_profile()),
                    ],
                    RenderingQuality::High,
                    lunco_hooks::generation(),
                )
                .is_err()
        );
        assert!(!profiles.is_available());
    }

    #[test]
    fn profile_parser_rejects_a_non_map_policy_result() {
        assert!(
            RenderQualityProfile::from_policy_value(&lunco_hooks::HookValue::Bool(false))
                .unwrap_err()
                .contains("expected map")
        );
    }

    #[test]
    fn shadow_allocation_estimate_saturates_and_counts_all_classes() {
        assert_eq!(
            estimate_shadow_allocation_bytes(usize::MAX, usize::MAX, usize::MAX, usize::MAX, 0, 0),
            u64::MAX
        );
        assert_eq!(
            estimate_shadow_allocation_bytes(1024, 512, 2, 1, 1, 1),
            18 * 1024 * 1024
        );
    }

    #[test]
    fn settings_validate_profile_ranges_before_render_use() {
        let mut settings = valid_settings();
        assert!(settings.validate().is_ok());

        settings.shadow_budget_bytes = 1;
        assert_eq!(
            settings.validate(),
            Err("shadow byte ceiling is below the configured maximum shadow allocation")
        );
        settings.shadow_budget_bytes = 1024 * 1024 * 1024;

        settings.horizon_march_steps = 0;
        assert_eq!(
            settings.validate(),
            Err("horizon march steps must be between 1 and 4096")
        );
        settings.horizon_march_steps = 24;

        settings.primitive_radial_segments = 2;
        assert_eq!(
            settings.validate(),
            Err("primitive mesh tessellation values are below their minimum")
        );
        settings.primitive_radial_segments = 32;

        settings.camera_exposure_ev100 = f32::NAN;
        assert_eq!(
            settings.validate(),
            Err("camera exposure EV100 must be finite")
        );
    }

    #[test]
    fn nurbs_sample_resolution_uses_the_generic_profile_values() {
        let profile = valid_profile();
        assert_eq!(profile.nurbs_surface_subdivisions(9), 27);
        assert_eq!(profile.nurbs_trim_subdivisions(9), 27);
    }
}
