//! Late-bind the terrain's baked derived layers onto its **static-mesh** material.
//!
//! Two terrain render paths consume `TerrainDerivedMaps`:
//!
//! - the **streamed LOD tiles** — each tile states a `ShaderLook` and the derived
//!   maps are texture layers on it, so `lunco-terrain-surface` binds nothing and
//!   `shader_look.rs` does the work. That path is fully decoupled.
//! - the **static mesh** — its material is NOT authored by the terrain crate at
//!   all: `lunco-usd-sim`'s `apply_usd_shader_materials` mints a `ShaderMaterial`
//!   on the terrain prim asynchronously (`materialType="shader"`). There is no
//!   intent component to edit — the material *is* the authority — so filling its
//!   empty map slots means naming `MeshMaterial3d<ShaderMaterial>`.
//!
//! That is why this system lives here and not in `lunco-terrain-surface`: it is
//! material *binding*, and this crate is the only one allowed to do that. Moving it
//! is what let the terrain crate stop naming a material. It retries every frame
//! until the async USD material exists, then marks the terrain `DerivedLayersBuilt`
//! and stops scanning.

use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use crate::shader_material::ShaderMaterial;
use lunco_materials::ParamValue;
use lunco_terrain_surface::{DerivedLayersBuilt, TerrainDerivedMaps};

/// Bind the baked surface/normal layers onto the terrain's own static-mesh
/// material once it exists, then mark the terrain done.
fn apply_derived_layers(
    mut commands: Commands,
    q: Query<
        (Entity, &TerrainDerivedMaps, &MeshMaterial3d<ShaderMaterial>),
        Without<DerivedLayersBuilt>,
    >,
    materials: Option<ResMut<Assets<ShaderMaterial>>>,
) {
    let Some(mut materials) = materials else { return };
    for (entity, handles, mat3d) in &q {
        let Some(mut material) = materials.get_mut(&mat3d.0) else { continue };
        // Yield to an authored map: a USD `lunco:terrain:layer:surface/normal:map`
        // (bound elsewhere) takes precedence — only fill a slot still empty, so
        // the derived bake is the fallback, not an override.
        let mut weights: Vec<(&str, ParamValue)> = Vec::new();
        if material.surface_map.is_none() {
            material.surface_map = Some(handles.surface.clone());
            weights.push(("weight_rough", ParamValue::F32(1.0)));
            weights.push(("weight_ao", ParamValue::F32(1.0)));
        }
        if material.normal_map.is_none() {
            material.normal_map = Some(handles.normal.clone());
            weights.push(("weight_normal", ParamValue::F32(1.0)));
        }
        if !weights.is_empty() {
            material.set_many(weights);
        }
        commands.entity(entity).try_insert(DerivedLayersBuilt);
        info!("[terrain-layers] bound DEM-derived surface+normal layers ({}²)", handles.res);
    }
}

pub(crate) fn build(app: &mut App) {
    app.add_systems(Update, apply_derived_layers);
}
