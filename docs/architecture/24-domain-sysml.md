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
source, origin, and generation; generic document hosting supplies journaling,
undo/redo, and save lifecycle. Lifecycle events snapshot source and origin for
`AsyncWorkAdmission` at `Interactive` priority. `SysmlDocumentAnalyses` commits
the immutable parser/resolver result only while both generation and origin URI
still match. `InspectSysmlDocument` exposes `pending`, `ready`, or `failed`
analysis state for that exact revision. Pending responses carry null analysis
fields, and the Rhai editor reports them as retryable instead of treating an
unfinished parse as clean. Source edits themselves only validate and update
text; they do not parse on the caller's schedule.

The optional `lunco-sysml-ui` application adapter provides a SysML workspace
with **Requirements**, **Traceability**, and **Structure** views. The
requirements view browses analyzed Twin sources, jumps from a failed check to
its requirement declaration, and edits through the canonical `SysmlDocument`
command/save lifecycle. Requirement evidence, per-case test
execution, and model coverage (`require` criteria and resolved `verify` links)
remain separate status dimensions, with independent filters. Evidence detail
shows channel, verification identity, source revision, simulation tick, and
check-level verdicts, explanations, and actual/expected values when the observer
supplies them. Definitions and usages carry distinct visible roles. Syntax,
unresolved-name, and package-collision diagnostics make the model visibly
invalid, prevent a `VERIFIED` roll-up, and disable verification actions. The
view keeps up to 64 detail records per requirement and retains separate
pass/fail/inconclusive/error counts for larger result sets. A mapped case
can run from its linked requirement. **Run selected tests** runs the
distinct mapped cases for the selected requirement; **Run all mapped tests**
runs each distinct mapped case once, sequentially, through the production
headless scene-test runner. A completed suite keeps its source revision, case
outcomes, and pass/fail/incomplete/cancelled counts. **Show failures** filters
the list to failed requirements, and **Rerun failed** launches only failed
cases. The suite can be stopped while it is active.

The Requirements overview shows separate usage, definition, and source-file
counts plus quick filters for missing criteria, missing `verify` links,
unmapped Twin tests, and tests without a current result. Its sortable table
uses the full center pane and keeps evidence, Twin test execution, model
coverage, and overall status in separate columns. A docked Requirement details
panel stays in the lower-right Editor pane, follows the selected row, and
summarizes formal criteria, subjects, explicit `satisfy` links, verification
mappings, and evidence. It links to the full Traceability view and can open the
active Twin's `twin.toml` when a test mapping is missing. The ID sort follows
the visible ID, role cells spell out Definition or Usage, and source cells
show the file name and line. Column headers support independent sorting, drag
reordering, and divider resizing. The detail summary keeps the selected
subject and verification-mapping state visible while Engineering trace stays
collapsible. Missing-verify counts are scoped to usages;
unmapped counts flag requirement elements with at least one unmapped case, and
not-run counts flag elements whose mapped cases have no current result.
Details show a usage statement or, when absent, text from an
unambiguously resolved definition, with separate source links. Each case
displays its scene path near **Run mapped test**.

Status explanations live on column-heading and status-label hints. **Status
details** opens aggregate counts on demand; the main workspace does not expand
a status reference. Summary cards use stacked text and native button focus,
disabled, and selected states. Their counts describe requirement entries;
run-button counts describe unique mapped tests. The selected requirement's
statement precedes source metadata. The application catalog exposes
**Tutorials → LunCoSim → SysML Requirements**, an authored Rhai tour using the
active Twin and existing guided-tour commands.

The runner returns report schema 2 over the child-process boundary. It separates
the four-state verification verdict from runner completion status and includes
the source revisions observed at start and completion. The UI rejects results
whose observed revisions do not match the requested source. Runner-level reasons
for readiness failures, runtime faults, or exhausted limits stay separate from
authored check results. The UI builds check identities and explanations from
typed report evidence instead of inferring status from log text. The bounded
captured output remains available per case. Evidence check details can open the associated
requirement declaration in the inline source editor. Requirements without a
resolved `verify` link or a Twin test mapping remain visible as coverage gaps
and are skipped. Tests read saved Twin files, so running is disabled while an
indexed SysML document or local source draft has unsaved edits.

