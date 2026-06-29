//! Projection-agnostic terrain LOD spine — the shared core both terrain crates build on.
//!
//! This is the pure, render-free, physics-free heart of the terrain system:
//! - [`quadtree`] — CDLOD quadtree selection over an abstract square region:
//!   distance-range refinement from a fixed canonical screen metric (view-
//!   independent → deterministic across peers), 3D-Tiles geometric-error, and
//!   CDLOD geomorph bands. `select_3d` takes eye-height so altitude coarsens.
//! - [`tile`] — uniform planar tile-grid math: world↔tile mapping, the resident
//!   ring of tiles around a focus (the physics-collider-ring substrate).
//! - [`source`] — the [`HeightSource`] trait (`height_at` as a pure function of
//!   position) + a deterministic analytic FBM source for bring-up/tests.
//!
//! It depends on **nothing** but std + serde — no bevy, avian, big_space, DEM, or
//! sphere projection. Those live in the two terrain crates that build on this spine:
//! - `lunco-terrain-surface` — **surface** scale: a DEM-backed `HeightSource` +
//!   avian heightfield colliders + big_space per-tile anchoring for local ground.
//! - `lunco-terrain-globe` — **globe** scale: a cube-sphere region map + radius
//!   `HeightSource` for whole bodies seen from orbit.
//!
//! Keeping the LOD spine here (pure, wasm-safe, unit-tested) is what lets both
//! scales share one selection algorithm instead of duplicating it. The future
//! orbit→surface bridge is a *composite* `HeightSource` that returns the site DEM
//! inside a georeferenced region and the globe height outside it.

pub mod quadtree;
pub mod source;
pub mod tile;

pub use quadtree::{QuadCoord, Quadtree, Selected, Square};
pub use source::{AnalyticHeightSource, HeightSource};
pub use tile::{TileCoord, TileGrid};
