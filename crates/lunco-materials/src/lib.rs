//! LunCoSim shader appearance — **render-free**.
//!
//! This crate is the *intent* half of the custom-shader boundary. It names no
//! material, no render pipeline, and no `bevy_pbr` type, so a domain crate can
//! depend on it without linking `bevy_render` → wgpu/naga. The concrete
//! `ShaderMaterial` (the `AsBindGroup`) it describes lives in `lunco-render-bevy`,
//! the one crate allowed to name `bevy_pbr`.
//!
//! ## What lives here
//! - [`dyn_params`] — the **WGSL-reflected parameter schema**. Each `.wgsl`
//!   declares its own `struct Material` (real field names) plus `//!@` annotation
//!   comments (UI ranges, defaults, engine-filled fields); [`ParamSchema`] parses
//!   that source into name → std140 offset, and [`ParamValue`]s pack into the
//!   opaque 256-byte uniform block at those offsets. **No parameter names, ranges
//!   or defaults are hardcoded in Rust** — adding a parameter is editing a shader.
//! - [`look`] — [`ShaderLook`], the appearance **intent**: a shader *path*, an open
//!   `BTreeMap` of named params, and named [`TextureLayer`]s. Insert it next to
//!   `Mesh3d`; `lunco-render-bevy` binds it.
//! - [`vertex`] — [`ATTRIBUTE_MORPH_TARGET`], the CDLOD geomorph vertex attribute.
//!   A `MeshVertexAttribute` is `bevy_mesh`, hence render-free, so it lives with
//!   its *author* (`lunco-terrain-surface`) rather than with the material that
//!   consumes it.
//! - [`catalog`] — the pickable-[`ShaderCatalog`] and the WGSL starting templates
//!   (`CreateShader`). Plain strings + schema reflection.
//! - [`naming`] — [`to_snake_case`], the camelCase-USD → snake_case-WGSL bridge
//!   both authoring paths share.
//!
//! See `docs/architecture/render-decoupling.md`.

pub mod catalog;
pub mod dyn_params;
pub mod look;
pub mod naming;
pub mod vertex;

pub use catalog::{
    is_prop_pickable_source, shader_template, shader_template_kinds, ShaderCatalog, ShaderEntry,
};
pub use dyn_params::{ParamField, ParamSchema, ParamType, ParamValue, UiKind};
pub use look::{ShaderLook, ShaderLookKey, TextureLayer};
pub use naming::to_snake_case;
pub use vertex::ATTRIBUTE_MORPH_TARGET;
