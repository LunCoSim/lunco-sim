# Model authoring UX and runtime feedback gaps

**Status:** Current capability report; the requested authoring facades are implemented
**Reviewed:** 2026-09-11
**Reconciled:** 2026-09-11 against `tutorials` `6d9237886` and the current
Twin handover/check catalog.
**Scope:** Generic authoring and live-inspection UX used while building an
articulated vehicle and its USD/Modelica/Rhai Twin. The modeling session is
an evidence source; the requested improvements are reusable platform
capabilities, not vehicle-specific APIs.

## Executive result

The previous P0 release/motion conclusion is stale. Per the current vehicle
state, landing, adapter release, ownership handoff, and rover motion are
working;
the older `NO-VERDICT` run is historical evidence from an earlier Twin revision,
not the current platform backlog. A fresh bounded Rhai run is still required for
qualification evidence, but no new vehicle-specific runtime feature follows
from that old result.

The shared core already has the authoring primitives needed to build the
vehicle study: explicit USD documents and edit targets, typed/journaled USD
operations, isolated previews, selection and transform gizmos, standard joint
frame editing, topology/collision queries, component-bundle plans, strict
connection lint/preflight, semantic input edges, causal tracing, and reusable
Rhai physics-evidence checks. The actual gap is agent ergonomics: a new agent
must currently reconstruct a multi-step workflow from many separate queries and
tool libraries.

The highest-value additions from this report are now delivered in the generic
`model_authoring` Rhai library: one complete assembly context, one readiness
report, one generic scene recipe, one typed port/wiring planner, and one
component publication validator. They return dry plans and path-addressed
findings, while Rust remains limited to the generic composed USD query fields
needed by those facades.

The UX should keep one explicit identity tuple throughout authoring and
runtime evidence:

```text
Twin -> document -> preview/scene -> USD path -> layer/edit target -> generation
```

It should show the dry plan, the exact affected paths, validation/lint results,
projection readiness, backend readiness, and the completion acknowledgement in
one place. This removes the repeated guesswork seen in the modeling session:
whether a command was merely accepted, whether a connection resolved, which
object owned an input, whether a joint was still attached, and whether a visible
part was actually the collision proxy.

No evidence justifies a vehicle-specific Rust builder, a second scene graph, a
custom CAD kernel, a new USD writer, or moving Twin policy out of Rhai. The
vehicle names, dimensions, routes, thresholds, and assumptions belong in the
published Twin; shared tools should consume explicit manifests and standard USD
schemas.

## Agent-ready authoring workflow

Another agent should be able to follow this loop without knowing internal Rust
types or guessing paths:

```text
context(root) -> readiness(root) -> component/assembly plan
  -> preview and path diff -> typed proposal -> reproject -> save
  -> scene recipe -> wiring check -> bounded Rhai run
```

The current primitives plus `model_authoring` cover the deterministic authoring
loop. Remaining work is human-facing evidence presentation and candidate diff,
not another assembly-specific API:

| Workflow need | Existing owners to compose | Smallest useful addition |
|---|---|---|
| Discover an assembly | `authoring_context`, `InspectUsdDocument`, `QueryUsdPrim`, `assembly_audit` | `model_authoring::model_context` delivered; returns identity, generation, children, references, variants, frames, mounts, bodies, joints, colliders, ports, and available actions |
| Know whether it is buildable | `RunLint`/`LintReport`, `assembly_audit`, `connection_preflight`, `physics_acceptance` | `model_authoring::readiness_report` delivered; Twin supplies model policy, the generic facade supplies universal checks |
| Create a simulation scene | `assembly_edit`, reference/variant plans, `waypoint_editor`, camera and program attach commands | `model_authoring::scene_recipe` delivered for references, placement, start state, terrain/environment, camera, route, and program hand-offs |
| Connect Modelica/Rhai/physics | USD `inputs:`/`outputs:`, `AttachProgram`, typed `SetConnection`, lint | `model_authoring::port_graph`/`wiring_plan` delivered with endpoint direction/type validation and one typed connection plan |
| Publish a reusable part | `component_bundle_plan`, typed references/metadata, explicit Save-As | `model_authoring::publish_component` delivered for root, `defaultPrim`, `kind`, schemas, references, provenance, and explicit Save-As |
| Compare alternatives | `DocumentRegistry::fork`, isolated preview, typed plans | Path-based before/after diff including topology, mass, frames, joints, ports, and lint consequences |

