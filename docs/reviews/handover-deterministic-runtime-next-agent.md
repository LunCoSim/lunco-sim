# Handover: deterministic runtime work

**Prepared:** 2026-09-28
**Workspace:** `/home/rod/Documents/luncosim-workspace/lunar-soil`
**Branch:** `codex/lunar-soil`
**D3/D9 physical-input acceptance commit:** `b1747728e`
**D9 direct-command owner test commit:** `211d7d495`
**D9 session-record validation commit:** `d4937baff`
**D9 archive feature commit:** `935a527322bc9ab8c753920aa1e31ee7cf1059ca`
**D9 archive merge commit:** `61737e56e645ab58f3b033c5e2968c0bc119dc75`
**Latest observed `origin/main` before this handover refresh:** `79b3546c1`

The D9 session-input boundary at `4acf09915` and its 2026-09-27 acceptance,
direct-command, record-validation, and archive-codec commits are integrated on
local `main`. Archive feature commit `935a52732` was merged with first-parent
history preserved as `61737e56e` (base `18a826e21`). Local `main` later gained
the optimization and terrain-streaming integrations (`14b88558f`,
`2828983f1`, `e00664d88`, `eaa9baded`, and `9768cc1d1`), then advanced to
`6b8258edf` for the generated `.tracy` ignore rule. All five worktrees were
then at `6b8258edf`; this handover-only update was committed locally and
fast-forwarded across `main`, `codex/lunar-soil`, `tutorials`, `usd`, and
`optimization`. The tutorial celestial LOD edits and optimization source and
documentation edits remain in their worktrees; the tutorials backup stash
`d22f8f25d` and earlier recovery stashes remain intact. The USD worktree is
clean. A Tracy build was active in optimization at the last check and was not
interrupted. At that earlier handover, the last observed `origin/main` was
`6b8258edf`; its reflog shows
an `update by push` during this continuation, but this agent did not run a
push. The evidence below stays tied to the individual builds and source
revisions named in each section.

**Latest local integration (2026-09-28):** merge commit `544c2991e` integrates
`7170e77f7` into local `main` at `80f28c140`; the merged GUI runtime advertised
a settled schema of 242 commands and the command reference was regenerated in
`8931cd0ab`. Local `main` then advanced with the optimization telemetry
projection commit `bb4eab1b5`. The task branch added local port-inspector input
admission in `382db427a` and fast-forwarded onto `main`. Main then merged
optimization commit `b68da02ba` as `52a5922c3`, reusing one canonical owner
reader and child snapshot for control and program projection. The task branch
fast-forwarded to that merge, then `e648adef6` fixed the shader-contract Rhai
gate's unsigned `uniform_size` comparison and was fast-forwarded onto `main`.
At that checkpoint both branches pointed to `e648adef6`. The
post-`382db427a` production build
`cargo build -p lunco-luncosim --bin luncosim --features tracy -j 4` passed.
The owned API session used PID 307181 on port 4733 and exited through API
`Exit`; the port is closed. At that checkpoint local `main` was clean and 13
commits ahead of the observed `origin/main` at `79b3546c1`; no push was made.

**Continuation integration (2026-09-28):** `main` advanced by 20 commits after
`e648adef6`; `codex/lunar-soil` was fast-forwarded to `54ad69a32`
(`perf(ui): reduce entity tree snapshot scans`). The task handover draft and
untracked `scripts/perf/` capture were preserved. The integrated delta includes
off-thread engine/Twin dataset discovery with Twin results committed in scan
order, bounded Twin-policy preparation held through lifecycle activation, and
stable path-ordered USD child admission under count/time budgets. The relevant
async owners and updated architecture docs were reviewed; `git diff --check`
passed and `python3 scripts/validate_skills.py` passed all 43 skills. No source
tests or runtime acceptance were run in this continuation. Local `main` is
clean and both local branches point to `54ad69a32`. At the start of this
continuation local `main` was 33 commits ahead of `origin/main`; during review,
the shared reflog recorded `origin/main` advancing to `54ad69a32` by an
`update by push` at 07:07 +02:00. This agent did not run a push. The task checkout has no
`target/` directory and only 2.0 GiB free, while the sibling `main/target/` is
25 GiB. Do not clean that sibling target. At the last process check, a separate
`main` session (PID 1070084, port 49733) and a `terrain` session (PID 1071139,
port 49732) were active and left untouched; recheck session ownership before
runtime work.

**Follow-up integration (2026-09-28):** local `main` advanced four commits
from `54ad69a32` to `5ac91bf93` and `codex/lunar-soil` was fast-forwarded to
that head. The commits coalesce celestial projection work, keep USD telemetry
progress in its owner index, derive entity-tree ancestry only for named nodes,
and read initial DEM bridge facts from the prepared projection plan. The task
branch is now four commits ahead of `origin/main`; no push was run. The incoming
range and integrated worktree pass `git diff --check`. These changes have not
been cargo-tested or runtime-validated in this continuation. The task target is
still absent and free space fell to 1.1 GiB; `main/target/` remains 25 GiB and
must not be cleaned. The latest process scan found no Cargo, luncosim, or Tracy
capture process. Optimization, USD, tutorials, and terrain worktrees have
unrelated dirty files and were left untouched.

