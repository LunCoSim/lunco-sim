# Feature Specification: Agent-Driven Simulation Loop

**Feature Branch**: `033-agent-driven-simulation`
**Created**: 2026-04-25
**Status**: Draft
**Input**: Expose model introspection and runtime mutation primitives so an AI agent can run a full "load → tune → simulate → observe" loop through the API without UI.

## Agent-Workflow Anchor

> *"I want to simulate the Annotated Rocket Engine. Load it, set the valve to 50%, then to 100%, pause, then resume, and tell me the chamber pressure."*

This is the validation scenario. Spec 032 (`model-source-listing`) lets the agent **find and open** AnnotatedRocketStage. This spec lets it **inspect, mutate, observe**. The two specs together produce the closed agent loop:

```
list_bundled / find_model    │ "what's available?"      │ spec 032
list_open_documents          │ "what's loaded?"          │ spec 032
open(uri)                    │ "load it"                 │ spec 032
describe_model(doc_id)       │ "what knobs does it have?"│ spec 033 ← this
set_input(doc_id, …)         │ "turn this knob"          │ spec 033 ← this
CompileActiveModel /         │ "start it"                │ already exists
ResumeActiveModel
PauseActiveModel             │ "pause it"                │ already exists
snapshot_variables(doc_id)   │ "what's the value now?"   │ spec 033 ← this
SubscribeTelemetry           │ "stream values to me"     │ already exists
```

## Problem Statement

Today an AI agent can compile, pause, resume, and reset a Modelica model, and subscribe to a telemetry stream — but it has no way to ask the model "what inputs do you accept?", no way to push a runtime value into one of those inputs, and no way to take a one-shot snapshot of current variable values without committing to a streaming subscription. The information exists internally — `ast_extract::collect_inputs_with_defaults_from_classes` walks every input declaration, the simulation worker accepts input updates between steps, the stepper carries the current state vector — but none of it is reachable through the API. The agent therefore cannot run the loop the workbench is built for: tweak a knob, observe the effect, decide the next action.

There is also no fuzzy-search across model sources — the agent has to call `list_bundled`, `list_twin`, and `list_msl` and grep client-side to translate "Annotated Rocket Engine" (a human description) into `bundled://AnnotatedRocketStage.mo` (an opener). For a single discovery step at the start of every workflow this is wasteful and error-prone.

## User Scenarios & Testing

### User Story 1 - Fuzzy Find Model Across All Sources (Priority: P1)

As an AI agent
I want to type a human-readable name fragment and get back ranked URIs
So that I can resolve "Annotated Rocket Engine" to `bundled://AnnotatedRocketStage.mo` in one round trip without re-implementing search client-side.

**Why this priority**: Eliminates the first-mile friction of every workflow. Without it the agent burns 3 calls + ad-hoc grep before it can `open()`.

**Independent Test**: Issue `find_model("rocket")`, receive a ranked list including `bundled://AnnotatedRocketStage.mo`, `bundled://RocketEngine.mo`, and any twin file with "rocket" in its path.

**Acceptance Scenarios**:

1. **Given** the bundled examples include `RocketEngine.mo` and `AnnotatedRocketStage.mo`, **When** I call `find_model("rocket")`, **Then** both appear in the response with their `bundled://` URIs and a relevance score.
2. **Given** a Twin is open containing `models/rover.mo`, **When** I call `find_model("rover")`, **Then** the file appears with its absolute path as URI and a `source: "twin"` tag.
3. **Given** I call `find_model("PID")`, **When** the MSL library is loaded, **Then** results include `Modelica.Blocks.Continuous.PID` and any examples named `*.PID*` from MSL.
4. **Given** an empty query, **When** I call `find_model("")`, **Then** I receive an `ApiResponse::Error` (do not return the entire universe).

---

### User Story 1.5 - Compile a Specific Class From a Multi-Class Document (Priority: P1)

As an AI agent
I want to fire one call that compiles a chosen class from a document with several non-package classes
So that `AnnotatedRocketStage.mo`'s `RocketStage` (or `Engine`, `Tank`, etc.) compiles without ever needing the GUI's class-picker modal.

**Why this priority**: Without this primitive, every multi-class document is a hard wall for the agent. `CompileActiveModel` silently aborts and waits for a modal click that an API caller cannot produce. Discovered while validating the spec 032 workflow against AnnotatedRocketStage.

**Independent Test**: Open a multi-class doc via `open_uri`, call `list_compile_candidates(doc_id)` to enumerate the choices, then `compile_model(doc_id, class="RocketStage")`. Verify `compile_status(doc_id)` reaches `state: "ok"` without any UI interaction.

