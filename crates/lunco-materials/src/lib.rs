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
mod shader_material;

pub use blueprint::*;
pub use shader_material::*;