All five primary additions are Rhai-only. They fail loudly, carry the
explicit document/edit-target/generation tuple, and return the existing command
names an agent can call next.

## Evidence from the live modeling session

### 1. A bound action was not self-diagnosing

The lander release action was visibly bound to `G`, but the first Rhai
implementation called a helper that used `this` outside the top-level hook
context. The runtime reported only:

```text
on_event() failed: 'this' not bound
```

The action appeared to do nothing until the script was inspected and the live
event path was traced. After moving state ownership to the hook and making the
release helper context-independent, the same semantic edge detached the joint
successfully. The app needs to show the target, semantic intent, correlation
id, handler, and last error directly in the control/inspector surface.

**Current status:** the Twin-local helper and semantic input path are repaired;
this is no longer a generic missing-command finding. The generic UI still does
not present the complete action/owner/postcondition chain in one place.

### 2. Command acceptance was not operation completion

`RestartScene`, intent injection, and joint commands returned accepted results,
but acceptance alone did not prove that the scene had finished loading, that a
joint had detached, or that the rover was controllable. The reliable check
required a second query (`QueryUsdPrim`, `ScriptInspect`, or telemetry) and log
correlation.

The UI/API should distinguish at least:

```text
accepted -> queued -> applying -> applied
                         \-> rejected/failed/stale
```

Each terminal state needs the operation id, document/scene generation, target,
and a short owner-provided reason. A green `/api/ready` must not be the only
indicator when model projection, script execution, or physics admission is
still pending.

**Current status:** typed command acknowledgements and the Rhai
`editor_workflow` checkpoint now distinguish admission from projection/lint
completion, with explicit opt-in autosave. The missing piece is the shared
human-visible evidence surface, not another command status enum.

### 3. Missing USD connections were buried in runtime output

The current composed surface-op scene authored lander GNC connections to:

```text
/SurfaceOps/Lander/Altimeter/Model
```

That prim does not exist in the composed stage, so all altimeter wires were
dropped. The log did contain the exact path-addressed warning, but it was mixed
with graphics and collision messages. The descent controller consequently had
no valid altitude/range input and the fresh run did not establish reliable
touchdown evidence.

The authoring UI should validate every connection against the composed stage
before launch and present the source prim, requested output, expected type, and
nearest valid candidates. Runtime should retain the same diagnostic as a
structured binding failure rather than relying on a scrolling log.

**Current status:** strict `RunLint`, live `LintReport`, and the authored
`connection_preflight` regression now reject missing source prims, missing
ports, direction errors, and type mismatches before they can be mistaken for
runtime success. The remaining UX gap is diagnostic-to-selection/reveal/frame
navigation in the focused USD preview. The historical missing-source path must not be
listed as an unimplemented linter feature; the current Twin still needs a
fresh run proving its authored connections are clean.

### 4. Selection, possession, and semantic target were easy to confuse

When the lander was possessed, clicking the rover did not visibly transfer
control. The root cause was not obvious because the selected object, possessed
object, controller owner, and camera-follow target were not shown as separate
state. A target-scoped intent edge must make these distinctions explicit:

```text
selected:   /.../VehicleB
possessed:  VehicleB controller
intent:     release
owner:      VehicleA lander
camera:     following VehicleB
```

Vehicle-root selection priority and click-through should remain generic. The
UI needs a clear handoff acknowledgement and a visible active-target badge.

**Current status:** selection intent, possession routing, `InspectSelection`,
and causal tracing are generic capabilities. The current vehicle evidence
reaches the second vehicle after release, but the post-release body remains at its
release pose. Keep the runtime lifecycle failure separate from the older
selection-routing symptom; the missing UX is a unified active-target and
postcondition readout.