**Continuation integration (2026-09-28):** local `main` advanced one commit to
`9b86bf8ff` (`perf(camera): queue camera path discovery`), and the task branch
was fast-forwarded. The change queues inserted USD prim identities and stage
generation/asset invalidations, then retries candidates with pending runtime
prerequisites. The camera-path math tests are present, but this merge has not
been runtime-validated in this checkout. `cargo test -p lunco-usd-bevy-camera
-j 4` passed all 46 tests. The `camera_path` tests cover interpolation and aim
blending only; the repository has no authored scene fixture with
`lunco:path:camera`, so the new discovery and retry lifecycle remains without
runtime acceptance. Local `main` and the task branch are now five commits ahead
of `origin/main`; no push was made.
There are 31 GiB free and this checkout has no `target/`, so no cleanup was
needed. A Cargo build is active in the separate USD worktree (PID `1530551`),
and the terrain app remains active on port `49732` (PID `1440998`); neither was
controlled or stopped. Existing unrelated changes in optimization, tutorials,
and terrain worktrees remain untouched.

**Continuation integration (2026-09-28):** local `main` advanced one commit to
`90fa387fe` (`perf(usd-sim): select bounded prim work prefix`), and the task
branch was fast-forwarded. The owner now selects 32 prims before sorting the
selected prefix. Review lead: equal authored prim paths are ordered by Bevy
`Entity` bits; the same path can occur in distinct loaded stages, so check
whether a stable stage/scene identity should qualify this ordering before
claiming cross-run determinism. No behavior regression has been demonstrated.
A production Cargo build is active in the separate terrain worktree (PID
`1647373`), and the terrain app remains on port `49732` (PID `1440998`); neither
was controlled. This checkout has a 2.6 GiB target and 23 GiB free, so no
cleanup is needed. The task branch and local `main` are six commits ahead of
`origin/main`; no push was made.

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

### D9 bounded session-input archive codec (2026-09-27)

- Added `SessionInputCaptureArchive` to `lunco-core-session`. Its versioned
  binary framing accepts at most 65,536 validated, strictly ordered records
  and 16 MiB; decoding checks the header, exact payload consumption, record
  count, semantic payloads, and producer pairing. The wire projection uses
  ordinary typed enum variants so the existing internally tagged Serde enums
  do not leak JSON or fail the binary codec. It preserves exact `f64` spawn
  poses and all current producer and payload variants.
- `cargo test -p lunco-core-session -j 4` passed 49 unit tests and 2 authz
  integration tests. Archive tests cover variant round-trip, invalid decoded
  records, bad magic/version/count/length, trailing payload bytes, and the byte
  limit. `SessionInputStream::begin` rejects configured limits above the
  archive's record cap.
- The archive is an encode/decode capability only. Runtime capture remains
  memory-backed; no storage writer, baseline manifest, or playback consumer is
  installed, so whole-session replay remains open.

### D9 replay-baseline USD owner snapshot (2026-09-27)

- `StageRecipe::content_closure` in `lunco-usd-compose` now freezes every
  successfully fetched root/dependency layer as a sorted `(logical layer id,
  CIDv1 raw/SHA-256)` row. It rejects absent root bytes, unresolved dependency
  diagnostics, and empty layer identifiers. This does not use the volatile
  `UsdStageRevision` counter and does not yet assemble an application baseline.
- Focused tests cover stable ordering/content IDs and fail-closed partial
  recipes. The remaining baseline owners and the capture/playback integration
  still need implementation.

### Replay-baseline source owner audit (2026-09-27)

- `SysmlAnalysis::content_closure()` supplies sorted CIDv1 raw/SHA-256
  identities for the exact project source files and embedded standard library
  represented by an analysis. It rejects parser/resolution diagnostics and
  empty or duplicate logical names. `source_revision` and
  `source_fingerprint` remain 64-bit FNV analysis/cache values.
- `cargo test -p lunco-sysml-ast content_closure -j 4` passed all four focused
  closure cases. Build identity now lives in `lunco-core::BuildIdentity`; the
  headless runtime stamps it when the host does not provide one, and the
  focused runtime test verifies host identity preservation. Application-level
  baseline assembly still does not consume either owner snapshot.
- `RhaiScenarioRuntime::active_content_closure(entity)` returns sorted
  CIDv1 raw/SHA-256 identities for the exact root, source-backed literal
  imports reachable from the root and authored prelude, and the prelude files
  captured with that entity's last committed program. Inline roots use a
  CID-derived identity. Missing source text or duplicate source identities
  produce a closure error without changing Rhai execution. `RhaiSource` assets
  retain their loaded dependency handles. A
  session-wide snapshot joining active roots to complete closures remains open;
  baseline capture must not infer roots from the global script registry.
- `cargo test -p lunco-scripting-rhai-world rhai_content_closure -j 4` passed
  both focused checks for exact content IDs, stable ordering, changed bytes,
  transitive prelude imports, and fail-closed missing or duplicate source
  identities.
- Immutable Modelica source preparation addresses each exact root file with a
  portable URI and CIDv1 raw/SHA-256 identity before the compiler commit;
  `PreparedSourceRoot::content_closure()` exposes the prepared result. The
  compiler exposes current source content by source-set id; a failed latest
  admission and parsed-only source bundles return explicit errors. Per-root
  capture does not yet join the active model document and all compile
  dependencies. The focused compiler tests cover ordering, exact bytes,
  root-relative paths, portability errors, and failed latest admission state.
  `cargo test -p lunco-modelica-compiler source_root_content_closure -j 4`
  passed all three focused checks.
- `lunco-core::BuildIdentity` is shared across hosts. `LunCoSimRuntimePlugin`
  installs the stamped build constants when the host has not supplied an
  identity, so headless and GUI baselines can name the software revision.
  The live solver facts snapshot is implemented; initial runtime-state capture
  and the composite collector remain unimplemented.

### D9 replay-baseline live Modelica solver snapshot (2026-09-27)

- `ModelicaLiveSolverSnapshot` now travels in `ModelicaResult` and is retained
  by `ModelicaModel`. The worker builds it from the same `LiveBuildPlan` as the
  stepper: canonical solver id, registered capabilities, live/predicted
  profile, backend-neutral f64 settings, and name-ordered instance overrides.
