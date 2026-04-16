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

*Not yet implemented — planned.*

- **Per-entity pause**: `SimPaused` marker component (in `lunco-core`). Any
  system that respects it skips that entity: wire propagation, force
  integration, Modelica stepping. Outputs remain readable at their last
  value so downstream consumers keep running on frozen data.
- **Global Avian pause**: Avian's built-in `Time<Physics>::pause()`.
- **Global Modelica pause**: planned future `ModelicaEnabled` resource.
- **Time warp**: `Time<Virtual>::set_relative_speed(factor)`. Bevy's virtual
  time scales the `FixedUpdate` accumulator; more fixed-timestep iterations
  run per frame. All engines speed up together because they share the same
  schedule.

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
