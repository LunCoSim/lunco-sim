//! Terrain **analysis overlay** — the render VIEW of a [`SurfaceField`], the
//! in-material shading plane of `Data → Transfer → Blend`.
//!
//! A [`SurfaceField`](lunco_terrain_core::SurfaceField) is data (headless, queried by
//! [`TerrainField`](crate::query)); this module is ONE consumer of it — the on-screen
//! colourised overlay painted over the lit regolith on the streamed LOD tiles. The
//! slope-hazard transfer (green ≤ safe angle → red ≥ cliff angle) is evaluated **in
//! the tile shader**, running the SAME smoothstep + ramp as
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
//! Everything is **uniform-driven**: [`TerrainOverlayParams`] flows into the tile
//! materials as a handful of floats ([`OverlayUniforms`]), so re-tuning the critical
//! angle is a uniform write — no re-bake, no pipeline permutation. New tiles pick up
//! the current params at build ([`OverlayUniforms::apply`] in `build_tile_material`);
//! a live edit to the params is pushed onto the already-resident materials by
//! [`sync_terrain_overlay`]. See `docs/architecture/terrain-layered-rendering.md`.

use bevy::prelude::*;
use lunco_core::{on_command, register_commands, Command};
use lunco_materials::{ParamValue, ShaderLook};

use crate::stream_viz::{set_param, LodTiles, TerrainShaderMode};

/// The overlay's shader uniforms — the compact, per-material form of
/// [`TerrainOverlayParams`]. `Copy` so it threads cheaply through the tile-spawn path.
/// Only the Lit terrain shaders (`terrain_geomorph`/`_web`) declare these params; the
/// flat/debug shader simply doesn't, and the by-name writes are ignored there.
#[derive(Clone, Copy)]
pub struct OverlayUniforms {
    /// `0` = no overlay, `1` = slope hazard.
    pub mode: f32,
    /// Blend weight of the overlay colour over the lit surface (`0..1`).
    pub opacity: f32,
    /// Slope (radians) at/below which ground is fully traversable (green).
    pub safe_rad: f32,
    /// Slope (radians) at/above which ground is impassable (red).
    pub cliff_rad: f32,
}

impl OverlayUniforms {
    /// The disabled state — every tile builds with this until an overlay is armed.
    pub const OFF: Self = Self { mode: 0.0, opacity: 0.0, safe_rad: 0.0, cliff_rad: 0.0 };

    /// Write the four params onto a tile's [`ShaderLook`] by name. They are part of
    /// the look's key, so every tile at the same overlay setting still shares one
    /// material — the overlay is a *uniform*, not a re-bake (`D2`).
    pub fn apply(&self, look: &mut ShaderLook) {
        set_param(look, "overlay_mode", ParamValue::F32(self.mode));
        set_param(look, "overlay_opacity", ParamValue::F32(self.opacity));
        set_param(look, "overlay_safe_rad", ParamValue::F32(self.safe_rad));
        set_param(look, "overlay_cliff_rad", ParamValue::F32(self.cliff_rad));
    }
}

/// Live-tunable terrain analysis-overlay state (global across terrains; Inspector /
/// `SetTerrainOverlay` command). Stored in degrees for a friendly UI; converted to the
/// shader's radian uniforms by [`uniforms`](Self::uniforms).
#[derive(Resource, Clone, Copy, PartialEq, Reflect)]
#[reflect(Resource)]
pub struct TerrainOverlayParams {
    /// Whether the slope-hazard overlay is drawn at all.
    pub enabled: bool,
    /// Slope (degrees) up to which ground is coloured green (safe).
    pub safe_deg: f32,
    /// Slope (degrees) at/beyond which ground is coloured red (cliff) — the
    /// **critical angle**, the headline live knob.
    pub cliff_deg: f32,
    /// Overlay blend opacity over the lit regolith (`0..1`).
    pub opacity: f32,
}

impl Default for TerrainOverlayParams {
    fn default() -> Self {
        // Off by default (normal rendering is untouched); the angles match the
        // derived-map hazard bake defaults so arming it looks consistent.
        Self { enabled: false, safe_deg: 15.0, cliff_deg: 30.0, opacity: 0.6 }
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
            mode: 1.0,
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
}

#[on_command(SetTerrainOverlay)]
fn on_set_terrain_overlay(
    trigger: On<SetTerrainOverlay>,
    mut params: ResMut<TerrainOverlayParams>,
) {
    let ev = trigger.event();
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
    debug!(
        "[terrain-overlay] enabled={} safe={}° cliff={}° opacity={}",
        params.enabled, params.safe_deg, params.cliff_deg, params.opacity
    );
}

register_commands!(on_set_terrain_overlay);

/// Push the current overlay params onto every resident tile's look when they change
/// — the live-tuning path (freshly-built tiles already read the current params at
/// build). Change-driven: no-op on unchanged frames, so a still overlay costs
/// nothing; a slider drag re-states N looks that all collapse back onto ONE material
/// in the binder's cache.
pub fn sync_terrain_overlay(
    params: Res<TerrainOverlayParams>,
    terrains: Query<&LodTiles>,
    mut looks: Query<&mut ShaderLook>,
) {
    if !params.is_changed() {
        return;
    }
    let u = params.uniforms();
    // D8 — Lit tiles ONLY: the flat/debug shader declares no `overlay_*` params, so
    // writing them there would only insert dead keys and mint a pointless material
    // variant per band (`tile_look` gates the same way).
    for tiles in &terrains {
        if tiles.shader_mode() != TerrainShaderMode::Lit {
            continue;
        }
        for entity in tiles.tile_entities() {
            if let Ok(mut look) = looks.get_mut(entity) {
                u.apply(&mut look);
            }
        }
    }
}

/// Register the overlay resource, the `SetTerrainOverlay` command, and the live-sync
/// system. Idempotent resource init so plugin ordering doesn't matter.
pub fn register(app: &mut App) {
    app.init_resource::<TerrainOverlayParams>();
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
        app.add_observer(on_set_terrain_overlay);
        app
    }

    /// D1: an OMITTED field keeps its current value. Re-tuning the critical angle
    /// must not disarm the overlay (the old `enabled: bool` + `#[Command(default)]`
    /// made `{"cliff_deg":25}` silently turn it OFF).
    #[test]
    fn retuning_an_angle_does_not_disarm_the_overlay() {
        let mut app = test_app();
        app.world_mut().resource_mut::<TerrainOverlayParams>().enabled = true;

        app.world_mut()
            .trigger(SetTerrainOverlay { cliff_deg: Some(25.0), ..default() });
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
        app.world_mut()
            .trigger(SetTerrainOverlay { opacity: Some(0.0), ..default() });
        app.world_mut().flush();
        assert_eq!(app.world().resource::<TerrainOverlayParams>().opacity, 0.0);
    }

    /// And an explicit `enabled: false` still disarms it.
    #[test]
    fn explicit_disable_still_works() {
        let mut app = test_app();
        app.world_mut().resource_mut::<TerrainOverlayParams>().enabled = true;
        app.world_mut()
            .trigger(SetTerrainOverlay { enabled: Some(false), ..default() });
        app.world_mut().flush();
        assert!(!app.world().resource::<TerrainOverlayParams>().enabled);
    }
}
