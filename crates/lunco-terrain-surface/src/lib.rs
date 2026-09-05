//! Streamed, dynamically-LOD'd lunar terrain.
//!
//! Large-scale lunar surfaces can't live as one static mesh (a 2 km big_space
//! cell at 5 cm detail is ~1.6 billion samples). This crate streams the surface
//! as a grid of **tiles** around the viewer, each built from a **DEM /
//! heightfield source**, with dynamic level-of-detail. It is the streaming
//! counterpart to the authored USD material intent and the scatter in
//! `lunco-obstacle-field`.
//!
//! Design constraints (see `docs/architecture/terrain-layered-rendering.md` Parts F–G
//! and `docs/architecture/terrain-substrate.md`):
//! - **Tile ≤ big_space cell**; tiles anchor via `lunco_core` `CellCoord` and
//!   stream by `FloatingOrigin` position. A tile never straddles a cell.
//! - **Physics LOD is deterministic** — colliders are built at a canonical
//!   resolution independent of visual LOD, so networking still replicates only
//!   the spec and every peer agrees on contact.
//! - **Pure, deterministic core** — [`lunco_terrain_core::HeightSource`] `height_at` is a pure function of
//!   position, so derived data is content-addressable, cacheable, and
//!   re-derivable on any peer with nothing to transfer.
//! - **wasm-safe** — the core touches only std + glam; heavy work is chunked or
//!   pre-baked at the plugin layer.
//!
//! The projection-agnostic LOD spine — the quadtree-CDLOD selector, tile-grid
//! ring math, and the [`HeightSource`] trait — lives in the pure leaf crate
//! [`lunco_terrain_core`] (shared with the cube-sphere planetary tiler). This
//! crate is the **planar DEM adapter** on top of it.
//!
//! Layers:
//! - `lunco-terrain-bake::dem` — loader for real DEM assets from `lunar_terrain_exporter`
//!   (a georeferenced float32 GeoTIFF) into a reused `HeightGrid`, which then
//!   acts as a [`HeightSource`]. This replaces the analytic placeholder with
//!   real LOLA elevation. Byte-based and filesystem-free → identical on native
//!   and wasm (the host supplies bytes via `lunco-storage` / `AssetServer`).
//! - `lunco-terrain-bake::bake` — resample a [`HeightSource`] into a render/collider-sized
//!   `HeightGrid` (the bridge from a too-dense DEM to a drawable/collidable tile).
//! - [`terrain`] — M3 spawn: build a static terrain entity (mesh + avian
//!   `Collider::heightfield`) from a DEM asset via the `SpawnDemTerrain` command.
//! - [`plugin`] — the Bevy [`TerrainSurfacePlugin`]. Wires the M3 spawn path;
//!   tile streaming + LOD + the dynamic-physics collider ring land in M7.

pub mod band;
pub mod collider_ring;
pub mod derived_layers;
pub mod georef;
pub mod oracle;
pub mod overlay;
pub mod plugin;
pub mod query;
pub mod stream_viz;
pub mod surface_query;
pub mod terrain;
pub mod terrain_layers;
pub mod tile_cache;
pub mod tile_mesh;

/// The shared Nyquist filter policy for independent visual and physics surface
/// products. The products share the analytic oracle, but neither product's
/// quality or selection controls the other. See [`band`].
pub use band::SurfaceBand;
pub use collider_ring::{
    resolve_collider_settings, ColliderTileOf, ColliderTiles, TerrainColliderRing,
    TerrainColliderSettings, MAX_COLLIDER_DEPTH, MAX_COLLIDER_RESOLUTION, MIN_COLLIDER_DEPTH,
    MIN_COLLIDER_RESOLUTION,
};
pub use derived_layers::{TerrainAuthoredMaps, TerrainDerivedMaps, TerrainDerivedStatus};
pub use georef::{FlatSiteSurface, TerrainGeoref, DEFAULT_ANCHOR_BODY};
/// The base raster [`SurfaceOracle`] composes over.
///
/// Re-exported because it is already part of this crate's PUBLIC surface —
/// `SurfaceOracle::new`/`bare` take `Arc<HeightGrid>` — and a caller could see the
/// constructor but had no way to name its argument without depending on
/// `lunco-obstacle-field` directly, which is an implementation detail of where the
/// type happens to live.
pub use lunco_obstacle_field::field::HeightGrid;
pub use lunco_terrain_core::{
    hazard_color, hazard_from_slope, AnalyticHeightSource, HeightSource, QuadCoord, Quadtree,
    Selected, Square, TileCoord, TileGrid, TransferFn,
};
pub use oracle::{
    raycast_surface, DemHeightField, HeightContribution, SurfaceOracle, TerrainBodyCurvature,
};
pub use plugin::{TerrainSurfacePlugin, TerrainSurfaceSet};
pub use query::{register_terrain_queries, TerrainHeightProvider};
pub use stream_viz::{
    LodFrozen, LodTiles, SetTerrainRenderingQuality, TerrainLodViz, TerrainNodeErrors,
    TerrainStreamLockstep, TerrainStreamStatus, TerrainVisualFocus, TileShadowCache,
};
pub use surface_query::report_unreachable_dem_frame;
pub use surface_query::{
    fit_footprint, height_in_footprint, GridSurfaceQuery, SurfaceFit, SurfaceHit, SurfaceSample,
    TerrainPoseInPhysicsFrame,
};
pub use terrain::{
    resolve_dem_request_parameters, BrushTerrain, DemBaseGrid, DemTerrainRequest, DemTerrainSource,
    DemTerrainSurface, DocBackedTerrain, FlattenTerrain, PlaceCrater, PlaceRock,
    RegenerateTerrainLayers, RemoveTerrainLayer, SpawnDemTerrain, TerrainGenPhase,
    TerrainGenStatus, TERRAIN_BUILD_FAULT_KIND,
};
pub use terrain_layers::{
    edit_attr_writes, make_crater_layer, parse_edit, rock_instance_layer, terrain_layer_params,
    EditKind, EditsLayer, LayerAttrSource, LayerEntry, LayerId, LayerScatterCx, TerrainLayer,
    TerrainLayerAppExt, TerrainLayerParams, TerrainLayerParser, TerrainLayerParserRegistry,
    TerrainLayerStack, TerrainLayersApplied, TerrainRock, TerrainScatterEntity,
    TerrainScatterOwner, EDITS_LAYER_ID,
};
pub use tile_mesh::{bake_tile_mesh, TileMesh};
