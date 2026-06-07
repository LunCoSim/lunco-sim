# lunco-mobility

Surface mobility and traction physics for LunCoSim planetary rovers.

## What This Crate Does

This crate implements high-performance physics models for surface vehicles, focusing on stability and realistic ground interaction.

- **Raycast-Based Wheel Model** — Uses emulated suspension rays instead of complex mesh-to-mesh collision for high performance on irregular terrain.
- **Suspension Physics** — Spring-damper system (Hooke's Law) for realistic vehicle dynamics and oscillation suppression.
- **Traction & Friction** — Coulomb friction model for longitudinal drive and lateral skid/slip behaviors.
- **Steering Mixing** — Support for Differential (Skid) drive and Ackermann steering architectures.
- **Joint-Based Suspension** — Prismatic joint support for vehicles with physical collision wheels.

## Architecture

Mobility logic runs in the `FixedUpdate` schedule, chain-linking suspension and traction systems.

```
lunco-mobility/
  ├── WheelRaycast       — The core high-performance wheel component
  ├── Suspension         — Spring-damper configuration for joints
  ├── DifferentialDrive  — Control mixing for skid-steer rovers
  ├── AckermannSteer     — Control mixing for articulated steering
  └── systems.rs         — Ray-world intersection and force application logic
```

### The Raycast Advantage

By using a single ray per wheel:
1. We eliminate wheel "snagging" on terrain geometry.
2. We ensure numeric stability during high-speed travel.
3. We provide a clean interface for visual wheel mesh positioning.

## Usage

```rust
app.add_plugins(LunCoMobilityPlugin);

// Spawning a raycast wheel
commands.spawn((
    WheelRaycast {
        rest_length: 0.5,
        spring_k: 10000.0,
        ..default()
    },
    RayCaster::default(), // From avian3d
));
```

## Wheel-physics oracle (Modelica reference)

The numerically-sensitive force laws (`suspension_force_mag`, `contact_friction`,
`drive_force_mag`) are validated against a **continuous, proper-solver reference**
— Step 2 of [`docs/architecture/28-modelica-realtime-physics.md`](../../docs/architecture/28-modelica-realtime-physics.md).

- **Declarative physics:** [`assets/models/QuarterCar.mo`](../../assets/models/QuarterCar.mo)
  states the ideal suspension dynamics (one sprung mass on a spring-damper strut,
  `m·χ̈ = m·g − (k·χ + c·χ̇)`) with the rover's real parameters (`k=8000`, `c=2800`,
  `m=250`). An adaptive Modelica solver integrates it as ground truth.
- **In-repo oracle:** `#[cfg(test)] mod oracle` integrates the *same* equations with
  fine-step RK4 (≈ the Modelica answer to many digits, this system being non-stiff)
  and compares against the production law stepped semi-implicitly at `dt = 1/60`.
  Run: `cargo test -p lunco-mobility --lib oracle -- --nocapture`.

It establishes three things:

1. **Gentle regime** — the Rust law tracks the continuous reference to sub-cm
   (≈3 mm) and settles at `χ_eq = m·g/k`. Physics + integration validated.
2. **No limit-cycle** — an under-damped config still decays (late ringing < 15 % of
   early). The dead-band / `.max(0)` bugs would ring forever; this is the guard.
3. **The bound is the fix** — on a hard landing the production law caps the force at
   `2·k·χ` (3.2 kN) while the old `.max(0)` cliff passes the full `c·v` impact spike
   (36 kN, the 27 kN-class transient the jitter work removed). The Rust law is a
   *stabilised approximation*: it agrees with the continuous physics in the gentle
   regime and intentionally caps stiff transients to stay stable at a fixed step —
   exactly the gap the oracle is meant to measure.

**Live Modelica cross-check (optional):** run `QuarterCar.mo` through lunica (open
in the workbench, Compile, FastRun) and compare its `f_susp` / `chi` trace against
the RK4 reference — they integrate identical equations and should match. Friction
and drive oracles (`contact_friction` vs a Coulomb-cone reference; `drive_force_mag`)
are the next scenarios to add to the module.

## See Also

- `lunco-controller` — Translates user input into the `DriveRover` events consumed here.
- `lunco-hardware` — Provides the physical actuators (motors, brakes) that mobility systems interface with.
- `avian3d` — The underlying physics engine for force integration.
- `docs/architecture/28-modelica-realtime-physics.md` — the realtime/multiplayer physics plan this oracle is Step 2 of.
