# Upstream physics integration review

Status: Bevy 0.20 / Avian 0.8 migration in progress, 2026-10-22.
The application retains the native `e4f073b` pin; the dense island candidate is
not adopted. The regular production Tracy rebuild passes. Current production
gates pass isolated tire (4 checks), Lunokhod (102), shared drivetrain parity
(20), Modelica raycast drive (8), object suspension (11) and coupled wheel
suspension (16). Three-Twin qualification remains incomplete: Roadster's ramp
and medium/high-speed steering gates fail, and Griffin's high-energy engine-off
drop needs physical failure classification rather than a settling assertion.
Earlier measurements below qualify their recorded artifacts only.

## Ownership

Use Avian 0.8 with the explicitly authorized LunCoSim native motor corrections,
and upstream registry Parry f64 0.31. Native plugins own joint
preparation, XPBD solving, motor laws, passive joint damping, contact generation,
friction and swept CCD. Application composition installs those plugins in both
headless and graphical hosts. The native collider tree owns proxy lifecycle.

LunCoSim owns USD schema interpretation and SI conversion, typed admission,
BigSpace coordinate transport, body collider ownership, generic multi-body gear
relations, actuator ports and completed native telemetry. Gear relations consume
native `SolverBodies` storage rather than duplicating solver-body components.

World positions, authored parameters, domain signals and persistence remain f64.
The user accepted Avian's internal f32 solver state for this migration. TODO:
qualify and address local-frame joint/contact accuracy and long-duration drift.
Conversions at native integration boundaries must be explicit and range checked;
widening native output does not recover its original precision.

## Dependencies

Bevy 0.20 and Avian 0.8 are upstream releases. The authorized forks are pinned
to immutable published commits:

- Avian `e4f073bb8914beb9fb0aa97795ae93d08a478528`: implicit motor integration,
  first-step collision admission, fixed-joint block solving, completed reactions,
  disabled-body membership, native collision-hook proxy synchronization and
  coupled prismatic frame/limit solving. The separate dense island candidate
  fails loaded-friction, driving and timing qualification and remains outside
  the application pin.
- Bevy shader `a12e8366cdd9c0ee8dabd3ada7bb7e8f02f0b338`: named asset
  sources map to WESL packages, relative and cross-source imports retain native
  dependency handles, and live source preparation uses registered readers. All
  11 native shader tests pass without a GPU.
- BigSpace `581295f909ddddad138a876b497dbc1fdff389b6`: Bevy's native
  `QueryIterMany::matched` contract for mixed spatial/non-spatial children in both
  propagation paths. The regression reproduces both panics before the fix;
  all 41 native library tests pass with the fix.
- Transform gizmo `01302a171ac3db662c0c322ef9e98fe7d5f33a6a`: Bevy 0.20 /
  egui 0.36 integration, preserving viewport/DPI and depth-aware picking.

## Coupling boundary

