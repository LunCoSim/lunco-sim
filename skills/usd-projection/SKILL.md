---
name: usd-projection
description: >
  Extend or diagnose LunCoSim's USD-to-ECS projection: add a supported prim or
  attribute, trace an ignored field, or fix edits that fail to persist, undo,
  replicate, or render. Use for `lunco-usd*` machinery and document-owned ECS
  state. Prefer this skill for projection internals; use edit-usd-assembly for
  live headful assembly authoring, build-usd-scene for scene authoring, and
  luncosim-architecture for cross-domain ownership.
---

# USD → ECS projection

Before adding a field or reader branch, use
[`luncosim-architecture`](../luncosim-architecture/SKILL.md) and the
[standard-schema boundary](../../docs/architecture/clean-architecture-and-usd-standards.md).
Prefer the OpenUSD schema that owns the concept. A migration is a clean
cutover: update the authored source and all readers, delete the superseded spelling
and compatibility branch, regenerate schema artifacts, and add a negative
test. Never make the ECS projection a second source of truth.

**USD is the source of truth. The ECS is a projection of it.** Every entity you
see is a rendering of a prim. Nothing is authoritative because it is in the
world; it is in the world because it is in the document.

That single sentence generates every rule below.

## The pipeline, end to end

```
UsdOp  ──►  UsdDocumentRegistry::apply   (journals + inverts)
              │
              ▼
          openusd Stage (the live CanonicalStage, NonSend)
              │  StageSink fires → RawStageChange { resynced, info_only }
              ▼
          project_stage_changes            (lunco-usd-commands/src/live_consume.rs)
              ├── resynced   → structural: spawn / despawn prims
              └── info_only  → attribute-only: translate, rotate, domes …
              │
              ▼
          UsdVisualProjectionQueued  (bounded ECS binding queue)
              │
              ▼
          UsdVisualPlugin (lunco-usd-bevy/src/lib.rs)
              └── match reader.type_name(path) → visual components
                         │
                         ▼
          UsdAnimationPlugin (lunco-usd-bevy-animation)
              └── sample authored timeSamples into visual intent
```

The asset loader composes the fetched layer closure and snapshots the complete
default-time `UsdRead` surface into `UsdStageProjectionPlan` on its worker. The
`Add, UsdPrimPath` observer and `sync_usd_visuals` both feed the same
`UsdVisualProjectionQueued` marker. `process_queued_usd_visuals` drains that
queue with its configured per-frame budget, but the extractor reads only the
owned plan during initial materialisation; it does not parse USD, walk a live
stage, or resolve composed bindings on the UI thread. CPU mesh-data geometry
uses the render-free `lunco-usd-geometry` package, while its existing async
compute path and only Bevy asset insertion remain on the main thread. After the initial asset generation, explicit live edits use the
canonical `StageView` and the same extractor contract.

The scene transaction closes from asset and structural-projection outcomes.
`UsdSceneProjectionFailed` on the running mount must fail and clear that
transaction before a generation can commit; preview errors are separately
owned. Missing default roots and nonexistent or non-prim explicit targets are
visible projection failures. Verify the failure edge as well as queue drainage.

Incremental structural reconciliation resolves a live path through the
`UsdPrimPath` lifecycle index keyed by stage asset and prim path. Insert and
removal observers maintain it, including component replacement and preview
duplicates; do not add a per-frame `Added` query or scan the full scene for each
changed path. The canonical stage records a weak identity for each merged
`Arc<StageRecipe>` so subsequent instances of that loaded revision skip cloning
and comparing its full layer closure. A reloaded recipe has a new identity and
must merge its current bytes before authoring the reference. Live instance
handles own asset lifetime; the index and weak recipe cache are derived state.
Transform edits update every indexed entity sharing that stage/path, including
preview copies. Scale is applied only beneath `UsdPreviewOnly`; a live body's
scale remains unchanged. Structural reconciliation still excludes previews.
For a document-backed runtime reference spawn, the authored document keeps the
reference arc. The live canonical stage receives only the instance root while
an instance-scoped view over the shared immutable prepared snapshot projects
the source asset's `defaultPrim` subtree. The view maps paths into the instance
namespace and stores root pose and `lunco:catalogId` overrides; creating the
view is constant time with respect to the source prim count. The first other
authored edit composes the reference once, then shared promotion state switches
every entity clone to the canonical reader. Root deletion can remove an
unpromoted instance without composing its source. Keep simple `QueryUsdPrim`
reads on the same plan; explicit geometry/topology queries use the owning
document's composed stage. A failed promotion faults the mounted scene and
rejects that live edit.

