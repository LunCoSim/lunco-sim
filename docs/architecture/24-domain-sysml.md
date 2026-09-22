# 24 — SysML Domain

> Status: Bounded SysML v2 source/document loading, typed semantic values, a typed neutral constraint IR, Rumoca/Modelica lowering, and Rhai-owned requirement verification implemented; full KerML execution remains out of scope · Audience: contributors extending SysML v2 structure & requirements
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
- direct type identity as a native `SysmlTypeRef`, with quantity-value family
  and most-specific quantity kind classified through resolved SysML
  inheritance (rather than a hard-coded quantity-name table);
- collection cardinality kept separate from its scalar element category;
- unit-bearing literals and typed quantity-kind references;
- enumeration and structured-value categories;
- part, item, and port element categories from resolved definitions;
- an explicit Modelica mapping for scalar/quantity values, primitive arrays,
  enumerations, and structured values.

Spatial values do not use a second vector implementation. The Rhai adapter
lowers the standard `CartesianThreeVectorValue` to the existing f64 Bevy/glam
`DVec3`, and quaternions to `DQuat`, which are already registered by the
shared Rhai math bridge. Bevy f32 render transforms remain a later projection
boundary, never the SysML requirement representation.

Unit-bearing coordinates remain arrays of typed scalar `Quantity` values in
the generic SysML-to-Rhai bridge. A geometry policy may lower a
`LengthValue[3]` to `DVec3` only after it resolves and validates each
component's unit through the authored UCUM-compatible catalog and the native
quantity seam. The current Modelica geometry adapter performs that explicit
conversion to canonical metres; it never infers a unit from a bare number.
This keeps numeric vectors and dimensioned positions distinct. Rhai receives
one native `SysmlType` value instead of duplicate hand-built type maps. Unit
suffixes remain authored symbols and unresolved symbols fail the adapter; no
conversion is silently guessed or dropped.

The parser and source projection retain the declared elements and resolved
relationships from the selected files. The specialized typed records cover
parts, items, ports, attributes, requirements, verification cases, and
source-spanned constraint expression trees. Expression feature leaves carry
snapshot-scoped resolved handles; relationship ends preserve both element and
feature handles. Other parsed metamodel elements remain available as
source-backed generic elements rather than being assigned invented runtime
semantics.

This is not a SysML/KerML execution engine. The bounded constraint path below
is deliberately explicit about that boundary: it compiles the resolved
expression subset into a typed neutral IR, then lets a Rhai policy select
bindings and a Modelica/Rumoca adapter lower the supported equations. It does
not silently turn an unsupported expression into a scalar or a passing
verification result. General derived-feature evaluation, feature-chain
navigation, invocation/default-parameter semantics, N-dimensional non-Real
collections and aggregate operations, full quantity and unit conversion,
redefinition/subsetting semantics, temporal/behavioral execution, and
applicability/configuration semantics still require generic language support.
Unsupported syntax and unresolved references remain explicit with source
spans instead of being guessed from source text.

The Rhai functions `sysml_value(path, qualified_name)` and
`sysml_value_from_report(report, qualified_name)` return a tagged result map.
Successful reads carry the native value in `value`; an intentionally omitted
attribute in a selected source report is reported as `found: false` so the
Rhai helper can issue a selected query. Invalid reports, missing attributes,
and unsupported literals return `ok: false` with an error message. The
non-fatal failure also appears as a scene-scoped `RuntimeDiagnostics` warning;
scripts receive the structured result and the application remains running. A
successful read clears only the `sysml-query` warning for that same source
path, preserving diagnostics from other sources.
`sysml_requirements::native_value` unwraps successful native values and
preserves failed results for the owning verification to report.

### Constraint IR and transport boundary

