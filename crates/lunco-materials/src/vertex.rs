//! Custom mesh vertex attributes — **render-free**.
//!
//! A [`MeshVertexAttribute`] is `bevy_mesh`, which depends only on `wgpu-types`
//! (a plain data crate); it does **not** pull `bevy_render` → wgpu/naga. So the
//! attribute lives with the mesh *author* (`lunco-terrain-surface`, which is
//! render-free and inserts it on its CDLOD tiles), not with the material that
//! consumes it.
//!
//! The consumer side — binding `@location(8)` into the vertex layout — is
//! `lunco-render-bevy`'s `ShaderMaterial::specialize`, which imports this constant
//! back. See `docs/architecture/render-decoupling.md`.

use bevy::mesh::{MeshVertexAttribute, VertexFormat};

/// Custom vertex attribute: each vertex's CDLOD **parent-lattice position** (the
/// morph target). A geomorph vertex shader lerps `POSITION → this` by camera
/// distance so a tile collapses smoothly onto its coarser parent (no LOD pop).
/// Only terrain LOD-tile meshes carry it; `ShaderMaterial::specialize`
/// (`lunco-render-bevy`) adds it to the vertex layout only when the material's
/// `vertex_shader` is set, so the ordinary fragment-only path is untouched.
/// Shader side: `@location(8)`.
pub const ATTRIBUTE_MORPH_TARGET: MeshVertexAttribute =
    MeshVertexAttribute::new("Lunco_MorphTarget", 0x4d_4f_52_50, VertexFormat::Float32x3);

/// Custom vertex attribute: the surface normal **of the parent lattice** — the
/// normal belonging to [`ATTRIBUTE_MORPH_TARGET`], not to `POSITION`.
///
/// The geomorph vertex shader lerps `POSITION → morph target`, so during a morph
/// (and for the whole of a freshly-spawned tile's reveal, which starts fully
/// morphed) the surface actually drawn is the PARENT surface. Shading it with the
/// child's fine normal makes geometry and lighting disagree: measured at up to
/// ~22 deg on lunar relief, flipping the sign of `N·L` on ~4% of quads under a
/// grazing sun — those quads shade BLACK, which is the "new LOD tiles appear dark
/// then correct themselves" artifact. Lerping normal alongside position by the
/// same factor keeps shading attached to the geometry being drawn.
/// Shader side: `@location(9)`.
pub const ATTRIBUTE_MORPH_NORMAL: MeshVertexAttribute =
    MeshVertexAttribute::new("Lunco_MorphNormal", 0x4d_4f_52_51, VertexFormat::Float32x3);
