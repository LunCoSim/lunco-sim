# lunco-cosim

Co-simulation orchestration for LunCoSim. Connects multiple simulation models
(Modelica, FMU, GMAT, Avian) in a single Bevy world via explicit wires that
route named outputs to named inputs, following the FMI/SSP pattern.

The backend-neutral runtime contract is owned by
[`lunco-cosim-core`](../lunco-cosim-core/): `SimComponent`, `SimStatus`,
`SimConnection`, `ConnectionBinding`, `BoundConnection`, connection diagnostics,
control holds, typed control commands, and shared connector contracts. This crate
owns the Avian integration, port backends, command observers, binding transaction,
and fixed-step propagation that realize that contract.

## Architecture at a glance

Every participant — Modelica/FMU model, Avian rigid body, joint, raw physics
query, or
hardware signal — exposes its state as **named scalar ports** through one shared
surface, the **`PortRegistry`** (defined in `lunco-port-core::ports`, *below* every
participant so wires, the HTTP API, the inspector, rhai, and Python all read/write
through it without depending "up" into this engine). `lunco-cosim` owns and
registers the built-in backends (`ports::register_builtin_port_backends`):

Port names stay strings in USD, Rhai, commands, and diagnostics. Fixed-step
`CompiledWiring` resolves output sources, readable input-side sources, and input
targets to owning-backend slots when connections or the shared
`PortTopologyRevision` changes; steady propagation uses dense target indices
and those slots. A write-only input is a valid target but not a readable source.
Avian named discovery shares table-derived candidates across resolution,
declarations and metadata, checking entity membership live; see the
[port contract](../../docs/architecture/22-domain-cosim.md#core-concepts).
Successful propagation publishes to diagnostics once per compiled target;
scene teardown clears its publication cursor with the scene-owned ledger.
`PortMap<T>` provides dynamic named surfaces with layout-checked process-local
handles. Structural edits publish a topology revision and recompile the fabric;
stale handles are rejected rather than retried through a name path. Sparse
`PortHolds` retains controller input and authored-program setpoints in separate
layers; controller values take precedence over program values, which take
precedence over the wiring fabric. The authored layer survives authority
release so autopilot guidance resumes naturally. Holds are iterated by borrowed names and matched once to compiled target
indices, so the tick does not hash every target against the hold table.

At admission, `SimConnection::resolved_transform` converts resolved compatible
units before composing the authored gain and offset in target units, following
SSP. An omitted base-unit definition keeps the raw value under FMI's unit rule.
Incompatible unit contracts and unrepresentable coefficients reject the edge
and hold physical execution. Dense slots cache these coefficients; steady
exchange applies compiled arithmetic and retains the port registry's live
write validation.

Binding and solver initialization share `connected_initial_inputs`. All fan-in
samples are captured before initial writes, in the same authoritative order as
live exchange. Unbound contributions and missing samples wait without writing zero or a partial sum;
explicit holds retain priority. The ordering includes input/output side and
coefficient bits, so repeated sources with different transforms stay
independent of ECS spawn order. A rejected initial value is a structured
connection fault. `dropped_value` is optional in diagnostics: no attempted write
means no numerical value.
Public diagnostic values use the
[`GetBrokenConnections` numeric contract](../lunco-api/README.md); internal
diagnostics retain the rejected `f64`, including non-finite fan-in results.

`python3 scripts/api/test_connection_units.py --port <free-port>` runs the
authored native length/mass contract gate. Add `--windowed` to use the same
observer in the visible production host. It requires the post-fault verdict,
including the retained valid target value, rather than treating fault arrival
alone as the test result.

Initial handoff and live exchange use the registry's scoped `input_transaction`.
Its immutable staging view keeps each live owner validation current through
atomic commit under the same exclusive world borrow. Reusable staging storage
retains capacity between exchanges. A rejected live value preserves every target
in that batch and latches `cosim-input-write-rejected`, including after previous success. The
shared failure owner pauses native physics immediately, before its scheduled
integration, and retains the real rejected value and owner diagnostic.
`python3 scripts/api/test_connection_transaction.py --port <free-port>`
qualifies this through a Modelica source and native body; add `--windowed` for
the same ten-check observer in the visible host.

`PortRegistry::entity_port_infos` is the inspection projection used by the native
Ports panel and `ReadPorts`/`ListPorts`. It preserves the backend-owned live value
as an optional sample: a declared output that has not produced a value stays
unavailable instead of appearing as `0.0`. The projection adds scalar type,
typed optional unit and frame, inclusive bounds, source, current authority, and
manual-writability contract. Modelica and native FMI participants publish
`ScalarPortContracts` at source admission; their shared backend reads that
component for topology and write validation. Unit definitions retain the
separate radian exponent and relative-quantity semantics. Consumers do not reconstruct those facts from a
port name; writes go through typed `SetPorts` or `SetPortsBatch` commands.

`SetPorts` validates every named input against its owner metadata, including
writability and any inclusive bounds, before accepting the batch. External API,
identified direct typed, and non-Simulation Rhai commands with stable producer
identity receive an admission receipt for the next fixed tick. Live
port-inspector commands carry the active local session identity into that same
admission path. A receipt does not claim that the value has already reached
the port backend. The ordered session commit then applies or visibly faults
the admitted write. Simulation-clock Rhai and
fixed-step controller writes keep their owning execution path. Explicit
external `ReleasePort`/`ReleaseControl` commands share the admission queue and
remove local input holds at their ordered commit without writing replacement
values. A `ControlAuthorityChanged` event queues `ReleaseControlInputs` for each
released endpoint at the next fixed tick. This clears controller-owned holds
and held semantic intents while preserving authored program setpoints and
physics state. Connection loss uses the same lifecycle release.
Twin-owned safety behavior writes declared setpoints explicitly with
`SetPorts`. `PortRegistry` uses the same declared owner resolution for named
writes and fast-path input locators, so a lower owner cannot receive a write
after the selected owner refuses it. Session capture retains the lifecycle
hold-release record at its commit when active. A missing stable target, scene
generation, tick, or shared order stamp reports a runtime error and keeps the
existing holds. The release never writes replacement endpoint values or
changes velocity.

`SetPortsBatch` writes inputs on 2 to 256 stable targets as one session-input
transaction. The host requires ownership of every target. All target identities,
lifecycle fences, write names, writability contracts, and metadata bounds are
validated before admission or application. One record captures all participants
at one effective tick; a release affecting a queued participant cancels the
whole transaction.

`PortRegistry::port_entities` is the corresponding discovery projection. Each
backend enumerates the component or authored surface it owns, and the registry
merges those candidates once per inspection sample. A consumer must use this
path instead of scanning every ECS entity and probing every backend.
For bounded-cadence views, each backend also supplies an identity-only
`topology_key`; consumers cache the `entity_port_infos` metadata while reading
live values through the registry on each sample. Values therefore stay current
without rebuilding the port table when only physics or solver state changes. The
owner publishes `PortTopologyRevision` from lifecycle observers and structural
checks. Modelica's `SimComponent` maps use `ScalarPortMap`, which maintains an
order-independent name-set key as names are inserted or removed; numeric sample
writes do not re-hash the map. Avian groups keep their topology key and
invalidation hook beside their membership predicate, including any optional
backing-component presence that changes the emitted rows.

| Backend | Ports |
| --- | --- |
| **Modelica `SimComponent`** | its declared `input`/`output` variables (`height`, `netForce`, …) |
| **Avian rigid body** (`RIGID_BODY_GROUP`) | **out:** `position_{x,y,z}`, `velocity_{x,y,z}`, `quat_{w,x,y,z}`, `yaw`/`pitch`/`roll`, `angvel_{x,y,z}` · **in:** `force_{x,y,z}` (world), `force_local_{x,y,z}` (body-frame), `torque_{x,y,z}`, `mass`, `inertia_{xx,yy,zz}`, `com_{x,y,z}` |
| **Avian revolute joint** (`REVOLUTE_JOINT_GROUP`) | `angle` — out (measured twist) + in (drives the `AngularMotor`) |
| **Avian prismatic actuator** (`PRISMATIC_JOINT_GROUP`) | `displacement` — out (slider offset) + in (drives the `LinearMotor`) |
| **Avian observations** | Native rigid-body/contact facts plus a port adapter for physics-owned `RaycastObservation` → ray_distance, ray_hit_valid, hit point/normal, and sample time |
| **Modelica sensor conversions** | IMU, altimeter, attitude, and touchdown semantics are ordinary Modelica inputs/outputs wired in USD |
| **Hardware** (`Port`) | `value` (f64) |
| **USD component** (`PortSurface`) | authored input/output names backed by scalar `Port` endpoints |

Avian's foreign components are exposed declaratively via the `AVIAN` spec table
(`ports.rs`). **Adding a port group:** declare the `AvianGroup` (present-predicate,
entity enumerator, port and metadata contract key, and `AvianPort`s with read/write
closures), provide its lifecycle/structural invalidation hook, and list it in
`AVIAN`. The group key must match the rows emitted by its port definitions; live
samples never belong in that key.

A [`SimConnection`] connects any output port to any input port. The cosim master
runs in `FixedUpdate`:

1. **`Propagate`** — `propagate_connections` reads every source output and writes
   the target input (summing with `+=` so multiple wires sum into one input). A
   `force_*` write lands in the body's `PendingForces` accumulator; a joint
   `angle`/`displacement` write drives that joint's motor inline. Changing a
   motor target wakes sleeping dynamic endpoints through Avian's island wake
   command, so the next solver step can consume the new setpoint; repeating an
   unchanged target does not wake the island.
2. **`ApplyForces`** — the single `apply_pending_forces` system drains
   `PendingForces` into Avian's `Forces` (world force, `apply_local_force` for
   body-frame, `apply_torque`) and clears it. Bodies are `RigidBody::Dynamic`;
   Avian's own integrator advances them in `FixedPostUpdate`. Gravity is a force
   applied separately by [`lunco-environment`](../lunco-environment) — models
   produce thrust/buoyancy only, never weight.

