# Handover: deterministic runtime work

**Prepared:** 2026-09-27
**Workspace:** `/home/rod/Documents/luncosim-workspace/lunar-soil`
**Branch:** `codex/lunar-soil`
**D3/D9 physical-input acceptance commit:** `b1747728e`
**D9 direct-command owner test commit:** `211d7d495`
**D9 session-record validation commit:** `d4937baff`
**Latest local main merge commit:** `41f1270e733e9900d54a96d5e1fb928f1c13f1db`

The D9 session-input boundary at `4acf09915` had previously been integrated
across the local worktrees. The 2026-09-27 D3/D9 acceptance commit
`b1747728e` was then merged into local `main` as `da7b244d`; that merge also
retains the newer `main` commits through `c7807d1ca`. The `tutorials`, `usd`,
and `optimization` worktrees have now been fast-forwarded with their unrelated
edits preserved. The tutorial celestial LOD changes stayed in place; the USD
status-bar edit and optimization source, documentation, and profiling edits
were reapplied successfully after fast-forwarding. New task-scoped stashes hold
recovery copies, and the previous stashes remain intact. The five local heads
share the tip containing this handover update. The D9 direct-command owner test
commit `211d7d495` was merged into local `main` as
`7c65820584b846df914cc88f4204bf6340e085bc`; its first parent was `a0dde5fcb`,
the then-current `origin/main`. The D9 session-record validation commit
`d4937baff` was merged as `41f1270e733e9900d54a96d5e1fb928f1c13f1db`, and
`main`, `codex/lunar-soil`, `tutorials`, `usd`, and `optimization` now share
that tip. Tutorial and optimization edits were stashed to advance their
branches and reapplied; both worktrees pass `git diff --check`, and the backup
stashes remain. No push was made. The evidence below stays tied to the
individual builds and source revisions named in each section.

## Active user objective

Implement a robust cross-domain deterministic simulation architecture in
`lunar-soil`: explicit progress admission and deterministic async result commits,
complete causal dependencies and stable ordering across USD, Modelica, SysML,
Rhai, physics, and replay, with production tests and performance evidence;
preserve clean local `main` integration.

Read [`open-deterministic-simulation-contract.md`](open-deterministic-simulation-contract.md)
and [`62-deterministic-runtime-and-async-boundaries.md`](../architecture/62-deterministic-runtime-and-async-boundaries.md)
before selecting the next gap. D25 controller ordering and D26 projection
admission are complete. Whole-session replay, full causal closure, remaining
cadence work, and performance evidence remain open.

## Current progress

### D9: command origin, causal edge trace, and producer identity

Reflected API commands retain `ApiTransport` or Rhai execution context through
`ActiveCommandId` and `CommandOccurred`. Rhai scenario commands also carry the
executing actor's stable `GlobalEntityId`; application-level Rhai calls may
have no actor. `SimulateIntentEdge` preserves origin through
`SemanticIntentEdge` and bounded `CausalTrace`, including actor id, route, phase,
generation, and sequence.

External API, application-Rhai, and direct typed `SimulateIntentEdge` calls
now enter the bounded `lunco-core-session::PendingSessionInputs` queue. The
controller remains the semantic payload consumer. Admission requires a stable
target id, a committed scene generation, and the fixed simulation tick; it
assigns the next tick and per-tick sequence. The fixed-step owner checks that
the stamp is due and that generation and target still match before emitting
the edge ahead of control propagation. `CausalTrace`, `intent.edge`, and the
API acknowledgement carry the correlation id and admission stamp. The
acknowledgement correlation id lets clients query the exact edge while later
edges are being emitted.
Simulation-clock Rhai edges stay in their deterministic hook pass and carry no
external-input stamp.

