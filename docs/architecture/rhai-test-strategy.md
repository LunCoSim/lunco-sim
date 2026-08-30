> Status: Active · Audience: contributors moving simulation tests between Rust and Rhai

# Test ownership and migration boundary

Rhai tests are the production acceptance layer for authored policy and
observable behavior. Rust tests remain the mechanism layer. The boundary is not
"small test versus large test": it is who owns the decision and whether the
claim can be observed through a public production surface.

| Claim | Owner | Test form |
|---|---|---|
| USD identity, composition, schema shape, asset edges | USD / asset crates | Rust structural tests plus `--validate` |
| Modelica parsing, AST/source contract, solver/math kernel | Modelica / Rust mechanism | Rust unit and integration tests |
| Avian joint, collider, contact and numerical mechanics | Avian / Rust mechanism | Rust mechanism tests |
| Command dispatch, reflection, script lifecycle, hot reload, authority and teardown | Rust scripting/runtime | Generic Rust seam tests |
| Mission sequencing, route choice, behavior policy, authored tolerances and expected outcomes | Rhai / USD | Production scene + `assets/scenarios/tests/*.rhai` |
| Tutorial steps and application-specific assertions | Rhai observer | Production tutorial scene gate |

The rule is: move a Rust assertion only after an authored fixture can fail for
the same reason through the public runtime path. A Rust test that supplies a
spy command, fake world, private component, or direct function call is not a
production behavior test merely because Rhai appears in its source.

## What has moved in this wave

`rhai_scenario_drives_real_rover` was removed from
`crates/lunco-scripting/tests/rhai_rover_live_test.rs`. It used a `DriveLog` spy
in a `MinimalPlugins` world and therefore proved the generic dispatch seam, not
a rover. The authored `autopilot_hold` scene is the production owner for that
outcome: it runs the real rover, engages the same public autopilot command, and
requires a routed control rover to move before accepting the no-route hold.

The generic command path and hot-reload generation check remain in
`rhai_rover_live_test.rs`; those are runtime-mechanism contracts and still
belong in Rust. The mobility allocation split in `allocation_spec.rhai` is the
model for future moves: exact unobservable kernel arithmetic stays Rust, while
its live consequence moves to an authored control fixture.

The second review also removed the Rust integration copy of
`appending_waypoints_while_running_resumes_route_and_drives_the_new_legs` from
`crates/lunco-autopilot/tests/waypoint_lifecycle_test.rs`. The existing
`autopilot_hold` production scene now holds a real rover with no route, then uses
the public Rhai `patrol(...)` update while that rover is part-way through a real
route. Its forward-motion assertion catches a reset-to-leg-zero U-turn, while
the same scene's routed rover remains the anti-trivial drive control. The
retained Rust tests cover exact cursor arithmetic and scene teardown; neither
is replaced by a weaker string-level check.

The same boundary now applies to scene commands and runtime markers. The
`test_detach_joint_command` unit case moved into `assets/scenarios/tests/joint.rhai`:
the real scene first proves the fixed joint holds `CubeB`, sends the public
`DetachJoint` command, and then requires the released body to fall while the
independent `FreeCube` remains the simulation witness. The two synthetic
`observer_test` waypoint cases were removed because
`assets/scenarios/tests/runtime_waypoint.rhai` already spawns a real rover,
creates two public runtime waypoints, and verifies ordered collision-backed
arrival events plus `RuntimeWaypointStatus`.

This cleanup also removed tests with no maintained production claim: a
dump-only USD probe, historical Rumoca emitter round-trip and bisection suites,
a compile-only Bevy no-op, an intentionally panicking Avian measurement probe,
and a legacy waypoint JSON-shape check. It also removed a known-failing MSL
diagnostic and a duplicate external-bundle presence check; the passing
Modelica source-root admission example remains the maintained MSL contract. The two
selection/drag integration files were also removed because they simulated
state without invoking the production systems and asserted the retired
Shift-select/`DragModeActive` contract. Current schema, parser, lifecycle,
physics, editor-selection, and source-preservation contracts remain covered at
their owners. Editor selection is intentionally not translated into a headless
Rhai scene: `SceneEditPlugin` is UI-gated and exposes no production headless
selection observer. Its owning Rust test now exercises the real shared
selection observer and verifies toggle/highlight state without entering the
separate active-gizmo drag mode.

