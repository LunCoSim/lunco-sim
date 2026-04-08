# LunCoSim USD System

## Overview

The USD subsystem loads rover definitions from USD (Universal Scene Description) files and
maps them to Bevy entities with Avian3D physics and LunCoSim simulation components. It mirrors
the behavior of the procedural `rover_sandbox` binary while keeping all visual, physics, and
electrical parameters in declarative `.usda` files.

## Architecture

### Pipeline

```
┌─────────────┐    ┌──────────────────┐    ┌──────────────────┐
│  .usda file │───>│  UsdBevyPlugin   │───>│  UsdAvianPlugin  │
│  (rover)    │    │  (visual sync)   │    │  (physics map)   │
└─────────────┘    └──────────────────┘    └────────┬─────────┘
                                                    │
                     ┌──────────────────┐           │
                     │  UsdSimPlugin    │<──────────┘
                     │  (FSW, wheels,   │
                     │   steering)      │
                     └──────────────────┘
```

### Three Plugin Phases

1. **UsdBevyPlugin** — Spawns child entities for USD prims and attaches meshes + transforms.
2. **UsdAvianPlugin** — Maps USD physics attributes (`physics:rigidBodyEnabled`,
   `physics:mass`, `physics:collisionEnabled`) to Avian3D components.
3. **UsdSimPlugin** — Detects simulation schemas (`PhysxVehicleContextAPI`,
   `PhysxVehicleWheelAPI`, `PhysxVehicleDriveSkidAPI`) and creates `WheelRaycast`,
   `FlightSoftware`, `DifferentialDrive`, etc.

All three plugins use a **deferred processing system** (`process_*_prims`) that runs in the
`Update` schedule **after** `sync_usd_visuals`, ensuring the USD asset is fully loaded before
any component mapping occurs. This avoids the async loading race condition where the observer
fires before the asset is available.

### Entity Layout (Raycast Rover)

The USD rover matches the procedural `spawn_raycast_rover` entity structure exactly:

```
Rover (root entity)
├── Transform: position set by Rust, rotation = identity
├── Components: Vessel, RoverVessel, RigidBody, Collider, Mass,
│               LinearDamping, AngularDamping, Mesh3d,
│               DifferentialDrive (or AckermannSteer), FlightSoftware
│
├── Wheel_FL (child entity — physics)
│   ├── Transform: relative position, rotation = IDENTITY (for raycasting)
│   ├── Components: WheelRaycast, RayCaster (Dir3::NEG_Y), RayHits
│   └── Wheel_FL_visual (grandchild entity — rendering)
│       ├── Transform: (0,0,0), rotation = 90° Z (wheel orientation)
│       └── Components: Mesh3d, MeshMaterial3d, CellCoord
│
├── Wheel_FR ... (same structure)
├── Wheel_RL ... (same structure)
└── Wheel_RR ... (same structure)
```

### Wheel Entity Splitting

The USD file defines each wheel as a **single entity** with both a mesh and a rotation.
However, the raycast wheel needs:
- A **physics entity** with **identity rotation** so `RayCaster::new(Dir3::NEG_Y)` casts
  straight down (local space). If rotated, rays go sideways → no suspension.
- A **visual entity** with **90° Z rotation** so the cylinder renders as a rolling wheel
  (not a flat pancake).

The `process_usd_sim_prims` system splits the USD wheel entity into these two parts at
runtime, matching the procedural code's two-entity-per-wheel pattern.

### Raycast Exclusion Filter

Wheel raycasters use `SpatialQueryFilter::from_excluded_entities([rover_entity])` so wheels
don't hit their own chassis. Without this filter, downward rays immediately collide with the
chassis collider, pushing the rover into the sky (jiggling/jumping bug).

### Coordinate Systems

| System | Up Axis | Forward Axis | Notes |
|--------|---------|--------------|-------|
| USD    | Y       | +Z           | Standard USD convention |
| Bevy   | Y       | -Z           | Right-handed, Z-backward |
| Avian3D| Y       | -Z           | Matches Bevy |

