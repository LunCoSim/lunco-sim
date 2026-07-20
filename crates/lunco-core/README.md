# lunco-core

Core types, plugins, and diagram system for the LunCo simulation.

## What This Crate Does

- **Architecture primitives** — `Port`, `Wire`
- **Plugin system** — base plugins for simulation orchestration
- **Coordinate systems** — precision handling, spatial transforms
- **Diagram system** — `ComponentGraph`: canonical graph data for visualization

## Architecture

### Entity Viewer Pattern

All UI panels are **entity viewers** — they watch a selected entity and render its data. This crate provides the foundational types that domain crates build on.

```
lunco-core/src
  ├── architecture.rs    — Port, Wire
  ├── diagram.rs         — ComponentGraph (canonical graph data)
  ├── telemetry.rs       — TelemetryEvent capture
  ├── log.rs             — Simulation logging
  ├── commands.rs        — Command envelope + `Command` trait / `register_commands` / `on_command` macros
  ├── coords.rs          — DVec3 helpers over the big_space hierarchy
  ├── world.rs           — the world shell: persistent big_space root every scene mounts into
  ├── markers.rs         — architectural marker components for big_space
  ├── attach.rs          — atomic re-parenting of GridAnchor entities across Grids
  ├── invariants.rs      — debug-build runtime checks for big_space invariants
  ├── ids.rs             — shared 53-bit time-sorted id generator (OpId, GlobalEntityId)
  ├── identity.rs        — deterministic network identity from provenance (M1)
  ├── session.rs         — networking authority substrate (SessionId, roles) — no wire dep
  ├── reconcile.rs       — predict-own reconciliation decision (input-replay)
  └── mocks.rs           — test mocks
```

### ComponentGraph

The canonical graph representation for all diagram visualization across the project:

```
ComponentGraph (pure Rust, no Bevy dependency)
  ├── Nodes with typed ports (named, typed connection points)
  ├── Edges with semantic kinds (Connect, Wire, Signal, Extends, etc.)
  └── Convertible to: lunco-canvas (rendering), petgraph (analysis)
```

Built by domain-specific builders:
- `ModelicaComponentBuilder` — Modelica AST → ComponentGraph
- `WireGraphBuilder` (planned) — ECS ports/wires → ComponentGraph
- `FswGraphBuilder` (planned) — FSW architecture → ComponentGraph

Ontology alignment: every `ComponentGraph` concept maps to SysML v2 terms from the engineering ontology in `docs/architecture/01-ontology.md`.

| ComponentGraph | SysML v2 | Modelica |
|---------------|----------|----------|
| `ComponentPort` | Proxy Port | Connector |
| `EdgeKind::Wire` | Connection | `connect()` |
| `NodeKind::Component` | Part | Model/Block |
| `NodeKind::Subsystem` | Part (composite) | Package |

## See Also

- [Workspace UI/UX Research](../../docs/architecture/research/ui-ux-inspiration.md) — architecture decisions
- [Engineering Ontology](../../docs/architecture/01-ontology.md) — engineering terminology source of truth
