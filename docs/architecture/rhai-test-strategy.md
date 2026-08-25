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

## Test tiers

1. **No-build asset graph preflight.**

   `scripts/run_scene_tests.sh` discovers every scene under
   `assets/scenes/tests/`, skips only scenes carrying the authored
   `lunco:notHeadlessTestable` reason, and verifies every headless
   `@lunco://scenarios/tests/*.rhai@` edge exists. The same contract is checked
   by `lunco-scene-commands`; the shell mirror exists so `--no-build` can fail
   before launching anything.

2. **Production scene gate.**

   ```bash
   ./scripts/run_scene_tests.sh --no-build
   ./scripts/run_scene_tests.sh --no-build autopilot
   ```

   This reuses `target/debug/luncosim` and runs each authored scene through
   `luncosim test --scene`, with deterministic `--threads 1 --jitter 0` and a
   real telemetry verdict. The default mode performs one Cargo build first;
   `--no-build` is the script/USD iteration path. A scene run is still a fresh
   headless test process because the current CLI accepts one scene and exits;
   that is separate from rebuilding the Rust core.

3. **Live no-restart Rhai checks.**

   Keep one production session running with an explicit API port. Attach or
   replace behavior with `scripts/api/run_scenario.sh`, or run a standalone
   assertion file through `RunRhai`:

   ```bash
   target/debug/luncosim --api 4101 --scene scenes/tests/sensor.usda
   ./scripts/api/run_rhai_test.sh 4101 assets/scripting/tests/test_usd_query.rhai /SandboxScene/Box
   ```

   The wrapper prepends the generic assertion/USD libraries, sends the source
   through the live API, and returns `TESTS_OK`/`TESTS_FAIL`. Editing the Rhai
   file and invoking it again needs neither a Rust rebuild nor an app restart.
   Stop the session with the typed API `Exit` command when finished.

4. **Rust mechanism suite.**

   Run focused Cargo tests after Rust changes. The full workspace suite remains
   the final broad check, but it is not the authoring loop for a scene or Rhai
   edit. Rust tests must stay content-agnostic when they exercise scripting:
   dispatch, lifecycle, diagnostics, permissions, cache invalidation and
   teardown are valid; a tutorial's route or a rover's expected distance is not.

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
- orphan or externally-targeted scenario assets, such as
  `assets/scenarios/tests/wheel_sinking_parity.rhai`, until a matching authored
  scene exists. They are not silently counted as production gates.

Every subsequent deletion must name its replacement scene/scenario and retain a
negative or anti-trivial control. If no production surface can expose the
claim, the correct outcome is to keep the Rust mechanism test and document the
boundary—not to weaken the claim to make it scriptable.