`lunco-sysml-ir` is the neutral semantic boundary between the source-backed
SysML projection and downstream execution providers. It owns no Griffin names,
USD paths, Modelica classes, or verification policy. For the currently
supported subset it preserves the resolved constraint identity, source spans,
feature handles, parameter direction, multiplicity, value category, quantity
metadata, typed literals, operators, conditional expressions, dependencies,
diagnostics, and a deterministic source fingerprint. Type and multiplicity
errors are compile failures; an invalid compiled constraint cannot be passed
to an evaluator.

`lunco-sysml-modelica` is the only current lowering backend. It renders a
valid typed IR into a standalone Modelica model and admits that source through
the repository's Rumoca parser boundary before a solver is considered. Rumoca
therefore supplies Modelica parsing/compilation and numerical execution; it
does not supply SysML/KerML name resolution, feature navigation, requirement
membership, unit semantics, or engineering intent. Those remain source
projection and Rhai/provider responsibilities.

There are two intentional Rhai surfaces. In-process authored policy may retain
an immutable native `SysmlModel` handle for repeated selection. API/MCP and
other transport-oriented callers use path-level structured functions:
`sysml_constraint_ir(path, qualified_name)`,
`sysml_modelica_constraint(path, qualified_name)`, and
`sysml_evaluate_constraint(path, qualified_name, observations, abs_tol,
rel_tol)`. The latter construct the source snapshot at the call boundary and
return structured maps, so a client never has to serialize or retain a native
Rust-backed handle. Authored source and fixture references use canonical
`lunco://` asset URIs; a USD prim path inside a binding remains a USD path
such as `/World/Griffin`, not an asset URI.

The production contract is tested at the level where users consume it: Rhai
assets cover valid evaluation, source-linked diagnostics, and transport-safe
path calls, while a scene test exercises the native verdict channel and
Rumoca-admitted lowering. Rust retains only mechanism tests for IR compilation
and Modelica lowering. This split is important for Griffin: adding a new
product-specific assertion in Rust would hide the missing authored SysML
semantics instead of making them portable.

The runtime accepts source-level replace/range edits through the
`ApplySysmlOps { doc_id, ops, parent_generation? }` command. The command uses
the generic document host, so one reviewed batch is atomic, journaled and one
undo group; stale generations, invalid UTF-8 ranges and read-only origins are
returned as explicit errors. `InspectSysmlDocument` supplies source identity,
generation, origin and diagnostics for the Editor. For production checks,
`ValidateSysml` reports source validation status, structural `lint.sysml`
findings, source files and revision; it does not evaluate Twin manifest
bindings or requirement observations. `AnalyzeSysml` exposes selected
language-neutral semantic fact tables through the `HookValue` ABI. Its table,
name, and bounded-page selectors limit API payload size; they do not encode
project acceptance policy. A page request supplies non-negative `offset` and
positive `limit`; `analysis.page.tables` reports each selected table's total,
returned count, and `has_more`, and each page carries the same source revision
that Rhai must verify while assembling a larger result. Rhai runtime tools that
need native SysML/Rhai values may use `sysml_analysis(path)` for a deliberately
bounded source, then select attributes, relationships, and constraints in
Rhai. `source_with_attributes()` is value-only and intentionally omits
requirement/verification identity tables; any observer that emits
requirement-linked evidence must use `source_with_selection()` with explicit
identity selectors. For Twin-scale sources, use selected `AnalyzeSysml` pages or
`sysml_requirements::source()`, which pages the requirement/verification facts
and keeps only compact identities and coverage links. Detailed records remain
available through `source_with_selection()`. `sysml_attribute(path, qualified_name)` is the
exact-identity accessor for a single source-backed `SysmlAttribute`;
`sysml_value` is its typed-literal convenience view. These direct Rhai values
retain native `SysmlType`, `SysmlTypeRef`, and f64 spatial/quantity values
without serializing to JSON and rebuilding them. A filesystem path or
`twin://name/relative` selects one source; `twin://name` loads the
manifest-declared, indexed Twin source set.

