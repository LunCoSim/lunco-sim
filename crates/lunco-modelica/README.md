# lunco-modelica

Modelica simulation integration for LunCoSim using Rumoca.

## What This Crate Does

- **Modelica compilation** — parses and compiles `.mo` files via `rumoca-session`
- **Simulation execution** — runs Modelica models as `SimStepper` instances
- **Workbench UI** — code editor, component diagrams, parameter tuning, time-series plots
- **AST-based editing** — a `ModelicaDocument` whose source is canonical and whose AST is cached + refreshed per op; every editing action (diagram, code editor, parameter inspector) funnels through a typed `ModelicaOp` and a single span-based apply pipeline

> Full architecture (document model, op set, apply pipeline, name
> resolution, diagram ↔ code sync) lives in
> [**`docs/architecture/20-domain-modelica.md`**](../../docs/architecture/20-domain-modelica.md).

## Compile / Run lifecycle

Compiling a model **never** auto-starts a live realtime sim. The
per-doc run-state is a small machine over `ModelicaModel`:

```
Uncompiled/Stale ──[Compile]──▶ Ready (paused) ──[Run]──▶ Running
                                      ▲                      │
                                      └────────[Pause]───────┘
Compile error ─────────────────▶ Blocked (paused)
```

Key rules:

- **Compile never auto-starts a live sim.** A successful compile leaves
  the model paused/ready; you start stepping explicitly with Run.
- **Run = compile-if-stale, then play.** If the model is already
  compiled and clean it just unpauses (no recompile); otherwise it
  compiles and resumes on success.
- **Compiled + clean is never recompiled.** `CompileModel` is idempotent
  — it skips the worker dispatch when `is_compiled && !stale &&
  !is_compiling`. Pass `force: true` to override. Staleness is
  `!is_compiled || compiled_generation != document.generation`.
- **Fast Run is orthogonal.** It runs a batch experiment off-thread and
  never touches the live run-state.

| Verb | Effect |
|---|---|
| `CompileModel` / `CompileActiveModel` | Compile only, idempotent (skip if compiled & clean unless `force`). Never plays |
| `RunActiveModel` | Compile-if-stale, then play |
| `ResumeActiveModel` | Unpause (no compile) |
| `PauseActiveModel` | Pause |
| `ResetActiveModel` | Reset `t → 0` |
| `RestartActiveModel` | Reset + Run |
| `FastRunActiveModel` | Batch run → Experiment (orthogonal, unchanged) |

## Architecture at a glance

### Document as source of truth

`ModelicaDocument` owns:

- **`source: String`** — canonical text (lossless round-trip of comments + formatting)
- **`ast: Arc<AstCache>`** — parsed AST, refreshed eagerly after every mutation
- **`changes: VecDeque<(u64, ModelicaChange)>`** — structured change ring buffer for consumer polling

Op set: `ReplaceSource`, `EditText`, `AddComponent`,
`RemoveComponent`, `AddConnection`, `RemoveConnection`,
`SetPlacement`, `SetParameter`. Every variant — even the structural
ones — is applied as a span-located text patch, so comments and
formatting outside the edited range stay intact.

See [`src/document.rs`](src/document.rs) for the full op surface and
[`src/pretty.rs`](src/pretty.rs) for the subset pretty-printer used
when emitting new nodes.

### Entity viewer pattern

All UI panels watch a `ModelicaModel` entity (which points at a
document via `DocumentId`) and render from the shared document:

```
              ModelicaDocument  ◀─── AST ops from any panel
                   │                 (diagram, inspector, code edit)
       ┌───────────┼──────────────┐
       ▼           ▼              ▼
  DiagramPanel  CodeEditor    InspectorPanel
  (canvas       (text editor  (params, inputs,
   view over     with debounced  live variables)
   document)     commit → doc)
```

`WorkbenchState.selected_entity` is the selection bridge — any
context (library browser, 3D viewport click, colony tree) can set
it to open the Modelica editor for any entity.

### Diagram panel

The diagram panel is a **`lunco-canvas` view over the document**:

- On every frame, if `doc.generation()` advanced past `last_seen_gen`,
  rebuild the canvas scene from the cached AST (synchronous — sub-millisecond).
- User actions (drag from palette, draw wire, drag to move, right-click
  delete) emit AST ops. The outer render loop drains them and applies
  to the document.