- Native and browser worker paths retain it in the compiled-model cache and
  return it on successful compile, reset, parameter-update, and Step results,
  including Step auto-init. A new compile clears the old snapshot; the Bevy
  bridge applies it after session fencing; new compile results also pass the
  source-generation check, and failures clear the snapshot.
- `cargo test -p lunco-modelica-worker solver -j 4` passed 3 tests, including
  resolved-plan settings/override order and response-owner retention/clearing.
  `cargo test -p lunco-modelica-worker stale_compile_result_keeps_active_model_held_for_current_revision -j 4`
  passed and confirms a stale result carrying a snapshot is not retained.
- `cargo check -p lunco-modelica-worker --target wasm32-unknown-unknown -j 4`
  passed. It emitted dead-code warnings in the unchanged
  `lunco-assets-runtime::scripting::collect_toml_sources` and
  `lunco-modelica-core::modelica_lint::MAX_ACTIVE_LINTS`.
- The source snapshot does not include initial live variable/input state and
  does not assemble active model source with its full compile dependency
  closure. Baseline collection, durable storage, and playback remain open.

The 2026-09-28 continuation confirmed that no session-wide baseline collector
or initial authoritative-state snapshot has been added. The next replay step is
to map the application/runtime readers for the existing USD, Rhai, SysML,
Modelica, build, physics, and identity owner facts into one fail-closed typed
baseline boundary, then connect that baseline to capture/export before adding
playback. Do not infer active script roots from the global registry or treat a
solver configuration snapshot as its initial live state.

The owner audit identifies these inputs: `UsdStageAsset.recipe` supplies the
base `StageRecipe` closure, while `CanonicalStage::generation()` distinguishes
later live authored edits; a recipe-only hash cannot stand in for the current
authored document after that generation advances. `SysmlAnalysis::content_closure()`
and `RhaiScenarioRuntime::active_content_closure(entity)` already provide
source-owned exact closures. `ModelicaModel` retains the compiled source
generation, solver configuration, current variables, inputs, and model time,
but not a complete initial Rumoca stepper state or the exact assembled compile
closure. `PhysicsComputeProfile` records pool width as an execution profile,
not physics state or a determinism guarantee. `BuildIdentity` supplies the
host's product/source stamp. The collector must join these through the active
scene and stable entity identities, and fail if any current owner snapshot is
missing or stale.

**Replay baseline boundary audit (2026-09-28):** `StartSessionInputCapture`
currently begins `SessionInputStream` using only its record limit, and archive
version 3 contains validated input records without a baseline. Keep
`lunco-core-session` domain-agnostic: it should own the typed baseline value,
capture lifecycle, validation, and archive framing, while the application/domain
composition gathers facts from each authoritative owner. Capture must bind the
snapshot to the same committed scene generation and simulation tick as its
first record; a cached closure or solver profile is insufficient if its source
owner has changed. The current Modelica snapshot is still missing initial
stepper state, so a fail-closed collector cannot yet claim complete replay
readiness. Do not start playback until the archive round-trips the baseline and
the production path verifies restore before applying the first ordered input.

At the prior process check, PID `1440998` was running the production binary
from the separate `/home/rod/Documents/luncosim-workspace/terrain` checkout on
API port `49732`; it was left untouched. This task checkout then remained at
`9b86bf8ff`, five commits ahead of `origin/main`, with no push by this agent.
Its `target/` was absent; `/home` had 31 GiB free.

The Modelica runtime-state audit traced the live solver to the worker-owned
`LiveStepper`. Its current adapter exposes observable `SessionState`, time,
inputs, and reset, but no snapshot/restore contract. Rumoca's adaptive
`SimulationSession` keeps its backend private; LunCoSim's fixed-step session
also keeps its state vector, parameters, step index, and time origin private.
The worker owns these steppers on its dedicated thread. A complete replay
baseline therefore needs either a genesis-only capture contract that rebuilds
each stepper before the first simulation tick, or solver-owner snapshot/restore
support for every enabled backend. A list of visible variable values is not
enough to promise same-state continuation from an arbitrary capture tick.

### D27 Modelica and generic port admission continuation (2026-09-28)

- Live API `SetPorts`, `ReleasePort`, and `ReleaseControl` use the shared
  pending-input queue. Their acknowledgements return stable target, producer,
  correlation, and next-tick admission identity. A release commits at its own
  sequence after earlier admitted writes; every admitted write remains in the
  queue and capture stream. Simulation-clock Rhai remains on its owning pass.
- `ReleaseControl` and `ReleasePort` clear the requested local setpoint holds
  at their ordered commit without writing replacement endpoint values.
  `ControlAuthorityChanged` queues `ReleaseControlInputs` for each released
  endpoint. Its ordered commit clears controller holds and simulated intents,
  while authored program setpoints and physics state remain active. Twin policy
  owns stop setpoints and expresses them as named `SetPorts` writes.
- The production gate `session_authority_release_preserves_ports.rhai` checks
  that claim/release records the lifecycle release and keeps endpoint values
  unchanged. Connection loss uses the same next-tick lifecycle release; the
  session stream captures it when recording is active.
- The current production acceptance is
  `scripts/api/test_session_input_admission.py`, including the Rhai release
  verifier and the authority-release value-preservation scenario. On the
  current integration, the API gate passed on owned port 4732 with `TESTS_OK
  13`, `TESTS_OK 10`, and `TESTS_OK 6`; the authority-release capture retained
  endpoint values and recorded the ordered lifecycle release. The production
  `tutorial_first_drive` scene gate passed in 770 ticks: it released authority
  during active Modelica guidance, observed continued movement through the
  waypoint sensor event, and verified the authored brake step afterward. The
  current physics scenario also checks positive finite mass/inertia across
  startup ticks so unintended endpoint writes surface through a production
  verdict.
