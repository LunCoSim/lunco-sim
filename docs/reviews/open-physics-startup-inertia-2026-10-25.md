# Physics-ready body before inertia preparation

Status: open. The existing four-rover determinism gate fails on the normal
baseline before the fixed-schedule executor experiment. Do not replace the
reference or relax the finite-inertia assertion to make it pass.

The production binary is SHA-256
`09da73e5ec99ecff70c83b4603045cc5ec09dc616122244f9b542f1d281ad926`,
independently copied before editing the application executor configuration.
On owned API 5285, `multi_rover_stress_4` with one Compute thread, no jitter and
the registered exact reference completes 780 ticks and fails 3 of 24 checks:
the complete startup/lifecycle physics trace, reference comparison and its
deliberately altered-row negative. Its first mismatch is an incomplete physics
roster at tick zero.

The authored diagnostic reports the actual rejected facts. A second baseline
on API 5295 has the same three failures. All four rovers at ticks zero and one
have `physics_state_ready=true`, `physics_state_pending=false`,
`physics_pose_seeded=true`, no admission request or initialization pending,
`rigid_body_disabled=false`, mass 720 kg and finite computed mass, but
`computed_inertia_finite=false`. Later selected checkpoints do not report that
condition. The baseline gate's existing validity predicate therefore excludes
all four initial rows. The golden comparison cannot begin from a complete
initial state.

The first added diagnostic exceeded Rhai's expression-complexity limit. Its
failed preparation run on API 5294 is excluded; the retained diagnostic uses
compact typed maps and executes successfully on 5295. No assertion changes.
All owned test processes exit and release their ports.

Ownership spans native Avian geometry preparation, the application's collider
ownership reduction and USD dynamic admission. Native `MassPropertyPlugin`
and `ColliderBackendPlugin` prepare geometry/mass in `FixedPostUpdate`; the
application's `update_body_mass_from_colliders` also runs there. USD admission
publishes `PhysicsStateReady` before the scenario's time-zero callback, while
the native preparation pass has not completed the computed inertia consumed
by that callback. Merely delaying a checkpoint would hide this ordering gap.

Avian's maintained plugin constructors already accept a custom schedule.
Investigate composing the native collider-scale/mass preparation and the
existing ownership reduction through one shared preparation boundary that can
run before dynamic readiness and before each fixed solver step. Preserve
current hierarchy, scale, explicit-property ownership and deterministic order;
do not advance time to prime the geometry or duplicate native mass formulas.
This supported application integration should be exhausted before proposing
a dependency-source change.

Required completion evidence: a production time-zero body query with the
authored finite inertia, the unchanged exact four-rover gate including its
altered-row negative, same-tick scale/mass preparation, invalid geometry
rejection, and normal Twin admission. The executor experiment's acceptance
must separately distinguish this pre-existing failure from changes it causes.

Raw baseline receipts and logs stay ignored under
`.cache/codex-evidence/performance-20261023/fixed-schedule-baseline-*`.
