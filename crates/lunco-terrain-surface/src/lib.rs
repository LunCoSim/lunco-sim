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
//! - [`plugin`] — `TerrainSurfacePlugin` owns DEM/query/physics composition;
//!   `TerrainSurfaceVisualizationPlugin` separately owns camera-driven LOD and
//!   derived visual products for hosts that present frames.

pub mod band;
pub mod collider_ring;
pub mod derived_layers;
pub mod georef;
pub mod oracle;
pub mod overlay;
pub mod plugin;
pub mod query;
pub mod stream_viz;
mod surface_change;
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
    ColliderTileOf, ColliderTiles, MAX_COLLIDER_DEPTH, MAX_COLLIDER_RESOLUTION, MIN_COLLIDER_DEPTH,
    MIN_COLLIDER_RESOLUTION, TerrainColliderRing, TerrainColliderSettings,
    resolve_collider_settings,
};
pub use derived_layers::{TerrainAuthoredMaps, TerrainDerivedMaps, TerrainDerivedStatus};
pub use georef::{DEFAULT_ANCHOR_BODY, FlatSiteSurface, TerrainGeoref};
/// The base raster [`SurfaceOracle`] composes over.
///
/// Re-exported because it is already part of this crate's PUBLIC surface —
/// `SurfaceOracle::new`/`bare` take `Arc<HeightGrid>` — and a caller could see the
/// constructor but had no way to name its argument without depending on
/// `lunco-obstacle-field` directly, which is an implementation detail of where the
/// type happens to live.
pub use lunco_obstacle_field::field::HeightGrid;
pub use lunco_terrain_core::{
    AnalyticHeightSource, HeightSource, QuadCoord, Quadtree, Selected, Square, TileCoord, TileGrid,
    TransferFn, hazard_color, hazard_from_slope,
};
pub use oracle::{
    DemHeightField, HeightContribution, SurfaceOracle, TerrainBodyCurvature, raycast_surface,
};
pub use plugin::{TerrainSurfacePlugin, TerrainSurfaceSet, TerrainSurfaceVisualizationPlugin};
pub use query::{TerrainHeightProvider, register_terrain_queries};
pub use stream_viz::{
    LodFrozen, LodTileOf, LodTiles, SetTerrainRenderingQuality, TerrainLodViz, TerrainNodeErrors,
    TerrainStreamFrameDriven, TerrainStreamStatus, TerrainVisualFocus, TileShadowCache,
};
pub use surface_query::report_unreachable_dem_frame;
pub use surface_query::{
    GridSurfaceQuery, SurfaceFit, SurfaceHit, SurfaceSample, TerrainPoseInPhysicsFrame,
    fit_footprint, height_in_footprint,
};
pub use terrain::{
    BrushTerrain, DemBaseGrid, DemTerrainRequest, DemTerrainSource, DemTerrainSurface,
    DocBackedTerrain, FlattenTerrain, PlaceCrater, PlaceRock, RegenerateTerrainLayers,
    RemoveTerrainLayer, SpawnDemTerrain, TERRAIN_BUILD_FAULT_KIND, TerrainGenPhase,
    TerrainGenStatus, resolve_dem_request_parameters,
};
pub use terrain_layers::{
    EDITS_LAYER_ID, EditKind, EditsLayer, LayerAttrSource, LayerEntry, LayerId, LayerScatterCx,
    TerrainLayer, TerrainLayerAppExt, TerrainLayerParams, TerrainLayerParser,
    TerrainLayerParserRegistry, TerrainLayerStack, TerrainLayersApplied, TerrainRock,
    TerrainScatterEntity, TerrainScatterOwner, edit_attr_writes, make_crater_layer, parse_edit,
    rock_instance_layer, terrain_layer_params,
};
pub use tile_mesh::{TileMesh, bake_tile_mesh};
