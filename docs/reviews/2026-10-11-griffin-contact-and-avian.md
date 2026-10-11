# Griffin contact and Avian integration review

The accepted production artifact passes startup engine-off impact, powered
rover-leg collision, native drive, continuous-contact, startup-admission and
negative-owner gates. The bounded scripting contract also passes. This review
covers contact correctness, native drives, collider provenance and the document
operations needed to author that provenance. The requested 0.5 ms full physics
budget remains unmet; passing correctness gates does not establish it.

## Authoritative owners

`lunco-physics` closes continuous material-contact inequalities and affected
native hard-joint geometry inside the same Avian substep, before velocity
projection. It consumes actual solved COM motion in f64, authored CCD settings,
native collision hooks, stable collider identities and native composite BVHs.
Changed trajectories are revalidated before writeback. Compliant channels and
motors retain their ordinary native integration; closure does not repeat them.
The continuous owner pools unique leaves, computes each motion and whole-arc
bound once, sharing body motion across leaves, and uses Parry's native BVH
for candidate admission. The prepared
pair table retains hooks, layers and pair-specific dynamic-body eligibility;
stable pair indices determine result consumption and correction order. Pure
candidate queries use a synchronous Bevy compute-pool scope with immutable
shapes and trajectories, disjoint contact caches and per-chunk native EPA
workspaces. All chunks join before ordered diagnostics, failures and impact
admission can affect the solver. There is no worker access to the World or
worker-completion-driven state transition. One topology lasts for the
prepared step; corrected trajectories replace exact leaf bounds and refit
ancestors with zero change-detection margin. Refitting expands and shrinks
bounds. Exactly unchanged poses cannot create a new crossing; prepared native
inequalities still own their contacts. The symmetric hard-joint closure acquires one body query across all
forward/reverse native passes. Material-witness chord remainders certify clear
paths directly; uncertain paths retain the complete extremum search.
Initial contacts use native prepared geometry, positive-time impacts use valid
native cast witnesses, and mesh features use native normal constraints.
Unsupported geometry, ambiguous rotation increments and failed convergence
raise owner faults and refuse the affected tick's pose writeback.

An interior witness at a real finite triangle point can prove initial
intersection without its minimum translation depth. Native opposite cylinder
supports supply a plane-crossing chord witness when the center projection lies
outside a tilted cylinder. The actual finite triangle and native cylinder point
query must certify strict interior; touching and uncertain cases retain native
queries. If the conservative motion
bound cannot reach Parry's unchanged directional impact criterion, the proven
overlap needs no native query. Both consumers use Parry's criterion method;
initially separated shapes retain their future-impact geometry. Prepared
material-contact constraints keep their existing position tolerance. Changed
trajectories revalidate the displacement bound. The scoped dispatcher bounds
queries per worker through the CCD settings owner. Small batches execute the
same kernel directly, retaining prepared pair order.

