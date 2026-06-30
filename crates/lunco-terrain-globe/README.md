# lunco-terrain-globe

Streaming **globe-scale** planetary terrain for LunCoSim: QuadSphere (cube-sphere)
quadtree-CDLOD tiling, LOD, avian heightfield collision, and big_space anchoring for
whole celestial bodies seen from orbit.

> **Crate split.** The old monolithic `lunco-terrain` has been split into
> `lunco-terrain-{core,globe,surface}`:
> - **`lunco-terrain-core`** — the projection-agnostic, render-/physics-free LOD
>   spine (CDLOD `quadtree` selection, planar `tile` math, the `HeightSource`
>   trait). Shared primitives; depends on nothing but std + serde; both terrain
>   crates build on it.
> - **`lunco-terrain-globe`** (this crate) — globe scale: cube-sphere region map
>   + radius `HeightSource` for whole bodies.
> - **`lunco-terrain-surface`** — surface scale: a DEM-backed `HeightSource` +
>   avian heightfield colliders + big_space per-tile anchoring for local ground,
>   plus surface/regolith detail.
>
> A future orbit→surface bridge is a *composite* `HeightSource` returning the
> site DEM inside a georeferenced region and the globe height outside it.

## Responsibility

This crate implements the **globe surface representation** layer:

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
lunco-terrain-globe/src/
  ├── lib.rs              # TerrainPlugin, TerrainTileConfig, TileCoord
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
use lunco_terrain_globe::TerrainPlugin;

app.add_plugins(TerrainPlugin);
```

Terrain tile configuration:

```rust
use lunco_terrain_globe::TerrainTileConfig;

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