`SimulatedIntents` keys held state by target, intent, and stable producer
identity. API transport and direct typed callers use a nonzero caller-supplied
`producer_id`; Twin Rhai uses route plus stable actor identity, and actorless
Rhai uses route plus `producer_id`. IDs distinguish producers within each
source class and are not authorization credentials. Releasing one producer
leaves other producers' holds intact. External API, application-Rhai, and direct
typed `SimulateIntent` changes for fixed-simulation targets share the bounded
input queue with `SimulateIntentEdge`. Both action types require producer
identity, a stable target id, and committed scene generation, receive the next
fixed tick and shared per-tick sequence, and are revalidated at the fixed-step
owner before control propagation. Held commits publish `intent.hold`; the
command acknowledgement and event retain the same producer id, correlation id,
and admission stamp. Edges also carry the producer id through `intent.edge` and
`CausalTrace`. Deterministic Simulation Rhai behavior stays in its current pass,
while local-embodiment commands keep their interaction cadence. The fixed-step vessel
controller captures physical `ActionState<UserIntent>` into a by-value
`PhysicalIntentFrame` semantic snapshot, separate from simulated holds until
intent translation. Frame admission requires the local input `SessionId`,
target `GlobalEntityId`, and a committed scene generation. It carries those
identities plus the current `SimTick` and a sequence from the shared per-tick
allocator. Missing admission facts or duplicate target/session order keys hold
the input with a structured runtime error; ordering does not fall back to Bevy
`Entity` bits. The frame is discarded after the pass. An active bounded
`SessionInputStream` retains physical frames and admitted external
`SimulateIntent`/`SimulateIntentEdge` payloads in memory, with producer class,
stable producer id or Rhai route and actor, stable target, generation, tick,
sequence, and correlation where applicable. Raw-file `SpawnEntity` admissions
also share this stream and fixed-tick queue. Document-backed spawns remain
authored `ApplyUsdOps` in the Twin journal. Other commands, durable writing,
and playback remain outside this capture.

Verification for this increment:

- `cargo test -p lunco-controller semantic_edge_is_atomic_target_scoped_and_script_visible -j 4` —
  1 passed, including the typed acknowledgement correlation id.
- The prior full `cargo test -p lunco-controller -j 4` passed all 18 tests,
  including stable admitted tick/order and source-scoped held input coverage.
- `cargo test -p lunco-controller external_held_intent_commits_at_its_admitted_tick_and_publishes_its_stamp -j 4` — passed after the capture extension; it interleaves an edge and held API input, checks no early state change, and verifies shared sequence, stamps, producer, and typed retained payloads.
- `cargo test -p lunco-controller physical_frame_respects_keyboard_capture_without_masking_simulated_holds -j 4` — passed again after making the frame a by-value snapshot; the test releases `ActionState` after capture and confirms the captured semantic state remains stable, while keyboard capture suppresses only physical input and leaves API holds active.
- Current continuation: `cargo test -p lunco-controller -j 4` passed all 23
  tests. The possessed-avatar keyboard test supplies a local `SessionId`, target
  `GlobalEntityId`, and committed scene, then verifies its physical frame
  advances the shared per-tick sequence after an already admitted API edge.
  New negative tests verify that a missing scene generation and duplicate
  target/session order keys hold input and emit structured runtime errors.
- `cargo build -p lunco-luncosim -j 4` — passed after the API acknowledgement
  changes, including this held-input admission increment.
- `rustfmt --edition 2024 --config skip_children=true` on the three changed
  Rust owners — passed after implementation and runtime validation.
- This continuation formatted `crates/lunco-controller/src/lib.rs` and
  `crates/lunco-control-core/src/lib.rs` once after the controller suite passed.
- `python3 scripts/validate_skills.py` — 43 skills valid.
- `git diff --check` and `git diff --cached --check` — passed with the existing
  mixed staged/unstaged work preserved.
- Windowed production `free_flight_speed_boost` attached through the live API
  — `TESTS_OK 10`; current attached run measured 23.1006 m/s base,
  23.0992 m/s diagonal, 163.3417 m descent, and 166.064 m ascent.
- A live API edge to Avatar returned correlation id `1` and admission
  `{scene_generation: 1, effective_tick: 1736, sequence: 1}`. `CausalTrace`
  queried by that exact id and the `intent.edge` telemetry event both reported
  the same stamp; telemetry placed delivery at tick 1736.
- The rebuilt production headless session on port 4103 accepted API
  `SimulateIntent` for the skid rover with correlation id `3` and admission
  `{scene_generation: 1, effective_tick: 1997, sequence: 1}`. Its `intent.hold`
  event reported the identical id and stamp with `U64` correlation and target
  ids; attached `simulate_intent_input_admission.rhai` passed `TESTS_OK 4`.
  This verifies the command/event boundary only; the enclosing
  `physical_rover_controls` fixture still has its independent existing
  undeclared-dependency diagnostics.
