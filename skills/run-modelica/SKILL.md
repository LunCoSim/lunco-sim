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
**4101** (`lunco_core::session::DEFAULT_API_PORT`).

| App | Launch | Modelica surface |
|---|---|---|
| **`lunica`** | `target/debug/lunica --api 4101` | **The Modelica workbench itself** — nothing to switch to. Prefer this for pure Modelica work. |
| **`luncosim`** | `target/debug/luncosim --api 4101` | Ground-physics simulator; Modelica lives under the **`modelica_analyze` perspective** — switch to it (below) before diagrams/plots render. |
| **`luncosim-server`** | `target/debug/luncosim-server --api 4101` | Headless LunCoSim host; use the GUI `luncosim` command for the workbench. |

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
- **If an app is already running on 4101, do NOT start another and do NOT
  `Exit` it** — reuse it. Killing it destroys the user's open tabs/state. Only
  restart when the user says so or the binary is verifiably stale after a rebuild.

### No-API alternative: the `modelica_run` CLI
For a one-shot compile→step→CSV with **no server at all** (CI, quick numeric
check), skip the API entirely:

```bash
cargo run -p lunco-modelica --bin modelica_run -- \
  assets/models/AnnotatedRocketStage.mo AnnotatedRocketStage.RocketStage \
  --duration 30 --dt 0.001 --input valve_command=0.7 \
  --record altitude,velocity --output /tmp/run.csv
```
Fixed-step only, one run, no sweeps/plots. For parameter sweeps, comparison, or
live interaction use the API (§4–6).

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

For a multi-domain run, use the shared `AttachProgram { doc, spec }` command
instead of writing marker components or maintaining a separate binding table.
The spec authors the `LunCoProgramAPI` child, explicit scalar inputs and
outputs, and native USD connections in one journaled change set. In Rhai the
same surface is `attach_program(...)` with `program_input_connection(...)`,
`program_input_default(...)`, and `program_output(...)` helpers. Verify the
result with `ListPorts`, `CosimStatus`, and `GetBrokenConnections`; a source
without declared ports is reported as source-only and does not step.

Modelica packages under `assets/models/<Root>/` use the standard
`package.mo`/`package.order` layout and members' `within` declarations. A
qualified reference such as `LunCo.Electrical.Battery` is resolved by its root
segment through the normal Modelica search-path inventory; do not add a
library-specific Rust load call. The compiler loads a cold structured root on
the unresolved-reference path and retries. A generated policy's `source_roots`
metadata can prewarm a dependency, but it is not the source of truth for class
discovery.

For policy-owned generated models, keep contract assertions in
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

Node movement has two valid outcomes. On an editable `.mo` document, drag a
component and verify that the standard `annotation(Placement(...))` changes in
the source and survives a re-projection. On a generated document, the canvas
is intentionally read-only because USD plus the Rhai policy owns the source;
use `Duplicate to edit`, then perform the same placement check. A drag that
appears to work but disappears on reload is a product bug, not an acceptable
generated-model editing mode.

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
  Invalid parameters return HTTP 422; deferred commands return their completed
  result on the same request.
- **Query providers** (return data): return the payload directly, e.g.
  `{"runs":[...]}`. `ListRuns`, `GetExperimentResult`, `DescribeModel`,
  `SnapshotVariables`, `CompileStatus`, `ListCompileCandidates`,
  `ListBundled`, `ListOpenDocuments`, `FindModel` are all query providers —
  invoked with the same tagged `ExecuteCommand` form. Built-in discovery and
  entity listing use their own explicit `type` values.

`doc: 0` always means "the active document/tab".

## 2. Two run modes — pick the right one

| | **Interactive (live)** | **Batch (Fast Run / Experiment)** |
|---|---|---|
| Verb | `RunActiveModel` | `FastRunActiveModel` / `RunExperiment` |
| Pace | wall-clock realtime, steps forever | as fast as possible, `t_start→t_end`, then stops |
| Use for | inspection, physics-in-loop, 3D viz, possession | parameter sweeps, regression, "what if I bump this constant?" |
| Read results | `SnapshotVariables` (live), `ReadPorts`/`WatchPorts` | `GetExperimentResult` (full trajectory) |
| Poke inputs | `SetModelInput` (takes effect next step) | overrides baked into the run request |
| Stored as | live stepping model | first-class `Experiment` in the registry (plot/compare) |

## 3. Recipe A — run a model live (interactive)