**Acceptance Scenarios**:

1. **Given** AnnotatedRocketStage is open, **When** I call `list_compile_candidates(doc_id)`, **Then** I receive `[{qualified: "RocketStage", kind: "model"}, {qualified: "Tank", kind: "model"}, ...]` — every non-package class the document defines.
2. **Given** the document has 6 non-package classes, **When** I call `compile_model(doc_id, class: "RocketStage")`, **Then** the compile proceeds with `RocketStage` as the target — the GUI picker modal does NOT open and `compile_status` transitions to `"compiling"` then `"ok"`.
3. **Given** I call `compile_model(doc_id)` with no `class` field on a multi-class doc, **When** the existing `drilled_in_class` is `None`, **Then** `compile_status` reports `state: "needs_class_choice"` with the candidate list — the API caller can recover without a modal.
4. **Given** I pass a `class` name that is not a non-package class in the document, **When** I call `compile_model`, **Then** `compile_status` transitions to `"error"` with a clear message naming the bad class and listing the valid choices.
5. **Given** I have already called `set_active_class(doc_id, "Engine")`, **When** I subsequently call `compile_model(doc_id)` with no `class`, **Then** the previously-set class is used — sticky across calls within the session.

---

### User Story 1.6 - Read the Current Source of an Open Document (Priority: P1)

As an AI agent
I want to fetch the in-memory source text of any open document — including Untitled docs that have no filesystem path
So that I can reason about what was loaded, see uncommitted edits, or feed the source into a downstream tool (lint, format, diff, search) without re-reading the file from disk.

**Why this priority**: The agent's mental model of the workspace is incomplete without source visibility. The existing `GetFile(path)` command requires a filesystem path (Untitled docs have none) and logs to the console (the agent never receives the bytes back). Both gaps land on the same fix: a query provider that returns the live source.

**Independent Test**: Open `bundled://AnnotatedRocketStage.mo` (which lands as Untitled — no fs path), call `get_document_source(doc_id)`, receive the complete `.mo` source as a string in the response payload.

**Acceptance Scenarios**:

1. **Given** I have opened a bundled example as Untitled, **When** I call `get_document_source(doc_id)`, **Then** I receive `{source: "...", kind: "modelica", generation: N, dirty: true, origin: {kind: "untitled", name: "..."}}` with the full embedded source.
2. **Given** I have opened a file from disk and edited it without saving, **When** I call `get_document_source`, **Then** I receive the in-memory edited source — not the on-disk version — and `dirty: true`.
3. **Given** an unknown `doc_id`, **When** I call `get_document_source`, **Then** I receive `EntityNotFound`.
4. **Given** a document of a non-Modelica kind (USD, SysML, future Markdown), **When** I call `get_document_source`, **Then** I still receive the source text and the correct `kind` label — the provider is type-agnostic at the cross-domain level.

---

### User Story 2 - Describe a Model's Inputs and Parameters (Priority: P1)

As an AI agent
I want to ask "what tunable knobs does this open document have?" with one call
So that I know `set_input(doc, "valve", …)` is the right next move and the value range is `0..1`.

**Why this priority**: Without this, the agent has to either parse the source itself or rely on the user to specify input names — defeating the purpose of agent automation. The data is already in the AST extractor.

**Independent Test**: Open AnnotatedRocketStage, call `describe_model(doc_id)`, receive a JSON object listing every input + parameter with name, type, default, bounds, and description.

**Acceptance Scenarios**:

1. **Given** AnnotatedRocketStage is open, **When** I call `describe_model(doc_id)`, **Then** I receive `{inputs: [...], parameters: [...], outputs: [...]}` where `inputs` includes the `valve` field with its type, default, min/max bounds, and description string.
2. **Given** a model has no inputs, **When** I call `describe_model`, **Then** I receive `inputs: []` (not an error).
3. **Given** the document has not yet been compiled, **When** I call `describe_model`, **Then** I still receive the introspection data — it comes from the AST, not the running simulation.
4. **Given** I pass an unknown `doc_id`, **When** I call `describe_model`, **Then** I receive `EntityNotFound`.

---

### User Story 3 - Set a Runtime Input Value (Priority: P1)

As an AI agent
I want to push a value into a running model's input slot without recompiling
So that "set valve to 50%, then to 100%" works as two cheap calls, not two compile cycles.

**Why this priority**: This is the core mutation primitive. Without it the workflow stalls at step 3.

**Independent Test**: With AnnotatedRocketStage compiled and running, call `set_input(doc_id, "valve", 0.5)`. Verify (via telemetry or `snapshot_variables`) the running stepper reflects the new value within the next sim step.