- The earlier production headless fixture run passed causal assertions but
  failed three motion assertions at 0 m/s. It is not counted as a passing
  headless gate; the windowed cadence run is separate evidence.

Fresh evidence after the `main` merge:

- `cargo build -p lunco-luncosim --bin luncosim -j 4` — passed on the merged
  tree (`19c1c3e2-dirty`) after adding typed held/edge input records to capture.
- A fresh production API run of `free_flight_speed_boost` passed
  `TESTS_OK 11`; measured base/diagonal speeds were 23.1/23.1001 m/s and
  descent/ascent distances were 163.3417 m.
- The rebuilt production binary passed the updated
  `simulate_intent_input_admission.rhai` gate on port 4122 with `TESTS_OK 8`.
  The API `SimulateIntent` receipt and `intent.hold` event matched correlation
  id 1 and `{scene_generation: 1, effective_tick: 432, sequence: 1}`; the
  completed `ReadSessionInputStream` record retained the same stamp, target,
  `api_transport` producer, and `{intent: "action", held: true}` payload.
  The fixture still emitted its independent undeclared-dependency diagnostics.
  This verifies one admitted API held input and capture lifecycle; the run did
  not exercise physical-key capture or durable replay.
- A follow-up windowed production API run on port 4123 attached that Rhai gate
  to the fixture's authored `ControlsTest` actor and again passed `TESTS_OK 8`.
  The API receipt, `intent.hold`, and completed stream record matched
  correlation id `2` and `{scene_generation: 1, effective_tick: 659,
  sequence: 1}`, target `1975542653690512`, and the `action` held payload.
  Running the gate dynamically on `WorldRoot` was invalid: its declared
  simulation query correctly reported that no live scenario owned the query.
  Injected `W` produced an avatar self-driver edge, but the fixture emitted no
  controlled-vessel physical-frame record, so physical-frame runtime
  acceptance remains open.
- An earlier port 4120 attempt of that gate reported `FAIL`; the fixture's
  repeated steering dependency diagnostic obscured its assertion output. The
  prompt port 4121 rerun passed. Keep the fixture diagnostics separate from
  the D9 result.
- The command reference was regenerated from the merged build's live runtime
  schema: 235 runtime-visible commands across 53 crates.
- The owned sessions on ports 4119, 4120, 4121, 4122, and 4123 were stopped through
  API `Exit`; their processes and listeners are gone. The 4121 process was PID
  142057; the new capture run on 4122 used PID 226406. The two owned 4123 runs
  used PIDs 256878 and 265459. No visual or performance acceptance was run.

Older sessions on port 4103 were also stopped through API `Exit`; they are not
evidence for this merged-main run. No runtime session is currently owned by
this task. No performance measurement was taken.

### D9 producer identity and typed rejection continuation (2026-09-27)

- `cargo test -p lunco-core -p lunco-controller -j 4` passed after the typed
  rejection change: 17 core tests and 24 controller tests. The new core test
  distinguishes `Failed` handler results from terminal `Rejected` results;
  controller coverage exercises producer-scoped admission and capture.
- `cargo build -p lunco-luncosim --bin luncosim -j 4` passed. The live command
  schema reports `SimulateIntent` and `SimulateIntentEdge` as defaulted so
  missing optional inputs reach owner validation. The new binary returned HTTP
  409 for a missing API producer ID and for missing `held`, with explicit
  terminal rejection messages.
- On the production app's owned port 4131, the attached
  `simulate_intent_input_admission.rhai` observer passed `TESTS_OK 11`. It
  verified an actor-backed Twin Rhai submission without `producer_id` and a
  rejected submission when Rhai supplied one. The app's log also reported
  `SIMULATE INTENT INPUT ADMISSION: PASS`.
- API `SimulateIntent` producer 8181 returned correlation 3 and
  `{scene_generation: 1, effective_tick: 1726, sequence: 1}`. `intent.hold`
  telemetry and the completed `ReadSessionInputStream` record retained the
  same values, target `1975542653690512`, and `{intent: "action", held: true}`.
- API `SimulateIntentEdge` producer 9191 returned correlation 4 and
  `{scene_generation: 1, effective_tick: 1743, sequence: 1}`. `CausalTrace`
  queried with that exact correlation reported `api_transport` producer 9191
  and the same admission stamp.