Wheel rotation `rotateXYZ = (0, 0, 90)` in USD rotates the cylinder from Y-aligned
(default) to X-aligned (rolling orientation). The visual child inherits this rotation.

### Reference Resolution

USD references (e.g., `@/components/mobility/wheel.usda@`) are resolved relative to the
**USD asset root** (`assets/`). The `UsdComposer::flatten()` function walks the directory
tree from the loaded file's parent up to find the `assets/` directory, then resolves
`/`-prefixed paths against it.

```
assets/
├── vessels/rovers/
│   └── sandbox_rover_1.usda  ← loaded here
└── components/mobility/
    └── wheel.usda            ← referenced as @/components/mobility/wheel.usda@
```

The reference `/components/mobility/wheel.usda` resolves to:
`assets/` + `components/mobility/wheel.usda` ✓

## File Structure

```
assets/
├── scenes/sandbox/sandbox_scene.usda   # Scene: ground + ramp (rovers spawned by Rust)
├── vessels/rovers/
│   ├── sandbox_rover_1.usda            # USD rover — skid steering (red)
│   ├── sandbox_rover_ackermann.usda    # USD rover — Ackermann steering (yellow)
│   └── ... (additional rover variants)
└── components/mobility/
    └── wheel.usda                       # Reusable wheel component definition

crates/
├── lunco-usd/                           # Re-export crate
├── lunco-usd-bevy/                      # Visual sync: meshes, transforms, children
├── lunco-usd-avian/                     # Physics mapping: RigidBody, Collider, Mass
├── lunco-usd-sim/                       # Simulation: wheels, FSW, steering, wiring
└── lunco-usd-composer/                  # USD composition: reference resolution

crates/lunco-client/src/bin/
├── rover_sandbox.rs                     # Procedural rovers (reference implementation)
└── rover_sandbox_usd.rs                 # USD rovers + procedural rovers (this system)
```

## Adding a New Rover Variant

1. Copy an existing rover file: `cp sandbox_rover_1.usda my_rover.usda`
2. Edit `apiSchemas` to change steering type:
   - Skid: `["PhysxVehicleContextAPI", "PhysxVehicleDriveSkidAPI"]`
   - Ackermann: `["PhysxVehicleContextAPI", "PhysxVehicleDrive4WAPI"]`
3. Adjust position/color in `rover_sandbox_usd.rs` `spawn_sandbox()`
4. Load via `asset_server.load("vessels/rovers/my_rover.usda")`

## Testing

All tests load **real USD files** through the same pipeline as runtime:

```bash
cargo test --package lunco-usd
```

Key test files:
- `dump_usd_rover.rs` — dumps complete entity/component state for debugging
- `integration_asset_loading.rs` — verifies full pipeline (composition → Bevy → Avian → Sim)
- `rover_structure.rs` — verifies each wheel has identity rotation + visual child with 90° Z

## Comparison with Procedural Code

| Feature | Procedural (`rover_sandbox`) | USD (`rover_sandbox_usd`) |
|---------|------------------------------|---------------------------|
| Chassis | `Collider::cuboid(2.0, 0.3, 3.5)` | USD `Cube` with width/height/depth |
| Wheels | Spawned by Rust code | Defined in `.usda`, composed at load |
| Physics | Added by `spawn_raycast_rover` | Mapped by `UsdAvianPlugin` |
| FSW/Ports | Created by `spawn_raycast_rover` | Detected by `UsdSimPlugin` |
| Steering | `DifferentialDrive`/`AckermannSteer` in Rust | Detected from USD `apiSchemas` |
| Visual mesh | `Cylinder::new(0.4, 0.3)` + rotation | USD `Cylinder` + `rotateXYZ` |

Both produce **identical entity structures** — the USD path simply moves parameter definition
from Rust code into declarative `.usda` files.