**Acceptance Scenarios**:

1. **Given** the model is compiled and running, **When** I call `set_input(doc_id, "valve", 0.5)`, **Then** the worker thread applies the value before the next step and the corresponding telemetry value reflects it.
2. **Given** the model is paused, **When** I call `set_input` and then `ResumeActiveModel`, **Then** the model resumes with the new value already in effect.
3. **Given** I pass an input name that does not exist, **When** I call `set_input`, **Then** I receive `ApiResponse::Error` with the missing name and a hint to call `describe_model` first.
4. **Given** the value violates declared bounds, **When** I call `set_input`, **Then** I receive an error naming the bound — clamping is the caller's job, not the API's.
5. **Given** I call `set_input` rapidly (e.g. 10 calls in 100 ms), **When** the worker is mid-step, **Then** values are squashed (last-writer-wins per name) — matching the existing internal `UpdateParameters` squashing behaviour.

---

### User Story 4 - Snapshot Current Variable Values (Priority: P2)

As an AI agent
I want a one-shot read of current variable values without subscribing to a stream
So that I can answer "what's the chamber pressure right now?" with one round trip.

**Why this priority**: Streaming telemetry is overkill for "answer this question once." A snapshot is the right primitive for poll-style agents.

**Independent Test**: With a model running, call `snapshot_variables(doc_id, ["chamberPressure", "thrust"])`, receive `{chamberPressure: 4.2e6, thrust: 1.8e5, t: 2.45}` immediately.

**Acceptance Scenarios**:

1. **Given** a model is running, **When** I call `snapshot_variables(doc_id, ["chamberPressure"])`, **Then** I receive the current value plus the simulation time `t` as of the last completed step.
2. **Given** I omit the `names` filter, **When** I call `snapshot_variables(doc_id)`, **Then** I receive every published variable (states + outputs).
3. **Given** the model has not been compiled, **When** I call `snapshot_variables`, **Then** I receive an empty object with a warning, not an error — the doc exists, it just has no live state.
4. **Given** I request a variable name that does not exist, **When** I call `snapshot_variables`, **Then** that name is omitted from the response (other names still return) — same forgiving behaviour as `SubscribeTelemetry`.

---

### User Story 5 - Compose a Full Workflow Without UI (Priority: P1)

As an AI agent driving the workbench headlessly
I want every step of the AnnotatedRocketStage workflow available through one consistent API surface
So that I can write the workflow as a script and run it under CI without the UI being present.

**Why this priority**: Validates the whole spec end-to-end. If steps 1–4 work but cannot be composed in this exact sequence, we have not delivered the value.

**Independent Test**: A single test script (CI-runnable) executes:

```
1. find_model("rocket")              → resolve URI
2. open(uri)                         → load
3. CompileActiveModel(doc)           → compile (existing)
4. describe_model(doc)               → list inputs
5. set_input(doc, "valve", 0.5)      → tweak
6. snapshot_variables(doc, ["thrust"]) → observe
7. set_input(doc, "valve", 1.0)      → tweak again
8. PauseActiveModel(doc)             → pause (existing)
9. snapshot_variables(doc, [])       → final read
10. ResumeActiveModel(doc)           → resume (existing)
```

…and produces a deterministic transcript without ever touching egui.

**Acceptance Scenarios**:

1. **Given** a fresh workbench, **When** I run the script above, **Then** every call returns success and the final transcript matches the expected reference (within a tolerance for the simulation values).
2. **Given** the workbench was started with `--api 3000 --headless`, **When** I run the script, **Then** behaviour is identical to the windowed run — no API call requires a render frame.

---

## Requirements

### Functional Requirements

- **FR-001**: `find_model(query: String)` MUST search across bundled, twin, MSL, and currently-open documents in a single call, returning ranked hits with a stable URI per hit and a numeric relevance score. The API MUST reject empty queries.
- **FR-002**: `describe_model(doc_id)` MUST return `{inputs, parameters, outputs}`, each entry carrying at minimum `name`, `type`, `default` (when present), `bounds` (when declared in the source), and `description` (when annotated). Data MUST come from the AST, not the running stepper, so it is available before compile.
- **FR-003**: `set_input(doc_id, name, value)` MUST forward to the existing simulation worker's input-update path, with last-writer-wins squashing per name. It MUST return `EntityNotFound` for unknown `doc_id`, and a named error for unknown `name` or out-of-bounds `value`.
- **FR-004**: `snapshot_variables(doc_id, names?)` MUST return current values for the requested names (or all published variables when `names` is omitted), plus the simulation time `t` of the last completed step. Unknown names MUST be silently omitted.
- **FR-005**: All four endpoints MUST be reachable via the existing `POST /api/commands` HTTP endpoint and as typed MCP tools in `mcp/src/index.js`, registered through the `ApiQueryProvider` mechanism introduced by spec 032 P1.
- **FR-006**: The workflow described in User Story 5 MUST be executable in headless mode (`--headless`) without any windowed render path.

