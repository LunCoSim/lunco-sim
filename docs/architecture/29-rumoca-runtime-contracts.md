# 29 — Rumoca runtime contracts

LunCoSim uses Rumoca 0.10.2 with one dependency revision across compilation,
AST tools, lowering, simulation, and native execution. The LunCoSim fork supplies
compile source-closure provenance, prepared FMI session construction,
cooperative evaluation cancellation, and scoped retained-output admission.

## Compilation and source identity

`ModelicaCompiler` owns user overlays and admitted source roots. Source enters
unchanged through the shared parser and compilation session. The compile result
carries the exact reachable file closure and source-set keys used for content
identity. Invalid or unbalanced models stop at the compiler's phase diagnostic;
unchecked DAE construction is not a diagnostic or runtime entry point.

External input declaration defaults retain their qualified identity. Internal
and connected bindings retain equation ownership. The production
`modelica_scoped_input_defaults.rhai` scenario covers live values, overrides,
reset, and batch execution. `modelica_aggregate_output.rhai` covers outputs
consumed as state derivatives and invalid source rejection.

Native batch compilation submits `CompileBatch` through the existing worker
source-root admission lane to the same Rumoca actor used for live models. The
bounded artifact cache includes library generation and retains the exact source
CID with its checked DAE. Copies preserve package scope through `within`;
Rumoca owns parent-import resolution and validates missing references; a
standalone `within` namespace is not an external library admission requirement.
`modelica_duplicate_lifecycle.rhai`
exercises copied classes with commented, concatenated descriptions and a type
alias declared only in the admitted parent package, then rejects a missing type
without reusing the completed trajectory or crashing the host.

## Declaration dimensions and connector presentation

Authoring `DescribeModel`, experiment setup, and parameter rows consume Rumoca's cached
`ClassComponentDeclaration` query. The session owns lexical/import lookup,
inherited nested types, package aliases, exact qualified type identity, effective
causality, primitive type,
and type-alias dimensions. Authored expressions and source coordinates remain
separate from those type facts. Missing or cyclic type information is a visible
query rejection, never a name-based signal classification.

`scalar_type` and `has_array_dimensions` describe the admitted declaration and
its resolved type. Numeric run text uses the shared `parse_scalar_run_value` parser
against the primitive enum. Empty text omits an override; malformed, non-finite,
aggregate, array, and String values reject scalar admission. The compiler and
solver retain final tunability ownership. Fast Run retains its setup dialog
and entered text when validation rejects a value; the existing draft stays
unchanged. Session revision invalidates the bounded declaration cache;
repeated UI reads reuse its immutable result. Adopting another session's cache requires full
source-signature validation, even when their revision numbers match.

The session package-definition map records its validated session revision.
Unchanged declaration queries reuse it without rebuilding the whole-library
source signature. After a revision changes, the existing dependency signature
validates reuse or triggers reconstruction. The maintained Rumoca revision
supplies this cache contract, the base-class lookup context described below,
and immutable shared class interfaces.
Native editor indexing retains the real loading state until metadata publication
completes; clearing a progress indicator does not complete that work.

The shared declaration lookup distinguishes member types from base-class names.
Base names are resolved before expanding the current class's inherited contents
(MLS 5.6.1.4), while inherited types in enclosing scopes remain visible. Recursive
base lookup uses this same context; sibling `extends` clauses do not recursively
resolve each other. Mutable session and snapshot readers share the lookup owner.
Missing bases and inheritance cycles still reject declaration capture.
Cached class interfaces share immutable facts across scope reads and snapshots;
lookups do not deep-clone the interface's members or nested headers. The existing
file fingerprint invalidates edited facts, and retained snapshots stay isolated
from the replacement.

The native indexer parses each source once. Its flat class inventory moves nested
declarations out of the one AST copy, rather than cloning descendants at every
package level. Documentation comes from each qualified class's own annotation;
there is no second documentation parse or short-name documentation map. Qualified
class order is stable. Paired source/AST documents remain complete for session
installation and the persisted parsed bundle.

