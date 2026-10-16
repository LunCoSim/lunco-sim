# Upstream-only physics integration review

Status: regular `cargo run` compiles and launches against stock dependencies.
Joint-drive and coupled/uncoupled differential gates pass. Griffin loads and
advances; sustained contact and performance qualification remain incomplete.
2026-10-16.

The five local dependency patches and their source directories are removed in
the application checkpoint. This is not a fully qualified replacement yet. Do not
merge this removal until the application builds and the production gates pass.

No new remote repositories or dependency branches were created. Rumoca's
previously authorized main-branch dependency and the existing transform-gizmo
dependency are outside this removal.

## Decision

Use unmodified upstream dependencies. Keep application-specific clock,
composition, ordering, drive realization, ownership and presentation contracts
at their existing LunCoSim owners. Reuse public native kernels and extension
points. Do not copy the removed dependency implementations into a new module.

Actual errors inside a collision kernel require a qualified upstream correction
or an explicitly reviewed backend change. Rejecting invalid results protects
the simulation, but does not make the underlying query correct or establish
mission acceptance. Never turn a failed query into clear space, invent a
contact normal, or repeatedly change seeds until a query returns an answer.

User approval is required before any future dependency source modification,
vendoring, fork creation or publication of dependency patches.

## Scope and evidence

The audit compares the tracked source at `5f954bc55` with the corresponding
unmodified registry packages. Thirty-one Rust files differ beyond trailing
whitespace: Avian 17, Parry 10, Bevy Render 1, interpolation 2, glam 1. Parry's
feature manifest also differs. Complete comparison receipts are under
`target/dependency-audit-*.patch` and `target/dependency-audit.json`.

| Package | Registry version selected after removal | Purpose of the local changes |
| --- | --- | --- |
| `avian3d` | 0.7.0 | Motors, repeated hard constraints, solver sequencing, reaction reporting, damping, mass ownership, collider insertion and worker dispatch |
| `parry3d-f64` | 0.27.0 | GJK correction, touching witnesses, point projection, motion precision, early certificates, reusable EPA scratch and scalar feature selection |
| `bevy_render` | 0.19.1 | GPU output allocation stride under scalar math |
| `bevy_transform_interpolation` | 0.5.0 | Bounded presentation fraction during retained fixed-step backlog |
| `glam` | 0.32.1 | Aligned-mask serialization under scalar math and fourth-lane serialization |

`cargo check -p lunco-physics -j 4 --offline` and the same command with `--tests`
both exit 0 against the registry dependencies. Receipts:
`target/upstream-physics-projection-check.log` and
`target/upstream-physics-tests-check.log`. This verifies the physical owner and
its test code, not a full application build or execution of all physics tests.

`cargo check -p lunco-luncosim -j 4 --offline` also exits 0 after this worktree's
dev outputs were cleaned following disk exhaustion. Receipt:
`target/upstream-application-check.log`. The plugin-composition and reaction
cutover also passes the same full application check in
`target/upstream-owned-xpbd-check.log`. These checks include the contact-provenance
resolver and Rhai reader changes. They are source/API checks, not a linked binary,
`cargo run`, scene load, performance result or dependency-kernel qualification.

Regular `cargo run -j 4 -- --api 4384 --no-vsync --no-throttle` subsequently
compiled and launched the windowed application. Opening the exact Twin root
`/home/rod/Documents/models/lunar-base-model/twins/astrobotic-griffin-1` completed
scene readiness and coupled initialization. Nine Modelica participants ran with
the default 1/60-second communication period; three environment probes remained
native Idle participants. A captured and inspected View-perspective image shows
the lander and terrain. Receipts: `target/upstream-cargo-run-resumed.log` and
`target/upstream-griffin-cargo-run/`.

This is a loading result, not sustained vehicle acceptance. At approximately
87.2 seconds of simulation time, the continuous-contact owner faulted:
`native continuous initial contact did not return contact geometry for its
enclosing prediction`, composite member 763. The Modelica times were aligned
within 4e-12 seconds at the fault. The structured safety hold is required, but
does not repair that query. Exact leaf shapes and the relative pose are needed
to compare the application-seeded query with the standard native query before
assigning the cause to a dependency. No 200 FPS or sub-millisecond physics
result is claimed.

