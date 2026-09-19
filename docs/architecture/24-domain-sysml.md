# 24 — SysML Domain

> Status: Bounded SysML v2 source/document loading, typed semantic values, and Rhai-owned requirement verification implemented · Audience: contributors extending SysML v2 structure & requirements
>
SysML v2 is the portable source for **logical system structure, requirements,
and verification intent** — a peer domain inside a Twin alongside Modelica and
USD. The implemented runtime loads and resolves a bounded SysML subset and
exposes typed facts to Rhai. SysML does not own executable prim identity,
composed scene topology, spatial geometry, or physics; those remain USD facts.
Modelica owns continuous equations and state, while Rhai owns runtime policy,
observations, and verdicts. SysML is not the Twin container itself; see
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

`lunco-sysml` owns the source-backed `SysmlDocument` and reversible
`SysmlOp::{ReplaceSource, EditText}` operations. The document retains its
source, origin, generation and refreshed `lunco-sysml-ast` analysis; generic
document hosting supplies journaling, undo/redo and save lifecycle. The
`InspectSysmlDocument` query exposes the current source identity and
diagnostics, while document commands apply source edits through that host.

This document lifecycle is not a claim that SysML has dedicated BDD, IBD,
requirements-tree or text-editor UI. The production surface is the generic
document API, typed reports, and Rhai verification described below.

## 5. Parser strategy

**Today:** `lunco-sysml-ast` embeds the pinned `sysmlv2-semantics` parser and
its `sysmlv2-stdlib` data. The crate exposes a stable LunCoSim projection of
source-backed elements, typed attributes/literals, requirement records,
verification records, resolved references, and syntax/name/collision
diagnostics; the upstream model remains private to the AST boundary.

The typed projection now also preserves the standard concepts needed by the
Griffin component model:

- kernel primitive categories (`Boolean`, `Integer`, `Rational`, `Real`,
  `Complex`, and `String`);
- feature multiplicity, including lower/upper bounds and ordered/unique flags;
- quantity kind and unit-bearing literals;
- enumeration and structured-value categories;
- part, item, and port element categories from resolved definitions;
- an explicit Modelica mapping for scalar/quantity values, primitive arrays,
  enumerations, and structured values.

Spatial values do not use a second vector implementation. The Rhai adapter
lowers the standard `CartesianThreeVectorValue` to the existing f64 Bevy/glam
`DVec3`, and quaternions to `DQuat`, which are already registered by the
shared Rhai math bridge. Bevy f32 render transforms remain a later projection
boundary, never the SysML requirement representation.

The parser and source projection retain the declared elements and resolved
relationships from the selected files. The specialized typed records cover
parts, items, ports, attributes, requirements, verification cases, and opaque
constraint expressions. Other parsed metamodel elements remain available as
source-backed generic elements rather than being assigned invented runtime
semantics.

This is not a SysML/KerML execution engine. Constraint expressions, general
derived-feature evaluation, N-dimensional non-Real collections, full quantity
conversion, redefinition/subsetting semantics, and state/behavior execution
are not evaluated by the runtime. Rhai owns verification policy and consumes
the typed facts that the current projection can establish; unresolved values
remain explicit instead of being guessed from source text.

The Rhai functions `sysml_value(path, qualified_name)` and
`sysml_value_from_report(report, qualified_name)` return a tagged result map.
Successful reads carry the native value in `value`; an intentionally omitted
attribute in a selected compact report is reported as `found: false` so the
Rhai helper can issue a selected query. Invalid reports, missing attributes,
and unsupported literals return `ok: false` with an error message. The
non-fatal failure also appears as a scene-scoped `RuntimeDiagnostics` warning;
scripts receive the structured result and the application remains running. A
successful read clears only the `sysml-query` warning for that same source
path, preserving diagnostics from other sources.
`sysml_requirements::native_value` unwraps successful native values and
preserves failed results for the owning verification to report.

The runtime accepts source-level replace/range edits through the
`ApplySysmlOps { doc_id, ops, parent_generation? }` command. The command uses
the generic document host, so one reviewed batch is atomic, journaled and one
undo group; stale generations, invalid UTF-8 ranges and read-only origins are
returned as explicit errors. `InspectSysmlDocument` supplies source identity,
generation, origin and diagnostics for the Editor. Structured
requirement/part operations and verification execution remain follow-up work;
the Rhai adapter reports requirements and owns verification policy without
mutating the semantic model directly. For production checks, the
scene-validation plugin registers the compact `ValidateSysml { path }` query. A
filesystem path or `twin://name/relative` validates one source; `twin://name`
loads the manifest-declared, indexed Twin
source set. The query returns typed attributes, requirement/verification
records, source files, diagnostics, and a deterministic source revision. The
compact Rhai projection uses one lossless map keyed by qualified SysML names;
it does not duplicate a short-name map that could overwrite colliding
component literals. Short-name collisions remain available in the full
validation report for tooling, while a requirement script must use the
qualified key. The shared `lunco-sysml-report` crate owns this transport shape,
keeping the Rhai/API boundary bounded without a second filesystem walker or
product-specific Rust projection.

