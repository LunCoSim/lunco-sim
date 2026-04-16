# LunCoSim Documentation

Welcome. This directory is the authoritative home for LunCoSim architecture,
design, and reference documentation.

## Where things live

| Path | Purpose |
|------|---------|
| [`principles.md`](principles.md) | Non-negotiable project principles (TDD, plugin-first, etc.) |
| [`architecture/`](architecture/) | Design narrative — how LunCoSim is structured |
| [`architecture/01-ontology.md`](architecture/01-ontology.md) | Terminology reference — the authoritative source for terms like "Space System", "Port", "Connection", "Attribute" |
| `../specs/` | Detailed feature specifications (contracts for implementation) |
| `../crates/<crate>/README.md` | Per-crate quick-start (use this when you want to use a crate) |
| [`architecture/research/`](architecture/research/) | Historical analysis, inspiration, rejected paths |
| Legacy files (top-level: `api.md`, `USD_SYSTEM.md`, `GRAVITY_ARCHITECTURE.md`, `WEB_BUILD.md`) | To be retired or migrated into numbered architecture docs |

## Reading order for newcomers

1. **[`architecture/00-overview.md`](architecture/00-overview.md)** — what LunCoSim is, the three-tier model, how the crates layer together
2. **[`principles.md`](principles.md)** — how we work (TDD, plugin-first, hot-swappable, interop principle, documentation mandate)
3. **[`architecture/01-ontology.md`](architecture/01-ontology.md)** — vocabulary (Space System, Port, Connection, Attribute, CommandMessage, etc.)
4. **[`architecture/10-document-system.md`](architecture/10-document-system.md)** — the foundational data model: Documents, DocumentOps, DocumentViews
5. **[`architecture/11-workbench.md`](architecture/11-workbench.md)** — UI/UX architecture: workspaces, panels, detachable windows, command palette
6. **[`architecture/13-twin-and-workflow.md`](architecture/13-twin-and-workflow.md)** — what a Twin is, two-file strategy, save/load/workflow
7. Domain docs as relevant: `20-domain-modelica.md`, `21-domain-usd.md`, `22-domain-cosim.md`, `23-domain-environment.md`, `24-domain-sysml.md`

## Numbering convention

Architecture docs follow a numeric prefix:

| Range | Category |
|-------|----------|
| `00`–`09` | Foundation (overview, ontology) |
| `10`–`19` | Framework (document system, workbench, UI widgets, project structure, simulation control) |
| `20`–`29` | Per-domain design (Modelica, USD, cosim, environment, SysML, mission) |
| `30`–`39` | Forward-looking / roadmap (collaboration, advanced features) |
| `research/` | Un-numbered historical / inspiration material |

## Writing new docs

- **Crate READMEs** are for "how do I use this crate right now." Quick starts, usage examples.
- **Architecture docs** are for "how does LunCoSim fit together." Narrative, design rationale, cross-cutting concerns.
- **Specs** are contracts — what a feature MUST do. Written before implementation.
- **Ontology** is for terms. If you're coining a new term, add it here first.

One topic, one home. Avoid duplicating content across architecture docs and crate READMEs — link instead.

## Doc lifecycle

- **Draft** → live review, may change significantly. Prefix title with `> **Draft**`.
- **Active** → current, describes how things are / will be.
- **Superseded** → kept for history, moved to `research/` with a note pointing at the replacement.
- **Implemented** → doc describes a design that is now realized in code. Stays active.

## Contributing

Architecture changes that touch ontology, framework, or cross-cutting concerns should be discussed in an architecture doc before code is written. Small domain changes can be captured in the relevant `20-domain-*.md` at review time.
