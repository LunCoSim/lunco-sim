//! Persisted render-quality intent and the conservative shadow allocation
//! policy shared by the scene projectors and the workbench.

use bevy::prelude::{Component, Resource};
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};

use crate::camera::{MsaaLevel, ToneMap};

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
                shadow_budget_bytes: 256 * 1024 * 1024,
                horizon_shadow_cache_enabled: true,
                horizon_shadow_cache_sun_threshold_deg: 0.05,
                horizon_march_steps: 48,
                horizon_cache_samples_per_axis: 2,
                shadow_minimum_distance: 0.1,
                shadow_first_cascade_far_bound: 40.0,
                shadow_maximum_distance: 1500.0,
                shadow_cascade_overlap: 0.1,
                shadow_depth_bias: 0.06,
                shadow_normal_bias: 2.5,
                camera_tone_map: ToneMap::AgX,
                camera_msaa: MsaaLevel::X2,
                camera_exposure_ev100: 16.0,
                render_failure_quiet_period_secs: 0.5,
                render_failure_give_up_after_secs: 5.0,
                camera_bloom_intensity: 0.15,
                camera_bloom_low_frequency_boost: 0.7,
                distant_light_default_illuminance: 128_000.0,
                local_light_default_intensity: 1_000.0,
                rect_light_default_intensity: 10_000.0,
                dome_default_intensity: 1_000.0,
                local_light_default_range: 30.0,
                local_shadow_map_near_z: 0.1,
                dome_cubemap_face_size: 1024,
                primitive_sphere_longitudes: 48,
                primitive_sphere_latitudes: 32,
                primitive_radial_segments: 64,
                primitive_capsule_longitudes: 32,
                primitive_capsule_latitudes: 16,
                terrain_mesh_cache_bytes: 640 * 1024 * 1024,
                terrain_derived_map_resolution: 1024,
                terrain_derived_ao_directions: 8,
                terrain_derived_ao_steps: 8,
                terrain_derived_ao_radius_fraction: 0.15,
                terrain_derived_roughness_base: 0.6,
                terrain_derived_roughness_saturation_radians: 0.6,
                terrain_derived_texture_anisotropy: 4,
                terrain_rock_max_instances: 6_000,
                terrain_rock_mesh_buckets: 6,
                terrain_rock_mesh_cube_count: 4,
                terrain_rock_lod_start_distance: 2_500.0,
                terrain_rock_lod_fade_distance: 500.0,
                terrain_lod_tile_resolution: 49,
                terrain_lod_cinematic_resolution: 2049,
                terrain_lod_pixel_error: 2.0,
                terrain_lod_max_depth: 8,
                terrain_lod_probe_resolution: 9,
                terrain_lod_bakes_per_frame: 24,
                terrain_lod_max_inflight_bakes: 64,
                terrain_lod_tile_budget: 768,
                terrain_lod_cover_edits_per_frame: 64,
                terrain_lod_hysteresis_ratio: 1.30,
                terrain_lod_morph_start_ratio: 0.55,
                nurbs_surface_samples_per_control_span: 6,
                nurbs_surface_minimum_subdivisions: 8,
                nurbs_surface_maximum_subdivisions: 128,
                nurbs_trim_curve_samples: 24,
                nurbs_trim_minimum_subdivisions: 12,
                nurbs_trim_maximum_subdivisions: 96,
                curve_samples_per_segment: 8,
                curve_radial_segments: 12,
            },
            Self::Low => RenderQualityProfile {
                directional_shadow_map_size: 512,
                point_shadow_map_size: 512,
                directional_cascades: 1,
                max_directional_shadow_casters: 1,
                max_point_shadow_casters: 2,
                max_spot_shadow_casters: 2,
                shadow_budget_bytes: 32 * 1024 * 1024,
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
                terrain_lod_hysteresis_ratio: 1.20,
                terrain_lod_morph_start_ratio: 0.45,
                nurbs_surface_samples_per_control_span: 3,
                nurbs_surface_minimum_subdivisions: 6,
                nurbs_surface_maximum_subdivisions: 64,
                nurbs_trim_curve_samples: 12,
                nurbs_trim_minimum_subdivisions: 8,
                nurbs_trim_maximum_subdivisions: 48,
                curve_samples_per_segment: 4,
                curve_radial_segments: 6,
            },
            Self::High => RenderQualityProfile {
                directional_shadow_map_size: 2048,
                point_shadow_map_size: 2048,
                directional_cascades: 2,
                max_directional_shadow_casters: 2,
                max_point_shadow_casters: 8,
                max_spot_shadow_casters: 8,
                shadow_budget_bytes: 2 * 1024 * 1024 * 1024,
                horizon_shadow_cache_enabled: true,
                horizon_shadow_cache_sun_threshold_deg: 0.02,
                horizon_march_steps: 96,
                horizon_cache_samples_per_axis: 3,
                shadow_minimum_distance: 0.1,
                shadow_first_cascade_far_bound: 80.0,
                shadow_maximum_distance: 3000.0,
                shadow_cascade_overlap: 0.1,
                shadow_depth_bias: 0.03,
                shadow_normal_bias: 1.5,
                camera_tone_map: ToneMap::AgX,
                // Keep the highest profile on the stable offscreen path: X4 made the
                // large streamed Summer Space School terrain capture render black on
                // the supported RTX 5060, while the rest of this profile remains above
                // Balanced for shadows, sky, terrain, and geometry.
                camera_msaa: MsaaLevel::X2,
                camera_exposure_ev100: 16.0,
                render_failure_quiet_period_secs: 0.5,
                render_failure_give_up_after_secs: 5.0,
                camera_bloom_intensity: 0.15,
                camera_bloom_low_frequency_boost: 0.7,
                distant_light_default_illuminance: 128_000.0,
                local_light_default_intensity: 1_000.0,
                rect_light_default_intensity: 10_000.0,
                dome_default_intensity: 1_000.0,
                local_light_default_range: 50.0,
                local_shadow_map_near_z: 0.05,
                dome_cubemap_face_size: 2048,
                primitive_sphere_longitudes: 96,
                primitive_sphere_latitudes: 64,
                primitive_radial_segments: 128,
                primitive_capsule_longitudes: 64,
                primitive_capsule_latitudes: 32,
                terrain_mesh_cache_bytes: 1024 * 1024 * 1024,
                terrain_derived_map_resolution: 2048,
                terrain_derived_ao_directions: 16,
                terrain_derived_ao_steps: 16,
                terrain_derived_ao_radius_fraction: 0.2,
                terrain_derived_roughness_base: 0.6,
                terrain_derived_roughness_saturation_radians: 0.6,
                terrain_derived_texture_anisotropy: 8,
                terrain_rock_max_instances: 12_000,
                terrain_rock_mesh_buckets: 12,
                terrain_rock_mesh_cube_count: 8,
                terrain_rock_lod_start_distance: 4_000.0,
                terrain_rock_lod_fade_distance: 800.0,
                // Keep High on the proven interactive terrain envelope. High still
                // raises lighting, derived-map, rock, and mesh budgets, but doubling
                // the CDLOD cover made terrain geometry dominate the frame and did
                // not improve the authored surface contract.
                terrain_lod_tile_resolution: 49,
                terrain_lod_cinematic_resolution: 2049,
                terrain_lod_pixel_error: 2.0,
                terrain_lod_max_depth: 8,
                terrain_lod_probe_resolution: 9,
                terrain_lod_bakes_per_frame: 24,
                terrain_lod_max_inflight_bakes: 64,
                terrain_lod_tile_budget: 768,
                terrain_lod_cover_edits_per_frame: 64,
                terrain_lod_hysteresis_ratio: 1.30,
                terrain_lod_morph_start_ratio: 0.55,
                nurbs_surface_samples_per_control_span: 10,
                nurbs_surface_minimum_subdivisions: 12,
                nurbs_surface_maximum_subdivisions: 256,
                nurbs_trim_curve_samples: 48,
                nurbs_trim_minimum_subdivisions: 16,
                nurbs_trim_maximum_subdivisions: 192,
                curve_samples_per_segment: 16,
                curve_radial_segments: 24,
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
    #[serde(default = "default_horizon_shadow_cache_enabled")]
    pub horizon_shadow_cache_enabled: bool,
    #[serde(default = "default_horizon_shadow_cache_sun_threshold_deg")]
    pub horizon_shadow_cache_sun_threshold_deg: f32,
    #[serde(default = "default_horizon_march_steps")]
    pub horizon_march_steps: usize,
    #[serde(default = "default_horizon_cache_samples_per_axis")]
    pub horizon_cache_samples_per_axis: usize,
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
    #[serde(default = "default_camera_tone_map")]
    pub camera_tone_map: ToneMap,
    #[serde(default = "default_camera_msaa")]
    pub camera_msaa: MsaaLevel,
    #[serde(default = "default_camera_exposure_ev100")]
    pub camera_exposure_ev100: f32,
    #[serde(default = "default_render_failure_quiet_period_secs")]
    pub render_failure_quiet_period_secs: f64,
    #[serde(default = "default_render_failure_give_up_after_secs")]
    pub render_failure_give_up_after_secs: f64,
    #[serde(default = "default_camera_bloom_intensity")]
    pub camera_bloom_intensity: f32,
    #[serde(default = "default_camera_bloom_low_frequency_boost")]
    pub camera_bloom_low_frequency_boost: f32,
    #[serde(default = "default_distant_light_default_illuminance")]
    pub distant_light_default_illuminance: f32,
    #[serde(default = "default_local_light_default_intensity")]
    pub local_light_default_intensity: f32,
    #[serde(default = "default_rect_light_default_intensity")]
    pub rect_light_default_intensity: f32,
    #[serde(default = "default_dome_default_intensity")]
    pub dome_default_intensity: f32,
    #[serde(default = "default_local_light_default_range")]
    pub local_light_default_range: f32,
    #[serde(default = "default_local_shadow_map_near_z")]
    pub local_shadow_map_near_z: f32,
    #[serde(default = "default_dome_cubemap_face_size")]
    pub dome_cubemap_face_size: u32,
    #[serde(default = "default_primitive_sphere_longitudes")]
    pub primitive_sphere_longitudes: u32,
    #[serde(default = "default_primitive_sphere_latitudes")]
    pub primitive_sphere_latitudes: u32,
    #[serde(default = "default_primitive_radial_segments")]
    pub primitive_radial_segments: u32,
    #[serde(default = "default_primitive_capsule_longitudes")]
    pub primitive_capsule_longitudes: u32,
    #[serde(default = "default_primitive_capsule_latitudes")]
    pub primitive_capsule_latitudes: u32,
    #[serde(default = "default_terrain_mesh_cache_bytes")]
    pub terrain_mesh_cache_bytes: u64,
    #[serde(default = "default_terrain_derived_map_resolution")]
    pub terrain_derived_map_resolution: usize,
    #[serde(default = "default_terrain_derived_ao_directions")]
    pub terrain_derived_ao_directions: usize,
    #[serde(default = "default_terrain_derived_ao_steps")]
    pub terrain_derived_ao_steps: usize,
    #[serde(default = "default_terrain_derived_ao_radius_fraction")]
    pub terrain_derived_ao_radius_fraction: f64,
    #[serde(default = "default_terrain_derived_roughness_base")]
    pub terrain_derived_roughness_base: f32,
    #[serde(default = "default_terrain_derived_roughness_saturation_radians")]
    pub terrain_derived_roughness_saturation_radians: f32,
    #[serde(default = "default_terrain_derived_texture_anisotropy")]
    pub terrain_derived_texture_anisotropy: u16,
    #[serde(default = "default_terrain_rock_max_instances")]
    pub terrain_rock_max_instances: usize,
    #[serde(default = "default_terrain_rock_mesh_buckets")]
    pub terrain_rock_mesh_buckets: usize,
    #[serde(default = "default_terrain_rock_mesh_cube_count")]
    pub terrain_rock_mesh_cube_count: usize,
    #[serde(default = "default_terrain_rock_lod_start_distance")]
    pub terrain_rock_lod_start_distance: f32,
    #[serde(default = "default_terrain_rock_lod_fade_distance")]
    pub terrain_rock_lod_fade_distance: f32,
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
    #[serde(default = "default_terrain_lod_cover_edits_per_frame")]
    pub terrain_lod_cover_edits_per_frame: usize,
    #[serde(default = "default_terrain_lod_hysteresis_ratio")]
    pub terrain_lod_hysteresis_ratio: f64,
    #[serde(default = "default_terrain_lod_morph_start_ratio")]
    pub terrain_lod_morph_start_ratio: f64,
    #[serde(default = "default_nurbs_surface_samples_per_control_span")]
    pub nurbs_surface_samples_per_control_span: usize,
    #[serde(default = "default_nurbs_surface_minimum_subdivisions")]
    pub nurbs_surface_minimum_subdivisions: usize,
    #[serde(default = "default_nurbs_surface_maximum_subdivisions")]
    pub nurbs_surface_maximum_subdivisions: usize,
    #[serde(default = "default_nurbs_trim_curve_samples")]
    pub nurbs_trim_curve_samples: usize,
    #[serde(default = "default_nurbs_trim_minimum_subdivisions")]
    pub nurbs_trim_minimum_subdivisions: usize,
    #[serde(default = "default_nurbs_trim_maximum_subdivisions")]
    pub nurbs_trim_maximum_subdivisions: usize,
    #[serde(default = "default_curve_samples_per_segment")]
    pub curve_samples_per_segment: usize,
    #[serde(default = "default_curve_radial_segments")]
    pub curve_radial_segments: usize,
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

const fn default_horizon_shadow_cache_enabled() -> bool {
    balanced_profile().horizon_shadow_cache_enabled
}

const fn default_horizon_shadow_cache_sun_threshold_deg() -> f32 {
    balanced_profile().horizon_shadow_cache_sun_threshold_deg
}

const fn default_horizon_march_steps() -> usize {
    balanced_profile().horizon_march_steps
}

const fn default_horizon_cache_samples_per_axis() -> usize {
    balanced_profile().horizon_cache_samples_per_axis
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

const fn default_terrain_derived_map_resolution() -> usize {
    balanced_profile().terrain_derived_map_resolution
}

const fn default_terrain_derived_ao_directions() -> usize {
    balanced_profile().terrain_derived_ao_directions
}

const fn default_terrain_derived_ao_steps() -> usize {
    balanced_profile().terrain_derived_ao_steps
}

const fn default_terrain_derived_ao_radius_fraction() -> f64 {
    balanced_profile().terrain_derived_ao_radius_fraction
}

const fn default_terrain_derived_roughness_base() -> f32 {
    balanced_profile().terrain_derived_roughness_base
}

const fn default_terrain_derived_roughness_saturation_radians() -> f32 {
    balanced_profile().terrain_derived_roughness_saturation_radians
}

const fn default_terrain_derived_texture_anisotropy() -> u16 {
    balanced_profile().terrain_derived_texture_anisotropy
}

const fn default_terrain_rock_max_instances() -> usize {
    balanced_profile().terrain_rock_max_instances
}

const fn default_terrain_rock_mesh_buckets() -> usize {
    balanced_profile().terrain_rock_mesh_buckets
}

const fn default_terrain_rock_mesh_cube_count() -> usize {
    balanced_profile().terrain_rock_mesh_cube_count
}

const fn default_terrain_rock_lod_start_distance() -> f32 {
    balanced_profile().terrain_rock_lod_start_distance
}

const fn default_terrain_rock_lod_fade_distance() -> f32 {
    balanced_profile().terrain_rock_lod_fade_distance
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

const fn default_dome_default_intensity() -> f32 {
    balanced_profile().dome_default_intensity
}

const fn default_dome_cubemap_face_size() -> u32 {
    balanced_profile().dome_cubemap_face_size
}

const fn default_primitive_sphere_longitudes() -> u32 {
    balanced_profile().primitive_sphere_longitudes
}

const fn default_primitive_sphere_latitudes() -> u32 {
    balanced_profile().primitive_sphere_latitudes
}

const fn default_primitive_radial_segments() -> u32 {
    balanced_profile().primitive_radial_segments
}

const fn default_primitive_capsule_longitudes() -> u32 {
    balanced_profile().primitive_capsule_longitudes
}

const fn default_primitive_capsule_latitudes() -> u32 {
    balanced_profile().primitive_capsule_latitudes
}

const fn default_camera_tone_map() -> ToneMap {
    balanced_profile().camera_tone_map
}

const fn default_camera_msaa() -> MsaaLevel {
    balanced_profile().camera_msaa
}

const fn default_camera_exposure_ev100() -> f32 {
    balanced_profile().camera_exposure_ev100
}

const fn default_render_failure_quiet_period_secs() -> f64 {
    balanced_profile().render_failure_quiet_period_secs
}

const fn default_render_failure_give_up_after_secs() -> f64 {
    balanced_profile().render_failure_give_up_after_secs
}

const fn default_camera_bloom_intensity() -> f32 {
    balanced_profile().camera_bloom_intensity
}

const fn default_camera_bloom_low_frequency_boost() -> f32 {
    balanced_profile().camera_bloom_low_frequency_boost
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

const fn default_terrain_lod_cover_edits_per_frame() -> usize {
    balanced_profile().terrain_lod_cover_edits_per_frame
}

const fn default_terrain_lod_hysteresis_ratio() -> f64 {
    balanced_profile().terrain_lod_hysteresis_ratio
}

const fn default_terrain_lod_morph_start_ratio() -> f64 {
    balanced_profile().terrain_lod_morph_start_ratio
}

const fn default_nurbs_surface_samples_per_control_span() -> usize {
    balanced_profile().nurbs_surface_samples_per_control_span
}

const fn default_nurbs_surface_minimum_subdivisions() -> usize {
    balanced_profile().nurbs_surface_minimum_subdivisions
}

const fn default_nurbs_surface_maximum_subdivisions() -> usize {
    balanced_profile().nurbs_surface_maximum_subdivisions
}

const fn default_nurbs_trim_curve_samples() -> usize {
    balanced_profile().nurbs_trim_curve_samples
}

const fn default_nurbs_trim_minimum_subdivisions() -> usize {
    balanced_profile().nurbs_trim_minimum_subdivisions
}

const fn default_nurbs_trim_maximum_subdivisions() -> usize {
    balanced_profile().nurbs_trim_maximum_subdivisions
}

const fn default_curve_samples_per_segment() -> usize {
    balanced_profile().curve_samples_per_segment
}

const fn default_curve_radial_segments() -> usize {
    balanced_profile().curve_radial_segments
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
        self.validate().map(|()| self.profile())
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
        let profile = balanced_profile();
        Self {
            directional_shadow_map_size: profile.directional_shadow_map_size,
            point_shadow_map_size: profile.point_shadow_map_size,
            directional_cascades: profile.directional_cascades,
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
        }
    }
}

impl SettingsSection for RenderingQualitySettings {
    const KEY: &'static str = "rendering_quality";

    fn validate_section(&self) -> Result<(), String> {
        self.validate().map_err(str::to_owned)
    }
}

/// Why a shadow map is temporarily suppressed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShadowMapSuppressionReason {
    /// The explicit graphics caster limit excluded this caster.
    ConfiguredLimit,
}

/// Records the user/scene shadow intent while the explicit graphics caster
/// limit excludes a map. `last_applied_enabled` is the effective value written
/// by the suppression policy. A restore therefore cannot overwrite a later
/// explicit owner that changed the Bevy component while it was suppressed.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShadowMapSuppressed {
    /// The value to restore when the suppression is removed, including a
    /// policy or command change made while the map was suppressed.
    pub restore_enabled: bool,
    /// The last effective value written by the suppression owner.
    pub last_applied_enabled: bool,
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

    #[test]
    fn preset_is_only_a_suggestion_until_explicitly_applied() {
        let mut settings = RenderingQualitySettings {
            directional_shadow_map_size: 4096,
            shadow_budget_bytes: 256 * 1024 * 1024,
            ..Default::default()
        };
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
        assert_eq!(
            settings.profile().shadow_budget_bytes,
            2 * 1024 * 1024 * 1024
        );
        assert!(estimate_directional_shadow_bytes(RenderingQuality::High, 1) > 0);
    }

    #[test]
    fn shadow_allocation_estimate_saturates_instead_of_wrapping() {
        assert_eq!(
            estimate_shadow_allocation_bytes(usize::MAX, usize::MAX, usize::MAX, usize::MAX, 0, 0),
            u64::MAX
        );
    }

    #[test]
    fn every_suggested_profile_covers_its_configured_shadow_maximum() {
        for quality in RenderingQuality::all() {
            let mut settings = RenderingQualitySettings::default();
            settings.apply_preset(quality);
            let profile = settings.profile();
            assert!(
                profile.shadow_budget_bytes >= profile.maximum_shadow_allocation_bytes(),
                "{quality:?} profile exceeds its explicit shadow ceiling"
            );
            assert!(
                settings.validate().is_ok(),
                "{quality:?} profile is invalid"
            );
        }
    }

    #[test]
    fn a_shadow_ceiling_below_configured_maximum_is_rejected() {
        let mut settings = RenderingQualitySettings::default();
        settings.shadow_budget_bytes = 1;
        assert_eq!(
            settings.validate(),
            Err("shadow byte ceiling is below the configured maximum shadow allocation")
        );
    }

    #[test]
    fn validated_profile_never_exposes_invalid_quality_to_runtime() {
        let valid = RenderingQualitySettings::default();
        assert_eq!(valid.validated_profile(), Ok(valid.profile()));

        let invalid = RenderingQualitySettings {
            primitive_radial_segments: 2,
            ..valid
        };
        assert_eq!(
            invalid.validated_profile(),
            Err("primitive mesh tessellation values are below their minimum")
        );
    }

    #[test]
    fn horizon_shadow_quality_is_explicit_and_validated() {
        let mut settings = RenderingQualitySettings::default();
        assert_eq!(settings.profile().horizon_march_steps, 48);
        assert_eq!(settings.profile().horizon_cache_samples_per_axis, 2);
        assert!(settings.validate().is_ok());

        settings.horizon_march_steps = 0;
        assert_eq!(
            settings.validate(),
            Err("horizon march steps must be between 1 and 4096")
        );

        settings.horizon_march_steps = 48;
        settings.horizon_cache_samples_per_axis = 9;
        assert_eq!(
            settings.validate(),
            Err("horizon cache samples per axis must be between 1 and 8")
        );

        settings.horizon_cache_samples_per_axis = 2;
        settings.horizon_shadow_cache_sun_threshold_deg = 180.0;
        assert_eq!(
            settings.validate(),
            Err("horizon cache sun threshold must be finite and in (0, 180) degrees")
        );
    }

    #[test]
    fn light_defaults_are_authoritative_settings_and_are_validated() {
        let mut settings = RenderingQualitySettings {
            distant_light_default_illuminance: 90_000.0,
            local_light_default_intensity: 700.0,
            rect_light_default_intensity: 4_000.0,
            ..Default::default()
        };
        assert_eq!(
            settings.profile().distant_light_default_illuminance,
            90_000.0
        );
        assert_eq!(settings.profile().local_light_default_intensity, 700.0);
        assert_eq!(settings.profile().rect_light_default_intensity, 4_000.0);
        assert_eq!(settings.profile().dome_default_intensity, 1_000.0);
        assert!(settings.validate().is_ok());

        settings.local_light_default_intensity = 0.0;
        assert_eq!(
            settings.validate(),
            Err("local-light default intensity must be finite and greater than zero")
        );

        settings.local_light_default_intensity = 700.0;
        settings.dome_default_intensity = f32::NAN;
        assert_eq!(
            settings.validate(),
            Err("dome default intensity must be finite and non-negative")
        );
    }

    #[test]
    fn dome_face_size_is_explicit_and_validated() {
        let mut settings = RenderingQualitySettings::default();
        assert!(settings.dome_cubemap_face_size.is_power_of_two());
        assert!(settings.validate().is_ok());

        settings.dome_cubemap_face_size = 1000;
        assert_eq!(
            settings.validate(),
            Err("dome cubemap face size must be a power of two between 1 and 4096")
        );

        settings.dome_cubemap_face_size = 8192;
        assert_eq!(
            settings.validate(),
            Err("dome cubemap face size must be a power of two between 1 and 4096")
        );
    }

    #[test]
    fn primitive_mesh_quality_is_explicit_and_validated() {
        let mut settings = RenderingQualitySettings::default();
        assert_eq!(
            settings.profile().primitive_sphere_longitudes,
            RenderingQuality::Balanced
                .profile()
                .primitive_sphere_longitudes
        );
        assert!(settings.validate().is_ok());

        settings.primitive_radial_segments = 2;
        assert_eq!(
            settings.validate(),
            Err("primitive mesh tessellation values are below their minimum")
        );
    }

    #[test]
    fn curve_tessellation_is_explicit_and_validated() {
        let mut settings = RenderingQualitySettings::default();
        assert_eq!(settings.profile().curve_samples_per_segment, 8);
        assert_eq!(settings.profile().curve_radial_segments, 12);
        assert!(settings.validate().is_ok());

        settings.curve_radial_segments = 2;
        assert_eq!(
            settings.validate(),
            Err("curve radial segments must be at least three")
        );
    }

    #[test]
    fn terrain_quality_is_authoritative_and_validated() {
        let mut settings = RenderingQualitySettings::default();
        assert!(settings.validate().is_ok());
        assert_eq!(settings.profile().terrain_derived_map_resolution, 1024);
        assert_eq!(settings.profile().terrain_derived_ao_directions, 8);
        assert_eq!(settings.profile().terrain_derived_ao_steps, 8);
        assert_eq!(settings.profile().terrain_derived_ao_radius_fraction, 0.15);
        assert_eq!(settings.profile().terrain_derived_roughness_base, 0.6);
        assert_eq!(
            settings
                .profile()
                .terrain_derived_roughness_saturation_radians,
            0.6
        );
        assert_eq!(settings.profile().terrain_derived_texture_anisotropy, 4);
        assert_eq!(settings.profile().terrain_rock_max_instances, 6_000);
        assert_eq!(settings.profile().terrain_rock_mesh_buckets, 6);
        assert_eq!(settings.profile().terrain_rock_mesh_cube_count, 4);
        assert_eq!(settings.profile().terrain_rock_lod_start_distance, 2_500.0);
        assert_eq!(settings.profile().terrain_rock_lod_fade_distance, 500.0);
        assert_eq!(
            settings.profile().terrain_lod_tile_resolution,
            RenderingQuality::Balanced
                .profile()
                .terrain_lod_tile_resolution
        );
        assert_eq!(settings.profile().terrain_lod_cover_edits_per_frame, 64);
        assert_eq!(settings.profile().terrain_lod_hysteresis_ratio, 1.30);
        assert_eq!(settings.profile().terrain_lod_morph_start_ratio, 0.55);
        let high = RenderingQuality::High.profile();
        let balanced = RenderingQuality::Balanced.profile();
        assert_eq!(
            high.terrain_lod_tile_resolution,
            balanced.terrain_lod_tile_resolution
        );
        assert_eq!(
            high.terrain_lod_cinematic_resolution,
            balanced.terrain_lod_cinematic_resolution
        );
        assert_eq!(
            high.terrain_lod_pixel_error,
            balanced.terrain_lod_pixel_error
        );
        assert_eq!(high.terrain_lod_max_depth, balanced.terrain_lod_max_depth);
        assert_eq!(
            high.terrain_lod_probe_resolution,
            balanced.terrain_lod_probe_resolution
        );
        assert_eq!(
            high.terrain_lod_bakes_per_frame,
            balanced.terrain_lod_bakes_per_frame
        );
        assert_eq!(
            high.terrain_lod_max_inflight_bakes,
            balanced.terrain_lod_max_inflight_bakes
        );
        assert_eq!(
            high.terrain_lod_tile_budget,
            balanced.terrain_lod_tile_budget
        );
        assert_eq!(
            high.terrain_lod_cover_edits_per_frame,
            balanced.terrain_lod_cover_edits_per_frame
        );
        assert_eq!(
            high.terrain_lod_hysteresis_ratio,
            balanced.terrain_lod_hysteresis_ratio
        );
        assert_eq!(
            high.terrain_lod_morph_start_ratio,
            balanced.terrain_lod_morph_start_ratio
        );

        settings.terrain_lod_tile_resolution = 2;
        assert_eq!(
            settings.validate(),
            Err("terrain tile resolution must be between 3 and 4097")
        );

        settings.terrain_lod_tile_resolution = 49;
        settings.terrain_lod_pixel_error = 0.05;
        assert_eq!(
            settings.validate(),
            Err("terrain LOD pixel error must be finite and in [0.1, 32]")
        );

        settings.terrain_lod_pixel_error = 33.0;
        assert_eq!(
            settings.validate(),
            Err("terrain LOD pixel error must be finite and in [0.1, 32]")
        );

        settings.terrain_lod_pixel_error = 2.0;
        settings.terrain_derived_map_resolution = 1000;
        assert_eq!(
            settings.validate(),
            Err("terrain derived-map resolution must be a power of two between 1 and 4096")
        );

        settings.terrain_derived_map_resolution = 1024;
        settings.terrain_derived_ao_steps = 65;
        assert_eq!(
            settings.validate(),
            Err("terrain derived ambient-occlusion samples must be between 1 and 64")
        );

        settings.terrain_derived_ao_steps = 8;
        settings.terrain_derived_ao_radius_fraction = 0.0;
        assert_eq!(
            settings.validate(),
            Err("terrain derived ambient-occlusion radius fraction must be finite and in (0, 1]")
        );

        settings.terrain_derived_ao_radius_fraction = 0.15;
        settings.terrain_derived_texture_anisotropy = 17;
        assert_eq!(
            settings.validate(),
            Err("terrain derived texture anisotropy must be between 1 and 16")
        );

        settings.terrain_derived_texture_anisotropy = 4;
        settings.terrain_rock_mesh_buckets = 1;
        assert_eq!(
            settings.validate(),
            Err("terrain rock mesh buckets must be between 2 and 64")
        );

        settings.terrain_rock_mesh_buckets = 6;
        settings.terrain_rock_lod_fade_distance = 0.0;
        assert_eq!(
            settings.validate(),
            Err("terrain rock LOD fade distance must be finite and greater than zero")
        );

        settings.terrain_rock_lod_fade_distance = 500.0;
        settings.terrain_lod_cover_edits_per_frame = 0;
        assert_eq!(
            settings.validate(),
            Err("terrain LOD cover edits per frame must be between 1 and 4096")
        );

        settings.terrain_lod_cover_edits_per_frame = 64;
        settings.terrain_lod_hysteresis_ratio = 1.0;
        assert_eq!(
            settings.validate(),
            Err("terrain LOD hysteresis ratio must be finite and in (1, 4]")
        );

        settings.terrain_lod_hysteresis_ratio = 1.30;
        settings.terrain_lod_morph_start_ratio = 1.0;
        assert_eq!(
            settings.validate(),
            Err("terrain LOD morph start ratio must be finite and in [0, 1)")
        );
    }

    #[test]
    fn camera_quality_is_explicit_and_preset_only() {
        let mut settings = RenderingQualitySettings::default();
        assert_eq!(settings.profile().camera_tone_map, ToneMap::AgX);
        assert_eq!(settings.profile().camera_exposure_ev100, 16.0);
        assert!(settings.profile().camera_bloom_intensity > 0.0);

        settings.camera_bloom_intensity = 0.0;
        assert!(settings.validate().is_ok());
        assert!(settings.preset().is_none());

        settings.apply_preset(RenderingQuality::Low);
        assert_eq!(settings.profile().camera_bloom_intensity, 0.0);
        assert_eq!(settings.profile().camera_msaa, MsaaLevel::Off);
        assert_eq!(settings.preset(), Some(RenderingQuality::Low));
    }

    #[test]
    fn camera_exposure_rejects_non_finite_values_without_normalization() {
        let settings = RenderingQualitySettings {
            camera_exposure_ev100: f32::NAN,
            ..Default::default()
        };
        assert_eq!(
            settings.validate(),
            Err("camera exposure EV100 must be finite")
        );
    }

    #[test]
    fn render_recovery_timings_are_explicit_and_ordered() {
        let mut settings = RenderingQualitySettings::default();
        settings.render_failure_quiet_period_secs = 0.75;
        settings.render_failure_give_up_after_secs = 12.0;
        assert!(settings.validate().is_ok());

        settings.render_failure_give_up_after_secs = 0.75;
        assert_eq!(
            settings.validate(),
            Err("render failure give-up period must be finite and greater than the quiet period")
        );
    }

    #[test]
    fn camera_bloom_rejects_invalid_values_without_clamping() {
        let mut settings = RenderingQualitySettings {
            camera_bloom_intensity: f32::NAN,
            ..Default::default()
        };
        assert_eq!(
            settings.validate(),
            Err("camera bloom intensity must be finite and non-negative")
        );

        settings.camera_bloom_intensity = -1.0;
        assert_eq!(
            settings.validate(),
            Err("camera bloom intensity must be finite and non-negative")
        );
    }

    #[test]
    fn nurbs_tessellation_is_explicit_and_validated() {
        let balanced = RenderingQuality::Balanced.profile();
        assert_eq!(balanced.nurbs_surface_subdivisions(9), 54);
        assert_eq!(balanced.nurbs_trim_subdivisions(9), 54);

        let mut settings = RenderingQualitySettings::default();
        settings.nurbs_surface_samples_per_control_span = 2;
        settings.nurbs_surface_minimum_subdivisions = 16;
        settings.nurbs_surface_maximum_subdivisions = 20;
        assert_eq!(settings.profile().nurbs_surface_subdivisions(4), 16);
        assert_eq!(settings.profile().nurbs_surface_subdivisions(20), 20);
        assert!(settings.validate().is_ok());

        settings.nurbs_surface_maximum_subdivisions = 8;
        assert_eq!(
            settings.validate(),
            Err("NURBS surface subdivision minimum must not exceed its maximum")
        );
    }

    #[test]
    fn all_shadow_classes_share_one_logical_allocation_estimate() {
        assert_eq!(
            estimate_shadow_allocation_bytes(1024, 512, 2, 1, 1, 1),
            18 * 1024 * 1024
        );
    }
}