A diagnostic-only rebuild also passes regular `cargo run`. Two bounded reruns,
one with View active and one reopened into Editor, did not reproduce that
contact fault; the last read reported ready with nine running models at 217.25
seconds and a maximum inter-model spread below 7e-11 seconds. This does not
invalidate the first failure. The initial-query diagnostic now retains leaf
geometry, relative pose, prediction, initial direction and native result on
failure. Receipts: `target/upstream-contact-reproduction/` and
`target/upstream-contact-editor-reproduction/`. The owned diagnostic session
exited through API `Exit`, and port 4384 was verified closed.

The pre-cutover headless `native_force_drives` baseline failed at tick 2 with
`avian-backend-pose-invalid` on `/World/ComputedBob`: its solved X coordinate
reaches approximately -3.85e77 metres. Receipt:
`target/upstream-native-force-drives.log`. This confirms that the stock motor
mode cannot qualify the authored dimensional-drive contract; a successful
Griffin load is insufficient acceptance for the cutover.

A stock Parry probe also checks the public default contact query independently
of the application seed. For a triangle and cuboid touching only at
`(0, -2, -2)`, both default and seeded queries return zero witnesses. The second
witness is outside the cuboid, as verified by its public point-membership
query. The regression assertion fails against unmodified Parry 0.27.0.
Receipts: `target/upstream-native-contact-contract-probe.rs` and
`target/upstream-native-contact-contract-result.json`. This is a separate
demonstrated kernel defect; it does not establish the first Griffin fault's
cause. No retry or alternate geometry is admitted into the runtime.

`cargo build -p parry3d-f64 -j 4 --offline` succeeds. A standalone numerical
probe linked to that unmodified registry library builds successfully and exits
1, reproducing both:

- Coincident cylinders with half-heights/radii `(50, 100)` and `(1.5, 1)` return
  `intersection_test = false`, although the smaller cylinder lies inside the
  larger cylinder.
- A cylinder with half-height `0.1`, radius `0.2`, queried at its center with
  `solid=false`, returns boundary distance `0.2`. Its nearest boundary is a cap
  at distance `0.1`.

Receipts: `target/upstream-query-probe.rs`,
`target/upstream-query-probe-result.json` and
`target/upstream-query-probe.log`. These are dependency math probes, not runtime
or vehicle acceptance. They make an unconditional promise that every defect can
be repaired by an application wrapper incorrect.

### Test stock behavior before replacing orchestration

Application calls to a custom API do not prove the custom mechanism necessary.
Qualify the supported native configuration before designing a replacement.
Do not treat every removed patch as a requirement to reproduce elsewhere.

The stock Avian 0.7 prismatic solver was exercised directly with prepared joint
frames, a static endpoint and unit dynamic inverse mass, without application
scheduling or scene projection. At stiffness 20 and target displacement 1:

| Native mode | Duration | Position change | Implicit dimensional expectation |
| --- | --- | --- | --- |
| `ForceBased` | 0.01 s | 0.2 m | 0.001996008 m |
| `SpringDamper` with equivalent natural frequency | 0.01 s | 0.001996008 m | 0.001996008 m |
| `ForceBased` | 0.02 s | 0.4 m | 0.007936508 m |
| `SpringDamper` with equivalent natural frequency | 0.02 s | 0.007936508 m | 0.007936508 m |

With a zero force cap, the enabled force-based motor still moves by 0.2 m at
0.01 s; explicitly disabling the motor produces zero motion. The probe exits 1
for the failing dimensional/cap cases, while asserting the spring-damper
control cases. Receipt: `target/upstream-avian-drive-result.json` and
`target/upstream-avian-drive-probe.log`. This isolates a specific native mode;
it does not establish a general solver failure or a vehicle result.

