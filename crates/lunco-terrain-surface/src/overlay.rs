//! Terrain **analysis diagnostic** — the render VIEW of a [`SurfaceField`].
//!
//! A [`SurfaceField`](lunco_terrain_core::SurfaceField) is data (headless, queried by
//! [`TerrainField`](crate::query)); this module is ONE consumer of it — the on-screen
//! colourised diagnostic material used by the streamed LOD tiles. The slope-hazard
//! transfer (green ≤ safe angle → red ≥ cliff angle) is evaluated **in the separate
//! diagnostic shader**, running the SAME smoothstep + ramp as
//! [`lunco_terrain_core::transfer`] (one definition, shared via the `lunco::transfer`
//! WGSL module), so the colour RAMP matches the legend swatch.
//!
//! **The slope it ramps is view-dependent, and that is a known limit.** The shader
//! takes its slope from the baked DEM-resolution normal map where that map is bound
//! (`weight_normal > 0` — the far/coarse tiles, exactly where the LOD mesh has thrown
//! the relief away), and otherwise from the tile's own geometric normal. Near tiles
//! out-resolve the map, so their mesh normal IS the finer truth; but a tile whose
//! geometry is coarse and whose map is not yet baked still shades from LOD geometry
//! and can under-report a cliff. So the pixel is a good guide, **not** a substitute
//! for querying the field: a headless `TerrainField`/`SlopeField` read (un-band-limited
//! oracle, `eps = cell size`) is the authority a traversability decision must use.
//!
//! Everything is **uniform-driven**: [`TerrainOverlayParams`] flows into the separate
//! diagnostic material as a handful of floats ([`OverlayUniforms`]), so re-tuning the
//! critical angle is a uniform write — no re-bake and no production-pipeline
//! permutation. New tiles and live edits use [`sync_terrain_overlay`]. See
//! `docs/architecture/terrain-layered-rendering.md`.

use bevy::prelude::*;
use lunco_core::{on_command, register_commands, Command};
use lunco_materials::ShaderLook;

use crate::derived_layers::{TerrainAuthoredMaps, TerrainDerivedMaps};
use crate::oracle::DemHeightField;
use crate::stream_viz::LodTiles;
use crate::stream_viz::TileShadowCache;

/// The diagnostic tool's controls. `Copy` so they thread cheaply through the
/// tile replacement path. They are consumed only by `terrain_debug.wgsl`; the
/// production terrain material does not declare diagnostic inputs.
#[derive(Clone, Copy)]
pub struct OverlayUniforms {
    /// `0` = production material, `1` = slope hazard, `2` = LOD depth.
    pub mode: f32,
    /// Blend weight of the diagnostic colour over its diagnostic base (`0..1`).
    pub opacity: f32,
    /// Slope (radians) at/below which ground is fully traversable (green).
    pub safe_rad: f32,
    /// Slope (radians) at/above which ground is impassable (red).
    pub cliff_rad: f32,
}

/// The authored shader intent used by the terrain diagnostic tool.
///
/// This is configuration, not terrain production policy. The tool can replace
/// the look at runtime through `SetTerrainOverlay`; the tile streamer only
/// receives the selected intent and never names a diagnostic asset.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct TerrainDiagnosticLook(pub ShaderLook);

impl Default for TerrainDiagnosticLook {
    fn default() -> Self {
        let shader = lunco_assets::engine_asset_uri("shaders/terrain_debug.wgsl");
        Self(ShaderLook::new(shader.clone()).with_vertex_shader(shader))
    }
}

/// Diagnostic modes consumed by `terrain_debug.wgsl`.
pub mod overlay_mode {
    /// Production material — the diagnostic tool is inactive.
    pub const OFF: f32 = 0.0;
    /// Slope-hazard traversability colouring.
    pub const SLOPE_HAZARD: f32 = 1.0;
    /// CDLOD tile depth diagnostic.
    pub const LOD_DEPTH: f32 = 2.0;
}

impl OverlayUniforms {
    /// The disabled state — every tile builds with this until an overlay is armed.
    pub const OFF: Self = Self {
        mode: overlay_mode::OFF,
        opacity: 0.0,
        safe_rad: 0.0,
        cliff_rad: 0.0,
    };
}

/// Live-tunable terrain analysis-overlay state (global across terrains; Inspector /
/// `SetTerrainOverlay` command). Stored in degrees for a friendly UI; converted to the
/// shader's radian uniforms by [`uniforms`](Self::uniforms).
#[derive(Resource, Clone, Copy, PartialEq, Reflect)]
#[reflect(Resource)]
pub struct TerrainOverlayParams {
    /// Whether the separate terrain diagnostic material is active.
    pub enabled: bool,
    /// Slope (degrees) up to which ground is coloured green (safe).
    pub safe_deg: f32,
    /// Slope (degrees) at/beyond which ground is coloured red (cliff) — the
    /// **critical angle**, the headline live knob.
    pub cliff_deg: f32,
    /// Diagnostic colour blend opacity (`0..1`).
    pub opacity: f32,
    /// Select the LOD-depth diagnostic instead of slope hazard. Still requires
    /// `enabled`.
    pub lod_depth: bool,
}

