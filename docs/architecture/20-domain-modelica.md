# 20 — Modelica Domain

> Status: Active · Audience: contributors on behavioral modeling and the Modelica workbench
>
> Behavioral modeling using Modelica + the rumoca runtime. Wired into
> Bevy ECS as a `ModelicaDocument` per model, stepped in `FixedUpdate` on
> a background worker thread.
>
> Engineering docs live in
> [`../../crates/lunco-modelica-compiler/`](../../crates/lunco-modelica-compiler/),
> [`../../crates/lunco-modelica-core/`](../../crates/lunco-modelica-core/),
> [`../../crates/lunco-modelica-index/`](../../crates/lunco-modelica-index/), and
> [`../../crates/lunco-modelica-ui/`](../../crates/lunco-modelica-ui/) and
> [`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md).

USD co-simulation samples the solver input buffer after the fixed-step script
transaction, including admitted `SetPorts` commands, and before Modelica step
dispatch. The copy is explicitly ordered after `ScriptingSet`; eventual
positive thrust is insufficient to verify this boundary. The production
`rocket_engine_observables` gate checks the first accepted solver advance after
a Rhai throttle command using its captured input vector and adjacent solver
timestamps. Script output wires retain their separate one-tick exchange delay.

## Contents

Live compile admission retains an entity's existing name. The generated class
name remains simulation metadata; it must not replace a vehicle's authored
identity in control HUDs or the scene tree. An unnamed participant receives its
model name when first admitted.

- [1. Scope](#1-scope)
- [2. Architecture in layers](#2-architecture-in-layers)
- [2a. Generated network schemas](#2a-generated-network-schemas)
- [3. Runtime architecture — background worker](#3-runtime-architecture--background-worker)
- [4. Execution pipeline](#4-execution-pipeline)
- [5. Document System integration](#5-document-system-integration)
- [6. Modelica public ports](#6-modelica-public-ports)
- [7. Type vs. instance distinction](#7-type-vs-instance-distinction)
- [7a. Document identity — three tiers, one truth](#7a-document-identity--three-tiers-one-truth)
- [8. New-model workflow (target)](#8-new-model-workflow-target)
- [9. The Modelica diagram editor](#9-the-modelica-diagram-editor)
- [9c. Canvas animation + multi-user roadmap](#9c-canvas-animation--multi-user-roadmap)
- [10. Panels (current + planned)](#10-panels-current--planned)
- [11. Current gaps](#11-current-gaps)
- [12. Dymola-workflow parity](#12-dymola-workflow-parity)
- [13. See also](#13-see-also)

## 1. Scope

Modelica is LunCoSim's language for declarative behavioral models —
electrical circuits, thermal flow, life-support systems, balloon dynamics,
anything with equations of motion or state. Models live in `.mo` files
attached to 3D entities (Space Systems) in the USD scene.

The Modelica runtime is **Rumoca**. The compiler, DAE, solver, and simulation
crates use the same published revision, recorded in the workspace manifest.

Rumoca's `rumoca-phase-parse` owns strict and recovering parsing, including a
single leading UTF-8 BOM and source coordinates in the original text. Loaders,
documents, and editor paste retain the authored bytes.
The production `modelica_source_encoding.rhai` gate verifies exact text, batch
output and a parser-owned negative diagnostic for the current source generation.
Run admission uses the document index's current candidates and `within` metadata
through the shared `sim_target::resolve_requested_class` resolver. Explicit and
default targets enter the compiler with one qualified identity; dispatch does
not reparse source or repair package prefixes. `modelica_class_compile.rhai`
verifies qualified, relative, bare quoted, default and headless run targets.
Compiler admission parses each unique URI through Rumoca's public
`ParsedSourceDocument` and keeps it beside the serializable LunCo
`SourceFileSnapshot`. Source roots and user overlays enter Rumoca through
source-bearing session APIs, so DAE provenance retains the exact authored bytes
even for `twin://` URIs. The serialized library snapshot is reparsed on the
source-preparation worker to rebuild that Rumoca document. Single-source and
multi-source compilation share this boundary. `ModelicaDefinitionExt` selects
classes from parsed names and `within` tokens; it does not split qualified names
with string delimiters. Exact source CIDs permit reuse of loaded root
definitions; changed owned sources, mixed ownership and conflicting contents
under one URI fail before overlay installation. New `within` namespaces can
compile without requiring a filesystem package. Participating source identity
uses the exact source text in Rumoca's compile result for source spans present in
the flattened target and for generated structural overrides.
Diagram projection resolves its selected document-relative name through the
shared class-lookup owner. The source document owns its `within` scope; a
successful local AST lookup alone does not establish the compiler identity.
This same rule applies to fresh files and retained documents after scene
reloads.

[`lunco-modelica-ast`](../../crates/lunco-modelica-ast/) owns AST interface
projections and parse-time lint facts. It has no Bevy, document,
worker, UI, or solver state. `lunco-modelica-document` owns the headless
 document and source-editing seams; `lunco-modelica-compiler` owns the
 headless Rumoca session and compiler-side source admission;
 `lunco-modelica-source-roots` owns Twin/workspace source-root inventory and
 worker admission; `lunco-modelica-core` owns
 document/runtime synchronization; `lunco-modelica-worker` owns worker and
 simulation seams; `lunco-modelica-ui` owns workbench
 presentation and the `lunica` application facade. The reusable egui graphics
renderer is isolated in [`lunco-modelica-icon-ui`](../../crates/lunco-modelica-icon-ui/);
the diagram canvas and model preview consume that package directly.

LunCoSim installs `ModelicaCorePlugin` and `ModelicaExecutionPlugin` in
`LunCoSimSimulationPlugin` for windowed, headless, and offscreen hosts.
`ModelicaPlugin` and `ModelicaWorkbenchPlugin` add presentation to that existing
engine. The standalone `lunica` application likewise installs one engine before
choosing its workbench or headless runner.
HTML documentation rendering is isolated in
[`lunco-modelica-docs-ui`](../../crates/lunco-modelica-docs-ui/); the Modelica
view supplies only the selected class's documentation fragments.

The transport-free Modelica API surface is a separate production capability
in [`lunco-modelica-api`](../../crates/lunco-modelica-api/). API-enabled hosts
install it alongside the compiler; compiler-only builds do not inherit query
registration, edit-command registration, or Workspace API dependencies. Workspace queries remain owned by
[`lunco-workspace-api`](../../crates/lunco-workspace-api/).

`CreateScratchModelicaDocument` allocates a source-backed untitled document in
the shared Modelica registry in both headless and workbench hosts. It does not
open a tab or select an active Editor document; those are presentation/session
decisions. Rhai tools that generate Modelica for a solve can therefore use the
same document lifecycle and asynchronous Rumoca experiment path in production
scene tests and non-UI hosts.

`RunModelicaSolve` starts the existing asynchronous experiment runner against
an explicit document id, parsed class, and source generation. Its optional
`inputs` map supplies finite constant Real bindings and `tolerance` selects the
run tolerance through the shared bounds validator. It returns an
experiment id immediately; callers poll `RunStatus` and read finite `f64`
series through `GetExperimentResult`. Required package roots come from
Rumoca's strict document AST and enter through the same
`admit_compile_roots` owner as workbench experiments before dispatch.
Read-only source-library documents use that owner's compile overlay contract.
Downsampled trajectories retain the final row by sample index across all
columns, including constant and periodic values, while honoring the point cap.
`CloseScratchModelicaDocument` removes
only untitled generated documents and their run records. It cancels live
experiments and delegates document and linked execution-entity retirement to
the shared `CloseDocument` owner.

Fast Run bounds read the current document AST annotation after any explicit
draft override, then use the documented one-second default if neither is present.
Invalid annotation bounds return an error through the same resolver; they do
not select the default. The shared bounds guard rejects invalid explicit
inputs and grids beyond `SAMPLE_CAP` before solver allocation. See
[`25-experiments.md`](25-experiments.md#bounds-admission) for the admission contract.
Each admitted job receives its complete immutable `ModelSource`; sources and
annotation defaults are never kept in class-name maps. Native compiled DAE
snapshots include the runtime owner, are retired on its `TwinClosed`, and cannot
be republished by a cancelled worker. Native and wasm Fast Run compilers release
user overlays after capturing compilation outcomes while retaining application libraries.

`CompileModel` is a deferred command. `CompileRequested` carries typed one-shot
completion tickets through input admission and request coalescing. Dispatch
transfers those tickets to the exact participant session; the worker bridge
answers only after accepting that session's source generation and initialized
outputs, including a compile that remains paused. Admission errors, stale
sources, worker failure, replacement and retirement terminate the tickets.
A replacement compile retains the current session, observations, and outstanding
Step receipt until that exact endpoint is admitted. It stops further old-session
step admission even if Resume arrives while replacement is pending, then enters
the ordered compile queue. An unchanged exact class/generation can acknowledge its existing checked
session. The command adapter drains these replies through `finish_command_result`;
queued work or a different compiled session cannot acknowledge the request.
API calls requiring a class choice reject with the available candidates;
interactive calls retain the class picker. Sibling source units use the parsed
class inventory for qualified identities, including `within` and nested
classes. Duplicate qualified declarations reject with the class and both
document identities; distinct qualified classes may share a leaf name. Every
declaration in an admitted source remains available to compilation.

Native worker, compiler actor, and solve-pool startup are fallible. A startup
failure or unexpected worker disconnect travels through the typed worker-wide
`ModelicaResult.worker_failure` boundary before participant routing. The bridge
records `ModelicaWorkerFailure`, faults the affected runtime, ends live compile
and step state, and rejects later compiles. This application-owned failure
survives scene teardown; a buffered success cannot revive a dead worker.

Rumoca's shared native JIT builder enables inline stack probes. Large generated
frames must touch guard pages in order on Windows; this is a code-generation
invariant, not a Modelica complexity limit or an application stack-size setting.
All native emitters use that builder. Rumoca's fresh-thread regression executes
a 768 KiB frame and checks both frame edges across a native helper call.
The application retains its thread-affine solver pools, typed channels and
ordered completion admission. Compilation and solver errors use their existing
terminal result paths; Rust unwind handling does not contain native access
violations or exhausted stacks.

Participant input, integration, and observation failures are terminal step
results in both native and browser workers. Only a complete finite observation
can publish runtime values or a presentation stream. See the
[checked Rumoca runtime contract](29-rumoca-runtime-contracts.md) for ownership
and its production negative gate.

The response bridge changes document diagnostics only when their contents or
compile lifecycle change. Successful live steps with no diagnostics do not mark
that resource changed; status-bar consumers retain their native Bevy change
gate. A successful reset still marks the document ready and clears its errors.

Rumoca owns parsed source class identity and session lookup. LunCo's
`ModelicaDefinitionExt` walks parsed class names, while source-root ownership
uses the first token of the AST's `within` name, or the top-level class names
for a valid empty `within;`. This preserves quoted identifiers whose spelling
contains dots and admits top-level Twin Modelica files. Rumoca's
`Session::class_lookup_query` selects the current indexed document and
declaration, including source-root documents. The application keeps no second
class-to-document index or negative lookup cache.

`DescribeModel` returns the canonical authored class name. Explicit names select
an exact qualified or document-relative declaration; a bare name is admitted
only when unique in the owning document. Inherited member facts come from
Rumoca's complete typed declaration query, with exact qualified type identities
and unchanged authored type spelling in the component AST. The source-library
indexer and canvas consume a shared immutable diagram snapshot; neither guesses
connector semantics from type names. Unresolved or cyclic bases are a
terminal query diagnostic. UI inputs, parameters, icons, source editing, and
metadata retain the selected class identity; missing metadata cannot select a
similarly named class.

The source identity production gate covers duplicate leaves, quoted identifiers,
`within` qualification, empty/unknown requests, and incomplete inheritance
(`modelica_class_identity.rhai`). `modelica_class_compile.rhai` separately proves
qualified and quoted class compilation, exact retained trajectories and units,
and terminal wrong-package admission. The integrated native production build
passed both gates (13 and 3 assertions) plus retained run metadata (20); the
browser worker compile check passed. Summer Space School's six authored startup
assertions passed with all three participants running in the integrated native
session.

Rumoca's `ClassDef.location` owns complete declaration bounds, including
qualifiers, terminating semicolons and element constraints. Library source
slicing and duplicate preparation use that range against its matching source
text; neither rescans surrounding characters nor clamps invalid coordinates.
Identifier-token locations remain separate rename spans. Same-line siblings,
comments and short definitions are covered by the parser's source-span checks.
Read-only library document loading selects the exact qualified class through
Rumoca's stored-definition lookup; a missing sibling cannot resolve another class with
the same leaf name.

After a typed component or variable addition, the document indexes the new
declaration from the accepted AST. The inspector immediately shows its type
and modifications. The source parser supplies the document byte range on the
next parse. A modification draft is scoped to its document, class, component,
parameter, and authored value. It commits through the document command when
the field loses focus.

Deferred edit parsing wakes at its recorded debounce deadline even when the
document-registry and worker-capacity revisions are unchanged. The source
generation remains the admission fence; completed runs retain their compiled
metadata while the replacement draft parses. Only edits to that document
extend its debounce; unrelated mouse and keyboard activity cannot delay
admission. Worker capacity and completion budgets bound preparation.

Scratch and reserved duplicate documents register their typed creation context
synchronously at allocation. Deferred document-open delivery cannot rebind them
to a replacement Twin. Duplication captures the target owner before queued
command delivery and snapshots resident source bytes with their document owner.
The task resolves library paths, reads the selected source, and renames only the
parser-owned class name tokens. The copy retains its explicit `within` scope;
Rumoca resolves enclosing imports from admitted package sources. Native batch
and live compilation use the same compiler actor, source roots, and artifact
cache. Batch replies carry typed immutable DAE and content identity; only the
background run thread waits for compilation.
Known-source read, decode, parse, or rewrite failures end the operation visibly;
source and target retirement cancel it. Failed or cancelled duplicates release
their pending document metadata, name, tab and busy handle. Successful native
duplicates use the canonical syntax cache. Opening busy handles finish when the
document is installed, independent of tab visibility or view mode. Diagram
projection owns its own tracked task only when a canvas requests it; hidden,
text-only, or already-projected documents retain no opening handle.
Browser tasks retain the deferred editor parsing contract; Bevy tasks
in a browser do not imply execution on a separate JavaScript worker.

File opening captures local root IDs and the exact replicated mount before it
leaves the caller. Its existing async task resolves native canonical identity
through storage and reads the source; local root ownership takes precedence.
Browser mounted files use the same validated OPFS identity as their storage
reader, while picked bytes and private WebStorage/session sources have explicit
Application lifetime. Installation rejects a retired owner. A dirty resident
source cannot be relabeled by reopening its path under another owner; a clean
explicit reopen can install newly read source and its new admitted lifetime.
Modelica editor file opens capture resident origins, owners, and source
generations before dispatch. The file worker resolves aliases; installation
validates those captured facts before rebinding a clean resident source.
Dirty sources and sources changed or installed by another owner after admission
cannot be overwritten by a pending read.
`GetFile` admits an I/O task with the current runtime and immutable file-root
snapshot. Standard file URIs are decoded by Storage; mounted browser reads use
OPFS. Retirement cancels its request carrier, and completion validates the
resolved source owner before publishing log text. Read errors publish a
structured diagnostic; invalid addresses or missing I/O capability reject the
command at admission. Its acknowledgement admits the read; file content and
read failures arrive asynchronously in the diagnostic stream.

Workspace folder association remains a display/authoring lens. Active document
scope and Modelica teardown read the stored runtime owner. Deferred document-open
events require prior admission registration when a Workspace exists.

Fast Run admission records an `ExperimentOrigin` in the sole
`ExperimentOrigins` registry. Local document work pins its exact document and
runtime lifetime through `PinnedDocumentRuntimeOwner`; replicated work pins the
exact authenticated connection and admitted scene mount. Presentation grouping
identifies retained history and never determines ownership. Definition replay
rejects a UUID attributed to another origin before changing its row, and replay
from the same origin preserves terminal status and results.

Each pending handle and emitted lifecycle message carries its immutable admitted
origin. The receiver validates it before changing status, publishing results,
or updating UI/playback. Closing an owner withdraws queued work, requests
cancellation of executing work, marks unfinished rows `Cancelled`, and retires
receivers. A buffered completion cannot revive a cancelled row or update a UUID
that was subsequently admitted under another origin. Loose-document work has
application lifetime.

The UI publishes results only while the pinned document owner remains in the
active workspace scope. Automatic plot selection also requires the selected
or pinned experiments document to match. Playback entities carry the same typed
source attribution. Document closure releases its playback signals and entity;
scene reload resets the registered playback entity map and reclaims its
`SceneOwned` entities. Closing their Twin also drops document pins and
live/archived plot selections. Completed trajectory history remains queryable.
`ExperimentRemoved` reports
explicit deletion and bounded registry eviction, retiring source attribution
and pending handles, as well as UI run visibility, without scanning retained
history each frame. Retained historical results are fenced by their admission
owner before active plots or canvas snapshots consume them.

`ModelicaCorePlugin` also owns the shared Rumoca engine-sync driver and the
deferred structural-edit queue. Headless source edits therefore receive the
same asynchronous parse/install cycle as workbench edits; the UI plugin only
adds presentation and pacing hints.
The driver classifies exact documents linked to non-editor scene participants
as `SimulationRequired` in the shared bounded admission queue. The focused
editor document is `Interactive`; remaining documents are `Background`.
Submission is ordered by priority, document identity and source generation
within the local parse bound. Scene demand comes from the document registry's
entity links and the participant's matching document, independently of editor
visibility. Parse diagnostics report queue wait separately from parsing and
installation; priority does not preempt an already running task or change
source-generation validation.

The shared render-free participant contract is [`lunco-modelica-runtime`](../../crates/lunco-modelica-runtime/).
It owns `ModelicaModel`, the serialized worker command/result messages, source
assets, generated USD-document metadata, communication scheduling, notices,
samples, and telemetry layout. The compiler host consumes that contract; USD
projection and UI adapters depend on the runtime package directly when they do
not need Rumoca compilation. The generic `DocumentRegistry<ModelicaDocument>`
from `lunco-doc-bevy` is installed by each Modelica host; generated-document
metadata is a separate runtime resource because it follows projection lifecycle
rather than authored editing.

Live preparation reports typed `ModelicaPreparationPhase` transitions through
the worker result transport: compilation queue, session construction, and
successful construction. The bridge validates
the participant session and source generation before publishing lifecycle
notices, then returns without consuming these messages as solver transactions.
The UI mirrors all notice severities into Recent status as discrete entries.
New-participant failures identify the model and report startup failure across
compilation, solver construction and the initial observation; the model remains
stopped and the owning diagnostic retains the actual cause.
Preparation notifications do not change simulation time, outputs, compile
completion, or the required participant admission hold.
Successful construction also reports a typed `ModelicaConstructionProfile`:
worker queue wait, solver-session construction, scalar metadata
collection, and the first checked observation, in nanoseconds. Queue wait is
unavailable on transports that cannot measure it. The fenced lifecycle receiver
retains these facts with the participant and emits the existing Modelica notice
plus a structured log naming the model. `CosimStatus.modelica_construction_profile`
exposes them only for an installed compiled model, including compact queries.
Failed or rejected/replaced participants cannot expose the retired profile.
The production `modelica_source_target` gate covers successful construction,
compact readback, rejection, replacement and recovery. These measurements are
diagnostic wall time and do not select simulation transitions.

USD instance values reach the worker as typed `ModelicaAuthoredBindings`.
Before numerical input admission, `PrepareInterface` uses the shared compilation
pipeline to produce a checked DAE and its exact scalar interface. The compiler
first resolves supplied parameter candidates through Rumoca's scoped declaration
query so authored dimensions and conditions can be bound before strict
instantiation. This query does not publish runtime ports. The checked DAE then
classifies every supplied value as a scalar parameter or runtime input. A name
outside that boundary is rejected unless USD captured a distinct registered
input owner on that entity. That evidence uses opaque `PortBackendId` values,
excluding generic authored-input storage and the co-simulation backend; matching
output names and human-readable metadata labels do not establish ownership.
Expression-bound and inherited parameters follow the same path. Literal source
defaults are presentation data, not a declaration inventory.

The first strict compilation applies representable parameter bindings through
`Session::set_structural_overrides`, including parameters with no source default.
Integer and Boolean bindings require exact representable literals. Rumoca
invalidates resolved trees and downstream artifacts when structural overrides
change, so each compile sees the current synthetic literal source map. LunCoSim
captures diagnostics before clearing the modifiers. Immutable variant reuse
requires the same source/root identity and the same set of unindexed root-bound
parameter names; every
checked structural binding must also match its requested value bits. Checked DAE
`is_tunable` facts own this distinction. A tunable value change can therefore
reuse the DAE, while omitting a previously bound name cannot retain that binding.
Indexed scalar tunables remain checked lowering overlays; indexed structural
bindings receive an explicit diagnostic because root modifiers cannot represent
them. Every requested tunable value is supplied again to solver lowering.

The interface projects actual conditional presence, scalar input/parameter
identities, numeric observables, descriptions and units from that DAE. Parameter-
dependent bounds use admitted tunables through Rumoca's numeric evaluator;
unsolved bounds are explicitly `InitializationDependent`, never an invented
value or an absent bound. Initialization start guesses are not settled parameter
bindings. Interface preparation constructs no solver, publishes no numerical
samples, and shares its compiled artifact with subsequent participant preparation.
The exact scene/source/session lease and admitted library revision fence reuse.

`ResolvedModelicaInterface.input_groups` stores each checked runtime input once:
its aggregate name, dimensions, ordered scalar names, primitive type, indexed-path
eligibility and declaration-binding defaults. Group shape and scalar order come
from the DAE, not scalar-name parsing. `Known` defaults are evaluated in the same
admitted parameter context; an absent binding, `InitializationDependent` binding,
and unsupported numeric evaluation remain distinct. Coupled capture replaces an
array binding only with a complete aggregate. It fills an omitted element only
from a checked `Known` default, validated against its scalar type. Missing elements
otherwise fail admission. Start values never complete an array default.

The full requested bindings remain in the immutable solver snapshot. Connected
initial samples take precedence over authored unconnected input values. Accepted
parameter overrides update the model and shared participant views at result
admission; the checked DAE stays immutable.

The worker retains bounded compiled-DAE artifacts. Their identities include the
admitted source bytes, numeric structural values, Rumoca build identity, and host
platform. Reuse ends at the checked DAE: live state and solver sessions are
constructed per participant through Rumoca's public
`SimulationSession::new_with_diagnostics` API. Reset passes the currently
admitted input values to a fresh session without rereading mutable compiler
sources. Initial state values use Rumoca's checked `SimOptions::start_overrides`.

`lunco-modelica-compiler::ModelicaCompiler::scalar_declarations_from_checked_dae`
projects units, descriptions and statically known bounds through the shared
checked interface evaluator. The worker adapts those declarations to runtime
metadata; it does not evaluate DAE expressions independently.

Batch runs build their requested output grid in `lunco-modelica-core::sim_target`
and admit retained values through `RunResultLimits` before allocating trajectory
columns. They advance Rumoca's public session to each admitted observation time.
Cancellation is checked at LunCoSim's compile-result and session-operation
boundaries; Rumoca main does not expose an interruptible numerical-kernel API.

`lunco-modelica-runtime::ModelicaCacheLimits` bounds compiled-DAE entries in
memory. Native persistent reuse stores validated compiled DAEs under the
versioned `compiled-dae-v6` namespace and its captured disk budgets. Each
participant owns its Rumoca session and initialized state.

Generated-network compilation consumes its complete synthesized document and
the admitted source roots, not other open Modelica documents. Their reserved
bundled `generated/` filename namespace is classified by the runtime metadata
owner and used by structural hashing. Opening editor documents or completing
another generated network cannot change this compile source set. Authored
documents retain sibling-document compilation for cross-document definitions.

Session IDs fence stale results: when a Compile or UpdateParameters bumps
the session, any in-flight Step for the old session is discarded.

Live Modelica sessions are open-ended. The worker options span sets the root-scan
scale, and every requested step must reach its public target. Batch experiments
carry their explicit stop time through `stepper_options_from_bounds`; the
runner owns bounded output sampling. Fixed RK4 uses the same open live clock.

Panics in the worker are caught (`catch_unwind`) and reported as solver
errors rather than crashing the app. This tolerance is essential for
interactive parameter tuning — an unstable parameter shouldn't kill the
whole sim.

When a live solver step fails, the terminal RuntimeFault keeps the solver
error and lists Modelica values that were not produced by the failed step. The
list includes each value's last accepted sample and the sample and
communication-target times. Names are sorted for stable
diagnostics; when no successful sample exists, the message says so explicitly.

## 4. Execution pipeline

Co-simulation exchange and stepping use a shared fixed timestep (60 Hz by
default). Worker completion admission precedes clock
projection and can release pending work between complete fixed cycles.
Ordering uses these system sets:

```
SimulationCompletions:
  ModelicaSet::HandleResponses    — validate worker results and release completed causal holds
  USD coupled initialization     — admit solved inputs and certify constructed participants
  sync_modelica_outputs          — ModelicaVariables.values → SimComponent.outputs
First:
  ClockProjectionSet             — project remaining holds before TimeSystems samples time

FixedUpdate:
  CosimSet::Propagate             — propagate_connections
  CosimApplySet::ApplyForces       — apply_sim_forces
  (sync_inputs_to_modelica)       — SimComponent.inputs → ModelicaModel.inputs
FixedLast:
  FixedCycleVerificationSet       — settle the native step and verify its endpoint
  ModelicaSet::SpawnRequests      — dispatch due Step commands for the accepted interval
```

The worker command stream is submitted in stable `GlobalEntityId` order, with
the world-local entity key as a tie-breaker for unaddressable local models.
That ordering keeps serialized worker service independent of ECS query layout;
it does not make adaptive-solver trajectories bit-reproducible across machines.
The remaining cross-domain admission and replay requirements are recorded in
[`62-deterministic-runtime-and-async-boundaries.md`](62-deterministic-runtime-and-async-boundaries.md).

See [`22-domain-cosim.md`](22-domain-cosim.md) for the full pipeline.

Fixed-step publication exposes `modelica_outputs_copy_variables` and
`modelica_outputs_seed_inputs` trace spans. The copy span runs when the required
`ModelicaVariables.values` snapshot changes, or a model, USD participant marker,
or destination component is added. Input seeding and status still follow
`ModelicaModel` changes. Worker admission commits the map and communication clock
serially in `First`; it preserves native f64 bits, detected-symbol/output
precedence, and existing keys. Clock/input writes alone do not dirty the snapshot.
Compile replacement and reset clear it; model removal retires it. Missing
variable state on a live participant raises `modelica-variables-missing` and
rejects buffered results until scene teardown. Per-model terminal state also
rejects lifecycle results without pending compile/reset work, so the single
first-fault record does not leave other failed participants exposed.
Reflection, telemetry, canvas,
co-simulation and API readers consume this one map. External API variable fields
retain their existing shape. `ResetActiveModel` completes through command-result
admission and rejects missing runtime state visibly.

The observable snapshot retains one Bevy-reflected `IndexMap` under an
owner-controlled mutation boundary. Checked compiler declarations classify
variables. Canonical-ID row bindings preserve admitted ordering, omitted values,
duplicates and exact f64 bits; numerical updates use retained indices. Named
inspection and typed input/parameter commands retain their public contracts.
Generic reflected writes cannot modify snapshots or live name/value maps.
Compile admission and
fixed-step dispatch share the worker's name-sorted input sampler; hash-table iteration never chooses solver input
order. The USD input bridge uses native borrowed entry lookup and applies the
authored command surface after connected samples, preserving its precedence.
Worker admission retains existing keys and compares f64 bits before marking the
snapshot changed. Regular development builds optimize `lunco-modelica-worker`
at level 3 with assertions enabled. Causal wire ordering comes from the existing
connection comparator, independently of the observable map's iteration order.

USD-backed participants publish public samples only for their checked
`DeclaredOutputPorts`. A declaration change retires removed public keys;
inspection and telemetry retain their separate observable inventory. Copying a
worker observation cannot introduce an undeclared public output.

Production gates `modelica_variable_projection` and `modelica_variables_missing`
cover retained publication while paused, typed input writes, rejected raw snapshot
and input/parameter-map mutation, resumed solver replacement, and the required-state
fault. The indexed inventory and binding contract is in
[architecture 22](22-domain-cosim.md#names-bindings-and-numerical-exchange). Exact multi-rover replay checks the fixed-step
consumer; neither the solver nor communication schedule changes.

Standalone Modelica port topology publication excludes entities with
`SimComponent`, matching its port backend. The co-simulation port owner
publishes those entities' topology instead. Live input values do not invalidate
either contract; input membership and signal metadata do. Modelica source
admission projects live engineering facts into the backend-neutral
`ScalarPortContracts` component. The port backend reads that component;
`ModelicaSignalLayout` retains source addressing and provenance. Both are
admitted and retired with the same source surface.

The render/update-side engine synchronizer is separately revision-gated: the
document registry advances one monotonic revision for document membership or
source mutations, then the synchronizer compares each document generation
before parsing or upserting it. Async parse completion remains an explicit
mutation signal, so a stale completion cannot leave a changed document asleep.
The parse worker queues an owned `SyntaxCache` with its spawn generation,
shared AST, and located diagnostics at the session-install boundary. Native
workers prepare the session's separate owned AST before acquiring its mutex;
the update-side FIFO moves the queued syntax without cloning a parsed tree.
The drain checks the generation before document installation and charges both
queue extraction and index installation to its per-update budget. One result
can exceed that budget, but subsequent results wait for another update.
WASM transport and prepared-library results use this same document consumer;
they do not install the document directly in addition to queuing a completion.
The production `modelica_parse_completion.rhai` gate exercises a clean scratch
document, source-located invalid syntax with healthy-index retention, and a
repaired generation with refreshed classes and cleared parse diagnostics.
Runtime Modelica telemetry is sampled from solver model time at the configured
rate; its cursor only avoids rebuilding the same batch between due samples, and
the shared signal registry remains the sole channel-history authority.
Live metadata comes from the checked DAE scalar declarations published with worker
construction. The existing session/source-generation fences admit it together
with the solver; successful compile, reset, or parameter-update results require
the inventory. A reinitializing Step moves newly checked facts on its exact
completion. The runtime retains immutable compiled facts, and telemetry only
publishes metadata when that inventory or the generated USD identity changes.
It never walks authoring documents or interprets variable names as units. Draft
edits do not change an installed solver's unit/description facts. The shared
signal registry owns comparison, storage, and first/changed publication.
Generated USD Modelica source metadata has a separate invalidation boundary in
`lunco_modelica_runtime::generated_source`: the generated source projection and
its document link/removal lifecycle. Live `ModelicaModel` output and clock
updates do not rebuild that source registry; they remain solver state and are
consumed through the Modelica runtime paths.
Member class discovery follows the Modelica source asset lifecycle: an asset
load or failure event settles the declaration, and a source modification
invalidates only that declaration before re-projection. There is no
time-based source-resolution deadline or steady-state pending scan.

### 4.1 Run-state machine + command semantics

Live stepping is gated per-entity by run-state on `ModelicaModel`.
**Compiling a model never auto-starts a live realtime sim** — a fresh
compile leaves the model paused/ready, and live stepping begins only on
an explicit Run.

```
Uncompiled/Stale ──[Compile]──▶ Ready (paused) ──[Run]──▶ Running
                                      ▲                      │
                                      └────────[Pause]───────┘
Compile error ─────────────────▶ Blocked (paused)
```

State on `ModelicaModel`:

- `paused: bool` — the per-frame gate in `spawn_modelica_requests`
  (`if model.paused { continue }`). Running ⇔ `is_compiled && !paused`.
- `is_compiled: bool` — worker has installed a stepper.
- `is_compiling: bool` — a Compile is in flight.
- `compiled_generation: u64` — document `generation_owned()` at the last
  *successful* compile.
- `pending_generation: u64` — generation captured at compile dispatch;
  copied to `compiled_generation` only when the matching worker result lands
  for the still-current document revision. An edit during compilation causes
  that result to be discarded; an active participant keeps its run intent and
  requests a compile for the current parsed revision.
- `resume_after_compile: bool` — transient; set by `RunActiveModel` so
  the post-compile success handler unpauses (instead of staying paused).
  Cleared on both success and error so a failed Run never silently
  auto-plays on a later unrelated success.

Staleness: `stale = !is_compiled || compiled_generation != gen`, where
`gen` is the document's current `generation_owned()`.

Worker lifecycle and simulation time are separate schedule domains. The execution
host registers `handle_modelica_responses` once in the core runtime's
`SimulationCompletions` schedule. `First` services that schedule after message
rotation and before `ClockProjectionSet`. The headless driver services it while
waiting for the next fixed deadline, and the windowed driver services it on
response readiness during the render handoff. The shared fixed runner
services it once before checking admission of a held pending cycle. Ready
receipts can release that cycle in the same application update; unfinished work
yields without waiting. The same system state,
validation, and ordered response reader serve every invocation. No clock or
simulation tick advances in this schedule. An off-thread compile or step can settle while
the fixed clock is held, and a completed causal step releases its hold before
the virtual clock samples this update. Remaining participants and runtime faults
retain the hold. Compile-intent admission stays in the `Update` lifecycle cycle.
The plugin registers `spawn_modelica_requests` in `FixedUpdate`, where the
master clock advances and the next deterministic communication request is
issued. `ModelicaExecutionPlugin` consumes `CompileRequested` intent and the
worker validates the pinned document runtime owner before gathering the current
snapshot; sibling documents must share that exact admitted runtime. The UI
command resolves class selection and records ordered run intent. The complete
participating source unit waits for current syntax/AST generations before class
deduplication and dependency discovery consume its bytes. A requested target entity that has been
removed is rejected; it cannot create an editor actor implicitly. Completion
admission continues while the fixed clock is held, so readiness can observe
the terminal worker result and release its preparation hold.
Compile requests retain first-admission order across participants. Deferred
requests for the same source owner, participant and class coalesce across
interleaved participants; they retain every completion listener, any forced
compile and the latest ordered run intent. A different class for that participant
is an ordering boundary: subsequent requests cannot coalesce across it. A later
different target supersedes the earlier session and terminates its listeners
explicitly. Class spelling is not priority.

Verb semantics:

| Verb | Effect |
|---|---|
| `CompileModel` | Deferred terminal result after the exact checked session is accepted, or an explicit rejection/cancellation. An already compiled, unchanged exact class reuses its session; `force: true` dispatches again. Success leaves the model paused/ready unless explicit Run intent was requested. |
| `RunActiveModel` | Compile-if-stale, then play. A clean installed target resumes immediately. Otherwise the compile owner retains the original command identity, origin and API correlation until exact session installation or rejection. External ambiguous targets require an explicit class. |
| `ResumeActiveModel` | Apply running intent to an installed or admitted participant, including a pending compile; terminal applied/rejected result, without compiling. |
| `PauseActiveModel` | Apply paused intent to an installed or admitted participant, including a pending compile; terminal applied/rejected result. |
| `ResetActiveModel` | Validate the installed target, prepare its retained checked DAE with current inputs, and complete after exact session installation or rejection. Zero time and retain the latest run intent; no source compilation. |
| `RestartActiveModel` | One checked Reset transaction with running intent. Complete after its installation or propagate its failure. A later Pause/Resume replaces the intent. |
| `FastRunActiveModel` | Orthogonal: batch compile + simulate off-thread → `Experiment`. Never touches live run-state. |

Pause applies future run intent without cancelling an admitted communication
transaction. An outstanding Step retains the shared-clock barrier even while
the participant is paused; only its accepted result or terminal fault retires
that boundary. The paused child clock remains frozen after the endpoint settles.

Strict DAE compilation failures remain terminal. Rumoca's checked phase
report supplies the balance and source diagnostics, which reach the Modelica
Error status, history popup, and document diagnostics. The author must correct
the independent equations or constraints before a solver can be prepared.

The toolbar (`ui/panels/model_view/render.rs`) maps these to one
Compile button (🚀 → `CompileModel`, compile only), a Run/Pause
toggle (▶ → `RunActiveModel`, ⏸ → `PauseActiveModel`), Reset (⟲), and
Restart (⟳ → `RestartActiveModel`). The `CompileStatus` API query
reports the run-state (`is_compiled`, `is_compiling`, `paused`,
`running`, `stale`, `current_time`) alongside the compile state.

`GetDiagnostics {doc_id}` is the shared authoring query for source problems
across Modelica, USD, Rhai, SysML, and other document owners. It combines
compile diagnostics with each installed parser, semantic-analysis, and lint
channel. Every diagnostic has the same shape: domain, source, stable code,
severity, message, URI, optional one-based source range, optional zero-based
byte offsets, subject, suggestion, and related locations. The report exposes
each channel's generation and `pending`, `ready`, `failed`, or `unavailable`
state; clients must wait for `complete: true` before treating no findings as a
clean result. Rumoca Help and Note findings are surfaced as hints and
informational rows, along with warnings and errors. The Modelica diagnostics
panel and API query read the same per-document `DocumentDiagnostics` snapshot.
Rumoca's source linter runs off the update thread for open documents; only the
current source generation is published. This source-level feedback is separate
from the authored `lint.modelica` policy used by explicit asset validation.

## 5. Document System integration

`ModelicaDocument` implements the Document System trait
(see [`10-document-system.md`](10-document-system.md)) and is the
authoritative in-memory representation of a `.mo` file.

### 5.1 Canonical state: text + cached AST

Source text is canonical. The AST is a **cached projection**,
refreshed eagerly after every mutation so panels that need structural
access (diagram, parameter inspector, placement extractor) can read
`doc.ast()` without reparsing.

```rust
pub struct ModelicaDocument {
    id: DocumentId,
    source: String,                    // canonical
    ast: Arc<AstCache>,                // derived, refreshed per op
    generation: u64,
    origin: DocumentOrigin,
    last_saved_generation: Option<u64>,
    changes: VecDeque<(u64, ModelicaChange)>,
}

pub struct AstCache {
    pub generation: u64,
    pub result: Result<Arc<StoredDefinition>, String>,
}
```

Text is canonical (not AST) because:

- Comments and hand-chosen formatting must round-trip losslessly.
  A code editor that reformats the file on every parameter tweak is
  not usable — IDE-style edits require that only *edited regions*
  change text, not the whole file.
- AI tooling (e.g. Claude's `Edit` / `Write`) operates on text ranges.
  A text-canonical document is compatible with those flows out of the
  box; an AST-canonical one would require a dedicated text adapter.

The trade-off is that we need **span-aware AST ops** — see § 5.3.

### 5.2 Op set

`ModelicaOp` is `#[non_exhaustive]` — all variants are implemented and
cover the editing surface:

| Op | Effect | Used by |
|----|--------|---------|
| `ReplaceSource { new }` | Full-buffer swap | Coarse text commits |
| `EditText { range, replacement }` | Byte-range replacement | Granular text edits |
| `AddComponent { class, decl }` | Insert a new component declaration | Diagram: drag from palette |
| `RemoveComponent { class, name }` | Delete a component's full declaration | Diagram: right-click delete |
| `AddConnection { class, eq }` | Insert a `connect(...)` equation | Diagram: wire ports |
| `RemoveConnection { class, from, to }` | Delete a `connect(...)` equation | Diagram: disconnect wire |
| `SetPlacement { class, name, placement }` | Set/replace the `Placement` annotation | Diagram: drag-to-move |
| `SetParameter { class, component, param, value }` | Replace one parameter modifier | Parameter inspector |

`class` accepts qualified dotted paths (`Pkg.Inner`,
`Modelica.Blocks.Continuous.Integrator`) so ops work on nested classes
too.

### 5.3 Apply pipeline

Every op funnels through a single pure function
`op_to_patch(source, ast, op) -> (range, replacement, change)` and a
single mutation path `apply_patch(range, replacement, change)` that:

1. Validates bounds + char-boundary alignment
2. Splices the source buffer
3. Bumps generation, refreshes `AstCache`
4. Pushes the structured change onto the ring buffer
5. Returns an `EditText` inverse carrying the exact removed bytes

All ops — including AST-level ops — are implemented as **span-locate +
text patch**:

- `AddComponent` → locate insertion point via `ClassDef.equation_keyword` /
  `end_name_token` tokens, render the decl via the pretty-printer,
  splice
- `RemoveComponent` → use `Component.location` for the full decl span,
  extend through the terminating `;` using a paren-aware scanner
- `RemoveConnection` → find the matching `Equation::Connect` by port
  pair (order-insensitive, Modelica `connect` is symmetric), scan
  backward to the `connect` keyword, forward to `;`
- `SetPlacement` → locate the component's annotation span, find the
  `Placement(...)` sub-call at paren depth 0, replace in place;
  sibling annotations (Dialog, Documentation) are preserved
- `SetParameter` → locate the component's modifications list, parse
  entries at depth 0, replace `name = value` or append as needed

This keeps all source mutation on one code path (uniform undo,
uniform change emission) and means every op produces a byte-level
minimal patch — comments and formatting outside the edited span stay
intact.

### 5.4 Pretty-printer for new nodes

The pretty-printer only renders **new** nodes being spliced in. It is
deliberately *not* a full AST round-trip serialiser — existing nodes
live in the source text, never re-emitted.

```rust
pub struct PrettyOptions {
    pub indent: String,
    pub continuation_indent: String,
}
PrettyOptions::tabs()       // "\t" / "\t\t" — workbench default
PrettyOptions::two_space()  // "  " / "    " — library default + tests
```

The workbench installs tab indentation at startup (`ModelicaPlugin::build()`).
Options are process-wide so every op path (diagram panel, scripts,
tests) produces consistent output. Annotations go on their own
continuation line so generated source stays readable:

```modelica
	Modelica.Blocks.Continuous.Integrator I1
		annotation(Placement(transformation(extent={{-10,-10},{10,10}})));

	connect(I1.y, Gain1.u)
		annotation(Line(points={{0,0},{10,10}}));
```

### 5.4a External API surface (`api_edits.rs`)

Each `ModelicaOp` variant has a Reflect-registered command wrapper so
external callers (HTTP / MCP / agent SDK) hit the same code path as the
GUI panels (per AGENTS.md §4.1):

| API command | Wraps | Purpose |
|-------------|-------|---------|
| `SetDocumentSource { doc_id, source }` | `ReplaceSource` | Whole-buffer rewrite — agent batch edits, lint apply, source import |
| `AddModelicaComponent { doc_id, class, type_name, name, x, y, w, h }` | `AddComponent` | Drop a component into a class with a placement |
| `RemoveModelicaComponent { doc_id, class, name }` | `RemoveComponent` | Delete a component declaration |
| `ConnectComponents { doc_id, class, from, to }` | `AddConnection` | Add a `connect(a.p, b.q);` equation; `from`/`to` are dot-paths |
| `DisconnectComponents { doc_id, class, from, to }` | `RemoveConnection` | Drop the matching connect equation |
| `ApplyModelicaOps { doc_id, ops: Vec<ApiOp>, parent_generation? }` | All structural variants | Batch fan-out: `AddComponent / RemoveComponent / AddConnection / RemoveConnection / SetPlacement / SetParameter / EditText` in order; an optional generation cursor rejects stale groups |
| `RenameModelicaClass { doc_id, old_name, new_name }` | string-level rewrite | Rename a top-level class declaration + its `end OLD;` closer; if the doc origin is `Untitled`, the origin name is updated too so the tab title follows |

`ApplyModelicaOps` is the primary path for agent / canvas drag-drop —
each op in the batch becomes its own undoable step, but the caller
gets a single round-trip and a guaranteed ordering. The single-op
wrappers exist because hand-written agent code reads cleaner with
named commands than with a flat `ApiOp::AddComponent { ... }` payload.

API edits waive the source-edit debounce (`waive_ast_debounce`). They enter
the bounded parse queue immediately and refresh the canvas and editor when
the generation-checked result is installed — see § 5.7.

**Current gaps** — typed wrappers we don't expose yet, available
via `ApplyModelicaOps` or planned as standalones:

- `SetPlacement` / `SetParameter` — only via `ApplyModelicaOps`
- `EditText { range, replacement }` — available through `ApplyModelicaOps`; the
  Rust owner validates UTF-8 boundaries and schedules the parse without
  blocking the editor
- `RenameModelicaComponent`, `RenameModelicaPort` — not implemented
- `AddClass` / `RemoveClass` / `MoveClass` (between packages) — not
  implemented; `RenameModelicaClass` covers in-place rename only
- `SetClassAnnotation` (Icon, Diagram graphics) — not implemented;
  whole-source replace is the workaround
- `AddImport`, `AddExtends`, `SetDocumentation` — not implemented

### 5.5 Structured change events

After every successful mutation the document pushes a
`ModelicaChange` onto a bounded ring buffer
(`CHANGE_HISTORY_CAPACITY = 256`). Consumers poll via
`doc.changes_since(last_seen_generation)`:

```rust
pub enum ModelicaChange {
    TextReplaced,               // text-level ops + undo/redo
    ComponentAdded   { class, name },
    ComponentRemoved { class, name },
    ConnectionAdded  { class, from: PortRef, to: PortRef },
    ConnectionRemoved{ class, from: PortRef, to: PortRef },
    PlacementChanged { class, component, placement },
    ParameterChanged { class, component, param, value },
}
```

`changes_since` returns `None` when the consumer has fallen further
behind than the retention window — caller must then do a full rebuild
from the current AST. Panels (diagram, inspector) use this to patch
their render state incrementally rather than rebuild on every frame.

### 5.5a Cross-truth rules (R1–R4)

Three views read the same document — text editor, canvas diagram, and the
autosave/sync machinery — and each holds transient state the others cannot see
(an uncommitted keystroke, a drag in progress, an open modal). The cross-truth
rules are the contract that keeps them from trampling each other. They are
numbered because the code and tests cite them by number
(`tests/cross_truth_policy.rs`; the definition lives here).

- **R1 — active gesture wins until idle.** While any gesture source is active
  (`IsGestureActive` in `ui/wasm_autosave.rs`: `canvas` drag, `text` pending
  commit, `modal` dialog), autosave bails and retries on the next
  `DocumentChanged` after all sources clear. Per-source booleans, one writer
  each — a single global flag would race when two gestures overlap.
- **R3 — mode-switch flush.** Leaving the text view with an uncommitted buffer
  force-flushes the diff into a real `EditText` op *before* the canvas tab
  activates, so the canvas's first render observes the new generation
  (`code_editor.rs` `commit_editor_buffer`). Entering text mode never flushes.
- **R4 — a removed class closes its tabs.** When a `RemoveClass` op lands,
  every tab drilled into the removed class or a descendant closes
  (`close_drilled_tabs_on_class_removed`, watermarked via `changes_since` so
  it is O(new changes)). Without it the dangling tab falls through to
  first-tab behaviour and renders an unrelated class.

(No R2 is cited anywhere in the tree — the numbering simply skips it.)

### 5.6 Type resolution (MLS §5.3)

Modelica's type lookup follows the rules in
[Modelica Language Spec §5.3 — Static Name Lookup](https://specification.modelica.org/maint/3.7/class-predefined-types-and-declarations.html#static-name-lookup).
Our implementation (used by the diagram panel's AST→canvas rebuild and
by the class resolver on AST-level ops) follows a subset of those
rules:

1. **Fully-qualified path** — a reference containing `.` (e.g.
   `Modelica.Blocks.Continuous.Integrator`) is treated as absolute
   and matched directly against the MSL index by path.
2. **Simple name with import** — a reference without `.` is resolved
   against the containing class's `import` declarations:
   - `import A.B.C;` → `C` → `A.B.C`
   - `import D = A.B.C;` → `D` → `A.B.C`
   - `import A.B.{C, D};` → `C` → `A.B.C`, `D` → `A.B.D`
3. **Unresolved** → the reference is surfaced as a skipped component
   (non-fatal in the diagram rebuild; would be a compile error in
   rumoca's type checker).

**Deliberately not implemented (yet):**

- `import A.B.*;` (unqualified) expansion — needs an MSL index walk
  for every `A.B` child
- Enclosing-class scope lookup (MLS §5.3.1 step 2) — flat top-level
  package is the only source of types today
- Short-name-tail heuristics (e.g. match `Integrator` to the first
  MSL entry whose path ends in `.Integrator`) — rejected as unsafe;
  multiple MSL classes share short names across branches, so a
  suffix match picks whichever loaded first. Not what the Modelica
  spec means by name resolution.

See
[`../../crates/lunco-modelica-ui/src/ui/panels/canvas_projection.rs`](../../crates/lunco-modelica-ui/src/ui/panels/canvas_projection.rs)
(`import_model_to_diagram`) for the call site, and
[`../../crates/lunco-modelica-document/src/document/core.rs`](../../crates/lunco-modelica-document/src/document/core.rs)
(`resolve_class`) for the class-path resolver used by AST ops.

## 6. Modelica public ports

Checked DAE runtime inputs define writable Modelica ports. Numeric observables,
including authored outputs and local states, define readable values that USD may
export. This inventory excludes constants, runtime inputs and parameters, which the bridge
does not publish as participant output samples; a same-named native output keeps
its registered owner. Reading a local state does not change its Modelica equation
ownership or make it writable. See the
[co-simulation public port contract](../../crates/lunco-cosim/README.md#modelica-public-ports)
for initialization, runtime writes, and missing-sample admission.

The USD source dispatcher remains after wiring and authored-default publication
in the ordered Update chain. Native `PendingModelicaSource` membership and the
absence of `SceneLoadInFlight` admit its exclusive World boundary. A settled
scene with no pending sources skips that boundary; deferred source insertion,
recompile and scene replacement wake it through current component membership.
Source arrival queues `PrepareInterface`; its strict compilation uses symbolic
runtime inputs and requires no connected numerical samples. The checked reply
publishes scalar surfaces and effective metadata before input admission, including
readable values that USD consumes without redeclaring them. `PortSurfacePending`
remains until this exact source lease is admitted. Publication supplies no unsolved
output samples. A terminal interface error faults the affected participant instead
of leaving an unresolved readiness wait. Source replacement refreshes the same
checked interface through its owner.

Native port backends publish `PortSurfaceReady` when their declaring component
is installed. This includes celestial `LinkNode` endpoints, whose outputs do not
require a Modelica participant. The shared marker admits the source to USD
wiring and wakes its derived connection cache. Initial sampling still requires
an actual declared finite source value. `CosimStatus` with values included
exposes `pending_initial_inputs` and ordered `initial_input_sources` for
uncompiled participants, separating unresolved connections, pending source
surfaces and missing samples.


### Coupled initial admission

`lunco-usd-sim-cosim` owns admission of connected USD Modelica participants at
`t=0`. Source interfaces and native physical definitions become available before
numerical admission. Acyclic participants initialize from finite producer samples.
Participants with complete initial inputs enter their independent preparation
without delaying a separate coupled plan. Required external outputs still wait
for their registered initial expressions or samples; absent values remain pending.
When the remaining participants depend on one another, the owner captures their
exact source, authored bindings, scene generation and ordered wiring. Capture
waits for the existing domain-discovery hold and every pending port surface,
including generated networks without a published participant. Independently
initialized networks can still add wires to the complete certification snapshot.
Their numerical preparation does not hold coupled capture once declarations
and wiring are available. The owner invokes
the required `synth.coupled-initialization` hook to emit a Modelica
composite containing those instances, their input bindings and input probes.
The Rust owner supplies complete input groups; Rhai emits nested array literals
in their checked order as whole-input modifiers. Participant parameter values
travel separately as qualified `Initialize.parameter_overrides` names such as
`p0.k` or `p0.gains[2]`, through the shared compiler/lowering admission. They are
not injected as member source modifiers. Indexed tunables retain scalar lowering
semantics; indexed structural overrides remain unsupported by the root-modifier
owner. A requested binding whose aggregate name itself contains an index is
also rejected because the source modifier cannot represent that target.

The current USD scalar-port authoring API cannot name indexed Modelica scalar
ports. Aggregate metadata and policy emission do not extend that authoring API.
The production `modelica_initialization_arrays` fixture exercises the shipped
Rhai emitter directly with checked input groups and incomplete/excess negative
cases; it does not claim indexed USD scene wiring coverage.

The existing compiler and preparation pipeline compiles and initializes the
composite as a request-owned job. Its identity contains the scene generation
and a sequence within that generation. The worker returns checked finite
observations, solver settings and preparation timings under that request identity, without
installing a live session or publishing a scene model. Both native hosts use
Rumoca's configured execution backend for the transient session. The browser
uses its supported execution backend. Participant sessions are constructed
independently from the same checked-DAE contract. Both jobs share checked
artifacts and bounded worker lanes.
Native and browser transports keep initialization replies
separate from participant lifecycle results.
The `Update` lifecycle submits interface preparation and captures the initial
plan against settled USD wiring. `SimulationCompletions` drains the preparation
mailbox, publishes checked interfaces, accepts a ready solve, and certifies the
participants after their construction results arrive. Both hosts use this
reader while the simulation clock is held. Admission
resamples the current providers and compares the complete plan once an outcome
is available. A pending worker result leaves the existing hold in place.
Scene retirement cancels the exact request; late replies cannot authorize a
different generation. Failed admission revokes unconsumed solved inputs and
participant holds, terminates pending member operations and raises the owner fault.

The physics owner's initial joint-island membership includes joints temporarily
disabled by its readiness freeze, using that freeze's exact ownership marker.
Authored disabled joints remain excluded. Live mass sampling still follows active
Avian links; releasing readiness does not change the initialization topology.
Marker insertion/removal invalidates the same scene-owned membership cache.

Compile admission retains pending requests in source-document/entity/class order.
`lunco-modelica-worker` runs `InitializationSampling` once for that ordered batch,
on its first eligible USD participant. Each target still reads its own complete
input transaction through `initialization_inputs`; the shared pass only refreshes
read-only physical providers at the same initial coordinate. Missing input samples
retain the request before source copying, parameter extraction or dependency
traversal. Every later batch revalidates the source lease, live participant,
document and input observations. A failed sampling schedule rejects eligible
requests visibly; a retired request publishes nothing.

The hook takes typed member/class/binding/probe facts and returns a typed map with
`source`. The application policy registry installs `initialize` from
`synth_acausal_network.rhai`, sharing its instance declaration emitter with acausal
synthesis. The call inherits Twin lifecycle ownership, Preparation phase and no
simulation clock. A missing policy, malformed result or unsatisfied initial system
raises `modelica-initialization-failed` and holds the affected participants.

A worker result is accepted only against the captured scene, sources, authored
values, physical samples and wiring. Its finite input and source-state observations
enter live construction together through one-shot `ConsistentInitialInputs`.
The worker validates the state inventory against the checked participant DAE.
Rumoca identifies free initial state coordinates through the checked initialization
owner; these have no authored given value, projection writer or update writer.
Only these coordinates can receive an accepted-state constraint. Rumoca validates
identity, domains and duplicate requests before comparing values bitwise with the
checked prepared `initial_y`. Matching values require no new owner; an all-identity
request preserves the original component, initialization and event program.
Nonredundant constraints acquire given ownership only after the component passes
the continuous-state capability guard. Every original initial residual remains
checked. Per-instance constraints do not overwrite the shared cache or act as
start guesses.
Ordinary authored initialization carries no coupled state admission. A coupled
admission with a missing required free coordinate is an error, including an
explicitly empty supplied inventory.
All members remain paused
under `CoupledInitializationPending` until their own solvers are constructed at
`t=0` and the actual connected observations agree with the composite within the
active solver tolerances. Pending participant constructors close the coordinator's
readiness gate until every member has an outcome; errors or retired construction owners wake
it immediately. Full certification then samples and validates the current graph
before releasing the members together. A changed
source or wire cannot release an old certificate; failed certification reports the
same terminal runtime fault. Scene teardown cancels the preparation job and retires
its admission state. Settled scenes skip the preparation coordinator.

Nonredundant free-state constraints support continuous Real coordinates whose
initialization has no event, pre/history, clock or chart-transfer requirement,
and no initialization-owned parameter unknowns. Rumoca refuses unsupported
changes with the coordinate name, requested value, prepared value and component
capability facts. Identity requests require no feature transfer. Locally
determined participants retain their initialization equations. This is a checked
coordinate constraint contract, not cross-instance FMU snapshot import. The worker
and coordinator verify every supplied state observation after initialization
events, including normalized identities, as well as connected inputs before
release. Preserving an event program never bypasses that final certification.
Changed parameters or inputs that contradict an admitted initial solution require
a fresh coupled admission; they cannot silently replace that solution on rebuild.

Successful certification retains a scene-scoped diagnostic receipt in
`CosimStatus.last_coupled_initialization`: scene generation, solver identity
and worker construction timings. This is the last completed
initialization, not a permit for a later graph. Failed or superseded work cannot
publish a successful receipt; teardown removes it. The
`modelica_initialization_network` production Rhai gate checks the retained receipt
alongside initial values, communication rates, pause/resume and clock behavior.

Modelica execution also publishes the scene-scoped `ModelicaHealthSnapshot`
from the same admitted step counters used by `CosimStatus`. Its one-second
native wall-clock window reports completed-step FPS and worker service time
through the shared `engine-health` exposure. See the
[telemetry health contract](telemetry-subsystem.md) for aggregation, pause,
unavailable samples and teardown semantics. This continuous diagnostic math
does not change simulation scheduling or require authored selection policy.
The production `modelica_perf_hud.rhai` gate covers running, paused and cleared
states, including absence of measurements after the participants retire.

`lunco-usd-avian-joints` seats native joint definitions after validated initial
body poses and installs them disabled. `PhysicsJointPrepared` keeps those
measurements available while body readiness prevents solver-island admission.
Activation waits for real Avian body nodes and removes only the owner's temporary
disable. `lunco-physics` computes initial articulated membership in stable
`PhysicsOrderKey` order. The Avian port backend exposes island mass as a typed
`PortInitialExpression` over member mass inputs; the coordinator follows those
inputs through their wires into Modelica equations. This uses the actual topology
and modeled mass relations rather than a sampled or estimated assembly mass.
`PortRegistry` selects this relation through the same declared owner as reads;
an owner distinguishes a pending relation from an independent numerical sample.
Pending initial topology holds admission even when a current active-island sample
is available. An independent source must provide a real finite sample.

This lifecycle follows FMI 3.0's separation of Initialization Mode and running
communication: initialization loops are solved at one physical coordinate before
participants advance. See the standard's [algebraic-loop rules](https://fmi-standard.org/docs/3.0.2/#algebraicLoops)
and [initial unknowns](https://fmi-standard.org/docs/3.0.2/#ModelStructure).
The composite implementation applies to source-backed Modelica participants.
Running causal exchange retains its explicit communication-step semantics; this
change does not establish FMU archive interoperability or conformance. Same-time
coupling of opaque Co-Simulation FMUs requires their declared state/rollback and
step capabilities; it cannot be obtained by repeatedly reading output ports.

The production scenes `modelica_initialization_network.usda` and
`modelica_initialization_failure.usda` exercise consistent feedback admission and
terminal rejection of inconsistent initial equations respectively. Their shared
Rhai gate observes the real participant ports and lifecycle. Twin acceptance stays
in the Twin's authored tests.

Persistent compiled-DAE artifacts include the exact clean Rumoca build identity
in their key and validated record. Unknown or dirty compiler identities disable
persistent reuse. Rumoca sessions own solver state per participant.

The default running communication interval is one master physics tick. Longer
integer tick multiples are explicit multirate settings. Each worker request
admits its ordered input vector atomically, and the shared clock waits for the
complete validated response. Rumoca's native evaluator also covers zero-state
algebraic components; their empty continuous state does not disable compiled
output evaluation. The positive initialization scene additionally exercises
connected participants using the default cadence and an authored 30 Hz period.

## 7. Type vs. instance distinction

Every `.mo` file defines a **Modelica type** (`model Balloon`, `model SolarPanel`).
A `ModelicaModel` component on a Bevy entity is an **instance** of that type.

| Concept | Lives in | Analogy |
|---------|----------|---------|
| Type (definition) | `models/Balloon.mo` — single file, one per model | Rust `struct` declaration |
| Instance (running) | Bevy entity with `ModelicaModel { model_name: "Balloon", ... }` | Rust instance of that struct |

- Editing the type (changing `Balloon.mo`) affects **all** instances on
  the next recompile.
- Authored USD bindings and compile-time overrides select values for one
  instance; they are distinct from the source declaration.

The document-backed Inspector parameter control uses `SetModelicaParameter`.
It edits the declaration in the participant's exact selected class, leaves the
document dirty, and reinitializes that participant with the checked replacement.
The edit is a source change and becomes a type default when saved. Other
instance overrides remain intact. USD-backed controls require editing their
authored USD binding and are disabled in the document-only inspector path.
The selected participant's Modelica Parameters section identifies its class,
shows its source URI and Modelica state, and opens the class through the typed
`OpenClass` view command. The source URI is provenance; the fully qualified
class is the navigation identity. A Modelica failure exposes its owner error
in the Inspector. “Compiled” describes the compile result and does not claim
that a participant is currently stepping.

## 7a. Document identity — three tiers, one truth

A Modelica document exposes three identity views:

| Tier | Source of truth | Used by |
|------|-----------------|---------|
| **File** | `Document::origin` (`Untitled{name}` or `File{path, writable}`) | Save logic, dirty check, Files browser |
| **Workspace entry** | `WorkspaceResource.DocumentEntry.title` | Tab label, Recents |
| **Modelica class** | AST top-level class name (in source) | Compile, references, drill-in, Class browser, journal entries |

**Modelica's class-first identity is authoritative** — same as Dymola
and OMEdit, where the class name is what you see in tabs and the
file is filesystem implementation detail. Workbench follows this
convention with VS Code's untitled handling for the unsaved case.

### 7a.1 Title derivation

`DocumentEntry.title` is **derived**, not stored. A `Last`-schedule
system recomputes it whenever `(origin, ast_first_class,
drilled_in_class, dirty)` changes:

```
title = derive(origin, ast_first_class, drilled_in_class, dirty)

  primary  = ast_first_class            (Modelica name lookup wins)
           | drilled_in_class           (multi-class file → focused class)
           | origin.display_name()      (parse failed → fall back)
           | "Untitled-N"               (no source yet)

  prefix   = drilled_pkg.drilled_class  (if drilled into a sub-class)

  postfix  = "●"                        (dirty)

  style    = italic                     (origin.is_untitled || dirty)
```

Multi-class file shows `Pkg.Active` on the tab plus a
breadcrumb above the active editor (Dymola-style: `Pkg ▸ Active`,
each segment clickable to drill out). Switching drilled class flips
the tab label without remounting the document.

### 7a.2 Rename behaviour

`RenameModelicaClass`:
1. Rewrites the source declaration + `end OLD;` closer (string-level,
   first match only — multi-class files require explicit
   class-targeted renames in v1).
2. If the doc origin is `Untitled`, updates `origin.name` to match.
   Title derivation picks up the new class name automatically next
   frame.
3. **If the doc is File-backed**, behaviour is governed by the
   `modelica.naming.rename_class_renames_file` setting:
   - `Always` — rename the `.mo` file in lock-step (Dymola default).
   - `Ask` — prompt the user; default for the workbench.
   - `Never` — file stays at its old path; class name and filename
     diverge until the user does a Save-As (VS Code default).

The setting lives in the `modelica.naming` section of
`settings.json` (see `11-workbench.md` § 9b.2). Per-twin overrides
let library projects pin `Always` while luncosim twins keep `Never`.

### 7a.3 Save-As default

When an Untitled doc transitions to File via Save-As, the suggested
filename is `<ast_first_class>.mo` in the active Twin's models
folder, governed by `modelica.naming.save_as_default_uses_class_name`
(default `true`). Users can override the suggested name; the setting
just controls the default.

### 7a.4 Implementation notes

- `WorkspaceResource.DocumentEntry.title` becomes a derived field —
  the system that maintains it reads from
  `(DocumentRegistry<ModelicaDocument>, drilled_in_class, dirty)` and writes
  the entry. No call site sets `entry.title` directly any more; that
  was the source of the drift.
- The italic + dirty-dot styling is handled by the tab renderer
  (`lunco-workbench`) reading `UiSettings + DocumentEntry.origin +
  dirty`. No per-domain logic in the renderer.
- `RenameModelicaClass` no longer needs to touch
  `WorkspaceResource` directly — the title-derive system picks up
  the AST change. The title is derived in the Modelica UI projection from
  the document index and origin.

## 8. New-model workflow (target)

1. **File → New Modelica Model** (or Ctrl+Shift+N in Analyze workspace)
2. Dialog: name, kind (`model | block | connector | package`), template
   (empty, from MSL, copy existing), location (project `models/` folder
   by default)
3. New `ModelicaDocument` created in memory, opens in Analyze workspace
   with empty Diagram + skeleton source
4. User edits via Diagram (drag MSL components, connect ports) or Code
5. Each edit is a `ModelicaOp` applied to the document; views re-render
6. Ctrl+S saves the document as a `.mo` file; Library Browser refreshes
7. To use the model: drag it from Library onto viewport, or right-click
   an entity → "Attach Modelica model"

The current workflow keeps Code Editor, Diagram, Telemetry, and Inspector on the
same `ModelicaDocument`/operation pipeline:
- Code Editor and Diagram edits share the document projection and reparse
  boundary (see § 11 Gaps for the remaining visual polish)
- Parameter edits from both Model State and Component Properties use
  `ModelicaOp::SetParameter`, so the source, journal, projection, and compiled
  model observe one authoritative change

## 9. The Modelica diagram editor

The diagram panel (`crates/lunco-modelica-ui/src/ui/panels/canvas_diagram/`)
renders on top of `lunco-canvas` — the workbench's own canvas
substrate. The panel is a thin *view* over a `ModelicaDocument`: the
document is the authoritative state, the canvas scene is a rendered
projection.

```
         ModelicaDocument (source + cached AST)
                   ▲             │
         AST ops   │             │  changes_since(gen)
  (drag, connect,  │             │  (TextReplaced,
   delete, move,   │             │   ComponentAdded, …)
   paramedit)      │             ▼
                  CanvasDiagramPanel ◀──── canvas_projection
                   │
                   ▼
                  lunco-canvas Scene  (renders nodes / wires;
                                       owns pan/zoom/selection)
```

### 9.1 Sync flow

Each frame:

1. **Open-model rebind** — if `WorkbenchState.open_model.doc` changed,
   the panel resets the change-stream cursor so the next sync does a
   clean rebuild.
2. **Document → scene projection** — if `doc.generation() !=
   last_seen_gen`, parse and project the source on the compute task pool.
   The UI thread only takes short cache/read locks; a root or inheritance
   miss stays in an explicit loading state and is retried from the generic
   source-root completion event. No Modelica parse or inheritance walk is
   allowed to block canvas painting. The graph, merged icons, traced unit and
   semantic zoom previews use the selected document's exact syntax revision.
   When a workspace declaration competes with it, projection uses an isolated
   Rumoca view that gives the selected declarations priority and retains
   unrelated dependency classes.
   Source-root authoring mirrors use shared `AsyncWorkAdmission`; when its
   bounded queue is full, the worker response and exact source snapshot remain
   ordered for retry after capacity or source-operation revision changes.
3. **Canvas render** — user interaction happens in `lunco-canvas`; it
   owns pan/zoom/selection/drag state between frames.
4. **User action → op emission** —
   - Right-click Add Component → `AddComponent`
   - Right-click Delete → `RemoveComponent`
   - Wire draw/disconnect → frame-to-frame wire-set diff →
     `AddConnection` / `RemoveConnection`
   - Drag-to-move → one `NodeMoved` event on release → `SetPlacement`
     with the live icon extent preserved; authored wire waypoints move with
     their node, while generated routes are recomputed from topology.
5. **Apply + echo suppression** — pending ops are applied to the
   `DocumentHost`, `last_seen_gen` is advanced past our own
   generations so step 2 doesn't rebuild in response to edits we
   just made.

Text edits in the Code Editor flow through the same pipe: the
editor's debounced commit (≈ 350 ms idle or focus-loss) calls
`checkpoint_source` → `ReplaceSource`, the generation bumps past
`last_seen_gen`, and the diagram rebuilds on its next frame.

That path is only for user-owned edits. Disk reloads and generated Modelica
projections use `DocumentRegistry<ModelicaDocument>::reload_external_source`, which
funnels through the shared `FileBacked::reload_base` contract. The external
source replaces the resident text and advances the normal invalidation
generation, but it does not enter the editor undo stack or Twin journal: the
file or USD synthesis owner is already authoritative. Formatting and other
interactive commands remain typed `ModelicaOp` operations on the host, so they
are undoable and journalled like code-editor edits.

### 9.2 Visual details

- Resolved classes render their authored Modelica `Icon` graphics, including
  inherited standard-library graphics; an unresolved class is an explicit
  diagnostic state, not an invented component card.
- Connector graphics, color, causality, placement, and `flow` metadata come
  from the resolved connector AST. The renderer is not a battery/motor/solar
  palette.
- Graphical terminals are public instances of resolved `connector` classes,
  including short connector definitions and inherited connectors. Plain
  `input`/`output` variables remain in the semantic interface and telemetry;
  causality alone does not create a connector icon. Component libraries author
  terminal positions and sizes through standard Modelica `Placement` annotations.
- Authored connection `Line.color` takes precedence over the resolved
  connector `Icon` color; an omitted color uses the neutral theme wire token.
  Generated-network route annotations carry geometry only. The canvas legend
  lists exact qualified connector identities and resolved causality. Wire hover
  exposes endpoint values and declared flow. Names do not classify physical
  domains or select colors.
- A live non-zero declared `flow` value animates directional dots on the
  actual routed polyline. Multiple flow variables are supported; the first
  active declared flow is selected, and a missing/non-flow scalar stays idle.
  The animation walk is O(route segments + dots) and allocates no strings in
  the per-frame lookup path.
- The scene is editable only when the document is editable. Dragging an
  authored document writes the standard Modelica `Placement` annotation.
  Generated documents remain read-only because USD plus Rhai is their source
  of truth; the banner's `Duplicate to edit` action creates the normal
  workspace document before a user moves nodes.
- Dot-grid background, pan/zoom, selection, port hit targets, and routed
  wire handles come from the generic `lunco-canvas` substrate.
- Generated diagram overlays (hover cards, wire tooltips, loading/error cards, and the
  connection legend) are owned by the canvas leaf. Direct painting uses its
  clip rectangle and an `egui::Area` overlay uses `constrain_to` with that same
  measured rectangle; no diagram surface may escape into workbench menus,
  status, inspectors, or neighbouring leaves. The legend intentionally claims
  input only for its own checkbox; other overlays do not claim pointer input,
  while intentional Modelica dialogs remain in the shared modal host.
- The diagram's `Connections` legend is the sole owner-facing control for the
  per-tab `Show nets` presentation state on that canvas. The legend remains
  visible while nets are hidden, and the same state controls connection
  rendering and edge hit-testing while retaining authored scene topology
  unchanged; it is workbench/egui state because runtime HUI/Rhai has no
  dynamic diagram graph control contract.

### 9.3 Why our own canvas

The diagram panel uses `lunco-canvas` because it provides:

- **Ports on every side** — schematic-style placement (top, bottom,
  left, right), not just left/right inputs/outputs.
- **Acausal connectors** — Modelica electrical / fluid ports are
  bidirectional; node-graph libraries built around `OutPin → InPin`
  edge direction don't fit.
- **Animation hooks** (see § 9c) — render-side tweens, glow,
  per-origin policies need access to the draw loop, which a
  third-party node-graph crate doesn't expose.
- **Grid snapping, custom shapes, multi-domain reuse** —
  `lunco-canvas` is the substrate for non-Modelica diagrams too
  (mission planner, cosim graphs).

Alternatives we evaluated and rejected for the workbench-wide
canvas substrate:

| Alternative | Verdict |
|-------------|---------|
| `egui_node_graph` | Node-graph oriented, not schematic |
| `egui_node_editor` | Less documented |
| `egui_graph` | New, less battle-tested |
| `egui-snarl` | What we started on; lacks port-side and acausal wires; no animation hook surface |
| Forking `egui-snarl` | ~50–200 LOC patch but ties us to upstream forever |
| **Custom `lunco-canvas`** | Picked — owned, extensible, animation-ready, no upstream coupling |

## 9c. Canvas animation + multi-user roadmap

The Modelica canvas is a Miro-style diagram surface — components,
connections, free placement. This section captures how we want it to
*feel* (animated, alive) and how that scales to multi-user later. The
guiding principle is Figma's: **animate the change, not the state.**

### 9c.1 Op origin tag

Every structural mutation funnels through `apply_ops_public` and
arrives carrying an origin:

```rust
pub enum OpOrigin {
    /// Mouse drag, keyboard, paste — user already saw the action,
    /// no animation needed.
    Local,
    /// API / agent / test script — animate so the viewer can follow
    /// what's happening.
    Api,
    /// Future: incoming op from a collaborator. Animated, with that
    /// user's color.
    Remote { user_id: UserId },
}
```

Origin is threaded through `apply_ops_public(world, doc, ops, origin)`
and recorded alongside the op in a `RecentChanges` ring buffer:

```rust
#[derive(Resource, Default)]
struct RecentChanges {
    entries: VecDeque<RecentChange>,  // bounded ~256
}
struct RecentChange {
    doc:    DocumentId,
    op:     ModelicaChange,            // structural summary
    origin: OpOrigin,
    at:     Instant,
}
```

Render-side systems read `RecentChanges` to decide what to animate.
The journal subsystem already records the same op surface — origin is
an extra annotation, not a separate event channel.

### 9c.2 Tween primitive

Animation is **render-only**. The source AST `Placement` is the truth
the moment the op applies; the renderer interpolates between *previous
rendered position* and *new placement* over a short window. Source
mutation already happens in one frame — animating the source itself
would corrupt undo, journal, and AST refresh.

```rust
#[derive(Component)]
struct CanvasTween {
    from:     Placement,        // last-rendered pre-op
    to:       Placement,        // post-op (matches AST)
    start:    Instant,
    duration: Duration,         // 0 ⇒ skip / instant
    ease:     EaseKind,         // EaseOutCubic | Spring | Linear
}
```

The render system reads `lerp(from, to, ease(t))` instead of the raw
placement when a tween is active; despawns the tween at `t ≥ 1`.

### 9c.3 Per-origin animation policy

| Origin | Tween | Pulse | Camera focus |
|---|---|---|---|
| `Local` | 0 ms (instant) | none | none — user has the cursor |
| `Api` | `tween_ms` (default 250) | `pulse_ms` (default 1000) | per `add.focus_behavior` |
| `Remote { user }` | same as Api but pulse colored from `user`'s presence color | yes | optional ("follow user") |

The user can override per-call: `AddModelicaComponent { animate: false }`
forces an instant local-style apply even for an API caller, and
`ApplyModelicaOps { animate: true }` forces animation for what would
otherwise be a `Local` mouse-drag batch (e.g. an "import diagram"
scripted action).

### 9c.4 Pulse glow

Figma-style outer glow on newly-added components — a soft ring
around the node that fades to transparent over `pulse_ms`. Implemented
as a transient `PulseGlow { until: Instant, color: Color32 }`
component on the canvas node; renderer adds the glow at draw time and
the system despawns the component when `now > until`.

Color: theme-driven for `Api` origin, user-presence-color for
`Remote`. Pulse style is fixed (outer glow) but `pulse_ms` is
settings-driven, so users can tune intensity / duration or disable
(0 ms).

### 9c.5 Batch focus debounce

A scripted `AddComponent × N` (the rocket-build flow) shouldn't
ping-pong the camera. Strategy:

1. Each `AddComponent` with `Api` origin schedules a single-component
   `Center` focus.
2. If another `AddComponent` arrives within
   `add.batch_debounce_ms` (default 200), cancel the per-component
   focus.
3. After `batch_debounce_ms` of idle, fire one `FitVisible` over the
   accumulated set.

So a 10-component build animates each spawn (with pulse) but only
runs one camera move at the end — frames the whole diagram for the
viewer.

### 9c.6 Camera tween

The per-component `Center` and end-of-batch `FitVisible` use a
camera tween that interpolates `(pan, zoom)` toward the target over
`tween_ms`. Same ease curve as node tweens for consistency. The
existing `SetZoom` / `PanCanvas` commands set the camera directly;
the animation system layers a smooth-pan above them so manual
`SetZoom` from a script also has the option to animate.

### 9c.7 Settings tree

Following the `11-workbench.md` § 9b multi-level convention. Defaults
shown.

```
modelica.canvas.animation.tween_ms          u32   250
modelica.canvas.animation.ease              enum  ease_out_cubic | spring | linear   default ease_out_cubic
modelica.canvas.animation.pulse_ms          u32   1000
modelica.canvas.animation.local_origin      enum  Instant | Animated  default Instant   (you already see it)
modelica.canvas.animation.api_origin        enum  Instant | Animated  default Animated  (script readability)
modelica.canvas.animation.remote_origin     enum  Instant | Animated  default Animated  (future, multi-user)

modelica.canvas.add.focus_behavior          enum  None | Center | FitVisible   default Center
modelica.canvas.add.batch_debounce_ms       u32   200

ui.reduce_motion                            bool  false
                                            (when true, all tween_ms → 0; honours OS prefers-reduced-motion)
```

`ui.reduce_motion` is the global override — accessibility +
matches macOS/iOS conventions. Mirror it from the OS preference at
startup, allow user override in the Settings panel.

Per-call API override: every structural-edit command (`AddComponent`,
`ConnectComponents`, `ApplyModelicaOps`) gains an optional `animate:
Option<bool>` field. `None` ⇒ read from settings; `Some(true)` /
`Some(false)` ⇒ override.

### 9c.8 Presence (deferred — multi-user precursor)

Pre-CRDT, but useful even single-user (paired with an agent
co-pilot):

```rust
struct CanvasPresence {
    user_id: UserId,
    color:   Color32,                   // stable hash of user_id
    cursor:  Option<CanvasPos>,
    selection: HashSet<ComponentName>,
    drilled_in_class: Option<String>,   // they're focused on Pkg.Sub
}
```

Broadcast over a presence channel separate from doc state — matches
Yjs's `awareness` separation. Each remote presence renders as a
colored cursor + ghost-selection rectangle. Stale entries decay
after 30 s without ping. `OpOrigin::Remote` reuses the same color.

Settings:

```
modelica.canvas.collab.show_remote_cursors   bool   true
modelica.canvas.collab.show_remote_selection bool   true
modelica.canvas.collab.user_color            "auto" | "#RRGGBB"   "auto"
modelica.canvas.collab.follow_user           Option<UserId>       (camera follows that user)
```

Status: **deferred**. Lives in this spec for shape; not yet built.

### 9c.9 CRDT-backed source (deferred — full multi-user)

Two approaches were evaluated:

**(a) Text CRDT on the `.mo` source (Yjs `Y.Text` / `yrs`)**
- Pro: works for any future text-shaped doc kind with no domain
  changes.
- Con: structural ops (`AddComponent`, `ConnectComponents`) become
  bursts of character inserts on the wire; the journal loses its
  "Alice added a Pump" granularity unless we re-derive from a diff.

**(b) Structural CRDT over the AST (preferred)**
- Each top-level class is a `Y.Map`. Components a `Y.Map<name,
  ComponentDecl>`. Connections a `Y.Array<ConnectEq>`. Annotations
  stay text-CRDT'd internally.
- Render → text via the existing pretty-printer; persist that text
  on disk so non-collaborating users still get readable `.mo`.
- Pro: structural ops stay structural across the wire. Remote
  `AddComponent` becomes a single "Alice added Pump" event with the
  same animation path as `OpOrigin::Api`.
- Con: more upfront work, but aligns with the existing `ModelicaOp`
  vocabulary.

**Decision: (b)** when the work lands. `OpOrigin::Remote` plugs in
directly — same animation code, different origin tag. The journal
becomes a shared event log: each user's edits flow into a single
ordered history (Lamport timestamps), which dovetails with the
Twin-journal subsystem in `13-twin-and-workflow.md` § 5a and the
SysML v2 REST API path in that same doc.

**Library choice (deferred):** `yrs` (Rust port of Yjs, same wire
format) — picking it later means a future web-collab room "just
works" against a JS Yjs server. Automerge is the alternative but
slower in our shape.

**Server (deferred):** WebSocket relay snapshotting to the Twin's
`.lunco/journal/` is the leading option; simpler to persist than
WebRTC P2P and reuses the journal store. Spec lives in the
twin-journal doc; not in scope here.

### 9c.10 Implementation order

1. **Layer 0 — Tween primitive.** `CanvasTween` component, render
   interpolation. Single-user, no behavioural change for `Local`
   origin yet.
2. **Layer 1 — Op origin tag.** Thread `OpOrigin` through
   `apply_ops_public`, populate `RecentChanges`.
3. **Layer 2 — Pulse + auto-focus.** `PulseGlow` component, camera
   tween, batch debounce. This is the demo-worthy quick win — turns
   a scripted rocket build into something visually beautiful.
4. **Layer 3 — Presence.** Cursors + selections over a websocket
   channel.
5. **Layer 4 — CRDT.** `yrs`-backed structural CRDT, journal merge.

## 10. Panels (current + planned)

| Panel | Current | Notes |
|-------|---------|-------|
| **Diagram** | ✅ Working, authored Modelica icons, routed connections, editable placements | `canvas_diagram/`, on `lunco-canvas` |
| **Code Editor** | ✅ Working | 423 LOC, plain egui TextEdit |
| **MSL Palette** | ✅ Working | ~20 MSL components |
| **Library Browser** | ✅ Working | File tree of available `.mo` files |
| **Package Browser** | ✅ Working | Installed source-library roots and structured engine Modelica packages |
| **Telemetry / Parameters** | ✅ Working | Runtime values, inputs, plotting toggles, and document-backed parameter edits |
| **Inspector** | ✅ Working | Context-aware selected-component editor backed by `ModelicaOp::SetParameter` |
| **Graphs** | ✅ Working | Live and completed-run trajectories via shared `lunco-viz` rendering |

The Twin Browser's **Modelica** section uses the shared source store for the
configured source library and structured engine Modelica packages, including
the engine packages when no external source library is installed, and lists
every `.mo` file indexed by the active Twin, including files that have not yet
been opened as documents. Native source inventory combines the configured
optional library root with the engine's `assets/models` root, so authored `package.mo`
files determine each package identity and nested package paths remain under
their declared root. The source store retains the configured library root as
explicit metadata, so the engine root does not become the indexer's or
settings panel's primary library when no configured library is present. The
shared package-tree provider scans every installed source root.
An active filter completes candidate roots on background workers before
presenting matching paths. Twin paths come from `Twin::files()` and open
through the existing Modelica file loader with the emitting Twin's absolute
source path; read-only source ownership comes from the shared source store.
The same filter searches Twin file paths and classes already indexed by open
Twin documents; rows show open, modified, parsing, parsed, and parse-error
state from their owning documents. Unopened Twin files remain searchable by
path without being parsed as a side effect of browsing. The section's search
prompt names these scopes, and read-only library roots are grouped separately
from Twin files. A parse-error row opens its source and focuses Modelica
Diagnostics.

The Twin Browser's **Scene files** view reads the loaded scene's resolved
dependency closure and opens its `.mo` entries through the Modelica file
dispatcher. The selected Modelica participant's Inspector also offers an
**Open model** action using its exact fully qualified class identity. These
paths reuse the existing dependency resolver and Modelica class navigation;
they do not maintain a second scene-to-source index.

## 11. Current gaps

The following issues are tracked as implementation work, not architectural
decisions:

**Typed material and surface-finish binding**: Modelica does not yet resolve a
selected SysML material/finish record into typed, unit-preserving constitutive
properties. The required path must preserve grade/form/temper or directional
layup, applicable temperature/environment, and source status; a finish is a
separate assignment with its own thermal/optical data. `UsdShade` appearance
presets are not property models. Until the generic resolver and law mapping
exist, do not copy material constants into `.mo` files or infer engineering
properties from a render shader.

### P0 — Blocking

**Diagram ↔ Code synchronization**: The Diagram and Code editor share a single `ModelicaDocument`. Edits in either panel flow through the document and update the other on the next frame. Opening a file from the Library Browser populates both views from the same source. See § 5 and § 9 above.

**Acausal connector visual** (in progress on `lunco-canvas`):
Modelica electrical / fluid connectors are acausal — wires shouldn't
have an arrow direction. The migration off egui-snarl unblocks this;
the rendering work to drop the directional arrow on connector wires
is tracked separately on the canvas crate.

### P1 — Degrading workflow

- **No initial conditions in VisualDiagram** — `ParamDef` only stores a
  single value, not `start`, `fixed`, `min`, `max`.
- **No Modelica class hierarchy** in the visual editor — only flat models.
  Subsystems/packages are planned.
- **No simulation configuration UI** — hardcoded solver + tolerances.

### P2 / P3 — Polish

- Undo/redo for diagram ops (comes free from Document System op inverses)
- Component search / filter in palette
- Right-click context menu on components
- Pre-compile validation (unconnected ports, missing ground, cycles)
- Editable model name (currently auto-generated `VisualModel1`, `VisualModel2`)

## 12. Dymola-workflow parity

LunCoSim's Modelica workspace aims at Dymola-familiarity but isn't 1:1.
Feature parity snapshot:

| Feature | Dymola | LunCoSim |
|---------|--------|----------|
| Package browser | ✅ | ✅ |
| Library browser | ✅ | ✅ |
| Diagram canvas | ✅ custom icons | ✅ authored Modelica icons + routed wires |
| Text view | ✅ | ✅ |
| Parameter dialog | ✅ | ✅ (Telemetry + selected-component Inspector) |
| Plot variables | ✅ | ✅ (shared `lunco-viz` plot widgets) |
| Variables browser | ✅ | ✅ |
| Compilation pipeline | ✅ | ✅ (rumoca) |
| Simulation setup dialog | ✅ | ❌ (continuous stepping instead) |
| Live-parameter editing during sim | ❌ | ✅ (LunCoSim advantage) |
| Icon designer | ✅ | ❌ |
| Documentation view | ✅ | ❌ |
| Animation view | ✅ | ✅✅ (3D world IS this — LunCoSim advantage) |

Rough **80 %** feature parity on the core loop. The gaps are solvable
within 2–3 months of focused work; the biggest wins come from the
Document System migration (unlocks live diagram↔code sync) and
finishing the acausal-connector visuals on `lunco-canvas`.

## 13. See also

### Source

- [`../../crates/lunco-modelica-compiler/`](../../crates/lunco-modelica-compiler/) — headless Rumoca session and source-admission host
- [`../../crates/lunco-modelica-core/`](../../crates/lunco-modelica-core/) — document/runtime crate root
- [`../../crates/lunco-modelica-worker/`](../../crates/lunco-modelica-worker/) — stateful worker, live solver, and co-simulation bridge
- [`../../crates/lunco-modelica-execution/`](../../crates/lunco-modelica-execution/) — host plugin, transport, and execution adapters
- [`../../crates/lunco-modelica-document/`](../../crates/lunco-modelica-document/) — `ModelicaDocument`, op set, apply pipeline, and source-editing seams
- [`../../crates/lunco-modelica-api/`](../../crates/lunco-modelica-api/) — transport-free Modelica queries and document edit commands
- [`../../crates/lunco-modelica-ast/`](../../crates/lunco-modelica-ast/) — normalized Rumoca parse boundary, AST projections, and Modelica lint facts
- [`../../crates/lunco-modelica-document/src/document/core.rs`](../../crates/lunco-modelica-document/src/document/core.rs) — `ModelicaDocument`, op set, apply pipeline, span-based patch helpers, qualified-path `resolve_class`
- [`../../crates/lunco-modelica-core/src/pretty.rs`](../../crates/lunco-modelica-core/src/pretty.rs) — subset pretty-printer, `PrettyOptions`
- [`../../crates/lunco-modelica-ui/src/ui/panels/canvas_projection.rs`](../../crates/lunco-modelica-ui/src/ui/panels/canvas_projection.rs) — diagram panel, sync-from-document, wire/position diffing, scope-aware type lookup
- [`../../crates/lunco-modelica-ui/src/ui/panels/code_editor.rs`](../../crates/lunco-modelica-ui/src/ui/panels/code_editor.rs) — code editor, debounced commit (`EDIT_DEBOUNCE_SEC`), word-wrap toggle

### Adjacent docs

- [`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md) — cosim-level engineering docs
- [`10-document-system.md`](10-document-system.md) — shared Document System foundation
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — how Modelica files live inside a Twin
- [`14-simulation-layers.md`](14-simulation-layers.md) — Twin / Scenario / Run / Model lifecycle the Modelica stepper participates in
- [`15-adaptive-fidelity.md`](15-adaptive-fidelity.md) — multi-clock + LoD (future)
- [`22-domain-cosim.md`](22-domain-cosim.md) — cosim pipeline ordering
- [`23-domain-environment.md`](23-domain-environment.md) — how environment (gravity, atmosphere) flows into Modelica inputs
- [`24-domain-sysml.md`](24-domain-sysml.md) — SysML as the structural peer; references Modelica models as realizations
- `specs/014-modelica-simulation` — detailed spec

### External references

- [Modelica Language Specification §5.3 — Static Name Lookup](https://specification.modelica.org/maint/3.7/class-predefined-types-and-declarations.html#static-name-lookup) — the scope/import resolution rules our type lookup follows
- [Modelica Language Specification §18 — Annotations](https://specification.modelica.org/maint/3.7/annotations.html) — `Placement`, `Line`, `Icon` annotation shapes our pretty-printer emits
- [rumoca on GitHub](https://github.com/CogniPilot/rumoca) — the parser + runtime crate family

Live participants retain their admitted document owner. Closing the owner sends
worker Despawn and retires solver state, sample streams, telemetry, and pending
compile results. Editor-created actors are despawned; a USD projection keeps its
geometry while its Modelica participant is removed. A per-entity session epoch
survives participant removal/recreation and prevents an old worker response
from matching the replacement participant. Captured compilation outcomes and
diagnostics release their user-source overlays; application libraries retain
their own lifetime. Generated domain documents carry the exact source mount's
runtime attribution before first compile admission, including replicated scenes.

### Declaration-scoped input defaults

Original input bindings remain in admitted Modelica source. The patched Rumoca
DAE phase initializes external input slots at fully qualified paths; internal
and connected inputs retain equation ownership. Live, reset, and batch solvers
consume that same DAE initialization, including parameter expressions. The
Editor compile dispatch retains runtime overrides and admits source parameter
metadata only from the selected class. It never seeds input values from the
document-wide class index. The production gate
`modelica_scoped_input_defaults.rhai` checks distinct class
defaults, repeated instances, overrides, reset, and invalid-default diagnostics.

Compose a component with bound inputs through instance modifications when the
parent supplies those values. An additional equation does not replace the
declaration binding. Sensors, guidance blocks, rover drivetrain models, and
the acausal-network synthesis policy use those modifiers for supplied inputs,
retaining each component's standalone defaults and the parent formulas. The
scoped-defaults gate checks both modifier replacement and rejection of a
duplicate equation. See the
[Modelica modification rules](https://specification.modelica.org/maint/3.6/inheritance-modification-and-redeclaration.html).

## External resource addressing

Modelica contributes language semantics to the common
[asset system](56-asset-resolution-and-cache.md#source-libraries-and-resource-views).
`lunco-modelica-ast::resource_uri` interprets MLS 13.5 URIs, including
case-insensitive schemes, qualified package authorities, percent encoding and
separate fragments. Rumoca validates qualified names and supplies declaration
facts. URI decoding does not apply to internal literal asset addresses.

The compiler publishes `SourceFileFacts` from the exact admitted source-set
ASTs. Source provenance stays attached to each file; package membership is a
projection across those facts, including classes in separate files.
`SourceRootRegistry` retains them only for the current operation and source
lifetime. Its immutable `ResourceContext` projection maps a resource
to a typed asset location; it does not search storage or read bytes. Installed
library package facts come from the parsed source artifact, including packages
that are absent from the palette index. Missing or ambiguous package owners
reject rather than selecting another same-name library.

For directory packages, resource paths follow the qualified hierarchy even if
nested packages are stored in one `.mo` file. A top-level single-file package
uses the source file's directory. A resource path cannot begin with a class
member name; that member belongs in the qualified URI authority. This mapping
is shared by documentation images, Bitmap annotations and clicked links, as
required by [MLS 13.5](https://specification.modelica.org/maint/3.6/packages.html#external-resources).

`lunco-modelica-library::package_paths` owns installed-library class-to-file
mapping. Widgets do not join cache paths or load files. Bitmap painting uses
the common asynchronous egui asset/image pipeline. Opening or editing a
file-backed package refreshes facts only after its current parse settles.
A directory-backed view overlays its existing source-set owner; saving commits
that file's facts and fences older in-flight admission results. Closing discards
only unsaved facts. Independent document registrations retire on document close;
Twin-owned registrations retire on Twin close. Resource-scope changes invalidate
bytes and textures
before another source can reuse the same displayed URI.

Native file and directory renames relocate the root and each file/package's
provenance, including saved facts and document overlays. Pre-rename load results
are fenced; the existing worker reloads the affected disk source sets after the
rename/class/save transaction. Closing or reopening a view cannot restore a
retired path or namespace.
