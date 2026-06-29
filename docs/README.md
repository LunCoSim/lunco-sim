# LunCoSim Documentation

Welcome. This directory is the authoritative home for LunCoSim architecture,
design, and reference documentation.

## Application Guide

Documentation for each primary binary and tool in the workspace.

| App | Purpose |
|---|---|
| [**Sandbox**](apps/sandbox/README.md) | Ground mobility and physics testing |
| [**Lunica**](apps/lunica/README.md) | Modelica engineering workbench |
| [**Assets Manager**](apps/assets-manager/README.md) | Download and process workspace assets |
| [**Model Viewer**](apps/model-viewer/README.md) | Minimal USD model inspection |

## Strategic Roadmap

We are evolving from a high-fidelity sandbox into a complete autonomous mission design stack.

| Milestone | Status | Description |
|---|---|---|
| **Space Robotics Core** | ✅ Foundation | Multi-domain co-simulation (USD + Modelica + Avian3D) with f64 precision. |
| **Real-world Validation** | 📝 Planned | [**HIL/SIL Integration**](../specs/027-hil-sil-integration/) for Hardware-in-the-loop validation. |
| **Industrial Interop** | 📝 Planned | [**NASA GMAT**](../specs/022-fmu-gmat-integration/) for orbital mechanics and **ROS2** for robotics control. |
| **Advanced Physics** | 📝 Planned | [**PINN Terramechanics**](../specs/025-terramechanics/) for high-fidelity soil interaction. |
| **Autonomous Missions** | 📝 Planned | [**Agent-Driven Sim**](../specs/033-agent-driven-simulation/) and [**Mission Replay/Audit**](../specs/020-world-state-and-replay/). |

## Architecture & Framework

| Path | Purpose |
|------|---------|
| [`principles.md`](principles.md) | Non-negotiable project principles (TDD, plugin-first, etc.) |
| [`crates-index.md`](crates-index.md) | Navigation guide for the workspace structure |
| [`scripting-guide.md`](scripting-guide.md) | How to write rhai scenarios — lifecycle, verbs, sequencing, tools, persistence |
| [`rhai-integration-design.md`](rhai-integration-design.md) | Rhai scripting design rationale + as-built reference |
| [`architecture/`](architecture/) | Design narrative — how LunCoSim is structured |
| [`architecture/01-ontology.md`](architecture/01-ontology.md) | Terminology reference — Space System, Port, Connection, Attribute |
| `../specs/` | Detailed feature specifications (contracts for implementation) |
| `../crates/<crate>/README.md` | Per-crate quick-start (use this when you want to use a crate) |
| [`../scripts/perf/README.md`](../scripts/perf/README.md) | Performance profiling subsystem |
| [`architecture/research/`](architecture/research/) | Historical analysis, inspiration, rejected paths |

## Reading order for newcomers

1. **[`architecture/00-overview.md`](architecture/00-overview.md)** — what LunCoSim is, the three-tier model, crate layers
2. **[`principles.md`](principles.md)** — how we work (TDD, plugin-first, interop, documentation mandate)
3. **[`architecture/01-ontology.md`](architecture/01-ontology.md)** — vocabulary (Space System, Port, Connection, CommandMessage, etc.)
4. **[`architecture/10-document-system.md`](architecture/10-document-system.md)** — the foundational data model: Documents, DocumentOps, DocumentViews
5. **[`architecture/11-workbench.md`](architecture/11-workbench.md)** — UI/UX architecture: workspaces, panels, command palette
6. **[`architecture/12-api.md`](architecture/12-api.md)** — transport-agnostic API layer, typed commands, and queries
7. **[`architecture/13-twin-and-workflow.md`](architecture/13-twin-and-workflow.md)** — what a Twin is, save/load/workflow
8. **[`architecture/17-view-and-intent.md`](architecture/17-view-and-intent.md)** — camera systems and the 5-layer control model
9. Domain docs as relevant: `20-domain-modelica.md`, `21-domain-usd.md`, `22-domain-cosim.md`, `23-domain-environment.md`, `24-domain-sysml.md`

## Numbering convention

Architecture docs follow a numeric prefix:

| Range | Category |
|-------|----------|
| `00`–`09` | Foundation (overview, ontology) |
| `10`–`19` | Framework (document system, workbench, API, viewport, control) |
| `20`–`29` | Per-domain design (Modelica, USD, cosim, environment, SysML) |
| `30`–`39` | Infrastructure & Deployment (Wasm, web workers, CI/CD) |
| `40`–`49` | Low-level subsystems (Asset IO, axes & units, logging) |
| `90`–`99` | Forward-looking / roadmap (collaboration, advanced features) |
| `research/` | Un-numbered historical / inspiration material |

## Writing new docs

- **Crate READMEs** are for "how do I use this crate right now."
- **Architecture docs** are for "how does LunCoSim fit together." Narrative, rationale.
- **Specs** are contracts — what a feature MUST do. Written before implementation.
- **App READMEs** are for "what is this binary and how do I run it."

One topic, one home. Avoid duplicating content — link instead.

## Doc lifecycle

- **Draft** → live review, prefix title with `> **Draft**`.
- **Active** → current design.
- **Superseded** → kept for history in `research/`.
- **Implemented** → doc describes a design that is now realized in code. Stays active.
