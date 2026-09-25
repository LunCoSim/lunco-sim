# Griffin SysML/KerML IR migration review

**Reviewed:** 2026-09-22
**Update:** 2026-09-25

**Input:** `griffin-kerml-architecture-and-implementation-report-2026-09-22.md` and the current `astrobotic-griffin-1` SysML/Rhai sources

**Scope:** What the new LunCoSim neutral constraint IR enables, what Griffin still lacks, and which workarounds must be removed rather than preserved

## Executive finding

The new `lunco-sysml-ir` and `lunco-sysml-modelica` crates close the missing
generic compilation boundary identified in the handover report. They do not
make an existing Griffin model executable automatically. Griffin currently has
typed-looking values, requirements, verification identities, render metadata,
and study assumptions, but not an executable SysML topology/constraint model.

The current Griffin sources therefore make the right architectural layers
visible while still leaving the engineering model split across three
representations:

```text
SysML attributes and doc text
  + Rhai qualified-name tables and relationship reconstruction
  + USD paths and offsets
  -> manually assembled geometry/verification behavior
```

The target is a source-driven graph:

```text
standard SysML/KerML definitions/usages and relationships
  -> resolved semantic graph and typed neutral constraint IR
  -> Rhai-selected provider bindings
  -> Modelica/Rumoca equations and USD observations/proposals
  -> evidence linked to the originating requirement/constraint/source revision
```

No Griffin-specific Rust should be added to bridge the current gap. The
missing language mechanisms belong in the generic AST/IR/provider layers;
Griffin should then author standard source and policy over those mechanisms.

### 2026-09-23 migration update

The generic `sysml_requirements::constraint_check` path now binds named
provider observations to the neutral constraint IR and preserves its four-state
verdict, source revision, and constraint fingerprint in evidence. It also has
an explicit-document evaluator for Editor USD queries. Griffin's payload
capacity and ramp-command bounds now have source-authored scalar constraints
and use that evaluator instead of Twin-specific Rhai predicates. The ramp
requirement prose was reconciled with its active 50 degree SysML datum; the
previous 0.58 rad text was stale.

This is an implemented migration slice, not production Editor acceptance. The
generic check path and Griffin constraints still need a focused live Rhai
check. Most Griffin mesh/layout predicates and requirement checks expressed
only in Twin Rhai remain to be migrated. The Twin-specific geometry-parameter
audit also remains separate from the generic requirement audit. The language
gaps below still block source-driven topology and aggregate constraints.

### 2026-09-25 typed feature-path update

The generic AST now preserves dotted `PATH_EXPR` navigation as an ordered
`FeatureChain`; the constraint IR carries resolved segments as typed,
snapshot-scoped handles through dependency collection, provider observations,
and Modelica input bindings. The Rhai API accepts complete typed paths for
general observations, and the scalar-parameter helper emits only dependencies
that the constraint actually uses. Provider diagnostics carry the authored
feature or constraint source location. Rust diagnostic identities use a
single `IrDiagnosticCode` catalog and serialize with stable `SYSML-IR-NNN`
report names. Feature-reference IR stores only typed snapshot handles;
qualified labels are derived from the source projection when a report needs
them. Constraint body projection assigns each expression statement to its
nearest owning constraint declaration, preventing nested constraint bodies
from being duplicated into a parent constraint's IR.

The affected crates compile with `cargo check`. This is compile evidence only:
no Rhai fixture or Griffin Editor/runtime gate has run, so actual path
resolution and provider behavior remain unverified. Optional `?.` access,
navigation through feature-valued collections, and collection-valued feature
evaluation remain unsupported.

### 2026-09-25 parameterized constraint-definition update

The AST now projects constraint declaration kinds and resolves function-style
actual arguments to owned `in`/`inout` feature handles. The neutral IR compiles
calls to source-projected `ConstraintDefinition`s by binding actual expressions
to typed formals and compiling the definition body in its own parameter scope.
The evaluator and Modelica lowerer apply those bindings; Rhai exposes the call
tree and constraint kind. Type, multiplicity, and unit mismatches, incomplete
bindings, recursive calls, non-Boolean bodies, whole-structured-value use,
computed-argument member navigation, and collection-valued member navigation
are rejected with source-linked diagnostics. Member navigation is supported
when a scalar structured formal is bound to a resolved source feature path; the
IR rebases that member path onto the call-site path and requests the resulting
typed provider observation.

