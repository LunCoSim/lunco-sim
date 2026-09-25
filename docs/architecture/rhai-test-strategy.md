> Status: Active · Audience: contributors choosing and maintaining test ownership

# Test ownership and language boundary

Rhai tests are the production acceptance layer for authored policy and
observable behavior. Rust tests remain the mechanism layer. The boundary is not
"small test versus large test": it is who owns the decision and whether the
claim can be observed through a public production surface.

| Claim | Owner | Test form |
|---|---|---|
| USD identity, composition, schema shape, asset edges | USD / asset crates | Authored USD + Rhai scene gate; Rust only for a generic reader/parser seam |
| Dynamic assembly construction and authored rejection policy | Rhai tools over USD commands/queries | Production scene + authored negative plans |
| Modelica parsing, AST/source contract, solver/math kernel | Modelica / Rust mechanism | Rust unit and integration tests |
| Avian joint, collider, contact and numerical mechanics | Avian / Rust mechanism | Rust mechanism tests |
| Command dispatch, reflection, script lifecycle, hot reload, authority and teardown | Rust scripting/runtime | Generic Rust seam tests |
| Mission sequencing, route choice, behavior policy, authored tolerances and expected outcomes | Rhai / USD | Production scene + `assets/scenarios/tests/*.rhai` |
| Tutorial steps and application-specific assertions | Rhai observer | Production tutorial scene gate |

Reusable physics acceptance vocabulary is also Rhai-owned. The generic
`assets/scripting/tools/physics_acceptance.rhai` library composes existing
contact, pose, joint-drive, readiness, binding, and runtime-diagnostic reads;
the authored fixture supplies entity paths and thresholds. Rust remains the
owner of the solver and low-level telemetry, so a Rhai acceptance helper never
changes collision policy or becomes a second physics model.

## SysML requirements and authored verification

The architecture keeps source facts, policy, and runtime observation separate:

| Concern | SysML v2 | Rhai / production runtime |
|---|---|---|
| Normative statement and identifier | `requirement def`/usage, `doc`, attributes and constraints | Reads the resolved requirement; does not redefine it |
| Traceability | `satisfy`, `verify`, and standard realization references | Resolves USD prims, Modelica participants and source revisions |
| Verification intent | `verification def`/usage, subject and verified requirement | Selects the existing scene/backend and declares required observations |
| Supported scalar predicate | Constraint expression compiled to the neutral SysML IR | Binds typed provider observations and records `pass`, `fail`, `inconclusive`, or `error` |
| Measurement and actuation | Not a second simulator | Public USD queries, commands, telemetry, Modelica ports and physics facts |
| Verdict and evidence | Supplies source identity and constraint intent | The generic Rhai evaluator emits source-revision and constraint-fingerprint evidence |