## Test tiers

1. **Production-owned test discovery.**

   `luncosim test --list` discovers every scene under `assets/scenes/tests/`
   through composed USD, resolves its test Rhai source, and classifies the
   execution domain from the source's top-level literal `TEST_KIND` constant.
   Omission means deterministic headless execution; `TEST_KIND = "graphics"`
   selects the GPU-backed renderer. `scripts/run_scene_tests.sh` consumes this
   result and does not maintain a second scene or graphics classifier.

2. **Production scene gate.**

   ```bash
   ./scripts/run_scene_tests.sh --no-build
   ./scripts/run_scene_tests.sh --no-build -j 4
   ./scripts/run_scene_tests.sh --no-build --exact joint
   ./scripts/run_scene_tests.sh --no-build autopilot
   ```

   This reuses `target/debug/luncosim` and runs each authored scene through
   `luncosim test --scene`, with deterministic `--threads 1 --jitter 0` and a
   real telemetry verdict. Scene materialization and asynchronous Modelica
   participant readiness use a wall-clock liveness budget
   (`--readiness-timeout`, default 420 seconds), not a fixed update count: an
   async compile may require different numbers of `app.update()` calls on
   different machines. The shell gate keeps this startup budget separate from
   its larger `SCENE_TIMEOUT` wall-clock execution backstop (default 900
   seconds), because a valid long-running mission must not be killed while it
   is still advancing. `--max-ticks` remains the simulated-time verdict bound.
   The default mode performs one Cargo build first;
   `--no-build` is the script/USD iteration path. Discovery still resolves the
   authored scene-to-scenario edge before execution. A scene run is a fresh
   headless test process because the current CLI accepts one scene and exits;
   that is separate from rebuilding the Rust core. The runner schedules up to
   four headless production processes concurrently by default; `-j/--jobs N`
   changes that process bound (`-j 1` is useful for serial diagnosis). Every
   gate process still receives `--threads 1 --jitter 0`, so process parallelism
   does not change the deterministic test contract. Graphics scenes remain a
   separate serial GPU/offscreen pass.
   Use `--exact <scene-name>` for the smallest edit-loop run; an unqualified
   argument remains a substring group selector (for example, `joint` also
   matches `g7_joints`).

3. **Live no-restart Rhai checks.**

   Keep one production session running with an explicit API port. Attach or
   replace behavior with `scripts/api/run_scenario.sh`, or run a standalone
   assertion file through `RunRhai`:

   ```bash
   target/debug/luncosim --api 4101 --scene scenes/tests/sensor.usda
   ./scripts/api/run_rhai_test.sh 4101 assets/scripting/tests/test_usd_query.rhai /SandboxScene/Box
   ```

   The helper prepends the generic assertion/USD libraries, delegates transport
   to the native `luncosim rhai --stdout` client, and returns
   `TESTS_OK`/`TESTS_FAIL`. Editing the Rhai
   file and invoking it again needs neither a Rust rebuild nor an app restart.
   Stop the session with the typed API `Exit` command when finished.

