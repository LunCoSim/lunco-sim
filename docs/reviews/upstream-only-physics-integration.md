# Upstream physics integration review

Status: Bevy 0.20 / Avian 0.8 migration in progress, 2026-10-21.
The regular production Tracy build passes. The corrected native motor path
passes 20 production checks. High-speed contact acceptance passes 12 checks.
The Modelica drive gate passes 8 checks and travels 8.86 m. Its authored start
height is 1.10 m, clearing the wheel support geometry instead of starting 5 cm
inside the ground. Connection-unit rejection and lint reporting pass 6 checks;
mechanical-loss and thermal laws pass 51 checks. Analytic wheel comparison and
clean graphical performance qualification remain pending.

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

- Avian `559d2d0ab896afa884139234c8bb83eb8a6c71df`: implicit motor integration,
  first-step collision admission, fixed-joint block solving, completed reactions,
  disabled-body membership. All 81 native unit tests pass.
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