The affected crates pass `cargo check`; no tests or Griffin runtime/Editor
session were run. Griffin currently authors reusable parameterized constraint
definitions, but the scanned requirement files use `require` membership and do
not contain function-style calls with actual argument bindings. Therefore this
generic capability is compile-verified only and is not yet an executed Griffin
verification path. Defaults, output binding, and general user-function
execution remain open.

### 2026-09-25 standard requirement-evaluation update

Requirement constraint membership is now projected with a Rust enum that
distinguishes standard `require` from `assume`. The neutral IR can compile a
constraint by its snapshot-scoped semantic handle and evaluate all `require`
memberships on one requirement, aggregating their four-state results. An
optional verification-case handle must resolve a `verify` relationship to the
same requirement. Rhai exposes this as `SysmlModel.evaluate_requirement`.
The generic IR also exposes an opt-in `SysmlModel.audit_requirements(policy)`
operation for duplicate short names, project-required identifiers and typed
subjects, verification coverage, missing-formal-constraint classification, and
unresolved names/verification targets. The Griffin requirements tool exposes
that audit as an explicit review operation; normal Twin startup does not run
it. A generic `sysml-audit` CLI now runs the same policy over one or more
SysML/KerML files or directories, with explicit policy flags and JSON output.
Identifier and typed-subject policies apply to requirement usages, where those
SysML properties belong; reusable definitions are audited for formal required
constraints separately. Verification coverage comes from the resolved
`verifiedRequirement` relationship in the semantic model. The authored
`lint.sysml` policy's verification checks now compare those same resolved
snapshot handles; written `verify` names remain display/source data rather than
identifiers used to decide coverage. The 154 informational findings are
definitions without a formal SysML `require` constraint, not failed or
unverified requirements: of 183 definitions, 29 have formal `require`
constraints and 154 do not; 113 of the 154 belong to Griffin, 37 to FLIP, and 4
to the Moon Base project. The engineering-review audit found resolved
verification links for all 183. That proves model traceability only; the audit
does not execute the mapped Rhai procedures or inspect fresh runtime evidence.
For example, GLL-006 has no embedded formal predicate, but the authored landing
leg scenario contains checks for each strut's type, radius, height, axis, and
placement. This is code presence, not a passing run. The CLI now names each
finding and separates formal predicates, resolved verification links, and
unexecuted external evidence. Quantitative geometry and clearance checks should
be formal constraints when they bind typed source values to composed USD
measurements; other measurable requirements can be accepted by a mapped,
executable verification procedure. Visual, mission-flow, runtime, provenance,
and evidence requirements need suitable verifier/evidence procedures instead.
The Twin's explicit quality audit uses the engineering-review policy without a
blanket formal-predicate requirement. It accepts a missing formal constraint as
informational and checks resolved verification links, but cannot distinguish a
link to an executable verifier from a procedure that has not run or produced
evidence. Unknown supplier data must remain explicitly provisional rather than
being turned into invented numeric predicates.

This closes generic membership selection and aggregate evaluation, not the
complete SysML binding semantics. The existing Griffin solar verification
scenario calls `sysml_requirements::required_constraints_check` for GSA-005,
GSA-006, and GSA-009; that shared adapter selects a source predicate and calls
`SysmlModel.evaluate_requirement`, which evaluates the full required set using
resolved requirement and verification handles. Provider observations for
Griffin's current bare memberships are still assembled from Rhai maps keyed by
parameter display names, with the qualified predicate name selecting which
binding shape to prepare. The AST now projects explicit feature-value bindings
on constraint usages through resolved redefinition handles, and the IR
substitutes supported typed actual expressions before requirement evaluation.
Defaults, output binding, general binding relationships, and
specialization/redefinition traversal remain unsupported. The explicit CLI
engineering-review audit was run over the Twin's 21 SysML/KerML source files:
it found no parser/name diagnostics and no policy errors. It reported 154
informational findings for definitions without formal SysML `require`
constraints; this policy does not require every qualitative requirement
definition to carry a formal predicate. The
grouped evaluator and bound-argument path remain compile-checked only: no tests,
authored scenario, actual-bound Griffin usage, authored `lint.sysml` policy
execution, or Editor/runtime verification were run in this batch.