The native spring-damper mode is usable when its frequency/damping-ratio
contract matches the authored requirement. This probe does not authorize
reinterpreting arbitrary USD stiffness/damping that way: effective inertia can
change, and pure damping needs its own valid representation. Zero authored
force/torque can be represented by a disabled native motor at the owning
admission boundary. Per-timestep gain rescaling is not a replacement for
dimensional semantics.

### Application API work and qualification

`joint_projection::JointProjection` prepares hard geometric channels from the
public joint definitions for fixed, revolute, prismatic, spherical and distance
joints. One fixed-size row representation consumes native point-frame
preparation and positional/angular impulse kernels. The ordinary ordered pass
integrates motors and compliant channels; repeated hard closure and CCD consume
only these prepared geometric rows. Preparation and solving use the same stable
joint order. An invalid candidate returns without either endpoint write.

Six actual source-module tests pass: mixed soft translation/hard rotation,
noncommuting world joint frames, motor exclusion across repeated closure,
independent slider alignment/travel compliance, distance and spherical limits,
and invalid/unprepared geometry without partial writes. Receipt:
`target/upstream-joint-projection-test.log`. The spherical rows follow the public
twist-axis/cone contract. Ordinary native joint solving still requires numerical
qualification; these kernel tests do not establish agreement of both passes.

The shared headless/headful physics composition disables `XpbdSolverPlugin`.
The application joint owner installs native public preparation kernels, stable
ordinary drive and geometric passes, hard closure, f64 velocity projection and
one reaction finalization path. Projection precedes contact relaxation. The
native contact solver remains installed. Its bundled passive damping systems
are removed through Bevy's public schedule API because Avian exposes no separate
plugin for selecting that damping implementation.

Reaction finalization includes native and hard-closure multipliers using the
actual physics-step and substep durations. All joint types stage their reports
before a single commit, preserving the published reports if any candidate
fails. Sleeping endpoints retain their published reaction; disabled joints
clear theirs. `JointForces` owns the completed report without a duplicate cache.
The joint owner installs the common runtime-fault boundary for native body and
contact writeback, including faults raised during reaction preparation. Passive
damping impulses still need inclusion. No production reaction verdict is
claimed yet.

The native-schedule seam regression
`reaction_failure_refuses_the_entire_completed_step_writeback` first completes
a valid step, then injects an invalid internal multiplier after substeps. One
changed valid reaction is staged before the later invalid joint. With a grouped
run condition, this test fails: native finalization writes a changed body
velocity despite the fault. With a separate condition per guarded phase, it
passes, retaining both bodies and both published reactions, pausing physics
with zero delta and naming the invalid joint. Receipts:
`target/upstream-joint-finalization-baseline.log` and
`target/upstream-joint-finalization-test.log`. This tests internal lifecycle and
writeback safety, without implying an authored vehicle or performance verdict.

The compiled physics test binary also passes 17 joint, damping, precision,
hard-closure and continuous-contact writeback tests in one bounded run
(`target/upstream-physics-owner-tests.log`). The real USD joint consumer passes
`admitted_joint_detaches_without_solver_island_corruption` with the same plugin
composition (`target/upstream-joint-detach-test.log`).

Discrete manifold part identities are explicitly optional. Known continuous
parts retain their indices; a non-compound's provenance identifies its one
source. An unknown discrete compound member returns no source path, never a
guessed part zero. The physics snapshot, provenance resolver and Rhai reader use
this same contract.

`lunco-physics::joint::revolute_coordinate` measures the signed f64 coordinate
from public local joint bases and the hinge axis. Co-simulation, mobility and
the joint panel now share this owner. It reuses backend rotation admission and
the existing axis projection. No dependency source change is required.

The actual source-module test
`joint::tests::revolute_coordinate_uses_both_joint_frames` passes against stock
Avian, covering distinct joint bases, nontrivial body orientation, signed
sub-nanoradian angles, quaternion hemispheres and invalid inputs. Receipt:
`target/upstream-joint-coordinate-result.json`. The isolated module check does
not prove a linked or running application. No production acceptance or
performance improvement is claimed.