Acceptance remains Twin-authored: each Twin keeps its SysML requirements, USD
fixture, and Rhai scenario together. Core runtime code contains only this
generic read-side bridge; product names in this document are examples, not
shipped acceptance assets or assertions.

## 6. SysML v2 requirement and verification contract

SysML is the normative home for a requirement's *intent*; it is not a second
physics engine or a replacement for the production scene runner. A requirement
is a standard SysML definition/usage, and a verification case is a standard
SysML verification definition/usage that names the requirement it answers.
`satisfy`, `verify`, and realization references carry traceability. Rhai reads
these typed records, observes the composed USD/Modelica runtime, and owns the
verification procedure and verdict.

### Requirement quality

An informal stakeholder request is not yet a baselined requirement. Each
requirement should state one subject and one necessary, feasible,
implementation-independent behavior or characteristic that is clear,
measurable or otherwise verifiable, and traceable to a parent goal. Its
operating condition, observable measure, units, threshold/range/tolerance,
time or sampling window, and verification method must be defined well enough
for an independent reviewer to decide pass or fail. Vague expectations such as
“make it look good”, “be stable”, or “handle errors” remain open expectations
until the stakeholder supplies an approved reference and finite acceptance
criteria. The authoring question protocol and examples live in the
[`sysml-requirements` skill](../../skills/sysml-requirements/SKILL.md#requirement-quality-gate).

This quality gate is consistent with
[`NASA Appendix C`](https://www.nasa.gov/reference/appendix-c-how-to-write-a-good-requirement/)
and [`NPR 7123.1C`](https://nodis3.gsfc.nasa.gov/displayAll.cfm?Internal_ID=N_PR_7123_001C_&page_name=all).

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
4. **Component ownership.** Optional `[[components]]` records make the split
   enforceable: each component names one indexed `.sysml`/`.kerml` requirement
   source and one qualified verification case. Shared requirement sources,
   scenes, scripts, missing mappings, and unsafe USD prim roots are rejected;
   the case supplies the component's Twin-local USD fixture and Rhai observer.
5. **Read-only Rhai bridge and lint.** `ValidateSysml`,
   `sysml_requirements::source()` and the native report helpers expose the
   resolved requirement/verification snapshot and source revision. The generic
   `sysml_requirements::evaluate` tool observes the composed USD stage and
   `report_structured_verdict` emits machine-readable evidence plus the normal
   test verdict envelope. The explicit `ValidateAsset`/`ValidateSysml` paths
   also run `lint.sysml` from `assets/scripting/policy/lint_sysml.rhai` over
   typed AST facts, so missing subjects, empty verification cases, unresolved
   verification targets, and uncovered requirement usages are reported by
   reloadable Rhai policy. No requirement-specific Rust assertion is added.
6. **Production selector.** `luncosim test --scene <PATH> --verification
   QUALIFIED_NAME` validates the Twin mapping before constructing the
   simulation and selects its declared verdict channel. The mapped Rhai
   observer still owns measurement and verdict policy.

The remaining work is bounded follow-up: full KerML expression/constraint
execution, a full SysML editor, and a SysML-to-USD projection are not part of
this integration. `SysmlPlugin` opens the checked Twin source set after
`TwinAssetMounted`; a full source browser remains a UI concern.

### Verification ownership

- State requirement intent, units, limits, and traceability in standard SysML.
- Map a qualified verification case to its production scene and Rhai observer
  in the Twin manifest. Keep component ownership there as well when used.
- Let the Rhai observer read typed SysML facts, inspect the composed USD and
  live simulation through public queries, and emit the verdict and evidence.
  Do not duplicate those observable behavior assertions in Rust tests.
- Keep Rust tests for mechanisms that the production Rhai/API surface cannot
  observe, such as parser lowering, serialization, schema composition, and
  generic lifecycle invariants.
- A parser/preflight result is not runtime acceptance. Run the mapped
  production scene test and inspect its authored verdict channel.

## 7. Status

The SysML v2 integration is enabled by default in the production app, core, and
server (and can be explicitly removed with `--no-default-features`): the pure AST projection, Bevy source/document plugin,
canonical journal domain, `.sysml`/`.kerml` classification, `[sysml]` Twin
manifest source roots, manifest-aware source discovery, pre-flight validation,
read-only Rhai requirement/verification reports, structured evidence,
verification registry, and CLI selector are available. Full KerML expression
execution, a full editor, and automatic SysML-to-USD projection remain outside
the supported subset.

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
- [`10-document-system.md`](10-document-system.md) — the shared document editing pattern
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — two-file strategy, Twin structure
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica as the behavior realization
- [`21-domain-usd.md`](21-domain-usd.md) — USD as the geometric realization
- `specs/013-sysml-integration` — detailed spec (when written)