### 2026-09-25 structured predicate-argument path update

The IR now preserves a resolved feature path when a constraint-definition call
passes a scalar structured feature to a formal parameter. Navigation through a
member is rebased onto the call-site path, and dependency validation/evidence
therefore uses the actual source feature path rather than a structured value
encoded by name. Whole structured-value evaluation, computed-argument member
navigation, and collection-valued navigation remain unsupported. The change
compiles, but no authored invocation or production verification was run;
Griffin still has no usage-site actual bindings for its solar predicates.

## Audit of the current Griffin model

The primary Griffin requirement/configuration sources contain many useful
engineering values and requirement usages. A small bounded scalar constraint
slice now exists for payload capacity, ramp command range, bus clearances, and
observed counts. The sources still lack the relationships needed to execute
the overall model. The current audit found no complete topology/constraint
library with `binding`,
`connect`, `flow`, `port`, `action`, `state`, `transition`, `satisfy`,
`refine`, or `derive` graph for the lander assembly. Geometry is primarily
represented by attributes, arrays, string IDs, USD paths, and explanatory
`doc` text.

That shape is valid as an early requirements/configuration baseline, but it is
not enough to drive a SysML constraint plan. In particular, the large
`_qualified_attribute(name)` mapping in `tools/griffin_spec.rhai` is evidence
that source identity and navigation are being reconstructed in policy instead
of coming from resolved feature membership. The requirements observer also
manually joins expected names and verification records. Those helpers are
useful diagnostics during migration, but they must not become Griffin's
permanent semantic layer.

| SysML/KerML capability | Griffin today | Consequence | Required generic foundation |
|---|---|---|---|
| Part usages, feature membership, roles, and multiplicity | Many `part def` and attribute declarations, but no complete lander usage graph with typed component roles and bounds | Arrays and parallel IDs can drift; policy cannot navigate an authored assembly | Resolved feature membership, usage identity, redefinition/subsetting, and multiplicity-preserving handles |
| Ports, interfaces, connections, and bindings | Values such as `sourceComponent`, `usdPath`, and frame/unit strings | Rhai manually joins endpoints and providers; no typed contract says which observation supplies a feature | Typed endpoint/feature chains and standard binding/connectors with source spans and target identity |
| Constraint definitions/usages | Reusable parameterized definitions exist; source projection distinguishes `require`/`assume`; the IR executes grouped `require` memberships by handle and applies projected feature-value bindings; Griffin's solar scenario calls the grouped path | Current Griffin usage members have no actual argument bindings, so providers still select values by qualified predicate and parameter display name. Defaults, output binding, and full specialization traversal remain | Complete default/output/general bindings and specialization traversal; derive provider observations from source-owned feature bindings where the model defines them |
| Feature navigation | Dotted `PATH_EXPR` projects to typed feature chains; feature-path arguments to predicate definitions rebase formal-member navigation onto the call-site path and its provider dependencies | The generic path behavior compiles, but has no authored invocation or production runtime evidence; Griffin providers still use parameter display names, optional navigation and collection-valued traversal remain unsupported | Author a Griffin source usage with resolved actual feature bindings, verify the exact path dependencies and stale/missing-value outcomes in the production observer, then extend collection navigation only for an authored need |
| Collections and aggregates | Parallel arrays and index assumptions | Index drift and hand-coded reductions for mass, COM, inertia, envelopes | Homogeneous scalar sequence literals, one-based indexing, scalar indexing, `size`/emptiness, `sum`/`product`, Boolean aggregates, and scalar `min`/`max` now compile/evaluate. Feature-valued and structured collections, collection navigation, filtering/selection, and reusable reductions remain open |
| Quantities and units | Many anonymous `Real` values and naming conventions such as `...M`, `...Kg`, `...Deg`, plus string `frameUnits` | Unit correctness is policy convention rather than a source-checked contract | Standard quantity kinds, unit literals/conversion contracts, dimensional checking, and frame/time metadata |
| Frames and realization | USD paths, component names, and frame information are strings | A value can be numerically valid but attached to the wrong prim/frame | Provider binding contract: source feature, provider (`usd`, `modelica`, telemetry, derived), target, frame, unit, and time validity |
| Requirement/verification membership | Required/assumed memberships and verify targets are represented by source-linked typed handles; grouped evaluation validates verification coverage | Assert/invariant/satisfy provenance and full source-to-provider evidence identity are incomplete; most Twin observers still use authored names | Complete the generic relationship graph and derive evidence from source identities rather than duplicated status catalogs |
| Applicability and behavior | Mission scenarios orchestrate states and timers in Rhai | Geometry/requirements cannot state when a constraint applies | Generic configuration/mode/phase/interval predicates, then standard action/state/transition semantics |
| Continuous realization | Modelica is generated/selected by Rhai without a source-level realization link | SysML intent, Modelica class, and result provenance can diverge | Typed realization/exhibit links and a result contract carrying source/model revisions |
| Reactivity | A source change does not identify all affected geometry, verification, and USD consumers | Stale manually assembled plans can survive a source edit | Dependency graph from source revision/fingerprint to compiled IR, provider plan, result, and evidence |