Native support bounds include the entire rotational arc and changing support
features. Initial contact geometry is reused only while its starting geometry
is immutable in one substep. Every endpoint support gap is checked for finite
values before minimum reduction; a nonfinite endpoint cannot disappear through
floating-point `min` semantics. Parry's native GJK/EPA accepts the finite-feature
direction as its initial search direction for convex triangle pairs; it still
computes the depth. Finite edges, changing tilt, offset COM motion and changed
endpoints must retain their sweep when the full-arc bound cannot certify it.
Cylinder radial features supply another candidate separating axis, independent
of cap displacement. Full occupied-shape supports still own the complete arc
certificate. Successful axes are rechecked on each changed trajectory;
the cache cannot retain a stale clear-space result. Only candidate directions
survive later substeps in the same prepared physics step. Capturing starting
poses recursively invalidates overlap proofs, depths and native contacts;
the next physics-step preparation clears all pair caches. An immutable convex
query interval shares native endpoint
poses and local bounding spheres across all of its candidate-axis tests; every
changed sweep constructs a new interval. Internal identity lookups
use Bevy's fixed-hasher maps; prepared pair and part order remain explicit.
The continuous owner certifies a finite triangle's initial minimum translation
depth from native supports and finite geometry. An interior ball at a real
triangle point inside a cylinder proves a lower depth bound using its native
nearest-boundary distance. Whole-projection clearance at all three edges can
also prove that bound. A matching support depth supplies the upper bound;
the whole-arc gap must respect the lower bound minus tolerance. Every shorter
translation still intersects the finite triangle. Edge and deep-overlap cases
without a proof retain native GJK/EPA. Depth certificates share the immutable
substep/leaf lifetime of initial contacts. A cached deep initial contact rules
out other clear separating axes at time zero; its axis is checked first.
Native relative contact poses use `Pose::inv_mul`, subtracting world origins
before inverse rotation. Material gap evaluation subtracts body centers before
adding local witness offsets. Both preserve small contact gaps at large common
translations. Native contact samples validate both local witnesses and both unit normals.
The query adapter retains unsupported queries and incomplete unbounded results
across Parry's optional-hit boundary. Such failures cannot become clear-space
results; the continuous owner holds the affected tick before writeback.
Enclosing initial queries must likewise return geometry, even for separated
shapes. Missing geometry is an owner error; validated witnesses enter the
immutable cache once. The negative missing-result test rejects an incomplete
internal prediction rather than accepting a clear-space result.
Native triangle/cylinder and triangle/cuboid references specialise the same
GJK/EPA entry point without virtual support calls inside its iterations. Both
pair orientations retain their native local frames; tests compare witness,
normal and depth exactly with the generic native kernel.
The owner shares native EPA allocation scratch across queries. Native EPA resets
its current polytope on every call; varied-shape tests compare its result with
fresh native scratch. Exact single-point touching retains the generating support
direction and real material witnesses. Missing support provenance rejects that
degenerate query. The non-origin corner fixture reproduced fabricated zero
witnesses before the native correction.
The existing native relative-point displacement bound is shared with the
continuous owner. It can certify the convex-leaf residual tolerance directly:
every support gap, including the initial contact axis, changes by no more than
that upward-rounded bound, including native arithmetic margin at the actual
coordinate and geometry scale. Both offset pivots contribute; every corrected
trajectory re-establishes the proof. Larger or uncertain bounds retain native
geometry. Prepared material-contact inequalities still run, and the authored
contact tolerance is unchanged.

The pinned Avian source owns dimensional implicit force/acceleration motors,
both-endpoint coordinate inertia, anchor velocity and force saturation. It also
owns SO(3) velocity projection, signed revolute coordinates, completed joint
reaction accumulation and joint damping. Sleeping islands retain their last
completed reaction; disabled joints or endpoints clear it. USD omitted drive
gains remain zero and omitted force caps remain unbounded. Readers, commands,
co-simulation and telemetry consume these owners rather than reconstructing a
law from one body's mass. Aliased endpoints fail before mutable body access.

USD owns geometry and provenance. The registered `LunCoDerivedGeometryAPI`
validates exact Cube/Cylinder proxies against their composed sibling source in
native double precision before collider admission. Admission publishes actual
source membership and native subshape identities. The exact rover collider
hierarchy, including wheels, supplies contact evidence. Authored Rhai chooses
the structural members and their body owners; Rust applies generic validation.
Griffin regenerates source geometry first, then its 40 landing-gear proxies
after the document generation composes.

The document owner removes attributes and relationships through local property
operations with `RestoreProperty` inverses. Undo preserves unrelated edits and
exact target list operations. Live projection consumes the document's resolved
property so removing a strong opinion reveals the weaker opinion correctly.

Hook review: continuous math, inertia, collision admission and safe writeback
are engine invariants with typed native inputs and outputs, installed for the
simulation lifecycle. They are required mechanisms rather than changeable
scenario policy. Member selection and source regeneration remain Twin Rhai;
lint selection and severity remain the installed USD Rhai policy.

## Maintained dependency assessment

