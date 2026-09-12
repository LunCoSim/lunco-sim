# 62 — Chrono physics backend

> Status: Draft · Audience: physics, cosim, and packaging contributors
>
> Under review; no backend, successful build, performance, or physical validation
> is claimed. [Issue #52](https://github.com/LunCoSim/lunco-sim/issues/52) tracks P0.

## Decision and scope

Evaluate an optional native C++ Chrono worker controlled by a thin Rust adapter.
P0 establishes whether a headless rigid-terrain experiment builds, steps, and can
be packaged reproducibly. A Rust rewrite and production integration are outside
P0. No schedule estimates apply until measurements exist.

The [paper](https://arxiv.org/html/2410.04371v1) motivates vehicle/soil/sensor
experiments, not LunCoSim compatibility or real-time guarantees. Its dust model is
phenomenological. SCM, CRM/DEM excavation, Sensor, dust, ROS 2, HIL, and dataset
generation are possible follow-ups requiring separate review.

## Existing owners and extension boundary

| Concern | Existing owner | Proposed relationship |
|---|---|---|
| Scene facts | Composed USD and canonical shared readers | Consume topology, geometry, mass, joints; no duplicate production vehicle database |
| Mechanical solve | Avian; admission/configuration in `lunco-physics` | Replace only a complete isolated mechanical island |
| Physical frame | `lunco_core::ActivePhysicsFrame` | Retain single frame authority |
| Continuous equations | Modelica | Keep electrical, thermal, and controller state; no duplicate mechanical integration |
| Causal exchange | `lunco-cosim` and `PortRegistry` | Extend current admission and port exchange |
| Time | `lunco-time` | Follow master-requested intervals |
| Batch runs | `lunco-experiments::ExperimentRunner` | Candidate first production consumer |
| Scenario policy | Rhai | Use existing commands and authored verdicts |

[14](14-simulation-layers.md) explicitly marks the general `Backend`/`Participant`
registry as unimplemented. [25](25-experiments.md) allows future FMU, codegen, HIL,
and remote workers. Chrono fits that intended boundary, not a second control plane.
P0 uses a standalone Rust harness. Compare a narrow native adapter with FMI-CS
before choosing production integration; do not claim an existing FMI wrapper.
Later batch work should reuse `ExperimentRunner`; live work extends
[22's macro-step contract](22-domain-cosim.md). `lunco-worker-transport` is wasm-only
Web Worker plumbing, not a native C++ launcher. Encoding and crate names remain open.

## Mechanical ownership

`ActivePhysicsFrame` selects coordinates; it does not allow duplicate state
integration. The [Avian bridge](../../crates/lunco-usd-avian/src/big_space_bridge.rs)
owns f64 pose exchange today. A Chrono admission/projection path is missing and
must be reviewed before live use.

Admit a complete rover and terrain contact environment at run initialization.
Its chassis, wheels, attachments, joints, drives, and contacts leave Avian and
raycast mobility as a unit. ECS entities become visual/telemetry projections
without active Avian solver/collider state. USD remains authored truth.
Other Avian islands may coexist only without mechanical contact or joints to it.
Reject cross-engine joints, collisions, moving terrain exchange, and two-way force
coupling at admission. Static terrain snapshots carry provenance; edits restart
the experiment. Runtime solver handoff is excluded: poses alone cannot transfer
contact history, soil state, joints, and velocities.

Modelica controllers may later exchange causal commands/observations. Select either
Chrono drivetrain commands or external actuator torques per actuator; never apply
both or integrate its mechanical state in Modelica as well.

## Coordinates and time

Follow [41](41-axes-and-units.md): f64, right-handed, Y-up, -Z-forward, SI metres,
kilograms, seconds, radians. Consume canonical USD reader output without double
conversion. Reject non-SI scalar metadata the readers cannot convert.
For a worker configured X-forward/Z-up/right-handed, the adapter's proper rotation
is `(x,y,z)_worker = (-z,-x,y)_canonical`. Confirm this with the pinned demo; do not
assume every Chrono asset uses that convention. Rotate gravity, velocities,
forces, and torques; conjugate orientation and inertia. Serialize named quaternion
`w,x,y,z` components, not library memory layouts.

Use a frozen experiment frame derived from `ActivePhysicsFrame`, recording origin
and revision. Use existing f64 helpers, not render `GlobalTransform` or public
`CellCoord` identities. P0 excludes moving/rotating frames. A real frame change
invalidates the run; render recentering must not change physical state. Moving
frames need explicit velocity transport and inertial-force semantics.

[19](19-unified-time-and-clock.md) owns `SimTick`/`TimeTransport`. Worker time is
elapsed simulation seconds from recorded run start. Epoch remains the existing
calendar projection, never wall time or an invented Julian date; celestial runs
use authored epoch metadata. Reset/re-anchor invalidates requests and restarts
state. Pause/warp keep master step meaning. A P0 test cadence is not a new live clock.

## Exchange and lifecycle

Proposed messages carry protocol version, run generation, scene identity, frame
revision, sequence, start/stop time, inputs, and complete output state. Worker
handles map to existing identities. Live integration uses doc 22's declared
communication points and causal barrier: accept only matching in-flight endpoints,
with solver substeps landing on them. Wait asynchronously; never block UI or let
stale physics cross a causal boundary. Derive cadence from the time owner.

Reject non-finite, stale, duplicate, incompatible, and out-of-order responses.
Crash/timeout/partial steps fail visibly, preserve diagnostics, and never silently
switch to Avian. Reset, cancellation, Twin close, and
[scene teardown](61-scene-lifecycle-and-teardown.md) reap workers and invalidate replies.
Start with bounded state messages; shared memory/GPU interop need measurements.
Shared memory does not itself mean zero-copy GPU transfer. Fixed stepping does
not prove cross-platform determinism: exclude client prediction/rollback under
[28](28-modelica-realtime-physics.md). Networking needs separate review.

## Native build, licensing, and web

Use an external CMake adapter against a pinned installed Chrono and its
[`template_project`](https://github.com/projectchrono/chrono/tree/main/template_project)
pattern with `Chrono_DIR`. Record commit, compiler, preset, modules, assets, solver
settings, and runtime libraries. Start CPU/headless with Core + Vehicle on rigid
terrain. Upstream Vehicle JSON examples are spike fixtures; production scene
facts must come through the existing USD composition boundary.

Native capability is opt-in. Missing/incompatible workers leave ordinary Avian
scenes usable but fail Chrono-dependent runs at preflight with actionable errors.
Loading USD must not download or launch executables. Use trusted configuration
and explicit run commands. Validate packaged dependencies from an installed
directory on every claimed platform.

Chrono is BSD-3-Clause; LunCoSim is Apache-2.0. Preserve Chrono copyright,
conditions, disclaimer, and non-endorsement requirements in redistributed source
and binaries; retain LunCoSim's license/notices. Process separation simplifies
packaging but removes no dependency or asset license obligations. Inventory each
redistributed component; check optional CUDA/OptiX terms for the pinned version.

[`scripts/build_web.sh`](../../scripts/build_web.sh) builds wasm, which cannot spawn
the C++ worker. Compile out native process dependencies and report Chrono
unavailable. Ordinary scenes remain usable; Chrono-dependent runs are refused.
Recorded-result visualization may use existing facilities where supported without
claiming live execution. No silent Avian substitution or implicit remote upload.
Remote execution requires a separate connection/authorization design.

## P0 evidence gate

Issue #52 owns the executable checklist. P0 must provide:

- A pinned headless rigid-terrain demo build/run with commands, environment,
  asset provenance, logs, exit codes, and missing platforms recorded.
- A Rust harness starting/stopping the worker, negotiating capabilities,
  sending inputs/step intervals, and receiving finite endpoint state.
- Repeat-run trajectory differences with declared tolerances and step-refinement
  sensitivity; separate repeatability, physical accuracy, and real-time factor.
- Basis-vector, quaternion/inertia, SI, frame/recenter, and run-time/epoch tests,
  including reset generations and invalid/protocol-failure cases.
- Crash/timeout cleanup evidence, packaging inventory, measured costs, and a
  review of missing admission, frame, port, and causal-barrier mechanisms.

Rust tests cover pure conversion/protocol seams; the harness covers P0 process
behavior. Future integration needs positive and negative authored Rhai verdicts
through production `luncosim`, including duplicate ownership and unsupported
cross-engine contacts. This documentation PR satisfies none of those runtime gates.

Conclude P0 with go/no-go and a supported-platform matrix. Only a reviewed follow-up
may scope live rigid terrain, SCM, or sensors. Ray-traced Sensor requires a separate
compatible NVIDIA/CUDA/OptiX setup; consult the pinned release's
[installation documentation](https://api.projectchrono.org/module_sensor_installation.html).
A CPU build does not establish GPU capability.
