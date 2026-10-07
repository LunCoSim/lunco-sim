---
name: run-modelica
description: >
  Recipe for running Modelica models and building experiments/graphs in
  lunica (LunCoSim), driven from the HTTP API with curl. Trigger whenever
  you need to: launch the workbench, open/compile a Modelica model, run it
  live (interactive realtime) or as a fast batch, sweep parameters across
  many runs, read simulation results/trajectories, poke runtime inputs, or
  plot and compare runs — without asking the user to click. Covers the
  `--api` launch, the `POST /api/commands` envelope, the command + query
  catalog, run bounds/solver semantics, and reading experiment results.
  Prefer curl over the MCP `mcp__lunco__*` tools — the MCP bridge is often
  unavailable; every MCP tool has a curl equivalent shown here.
---

# Run Modelica models & build experiments

lunica exposes a reflect-registered command API and structured query
providers over `POST /api/commands`. **Drive everything with curl.** The
`mcp__lunco__*` tools mirror this API but are frequently down — use curl as
the primary surface; only fall back to MCP if a human explicitly asks.

## 0. Launch an app in API mode

Modelica runs inside any app that embeds `LunCoApiPlugin` + the Modelica
workbench. Build the named binary in the current worktree, then invoke it
directly. The API server only exists when you pass `--api`. Default port is
**4101** (`lunco_api_contracts::DEFAULT_API_PORT`).

Before creating a new Modelica file, inventory the maintained package roots and
the closest composed USD network in the current checkout. Reuse existing
component classes through USD or the generated wrapper when their equations and
public contract fit; add a new mission/vehicle model only when the equation or
interface is genuinely absent. An isolated Modelica compile proves neither USD
wiring nor runtime physics.

Compile admission preserves an existing participant/vehicle name; generated
class identity belongs to simulation metadata. Verify a control HUD against
the physical vehicle identity after compile and reload.

For shared plume calculations, reuse
`LunCo.Propulsion.computePlumePhotometry`. `PlumePhotometry` supplies named USD
ports; `RCSJet` calls the function directly with static nozzle parameters.
See [USD-driven visuals](../../docs/architecture/50-usd-driven-visuals.md) for
the fixed-size result contract. Validate model refactors through the production
`rcs_feed_starvation` and `rocket_engine_plume_defaults` scene gates, then
measure cold solver preparation and running-step cost separately.

For material-dependent equations, use the component's typed SysML material
assignment and property source described in the
[mission engineering material gate](../interactive-component-authoring/references/mission-engineering-quality.md#physical-materials-and-engineering-properties).
Keep values and units typed until the Modelica-source emission boundary, and
derive generated parameter literals from that source rather than maintaining a
second hand-edited material table in `.mo`. The shared material catalogue and
generic cross-domain binding are not yet established; if the current bridge
cannot provide a required property with its unit and applicability, report that
generic gap instead of substituting a copied constant.

For radiation or surface thermal models, also resolve the typed finish/coating
assigned to the analyzed face and use its sourced engineering optical/thermal
properties. `UsdShade` shader presets are presentation mappings only and must
not be used as Modelica property sources.

| App | Launch | Modelica surface |
|---|---|---|
| **`lunica`** | `"$LUNICA_BIN" --api 4101` | **The Modelica workbench itself** — nothing to switch to. Prefer this for pure Modelica work. |
| **`luncosim`** | `$LUNCOSIM_BIN --api 4101` | Ground-physics simulator; an explicit `--scene` launch opens **View**, while Modelica lives under the **`modelica_analyze` perspective** — switch to it (below) before diagrams/plots render. |
| **`luncosim-server`** | `$LUNCOSIM_SERVER_BIN --api 4101` | Headless LunCoSim host; use the GUI `luncosim` command for the workbench. |

**In `luncosim`, switch to the Modelica view before plotting/screenshotting.**
The compile/run/experiment *commands and query providers work regardless* (they're
headless-safe), but the diagram/plot panels only paint when their perspective is
active. Switch with:

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands -H "Content-Type: application/json" \
  -d '{"type":"ExecuteCommand","command":"ActivatePerspective","params":{"id":"modelica_analyze"}}'
# other ids: "sandbox_view", "rover_build". Reset a broken layout: {"type":"ExecuteCommand","command":"ResetWorkspaceLayout","params":{}}
```

- Add `--no-ui` for a headless compile/run server (no window, no GPU). The API
  surface is identical; screenshots, diagrams, and 3D viz are what you lose (so
  perspective switching is moot). `GetExperimentResult`/`SnapshotVariables` still
  give full numeric results headless.
- Use production `luncosim` for scene-test and visual evidence. Use
  `luncosim-server` or `luncosim --no-ui` for numeric/API evidence only, and
  report the binary, revision, readiness state, and evidence type separately.
- Launch your own session on an explicitly free API port. Never reuse or stop
  another session. Verify PID, executable, and checkout before controlling your
  session; use its API `Exit` and verify shutdown before replacing its binary.
- Standalone Modelica ports exclude entities with `SimComponent`; the
  co-simulation backend owns their topology publication. Live sample changes
  must not invalidate resolved handles without a structural contract change.

### No-API alternative
The standalone runner is owned by `lunco-modelica-execution`, not
`lunco-modelica-core`. For a one-shot current-source compile and fixed-step
solve with CSV output:

```bash
cargo run -p lunco-modelica-execution --bin modelica_run -- \
  assets/models/AnnotatedRocketStage.mo AnnotatedRocketStage.RocketStage \
  --duration 30 --dt 0.001 --input valve_command=0.7 \
  --record altitude,velocity --output /tmp/run.csv
```

This runner uses the shared compiler and solver path, but it is not a substitute
for the active Twin/API test when the source depends on Twin-indexed packages,
Editor state, or scene composition.

Wait for readiness with an `until` loop (never chained `sleep`s):

```bash
until curl -s -o /dev/null -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"type":"ExecuteCommand","command":"Ping","params":{}}'; do sleep 1; done
```

Stop with the `Exit` command (never `pkill`/`kill` — those need user confirm):

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" -d '{"type":"ExecuteCommand","command":"Exit","params":{}}'
```