Changes to a document's root scene are projected by its canonical stage. Only
different recipe roots that compose the changed layer are dependent refresh
targets; the root scene never refreshes itself as a dependent stage.

For a bounded typed edit, the dependent owner composes the current persistent
document layers directly to `Sdf Data` on a worker, then patches only the
affected prim specs and fields in the existing canonical stage with one
`Stage::batch_edit`. The normal sink projects those paths into ECS before an
active-stage progress hold is released. Afterward, a background worker
serializes the changed source and composes the immutable asset plan for a later
mount. That plan refresh does not hold simulation progress or reset the live
scene. Coarse composition edits and updates that span multiple changed source
layers use the full-stage reset owner.

Native asset inputs follow the [shared preparation gate](../../docs/architecture/55-scene-addressing-and-roots.md#native-payload-and-source-admission): preserve the reader's current prepared table, retain edits/hints across superseded jobs, and release `UsdNativeAssetPreparation` only after that exact stage revision reaches live consumption or teardown. A queued visual or installed policy must remain deferred while native preparation holds its generation.

Doc-backed Twin admission is also asset-event driven. `UsdSourceText` is loaded
through the shared typed `load_asset_path` address and registered source scheme;
scene, preview, schema, reference and transitive layer readers preserve literal
filename characters and attach Bevy labels separately. `AssetEvent` and
`AssetLoadFailedEvent` advance or fail the pending document transaction. The
default Twin path parses the exact source revision through native
`AsyncWorkAdmission`, commits it through `DocumentRegistry::open_prepared_file`,
restores the runtime overlay, and serializes a cloned persistent document on
the worker pool. Source text and document generation are checked again before
the Twin overlay is published and `LoadScene` is submitted. Runtime sidecar
read/parse and editor-preview initial serialization remain synchronous; wasm
worker transport for this path is not installed, so the owner reports that
boundary visibly. The live USD stage stays with its thread-affine owner.
Referenced stage closures follow the same event boundary before a reference is
authored onto the live stage. Each instance retains the real source handle and
one shared `UsdReferenceSnapshot` containing the canonical recipe, unscoped plan
and actual loaded source revision. The canonical stage caches only weak snapshot
identities: warm sibling reuse must validate the exact scene origin, current
source recipe/plan and live mount before skipping address I/O and composition. Native
file-URI references prepare confined typed addresses and `StageRecipe::reanchor`
composition on the shared bounded reference lane, used by incremental spawns
and coarse rebuilds. Preserve authored layer bytes and absolute identifiers;
never inject transport aliases into the resolver. Publication rechecks exact
stage lifetime/generation, source, operation, live mount and loaded source
revision; worker panics become terminal completion errors. Current reference
and document-projection holds remain through ordered projection, and replaced
requests cannot consume outgoing completions. Each instance carries only its
namespace, root identity, and root overrides; descendants
reuse that view through the same queue. The entity reader invalidates that
prepared source when the canonical stage generation changes, so authored
overrides are always read from the live composed stage. Do not add frame-count
timeouts, per-frame load polls, or direct filesystem reads to this path. After admission,
`DocumentChanged` and stage-asset lifecycle events wake the single
`sync_twin_overlays` owner; do not add a per-frame generation scan or a
viewport-specific edit/reload path.

On first projection, the `twin://` stage already contains the document's current
base and runtime layers. Replay transient view edits from the document's bounded
view-operation suffix since its current source baseline. Do not query the global
op ring from generation zero: runtime restores are non-op generation changes,
and persistent edits do not belong in the view replay. If the view suffix has
expired, rebuild from the complete composed document.

For the mounted primary scene, reference fetches stay parallel but live-stage
mutation and terminal failure publication follow reference operation order. A
later completed reference remains prepared until every earlier active
reference for that scene resolves. Keep its ready or failed outcome while
deferred; never let asset completion order choose composition or fault order.
Preview stages do not acquire this simulation-specific commit barrier. The
production `route_lifecycle` Rhai scene gate covers the fixed-tick readiness
contract with multiple live references; low-level owner tests cover the
ready-prefix ordering seam.

The Editor is document-scoped. `DocumentId` from the existing
`DocumentRegistry<UsdDocument>` identifies the file being edited; the Twin
Browser opens the explicit `OpenUsdPreview { preview, doc_id, edit_target }`
session and can later use `FocusUsdPreview` or `CloseUsdPreview`. A session
owns one projected composed stage; `OpenUsdPreviewView { preview, view }`
adds a camera/render target over that stage for another dock tab or split.
`FocusUsdPreviewView` and `CloseUsdPreviewView` address the exact view.
Native editor view-model resources are keyed by `UsdPreviewId` and derive one
entry per open session; panels paint the focused view's session entry. Visible
preview targets are bounded by `UsdPreviewRenderBudget` (2048 px per axis,
4,194,304 pixels per view, and 8,388,608 visible pixels per frame by default),
while hidden view cameras stay inactive. The shared ECS selection is a
focused-session projection restored from editor-owned session selection, not a
document identity. Panel writes use the session's explicit `DocumentId`,
`LayerId`, and projection generation.

The active Twin's asynchronous workspace restore recreates saved view tabs over
the same document-scoped preview session and restores each view's camera and
presentation settings. Reopening the same file reuses the registered document;
use the explicit **Open view** action to add another perspective. A view tab
without a restored session is dropped from the saved dock layout rather than
shown empty.

Preview projection is a presentation scope over the same composed stage. It
owns one session-local light and excludes authored scene-wide
`DistantLight`/`DomeLight` prims from that render layer; authored local
`SphereLight`/`RectLight` prims remain part of the assembly. A preview is
ready only after its root and descendants are synced and that subtree's visual
queue plus asynchronous mesh phase have settled. Consumers use the typed
`projection_ready` state from `InspectUsdViewport`; they do not infer
readiness from a document generation or camera state.

Each `UsdPreviewView` also exposes Visual and Text modes over that same
session. Visual mode renders the projected stage; Text mode displays an
asynchronous, generation-matched authored or composed USDA snapshot as
read-only text. `SetUsdPreviewViewMode` and `SetUsdPreviewTextLayer` change
only view presentation, so they preserve the document, projected stage,
selection, camera, and lifecycle identity. Text snapshots are coalesced per
session and discarded when the document generation or preview lease changes;
they do not create a second parser, document registry, or source writer.

When an agent needs to answer “what is visible?” or edit the item a user has
open, call the UI-owned `InspectUsdViewport` query (or
`assembly_edit::viewport()`) and correlate its explicit preview/view handles
with `CaptureScreenshot`. The query is presentation context, not a second
document registry: use the returned `doc_id`, `edit_target`, and projection
generation with document inspection and typed edit commands. Never infer
identity from a tab title, filesystem basename, entity order, or the live
simulation viewport.

The Files section sends a `.usda`, `.usd`, or `.usdc` click through the existing
`BrowserAction::OpenFile` and async document pipeline. The emitting Twin's resolved absolute path is
preserved, so an inactive Twin is not accidentally anchored on the active one.
Once the document is admitted, the viewport derives its document-backed
`UsdPreviewId::for_document` with `LayerId::root()`, opens the session's primary
`UsdPreviewView` as an instance-backed dock tab, and focuses that exact tab.
File reads capture `FileDocumentAdmission` before dispatch and validate the
resolved runtime owner before publication. A retired owner cannot install a
late source; repeated requests coalesce only with the same admission snapshot.
Indexed Twin source requests and leases retire by exact `TwinId`, and stored
document ownership fences deletion after a source rebind. Preview admission
rejects retired document owners; private restored snapshots explicitly belong
to Application. See the [source lifetime contract](../../docs/architecture/55-scene-addressing-and-roots.md#document-source-admission-and-lifetime).
Browser file picks carry request-owned bytes into this same asynchronous preparation pipeline.
Each successful import installs a fresh pathless Application document; the
filename is display data, even when another pick has the same name. Native
Save-As requests pin the exact source owner before the backend starts. Browser
Save-As uses a fallible download and only publishes saved state after admission.
Repeated admitted clicks reuse the same document, preview session and view tab. Explicit
`FocusUsdPreview` and `FocusUsdPreviewView` commands foreground their matching
instance tab as well; replacing or closing a session removes its view tabs with
the presentation resources.
The preview root carries `UsdPreviewOnly`, the USD projection ownership fence.
Consumers that can create simulation side effects must use the shared bounded
`is_preview_only` ancestry helper rather than names, stage handles, or missing
physics components. Authored controls and program projection use that same
ancestry fence. Cosim discovery marks preview prims examined without loading
their programs, and wire derivation excludes preview descendants so duplicate
USD paths cannot claim mounted-scene endpoints. Live operator entities are
admitted only through their `UsdSceneRoot` ownership. The USD DEM bridge marks
preview terrain prims examined without creating `DemTerrainRequest`; preview
DEMs must not create collider rings, analytic query sources, or hold mission
physics. Relief in an isolated Editor terrain preview still needs a separate
render-only terrain realization and spatial demand. Mounted-scene live-edit
reconciliation ignores preview copies when looking up an entity by stage and
USD path, so a preview duplicate cannot satisfy the mounted scene's structural
spawn or refresh. Procedural scene backgrounds are also excluded from previews
because the skybox renderer has one scene-wide owner. Unscoped `QueryUsdPrim`
reads select the mounted scene by ignoring
`UsdPreviewOnly` roots; preview roots do not make live queries ambiguous.
Never choose an editor stage by entity count, insertion order, or the current
simulation viewport, and never use an active-viewport fallback for an entity
that lacks an explicit document binding.

When an agent is creating or modifying a reusable assembly, use the dedicated
[interactive Assembly Editor runbook](../edit-usd-assembly/SKILL.md). It
requires a headful production window, explicit document/preview handles,
typed USD edits, a screenshot after each coherent change, and a user-feedback
checkpoint. This projection skill owns the implementation boundary behind that
workflow; it does not authorize direct USDA or ECS edits.

For agent or editor synchronization, call `SyncUsdDocument` with the explicit
document generation. Use its typed delta while the cursor is covered; consume
the returned base/runtime layer snapshot when a full reload or expired history
window breaks the authored-operation stream. Full reloads advance the projection
generation without adding a fabricated authored operation. Reject future cursors.
To edit a composed path, call
`ResolveUsdTarget` with the explicit document id, prim path, and `@root@` or
`@runtime@` target. A referenced or payloaded path that has a local authored
opinion in the current document is valid from that document layer while the
existing `CanonicalStage` projection catches up; a composed-only path still
requires the mounted canonical stage. Use the returned `edit_scope` and
typed-operation validation rather than treating a composed read as permission
to move or remove a referenced/variant prim. Do not inspect flat layer data as
a replacement for OpenUSD PCP resolution or use it to invent a composed-only
target.

For a transient edit target that a tool has just authored, pass
`authored_children: true` to `ResolveUsdTarget` when it needs that layer's direct
child paths before scene projection settles. This returns the selected layer's
`primChildren` paths only; it does not resolve or enumerate composed children.

Agents and editor automation should use the built-in `assembly_edit` Rhai
library. It is a thin wrapper over `OpenFile`, `InspectUsdDocument`,
`InspectUsdViewport`, `ResolveUsdTarget`, `SyncUsdDocument`,
`ApplyUsdOp`/`ApplyUsdOps` (including
`SetTimeSample` and `RemoveTimeSample` for keyframes),
`AttachComponent`, `DetachComponent`, `UndoDocument`/`RedoDocument`,
`CreateUsdProposal`, `InspectUsdEditSession`, `ReviewUsdProposal`, and
`CommitUsdProposal`; it does not create another document registry, resolver,
USDA writer, or operation log. `open`
returns the normal asynchronous command acknowledgement and callers discover
the resulting id through `ListOpenDocuments`. Read helpers require an explicit
`doc_id`; authored helpers require `doc_id`, an edit target, and a USD path. Their
optional `parent_gen` is the existing stale-write precondition, and `batch`,
`transform`, or a keyframe change land as typed journal/undo operations. A
proposal requires a generation and explicit `SourceAsset`/`Assembly`/
`InstanceOverride` scope, validates without mutating the document, and is
visible through `InspectUsdEditSession`. Mute/unmute and reject are review-only;
commit rechecks generation, layer revision, origin, file watermark, scope, and
typed validation before using the ordinary grouped journal/undo path. A
conflict requires a fresh proposal; no automatic rebase or overwrite exists.
Use the tool catalog and completion query to discover the source and
signatures.

A scene loaded from disk and a prim authored at runtime therefore produce
identical entities without one heavy deferred-command flush monopolising the
window. The queue marker is the projection ownership fence: one prepared
hierarchy creates one child under its USD parent, so the projector does not
scan the world for duplicate stage paths. The same composed path is valid in
separate scene mounts and runtime instances; hierarchy and instance identity
scope those projections.

Generated Modelica domain projection follows the same ownership and change
set rule: apply the shared `is_domain_network_root` predicate before selecting
a synthesizer, then revisit only queued root entities. Live USD changes arrive
as typed `UsdSceneChangeBatch` path sets and are routed through the canonical
stage/root/member reverse index; stage-asset changes and generation gaps only
requeue roots on that stage. Reserve the all-prim pass for initial discovery.
Do not use the USD wiring latch as a membership signal, add a second stage scan,
or use a name-based candidate list.
After validation, publish the generated source and interface, link the source
to its normal Modelica document, then let lifecycle compile admission dispatch
it. Do not send a worker compile directly from USD projection; the standard
path owns document-generation and session fencing for generated and authored
models alike.
Lifecycle projection is followed by stable identity admission and API/path
index publication before the time spine releases simulation. Newly projected
references must resolve by path on their first resumed tick through that
ordered runtime path.
The generated-source browser/API projection has its own source/document
invalidation boundary. Do not gate it on live `ModelicaModel` output or clock
changes; those are solver state and must stay in the Modelica runtime owner.
Member class discovery is driven by `ModelicaSource` asset load, failure, and
modification events. Do not add a time-based give-up deadline or poll pending
sources on stable frames; an unavailable source remains explicitly pending
until the asset owner publishes a terminal outcome.

### Scene precision boundary

The mounted `UsdSceneRoot` is a nested BigSpace `Grid` below the active site
frame. Its top-level USD prims are direct Grid children and carry their own
`CellCoord`; their visual and collision descendants remain ordinary children
under the prim root and use `LowPrecisionRoot`. Terrain and rover/lander roots
are siblings in this scene frame. Terrain is never the parent of a vehicle,
because it does not own vehicle identity, physics, or lifecycle. Runtime and
replicated catalog spawns must enter through the same cell/local placement
boundary as authored top-level prims.

## Law 1 — every edit goes through `ApplyUsdOp`

An authored edit that does not lower to a `UsdOp` is absent from **save,
journal, undo, and network replication**. Route every authored editor/runtime
mutation through the same projection boundary.

```rust
commands.trigger(ApplyUsdOp { doc_id: doc, parent_gen: None, op });      // one op
apply_ops_as_change_set(world, doc, "Edit material", ops);       // N ops, ONE undo unit
```

Prefer `apply_ops_as_change_set` whenever an intent lowers to more than one op —
a loop of `ApplyUsdOp` journals N independent entries, and undo then peels off
one and leaves the object half-edited.

This law is for authored edits. A derived presentation may use the typed
`ApplyUsdTransientOps` command after resolving its source from the composed
stage when it needs a USD prim identity, schema, or picking contract. That path
is generation-checked and projected through OpenUSD, but is explicitly outside
save, undo/redo, and the Twin journal. Do not use it for a user edit or to
refresh dense per-edit geometry. Keep high-frequency or terrain-sampled view
geometry in its transient render owner; snapshot inputs, coalesce per target,
bound worker admission, and commit only current results to the existing render
entity. For example, route edits update their normal authored projection once,
then `UpdateUsdCurveView` prepares sparse surface strokes without a second USD
generation. Terrain fragments own the drape, so LOD/elevation changes need no
route tessellation. Inspect current publication through `InspectUsdCurveView`.

`usd.document.projected` includes the reconciled `changed_prim_paths` in its
typed event data. A policy that caches composed facts should check this path set
before issuing document queries; unrelated document edits are not a request to
rescan the whole owned subtree. The event closes the live handoff only after
referenced roots changed by that edit and their prepared instance projections
have reached the ECS scene. Typed reference-add intents seed this required-root
set before the asynchronous asset admission emits a stage notice; pending
references elsewhere in the mounted stage do not delay this edit or retain its
document-projection hold. An authoritative pending reference still owns its
independent `SceneReferences` key until its instance projection is in ECS.
Partial ancestor sink notices are accumulated into that one completion event.
Each live `UsdInstanceProjection`
retains its source asset handle with its remapped immutable plan. Sibling
instances of the same asset reuse that prepared composition while any live
instance still uses it.

Writing an ECS component directly is legitimate **only** for state that is
genuinely not part of the document (a camera's current yaw, a hover highlight).
If a user would expect it to survive save-and-reload, it belongs in USD.

`AttachProgram { doc_id, spec }` is the canonical multi-op authoring intent for a
source-backed Modelica, Python, Rhai, or behaviour-tree program. It lowers the
complete `LunCoProgramAPI` child, source asset, scalar ports, defaults, and
connections through this same change-set path. A palette or script must call
that command; it must not create an ECS marker or write a parallel registry.
An empty contract is source-only and remains visibly distinct from a running
cosim participant.

Composed asset I/O uses `UsdRead::asset_identifier`, the canonical identifier
annotated by the maintained strongest-default OpenUSD resolver. Keep `asset`
for raw authoring/query text. Never reanchor a child layer's asset string against
the scene root. Initial/live preparation carries the same context; missing
consumed context is an error. Time-sampled assets without that annotation are
not admitted. Render consumers retain typed `load_asset_path` addresses and
reuse admitted handles. Binary arc classification preserves the full logical
filename; URL query/fragment semantics apply only to explicit HTTP addresses. DEM directory lookup uses the existing I/O worker and
asset owner's directory transport, with exact mount checks before publication.
See [asset provenance](../../docs/architecture/55-scene-addressing-and-roots.md#native-payload-and-source-admission).

## Law 2 — ask the scene root, never guess

To author a new top-level prim you need the target document *and* the parent
path. Both come from the scene root:

```rust
roots: Query<&UsdPrimPath, With<lunco_usd_bevy::UsdSceneRoot>>
let doc = lunco_usd_bevy_twin::scene_document_for(&backed, &asset_server, root.stage_handle.id())?;
let parent = &root.path;            // "/SandboxScene", "/World", …
```

Two failure modes this exists to prevent:

- **Counting the registry** ("there's only one document") — false. The registry
  also holds terrain and script documents.
- **Hardcoding `/World`** — the luncosim scene is rooted at `/SandboxScene`. A
  prim authored outside the mounted `defaultPrim` subtree *composes into the
  layer and is then never mounted*: it saves, it journals, and it is invisible.
  This failure is completely silent.

## Law 3 — spell it the way USD spells it

Use the real schema. `UsdLuxDomeLight` for an HDRI, `UsdPreviewSurface` for a
material, `UsdPhysics*` for physics. Before inventing anything, check whether
USD already defines it — a scene that leaves this app must still mean what it
said.

- `inputs:*` is the **UsdShade** namespace: it lives on a `Shader` prim, reached
  by `material:binding` → `outputs:surface`. A `float inputs:metallic` on a
  Sphere is not valid USD, and no DCC will read it back. Use
  `lunco_usd_core::material::ensure_preview_surface_ops()` — it builds the
  Material+Shader+binding for you, and it is deliberately in `lunco-usd-core` so
  every crate authors materials the same way.
- `primvars:displayColor` / `displayOpacity` are the *only* Gprim display
  attributes. There is no "display emissive" — **emission requires a material**.
- Genuinely new concepts get the `lunco:` vendor namespace (`lunco:dome:skybox`,
  `LunCoProceduralSkyAPI`, `lunco:terrain:*`). That is the correct, spec-sanctioned way to extend USD. What
  is *not* correct is inventing a second spelling for something USD already has.

The procedural camera-background contract is an `Xform` with
`LunCoProceduralSkyAPI` and a standard `UsdShade` material binding. Read
that API once in `lunco-usd-bevy` and stamp the existing render-free
`ProceduralSkybox` component. Do not project a `UsdGeomGprim` for the
background, carry `info:wgsl:vertexAsset`, or read the API again in a
downstream shader projector. `UsdLuxDomeLight` remains the standard path for
textured environment lighting.

**Never add an alias to make a file load.** A tolerant reader (`inputs:roughness`
*or* `perceptual_roughness` *or* bare `roughness`) is not robustness — it is a
trap. It teaches callers the invalid spelling and hides the bug: the writer
authors garbage, the reader accepts it, and the two conceal each other until the
file opens in Houdini and the material is gone. If the wrong form is authored,
the right behaviour is for it to visibly do nothing.

## Adding support for a new prim type or attribute

1. **Read it.** Extractors use the `UsdRead` trait
   (`lunco-usd-bevy-stage/src/read.rs`), implemented by both `StageView` (the live
   composed stage) and `UsdStageProjectionPlan` (the worker-produced initial
   snapshot). Authoring-layer reads use `UsdDataExt` separately; runtime
   extractors never switch to that source.
   - Floats: use `real` / `real_f32`, **never** `scalar::<f64>` — a `float`-
     authored value silently reads `None` through the f64 path.
   - Asset paths: `read_token` (it coerces `String`/`Token`/`AssetPath`), then
     `resolve_texture_path` to make it relative to the stage layer. Downloaded
     assets are `lunco://textures/…` (declared in a crate's `Assets.toml`).
2. **Dispatch it.** Prim types are a `match` on `reader.type_name(&path)` inside
   `instantiate_usd_prim_from_reader`. There is no registry to add to.
3. **Project it.** Insert components. Keep render-bound types out of
   `lunco-usd-bevy` — it is render-free by contract (`cargo tree -p lunco-usd-bevy
   -i wgpu` must be empty). `bevy_light` / `bevy_image` / `bevy_camera` are fine;
   `bevy_pbr` / `bevy_render` are not, and belong in `lunco-render-bevy`.
4. **Re-project it on edit.** *This is the step people forget.* A structural
   change (new prim) reconciles automatically. An **attribute-only** edit arrives
   as `info_only`. Standard transforms and lights stay on the generic path; a
   domain-specific in-place refresh registers a typed `UsdLiveEditOwner` with
   `lunco_usd_bevy_core::live_edit::UsdLiveEditRegistry`. The owner claims only
   its attributes, invalidates its own projection marker when required, and
   refreshes from the composed stage. Owners also promote a local subtree reset
   to a stage reset when their runtime topology crosses that subtree. Before a
   full reset, each owner retires its derived state synchronously; a rejection
   leaves the current projection in place and faults the active simulation.
   Keep `live_consume.rs` generic: if you add an editable attribute and skip both
   paths, `SetFoo` will journal and save correctly and **nothing will move on
   screen** until reload.
5. **Author it.** Add a command that lowers to `UsdOp`s (Law 1) and register it
   with `register_commands!` — a command is only reachable from the HTTP API /
   MCP / rhai if its *type* is in the reflect registry.
6. **Test it.** Because extractors use the shared composed-reader contract,
   unit-test the prepared `UsdStageProjectionPlan` for initial-load behavior
   and use a live `StageView` only for explicit authored-edit behavior — no App,
   no renderer. Keep pure animation topology/transform-reader tests in
   `lunco-usd-bevy-core/src/animation.rs`; runtime animation-system tests belong
   to `lunco-usd-bevy-animation`, and observable scene/policy assertions belong
   in Rhai. Do not create a test-only crate or import animation readers into the
   visual test target.

## Worked example

`crates/lunco-usd-bevy/src/dome.rs` (HDRI environment) is the whole checklist in
one file: standard schema (`UsdLuxDomeLight`), `lunco:` only for the two knobs
UsdLux genuinely lacks, a shared reader used by both the load path and the live-
edit path, an `info_only` refresh so runtime edits appear, a `SetDomeLight`
command that lowers to ops, and pure-function tests.

## Gotchas

- `bevy::init_asset::<A>()` is **destructive**, not idempotent — it wipes
  `Assets<A>` and swaps the allocator. Guard with `contains_resource`.
- The `CanonicalStage` is `NonSend` (openusd `Stage` is `!Send`). Read it under a
  short borrow and release it *before* mutating the world.
- `reconcile_structural_live` does nothing for a prim that exists **and** already
  has an entity — it spawns and despawns only. Refreshing an existing entity is
  your job.

Active `TwinClosed` retires the scene mount and publishes `SceneOwnerRetired`
before deferred teardown. Scene-time readiness closes immediately; pending
admissions and scene requests are discarded, the active transaction fails, and
`ClearScene` runs at the next lifecycle phase. Inactive Twin closure must not
clear another Twin's scene. See architecture 61 for the shared owner contract.
