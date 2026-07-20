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
  └── assembler.rs   — Generic entity composition helpers (chassis, wheels, FSW, sensors)
```

### Dependencies

This crate is a central hub for robot construction:
- `lunco-core` (Vessel markers)
- `lunco-mobility` (Raycast wheel setup)
- `lunco-hardware` (Actuator/Sensor setup)
- `lunco-controller` (Input map setup)

## Usage

```rust
// Compose a complete rover from its parts via the `assembler` module helpers
// (chassis + wheels + FSW + OBC + hardware), which link coordinate frames and
// component wiring consistently.
use lunco_robotics::assembler;
```

## See Also

- `lunco-sandbox` / `luncosim` (the app binaries) — primary consumers of these assembly helpers.
- `lunco-usd` — The counterpart for assembling vessels from USD scene definitions.
