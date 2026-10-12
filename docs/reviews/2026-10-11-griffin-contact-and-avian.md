# Griffin contact and Avian integration review

The current production artifact passes startup engine-off impact, powered
rover-leg collision in both compute profiles, spring startup, continuous contacts
and terminal rejection of a non-unit physical ancestor rotation. This review
covers contact correctness, native drives, collider provenance and admission.
The requested 0.5 ms full physics budget remains unmet. Full-state replay and
cross-platform acceptance remain open.

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

Native mass aggregation stops at independent nested rigid-body boundaries and
consumes geometry-derived mass only for live collider markers. Explicit authored
mass remains independent of collider geometry. Compound part order follows
canonical composed source identity. Rootless collider-transform propagation runs
in fixed preparation before native scale cooking and mass reduction; constructor
scale cannot be replaced from stale render GlobalTransform.

`lunco-physics::avian_backend_pose_is_valid` owns backend pose admission. A rigid
rotation must meet Glam's existing unit-quaternion tolerance, alongside finite,
backend-representable positions. The bridge checks raw body, ancestor and target
frame rotations before spatial composition can normalize them. Missing poses,
cycles and excessive coordinate depth are rejected without a hierarchy panic.
Admission and fixed preparation share the frame diagnostic owner and structural
invalidation contract. Refusing the frame publishes diagnostics and its hold at
either boundary. Collider preparation validates rotations and ancestry before
composition, leaves rejected transforms unchanged, and holds integration.

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
regular sccache. The current artifact SHA256 is
`5e1998376d84ab44e6302d494767db6ef8bd83ca237a2623ff9e31aa3ced1562`.
Six production runs used owned API ports 6372–6377 at 60 Hz and zero jitter.
Every gate returned exit zero and released its process and port. Receipts,
workload snapshots and complete timing histories are retained under
`/home/rod/.local/state/luncosim-physics-audit-20261011/`.

| Production gate | Result |
| --- | --- |
| `INVALID_PHYSICS_ROTATION` | Public ancestor-transform edit raises terminal `avian-backend-pose-invalid` at tick two without a process crash |
| `GRIFFIN_ROVER_LEG_COLLISION`, default pool | 88 checks over 900 ticks; all 40 admitted structural sources and actual rover collision response |
| `GRIFFIN_ROVER_LEG_COLLISION`, one thread | Same 88 checks and exactly equal selected contact/motion records |
| `GRIFFIN_ENGINE_OFF_IMPACT` | 9 checks over 1800 ticks; no thrust, pad centers/rims, stops and final four-pad support |
| `PRISMATIC_SPRING` | 26 checks, including tick-zero startup and consecutive ticks, equilibrium and reaction lifecycle |
| `CONTINUOUS_CONTACTS` | 12 checks for coupled contact, rotating attachment, pair scope and initial-contact boundaries |

The impact measured maximum native penetration 2.232526 mm, minimum sampled
pad clearance -2.346275 mm and hard-stop error 4.212971 nm. Delivered thrust
remained zero and all four feet supported the lander. The rover reached
2.454570 m/s and contacted the PX shock-barrel tube collider with maximum pair
penetration 0.059622 mm and positive native impulse. The selected rover records
match the prior accepted admission artifact exactly in both compute profiles;
the impact penetration, clearance and travel error also match. Selected-state
comparisons do not establish complete replay.

Focused owner tests pass for non-unit/zero/nonrepresentable rotation rejection,
valid float-widened rotations, invalid body and ancestor rotations, disconnected
and excessive-depth coordinate chains, earliest frame diagnostics and holds,
and rejected collider preparation leaving the transform unchanged. The native
rotation predicate test failed before correction. The production negative gate
exercises the public Rhai reflection surface, after initially valid admission.
Its USD fixture was authored and read back through document operations.

The unchanged contact-kernel evidence includes twenty-four `ccd::tests`, native
joint/drive mechanisms, scalar/SIMD serialization, source-admission lifecycle,
and collider mass/scale/part-order seams. Earlier accepted production gates for
native drives, derived-source rejection, aliased joint endpoints, lint,
prismatic drives and bounded scripting are recorded in their separate receipts;
they were not repeated as equivalent coverage for this pose-admission change.

## Source admission

