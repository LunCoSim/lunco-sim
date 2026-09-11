# lunco-cosim

Co-simulation orchestration for LunCoSim. Connects multiple simulation models
(Modelica, FMU, GMAT, Avian) in a single Bevy world via explicit wires that
route named outputs to named inputs, following the FMI/SSP pattern.

## Architecture at a glance

Every participant — Modelica/FMU model, Avian rigid body, joint, raw physics
query, or
hardware signal — exposes its state as **named scalar ports** through one shared
surface, the **`PortRegistry`** (defined in `lunco-core::ports`, *below* every
participant so wires, the HTTP API, the inspector, rhai, and Python all read/write
through it without depending "up" into this engine). `lunco-cosim` owns and
registers the built-in backends (`ports::register_builtin_port_backends`):

`PortRegistry::entity_port_infos` is the inspection projection used by the native
Ports panel and `ReadPorts`/`ListPorts`. It preserves the backend-owned live value
and adds the scalar type, optional unit and inclusive bounds, source, current
authority, and manual-writability contract. Consumers do not reconstruct those
facts from a port name; writes still go through the existing typed `SetPorts`
command.

`PortRegistry::port_entities` is the corresponding discovery projection. Each
backend enumerates the component or authored surface it owns, and the registry
merges those candidates once per inspection sample. A consumer must use this
path instead of scanning every ECS entity and probing every backend.
For bounded-cadence views, each backend also supplies an identity-only
`topology_key`; consumers cache the `entity_port_infos` metadata while reading
live values through the registry on each sample. Values therefore stay current
without rebuilding the port table when only physics or solver state changes.

| Backend | Ports |
| --- | --- |
| **Modelica `SimComponent`** | its declared `input`/`output` variables (`height`, `netForce`, …) |
| **Avian rigid body** (`RIGID_BODY_GROUP`) | **out:** `position_{x,y,z}`, `velocity_{x,y,z}`, `quat_{w,x,y,z}`, `yaw`/`pitch`/`roll`, `angvel_{x,y,z}` · **in:** `force_{x,y,z}` (world), `force_local_{x,y,z}` (body-frame), `torque_{x,y,z}`, `mass`, `inertia_{xx,yy,zz}`, `com_{x,y,z}` |
| **Avian revolute joint** (`REVOLUTE_JOINT_GROUP`) | `angle` — out (measured twist) + in (drives the `AngularMotor`) |
| **Avian prismatic actuator** (`PRISMATIC_JOINT_GROUP`) | `displacement` — out (slider offset) + in (drives the `LinearMotor`) |
| **Avian observations** | Native rigid-body/contact facts plus RaycastObservation → ray_distance, ray_hit_valid, hit point/normal, and sample time |
| **Modelica sensor conversions** | IMU, altimeter, attitude, and touchdown semantics are ordinary Modelica inputs/outputs wired in USD |
| **Hardware** (`Port`) | `value` (f64) |

Avian's foreign components are exposed declaratively via the `AVIAN` spec table
(`ports.rs`) — adding a kind (a new joint or raw physics query) is one AvianGroup entry,
no observer or sync system. **Adding a port group:** declare the `AvianGroup`
(present-predicate + entity enumerator + `AvianPort`s with read/write closures)
and list it in `AVIAN`.

A [`SimConnection`] connects any output port to any input port. The cosim master
runs in `FixedUpdate`:

1. **`Propagate`** — `propagate_connections` reads every source output and writes
   the target input (summing with `+=` so multiple wires sum into one input). A
   `force_*` write lands in the body's `PendingForces` accumulator; a joint
   `angle`/`displacement` write drives that joint's motor inline.
2. **`ApplyForces`** — the single `apply_pending_forces` system drains
   `PendingForces` into Avian's `Forces` (world force, `apply_local_force` for
   body-frame, `apply_torque`) and clears it. Bodies are `RigidBody::Dynamic`;
   Avian's own integrator advances them in `FixedPostUpdate`. Gravity is a force
   applied separately by [`lunco-environment`](../lunco-environment) — models
   produce thrust/buoyancy only, never weight.

Avian *outputs* are read on demand through the registry (state is stable between
physics steps), so there is no per-tick output-snapshot system.

## Modelica model convention: declare everything you want to observe as `input` or `output`

**The rule:** in any Modelica model driven by `lunco-cosim`, every variable
that needs to be read by the co-simulation wires — or written into the model
from outside — must have explicit `input` or `output` causality.

```modelica
model Balloon
  parameter Real mass = 4.5;

  // Wires feed these in from Avian each step
  input Real height = 0;
  input Real velocity = 0;

  // State — rumoca keeps this in the solver regardless
  Real volume(start = 4.0);

  // ALL OBSERVABLE DERIVED VALUES MUST BE `output`
  output Real netForce;
  output Real buoyancy;
  output Real weight;
  output Real drag;
  output Real temperature;
  output Real airDensity;
equation
  // ...
end Balloon;
```