Canvas projection captures an immutable `DiagramSnapshot` from the shared
workbench engine's Rumoca session. The snapshot contains the selected class's
cached inheritance-merged declarations, exact resolved component and connector
identities, effective causality, flow members, and presentation annotations.
Capture takes an explicit `DiagramScope`. Canvas consumers request
`ComponentGraph`; the source-library indexer requests `ClassInterface` from the
same declaration owner. Interface capture retains the target's declarations,
public connector facts and merged target icon, without internal component graphs,
internal icons or equations. Each indexed class retains its own interface
resolution diagnostic; graph projection and compilation validate their required
bodies independently. The indexer admits the paired parsed source set without
reparsing.
Unresolved indexed declarations carry an explicit diagnostic state.
Graph projection performs no type-name classification, package guessing,
source-library parsing, or independent inheritance lookup. Missing types and
cyclic declarations reject snapshot capture visibly; source-root completion
requests a new projection. Admission selects the bundled-package inventory or
the installed source-library bundle; an absent root stays a visible failure.
A complete current snapshot does not request unrelated library roots. Document generations still reject superseded work.

The add-component palette filters Modelica's typed `partial` prefix. Namespace
spelling does not classify a component as decorative. Drill-in tabs start in
diagram view and retain the user's selected view when their document loads.
Document installation does not run semantic queries on the UI thread.

Inherited icon projection receives each base identity from Rumoca's scoped type
candidates and exact class lookup. The annotation layer merges graphics and
coordinate systems only; it does not expand imports or construct scope paths.
Boolean visibility defaults are restricted to scalar Boolean parameters.
The background diagram snapshot projects and caches icons from those resolved
declarations; document opening does not schedule a separate type-name warm walk.

Block and expanded connection graphs consume the same declaration ports. Wires
come only from the selected class's equations; authored route annotations decorate
those edges. Connector icon lookup uses the exact resolved identity. Parameter label units
come from declaration metadata; type-name unit tables do not participate. A class
without authored placement uses the configured deterministic grid; a class
without an icon uses the renderer's named component presentation. These are
presentation defaults and do not supply missing semantic facts. A conditional
source declaration is marked as conditional, including a literal `if true`;
Rumoca owns condition evaluation and compiled instance presence. The diagram
performs no Boolean-expression interpretation or active-instance inference.

Dynamic icon references read only the component's qualified runtime path. The
Modelica built-in `time` reads the participant clock or selected experiment
sample time. A missing component value cannot read a similarly named root value;
live values replace the experiment snapshot when a participant is present.

Canvas wires consume the projected `PortKind`. Rumoca's short-form class
causality supplies signal connector direction, and resolved flow members supply
acausal ports. `modelica_declaration_shapes.rhai` and `modelica_type_prefixes.rhai`
check public declaration facts. Inline snapshot/projection tests cover exact
port names, inherited flow metadata, aliased direction, source scope, negative
capture, and session-revision invalidation.

The source-root owner loads the native declaration index off-thread and registers
its exact declared namespaces for the installed library. Browser index readiness
publishes the same registration. Library admission installs paired parsed records
through the ordered worker channel without reparsing. Provisioning and index
generation remain at the native workbench adapter.

Experiment admission uses the same source-root registry and ordered load channel as interactive compilation. Required roots are queued before the immutable batch job; unknown or failed roots reject admission before registering a run.

## Prepared execution and persistence

Compiled DAE reuse is keyed by the exact selected target, document URI,
primary and sibling source bytes, and source-root revision. Prepared solver
reuse uses the compiler's exact participating-source CID, excluding unrelated
siblings and transient document paths. Generated and authored declarations use
this same content contract. Identifiers, strings, annotations, and numeric
literals remain unchanged throughout source admission and cache identity.

The worker prepares one checked `FmiComponent`, borrowing it for live session
construction. Its FMI inventory and executable model come from the same
correlated lowering. Prepared cache version 13 stores the exact clean compiler
build identity and canonical typed FMI
construction wire through a bounded JSON persistence adapter and zstd. Loading
replays Rumoca's checked construction; unsupported versions, malformed metadata,
identity mismatches, and oversized records are rejected before use.

