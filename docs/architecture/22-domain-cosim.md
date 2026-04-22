# 22 — Co-Simulation Domain

> Connects multiple simulation engines (Modelica, FMU, GMAT, Avian) in a
> single Bevy world via explicit `SimConnection`s between named ports.
> Implements the FMI/SSP pattern.

This architecture doc summarizes the high-level model. For in-depth
engineering docs (system ordering, panel registration, convention details),
see **[`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md)**.

## Core concepts

Defined in [`01-ontology.md`](01-ontology.md) section 4a:

- **`SimComponent`** — wraps a model instance; exposes named inputs / outputs
- **`SimConnection`** — links a source port to a target port (FMI/SSP Connection)
- **`SimPort`** — metadata for a connectable interface point
- **`AvianSim`** — Avian physics treated as a cosim model

## Execution pipeline

All cosim and physics systems run in `FixedUpdate` at a shared fixed timestep
so every engine advances with the same `dt`:

```
FixedUpdate:
  1. ModelicaSet::HandleResponses   — receive async results from worker thread
  2. sync_modelica_outputs          — ModelicaModel.variables → SimComponent.outputs
  3. CosimSet::Propagate            — propagate_connections: source outputs → target inputs
  4. CosimSet::ApplyForces          — apply_sim_forces: route netForce into Avian Forces
  5. sync_inputs_to_modelica        — SimComponent.inputs → ModelicaModel.inputs
  6. ModelicaSet::SpawnRequests     — send next step command with fixed dt

FixedPostUpdate:
  7. Avian PhysicsSchedule          — integrate_positions, constraint solve, writeback
  8. read_avian_outputs             — Position / LinearVelocity → AvianSim.outputs
```

The master loop reads outputs, propagates through connections, writes inputs,
then steps all engines — this is the FMI master algorithm.

## Where the master loop fits

The pipeline above is the *body* of the per-tick advance. The layer that
**owns** the pipeline is `Twin` — the Bevy Resource introduced in
[`14-simulation-layers.md`](14-simulation-layers.md). The loop advances
the active `Run`s, which reference `Scenario`s materialised from
`twin.toml` `[scenarios.*]`. Today's implicit "one doc, one model,
steps forever" is the degenerate case: one implicit Twin, one implicit
Run, one participant — same master loop.

## Backend registry (dynamic, plugin-driven)

Backends self-register at app boot. Each domain crate ships a Bevy
plugin that inserts itself into `BackendRegistry`:

```rust
// lunco-modelica
impl Plugin for ModelicaBackendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BackendRegistry>();     // idempotent
        app.world_mut().resource_mut::<BackendRegistry>()
            .register(Arc::new(ModelicaBackend));
    }
}
```

Dropping a crate removes its backend. Scenarios referencing missing
backends fail gracefully at load. FMU / Python / GMAT / DCP backends
arrive as separate crates — no core edits.

`Backend` + `Participant` traits live in `lunco-cosim`. See
[`14-simulation-layers.md`](14-simulation-layers.md) for the full
signatures and capability flags.

## Typed connections + island partitioning

Connections carry a kind:

```rust
pub enum ConnectionKind {
    Causal,   // output → input (signal). Our SimConnection today.
    Acausal,  // Modelica connect, FluidPort, Flange, Pin. Kirchhoff-style.
}
```

Acausal connections cannot cross cosim boundaries without losing
accuracy (fake algebraic loops delay one signal by a step). At Run
start, the **IslandPartitioner** groups participants:

1. Union-find over participants connected by acausal edges.
2. Each island must share a backend that advertises
   `caps.can_absorb_acausal`. Otherwise → scenario-load error.
3. Backend `fuse()` collapses the island into one participant. For
   Modelica this means code-generating a wrapper `.mo` that replicates
   the connections as `connect()` equations and compiling once.
4. Inter-island connections remain as `SimConnection` and are propagated
   by the master each tick (causal only).

Result: three Modelica components wired by `FluidPort`s become one
flattened DAE with one stepper (Dymola's default behaviour). A
Modelica + Python mix becomes two islands bridged by causal signals
(classical cosim). Users can opt out per participant with
`explicit_boundary = true` for debugging.

Balloon case today: Modelica balloon + Avian rigid body, three causal
wires. Two islands, one causal bridge. No acausal edges → no fusion →
identical to today's behaviour. Partitioner lands without regression.

## Dynamic bodies, not Kinematic

Balloon (and other subsystem-driven bodies) are `RigidBody::Dynamic`.
Modelica's `netForce` flows through `SimConnection` into `AvianSim.inputs`,
then `apply_sim_forces` applies it via `Forces::apply_force`. Avian's own
integrator advances velocity and position. Gravity is applied by
[`lunco-environment`](23-domain-environment.md)'s
`apply_gravity_to_rigid_bodies` system — Modelica models no longer subtract
weight; they only produce aerodynamic / buoyancy force.

Historical note: earlier designs used Kinematic bodies with direct `Position`
writes. That caused (a) change-detection conflicts with gizmo drags,
(b) double-integration when `LinearVelocity` was also written, (c) missing
collision response on joints. Current Dynamic-body design avoids all three.

## Pause and time warp

Pause / resume / reset / time-warp are all `TwinCommand` variants
dispatched through the Twin resource. The master-loop pipeline reads
`Run.status` and `Run.rate_factor` each tick:

- **Pause** = `Run.status = Paused`. Master loop skips all pipeline
  steps. Wall time continues; sim time frozen. Parameter edits queue as
  `SetParam` TwinCommands and apply on Resume (Tunable semantics).
- **Resume** = `Run.status = Running`. Master loop resumes at the next
  FixedUpdate tick from the same `current_t`.
- **Reset** = bumps session id (existing mechanism), sends
  `ParticipantCommand::Reset` to every island, clears trace +
  input_log, `current_t = t_start`, `status = Idle`.
- **Time warp** = `Run.rate_factor` scales. Per-tick advance =
  `rate_factor × FixedUpdate.dt`. Clocks with different base dt scale
  proportionally. Global slider → same factor applied to every clock's
  rate (see [`15-adaptive-fidelity.md`](15-adaptive-fidelity.md)).

Historical note (pre-Run model): earlier designs used a per-entity
`SimPaused` marker and ad-hoc `Time<Physics>::pause()`. That remains a
correct low-level mechanism, but the Run-centric model is now the
single source of truth — toolbar / API / scripts all go through
TwinCommand, not direct component mutation.

## Convention: Modelica `output` requirement

Rumoca (our Modelica runtime) eliminates algebraic variables from the
solver during DAE preparation unless they're declared as `output`. This is
a rumoca limitation that has been worked around by convention:

```modelica
model Balloon
  input Real height = 0;
  input Real velocity = 0;
  Real volume(start = 4.0);

  // ALL observable derived values must be `output`
  output Real netForce;
  output Real buoyancy;
  output Real drag;
end Balloon;
```

See [`../../crates/lunco-cosim/README.md#modelica-model-convention`](../../crates/lunco-cosim/README.md)
and [`20-domain-modelica.md`](20-domain-modelica.md) for the full story,
including planned upstream fixes to the rumoca fork.

## See also

- [`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md) — engineering docs
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica-specific design
- [`23-domain-environment.md`](23-domain-environment.md) — environment/gravity integration
- `specs/014-modelica-simulation` — detailed Modelica spec
- `specs/022-fmu-gmat-integration` — FMU and GMAT integration spec