impl Default for TerrainOverlayParams {
    fn default() -> Self {
        // Off by default (normal rendering is untouched); the angles match the
        // derived-map hazard bake defaults so arming it looks consistent.
        Self {
            enabled: false,
            safe_deg: 15.0,
            cliff_deg: 30.0,
            opacity: 0.6,
            lod_depth: false,
        }
    }
}

impl TerrainOverlayParams {
    /// The shader-facing uniforms for the current state — [`OverlayUniforms::OFF`]
    /// when disabled, so a build never leaks a stale colour.
    pub fn uniforms(&self) -> OverlayUniforms {
        if !self.enabled {
            return OverlayUniforms::OFF;
        }
        OverlayUniforms {
            mode: if self.lod_depth {
                overlay_mode::LOD_DEPTH
            } else {
                overlay_mode::SLOPE_HAZARD
            },
            opacity: self.opacity.clamp(0.0, 1.0),
            safe_rad: self.safe_deg.to_radians(),
            cliff_rad: self.cliff_deg.to_radians(),
        }
    }
}

/// Arm / re-tune the terrain analysis overlay at runtime (MCP / scripting / UI).
///
/// **Every field is optional: an OMITTED field keeps its current value.** So
/// `{ "enabled": true }` arms the overlay with the existing angles/opacity, and
/// `{ "cliff_deg": 25 }` re-tunes the critical angle without touching `enabled`.
///
/// The fields are `Option<T>` rather than zero-sentinels because the sentinel form
/// could not represent "omitted" for `enabled` — `#[Command(default)]` gave it
/// `false`, so a re-tune like `{"cliff_deg":25}` silently turned the overlay OFF —
/// and it made `opacity: 0` unsettable.
#[Command(default)]
pub struct SetTerrainOverlay {
    pub enabled: Option<bool>,
    pub safe_deg: Option<f32>,
    pub cliff_deg: Option<f32>,
    pub opacity: Option<f32>,
    /// Switch the overlay to the LOD-depth view (still needs `enabled`).
    pub lod_depth: Option<bool>,
    /// Replace the diagnostic fragment shader asset. Omitted keeps the current
    /// diagnostic material.
    pub shader: Option<String>,
    /// Replace the diagnostic vertex shader asset. Omitted keeps the current
    /// diagnostic vertex stage.
    pub vertex_shader: Option<String>,
}

#[on_command(SetTerrainOverlay)]
fn on_set_terrain_overlay(
    trigger: On<SetTerrainOverlay>,
    mut params: ResMut<TerrainOverlayParams>,
    mut diagnostic: ResMut<TerrainDiagnosticLook>,
) {
    let before = *params;
    let ev = trigger.event();
    if ev
        .shader
        .as_deref()
        .is_some_and(|shader| shader.trim().is_empty())
    {
        warn!("[terrain-overlay] diagnostic shader source cannot be empty; request ignored");
        return;
    }
    if ev
        .vertex_shader
        .as_deref()
        .is_some_and(|shader| shader.trim().is_empty())
    {
        warn!("[terrain-overlay] diagnostic vertex shader source cannot be empty; request ignored");
        return;
    }
    if let Some(lod_depth) = ev.lod_depth {
        params.lod_depth = lod_depth;
    }
    if let Some(enabled) = ev.enabled {
        params.enabled = enabled;
    }
    if let Some(safe) = ev.safe_deg {
        params.safe_deg = safe;
    }
    if let Some(cliff) = ev.cliff_deg {
        params.cliff_deg = cliff;
    }
    if let Some(opacity) = ev.opacity {
        params.opacity = opacity.clamp(0.0, 1.0);
    }
    if let Some(shader) = ev.shader.as_deref() {
        diagnostic.0.shader = lunco_assets::engine_asset_uri(shader.trim());
    }
    if let Some(vertex_shader) = ev.vertex_shader.as_deref() {
        diagnostic.0.vertex_shader = Some(lunco_assets::engine_asset_uri(vertex_shader.trim()));
    }
    debug!(
        "[terrain-overlay] enabled={} lod_depth={} safe={}° cliff={}° opacity={}",
        params.enabled, params.lod_depth, params.safe_deg, params.cliff_deg, params.opacity
    );
    if before.enabled != params.enabled || before.lod_depth != params.lod_depth {
        info!(
            "[seminar] terrain diagnostic toggle: enabled={} mode={} safe={:.1}° cliff={:.1}° opacity={:.2}",
            params.enabled,
            if params.lod_depth { "lod" } else { "slope" },
            params.safe_deg,
            params.cliff_deg,
            params.opacity,
        );
    }
}