`Port::with_contract`, `EngineUnit`, `SimConnection::resolved_transform` and
`JointTorqueActuator::new` supply the shared typed engineering boundary. External
joint effort requires explicit conjugate units and coordinate frames, a disabled
native motor, and valid live endpoints. Lints reuse the runtime contract reader.
Modelica owns motor, gearbox, efficiency and thermal laws. Avian owns body motion
and contacts. See the canonical physical coupling contract in
[`22-domain-cosim.md`](../architecture/22-domain-cosim.md#physical-coupling-contracts).

## Required qualification

The native collider-detach regression must preserve the standalone proxy key and
retire the corresponding proxy on despawn. Production tests must exercise joint
frames, limits, force and acceleration drives, passive damping, gear relations,
CCD, contact provenance and invalid admission without crashing the application.
Roadster restart must leave zero native contact pairs with retired or noncollider
endpoints. Compare the same authored hull and analytic cylinder geometry with
source hashes and correct axle coordinates.

Capture native physics phase costs using the regular production Tracy build.
Keep separate clean timing evidence for product acceptance. Compilation does not
establish improved physics time, joint stability, rendering correctness or FPS.

## Current diagnostic capture

`avian08-roadster-hull-20261021.tracy` records the regular production Tracy
build on owned API 4764 / profiler 8097. Its native export and compression
framing pass, and the saved sampling period is 1,000,000 ns. In the final 120
complete physics steps, mean inclusive PhysicsSchedule time is 2.720 ms:
swept CCD 1.434 ms, substeps 0.432 ms, solver AABB updates 0.174 ms, narrow
phase 0.183 ms, broad phase 0.006 ms, and other physics 0.492 ms. These phase
rows are disjoint; solver substeps already contain native joint/contact work.
The API's retained history reports a 2.426 ms median step. The external joint
effort adapter averages 0.019 ms over the complete capture.

Roadster restart retains 10 joint graph edges and zero contact pairs with
missing entities or non-collider endpoints. Authored source hashes remain
unchanged. This instrumented run is diagnostic evidence, not clean FPS
acceptance. Native exports, phase analysis and runtime reports are retained
under `.cache/codex-evidence/physics-followup-20261021/`.

The matching cylinder capture (`avian08-roadster-cylinder-20261021.tracy`)
also has a 1,000,000 ns sampling period. Its final 120 complete physics steps
average 0.901 ms (median 0.773 ms): swept CCD 0.003 ms, substeps 0.438 ms,
solver AABB updates 0.010 ms, narrow phase 0.028 ms, broad phase 0.005 ms,
and other physics 0.415 ms. The API history median is 0.654 ms. Body, collider,
joint and contact counts match the hull run; restart retains zero invalid contact
pairs and authored source hashes remain unchanged. The diagnostic overlay uses
an X-axis cylinder (radius 0.34 m, front/rear widths 0.25/0.30 m) with a separate
crowned visual mesh. This is an unsaved comparison, pending behavioral adoption.

## WESL qualification checkpoint

The native loader owns shader dependencies in graphical and headless hosts.
Material reflection and stage admission use the WESL frontend. The application
import catalog and handwritten stage/struct parsers are removed. Live source
edits prepare dependencies asynchronously, publish only the latest edit and
validate their Twin lease before publication.

The regular production build and networking replay compile check pass. Focused
checks pass: 29 material tests, one asynchronous source lifecycle test, and two
lossless USD property tests. The public shader contract gate passes 69 authored
checks, including malformed source and invalid extension rejection. GPU gates
pass for terrain appearance, terrain normal transforms, vertex-shadow alignment
and plume depth occlusion. The terrain gate also checks its prepass variants
and authored overlay switching. The current native contact gate passes 12
checks. These results do not qualify the three complete Twin physics suites.

## Physics qualification in progress

Body-root primitive colliders now retain their standard USD axis in their native
body-local mount. One lowering test covers Cylinder, Cone, Capsule and Plane on
all three axes in both Y-up and Z-up stages. The production isolated-tire gate
passes four authored checks: correct X-axis bounds, sustained contact, bounded
contact penetration and no ground crossing. Peak native penetration is
0.000029288 m. The free tire may topple, so center height alone is not a contact
penetration measurement.

The Summer Lunokhod flat-ground gate passes 102 checks after migrating its
lifecycle/dependency declarations and using the existing Modelica waypoint
setpoint helper. Tesla steering and Griffin engine-off impact still fail their
authored gates. The complete three-Twin suite and graphical physics acceptance
remain open.

Profile revolution accepts an explicit local direction in both the geometry
kernel and document operation. Points and normals rotate together in f64; the
command rejects zero/non-finite directions before document mutation. The native
axis test and 18-check production Rhai geometry contract pass. Tesla's authored contact geometry is now analytic X-axis cylinders, with crowned
visual meshes excluded from collision. On the retained native pin, the isolated
tire passes, but the ramp and medium/high-speed steering gates still fail.

The native offset-slider reproduction passes blocked-axis and hard-stop checks.
A loaded-slider impact preserves contact and travel on static ground and a moving
kinematic platform. Attached-load drive equilibrium passes for linear and rotary
joints under force, acceleration and spring-damper models. Small-angle hinge
alignment and velocity-projection lifecycle checks also pass.

The unqualified dense candidate's typed solve-failure bridge is not installed
in the application. Physical damage detection and numerical/model validity
boundaries remain separate requirements; neither is established by compilation
or by a solver-only test.

## Solver architecture reassessment

The dense island checkpoint is not qualified for main. It couples native joint
frames, unilateral contact normals and implicit drives after the native contact
velocity/friction passes. Its dense constraint-space matrix is assembled every
refinement; each active-set change copies and refactorizes a full matrix. The
worst-case factorization cost is cubic in scalar constraint count, with quadratic
storage, repeated across eight substeps. Redundant loop and contact rows consume
work even though the mechanism has far fewer independent degrees of freedom.

The current native friction solve precedes the extra support correction. The
normal load added through the joint island therefore does not participate in
that same friction solve. A generic asset-free reproduction places a 4000 kg
load on a 180 kg contact body on a 0.12 rad slope, with friction coefficient 0.8.
It slides 0.1785 m in five seconds instead of remaining within the 2 mm check.
This rejects the contact coupling independently of Twin authoring.

The owned Griffin Tracy capture uses published Avian `a6681b6`, API 4766, Tracy
8097 and verified 1 ms sampling. The native island system consumes 57.307 seconds
across 1800 physics ticks (14,400 substeps): 31.84 ms per tick on average. Late
whole-step history medians reach approximately 72 ms. The scene reports 25
bodies, 14 dynamic bodies, 109 colliders, 19 joints and 154 contact pairs near the
end. Native CSV export, capture framing and collector-to-app identity pass. This
is diagnostic attribution, not clean product performance acceptance; the cohort
fails its landing-speed verdict. Evidence is in
`.cache/codex-evidence/physics-followup-20261021/native-coupled-griffin-profile/`.

### Established approaches

- **Unified impulse/TGS solver in Avian:** joints, drives, normal contacts and
  friction share velocity-space response, warm starting and ordering. This
  avoids a late support correction, but requires maintaining a new native joint
  solver. Avian's [replacement issue](https://github.com/avianphysics/avian/issues/440)
  remains open; its maintainer describes an unfinished impulse implementation
  and cautions about mixing AVBD with impulse contact response.
- **Reduced-coordinate articulations with a coupled contact solver:** represent
  a mechanism by its base pose and actual joint coordinates, eliminating blocked
  joint degrees of freedom. This directly targets light links carrying heavy
  chassis loads. [Rapier supports multibody joints and hybrid loop closure](https://rapier.rs/docs/user_guides/rust/joint_constraints/).
  Its [f64 engine and determinism feature](https://rapier.rs/docs/user_guides/rust/getting_started/)
  are relevant to the existing precision and replay contracts. Its Bevy plugin
  [0.37 targets Bevy 0.20](https://docs.rs/crate/bevy_rapier3d/0.37.0/source/Cargo.toml),
  but uses the f32 engine; an f64 qualification must use `rapier3d-f64` directly.
  Loop closures, joint-reaction telemetry, runtime detach/rebind and terrain
  streaming must be qualified before application migration.
- **MuJoCo CPU:** articulated dynamics plus sparse primal Newton/CG contact
  optimization, with circular friction cones and incremental factor updates.
  It is a serious comparison for these small mechanisms and many contacts.
  Its [computation contract](https://mujoco.readthedocs.io/en/stable/computation/)
  also explains soft-contact slip and warns that its optional NoSlip post-pass
  is an ad-hoc correction. A fair comparison must preserve authored compliance
  and validate terrain/topology lifecycle and Modelica effort coupling.
- **PhysX articulations:** established reduced-coordinate mechanism handling
  and [standard USD articulation integration](https://github.com/NVIDIA-Omniverse/PhysX/blob/main/ovphysx/docs/simulation_setup/articulations.md).
  C++ integration, platform support, precision boundaries and runtime joint
  removal/rebuild semantics require explicit qualification.
- **AVBD:** the [SIGGRAPH 2025 method](https://graphics.cs.utah.edu/research/projects/avbd/)
  addresses hard constraints and high stiffness/mass ratios using small local
  blocks. Its research results justify consideration, not a claim of production
  compatibility or a reason to replace one unqualified custom solver with another.

[Catto's solver comparison](https://box2d.org/posts/2024/02/solver2d/) and the
[Small Steps paper](https://matthias-research.github.io/pages/publications/smallsteps.pdf)
support substepping and consistent constraint formulation. We already use eight
substeps; increasing that count does not resolve the measured split friction
ownership or dense repeated factorization.

The selected direction keeps Avian and must qualify load transfer, slope
friction, offset stops, steering, impact and topology lifecycle before adopting
a solver extension. A Schur complement
can reduce factorization work in a maximal-coordinate solver, but does not by
itself repair inconsistent friction coupling. No measured alternative has yet
qualified the complete three-Twin behavior or performance target.

### Vehicle guidance and model audit

The selected direction retains Avian and Bevy ECS. Native joint/island graphs
own connectivity; projected components own the scene's mechanical identities.
Preparation must invalidate on admitted topology, mass and frame changes, with
deterministically ordered commits. Numerical solve data may be compact cached
arrays, but must not become a second topology or lifecycle authority.

Avian's current [motor example](https://github.com/avianphysics/avian/blob/main/crates/avian3d/examples/joint_motors_3d.rs)
demonstrates isolated wheel, servo and slider joints. No maintained complete car
example was found in the current official examples, README or API documentation.
The [vehicle discussion](https://github.com/avianphysics/avian/discussions/614)
is unanswered community experimentation, including moving raycast/force work
into the substep schedule; it does not establish a validated vehicle recipe.
Several joint examples use 50 or 80 substeps. These do not justify raising the
application's eight-substep contract. Custom integration markers replace free
integration, not the contact/joint inertia response.

Physical wheels now bind their Modelica effort/motion ports to the authored
hinge entity. Standard USD joint reading and admission retain both local frames,
limits and native solver identity. The separate wheel-hinge constructor and
wheel exclusion in the joint reader are removed. Raycast wheels retain their
body mount and tire/contact realization without a native wheel hinge.

The drivetrain comparison's initial root height of 1.0 m placed a 0.4 m tire,
mounted 0.65 m below its root, 0.05 m below the support plane. Initialization
correctly paused that assembly. Both variants now start at 1.1 m, preserving
equal initial geometry. The retained `e4f073b` pin passes all 20 production
checks with ordinary authored hinge projection. The steering agreement
criterion has not been relaxed.

Tesla's reference dry mass is 1700 kg, with 128 kg assigned to its four wheels,
four carriers and two steering knuckles. Its Modelica chassis calculation
subtracts those masses and parallel-axis inertia contributions. The 24 kg,
0.34 m wheel's 1.4 kg m² axial inertia is consistent with a solid-cylinder
estimate of 1.3872 kg m². Using the reference sprung mass of 1572 kg, each
300,000 N/m suspension gives approximately 4.40 Hz ride frequency, 12.8 mm
static compression and damping ratio 0.645 at 14,000 N s/m. These are stiff
study assumptions, not grounds for arbitrary solver-oriented mass inflation.
The authored analytic-cylinder wheels preserve the X-axis geometry contract.
On the retained native pin, steering passes the 5 m/s comparison but fails at
15 and 30 m/s: wheel heading departs from its target while all suspensions sit
at their upper travel stops. The ramp gate also fails. Geometry cleanup alone
does not qualify this vehicle.

Omniverse's [vehicle authoring guide](https://docs.omniverse.nvidia.com/kit/docs/omni_physics/110.1/dev_guide/vehicles/vehicles.html)
uses raycasts or sweeps for suspension/road contact and separates this from
optional collision and render geometry. Its [USD representation](https://docs.omniverse.nvidia.com/kit/docs/omni_physics/latest/extensions/ux/source/omni.physx.vehicle/docs/usd_representation.html)
uses the same wheel, tire, suspension and attachment schemas already consumed
here, allows mesh collision, and permits external per-wheel control. Reuse those
schemas with Modelica effort/motion coupling rather than adding a duplicate
wheel API or a built-in drivetrain. Our authored `wheel-mesh-rolling-contact`
lint warns about mesh rolling fidelity without rejecting standard USD meshes.
The production `wheel_collider_lint` gate passes five checks over mesh,
analytic and raycast realizations and a visual-only tread control.

Wheel render ownership uses standard USD purpose and visibility: one centered
`purpose=render` child receives measured axle roll while its collision proxy is
hidden. Both realizations bind that authored child after direct-child structural
admission; mesh orientation does not define the mechanical axle. The shared
authoring reader rejects overlapping visible geometry, multiple render roots,
off-center roots and reset transform stacks, with Rhai exposing
`wheel-visual-contract`. The production `wheel_render_root` gate passes ten
checks including X-axis raycast roll and malformed source fixtures through
`ValidateAsset`. Explicitly disabled collider metadata remains valid on a
query-only wheel; enabled or malformed collision still fails lint.
The bounded production cohort passes 51 checks across render ownership (10),
mesh warning (5), coupled wheel pause (16) and drivetrain parity (20). Loading
the invalid source directly exits with `usd-wheel-visual-invalid` before
simulation, without a panic. Tesla's rendered analytic-wheel gate passes 13
checks and two inspected views show one crowned tire surface; its cabin mesh
hull remains intact. The existing free-wheel torque/bearing mechanism test
also passes after updating its native Avian fixture types. Evidence is under
`.cache/codex-evidence/physics-followup-20261021/wheel-render-root/`.

The regular `target/debug/luncosim` cohort and source hashes are retained under
`.cache/codex-evidence/physics-followup-20261021/native-e4-analytic-wheel-runtime/`;
wheel warning evidence is in the adjacent `wheel-collider-lint/` directory.
After incorporating main's source-snapshot and reflected-setting fixes, the
regular Tracy build passes again. The bounded integration cohort passes wheel
collider lint (5 checks), coupled wheel suspension (16) and drivetrain parity
(20), with successful process exits and released owned API ports. Its artifact
hash and reports are in `integrated-wheel-contract-runtime/` under that evidence
directory. Other unchanged-input results retain their earlier artifact scope.

Griffin's four leg drives each have 12,000 N force caps and 0.07 m total travel.
Even the generous full-travel work bound is only 3360 J across all four drives.
A 6 m/s impact at the authored hull and leg masses (4000 + 4×180 kg) carries
84,960 J before including the other movable members. Most of that impact
therefore reaches rigid stops/contact; this
is a solver stress case, not evidence that the landing hardware absorbs the
energy realistically. A realistic crush/dissipation law requires authored
engineering inputs. Six flight latches also duplicate the blocked freedoms of
their parallel ramp hinges. Removing redundant solve rows is physically sound
only if latch release preserves the hinge and its state atomically.

The current wheel core still contains brake and bearing laws and heat
publication. Completing the requested Modelica ownership requires an authored
cutover of those laws and their static-holding/zero-crossing coupling, followed
by deletion of the Rust laws and all callers. Moving their arithmetic into an
explicit sampled torque without qualifying holding and energy would introduce
chatter or lose static braking. Motors, gearboxes and speed/temperature laws
already have authored Modelica owners; no new Rust hardware model is planned.

Bevy 0.20's [`Ready`](https://github.com/bevyengine/bevy/pull/25296)
is emitted after native scene descendants and dependencies spawn. USD structural
projection and subsequent collider, pose, joint and Modelica preparation have
separate completion facts. A native scene event can start structural consumers;
it cannot certify physical admission or replace persistent scene-lease state.
