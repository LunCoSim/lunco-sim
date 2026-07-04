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
workbench. The API server only exists when you pass `--api`. Default port is
**4101** (`lunco_core::session::DEFAULT_API_PORT`).

| App | Launch | Modelica surface |
|---|---|---|
| **`lunica`** | `cargo run --bin lunica -- --api 4101` | **The Modelica workbench itself** — nothing to switch to. Prefer this for pure Modelica work. |
| **`sandbox`** | `cargo run --release -p lunco-sandbox --bin sandbox -- --api 4101` | Physics test bed; Modelica lives under the **`modelica_analyze` perspective** — switch to it (below) before diagrams/plots render. |
| **`luncosim`** | `cargo run -p luncosim -- --api 4101` | Flagship sim; same — Modelica under `modelica_analyze`. |

**In `sandbox`/`luncosim`, switch to the Modelica view before plotting/screenshotting.**
The compile/run/experiment *commands and query providers work regardless* (they're
headless-safe), but the diagram/plot panels only paint when their perspective is
active. Switch with:

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands -H "Content-Type: application/json" \
  -d '{"command":"ActivatePerspective","params":{"id":"modelica_analyze"}}'
# other ids: "sandbox_view", "rover_build". Reset a broken layout: {"command":"ResetWorkspaceLayout","params":{}}
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
  -d '{"command":"Ping","params":{}}'; do sleep 1; done
```

Stop with the `Exit` command (never `pkill`/`kill` — those need user confirm):

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" -d '{"command":"Exit","params":{}}'
```

## 1. The request envelope

Everything is one endpoint: `POST /api/commands`. The JSON shape is always
`{"command":"<Name>","params":{...}}`. **Always include `params` even when
empty** (`"params":{}`) — without it the command silently no-ops with a
`invalid type: null` deserialization error.

```bash
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"command":"<Name>","params":{ ... }}'
```

Two kinds of `command` share this envelope:

- **Commands** (fire-and-forget mutations): return `{"command_id": N}`. Errors
  log server-side; the HTTP call still returns a command_id.
- **Query providers** (return data): return the payload directly, e.g.
  `{"runs":[...]}`. `ListRuns`, `GetExperimentResult`, `DescribeModel`,
  `SnapshotVariables`, `CompileStatus`, `ListCompileCandidates`,
  `ListBundled`, `ListOpenDocuments`, `FindModel` are all query providers —
  invoked with the **same `{"command":...}` form**, NOT the `{"type":...}`
  form. (`{"type":...}` is only for the built-in `ListEntities` /
  `DiscoverSchema` / `QueryEntity` meta-queries.)

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
post '{"command":"Open","params":{"uri":"bundled://SpringMass.mo"}}'
#    bundled://Name.mo | Modelica.Blocks.Examples.PID_Controller | /abs/path.mo | mem://Untitled
#    List embedded examples first: {"command":"ListBundled","params":{}}

# 2. Wait for the AST parse (background). Poll CompileStatus until ast_parsed:true:
post '{"command":"CompileStatus","params":{"doc":0}}'   # -> {state, ast_parsed, candidates, picker_pending, ...}

# 3. Compile + play. class REQUIRED if the file has >1 non-package class
#    (the GUI picker can't be shown over the API). Discover choices:
post '{"command":"ListCompileCandidates","params":{"doc":0}}'   # -> {candidates:[{qualified,short}]}
post '{"command":"RunActiveModel","params":{"doc":0,"class":"SpringMass"}}'

# 4. Read live values (t + parameters + inputs + variables). Filter with names:
post '{"command":"SnapshotVariables","params":{"doc":0,"names":["x","v"]}}'

# 5. Poke a runtime input live (no recompile, applies next step):
post '{"command":"SetModelInput","params":{"doc":0,"name":"F","value":10.0}}'

# 6. Pause / Resume / Reset / Restart:
post '{"command":"PauseActiveModel","params":{"doc":0}}'
post '{"command":"RestartActiveModel","params":{"doc":0}}'   # reset t=0 then run
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
post '{"command":"RunExperiment","params":{
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
  post "{\"command\":\"RunExperiment\",\"params\":{\"doc\":0,\"class\":\"RocketStage\",
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
post '{"command":"ListRuns","params":{}}'

# Pull a full trajectory: times + series (dotted Modelica path -> samples).
# Target by experiment_id, OR by doc (its latest run). Filter + downsample:
post '{"command":"GetExperimentResult","params":{
  "doc":0, "variables":["altitude","velocity"], "max_points":500
}}'
# max_points = strided downsample, final sample always kept. Omit = uncapped.
# Returns {state:"Done", times:[...], series:{"altitude":[...], ...}} or an
# error if the run is not Done (Pending/Running/Failed-without-partial).
```

Cancel / clean up:
```bash
post '{"command":"CancelExperiment","params":{"all":true}}'           # or {"experiment_id":"<uuid>"}
post '{"command":"DeleteExperiment","params":{"all":true}}'           # terminal runs only
post '{"command":"RenameExperiment","params":{"experiment_id":"<uuid>","name":"baseline"}}'
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
post '{"command":"NewPlotPanel","params":{"title":"Ascent","signals":["altitude","velocity"],"source":0}}'

# Add another signal to an existing plot (plot=0 = the default graph):
post '{"command":"AddSignalToPlot","params":{"plot":0,"signal":"mass"}}'
```

`signals` in `NewPlotPanel` become the plot's **picked variables**; every
completed run then contributes those series. Run one sweep (§4), open the plot
once with the variables you care about, and each new run lands on the same axes.

### Typical end-to-end: sweep → compare
```bash
# 1. sweep 4 runs (see §4 loop) with labels Isp=280..340
# 2. open the comparison plot on the variable of interest
post '{"command":"NewPlotPanel","params":{"title":"Isp sweep","signals":["altitude"],"source":0}}'
# 3. confirm the runs landed, then screenshot for the human
post '{"command":"ListRuns","params":{}}'
curl -s -X POST $API -H "Content-Type: application/json" \
  -d '{"command":"CaptureScreenshot","params":{}}' -o /tmp/sweep.png   # then Read the PNG
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

- **Missing `params`** → silent no-op. Always send `"params":{}`.
- **Multi-class file** → `compile`/`run` need `class`. Without it, if >1
  non-package class the run aborts with `picker_pending` (the GUI would show a
  modal). Call `ListCompileCandidates` first, pass the short or qualified name.
- **Fire before parse** → `no compilable top-level class`. Poll `CompileStatus`
  until `ast_parsed:true` before compiling/running a just-opened doc.
- **`GetExperimentResult` errors** unless the run is `Done` (or `Failed` with a
  partial). Check `ListRuns` state first; a big sweep runs async.
- **`Open` vs old verbs**: prefer `Open{uri}`. `OpenClass` is MSL-only;
  `OpenFile` is filesystem-only.
- **Live ≠ batch**: `SnapshotVariables` reads the *live* stepping model;
  `GetExperimentResult` reads a *stored batch run*. They are different objects.
- **Blank plot/diagram in `sandbox`/`luncosim`** → the Modelica perspective
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