### Structured package lookup

### Attach a Modelica source to a USD body

For a multi-domain run, use the shared `AttachProgram { doc_id, spec }` command
instead of writing marker components or maintaining a separate binding table.
The spec authors the `LunCoProgramAPI` child, explicit scalar inputs and
outputs, and native USD connections in one journaled change set. In Rhai the
same surface is `assembly_edit::attach_program(...)` with
`assembly_edit::program_input_connection(...)`,
`assembly_edit::program_input_default(...)`, and
`assembly_edit::program_output(...)` helpers. Verify the
result with `ListPorts`, `CosimStatus`, and `GetBrokenConnections`; a source
without declared ports is reported as source-only and does not step.

Modelica packages under `assets/models/<Root>/` and inside a Twin use the standard
`package.mo`/`package.order` layout and members' `within` declarations. A Twin may
declare `[modelica].paths` (Twin-relative, with `"."` meaning the Twin root) and
`externals` in `twin.toml`. Without that section, the Modelica owner derives roots
from the indexed Twin `.mo` files, so package discovery does not depend on a
hard-coded folder. A qualified reference such as `LunCo.Electrical.Battery` is
resolved by its root segment through the normal Modelica search-path inventory;
do not add a library-specific Rust load call. Every live compile admits its
required roots through the existing `LoadSourceRoot` worker path before it
sends `Compile`. File-backed source assets carry root requirements from their
prepared AST interface, document compiles derive them from the primary and
sibling ASTs, and a generated policy's `source_roots` list is its required root
manifest. The worker commits root preparations in admission order before
compilation, rejects unadmitted roots, and reports failed roots without retrying
synchronous discovery. Synchronous compiler helpers remain for CLI and batch
callers.
The policy list does not replace composed USD facts for member-class
discovery. The worker's prepared-solve cache keys library state from the
revisions that `ModelicaCompiler` records while admitting source roots. Its
source key is captured from the successful strict compiler's participating-source
closure before clearing user overlays. Generated class/network-title identity is
normalized; equations, parameters, initial values, participating siblings, and
contributing library bytes remain structural. Unrelated sibling edits and runtime
document IDs do not invalidate prepared solve IR. The captured key remains with
the immutable DAE through shared reuse and resets; the precompile DAE cache uses
the full submitted input set until the strict closure is known. It does not scan
the complete Modelica tree during the first live stepper build.

For preparation evidence, use actual owner INFO logs: `cache=memory-hit` at
in-memory admission, `cache=disk-hit` after persistent lookup, and `cache=miss`
when lowering runs. Disk preparation includes lookup, lowering, and total
duration plus the source key. Stepper construction is not evidence of cache reuse.

Generated USD networks compile their complete synthesized source plus admitted
source roots; unrelated editor documents are not sibling inputs. The reserved
bundled `generated/` filename namespace drives provenance and structural cache
classification. Verify repeat-load cache reuse while changing document-open
order; authored Modelica documents retain their multi-document source sets.

Domain discovery does not resynthesize ordinary scene-owned networks when a
content prim receives its GID: that identity does not enter their namespace.
Instance-scoped identities and unsettled provenance remain discovery inputs,
alongside prim paths, stage revisions, wiring, and member-source events.

Initial network results may publish as a bounded four-root prefix only while
their own fixed-clock admission hold is present and no prior projection is
installed. Live replacements remain one per Update; repeated results for a
root wait for ECS publication. Worker completion never bypasses the oldest
request or its Twin/stage/instance validation.

On native runs, one dedicated Rumoca actor owns the mutable session and shared
DAE cache. Source-root installation, `Compile`, `Reset`, parameter updates, and
cache-invalidating Step auto-init share a FIFO. The worker commits results in
submission order after entity-session and library-generation checks; a Step
that needs a rebuild resumes only after its initialization commits. These
continuations leave the Modelica command owner free to service other entities,
and actor requests plus solve preparations share bounded admission. Persistent
solve-cache reads and writes run with DAE lowering in the preparation pool; the
native owner thread does not perform cache I/O or fall back to synchronous
lowering. Wasm keeps compilation inside its Modelica Web Worker.

Shared immutable RAM caches use the captured
`lunco_modelica_runtime::ModelicaCacheLimits` resource: positive compiled and
prepared entry capacities, each defaulting to 64 per worker. Follow the
[Modelica cache contract](../../docs/architecture/20-domain-modelica.md) for
configuration timing, FIFO reuse, and browser startup/respawn admission. Entry
caps bound graph count rather than heap bytes; eviction preserves live owners.
Run the generic `immutable_reuse_cache_` tests and compile the browser worker
bundle when changing configuration transport. Do not infer Twin teardown from
shared immutable cache contents.
For shared browser worker transport changes, run the generic `worker_lifecycle_`
tests in a real browser and compile both Modelica execution and DEM bake
consumers. Replacement must release the old handlers, including replacement
inside an executing callback; failed replacement leaves an empty slot.
For native compiler heartbeat changes, run the generic `compile_heartbeat_`
lifetime tests; ensure normal, error, and unwind exit interrupt the wait and
release the owned thread.

