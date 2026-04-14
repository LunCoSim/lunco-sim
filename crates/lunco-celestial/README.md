# lunco-celestial

Solar system simulation and celestial mechanics.

## Responsibility

This crate implements the **orbital mechanics** layer of the LunCoSim digital twin:

- **Ephemeris**: High-precision planetary positioning and rotation data over time
- **Gravity**: Per-entity surface gravity using body-fixed coordinates
- **SOI (Sphere of Influence)**: Automatic coordinate frame transitions between bodies
- **Missions**: Spacecraft spawning, visibility, and alignment
- **Trajectories**: Rendering of orbital paths

**What it does NOT contain:**
- Terrain generation or mesh rendering (see [`lunco-terrain`](../lunco-terrain/))
- Camera/avatar control (see [`lunco-avatar`](../lunco-avatar/))
- Physics for surface vehicles (see [`lunco-mobility`](../lunco-mobility/))

## Architecture

`CelestialPlugin` is a **Layer 2 domain plugin** — it can run headless without any rendering resources.

```
lunco-celestial/
  ├── clock.rs          # Time resource wrapper
  ├── ephemeris.rs      # Body position calculations
  ├── gravity.rs        # Per-entity gravity systems
  ├── soi.rs            # Sphere of influence transitions
  ├── systems.rs        # Body rotation, tile sync
  ├── coords.rs         # Coordinate frame helpers
  ├── missions.rs       # Spacecraft spawning & visibility
  ├── trajectories.rs   # Orbital path rendering
  └── ui/               # Time panel, body browser
```

## Dependencies

| Dependency | Why |
|---|---|
| `lunco-core` | `CelestialClock`, `TimeWarpState`, `Command` macros |
| `lunco-terrain` | Terrain tile re-exports for backward compatibility |
| `lunco-controller` | Avatar input map (used by camera spawning) |

## Multiplayer

**Server (headless):** Runs ephemeris, gravity, SOI — all shared truth.
**Client (rendering):** Same systems + terrain generation + UI panels.

The celestial clock and body positions are **authoritative** — all clients receive the same ephemeris data from the server.

## Usage

```rust
use lunco_celestial::CelestialPlugin;

app.add_plugins(CelestialPlugin);
```

For gravity-only usage (sandbox tests):

```rust
use lunco_celestial::GravityPlugin;

app.add_plugins(GravityPlugin);
```