The traceability view maps one selected requirement across its declared subject
types, explicit standard `satisfy` relationships, resolved `verify` cases, Twin
test registrations, run outcomes, and revision-scoped evidence. It keeps
subject type declarations distinct from `satisfy` relationships and shows a
missing relationship as a gap instead of inferring one. Model elements and
verification declarations open at their analyzed source lines; mapped tests can
run from the map. The structure view presents the analyzed package, part, item,
interface, port, and connection hierarchy with text filtering and source
navigation. Both views use the same prepared analysis snapshot as the
requirements panel. **Open full traceability** focuses or opens the center
SysML Requirements panel before switching to its Traceability tab, even when
another center panel is active.

The requirement roll-up is intentionally strict: `VERIFIED` requires a valid
analyzed model, a formal `require` criterion, all `verify` links mapped to Twin
tests, current passing requirement evidence, and passing results for every
mapped test. `FAILED` means a standard fail; `INCONCLUSIVE`, `ERROR`, and
`RUN ERROR` preserve insufficient evidence, evaluation errors, and runner errors
as separate outcomes. `INVALID MODEL`, `STALE`, `RUNNING`, and `INCOMPLETE` keep
invalid source, old, active, and missing proof explicit. This roll-up summarizes
supplied verification evidence; it does not imply full KerML constraint
execution. A missing formal `require` criterion is a model-coverage issue, not a
test failure.

The panel belongs to the existing workbench Editor level, alongside native
document tools; it does not add a Twin HUD surface or a new workspace. The
structure view is a navigable hierarchy, not full BDD/IBD diagram editing or a
general-purpose SysML editor. The headless document and analysis owners remain
usable without this UI crate.

## 5. Parser strategy

**Today:** `lunco-sysml-ast` embeds the pinned `sysmlv2-semantics` parser and
its `sysmlv2-stdlib` data. The crate exposes a stable LunCoSim projection of
source-backed elements, typed attributes/literals, requirement records,
verification records, resolved references, and syntax/name/collision
diagnostics; the upstream model remains private to the AST boundary.

`SysmlAnalysis::content_closure()` freezes this exact source set for replay
identity. It returns sorted CIDv1 raw/SHA-256 identities for project files and
every embedded standard-library file when enabled, and fails when diagnostics,
empty names, or duplicate names leave the analysis incomplete or ambiguous.
The FNV source revision and fingerprint remain cache identities only.

Expression nodes use payload-carrying variants: each feature reference,
invocation, literal, operator, index, collection, conditional, or unsupported
syntax node carries only its own data and child topology. Fixed-arity operands
are represented directly, so the IR compiler does not reconstruct arity from a
generic child vector or diagnose missing operator/literal payloads that the AST
type can make impossible. Unsupported syntax remains an explicit source-linked
variant.

The typed projection now also preserves the standard concepts needed by the
Griffin component model:

- kernel primitive categories (`Boolean`, `Integer`, `Rational`, `Real`,
  `Complex`, and `String`);
- feature multiplicity, including lower/upper bounds and ordered/unique flags;
- direct type identity as a native `SysmlTypeRef` backed by a
  snapshot-scoped `SysmlElementHandle`, with quantity-value family and
  most-specific quantity kind classified through resolved SysML inheritance
  (rather than a hard-coded quantity-name table);
- collection cardinality kept separate from its scalar element category;
- authored quantity-literal unit suffixes retained as lexical source data,
  with resolved measurement-unit feature identity, declared unit type, SI
  dimension, and conversion scale when the source reference resolves;
- enumeration and structured-value categories;
- part, item, and port element categories from resolved definitions;
- an explicit Modelica mapping for scalar/quantity values, primitive arrays,
  enumerations, and structured values.

