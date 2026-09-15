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

## Rust alternatives: evaluate before adopting Chrono

Source review dated 2026-09-15; candidates have not been built or benchmarked here.
No complete Rust equivalent of the paper's integrated vehicle/soil/sensor stack
was established by this review. That does not mean each needed capability requires
Chrono. Select the smallest missing capability before selecting an engine.

| Option | Evidence and reusable capability | Fit and remaining work |
|---|---|---|
| Existing Avian + LunCoSim | Workspace uses Avian 0.7 with f64/Parry; `lunco-mobility` already supplies contact-plane traction, spring/damper suspension, joint wheels, rocker-bogie coupling, and Modelica/oracle reference cases | First baseline for rigid-terrain rover work. Retains USD, ports, frame, and time ownership. Existing tests are not lunar-soil validation |
| [Rapier](https://github.com/dimforge/rapier) | Rust rigid-body engine with [reduced-coordinate multibody joints](https://rapier.rs/docs/user_guides/rust/joint_constraints), useful for articulated robotics | Evaluate only for a measured joint/solver limitation. Separate state/handles still need projection and ownership work; Rust removes the C++ boundary, not integration cost. Not a turnkey SCM/excavation/sensor stack |
| [Parry](https://github.com/dimforge/parry) | Rust geometric queries and collision detection; already used through Avian's Parry feature | Reuse geometry capabilities when needed; not a dynamics or soil constitutive solver |
| [Salva](https://github.com/dimforge/salva) | Particle fluids with DFSPH/IISPH, viscosity, elasticity, optional Rapier coupling, and advertised WASM support | Potential fluid/particle research component. Fluid SPH is not Chrono CRM regolith: plasticity, soil calibration, Avian coupling, precision, and current dependency compatibility need evidence |
| [Sparkl](https://github.com/dimforge/sparkl) | Rust MPM code with particle/material simulation and a PTX build path | Research candidate for deformable material. Inspect precision, CPU/GPU paths, toolchain, constitutive models, maintenance, and platform support before adoption. Do not assume Rust implies browser or portable GPU support |
| Focused in-house soil extension | Existing mobility force application, terrain substrate, and Modelica reference machinery | Candidate for bounded pressure-sinkage/shear behavior without a second rigid-body solver. It is a new numerical model requiring calibration, not a small rewrite of all Chrono |

The source inventory is anchored in `Cargo.toml`,
[`lunco-mobility`](../../crates/lunco-mobility/README.md), and
[terrain substrate](terrain-substrate.md). The scoped mobility/terrain-core search
did not identify a Bekker/Janosi/SCM implementation. Inspect wider owners before
implementation; lack of a name match is not proof that no equivalent exists.

### Bounded Rust implementation candidate

First define the requested output: wheel sinkage and drawbar pull/slip on a
restricted soil regime, not excavation or arbitrary granular flow. A prototype
could evaluate Bekker pressure-sinkage and Janosi-Hanamoto shear laws using
authored soil parameters. The equations are compact; the difficult work includes
contact-patch integration, persistent soil/shear state, unloading/reloading,
overlapping wheels, terrain deformation, numerical stability, and calibration.
Do not estimate complexity from the number of equations or code lines.

Keep Avian as rigid-body owner, terrain state at the terrain owner, and the selected
wheel/soil force path at mobility. Replace the relevant hard-ground reaction in
the selected mode instead of adding soil force on top of existing traction or
collider response. Modelica owns reference equations and suitable continuous
subsystems; Rust owns spatial queries and hot numerical mechanisms; Rhai owns
experiment policy. Review this ownership before introducing any state or schema.

Rendering-only improvements (lunar reflectance, shadows, or cosmetic dust) should
be scoped through existing rendering/material owners. They do not establish
camera calibration, sensor noise fidelity, or physically validated dust transport.
No native Rust sensor library equivalent to Chrono::Sensor was established here.

### Required comparison gate

Before choosing Chrono, record a capability gap and compare the existing baseline,
a bounded Rust extension, relevant Rust libraries, and Chrono on the same authored
experiment. Evaluate physical error against independent reference data, timestep
and grid convergence, runtime/memory, f64 and WASM support, ownership complexity,
license/dependency burden, and maintenance effort. Pin candidate revisions;
validate current toolchain support rather than treating README claims as tests.

For soil, use plate sinkage and a single-wheel sweep over load/slip with calibrated
parameters, then a rover case. Predeclare tolerances and negative cases. Agreement
with Chrono alone is cross-checking, not experimental validation. Approve a focused
Rust implementation if it meets the required regime and is cheaper to maintain;
retain Chrono only for a demonstrated capability/accuracy benefit. Large-strain
excavation, general DEM/MPM/SPH, and calibrated sensors remain substantial projects.

This alternatives review precedes an adoption decision. The existing Chrono P0
build can supply comparison evidence; a successful build does not select Chrono.

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

## Fork workflow and commit/merge requirements

Develop and validate the integration on a topic branch in a LunCoSim fork, then
submit reviewed PRs upstream. The existing architecture remains the core simulation
contract. Fork Chrono itself only if a demonstrated upstream change is necessary;
otherwise pin and build upstream Chrono externally. Do not copy its checkout into
LunCoSim. Keep documentation, the P0 experiment, and later runtime adoption as
separately reviewable changes.

The C++/Rust boundary is a primary engineering risk. P0 must record compiler and
runtime compatibility, process startup/library discovery, serialization precision,
ownership/lifetime errors, failure cleanup, and packaging behavior. Process isolation
avoids exposing C++ ABI types to Rust but does not remove those costs. If a later
FFI path is proposed, it needs explicit allocation/deallocation and exception
containment rules and its own evidence. Schedule follow-up work from measured P0
results rather than assuming that linking two languages is trivial.

For every integration commit and merge:

- Commit source code, documentation, authored textual fixtures, manifests,
  lockfiles, and reproducible build recipes. Do not commit compiled binaries,
  generated build/install trees, downloaded payloads, archives, caches, or binary
  test/benchmark outputs. Git LFS is not an exception to this policy.
- Generate binaries from pinned source, or obtain external binary assets through
  the existing `lunco-assets` download/cache system with manifest URLs, hashes,
  provenance, and license metadata. Follow [asset I/O policy](40-asset-io.md);
  do not introduce an adapter-specific downloader. Keep asset acquisition separate
  from trusted executable activation.
- Put Chrono source downloads in ignored `extern/`, and CMake build/install/output
  trees under ignored `target/chrono/`. Release payloads belong in release/CI
  artifact storage, not Git. Commit textual summaries and links to large evidence.
- Stage explicit source paths and inspect `git diff --cached --stat`,
  `git diff --cached --numstat`, and the full staged diff. Use descriptive,
  focused commits with the relevant validation in the PR. Never force-add an
  ignored artifact to make a build work.
- Verify ignore rules with `git check-ignore` and inspect all commits in the PR,
  not only its final tree. `.gitignore` neither untracks existing files nor removes
  binaries committed earlier. Any introduced artifact must be absent from the
  proposed commit history before merge; coordinate history changes if needed.
- Require an automated source/artifact check for the implementation PR, including
  generated-path and binary-payload detection, and prove a fresh checkout can
  rebuild/download its inputs. An extension denylist alone is insufficient:
  executables can be extensionless and textual source can use misleading suffixes.

These are merge requirements, not claims that the future CI gate is implemented.
Existing unrelated repository binaries are outside this proposal's cleanup scope.

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
