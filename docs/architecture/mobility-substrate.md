# Mobility classifier — Substrate C

> Substrate C of the [efficiency architecture](efficiency-and-maintainability.md).

*Part of the efficiency/maintainability architecture. See
`caching-and-precompute-strategy.md`.*

## What it is (and honestly, what it isn't)

`Mobility` is the source-agnostic **declared** motion class of a physics body —
`Static` / `Kinematic` / `Dynamic` — set by whichever source spawns it (USD
physics schema, a rhai script, a Modelica model), and projected onto the live
avian `RigidBody`.

Unlike substrates A/B/D/E, C is **not a hot-path optimization**. The per-tick win
it was scoped for — static/kinematic bodies skipping physics work — is *already*
captured: the USD→avian path classifies bodies correctly and avian's solver
already skips `Static`. There is no per-frame mobility re-derivation to fix (spawn
is one-shot; the animated-demotion and `Dynamic`-settling systems are already
change-gated). So C is a **unification / structure-state** play, not a speedup.

## The structure/state split

The point is the north-star split applied to physics bodies:

- **`Mobility` = structure (declared intent).** "This rover IS a dynamic body."
  Stable. Lives in `lunco-core` (no avian dependency), so any source or reader
  sets it downward.
- **`RigidBody` = state (live engine type).** Projected from `Mobility`, but *not
  always 1:1*: a `Dynamic`-declared body spawns transiently `Kinematic` while its
  joints settle (`ShouldBeDynamic` → `activate_dynamic_bodies`), and an animated
  body is demoted to `Kinematic` so the sampler owns its pose.

Recording the declared class separately keeps the stable intent queryable even
while the engine body type is mid-transition — e.g. network-prediction
eligibility should ask "is this *meant* to be dynamic" (`Mobility::Dynamic`), not
read a body that is transiently `Kinematic` during settling.

## Wiring (additive, low-risk)

- **`lunco-core::mobility::Mobility`** — the enum + component. Neutral substrate,
  no avian.
- **USD spawn path** (`lunco-usd-avian`) records `Mobility` at every existing
  classification point (terrain / trigger / collision-child → `Static`;
  `physics:kinematicEnabled` → `Kinematic`; `PhysicsRigidBodyAPI` / legacy
  `rigidBodyEnabled` → `Dynamic`; animated-demotion → `Kinematic`). The existing
  `RigidBody`/`ShouldBeDynamic`/settling logic is **unchanged** — `Mobility` is
  added alongside it, so there is zero regression risk to the physics-sensitive
  spawn path.
- **`project_mobility_to_rigid_body`** — maps a declared `Mobility` onto a
  `RigidBody` for bodies the USD path didn't build, gated
  `(Changed<Mobility>, Without<RigidBody>)`. The `Without<RigidBody>` gate means it
  **never** overrides a USD-managed body (including the transient settling
  `Kinematic`); it only serves a rhai / Modelica / editor source that spawns a
  body by declaring mobility alone (one knob, no avian dependency upstream).
  Empty in steady state. Locked by a unit test.

## Follow-ups (deferred)

- **Live mobility flips.** A declared-mobility change on a body that already has a
  `RigidBody` (runtime static⇄dynamic) is out of scope — it needs engine-aware
  transition handling (re-inserting `RigidBody` mid-sim, re-settling joints).
- **Consumers reading intent.** Migrate call sites that inspect `RigidBody` for
  *intent* (e.g. `lunco-networking/sync.rs` prediction eligibility, which can
  misclassify a settling body) to read `Mobility` instead — a correctness
  improvement, done carefully.
- **rhai / Modelica sources.** Expose `Mobility` as a settable field/attribute so
  those runtimes declare mobility directly; the projector already honours it.