Native optional solve-cache reuse captures the cache-owned
`lunco_modelica_worker::worker::PreparedSolveDiskLimits` resource before
`ModelicaExecutionPlugin` starts the worker. Defaults are 32 retained records, 64 MiB compressed,
256 MiB decoded, and a 64 MiB zstd window; see
[the cache ownership contract](../../docs/architecture/20-domain-modelica.md) for
valid ranges and configuration timing. Missing records are cache misses;
rejected existing records warn and recompute admitted equations. Invalid limits
are a typed worker startup failure. For decode changes, run the generic
`prepared_solve_disk_cache_` tests; do not substitute repository models for the
inline-storage boundary fixtures.
For publication/retention changes, also run `persistent_solve_cache_` and storage
`cache_directory_transaction_` tests, including the native Windows CI path.
Keep heavy encoding outside the storage transaction; never remove its lock
file or broaden retention beyond the owned cache namespace. Optional publication
failures warn while preserving the admitted solver result.

On native desktop startup, cache-miss solve-IR lowering runs in the worker's
bounded preparation pool because the DAE input and solve options are immutable.
The worker alone commits the resulting solve model and constructs the live
stepper; `Step`, `Reset`, parameter updates, and source-root changes remain
ordered behind that commit. Persistent solve-IR entries are keyed by structural
source identity, the content-sensitive admitted-library revision, solver, and
parameter overrides. A source-root change clears worker-local prepared models
while retaining disk entries for future matching revisions. Readiness is still
the completion barrier, so
physics must not be started before `/api/ready` reports `ready=true`,
`world_hold=false`, and `pending_count=0`.

Bundled and workspace source roots are parsed to completion before admission and
installed as one parsed source set. A failed member therefore keeps the root out
of the Rumoca session; do not treat a nonzero parsed count as readiness.

The registry marks each root as Application- or Twin-owned. Twin root IDs include
the stable Twin ID. `TwinClosed` removes only that Twin's source sets and queues
ordered unloads to the Rumoca actor; other Twin and application roots remain.

Twin closure also retires its open Modelica documents through the core
`CloseDocument` owner, including linked execution entities. Workspace replacement
closes every editor document before `TwinAdded`; unfinished UI preparations are
canceled and file-read completions are fenced by their admitted runtime owner.
Individual document closure also cancels parse preparation, save-close
continuations, and document-owned modals before retiring the editor state.
Reopening the same Twin starts fresh. `RestartScene` leaves editable documents
resident while rebuilding their scene-owned execution.

Operation IDs fence an in-flight file read from installing a source set after
its owner closes. If the worker has not created its channel yet, pending unloads
stay queued and are sent before a later Twin load. A disconnected channel is a
terminal unload failure: it reports a runtime fault, retains the queued
operation, and blocks later Twin root admission instead of retrying on every
update.

For policy-owned generated models, keep contract assertions in authored
`assets/scenarios/tests/*.rhai` scenes; standalone live probes may use
`assets/scripting/tests/*.rhai`. Rust should provide the composed facts and
invoke the registered policy; Rhai should assert the generated source,
topology, layout, and UI metadata. The policy result is strict: it must return
`source`, `units`, `layout.units`, `layout.members`, `source_roots`, and
`member_output_aliases` (the last may be an explicit empty array). Missing or
invalid fields are projection errors; do not add a Rust-side generated-model
fallback. `layout.units` uses root-diagram coordinates, while each entry in
`layout.members` is local to the owning unit diagram; member overlaps are
checked within that unit coordinate system.

On native development checkouts, the active prelude and policy files are read
from `assets/scripting/` at startup, so Rhai edits require a restart rather than
a Rust rebuild. A present editable directory is authoritative: unreadable or
empty directories and parse failures are errors. Packaged/wasm builds use their
compiled-in asset set because no editable source tree is available.

When reviewing a generated diagram, click its generated browser row first. A
single-unit network opens the unit-level class and shows its real members;
multi-unit networks open the root wrapper. Use `FitCanvas` after drill-in when
the tab was opened alongside the root, since navigation is scoped to the
focused Modelica tab.

For electrical generated networks, verify the unit diagram's labelled power
bus and follow at least one routed `connect(...)` branch through the rail. The
Rhai policy uses readable `network_system`/`network_unit_N` unit instances and a
topology-derived hub with adaptive branch lanes, so inspect a larger network
with `FitCanvas` rather than assuming the six-member demo's geometry scales.
Components that need directional presentation apply
`LunCoModelicaTopologyAPI` with `source`, `storage`, or `load`; this metadata
does not alter acausal solver direction.
Member icons must resolve from
their native Modelica classes; a fabricated card or direct solar-to-motor wire
is a projection defect, not an acceptable fallback.

When an authored Modelica endpoint also carries `InputPorts`, that component is
the single public command boundary. `SetPorts` and Rhai writes land there, and
the generic Modelica bridge mirrors only names accepted by the compiled model
into its solver input buffer. External live `SetPorts` commands with a stable
producer id enter the shared next-fixed-tick queue and are captured as ordered
named writes; Simulation-clock Rhai writes remain derived behavior. Do not add a
vehicle-specific setter. External API/direct and non-Simulation Rhai
`ReleasePort`/`ReleaseControl` commands use the same producer identity and
ordered session boundary. Releases clear local holds only; Twin policy sends
explicit setpoints with `SetPorts`. Battery
empty events use the authored 0.1% usable-storage reserve in `Battery.mo`, not
a solver-epsilon comparison.

Node movement has two valid outcomes. On an editable `.mo` document, drag a
component and verify that the standard `annotation(Placement(...))` changes in
the source and survives a re-projection. On a generated document, the canvas
is intentionally read-only because USD plus the Rhai policy owns the source;
use `Duplicate to edit`, then perform the same placement check. A drag that
appears to work but disappears on reload is a product bug, not an acceptable
generated-model editing mode.

Use the shared AST `strip_within_prefix` for qualified lookup and editing;
similarly spelled package prefixes must not match an authored `within` package.
Reuse the AST's `qualified_name_segments` and `parent_qualified` for class paths;
quoted identifiers containing dots remain one segment. Extraction checks do not
prove the compiler accepts that qualified name: validate compilation separately
through its actual command/error boundary.