### Key Entities

- **`FindModelProvider` (new)**: An `ApiQueryProvider` that aggregates the four sources, applies fuzzy-match scoring, and emits a unified ranked list. Implementation should live in `lunco-modelica` initially (reuses the bundled + MSL indexes) but extension points allow other domains (USD, SysML) to contribute search results in the future.
- **`DescribeModelProvider` (new)**: An `ApiQueryProvider` that takes a `doc_id`, looks up the `ModelicaDocument`, runs the AST extractors that already exist (`collect_inputs_with_defaults_from_classes`, `collect_parameter_bounds_from_classes`, `collect_descriptions_from_classes`), and projects the result to JSON.
- **`SetInputCommand` (new Reflect Event)**: A fire-and-forget command in the existing typed-command style. Forwards to the simulation worker's `UpdateInputs` channel, reusing the squashing logic the worker already implements for live parameter updates.
- **`SnapshotVariablesProvider` (new `ApiQueryProvider`)**: Reads the current state vector held by the worker thread for `doc_id`, projects the requested names to JSON. Returns an empty payload (with `t: null`) if the doc has no compiled stepper yet.

---

## Success Criteria

- **SC-001**: The User Story 5 transcript runs to completion in `<5 s` against a warmed-up workbench (build + first compile excluded).
- **SC-002**: `find_model` returns in `<200 ms` even with the full MSL index (~2500 classes) loaded — fuzzy match must scale.
- **SC-003**: `describe_model` returns valid data for every bundled model (`AnnotatedRocketStage`, `RC_Circuit`, `BouncyBall`, …) without compile.
- **SC-004**: `set_input` followed by `snapshot_variables` reflects the new value within `≤2 sim steps`.
- **SC-005**: The workflow is reproducible without UI — no test step depends on a render frame.

---

## Out of Scope

- Multi-document workflows (one agent simultaneously tuning two models). The primitives are per-`doc_id` and naturally compose, but coordination is left to the agent.
- Bound-violation auto-clamping. The API rejects out-of-bounds; clamping policy belongs to the caller.
- Streaming description / hot-reload of the AST when the document changes. `describe_model` is a snapshot; callers re-call after edits.
- Reverse direction (subscribing to value changes when an input is set externally by another caller). `SubscribeTelemetry` already covers value streams; this spec does not add a parallel channel.

## Assumptions

- The simulation worker already supports between-step input updates (verified — `UpdateParameters` exists in `crates/lunco-modelica/src/lib.rs` and the worker squashes them).
- AST input/parameter extraction already returns enough metadata (verified — `ast_extract::collect_*` functions return name, default, bounds, description).
- MSL search is acceptable as a substring match initially; better ranking (token-overlap, classname-prefix preference) is an iteration on `FindModelProvider`, not a re-architecture.

## Implementation Phases

This spec depends on spec 032's `ApiQueryProvider` infrastructure (P1 of 032). Once 032 is fully landed:

- **P0** — Multi-class compile + source visibility (User Stories 1.5, 1.6). Unblocks the immediate AnnotatedRocketStage workflow without touching the worker's input channel.
  - `ListCompileCandidatesProvider` + `CompileStatusProvider` + `GetDocumentSourceProvider`
  - Extend `CompileActiveModel` with optional `class: String` (empty = inherit picker behaviour)
  - Optional `SetActiveClass` Reflect event (writes `DrilledInClassNames`).
- **P1** — `DescribeModelProvider` + `SnapshotVariablesProvider` (read-only, no worker plumbing changes).
- **P2** — `SetInputCommand` (extend the existing worker input channel; reuse squashing).
- **P3** — `FindModelProvider` (fuzzy match across the listing endpoints from spec 032).
- **P4** — End-to-end CI script realising User Story 5 + MCP wrappers + docs.
  - Smoke script lives at `tests/api/agent_workflow.sh`; covers
    find → open → list_open_documents → list_compile_candidates →
    compile_model(class) → compile_status poll → describe_model →
    set_input (happy path + error path) → ResumeActiveModel →
    snapshot_variables → PauseActiveModel. Run after starting the
    workbench with `--api 3000`. Does not pkill or send Exit; safe to
    run against a live user session.