- This `physical_rover_controls` run continues to emit its independent Rhai
  undeclared-dependency diagnostics. The D9 observer verdict passed despite
  those fixture diagnostics. API `Exit` closed port 4131 and process 728693 is
  gone. Port 47123 was left untouched. No visual or performance acceptance was
  run.

The session-owned commit coordinator now drains external semantic inputs at
their assigned fixed tick, validates target and scene stamps, captures records,
and publishes typed commit events synchronously in `(effective_tick, sequence)`
order. Raw-file runtime spawns also commit at their assigned fixed tick and
order. Spawn records retain producer, scene-root and active-frame identities,
catalog entry, exact `f64` pose, correlation, and reserved root GID; the commit
inserts that identity before normal admission and network replication. The
canonical `WorldGrid` uses deterministic content provenance so its active-frame
identity is stable. Document-backed spawns remain `ApplyUsdOps` entries in the
Twin journal. The controller applies held and edge payloads;
`drive_from_bindings` is ordered after this commit set. Physical frames enter
at their consuming fixed tick through the controller boundary and share the
per-tick allocator, but are not deferred through the external queue. See
[`command-journal.md`](../architecture/command-journal.md) for the owner and
record shape. Other typed command payloads, durable recording/playback, and
supported-profile divergence evidence remain open.

### D14: owner context for camera, runtime UI, render, and USD projection policies

Camera presentation policy calls now receive
`Application/Presentation/Preparation`. Runtime UI recording, visibility, and
properties calls receive their typed UI or camera-status presentation context.
The Rhai policies reject off-cycle calls. `publish_exposure` runs in the UI
cycle set, which remains ordering metadata rather than an independent cadence.
The rendering-quality catalog invokes its hooks as
`Application/Presentation/Initialization` at startup and
`Application/Presentation/Preparation` after a committed policy revision. The
shadow-warning policy receives `Application/Presentation/Preparation` from its
`PostUpdate` owner when shadow configuration changes. Those owners remain in
their host schedules; the cycle set does not create a separate cadence.

Verification recorded for these changes:

- `cargo check -p lunco-luncosim-exposures -j 4` — passed.
- `cargo test -p lunco-luncosim-exposures camera_ -j 4` — passed 3 tests.
- `cargo test -p lunco-render profile_contexts_use_the_application_presentation_clock -j 4` — passed, 1 test.
- `cargo test -p lunco-render-recovery configured_shadow_caster_limits_preserve_authored_lights_and_deduplicate_warning -j 4` — passed, 1 test.
- `cargo build -p lunco-luncosim -j 4` — passed.
- `cargo test -p lunco-usd-bevy-runtime-core component_refresh_context_tracks_the_projection_owner -j 4` — passed, 1 test.
- Production API `test_hook_policies.rhai` on the rebuilt empty app — `TESTS_OK 40`.
- Production headless `route_lifecycle` — `PASS 84 assertions`.
- Windowed production `test_camera_presentation_owner.rhai` on `sensor.usda` —
  `TESTS_OK 1`; the active camera was selected through the scheduled owner.
- `python3 scripts/validate_skills.py` — 43 skills valid after the skill edit.
- `rustfmt --edition 2024 --config skip_children=true` on the two render owners
  and USD projection owner — passed.
- `git diff --check` and `git diff --cached --check` — passed after the latest
  documentation and formatting updates.

The `usd.component_refresh` owner now stamps mounted-Twin edits as
`Twin/Lifecycle/Preparation` with the active or committed transition generation
and preview-only edits as `Application/Lifecycle/Preparation`. The focused
runtime-core test validates both route shapes and rejects a missing mounted-Twin
generation; the production `test_hook_policies.rhai` run passed 40 assertions,
including rejection of a policy call outside Lifecycle Preparation. A separate
exploratory run with `sensor.usda` mounted observed the Application camera hook
as unavailable from the Twin-scoped REPL. That sensor session remained not-ready
with pending Altimeter/IMUSensor work and logged a GPU allocation failure, so it
is not a readiness or performance result. The windowed camera-owner test passed
in a separate sensor session.

The most recent empty-app API session on port 4103 was stopped with API `Exit`;
its PID and listener are gone.