For class source extraction, reuse `lunco-modelica-ast::ast_extract::class_full_text_span`
from the same parsed bytes. Its inline seam test covers same-line enclosing
declarations and class qualifiers; do not derive declaration bounds by scanning
backward through arbitrary identifiers.
Read-only library views also require exact qualified identity. The generic
library extraction seam checks sibling leaf names and a missing sibling without
reading repository fixtures.

Duplicate admission pins its original target scope and resident source snapshot.
Check an exact nested qualified class when sibling packages share a short name;
known-source errors must report failure without installing a substitute document.
Closing the source or target lifetime during preparation must cancel publication
and release pending names, tabs and status. Use inline parser tests for span and
rewrite boundaries, and authored Rhai commands/queries for these public lifecycle
outcomes. `assets/scenarios/tests/modelica_duplicate_lifecycle.rhai` exercises
qualified siblings and quoted names, error diagnostics and exact-name reuse,
observed pending source-close cancellation, original-target Twin replacement
without retagging, and a valid successor. Run it in a UI-capable host with
Application/Retain lifetime and API-authored `root_a`, `root_b`, `entry_scene`
fixtures. Asynchronous folder scanning permits a duplicate to install while its
old Twin is live; the gate preserves that valid retained-source outcome. Exact
Twin/connection retirement of never-ready tasks belongs to the generic resource
seam test, since public folder replacement cannot force that worker boundary.
Native preparation runs on the existing task pool; browser Bevy tasks
retain the browser editor's deferred parsing contract.

Projection must remain responsive while a native package or inherited icon is
being resolved. Verify that `/api/ready` stays responsive, the canvas shows an
explicit loading/error state, and the completion event reprojects the authored
icons. Do not add a synchronous parse, mutex wait, invented icon, or domain
specific visual retry path to hide a miss.

Energy-flow animation is generic Modelica behavior, not generated-policy code:
`LunCo.Electrical.Pin.i` is a standard `flow Real`, just like the rocket and
lander `FluidPort` flow variables. Confirm the connector projection reports the
flow variable and that a non-zero live `instance.p.i` moves dots along the
rendered edge; zero current must remain visually idle. If dots are absent,
inspect the flow metadata and node-state keys at the shared canvas owner before
adding any policy-specific renderer.

## 1. The request envelope

Everything is one endpoint: `POST /api/commands`. The JSON shape is always
`{"type":"ExecuteCommand","command":"<Name>","params":{...}}`. **Always include `params` even when
empty** (`"params":{}`) — this keeps every request explicit and discoverable.

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"type":"ExecuteCommand","command":"<Name>","params":{ ... }}'
```

Two kinds of `command` share this envelope:

- **Commands** (fire-and-forget mutations): return `{"data":{"accepted":true}}`; result-returning commands put their command-specific payload in the same `data` envelope.
  Invalid parameters return HTTP 422. A deferred command may complete its
  command acknowledgement later on the same request; that acknowledgement is
  not necessarily completion of the domain work. In particular,
  `RunExperiment` returns its exact `experiment_id` once the run is registered,
  while the numerical solve continues asynchronously.
- **Query providers** (return data): return the payload directly, e.g.
  `{"runs":[...]}`. `ListRuns`, `GetExperimentResult`, `DescribeModel`,
  `SnapshotVariables`, `CompileStatus`, `GetDiagnostics`, `ListCompileCandidates`,
  `ListBundled`, `ListOpenDocuments`, `FindModel` are all query providers —
  invoked with the same tagged `ExecuteCommand` form. Built-in discovery and
  entity listing use their own explicit `type` values.

`doc_id: 0` always means "the active document/tab".

## 2. Two run modes — pick the right one

| | **Interactive (live)** | **Batch (Fast Run / Experiment)** |
|---|---|---|
| Verb | `RunActiveModel` | `FastRunActiveModel` / `RunExperiment` |
| Pace | wall-clock realtime, steps forever | as fast as possible, `t_start→t_end`, then stops |
| Use for | inspection, physics-in-loop, 3D viz, possession | parameter sweeps, regression, "what if I bump this constant?" |
| Read results | `SnapshotVariables` (live), `ReadPorts`/`WatchPorts` | `GetExperimentResult` (full trajectory) |
| Poke inputs | `SetModelInput` (admitted for the next fixed tick in a live session) | overrides baked into the run request |
| Stored as | live stepping model | first-class `Experiment` in the registry (plot/compare) |

## 3. Recipe A — run a model live (interactive)

```bash
API=http://127.0.0.1:4101/api/commands
post(){ curl -s -X POST $API -H "Content-Type: application/json" -d "$1"; }

# 1. Open a model. Prefer the unified opener (bundled example / qualified source-library name / path):
post '{"type":"ExecuteCommand","command":"Open","params":{"uri":"bundled://SpringMass.mo"}}'
#    bundled://Name.mo | Modelica.Blocks.Examples.PID_Controller | /abs/path.mo | mem://Untitled
#    List embedded examples first: {"type":"ExecuteCommand","command":"ListBundled","params":{}}

# 2. Wait for the AST parse (background). Poll CompileStatus until ast_parsed:true:
post '{"type":"ExecuteCommand","command":"CompileStatus","params":{"doc_id":0}}'   # -> {state, ast_parsed, candidates, picker_pending, ...}
#    Read parser/compiler/lint findings and actionable Rumoca suggestions.
#    Poll while complete is false; pending is not a clean result.
post '{"type":"ExecuteCommand","command":"GetDiagnostics","params":{"doc_id":0}}' # -> {state,complete,channels[],diagnostics:[{domain,source,code,severity,message,uri,line,column,suggestion}]}

