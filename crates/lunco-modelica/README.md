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
  (snarl as     (text editor  (params, inputs,
   view over     with debounced  live variables)
   document)     commit → doc)
```

`WorkbenchState.selected_entity` is the selection bridge — any
context (library browser, 3D viewport click, colony tree) can set
it to open the Modelica editor for any entity.

### Diagram panel

The diagram panel is an **egui-snarl view over the document**:

- On every frame, if `doc.generation()` advanced past `last_seen_gen`,
  rebuild snarl from the cached AST (synchronous — sub-millisecond).
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
| Diagram | `modelica_diagram_preview` | Center tab | Component block diagram (egui-snarl) |
| Telemetry | `modelica_inspector` | Right dock | Parameters, inputs, variable toggles |
| Graphs | `modelica_console` | Bottom dock | Time-series plots |

Users can drag, split, tab, and float panels freely. Layout persists via `bevy_workbench` persistence.

## Binaries

| Binary | Target | Description |
|--------|--------|-------------|
| `modelica_workbench` | Desktop | Full Modelica workbench with all panels |
| `modelica_workbench_web` | wasm32 | Web version (inline worker, no threads) |
| `modelica_tester` | CLI | Standalone tester for Modelica compilation |

## Key Dependencies

- `rumoca-session`, `rumoca-phase-parse` — Modelica compilation (LunCoSim/rumoca fork)
- `bevy_workbench` — docking, persistence, panel system
- `egui-snarl` — interactive node graph rendering
- `egui_plot` — time-series charts

## See Also

- [**Modelica Domain Architecture**](../../docs/architecture/20-domain-modelica.md) — full design doc: document model, op set, pretty-printer, name resolution (MLS §5.3), diagram ↔ code sync
- [Document System Foundation](../../docs/architecture/10-document-system.md) — shared `Document` / `DocumentOp` / `DocumentHost` trait layer
- [Workspace UI/UX Research](../../docs/research-ui-ux-architecture.md) — architecture decisions
- [Plan: Switch to Parser](../../docs/plan-switch-to-parser.md) — regex → AST migration
- [Modelica Language Specification §5.3](https://specification.modelica.org/maint/3.7/class-predefined-types-and-declarations.html#static-name-lookup) — the static name lookup rules our type resolver follows
- [Modelica Language Specification §18](https://specification.modelica.org/maint/3.7/annotations.html) — `Placement`, `Line`, `Icon` annotation shapes
