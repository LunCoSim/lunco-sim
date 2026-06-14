//! Celestial grid overlay material — the planet-tile lat/long grid.
//!
//! Moved here out of `lunco-materials` (where it was the bespoke, dual-mode
//! `BlueprintMaterial`) into the celestial crate that is its only consumer, and
//! slimmed: it draws ONLY the lat/long grid (the Cartesian "blueprint floor"
//! mode lives on as the dynamic `shaders/blueprint_grid.wgsl` for flat ground).
//!
//! It stays an `ExtendedMaterial<StandardMaterial, _>` on purpose: the planet
//! tiles are **textured** (earth/moon albedo) with the grid overlaid, so the
//! base `StandardMaterial` keeps the texture + full PBR — something the dynamic
//! `ShaderMaterial` (no albedo-texture binding) can't do. The lat/long mapping
//! comes from the quadsphere tile's UVs (a terrain/system concern); the shader
//! just draws a grid on UVs. [`crate::celestial_visuals_system`] drives
//! `grid_fade` from camera altitude (1 = full grid far out, 0 = hidden near the
//! surface).

use bevy::prelude::*;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin};
use bevy::render::render_resource::AsBindGroup;
#[cfg(target_arch = "wasm32")]
use bevy::asset::load_internal_asset;
use bevy::shader::{Shader, ShaderRef};
use std::marker::PhantomData;
use uuid::Uuid;

/// Asset path of the grid shader (native: load by path → hot-reloadable).
#[cfg(not(target_arch = "wasm32"))]
const CELESTIAL_GRID_SHADER_PATH: &str = "shaders/celestial_grid.wgsl";

/// UUID for the grid shader. **Load-bearing on wasm**: the embed-assets build has
/// no filesystem, so [`CelestialGridMaterialPlugin`] registers the source to this
/// const handle. Native loads by path instead.
const CELESTIAL_GRID_SHADER_UUID: Uuid = Uuid::from_u128(0x4d5e6f7a_8b9c_0d1e_2f3a_4b5c6d7e8f90);
pub const CELESTIAL_GRID_SHADER_HANDLE: Handle<Shader> =
    Handle::Uuid(CELESTIAL_GRID_SHADER_UUID, PhantomData);

/// Uniforms for the lat/long grid overlay (binding 100, on top of the base
/// `StandardMaterial`). Field order/layout matches `celestial_grid.wgsl`.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Copy)]
pub struct CelestialGridExtension {
    #[uniform(100)]
    pub line_color: LinearRgba,
    /// Lon/lat grid cell counts.
    #[uniform(100)]
    pub subdivisions: Vec2,
    /// Sub-pixel fade window (start, end) on the per-pixel cell size.
    #[uniform(100)]
    pub fade_range: Vec2,
    #[uniform(100)]
    pub line_width: f32,
    /// System-driven (camera altitude): 1 = full grid, 0 = hidden.
    #[uniform(100)]
    pub grid_fade: f32,
}

impl Default for CelestialGridExtension {
    fn default() -> Self {
        Self {
            line_color: LinearRgba::new(0.0, 0.5, 1.0, 1.0),
            subdivisions: Vec2::new(24.0, 12.0),
            fade_range: Vec2::new(0.2, 0.6),
            line_width: 1.5,
            grid_fade: 1.0,
        }
    }
}

impl MaterialExtension for CelestialGridExtension {
    fn fragment_shader() -> ShaderRef {
        #[cfg(not(target_arch = "wasm32"))]
        {
            CELESTIAL_GRID_SHADER_PATH.into()
        }
        #[cfg(target_arch = "wasm32")]
        {
            CELESTIAL_GRID_SHADER_HANDLE.into()
        }
    }
}

/// The celestial grid material: a `StandardMaterial` (textured planet) with the
/// lat/long grid overlay.
pub type CelestialGridMaterial = ExtendedMaterial<StandardMaterial, CelestialGridExtension>;

/// Registers the grid material's render pipeline (and, on wasm, embeds its
/// shader source to the const handle).
pub struct CelestialGridMaterialPlugin;

impl Plugin for CelestialGridMaterialPlugin {
    fn build(&self, app: &mut App) {
        // Native loads the shader from `assets/` by path (hot-reload). Only wasm
        // (no filesystem) needs the source embedded to the const handle.
        #[cfg(target_arch = "wasm32")]
        load_internal_asset!(
            app,
            CELESTIAL_GRID_SHADER_HANDLE,
            "../../../assets/shaders/celestial_grid.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(MaterialPlugin::<CelestialGridMaterial>::default());
    }
}
