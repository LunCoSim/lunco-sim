# lunco-core

Core types, plugins, and diagram system for the LunCo simulation.

## What This Crate Does

- **Architecture primitives** — `DigitalPort`, `PhysicalPort`, `Wire`, `CommandMessage`, `CommandResponse`
- **Plugin system** — base plugins for simulation orchestration
- **Coordinate systems** — precision handling, spatial transforms
- **Diagram system** — `ComponentGraph`: canonical graph data for visualization

## Architecture

### Entity Viewer Pattern

All UI panels are **entity viewers** — they watch a selected entity and render its data. This crate provides the foundational types that domain crates build on.

```
lunco-core
  ├── architecture.rs    — DigitalPort, PhysicalPort, Wire, CommandMessage
  ├── diagram.rs         — ComponentGraph (canonical graph data)
  ├── telemetry.rs       — TelemetryEvent capture
  └── log.rs             — Simulation logging
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

Ontology alignment: every `ComponentGraph` concept maps to SysML v2 terms from the engineering ontology.

| ComponentGraph | SysML v2 | Modelica |
|---------------|----------|----------|
| `ComponentPort` | Proxy Port | Connector |
| `EdgeKind::Wire` | Connection | `connect()` |
| `NodeKind::Component` | Part | Model/Block |
| `NodeKind::Subsystem` | Part (composite) | Package |

## See Also

- [UI/UX Research](../../docs/architecture/research/ui-ux-inspiration.md) — architecture decisions
- [01-ontology.md](../../docs/architecture/01-ontology.md) — engineering terminology source of truth
