# 15 — Adaptive Fidelity (Multi-Clock & LoD)

> Multiple clocks, multiple fidelities per participant, policy-driven
> switching. Handles the mission-scope vs physics-scope time-scale split,
> time-warp, and CPU-budget-driven level-of-detail.

This extends [`14-simulation-layers.md`](14-simulation-layers.md). Read
that first. Implementation is tracked by task #96 (post-MVP).

## Problem

LunCoSim simulations span time scales from microseconds (flight-computer
loop) to days (surface mission). Running every participant at physics
rate when the user is looking at a mission-scope view is wasteful and
often impossible — e.g. a rover at 100× time-warp would need
terramechanics at 6 kHz.

The solution every serious mission tool converges on: **multi-clock +
multi-fidelity simulation**. Each participant is pinned to a clock;
participants offer multiple implementations at different fidelities; a
policy picks the cheapest viable fidelity given user focus, time-warp,
and CPU budget.

[`22-domain-cosim.md`](22-domain-cosim.md) anticipates this for orbital
mechanics (`FullPhysics / HybridBlend / OnRails`). This doc generalises
it so the regolith-rover case works: close-up at 1× runs full wheel
dynamics; zoomed-out at 100× runs a rate model ("42 kg/hr delivered").

## User stories

### Rover regolith delivery at mission scope (P1)

Drag time-warp 1× → 100×. Rover automatically switches from
`FullDynamics` to `RateModel` when warp × cost exceeds CPU budget. Depot
inventory grows at the correct rate regardless of active fidelity. Zoom
back in → back to `FullDynamics` without discontinuity.

### HIL flight computer alongside slow vehicle sim (P1)

Flight-computer's 1 kHz control loop runs in parallel with 60 Hz vehicle
dynamics, same scenario. Scenario declares
`clocks = { fast: 16.6ms, wall: 1ms, slow: 1s }`. Throttle command
latches from wall → fast; integrated delivery accumulates from fast →
slow.

### Developer pins fidelity for debugging (P2)

Per-participant Fidelity dropdown: Auto | Full | Rate. `Full` overrides
automatic switching for that participant.

### Portrait preserved across swap (P1)

`snapshot_portrait()` / `restore_from_portrait(p)`. After swap,
`position`, `velocity`, `battery_soc`, `carried_mass`, `mode` are
bit-exact. Fidelity-specific state (wheel flux, motor winding) discarded.

## Clocks

```rust
pub struct Clock {
    pub id: ClockId,           // "fast", "slow", "wall"
    pub dt: f64,               // tick size in simulation seconds
    pub rate_factor: f64,      // 1.0 = real-time, higher = time-warp
    pub role: ClockRole,       // Physics | Mission | IO | Custom
}
```

Every Participant pinned to exactly one clock. MasterClocks advance each
clock independently at `rate_factor × dt` per wall second. Global
time-warp multiplies every clock's `rate_factor` uniformly.

Default: one implicit `main` clock at FixedUpdate rate when no explicit
clocks are declared.

## Fidelity bundles

```rust
pub struct ParticipantSpec {
    pub id: ParticipantId,
    pub clock_id: ClockId,
    pub fidelities: Vec<Fidelity>,       // >= 1 entry
    pub portrait_schema: PortraitSchema,
    pub policy: FidelityPolicyRef,
}

pub struct Fidelity {
    pub id: FidelityId,                  // "full", "rate", ...
    pub source: ModelSource,             // .mo class, .fmu, rust fn, ...
    pub cost_factor: f32,                // 1.0 baseline, 0.01 = 100× cheaper
    pub caps: FidelityCaps,
}
```

Default: one entry, id `default`.

## StatePortrait — the handoff contract

Declared per participant. Backend-agnostic serialisable struct. Every
fidelity emits/consumes the same portrait. Fidelity-specific internal
state is dropped on swap.

```ron
portrait_schema: {
    position:     "Vec3",
    velocity:     "Vec3",
    battery_soc:  "f64",
    carried_mass: "f64",
    mode:         { enum: ["Idle", "Driving", "Loading", "Dumping"] },
}
```

## Cross-clock rendezvous

Connections whose endpoints sit on different clocks declare a bridge:

- **Latch** — slow clock reads last published value on its tick. For
  set-points (throttle, target attitude).
- **Accumulate** — slow clock sums fast-clock output between ticks;
  master resets accumulator after read. For rates (kg/s → kg, amp → Ah).
- **Event** — fast clock emits discrete event; slow clock queues and
  processes on its tick. For state transitions (arrived, loaded).

Same-clock connections need no bridge.

## FidelityPolicy

```rust
pub trait FidelityPolicy {
    fn pick(&self, ctx: &PolicyContext, participant: &ParticipantSpec)
        -> FidelityId;
}

pub struct PolicyContext {
    pub rate_factor: f64,
    pub user_focus: Option<ParticipantId>,
    pub camera_distance: Option<f32>,
    pub cpu_budget_used: f32,
    pub wall_clock_lag: Duration,
}
```

Built-in:
- `FixedLevel { id }` — manual override.
- `AutoFromFocus` — upgrade focused, downgrade others.
- `AutoFromSpeed` — downgrade above warp threshold.
- `AutoFromBudget` — stay within CPU budget per frame.
- `Composite(…)` — max-coarseness wins.

Twin registers custom policies at plugin build time.

## Hysteresis

A participant must stay in a new fidelity for at least N ticks (default
30) before switching again. Prevents flicker at boundary conditions.

## PhysicsMode becomes a policy

`22-domain-cosim.md` describes `FullPhysics / HybridBlend / OnRails`
state machine. In this framework it's one `OrbitalRailsPolicy`
implementation — not a parallel system. Same hysteresis, same portrait,
same commands.

## Dependencies

- **`14-simulation-layers.md`** — the Run/Scenario/Twin shape.
- **`22-domain-cosim.md`** — pipeline + Backend traits.
- **Task #94** — four-layer architecture formalisation.
- **Task #95** — BackendRegistry + Twin scenarios.
- **Task #96** — this spec's implementation task.

## Out of scope

- Continuous cross-fade blending between fidelities. Swap is discrete;
  blending is v2.
- User UI for authoring custom `StatePortrait` fields. Declared in the
  scenario file for now.
- Learned surrogate models (ML-trained rate models). Fidelities are
  hand-authored.

## MVP stance

Modelica MVP does not implement this. Scenario schema reserves
`clock_id: "main"` and `fidelities: [default]` on every participant so
future adoption is an extension, not a migration. One clock, one
fidelity, no policy.
