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
check. Most Griffin mesh/layout predicates, the custom parameter audit, and
requirement checks expressed only in Twin Rhai remain to be migrated. The
language gaps below still block source-driven topology and aggregate
constraints.

### 2026-09-25 typed feature-path update

The generic AST now preserves dotted `PATH_EXPR` navigation as an ordered
`FeatureChain`; the constraint IR carries resolved segments as typed,
snapshot-scoped handles through dependency collection, provider observations,
and Modelica input bindings. The Rhai API accepts complete typed paths for
general observations, and the scalar-parameter helper emits only dependencies
that the constraint actually uses. Rust diagnostic identities now use
`IrDiagnosticCode` and serialize with their stable `SYSML-IR-NNN` report names.

The affected crates compile with `cargo check`. This is compile evidence only:
no Rhai fixture or Griffin Editor/runtime gate has run, so actual path
resolution and provider behavior remain unverified. Optional `?.` access,
navigation through feature-valued collections, and collection-valued feature
evaluation remain unsupported.

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
| Constraint definitions/usages | Relations live in Rhai mechanical calls or prose; no reusable source constraint library | A change to SysML values does not recompile a source-selected constraint plan | Standard-library scalar/collection invocations now compile to typed IR; reusable constraint usage membership, user-function bodies, defaults, and general result binding remain required |
| Feature navigation | Dotted `PATH_EXPR` now projects to typed feature chains and exact dependency paths; compile-checked, not runtime-verified | Griffin source and providers have not yet been migrated to consume full typed paths; optional navigation and collection-valued traversal remain unsupported | Runtime-verified standard feature navigation, collection-aware path typing, and explicit unavailable/invalid propagation |
| Collections and aggregates | Parallel arrays and index assumptions | Index drift and hand-coded reductions for mass, COM, inertia, envelopes | Homogeneous scalar sequence literals, one-based indexing, typed `sum`/`product`, size predicates, and Boolean aggregates now compile/evaluate; feature-valued, structured/N-dimensional collections and reductions remain required |
| Quantities and units | Many anonymous `Real` values and naming conventions such as `...M`, `...Kg`, `...Deg`, plus string `frameUnits` | Unit correctness is policy convention rather than a source-checked contract | Standard quantity kinds, unit literals/conversion contracts, dimensional checking, and frame/time metadata |
| Frames and realization | USD paths, component names, and frame information are strings | A value can be numerically valid but attached to the wrong prim/frame | Provider binding contract: source feature, provider (`usd`, `modelica`, telemetry, derived), target, frame, unit, and time validity |
| Requirement/verification membership | Requirement text and verification IDs are present; checks are manually registered | Evidence cannot be derived from the constraint's standard membership/provenance | Requirement/constraint/verification graph with `satisfy`/`verify`/`assume`/`assert` provenance |
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
   semantics, and collection-valued paths (the typed dotted-path representation
   and exact provider identity now compile);
2. reusable constraint definitions/usages, user-defined function-body
   execution, default expansion, argument binding, direction, and result typing
   (the current typed `Invocation` supports only recognized standard-library
   calls with explicit arguments);
3. feature-valued and structured collection construction, multidimensional
   indexing, and reductions for station, leg, engine, mass, COM, inertia, and
   envelope sets (homogeneous scalar sequences and one-based indexing now
   compile through the IR);
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