- Unclassified direct `SetPorts`, `ReleasePort`, and `ReleaseControl` events
  without stable producer identity still use the immediate owner path and are
  not captured. This remains an open replay boundary.
- After integration, `cargo build -p lunco-luncosim --bin luncosim --features
  tracy -j 4` passed on local `main`. The merged GUI runtime published the
  settled 242-command schema; `docs/commands-reference.md` was regenerated from
  that response and committed as `8931cd0ab`.
- Live port-inspector `SetPorts` and `ReleasePort` events now use
  `PanelCtx::trigger_command` with `CommandOrigin::LocalUser` and the active
  `LocalSession`. The origin survives the workbench's deferred render queue;
  the cosim owner admits stable live scene targets through `PendingSessionInputs`.
  Editor-only targets without `GlobalEntityId` keep their immediate path. If the
  local session resource is absent, the two controls are disabled with a visible
  status message.
- `cargo test -p lunco-workbench-core
  deferred_panel_command_retains_and_restores_its_user_origin -j 4` passed
  (1 test); `cargo test -p lunco-core-session
  local_user_input_uses_the_local_session_as_its_producer_identity -j 4` passed
  (1 test). Focused `cargo check` passed for workbench core, core session,
  cosim, controller, the USD/cosim API adapter, and edit UI. Skill validation
  passed with 43 skills.
- After `382db427a` integration, the production API gate
  `scripts/api/test_session_input_admission.py` passed against that main binary
  on owned port 4734: `TESTS_OK 13` for Modelica and
  port admission, then `TESTS_OK 7` for port/control release. The observed
  admission ticks were 28 and 32, with release ticks 46 and 47. Process 459422
  exited through API `Exit`, and the port closed.
- The latest main delta `b68da02ba` reports a passing focused
  `cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-bevy-authored-runtime`.
- The authored `shader_asset_contracts` scene exercises its initial
  `LunCoProgramAPI` owner on the merged runtime. Its Rhai assertion now converts
  `ValidateAsset.uniform_size`'s `UInt` explicitly for the bounded 256-byte
  comparison. The production gate passed with `TESTS_OK 57` in 62 ticks using
  the task asset root and the `52a5922c3` binary. The fixture still logs its
  missing-`DirectionalLight` diagnostic. No post-change performance profile
  was obtained.
- The task checkout `target/` was cleaned again after reaching 4.3 GiB with
  1.9 GiB free; Cargo removed 4.6 GiB. The existing `main/target/` and shared
  Cargo caches were preserved for the post-integration production build.
- This increment does not add archive playback or a baseline manifest. Capture
  for unclassified direct port events, whole-session cross-domain replay, and
  performance evidence remain open. No production GUI session was launched for
  this local-panel path.

### Main fast-forward and deterministic continuation (2026-09-28)

- This checkout was fast-forwarded from `cb10b4472` to `85e5d433d`, the fetched
  `origin/main` head. In-progress deterministic changes were preserved in a
  local stash and reapplied; the stash remains until the merged work is fully
  reviewed and validated.
- Session-authority release clears controller-owned holds and simulated
  intents at the ordered tick while preserving authored program setpoints,
  endpoint values, and physics state. Authored policy writes an explicit
  `SetPorts` stop setpoint when required. The production physics Rhai gate
  checks that mass and inertia remain finite.

### D28 USD simulation preview admission continuation (2026-09-28)

- Local `main` advanced from `90fa387fe` to `456d2e736`; the task checkout was
  fast-forwarded to that head before this change. Commit `106031a97` excludes
  preview-only prims from the 32-row simulation-work prefix. The selector
  expands only as needed to find 32 non-preview candidates, caches ancestry
  checks across expansion, and marks preview rows processed before topology
  preparation.
- `cargo test -p lunco-usd-sim pending_sim_work_tests -j 4` passed all four
  owner tests. This covers preview-heavy selection, bounded ancestry checks,
  work lifecycle edges, and teardown. No production editor stress fixture with
  more than 32 preview prims was run; D28 records that acceptance gap.
- After review, commit `106031a97` was fast-forwarded into local `main`. The
  task and main worktrees now share that head and are eight commits ahead of
  `origin/main` at `54ad69a32`; no push was made. Only the simulation owner and
  open-contract row were committed. The handover update and `scripts/perf/`
  capture remain uncommitted.
- This checkout has a 4.2 GiB `target/` and 8.6 GiB free; no cleanup was needed.
  Cargo is idle. A separate Tutorials app (PID `1593496`, API port `4158`)
  remains active and untouched. No task-owned API session was launched.


## Current production acceptance update (2026-09-28)

- `codex/lunar-soil` fast-forwarded from `85e5d433d` to local `main` at
  `651487c8d`, integrating the bounded terrain and telemetry UI work. No push
  was made. The separate `main` worktree remains dirty in 30 files, with 12
  paths overlapping this task; preserve it and do not fast-forward its checked
  out branch until those edits are handled by their owner.
- This checkout alone was cleaned after measuring 8.8 GiB in `target/` and
  651 MiB free; Cargo removed 10.2 GiB. The current integrated production build
  passed: `cargo build --locked -j 4 -p lunco-luncosim --bin luncosim`.
- `scripts/compare_deterministic_startup_dependencies.py` passed four
  production runs at Compute widths 1 and 24. Every sensor scenario's
  `on_start`/first `on_tick` boundary was 0/1 and the first-tick actor, scene,
  IMU, altimeter, and contact snapshot matched with digest
  `92753352336a37dd97ed30837833bba97c64db68ef0649b8f1e29727583999da`.
