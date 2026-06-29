# lunco-robotics

High-level assembly and spawning helpers for LunCoSim robots and rovers.

## What This Crate Does

This crate provides factory-style logic for assembling complex vessels from their constituent parts (chassis, wheels, software, sensors).

- **Rover Assembly** — Automated spawning of rovers with consistent coordinate frames and component linkages.
- **Entity Composition** — Coordinates the spawning of nested entities (Flight Software, OBC, Mobility, Hardware).
- **Template-Based Spawning** — Provides standard configurations for different vehicle types (e.g., standard rovers, landers).

## Architecture

`lunco-robotics` is a **Composition Layer** that orchestrates multiple domain crates.

```
lunco-robotics/
  ├── rover.rs       — Factory for standard rover vessels
  └── assembler.rs   — Generic entity composition helpers
```

### Dependencies

This crate is a central hub for robot construction:
- `lunco-core` (Vessel markers)
- `lunco-fsw` (Flight Software setup)
- `lunco-mobility` (Raycast wheel setup)
- `lunco-hardware` (Actuator/Sensor setup)
- `lunco-controller` (Input map setup)

## Usage

```rust
app.add_plugins(LunCoRoboticsPlugin);

// Use a helper to spawn a complete rover
let rover_id = lunco_robotics::rover::spawn_standard_rover(&mut commands, ...);
```

## See Also

- `luncosim` (the app binary) — The primary consumer of these assembly helpers.
- `lunco-usd` — The counterpart for assembling vessels from USD scene definitions.
