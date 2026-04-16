# 24 — SysML Domain

> **Stub.** SysML v2 is the source of truth for **system structure and
> requirements** — a peer domain inside a Twin, co-equal with Modelica
> (behavior) and USD (geometry). Not the Twin container itself; see
> [`13-twin-and-workflow.md`](13-twin-and-workflow.md) for the two-file
> strategy.

## 1. Scope

A SysML Document captures:

- **Parts, ports, connections** — the system architecture. What components
  exist, how they're composed, how they connect.
- **Requirements** — IDs, text, satisfies-relationships to parts,
  traceability chains.
- **Verifications** — analytical checks and simulation-based verification
  cases that validate requirements.
- **Realizations** — links from SysML parts to their domain-specific
  behavior (Modelica models) or geometry (USD prims).

In the three-tier architecture
([`00-overview.md`](00-overview.md)), SysML sits in **Tier 1** as an
editable Document alongside Modelica, USD, Mission, etc.

## 2. File format: standard SysML v2 textual syntax

SysML Documents are plain `.sysml` files using OMG-standard SysML v2
textual syntax. They contain **only standard SysML content** — no
LunCoSim-specific annotations, no tool-proprietary extensions.

This is the [interop principle](13-twin-and-workflow.md#the-two-file-strategy)
at work: our `.sysml` files must round-trip cleanly through external SysML
v2 tools (Cameo Systems Modeler, OpenMBEE services, any future Rust SysML
tooling) without losing semantic content.

Tool-specific configuration — Modelica paths, workspace preferences,
reference strategy — stays in `twin.toml`, never inside `.sysml` files.

## 3. Example

A `system.sysml` describing a simple lunar-rover architecture:

```sysml
package LunarBaseAlpha {
    version "0.3.0";

    part def LunarBase : System {
        part rover : Rover;
        part balloon : Balloon;
        part habitat : Habitat;
    }

    part def Rover : System {
        port electrical : ElectricalPort;
        port mechanical : MechanicalPort;
        attribute mass : Real = 500.0;

        // Realization links — point at other Documents in the Twin.
        // The @"..." syntax is standard SysML v2 external-reference.
        attribute behavioralRealization =
            @"electrical/rover_drive.mo"::RoverDrive;
        attribute geometricRealization =
            @"main_scene.usda"::"/World/Rover";
    }

    // Requirements with IDs and traceability
    requirement def PowerBudget {
        doc /* Rover electrical subsystem MUST operate within 500 W nominal. */
        attribute maxPower : Real = 500.0;
    }

    satisfy PowerBudget by rover;
}
```

## 4. Relationship to the Document System

Under [`10-document-system.md`](10-document-system.md) terms:

```rust
pub struct SysmlDocument {
    // AST from our SysML v2 parser
    ast: SysmlAst,
    generation: u64,
}

pub enum SysmlOp {
    AddPart       { path, type_name },
    RemovePart    { path },
    AddPort       { part, port_name, port_type },
    AddConnection { from: PortRef, to: PortRef },
    AddRequirement { id, doc_text },
    SetAttribute  { path, attr, value },
    AddRealization{ part, kind: RealizationKind, target: DocumentRef },
    // ...
}
```

Views observing a `SysmlDocument`:

- **BDD panel** — Block Definition Diagram (parts + types)
- **IBD panel** — Internal Block Diagram (composition + connections)
- **Requirements panel** — flat list / tree of requirements with traceability
- **SysML text editor** — direct textual editing with syntax highlighting
- **Parts tree** — hierarchical navigator in the Scene Tree dock

## 5. Parser strategy

**Today:** no Rust production SysML v2 parser exists. LunCoSim will
author a **subset parser** covering the features the domain actually
needs — probably 500–1500 lines of Rust. The AST is designed to be
swap-able with a full parser when the ecosystem matures.

**Supported subset (initial):**

- `package` declarations with attributes, imports
- `part def` and `part` instances with attributes, nested parts
- `port` declarations with type references
- `connection` statements
- `requirement def` with ID, doc, attributes
- `satisfy` relationships
- Standard `@"path"::"selector"` external references
- Comments and doc-strings

**Not yet supported (Phase 2+):**

- `interface def`
- Parametric constraints
- `state def` (state machines)
- Full expression language
- Behavior definitions (activities, actions)
- Allocations, refinements
- Analysis/verification execution

Users hitting an unsupported feature get a clear "not yet supported —
feature X at line N" error.

## 6. Status

- ✅ Design (this doc, plus ties to `10-document-system.md` and `13-twin-and-workflow.md`)
- ❌ Parser implementation
- ❌ `SysmlDocument` in Bevy
- ❌ BDD / IBD panels
- ❌ Requirements panel

All Phase 2+ of the overall roadmap. Modelica domain gets Document
System treatment first; SysML follows once the pattern is proven on
a simpler domain.

## 7. What this does NOT do

Explicit non-goals, to avoid scope creep:

- **`.sysml` files are NOT the Twin manifest.** `twin.toml` owns tool
  configuration — see [`13-twin-and-workflow.md`](13-twin-and-workflow.md).
- **SysML does NOT replace Modelica.** Behavior stays in Modelica; SysML
  references Modelica realizations.
- **SysML does NOT replace USD.** Geometry stays in USD; SysML references
  USD realizations.
- **Full SysML v2 support is not a v1 goal.** The subset covers the
  critical-path features; the rest grows with demand.

## 8. See also

- [`00-overview.md`](00-overview.md) — three-tier architecture
- [`01-ontology.md`](01-ontology.md) — Port, Connection, Attribute definitions (SysML-aligned)
- [`10-document-system.md`](10-document-system.md) — the editing pattern SysML will adopt
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — two-file strategy, Twin structure
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica as the behavior realization
- [`21-domain-usd.md`](21-domain-usd.md) — USD as the geometric realization
- `specs/013-sysml-integration` — detailed spec (when written)