Independent UI and visualization cadences remain open. Do not introduce a
second mutable Rhai interpreter schedule; owner cycles need isolated state or
typed queues before that is safe.

### Latest local-main integration review (2026-09-27)

- Fast-forwarded this checkout and the clean `tutorials` worktree from
  `13137c115` to local `main` at `4d8b139f0`. The `main`, `usd`, and
  `optimization` worktrees were already there. The optimization worktree has
  unrelated source, docs, and profiling edits; preserve them. No conflicts or
  pushes occurred.
- Reviewed `54acee747`'s Twin `DocumentSaved` analysis refresh and `4d8b139f0`'s
  possessed route-context production test. `cargo test -p lunco-sysml -j 4`
  passed all 14 tests in 3m09s. The tests cover path matching, refresh-set
  bookkeeping, and existing analysis fences. There is still no production
  Rhai/API gate that saves an indexed source, observes the reload, and checks
  the new Twin analysis revision; this is recorded in
  [`open-deterministic-simulation-contract.md`](open-deterministic-simulation-contract.md).
- `git show --check HEAD~2..HEAD` passed for the two integrated commits.
  The route-context test is a windowed UI acceptance and was not run in this
  review; no visual or performance result is claimed.
- The shared order allocator is owned by `lunco-control-core`, with
  scene-teardown reset there. Core-session owns the bounded queue and ordered
  fixed-tick commit boundary; the controller applies semantic payloads and
  admits physical frames at their consuming tick. Raw runtime spawns enter the
  same queue; physical frames remain controller-boundary admissions.
- After that extraction, `cargo test -p lunco-control-core -p lunco-controller -j 4`
  passed all 7 control-core and 24 controller tests, including same-tick
  sequence sharing by external semantic input and physical frames, forward-tick
  reset, and scene-teardown reset.

### D9 shared session-input queue continuation (2026-09-27)

- `lunco-core-session::PendingSessionInputs` owns bounded semantic payload
  storage and scene-teardown clearing. The queue stores stable target identity,
  producer, origin, scene generation,
  effective tick, sequence, and the typed session payload. The session owner
  validates and resolves each due target, records it when capture is active,
  and synchronously publishes one typed commit event before physical sampling
  while simulation time is running. Paused inputs remain queued for their
  admitted tick after play resumes. The controller applies held and edge
  payloads. Physical frames remain admitted at their consuming fixed tick and
  cannot be deferred through this external queue.
- `cargo test -p lunco-core-session -p lunco-controller -j 4` passed after the
  coordinator change: 24 controller tests, 40 core-session unit tests, and 2
  authz integration tests. The tests cover ordered event publication at the
  assigned tick, shared sequence assignment across ticks, due-tick draining,
  invalid-payload and physical-frame rejection without sequence consumption,
  plugin ownership, and teardown clearing.
- `cargo build -p lunco-luncosim --bin luncosim -j 4` passed in 6m01s. On the
  rebuilt production binary, a live API session on owned port 4192 attached
  `simulate_intent_input_admission.rhai` after its capture reached `recording`,
  then sent API `SimulateIntent` from producer 8181. The acknowledgement,
  `intent.hold`, and completed `ReadSessionInputStream` record matched
  correlation 1 and `{scene_generation: 1, effective_tick: 17, sequence: 1}`;
  the observer reported `TESTS_OK 11` and
  `SIMULATE INTENT INPUT ADMISSION: PASS`. API `Exit` closed port 4192 and
  process 1183357 is gone.
- The vehicle scene's dependency plan and physical input capture are now
  covered by separate production checks. The exact scene-test gate passed
  `physical_rover_controls` (`TESTS_OK 33`, 2,220 ticks) after declaring the
  steering-hinge and front-wheel reads. A headful API observer on port 4196
  passed `physical_input_stream_capture.rhai` (`TESTS_OK 1`), retaining 15
  configured-forward physical-controller frames in a completed 21-record
  stream, with no `ScriptStatus` diagnostics.