- The production `port_owner_collision` Rhai scene passed 11 assertions in two
  ticks and emitted one `PORT_OWNER_COLLISION` error naming both input owners
  and registry precedence. The rule is Rhai policy over typed runtime registry
  facts.
- `scripts/api/test_session_input_admission.py` passed on the integrated
  production binary on owned port 46372: `TESTS_OK 13` for Modelica and
  SetPorts admission, `TESTS_OK 10` for ordered ReleaseControl, and `TESTS_OK 6`
  for authority release. Process 996863 exited through API `Exit`; the port
  closed. ReleaseControl clears endpoint-local input holds and leaves values
  unchanged; authored policy writes explicit stop setpoints through SetPorts.
- `cargo test --locked -j 4 -p lunco-time` passed all 53 tests, including
  startup hold, late progress admission, retained fixed-time, and scene epoch
  installation without resetting the global fixed cycle. `python3
  scripts/validate_skills.py` passed all 43 skills; Python syntax compilation
  and `git diff --check` passed. The whole-session baseline, durable replay,
  controlled opposite async completion order, cross-machine floating-point
  behavior, and production performance acceptance remain open.
- A follow-up app relink exceeded the remaining disk space after the test-profile
  outputs had been built. The linker reported disk full. This checkout's target
  was cleaned again, removing 9.7 GiB; the shared Cargo cache and sibling targets
  were preserved. There is currently no `target/debug/luncosim`; rebuild in this
  checkout before another production run.

## Runtime and repository constraints

- Do not edit authored USD through shell or patch tools. Use the live USD
  document operations for authored changes.
- Runtime checks must use this checkout's production binary on an explicit free
  port. Verify PID, executable, working directory, and listener before control;
  stop only the owned app through API `Exit`.
- Preserve unrelated edits in the optimization worktree and existing stashes.
  Do not reset or push. The task checkout's `target/` was cleaned after checking
  its 13 GiB size and 2.1 GiB free space; Cargo removed 14.1 GiB of local build
  outputs. A later task-local cleanup removed 4.6 GiB after that target grew to
  4.3 GiB. Source, shared Cargo caches, and sibling targets were preserved.
- Report source, scene-test, live API, and visual evidence as distinct claims.
- Performance profiling remains open. On 2026-09-28, independent Apollo
  sessions (PIDs 464419 and 466277, API ports 43453 and 4158) were active in
  the integrated `main` checkout; both were left untouched. Tracy capture PID
  464318 on port 8086 was active during an accidental `--version` probe that
  briefly launched another Tracy-enabled app. Treat that capture as potentially
  contaminated. The probe process was stopped by its exact PID. Both Apollo
  sessions had exited by the final check; a separate terrain session (PID
  297685, API port 47127) remained active at about 187% CPU and was left
  untouched. No clean FPS window or new Tracy analysis was obtained; recheck
  ownership and build state before the next profiling run.

## Current continuation after main fast-forward (2026-09-28)

- `codex/lunar-soil` was fast-forwarded from `8cc677173` to local `main` at
  `63af9676c`. Local `main` is ten commits ahead of `origin/main` at
  `2581dd81b`; no push was made. The existing handover edit and untracked
  `scripts/perf/` capture were preserved.
- The integrated USD simulation admission owner passed
  `cargo test -p lunco-usd-sim pending_sim_work_tests -j 4` (10 passed). The
  same compiled test binary's `dynamic_activation_tests` filter passed all five
  tests. `git diff --check` passed after the integration review.
- `cargo clean` in this checkout removed 2.5 GiB of its `target/` outputs after
  the focused tests. The final check showed 8.8 GiB free and no `target/` here;
  sibling worktree targets and shared Cargo caches were left intact. No
  production binary build or task-owned API session was run after the merge.
- The general workflow has deterministic fixed-tick admission and ordering for
  admitted scene state and captured input, with same-build cross-Compute
  production stress evidence documented under D4. It is not yet a complete
  replay workflow: the session archive lacks a composite owner baseline and a
  playback consumer, and several authoritative commands remain uncaptured.
- Baseline continuation remains the next replay gap. The active Modelica worker
  exposes values/time/reset but not a portable solver checkpoint; both Rumoca's
  adaptive stepper state and LunCoSim's fixed-step state vector/index are
  private. Do not represent visible variables or solver settings as a complete
  restorable state. Decide and implement either an explicit genesis-only
  capture boundary or solver-owner snapshot/restore before claiming replay.
- Current source still uses `@peer host|client|both` for process-role routing;
  `2ec8aaa3f` is the later contract after the earlier `@run-on` rename. Keep
  process role distinct from Rust runtime scope, cycle, and clock.

## Domain-network startup admission continuation (2026-09-29)

- Commit `d2f7de95c` adds a root-scoped `UsdDomainProjection` progress hold for
  initial USD Modelica network discovery. The hold covers member-source
  resolution, synthesis, and generated `SimComponent` publication; the binding
  epoch stays open while its port surface is unknown. At the
  `SimulationProgressAdmissionSet` boundary the domain hold releases only once
  the component is present, while the existing Modelica compile hold overlaps
  it. This fixes the 4-rover startup case that previously emitted unresolved
  connection-binding warnings before its generated interfaces existed.
- `cargo check --locked -j 4 -p lunco-usd-sim-domain -p lunco-usd-sim-cosim`
  passed. `cargo build --locked -j 4 -p lunco-luncosim --bin luncosim
  --no-default-features --features api-transport` passed; it reported the
  pre-existing unused `ApiResponse` import in `lunco-api-transport`.