### Why

Rumoca's DAE preparation pipeline aggressively substitutes algebraic variables
into the state equation. If you declare an algebraic as a bare `Real netForce`,
rumoca eliminates it during index reduction — the variable literally stops
existing in the solver. After compile:

- `stepper.variable_names()` returns `["volume"]` — `netForce` is gone.
- `stepper.get("netForce")` returns `None` — no name-based recovery either.

So `SimComponent.outputs["netForce"]` never gets populated, the
`netForce → force_y` wire has nothing to read, no force reaches Avian, and the
balloon sits still.

Declaring a variable with `output` causality tells rumoca it's part of the
model's public interface and must be preserved in the solver index. After that
single keyword change, `stepper.get("netForce")` returns a real value and the
whole cosim chain lights up.

This is a [known, reproducible rumoca limitation](#upstream-rumoca-workaround)
— the convention above is both the fix *and* good FMI/SSP hygiene: a model's
inputs and outputs are its public interface; everything else is private
implementation.

### Inputs: same story

Inputs without explicit `input` causality get inlined as constants. Always
declare them:

```modelica
input Real height = 0;   // runtime-settable, default 0
input Real velocity = 0;
```

The default value (`= 0`) is stripped by `lunco-modelica-core` at compile time
(`strip_input_defaults`) so the variable becomes a true runtime slot
settable via `stepper.set_input("height", ...)`. Without the `input` keyword
at all, the variable would vanish like the algebraics did.

### Checklist when adding a new Modelica model to lunco-cosim

1. Mark every wire-destination as `input Real name = <default>;`.
2. Mark every wire-source as `output Real name;`.
3. States can stay bare (`Real x(start = ...);`) — rumoca always keeps them.
4. Parameters stay as `parameter Real foo = 1.0;`.
5. Add or extend an authored scene under `assets/scenes/tests/` and a Rhai
   scenario under `assets/scenarios/tests/`. Run it through the production
   `target/debug/luncosim test --scene ...` command and assert every variable
   that the USD boundary actually wires. If a value is absent, the authored
   Modelica class is missing an `output` declaration or the USD interface is
   incomplete.

## Tests

- **`tests/cosim_test.rs`** — the generic cosimulation contract: live Avian
  ports, SimConnection propagation, force accumulation, and admission rules.

- **`tests/balloon_e2e_test.rs`** — unit-level wire propagation and force
  application with mocked `SimComponent.outputs`. Fast; no Modelica worker
  involved.

- The application-level **Modelica → Python → Avian** chain lives in the
  authored [`assets/scenes/tests/cosim_chain.usda`](../../assets/scenes/tests/cosim_chain.usda)
  scene and its Rhai scenario. It crosses the worker, scripting, USD projection,
  and physics composition boundary through the production runner; this leaf
  package keeps only co-simulation mechanism tests.

- **`assets/scenes/tests/modelica_balloon_earth.usda`** plus its Rhai scenario —
  exercises the shipped Balloon source through the production USD/Modelica
  path and asserts the published `netForce` and physical rise. Rust keeps only
  generic compiler/solver seams whose behavior cannot be observed through the
  public scene surface.

Run with:

```bash
scripts/run_rust_tests.sh -p lunco-cosim --module cosim_test
scripts/run_rust_tests.sh -p lunco-cosim --module balloon_e2e_test
target/debug/luncosim test --scene scenes/tests/cosim_chain.usda
target/debug/luncosim test --scene scenes/tests/modelica_balloon_earth.usda
```

`lunco-cosim`'s dep graph is small, so these tests recompile in a few seconds
on an incremental build — unlike `luncosim` / `lunco-luncosim` tests, which pull in the full
Bevy renderer.

## Upstream rumoca workaround

The `output`-keyword convention works around a limitation in our
[rumoca fork](https://github.com/LunCoSim/rumoca). Rumoca's
`SimulationSession::variable_names()` returns only the post-reduction solver state
(`solver_names` truncated to `n_total = dae.f_x.len()`), and
`SimulationSession::get(name)` only knows about indices present in the solver — so
after aggressive substitution, algebraic values are unreachable even by name.

A cleaner upstream fix would be one of:

- **Option A:** Don't substitute variables declared with `output` causality in
  `prepare_dae`. Keep them in `dae.f_x` as trivial equations. (Already
  effectively our current behavior — but we're relying on it rather than it
  being documented.)
- **Option B:** Add a `SimulationSession::evaluate_algebraic(name) -> Option<f64>`
  API that re-evaluates an eliminated algebraic expression using the current
  state + inputs + parameters. Algebraic expressions still live in
  `dae.algebraics`; we'd just need an expression evaluator over the current
  solver state.
- **Option C:** Keep a side-table of "observable" variables with their
  symbolic expressions, and have `get(name)` fall back to evaluating from that
  table when the name isn't in the solver index.

Until one of these lands, the `output` convention is the supported way to
declare model variables to lunco-cosim.
