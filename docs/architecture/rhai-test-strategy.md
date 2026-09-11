> Status: Active · Audience: contributors moving simulation tests between Rust and Rhai

# Test ownership and migration boundary

Rhai tests are the production acceptance layer for authored policy and
observable behavior. Rust tests remain the mechanism layer. The boundary is not
"small test versus large test": it is who owns the decision and whether the
claim can be observed through a public production surface.

| Claim | Owner | Test form |
|---|---|---|
| USD identity, composition, schema shape, asset edges | USD / asset crates | Rust structural tests plus `--validate` |
| Dynamic assembly construction and authored rejection policy | Rhai tools over USD commands/queries | Production scene + authored negative plans |
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
belong in Rust. Motion allocation is no longer a Rust or test-only kernel: each
vehicle's authored Modelica/Rhai program owns its output ports and its live
scene test owns the observable motion result.

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
state without invoking the production systems. Current schema, parser,
lifecycle, physics, editor-selection, and source-preservation contracts remain
covered at their owners. Editor selection is intentionally not translated into
a headless Rhai scene: `SceneEditPlugin` is UI-gated and exposes no production
headless selection observer. Its owning Rust tests exercise the shared
selection observer, replace/extend/remove semantics, and highlight state
without entering the separate active-gizmo drag mode.

Dynamic asset construction follows the same boundary. The generic
`assembly_component_builder` scene starts from an empty USDA frame and checks
component-bundle facts, typed parameter plans, generic reference composition,
and duplicate/invalid identities in Rhai. `assembly_pattern_builder` covers
ordered repeated and local-axis mirror reference plans, including malformed
placement rejection. Rust retains only the generic typed-op and live
reference-materialization mechanisms that Rhai calls; there is no
scene-specific Rust writer or `include_str!` fixture test. Twin-specific model
recipes and their acceptance scenes stay in the owning Twin.

## Test tiers

1. **Production-owned test discovery.**

   `luncosim test --list` discovers every scene under `assets/scenes/tests/`
   through composed USD, resolves its test Rhai source, and classifies the
   execution domain from the source's top-level literal `TEST_KIND` constant.
   Omission means deterministic headless execution; `TEST_KIND = "graphics"`
   selects the GPU-backed renderer, while `TEST_KIND = "editor"` selects the
   production windowed host for document/preview/selection workflows.
   `scripts/run_scene_tests.sh` consumes this result and does not maintain a
   second scene or execution-domain classifier.

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
   ./scripts/run_rust_tests.sh -p lunco-modelica-core --module ast_mut_topology -- --nocapture
   ./scripts/run_rust_tests.sh -p lunco-usd-sim --filter usd_connection_mechanics::rewire_derives_at_load_and_clears
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

## Current authored regression coverage

The production gate is the coverage index for claims that an asset author can
observe through USD, commands, queries, or telemetry. A row may point at more
than one fixture when the positive behavior and its negative control are
separate scenes.