4. **Rust mechanism suite.**

   Run focused Cargo tests after Rust changes. The full workspace suite remains
   the final broad check, but it is not the authoring loop for a scene or Rhai
   edit. Rust tests must stay content-agnostic when they exercise scripting:
   dispatch, lifecycle, diagnostics, permissions, cache invalidation and
   teardown are valid; a tutorial's route or a rover's expected distance is not.

   Cargo keeps each integration source file as its own target. Use the wrapper
   to select that target without typing the Cargo target name:

   ```bash
   ./scripts/run_rust_tests.sh -p lunco-modelica --module ast_mut_topology -- --nocapture
   ./scripts/run_rust_tests.sh -p lunco-usd --filter integration_asset_loading::test_sandbox_scene_composes
   ```

   The wrapper maps `--module`/`--file` to `--test <source-file>`, maps a
   `module::test` filter to that same target, uses four Cargo jobs, and enables
   `sccache` when available. Use `--check` for compile-only feedback (it does
   not link or run a test binary); use `--no-run` when linking the actual test
   binary is part of the check. Add `--lib` for inline library tests; with that
   selector, `--no-run` compiles the unit-test harness while `--check` checks
   the library itself. Do not use a bare `cargo test --workspace` as the edit
   loop. Select the owning crate and module/test instead. Run the production
   scene gate only when an authored runtime path is affected; run the broad
   workspace suite at handoff or after a cross-crate change.

## Migration decision table

Move to Rhai when all of these are true:

- the assertion names authored policy or an application outcome;
- the subject is reachable through USD + public commands/queries/events;
- the fixture has a control or anti-trivial movement/measurement guard;
- the scenario emits one terminal verdict and is discovered by the production
  scene runner; and
- the Rust test being removed is not the only negative or exact low-level
  contract for the mechanism.

Keep in Rust when any of these are true:

- it validates parser/AST/schema/source shape before runtime;
- it validates an exact numerical primitive that has no public observation;
- it isolates a lifecycle or authority failure that would be ambiguous in a
  full scene; or
- moving it would require a test-only command/component that production cannot
  use.

Do not migrate by wrapping a Rust test in a Rhai string and executing it from a
Rust harness. That still requires a rebuild and only changes the syntax. The
Rhai test must be an authored asset or a live `RunRhai` source evaluated by the
production binary.

### Rover loader boundary

The rover loader tests follow the same split. `allocation_spec.rhai`,
`drivetrain_parity.rhai`, and `ackermann_parity.rhai` already own the public
runtime outcomes: control-surface allocation, wheel-realization parity,
steering, and real motion. Adding another Rhai copy of those assertions would
duplicate the acceptance gate.

The Rust tests in `crates/lunco-usd/tests/rover_structure.rs` and
`integration_asset_loading.rs` therefore retain only the projection claims that
the production Rhai surface cannot observe precisely: composed USD paths and
schema edges, Avian compound-shape lowering, render-free physics projection,
appearance intent (`Mesh3d`/`PbrLook`), and asynchronous observer ordering. They
select wheel entities through the reflected `WheelRaycast` component and the
canonical `UsdPrimPath`/`visual_entity` links; Bevy `Name` is presentation data,
not an identity selector. If a future public query exposes one of these
mechanism claims end-to-end, move that exact assertion to an authored scene and
remove the Rust duplicate in the same change.

## Remaining migration work

The following are intentionally not deleted until their production replacements
exist:

- `lunco-autopilot` behavior-tree tests that assert exact leaf/kernel semantics;
  production scenes cover route outcomes, not every private node transition;
- `lunco-usd-sim` synthesizer tests for malformed policy result shapes and
  boundary validation; Rust owns the ABI firewall, while policy-specific
  generated topology checks can move only when a live inspectable result is
  available;
- `lunco-cosim` and `lunco-modelica` tests that construct participants directly;
  these protect generic coupling, parser and solver mechanisms, not authored
  mission policy;
- avatar teleport/grid tests, render-to-physics writeback tests, and USD
  projection tests whose public surfaces do not expose the exact frame or
  lifecycle fact they assert;
- orphan or externally-targeted scenario assets, such as
  `assets/scenarios/tests/wheel_sinking_parity.rhai`, until a matching authored
  scene exists. They are not silently counted as production gates.

Every subsequent deletion must name its replacement scene/scenario and retain a
negative or anti-trivial control. If no production surface can expose the
claim, the correct outcome is to keep the Rust mechanism test and document the
boundary—not to weaken the claim to make it scriptable.
