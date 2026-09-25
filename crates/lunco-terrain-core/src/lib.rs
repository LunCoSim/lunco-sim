//! Projection-agnostic terrain LOD spine — the shared core both terrain crates build on.
//!
//! This is the pure, render-free, physics-free heart of the terrain system:
//! - [`quadtree`] — CDLOD quadtree selection over an abstract square region:
//!   distance-range refinement from a fixed canonical screen metric (view-
//!   independent → deterministic across peers), 3D-Tiles geometric-error, and
//!   CDLOD geomorph bands. [`Quadtree::select_with_error`] is the one-shot
//!   selection over MEASURED per-node error (the reference form; the live
//!   streamer evolves a persistent cover incrementally against the same metric);
//!   `select`/`select_3d` are the uniform `root / 2^depth` schedule, kept for
//!   tests and as the spine's simple form.
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

pub mod carve;
pub mod collider;
pub mod crater;
pub mod derive;
pub mod error;
pub mod field;
pub mod modifier;
pub mod overzoom;
pub mod quadtree;
pub mod quantize;
pub mod source;
pub mod tile;
pub mod transfer;

pub use carve::{CarveField, CarvePrimitive};
pub use collider::{prepare_collider_heights, slope_limit_grid};
pub use crater::{
    ANALYTIC_RADIUS_FLOOR_M, CRATER_REACH, Crater, CraterField, Craters, MAX_CRATERS_PER_HA,
    crater_profile, crater_profile_rim_limited,
};
pub use error::measure_node_error;
// `FieldKind` is NOT re-exported: it has no definition (optimization removed it as dead
// code) and nothing referenced it but this line.
pub use derive::{
    albedo_map, ao_map, hazard_from_slope, los_hit, normal_map, normal_slope_maps,
    pack_normal_rgba8, pack_surface_rgba8, roughness_from_slope, slope_map, upsample_bilinear,
};
pub use field::{AspectField, ElevationField, SlopeField, SurfaceField, field_map};
pub use modifier::{
    BodyCurvature, BrushModifier, FlattenModifier, HeightModifier, LayeredHeightSource,
};
pub use overzoom::Overzoom;
pub use quadtree::{QuadCoord, Quadtree, Selected, Square};
pub use quantize::{QuantizedHeightSource, quantize};
pub use source::{
    AnalyticHeightSource, BoundedHeightSource, CompositeHeightSource, HeightSource,
    normal_at_bounded, square_boundary_height_at, square_boundary_posting_spacing,
    square_boundary_sample_coordinate,
};
pub use tile::{TileCoord, TileGrid};
pub use transfer::{HAZARD_CLIFF, HAZARD_SAFE, HAZARD_WARN, Rgba, TransferFn, hazard_color};