`rotation_displacement` supplies the CCD trajectory and material-witness math
with an admitted f64 rotation logarithm. Native glam 0.32.1
`to_scaled_axis()` has a small-angle threshold unsuitable for those derivatives.
The actual source-module regression passes for signed small increments, norm
drift, quaternion hemispheres and invalid-frame rejection. Receipt:
`target/upstream-angular-displacement-result.json`. Invalid frames propagate
through the continuous owner's existing error/writeback-refusal path.

The contact path now uses Parry's published parameterized contact interface.
The EPA pool and threaded scratch parameters are removed. Scoped queries retain
prepared result order, including the serial path. Dependency-only early-skip
APIs and their cache state are removed with their implementation-specific tests.
Geometry correctness regressions remain; this cutover does not establish that
the selected upstream kernels satisfy them.

`joint::integrate_joint_damping` owns passive pair damping. The USD coefficients
are rates in 1/s, distinct from dimensional drive damping. It integrates their
point-velocity decay followed by angular-velocity decay once per physical
substep. The ordered owner derives one common point at the midpoint of the
current joint anchors. Equal/opposite impulses include their moments at that
point, preserving total angular as well as linear momentum for free endpoints.
Native locks and dominance restrict the movable subspace. The point response
uses maintained f64 eigenvalue/eigenvector kernels with an isolated first root;
its numerical rank threshold is 32 machine epsilons after response normalization.
Zero mobility is a physical no-response channel. Invalid or unrepresentable
state returns before either endpoint write, through the ordered owner's safety
hold and runtime-fault boundary.

The five actual source-module tests cover unequal anisotropic tensors,
linear/angular momentum including offset-anchor orbital moments, decreasing
kinetic energy, exact individual rate flows, convergence of their split
composition under subdivision, rank-one point response, locked/dominant
endpoints and invalid-input atomicity.
Receipt: `target/upstream-joint-damping-test.log`. This is numerical kernel
evidence against stock Avian, not a production runtime or performance verdict.
The duplicated damping assertion in CCD is removed; damping has one test owner.
The existing production `native_force_drives` Rhai gate now asserts the
exponential passive-rate contract. It has not run against this cutover. Rust owns this continuous numerical
mechanism; it has no changeable lifecycle or selection policy requiring a hook.
The final reaction owner must also include passive damping impulses in completed
joint-force reports. That telemetry cutover is not implemented or qualified yet.

## Targeted custom integration