Run inputs and tunable parameters enter `SimOptions::initial_inputs` and
`param_overrides`. Rumoca validates targets and recomputes initialization and
parameter dependents. The compiled DAE stays immutable across sweep points. Reset carries current
admitted inputs explicitly across the worker boundary, including values changed
while the participant is paused.
Native and browser workers apply every admitted input through Rumoca's checked
session API. Input rejection, integration failure, incomplete advancement, and
observation failure terminate the step without publishing values or a stream
snapshot. The live solver boundary requires finite time and visible values;
invalid observations produce a `modelica-step-failed` runtime fault. Initial
observations are checked before compile, reset, or parameter-update success.
The source-library bundle stores `ParsedSourceDocument` with expanded source
text and syntax together. Admission uses `replace_in_memory_source_set`;
source spans and portable content identities address those retained bytes.
The native indexer writes the same paired-source record shape. Empty source sets
and read or parse failures reject admission before publishing artifacts. The CLI and native workbench provisioning adapter invoke the same indexer;
compiler and UI hosts consume its generated artifacts.
The artifact tag is `rumoca-0.10.2+bincode2-ast3`. Native and browser decoders require
complete records and full byte consumption; a partial bundle is never installed.
Class documentation reads each exact parsed declaration and consumes Rumoca's
decoded strings once. Equal short names in different packages retain their own
text; indexing does not reparse sources or decode escapes again.
Declaration locations include qualifiers and terminating semicolons. Artifact
consumers require this span contract before using source ranges.
The packager admits every source before publishing either artifact; read or parse
errors abort publication with the source identity.

## Solvers and clocks

`solver_backends` registers BDF for offline implicit integration, RK45 for
adaptive authoritative live integration, and fixed RK4 for deterministic
prediction of qualified continuous, event-free models. A requested solver must
resolve to a registered capability; unsupported selections are terminal errors.

Live sessions have no stop time. Their finite options span supplies the
root-scan scale. Batch runs use the admitted `RunBounds` and canonical output
grid through Rumoca's checked batch solver in both native and browser hosts.
Live co-simulation uses the separately admitted persistent participant session.
Its demand and communication endpoints come from `TickClock` on the physics
master's integer lattice, with the default exchange every master interval.
Explicit slower exchanges are validated integer multiples. The worker receives
derived start/stop seconds and returns its actual accepted solver coordinate;
transaction validation never fabricates a corrected output timestamp. See
[architecture 19](19-unified-time-and-clock.md) for pause, reset and speed semantics.
Fixed RK4 applies exactly the configured step lattice and rejects models
with unsupported event, discrete, clock, or external-table constructs.

A scoped cancellation guard is observed by the evaluator and ME session loop.
A scoped output budget admits grid dimensions before allocation and every
recorded trace append, including event observations. Guards are thread-local,
restore nested callers, and cannot migrate between worker threads.

## Source-preserving editing and lint

Structured document edits splice the original source spans; untouched bytes,
comments, and formatting are preserved. Only new nodes use the local emitter.
Connect annotation routing has one parser-backed source reader in
`lunco-modelica-index::annotation_source`.

Rumoca lint jobs capture the document’s source generation with its text.
Completed jobs publish only for that same current source generation; parser
channels remain pending while the syntax cache is stale. Publication tracks
source generation, AST generation and lint revision independently.

Parse-time `lint.modelica` policy checks declaration conflicts. Conditional
expressions and events are compiler/solver capabilities, not blanket syntax
errors. Runtime acceptance uses authored Rhai through the production binary.

The `modelica_editor` facade consumes native source and compile query data
through its shared query-result predicate. Inspection combines both current
facts; `authoring_session::checkpoint` requires a ready compiled model before
reporting verification. Invalid current source and retired documents remain
visible rejections. The production `modelica_editor_checkpoint.rhai` gate covers
that consumer and its negative cases.

## Ownership review

