# Upstream physics integration review

Status: Bevy 0.20 / Avian 0.8 migration in progress, 2026-10-22.
The regular production Tracy build passes. The corrected native motor path
passes 20 production checks. High-speed contact acceptance passes 12 checks.
The Modelica drive gate passes 8 checks and travels 8.86 m. Its authored start
height is 1.10 m, clearing the wheel support geometry instead of starting 5 cm
inside the ground. Connection-unit rejection and lint reporting pass 6 checks;
mechanical-loss and thermal laws pass 51 checks. Analytic wheel timing is recorded below; clean graphical and ramp qualification
remain pending.

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

- Avian `11dfbb5bb89d9763aeb645385ff77a40427f936b`: implicit motor integration,
  first-step collision admission, fixed-joint block solving, completed reactions,
  disabled-body membership and native collision-hook proxy synchronization. All
  82 native unit tests pass.
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
axis test and 18-check production Rhai geometry contract pass. The analytic
cylinder steering comparison remains unsaved and still fails the steering
verdict; it is not a behavioral adoption decision.
