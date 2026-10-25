# Native geometry preparation before physics readiness

`lunco-physics::body_mass::with_geometry_preparation` configures Avian's
maintained collider backend and mass plugin on `PhysicsGeometryPreparation`.
The spatial bridge runs that schedule before initial collider queries and each
fixed solver invocation. Rootless collider transforms precede native shape
cooking and mass recomputation; the existing ownership reduction runs last.
USD dynamic admission follows `PhysicsGeometryAdmission` before publishing
`PhysicsStateReady`. No physics clock advances during preparation.

The native plugins retain shape cooking, override handling and recomputation.
The application retains its existing `RigidBodyColliders` ownership reduction,
including nested-body exclusion and stable source order. The USD projector
supplies authored components; it installs no separate mass-reduction system.
The bridge rejects a host missing the shared preparation configuration during
plugin construction. Public native schedule configuration is sufficient; this
change modifies no dependency source or solver settings.

This boundary is an engine invariant, so it belongs in Rust's typed schedule
composition. Scenario timing, assertions and reference selection remain Rhai
policy. There is no new hook or parallel geometry formula. Native solver
precision retains the documented deferred f32 work; body-owned reduction uses
the existing f64 math and explicit native component boundary.

## Verification

The normal baseline binary `09da73e5ec99ecff70c83b4603045cc5ec09dc616122244f9b542f1d281ad926`
reports all four rovers ready at ticks zero and one, with finite 720 kg mass but
nonfinite inertia. Three independent baseline runs have identical available
checkpoints. The full four-rover gate fails three of 24 checks.

The focused `collider_preparation_consumes_current_rootless_scale_before_mass_reduction`
mechanism check now requires scale, mass and finite inertia after `PreUpdate`,
before any fixed step. It also checks that changing ancestor scale updates mass
at the next fixed boundary. It passes with the final configuration guard.

The production scenario derives elapsed time from its authoritative tick and
declared tick rate. Summing rounded floating-point deltas moved its 180/360
checkpoints to 181/361. The selected checkpoints remain 0, 1, 180, 360, 540 and
720, with an explicit final state at 780. No assertion or tolerance is relaxed.

The maintained reference's Modelica variable set predates the current library:
each rover now exposes 507 variables instead of 442, including gearbox power,
heat and motor resistance facts. Its replacement is captured through
`scripts/capture_deterministic_physics_reference.py`, with every non-reference
assertion and the altered-row discriminator enabled. All ten registered
profiles match exactly in two independent production processes. A third replay
per profile passes every check against the candidate: four, eight and twenty
rovers with single/default Compute pools, plus four jitter/seed combinations.
The ten replay verdicts contain 360 passing checks in total. The promoted
expected-state fixture is byte-identical to that tested candidate; raw logs and
profiling data remain ignored.

The regular production binary is
`fcd1c9b352bd9b9ecf9f2d8f47f6dc76ab68ea788c5447f1e0aedd595e038a5f`.
The builds use regular `target/`, sccache and `-j 4`. The production
`invalid_physics_rotation` gate passes its expected terminal
`avian-backend-pose-invalid` verdict without crashing. Owned API ports
5299–5302 are released after their tests.

Normal Summer/API 5303, Griffin/5304 and Tesla/5305 windows retain finite mass
and inertia, complete terrain readiness, clear registered diagnostics and
inspected scene images. Each measurement follows five seconds of readiness
soak and includes ten seconds with at least 240 fresh coupled ticks. Summer's
72, Griffin's 271 and Tesla's 128 admitted source hashes match their preceding
baseline cohorts and remain unchanged through their measured windows.

Griffin's launch controller exits 143 while its owned application remains live.
The interrupted loading measurement is excluded. After verifying the PID,
checkout, executable and port, a separate observer completes the settled
measurement through that same owned API. The lander reaches four-pad handoff
and sleeps. Its native mass may differ from an earlier run at a different
mission tick; this is not an exact-state comparison between matched ticks.
The application exits through its API and releases its PID and port.

| Twin | Coupled median/p95 | Frame median/p95 |
|---|---:|---:|
| Summer | 4.198/4.931 ms | 13.604/16.180 ms |
| Griffin | 6.082/7.367 ms | 20.246/24.580 ms |
| Tesla | 2.855/3.775 ms | 11.959/14.821 ms |

These are contention-qualified normal observations, not an isolated speedup.
Another Tesla session on API 4499 remains active throughout. Tesla's existing
frame/datum-unavailable HUD remains visible; its stationary window is not a
complete driving-course verdict. Summer and Griffin retain terrain, scene
geometry and shadows. All owned application processes close.

Loading, coupled-tick and graphics targets remain open. Raw receipts and
profiling data are ignored under
`.cache/codex-evidence/performance-20261023/geometry-admission-*` and
`summer/griffin/tesla-geometry-admission-normal/` in the same directory.
