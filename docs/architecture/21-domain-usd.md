# 21 — USD Domain

> Status: Active · Audience: contributors on scene-graph, geometry, and the 3D world
>
> USD (Pixar Universal Scene Description) is the scene-graph and asset format
> LunCoSim uses for the 3D world. Bases, rovers, habitats, terrain — everything
> physical — lives as USD prims in USD stages. See
> [`../../crates/lunco-usd-document/`](../../crates/lunco-usd-document), [`../../crates/lunco-usd-data/`](../../crates/lunco-usd-data), [`../../crates/lunco-usd-authoring/`](../../crates/lunco-usd-authoring), [`../../crates/lunco-usd-core/`](../../crates/lunco-usd-core), [`../../crates/lunco-usd-commands/`](../../crates/lunco-usd-commands/) and companion crates
> `lunco-usd-geometry`, `lunco-usd-avian-core`, `lunco-usd-avian-filters`, `lunco-usd-avian-joints`, `lunco-usd-avian`, `lunco-usd-avian-lint`, `lunco-usd-bevy-stage`, `lunco-usd-bevy-core`,
> `lunco-usd-bevy-runtime-core`, `lunco-usd-bevy-authored-runtime`, `lunco-usd-bevy-runtime-persistence`, `lunco-usd-bevy-runtime`, `lunco-usd-bevy-scene`, `lunco-usd-bevy-twin`, `lunco-usd-bevy-camera`, `lunco-usd-bevy-light`, `lunco-usd-bevy-animation`, `lunco-usd-bevy` and
> `lunco-usd-bevy-lathe`, `lunco-usd-bevy-mesh`, `lunco-usd-queries`, `lunco-usd-sim`,
> `lunco-usd-sim-authoring`, `lunco-usd-sim-core`, `lunco-usd-sim-cosim`, `lunco-usd-sim-cosim-api`,
> `lunco-usd-sim-domain`, `lunco-usd-sim-domain-api`.