| Contract | Positive production fixture | Negative/control fixture | Observation boundary |
|---|---|---|---|
| Explicit camera, light, composed geometry, collision, and fixed-joint wiring | `assets/scenes/tests/authored_runtime_contracts.usda` + `authored_runtime_contracts.rhai` | `authored_runtime_contracts_negative.usda` + matching Rhai | `QueryUsdPrim`, authored camera track, `Raycast`, `GroundHeight` |
| Composed battery/solar envelopes | `battery_mounts` | `battery_mounts_negative` | `QueryUsdPrim` and world poses |
| Component socket, plug-kind, and joint rejection | `socket_attach_rejection` | same fixture's rejected command cases | public `AttachComponent` |
| Existing-mount nested-frame snap, joint-anchor update, and invalid-frame rejection | `assembly_mount_frame_realign` | same fixture's rejected Rhai plans | dynamic `assembly_builder` plan plus `QueryUsdPrim` |
| Generic component/reference construction, schema-aware regeneration/compliance, deterministic pattern/mirror placement, typed parameter plans, schema-driven property catalogs and dry patches, AI-readable authoring context, and dry placement/attachment planning | `assembly_component_builder`, `assembly_pattern_builder`, `assembly_mount_frame_realign`, `assembly_property_editor` | same Rhai scenarios' invalid asset, missing parent, duplicate parameter/identity, malformed placement, unsupported mode, missing socket, invalid frame, stale generation, wrong type, structural edit, custom-property, and standard-schema mismatch cases | dynamic `assembly_builder`/`assembly_audit`/`assembly_edit` plans, proposal/review/commit, direct standard-USD compliance, and composed `QueryUsdPrim`/`InspectUsdDocument` |
| Reload/reset and event-gated authored policy | `component_detach`, `rhai_event_delivery` | `rhai_event_delivery_negative` | public commands and telemetry |
| Wheel contact, steering, ramp/leg clearance, and vehicle assembly | `drivetrain_parity`, `ackermann_parity`, `sandbox_ramp_placement`, `landing_legs`, `lander_rover_stack` | `rocker_bogie_*_nodiff`, `escape_containment` | authored verdicts over production physics |
| Supported multi-rover stress cardinalities and shared-command motion | `multi_rover_stress_4`, `multi_rover_stress_8`, `multi_rover_stress_20` | `multi_rover_stress_negative` (three-rover unsupported cardinality) | discovered roster, production patrol command, world poses, and terminal Rhai verdict |
| Possession and handoff authority | `tutorial_authority_handoff`, `descent_lander_runtime` | authority-conflict cases in those scenarios | semantic commands, events, and final owner |
| Terrain stream readiness and terrain-progress completion | no repository-owned deterministic DEM fixture | external DEM scenes are not accepted as this branch's authored gate | `TerrainLodStatus` and `ReadExposures` exist; a test-owned DEM/Twin is still required |
| Rigid bodies escaping scene bounds | `escape_containment` | deliberate out-of-bounds body | terminal `physics-body-escaped` verdict; the owning physics boundary also emits one shared `TelemetryEvent` for log/status consumers |

The two new authored contract fixtures deliberately do not assert terrain
stream completion: a flat `Plane` is not a streamed DEM, and treating it as one
would make `TerrainLodStatus` coverage falsely green. Adding a deterministic
terrain case requires a maintained DEM/Twin fixture and its asset-resolution
contract; it does not require a Rust change. The existing wheel, ramp, landing,
and vehicle rows currently expose runtime failures or liveness failures when
they fail, so their fixes belong to the owning Rust/physics/runtime cards rather
than to another authored shim. Headless scene tests also do not require a
renderer-selected active-camera exposure; that presentation fact belongs to a
GPU-backed visual acceptance pass, while the authored camera track and spawned
camera remain observable here.

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

Long USD fixtures and asset-specific setup do not belong in Rust string
constants. Author those facts as `.usda` files beside the production assets and
observe them with an authored Rhai scene. Rust mechanism tests may use the
smallest programmatic stage needed to isolate a lower-level seam that has no
production observation; they must not recreate a full asset or scenario.

### USD simulation boundary

The authored production scenarios own public USD simulation outcomes: composed
rover topology, wheel realization and parameters, drivetrain parity, wiring,
collision/appearance intent, and the resulting physics behavior. The former
large `lunco-usd-sim` asset-loader, rover-structure, mobility, link-occlusion,
and EPS integration targets were removed after their observable claims gained
production query/telemetry coverage. Keeping those assertions in Rust would
make every asset-policy edit relink a separate integration binary without
adding a stronger boundary.

Rust retains only mechanisms that the production surface cannot observe without
inventing test-only APIs: the generic USD-to-`SimConnection` derived-cache
system (`tests/usd_connection_mechanics.rs`), pure wheel-parameter validation in
`src/wheel_params.rs`, and the lower-level USD document/projection
tests in their owning crates. The separate wheel/tire/suspension target contract
is now an authored `wheel_attachment_contract` USD + Rhai gate. Raw authoring
facts that do not require Bevy or Avian stay with
`lunco-usd/tests/live_spawn_projection.rs`, which already owns the document
projection target. If a future public query exposes one of these mechanism
claims end-to-end, move that exact assertion to an authored scene and remove
the Rust duplicate in the same change.

## Remaining migration work

The following are intentionally not deleted until their production replacements
exist:

- `lunco-autopilot` behavior-tree tests that assert exact leaf/kernel semantics;
  production scenes cover route outcomes, not every private node transition;
- `lunco-usd-sim` synthesizer tests for malformed policy result shapes and
  boundary validation; Rust owns the ABI firewall, while policy-specific
  generated topology checks can move only when a live inspectable result is
  available;
- `lunco-cosim` and `lunco-modelica-core` tests that construct participants directly;
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