The reviewed release is upstream `v0.10.2` (`276e82bd`); upstream main was
`0b249f0f` on 2026-10-11. The pinned fork also includes upstream's
[external-function binary cache fix](https://github.com/CogniPilot/rumoca/commit/4afaf0af02dc1fbce0a189771434de49b27bab20).
Parsed AST fields must be serialized completely for positional readers.
The Diffsol backend directly depends on the corrected source retained in the
pinned Rumoca repository. Downstream Git consumers receive its accepted-state,
minimum-step, Newton-convergence, and failure-budget corrections through that
dependency edge. The application carries no separate solver patch or sampler
endpoint substitution.

| Mechanism | Authoritative owner | LunCoSim responsibility |
|---|---|---|
| Expression rendering | `rumoca-ir-ast::Expression` Display | Use its complete representation in component modifications, indexer defaults, and canvas labels. |
| Complete declaration extents | Rumoca parser `ClassDef.location` | Slice the original source after range validation; use name tokens separately for renaming. |
| Qualified source names | `rumoca_core::qualified_name_segments` and `StoredDefinition` declaration queries | Preserve quoted identifiers and exact package boundaries without a host name parser. |
| String literal decoding and encoding | Rumoca parser tokens and `rumoca-core::escape_modelica_string` | Copy decoded values into typed metadata; use canonical escaping when inserting new source nodes. Descriptions concatenate decoded tokens without adding whitespace. |
| Checked DAE, initialization, parameter and input binding | Rumoca compiler and `SimOptions` preparation | Supply the complete authored source closure and admitted causal inputs. |
| FMI preparation and ordinary sessions | `rumoca_sim::prepare_fmi_component` and `SimulationSession::new` | Cache checked components and select registered live solver capabilities. |
| Live scalar units, descriptions, and bounds | `FmiComponent::variables()` and `FmiVariable::scalar_names()` | Carry exact compiled facts through worker admission; keep USD provenance and channel retention at their application owners. |
| Retained trajectory units and descriptions | `SimResult::variable_meta` and `rumoca_sim::build_variable_meta` | Store exact per-run series facts with completed artifacts; graphs consume retained facts. |
| Component initialization, input transactions, algebraic refresh and observations | Rumoca Model Exchange host (`MeRetainedComponent` / `MeSimulationSession`) | Both adaptive and fixed sessions use this owner. Fixed RK4 supplies its numerical plugin through the retained derivative capability; initial equations run before the first observation. |
| Fixed prediction step lattice | `lunco-modelica-solver::FixedStepSession` | Keep exact communication steps and reject unsupported event/discrete/clock/table models. |
| Source edits and diagram projection | Modelica document/AST/index owners | Splice exact authored spans, preserve untouched text, and project typed presentation facts. |
| Co-simulation and lifecycle | LunCoSim cosim/worker owners | Admit causal samples in stable wire order, retain typed execution identity, and retire Twin-owned work. |

`within` places declarations in their native Rumoca namespace; it does not
require a separately registered source library. Source-root admission consumes Rumoca’s
`compile::source_root_dependencies` query. Rumoca resolves lexical scopes and
imports, including aliases in nested classes; LunCoSim only maps the resulting
external namespaces to its library registry. Structural component edits reparse
through the document owner; projection and compilation consume that admitted
source without a per-component file loader. The production
`modelica_class_compile.rhai` gate covers qualified and default run targets
inside an authored `within` namespace and an imported unit alias.

Plain package/type/function documents have no automatic simulation target.
An explicit class selection or a runnable model/block/class is required.
USD source admission rejects a missing target before sending worker compilation
and retains the terminal diagnostic on the shared port surface. Source edits
to standalone USD programs reproject their owning subtree; the worker teardown
observer retires the installed solver and observations before replacement
admission. Rejected source configuration also removes an installed model through
that same teardown boundary.
Library-only web documents open for inspection without a solver participant.
Generated networks compile the exact root name already validated by synthesis;
they do not select another class from the source-wide interface. The production
`modelica_source_target.usda` runs its attached `modelica_source_target.rhai`
gate through source rejection and live replacement, including construction
profile availability and retirement.
Native construction can reuse relocatable Cranelift artifacts through Rumoca's
explicit thread-local cache scope; LunCoSim owns budgets, asynchronous persistence
and public accounting. See the [cache ownership contract](20-domain-modelica.md)
for identity, failure and lifetime boundaries. This scope retains no solver state.
The workspace pins Rumoca revision `f8df42b448c31273d94926183dac2fffeccef9d0`
from `LunCoSim/rumoca`; normal builds require no local dependency override.
Precompile source interfaces project only direct declarations of the selected
class: parameter/input literal bindings, explicit causal ports, and authored
documentation. Generated synthesis requests its exact validated class through
Rumoca's qualified AST lookup. Sibling and nested class declarations never enter
that interface. Source-root requirements still cover the complete definition;
checked DAE/FMI facts own inheritance, aliases, evaluated defaults, and live
variable metadata.
Live telemetry units, descriptions, and input bounds come exclusively from the
checked FMI scalar inventory. Construction carries that inventory through the typed worker
result alongside the installed solver plan. Session and source-generation fences
apply before publication; a successful lifecycle result without its inventory is
a terminal protocol diagnostic. Ordinary steps carry no repeated inventory.
The runtime retains one immutable metadata map; telemetry observes its identity
and publishes first or changed channel facts. Draft document changes cannot
replace the facts of an installed solver. Generated USD layouts retain authored
provenance, grouping, exposure, and precompile port facts; they do not supply a
second live unit/description reader. Undocumented declarations remain absent,
even for connector-shaped names.

Input controls read per-scalar minimum and maximum values from the same checked
FMI inventory. They do not parse bundled source to repair missing bounds.
`SnapshotVariables.metadata` exposes exact checked scalar facts alongside values
and applies the same optional name filter. Draft documents remain the owner of
pre-compile authoring facts; the UI does not overwrite compiled channel metadata.
Optimistic authoring edits wait for parsed metadata to reconcile.
`DescribeModel` numeric defaults include Rumoca's semantic start value when no
binding is authored. Input-binding admission and lint facts separately retain
the absence of an authored default; a start guess is not a causal input binding.
An authored non-literal binding stays unresolved in numeric authoring metadata;
it never falls through to a start value.

### Candidates for further upstream ownership

- A persistent fixed-step Model Exchange backend could move the generic RK4
  loop into Rumoca. The released `fmi_me::fixed_step::fixed_step_rk4` is a
  finite batch driver starting at zero and recording each step; it does not
  provide the open session, changing input, or step-lattice API used here.
  `MeIntegratorBackend` is the appropriate extension boundary. Solver selection,
  prediction admission, and ECS synchronization remain application-owned.
- `Equation::Connect` should retain its authored annotation in the parsed AST.
  The inspected release and main store only endpoints. The current source-span
  annotation reader and source-preserving edit path remain necessary until the
  parser represents that field.
- The fork's prepared FMI borrowing/replay, exact compile source provenance,
  cancellation, and retained-output admission should be upstream contracts.
  The inspected main does not expose the equivalent public construction and
  guard surface consumed by this host. Their owning Rumoca crates remain the
  correct implementation location.
- Upstream main exposes trajectory sensitivities, adjoint objective gradients,
  and A/B/C/D linearization. New numerical-analysis tools should consume those
  checked Rumoca APIs when that revision is admitted; application code should
  supply experiment orchestration and typed result presentation.

Verification covers decoded strings and complete expression metadata, source
index reconciliation, fixed-step/algebraic math, and deterministic input
schedules. The production metadata scenario observes decoded authoring facts through
`DescribeModel`; controller and scoped-input scenarios exercise live
initialization/reset through the worker. `modelica_compiled_metadata.rhai`
checks repeated leaf names, inherited array units, scalar bounds, undocumented
connector-shaped names, reset, unchanged compiled facts during draft edits,
replacement after recompilation, and live participant retirement on scratch
closure. Browser compilation is a separate
check from browser runtime acceptance.
`modelica_run_metadata.rhai` checks distinct repeated leaf names, inherited
array units, absent units, query filtering, and immutable facts through draft
changes and a successor run. The artifact codec check retains those facts
through bounded persistence admission.
The production `modelica_observation_failure.usda` gate declares the expected
terminal fault when a zero-state model's output becomes non-finite. A low-level
worker test checks unknown and non-finite input packets before integration;
the public command surface rejects those packets before they reach the worker.