- `cargo test -p lunco-core-session -p lunco-controller -p lunco-scene-commands -p lunco-luncosim-edit-core -j 4` passed: 24 controller, 43 core-session, 5 edit-core, and 12 scene-command tests, plus 1 observer and 2 authz integration tests.
- `cargo build -p lunco-luncosim --bin luncosim -j 4` passed. Production `spawn_follows_physics` passed (`TESTS_OK 4`) in the scene runner and through an owned API session loading the raw-file scene. The API run captured one spawn at tick 3, sequence 1, verified producer, exact pose, correlation, scene root, active frame, and reserved root identity, and rejected an overflowing quaternion before admission. API `Exit` stopped the owned process and closed port 4196.
- Raw spawns and semantic controls share the queue and capture stream. Physical
  frames remain controller-boundary admissions; direct-command producer
  acceptance, other typed commands, durable recording/playback, physical-frame
  playback, and supported-profile divergence remain open. No visual or
  performance acceptance was run.

### D3 vehicle dependencies and D9 physical input capture (2026-09-27)

- `assets/scenarios/tests/physical_rover_controls.rhai` now declares its live
  dependency surface: `ReadPorts`, reads of both rover control entities, both
  Ackermann steering hinges and front wheels, and writes to both rover controls.
  The exact production scene-test gate passed
  `physical_rover_controls PASS`, `TESTS_OK 33`, and 2,220 fixed ticks on the
  current production binary. Its log had no undeclared-dependency diagnostic.
- Added `physical_input_stream_capture.rhai` as a headful production observer
  attached with `RunScenarioAsset` to `physical_rover_controls.usda`. On owned
  port 4196 it started capture, acquired the skid control, resolved the
  configured forward binding, injected press/release through the native-window
  event path, then asserted a retained physical-controller frame for that
  stable target with scene generation, effective tick, and sequence. Result:
  `TESTS_OK 1`, 15 matching frames in a completed 21-record stream, and
  `ScriptStatus` `ok:true` with no diagnostics. This covers the application
  input mapping and controller path; it does not claim external hardware input.
- The task-owned API process (PID 1510869) exited through API `Exit`; PID and
  port 4196 were confirmed gone. No Rust source changed, so the existing
  production binary was reused. The earlier requested checkout-local Cargo
  cleanup was already completed; it was not repeated.
- Focused owner test `cargo test -p lunco-controller
  api_and_direct_command_inputs_commit_in_order_with_capture -j 4` passed.
  An unclassified in-process `SimulateIntentEdge` with producer 9091 retained
  acknowledgement 302 and `{scene_generation: 1, effective_tick: 21,
  sequence: 3}` through causal trace, telemetry, and the captured
  `direct_command` record. API and Rhai ingress classify origin, so this
  owner path has no production Rhai/API scenario surface. Other typed
  payloads, durable recording/playback, physical-frame playback,
  supported-profile divergence, cross-domain causal closure, and
  performance evidence remain open.

### D9 session-record validation continuation (2026-09-27)

- `SessionInputRecord::validate` centralizes stable stamp, canonical semantic
  name, physical-frame ordering, runtime-spawn pose, and producer/payload
  pairing checks. `PendingSessionInputs::admit` uses the same validation before
  allocating order, with physical frames reserved for their consuming tick.
  `SessionInputStream::append` validates before retention and moves capture to
  `Failed` on malformed input without dropping earlier records.
- `cargo test -p lunco-core-session
  capture_fails_closed_on_invalid_payload_and_producer_pairing -j 4` passed.
  The package suite passed 44 unit tests and 2 authorization integration
  tests. Durable recording/playback, broader typed commands, physical-frame
  playback, and full cross-domain causal closure remain open.
- The replay feature specification had stale text saying raw-file spawns and
  stable producer identity were outside capture. `specs/020-world-state-and-replay/spec.md`
  and the command-journal contract now describe the current in-memory typed
  payloads and validation boundary; durable input persistence and playback
  remain unimplemented.

## Runtime and repository constraints

- Do not edit authored USD through shell or patch tools. Use the live USD
  document operations for authored changes.
- Runtime checks must use this checkout's production binary on an explicit free
  port. Verify PID, executable, working directory, and listener before control;
  stop only the owned app through API `Exit`.
- Preserve unrelated edits in the optimization worktree and the backup stash.
  Do not reset, repeat Cargo cleanup, or push. The earlier package-scoped Cargo
  cleanup removed only this checkout's selected build outputs; shared Cargo
  caches and source were preserved.
- Report source, scene-test, live API, and visual evidence as distinct claims.