```bash
API=http://127.0.0.1:4101/api/commands
post(){ curl -s -X POST $API -H "Content-Type: application/json" -d "$1"; }

# 1. Open a model. Prefer the unified opener (bundled example / MSL name / path):
post '{"type":"ExecuteCommand","command":"Open","params":{"uri":"bundled://SpringMass.mo"}}'
#    bundled://Name.mo | Modelica.Blocks.Examples.PID_Controller | /abs/path.mo | mem://Untitled
#    List embedded examples first: {"type":"ExecuteCommand","command":"ListBundled","params":{}}

# 2. Wait for the AST parse (background). Poll CompileStatus until ast_parsed:true:
post '{"type":"ExecuteCommand","command":"CompileStatus","params":{"doc":0}}'   # -> {state, ast_parsed, candidates, picker_pending, ...}

# 3. Compile + play. class REQUIRED if the file has >1 non-package class
#    (the GUI picker can't be shown over the API). Discover choices:
post '{"type":"ExecuteCommand","command":"ListCompileCandidates","params":{"doc":0}}'   # -> {candidates:[{qualified,short}]}
post '{"type":"ExecuteCommand","command":"RunActiveModel","params":{"doc":0,"class":"SpringMass"}}'

# 4. Read live values (t + parameters + inputs + variables). Filter with names:
post '{"type":"ExecuteCommand","command":"SnapshotVariables","params":{"doc":0,"names":["x","v"]}}'

# 5. Poke a runtime input live (no recompile, applies next step):
post '{"type":"ExecuteCommand","command":"SetModelInput","params":{"doc":0,"name":"F","value":10.0}}'

# 6. Pause / Resume / Reset / Restart:
post '{"type":"ExecuteCommand","command":"PauseActiveModel","params":{"doc":0}}'
post '{"type":"ExecuteCommand","command":"RestartActiveModel","params":{"doc":0}}'   # reset t=0 then run
```

`RunActiveModel` = compile-if-stale then play. If already compiled & clean it
just unpauses (no recompile). `CompileModel` compiles only (stays paused);
`ResumeActiveModel` unpauses only.

## 4. Recipe B — build an experiment (batch + parameter sweep)

`RunExperiment` is the agent-facing sweep verb: overrides come from the
**command**, not the UI, so you can sweep parameters without touching source.
Each run is stored as an `Experiment`; read its trajectory back with
`GetExperimentResult`.

```bash
# One run with a parameter override + custom bounds + a label:
post '{"type":"ExecuteCommand","command":"RunExperiment","params":{
  "doc":0, "class":"RocketStage",
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
  post "{\"type\":\"ExecuteCommand\",\"command\":\"RunExperiment\",\"params\":{\"doc\":0,\"class\":\"RocketStage\",
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

`FastRunActiveModel` is the same batch engine but reads bounds from the UI
"Simulation Setup" draft instead of the command — prefer `RunExperiment` for
scripted/agent runs so everything is explicit.

## 5. Recipe C — read experiment results

```bash
# List runs (newest first). Optional {"doc":N} filter. Each row is self-describing:
# experiment_id, name, state (Pending|Queued|Running|Done|Failed|Cancelled),
# wall_time_ms, the overrides that produced it, and the bounds it ran under.
post '{"type":"ExecuteCommand","command":"ListRuns","params":{}}'

# Pull a full trajectory: times + series (dotted Modelica path -> samples).
# Target by experiment_id, OR by doc (its latest run). Filter + downsample:
post '{"type":"ExecuteCommand","command":"GetExperimentResult","params":{
  "doc":0, "variables":["altitude","velocity"], "max_points":500
}}'
# max_points = strided downsample, final sample always kept. Omit = uncapped.
# Returns {state:"Done", times:[...], series:{"altitude":[...], ...}} or an
# error if the run is not Done (Pending/Running/Failed-without-partial).
```

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
| `ListBundled` | `{}` | embedded example models (`bundled://` URIs) |
| `FindModel` | `{query, limit?}` | fuzzy search examples/Twin/MSL/open docs → URIs |
| `Open` | `{uri}` | open bundled/MSL/path/mem into a tab |
| `ListOpenDocuments` | `{}` | `doc_id, title, kind, origin, active` per tab |
| `DescribeModel` | `{doc, class?}` | AST: components, connections, inputs, parameters, outputs (pre-compile) |
| `CompileStatus` | `{doc}` | `state, ast_parsed, candidates, picker_pending, drilled_in_class` |
| `ListCompileCandidates` | `{doc}` | `{candidates:[{qualified,short}]}` — the picker choices |

**Compile & run**
| command | params | effect |
|---|---|---|
| `CompileModel` | `{doc, class?, force?, resume_after_compile?}` | compile only (stays paused) |
| `RunActiveModel` | `{doc, class?}` | compile-if-stale + play (live) |
| `PauseActiveModel` / `ResumeActiveModel` / `ResetActiveModel` | `{doc}` | live stepping control |
| `RestartActiveModel` | `{doc}` | reset t=0 then run |
| `FastRunActiveModel` | `{doc, class?, t_end?, dt?, n_intervals?, tolerance?, solver?, h0?}` | batch, bounds from UI draft |
| `RunExperiment` | `{doc, class?, overrides[], inputs[], t_start?, t_end?, dt?, n_intervals?, tolerance?, solver?, h0?, label?}` | batch sweep, overrides from command |
| `SetModelInput` | `{doc, name, value}` | push live input value |
| `ConfirmClassPicker` | `{qualified?, cancel?}` | only if a picker modal opened in the GUI |

**Results & viz**
| command | params | returns / effect |
|---|---|---|
| `SnapshotVariables` | `{doc, names?}` | one-shot live `{t, parameters, inputs, variables}` |
| `ListRuns` | `{doc?}` | experiment rows (newest first) |
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
- **`Open` vs old verbs**: prefer `Open{uri}`. `OpenClass` resolves a
  Modelica class through the source-aware document/library path; `OpenFile`
  is filesystem-only.
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
