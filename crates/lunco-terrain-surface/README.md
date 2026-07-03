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
| `stream_viz` | streamed LOD visuals (`DemHeightField`, `LodTiles`, `TerrainLodViz`, `TerrainShaderMode`) |
| `terrain_layers` | composable USD-prim layer stack (`TerrainLayerStack`, `TerrainLayer`, parser registry) — craters / rocks / shader |
| `derived_layers` | off-thread surface/normal map bake from the DEM |
| `georef` | `TerrainGeoref` parsed from `lunco:anchor:*` (lat/lon/height, metersPerUnit) |
| `terrain` | the DEM terrain surface + spawn requests (`DemTerrainSurface`, `DemTerrainRequest`, `SpawnDemTerrain`) |
| `query` | terrain-height queries (`TerrainHeightProvider`, `register_terrain_queries`) |
| `plugin` | `TerrainSurfacePlugin` + `TerrainSurfaceConfig` |

## Usage

```rust
app.add_plugins(lunco_terrain_surface::TerrainSurfacePlugin);
```

## Status

Inert until a DEM terrain is spawned (via `SpawnDemTerrain` or a USD
`lunco:assetMode="layered"` terrain prim). Streaming visuals, the collider ring,
and the composable layer stack are all wired. The design narrative — the
height-oracle model, the three-channel layer taxonomy (height / carve / geometry),
error-driven detail, and orbit→surface scaling — is in
[`docs/architecture/terrain-substrate.md`](../../docs/architecture/terrain-substrate.md).