# 3. Compile + play. class REQUIRED if the file has >1 non-package class
#    (the GUI picker can't be shown over the API). Discover choices:
post '{"type":"ExecuteCommand","command":"ListCompileCandidates","params":{"doc_id":0}}'   # -> {candidates:[{qualified,short}]}
post '{"type":"ExecuteCommand","command":"RunActiveModel","params":{"doc_id":0,"class":"SpringMass"}}'

# 4. Read live values (t + parameters + inputs + variables). Filter with names:
post '{"type":"ExecuteCommand","command":"SnapshotVariables","params":{"doc_id":0,"names":["x","v"]}}'

# 5. Poke a runtime input live (no recompile, admitted for the next fixed tick):
#    For an open workbench model, use doc_id and keep producer_id stable.
post '{"type":"ExecuteCommand","command":"SetModelInput","params":{"doc_id":0,"name":"F","value":10.0,"producer_id":4101}}'
#    For a live Twin participant, use its stable target_gid from ListEntities
#    and leave doc_id at 0; do not pass both selectors.
post '{"type":"ExecuteCommand","command":"SetModelInput","params":{"doc_id":0,"target_gid":123456,"name":"throttle","value":0.5,"producer_id":4101}}'

# 6. Pause / Resume / Reset / Restart:
post '{"type":"ExecuteCommand","command":"PauseActiveModel","params":{"doc_id":0}}'
post '{"type":"ExecuteCommand","command":"RestartActiveModel","params":{"doc_id":0}}'   # reset t=0 then run
```

`RunActiveModel` = compile-if-stale then play. If already compiled & clean it
just unpauses (no recompile). `CompileModel` compiles only (stays paused);
`ResumeActiveModel` unpauses only.

If a live compile fails on an unbalanced DAE, its ordinary Modelica Error event
in Recent status events explains why simulation did not start and lists the
unknowns Rumoca's structural matcher could not pair with equations, plus their
categories and referencing equation rows. Selecting the row expands the full
message. `GetDiagnostics` also returns the explanation with the structured
compiler findings. Those names identify values the current equations cannot
determine; add or correct independent equations or constraints. The diagnostic
DAE is never simulated.

## 4. Recipe B — build an experiment (batch + parameter sweep)

`RunExperiment` is the agent-facing sweep verb: overrides come from the
**command**, not the UI, so you can sweep parameters without touching source.
Each run is stored as an `Experiment`. Its command acknowledgement contains
the exact `experiment_id` after registration; the acknowledgement is not a
completed numeric result. Retain that id, poll `RunStatus` with it, and read the
trajectory with `GetExperimentResult` using the same id. Do not identify a run
by its label or by whichever run is newest.

Runs capture their exact `ExperimentOrigin` at admission: a pinned local document
runtime lifetime or an authenticated replicated connection/mount. Presentation
grouping never determines ownership. Definition replay under another origin
rejects before mutation; same-origin replay preserves terminal history. Closing that Twin
cancels unfinished runs and retires late updates and playback signals; completed
results stay queryable within the bounded history. Loose-document runs have
application lifetime. A run's execution definition freezes when its registry
row becomes `Queued`, before runner admission. Change bounds, parameters, or
inputs by creating a new run; retained results keep the definition that produced
them. An identical definition replay preserves later display labels and results,
and conflicting replay is rejected visibly.
A completion must match its immutable pending-handle origin before registry
publication. A completion from an inactive Twin cannot automatically
select the replacement Twin's plot, and a different pinned experiments document
cannot receive its automatic plot selection. Registry deletion and bounded
eviction also remove the retained document/owner attribution and plot visibility.
Closing a Twin retires its document pins and archived plot selections. A run
receives immutable source text at admission, and annotations are read from its
current document; a same-name model in another Twin cannot supply either. Native cancellation uses the admitted run flag at evaluation and simulation-driver
checkpoints. An executing numerical kernel returns before its next checkpoint;
no cancelled trajectory publishes, and worker exit releases the scheduler slot.

For a production lifecycle regression, admit a run from a Twin document and
queue another behind it, replace the Twin, then query both exact run ids for
`Cancelled`. Verify the replacement's plot and playback signals receive no old
completion. Include an application-owned loose-document run as a negative
ownership case, plus a completed run whose retained result remains readable.
For the same-name source/bounds regression, launch
`RunScenarioAsset` with `source_asset:
"lunco://scenarios/tests/modelica_run_admission_isolation.rhai"` in an owned
UI-capable Modelica host. Its bounded asynchronous gate uses two scratch
documents, the ordinary `RunExperiment` acknowledgements and exact run ids,
and verifies that the unannotated document keeps the one-second default and
each trajectory retains its own admitted source. Require the authored
`MODELICA_RUN_ADMISSION_ISOLATION` verdict; launch acceptance alone is insufficient.

```bash
# One run with a parameter override + custom bounds + a label:
post '{"type":"ExecuteCommand","command":"RunExperiment","params":{
  "doc_id":0, "class":"RocketStage",
  "overrides":[{"name":"Isp","value":"300"}],
  "inputs":[{"name":"throttle","value":"1.0"}],
  "t_start":0, "t_end":120, "n_intervals":600,
  "solver":"bdf", "tolerance":1e-6,
  "label":"Isp=300"
}}'
```

Sweep = loop the same call with different overrides + labels (one run each):

```bash
for isp in 280 300 320 340; do
  post "{\"type\":\"ExecuteCommand\",\"command\":\"RunExperiment\",\"params\":{\"doc_id\":0,\"class\":\"RocketStage\",
    \"overrides\":[{\"name\":\"Isp\",\"value\":\"$isp\"}],
    \"t_end\":120,\"n_intervals\":600,\"label\":\"Isp=$isp\"}}"
