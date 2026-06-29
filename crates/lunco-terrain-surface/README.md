# lunco-terrain-surface

**Surface-scale** terrain: DEM-backed, dynamically-LOD'd local ground with avian
heightfield colliders and big_space per-tile anchoring.

Builds on `lunco-terrain-core`'s projection-agnostic LOD spine (re-exported here
as `quadtree` / `source` / `tile`) and adds the bevy / avian / big_space / DEM
layers the core deliberately omits. The complement to `lunco-terrain-globe`
(orbit scale).

## Modules

| Module | Role |
|--------|------|
| `dem` | DEM ingest — GeoTIFF decode (`decode_geotiff_f64`, `height_grid_from_geotiff`, `DemMetadata`, `DemError`) → a `HeightSource` |
| `bake` | resampling (`resample`) |
| `tile_mesh` | per-tile mesh baking (`bake_tile_mesh`, `TileMesh`) |
| `collider_ring` | resident avian heightfield collider ring around the focus (`TerrainColliderRing`, `ColliderTiles`) |
| `stream_viz` | streamed LOD visuals (`DemHeightField`, `LodTiles`, `TerrainLodViz`) |
| `terrain` | the DEM terrain surface + spawn requests (`DemTerrainSurface`, `DemTerrainRequest`, `SpawnDemTerrain`) |
| `query` | terrain-height queries (`TerrainHeightProvider`, `register_terrain_queries`) |
| `plugin` | `TerrainSurfacePlugin` + `TerrainSurfaceConfig` |

## Usage

```rust
app.add_plugins(lunco_terrain_surface::TerrainSurfacePlugin);
```

## Status

Inert at M0 (config only) until a DEM terrain is spawned; see
`docs/terrain-streaming-PLAN.md` for the streaming roadmap.
