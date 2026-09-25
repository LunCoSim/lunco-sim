# lunco-terrain-surface

**Surface-scale** terrain: DEM-backed, dynamically-LOD'd local ground with avian
heightfield colliders and big_space per-tile anchoring.

Builds on `lunco-terrain-core`'s projection-agnostic LOD spine and adds the
bevy / avian / big_space / DEM
layers the core deliberately omits. The surface geometry is paired with the
material intent authored by the owning USD `UsdShade` network. The complement
to `lunco-terrain-globe` (orbit scale).

## Modules

| Module | Role |
|--------|------|
| `lunco-terrain-bake` | The pure bevy/avian-free bake pipeline (GeoTIFF decode, crop/resample, crater stamp) is owned by [`lunco-terrain-bake`](../lunco-terrain-bake/README.md) so the wasm DEM Web Worker runs the same code. |
| `tile_mesh` | per-tile mesh baking (`bake_tile_mesh`, `TileMesh`) |
| `collider_ring` | resident avian heightfield collider ring around the focus (`TerrainColliderRing`, `TerrainColliderSettings`, `ColliderTiles`) |
| `stream_viz` | streamed LOD visuals (`DemHeightField`, `LodTiles`, `TerrainLodViz`) using the owning USD `ShaderLook` |
| `terrain_layers` | composable USD-prim layer stack (`TerrainLayerStack`, `TerrainLayer`, parser registry) — craters / rocks |
| `derived_layers` | off-thread surface/normal map bake from the DEM |
| `georef` | `TerrainGeoref` parsed from `lunco:anchor:*` plus `FlatSiteSurface` derived from an explicitly designated standard USD site plane |
| `terrain` | the DEM terrain surface + spawn requests (`DemTerrainSurface`, `DemTerrainRequest`, `SpawnDemTerrain`) |
| `query` | terrain-height queries (`TerrainHeightProvider`, `register_terrain_queries`) |
| `plugin` | `TerrainSurfacePlugin` (authoritative DEM, query, and collider pipeline); `TerrainSurfaceVisualizationPlugin` (camera-driven visual products) |

## Usage

```rust
app.add_plugins(lunco_terrain_surface::TerrainSurfacePlugin);
```

GUI compositions also install `TerrainSurfaceVisualizationPlugin` through
`lunco-luncosim-presentation`. Headless servers and scene-test hosts omit it,
so they do not schedule camera-driven LOD, visual-map baking, or terrain
overlays. Terrain height queries and authored collider rings remain available.
On visual hosts, terrain cover selection follows a 30 Hz wall-clock cadence;
the immutable selection calculation uses shared bounded background admission,
and its generation-fenced result is committed with tile residency each
`Update`. Tile-mesh bakes retain their per-terrain worker queue. Offline capture
lockstep computes selection synchronously every frame so visual output remains
tied to the captured frame.

## Status

Inert until a DEM terrain is spawned (via `SpawnDemTerrain` or a USD
`lunco:assetMode="layered"` terrain prim). The authoritative plugin wires the
collider ring and composable layer stack. Visual streaming is wired only by the
presentation plugin. The design narrative — the
height-oracle model, the three-channel layer taxonomy (height / carve / geometry),
independent visual/physics sampling, authored collider parameters, error-driven
visual detail, and orbit→surface scaling — is in
[`docs/architecture/terrain-substrate.md`](../../docs/architecture/terrain-substrate.md).