The handover's existing engineering-value and provenance work remains useful.
It should become the data carried by these typed relationships, not be
replaced by another free-form table or copied into Modelica/Rhai.

## Migration order

### P0 — establish one real Griffin constraint slice

Select one small, physically meaningful slice and author it through the
standard SysML source model rather than adding another Rhai relation table.
The existing segment/coincident-point fixture is the right mechanism seed;
the first Griffin slice should be one of:

- a tank/station placement with a typed station feature and frame;
- one landing-leg body/foot joint with endpoint features and a strut-axis
  relation; or
- one engine cluster datum and axis-alignment relation.

The slice must include a reusable relation definition, a usage with actual
parameter bindings, explicit units and frames, and a requirement/verification
link. The source constraint must compile to IR through
`sysml_constraint_ir(lunco://..., qualified_name)`, lower through Rumoca, and
produce evidence that names the source revision and IR fingerprint.

The acceptance condition is not merely “the Rhai script returns the expected
number.” It is that removing or changing the authored source feature changes
the resolved dependency/fingerprint and invalidates or recompiles the
affected plan. There must be no second copy of the relation in a Griffin
qualified-name map.

### P0 — replace string identity with typed provider bindings

For the selected slice, replace strings that currently mean different things
(`usdPath`, `sourceComponent`, station IDs, frame/unit text) with explicit
SysML features and provider bindings. The binding contract must distinguish:

1. the source feature and its resolved SysML handle;
2. the observation provider (`usd`, `modelica`, telemetry, or derived);
3. the provider target, such as a USD prim path;
4. the frame, unit, and time-validity contract; and
5. the observation state: available, unavailable, invalid, or stale.

`lunco://` is appropriate for source assets and scripts. It must not be used
to disguise a USD prim path: `/World/Griffin/LandingLegA` remains a USD path
inside a provider binding.

### P1 — implement the language mechanisms Griffin will immediately need

The current bounded IR handles typed scalar expressions, fixed primitive
arrays, dotted feature paths, and a recognized subset of standard-library
invocations. Griffin needs more before it can remove its main workarounds:

1. Runtime-verified navigation through part/feature usages, optional-access
   semantics, and collection-valued paths (typed dotted paths and exact
   provider identity compile, while runtime path resolution has not been
   exercised in Griffin);
2. default expansion, output/result binding, general binding relationships,
   specialization/redefinition traversal, and general user-defined
   function-body execution (explicitly bound calls to source-projected
   `ConstraintDefinition`s and grouped `require` membership evaluation are
   implemented; actual bindings compile but have not been exercised by a
   Griffin usage);
3. feature-valued and structured collection construction, collection path
   navigation, filtering/selection, multidimensional indexing, and reusable
   reductions for station, leg, engine, mass, COM, inertia, and envelope sets
   (homogeneous scalar sequences, one-based indexing, `size`, emptiness,
   `sum`, `product`, Boolean aggregates, and scalar `min`/`max` are supported);
