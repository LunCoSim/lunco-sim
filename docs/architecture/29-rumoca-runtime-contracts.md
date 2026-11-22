# 29 — Rumoca runtime contracts

LunCoSim pins one Rumoca revision across compilation, AST tools, lowering,
simulation, and native execution. Rumoca owns Modelica parsing, compilation, DAE construction, initialization, and
numerical integration. LunCoSim uses Rumoca public queries and
`SimulationSession`, with application-specific metadata, output grids, result
budgets, and lifecycle policy kept in its owning crates.

## Compilation and source identity

`ModelicaCompiler` owns user overlays and admitted source roots. Source enters
unchanged through Rumoca's public `ParsedSourceDocument`. LunCo keeps the
serializable `SourceFileSnapshot` beside that source-bearing Rumoca document and
admits roots through `replace_in_memory_source_set`; Twin and other virtual URIs
therefore retain the exact text addressed by parser spans. The serialized
library bundle remains a source snapshot, so source preparation rebuilds the
Rumoca parsed document from its retained bytes before session admission.

Rumoca's DAE result source map covers session documents. Compile identity keeps
only source IDs present in the flattened target's declarations, equations,
records, algorithms, and functions, plus Rumoca's generated structural-override
sources. It hashes exact source-map bytes and external table values. Invalid or
unbalanced models stop at the compiler's phase diagnostic; unchecked DAE
construction is not a diagnostic or runtime entry point.

Disk source discovery and source-root document/rename facts retain native
absolute paths, including Windows verbatim drive and UNC prefixes. The compiler
strips the native root before converting relative components to `/` for the
portable content closure. Native addresses remain available to diagnostics and
Modelica resource resolution; logical identities never include a machine root.
`source_root_content_closure_preserves_native_discovery_paths` exercises this
boundary with discovered disk sources and Windows drive/UNC spellings.

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

For an authored binding, the LunCoSim AST adapter asks Rumoca for the named
member at each segment of the binding path. It projects that member with the
same declaration logic used by the full editor inventory. The checked DAE
validates the resulting instance bindings.

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
installation and the persisted source-snapshot bundle.

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
publishes the same registration. Source preparation pairs each snapshot with a
Rumoca `ParsedSourceDocument`; the ordered worker channel installs those
source-bearing documents without reparsing the prepared input. Provisioning and
index generation remain at the native workbench adapter.

Experiment admission uses the same source-root registry and ordered load channel as interactive compilation. Required roots are queued before the immutable batch job; unknown or failed roots reject admission before registering a run.

## Compiled artifacts and simulation sessions

Compiled DAE reuse is keyed by LunCo's `source_unit_identity`: selected target,
document URI, primary and sibling source bytes, and the ordered content of all
admitted source roots. The key also includes the exact numeric parameter bits,
clean Rumoca build identity, operating system, and architecture. Current Rumoca
main returns its exact participating-source closure with the compiled DAE, so
the persistent lookup uses the already-admitted source unit before lowering;
an unrelated source or root edit can conservatively invalidate that DAE entry.
Compiled-artifact reuse uses the admitted source identity and numeric structural
values; persistent records also bind the clean Rumoca build and host platform.
Generated and authored declarations use the same source-admission boundaries.
Identifiers, strings, annotations, and numeric literals remain unchanged
throughout source admission and cache identity.

Interactive reuse additionally checks the exact document runtime leases,
candidate source generations and URIs, admitted source-root operations and requested
class. Compile, Run, status queries and deferred installation consume that same
source snapshot. A sibling edit invalidates reuse even when the primary
document's generation is unchanged.
Compile admission waits for the primary and every participating sibling's
current syntax and AST generations before using class names or dependencies.
Those facts and compiled bytes come from the same captured source unit;
generated units retain their deliberate exclusion of editor siblings.
Library source sets contribute their load/unload operation identities.
Document-backed namespace registrations without a worker operation are parser/editor facts; their source
documents are already fenced by exact leases, generations and URIs. Publishing
or retiring those namespace facts cannot invalidate otherwise unchanged source.
Any real nonzero library operation remains fenced, including document-backed loads.
A generated unit captures its own document and library operations, with no
editor sibling overlays.

