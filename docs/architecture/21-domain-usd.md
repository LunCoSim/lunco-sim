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
adapter; `lunco-usd-bevy-runtime-persistence` owns
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
`lunco-usd-bevy-core` owns the generic runtime projection mechanisms and
domain-owned live-edit registry;
`lunco-usd-bevy-scene` owns render-free ECS scene identity, lifecycle, ancestry,
projection ordering boundaries, the generic projection-reset and authored
info-change messages,
visual-split markers, authored billboard
contracts, shared geometry decoding, and composed collision/placement
envelopes; `lunco-usd-bevy-camera` owns render-free camera
projection intent, camera paths, mounts, selection, and viewport reconciliation;
it consumes the BasisCurves evaluator from `lunco-usd-geometry` rather than
owning a second curve implementation;
`lunco-usd-bevy-twin` owns the render-free document-to-`twin://` identity map,
workspace and preview leases, projection cursors, user-ownership events, and
the event-driven wake signal and document-to-mounted-stage lookup;
`lunco-usd-bevy-stage` owns canonical-stage storage and stage readers, while
`lunco-usd-bevy-core` owns the generic live-edit owner registry and runtime
projection mechanisms;
`lunco-usd-bevy-runtime-core` owns scene admission, stage loading, and the live
ECS projection systems that consume that state; `lunco-usd-bevy-runtime-persistence`
owns the independent runtime-overlay persistence boundary; `lunco-usd-bevy-runtime` owns
the complete application plugin composition;
`lunco-usd-bevy-lathe` owns the independent parametric NURBS/lathe mesh
projection; `lunco-usd-bevy-mesh` owns built-in, native-mesh, curve, and
NurbsPatch visual mesh projection plus quality invalidation;
`lunco-usd-bevy-light` owns UsdLux light and dome projection, including live
refresh from the generic authored info-change message;
`lunco-usd-bevy-animation` owns the render-free time-sample projection;
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
boundary;
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
owner with the generic USD runtime; `lunco-usd-sim-shader` owns shader intent
projection and its port backend; `lunco-usd-sim-cosim` owns participant
discovery, wiring, readiness, and Modelica/script exchange; and
`lunco-usd-sim-cosim-api` owns optional API query serialization. The application
bundle installs the implementation plugins explicitly, so vehicle changes do
not make the vehicle package depend on shader implementation or the 6.5k-line
cosim implementation.

Core USD schema assets register incrementally through
`lunco-usd-bevy-runtime-core`. `lunco-usd-authoring` applies matching linear-unit
facts as declarations arrive and validates missing entries only after all
vendored core schema sources load successfully.

Public command and document-lifecycle coverage for the document boundary lives
in `crates/lunco-usd-commands/tests/commands.rs`, so changes to those tests do not
recompile the command library's normal target. Private pending-load and
grouped-edit seams remain beside their owning implementation because they
cannot be observed through the public contract. Scene projection seams remain
beside `lunco-usd-bevy-runtime-core`; shared contract helpers are tested in
`lunco-usd-core/tests/`.

Public USD query behavior is exercised through the production query bridge by
`assets/scenes/tests/usd_query_api.usda` and
`assets/scenarios/tests/usd_query_api.rhai`, with its reference-arc layer in
`assets/scenes/fixtures/usd_query_api/site.usda`. The editor proposal lifecycle
is covered by the authored `assembly_editor_proposal` scene and scenario.

The public composed-stage reader, StageView, and prepared-reader contracts live
in `crates/lunco-usd-bevy-stage/tests/stage_reads.rs`. They use in-memory
`StageRecipe` closures, so fixture edits do not touch the asset filesystem and
do not rebuild the core library's inline test modules. Shipped-asset behavior is
asserted through the production scene/Rhai tests under `assets/scenes/tests/`
and `assets/scenarios/tests/`; the Rust target retains only generic USD
composition and reader seams.

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
    MovePrim        { edit_target, from_path, to_path }, // rename / reparent (NamespaceEditor)
    SetTranslate    { edit_target, path, value },
    SetRotate       { edit_target, path, value },
    SetAttribute    { edit_target, path, name, type_name, value },
    SetRelationship { edit_target, path, name, targets },
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

### Authored/view layers ⊕ the composed stage

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

Invariant: **every generation bump records exactly one op-log entry**. `ops_since`
returns `None` when the op ring is shorter than the generation delta, degrading safely
to a full rebuild (`rebuild_scene_from_composed`) rather than a silent projection lie.
Coarse ops (`ReplaceSource`, `MovePrim`, `RemoveTimeSample`, `SetRelationship`) rebuild;
the common interactive ops replay incrementally (`apply_incremental_op_to_stage`).

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