### 5. Live visual inspection lacked a stable debug contract

The session repeatedly encountered a bad camera framing, invisible/black
materials, floating visual parts, and confusion between visual geometry and
collision geometry. Isolated USD previews and framing are now implemented, but
the live scene still needs a compact inspection mode that can toggle:

- render geometry;
- collision/proxy geometry;
- joints and joint frames;
- local axes and mount sockets;
- active camera target and framing bounds;
- material/texture resolution and source provenance.

Proxy geometry must be explicitly marked and hidden from the normal render. A
debug overlay is preferable to making a white physics object the only visible
proof that a collider exists.

**Current status:** joint frames, body frames, forces, mass/inertia, velocity,
wheel forces, camera, collider, and explode diagnostics now exist as temporary
overlay leases or focused-preview markers. What is still missing is one
discoverable inspection mode that combines those layers with material/source
provenance and makes the selected physical owner obvious.

## Current authoring/tool gaps after reconciliation

### P0 — Root-scoped model context — delivered

`model_authoring::model_context(doc, root, edit_target)` now composes the
exact document description, target resolution, inspection, composed query, and
topology reads for the complete subtree.

The returned record contains document, edit target, generation, projection
state, root and child paths, references and variants, component kinds,
visual/collision ownership, bodies and joints, frames and mount occupancy,
input/output endpoints, material/source references, and the existing plan
functions applicable to each path. It distinguishes authored facts from
derived values and never selects a path by display name.

It returns the identity tuple, generation, all requested assembly facts, and
per-prim actions. The production Rhai scene test covers the complete path,
body, collider, component, and endpoint read. A human panel can consume the
same record later; it must not introduce another status store.

### P0 — One build-readiness report — delivered

`model_authoring::readiness_report` now gives one machine-readable preflight
over the caller-requested topology, physicality, mounts, connections, controls,
and runtime checks.

The report has stable topology, physicality, mounts/joints,
ports/connections, controls, and runtime sections and preserves the existing
check records and exact paths. It must not repair the stage or invent vehicle
thresholds. A Twin passes its explicit policy map; the generic facade supplies
only universal checks.

Omitted sections are explicitly `not_requested`; no policy or vehicle
threshold is invented. The production Rhai test covers a passing report and a
missing-control failure. The human evidence panel remains a later consumer.

### P1 — Preflight connection and topology navigation — partially implemented

Reuse the composed USD reader, standard component audit, topology facts, and
lint. Before simulation starts, validate that each authored connection source
prim exists, has the requested output, and is type-compatible with its sink.
Show the diagnostic at the exact authored consumer path and provide a generic
“select/reveal/frame this path” action carrying the explicit preview/document
identity. Never resolve by display name alone.

`RunLint`, `LintReport`, and the authored `connection_preflight` fixture now
cover the fail-closed validation. The remaining reusable improvement is to
return a finding that can directly select/reveal/frame its exact authored
consumer in the focused preview. The historical missing-altimeter path must
not remain listed as an unimplemented linter capability.

### P1 — Generic scene recipe — delivered

`model_authoring::scene_recipe` now accepts explicit references and placements
for assemblies, terrain/environment, cameras, initial state, routes, and
programs. It returns typed USD operations plus explicit hand-offs to the
existing route and program owners.

It uses existing reference, transform, camera, and attribute mechanisms and
fails on stale generations, invalid assets, and missing scene identities. It
is generic and contains no vehicle policy.

### P1 — Generic component publish flow — delivered

`model_authoring::publish_component` now provides the reusable publish
validation and dry plan around the existing component/editor owners:

1. validate the root and `defaultPrim`/`kind` metadata;
2. validate references, standard schemas, source/provenance, and no dangling paths;
3. return the ordinary metadata operations and explicit Save-As command for
   the existing review, lint, projection, and save workflow.

Invalid authored values must be rejected by the owning validator, not silently
clamped by a slider. Human and AI operations should produce the same dry plan.