Package ownership follows the same boundary: `lunco-usd-document` contains
the headless authored document/layer surface, layer identity, typed operations,
and edit history; `lunco-usd-data` contains reusable authored-data contracts,
stage convention conversion, and composed-value readers; `lunco-usd-authoring`
contains path-addressed authored-layer operations, USDA conversion, reference
helpers, and the schema registry; `lunco-usd-compose` contains send-safe stage
recipes and dependency interpretation; `lunco-usd-core` contains pure operation
lowerings, assembly, edit-session, and shared USD command/event contracts;
`lunco-usd-commands` contains the UI-free document lifecycle and authoring
observers that execute those contracts; `lunco-usd-queries` owns the
UI-free public query providers and their `UsdQueriesPlugin` registration for
document inspection, edit sessions, document synchronization, and explicit
assembly-target resolution;
`lunco-usd-bevy-runtime-core` owns scene admission, Twin-backed stage loading,
live document projection, and generic projection boundaries;
`lunco-usd-bevy-authored-runtime` owns the reusable authored control/program
adapter. `lunco-usd-bevy-core::program::apply_program_resolution` records the admitted source selection and
resolved asset address on its ECS owner. Reprojection of an unchanged source
preserves executable state; source or address changes enter executor admission.
`lunco-usd-bevy-runtime-persistence` owns
the opt-in Twin-scoped runtime-overlay load/save observers and restore operation;
`lunco-usd-bevy-scene-ports` owns the Bevy scene-property port backend;
`lunco-usd-bevy-runtime`
composes that runtime with the application plugin bundle; its default
`simulation` feature adds the standard vehicle/simulation projector, while
lean hosts may omit it; its `cosim` feature adds the authored Modelica/Rhai
participant projection and implies `simulation`;
`lunco-usd-geometry`
owns the reusable render-free BasisCurves evaluator, NURBS, trim, and
curve-sweep substrate;
`lunco-usd-bevy-stage` owns prepared/composed stage data and canonical readers;
its f64 world-transform reader is the common transform owner for authored
positions, cooked collision placement and geometry envelopes. The scene
collision-envelope walk starts in canonical stage coordinates and honors
descendant reset stacks. Selecting a collider child or its single-shape parent
therefore yields the same stage-space collision bounds. The
`assembly_collision_clearance_builder` production gate checks this through
both single and batch queries, including translated and rotated ancestors;
world-space cooked hull/mesh vertices also carry the f64 rigid
`local_to_stage` pose of their scaled cooking frame. Inverting that pose
recovers section-local dimensions without treating a rotated stage AABB as
a local box or borrowing the renderer's f32 transform;
`lunco-usd-bevy-core` owns the generic runtime projection mechanisms and
domain-owned live-edit registry;
`lunco-usd-bevy-scene` owns render-free ECS scene identity, lifecycle, ancestry,
projection ordering boundaries, the generic projection-reset and coalesced
`UsdSceneChangeBatch` messages carrying the projected canonical generation,
resynced paths, info-changed owning prim paths, and info-only transform paths,
visual-split markers, authored billboard
contracts, shared geometry decoding, and composed collision/placement
envelopes; `lunco-usd-bevy-camera` owns render-free camera
projection intent, camera paths, mounts, selection, and viewport reconciliation;
it consumes the BasisCurves evaluator from `lunco-usd-geometry` rather than
owning a second curve implementation;
`lunco-usd-bevy-twin` owns the render-free document-to-`twin://` identity map,
workspace and preview leases, projection cursors, user-ownership events, and
the event-driven wake signal, document-to-mounted-stage lookup, and native
source-input publication lease with its strong stage asset handle. The stage
loader shares `DocumentStageInputs`, a weak index of document-owned native
recipes at exact source identities, with that publisher. Every admission and
reload captures the current lease; authority/revision changes reject an
in-flight preparation, and retirement unregisters it. A first publication into
an already requested asset refreshes that same handle through Bevy's reload
API. The loader owns decoding and composition; runtime core owns ECS projection;
`lunco-usd-bevy-stage` owns canonical-stage storage and stage readers, while
`lunco-usd-bevy-core` owns the generic live-edit owner registry and runtime
projection mechanisms;
`lunco-usd-bevy-runtime-core` owns scene admission, stage loading, and the live
ECS projection systems that consume that state; `lunco-usd-bevy-runtime-persistence`
owns the independent runtime-overlay persistence boundary; `lunco-usd-bevy-runtime` owns
the complete application plugin composition;
`lunco-usd-bevy-lathe` owns the independent parametric NURBS/lathe mesh
projection; `lunco-usd-bevy-mesh` owns built-in, native-mesh, curve, and
NurbsPatch visual mesh projection plus quality invalidation, including shared
analytic primitive preparation and weak native mesh identity described in
[render decoupling](render-decoupling.md#immutable-primitive-mesh-assets);
`lunco-usd-bevy-light` owns UsdLux light and dome projection, including live
refresh from info paths in the generic stage-change batch;
`lunco-usd-bevy-animation` owns the render-free time-sample projection and
claims `xformOpOrder` edits through the shared live-edit registry. After each
committed order batch it reuses native animation classification to reconcile
`UsdAnimated` and invalidate `AnimationPlan`, preserving the entity and geometry;
`lunco-usd-bevy` owns hierarchy, transform, async projection orchestration, and
material intent while consuming the camera, light, lathe, and mesh packages
directly; authored control/program projection is installed by
`lunco-usd-bevy-runtime-core`, while scene-property port registration is owned
by `lunco-usd-bevy-scene-ports` and installed by the aggregate runtime; and
`lunco-usd-avian-lint` owns composed `UsdPhysics` lint facts;
`lunco-usd-avian-core` owns the USD-independent Avian/BigSpace physics-frame
bridge, including f64 pose synchronization, rootless collider propagation,
frame transport/reset, and backend admission validation;
`lunco-usd-avian-filters` owns standard USD collision filtering, transient joint
pair suppression, and Avian's single collision/contact hook;
`lunco-usd-avian-contracts` owns the shared physics ECS markers and the generic
invalidation seam used by live runtime edits, so the runtime core does not
depend on the full USD physics projector;
`lunco-usd-avian-joints` owns native Avian joint construction, seating, solver
admission, pair filtering, and graph-safe detach; `lunco-usd-avian` owns
OpenUSD physics projection and translates authored joint facts into that generic
boundary. `lunco-usd-avian-reader` collects body-owned collision sources only from active
composed descendants, stopping at deactivated subtrees and nested rigid bodies.
Inactive proxy geometry cannot suppress an active hull or contribute a collider;
prepared and live readers obey this same contract;
`lunco-usd-actuation` owns the render-free composed USD force/torque actuator
reader used by the simulation projectors;
`lunco-usd-sim-core` owns the small shared USD-simulation protocol, the
physical-wheel display-state contract, and the ground-collider readiness
contract observed by scene runners and editor systems;
`lunco-usd-sim-authoring` owns the render-free composed readers for PhysX
vehicle wheel attachments and gear drives, plus their authored lint facts;
`lunco-usd-sim-domain` owns composed component-network and Modelica projection;
`lunco-usd-sim-domain-api` owns optional generated-source API queries;
`lunco-usd-sim` owns vehicle projection and registers its in-place wheel edit
owner with the generic USD runtime. Prim admission is capped at 32 candidates
with simulation work per Update, ordered by stable stage source, instance root,
and authored prim path. Preview prims and ordinary prims excluded by the
current-generation topology index do not consume that prefix. Their processed
markers publish in stable prefix order without USD traversal; stale, missing,
or runtime-instance topology is never used to skip work. The projector
selects that prefix without sorting the full pending population and retains the
remainder for the next pass. This bounds discovery spikes while preserving
deterministic projection order.

Wheel coordinates keep three independent facts: the authored rigid-body pose,
the mechanical axle/steering directions, and the geometry basis. The shared
`lunco-usd-bevy-scene::read_geometry_orientation` reader maps generated
Y-axial primitives through their standard USD `axis` and stage up-axis. Mesh
points already express the canonical prim frame and receive identity here.
`lunco-usd-sim-authoring` converts the authored mechanical directions with the
stage convention in f64; neither direction rotates mesh vertices. The wheel
projector prepares native collision geometry and its basis together, then
applies that basis once inside the authored body frame. Rendering narrows the
same basis explicitly for its visual child. `PhysicalWheel` retains independent
mechanical and geometry bases so client roll is `roll_about_axle * geometry_basis`.
An explicit child with standard `purpose=render` is the wheel's render root.
Its rest rotation and geometry basis affect presentation only; both wheel
realizations roll about the authored mechanical axle. Hide the collision
proxy through `VisibilityAPI` when a separate render root is present. The root
must retain the wheel-local stack and be centered on the axle; put offset
submeshes below it. Multiple render roots and overlapping visible proxy
geometry are rejected by `wheel-visual-contract` and the runtime reader.
Wheel setup waits for the scene owner's direct-child admission marker before
resolving that root, without coupling simulation to asynchronous mesh baking.
Render-root topology changes require a full scene reload. Wheel parameter
resynchronization retains the admitted entity binding and rereads its rest basis.
Initial projection and live resynchronization consume the same geometry reader;
an accepted edit republishes its validated collider even when radius and width
are unchanged. Invalid geometry or stage conventions reject projection.

Raycast suspension travel stops are transient Avian constraints owned by
`lunco-mobility`. Admission requires at least one endpoint's native
`BodyIslandNode`; static/static support retains its ray samples and presentation
without adding a solver edge. Moving carriers and moving supports retain the
same unilateral stop. The production `static_wheel_contact` gate covers a
disabled rigid-body carrier on static ground.

The existing `PhysxVehicleWheelAPI`, tire, suspension and attachment schemas own
wheel authoring. Mesh collision remains supported under standard USD rules.
The authored `wheel-mesh-rolling-contact` lint warns about potentially unstable
rolling contact on enabled mesh colliders owned by wheels; it does not block
admission. Analytic cylinders and raycast wheels avoid that warning, as do
explicitly visual-only meshes. Prefer those contact models for ordinary tires;
retained mesh contact needs measured rolling, slip and penetration validation.

`UsdSimulationProjectionSettings` keeps simulation and ownerless marker work
separately bounded (32 and 128 per Update by default); zero limits raise a
runtime fault before consuming queued work. Neither limit changes physics
substeps or tick admission.
Dynamic-body admission uses one archetype eligibility filter for its query and
schedule gate: a body needs `ShouldBeDynamic`, `RigidBody`, and `UsdPrimPath`,
without initialization-pending, initialization-invalid, or object-paused markers.
Blocked assemblies retain their markers and initialization diagnostics. Native
membership changes reopen the gate; eligible admission still validates stable
identities against admitted bodies, orders promotion by logical USD identity,
and holds solver integration until authored constraints are ready. Preview
ancestry is checked inside admission and does not bypass these validations.
The shared `PhysicalWheel` component is registered for normal typed reflection
by the vehicle plugin, so authored vessel inspection reads the same runtime
radius contract used by render/editor consumers rather than a parallel query.
`lunco-usd-sim-shader` owns shader intent
projection and its port backend; `lunco-usd-sim-cosim` owns participant
discovery, wiring, readiness, and Modelica/script exchange; and
`lunco-usd-sim-cosim-api` owns optional API query serialization. The application
bundle installs the implementation plugins explicitly, so vehicle changes do
not make the vehicle package depend on shader implementation or the 6.5k-line
cosim implementation.

Live projection replacements use the registered domain owners as a synchronous
prepare boundary. An owner can promote a local prim refresh to a full-stage reset
when its derived topology crosses the subtree, then retires that state before the
generic projector replaces stage entities. A failed preparation leaves the old
ECS projection intact, emits a structured diagnostic, and faults the mounted
simulation instead of committing a partial reset. Avian retires joint graph
edges before native joint components and collider markers; a constraint owned
by another stage rejects the reset until its topology can be rebuilt together.

When a component document changes, the dependent-stage owner snapshots its
base/runtime revisions and coalesces layer updates against the immutable recipe.
Bounded `UsdPreparation` workers compose the persistent document layers directly
to `Sdf Data`, shared across dependent stages for the same revision. A bounded
typed edit publishes its affected prim specs and fields to the existing
thread-affine `CanonicalStage` as soon as that data is ready. If an active
dependent stage has only its initial immutable plan, this first edit opens
its admitted recipe through the shared canonical-stage owner and binds the
matching prepared snapshot before applying the patch. Native construction
traverses composition so referenced layers are addressable before any reader
or logging consumer runs. One
`Stage::batch_edit` transaction patches the owning layer, and the ordinary stage
sink reconciles only the changed paths. An active stage holds its exact
simulation-progress key through that ECS handoff. Only after the live patch is
committed does a background worker serialize the changed source and compose the
immutable projection plan needed by a future mount. This plan refresh does not
hold simulation progress or replace the live scene. Coarse composition changes
and edits spanning multiple source layers retain the registered full-stage reset
boundary. The owner accepts only current document revisions, operation/revision,
and target-plan identity. Identical layer bytes skip plan replacement.
`UsdStageAsset` shares its immutable `StageRecipe` through `Arc`, so ordinary
asset and pending-job snapshots do not copy the layer closure on the app thread.
When the live stage and prepared plan were built from that same recipe, the
canonical owner records the exact plan identity at the stage generation. The
asset `Modified` notification then reuses the already-open stage instead of
opening it again. Prepared topology facts can be committed at that nonzero
generation while it remains the recorded snapshot generation; any later live
edit invalidates that equivalence, so consumers require complete change history
or perform their normal live-stage reconciliation.

Default Twin scene admission captures the exact Workspace `TwinId` and live
asset mount, and validates both before installing source or projection. The
producer registers the document's stored runtime owner before lifecycle events;
file opens, private restore, new documents, forks and retirement follow the
[document source contract](55-scene-addressing-and-roots.md#document-source-admission-and-lifetime).
It uses `AsyncWorkAdmission` to parse the exact
`UsdSourceBytes` revision and serialize the restored persistent document
snapshot off the main schedule. The file-backed registry still owns path
identity, clean reload, dirty preservation, and lifecycle events through
`PreparedFileBacked`. Parse results are checked against the current source bytes;
serialized results are committed only for their captured document generation.
The live `Stage` remains in its thread-affine owner. Runtime sidecar read/parse
and editor-preview initial serialization are still synchronous, and the native
admission queue reports its missing wasm worker transport visibly.

The composed default asset reader retains the strongest contributing layer's
canonical identifier, including unresolved payloads. `UsdRead::asset_identifier`
is the fallible I/O read; `asset` remains authored/query text. Prepared plans and
live views carry the same derived context. Programs, materials, lights, celestial
albedo and DEM consumers retain that identity through typed source admission;
DEM directories resolve inside their existing I/O worker. See the
[composed asset contract](55-scene-addressing-and-roots.md#native-payload-and-source-admission).

Native binary, texture and source references use the stage worker's immutable
asset-admission table. Live changes with new native inputs pass the shared
revision-fenced preparation gate before the sink consumes them; its
`UsdNativeAssetPreparation` hold keeps the authoritative physics clock closed
through that projection. See the [asset admission owner](55-scene-addressing-and-roots.md#native-payload-and-source-admission).

Standard schema declarations come from `openusd-schemas` 0.8.0. The project
schema source is compiled by `openusd-build`, and
`lunco-usd-authoring::schema::native_registry` registers both families on each
runtime and authoring stage. OpenUSD owns inheritance, type, variability,
custom flags, and schema defaults. `SchemaMetadata` is an immutable projection
of the native definitions for inspector hints and scalar linear-unit facts.
Its single property table groups declarations by property name and then schema.
Unqualified reads compare only the declarations sharing that name and return
metadata only when their complete facts agree; schema-qualified reads select
the exact declaration. No live scene state is cached in this immutable table.
Schema availability does not depend on runtime asset I/O or frame polling.

`ResolveUsdTarget` reads one native stage made from the current document's
base, runtime, and view layers plus its admitted dependency recipe. Its source
is `document_composition`. Native prim validity, composition arcs, and spec
sites determine existence and edit scope; a derived live projection does not
decide whether a committed document target exists. A missing prim reports an
empty `prim_stack` and `under_arc: false`; its ancestor's reference arc does not
make that nonexistent child a composed target. The authored base retains its
admitted root identity and receives native Sdf spec copies; session layer inputs
use the native binary writer. The native session sublayer stack includes the
session parent before the view and runtime layers; edit-target selection accounts
for that ordering so successive overlay edits preserve the same authored layer.
Native prim resyncs reconcile the structural hierarchy. Property resyncs retain
their native property identity and enter the attribute consumer; they never
create separate property entities. Attribute authoring only changes the native
stage. The notice consumer applies each complete batch once, with ordered,
unique prim paths supplied to registered live-edit owners.
When a complete batch removes a prim, structural reconciliation owns that
removal; earlier property and transform notices cannot recreate its subtree.
Native asset preparation uses the same property scope: a property resync reads
only that property's asset defaults and time samples; prim resyncs traverse the
affected subtree. USD attribute types gate asset value-resolution work, so scalar
and presentation fields do not resolve asset provenance.
Binary source preparation retains parsed data without requiring text export.
Source display, text snapshots, and USDA export report native writer failures
at their respective consumer boundaries.

Document compound edits enter `Document::apply_sequence` on the host's private
candidate. `UsdDocument` uses one call-local `UsdEditSession` for single and
compound edits, retaining its native composed stage through ordered native
mutations. Validation, units, inverses, and target-layer extraction read that
same current composition. Direct Sdf layer replacement retires the session's
stage before another operation can read it. The editor never escapes the call
or enters the send-safe document. The host publishes the candidate, generations,
journal records, and undo group only after the complete sequence succeeds;
transient edits use that same boundary without durable history.

Public command and document-lifecycle coverage for the document boundary lives
in `crates/lunco-usd-commands/tests/commands.rs`, so changes to those tests do not
recompile the command library's normal target. Private pending-load and
native layer-composition seams remain beside their owning implementation.
`assets/scenes/tests/usd_compound_edit.usda` and its Rhai observer exercise
ordered compound application and terminal atomic rejection through the shared
production command/query bridge. Scene projection seams remain
beside `lunco-usd-bevy-runtime-core`; shared contract helpers are tested in
`lunco-usd-core/tests/`.

Public USD query behavior is exercised through the production query bridge by
`assets/scenes/tests/usd_query_api.usda` and
`assets/scenarios/tests/usd_query_api.rhai`, with its reference-arc layer in
`assets/scenes/fixtures/usd_query_api/site.usda`. The editor proposal lifecycle
is covered by the authored `assembly_editor_proposal` scene and scenario.

Relationship discovery uses native `UsdPrim::relationships`, preserving composed
property kinds and order without resolving targets to identify properties.
The reader returns native enumeration failures and absent-prim errors; snapshot
admission and `QueryUsdPrim` propagate them instead of publishing empty data.
Prepared plans retain empty and schema-declared relationships as well as their
target lists; attribute connections remain a separate surface. The
`native_relationships` production gate checks query discovery and live target
clearing, while the focused snapshot test compares native and prepared readers.

Collection query construction and expansion share the native `StageView` path.
During one immutable snapshot, native-equivalent membership queries reuse one
successful expansion and share its owned result. The query-reuse state stays
local to that worker call; no native query or stage handle crosses the prepared
asset boundary. Native evaluator queries remain distinct, and later snapshots
build fresh queries and memberships. Live reads continue to evaluate the current
stage. Collection failures retain their owning diagnostic.

Surface-terminal extraction uses each prim's resolved render binding. During
one immutable snapshot, prims bound to the same material share the result of
native `UsdShade` universal-surface resolution, including a valid absent terminal.
The material-path map is local to that worker call; subsequent snapshots and
live reads resolve their current stage. Invalid targets and shader graphs reject
the snapshot. This is composed-fact reuse at the stage owner, without a new
material resolver or policy hook. The focused surface-terminal test covers
shared bindings, empty terminals, replacement stages and cyclic graphs.

The public composed-stage reader, StageView, and prepared-reader contracts live
in `crates/lunco-usd-bevy-stage/tests/stage_reads.rs`. They use in-memory
`StageRecipe` closures, so fixture edits do not touch the asset filesystem and
do not rebuild the core library's inline test modules. Shipped-asset behavior is
asserted through the production scene/Rhai tests under `assets/scenes/tests/`
and `assets/scenarios/tests/`; the Rust target retains only generic USD
composition and reader seams.

The `lunco-usd-bevy` structural projector commits each prim's related component
facts through native bundles. Primitive shape identity accompanies its pending
mesh request, and grid cell and spatial projection publish together. Projection
completion removes both waiting markers in one operation; cancelled or rejected
geometry removes its complete pending definition set together. These groups
remain within the same prim's ordered command stream. Stable prim/direct-child
queues, `UsdVisualProjectionSettings`, generation checks and scene replacement
fences continue to own admission; physics consumes `UsdSceneProjected` at the
existing boundary.

## Scope

A USD **stage** is the 3D scene. This doc is the canonical reference for how a
scene is **owned, loaded, rendered, and edited**. The short version:

> **The Twin owns the scene. The live 3D world (the `Grid` / `BigSpace` root)
> is the *rendered result of the active Twin's current state* — its active USD
> stage *document* plus its active Run state. You don't load files into the
> world; the world is a projection of the Twin.**
>
> A **loose** `.usda` is not an exception: opening one resolves its owning
> folder (the nearest `twin.toml`, otherwise the file's parent), scans that
> folder, and selects the file as the Twin's default scene. The resulting
> folder Twin enters the same doc-first `twin://…` mount used by startup and
> tutorial loads. The folder may later be promoted to a manifest-backed Twin
> with `SaveAsTwin`.

This aligns with the canonical layer model in
[`14-simulation-layers.md`](14-simulation-layers.md) (*"Twin is the control
surface… owns documents + scenarios + runs"*) and the Document System in
[`10-document-system.md`](10-document-system.md).

## Relationship to the Document System

A USD stage is a `UsdDocument` in the Document System model. Editing in any
view produces a **typed `UsdOp`** that applies to the document; every other view
updates. The op is the single description of the delta — never a diff re-derived
by reading state back (the *author-once coherence* invariant, below).

Current `UsdOp` set (`lunco-usd-document/src/document.rs`), each carrying an
`edit_target: LayerId` naming which layer receives the opinion:

```rust
enum UsdOp {
    ReplaceSource   { edit_target, source },           // whole-layer text replace
    AddPrim         { edit_target, parent_path, name, type_name, reference, reference_prim_path },
    RemovePrim      { edit_target, path },
    RestorePrim     { edit_target, path, snapshot_usda, sibling_order },
    MovePrim        { edit_target, from_path, to_path }, // rename / reparent (NamespaceEditor)
    MoveProperty    { edit_target, from_path, to_path }, // preserve metadata and rewrite connections
    CopyProperty    { edit_target, from_path, to_path }, // copy a local property spec, including metadata and samples
    SetTranslate    { edit_target, path, value },
    SetRotate       { edit_target, path, value },
    SetAttribute    { edit_target, path, name, type_name, value },
    RemoveAttribute { edit_target, path, name },
    RestoreProperty{ edit_target, path, name, snapshot_usda, sibling_order },
    SetRelationship { edit_target, path, name, targets },
    RemoveRelationship { edit_target, path, name },
    SetConnection   { edit_target, path, name, type_name, sources }, // dataflow edges (W1)
    SetTimeSample   { edit_target, path, name, time, value },
    RemoveTimeSample{ edit_target, path, name, time },
}
```

`AddReference`/`AddPayload` are folded into `AddPrim { reference }` plus the
optional `reference_prim_path` and `author_reference`. An omitted target uses
the referenced layer's `defaultPrim`; an explicit absolute target preserves a
named source prim when the asset's composition or variants depend on it.
Programmatic and UI edits go through the
**`ApplyUsdOp { doc_id, parent_gen, op }`** or **`ApplyUsdOps { doc_id, parent_gen, label, ops }`** command
(`lunco-usd-core/src/commands.rs`). The observers that apply those contracts to
the document registry remain in `lunco-usd-commands/src/lib.rs`; the commands return
a generation-ack and direct source mutation is out.
Multi-op intents use one change set. `AttachProgram` is the typed source-backed
program authoring intent and lowers its complete source/port/wire contract to
that same USD operation path. `UsdOp` implements
both `DocumentOp` and `lunco_twin_journal::OpPayload` — so **authoring an edit *is*
journaling it *is* syncing it** (see the [networking sync architecture](../../crates/lunco-networking/SYNC_ARCHITECTURE.md)).
Typed connection preflight resolves referenced providers and applied API schemas
through the edited document's own authoring recipe. Runtime-only endpoints are
checked against the matching live port registry, so another open Twin cannot
change whether a document's authored wire is valid.

The command owner reuses the live stage's dependency recipe by canonical-stage
identity and resolver-closure revision. An ordinary edit does not deep-copy
every referenced layer's bytes; a newly injected dependency or rebuilt stage
invalidates that recipe. The live `Stage` remains thread-affine, and edits,
journaling, and ECS projection still commit through their ordered owner path.
Domain-network fact extraction from that live stage is bounded to one network
per update; Rhai policy, graph synthesis, Modelica parsing, and validation run
through `AsyncWorkAdmission`. Results commit in request order after Twin, stage,
and canonical-generation checks. Initial prepared-plan synthesis runs entirely on
a worker. This dependency-recipe cache removes redundant snapshots while the
domain pipeline keeps non-critical computation off the UI and physics cycles.
The USD policy projector caches authored policy facts per stage asset, prepared
projection-plan identity, and canonical generation. A cold cache starts from
the worker-prepared plan and promotes those facts to the current live generation
only when every intervening `UsdSceneChangeBatch` is present and proves that no
policy prim changed. For later live stage changes, contiguous generations let
the projector promote unaffected cached facts when resynced subtrees and
info-changed prims do not contain a policy. Relevant changes trigger a full
extraction for that stage; missing batches, plan replacement, or generation
gaps also use that conservative path. Unrelated stage edits skip policy asset
resolution and registry installation. Live canonical reads remain on their
owning thread. The presentation environment-settings owner also retains its
authored EV and bloom facts and environment prim paths with the exact stage
plan. Transform-only batches and info edits outside those prim paths advance
the cached generation without rereading the composed stage; relevant info edits,
resyncs, generation gaps, and plan replacement require a fresh composed read.
When the canonical owner confirms that the exact prepared plan
is still the current live-stage snapshot, read-only projections reuse its
composed schema indexes instead of traversing the live stage. The async-prepared
projection plan indexes prims by composed schema type and applied API schema.
Each native time-sample table is collected once per property; captured values
and sampled asset-source identities consume that same table, preserving native
sample order, f64 times and the explicit zero-time sample used by admission.
Native graph errors reject snapshot extraction. PBR `SetObjectProperty` commands
on a failed projection return its existing diagnostic before reading or adopting
an appearance intent, preserving that intent state on rejection.

Startup policy extraction visits only `LunCoPolicy` prims, environment settings
visit only `LunCoEnvironment` prims, and simulation topology extraction visits
only physics joints, wheel attachments, and vehicle roots. Initial simulation
topology, per-prim candidate sets, and authored vehicle output ports are
prepared from the immutable plan on `AsyncComputeTaskPool`; one indexed query
finds joint, attachment, vehicle-root, simulation-schema, and `lunco:` property
candidates and carries their composed type, API-schema, and property-prefix
facts into topology classification. The per-prim projector skips entities
without a simulation API or authored `lunco:` property, while still marking
them complete for readiness accounting. `UsdSceneChangeBatch` keeps topology
current: resynced paths that match an indexed source or currently carry a joint,
attachment, or vehicle-root schema, and info changes on indexed sources,
invalidate the cache. Unrelated resyncs and info changes only advance the
cached generation. If an exact worker-prepared plan is not available for a live
generation, the simulation owner snapshots the canonical stage recipe on its
owning thread and prepares a replacement projection plan and topology through
shared bounded `AsyncWorkAdmission`. At most four stage preparations are
pending. Before the first topology index is committed, the mounted primary root
holds its `UsdSimulationTopology` progress key so the scene cannot start
simulation against missing facts. Later generations retain the last committed
topology while a replacement is prepared, allowing existing simulation to keep
advancing; new or reprojected simulation prim admission stays queued until the
replacement commits. The owner commits only when both the asset-plan identity
and canonical generation still match; stale results are discarded. A
preparation failure records a scene runtime fault and safety hold instead of
admitting physics against incomplete facts. Hosts without worker transport
report that failure at the same owner boundary. No whole-stage topology
traversal runs synchronously in the simulation `Update` path.

A newly authored `SetAttribute` uses `RemoveAttribute` as its inverse, so adding
attributes to a growing runtime layer does not serialize the entire layer into
every undo record. Existing values use a typed restore when representable; the
full-source inverse remains for operations whose effects are genuinely
layer-wide. Removing a prim or attribute captures only its authored Sdf subtree
as a USDA fragment plus the owning parent's sibling order. Undo/redo restores
those specs in one atomic layer edit and sends the ordinary stage change notice;
it does not serialize the complete layer or rebuild the scene. Sibling order is
merged against the current parent list so unrelated later children are retained.
Undo of a transform value override preserves an inherited `xformOpOrder`
entry; it removes an order token only when the forward edit introduced it.
Live undo/redo copies the authoritative document's transform property and
order together through the generic layer patch; the native projection does
not make a separate transform-history decision.

Derived presentation has a separate typed boundary: `ApplyUsdTransientOps`
updates the runtime view from already-authored facts (for example, the route
ribbon) without becoming a user `UsdOp`. It still checks the document
generation and advances the live projection when its values change, but it
does not enter the document undo/redo stacks, save output, or Twin journal.
This keeps derived geometry reusable across Twin scenes without turning a
rendering cache into authored USD history.

Views observing a `UsdDocument`:

- **3D viewport / Grid** — renders the stage via Bevy + avian3d (the *live*
  world; see "Active stage" below)
- **Scene tree panel** — the prim hierarchy
- **USDA text editor** — text view of the stage
- **Property inspector** — attributes of the selected prim

The Editor text panel borrows the session’s generation-matched authored or
composed snapshot through `PanelCtx::resource`. Painting is not a preview-state
transition: it must preserve `UsdViewportState` change detection and must not
wake render-layer reconciliation. Text-layer and mode changes use typed intents.

### Authored/view layers ⊕ the composed stage

`UsdRead::purpose` returns the native `openusd_schemas::geom::Purpose` result.
Live reads use `ImageableExt::compute_purpose`; worker projection plans retain
that result for the same admitted revision. The nearest authored Imageable
ancestor supplies purpose, and an explicit `default` overrides inherited guide,
proxy, or render. Collision cooking, aggregate bounds, topology inspection,
and lint facts consume this typed result. Missing prims and native read failures
remain errors: physics faults and holds failed projection, while lint and
inspection expose diagnostics.

`UsdRead::effective_visibility` returns native `PurposeVisibility` for the
prim's resolved purpose through `ImageableExt::effective_visibility`.
Overall ancestor invisibility prunes the subtree; standard `VisibilityAPI`
opinions can show or suppress guide, proxy, and render geometry. Prepared
plans retain the checked result. A failed live read terminates visual
projection with `UsdSceneProjectionFailed` and hides the affected parent;
that reflected marker is available to generic inspection. Topology queries
report unknown `visual` state alongside the native diagnostic on read failure.

A running scene has three document layers and one composed stage. Each layer
has a distinct lifetime and write path:

- **`UsdDocument`** (`lunco-usd-document/src/document.rs`) — the authored `sdf::Data` layers:
  `base` (`@root@`, saved to the source file), `runtime` (`@runtime@`, durable
  document edits kept out of that source file), and `view` (`@view@`, disposable
  derived presentation). Twin policy may persist the runtime layer in
  `.lunco/runtime/<scene-path>`; the view layer is never saved, journaled, or
  included in that sidecar. Typed journal entries preserve user-authored root
  and runtime operations. All three layers remain send-safe and serializable.
- **`CanonicalStage`** (`lunco-usd-bevy-stage/src/canonical.rs`) — the live, *composed* openusd
  `Stage` with references / sublayers / variants resolved in
  `base ⊕ runtime ⊕ view` strength order. `Rc`-backed, therefore `!Send`:
  a main-thread `NonSend` resource (`CanonicalStages`). It is the projection engine —
  authoring onto it fires openusd's change sink, which reconciles the ECS.
  `QueryUsdPrim` reads this stage whenever it is mounted, including generation
  zero, so composed queries see current activity and transient `@view@` edits;
  it uses the worker-prepared plan only before the canonical stage is mounted.
  A document-scoped query may read ahead of the durable ECS sync cursor only
  when that exact document generation is already applied to the same mounted
  stage. Pending document operations remain a visible projection-not-current
  error. ECS consumers continue to use the durable projection cursor.
  Projection retains its separate generation-zero prepared-plan contract.
  Explicit-document `QueryUsdPrim` / `QueryUsdPrims` placement reads the
  canonical f64 USD matrix directly. Its `world_position` in `canonical_stage`
  coordinates must not pass through the rendering `Transform`; live physics
  pose queries retain their separate frame owner. The production `usd_query_api`
  gate composes a large parent translation with fractional child coordinates
  and checks exact single/batch readback, alongside schema-rejection and
  document-history boundaries. `quatd` orientation and joint-rotation values
  use the native `DQuat` basis conversions in both directions; authoring and
  document queries preserve their double components.

The `Send`/`!Send` boundary falls on this same seam by nature, so the two stay even if
openusd ever makes `Stage` `Send`. Save / journal / net-sync touch the cheap serializable
layers; composition (the expensive resolver work) is isolated to the one stage owner.

The runtime-layer sidecar is saved off-thread from coalesced document snapshots.
Incremental authored operations still reach the live stage immediately; adding a
route point does not serialize and reload the whole scene. Derived route ribbons
and visited marker colors use transient view operations and disappear when the
document closes.

### Op-driven projection (author-once coherence)

Edits reach the live world by **replaying the typed op onto the `CanonicalStage`**, not
by re-flattening the scene per edit:

```
UsdOp ─apply→ UsdDocument (base⊕runtime, op_log, generation++)
        │
        ├─ journal records op + inverse (undo / sync)
        └─ sync_twin_overlays replays op → CanonicalStage.author_*  (lunco-usd-bevy-runtime-core/twin_projection.rs)
                    │  fires openusd change sink
                    └─ project_stage_changes drains sink → reconcile ECS  (live_consume.rs)
                         · InfoOnly xformOp:translate → cheap pose update
                         · Resync → spawn added / despawn removed subtree
```

Authored operations and projection generations are separate contracts. A successful
authored operation records exactly one typed op. A full reload advances the
generation and records `UsdChange::FullReload`, but does not invent an authored
operation. `ops_since` returns `None` when the cursor crosses a full reload or the
op window expires; document sync then sends a layer snapshot, and the stage
projector rebuilds from composed source. Coarse authored ops (`ReplaceSource`,
`MovePrim`, `RemoveTimeSample`, `SetRelationship`) rebuild; common interactive ops,
including local subtree remove/restore, replay incrementally
(`apply_incremental_op_to_stage`). A reload whose parsed layer
source string is identical is an idempotent no-op and does not advance the
projection generation.

On a Twin's first projection, the loaded stage recipe already contains the
current base and runtime layers. The document projector replays the disposable
view layer from its own bounded operation suffix since the current source
baseline, so runtime restores and persistent edits cannot evict presentation
ops. If that view suffix itself expired, projection uses the complete composed
document rebuild path.

An authored standard `inputs:*` edit on a live model instance also advances the
backend-neutral `lunco_core::ModelStateRevision`. This is only an invalidation
signal: USD does not know whether the attached tool is Modelica, Rhai, physics,
or another backend, and it never issues a backend-specific rebuild request.
Each owner observes the revision and chooses its own lifecycle while the
instance override remains separate from the referenced source asset.

For modeling and scene tests, `RestartScene` is the supported full-reload boundary.
It clears the old USD-derived entities and worker state, reloads the stage, then
lets USD prim projection recreate cosim Modelica models and rewire connections
from the composed stage. Object/reference-level reload is intentionally still a
TODO: do not approximate it by respawning only a visual subtree, because that
would leave physics, connections, or Modelica worker state stale.
Before replacement asset I/O, the document owner republishes the current
persistent source for the scene and its tracked reference closure. This includes
referenced Editor buffers that advanced since their preview mounted. Dirty
buffers stay dirty, and disposable `@view@` opinions are excluded. Restart also
clears the view layer of the mounted source closure before remount. Derived route
positions and other presentation from the outgoing scene cannot enter the new
scene through canonical document projection. Clearing view data advances its
projection generation without changing authored dirty state or undo history.
Publishing
only the root would let a warm reload consume obsolete component parameters.


The **read** surface is the `UsdRead` trait (`lunco-usd-bevy-stage/src/read.rs`): `children`,
`scalar::<T>`, `attr_value`, `rel_target`, `scalar_at` (time-sampled), etc. The
same module owns the shared precision-tolerant value readers such as
`read_vec3_f64`, strict primvar/boolean decoding, and their time-sampled
variants. Consumers import those functions from `lunco_usd_bevy_stage::read`
directly; the visual adapter does not act as a generic USD facade, and
OpenUSD types such as `sdf::Path` remain direct OpenUSD dependencies. It is
implemented for both `StageView` (the live composed stage, `view.rs`) and `sdf::Data`
(the flattened layer), so one generic reader works against live and flattened alike.
The `UsdStageAsset` carries an `Arc`-shared `Send` `StageRecipe` (`recipe`), a prepared
`UsdStageProjectionPlan`, and handles for the layer-read receipts that keep
transitive source changes connected to Bevy's hot-reload graph; the live stage
is built on the main thread from the recipe, and there is no stored `reader`
object. The immutable snapshot shares one native `UsdShade::BindingCache` across
prim and purpose resolution, then drops it with its worker-local stage. Live
reads create a fresh cache; no binding cache survives an authored edit. Resolved
bindings that target a missing or non-material prim reject admission visibly.
Concrete authored `Sdf Data` reads borrow `SpecData::get` directly; fallible
abstract data reads use native `AbstractData::field`/`try_field`.

## Scene ownership — Twin → active stage → Grid

### The chain

```
Twin (workspace folder, owns documents)         spec 14
  └─ active USD stage = a UsdDocument            spec 10 / 21
        └─ composed (resolver-backed stage)      lunco-usd-bevy-stage/compose.rs
              └─ UsdStageAsset (prepared plan)    lunco-usd-bevy-stage/asset.rs
                    └─ UsdPrimPath root under Grid  → lunco-usd-bevy-scene contract
                                                       → sync_usd_visuals spawns entities
                          └─ the live 3D world      (avian + cosim translators key off prims)
```

The Grid is **downstream** of the Twin's stage document. Opening a different
Twin, or switching its active stage, re-points the Grid at a different stage
document. The built-in demo scene is just the **implicit Twin** opened at
startup (spec 14: *"one implicit Twin materialised on workspace open"*).

### Folder Twins vs loose files vs new — one pipeline, three doors

| Open entry point | Result |
|---|---|
| **Open Twin…** (folder) | real Twin (`root_path`, `twin.toml`, scenarios, runs) → designated stage active → Grid |
| **Open Scene…** (loose USD file) | owning folder is resolved and opened as a folder or manifest-backed Twin → document-first `twin://…` scene becomes active → Grid |
| **New scene** | ephemeral Twin → untitled stage document active → Grid |

For a folder without `twin.toml`, the active stage is the selected file's
`UsdDocument` and the folder remains browseable without manifest-backed
scenarios or runs. The file is still opened through `DocumentOrigin::File` and
saveable with `SaveDocument`. **`SaveAsTwin`** adds the manifest-backed Twin
metadata when the user wants it.

`lunco-usd-commands` completes a file save by publishing the same exported base
bytes to the document's existing `TwinRoots` asset identity before acknowledging
success and marking the document saved. This keeps later reference composition
consistent with disk. Save-As to another destination does not replace the
original asset's source. Runtime/view layers and their document projection lease
remain separate; ordinary incremental edits do not serialize the full base.
The production `saved_component_reference.rhai` gate observes a saved component
and a newly opened referencing assembly. Its API driver creates and restores a
journaled source fixture and verifies both child inclusion and f64 translation.

Structural base-layer edits, including documentation updates and `RemovePrim`,
operate on canonical SDF data. Source text is serialized by the native writer;
original USDA formatting and comments remain available only before an edit.

## Which stage opens — scene resolution

A Twin may contain **many** `.usda` files. Exactly one is the **active stage**
that projects into the Grid; the rest are an **asset library** — referenceable
into the active stage, never auto-loaded. This section is the canonical rule
for *which* stage opens.

### Why a declared entry point

Core USD has **no project-level entry point**. The organizing unit is a single
**root layer** (`.usd` / `.usda` / `.usdc`): you open *one* file and
**composition** (sublayers, references, payloads, variants) pulls in everything
else, producing the **stage**. The only entry-point mechanisms USD itself
provides are **within a file** (`defaultPrim` layer metadata) or **by naming
convention** — neither resolves "which file in a folder is the scene."

`twin.toml` fills that gap by **declaring** the entry point. The Twin layer
earns its keep precisely by naming the starting scene — we never *infer* it
from a folder of files.

### Resolution rule

The Twin's starting scene is never inferred from a folder of files: only an
authored `[usd] default_scene` can select the active scene. Other source
domains may have their own manifest or indexed-file selection rules.

| Open entry point | Browser | Active stage on open |
|---|---|---|
| **Open Folder** (no manifest) | lists all files (USD, Modelica, …) | **none** — clicking a `.usda`/`.usd`/`.usdc` opens its focused editor preview; scene replacement is explicit |
| **Open Twin** (`twin.toml`) | same folder browser | authored `twin.lifecycle` policy selects `[usd] default_scene` |
| **Twin** with no `default_scene` | same folder browser | the authored policy clears the viewport and explains that no scene was selected |
| **Loose USD file** (orphan) | owning folder | that file, selected during the folder scan |

Opening a Twin **is** opening its folder — same browser, same file list. The
application loading policy receives the parsed Twin manifest and indexed paths
after `twin://` is mounted, then selects the authored `default_scene` or clears
the viewport. A plain folder mounts no active scene until an explicit scene
transition; clicking a USD file opens its document through `OpenFile` and
focuses the isolated `OpenUsdPreview` lease without replacing the running scene.

The same `twin.lifecycle` plan selects indexed `tools/*.rhai` libraries,
`timelines/*.json` data, SysML/KerML sources, and Modelica roots. Each domain's
typed command checks the active Twin, indexed path, and `twin://` authority
before using its asynchronous loader. Rust owns those generic mechanisms;
Rhai owns which Twin assets to load and in what order.

Whether loaded automatically (Twin) or by an explicit scene transition, a
scene loads as a **single root** (the typed `SceneTransitionIntent` →
`LoadScene` path — clear-and-replace, one `UsdPrimPath` root under the Grid).
Loading another scene re-points that single active stage; it never stacks.

### Composition closure and partial scene loading

The root layer is the load transaction's required input: if its logical asset
cannot be read, the scene transition fails and reports the root error. Its
transitive USD composition graph is loaded through the canonical asset-source
resolver with shared limits for layer count, dependency width, depth, retained
bytes, and concurrent reads. All dependencies discovered across the current
breadth-first frontier are read in bounded parallel batches; results and
diagnostics are applied in authored dependency order. This keeps independent
branches in flight without letting I/O completion order affect the recipe.
Labeled read receipts preserve source-change reloads through Bevy's asset
dependency graph. On native, USD composition and prepared-plan extraction run
on Bevy's async-compute pool so they do not occupy an asset I/O worker. The
browser path still composes on the browser main thread until a Web Worker
transport is available. A missing sublayer, reference, or payload does not
discard already available siblings. The loader publishes the available stage, leaves
the missing authored arc unresolved as required by OpenUSD, and records a
scene-scoped `RuntimeDiagnostics` warning with both logical layer identifiers.
For an initial stage, after structural projection settles and before the scene
transition releases its simulation hold, `usd.scene_composition` receives the
root address and ordered missing `{referring_layer, dependency}` pairs. The same
policy runs for an incomplete closure of a newly mounted reference before its
prepared plan, layer bytes, or live ECS projection are committed. The shipped
required Rhai policy chooses `allow_partial`, preserving partial composition by
default. An active Twin policy may replace that decision with `reject_scene`.
For an initial stage the runtime tears down the partial stage and fails the
transition; for a dynamic primary-scene reference it faults and holds the
simulation while leaving the reference unprojected. Rejection of a preview or
additive reference remains local to that operation. A missing, faulting, or
malformed policy result fails admission visibly. Complete closures do not call
the policy. The loader remains responsible for fetching the available closure
and reporting missing arcs; the scene lifecycle and reference owners consume
the policy decision at their respective commit boundaries.

Other failures remain visible and terminal at their owner: unsafe traversal,
permission or storage errors, malformed required input, and exceeded closure
limits are not treated as missing files. The loader never rewrites a stale
authored URI to a different asset. Fixing an old `waypoint.usda` reference is
an authored Twin/library migration, not a Windows-path fallback.

After `TwinAssetMounted`, `twin.lifecycle` returns an ordered typed command plan.
The generic policy executor dispatches `OpenTwinScene`; the USD owner validates
the selected indexed path and the mount is **doc-first**: the scene's document
opens first (its base read through the
`twin://` source, web-ready). Generated runtime spawns, moves, and route points
are restored and written only when the owning Twin manifest opts in with the generic
`[settings] usd.runtime_persistence = true`; an omitted or false value makes
the `.lunco/runtime` cache inert in both directions. When enabled, the runtime
layer is restored before the initial mount and composed over the source scene;
the view layer starts empty. `LoadScene` then performs the **single** initial
projection with restored runtime edits already present (see the E1b flow in
[18-unified-journal-and-history](18-unified-journal-and-history.md)). The
Settings menu changes this same Twin setting through `SetTwinSetting`; it is
not a second global preference. The Twin's other `.usda` files are *indexed*
and shown in the browser but **not** mounted — a referenceable asset library,
composed into the active document on demand by an `ApplyUsdOp` carrying
`UsdOp::AddPrim { reference: Some(...) }`. Switching scenes re-points the
single active stage; it never stacks.

### `default_scene` is a path, the scene owns composition

`[usd] default_scene` names a path **relative to the Twin root**. The Rhai
loading policy selects whether to open it; the typed USD command validates and
loads the selected path. Keep the manifest thin: it points *at* a USD root; the USD root owns scene composition
(sublayers/references/payloads). Don't grow the manifest into a scene
description — that's USD's job. See
[`13-twin-and-workflow.md`](13-twin-and-workflow.md) § 3 for the `[usd]`
section.

## Verbs — they all reuse existing surfaces

| User intent | Operation | Surface |
|---|---|---|
| **Open a Twin** | Open a folder → designated stage becomes active → Grid renders it | existing `OpenFolder`/`OpenTwin` + folder picker |
| **Open a loose scene** | Open a `.usda` → owning-folder scan → folder Twin → doc-first `twin://…` scene becomes active → Grid | `OpenFile` document observer plus `UsdSceneRuntimePlugin` scene transition |
| **Built-in demo** | implicit Twin opened at startup | startup |
| **Add object / import** | author into the explicit document: `ApplyUsdOp { doc_id, parent_gen, op: AddPrim { reference: Some(...) } }` (primitives use `reference: None`); recompose into Grid; save with `SaveDocument` | existing `ApplyUsdOp` |
| **Attach a simulation program** | `AttachProgram { doc_id, spec }`; author a `LunCoProgramAPI` child, declared scalar ports, defaults, and USD connections as one change set | `lunco-usd-bevy-runtime-core::program_runtime` + normal USD projection |
| **Promote loose → Twin** | `SaveAsTwin` | existing |
| **Run / server** | `TwinCommand`s | existing `--api` surface (spec 14 "Headless + remote") |

---

## Technical Reference — Implementation Details

### Pipeline Phases

1. **UsdVisualPlugin** — Spawns child entities and attaches visual meshes, transforms, and appearance intent. It does not install authored runtime behavior.
2. **UsdAnimationPlugin** — Binds projected animated prims to the shared time domains and samples authored `timeSamples` into transform and material intent.
3. **UsdDiagnosticsPlugin** — Handles visual glTF placeholder hiding and failure-stub diagnostics; render-free stage failure state belongs to the `UsdScenePlugin`.
4. **UsdAvianPlugin** — Maps USD physics to Avian3D: rigid bodies (`PhysicsRigidBodyAPI`, with its `physics:rigidBodyEnabled`), mass-properties (`physics:mass`, `physics:diagonalInertia`, `physics:centerOfMass`), colliders (`physics:collisionEnabled`, all `UsdGeom` shapes), and **all joints** (see [Physics joints](#physics-joints)). It translates authored joint facts to the reusable `lunco-usd-avian-joints` boundary, which owns native construction and lifecycle. The separate `lunco-usd-avian-core` plugin owns the USD-independent Avian/BigSpace frame bridge and is installed directly by application composition.
5. **UsdShaderPlugin** — Projects authored `UsdShade` WESL intent and registers the shader-parameter port backend in the shared preparation phase.
6. **UsdSimPlugin** — Detects the standard vehicle/wheel schemas and authored vehicle topology, then creates the topology-derived `lunco_core::MobilityRoot`, `WheelRaycast`, `lunco_port_core::OutputPorts`, generic joint/shaft endpoints, `DifferentialCoupling`, and sensors. **UsdSimCosimPlugin** separately discovers programs, publishes model surfaces, and derives co-simulation wires. Vehicle motion allocation and wheel heading are produced by the composed Modelica/Rhai network; Rust only realizes the resulting generic values (see [`22-domain-cosim.md`](22-domain-cosim.md)).

Standalone collider and collision-only trigger admission reads the standard
`physics:collisionEnabled` boolean before loading materials or cooking a shape.
Omission retains the USD true default; false admits neither a native collider
nor its implicit static body. Malformed values report
`usd-avian-collider-invalid` and hold the owning operation. A rigid body's own
disabled geometry does not disable enabled child collision shapes: compound
collection remains at the body owner. Production `collision_enabled` and
`collision_enabled_invalid` scenes cover motion, admission and rejection.
Attached collider pose seeding tolerates retirement before its deferred marker
command applies; retired entities receive no marker, while surviving colliders
retain their seeded pose. The frame bridge owns this boundary independently
of source projection and scene reload.

Vehicle simulation prim discovery uses a separate `PendingEntityWork` instance
from the shared `lunco-usd-sim-core` contract. Path, visual-projection, and
processed-marker lifecycle observers queue only affected entities; one initial
discovery covers prims present before plugin installation. The update gate reads
that set instead of querying all projected prims on idle frames, and stage
topology is prepared only for stages represented in the current batch. Initial
topology and candidate indexes run on a worker; live topology refresh follows
`UsdSceneChangeBatch` and is limited to resynced joint/attachment sources and
non-transform info changes to indexed topology paths. Transform-only info
changes advance the cached generation without rebuilding joint topology;
structural resyncs and mixed edits still refresh it. Live USD invalidation
requeues the affected prim, while `SceneTeardown` retires the scene-owned work
and cache. A live refresh encodes the exact canonical recipe snapshot and opens
its composed stage on the worker. The shared `StageView` read interface extracts
only simulation topology; material, animation and visual snapshot preparation
are outside that operation. Only the owned topology crosses back to the World.
A completed prepared-plan task uses a nonzero canonical generation
without rescanning only when the canonical owner confirms that exact plan is
still the live stage's recorded snapshot. Otherwise it needs complete change
history or the conservative live refresh. A task is discarded when the live
topology index already proves the same canonical generation current; a late
worker result cannot cause a second whole-stage refresh. Tracy's
`usd_canonical_stage_asset_sync` span records plan reuse at the asset boundary;
`usd_sim_prepared_topology_cache` records whether the prepared topology task
can use its snapshot at the current generation. Pair it with
`usd_sim_joint_topology_scan` to verify that this path did not repeat extraction.

Authored controls and generic executable programs are resolved by the separate
`UsdAuthoredRuntimePlugin` after visual projection. It observes
`UsdSceneProjected` additions, queues non-preview owners, and runs its exclusive
resolver only while that typed pending set is non-empty; it does not poll the
full projected scene each Update.

### Compound collision ownership

`UsdAvianPlugin` reads the composed stage, including referenced descendants, when
it realizes each authored collision prim as a native collider child of one rigid
body. `ColliderOf` / `RigidBodyColliders` owns attachment, aggregation and linked
teardown; derived entities carry `UsdColliderSource` without duplicating
`UsdPrimPath` on the visual hierarchy. A prim carrying both
`PhysicsRigidBodyAPI` and `PhysicsCollisionAPI` contributes its own shape at
identity in body space; collision-enabled descendants contribute their
root-relative transforms. The body's ECS transform owns the root prim's scale,
rotation, and placement exactly once. Nested rigid bodies remain separate
ownership boundaries, and their shapes are not folded into the ancestor. The bridge
reconciles every attached proxy through native `ColliderOf` admission before
scale, mass and broad-phase preparation, including body-mode and readiness
transitions. `BodyMassFromColliders` reduces the native collider mass properties
in stable source order to fill automatic mass, inertia and center-of-mass fields.
Native override preparation and co-simulation retain ownership of explicit
computed properties through `NoAuto*`; an explicit mass rescales only automatic
inertia. Nested bodies and visual descendants never add mass to that
geometry reduction. Contact orientation uses the same stable physics identities.
Primitive `axis` belongs to the collider-local geometry basis, including when
the collision geometry is authored on the rigid-body prim itself. The body's
authored pose never substitutes for that basis; root and descendant primitives
use the same axis conversion before their native collider entities are attached.
The native `MassPropertyHelper` traverses the complete entity hierarchy without
a rigid-body boundary; USD mass ownership therefore reduces the native
`ColliderMassProperties` through `RigidBodyColliders` and Parry's public f64
mass-property math. Body-local transforms, reduction and computed results stay
f64. Native preparation invalidates the computed properties when geometry,
density or overrides change. That invalidation and membership changes wake the
body reduction; unchanged bodies do not repeat it each tick.
The physics owner registers native contact and constraint graph reset operations
through `init_scene_resource_with`; these run after entity-removal observers,
so full reload also releases isolated native contact nodes.

This rule applies equally to the live composed reader and the instance-scoped
view over the shared prepared plan used for runtime references. Consequently,
a reference does not lose a body-root shape merely because the referenced asset
also contains child colliders, and all prim paths in the referenced
`defaultPrim` subtree remain available to the existing joint, Modelica, and
collision-filter resolution paths.

`QueryUsdPrim.collision_geometry` reports the effective scaled Avian collider
for authored Mesh, Cube, Sphere, Cylinder, Cone, Capsule, and finite Plane
shapes. Triangle meshes retain topology; convex shapes return cooked hull
vertices; convex decomposition retains separate parts; analytic primitives
return exact dimensions and an explicit collider-local-to-stage pose. Mesh
vertices therefore describe the realized collider, not an authored source
mesh passed through unchanged. The composed collider pose stays in double
precision through the stage transform; only scale crosses Avian's `f32`
collider API boundary. `collision_bounds` reports a composed AABB and a
`fidelity` value;
`exact` means its extrema match the collision geometry, while
`conservative_geometry_envelope` identifies bounds derived from source-mesh
envelopes for convex decomposition or local bounds for curved primitives.
Consumers that need tight decomposition bounds must use cooked collision
geometry rather than assume the envelope is exact.

### Rover Definitions

#### Consolidated Base Files
| File | Control/drive policy | Default Wheel Type |
|------|----------|-------------------|
| `skid_rover.usda` | authored generic ports + Modelica/Rhai drive law | `raycast` |
| `ackermann_rover.usda` | authored generic ports + Modelica/Rhai heading law | `raycast` |

#### Wheel Type Declaration
The `lunco:wheelType` attribute on the **chassis prim** determines wheel behavior:
- `raycast` (default): `WheelRaycast`, `RayCaster`, entity splitting.
- `physical`: `RigidBody`, `Collider`, authored USD joints, and the generic
  Modelica/engine shaft boundary.

Physical wheel geometry is independent of its shaft axis. Analytic axial
primitives need the USD axis basis because their native meshes/colliders use
local +Y; an authored Mesh already contains its local orientation. Initial
projection and parameter resynchronization share that basis decision, while
joint and inertia orientation continue to use the shaft axis.

For authored profile geometry, `RevolveProfileMesh` accepts an explicit finite,
nonzero local axis direction (default +Y). Rhai supplies the small meridian and
the native geometry owner rotates points and normals together, then expands the
intent into ordinary Mesh attributes through the USD document journal. This
keeps tessellation out of script-sized point/index payloads and avoids rotating
a component's body or joint frames to orient its geometry.

#### Entity Layout (Raycast Rover)
Raycast wheels need identity rotation so `RayCaster` casts straight down. The system splits the USD wheel into:
1. **Physics entity**: identity rotation, NO mesh.
2. **Visual child entity**: correct orientation + mesh.

A raycast wheel decomposes traction in the **actual contact plane** (the ray-hit
normal), so a leaning single-track vehicle (bike/motorcycle) gets correct lateral
grip; for an upright wheel this is identical to the flat basis. The heading axis is
`lunco:wheel:headingAxis` (float3, wheel-local; default `+Y`) — a raked motorcycle
fork authors e.g. `(0, 0.91, 0.42)`. The final heading is an authored output, not a
vehicle-type rule in Rust.

### Physics Joints

All USD-authored Avian joints are translated by **`lunco-usd-avian`** from standard `UsdPhysics`
joint prims (`physics:body0/1` rels, `physics:axis` token, `physics:localPos0/1`
anchors, `physics:lowerLimit/upperLimit` or `physics:min/maxDistance`):

| USD prim | Avian joint | Notes |
|---|---|---|
| `PhysicsRevoluteJoint` | `RevoluteJoint` | 1-DOF hinge; exposes measured/commanded `angle` (rad), with authored limits carried by its port metadata and enforced by `SetPorts` |
| `PhysicsPrismaticJoint` | `PrismaticJoint` | 1-DOF slider; exposes `displacement` port |
| `PhysicsFixedJoint` | `FixedJoint` | rigid weld |
| `PhysicsSphericalJoint` | `SphericalJoint` | ball; `physics:coneAngle0/1Limit` → swing, limits → twist |
| `PhysicsDistanceJoint` | `DistanceJoint` | tether within `[minDistance, maxDistance]` |
| `PhysicsD6Joint` / `PhysicsJoint` | reduced | per-DOF `PhysicsLimitAPI` (`low>high`=locked) → the matching primitive; genuinely multi-DOF warns |

**Joint drive (`UsdPhysicsDriveAPI`):** `drive:angular:*` on a revolute or
`drive:linear:*` on a prismatic joint — `physics:targetPosition` (enables the
motor at load, so an Omniverse-authored mechanism seeks its setpoint with no
wire), `physics:targetVelocity`, `physics:maxForce` (motor saturation). A cosim
wire on the joint's `angle`/`displacement` port overrides the target per tick. The
native construction and admission boundary is **`lunco-usd-avian-joints`**.
Physical wheel hinges use the same USD joint reader, authored prim identity,
frames, limits and admission as other joints. The wheel projector binds its
Modelica torque/speed actuator to that authored hinge.

The native 3D prismatic solver couples the two blocked translations and three
blocked rotations. Its moving-axis Jacobian includes the anchor separation;
an active unilateral row enforces the lower or upper travel stop with the
joint's independent limit compliance. Fixed joints share the same native row
solver. Application code must not add a second alignment or hard-stop pass.

### Collision filtering — which pairs never touch

Two mechanisms, and only one of them is automatic.

**Jointed pairs** are filtered by avian: every joint this loader builds carries
`JointCollisionDisabled` (via `joint_bundle`), so a body never collides with the
body it is jointed to. That covers parent and child, and stops there.

**Everything else is authored**, through `UsdPhysicsFilteredPairsAPI`:

```usda
def Cube "Pad" ( prepend apiSchemas = [..., "PhysicsFilteredPairsAPI"] )
{
    rel physics:filteredPairs = </Lander/Hull>
}
```

Read by `lunco-usd-avian-filters::filtered_pairs`, which resolves each end to the entity
that owns the named body or exact source collider and hands the pair to
avian's one `CollisionHooks` slot (`UsdCollisionFilter`, installed by
`PhysicsPlugins::with_collision_hooks` in `lunco-luncosim`). Filtering is
symmetric: one opinion is the whole pair. A body endpoint filters every attached
collider; a collider endpoint filters only that authored surface. Native hook
flags propagate to the attached collider proxies before collision detection.

Two properties worth knowing:

- **Armed before first contact.** Resolution runs in `PhysicsSystems::Prepare`,
  ahead of the narrow phase, because avian's broad phase skips any pair already
  in the contact graph. A filter applied later does not remove an existing
  contact.
- **Nothing is inferred.** There is no "a vehicle does not collide with itself"
  rule, because *vehicle* is not a thing the physics knows — a rover on a
  lander's deck is one or two depending on the minute, and an arm should collide
  with its own base. MuJoCo, PhysX and URDF/MoveIt each landed in the same
  place: automatic for adjacency, authored beyond it.

**Group-vs-group** filtering, for when pairs stop scaling (twenty parts is O(n²)
rels), is `UsdPhysicsCollisionGroup` — read by `lunco-usd-avian-filters::collision_groups`
and mapped onto avian `CollisionLayers`, one layer bit per group:

```usda
def PhysicsCollisionGroup "Wheels"
{
    prepend rel collection:colliders:includes = </Rover/Wheels>
    prepend rel physics:filteredGroups = </Scene/Groups/Chassis>
}
```

- **Membership is a `UsdCollectionAPI`** (`collection:colliders:includes` /
  `:excludes`) — the schema applies `CollectionAPI:colliders`, and the standard
  `expandPrims` rule applies: an include brings its subtree, a deeper exclude
  takes part of it back out. Same construct as material binding and light
  linking, not a bespoke token.
- **`physics:mergeGroup`** — group prims sharing a non-empty merge key ARE one
  group and share a bit, so two layers can each contribute members without
  knowing about each other.
- **`physics:invertFilteredGroups`** — the listed groups become the only ones the
  group collides with, read literally (a group that inverts and does not list
  itself stops colliding with its own members).
- **Ungrouped bodies keep colliding.** Groups take bits from 1 up, never bit 0
  (avian's default) and never bit 7 (`TRIGGER_COLLISION_LAYER`), so adding a
  group never silently switches off a contact between two parts outside it.
- The table is resolved once per stage (`CollisionGroupTables`, cleared on
  teardown) because membership is a stage-wide question the loader asks one prim
  at a time.

The two spellings are proven to agree: `scenes/tests/filtered_pairs.usda` and
`scenes/tests/collision_groups.usda` are the same rig, referenced, filtered the
two different ways, sharing one control and one scenario.

### Collider geometry observations and NURBS

`QueryUsdPrim { collision_geometry: true }` exposes the effective Avian shape
derived from a composed `PhysicsCollisionAPI` prim through the shared
`lunco-usd-avian-reader`. Primitive results carry their cooked dimensions and
pose; mesh results carry the cooked triangle or convex-hull vertices. This is
the geometry the adapter will project from that composed USD state. It is
distinct from authored shape attributes, visual `geometry_bounds`, aggregate
`collision_bounds`, and proof that a live ECS collider has completed projection.
Requirement policies can use this query to compare source and backend geometry
without implementing a second collider reader.

`UsdGeomNurbsPatch` is not directly a `UsdPhysicsMeshCollisionAPI` collider.
The generic `PlanNurbsCollisionProxy` query and `nurbs.rhai` authoring tool
derive an invisible, source-linked `UsdGeomMesh` using the same geometry cook
used by Avian projection. Its `max_refinement_delta_m` is a sampled change
threshold between consecutive tessellation levels, not a certified surface
error bound. Projection deterministically re-cooks and rejects stale or edited
proxy data. The adapter advertises only modes it can realize: `none`,
`convexHull`, and `convexDecomposition`; it rejects unsupported standard modes
instead of substituting a different approximation.

### Standard schema support boundaries

Kept as a list rather than as folklore, because the cost of not knowing is
authoring that looks meaningful and does nothing. Anything here is a candidate;
nothing here is a bug.

| schema | status | why it is not read |
|---|---|---|
| `PhysicsArticulationRootAPI` | authored, **deliberately** inert | avian has no reduced-coordinate articulation, so there is no honest translation. Authored on `skid_rover`, `rocker_bogie`, `physical_drivetrain` and kept for PhysX round-trip — a tool reading these files back out wants it. |
| `UsdGeomPointInstancer` | composed reader + visual projection | Required arrays, ordered `prototypes`, prototype-root transforms, and `invisibleIds` are read with the OpenUSD contract. Static direct renderable Gprim prototypes share their Bevy mesh/material handles, so Bevy automatic instancing can batch them. Animated arrays/prototypes and arbitrary prototype subtrees fail visibly until their runtime samplers and multi-mesh render batch exist. |
| `instanceable = true` + prototypes | not read | same story one level down: every spawned copy is a full prim tree. |
| `UsdCollectionAPI` (general) | read only where `PhysicsCollisionGroup` applies it | membership resolution lives in `collision_groups.rs`. A general collection reader would also serve material binding and light linking. |
| `UsdGeomSubset` | not read | per-face material / collider subsets. No use case in the fleet yet. |
| `proxyPrim` relationship | not read | `purpose` covers the case we have (a proxy SIBLING). The rel names a specific proxy for a specific render prim, which nothing authors. |
| `extentsHint` | not read | model-level bounds cache. `extent` itself IS read (it is what `lunco:occluder` sizes from). |
| `UsdSkel` | not read | no skinned geometry in the fleet. |

The rule for adding to this table: if an asset authors it and the engine ignores
it, it belongs here — or the authoring should be deleted. Silence is the failure
mode, in both directions.

### Physics observations and sensor conversion

The engine exposes Avian's native rigid-body facts directly: position, linear
velocity, quaternion, angular velocity, mass/inertia, and collider contact.
Those ports are not semantic flight sensors and are available because the
physical component exists.

A mounted single ray applies the LunCoRaycastAPI. Rust performs the required
Avian spatial query and publishes raw distance, validity, hit position, hit
normal, and sample time. It also publishes the effective ray origin and
direction in the rigid body's local frame, composed from the sensor's authored
transform hierarchy. Conversions can consume the exact geometry the query used
without repeating mount coordinates. A miss remains invalid; it is never
converted to ideal altitude or another fallback.

IMU, altimeter, attitude estimator, and touchdown logic are ordinary Modelica
programs. USD authors their connections to the raw Avian ports and environment
probe outputs. This keeps the engine generic: adding a new conversion changes a
Modelica asset and its USD topology, not a Rust sensor registry.

### Cameras

Scene cameras are **standard `def Camera` (`UsdGeomCamera`) prims** —
`lunco-usd-bevy-camera` projects each to render-free camera intent and `lunco-render-bevy` binds the
complete inactive Bevy `Camera3d` pipeline (see [`17-view-and-intent.md §6`](17-view-and-intent.md)).

| Attribute | Meaning |
|---|---|
| `float focalLength`, `float verticalAperture` | vertical FOV = `2·atan(verticalAperture / (2·focalLength))` |
| `float2 clippingRange` | near / far planes |
| `token projection` | `perspective` (default) or `orthographic` |
| `double3 lunco:cameraLookAt` (`LunCoCameraAPI`) | aim the camera at this point (parent-local); overrides authored rotation |
| `LunCoCameraAPI` / `lunco:cameraRole` | explicit `viewport` or `sensor` runtime role for a participating camera |
| `LunCoCameraAPI` / `lunco:cameraPose` | explicit `authored` or `mounted` sole pose authority |

- **Placement:** `lunco:cameraPose = "authored"` keeps the camera in its USD
  hierarchy. `lunco:cameraPose = "mounted"` explicitly creates an onboard,
  grid-direct follower with a static local offset; it stays jitter-free while
  the persistent `OriginAnchor` tracks the selected camera. A nested prim alone
  never changes pose authority. Aim either camera with `lunco:cameraLookAt`.

- **Embodiment behavior:** `LunCoAvatarAPI` only marks the local avatar role. The
  initial interactive rig is generic Rust substrate; the avatar-specific
  `lunco-avatar-input` adapter projects shared semantic intents into camera
  behavior; Rhai selects free-flight,
  orbit, follow, or another composed behavior through the camera command/API
  surface. USD does not carry a camera-mode field.
- **Switching:** cameras spawn inactive; make one the active view with
  `set_camera("Name")` (rhai / API `SetActiveCamera`, matches the prim's leaf or
  full path) or the `KeyC` hotkey. Exactly one window camera renders at a time.
- **Invalid authoring:** an authored camera with an invalid USD attribute,
  invalid `LunCoCameraAPI` value, or contradictory pose declaration is marked
  `UsdSceneProjectionFailed` and hidden. The projector does not reinterpret it
  as an omitted camera or choose a previous/heuristic camera; only genuinely
  unauthored USD schema attributes use their standard defaults.
- **Initial presentation:** a `CameraTrack` or explicit operator/director
  selection takes priority. Otherwise the active-root `camera.default_presentation`
  Rhai policy prefers the unique `LocalEmbodiment` camera, then the first
  authored camera in stable USD-path order. Independently, the windowed
  `camera.scene_avatar` Rhai policy provisions a transient local avatar in
  every active scene that lacks an authored one. The shared camera owner
  validates its controls and creates the rig under the scene Grid without a
  `UsdPrimPath` or a document operation. Saving preserves the authored scene;
  camera switches preserve the transient avatar, and authored-avatar takeover
  or `SceneTeardown` removes it. Finite geometry supplies framing; empty scenes
  use the explicit host framing radius. Missing, faulting, or invalid policy
  results remain structured diagnostics. Headless hosts leave provisioning
  disabled. One camera-contract finding set owns
  readiness and severity; `RuntimeDiagnostics` and Recent Events project that
  set. An unresolved director key is a warning while a valid operator camera
  owns the viewport, and becomes an error when director control resumes.

The local avatar remains a runtime camera embodiment rather than a USD rigid
body. Its movement controller consumes the standard `UsdPhysics` colliders
projected by `UsdAvianPlugin` through Avian `MoveAndSlide`; no avatar-specific
collider schema duplicates those authored facts. The BigSpace frame conversion
and Twin-scoped traversal policy are defined in
[`45-big-space-correct-usage.md`](45-big-space-correct-usage.md#physics-boundary).

### glTF Payloads & Placeholders

For glTFs that ship via `Assets.toml` (e.g. Perseverance), we pair a `lunco://` payload with a **`def Cube` placeholder**. 
- Third-party tools (Blender, usdview) fall back to the Cube.
- Our pipeline overlays the photoreal glTF and hides the Cube.

#### Why a `.glb` payload isn't composable, and interop

A `.glb`/`.gltf` is **not a USD layer** — USD composition only composes formats
a registered `SdfFileFormat` plugin can parse (`.usda`/`.usdc`/`.usd`/`.usdz`).
The Rust reader reports foreign binary composition arcs as unresolved.
The render adapter reads their authored payload/reference from the native prim
stack and imports the canonical URI through Bevy's glTF loader (native + web).
No synthetic layer stands in for the missing file format.

**Composition-stack anchoring.** The binary arc is read from each authored spec
in the composed prim's `prim_stack`, with the URI anchored to that spec's layer.
So a `payload = @model.glb@` authored *inside a referenced `.usda` wrapper* (the
`scene → wrapper.usda → .glb` shape the `structures/*.usda` model wrappers use)
renders from the composed prim — e.g. `/Scene/Bldg/Visual` — exactly like a glb
referenced directly in the scene. The standard USD arc is the only asset
identity, and live edits are observed on the next read. Covered by
`glb_payload_in_referenced_wrapper_anchors_on_composed_prim`.

**To make the glb compose in external tools (Blender/usdview):** install
Adobe's open-source [`USD-Fileformat-plugins`](https://github.com/adobe/USD-Fileformat-plugins)
(glTF/FBX/OBJ/STL/PLY `SdfFileFormat` plugins) and point `PXR_PLUGINPATH_NAME`
at them. The `@terrain.glb@` payload then composes natively as `Mesh` geometry —
config only, no conversion, no engine code. This is the proper interop path.


### Reference Resolution
USD references (e.g., `@/components/mobility/wheel.usda@`) are resolved relative to the **USD asset root** (`assets/`). The `UsdComposer` resolves:
- `/`-prefixed paths anchor at the asset root.
- Plain relative paths anchor at the layer's directory.
- URI schemes (`lunco://`, `twin://`) pass through to the `AssetSource`.

See [`56-asset-resolution-and-cache.md`](56-asset-resolution-and-cache.md) for which form to
author, and why a relative `../` escape fails (silently, for `LunCoProgramAPI` source assets).

> [!WARNING]
> **Never try to remove a referenced arc by re-authoring `references =` in an `over`.**
> References **compose**; they do not overwrite. Re-authoring the arc adds a **second** copy
> of the asset onto the same prim — duplicate rigid body, collider and sensors — which
> yields a non-finite raycast origin and panics `avian3d` at load.
>
> To drop a referenced child, deactivate it instead:
> ```usd
> over "GNC" ( active = false ) { }
> ```
> Deactivating a prim drops its whole subtree, which is the intended way to subtract from a
> composed asset.

### Scene Editing Tools (UX Bridge)
The `lunco-luncosim-edit-core` and UI packages provide the interactive layer:
`lunco-luncosim-edit-ui` owns spawn interaction, palette, selection, preview
interaction, and panels; `lunco-luncosim-edit-gizmo-ui` owns the focused
transform-gizmo frontend and pose transaction adapter;
`lunco-luncosim-edit-inspector-ui` owns the Inspector and authored USD panels;
and `lunco-usd-prim-tree-ui` owns the reusable prim tree.
- **Spawning**: document-backed `SpawnEntity` lowers to `ApplyUsdOp` with `UsdOp::AddPrim { reference: Some(...) }` against its explicit document and parent path. Raw-file scenes admit runtime spawns through the fixed-tick `PendingSessionInputs` owner and preserve the reserved root identity for replication; see the [command journal](command-journal.md).
  A palette spawn mounts the stage's `defaultPrim` via the **empty-path sentinel**
  (`UsdPrimPath { path: "" }`) — the loader resolves and writes back the concrete
  prim path. USD stays the source of truth for the root prim; the loader resolves
  the authored `defaultPrim` rather than making a filename-based path guess.
- Live structural reconciliation resolves `(stage, prim path)` through the
  `UsdPrimPath` lifecycle index. Insert and removal observers keep the index
  current, including duplicate preview paths; scene edits no longer scan every
  live prim once for each changed path. A referenced spawn merges its immutable
  `StageRecipe` through the canonical stage's weak recipe-identity cache. Reuse
  of the same admitted recipe avoids another closure scan, while a new recipe
  revision refreshes changed layer bytes. `ReferenceLayerInputs` retains real
  source handles and shared canonical recipe/plan snapshots for the scene
  lifetime. The bounded preparation owner checks the current loaded source
  revision, stage and live mount before reusing those facts. Native USD
  composition supplies the namespace and root opinions; runtime entities read
  the canonical scene stage.
  Native file-URI references prepare confined typed addresses and reanchored
  recipes on the existing bounded reference worker lane. The canonical recipe
  keeps the authored file identity, while the handle keeps the Twin transport
  identity. Exact stage, source, operation and mount checks precede ordered
  publication; reference/document holds remain until admitted projection.
  Raw-file runtime spawns check candidate identities through the same
  lifecycle-maintained `ApiEntityRegistry` used by public entity lookup; queued
  spawn identities use a hash set. Neither path scans all live IDs or queued
  inputs per command.
- **Selection root**: a prim that declares `lunco:spawnable = true` — authored or
  *composed from a referenced wrapper* — is tagged `SelectableRoot`, so a click on
  a deep glTF sub-mesh resolves *up* (via `find_selectable`, depth cap 32) to the
  placed model root rather than the clicked leaf. This keeps the transform gizmo on
  the object's authored placement transform instead of dropping it at the world
  origin (a glb leaf carries a ~identity parent-local transform). Editor preview
  clicks are a separate offscreen-image input surface: the preview-local camera
  ray is cast against its composed hierarchy and selects the nearest non-empty
  `UsdPrimPath` ancestor, so clicking an authored part stays within the preview
  document without crossing into the live scene.
- **Manipulation**: `transform-gizmo-bevy` is a render-space frontend on an
  unparented proxy. `capture_gizmo_start` reads the authoritative
  `SimulationPoseQuery`; the editor converts the proxy pose through the active
  BigSpace frame and actual parent storage. Drag completion emits one
  `TransformEntity` scene command, whose live leg writes `(CellCoord, Transform)`
  through the canonical parent conversion and whose persistence leg authors
  translation plus rotation as one runtime-layer USD change set. USD preview
  targets use the same transaction boundary for canonical translation,
  rotation, and unitless scale through `UsdOp::SetScale`; live simulation
  targets keep scale unavailable until an authored physics topology/solver
  contract exists. The canonical-stage transform sink updates every indexed
  projection of the same stage and prim path, including preview duplicates;
  scale updates remain restricted to `UsdPreviewOnly` ancestry. Structural
  reconciliation continues to select the mounted simulation entity.
  The USD preview stores the exact egui image rectangle
  measured by the viewport and shares that rectangle with preview ray
  selection, render-target sizing, and gizmo picking; toolbar coordinates are
  not treated as scene coordinates. A primary drag captured by a gizmo is
  removed from preview camera panning. Since the gizmo library writes its
  proxy pose in `Last` while the normal interaction transfer runs in
  `PostUpdate`, a Last-stage snapshot keeps the transaction pose current on
  every frame and through release cleanup. The default
  `mouse_interaction` driver is disabled (Cargo `default-features = false`, only
  `gizmo_picking_backend` kept); `drive_gizmo_drag` starts from the current
  occlusion-aware `HoverMap` hit on a gizmo proxy, then owns that primary
  gesture until release or cancellation. Preview pan stays suppressed for the
  complete gesture, including after the pointer leaves the image. The owner
  transaction snapshots in `Update` from that latched proxy before the gizmo
  frontend consumes the drag messages in `Last`. Selection modifiers gate
  only gesture start. `InspectUsdViewport` publishes the measured image bounds
  and scale factor used by authored Rhai mouse tests to calculate logical
  pointer coordinates.
- **Undo**: Reverting a `UsdOp` in the document system automatically updates the 3D world.

| Scheme | Purpose | Resolves to |
|---|---|---|
| (none) | Layer-relative refs **inside a Twin** (co-located terrain, textures) | the layer's own source |
| `lunco://` | **Engine asset library** (rovers, parts, vessels, downloaded binaries) — location-independent ref usable from external Twins | `assets/...`, then `<cache>/...` |
| `twin://<mount>/...` | **Internal, runtime-only.** The authority assigned to one Twin mount lifetime. Never authored into a file. | the opened Twin folder or downloaded scenario through `lunco-storage` |

The `lunco://` scheme is the engine library scheme. Collaboration protocols
must use their own explicitly defined scheme.
>
> The cache is **not addressable**: a scheme pointing at it would bake a
> machine-local location into authored content, and the file would resolve only
> inside our pipeline. Downloaded binaries live at their logical `lunco://`
> address; the `lunco://` reader resolves `assets/` first,
> then the cache. Large binaries still stay out of git — they are *resolved*
> into the library, not *addressed* in the cache. See
> [`56-asset-resolution-and-cache.md`](56-asset-resolution-and-cache.md).
>
> **External Twins:** a scene living outside the project (its own repo) is opened
> via File → Open Folder. The Twin-open flow registers the folder and loads
> `twin://<assigned-mount>/<default_scene>`.
> The scene authors only **relative** paths (co-located terrain glb) and
> `lunco://` library refs — so the `.usda` is portable and identity
> (`Provenance`) comes from the asset owner's stable logical source conversion.
> See the [mount identity contract](55-scene-addressing-and-roots.md#3-mount-addresses-and-stable-source-identity).

### Coordinate Systems

| System | Up Axis | Forward Axis | Notes |
|--------|---------|--------------|-------|
| USD    | Y       | +Z           | Standard USD convention |
| Bevy   | Y       | -Z           | Right-handed, Z-backward |
| Avian3D| Y       | -Z           | Matches Bevy |

### Transform decode

Visual reprojection replaces the complete pose when a prim authors a transform
stack, including zero translation and identity rotation/scale. The primitive
axis correction is then applied once to that decoded pose. An omitted stack
preserves the existing spawn placement; a prior rendered pose is not the base
for an authored stack. The generic projection regression is
`canonical_dispatch_test::authored_identity_pose_replaces_previous_primitive_projection`.
Coarse document edits replace the canonical stage and advance its generation,
even though the replacement's change sink is empty. Generation zero identifies
the initial asset snapshot; an edited replacement must remain on the live
composed reader path.

The shared stage reader (`lunco-usd-bevy-stage`, `local_transform_at`) evaluates
a prim's ordered local transform through native `UsdGeomXformable`. Both static
projection and the production animation sampler use this contract. Native USD
owns matrix operations, all six Euler orders, quaternion orientation, single-axis
rotations, inverse operations, and reset-stack semantics. The adapter exposes
the double matrix in the stage's canonical frame for geometry consumers, and
converts it to a Bevy `Transform` at the rendering boundary.

An `xformOp:*` attribute contributes only when `xformOpOrder` names it. A prim
with no authored transform stack has the USD identity; a malformed ordered stack
is rejected. There is no separate per-channel transform decoder.

The fallible native reset-stack fact is shared by double world-geometry reads
and visual frame projection. A reset discards operations preceding its sentinel
and the ancestor stack. The visual adapter expresses that pose in the mounted
stage frame, retaining its authored parent for live reset retirement. Removing
reset restores that parent; reintroducing it re-expresses the current native
local pose. Stage ancestry is traversed completely and cyclic ECS ancestry
fails visibly instead of choosing a partial root.

### Animation

Authored `timeSamples` drive entities at the physical presentation time
(architecture doc 19 — the unified time spine). The live native stage resolves
each attribute's composed samples and sublayer/reference offsets at the requested
time code. `timeCodesPerSecond` maps presentation seconds to that stage's time
codes. Native ordered-transform variation and consumed appearance samples determine
`UsdAnimated`; the per-frame samplers then drive:

- **Transform** — the full transform decode above, evaluated at the entity's resolved time. A prim directly beneath a BigSpace `Grid` may use `double3 xformOp:translate`; its f64 position is split into `(CellCoord, Transform)` before the cell-local translation is narrowed. This high-precision path accepts a translate-first stack followed by rotations and scale; a transform stack whose ordered translation cannot be split with its authored meaning intact is rejected visibly. A translation attribute omitted from `xformOpOrder` is inert and does not block projection.
- **Visibility** — animated `visibility` token (held).
- **Material** — animated `inputs:diffuseColor` / `inputs:opacity` (and geom
  `primvars:displayColor`) into the entity's **`PbrLook`** — the render-free appearance
  intent ([`render-decoupling.md`](render-decoupling.md)). This crate names no material
  type; `lunco-render-bevy` binds `PbrLook` to a real material.

  > An animated prim carries the **`unshared`** opt-out. `PbrLook` materials are cached
  > by content, so an animated `displayColor` re-keys the cache every frame — minting a
  > material per frame and freeing none. That is an unbounded leak that presents as a
  > slow memory climb, not a crash.

An animated rigid body is demoted to `RigidBody::Kinematic` (`lunco-usd-avian`) so the
sampler's writes don't fight the physics solver. Unbound authored animation follows
`SimulationPresentationTime`; an explicit `TimeBinding` may select the editor's
**animation-preview** domain, driven by `ControlAnimation` and the Inspector
**Animation** section. See [`19-unified-time-and-clock.md`](19-unified-time-and-clock.md)
for the clock model.

### Testing
Production runtime acceptance tests load **real USD files** through the same
pipeline as runtime. Low-level projection tests use synthetic USDA composed
in-memory so they isolate the reader/mechanism without coupling Rust tests to
the shipped asset corpus. Ownership follows the narrowest production boundary:
- `crates/lunco-usd-bevy-stage/src/{asset,authoring,canonical,read,compose,view}.rs` — prepared asset, authored-layer, composed-stage, and live-stage substrate
- `crates/lunco-usd-bevy-stage/tests/stage_reads.rs` — public composed-stage, `StageView`, and prepared-reader integration contracts
- `crates/lunco-usd-bevy-scene/src/{lib,geometry,collision}.rs` — render-free ECS scene identity, lifecycle, ancestry, shared USD geometry readers, and composed collision/placement envelopes
- `crates/lunco-usd-bevy-twin/src/lib.rs` — render-free Twin/document leases, document-to-mounted-stage lookup, ownership events, and projection wake state
- `crates/lunco-usd-bevy-camera/src/{camera,camera_mount,camera_path,camera_switch,camera_track}.rs` — render-free camera projection, pose, path, selection, and track mechanisms; curve math is in `lunco-usd-geometry/src/curve.rs`
- `crates/lunco-usd-bevy-light/src/{light,dome}.rs` — UsdLux light readers, ambient-dome semantics, and HDRI environment projection
- `crates/lunco-usd-bevy-mesh/src/lib.rs` — built-in, native-mesh, curve, and NurbsPatch mesh projection plus quality invalidation
- `crates/lunco-usd-bevy-mesh/tests/meshes.rs` — low-level USD geometry-to-Bevy mesh tests owned by the mesh package
- `crates/lunco-usd-bevy-core/src/animation.rs` — low-level time-sample topology, value decoding, rotation, and transform-reader mechanisms
- `crates/lunco-usd-bevy-animation/src/lib.rs` — production animation planning, time-domain binding, and ECS sampling systems
- `assets/scenes/tests/usd_query_api.usda` + `assets/scenarios/tests/usd_query_api.rhai` — production inspection, reference-target resolution, and document-sync contracts
- `assets/scenes/tests/editor/usd_material_edit_projection/usd_material_edit_projection.usda` + `assets/scenarios/tests/usd_material_edit_projection.rhai` — production typed material edits, source replacement, and preview-generation lifecycle
- `crates/lunco-usd-viewport-runtime/tests/live_spawn_projection.rs` — document-backed USD authoring and raw asset composition facts
- `crates/lunco-usd-avian-lint/src/lib.rs` — composed `UsdPhysics` fact production for the authored lint policy
- `crates/lunco-usd-avian-core/src/lib.rs` — Avian/BigSpace frame bridge and low-level bridge tests
- `crates/lunco-usd-avian-filters/src/{filtered_pairs,collision_groups}.rs` — standard collision filtering, joint pair suppression, and Avian contact-hook mechanisms; runtime behavior is covered by the production Rhai scene-test assets
- `crates/lunco-usd-avian/src/lib.rs` — OpenUSD collider/joint extraction mechanisms with in-memory USDA fixtures; shipped asset and runtime ownership stays in the Rhai scene-test gate
- `crates/lunco-usd-sim-domain/src/lib.rs` — low-level component-network projection and synthesis mechanisms
- `crates/lunco-usd-sim-cosim/tests/usd_connection_mechanics.rs` — generic connection derivation and transform mechanics
- `assets/scenarios/tests/*.rhai` through the production `luncosim test` gate — composed USD → Bevy → Avian → simulation outcomes, including rover structure, wheel realization, wiring, EPS, link visibility, catalog discovery, and mounted component/material contracts; shipped rover composition/migration assertions also live here through `QueryUsdPrim`
- `crates/lunco-usd-bevy-core/src/point_instancer.rs` — required/optional PointInstancer arrays, prototype ordering, transforms, ids, masking, and negative malformed-data cases
- `assets/scenes/tests/point_instancer.usda` + `assets/scenarios/tests/point_instancer.rhai` — production composed-stage acceptance for the standard PointInstancer authoring contract
- `assets/scenes/tests/parametric_surface.usda` + `assets/scenarios/tests/parametric_surface.rhai` — production composed-stage acceptance for shipped `LunCoLatheAPI` reflector and nozzle assets

---

## See also

- [`41-axes-and-units.md`](41-axes-and-units.md) — coordinate/unit conversion boundary
- [`10-document-system.md`](10-document-system.md) — the document pattern
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — Twin container + layout
- [`14-simulation-layers.md`](14-simulation-layers.md) — Twin/Scenario/Run/Model + `participant_id`
- [`19-unified-time-and-clock.md`](19-unified-time-and-clock.md) — time spine + USD animation sampler/transport
- [`00-overview.md`](00-overview.md) — three-tier architecture
- `specs/030-usd-scene-integration` — detailed spec

Primitive collision derivation uses `LunCoDerivedGeometryAPI` with the registered
`lunco:derived:source` relationship. Cube and Cylinder proxies must be siblings
with identical positive double dimensions, axes and active transform stacks.
`lunco-usd-avian-reader` rejects stale derivations at the same cook boundary used
by runtime and authoring queries. Each admitted collider publishes one
`UsdColliderSource`; `QueryPhysicsState.collider_source_paths` and body-local
bounds aggregate the body's native attached colliders, excluding nested bodies.
Contact diagnostics aggregate the same native relationship and resolve
`contact_debug(...).parts[].source1` and `source2` directly from the participating
colliders. Each row carries its applied normal impulse. Internal mesh hulls or
triangles share their authored collider source; native subshape indices remain
optional facts. Composed source geometry is not substituted silently. NURBS mesh derivation
uses the same API with its source cook fingerprint and refinement settings.

`RemoveRelationship` removes only the selected authored layer's opinion, restores
it exactly through the local `RestoreProperty` inverse, and reprojects its live consumers.
Attribute and relationship removal share one property-subtree snapshot owner;
undo retains target list operations, property order and unrelated subsequent edits.
For attribute and relationship removal, the canonical root patches the resolved
property from the authoritative document because it combines document layers.
A removed runtime override reveals the base opinion without resetting unrelated
simulated objects; native reference/sublayer composition remains intact.

Document inspection and composition use the native stage; authored-layer reads
remain separate. Invalid source cannot open a composed stage. Dependent-layer
serialization uses the native codec for its exact layer identifier, including
USDC; the authored base/runtime merge is limited to that transport boundary.
Standard root metadata follows the [assembly editor contract](48-assembly-editor.md).