done
```

`overrides` / `inputs` are `[{name, value}]` with **string values** (string
injection, v1). `overrides` = top-level `parameter` literals; `inputs` =
runtime input variables.

### Bounds & solver semantics
- `t_start` / `t_end` — sim horizon (seconds). Default from model annotation.
- `dt` — output **Interval** (seconds between samples). Mutually exclusive with…
- `n_intervals` — output **NumberOfIntervals**: emits `n+1` evenly-spaced
  samples. Takes precedence over `dt` when set.
- `tolerance` — solver tolerance.
- `solver` — family: `"bdf"|"dassl"|"ida"` → BDF; `"esdirk34"|"rk"|"dopri"|"trbdf2"`
  → ESDIRK34; `"auto"`/omit → backend default (BDF).
- `h0` — initial step size (seconds).
- Omit any field to fall back to the model's `experiment(...)` annotation, then
  the backend default.

Bounds admission rejects nonfinite or non-increasing horizons, nonpositive
explicit `dt`, `tolerance`, or `h0`, and output grids exceeding 200,000
intervals. `n_intervals` must be positive. Rejection returns the owning error
without registering a run or changing the requested grid. A used authored
`NumberOfIntervals` must also be a finite positive integer within that limit;
only authored `Interval=0` has the documented omitted-spacing meaning.
`QueryExperimentBounds` reports invalid annotation bounds as a query error.
Production regression: attach
`lunco://scenarios/tests/modelica_run_bounds_admission.rhai` with
`RunScenarioAsset` to an addressable loaded scene root in an owned Modelica
host. It checks rejected explicit and authored bounds leave no experiment
rows, then requires a valid successor to complete.

`FastRunActiveModel` is the same batch engine but reads bounds from the UI
"Simulation Setup" draft instead of the command — prefer `RunExperiment` for
scripted/agent runs so everything is explicit.

For Rhai callers, `cmd("RunExperiment", ...)` can return a pending command
record while the deferred acknowledgement is being assembled. Poll
`command_result(command_id)` only until that acknowledgement supplies the
`experiment_id`; then poll `RunStatus` by that exact experiment id. The
`modelica_editor::experiment_ticket` / `poll_experiment_ticket` helpers wrap
both phases in a bounded, non-blocking caller-owned ticket. Keep the returned
ticket across ticks, and do not block the Editor/Rhai thread in a wait loop.
The ticket also binds readback to the explicit Modelica document and source
generation so a changed model cannot be mistaken for the solved source.

## 5. Recipe C — read experiment results

```bash
# List runs (newest first). Optional {"doc_id":N} filter. Each row is self-describing:
# experiment_id, name, state (Pending|Queued|Running|Done|Failed|Cancelled),
# wall_time_ms, the overrides that produced it, and the bounds it ran under.
post '{"type":"ExecuteCommand","command":"ListRuns","params":{}}'

# Pull a full trajectory: times + series (dotted Modelica path -> samples).
# For an explicit RunExperiment, always use its exact experiment_id:
post '{"type":"ExecuteCommand","command":"GetExperimentResult","params":{
  "experiment_id":"<id returned by RunExperiment>",
  "variables":["altitude","velocity"], "max_points":500
}}'
# max_points = strided downsample, final sample always kept. Omit = uncapped.
# Returns {state:"Done", times:[...], series:{"altitude":[...], ...}} or an
# error if the run is not Done (Pending/Running/Failed-without-partial).
```

`RunStatus` is the query provider for one run's progress and terminal state:

```bash
post '{"type":"ExecuteCommand","command":"RunStatus","params":{"experiment_id":"<exact id>"}}'
```

Poll until `done`, `failed`, or `cancelled`; only request the trajectory after
`done`. `ListRuns` is useful for discovery and UI review, not as a substitute
for retaining the id returned by the dispatch you just made.

A native worker panic, thread-admission failure, or disconnect without a
terminal result produces `failed` with its cause and retires the handle.
Live-worker startup or transport failure produces a worker-wide typed diagnostic,
holds the affected simulation, and rejects subsequent live compiles. Scene reload
does not restart a failed application worker; restart the owned app after resolving
the reported startup problem.
The scheduler releases the worker slot on every exit. A compiler poisoned by a
panic rejects subsequent compilation visibly; treat that diagnostic as a
failed run rather than polling indefinitely.

Cancel / clean up:
```bash
post '{"type":"ExecuteCommand","command":"CancelExperiment","params":{"all":true}}'           # or {"experiment_id":"<uuid>"}
post '{"type":"ExecuteCommand","command":"DeleteExperiment","params":{"all":true}}'           # terminal runs only
post '{"type":"ExecuteCommand","command":"RenameExperiment","params":{"experiment_id":"<uuid>","name":"baseline"}}'
```

## 6. Recipe D — visualize & compare runs (plots)

### How the experiment→plot model works
The Experiments panel **is** the comparison view — unlike Dymola/OMEdit you
don't juggle `.mat` filenames. It's one multi-series plot that draws a curve
for **every _visible run_ × every _picked variable_**. So:

- **Variables** you pick (e.g. `altitude`, `velocity`) = which series shape.
- **Runs** that are visible = which experiments overlay on top of each other.
- A 4-run Isp sweep with 1 picked variable → 4 curves (one per Isp), auto-
  labeled by run. Pick 2 variables → 8 curves. Comparison is the default.
- New runs **overlay automatically** as they finish `Done` — no re-plotting.

Two pickers live on the panel header (GUI): **▾ Variables N/M** (which signals)
and **▾ Runs** (which completed runs to overlay). Y-axis auto-groups by unit.

