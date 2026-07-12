//! Terrain **analysis overlay** — the render VIEW of a [`SurfaceField`], the
//! in-material shading plane of `Data → Transfer → Blend`.
//!
//! A [`SurfaceField`](lunco_terrain_core::SurfaceField) is data (headless, queried by
//! [`TerrainField`](crate::query)); this module is ONE consumer of it — the on-screen
//! colourised overlay painted over the lit regolith on the streamed LOD tiles. The
//! slope-hazard transfer (green ≤ safe angle → red ≥ cliff angle) is evaluated **in
//! the tile shader** from the geometric normal, running the SAME smoothstep + ramp as
//! [`lunco_terrain_core::transfer`], so the pixel colour matches a legend swatch or a
//! headless export exactly.
//!
//! Everything is **uniform-driven**: [`TerrainOverlayParams`] flows into the tile
//! materials as a handful of floats ([`OverlayUniforms`]), so re-tuning the critical
//! angle is a uniform write — no re-bake, no pipeline permutation. New tiles pick up
//! the current params at build ([`OverlayUniforms::apply`] in `build_tile_material`);
//! a live edit to the params is pushed onto the already-resident materials by
//! [`sync_terrain_overlay`]. See `docs/architecture/terrain-layered-rendering.md`.

use bevy::prelude::*;
use lunco_core::{on_command, register_commands, Command};
use lunco_materials::{ParamValue, ShaderMaterial};

use crate::stream_viz::LodMaterials;

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

    /// Write the four params onto a tile material by name (repacks once).
    pub fn apply(&self, m: &mut ShaderMaterial) {
        m.set_many([
            ("overlay_mode", ParamValue::F32(self.mode)),
            ("overlay_opacity", ParamValue::F32(self.opacity)),
            ("overlay_safe_rad", ParamValue::F32(self.safe_rad)),
            ("overlay_cliff_rad", ParamValue::F32(self.cliff_rad)),
        ]);
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
/// A numeric field left at its default `0` is treated as "keep the current value", so
/// `{ "enabled": true }` arms the overlay with the existing angles/opacity rather than
/// snapping everything to red. Pass positive `safe_deg` / `cliff_deg` / `opacity` to
/// set them.
#[Command(default)]
pub struct SetTerrainOverlay {
    pub enabled: bool,
    pub safe_deg: f32,
    pub cliff_deg: f32,
    pub opacity: f32,
}

#[on_command(SetTerrainOverlay)]
fn on_set_terrain_overlay(
    trigger: On<SetTerrainOverlay>,
    mut params: ResMut<TerrainOverlayParams>,
) {
    let ev = trigger.event();
    params.enabled = ev.enabled;
    if ev.safe_deg > 0.0 {
        params.safe_deg = ev.safe_deg;
    }
    if ev.cliff_deg > 0.0 {
        params.cliff_deg = ev.cliff_deg;
    }
    if ev.opacity > 0.0 {
        params.opacity = ev.opacity.clamp(0.0, 1.0);
    }
    info!(
        "[terrain-overlay] enabled={} safe={}° cliff={}° opacity={}",
        params.enabled, params.safe_deg, params.cliff_deg, params.opacity
    );
}

register_commands!(on_set_terrain_overlay);

/// Push the current overlay params onto every resident tile material when they
/// change — the live-tuning path (existing cached materials; freshly-built tiles
/// already read the current params at build). Change-driven: no-op on unchanged
/// frames, so a still overlay costs nothing.
pub fn sync_terrain_overlay(
    params: Res<TerrainOverlayParams>,
    lod_mats: Res<LodMaterials>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
) {
    if !params.is_changed() {
        return;
    }
    let u = params.uniforms();
    for h in lod_mats.values() {
        if let Some(mut m) = materials.get_mut(h) {
            u.apply(&mut m);
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