The function returns standard metadata operations and an explicit Save-As
command. Provenance is caller-supplied and remains a package manifest or
standard `assetInfo`; it does not create a LunCo schema or silently save.

### P1 — Generic port graph and wiring plan — delivered

`model_authoring::port_graph` and `wiring_plan` now discover standard USD
`inputs:`/`outputs:`/`connectors:` endpoints, classify USD/Modelica/Rhai source
domains, and validate direction and type before returning typed
`SetConnection` operations.

The production Rhai test covers one composed connection and a missing source
failure. The graph is scoped by exact document/root identity and uses no
vehicle-specific port vocabulary.

### P1 — Reliable dynamic Rhai tool loading — verified for this slice

The published Twin should be able to register a tool library, discover its
functions, and call it from another tool without copying source into a
scenario. The registration result must be atomic: compile errors, active-Twin
scope, registry generation, and callable function names are returned together.
If discovery succeeds but a namespaced call returns `Module not found`, that is
a tool-loading failure, not a model failure.

The new library is registered and called through the regular namespaced Rhai
tool system in the production scene test. Keep using that dynamic loading
path for Twin-local recipes; do not add a vehicle plugin or a Rust
implementation of a Twin recipe. A future same-session loading failure should
be tracked separately from this authoring slice.

### P1 — Disposable candidate preview and diff — partially implemented

`DocumentRegistry::fork`, isolated preview leases, and the non-mutating
`assembly_ui` explode presentation already provide the candidate lifecycle.
What is still missing is a concise before/after change view that lists affected
paths and the resulting material, collision, joint, and lint consequences.

Show changed paths and before/after values together with material, collision,
joint, and lint consequences. Do not create a parallel scene graph or a
temporary hand-written USDA writer.

### P2 — Release and motion evidence — regression, not a current feature blocker

The joint editor, joint-frame overlay, `assembly_joint_audit`, and
`physics_acceptance` cover the authored and runtime evidence needed by the
current vehicle study. Keep one bounded Rhai regression that proves
attachment, release, ownership, contact, and motion for future edits, but do
not prioritize a new release mechanism based on the historical handover.

The same regression is reusable for any mounted payload, deployable, door, arm,
or wheel assembly. If a future model exposes a real generic runtime gap, add
the smallest owner-side mechanism then; it is not a reason for a vehicle API.

### P2 — Stable camera and inspection presets — partially implemented

`Frame`, `Reset`, projected bounds, and explicit preview/session camera
ownership are implemented. The remaining feature is discoverable, persisted
view-only presets (assembly, selected part, orthographic views, active-target
follow) plus a visible active-camera state. These are useful for vehicle
visual review but do not block the first numeric landing proof.

### P2 — Structured diagnostic grouping — open

Group diagnostics by owner and severity, for example:

```text
Composition
  missing source prim
USD/Physics
  unresolved joint or collider
Modelica
  backend not ready or port mismatch
Rhai
  hook error with source/line
Presentation
  missing material/texture or invalid camera target
```

Keep full raw logs available, but make the actionable path, consequence, and
next action primary. Graphics-driver warnings must not hide a dropped control
wire.

## Twin package responsibilities

The Twin owns the vehicle facts and mission policy. The shared tooling
should make those facts easy to author and inspect, but must not absorb them.
The next agent working in the Twin should have these reusable package surfaces:

| Package surface | Owner | Purpose |
|---|---|---|
| Component manifests | Twin-local Rhai/USD | Define lander body, legs, ramps, adapter, wheels, solar, sensors, and propulsion with explicit dimensions, mass, frames, ports, references, and provenance |
| Assembly recipe | Twin-local Rhai over `assembly_builder` | Compose vehicle instances, mount/align them, select variants, and emit one dry typed plan |
| Scene recipe | Shared Rhai tool with Twin manifest input | Place the assembly and environment, set the initial state, add cameras/routes, and attach programs without vehicle-specific code in the shared core |
| Domain wiring | Twin-local Rhai over generic `port_graph`/`wiring_plan` | Connect Modelica power, thermal, propulsion, sensors, and mobility endpoints with explicit typed USD connections |
| Requirements | Twin-local Rhai | Check vehicle counts, layout, assumptions, and mission thresholds; report exact paths and never repair the stage |
| Simulation tests | Twin-local Rhai scenes/scenarios | Exercise settle, motion, deployment, release, controls, and integrated surface operations with isolated runs and real verdicts |