- Type references in rebuilt source are resolved via MLS §5.3 rules —
  fully-qualified path or import-based scope lookup — see
  [architecture § 5.6](../../docs/architecture/20-domain-modelica.md#56-type-resolution-mls-53).

### Code editor

Text-backed editor with IDE-standard **debounced commit**:

- Per-keystroke → local buffer only
- ~350 ms idle (or focus-loss) → `ReplaceSource` to the document
- Diagram panel sees the generation bump on its next frame → rebuild

Word-wrap is toggleable at the top of the panel (default off — long
lines scroll horizontally, matching VS Code's default).

### Panel Layout

| Panel | ID Pattern | Position | Purpose |
|-------|-----------|----------|---------|
| Library Browser | `library_browser` | Left dock | File navigation, drag `.mo` files |
| Code Editor | `modelica_preview` | Center tab | Source code editing, compile & run |
| Diagram | `modelica_diagram_preview` | Center tab | Component block diagram (`lunco-canvas`) |
| Telemetry | `modelica_inspector` | Right dock | Parameters, inputs, variable toggles |
| Graphs | `modelica_console` | Bottom dock | Time-series plots |

Users can drag, split, tab, and float panels freely. Layout persists via `bevy_workbench` persistence.

## Binaries

| Binary | Target | Description |
|--------|--------|-------------|
| `lunica` | Desktop | Full Modelica workbench with all panels |
| `lunica` | wasm32 | Web version (inline worker, no threads) |
| `modelica_tester` | CLI | Standalone tester for Modelica compilation |
| `msl_indexer` | CLI | Build `msl_index.json`; with `--warm` also full-compiles a list of models so rumoca's semantic-summary cache is hot before the workbench opens |
| `modelica_run` | CLI | Headless: compile a `.mo`, step it for a fixed duration, optionally dump per-step CSV |

### CLI workflow — warm cache, then run headless

The two CLI binaries compose:

```bash
# 1. (one-time per cache wipe) Warm the rumoca semantic-summary cache for
#    every bundled asset model + a default list of common MSL examples.
#    Takes ~7 min cold, ~30s if the parse cache from a prior run is intact.
LUNCOSIM_WARM_DIRS="$(pwd)/assets/models" \
  cargo run --release --bin msl_indexer -- --warm

# 2. Run AnnotatedRocketStage.RocketStage for 10s, dump per-step telemetry
#    to CSV. After the warm pass above, compile is ~ms instead of minutes.
cargo run --release --bin modelica_run -- \
    assets/models/AnnotatedRocketStage.mo \
    AnnotatedRocketStage.RocketStage \
    --duration 10 \
    --input valve.opening=1.0 \
    --record time,engine.thrust,airframe.altitude,airframe.velocity,tank.m \
    --output /tmp/rocket.csv
```

Both binaries share the same compile path the workbench uses, so the
warm cache benefits all three. `modelica_run` prints 1-second progress
ticks (sim-time, RTF, ETA) and a 5-second compile heartbeat — there's
no silent stall regardless of model size.

`msl_indexer` flags:
- `--warm` — full-compile a default list of common MSL examples after indexing
- `--warm-only NAME[,NAME…]` — explicit list (mix of MSL qualified names and `.mo` paths)
- `LUNCOSIM_WARM_DIRS=path1:path2` — env var, scans each dir for `.mo` files and warms every top-level model
- `-v, --verbose` — per-file scan logging

`modelica_run` flags:
- `<FILE.mo> <CLASS>` — required positional args
- `-d, --duration SECS` (default 10), `-t, --dt SECS` (default 0.01)
- `--output PATH` — write per-step CSV
- `--input N=V` — set runtime input (repeatable; warns on unknown name)
- `--record VAR,VAR` — comma-separated subset (default: all observables)
- `-v, --verbose` — per-step logging (otherwise 1-second wall-clock ticks)

## Key Dependencies

- `rumoca-session`, `rumoca-phase-parse` — Modelica compilation (LunCoSim/rumoca fork)
- `bevy_workbench` — docking, persistence, panel system
- `lunco-canvas` — interactive diagram rendering substrate
- `egui_plot` — time-series charts

## See Also

- [**Modelica Domain Architecture**](../../docs/architecture/20-domain-modelica.md) — full design doc: document model, op set, pretty-printer, name resolution (MLS §5.3), diagram ↔ code sync
- [Document System Foundation](../../docs/architecture/10-document-system.md) — shared `Document` / `DocumentOp` / `DocumentHost` trait layer
- [Workspace UI/UX Research](../../docs/research-ui-ux-architecture.md) — architecture decisions
- [Plan: Switch to Parser](../../docs/plan-switch-to-parser.md) — regex → AST migration
- [Modelica Language Specification §5.3](https://specification.modelica.org/maint/3.7/class-predefined-types-and-declarations.html#static-name-lookup) — the static name lookup rules our type resolver follows
- [Modelica Language Specification §18](https://specification.modelica.org/maint/3.7/annotations.html) — `Placement`, `Line`, `Icon` annotation shapes
