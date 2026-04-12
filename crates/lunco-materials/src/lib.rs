//! LunCoSim Custom Materials
//!
//! Self-contained material plugins for USD-driven rendering.
//! Each material is an independent Plugin that can be added
//! to any Bevy App without cross-crate dependencies.
//!
//! ## Architecture
//!
//! Each material is a self-contained unit:
//! - `ExtendedMaterial<StandardMaterial, Extension>` definition
//! - Shader registration via `load_internal_asset!`
//! - `MaterialPlugin<T>::default()` registration
//! - Post-sync system that reads `primvars:materialType` from USD and applies the material
//!
//! ## Adding New Materials
//!
//! 1. Define your `MyMaterialExtension` with `#[derive(AsBindGroup)]`
//! 2. Implement `MaterialExtension` for your extension
//! 3. Create `MyMaterialShaderPlugin` that registers your shader
//! 4. Create `MyMaterialPlugin` that registers everything
//! 5. Add `.add_plugins(MyMaterialPlugin)` to your binary
//!
//! No changes to `lunco-usd-bevy` needed.

mod blueprint;
mod solar_panel;

pub use blueprint::*;
pub use solar_panel::*;

use bevy::prelude::*;
use openusd::usda::TextReader;
use openusd::sdf::Path as SdfPath;

/// Reads a 3-component vector attribute from a USD prim.
///
/// Handles all common USD vector types:
/// - `color3f` → `Value::Vec3f`
/// - `double3` → `Value::Vec3d`
/// - `float3` → `Value::Vec3f`
/// - `Vec<f32>` / `Vec<f64>` array forms
///
/// Returns `None` if the attribute doesn't exist or can't be converted.
fn get_attribute_as_vec3(reader: &TextReader, path: &SdfPath, attr: &str) -> Option<Vec3> {
    if let Some(v) = reader.prim_attribute_value::<[f32; 3]>(path, attr) {
        return Some(Vec3::new(v[0], v[1], v[2]));
    }
    if let Some(v) = reader.prim_attribute_value::<[f64; 3]>(path, attr) {
        return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32));
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0], v[1], v[2])); }
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)); }
    }
    None
}