The **read** surface is the `UsdRead` trait (`lunco-usd-bevy-stage/src/read.rs`): `children`,
`scalar::<T>`, `attr_value`, `rel_target`, `scalar_at` (time-sampled), etc. The
same module owns the shared precision-tolerant value readers such as
`read_vec3_f64`, strict primvar/boolean decoding, and their time-sampled
variants. Consumers import those functions from `lunco_usd_bevy_stage::read`
directly; the visual adapter does not act as a generic USD facade, and
OpenUSD types such as `sdf::Path` remain direct OpenUSD dependencies. It is
implemented for both `StageView` (the live composed stage, `view.rs`) and `sdf::Data`
(the flattened layer), so one generic reader works against live and flattened alike.
The `UsdStageAsset` carries a `Send` `StageRecipe` (`recipe`) and a prepared
`UsdStageProjectionPlan`; the live stage is built on the main thread from the
recipe, and there is no stored `reader` object.

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
resolver with shared limits for layer count, dependency width, depth, and
retained bytes. A missing sublayer, reference, or payload does not discard
already available siblings. The loader publishes the available stage, leaves
the missing authored arc unresolved as required by OpenUSD, and records a
scene-scoped `RuntimeDiagnostics` warning with both logical layer identifiers.

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
5. **UsdShaderPlugin** — Projects authored `UsdShade` WGSL intent and registers the shader-parameter port backend in the shared preparation phase.
6. **UsdSimPlugin** — Detects the standard vehicle/wheel schemas and authored vehicle topology, then creates the topology-derived `lunco_core::MobilityRoot`, `WheelRaycast`, `lunco_port_core::OutputPorts`, generic joint/shaft endpoints, `DifferentialCoupling`, and sensors. **UsdSimCosimPlugin** separately discovers programs, publishes model surfaces, and derives co-simulation wires. Vehicle motion allocation and wheel heading are produced by the composed Modelica/Rhai network; Rust only realizes the resulting generic values (see [`22-domain-cosim.md`](22-domain-cosim.md)).

Authored controls and generic executable programs are resolved by the separate
`UsdAuthoredRuntimePlugin` after visual projection. It observes
`UsdSceneProjected` additions, queues non-preview owners, and runs its exclusive
resolver only while that typed pending set is non-empty; it does not poll the
full projected scene each Update.

### Compound collision ownership

`UsdAvianPlugin` reads the composed stage, including referenced descendants, when
it builds a rigid body's one Avian compound collider. A prim carrying both
`PhysicsRigidBodyAPI` and `PhysicsCollisionAPI` contributes its own shape at
identity in body space; collision-enabled descendants contribute their
root-relative transforms. The body's ECS transform owns the root prim's scale,
rotation, and placement exactly once. Nested rigid bodies remain separate
ownership boundaries, and their shapes are not folded into the ancestor.

This rule applies equally to the live composed reader and the prepared,
path-remapped plan used for runtime reference instances. Consequently, a
reference does not lose a body-root shape merely because the referenced asset
also contains child colliders, and all composed prim paths remain available to
the existing joint, Modelica, and collision-filter resolution paths.

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
anchors, `physics:limitLower/Upper` or `physics:min/maxDistance`):

| USD prim | Avian joint | Notes |
|---|---|---|
| `PhysicsRevoluteJoint` | `RevoluteJoint` | 1-DOF hinge; exposes `angle` port |
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
native construction and admission boundary is **`lunco-usd-avian-joints`**; the
programmatic wheel hinge uses its `wheel_revolute_joint` plan.

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
that actually owns the collider — a collider under a body folds into that body's
compound shape, so naming either resolves to the body — and hands the pair to
avian's one `CollisionHooks` slot (`UsdCollisionFilter`, installed by
`PhysicsPlugins::with_collision_hooks` in `lunco-luncosim`). Filtering is
symmetric: one opinion is the whole pair.

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

A mounted single ray applies the LunCoRaycastAPI. Rust performs only the
required Avian spatial query and publishes raw distance, validity, hit position,
hit normal, and sample time. A miss remains invalid; it is never converted to
ideal altitude or another fallback.

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
- **Standalone presentation:** an interactive window host may ask the
  `camera.default_presentation` Rhai policy to choose `avatar`, `generated`,
  or `none` when no `CameraTrack` or unique `LocalEmbodiment` initial presentation
  is authored. Rust passes only derived USD/ECS counts, validates the closed
  result, and realizes `generated` as one render-free `SceneCamera` plus one
  unscoped directional light under the active `UsdSceneRoot` when projected
  bounds are finite. The pair is selected after deferred projection and is
  removed on authored/operator takeover or `SceneTeardown`. A missing,
  faulting, or invalid policy result is a diagnostic and no presentation;
  headless hosts can leave the convenience policy disabled, and a scene
  without finite bounds remains camera-less instead of receiving a fake camera.

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
Core USD ships no glTF plugin, so a `payload = @terrain.glb@` resolves to an
empty layer in stock USD. Our engine detects the binary extension and composes
the arc through an empty stub, then the render projection reads the authored
payload/reference directly from the live composed prim stack and routes its
canonical URI to Bevy's glTF loader (native + web).

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