The neutral constraint IR preserves `SysmlTypeRef` identity for quantity kinds,
enumerations, references, and structured values. A qualified name is retained
for diagnostics, source navigation, and Modelica metadata; identity equality
uses the source-snapshot handle. Runtime quantities and binding-contract units
are `Quantity` and `Unit` values from `lunco-engineering-values`; an observation
does not carry a second unit field. Compatibility is based on SI dimensions,
and compatible quantities are converted for addition, subtraction, comparison,
minimum, and maximum. Multiplication, division, powers, and square roots return
coherent SI units. `SysmlQuantityLiteral.measurement_reference` now carries the
resolved SysML unit feature handle, declared unit type, and an optional
standard-derived definition. `MeasurementUnit` definitions project quantity
power factors, coherent SI base-unit scales, linear reference-unit conversions,
prefixes, and supported multiplicative unit initializers into SI dimensions and
scale. Numeric conversion-factor expressions support arithmetic including
integer powers; addition of dimensioned units is rejected. Conversion exactness
from standard [`UnitConversion::isExact`](https://raw.githubusercontent.com/Systems-Modeling/SysML-v2-Release/master/sysml.library/Domain%20Libraries/Quantities%20and%20Units/MeasurementReferences.sysml)
relationships is preserved on native
`EngineeringUnit` and `Quantity` values and combined through quantity arithmetic.
This records scale exactness only; source measurement uncertainty remains a
separate value contract. Rhai exposes the unit flag as `scale_is_exact` and the
quantity's accumulated conversion flag as `conversion_is_exact`.
The Rhai adapter constructs a native `EngineeringUnit` from that typed
definition, and a source-literal quantity enters the neutral evaluator as a
native `Quantity`. The sibling `unit_symbol` remains source spelling only.
The bounded Modelica position adapter rejects units whose SI conversion is
approximate or unspecified before lowering the value to an unqualified vector.
`MeasurementScale` mappings and malformed or unsupported unit definitions do
not produce a runtime unit, so they remain unavailable instead of receiving a
guessed conversion.

Spatial values do not use a second vector implementation. The Rhai adapter
lowers the standard `CartesianThreeVectorValue` to the existing f64 Bevy/glam
`DVec3`, and quaternions to `DQuat`, which are already registered by the
shared Rhai math bridge. Bevy f32 render transforms remain a later projection
boundary, never the SysML requirement representation.

The typed `SysmlModel.value` projection preserves declared collection shape:
a structured `Position` value lowers to one native `DVec3`, while a declared
`Position[n]` lowers to an array of native `DVec3` values. The projection uses
the declared type and multiplicity to distinguish the vector's three scalar
components from the collection of vectors. Twin Rhai must consume this typed
value rather than reconstructing it from `AnalyzeSysml` syntax records.

Unit-bearing coordinates remain arrays of typed scalar `SysmlQuantityLiteral`
values in the generic SysML-to-Rhai bridge. A geometry policy may lower a
`LengthValue[3]` to `DVec3` only after it resolves each component's typed
measurement reference, verifies the engineering-unit dimension, and converts
the native quantity to coherent SI. The Modelica geometry adapter uses this
source-derived path; it never looks up the lexical unit spelling or infers a
unit from a bare number. This keeps numeric vectors and dimensioned positions
distinct. Rhai receives one native `SysmlType` value instead of duplicate
hand-built type maps.

The parser and source projection retain the declared elements and resolved
relationships from the selected files. Resolved function-call expressions now
retain the target element handle, bound input-parameter handles, and an optional
typed standard-function operation. The specialized typed records cover
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
verification result. The evaluator currently executes recognized standard
numeric functions (`abs`, `min`, `max`, `sqrt`, `floor`, `round`, `sum`,
`product`, `isZero`, `isUnit`), trigonometric functions and angle conversions
(`sin`, `cos`, `tan`, `cot`, `arcsin`, `arccos`, `arctan`, `deg`, `rad`),
sequence size predicates (`size`, `isEmpty`, `notEmpty`), and
Boolean aggregates (`allTrue`, `anyTrue`) over typed provider collections,
plus primitive scalar `ToString` overloads.
Calls retain snapshot-scoped function identity and formal input handles and are
exposed in the typed IR and Rhai reports; `sysml_standard_functions()` lists
the implemented operations with evaluator and Modelica-lowering availability.
`sysml_constraint_operators()` exposes the typed operator set used by the
constraint compiler. The Rhai model tools `constraint_ir`,
`modelica_constraint`, and `evaluate_constraint` expose compilation, backend
lowering, and provider-backed evaluation respectively. Modelica support in the
function catalog comes from the same lowering table used by the Modelica
adapter, which reports an explicit error for calls it cannot lower.

The standard `TrigFunctions::pi` feature is a source-linked numeric constant
and is listed by `sysml_standard_constants()`.
Parenthesized multi-value expressions compile to homogeneous typed collections;
single-index `sequence[index]` and `sequence#(index)` access uses SysML's
one-based indexing and is available to the evaluator and Modelica lowerer.
Out-of-range and zero/negative indices produce explicit evaluation errors.

General derived-feature evaluation, feature-chain navigation, user-defined
function-body execution, default-argument expansion and overload resolution
beyond recognized standard functions, feature-valued or N-dimensional
collections and aggregates beyond the supported scalar functions, static unit
inference for quantity feature types and non-linear or affine
`MeasurementScale` mappings,
redefinition/subsetting semantics, temporal/behavioral execution, and
applicability/configuration semantics still require generic language support.
KerML defines operator expressions through function invocation and overload
resolution; this subset currently lowers parsed operator syntax to `IrOperator`
with built-in type rules, so it does not yet retain or dispatch the resolved
operator-function target.
Trigonometric calls currently require unitless real-valued radians; typed
quantity angles are not accepted by those standard-function overloads. Square
root supports runtime quantities whose SI dimension exponents can be halved.
The evaluator does not yet implement string-to-numeric or string-to-Boolean
parsing, and Modelica lowering does not advertise `ToString` until its output
format can match evaluator semantics. Other standard-library
gaps include executable Rational and Complex values, collection-object
operations, sequence transforms, higher-order control functions with lambda
bodies, and vector constructors/`norm`/`inner`. Rational and Complex primitive
identities are preserved through the type projection and neutral IR; scalar
constraint execution emits an explicit unsupported-type diagnostic until exact
values are supported end to end.
The call catalog is an executable subset, not a claim of full KerML Function
Library conformance.
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
`lint.sysml` checks documentation, subjects, verification coverage, typed
source-evidence links, formal acceptance constraints, and identifier-like
strings that encode enumerators or member identities,
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

The UCUM-compatible unit catalog is authored at the Rhai library edge for
explicit unit input at non-SysML boundaries. SysML source quantities use their
resolved standard measurement-unit definition and never consult that catalog.
Rhai policy chooses the relation and tolerance, queries USD facts, and
orchestrates Modelica/Rumoca. Native Rust functions perform the generic value
operation and return residual-ready values. Provider observations must carry a
native `Quantity` built from a resolved `EngineeringUnit`; put an optional
typed `EngineeringUnit` in `BindingContract.unit`. Separate observation unit
labels and `{ value, unit: "..." }` quantity maps are rejected. The current
SysML projection retains suffix spelling for display and resolves supported
linear `MeasurementUnit` definitions from the semantic model. Unsupported
scales stay unavailable. This keeps the source-of-truth chain explicit:

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
informational policy result, since SysML is optional for a Twin. It commits an
empty ready analysis without initializing the embedded standard library;
non-empty Twin source sets resolve against that library on the async analysis
worker.

Saving an open document that belongs to a mounted Twin's indexed SysML source
set invalidates that Twin's analysis snapshot immediately. The runtime matches
the document's canonical file identity to the prepared source set, reloads the
same `twin://` asset, and waits for that asset's change event before capturing
source text for the next async analysis. `AnalyzeSysml` therefore reports its
preparation state instead of serving the pre-save snapshot during this refresh.
Asset load or analysis failures remain explicit terminal states. Unsaved edits
do not alter the Twin snapshot; this lifecycle is tied to a successful save.

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
verification registry, CLI selector, typed constraint IR with the recognized
standard-function subset, and Rumoca-admitted Modelica constraint lowering are
available. Full KerML expression execution, feature navigation, user-defined
function bodies/defaults, general collection/index/aggregate semantics, a full
editor, and automatic SysML-to-USD projection remain outside the supported
subset.

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