`ReadActiveTwinContract` exposes the active Twin's component and verification
records. The policies join these generic inputs at the Rhai boundary:
`lint.sysml` checks structural source quality,
`sysml_requirements.rhai` handles source provenance and requirement
verification, and `sysml_modelica_constraints.rhai` selects geometry
constraints and assembles Modelica source. These policies keep their own
selection and rules while sharing the same typed SysML facts. The requirement
policy preserves qualified attribute identity and reports collisions rather
than silently choosing one. It can request selected attributes instead of
projecting every literal on each pass. `source()` reads the complete Twin
identity/coverage set in bounded pages and rejects a source-revision change
during assembly; parameter values and detailed requirement attributes remain
selected on demand.

Keep project-specific parameter acceptance in Rhai: dimensional limits,
component counts, acceptable material choices, and geometry rules should be
editable policy, not Rust branches that force a rebuild for every design
iteration. Rust owns the stable language substrate—SysML parsing and semantic
resolution, typed source objects, and faithful conversion to shared native
types. Native type metadata is evidence for Rhai policies, not a precomputed
Griffin verdict. Use the selected `AnalyzeSysml` API projection where a
language-neutral client benefits from a smaller transport payload; use one
native snapshot plus Rhai selection for small in-process sources, and bounded
fact pages for Twin-scale data. Changing a source parameter or authored Rhai
acceptance check must not require rebuilding Rust.

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
5. **Read-only typed bridge and independent policies.** `ValidateSysml`
   reports source status and runs the structural `lint.sysml` policy;
   `AnalyzeSysml` selects typed facts; and
   `ReadActiveTwinContract` exposes active Twin bindings. `lint.sysml` checks
   structural quality, `sysml_requirements.rhai` handles provenance and
   requirement verification, and `sysml_modelica_constraints.rhai` assembles
   geometry constraint models. Each is reloadable Rhai policy over the same
   generic Rust facts. `report_structured_verdict` emits machine-readable
   evidence plus the normal test verdict envelope. No product-specific Rust
   assertion or geometry rule is added.
6. **Production selector.** `luncosim test --scene <PATH> --verification
   QUALIFIED_NAME` validates the Twin mapping before constructing the
   simulation and selects its declared verdict channel. The mapped Rhai
   observer still owns measurement and verdict policy.

### Typed engineering values and policy boundary

The shared `lunco-engineering-values` crate is a mechanism seam, not another
domain language. It accepts a resolved unit definition (`symbol`, SI
dimensions, scale, and optional affine offset), validates finite values, and
performs dimension-safe conversion. It deliberately does not parse unit names
or contain SysML, USD, Modelica, Griffin, or relationship vocabulary.

The UCUM-compatible unit catalog is authored at the Rhai library edge and can
be replaced by a Twin/SysML library. Rhai policy selects the catalog entry,
chooses the relation and tolerance, queries USD facts, and orchestrates
Modelica/Rumoca. Native Rust functions only perform the generic value
operation and return residual-ready values. This keeps the source-of-truth
chain explicit:

```text
SysML intent and typed constants
        -> Rhai policy and orchestration
        -> USD observations / Modelica-Rumoca equations
        -> typed residual and evidence
```

Native Bevy vectors, quaternions, and transforms remain the geometry runtime
values. They are adapted to the policy-free quantity seam only when a
dimensioned scalar crosses a domain boundary; the neutral `HookValue` ABI is
not expanded with domain-runtime types.

The numerical convention is equally explicit: authored requirements,
Rhai/mechanical residuals, USD-stage geometry facts, and Modelica/Rumoca
exchange values are `f64`. The shared Rust math bridge owns representation
validity and numerically sensitive vector primitives such as finite-vector
validation and clamped cosine; it reuses the host/Rhai standard math surface
for functions such as `PI`, `acos`, and `sqrt` rather than copying constants
into each policy library. Rhai integer literals may be accepted at a policy
edge, but are canonicalized to `f64` before residual evidence is emitted.
Conversion to Bevy's `f32` `Vec3`/`Quat` is an explicit presentation or
renderer boundary only; it must not occur in SysML checks, USD measurements,
or Modelica-facing calculations.

