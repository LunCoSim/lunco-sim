# lunco-celestial

Headless celestial mechanics and semantic coordinate-frame services.

## Responsibility

This crate implements the reusable, representation-independent celestial model:

- **Ephemeris**: High-precision planetary positioning and rotation data over time
- **Frame transforms**: Typed f64 analytical conversion between semantic frames
- **Geodesy**: Body-fixed positions and local tangent frames
- **Kepler propagation**: Analytical orbit state

**What it does NOT contain:**
- BigSpace, ECS scene hierarchy, terrain, rendering, UI, or runtime asset
  loading (see [`lunco-celestial-spatial-core`](../lunco-celestial-spatial-core/)
  for shared frame contracts and [`lunco-celestial-spatial`](../lunco-celestial-spatial/)
  for runtime projection)
- A high-fidelity ephemeris implementation (see
  [`lunco-celestial-ephemeris`](../lunco-celestial-ephemeris/))

## Architecture

The package stops at f64 analytical values and semantic frame identity. A
runtime adapter chooses how to store or render those values.

```
lunco-celestial/src/
  ├── ephemeris.rs  # provider contract and resource
  ├── coords.rs     # coordinate conversions
  ├── frames.rs     # typed frame values
  ├── geo.rs        # geodesy and local tangent frames
  ├── iau.rs        # body rotation
  ├── kepler.rs     # analytical orbit propagation
  ├── registry.rs   # body catalog and semantic frame identity
  └── transform.rs  # f64 frame conversion
```

## Dependencies

| Dependency | Why |
|---|---|
| `lunco-core` | Shared body descriptors and engine primitives |
| `lunco-time` | Unified simulation epoch and time types |

## Multiplayer

**Server (headless):** Uses this package for semantic ephemeris and frame state.
**Client (rendering):** Adds `lunco-celestial-spatial` and its scene systems;
consumers that only need the shared frame contracts add
`lunco-celestial-spatial-core`.

Time (via `lunco_time::WorldTime` / `TimeTransport`) and body positions are **authoritative** — all clients receive the same ephemeris data from the server.

## Usage

```rust
use lunco_celestial::CelestialBodyRegistry;
```

For a Bevy scene with BigSpace projection, use
`lunco_celestial_spatial::CelestialPlugin`.
