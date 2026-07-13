//! LunCoSim Custom Materials
//!
//! Bevy render materials, kept **engine-agnostic**: nothing here is USD-specific.
//!
//! ## What lives here
//! - [`ShaderMaterial`] — the *one* general custom-shader material. Any `.wgsl`,
//!   chosen per-instance; new shaders are pure asset files, no Rust. Authoring it
//!   from USD is **not** here — that binding is a deterministically-ordered system
//!   in `lunco-usd-sim` (`apply_usd_shader_materials`), so material application can
//!   never race a downstream consumer. Note the coupling: a binary that adds
//!   `ShaderMaterialPlugin` but not `UsdSimPlugin` registers the render pipeline
//!   but performs no USD authoring — a `materialType="shader"` prim would render
//!   with its plain `StandardMaterial`.
//!
//! There is no bespoke per-effect material type: the old `BlueprintMaterial`
//! (a hand-rolled `ExtendedMaterial`) is gone — its grid look is now the
//! self-describing `assets/shaders/blueprint.wgsl` applied via `ShaderMaterial`.

pub mod dyn_params;
pub mod look;
mod shader_material;

pub use dyn_params::{ParamField, ParamSchema, ParamType, ParamValue, UiKind};
pub use look::{ShaderLook, ShaderLookKey, TextureLayer};
pub use shader_material::*;