4. dimensional quantity/unit checking and conversion contracts, with no
   inference from suffixes or bare `Real` values;
5. null/invalid/error semantics that preserve an inconclusive or invalid
   observation instead of manufacturing zero;
6. constraint membership/provenance for requirements, assertions, assumptions,
   verification, and satisfaction; and
7. applicability/configuration/mode/phase/interval semantics before using
   temporal or behavioral constraints.

These are generic AST/IR/provider capabilities. Adding Griffin-specific Rust
for any one of them would make the current workaround permanent and would not
be interoperable with another Twin.

### P1 — build the Griffin topology before increasing equation complexity

After the first slice, author the actual assembly graph: lander body, deck,
tanks, engines, legs, ramp, payload interface, avionics, and rover boundary.
Use typed part usages, role names, multiplicities, ports/interfaces, and
connections where the SysML semantics require them. Then add constraints in
small groups:

- tank placement and station datum relationships;
- leg symmetry, pivot locations, foot contact/support datum, and strut axes;
- engine axes, cluster datum, deck/skirt clearance, and plume keep-out;
- ramp rail/hinge/clearance relations; and
- payload and rover interface compatibility.

The landing foot must be a typed shape/interface choice with a real load
interface. A cylinder renamed as a pad, or a coordinate copied into a script,
is not a SysML model of the hemispherical contact requirement.

### P1 — connect continuous realizations deliberately

Only after the geometric slice is source-driven should Griffin add mass,
centre-of-mass, inertia, load cases, propulsion, power, and thermal equations.
Those equations belong in Modelica/Rumoca, but their parameter provenance and
realization identity belong in SysML. The Modelica source/result must carry the
SysML source revision and constraint fingerprint; otherwise a numerically
successful solve can still be a solve of stale or wrong intent.

### P2 — add mission behavior and evidence semantics

Mission states, transitions, actions, and temporal validity should be added as
standard behavioral/applicability semantics once the structural graph is
usable. Rhai can continue to own orchestration policy and evidence formatting,
but it should consume source-defined applicability and observation states rather
than encoding the mission state machine as an unrelated timer script.

## Workarounds to remove

The following are migration targets, not contracts to preserve:

- `_qualified_attribute` and similar hand-maintained qualified-name tables;
- parallel arrays whose indices encode an unstated relationship;
- string fields that stand in for typed features, ports, frames, or bindings;
- requirement checks that select expected names instead of traversing standard
  requirement/verification membership;
- `doc` prose that is the only expression of a geometric or physical relation;
- Rhai-generated Modelica that has no source-level realization/fingerprint;
- manual state/timer applicability where a standard state/phase/interval
  relation is required; and
- copied dimension tables or duplicated geometry values in Rhai/Modelica.

During migration, the existing helpers may report exactly which legacy fields
are still being used. They must fail closed when the typed source relationship
is absent; they must not silently fall back to a string or a guessed default.

## Definition of done for the first Griffin migration

The first slice is ready to expand when all of the following are true:

- its SysML source contains actual typed features/usages and a reusable
  constraint usage, not only attributes or `doc` text;
- `lunco-sysml-ast` resolves its identity and relationship ends without a
  Griffin-specific parser branch;
- `lunco-sysml-ir` produces a deterministic, source-linked, typed plan;
- Rhai selects provider bindings and policy without reconstructing qualified
  names from a hand-maintained map;
- Modelica/Rumoca compilation or USD observation failure is explicit and
  produces an unavailable/invalid/inconclusive result as appropriate;
- the result carries requirement, verification, source revision, and IR
  fingerprint provenance;
- a changed SysML value causes the affected plan to recompile/re-evaluate;
- the visible USD proposal is checked against its authored local frame before
  application; and
- the production Rhai/scene gate proves both a valid result and a negative
  diagnostic path.

This is the practical boundary for using the new tool with full SysML power:
first make the missing standard semantics generic, then make Griffin author
those semantics. Do not use the current bounded IR as a reason to encode a
second, Griffin-only language in Rhai.
