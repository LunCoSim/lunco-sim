# 24 — SysML Domain

> Status: Foundation implemented · Audience: contributors extending SysML v2 structure & requirements
>
SysML v2 is the source of truth for **system structure and
requirements** — a peer domain inside a Twin, co-equal with Modelica
(behavior) and USD (geometry). Not the Twin container itself; see
[`13-twin-and-workflow.md`](13-twin-and-workflow.md) for the two-file
strategy.

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
    private import ScalarValues::Real;
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
    // Serializable projection from lunco-sysml-ast
    analysis: Arc<SysmlAnalysis>,
    source: String,
    generation: u64,
}

pub enum SysmlOp {
    ReplaceSource { new: String },
    EditText { range: Range<usize>, replacement: String },
}
```

Views observing a `SysmlDocument`:

- **BDD panel** — Block Definition Diagram (parts + types)
- **IBD panel** — Internal Block Diagram (composition + connections)
- **Requirements panel** — flat list / tree of requirements with traceability
- **SysML text editor** — direct textual editing with syntax highlighting
- **Parts tree** — hierarchical navigator in the Scene Tree dock

## 5. Parser strategy

**Today:** `lunco-sysml-ast` embeds the pinned `sysmlv2-semantics` parser and
its `sysmlv2-stdlib` data. The crate exposes a stable LunCoSim projection of
source-backed elements, typed attributes/literals, requirement records,
verification records, resolved references, and syntax/name/collision
diagnostics; the upstream model remains private to the AST boundary.

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

The runtime currently accepts source-level replace/range edits. Structured
requirement/part operations and verification execution remain follow-up work;
the read-only Rhai adapter reports requirements without mutating the model.
For production checks, the scene-validation plugin registers the compact
`ValidateSysml { path }` query. A filesystem path or `twin://name/relative`
validates one source; `twin://name` loads the manifest-declared, indexed Twin
source set. The query returns typed attributes, requirement/verification
records, source files, diagnostics, and a deterministic source revision. It
also exposes a qualified attribute map and reports short-name collisions, so a
Rhai consumer can use a convenient local key when it is unique and fall back
to the lossless qualified key when definitions reuse a name. This keeps the
Rhai boundary bounded and avoids a second filesystem walker or
product-specific Rust projection.

Acceptance remains Twin-authored: each Twin keeps its SysML requirements, USD
fixture, and Rhai scenario together. Core runtime code contains only this
generic read-side bridge; product names in this document are examples, not
shipped acceptance assets or assertions.

## 6. SysML v2 requirement and verification contract

SysML becomes the normative home for a test's *intent*; it does not become a
second physics engine or a replacement for the production scene runner. A
requirement is a standard SysML definition/usage, and a verification case is a
standard SysML verification definition/usage that names the requirement it
answers. `satisfy`, `verify`, and realization references carry the traceability
that is currently implicit in Rhai filenames and comments.

The first Twin subset should use only portable SysML constructs:

```sysml
package ExampleRequirements {
    private import ScalarValues::Real;
    part def ExampleRover;

    requirement def REQ001_MassBudget {
        doc /* REQ-001: the flight rover mass shall not exceed 450 kg. */
        attribute maxMass : Real = 450.0;
    }

    requirement req001 : REQ001_MassBudget {
        subject rover : ExampleRover;
    }

    verification def Verify_REQ001 {
        subject rover : ExampleRover;
        verify req001;
    }
}
```

The example is deliberately limited to definitions, usages, subjects, scalar
attributes, and `verify`; every committed fixture must also pass
`lunco-sysml-ast` validation. The qualified SysML name is the stable key. A
human identifier such as `REQ-001` belongs in the requirement documentation (or
in a future standard metadata projection), not in a LunCo-specific annotation
that would break interchange.

### Current integration path

The implementation now covers the complete bounded path from source to an
authored runtime verdict:

1. **Semantic projection.** `lunco-sysml-ast` exposes requirement and
   verification records, qualified names, typed scalar attributes,
   `satisfy`/`verify` links, source ranges and a deterministic source-set
   revision.