Avian *outputs* are read on demand through the registry (state is stable between
physics steps), so there is no per-tick output-snapshot system.

## Modelica public ports

Declare external destinations as Modelica `input` and wire sources as `output`.
Those declarations define the public interface consumed by the authored USD
`inputs:` and `outputs:` properties. Internal state and algebraic variables
retain their Modelica ownership; solver elimination does not define the public
co-simulation contract.

External input bindings initialize their qualified runtime slots. Explicit live
input writes replace only the addressed slot, while internal and connected
inputs retain their equation bindings. Run values enter Rumoca's simulation
options before initialization; compiled DAE artifacts stay immutable.

Initial algebraic dependencies use the optional typed `PortInitialExpression`
backend contract. `PortRegistry::initial_expression` selects the same declared
owner as a numerical read. `PortInitialSource` distinguishes independent samples,
pending preparation and exact relations; a pending relation prevents reading a
partial current topology as an initial value. The Avian island-mass output expresses the sum of its
prepared mechanical members' mass inputs; USD Modelica admission consumes that
relation through the existing wires. A declared interface carries no numerical
sample until its owner produces one. See the [coupled admission lifecycle](../../docs/architecture/20-domain-modelica.md#coupled-initial-admission)
for source-backed feedback initialization, certification and the FMI boundary.

A missing demanded output or rejected input is a visible owner error. Do not
substitute zero or continue a wire exchange with an absent value. Add an authored
USD/Rhai production gate that observes the actual connected public ports when
changing a model's interface.

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
