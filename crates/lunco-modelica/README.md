# lunco-modelica

Modelica simulation integration for LunCoSim using Rumoca.

## What This Crate Does

- **Modelica compilation** — parses and compiles `.mo` files via `rumoca-session`
- **Simulation execution** — runs Modelica models as `SimStepper` instances
- **Workbench UI** — code editor, component diagrams, parameter tuning, time-series plots
- **AST-based extraction** — full Modelica AST parsing (no regex) for symbols, components, connections

## Architecture

### Entity Viewer Pattern

All UI panels watch a `ModelicaModel` entity and render its data. They don't know if they're in a standalone workbench, a 3D overlay, or a mission dashboard.

```
                    ModelicaModel entity
                    (attached to 3D objects
                     or standalone workbench)
                              │
           ┌──────────────────┼──────────────────┐
           ▼                  ▼                  ▼
     DiagramPanel      CodeEditorPanel    TelemetryPanel
     (egui-snarl)      (text editor)      (params/inputs)
```

`WorkbenchState.selected_entity` is the **selection bridge** — any context (library browser, 3D viewport click, colony tree) can set it to open the Modelica editor for any entity.

### Diagram System

```
rumoca-phase-parse (AST)
        │
        ▼
ast_extract.rs (symbol extraction — no regex)
        │
        ▼
ModelicaComponentBuilder (AST → ComponentGraph)
        │
        ▼
ComponentGraph (canonical graph data in lunco-core)
        │
        ▼
Snarl<ModelicaNode> (egui-snarl rendering in DiagramPanel)
```

**Diagram types:**
- **Block Diagram** — components as nodes, `connect()` as edges
- **Connection Diagram** — connector instances expanded as separate nodes
- **Package Hierarchy** — packages as subsystem nodes with containment edges

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

- [Workspace UI/UX Research](../../docs/research-ui-ux-architecture.md) — architecture decisions
- [Plan: Switch to Parser](../../docs/plan-switch-to-parser.md) — regex → AST migration