### Mechanical relation policy

`assets/scripting/tools/mechanical_relations.rhai` is the extensible authored
mechanical/CAD relation library. It consumes already-resolved native `f64`
vectors and caller-supplied tolerances, and returns residual/evidence records;
it does not know Twin names, USD paths, SysML identifiers, or Modelica
components. Its current generic vocabulary covers scalar equality and ranges,
point distance/coincidence, signed plane distance, under/clearance, angles,
parallelism, same direction, perpendicularity, line/plane relations,
collinearity, coplanarity, coincident axes, plane mirroring, axial half-turn
symmetry, midpoint-on-plane, and paired symmetry. A Twin can extend or replace
this Rhai module without a Rust rebuild. Rust supplies only the reusable
numeric mechanism; SysML and the observing policy supply intent, units,
tolerances, source identity, and verdict ownership.

The numerical policies do not use string comparisons as a type system. Rhai
values cross through explicit Rust predicates/converters (`f64_from`,
`f64_only`, `array_is`, `map_is`, `string_is`, `vec3_is_valid`,
`vec3_is_native`, `vec2_is_native`, `transform_is_finite`) and typed overloads;
the SysML adapter similarly exposes `sysml_model_is`, `sysml_quantity_is`, and
`sysml_enum_is` for opaque semantic wrappers. This keeps the dynamic boundary
clear while avoiding repeated `type_of` dispatch in the hot path.

Runtime numerical policy is exposed by
`assets/scripting/tools/numerical_settings.rhai` over the existing generic
active-Twin settings surface. It reads explicit, independently typed `f64`
fields such as `numerics.comparison.length_abs_m`,
`numerics.comparison.scalar_abs`, `numerics.comparison.angle_abs_rad`, and
`numerics.solver.residual_abs` on
each report/solve, so a setting update is visible without a process restart.
Consumers resolve the profile once and pass the selected value through their
call graph; they do not query settings inside per-vector loops. Missing or
integer-valued tolerance settings remain configuration errors. This runtime
profile controls algorithm/solver policy only; it never silently overrides a
normative SysML requirement tolerance, which stays in the source-backed
requirement record and is included in evidence. The Modelica translation
readback consumes `numerics.solver.residual_abs`; its USD placement frame
contract consumes the explicitly separate scalar and angle fields. The Griffin
Twin manifest shows the concrete persisted profile without introducing a Rust
global or an implicit default.

### Native-plan compatibility contract

Twin-local policies must consume the shared bridge contract rather than
matching historical Rhai type labels. Numeric SysML projections are normalized
through `f64_from`/`f64_only`; arrays, maps, strings, and native vectors use
`array_is`, `map_is`, `string_is`, and `vec3_is_native`. Opaque SysML handles
and AST nodes remain domain-typed values and may retain their explicit
registered identity checks. This distinction prevents a valid typed projection
from being mistaken for an empty geometry plan.

The Griffin production observer now verifies the complete native component plan
(`27` component records and `6` rail records) together with the mechanical
relation and symmetry evidence. The plan remains a Rhai-owned, source-backed
recipe consumed by the generic visual builder; no Griffin component count or
placement table was added to Rust.

The remaining work is bounded follow-up: full KerML expression/constraint
execution, a full SysML editor, and a SysML-to-USD projection are not part of
this integration. The authored Twin loading policy selects indexed SysML
sources from the manifest and file facts, then requests each through the typed
`LoadTwinSysmlSource` command. Rust validates the active Twin, asset authority,
and indexed path, then opens the selected sources through the async document
loader; a full source browser remains a UI concern. An empty source set is an
informational policy result, since SysML is optional for a Twin.

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
verification registry, CLI selector, typed constraint IR, and Rumoca-admitted
Modelica constraint lowering are available. Full KerML expression execution,
feature navigation/invocation, collection/aggregate semantics, a full editor,
and automatic SysML-to-USD projection remain outside the supported subset.

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