Avian explicitly supports application integrators through
[`CustomVelocityIntegration` and `CustomPositionIntegration`](https://docs.rs/avian3d/0.7.0/avian3d/dynamics/integrator/index.html).
The current kinematic path already uses custom position integration. Reuse
these interfaces when a body actually requires a different numerical method.
Custom velocity integration assumes responsibility for gravity, damping and
external forces; native contact/joint impulses still participate. Custom
position integration owns the solver-body position/rotation increments in the
native substep schedule. Neither path writes render transforms as physics state.

A stiff two-body drive usually needs a targeted constraint/impulse integration
step, not a replacement body integrator. Its effective inverse inertia includes
both endpoints, anchor offsets, locked axes and dominance. The existing gear
drive calculation is a reuse candidate, but must be qualified before promotion
to the shared owner. The audit found that acceleration mode clamped acceleration
to `max_force` before dividing by inverse mass. It now converts to force before
applying the physical cap, bounding impulse by `max_force * substep_dt` in both
modes. The new pure regression covers zero/positive caps and four inverse masses.
It fails on the exact baseline function and passes on the current function:
`target/gear-cap-regression-result.json`. At inverse mass 0.01, the baseline
applied impulse 0.208333333 instead of the allowed 0.002083333. This was an
application-side units error. The isolated source-function test does not replace
production runtime qualification; invalid-numeric error handling also requires
review before this calculation becomes the shared integration owner.

Keep the stock integration path for supported ordinary bodies. A selected
custom numerical method has an explicit typed owner and one application per
physical substep, in stable constraint order. It consumes physics time and the
same lifecycle in headless and headful hosts. Do not select it by Twin name,
renderer mode, wall-clock timing, worker completion or a silent failure retry.
Numerical invalidity must retain the existing diagnostic/writeback-refusal
boundary. Qualify force caps, pure damping, unequal/locked/kinematic endpoints,
momentum, energy behavior and timestep refinement before vehicle acceptance.

Custom integration can address an unsuitable drive/body integration method;
it cannot repair an incorrect collision query or establish FMU interoperability.
The shared master clock, FMI participant capabilities and rendering ownership
remain separate integration responsibilities.

## Remove the feature cascade

The local Parry manifest added `glamx/scalar-math` to enhanced determinism.
Cargo feature unification applies that choice to the same shared math package
used by Bevy. The glam serialization and Bevy shader-stride patches addressed
failures exposed by that global choice.

The upstream-only resolved feature tree retains Avian/Parry enhanced
determinism, libm and glam serde, without `scalar-math` on glam 0.32.1.
Receipt: `target/upstream-glam-features.txt`.

Keep that supported feature set. Qualify replay for an exact binary, numeric
profile and admitted input stream. Do not advertise cross-architecture bitwise
replay from a feature name. Do not disable GPU culling or shadows to compensate
for an incompatible global math feature. The ordinary renderer still needs a
fresh windowed build and GPU validation after the cutover.

The upstream fourth-lane serialization defect is separate from scalar math.
The searched application source has no direct `BVec3A`/`BVec4A` consumers. That
does not prove every reflected serialization path safe. Canonical simulation
and persistence should continue using their existing typed scalar/vector
contracts; add a boundary-specific test if an aligned mask is actually persisted.

## Application ownership and supported integration

| Concern | Existing owner and public upstream surface | Required application change |
| --- | --- | --- |
| Revolute coordinate | `lunco-physics::joint`; native local bases and hinge axes | One f64 joint-frame measurement shared by co-simulation, mobility and the joint panel. Use signed `atan2`, finite/rigid-frame admission and the native coordinate convention. No dependency method is needed. |
| Rotation increment | `lunco-physics` trajectory/velocity projection; glam quaternion APIs | Use one admitted shortest-rotation convention with explicit zero, sign and small-angle handling. Qualify its derivative against the actual position integrator. Do not create separate CCD and velocity-projection formulas. |
| Solver composition | `lunco-luncosim-simulation` plus `lunco-physics::joint_solver`; Avian plugin group and public XPBD functions | Select plugins at composition. Replace the required XPBD orchestration as one application plugin using public kernels; remove individual-system-name surgery and count assertions. Keep contact relaxation, joint/CCD closure and velocity projection in one reviewed substep graph. |
| Dimensional joint drives | `lunco-physics` drive realization; public solver bodies, effective inertias and constraint impulse APIs | Use the shared implicit coordinate drive in `lunco-physics::drive` for native joints and multi-body gears. Keep authored force/acceleration units, both endpoint inertias, anchor velocities and physical caps. Disable the native motor for any drive owned by this path. Targets, ports and telemetry must consume the same typed drive state. |
| Repeated contact/joint closure | `joint_projection::JointProjection`, CCD and ordered joint owners; public point preparation and positional/angular impulse primitives | One prepared row representation selects only zero-compliance geometric channels. It uses the shared f64 rotation displacement for world joint frames. Drives and compliant channels retain their single ordinary integration lifetime. Qualify the ordinary and closure passes together; numerical kernel tests alone do not prove that composition. |
| Joint reactions | Ordered physics-step owner; public solver-data multiplier accessors | Accumulate actual applied impulse once per substep and divide by completed physics duration. Include application-owned drive and closure contributions. Preserve sleeping results and explicitly invalidate disabled/retired joints. One typed result feeds ports and diagnostics. |
| Joint damping | Ordered substep owner; native solver-body and effective-inertia contracts | First determine whether the authored damping is a drive coefficient or passive joint damping. Drive damping uses the single drive realization. Any independent pair damping must conserve momentum, respect locked/kinematic axes and have one ordered application. Do not layer another damping pass over the native pass. |
| Body mass ownership | USD-to-physics projection; native `MassProperties`, shape mass calculations and `NoAuto*` markers | Aggregate admitted collider membership up to the nearest body boundary, in source order. Publish mass/inertia/COM coherently through the supported explicit-mass path. Preserve USD overrides and density semantics. Invalidate on topology, geometry, density and body changes, not per frame. |
| Collider insertion/scale | USD-Avian admission and fixed preparation; `ScalableCollider`, `ColliderTransform`, public scale preparation | Admit coherent physics-local geometry and scale together before native preparation. Establish an insertion transaction that cannot read a stale render `GlobalTransform`. Verify constructor scale, cooked shape, bounds and mass together, including headless operation. An after-the-fact visual repair is insufficient. |
| Contact source identity | Physics diagnostics plus USD collider provenance; native manifold and application CCD facts | Keep collider/body identity authoritative. Use known CCD part identities. Upstream Avian 0.7 does not retain Parry subshape ids in its public manifold. Unknown native compound members must remain explicitly unavailable. Never label a guessed part zero. Exact native member evidence requires a supported contact-generation/backend extension or an upstream API enhancement. |
| Transform interpolation | Physical presentation owner; public easing states and custom easing backend | Capture native endpoints once and consume `SimulationPresentationTime.interpolation`. Select one easing backend at plugin composition. No mutation of `Time<Fixed>` to satisfy presentation. Preserve the independent Interaction clock for UI/avatar easing. |
| EPA allocation scratch | Native contact-query interface | Use the published six-argument contact entry point and remove the application EPA pool and threaded parameters. Keep parallel read-only geometry queries and prepared result order. Measure actual allocation cost before considering an upstream API request. |
| Query skip certificates | Existing CCD owner; native supports, poses and bounding geometry | Remove dependency-only optimization calls. Retain only proofs whose bounds cover the actual upstream motion evaluation and contact criterion. Compare admitted contacts/trajectories before claiming a performance improvement. |
| Native body batch threshold | Bevy task-pool configuration and existing application scheduling | Remove the custom Avian setting. Profile stock dispatch first. Use existing host thread profiles for measurement; do not globally force one worker or copy native integrator systems merely to avoid dispatch. A missing native batch knob is an optimization opportunity for upstream. |

### Two incorrect shortcuts ruled out

Calling an ordinary native joint solve repeatedly with motors disabled is not
equivalent to hard-only closure. Upstream point/distance/prismatic code passes
zero as the prior multiplier to its compliant update. Repeating it changes the
effective compliant response. Therefore a simple clone-and-disable-motor wrapper
does not preserve the current physical contract.

Adding reversed ordering constraints to the already installed upstream XPBD
sets can create a schedule cycle: upstream orders them after relaxation. The
application needs explicit plugin composition, not contradictory schedule edges.

## Collision-kernel boundary

The current Avian release and inspected upstream development manifest both
depend on Parry `0.27`. The newer published Parry `0.31.1` uses glamx `0.3`,
where the current package uses `0.2`; it is not a patch-compatible version
substitution. An unrestricted Cargo update will not perform this migration.

Upstream documents maintained GJK fixes in its
[changelog](https://github.com/dimforge/parry/blob/master/CHANGELOG.md).
That does not establish that every touching/EPA or projection case is fixed.
Qualify each regression against the exact selected upstream version.

Preferred resolution: obtain compatible upstream corrections, then consume an
upstream release or explicitly reviewed unmodified upstream revision. Prepare
minimal reproducers locally; publishing issues or patches needs explicit user
authorization. A full collision backend replacement is a separate architectural
decision with a substantial surface, not a small adapter cleanup.

Until qualified, retain the existing runtime-fault and same-tick writeback
refusal boundary. It protects state but may stop a real mission. Neither that
hold nor disabling a regression test counts as fixing the kernel defect.

## Co-simulation, clocks and rendering

Dependency cleanup must preserve one fixed-time owner for physics, Modelica,
simulation timers and simulation-clock Rhai. Pause admits no physics steps;
speed changes affect wall-to-simulation demand, not solver `dt`. Default
communication remains one master physics tick. Explicit slower rates are
validated integer multiples; the current Modelica macro-step bound must remain
visible rather than claiming arbitrary-frequency support.

FMI supplies participant interfaces and capabilities; the importer chooses the
coupling algorithm. Its
[co-simulation contract](https://fmi-standard.org/docs/3.0.2/#co-simulation)
does not itself guarantee a zero-delay or converged inter-participant solve.
Direct-feedthrough dependencies, explicit held inputs, algebraic loops and
rollback capabilities need explicit master semantics. Do not silently remove a
sample delay, add one per connection, or call a native participant facade proof
of external FMU interoperability. FMU packaging and interoperability require
their own executable acceptance.

The current fixed runner executes complete `FixedMain` cycles on the host World
and can drain 64 cycles per host update. Separating Bevy's render sub-app alone
does not make the UI independent of that work.

For independent presentation, the existing deterministic-runtime boundary must
grow to one authoritative simulation owner with typed ordered commands and
immutable committed presentation snapshots. UI/render may consume the latest
snapshot without waiting for the next simulation step. Intermediate presentation
snapshots may be coalesced; authoritative ticks and inputs may not be dropped.
Scene transactions, query execution, script scope, timers and teardown need the
same ownership treatment. Headless and headful hosts consume the same step
service. This is an application architecture change, independent of dependency
forks, and must not be attempted by evaluating the live World concurrently.

Existing diagnostic evidence demonstrates this scheduling concern but is not
performance acceptance for the upstream cutover. The pre-cutover unprofiled
private-display reference completed 16 mission checks and 602 ticks in
10.031944 seconds, with median native physics 2.237639 ms and complete tick
4.327181 ms. It ran on battery alongside another application. Its frame median
33.630049 ms is not desktop FPS acceptance. See
`target/current-main-isolated-normal-summary.json`.

The separate Tracy window records four host fixed-loop invocations in seven
seconds, with median lifetime 1821 ms. Profiler overhead and contention affect
that magnitude; source confirms the synchronous ownership. Nested span times
are not additive. The artifacts do not establish 200 FPS or physics below 1 ms.

## Implementation and qualification order

1. Establish stock behavior for the relevant published configuration before
   preserving custom machinery. Complete the upstream API cutover without
   changing physics semantics:
   shared coordinate math, removal of EPA scratch plumbing, presentation
   endpoint consumption and dependency feature verification.
2. Implement the reviewed physics composition and ownership changes as bounded
   slices: hard geometry/drive lifetimes, reactions, explicit mass and collider
   admission. Resolve each genuine kernel regression before vehicle acceptance.
3. Run the existing authored drive, contact, landing-impact and rover-leg gates
   on the same production runtime in headless and headful modes. Include zero
   caps, unequal/locked/kinematic inertias, soft channels, nested bodies,
   collider removal, initial touching, finite edges and large coordinates.
   Reject invalid inputs visibly without process crashes.
4. Preserve exact ordered-input replay for each qualified execution profile.
   Test pause, resume, rate changes, timers, restart and explicit communication
   periods through production Rhai. Keep cross-platform replay claims bounded.
5. Establish the simulation/presentation ownership boundary, then profile the
   actual remaining application costs with `../tracy`. Run separate unprofiled
   FPS and full-physics measurements with power/workload context recorded.
6. Remove stale vendor documentation and retired APIs/tests together with each
   completed replacement. Update architecture 19, architecture 62, the crate
   index and affected skills. Commit only working, verified slices; preserve
   the newest target-branch changes during integration.

This review defines the cutover and its remaining qualification work. The
current application compiles, loads Griffin and passes the drive/differential
gates; sustained vehicle, collision-kernel and performance acceptance remain
open.


## Shared coordinate-drive realization

`lunco-physics::drive` owns the implicit force/acceleration calculation for both
ordinary joints and mobility gear relations. For freely drifted coordinate q,
rate v, mobility w and actual substep h, a dimensional drive applies
`J = h * (k * (target - q) + d * (target_rate - v)) /
(1 + w * h * (d + k * h))`, bounded by the physical force limit times h.
Acceleration-based and frequency-based models normalize gains by mobility;
the cap remains physical force/torque. Free drift is not predicted twice.

The public Avian solver body/inertia and angular increment APIs apply Jacobian
position increments. Shared f64 velocity projection realizes their velocity
change once. Effective mass includes locks and dominance; numeric admission
validates every endpoint candidate before any solver body is modified.
Ordinary joint motors run once and do not warm start. The local native joint
definition passed to the remaining compliant/geometric solve has its motor
disabled. Repeated hard closure never reapplies a drive. Completed reactions
include the physical drive impulse over the completed physics duration.

Gear state belongs to its authored gear prim. Projection retains both explicit
hinge relationships, resolves them within the current stage/instance, and
prepares stable authored order once per physics step. The gear consumer uses
the same coordinate-drive kernel and position-to-velocity projection as joints.
An invalid admitted gear faults and holds physics instead of disappearing or
selecting an arbitrary matching hinge.

This numerical mechanism has no changeable scenario policy or Twin-specific
hook: USD/ports own the authored law and targets, and Rust owns its continuous
math and solver invariants. Qualification uses the existing authored
`native_force_drives` and positive/negative `differential_rig` gates.

Gear and ordinary joint projection share `lunco-usd-avian-reader::joint::read_drive`.
Angular targets use radians; per-degree gains convert to per-radian gains.
Force-based angular gains and physical torque caps carry squared stage length;
acceleration-based gains do not carry stage length. No gear-specific scalar
parser or drive-type reader remains.

The coordinate-drive slice passes regular `cargo run -j 4` and the production
`native_force_drives` (18 checks), coupled `differential_rig` (5 checks), and
uncoupled control (5 checks) gates. All exit 0; receipts are
`target/coordinate-final-gates.json` and `target/coordinate-final-*.log`. Six
coordinate-kernel tests and the four-case force/acceleration, metre/centimetre
schema conversion test pass (`target/coordinate-drive-mechanism-tests.log` and
`target/coordinate-drive-usd-units-test.log`). The unlimited-cap assertion uses
`is_infinite()` to test the actual contract; the prior magnitude comparison
reported false for the observed positive infinity in Rhai. Contact-kernel,
sustained vehicle and performance qualification remain separate open work.

## Bounded physical presentation

The fixed runner retains pending authoritative cycles. Native interpolation's
raw `Time<Fixed>::overstep_fraction()` may therefore exceed one while the host
catches up. Griffin reproduced a non-unit dish render quaternion after that
extrapolation; canonical simulation admission correctly rejected it.

`lunco-time::simulation_presentation_fraction` is the shared owner of the
completed-state interval for both presentation time and physical rendering.
`lunco-luncosim-simulation` selects the maintained interpolation library's public
custom-easing markers and consumes its public endpoint states before its easing
bookkeeping. Native capture, invalidation and FixedFirst restore remain installed.
Pause presents the completed endpoint; backlog bounds presentation at that
endpoint without consuming or discarding simulation time. Invalid endpoints
raise a structured runtime fault and hold physics before asserting quaternion
math. This is continuous render math with no changeable policy hook.

Three generic composition/lifecycle seam tests pass, including retained backlog,
pause, canonical-position preservation, native restore, and malformed-endpoint
faulting (`target/bounded-physics-presentation-tests.log`). Regular `cargo run`
compiles and launches. The production force-drive and coupled/uncoupled gear
gates pass 18/5/5 checks (`target/bounded-presentation-gates.json`).

The owned headful Griffin run reports touchdown and ramp deployment and advances
past 337 simulation seconds without the reproduced rotation fault. Interactive
user inputs occurred during this run; it is not an unattended mission verdict.
A separate lightweight sample advances 10.016667 simulation seconds in 10.025039
wall seconds (`target/bounded-griffin-light-wall-progress.json`). The inspected
image `target/bounded-griffin-view.png` retains lander, rover, ramp and terrain.
Its approximately 65 FPS and 1.3–1.6 ms native physics timings do not meet the
performance target. Full UI/simulation ownership separation, collision-kernel
qualification and external FMU interoperability remain open.
