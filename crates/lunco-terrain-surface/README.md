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
| `georef` | `TerrainGeoref` parsed from `lunco:anchor:*` plus `FlatSiteSurface` derived from an explicitly designated standard USD site cube |
| `terrain` | the DEM terrain surface + spawn requests (`DemTerrainSurface`, `DemTerrainRequest`, `SpawnDemTerrain`) |
| `query` | terrain-height queries (`TerrainHeightProvider`, `register_terrain_queries`) |
| `plugin` | `TerrainSurfacePlugin` (wires the full DEM → streaming → collider pipeline) |

## Usage

```rust
app.add_plugins(lunco_terrain_surface::TerrainSurfacePlugin);
```

## Status

Inert until a DEM terrain is spawned (via `SpawnDemTerrain` or a USD
`lunco:assetMode="layered"` terrain prim). Streaming visuals, the collider ring,
and the composable layer stack are all wired. The design narrative — the
height-oracle model, the three-channel layer taxonomy (height / carve / geometry),
independent visual/physics sampling, authored collider parameters, error-driven
visual detail, and orbit→surface scaling — is in
[`docs/architecture/terrain-substrate.md`](../../docs/architecture/terrain-substrate.md).