[Avian 0.7.0](https://github.com/avianphysics/avian/releases/tag/v0.7.0) is the
latest published release checked on 2026-10-11 and matches Bevy 0.19. The local
patch retains its published source and licences, with exact changes documented
in `third_party/avian3d/LUNCOSIM-PROVENANCE.md`. Joint damping incorporates the
native correction from [PR 1078](https://github.com/avianphysics/avian/pull/1078).
Native body-only CCD does not establish articulated closure. The inspected
unpublished CCD work in [PR 1058](https://github.com/avianphysics/avian/pull/1058)
does not supply acceptance for this cylinder-based landing gear.

Parry's newer published release is 0.31.1. Its native GJK correction fixes a
reproduced false-negative intersection between coincident overlapping cylinders
in our 0.27 dependency. The failing fixture exited 101 before the correction.
`third_party/parry3d-f64` retains published 0.27 source and the Bevy 0.19 / glam
0.32 type contract, incorporating the published 0.31.1 GJK implementation and
the upstream scalar-math enhanced-determinism feature correction. Source archive
hashes, licensing and verification requirements are recorded in its provenance.
This is a native math correction; it does not substitute scene geometry or
alter authored contact tolerances.

Scalar math exposed missing aligned-mask serialization in glam 0.32.1, preventing
Bevy reflection from compiling. `third_party/glam` retains the published native
types and implements their tuple serialization in both scalar and SIMD builds.
It also corrects `BVec4A` fourth-lane serialization and the `BVec3A` tuple visitor
name. All sixteen mixed four-lane masks roundtrip with their exact wire values.
The root lock retains existing Windows dependency resolutions. Full dependency
upgrades and cross-platform deterministic replay require their own verification;
the local tests do not establish either.

The nonlinear motion evaluator preserves its supplied pose exactly at time zero
and evaluates the analytic pivot displacement directly. Pure translation does
not compose cancelling transforms. Its directional initial-overlap handler
uses upward-rounded bounds on relative linear speed and both angular point
speeds, including offset pivots. Contact sampling is omitted only when the
unchanged native thickness criterion is unreachable. Positive-time impacts,
zero-thickness pairs and stop-at-penetration mode retain native geometry.
These generic native optimisations are local changes recorded in the Parry
provenance; they are not attributed to an upstream release.
The native cylinder point query also retains minimum distance at tied cap/side
features. Equal cap distances cannot select a farther side surface, and rigid
transforms retain the same minimum-distance contract.

Development builds explicitly use optimisation level 3 for the workspace glam
and physics owners, retaining assertions and overflow checks. Cargo's wildcard
dependency override excludes workspace members; generic kernels are instantiated
in their caller. See the [Cargo profile contract](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides-and-generics).

## Verification

Production binary: `cargo build -p lunco-luncosim -j 4`, repository target and
regular sccache. The accepted artifact SHA256 is
`f747d6043c11b37827a0ce95a96c9f0ef95e2affcb38008ffd7ae54f8ba27065`.
Eleven physics/startup runs used owned API ports 6311–6321; the bounded scripting
contract used port 6324. All ran at
60 Hz and zero jitter. Spring startup also passed eight independent default-pool
launches on ports 4201 and 4206–4212 before the contact arithmetic correction.
Griffin collision passed with both one and default compute pools;
other gates used the default pool. Each accepted gate returned exit zero.
Receipts and complete timing histories are outside the build tree, under
`/home/rod/.local/state/luncosim-physics-audit-20261011/`; every owned process and port was released.

| Production gate | Result |
| --- | --- |
| `PRISMATIC_SPRING` | 26 checks; tick-zero startup and consecutive ticks, loaded equilibrium, momentum balance, sleeping reaction retention and disable clearing |
| `SCENARIO_SOURCE_FAILURE` | Expected `scenario-source-load-failed` fault from a live, typed program attachment |
| `NATIVE_FORCE_DRIVES` | 18 checks; both-endpoint inertia, SI law, zero gains/caps, damping, stop reactions and unbounded caps |
| `CONTINUOUS_CONTACTS` | 12 checks; coupled contact, rotating fixed attachment, exact pair scope and initial-contact boundaries |
| `DERIVED_PRIMITIVE_COLLISION` | Expected `usd-avian-collider-invalid` fault after live source drift |
| `INVALID_JOINT_ENDPOINTS` | Expected `usd-physics-joint-invalid` fault after live endpoint aliasing |
| `LINT_SELFTEST` | 53 checks, including isolated invalid drive cases and valid controls |
| `PRISMATIC_DRIVE` | 5 checks under gravity |
| `SCRIPTING_ASSET_CONTRACTS` | 38 checks through bounded runtime discovery, actual tool evaluation and authored source validation |
| `GRIFFIN_ROVER_LEG_COLLISION` | 88 checks over 900 ticks; all 40 admitted structural sources and actual collision response |
| `GRIFFIN_ENGINE_OFF_IMPACT` | 9 checks over 1800 ticks; no thrust, pad centers/rims, stops and final four-pad support |

The current production impact measured maximum native penetration 2.229524 mm,
minimum sampled pad clearance -2.344583 mm, hard-stop error 4.317259 nm and final
ground speed 1.556063 mm/s. The rover reached 2.454570 m/s and contacted the PX
shock-barrel tube collider, with maximum pair penetration 0.059465 mm and a
positive native collision impulse. No collision filters or authored mechanical
limits were weakened to obtain these verdicts.

Low-level tests cover contact trajectory bounds, SO(3) derivatives, finite mesh
features and native normal constraints, immutable contact-cache lifetime, local
property undo and resolved weak-property projection. The current native kernel
passed all twenty-four `ccd::tests`, including the overlapping-cylinder fixture,
finite-edge/swapped-frame geometry, exact starting and linear poses, offset
pivots, conservative positive-time witnesses, finite-triangle depth certificates, radial separation axes, changed-trajectory
impacts, swept-BVH admission, retained query errors and scoped-query equality
across one- and four-thread pools. The scoped-query negative case exposed a
nonfinite endpoint hidden by floating-point minimum reduction; it rejects that
endpoint after the owner correction. The new baseline motion/query tests failed before
the native corrections. Scalar aligned-mask
tests passed the truth table and five native valid/malformed-tuple tests; SIMD
passed all six native boolean serialization tests. The current production
artifact passes all ten distinct gates in the table; its second rover run uses
one thread. The low-level cache test proves that changed starting poses
invalidate native geometry recursively and that a retained candidate direction
cannot hide a new positive-time impact.

The new negative fixture reproduced a 3.896484 micrometre intersection rounded
to zero by world-space support subtraction at a common translation of 1e11
metres. Support certificates now use native relative poses and outward
arithmetic allowances; both pair orientations retain the intersection.
A second failing baseline reproduced the same crossing lost in a material-witness
gap. Material gaps and collider-to-COM witness offsets subtract origins before
adding local geometry; the large-translation and local results match exactly.

## Source admission

The spring gate reproduced a late startup actor despite the clock still being
required to start its program at tick zero. USD source paths had no simulation
hold while their root/import assets loaded. Compilation had a hold, leaving a
gap before the program document existed.

`lunco-scripting-rhai-runtime` owns source admission for USD paths and
`RunScenarioAsset`. It acquires an exact `ScriptSourcePreparation` key using the
existing scripting allocator. The shared source-closure owner validates the
complete published graph before writing source or prepared-AST registries. Root
and import publication gaps retain the request and hold, including Twin tool
libraries. Prelude activation uses the same closure readiness boundary. A
native in-memory seam test proves pending root/import publication and invalid
cycles leave both registries unchanged, then proves complete publication
commits all three fixture sources. The focused
`cargo check -j 4 -p lunco-scripting-rhai-runtime` also passed with all updated
callers and native body dispatch. Successful source closure preparation
transfers to compilation and startup in the ordered lifecycle pass.
All startup sources must publish before actor initialization. Replacement,
removal, and despawn release only their own operation. Failed authored sources
raise a structured fault and remain held until replaced or removed; there is
no frame-driven retry. This is a shared admission invariant in Rust; authored
scenario policy and the production verdict remain in Rhai.

The generic ECS lifecycle test passed replacement, failure retention,
independent ownership, removal, despawn and retirement of a deferred command
target; the retired command releases its key and records a terminal failure.
The production spring gate passed
26 checks in eight quiet independent launches, including tick-zero startup and
unbroken subsequent ticks. The missing-source gate was authored through
`NewDocument`, typed USD operations, `AttachProgram`, `SaveAsDocument` and
`SaveDocument`, with source readback. Its live stimulus raised the expected
`scenario-source-load-failed` fault at tick one and the production runner
exited zero. The current one-thread and default-pool runs report identical rover
endpoint, velocity, contact penetration and impulse values. This endpoint
comparison does not establish full-state replay.

## Measured performance limitation

Uninstrumented full `PhysicsTotalDiagnostics.step_time` includes CCD and joints.
The accepted artifact's complete rover acceptance windows are ticks 240–780.
The impact row covers all observed steps. Workload snapshots recorded another
worktree's Tracy build during all three runs and a rendered app later in the
batch. These contention-affected measurements do not
meet the requested 0.5 ms budget; every observed Griffin step exceeded it.

| Scene and compute profile | Samples | p50 ms | p95 ms | p99 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Griffin rover, default | 541 | 3.383507 | 5.254241 | 6.062250 | 9.636693 |
| Griffin rover, one thread | 541 | 3.468677 | 4.941276 | 5.890489 | 6.765031 |
| Griffin engine-off impact, default | 1797 | 3.105088 | 4.799952 | 6.257234 | 44.268047 |

A subsequent default rover run on API 6325 passed the same contact verdict.
Its complete 541-sample window measured p50 3.361306 ms and p95 5.959958 ms.
A different worktree's Rust compiler was active at its completion, so this
repeat also does not establish uncontended timing.

One late rover sample at step 898 measured 4.026709 ms total, 2.437502 ms in
continuous solving, 2.099177 ms in its sweeps and 0.182553 ms in its joint
closure. The native remainder also exceeds 0.5 ms, so optimizing CCD alone
cannot establish the full requested budget. The native step retained eight
substeps, 34 bodies, 14 dynamic bodies, 33 colliders and 18 admitted joints.

The scripting contract's full catalog request exceeded Rhai's aggregate string
limit: the external catalog response contains 117,061 string bytes, while the
shared sandbox permits 65,536. The consumer now uses the existing bounded
`ScriptComplete` owner for exact verbs, prelude functions, tool namespaces and
policy-hook discovery. Its 38-check production verdict passes without changing
sandbox limits or adding another discovery API.

A fresh, complete Tracy capture (`fresh-griffin-rover-complete.tracy`) retained
7,234,361 zones over 21.95 seconds. The owned session used API 6273 and Tracy
8086, and exact process/socket ownership was recorded. This diagnostic artifact
predates the relative-pose, body-dispatch and support-chord changes. Its self
spans attribute 886.49 ms across 73,366 initial triangle/cylinder queries,
596.34 ms to support bounds and 489.33 ms to symmetric joint closure across the
whole capture. Native integration and inertia loops each dispatched about
14 tasks per call. These are attribution values across overlapping threads,
not product timing. Captures and acceptance receipts live outside `target/`.
Native independent-body kernels use the existing solver configuration's batch
bound: small populations execute directly, larger ones retain adaptive batches.
No geometry, filters, substeps, tolerances or ticks were reduced.

The subsequent diagnostic artifact
`e7325190798c98bfb61de14e762d36e3a83a77479c57793ad168cdabeb7b3ffb`
passes the rover gate on API 6326. The complete capture
`current-griffin-rover-complete.tracy` records 6,203,042 zones over 28.83 seconds;
owned PID 515694/socket ownership is recorded in `current-tracy-connections.txt`.
Its initial triangle/cylinder queries fall to 25,050, with 295.731 ms aggregate
self time. Native integration and inertia kernels no longer dispatch the small
independent-body worker queries. Small candidate batches still dominate their
caller spans, motivating bounded CCD work assignment. These remain diagnostic
values, with concurrent builds, rather than uncontended product timing.
`ContinuousCollisionSettings.minimum_query_batch_size` bounds queries per worker
(default 64; zero permits one); smaller populations run the same kernel directly.
All 24 focused continuous-owner tests pass, including exact mixed witness, TOI,
error and cache equality between direct and four-thread execution. The normal
artifact `3f0fb57d4796f1f1b4d82da3350d83d2fa7f920bdcf39a3b6f5cf7127ddcac8d`
passes six production gates on APIs 6331–6336: default and one-thread rover,
engine-off impact, prismatic spring, continuous contacts and the expected
derived-primitive rejection. Its default rover window measures p50 3.168396 ms,
p95 4.511023 ms and p99 5.957073 ms; the one-thread window measures p50
3.108544 ms. Every one of those 541-step windows exceeds 0.5 ms. Concurrent
worktree builds prevent treating these as uncontended acceptance.

The dispatch comparison is not full-state replay evidence. Diagnostic launches
on APIs 6339 and 6340 differ in ramp center-of-mass values at the first physics
step, before the batch-setting commands at steps four/six. Entity allocation
IDs also differ, but removing only those IDs from the comparison retains the
mass-property divergence. All six ramp sections differ, including leaf sections.
The initial cause therefore precedes query dispatch. APIs 6341/6342 probe the
existing typed `QueryPhysicsState` owner for collider source paths and mass
overrides; these 120-tick runs intentionally end without the 900-tick verdict.

The native mass helper reproduces a separate ownership fault: a parent with
mass 2 and an owned mass-4 descendant reports 1506 when an independent nested
body and its descendant contribute 1500. The smallest generic helper test
fails before the correction. The native correction stops traversal at each
nested rigid-body boundary while retaining siblings, reparenting and body
removal semantics. Compound cooking also orders eligible source prims by
composed identity so native part indices and reduction order do not depend on
USD child presentation order. Production and initial-state comparisons below
validate the admitted geometry and mass contract.
Diagnostic compound records expose native scale, local part poses and native
part mass properties to locate the remaining first divergence.

The bridge also propagated collider transforms in the nested physics schedule,
after native fixed preparation had already cooked scale and mass. Rootless
propagation now runs before those preparation consumers. Native collider
insertion no longer replaces constructor scale from render GlobalTransform;
ordered preparation owns transform-driven scale. A focused native bridge seam
checks constructor scale with deliberately stale render data, then first-step
and changed-ancestor scale/mass agreement. It passes, as do both native mass
helper seams and the composed live/prepared compound-order test. Normal artifact
`2c895800b3d94e916a83eb01e73961f5482cf80f2674a80622b088b96da0454d`
passes seven production gates on APIs 6343–6349: default/one-thread rover,
engine-off impact, spring, native drives, continuous contacts and prismatic
drive. Both rover runs report exactly equal endpoint, velocity, impulse and
penetration values, including the shock-barrel source path. This remains
selected-state evidence, not complete replay acceptance.

Its default rover ticks 240–780 measure p50 3.385463 ms, p95 4.538225 ms,
p99 5.479994 ms and maximum 9.049829 ms. The one-thread window measures
p50 3.191029 ms, p95 4.288098 ms, p99 4.706639 ms and maximum 4.849035 ms.
Both windows contain 541 samples, all over 0.5 ms. Impact's 1795 observed
steps measure p50 3.117933 ms. Another worktree's Tracy build is concurrent;
these are contention-affected measurements and the budget remains unmet.

APIs 6350/6351 compare 120 diagnostic ticks with default and one compute
thread. All 1920 body/collider/compound records and 917 prepared-contact
records match exactly after normalizing allocation-only entity/contact IDs.
The first-step native geometry, scale and mass records also match. This
resolves the observed initial ramp mass divergence in these launches;
it does not replace the complete physics/Modelica replay gate.
API 6352 repeats the rover verdict after concurrent Cargo builds finish.
Its complete 541-step window measures p50 2.895677 ms, p95 3.859498 ms,
p99 4.345595 ms and maximum 4.665442 ms, all over 0.5 ms. Its workload
snapshot is retained. The verified normal artifact is archived outside
`target/`; subsequent disk recovery cleans only this checkout's build outputs.
The native helper also admits geometry-derived mass only for live collider
markers. Retained required components cannot keep a removed collider's mass
in the reduction; explicit authored mass remains independent of geometry.
The simulation installer uses the same level-3 development code generation
as the physics owner and workspace math owner, because it instantiates native
systems with USD collision hooks. Assertions, substeps, geometry and tolerances
are retained; this compiler setting alone is not timing acceptance.

The current Tracy artifact
`f3fdb10f3ed8b4ce358b273c3a15b078af2e9186c14e73c6d0934aaf23b4863b`
passes the 900-tick rover gate on owned API 6353 with a verified capture-client
connection on 8086. Both ports close. Its self-time export attributes 465.900 ms
to 14552 leaf-trajectory preparations, 534.478 ms to 61641 symmetric joint
sweeps, 358.763 ms to 900 native narrow-phase updates, and 223.303 ms to 25000
initial triangle/cylinder contacts. These are diagnostic totals with profiler
overhead, not acceptance timing. Each sweep admits 1770 prepared pairs from
113 unique leaves; unchanged leaves have been repeating the same native bound.
Owned normal production repeats on APIs 6358/6360 pass all 88 rover checks
and close their ports. Their complete 541-step windows measure p50 3.200186 /
3.173246 ms and p95 4.394266 / 4.285523 ms with the same verified
`2c895800b3d94e916a83eb01e73961f5482cf80f2674a80622b088b96da0454d`
artifact. This current full-step evidence still rejects the 0.5 ms budget.

The maintained four-rover exact reference separately expects 434 Modelica
variables while the current authored owner publishes 438. The added guidance
variables are `arrival_gate`, `travel_direction`, `wheel_approach` and
`wheel_steer` under `network_system.Drivetrain.guidance`. Its comparator now
reports added/removed names and still rejects mismatches with zero numerical
tolerance. That stale reference has not been regenerated and is not replay
acceptance. Full fixed-transaction cadence, viewport performance and
cross-platform exact state replay remain distinct verification gates.