- `python3 scripts/compare_deterministic_startup_dependencies.py` passed twice
  after the change: four production runs, first behavior tick 1, matching
  actor and sensor/physics snapshot across Compute widths 1 and 24, digest
  `92753352336a37dd97ed30837833bba97c64db68ef0649b8f1e29727583999da`.
- The task commit was fast-forwarded to local `main` at `d2f7de95c`; nothing was
  pushed. Main's 20 non-overlapping staged user edits are restored. Ten
  overlapping edits targeted the superseded `ControlSafeStop` value-writing
  contract; they remain recoverable in `stash@{0}` (`2da31eca2c86f3a6861245516888e072cb590c63`)
  rather than reintroducing that API. The separate untracked `scripts/perf/`
  work remains untouched.
- The task checkout's `cargo clean` removed 7.3 GiB; rebuild its production
  binary before another runtime check. The next broad replay gap remains a
  truthful genesis-only capture boundary or owner-supported solver snapshots;
  the current Modelica worker does not expose portable solver checkpoints.

## Incomplete USD composition policy and async ordering (2026-09-29)

- `codex/lunar-soil` was fast-forwarded from `2540d231b` to local `main` at
  `c7d991eac` (three commits); no push was made. Changes in those commits did
  not overlap the USD policy or composition-order work. Preserve the separate
  untracked `scripts/perf/` directory.
- The scene lifecycle now owns required deterministic
  `usd.scene_composition(facts: Map) -> Map`. It runs only for unresolved
  fetched arcs after structural projection and before releasing the scene
  simulation hold. The shipped application Rhai policy returns
  `allow_partial`, matching the existing warning plus partial-stage default;
  a Twin policy may return `reject_scene`. Rejection, missing policy, policy
  fault, or malformed result tears down the partial primary stage and fails the
  scene transition. Architecture and `author-hook-policy` guidance describe
  the contract.
- `cargo test -j 4 -p lunco-usd-bevy-stage
  reverse_async_completion_keeps_authored_recipe_order_and_identity` passed.
  It forced closure reads to finish in reverse order, verified authored-order
  application, and compared equal `StageContentClosure` identities.
  `cargo check -j 4 -p lunco-usd-bevy-runtime-core --lib` and
  `cargo check -j 4 -p lunco-usd-bevy-runtime-core --tests` passed; the latter
  reported the existing unused `mut` in `twin_projection.rs:4277`.
- `python3 scripts/validate_skills.py`, the application policy TOML/wiring
  check, and `git diff --check` passed. No `LUNCOSIM_BIN` or checkout
  production binary was available. A runtime-core test build was stopped as
  this checkout's new `target/` reached 1.8 GiB with only 2.9 GiB remaining;
  `cargo clean` restored disk space before the successful focused checks. The
  authored Rhai hook-policy test was not run through the production test
  binary. A final `cargo clean` after the checks removed another 1.9 GiB and
  left 4.0 GiB free.
- Still open: production `LoadScene` tests exercising both Rhai allow and Twin
  reject policies against a missing USD arc, and full AssetServer composition
  closure acceptance. The async test covers the production ordered join helper
  and recipe identity, not a live AssetServer source transaction. The broader
  replay, cross-machine numeric, browser-worker, and performance gaps remain
  open as recorded above.

## Initial AssetServer composition policy acceptance (2026-09-30)

- `assets/scenes/tests/dynamic_reference_composition/initial_allow.usda`
  loads the shared `initial_usd_composition.rhai` scenario against an initial
  stage containing an available referenced prim whose nested arc is missing.
  The Twin `scene_composition` policy returns `allow_partial` for this fixture.
- The production scene gate passed four assertions at `SimTick=1`: `/World`
  and `/World/Partial` were projected, `/World/Partial/Missing` was not
  fabricated, and runtime diagnostics retained
  `USD_COMPOSITION_MISSING_DEPENDENCY` with the `missing.usda` subject.
  Output: `TESTS_OK 4`, `INITIAL USD COMPOSITION: PASS`, and
  `luncosim test PASS ... ticks=1 updates=4 sim=0.02s`.
- Reproduce with the production binary:

  ```sh
  LUNCO_ASSET_ROOT="$PWD/assets" RUST_LOG=info \
    target/debug/luncosim test \
    --scene assets/scenes/tests/dynamic_reference_composition/initial_allow.usda \
    --max-ticks 900
  ```

- The isolated `assets/scenes/tests/initial_usd_composition_default` Twin has
  no local composition-policy source. Its `initial_default.usda` references
  the co-located `partial.usda`, which in turn references a missing layer in
  the same Twin. The root and partial stage projected, while diagnostics named
  `missing.usda`; the four-assertion Rhai gate passed through the shipped
  application policy's default `allow_partial` decision. Readback through
  `InspectUsdDocument` confirmed both authored arcs and the saved documents.
  The Twin override and application default paths are now both covered.
- Reproduce the application-default case with the production binary:

  ```sh
  LUNCO_ASSET_ROOT="$PWD/assets" RUST_LOG=info \
    target/debug/luncosim test \
    --scene assets/scenes/tests/initial_usd_composition_default/initial_default.usda \
    --max-ticks 900
  ```

- `expect_scene_load_failure(path, detail_contains)` now lets an authored
  scenario report its normal verdict, issue one expected `load_scene`, and
  leave the runner to verify its typed terminal event after the scenario actor
  is removed. The runner checks that the load, transition, mount root,
  simulation-progress hold, and projected prims are gone. On root-stage
  composition rejection or AssetServer load failure, the scene owner now
  invalidates `SceneMountState` before clearing projected entities. The first
  production rejection run found a real stale-mount-root defect; the gate
  exposed it before the owner fix.