The SysML source is the portable requirement contract. A run result is
separate evidence, never an edit to the requirement file. Rust exposes
`ValidateSysml` for source validation and `AnalyzeSysml` for selectable typed
facts; `ReadActiveTwinContract` supplies the active Twin's component and
verification bindings. Separate Rhai policies own structural lint,
requirement/provenance verification, and geometry-constraint selection. The
Rhai observer emits structured evidence through the production verdict
envelope. See
[`24-domain-sysml.md`](24-domain-sysml.md#sysml-v2-requirement-and-verification-contract)
for the source shape and implementation gates.

### Current production surface

The current runtime provides this bounded integration:

1. `lunco-sysml-ast` parses and resolves indexed `.sysml`/`.kerml` sources into
   source-backed elements, references, typed attributes, requirements,
   verifications, constraints, diagnostics and a source-set revision.
2. `ValidateSysml` reports validation status, structural `lint.sysml`
   findings, source diagnostics and identity. `AnalyzeSysml` returns selected
   typed fact tables; it does not assign project-specific meaning to
   constraints or requirement attributes.
3. `ReadActiveTwinContract` exposes Twin-owned component and verification
   records. `sysml_requirements.rhai` joins these to source identities and
   applies requirement/provenance policy.
4. `lint.sysml` applies structural source-quality policy; the independent
   `sysml_modelica_constraints.rhai` tool selects geometry constraint facts
   and assembles Modelica. `sysml_requirements::evaluate` can execute the
   bounded scalar constraint subset through the source-pinned `SysmlModel`
   handle; other policies can consume the same generic facts without adding
   Rust-side rules.
5. `report_structured_verdict` carries requirement identity, source revision,
   observations and evidence while the stable `TESTS_OK`/`TESTS_FAIL` envelope
   remains available. The production `--verification` selector validates one
   mapped case before the run; external clients serialize only at the API
   boundary.

Full KerML expression/constraint execution and automatic requirement-to-USD
projection remain outside the bounded integration. A constraint check does
not implement feature navigation, collection aggregates, full unit conversion,
requirement membership execution, or applicability. Rhai still selects the
provider target and maps USD/telemetry/Modelica values; it must not duplicate a
supported predicate in a Twin-specific boolean.

### Test and requirement ownership

Keep requirement intent, identifiers, units, authored limits, and every
supported acceptance predicate in standard SysML. Keep provider selection,
executable observations, command sequencing, runtime queries, orchestration,
and evidence formatting in the Twin's Rhai observer. This avoids turning
SysML into a second simulator and keeps checks editable without a Rust rebuild.

Use Rust tests only for generic mechanisms the production Rhai/API surface
cannot observe, such as parser lowering, serialization, schema composition,
and lifecycle invariants. Do not duplicate an observable behavior assertion in
Rust merely because its implementation is Rust. Do not wrap a Rust test in a
Rhai string and execute it from a Rust harness.

The authored test shape is Rhai, with SysML as its input contract. Use the
generic evaluator so a numeric limit is read from SysML rather than copied into
the script:

```rhai
let source = sysml_requirements::source();
let result = sysml_requirements::evaluate(source, [
    #{ id: "MASS-001", component: "rover",
       requirement: "ExampleRequirements::REQ001_MassBudget",
       verification: "ExampleRequirements::Verify_REQ001",
       kind: "attribute", path: "/Vehicle/Rover",
       attr: "mass", expected_attr: "massBudgetKg", tolerance: 0.001 }
]);
report_structured_verdict(result, "MASS REQUIREMENTS", "MASS_REQUIREMENTS");
```

For a source-defined predicate, use `constraint_check` and bind every input
explicitly. Missing values remain inconclusive; unknown parameter names and
invalid constraints remain errors. `evaluate_document(source, doc_id, checks)`
uses the same generic check table against an explicit open Editor document.

```rhai
let result = sysml_requirements::evaluate(source, [
    sysml_requirements::constraint_check(
        "PAYLOAD-001", "lander", "payloadCapacity", "VerifyPayloadCapacity",
        "ExampleRequirements::PayloadWithinCapacity",
        #{payloadKg: #{provider: "usd", state: "value", value: observed_payload_kg},
          capacityKg: #{provider: "source_literal", state: "value", value: capacity_kg}},
        "derived", 0.0)
]);
```

The Rust surface needed to support this is intentionally small: one
read-only SysML snapshot/query registration, one source-revision/key handoff,
and reuse of the existing Rhai scene runner and result protocol. No
requirement-specific Rust test module, Rust-side threshold, or second runner
should be introduced.

Use `ValidateSysml` for parse/source validation and `AnalyzeSysml` when a
policy needs semantic facts. `AnalyzeSysml` accepts an optional `tables` list
(`elements`, `references`, `relationships`, `constraints`, `attributes`,
`requirements`, `verifications`, or `diagnostics`) and optional
`attribute_names` selection. It uses the existing Twin-indexed source set and
manifest `[sysml]` roots rather than a second filesystem walker. Tables and
values cross the in-process boundary as typed `HookValue` structures; no JSON
copy is used inside Rust/Rhai. Qualified names remain authoritative, while
policies explicitly handle ambiguous short names.

`sysml_requirements::source()` requests requirement and verification facts,
then joins them with `ReadActiveTwinContract`. Its selected-attribute and
constraint-source variants request only the needed tables. `lint.sysml`,
`sysml_requirements.rhai`, and `sysml_modelica_constraints.rhai` are distinct
policy layers over these shared facts, not parallel Rust projections.

For component-level suites, `sysml_requirements::evaluate(source, checks)`
returns one structured result per check. The shared
`report_structured_verdict(report, title, channel)` prelude helper emits those
results and failures as a telemetry evidence map, then retains the normal
greppable PASS/FAIL line. Evidence collection stays generic while the Twin
still owns its check table and scene observations.

Acceptance contracts are Twin-authored: a Twin keeps its `.sysml` source,
scenario `.rhai`, and any fixture-local policy together. Core ships only the
generic query and typed projection; core runtime code must not contain
product-specific requirement IDs, thresholds, scenes, or acceptance assertions.

The rule is: move a Rust assertion only after an authored fixture can fail for
the same reason through the public runtime path. A Rust test that supplies a
spy command, fake world, private component, or direct function call is not a
production behavior test merely because Rhai appears in its source.

## What has moved in this wave

Asset-specific annotation appearance moved out of
`crates/lunco-usd-bevy/tests/material_binding_test.rs`. The production
`assets/scenes/tests/waypoint_visual.usda` scene composes the route-point,
landing-location, and predicted-landing assets, while
`assets/scenarios/tests/waypoint_visual.rhai` checks their composed shader
channels, opacity, collision intent, and shadow policy through `QueryUsdPrim`.
The Rust target retains only generic `PbrLook` projection cases such as opacity,
masking, additive blending, malformed values, and workflow binding; changing a
marker asset no longer requires a Rust fixture rebuild.

Generic scripting behavior is exercised through authored production scene
gates. The scripting crate does not maintain a second Rust integration harness
for scenario product behavior; low-level Rust tests, when needed for mechanisms
that authored runtime tests cannot observe, use inline fixtures at their
owning crate boundary.
Convergent journal ordering follows the same split: `lunco-twin-journal` keeps
the generic comparator and fallback mechanism test, while the production Rhai
hook probe reflects the typed `journal.merge.order` contract. The networking
adapter tests only its domain filtering and replay boundary; they do not embed
the Rhai runtime to repeat journal-policy coverage.
The task/mission semantics are exercised by the production
`scripting_task_contract` scene and
`assets/scenarios/tests/scripting_task_contract.rhai`; the Rust harness no
longer embeds one test script per task combinator. Route composition and progression are authored by the scene-level
`assets/scenarios/route_follow.rhai` program and observed by the production
scene scenarios. The `route_lifecycle` scene gate covers the current route
contract in one deterministic Rhai acceptance pass: source-asset program
attachment, add/move/delete (including referenced-point deactivation), empty
and recovered ribbons, semantic start/stop, invalid edits, and sensor-driven
arrival. There is no Rust test or Rust runtime path for a vessel-owned waypoint
list.

The same rule now covers the scripting asset surface itself. The production
`scripting_asset_contracts` scene validates the active prelude and policies
through `ScriptingCatalog` and `ValidateAsset`, and exercises the shipped Rhai
tool libraries through their registered modules. Rust no longer enumerates or
opens `assets/scripting/` from a package integration test; standalone probes in
`assets/scripting/tests/` are run by `scripts/api/run_rhai_test.sh` when a live
bridge is required.

The same boundary applies to authored physics and editor outcomes: when a
public USD/query/event surface can observe the claim, the acceptance assertion
belongs in its scene's Rhai observer. Rust keeps only the generic document,
projection, parser, lifecycle, and numerical mechanism tests that cannot be
observed without inventing a test-only API.

The shipped parametric-surface contract follows the same rule:
`assets/scenes/tests/parametric_surface.usda` references the antenna reflector
and lander nozzle directly, while
`assets/scenarios/tests/parametric_surface.rhai` checks their composed
`LunCoLatheAPI` schemas and parameters through `QueryUsdPrim`. The visual crate
therefore does not open shipped asset paths from a Rust test.

Tests with no maintained production claim are not part of the active suite.
Current schema, parser, lifecycle, physics, editor-selection, and
source-preservation contracts remain covered at their owners. Editor selection
is UI-gated, so native scene gestures run through the production windowed
editor gate rather than a headless mock. Readiness alone does not prove that a
requested scene opened: the editor runner also checks that the scene's USD root
is present in the live entity registry before waiting for its verdict. The
`route_interaction` scene observes the secondary click through the public
`scene.pointer` event, checks its `route.context` intent and authored USD
pointer policy, verifies prior selection remains unchanged, and then invokes
the explicit menu action that selects the waypoint. Rust tests retain generic
typed selection-command and gizmo mechanisms; Rhai owns route/selection policy.

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
   The production discovery API lives in `lunco-scene-validation`, alongside
   the asset and stage validation it reuses; the scene mutation crate does not
   own test inventory.
   Omission means deterministic headless execution; `TEST_KIND = "graphics"`
   selects the GPU-backed pixel-capture renderer, `TEST_KIND =
   "render-contract"` selects the GPU-backed offscreen host for render
   diagnostics that intentionally do not produce a pixel take, and `TEST_KIND =
   "editor"` selects the production windowed host for document/preview/selection
   workflows.
   Discovery is recursive. Put independent windowed fixtures in nested
   one-scene directories so each Editor run mounts only its fixture Twin and
   sibling editor sessions cannot mutate the same preview state.
   `scripts/run_scene_tests.sh` consumes this result and does not maintain a
   second scene or execution-domain classifier.

2. **Production scene gate.**

   ```bash
   ./scripts/run_scene_tests.sh --no-build
   ./scripts/run_scene_tests.sh --no-build -j 4
   ./scripts/run_scene_tests.sh --no-build --exact joint
   ./scripts/run_scene_tests.sh --no-build route
   ```

   This reuses `target/debug/luncosim` and runs each authored scene through
   `luncosim test --scene`, with a single-Compute fixed-step profile
   (`--threads 1 --jitter 0`) and a real telemetry verdict. This controls the
   pool width and time-step sequence but does not prove identical outcomes.
   Scene materialization and asynchronous Modelica
   participant readiness use a wall-clock liveness budget
   (`--readiness-timeout`, default 420 seconds), not a fixed update count: an
   async compile may require different numbers of `app.update()` calls on
   different machines. The shell gate keeps this startup budget separate from
   its larger `SCENE_TIMEOUT` wall-clock execution backstop (default 900
   seconds), because a valid long-running mission must not be killed while it
   is still advancing. `--max-ticks` limits cumulative clock-admitted fixed
   steps across scene replacements; it does not reset with the scene-local
   `SimTick` clock.
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
   Each gate process owns a fresh Bevy app, document registry, workspace, and
   Rhai state. Isolation is the default contract for scene, render, and editor
   acceptance runs: launchers keep settings in memory and runtime overlays are
   neither restored nor written, regardless of Twin policy. The production
   scene runner also fails before scenario start if a file-backed authored USD
   document is already dirty. Fixtures that edit documents resolve their own
   scene document and assert `dirty == false` before the first authored
   operation; they do not select an arbitrary open-document slot.
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

When a Twin needs to explain a handoff or admission failure, the generic
`QueryPhysicsState { id }` provider exposes body mode, linear/angular velocity,
computed mass/centre-of-mass/principal inertia, sleeping/readiness/admission
markers, initialization pending/invalid state, pose-seeded and pose-authoritative
markers, collider and disabled state, plus any published support footprint. It is
a read-only diagnostic companion to `QueryEntity` and `QueryUsdPrim`; it does
not encode rover or lander policy and can be consumed by Rhai, HTTP, or MCP.

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
| Generic component/reference construction, schema-aware regeneration/compliance, deterministic pattern/mirror placement, typed parameter plans, schema-driven property catalogs and dry patches, AI-readable authoring context, dry placement/attachment planning, and backend-neutral model-state invalidation | `assembly_component_builder`, `assembly_pattern_builder`, `assembly_mount_frame_realign`, `assembly_property_editor`, `model_state_invalidation` | same Rhai scenarios' invalid asset, missing parent, duplicate parameter/identity, malformed placement, unsupported mode, missing socket, invalid frame, stale generation, wrong type, structural edit, custom-property, standard-schema mismatch, independent instance, source-preservation, and parameter-rebuild cases | dynamic `assembly_builder`/`assembly_audit`/`assembly_edit` plans, generic `ModelStateRevision`, standard USD instance overrides, proposal/review/commit, direct standard-USD compliance, and composed `QueryUsdPrim`/`InspectUsdDocument` |
| Reload/reset and event-gated authored policy | `component_detach`, `rhai_event_delivery` | `rhai_event_delivery_negative` | public commands and telemetry |
| Wheel contact, steering, ramp/leg clearance, and vehicle assembly | `drivetrain_parity`, `ackermann_parity`, `sandbox_ramp_placement`, `landing_legs`, `lander_rover_stack` | `rocker_bogie_*_nodiff` | authored verdicts over production physics |
| Supported multi-rover stress cardinalities and shared-command motion | `multi_rover_stress_4`, `multi_rover_stress_8`, `multi_rover_stress_20` | `multi_rover_stress_negative` (three-rover unsupported cardinality) | discovered roster, authored Rhai route/control policy, world poses, and terminal Rhai verdict |
| Possession and handoff authority | `tutorial_authority_handoff`, `descent_lander_runtime` | authority-conflict cases in those scenarios | semantic commands, events, and final owner |
| Terrain stream readiness and terrain-progress completion | no repository-owned deterministic DEM fixture | external DEM scenes are not accepted as this branch's authored gate | `TerrainLodStatus` and `ReadExposures` exist; a test-owned DEM/Twin is still required |
| Rigid bodies escaping scene bounds | `escape_containment` | deliberate out-of-bounds body plus a grounded control body | required Rhai `physics.body_escape` policy pauses the escaped dynamic joint island; the control body settles under the continuing solver |

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

## Test ownership decision table

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

The same rule applies to shader assets. A Rust integration test that opens a
named file under `assets/shaders/` is an asset test even when it reaches the
file through `lunco-assets-core` or `lunco-storage`; changing the reader does
not change ownership. WGSL schema facts are exposed by `ValidateAsset` and are
asserted by `assets/scenes/tests/shader_asset_contracts.usda` with its Rhai
observer. Rust keeps only inline `ParamSchema`/packing tests that do not name a
repository asset.

Public USD document query contracts are exercised through the production
`usd_query_api` scene gate: `InspectUsdDocument`, `ResolveUsdTarget`, and
`SyncUsdDocument` read an authored fixture and observe actual journal-backed
edits, reference composition, cursor rejection, and history-window recovery.
`InspectUsdEditSession` is covered by the assembly proposal lifecycle gate.
This keeps document/query behavior in the same authored USD/Rhai path used by
the application instead of constructing a second provider-only world in Rust.

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
`lunco-usd-sim-authoring/src/wheel_params.rs`, and the lower-level USD document/projection
tests in their owning crates. The separate wheel/tire/suspension target contract
is now an authored `wheel_attachment_contract` USD + Rhai gate. Raw authoring
facts that do not require Bevy or Avian stay with
`lunco-usd-viewport-runtime/tests/live_spawn_projection.rs`, which already owns the document
projection target. If a future public query exposes one of these mechanism
claims end-to-end, move that exact assertion to an authored scene and remove
the Rust duplicate in the same change.

## Mechanism tests that remain in Rust

Keep Rust coverage for these implementation-owned boundaries until an existing
production surface can prove the exact contract without a test-only API:

- `lunco-usd-sim` synthesizer tests for malformed policy result shapes and
  boundary validation; Rust owns the ABI firewall, while policy-specific
  generated topology checks can move only when a live inspectable result is
  available;
- `lunco-cosim` and `lunco-modelica-core` tests that construct participants directly;
  these protect generic coupling, parser and solver mechanisms, not authored
  mission policy;
- render-to-physics writeback tests and USD projection tests whose public
  surfaces do not expose the exact frame or lifecycle fact they assert;
- orphan or externally-targeted scenario assets, such as
  `assets/scenarios/tests/wheel_sinking_parity.rhai`, until a matching authored
  scene exists. They are not silently counted as production gates.

Every subsequent deletion must name its replacement scene/scenario and retain a
negative or anti-trivial control. If no production surface can expose the
claim, the correct outcome is to keep the Rust mechanism test and document the
boundary—not to weaken the claim to make it scriptable.
