# lunco-terrain

Terrain generation, QuadSphere tiling, and collision for LunCoSim.

## Responsibility

This crate implements the **surface representation** layer:

- **QuadSphere Math**: Cube-to-sphere projection and LOD subdivision
- **Terrain Tiles**: Procedural mesh generation with height sampling
- **Collision**: Avian3D integration for physics interaction
- **Registry**: Custom terrain map definitions (heightmaps, caves, features)

**What it does NOT contain:**
- Orbital mechanics or body positioning (see [`lunco-celestial`](../lunco-celestial/))
- Avatar camera control (see [`lunco-avatar`](../lunco-avatar/))
- Physics solvers for vehicles (see [`lunco-mobility`](../lunco-mobility/))

## Architecture

Terrain is split into two layers within the same crate:

| Layer | What | Loaded On |
|---|---|---|
| **Layer 2 (Domain)** | Tile definitions, collision shapes, QuadSphere math | Server + Client |
| **Layer 3 (Visual)** | Mesh generation, rendering (feature-gated) | Client only |

```
lunco-terrain/
  ├── lib.rs              # TerrainPlugin, config resources
  ├── quad_sphere.rs      # Cube→sphere projection, LOD subdivision
  ├── tile.rs             # Mesh generation, height sampling
  └── registry.rs         # Custom terrain map definitions
```

## QuadSphere Design

The terrain uses a **Cube-Sphere projection** with recursive subdivision:

```
         ┌─────────────────┐
         │  6 Face QuadTree │
         │  (LOD 0..N)      │
         └────────┬────────┘
                  │
         ┌────────▼────────┐
         │  QuadSphere Node │
         │  ┌─────────────┐ │
         │  │ TileCoord   │ │
         │  │ collision   │ │
         │  │ features[]  │ │
         │  └─────────────┘ │
         └─────────────────┘
```

Each node becomes a `TerrainTile` entity with:
- **`TileCoord`**: `(body, face, level, i, j)` — deterministic identifier
- **Collision shape**: Avian heightmap or collider (shared server/client)
- **Mesh**: Procedural generation (client-only, LOD-dependent)

## Multiplayer Reality

```
Server (headless):
  ├── Loads terrain definitions
  ├── Registers collision shapes with Avian
  ├── Runs physics (rover hits cave ceiling)
  └── Syncs body positions to clients

Client (rendering):
  ├── Receives terrain definitions from server
  ├── Generates meshes around camera
  ├── Shows lava cave interior when avatar enters
  └── Same collision truth as server
```

**Key insight:** Terrain is deterministic. Two clients at the same coordinates generate identical meshes. **No need to sync terrain.** Each client generates tiles around its own camera.

**LOD is purely a rendering optimization.** The collision geometry is the same for everyone. Two players at different locations get different LODs but identical collision truth.

## Dependencies

| Dependency | Why |
|---|---|
| `lunco-core` | `Command` macro, basic types |
| `avian3d` | Collision shapes for terrain tiles |
| `big_space` | Grid cell coordinates for tile placement |

## Usage

```rust
use lunco_terrain::TerrainPlugin;

app.add_plugins(TerrainPlugin);
```

Terrain tile configuration:

```rust
use lunco_terrain::TerrainTileConfig;

app.insert_resource(TerrainTileConfig {
    tile_size_m: 500.0,
    tile_resolution: 32,
    max_lod: 12,
    ..default()
});
```

## Future: Visual Feature Gate

When the visual layer is implemented, the plugin will support:

```rust
// Server: collision only
app.add_plugins(TerrainPlugin::collision_only());

// Client: collision + rendering
app.add_plugins(TerrainPlugin::with_visualization());
```