- After fast-forwarding to `432655ce825a1e2663e57273989f77c6e97e0fd4` and
  rebuilding with `cargo build --bin luncosim -j 4`, all four production gates
  passed with `RUST_LOG=off`: the dynamic root rejection and existing dynamic
  reference rejection, plus application-default and Twin-override initial
  composition cases followed by a missing-stage load. The expected errors
  were observed, and failed scene state was fully cleared. A process launched
  with a `--scene` whose root is rejected before any scenario can declare an
  expected outcome remains unverified. Broader replay, cross-machine numeric,
  browser-worker, and performance gaps remain open.

## Rhai deterministic-reference comparison (2026-09-29)

- `luncosim test --determinism-reference PATH` loads the reference JSON into
  typed scene-runner records and exposes selected metadata/rows through the
  existing Rhai query bridge. Rhai owns profile selection, exact state
  comparison, and the authored verdict. Modelica variables are compared by
  sorted field in the scenario; the scene also verifies that an altered physics
  row is rejected. No full state trace or result bundle is built.
- Reproduce from the repository root with the default-feature production
  binary and checked-in reference. Linux/macOS:

  ```sh
  cargo build --bin luncosim -j 4
  scripts/test-deterministic-physics-profiles.sh
  ```

  Windows PowerShell:

  ```powershell
  cargo build --bin luncosim -j 4
  .\scripts\test-deterministic-physics-profiles.ps1
  ```

- The regular `cargo build --bin luncosim -j 4` succeeded on top of local
  `main` tip `de52c3608` after clearing this checkout's generated `target/`
  outputs to recover disk space. The production
  `scripts/test-deterministic-physics-profiles.sh` matrix then passed all ten
  profiles on that integrated source and printed
  `DETERMINISTIC_PHYSICS_PROFILES_OK`: serial and default Compute widths for
  the 4/8/20-rover fixtures, plus four seeded
  jitter profiles. The matrix leaves the fixed step at 60 Hz and uses the
  unpaced manual clock. It relies on scene-test exit status and does not parse
  logs. The authored gate includes 24 assertions per 4-rover run, including a
  negative control that verifies a deliberately altered state row fails.
- The PowerShell wrapper was reviewed but not executed here because neither
  `pwsh` nor `powershell` is installed. The Windows command is available for
  the other-machine run.
- This is same-host fixture comparison evidence. Cross-machine execution,
  whole-session replay, browser workers, and broader scene coverage remain
  open. The unrelated untracked `scripts/perf/` work remains untouched.

## D4 typed verdict details and refreshed production matrix (2026-09-29)

- `multi_rover_stress.rhai` reports the first differing state field, expected
  and actual values for physics/articulated rows, and Modelica variable/value
  pairs. Its Rhai negative control starts with a valid reference row, alters
  one field, and verifies the exact failure message.
- `report_verdict` sends status, check count, and failed assertions as a typed
  telemetry map. `luncosim test` prints those authored details alongside its
  process verdict, so failure output does not depend on logging environment
  variables. The profile launchers do not modify `RUST_LOG` or parse logs.
- The default-feature `cargo build --bin luncosim -j 4` passed. The
  `scene-4-serial` production profile passed at 780 ticks against the checked-in
  reference. A controlled run with an intentionally wrong seed exited 1 and,
  with `RUST_LOG=off`, printed its Rhai-authored failure messages through
  `luncosim test detail:`. This verifies both the structured pass and failure
  paths without log output.
- The complete shell profile matrix previously reached
  `DETERMINISTIC_PHYSICS_PROFILES_OK`: 4/8/20-rover serial/default-Compute
  profiles plus four jitter/seed profiles, each with 780 ticks and exact
  reference/final-stage comparisons. This is same-host determinism acceptance,
  not sustained performance evidence.
- The supplied other-computer `scene-4-serial` output is timestamped
  2026-09-29 17:51. It reached 780 ticks and returned a terminal scene failure,
  but gave no authored assertion detail. The supplied report identifies clean
  source `1df5d85e7a242ac4ee3544c38a141a91ee407ee3`; its committed reference
  fixture hashes to
  `3297aa8682cf9f503ea7085a7b07bdccf31ebdd392173bc9ef0e06b9562eb87f`, though
  the run itself did not print the hash. This predates `bb00f9bb4` (22:44),
  which added typed authored failure details, and `f095634e8` (2026-09-30),
  which removed rendered `Transform` values from the physics comparison. The
  old row included cell-local `Transform.translation`, so a render-projection
  mismatch is plausible; the missing assertion detail means it cannot be
  confirmed as the failure cause. The old result does not establish a
  physics-state mismatch. After removing only `|localTf=...` from profile rows,
  the canonicalized `.profiles` objects in the old and current references have
  the same SHA-256:
  `62604e5ed1274c2852f0b0005a657e27ded3821bba2ef69025a084d198c117fa`. The
  physics, Modelica, articulated, and final-stage reference values were
  unchanged by that comparison-field removal. Cross-machine comparison
  remains open; rerun the single profile from the current branch and capture
  its first expected/actual field, exact `HEAD`, and reference SHA-256.
- This work is for local `main` integration only; no push is authorized. The
  unrelated untracked `scripts/perf/` work remains preserved.

## Dynamic reference composition admission (2026-09-30)

- `drain_ref_spawns` now sends any nonempty prepared-reference dependency
  diagnostics through the same required `usd.scene_composition` evaluator as
  the initial scene path, before consuming the instance plan, installing layer
  bytes, or projecting entities. Rejection or hook failure uses the existing
  terminal reference-admission path; primary-scene progress stays held and
  later authored references remain blocked. The diagnostic includes the first
  unresolved dependency. Preview and additive failures remain operation-local.
