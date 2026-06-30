# lunco-mobility

Surface mobility and traction physics for LunCoSim planetary rovers.

## What This Crate Does

This crate implements high-performance physics models for surface vehicles, focusing on stability and realistic ground interaction.

- **Raycast-Based Wheel Model** — Uses emulated suspension rays instead of complex mesh-to-mesh collision for high performance on irregular terrain. Traction is decomposed in the **actual contact plane** (the ray-hit normal), so leaning single-track vehicles (bikes/motorcycles) get correct lateral grip; the steer axis is configurable (`lunco:steerAxis`) for raked motorcycle forks.
- **Suspension Physics** — Spring-damper system (Hooke's Law) for realistic vehicle dynamics and oscillation suppression.
- **Traction & Friction** — Coulomb friction model for longitudinal drive and lateral skid/slip behaviors.
- **Steering Mixing** — Differential (Skid), Ackermann, and **`GenericDriveMix`** (a USD-authored linear per-port mix for arbitrary motor topologies, incl. true per-wheel independent drive).
- **Differential Coupling** — `DifferentialCoupling`, a soft holonomic constraint averaging two rockers' pitch for rocker-bogie suspension (Avian has no gear joint).
- **Joint-Based Suspension** — Prismatic joint support for vehicles with physical collision wheels.

These are **parameterized primitives** — a vehicle is a USD file, not a Rust struct. See [`docs/architecture/33-spacecraft-modeling.md`](../../docs/architecture/33-spacecraft-modeling.md).

## Architecture

Mobility logic runs in the `FixedUpdate` schedule, chain-linking suspension and traction systems.

```
lunco-mobility/
  ├── WheelRaycast         — The core high-performance wheel component (contact-plane traction)
  ├── Suspension           — Spring-damper configuration for joints
  ├── DifferentialDrive    — Control mixing for skid-steer rovers
  ├── AckermannSteer       — Control mixing for articulated steering
  ├── GenericDriveMix      — USD-authored linear per-port mix (arbitrary motor topology)
  ├── DifferentialCoupling — Soft rocker-bogie differential constraint
  └── systems.rs           — Ray-world intersection and force application logic
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

- **Declarative physics:** three companion `.mo` models state the ideal dynamics the
  Rust laws approximate, with the rover's real parameters; an adaptive Modelica solver
  integrates each as ground truth:
  - [`assets/models/QuarterCar.mo`](../../assets/models/QuarterCar.mo) — suspension,
    `m·χ̈ = m·g − (k·χ + c·χ̇)` (`k=8000`, `c=2800`, `m=250`).
  - [`assets/models/SlidingBlock.mo`](../../assets/models/SlidingBlock.mo) — friction,
    a block decelerating to rest under continuous-through-zero contact friction.
  - [`assets/models/DrivenChassis.mo`](../../assets/models/DrivenChassis.mo) — drive,
    a chassis accelerating to a traction-balanced terminal velocity.
- **In-repo oracle:** `#[cfg(test)] mod oracle` integrates the *same* equations with
  fine-step RK4 (≈ the Modelica answer to many digits, this system being non-stiff)
  and compares against the production law stepped semi-implicitly at `dt = 1/60`.
  Run: `cargo test -p lunco-mobility --lib oracle -- --nocapture`.

It covers all three force laws (9 tests), each against the same continuous
reference:

**Suspension** (`suspension_force_mag`, quarter-car):
1. **Gentle regime** — tracks the continuous reference to sub-cm (≈3 mm) and settles
   at `χ_eq = m·g/k`. Physics + integration validated.
2. **No limit-cycle** — an under-damped config still decays (late ringing < 15 % of
   early). The dead-band / `.max(0)` bugs would ring forever; this is the guard.
3. **The bound is the fix** — on a hard landing the production law caps the force at
   `2·k·χ` (3.2 kN) while the old `.max(0)` cliff passes the full `c·v` impact spike
   (36 kN, the 27 kN-class transient the jitter work removed).

**Friction** (`contact_friction`, sliding block → rest):
4. **Smooth stop** — a sliding block tracks the reference (Coulomb→viscous knee) and
   comes to rest with **zero** sign-flips through zero.
5. **Dead-band chatters** — the old slip dead-band sign-flips **149×** near rest (the
   stiction limit-cycle = steering jitter) where the continuous law flips 0. The
   oracle catches that exact regression.
6. **Braking grips harder** — full-cone braking stops the block while weak coasting
   grip is still rolling.

**Drive** (`drive_force_mag`, longitudinal accel):
7. **Terminal velocity** — moderate throttle balances grip at `v_term = drive/k`
   (matches the reference to mm/s).
8. **Reverse mirrors forward** — negative throttle gives the exact mirror (the
   `clamp(0,1)`→`clamp(-1,1)` fix).
9. **Traction limit** — excess throttle breaks the friction cone (wheelspin); net
   accel → `(drive−μN)/m`.

The Rust laws are *stabilised approximations*: they agree with the continuous
physics in the gentle regime and intentionally cap stiff transients / regularise
through zero to stay stable at a fixed step — exactly the gap the oracle measures.

**Live Modelica cross-check (optional):** run any of `QuarterCar.mo` /
`SlidingBlock.mo` / `DrivenChassis.mo` through lunica (open in the workbench,
Compile, FastRun) and compare its trace (`f_susp` / `f_fric` / `f_drive`, `chi` / `v`)
against the matching RK4 reference in the `oracle` module — they integrate identical
equations and should agree to many digits. `DrivenChassis.mo` exposes `throttle` as a
parameter, so the 0.2 / −0.2 / 0.8 oracle scenarios are a parameter scrub.

## See Also

- `lunco-controller` — Translates user input into the `DriveRover` events consumed here.
- `lunco-hardware` — Provides the physical actuators (motors, brakes) that mobility systems interface with.
- `avian3d` — The underlying physics engine for force integration.
- `docs/architecture/28-modelica-realtime-physics.md` — the realtime/multiplayer physics plan this oracle is Step 2 of.