### Driving it from the API
```bash
API=http://127.0.0.1:4101/api/commands
post(){ curl -s -X POST $API -H "Content-Type: application/json" -d "$1"; }

# Open a plot tab seeded with the variables to compare across runs.
# source=0 = fresh panel; source=<VizId> = clone another plot's signal set + picks.
post '{"type":"ExecuteCommand","command":"NewPlotPanel","params":{"title":"Ascent","signals":["altitude","velocity"],"source":0}}'

# Add another signal to an existing plot (plot=0 = the default graph):
post '{"type":"ExecuteCommand","command":"AddSignalToPlot","params":{"plot":0,"signal":"mass"}}'
```

`signals` in `NewPlotPanel` become the plot's **picked variables**; every
completed run then contributes those series. Run one sweep (§4), open the plot
once with the variables you care about, and each new run lands on the same axes.

### Typical end-to-end: sweep → compare
```bash
# 1. sweep 4 runs (see §4 loop) with labels Isp=280..340
# 2. open the comparison plot on the variable of interest
post '{"type":"ExecuteCommand","command":"NewPlotPanel","params":{"title":"Isp sweep","signals":["altitude"],"source":0}}'
# 3. confirm the runs landed, then screenshot for the human
post '{"type":"ExecuteCommand","command":"ListRuns","params":{}}'
curl -s -X POST $API -H "Content-Type: application/json" \
  -d '{"type":"ExecuteCommand","command":"CaptureScreenshot","params":{}}' -o /tmp/sweep.png   # then Read the PNG
```

### Numbers vs pixels
- **Analysis / assertions → `GetExperimentResult`** (§5). Raw `times`+`series`;
  compare runs by fetching each `experiment_id` and diffing arrays. Never scrape
  a plot widget for values.
- **Show the human → `CaptureScreenshot`** (needs the UI build, not `--no-ui`).
- **Export → CSV**: the GUI's per-panel CSV export mirrors `GetExperimentResult`;
  for scripted export just persist the `GetExperimentResult` JSON yourself.

## 7. Command & query catalog

**Discovery / docs**
| command | params | returns / effect |
|---|---|---|
| `Ping` | `{}` | readiness check |
| `CreateNewScratchModel` | `{source, name}` | create a Modelica Editor document and return its exact `doc_id` |
| `ListBundled` | `{}` | embedded example models (`bundled://` URIs) |
| `FindModel` | `{query, limit?}` | fuzzy search examples/Twin/source libraries/open docs → URIs |
| `Open` | `{uri}` | open bundled/source-library/path/mem into a tab |
| `ListOpenDocuments` | `{}` | `doc_id, title, kind, origin, dirty, active` per tab |
| `DescribeModel` | `{doc, class?}` | AST: components, connections, inputs, parameters, outputs (pre-compile) |
| `CompileStatus` | `{doc}` | `state, ast_parsed, candidates, picker_pending, drilled_in_class` |
| `GetDiagnostics` | exactly one of `{doc_id}` or `{scope}` | shared parser/compiler/lint diagnostics; poll `complete`, then read channel states, stable codes, source locations, and suggestions. `scope` is `loaded_stages` or `twin` |
| `ListCompileCandidates` | `{doc}` | `{candidates:[{qualified,short}]}` — the picker choices |

**Compile & run**
| command | params | effect |
|---|---|---|
| `CompileModel` | `{doc, class?, force?, resume_after_compile?}` | compile only (stays paused) |
| `RunActiveModel` | `{doc, class?}` | compile-if-stale + play (live) |
| `PauseActiveModel` / `ResumeActiveModel` / `ResetActiveModel` | `{doc}` | live stepping control |
| `RestartActiveModel` | `{doc}` | reset t=0 then run |
| `FastRunActiveModel` | `{doc, class?, t_end?, dt?, n_intervals?, tolerance?, solver?, h0?}` | batch, bounds from UI draft |
| `RunExperiment` | `{doc, class?, overrides[], inputs[], t_start?, t_end?, dt?, n_intervals?, tolerance?, solver?, h0?, label?}` | dispatch a batch run; acknowledgement returns exact `experiment_id` |
| `SetModelInput` | `{doc_id, target_gid?, name, value, producer_id?}` | select an editor model by `doc_id` or a live Twin participant by `target_gid`; live API, direct typed, and actorless Rhai callers need a stable nonzero `producer_id` |
| `ConfirmClassPicker` | `{qualified?, cancel?}` | only if a picker modal opened in the GUI |

**Results & viz**
| command | params | returns / effect |
|---|---|---|
| `SnapshotVariables` | `{doc, names?}` | one-shot live `{t, parameters, inputs, variables}` |
| `ListRuns` | `{doc?}` | experiment rows (newest first) |
| `RunStatus` | `{experiment_id}` | one run's progress/terminal state |
| `GetExperimentResult` | `{experiment_id? \| doc, variables?, max_points?}` | full trajectory `{times, series}` |
| `CancelExperiment` / `DeleteExperiment` / `RenameExperiment` | see §5 | run lifecycle |
| `NewPlotPanel` / `AddSignalToPlot` | see §6 | plotting |
| `CaptureScreenshot` | `{}` | raw PNG bytes (save `-o`, then Read) |

## 8. Gotchas

- **Direction-to-joint controller**: write the coordinate contract before
  changing equations: world direction, inverse mount frame, joint axes/order,
  and the mesh's physical boresight. A compiling model or its own zero error is
  insufficient; inspect the live direction inputs, setpoints, measured joint
  angles, and rendered mechanism after a full scene reload.

- **Missing `params`** → silent no-op. Always send `"params":{}`.
- **Multi-class file** → `compile`/`run` need `class`. Without it, if >1
  non-package class the run aborts with `picker_pending` (the GUI would show a
  modal). Call `ListCompileCandidates` first, pass the short or qualified name.
- **Fire before parse** → `no compilable top-level class`. Poll `CompileStatus`
  until `ast_parsed:true` before compiling/running a just-opened doc.