register_commands!(on_set_terrain_overlay);

/// Switch resident tiles between the production material and the separate
/// diagnostic material when the tool changes. This is a material replacement,
/// not a branch or uniform in the production shader.
pub fn sync_terrain_overlay(
    mut commands: Commands,
    params: Res<TerrainOverlayParams>,
    diagnostic: Res<TerrainDiagnosticLook>,
    terrains: Query<
        (
            &LodTiles,
            &DemHeightField,
            Option<&TerrainDerivedMaps>,
            Option<&TerrainAuthoredMaps>,
            Option<&TileShadowCache>,
            &ShaderLook,
        ),
        With<crate::terrain::DemTerrainSurface>,
    >,
    mut looks: Query<&mut ShaderLook, Without<crate::terrain::DemTerrainSurface>>,
) {
    if !params.is_changed() && !diagnostic.is_changed() {
        return;
    }
    let u = params.uniforms();
    let diagnostic = diagnostic.0.clone();
    for (tiles, height_field, maps, authored, shadow, template) in &terrains {
        for (entity, depth, morph_start, morph_end) in tiles.tile_material_specs() {
            if let Ok(mut look) = looks.get_mut(entity) {
                *look = crate::stream_viz::tile_look(
                    template,
                    depth,
                    morph_start,
                    morph_end,
                    maps,
                    height_field.0.half_extent(),
                    authored,
                    shadow,
                    &diagnostic,
                    u,
                );
                if u.mode > 0.5 {
                    commands
                        .entity(entity)
                        .try_insert(crate::stream_viz::TerrainDiagnosticTile);
                } else {
                    commands
                        .entity(entity)
                        .try_remove::<crate::stream_viz::TerrainDiagnosticTile>();
                }
            }
        }
    }
}

/// Register the overlay resource, the `SetTerrainOverlay` command, and the live-sync
/// system. Idempotent resource init so plugin ordering doesn't matter.
pub fn register(app: &mut App) {
    app.init_resource::<TerrainOverlayParams>()
        .init_resource::<TerrainDiagnosticLook>();
    app.register_type::<TerrainOverlayParams>();
    app.add_systems(Update, sync_terrain_overlay);
    register_all_commands(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<TerrainOverlayParams>();
        app.init_resource::<TerrainDiagnosticLook>();
        app.add_observer(on_set_terrain_overlay);
        app
    }

    /// D1: an OMITTED field keeps its current value. Re-tuning the critical angle
    /// must not disarm the overlay (the old `enabled: bool` + `#[Command(default)]`
    /// made `{"cliff_deg":25}` silently turn it OFF).
    #[test]
    fn retuning_an_angle_does_not_disarm_the_overlay() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<TerrainOverlayParams>()
            .enabled = true;

        app.world_mut().trigger(SetTerrainOverlay {
            cliff_deg: Some(25.0),
            ..default()
        });
        app.world_mut().flush();

        let p = *app.world().resource::<TerrainOverlayParams>();
        assert!(p.enabled, "omitted `enabled` must keep the overlay armed");
        assert_eq!(p.cliff_deg, 25.0);
        assert_eq!(p.safe_deg, 15.0, "omitted `safe_deg` must keep its value");
        assert_eq!(p.opacity, 0.6, "omitted `opacity` must keep its value");
    }

    /// D1 (same class): the zero-sentinel made `opacity: 0` unsettable.
    #[test]
    fn opacity_zero_is_settable() {
        let mut app = test_app();
        app.world_mut().trigger(SetTerrainOverlay {
            opacity: Some(0.0),
            ..default()
        });
        app.world_mut().flush();
        assert_eq!(app.world().resource::<TerrainOverlayParams>().opacity, 0.0);
    }

    /// And an explicit `enabled: false` still disarms it.
    #[test]
    fn explicit_disable_still_works() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<TerrainOverlayParams>()
            .enabled = true;
        app.world_mut().trigger(SetTerrainOverlay {
            enabled: Some(false),
            ..default()
        });
        app.world_mut().flush();
        assert!(!app.world().resource::<TerrainOverlayParams>().enabled);
    }

    #[test]
    fn diagnostic_shader_sources_are_runtime_configurable() {
        let mut app = test_app();
        app.world_mut().trigger(SetTerrainOverlay {
            shader: Some("shaders/custom_diagnostic.wgsl".into()),
            vertex_shader: Some("shaders/custom_diagnostic.wgsl".into()),
            ..default()
        });
        app.world_mut().flush();

        let look = &app.world().resource::<TerrainDiagnosticLook>().0;
        assert_eq!(look.shader, "lunco://shaders/custom_diagnostic.wgsl");
        assert_eq!(
            look.vertex_shader.as_deref(),
            Some("lunco://shaders/custom_diagnostic.wgsl")
        );
    }
}