*Future Enhancement (Proper Internal Handling):* A small glTF→USD-layer adapter
in `lunco-usd-bevy-stage/compose.rs` can emit `Mesh` specs instead of stubbing. That is
an interop improvement; it must continue to use the authored USD payload as the
asset identity.

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
- **Spawning**: `SpawnEntity` lowers to `ApplyUsdOp` with `UsdOp::AddPrim { reference: Some(...) }` against its explicit document and parent path.
  A palette spawn mounts the stage's `defaultPrim` via the **empty-path sentinel**
  (`UsdPrimPath { path: "" }`) — the loader resolves and writes back the concrete
  prim path. USD stays the source of truth for the root prim; the loader resolves
  the authored `defaultPrim` rather than making a filename-based path guess.
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
  contract exists. The USD preview stores the exact egui image rectangle
  measured by the viewport and shares that rectangle with preview ray
  selection, render-target sizing, and gizmo picking; toolbar coordinates are
  not treated as scene coordinates. A primary drag captured by a gizmo is
  removed from preview camera panning. Since the gizmo library writes its final
  proxy pose in `Last` while the normal interaction transfer runs in
  `PostUpdate`, a Last-stage final-pose snapshot runs before release cleanup
  consumes the transaction. The default
  `mouse_interaction` driver is disabled (Cargo `default-features = false`, only
  `gizmo_picking_backend` kept); `drive_gizmo_drag` remains gated to focused
  handles, unclaimed egui pointer capture, and no selection modifier.
- **Undo**: Reverting a `UsdOp` in the document system automatically updates the 3D world.

| Scheme | Purpose | Resolves to |
|---|---|---|
| (none) | Layer-relative refs **inside a Twin** (co-located terrain, textures) | the layer's own source |
| `lunco://` | **Engine asset library** (rovers, parts, vessels, downloaded binaries) — location-independent ref usable from external Twins | `assets/...`, then `<cache>/...` |
| `twin://<name>/...` | **Internal, runtime-only.** The currently-open Twin's root, keyed by Twin name. Reads an external Twin scene + its co-located assets (fs on native, http on web). Never authored into a file. | the opened Twin folder |

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
> via File → Open Folder. The Twin-open flow registers the folder under
> `twin://<name>` (name from `twin.toml`) and loads `twin://<name>/<default_scene>`.
> The scene authors only **relative** paths (co-located terrain glb) and
> `lunco://` library refs — so the `.usda` is portable and identity
> (`Provenance`) is the stable `twin://<name>/<rel>`, not a machine path.

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

One shared stage stack (`lunco-usd-bevy-stage`, `local_transform_at`) decodes a prim's local
`Transform`, used by **both** the static load decoder (`read_transform_from_usd` + the
instantiate path) and the per-frame animation sampler, so a static pose and its animated
pose always agree. Precedence:

1. **`xformOpOrder`** (when authored) — honored exactly by `compose_xform_order_at`,
   including op order and `!invert!`. USD is row-vector (`M = S·R·T`, openusd's
   `Matrix4d::from_trs`): the **last** listed op is applied first to the geometry. Op
   matrices are built in glam's column form and right-multiplied, so the standard
   `["translate","rotateXYZ","scale"]` decodes to exactly `Transform{t,r,s}`.
2. **`xformOp:transform`** — a full `matrix4d` decomposed via `read_matrix_transform_at`.
3. **No piecewise fallback.** An `xformOp:*` attribute contributes only when
   `xformOpOrder` names it. A prim with no authored transform stack is the USD
   identity; a malformed authored stack is rejected. This keeps the runtime
   transform exactly aligned with `UsdGeomXformable` rather than accepting a
   second, non-USD transform dialect.

Rotation (`local_rotation_at`) covers every USD channel: the six Euler orders
`rotateXYZ`…`rotateZYX`, the quaternion `xformOp:orient` (`quatf`/`quatd`/`quath`), and
single-axis `rotateX/Y/Z`.

### Animation

Authored `timeSamples` drive entities at the current sim time (architecture doc 19 — the
unified time spine). At composition (`flatten_stage`) each attribute's composed
`timeSamples` and the stage `timeCodesPerSecond` are carried onto the flattened scene
(sublayer/reference `LayerOffset`s are baked in by PCP), so animation works on referenced
assets, not just single-layer files. A prim with any animated channel is tagged
`UsdAnimated`; the per-frame samplers then drive:

- **Transform** — the full transform decode above, evaluated at the entity's resolved time.
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
sampler's writes don't fight the physics solver. Playback is independent of the physics
clock: animated entities bind to a singleton **animation-preview** `TimeDomain`, driven by
the `ControlAnimation` command (API/MCP) and the Inspector **Animation** section
(play / pause / scrub / rate). See [`19-unified-time-and-clock.md`](19-unified-time-and-clock.md)
(T5/T7) for the clock model.

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
- `assets/scenes/tests/usd_material_edit_projection.usda` + `assets/scenarios/tests/usd_material_edit_projection.rhai` — production typed material edits, source replacement, and preview-generation lifecycle
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