- **`GetExperimentResult` errors** unless the run is `Done` (or `Failed` with a
  partial). Check `ListRuns` state first; a big sweep runs async.
- **Unified opening**: prefer `Open{uri}`. `OpenClass` resolves a Modelica
  class through the source-aware document/library path; `OpenFile` resolves a
  filesystem URI.
- **File lifetime**: native file opens pin canonical local/replicated root ownership
  before installation; browser mounted files read OPFS, while picker/private saves
  have Application lifetime. Closing the admitted owner retires file work even
  when the source remains in the editor registry. Dirty source cannot transfer
  owners through a path reopen; save or close it before a clean explicit reopen.
  Alias reopens resolve source and resident identities on the file worker. Their
  captured owner/generation must still match at install, so a late read cannot
  replace a concurrently changed or newly installed different-owner source.
- **Browser imports and saves**: shared picker results carry request-owned bytes
  into the existing asynchronous file-load pipeline; same-name picks create
  distinct pathless Application documents. Browser Save marks the document saved
  only after download admission succeeds. Verify cancellation/read failure,
  same-name overlapping reads and App teardown through the shared picker browser
  tests; see the [picker owner contract](../../docs/crates-index.md).
- **Portable Twin saves**: Save All and Save As Twin preflight generated filenames
  with the shared `lunco-assets-path` component validator before any save or manifest
  command. Reserved device names (even with `.mo`), final dots/spaces and forbidden
  characters reject visibly; no replacement filename hides invalid state. Rename
  uses lossless command paths and confines the actual source parent through Storage.
  Verify name algebra with the asset-path tests and source scope through the authored
  `modelica_save_all_scope.rhai` production gate; these prove different boundaries.
- **Complete result admission**: finite ordered times, matching series/metadata
  lengths, and finite values are required before a trajectory is marked Done.
  `experiments.result_limits` supplies the shared budgets; defaults are 8,000,000
  scalar values and 256 MiB artifact bytes. Failed partial streams may retain
  explicit missing-value holes. A malformed optional artifact warns without
  replacing a valid runtime result.
  Native and browser Fast Runs capture the same limits at admission. Actual
  lowered output dimensions and the requested grid are checked before batch
  trajectory allocation; event samples also consume the scoped recorder budget.
  Exercise `modelica_output_budget.rhai` with the documented default value cap
  and `max_parallel=1` to prove visible rejection and valid successor completion.
- **Durable batch history**: completed Twin-owned results persist asynchronously
  with their immutable definition and actual compiler source CID. Application
  runs have no Twin destination. Inspect `source_cid` and `restored_history` in
  `GetExperimentResult`; an archived result never claims the currently edited
  source or starts playback. Parsed-only contributing libraries without source
  bytes warn that persistence is unavailable while the run remains valid.
  Exercise `modelica_artifact_history.rhai` in fresh seed/restore sessions and
  verify the UUID artifact is durable before stopping the seed session. Seed
  creates a fresh Twin around its API-authored scene; restore opens the saved
  Twin through `OpenTwin`. Include
  a corrupt optional artifact and changed same-name source; history must retain
  its original CID and values. See the [artifact owner contract](../../docs/architecture/25-experiments.md#durable-completed-history).
- **File diagnostics**: `GetFile` acknowledges asynchronous read admission. It
  uses the same captured native/OPFS source path as document opens; inspect its
  log text or structured read-error diagnostic. Closing the admitted Twin or
  replicated lifetime retires the pending request before publication.
- **Live ≠ batch**: `SnapshotVariables` reads the *live* stepping model;
  `GetExperimentResult` reads a *stored batch run*. They are different objects.
- **Blank plot/diagram in `luncosim`** → the Modelica perspective
  isn't active. `ActivatePerspective{"id":"modelica_analyze"}` before capturing
  (§0). In `lunica` it's already the whole app. Commands/results don't need it —
  only the visible panels do.
- **Don't restart to "start clean"** — drive the API to add the state you need.
- **MCP fallback**: if the user insists on MCP, every command above maps to an
  `mcp__lunco__*` tool (`compile_model`, `run_scenario`→rhai only, `set_input`,
  `snapshot_variables`, `read_ports`, `describe_model`, `find_model`,
  `open_uri`, `list_bundled`, `list_open_documents`). Batch experiment verbs
  (`RunExperiment`/`ListRuns`/`GetExperimentResult`) have **no dedicated MCP
  tool** — use curl (or the generic `mcp__lunco__execute_command`).

### Omitted library inputs

Omitted inputs retain the owning Modelica library's defaults, including nested
components in generated USD networks. Explicit authored inputs take precedence.
The worker's initialized solver observation supplies runtime readback; do not
copy defaults into each Twin to compensate for a lifecycle bridge replacing
unbound slots with zero. Verify after advancing physics, not only at compile time.

Rumoca owns these defaults as qualified DAE input initialization expressions;
internal and connected bindings remain equation-owned. Verify class and instance
isolation, override, reset, batch execution, and invalid-default diagnostics with
`modelica_scoped_input_defaults.rhai` in an owned production session.

Run the focused production regression after building the normal `luncosim`:

```sh
LUNCOSIM_BIN=target/debug/luncosim LUNCOSIM_EPHEMERAL_SETTINGS=1 \
LUNCOSIM_ISOLATED_RUN=1 python3 scripts/api/run_editor_scene_test.py \
  --port 4261 --timeout 90 --scene scenes/fixtures/usd_query_api/site.usda \
  --scenario lunco://scenarios/tests/modelica_scoped_input_defaults.rhai \
  --log target/modelica-scoped-input-defaults.log
```

The fixture admits a Twin owner; the runner attaches the observer to that owner,
requires a real Rhai verdict, then verifies API Exit and port release.