The spring gate reproduced a late startup actor despite the clock still being
required to start its program at tick zero. USD source paths had no simulation
hold while their root/import assets loaded. Compilation had a hold, leaving a
gap before the program document existed.

`lunco-scripting-rhai-runtime` owns source admission for USD paths and
`RunScenarioAsset`. It reserves actor-owned source admission in
`ScenarioPreparationAdmissions` and retains the same key through document and
compilation handoff. Independent asset requests acquire exact
`ScriptSourcePreparation` keys from that same allocator. The shared source-closure owner validates the
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
`scenario-source-failed` fault at tick one and the production runner
exited zero. The current one-thread and default-pool runs report identical rover
endpoint, velocity, contact penetration and impulse values. This endpoint
comparison does not establish full-state replay.

## Measured performance limitation

Uninstrumented full `PhysicsTotalDiagnostics.step_time` includes CCD and joints.
The current artifact's rover windows cover every tick from 240 through 780.
The impact row covers all retained observed steps. Workload snapshots record
another worktree's Cargo test/build and a rendered app during this batch; these
are contention-affected measurements. Every observed Griffin step exceeded the
requested 0.5 ms budget.

| Scene and compute profile | Samples | p50 ms | p95 ms | p99 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Griffin rover, default | 541 | 3.334155 | 4.510673 | 5.122725 | 6.325722 |
| Griffin rover, one thread | 541 | 3.191339 | 4.395879 | 5.783851 | 7.265377 |
| Griffin engine-off impact, default | 1794 | 3.124765 | 4.957377 | 6.572262 | 47.523061 |

The earlier settled admission artifact's complete 541-step rover window measured
p50 2.895677 ms and p95 3.859498 ms, also entirely above 0.5 ms. Its independent
repeats measured p50 3.200186 and 3.173246 ms. A late step measured 2.901798 ms
total, including 2.073163 ms continuous solving, 1.834217 ms sweeps and
0.172571 ms joint closure. Native work outside CCD also exceeds 0.5 ms.
The scene retains eight substeps, 34 bodies, 14 dynamic bodies, 33 colliders and
18 joints. Geometry, filters, mechanical limits and tolerances remain unchanged.

The complete diagnostic capture `geometry-mass-griffin-rover-complete.tracy`
uses artifact
`f3fdb10f3ed8b4ce358b273c3a15b078af2e9186c14e73c6d0934aaf23b4863b`.
It passed the 900-tick rover gate on owned API 6353 with a verified capture-client
connection on 8086; both ports closed. Its self-time export attributes
465.900 ms to 14552 leaf-trajectory preparations, 534.478 ms to 61641 symmetric
joint sweeps, 358.763 ms to 900 native narrow-phase updates, and 223.303 ms to
25000 initial triangle/cylinder contacts. Each sweep admits 1770 prepared pairs
from 113 unique leaves. Profiler totals are diagnostic attribution with overhead,
not product timing or evidence that the budget is met.

The current artifact also passes seven rover runs on owned APIs 6378–6384
using the existing query-batch control. Selected contact/motion records match
exactly at batch bounds 64, 16 and zero. All seven windows contain 541 samples
and every sample exceeds 0.5 ms. Initial batch-16/zero windows measure p50
2.768009/2.757369 ms, between batch-64 controls at 3.494786/2.937575 ms.
Repeated 16/64/16 windows measure 3.320581/3.283862/3.196699 ms. Concurrent
workloads and changing control timings do not establish a reliable improvement;
the default remains 64. This comparison verifies selected-state equality and
rejects budget acceptance; it does not establish full-state replay.

The bridge's initial geometry, scale and mass records match in default and
one-thread diagnostic launches. All 1920 body/collider/compound rows and 917
prepared-contact rows match exactly after normalizing allocation-only IDs.
This closes the observed initial ramp-mass divergence in those launches; it
does not replace full physics/Modelica replay acceptance.

The maintained four-rover exact reference expects 434 Modelica variables while
the current authored owner publishes 438. Added guidance variables are
`arrival_gate`, `travel_direction`, `wheel_approach` and `wheel_steer` under
`network_system.Drivetrain.guidance`. The comparator reports added/removed names
and rejects mismatches with zero numerical tolerance. The stale reference has
not been regenerated. Full fixed-transaction cadence, viewport performance and
cross-platform exact state replay remain separate verification gates.