The current vehicle release, handoff, and motion path is treated as
working. Its bounded scenario remains useful as a regression, but it is not a
reason to add a new core mechanism. The old `NO-VERDICT` material in handovers
must be labelled historical when the Twin is next refreshed.

Vehicle-specific dimensions, counts, routes, and thresholds stay in the Twin
contract. The shared repository only needs the generic authoring/readiness
facades listed above.

## Existing capabilities to reuse

The following are already present and should remain the single owners:

| Need | Existing owner/capability |
|---|---|
| USD mutation | `UsdOp`, `ApplyUsdOp(s)`, journal, undo, generation checks |
| Composition | `lunco-usd-compose`, composed-stage reads, document-scoped edit targets |
| Preview | `UsdPreviewId`, isolated preview sessions, Visual/Text modes, framing, explode presentation |
| Component editing | `component_bundle_facts`, `component_bundle_plan`, `component_editor` update context |
| Topology | `QueryUsdPrim`, assembly audit, standard component reports |
| Validation | `RunLint`, `ValidateAsset`, standard component checks, namespace collision lint |
| Live invalidation | generic `ModelStateRevision` and backend-owned reactions |
| Controls | controller-owned semantic bindings, `SimulateIntentEdge`, `intent.edge` |
| Tracing | `CausalTrace`, `BindingStatus`, telemetry and typed acknowledgements |
| Runtime scripts | Rhai tool registration, script inspection, authored scenario hooks |
| Assembly policy | Rhai chooses parts/sockets/joint identities; Rust commits generic plans |

The report is therefore a UX/integration backlog, not a proposal to duplicate
these owners in a vehicle-specific tool or in Rust.

## Recommended next order

The requested generic authoring slice is complete. The next useful work is
human UX built on its records, in this order:

1. Add a path-based candidate diff over the existing forked preview. Show
   before/after values and topology, mass, frames, joints, ports, and lint
   consequences without adding a second scene graph.
2. Add diagnostic-to-selection/reveal/frame actions in the focused preview so
   readiness and lint findings are directly actionable.
3. Add one discoverable inspection mode combining render/collision geometry,
   joint frames, mounts, materials, source provenance, and the selected
   physical owner.
4. Add persisted view-only inspection presets after the common evidence
   records are stable.

These should consume `model_context`, `readiness_report`, and the existing
preview/selection owners. Vehicle-specific routes, dimensions, thresholds, and
component recipes stay in the Twin's Rhai package.

## Explicit non-goals

- no vehicle-specific Rust schema or builder;
- no second scene graph or ECS-only assembly registry;
- no custom CAD/BREP/STEP kernel for this prototype workflow;
- no unregistered parametric USD schema when standard USD composition,
  variants, metadata, and authored operations are sufficient;
- no raw USDA write path alongside the journalled USD operation path;
- no implicit save, implicit proposal commit, or runtime-layer persistence;
- no moving Twin policy or tutorial sequencing from Rhai into Rust.

## Definition of done for the generic authoring slice

From one fresh Twin, a human or AI can call `model_context`, understand the
complete assembly, call `readiness_report`, create a generic scene recipe,
connect the authored Modelica/Rhai graph, and obtain a validated reusable
component publication plan. All returned USD changes are dry, generation-
checked, and routed through the existing journal/review/save owners; the
production Rhai scene test provides the current evidence for this slice.
Vehicle-specific requirements remain in Twin-local Rhai checks; no vehicle
builder or vehicle schema is added to Rust. Runtime verdicts still require
real authored evidence, not a screenshot, timer, command admission, or generic
asset lint alone.