The worker retains the checked DAE and constructs a separate public Rumoca
`SimulationSession` for each participant. Session initialization, input handling,
state observation and integration remain owned by Rumoca; solver selection,
worker placement, typed snapshots and lifecycle fencing remain owned by LunCoSim.
The compiler adapter projects scalar metadata from checked DAE views and the
shared numeric evaluator. Persistent storage retains checked compiled artifacts;
each participant owns its own initialized session and numerical state.

Rumoca's immutable `ScalarProgramBlock` derives logical-output ownership from
the same checked program/output bindings used by its readers. Sparse and
repeated output identities retain their first stored occurrence. Clones share
the index, and checked wire replay reconstructs it without adding serialized
fields. The application consumes that owner directly; it maintains no parallel
output-to-program map.

Run inputs and tunable parameters enter Rumoca's checked `SimOptions` fields.
The compiled DAE stays immutable across sweep points. Reset carries current
admitted inputs explicitly across the worker boundary and builds a fresh public
`SimulationSession` from the retained DAE. Numerical state is never shared
between participants.

The LunCoSim solver adapter derives its `parameter_values` view from the
admitted checked DAE through Rumoca's `NumericEvaluator`, including expressions
and inherited values. The numeric editing inventory excludes String
declarations. Source edits validate checked scalar type, visibility, finality
and bounds before changing either source or session. Reset refreshes that view
from its own checked DAE and admitted overrides.
Pause and Resume record terminal applied/rejected outcomes and replace the
document's exact-source-leased run intent, including before a compile creates
its participant. Compile and Reset installation consume the latest intent.
Paused intent does not withdraw an already admitted Step: its exact in-flight
receipt independently retains the shared-clock barrier until admission or fault.
Restart owns one Reset completion, requests running, and reports the exact
installation or failure; a subsequent Pause or Resume supersedes that intent.
Run delegates compilation with its original typed command context and external
correlation, so the existing lifecycle completion reports initialization success
or failure to the original caller. External ambiguity rejects before opening a
picker. Compile admission preserves arrival order and only coalesces consecutive
identical source/participant/class requests, retaining every listener.
Parameter source edits replace the complete expression span supplied by Rumoca;
token locations identify only an anchor and cannot define the replacement range.
Modelica `SaveDocument` and `SaveAsDocument` record terminal owned write/download
results. Picker admission reports `picker_requested: true`; the follow-up save
owns its write result. Failed writes preserve origin and saved-generation state
and publish `modelica-save-failed` through the shared status bridge.
Native and browser workers apply every admitted input through Rumoca's checked
session API. Input rejection, integration failure, incomplete advancement, and
observation failure terminate the step without publishing values or a stream
snapshot. The live solver boundary requires finite time and visible values;
invalid observations produce a `modelica-step-failed` runtime fault. Initial
observations are checked before compile, reset, or parameter-update success.
Rumoca's Model Exchange host exposes checked output values with applied-input
overlays and verifies output cardinality. The fixed-step adapter reads that
public `visible_values` map and implements LunCo's selected-value lookup over
it. Output labels resolve at host construction; applied inputs are overlaid by
the host and retire with the session. The live stepper validates finite time
and every value before publishing typed worker rows. Names remain text on the
external worker wire, and the reflected variable snapshot retains String keys.
Native projection uses Rumoca's `TornTangentPreparation` owner to bind one JVP
source and share temporary seed/output/coefficient storage across its block
evaluators. Each invocation retains its exact logical seed shape and independently
owned Jacobian result. Workspace contents are not simulation state; the native
[workspace contract](../../../rumoca/docs/dev-guide/src/runtime/simulation-runtime.md#torn-tangent-workspaces)
defines clearing, nested evaluation and teardown at that owner.
The serialized source-library bundle stores `SourceFileSnapshot` records with
expanded source text and syntax together. The compiler source-preparation worker
rebuilds Rumoca `ParsedSourceDocument` values from those bytes and admits them
through `replace_in_memory_source_set`; source spans and portable content
identities address the retained text, including virtual Twin URIs. The native
indexer writes the source-snapshot record shape. Empty source sets and read or
parse failures reject admission before publishing artifacts. The CLI and native
workbench provisioning adapter invoke the same indexer; compiler and UI hosts
consume its generated artifacts.
The artifact tag is `rumoca-0.10.4+bincode2-ast4`. Native and browser decoders require
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
adaptive live integration, and fixed RK4 for predicted models. A requested
solver must resolve to a registered capability; unsupported selections are
terminal errors. LunCoSim maps the selected capability to Rumoca's `SimOptions`.
For prediction, it selects `SimSolverMode::FixedStep` on the same public
`SimulationSession` path. Rumoca owns the fixed-step integrator, session
initialization, input transactions, component evaluation and event handling.

Live sessions have no stop time. Their finite options span supplies the
root-scan scale. Batch runs use the admitted `RunBounds`; LunCoSim builds and
budgets the canonical output grid, then advances Rumoca's public session to each
observation in both native and browser hosts. Live co-simulation uses the
separately admitted persistent participant session.
Its demand and communication endpoints come from `TickClock` on the physics
master's integer lattice, with the default exchange every master interval.
Explicit slower exchanges are validated integer multiples. The worker receives
derived start/stop seconds and returns its actual accepted solver coordinate;
transaction validation never fabricates a corrected output timestamp. See
[architecture 19](19-unified-time-and-clock.md) for pause, reset and speed semantics.
The focused `lunco-modelica-solver` `simulation_session` integration target
checks LunCoSim's adapter against Rumoca's public session API for input updates,
advancing to a communication point, and observing state and output values.
Rumoca owns numerical-method behavior and its solver-specific tests. Vehicle
startup and native controls are verified through production Twin scenarios and
the window-input API.

`RunResultLimits` admits batch grid dimensions before LunCoSim allocates retained
trajectory columns. Cancellation is checked before and after compiler replies,
session construction, and each requested observation. Rumoca main currently has
no public cancellation hook inside a numerical kernel, so an in-progress compile
or session operation must return before its cancellation is observed.

## Parameter authoring and session replacement

`SetModelicaParameter {entity, key, value}` is a typed command shared by the
inspector, Rhai and headless API. Its contract lives in `lunco-modelica-runtime`;
`lunco-modelica-core::parameter_commands` owns admission and terminal replies.
Document-created actors carry authoritative provenance and receive stable entity
ids through the shared session identity admission, so scripts and external
commands address the same participant as the inspector.
The owner validates the exact selected class, current compiled document,
mutable declaration, finite bounded scalar and idle worker transaction before
changing source or session state. Read-only, stale or pending source, final or
unknown parameters, and participants without an editable document reject
visibly without changing the live session. USD-backed participants use their
existing authored USD parameter owner; their document-only inspector controls
are disabled with that explanation.

The document operation must commit synchronously before dispatch. A closed
worker channel leaves the authored edit visible and the installed session
unchanged; the command fails explicitly. Accepted replacements retain current
inputs, the full admitted sibling-source closure and every other instance
override in both native and browser workers. The explicit changed scalar
replaces its own override. A successful result refreshes checked declarations,
values and metadata at the same source-generation/session fence as compilation.
The command remains pending until that exact replacement is installed, and
source changes or retirement of that exact document/runtime/participant owner
terminate its reply. Application document requests survive unrelated scene
teardown and can still acknowledge their checked installation. These are
authored reinitializations, not continuous-state transfer.
`modelica_parameter_contracts.rhai` covers exact selected-class edits, sibling
sources, busy and invalid requests, and no-document rejection through the
production command surface. A low-level native worker check seeds a distinct
instance override and verifies it across successive parameter replacements.
The scenario uses the workbench's `CompileModel` command to create its editable
document participant and therefore runs in a windowed production session; the
parameter command itself is registered by the headless core.
`simulation_core_fourth_modelica.rhai` exercises changed-input and cached Reset,
ordered Pause/Resume across compilation and Reset, terminal run-control failures,
single-session Restart, and pending/invalid/recovered sibling source generations
through the windowed production command surface.
`simulation_core_fifth_modelica.rhai` covers failed and ambiguous Run completion,
ordered target replacement, consecutive identical compile coalescing and settled
Pause through that same surface. A low-level bridge seam controls the otherwise
unobservable outstanding worker receipt: no result and a stale result keep its
paused barrier held; only the exact current result releases it.

## Source-preserving editing and lint

Structured document edits splice the original source spans; untouched bytes,
comments, and formatting are preserved. Only new nodes use the local emitter.
Class edits use Rumoca's declaration span, including qualifiers and the
terminating semicolon.
Rumoca retains authored annotations on `Equation::Connect`. The canvas
projects route geometry from that annotation through
`lunco_modelica_index::connection_routes`. Route edits also update the
annotation in the document's installed AST.

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

All Rumoca dependencies pin upstream commit
`488d6d5f239fcae2bf4ce5a1b729a23d2cc6c396` from
[CogniPilot/rumoca](https://github.com/CogniPilot/rumoca). LunCoSim stores
checked DAE artifacts; each adaptive live or batch solver uses Rumoca's public
`SimulationSession` API. Predicted models use Rumoca's public Model Exchange
session and fixed RK4 integrator. Parsed AST fields are consumed by LunCoSim
adapters that compose public Rumoca queries with source-coordinate and
application metadata. Rumoca owns numerical integration; LunCoSim owns the
requested output grid, value budget, cancellation checkpoints between session
operations, and worker lifecycle. Rumoca currently exposes no
numerical-kernel cancellation hook.

| Mechanism | Authoritative owner | LunCoSim responsibility |
|---|---|---|
| Expression rendering | `rumoca-ir-ast::Expression` Display | Use its complete representation in component modifications, indexer defaults, and canvas labels. |
| Complete declaration extents | Rumoca parser `ClassDef.location` | Slice the original source after range validation; use name tokens separately for renaming. |
| Qualified source names | Rumoca `Name` tokens and `StoredDefinition` queries, composed by `ModelicaDefinitionExt` | Preserve authored `within` and quoted-identifier boundaries in source selection. |
| String literal decoding and encoding | Rumoca parser tokens and `rumoca-core::escape_modelica_string` | Copy decoded values into typed metadata; use canonical escaping when inserting new source nodes. Descriptions concatenate decoded tokens without adding whitespace. |
| Checked DAE, initialization, parameter and input binding | Rumoca compiler and `SimulationSession` | Supply the complete authored source closure and admitted causal inputs. |
| Live and batch solver sessions | `rumoca_sim::SimulationSession::new_with_diagnostics` | Select registered solver capabilities and keep each mutable session in its owning worker. |
| DAE scalar declarations and checked bounds | `lunco-modelica-compiler::scalar_declarations_from_checked_dae` over Rumoca DAE views and `NumericEvaluator` | Project checked facts once for worker metadata; keep USD provenance and channel retention at their application owners. |
| Retained trajectory units and descriptions | `SimResult::variable_meta` and `rumoca_sim::build_variable_meta` | Store exact per-run series facts with completed artifacts; graphs consume retained facts. |
| Component initialization, input transactions and observations | Rumoca `SimulationSession` | Live sessions and initial observations share this owner. `SimulationSession::selected_values` selects requested values from the checked state; Rumoca completes initialization before the first observation. |
| Fixed-step integration | Rumoca `SimulationSession` with `SimSolverMode::FixedStep` | The upstream RK45 solver supplies the fixed-step plugin, advancing to master targets with the configured step and clipping at accepted boundaries. |
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

Diagram projection uses the same Rumoca dependency query. Missing roots are
resolved and admitted through `lunco-modelica-source-roots`, which owns their
registration, operation ordering, and worker dispatch. The canvas keeps its
last committed scene and waits for the authoring engine's typed source-root
lifecycle event. A ready root starts projection from the current document
snapshot, and a worker-load or authoring-mirror failure is shown as a projection
diagnostic. The projection path does not rediscover files or load a second copy
of a root.

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
Native construction uses Rumoca's default parallel Cranelift compilation queue.
LunCoSim caches checked compiled DAEs; it does not install Rumoca's optional
native-code artifact store, which would serialize function compilation while
the cache scope is active. JIT code and solver state remain session-owned.
Native pure-call helpers address module-owned result cells through symbolic,
full-width data relocations. Artifact reuse binds fresh cells in the current
module; mutable results never travel with cached code. The compiled table
remains non-Sync and releases its code/data arena after callable addresses.
The workspace manifest and lockfile pin the Rumoca family. Normal builds resolve
the published revision directly.

The application compiler owner may persist a checked DAE and consumed experiment
defaults for an exact admitted source context. Flat IR and compiler source-map /
diagnostic data are not retained by live workers. Native DAE decoding
reconstructs through the checked constructor. Compilation input identity and
participating-source identity remain distinct; the artifact cache cannot bypass
source admission or retain a live session.
Rumoca's fixed root and visible-output plans carry checked executable selections
alongside their semantic metadata. Their common `PreparedOutputSelection` owner
retains program order, complete grouped outputs, direct/deduplicated observations,
current coordinates and error boundaries. Fixed readers cannot receive raw row
selections or own separate per-row caches. Compilation decline uses the same
prepared interpreter; admitted native errors propagate. No COM/input-dependent
result is cached. The SDK's [fixed execution contract](../../../rumoca/docs/dev-guide/src/runtime/simulation-runtime.md#fixed-output-execution)
owns this mechanism; LunCoSim supplies its existing ME backend and consumes the
same checked observations. Public runtime behavior is verified in authored Rhai;
native call counts, storage bounds and exact bit contracts use SDK unit tests.

Rumoca's exact assignment owner issues a checked `SharedAssignmentSchedule`
containing immutable `ScalarProgramBlock` storage, provenance and ordered target
rows. The interpreter prepares that same block through a shallow clone, and
the native backend consumes the proven schedule directly. Generated C reads
the same contract. Equivalence checks precede issuance; execution commits each
program's targets in source order. Mutable evaluation buffers and numerical
state remain instance-owned. The application adds no schedule proof or native
backend adapter.
Precompile source interfaces project only direct declarations of the selected
class: parameter/input literal bindings, explicit causal ports, and authored
documentation. Generated synthesis requests its exact validated class through
Rumoca's qualified AST lookup. Sibling and nested class declarations never enter
that interface. Source-root requirements still cover the complete definition;
checked DAE facts own inheritance, aliases, evaluated defaults, and live
variable metadata.
Live telemetry units, descriptions, and input bounds come exclusively from the
checked scalar projection of the admitted DAE. Construction carries that inventory through the typed worker
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
DAE projection. They do not parse bundled source to repair missing bounds.
`SnapshotVariables.metadata` exposes exact checked scalar facts alongside values
and applies the same optional name filter. Draft documents remain the owner of
pre-compile authoring facts; the UI does not overwrite compiled channel metadata.
Optimistic authoring edits wait for parsed metadata to reconcile.
`DescribeModel` numeric defaults include Rumoca's semantic start value when no
binding is authored. Input-binding admission and lint facts separately retain
the absence of an authored default; a start guess is not a causal input binding.
An authored non-literal binding stays unresolved in numeric authoring metadata;
it never falls through to a start value.

Rumoca exposes checked trajectory sensitivities, adjoint objective gradients,
and A/B/C/D linearization. Application numerical-analysis tools supply experiment
orchestration and typed result presentation around those APIs.

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

### Bound numerical publication

Rumoca's canonical `VarName` identities cross the internal observation boundary.
The immutable `ModelicaVariableDeclarations` and owner-controlled
`ModelicaVariables` bind them to numerical inventory indices and declared port
slots. The complete lifecycle and string-boundary contract is in
[architecture 22](22-domain-cosim.md#names-bindings-and-numerical-exchange); generic
reflection protection is in [architecture 12](12-api.md#owner-maintained-reflected-state).