2. **Twin source-set discovery.** `Twin::files()` is the authority for
   `.sysml`/`.kerml` discovery. `[sysml]` narrows the indexed set and orders an
   optional root; the parser resolves the selected sources once through the
   canonical Twin identity. There is no second filesystem walker.
3. **Verification registry.** Twin-owned `[verification]` configuration maps a
   qualified verification-case name to an indexed production scene and Rhai
   observer. Missing, duplicate, unsafe or mismatched mappings are explicit
   errors; the SysML file remains standard and portable.
4. **Read-only Rhai bridge.** `ValidateSysml`,
   `sysml_requirements::source()` and the native report helpers expose the
   resolved requirement/verification snapshot and source revision. The generic
   `sysml_requirements::evaluate` tool observes the composed USD stage and
   `report_structured_verdict` emits machine-readable evidence plus the normal
   test verdict envelope. No requirement-specific Rust assertion is added.
5. **Production selector.** `luncosim test --scene <PATH> --verification
   QUALIFIED_NAME` validates the Twin mapping before constructing the
   simulation and selects its declared verdict channel. The mapped Rhai
   observer still owns measurement and verdict policy.

The remaining work is bounded follow-up: full KerML expression/constraint
execution, a full SysML editor, automatic UI source-set discovery, and a
SysML-to-USD projection are not part of this integration.

### Migration rules

- Inventory each current Rhai assertion as a requirement, a verification
  observation, or a mechanism test. Only the first two move to SysML.
- Create SysML definitions/usages and a verification case beside the existing
  USD fixture and Rhai observer. Run both in shadow mode and compare verdicts
  before changing the gate.
- Move thresholds and acceptance text into SysML attributes/constraints where
  the selected parser can preserve them. Keep measurement, command sequencing,
  and runtime reads in the Rhai backend; it consumes the SysML case and emits
  observations rather than redefining the requirement. This is a Rhai test
  migration, not a new Rust test framework.
- Switch the production gate to the typed SysML verdict only after positive,
  negative, anti-trivial-motion, stale-generation, and evidence-path checks
  pass. Then remove duplicate Rhai assertions in the same change.
- Leave parser, USD schema, Modelica solver, Avian mechanics, command,
  lifecycle, and authority tests in Rust. SysML is not a wrapper around a Rust
  test, and a Rhai string executed by Rust is not a production migration.

## 7. Status

The SysML v2 integration is implemented in the current runtime behind the
opt-in `sysml` feature: the pure AST projection, Bevy source/document plugin,
canonical journal domain, `.sysml`/`.kerml` classification, `[sysml]` Twin
manifest source roots, manifest-aware source discovery, pre-flight validation,
read-only Rhai requirement/verification reports, structured evidence,
verification registry, and CLI selector are available. Full KerML expression
execution, a full editor, automatic UI source discovery, and automatic
SysML-to-USD projection remain outside the supported subset.

## 8. What this does NOT do

Explicit non-goals, to avoid scope creep:

- **`.sysml` files are NOT the Twin manifest.** `twin.toml` owns source-set
  configuration (`[sysml]`) — see [`13-twin-and-workflow.md`](13-twin-and-workflow.md).
- **SysML does NOT replace Modelica.** Behavior stays in Modelica; SysML
  references Modelica realizations.
- **SysML does NOT replace USD.** Geometry stays in USD; SysML references
  USD realizations.
- **Full SysML v2 support is not a v1 goal.** The subset covers the
  critical-path features; the rest grows with demand.

## 9. See also

- [`00-overview.md`](00-overview.md) — three-tier architecture
- [`01-ontology.md`](01-ontology.md) — Port, Connection, Attribute definitions (SysML-aligned)
- [`10-document-system.md`](10-document-system.md) — the editing pattern SysML will adopt
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — two-file strategy, Twin structure
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica as the behavior realization
- [`21-domain-usd.md`](21-domain-usd.md) — USD as the geometric realization
- `specs/013-sysml-integration` — detailed spec (when written)