- The focused owner test
  `incomplete_dynamic_reference_policy_rejection_holds_primary_and_blocks_successors`
  is present. It checks the mounted-scene facts and lifecycle context, the
  authored order of two missing dependencies, the first-dependency diagnostic,
  primary hold/fault, no instance plan, and ready-successor blocking. The
  focused test passed (`1 passed`, 50 filtered).
- The production `dynamic_reference_composition` Rhai gate passed both
  `allow.usda` and `reject.usda`. The allow case projected the valid root of a
  stage with a missing nested dependency and did not fabricate the unresolved
  child. The reject case observed the expected terminal
  `usd-reference-admission` fault for `/World/Partial`.
- The first allow run failed before admission because the fixture treated an
  absent `QueryUsdPrim.ok` field as false. It now accepts the direct prim-record
  response and checks the required generation; the allow and reject production
  runs then passed.
- `python3 scripts/validate_skills.py` passed all 43 skills, and
  `git diff --check` passed before the current documentation update.

## Current cross-machine failure investigation (2026-09-30)

- After fast-forwarding to `ab13d36cf`, the regular default-feature
  `cargo build --bin luncosim -j 4` passed. The rebuilt production binary
  passed `scene-4-serial` at 780 ticks with the exact reference, serial
  Compute, zero jitter, seed `6840157149251759617`, and 941 application
  updates. The reference SHA-256 was
  `4f1785e86deb5e05083561007ec8e08891ca5a0d19c82e9edcdb8f70c92e3496`.
  This is current same-host evidence after main integration, not a remote
  reproduction.
- The locally available production binary (built from `e901abe7`; the current
  source `9f9a301c` adds only telemetry-browser changes after it) passed the
  four-rover production gate at 780 ticks. It used the checked-in reference
  SHA-256 `4f1785e86deb5e05083561007ec8e08891ca5a0d19c82e9edcdb8f70c92e3496`.
  The shell profile matrix had already passed on this source line; this was a
  focused rerun prompted by the supplied remote failure.
- `world_pos` uses `SimulationPoseQuery`, which reads Avian's seeded f64
  `Position`/`Rotation` for rigid bodies. The current Rhai comparison excludes
  render `Transform` and also checks that each rover moves more than 0.25 m.
  The older scene gate serialized cell-local `Transform.translation` into the
  exact physics row and used it in the startup pose check. That f32 render
  projection could vary with interpolation/update timing and was not
  authoritative physics evidence.
- Rebuilt the default-feature production binary with the regular command
  `cargo build --bin luncosim -j 4` at
  `f095634e825fa487b1893d2239a094fbe420963d`. The focused production
  `scene-4-serial` run passed all 24 authored assertions and matched the
  reference at 780 ticks.
- Ran `RUST_LOG=off scripts/test-deterministic-physics-profiles.sh`. All ten
  4/8/20-rover serial/default-Compute and seeded-jitter profiles passed at 780
  ticks, including exact final-stage comparison. The run printed
  `DETERMINISTIC_PHYSICS_PROFILES_OK`. The checked-in reference SHA-256 is
  `4f1785e86deb5e05083561007ec8e08891ca5a0d19c82e9edcdb8f70c92e3496`.
- Both shell and PowerShell launchers now print the source revision and
  reference SHA-256 before the profile matrix. `bash -n
  scripts/test-deterministic-physics-profiles.sh`, the focused production run,
  the full matrix, and `git diff --check` passed. Neither `pwsh` nor
  `powershell` is installed here, so the PowerShell wrapper was not executed.
- After fast-forwarding to latest local `main` at
  `432655ce825a1e2663e57273989f77c6e97e0fd4`, the regular production build and
  `scene-4-serial` rerun passed. It completed 780 ticks and 1,062 application
  updates at 60 Hz, serial Compute, zero jitter, and seed
  `6840157149251759617`, matching reference SHA-256
  `4f1785e86deb5e05083561007ec8e08891ca5a0d19c82e9edcdb8f70c92e3496` with
  `numeric_tolerance=0`. Scene readiness held for 9,481 updates and physics
  admission settled in three zero-duration updates. The worktree also carried
  scene-load failure acceptance edits; the multi-rover fixture and reference
  were unchanged. This is same-host evidence on the latest integrated physics
  admission path, not a remote reproduction.
- The earlier remote `scene-4-serial` failure still has no assertion detail.
  Its reported clean source was `1df5d85e7a242ac4ee3544c38a141a91ee407ee3`; the
  fixture committed there hashes to
  `3297aa8682cf9f503ea7085a7b07bdccf31ebdd392173bc9ef0e06b9562eb87f`, but the
  run did not record the file hash. Its output predates typed Rhai failure
  details and the removal of render-interpolated `Transform` from physics
  comparison, so it cannot establish whether either caused the failure. Do not
  change tolerance or infer a physics mismatch from that output.
  Cross-machine acceptance stays
  open until the other computer reruns the profile from the same committed
  source revision and reference and reports the source revision, reference
  SHA-256, full output, and any `luncosim test detail:` field difference. On
  Windows PowerShell, run from the repository root:

  ```powershell
  git rev-parse HEAD
  Get-FileHash .\scripts\tests\fixtures\deterministic-physics-reference.json -Algorithm SHA256
  cargo build --bin luncosim -j 4
  $env:LUNCO_ASSET_ROOT = Join-Path (Get-Location) 'assets'
  .\target\debug\luncosim.exe test --scene assets/scenes/tests/multi_rover_stress_4.usda --threads 1 --jitter 0.0 --seed 6840157149251759617 --determinism-reference scripts/tests/fixtures/deterministic-physics-reference.json
  ```

  Send the complete output and the two identity values; the Rhai gate prints
  either the first failed scene assertion or the first expected/actual state
  field directly, without Python or log parsing.
