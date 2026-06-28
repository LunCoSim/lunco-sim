//! Streamed, dynamically-LOD'd lunar terrain.
//!
//! Large-scale lunar surfaces can't live as one static mesh (a 2 km big_space
//! cell at 5 cm detail is ~1.6 billion samples). This crate streams the surface
//! as a grid of **tiles** around the viewer, each built from a **DEM /
//! heightfield source**, with dynamic level-of-detail. It is the streaming
//! counterpart to the procedural look in `lunco-materials` and the scatter in
//! `lunco-obstacle-field`.
//!
//! Design constraints (see `docs/terrain-layered-pipeline-design.md` Parts F–G
//! and `docs/terrain-streaming-PLAN.md`):
//! - **Tile ≤ big_space cell**; tiles anchor via `lunco_core` `CellCoord` and
//!   stream by `FloatingOrigin` position. A tile never straddles a cell.
//! - **Physics LOD is deterministic** — colliders are built at a canonical
//!   resolution independent of visual LOD, so networking still replicates only
//!   the spec and every peer agrees on contact.
//! - **Pure, deterministic core** — [`source`] `height_at` is a pure function of
//!   position, so derived data is content-addressable, cacheable, and
//!   re-derivable on any peer with nothing to transfer.
//! - **wasm-safe** — the core touches only std + glam; heavy work is chunked or
//!   pre-baked at the plugin layer.
//!
//! Layers:
//! - [`tile`] — pure tile-grid math: world↔tile mapping, tile bounds, the ring
//!   of tiles around a point. Unit-tested, no Bevy.
//! - [`source`] — the terrain height source trait + a deterministic analytic
//!   source for bring-up (no external asset). `height_at` / `normal_at`.
//! - [`dem`] — loader for real DEM assets from `lunar_terrain_exporter`
//!   (float32 GeoTIFF + `metadata.yaml`) into a reused `HeightGrid`, which then
//!   acts as a [`HeightSource`]. This replaces the analytic placeholder with
//!   real LOLA elevation. Byte-based and filesystem-free → identical on native
//!   and wasm (the host supplies bytes via `lunco-storage` / `AssetServer`).
//! - [`bake`] — resample a [`HeightSource`] into a render/collider-sized
//!   `HeightGrid` (the bridge from a too-dense DEM to a drawable/collidable tile).
//! - [`terrain`] — M3 spawn: build a static terrain entity (mesh + avian
//!   `Collider::heightfield`) from a DEM asset via the `SpawnDemTerrain` command.
//! - [`plugin`] — the Bevy [`TerrainStreamingPlugin`]. Wires the M3 spawn path;
//!   tile streaming + LOD + per-rover collider ring land in M7.

pub mod bake;
pub mod cdlod;
pub mod dem;
pub mod plugin;
pub mod quadtree;
pub mod source;
pub mod terrain;
pub mod tile;
pub mod tile_mesh;

pub use bake::resample;
pub use cdlod::{morph_factor, node_instance, unit_grid, NodeInstance};
pub use dem::{decode_geotiff_f64, height_grid_from_geotiff, DemError, DemMetadata};
pub use plugin::{TerrainStreamingConfig, TerrainStreamingPlugin};
pub use quadtree::{QuadCoord, Quadtree, Selected, Square};
pub use source::{AnalyticHeightSource, HeightSource};
pub use terrain::{DemTerrainRoot, DemTerrainSurface, SpawnDemTerrain};
pub use tile::{TileCoord, TileGrid};
pub use tile_mesh::{bake_tile_mesh, TileMesh};
