---
name: edit-usd-assembly
description: >
  Create or modify a reusable LunCoSim USD assembly such as a rover, lander,
  payload, or sensor mount through the live headful Assembly Editor. Use when
  the user must see each change in the running window, give feedback between
  edits, and the agent must inspect the result with screenshots. Use the
  existing `assembly_edit` Rhai tools and typed USD commands; decompose
  reusable or articulated parts into separately testable USD components and
  compose them through typed references, datums, joints, and variants. Do not
  edit USDA text or ECS state directly.
---

# Interactive USD Assembly Editor

This is the human-and-agent workflow for editing an existing assembly. It is
not a second authoring API. The running production `target/debug/luncosim`
window is the shared workspace: the user sees the focused Editor preview, the
agent drives the same typed commands, and the agent inspects screenshots and
typed state after every coherent edit.

## Mandatory interactive mode

Start in **headful windowed mode**. Launch the production binary with an
explicit API port and without `--offscreen` or `--no-ui`:

```bash
target/debug/luncosim --api 4127
```

Use the existing headful session when one is already running. If no graphical
display is available, stop and report that the required interactive workflow
cannot be observed; do not silently switch to headless or offscreen mode.

The normal opening sequence is:

```bash
curl -s -X POST http://127.0.0.1:4127/api/commands \
  -H 'content-type: application/json' \
  -d '{"type":"ExecuteCommand","command":"OpenTwin","params":{"path":"<twin-root>"}}'
curl -s -X POST http://127.0.0.1:4127/api/commands \
  -H 'content-type: application/json' \
  -d '{"type":"ExecuteCommand","command":"ActivatePerspective","params":{"id":"editor"}}'
```

Wait for `/api/ready` to report `ready:true`, `world_hold:false`, and
`pending_count:0` after opening the Twin. Then query `ListOpenDocuments` and
use the returned id with the `assembly_edit` helpers. Do not replace these
steps with a guessed `doc` or a direct file path in an authoring command.

The user-visible window is part of the acceptance surface. Keep one session
alive while iterating. Do not apply a chain of unobserved edits and reveal only
the final file. After each coherent change set:

1. query the command acknowledgement and current document generation;
2. let the normal projection update the focused preview;
3. query the affected composed USD prim or session state;
4. capture a screenshot with `CaptureScreenshot` and inspect the PNG with the
   image viewer; and
5. show the result to the user and take feedback before the next material
   change or final save.

Use a project-local ignored artifact path such as
`target/assembly-editor/lander-after-mount.png` for screenshots. Do not create
an alternate screenshot or state protocol in `/tmp`, and do not treat a
screenshot as a substitute for typed USD or runtime verification.

For example, save the current visible frame through the existing screenshot
command:

```bash
mkdir -p target/assembly-editor
curl -s -X POST http://127.0.0.1:4127/api/commands \
  -H 'content-type: application/json' \
  -d '{"type":"ExecuteCommand","command":"CaptureScreenshot","params":{"save_to_file":true,"path":"target/assembly-editor/lander-after-mount.png"}}'
```

Read that local PNG with the image viewer before reporting the checkpoint.

## Respond to “the selected part is wrong”

Treat the visible selection as the user's referent, not a filename or display
name guessed from conversation. Read `selection_context()` first, then the
matching `viewport()` and `describe(doc)`. Require the focused preview, exact
selected paths, and matching ready document/projected generations. An empty,
stale, or ambiguous selection is a clarification point; a singular request
with multiple selected parts must not silently choose the primary entry.

Inspect the composed selected prim and capture the visible frame. If the
problem is not established by those facts and the user's requirements, ask
what should change before inventing a design. Explain the scoped correction in
ordinary language; users do not need to supply internal document handles.

Build a proposal from the captured identities and generation. Immediately
before committing, reread selection and document state. If focus, selected
paths, edit target, or generation changed, do not retarget or commit silently:
reinspect and resolve which selection the request applies to. Proposals carry
explicit paths and are not dynamically rebound to whatever is selected later.

After commit, wait for the matching projected generation, inspect composed
facts and the screenshot, and present the result for revision or undo. Use
the same document's `UndoDocument`/`RedoDocument` for feedback. Save remains a
separate approval gate. The authored `selection_ai_workflow` graphics scenario
exercises context isolation, selection-change detection, proposals, stale
generation rejection and undo; inspect its real production verdict, not just
frame completion. It does not substitute for user acceptance of a real design.

## Choose Editor or Builder

Use **Editor** (shown as `✎ Editor` in the perspective switcher) for one specific reusable assembly: open its USD document from
the Twin Browser, focus its isolated preview, and edit the authored prim tree.
This is the path for rover, lander, payload, and sensor work. Editor does not
show the live Entity list or spawn palette because those operate on the mounted
Twin rather than the selected document.

Use **Build** for general base composition: spawn and place complete USD
assemblies in the live Twin, and select them as one element. The group boundary
is the authored compound root projected through USD `PhysicsRigidBodyAPI` and
the existing `SelectableRoot`/`MobilityRoot` markers. Drill into the explicit
assembly document in Editor to modify its internal parts. Never add a second
group table or identify members by name prefixes.

`◉ View` is a separate live-Twin perspective for operating the simulation.
`USD · View N` tabs are separate presentation views inside Editor: each has
its own camera and render target while sharing the one explicit USD preview
stage. Neither kind of view is a document identity; query the explicit
handles before editing.

When leaving an Editor field to control a possessed vehicle in View or Build,
click the main 3D scene once. That scene press is the workbench's typed focus
handoff: it clears retained egui text focus while preserving capture for a
field that is still active. The controller then receives the normal
`InputBindingsSettings` → Leafwing `ActionState` → authored `ControlBinding`
path. Do not bypass this boundary with raw-key reads or a second vehicle input
path.

## Decompose the assembly like a lightweight CAD product

Before changing geometry, write the design intent as contracts. Separate facts
supported by public or global references from Twin study assumptions, and give
each independently reusable, articulated, or domain-owning part an explicit
owner. A useful split is:

- the assembly file owns the vehicle datum, placement, references, collection
  membership, host-facing joints, and cross-component connections;
- a component file owns one reusable part's local geometry, mass/collision
  envelope, mount datums, parameters, and internal mechanism;
- a Modelica scope owns domain equations, while a Rhai scenario owns mission
  sequencing and limiters;
- Rust owns only generic typed USD, physics, projection, and solver seams.

Do not split merely to create ceremony. Keep a part in the assembly when it is
only a one-off visual detail with no independent mount, mechanism, runtime
contract, or useful test. For an articulated or reusable part, use a separate
file under the Twin's `components/` tree with one explicit root and one clear
composition boundary. The component should be usable without the final
assembly, while the assembly should be understandable from its manifest and
references without opening every implementation detail.

For every component, create the smallest useful contract before detailed
geometry: required prim names/types, local frame and mount datum, dimensions or
envelope, mass/inertia ownership, collision policy, public parameters and their
units, and any known limits. Put the component's read-only requirement report
and boundary cases beside its Rhai authoring tool. The assembly gets a second
contract that checks counts, placement, symmetry, clearances, references,
joint endpoints, and cross-component wiring. A component passing alone does
not prove the assembled vehicle is correct.

Use this live loop for each component and then for the assembly:

1. Open or create the exact USD document in the headful Editor and inspect its
   document id, authored layer, target, and generation.
2. Build a pure Rhai typed-op plan. Use `AddPrim`, standard schemas,
   `SetAttribute`, `SetTranslate`/`SetRotate`/`SetScale`, relationships, and
   connections. Let the document owner maintain `xformOpOrder`; never inject
   that property manually.
3. Review and commit one coherent component change, wait for projection, query
   the affected composed prims, and capture/inspect a screenshot.
4. Run that component's requirement test in the same live process. Repeat the
   same session for the assembly integration, then run the cross-component
   suite against the composed root. Do not use a second simulator launch just
   to run a component or assembly test.
5. Save each component and the assembly only after the visual and typed
   checkpoints are acceptable. Record the exact files and known gaps.

When a component is deliberately a global reference, validate the referenced
asset's contract and keep local opinions limited to its instance transform,
mount metadata, variants, and host wiring. When a new Twin-owned component is
needed, author it through the live Editor and save it as a separate file; do
not paste a flattened copy of a reference or import a private CAD/FreeCAD
file as an undocumented authority.

## Variants and levels of detail

Use a USD variant set at the component or assembly boundary for real
configuration choices such as `stowed`/`deployed`, `payload_class`, or an
alternate equipment package. Keep the base contract common and place only the
opinions that differ inside the variant. Select variants with the typed
`SetVariantSelection` path after the referenced subtree is materialized, then
rerun the component and assembly checks for every supported selection.

Do not emulate a variant with duplicate top-level parts, name suffixes, or a
permanent invisible placeholder. If the live typed surface can select existing
variants but cannot create a variant set or author variant blocks, report that
as a Rust/typed-editor capability gap; do not fake canonical USD composition
with visibility flags. Reference-list replacement and authored metadata such as
`kind`/`defaultPrim` are available through typed editor operations; keep the
selected layer and generation explicit when using them.

Model at contract fidelity, not manufacturing detail. Use standard primitive
geometry for the silhouette, mounting envelopes, collision surfaces, and
visual features that the requirements actually inspect. Add meshes or
parametric detail only when it changes a requirement, a mating interface, a
physics envelope, or the visible identity of the vehicle. This keeps the live
CAD loop fast without making the component a placeholder.

## Open the exact document and preview

The document registry and OpenUSD composition system are authoritative. Never
guess a document id, preview id, prim path, or edit layer from a name or from
the active simulation viewport.

1. Open a Twin folder with `OpenTwin` (its path contains `twin.toml`), open a
   source with `assembly_edit::open(path)`, create a new assembly with
   `assembly_edit::new_document()`, or fork an existing document with
   `assembly_edit::fork_document(source, name)`. These are the normal async
   document lifecycle paths; `LoadScene` is only for mounting a scene.
2. Query `ListOpenDocuments` and select the returned `DocumentId`.
   IDs are process-wide live handles shared by all document domains. Confirm
   the returned kind and file origin; never derive an ID from an entity or
   reuse a number from a previous launch. A fork has its own fresh handle.
3. Query `assembly_edit::viewport()` and capture a screenshot when the user
   asks what is visible. Correlate the focused preview/view and visible tabs
   with `ListOpenDocuments`; use the returned explicit handles, never a title.
4. Use `assembly_edit::describe(doc)`, `inspect(doc, path)`, and
   `resolve_target(doc, path, edit_target)` to read composed topology, layer
   ownership, generation, and the legal target. Use `sync_document` when an
   already-known generation can be advanced by a typed delta.
5. Select the `Editor` perspective from the title-bar switcher, or activate it
   through the typed `ActivatePerspective { id: "editor" }` command, then open
   an isolated preview with `OpenUsdPreview { preview, doc_id, edit_target }`; the
   preview's primary view is exposed as an instance-backed dock tab. Use
   `FocusUsdPreview { preview }` when changing the visible document; it
   foregrounds that tab. Every panel and selection must remain bound to that
   `UsdPreviewId`.
6. For a second 3D view of the same assembly, use
   `OpenUsdPreviewView { preview, view }`. It creates a new camera/render
   target over the existing projected stage; it does not reload or duplicate
   USD. Opened view tabs can be dragged to a dock edge to create a split.
   Use `FocusUsdPreviewView` for the exact view and `CloseUsdPreviewView` when
   finished. The runtime parks cameras for hidden tabs and only sizes visible
   view targets from their measured dock rects. The USD viewport's
   `UsdPreviewRenderBudget` caps each target at 2048 px per axis and 4,194,304
   pixels, and caps visible views at 8,388,608 pixels per frame by default.
   These are presentation budgets, not authored USD values; invalid zero limits
   leave the target inactive.
   Wait for `InspectUsdViewport` to report `projection_ready: true` before
   selecting a prim or issuing an authoring command. The flag becomes true only
   after the preview root and all projected descendants have cleared their USD
   projection and asynchronous mesh phases; `projected_generation` is valid for
   edits only at that boundary.

   Navigate the focused view with primary/left-drag pan, secondary/right-drag
   orbit, middle-drag pan, and wheel zoom. The toolbar and agents use the same
   typed commands:
   `SetUsdPreviewViewMode { view, mode: "visual"|"text" }` and
   `SetUsdPreviewTextLayer { view, layer: "authored"|"composed" }` switch the
   presentation of the same preview session. Visual mode owns the shared
   projected stage and camera controls; Text mode is a read-only,
   generation-matched USDA snapshot. Switching modes never opens another
   document or resets selection/camera state. Use the text-layer command only
   while inspecting authored or composed source; mutations still go through
   typed USD operations.
   `SetUsdPreviewProjection { view, projection: "perspective"|"orthographic" }`,
   `PanUsdPreviewView { view, delta: [x, y] }`,
   `ZoomUsdPreviewView { view, factor }`, `FrameUsdPreviewView { view }`, and
   `ResetUsdPreviewView { view }`. These are view presentation operations; they
   do not author USD camera or transform values. Pan converts logical pointer
   deltas through the active projection and measured render-target viewport.
   `Frame` uses the projected
   visual bounds, and `InspectUsdViewport` reports projection, target, distance,
   and orthographic scale for screenshot correlation.

   For multi-part inspection, wait for `projection_ready: true`, then use the
   same explicit preview/document handles with
   `ExplodeUsdPreview { preview, doc_id, assembly, parts, action, axis, spacing }`.
   `action` is `Enable`, `Update`, or `Reset`; `assembly` must be an authored
   `kind = "assembly"` path and `parts` must be non-empty exact composed paths
   below it. Parts are stably ordered by path. Enable captures original local
   transforms, update reuses that baseline, and reset restores it. The
   operation is session-scoped presentation state: it never authors USD or
   changes journal/save/physics state, and reprojection/close clears it.

The built-in wrappers are in
[`assembly_edit.rhai`](../../assets/scripting/tools/assembly_edit.rhai). The
preview helpers are `preview_open`, `preview_view_open`,
`preview_view_focus`, `preview_view_mode`, `preview_view_text_layer`,
`preview_view_projection`, `preview_view_frame`,
`preview_view_reset`, `preview_view_pan`, `preview_view_zoom`,
`preview_view_close`, `preview_focus`, `preview_close`,
`preview_explode_enable`, `preview_explode_update`, and
`preview_explode_reset`. Use
`assembly_ui::select_prim(preview, path, extend, toggle)`
for selection: it focuses the explicit preview and dispatches `SelectUsdPrim`,
which resolves the path only inside that preview's stage and hierarchy.
After a selection or screenshot checkpoint, call
`assembly_edit::selection_context()` to expose the focused preview's exact
`DocumentId`/`UsdPreviewId`/prim-path identities, composed USD type and kind,
parent/assembly paths, primary and Inspector-target paths, and the existing
typed operation families. Use `selection_context_for(preview)` to inspect a
hidden open preview without changing the user's visible focus. The response
marks no-selection, multi-selection, stale entries, and duplicate projected
paths explicitly. Never use the returned display `name` as an edit key; pass
the returned exact path and document/edit target to the existing typed helper.

When the existing libraries do not express a reusable policy, create a
Twin-scoped or shared library following
[`author-rhai-tool`](../author-rhai-tool/SKILL.md). Keep the new function a
pure `*_plan` when it authors USD, and use a separate read-only `*_report` or
`*_lint` for requirements. Register it with `RegisterToolLibrary`, verify a
real namespaced call after the tool-generation maintenance pass, and keep the
same headful process for the edit, readback, screenshot, and test. A tool
registry listing is not proof that its module is callable.

For a document-scoped authoring check, use `cmd("RunLint", #{domain: "usd",
doc_id: doc})` only after the document projection is current, then read
`query("LintReport", #{doc_id: doc})`. The response is scoped to that document
and reports the generation and `projection_ready`; an unprojected document is
reported as not ready rather than linting a stale stage. The report also warns
when one composed entity has duplicate public port owners, including each
owner's USD path and registry precedence. Resolve that at the authoring
boundary by giving separate semantic owners distinct names; do not hide it with
a runtime write retry or fallback.

For semantic component construction, use
`assembly_builder::component_bundle_facts` and
`assembly_builder::component_bundle_plan` for reusable geometry, collision,
mass, dimensions, explicit frame paths, and actuator endpoint contracts.
Validate the facts, append the returned `.ops` to one reviewed proposal, then
query the composed root and children. The bundle is Rhai policy over existing
typed USD operations: it uses `UsdGeom`, `UsdPhysics`, `UsdShade`, `kind`, and
`inputs:`/`outputs:` where those standard owners fit. Draft units, limits, and
deployment state stay caller-side or in the owning Modelica/joint contract; do
not mirror them into unregistered `lunco:` properties. It does not create a
material, infer a rigid body/joint, or hide a missing mount relationship. Use
the explicit body/joint planners for articulation and
`find_compatible_socket`/`mount_component` for an authored socket attachment.
Do not hand-author a reference plus guessed transform when a component
advertises a mount plug.

Mission-specific construction belongs in the owning Twin's Rhai tool library.
Keep the core workflow generic: compose a referenced instance, wait for its
children, and submit explicit typed operations through the proposal/journal
boundary. Treat mass, inertia, dimensions, and frame values as caller-supplied
inputs; inspect exact composed prims and proposal diagnostics before
committing an authored edit. A Twin recipe may compose these generic plans,
but it must not add vehicle-specific builders or writers to the core.

For composed assembly diagnostics, use the companion
[`assembly_audit.rhai`](../../assets/scripting/tools/assembly_audit.rhai).
Pass the exact document id as the first argument to every stage-reading helper,
then the explicit manifest. Use `()` only for a mounted live-scene audit, never
to select the focused preview. `QueryUsdPrim` rejects closed, unmapped, and stale
document projections; wait for the document projection before auditing it.
Inspect its structured reports for topology,
mount reciprocity, joint frames, rigid-body/joint coverage, and
mass/inertia/collider coverage before proposing an edit. Its `explode_plan`
only returns preview deltas; it does not write USD or bypass the journal. Do
not list raycast wheels as rigid bodies or invent a joint to make that audit
pass. Missing relationships, bodies, or colliders are defects to fix at their
authored owner.
Use `assembly_audit::physicality_report(doc, manifest)` when an asset needs one
fail-closed physical/visual-only manifest. A `physical` entry delegates its
explicit `mass`, `inertia`, `joints`, and `colliders` to the existing audits. A
`visual-only` entry requires a non-empty reason and no composed collision
envelope. Duplicate paths, unknown roles, missing prims, and incomplete
coverage remain visible structured errors; the tool is dynamically reloadable
Rhai policy and does not create a parallel geometry or USD writer.
For a direct standard-schema compliance check on a generated or hand-edited
component, use `assembly_audit::standard_component_report(doc, manifest)`.
Provide the exact root and part paths plus the expected standard `type_name`,
shape, physics, visibility, purpose, and material-binding facts. It is
read-only, manifest-driven, and reports errors instead of repairing or
inferring missing fields; use it before proposing a regeneration or committing
an external USDA edit.

For `joint_frame_report`, supply `axis` only for revolute, prismatic or
spherical joints (their standard omitted axis is X). Fixed payload adapters,
distance joints and generic `PhysicsJoint` have no primary axis. Use
`require_positions`/`require_rotations` only when the design requires explicit
authored frames. Run `assembly_joint_audit` for the diagnostic regression.

To inspect and adjust a joint interactively, select the joint prim in the
Editor Prims tree and use the Inspector's `USD Joint` `Frames` controls for
`Local position 0` and `Local position 1`. Values are canonical metres in the
displayed basis. The focused USD preview draws each authored frame as an amber
anchor with red/green/blue XYZ arrows and links the two anchors, so a changed
position is visible on the composed bodies after reprojection. The markers are
preview-only and do not create runtime physics joints; do not move the joint
prim's transform when the intent is to change an endpoint frame. Position
changes use the existing generation-checked, journalled `ApplyUsdOp` path and
remain undoable. Non-finite values are rejected visibly by the Inspector.

For numeric part transforms, use the focused preview's Inspector Transform
section for referenced children as well as locally authored parts. Wait for
the component's dependency closure to load before proposing transforms; a
missing composed child must reject, not be manufactured by a preliminary
attribute edit. Check inherited transform order and undo with the authored
`referenced_part_edit` graphics regression when changing this owner.

Use the Inspector Transform
section or call the existing `assembly_edit::transform(doc, edit_target, path,
translation, rotation, scale, parent_gen)` helper with the exact selection
context. Preview values are local canonical metres, Euler XYZ degrees, and
unitless scale factors; the Inspector commits changed translation, rotation,
and/or scale fields as one journaled `ApplyUsdOps` edit. The typed
`UsdOp::SetScale` operation preserves the standard `xformOpOrder` and stage
unit/axis boundary.

For multiple scalar component values, edit the draft Parameters section and
press **Apply parameters** once. The Inspector commits only changed values as
one generation-checked `ApplyUsdOps` edit. AI callers use
`assembly_builder::parameter_plan` and the same `assembly_edit::batch` or
proposal flow; do not make one command per slider or create a vehicle-specific
parameter writer.
When editing a primitive's standard `axis`, compare the visible result after
reprojection with the composed attribute. Repeated axis edits must apply the
axis correction once to the authored pose, including identity rotation. A
generation acknowledgement alone does not prove that the visible pose agrees.
The standard transform gizmo is also available for the focused preview: select
the exact prim, drag its unparented proxy, and release to commit the changed
local translation, Euler XYZ rotation, and/or unitless scale as one
generation-checked `ApplyUsdOps` change set. Preview gizmo drags use Bevy's parent-local
`GlobalTransform::reparented_to` conversion and never create a live physics
identity or hold. Escape cancels; if the preview generation changes or the
session closes, the stale transaction is discarded and the newer USD projection
is left authoritative. Live simulation entities continue to use the BigSpace
gizmo path and `MoveEntity`/`TransformEntity` boundary.
Do not send a preview entity through `MoveEntity`: that command owns live
BigSpace/physics identities, while the preview owns only the explicit USD
document lease. Commit after a coherent value entry so one gesture creates one
change set, then inspect the returned generation and screenshot the projected
result.

The standard gizmo is bound to the presentation owner. A visible focused
preview camera receives `GizmoCamera`, and the measured workbench `PanelRects`
becomes its logical `GizmoOptions::viewport_rect`. Both the singleton preview
and separate preview tabs publish `SceneTarget::Offscreen` through the shared
`ScenePickGate`; the maintained gizmo picking backend consumes the same
rectangle used by rendering before testing handles. This admits a handle drag
despite the global egui focus flag while keeping live-scene input excluded.
Do not add panel-specific cursor math or a second gizmo driver.
The presentation helpers are in
[`assembly_ui.rhai`](../../assets/scripting/tools/assembly_ui.rhai): use
`panel_templates(preview, doc, edit_target)` to discover the nine existing
Editor surfaces/workflows and their explicit handles. `open_session(preview,
doc, edit_target)` activates the Editor, opens/focuses the explicit preview,
and foregrounds its primary view tab. Use `focus`, `open_structure`, `open_inspector`,
`open_connections`, `open_animation`, `open_mount`, and `open_review` to focus
the owning registered panels. Animation is an Environment section and mount
and review are Inspector sections; persistence is the existing document
lifecycle command group and has no fabricated panel. These helpers only
dispatch existing `ActivatePerspective`, preview-focus, and `FocusPanel`
commands; they do not own layout or create parallel document/view state.
Discover reflected command shapes with `DiscoverSchema` rather than inventing
JSON for a new command.

## Choose the USD ownership scope

Resolve the target before authoring. Select the smallest scope that owns the
fact:

| Scope | Author here |
|---|---|
| `SourceAsset` | reusable lander/component geometry, ports, mount plug, or physical contract |
| `Assembly` | Twin-owned wrapper, component references, sockets, joints, and assembly composition |
| `InstanceOverride` | one composed mission instance, only when the existing USD arc permits the override |

For a referenced or payloaded prim, a composed read may be read-only at the
current layer. Use the explicit `edit_scope` from `ResolveUsdTarget`; fork an
open document with `ForkDocument` when an independent editable assembly is
intended, then use `SaveAsDocument` to give it a file identity. Never write a
silent override into a different layer or edit the source file behind the
editor's back.

## Make an edit through the existing tools

Use the smallest existing typed intent that expresses the change:

- `transform`, `attribute`, `schema`, `variant`, `relationship`, and
  `connection` lower to `ApplyUsdOp`/`ApplyUsdOps`.
- Dynamic shader parameters are authored on the bound USD `Shader` prim as
  `inputs:<name>`. Resolve the composed Shader and exact declared `typeName`
  first; preserve USD roles and array shape, validate the literal with the
  shared USD parser, and submit one grouped edit for a multi-field change. The
  same resolver is used by `SetObjectProperty`; never guess a geometry
  `primvars:` destination from a parameter name.
- `add_prim`, `remove_prim`, `move_prim`, `payload`, and `active` expose the
  existing typed structural USD operations. They require an explicit target,
  exact paths, and the inspected generation; they never replace a layer's raw
  source.
- Source-defined geometry tools follow the same explicit contract. For example,
  `nurbs::set_points(doc, path, points, parent_gen)` returns the `ApplyUsdOp`
  result; pass the `DocumentId` and generation from `describe`/`QueryUsdPrim`,
  and inspect `ok`, `data.doc_id`, `data.paths`, `data.generation`, and
  `data.change_set_id` before treating the edit as accepted. Use
  `nurbs::set_point(doc, path, index, point)` only when the boolean convenience
  result is sufficient. Never infer the document from the active editor tab.
- `assembly_edit::references(doc, edit_target, path, references, list_op,
  parent_gen)` authors existing reference arcs through `SetReferenceArcs`.
  Each entry carries an asset identity and optional absolute target prim path.
  `Prepend`, `Append`, `Add`, and `Delete` preserve weaker-layer opinions;
  `Explicit` replaces the selected layer's list and an empty explicit list
  clears it. Inspect `references.authored` and `references.composed` before
  editing, and resolve the target first so a composed-only prim is reported as
  read-only rather than flattened or guessed.
- `assembly_edit::default_prim(doc, edit_target, path, parent_gen)` authors the
  stage root `defaultPrim` in `@root@` or `@runtime@`; use an absolute or
  root-relative existing prim path, and pass `()` to clear only that layer.
  `assembly_edit::prim_kind(doc, edit_target, path, kind, parent_gen)` authors
  a USD identifier such as `component`, `assembly`, or `group` on an existing
  prim; pass `()` to clear that layer's opinion. Inspect
  `metadata.defaultPrim` and `prim.metadata.kind` for root/runtime,
  document-composed, and canonical-stage values with their source labels.
- `assembly_edit::attach_component` and `assembly_edit::detach_component` use
  the existing mount, socket, joint, frame, ownership, and occupancy
  validators. Supply exact paths in the
  reflected `AttachSpec`/`DetachSpec`; never identify a part by a name prefix
  such as `Wheel_`.
- For an existing socket attachment, use
  `assembly_builder::mount_frame_realignment_plan` with the exact host, socket,
  part, and recorded joint paths. It composes nested canonical rigid
  `translate`/`rotateXYZ` frame chains in the dynamically reloadable Rhai tool,
  emits one reviewed part-transform plus joint-anchor plan, and preserves
  topology. It rejects missing or ambiguous mount relationships, unsupported
  frame operations, non-unit scale, malformed values, and body mismatches
  before proposal; do not hand-copy the socket pose into the part or joint.
- Before choosing an assembly action, use
  `assembly_builder::authoring_context(doc, path, edit_target)`. It returns the
  exact identity, generation, resolved authored target, topology/collision
  facts, socket occupancy, plug relationships, and only the actions supported
  by those authored facts. For a dry AI or human intent, use
  `assembly_builder::place_or_attach_plan` with either
  `mode: "attach_component"` (review `.spec`, then call the existing
  `assembly_edit::attach_component`) or `mode: "realign_existing_mount"`
  (review `.ops`, then use the normal proposal flow). Never add an AI-only
  writer or guess a frame from a part name.
- When the user is already working in the Editor, use
  `assembly_builder::selected_authoring_context(preview)` to bridge the exact
  single selection into the same authoring record. Pass `()` for the focused
  preview or an explicit preview id for a hidden session. It rejects no,
  multiple, stale, and ambiguous selection state; preserve its selection
  identity and generation until proposal review.
- Use `assembly_builder::functional_frame_catalog(doc, edit_target, root_path)`
  to read the registered mount frame. It follows only the authored
  `lunco:mount:frame` relationship and returns its exact path, standard
  transform facts, and socket paths. Keep generic datum and actuator paths as
  explicit plan inputs consumed by standard joint or Modelica contracts. Use
  `assembly_builder::align_frames_plan(doc, edit_target, moving_path,
  moving_frame_path, target_path, target_frame_path)` for a dry two-op visual
  placement plan; the roots must be sibling Xforms and the frame stacks must
  be rigid `translate`/`rotateXYZ` with unit scale. Physical mount topology
  still goes through the attach/realignment planners above.
- `assembly_edit::attach_program(doc, spec)` dispatches the existing typed
  `AttachProgram` contract. Build its `inputs` and `outputs` with the
  namespaced helpers `assembly_edit::program_input_connection`,
  `assembly_edit::program_input_default`, and `assembly_edit::program_output`;
  the source asset, host path, program name, edit target, and port paths remain
  explicit in `spec`.
- `assembly_edit::rigid_body_plan(edit_target, parent_path, name, mass,
  center_of_mass, diagonal_inertia)` returns a typed operation plan for a new
  body frame with explicit `PhysicsRigidBodyAPI`, `PhysicsMassAPI`, mass,
  centre-of-mass, and diagonal inertia. `assembly_edit::revolute_joint_plan`
  similarly requires two body paths, both local anchors and quaternions, a
  cardinal axis, ordered degree limits, and collision policy. Append the
  returned `.ops` to one reviewed proposal; these helpers do not author
  transforms, shapes, or hidden defaults.
- Use `assembly_edit::fixed_joint_plan` for a rigid adapter. It requires two
  distinct absolute body paths, both local anchors and quaternions, and an
  explicit collision policy, returning the standard `PhysicsFixedJoint`
  operations for the same reviewed proposal boundary.
- For construction from parts, use the hot-reloadable
  `assembly_builder` library. `place_plan` emits a local transform plan;
  `frame_plan` emits a validated Xform frame; `cube_plan`,
  `cylinder_shape_plan`, and `movable_cube_plan` compose standard geometry, collider,
  body, mass, and placement facts; `existing_rigid_body_plan` promotes an
  exact referenced Xform by defining one local over while retaining its authored
  geometry; it rejects a repeat once a complete body contract is already
  authored; `hinge_plan` delegates to the framed revolute planner; and
  `align_centers_plan`/`align_cube_edges_plan` derive placements from explicit
  queried paths and reject unsupported parent/frame assumptions. These helpers
  return the same typed `.ops` consumed by `propose`/`batch`; completed
  body/joint identities are rejected by the construction recipe and require an
  explicit update plan; these helpers do not write USDA or bypass the document
  owner.
- For a reusable referenced assembly, use
  `assembly_builder::referenced_instance_plan` or
  `assembly_builder::referenced_instance_targeted_plan` when the source prim
  must be explicit, or
  `assembly_builder::referenced_instance_plan` (or its targeted form) to
  author the explicit component identity, asset URI, parent, and local pose.
  Materialize first-use references before applying composition-changing
  variants; once the composed children are queryable, use
  `assembly_builder::select_variants_plan` as a second reviewed plan. Parameter
  changes use `assembly_builder::parameter_plan` and the same typed
  `ApplyUsdOps` command path as Inspector edits. This sequencing keeps async
  reference loading and coarse variant recomposition from producing a root
  with missing children.
- For generic human or AI property editing, start with
  `assembly_builder::editable_property_catalog(doc, path, edit_target,
  requested)`. Pass an explicit field array for a focused view or `()` to
  discover supported standard `UsdGeom`, `UsdPhysics`, `UsdShade`, `kind`,
  variant, and `inputs:`/`outputs:` fields. The result includes the USD owner,
  exact type, units, composed value, USDA literal, authored/editable status,
  edit scope, and source path. `xformOpOrder` and `extent` are visible but
  read-only; unknown names and guessed `lunco:` fields fail visibly.
- Build a dry change set with
  `assembly_builder::editable_property_patch_plan(doc, edit_target, path,
  edits, parent_gen)`, where each edit is `{ name, value, type_name }`.
  It reuses the existing typed transform, attribute, relationship, kind, and
  variant operations, checks the exact generation and target scope, rejects
  wrong types/paths and structural edits, and reports a true `no_op` with an
  empty `.ops` list when values already match. Submit `.ops` through the
  normal `assembly_edit::propose`/`review_session`/`commit_proposal` flow; the
  planner never writes USDA directly. `InspectUsdDocument` exposes standard
  variant selections at `prim.metadata.variantSelections` for the same
  human/AI read path.
- For repeated references, use
  `assembly_builder::referenced_instance_pattern_plan` with one explicit
  template and an ordered array of `{ name, translation, rotation, scale }`
  placements. It preserves that order, composes the existing referenced
  instance planner, and rejects duplicate names or placements without a local
  pose. For a reflected placement, use
  `assembly_builder::referenced_instance_mirror_plan`; provide a cardinal
  local axis and source translation. It rejects non-zero source Euler rotation
  because the reflected orientation must be authored explicitly, rather than
  silently guessed. Review the returned `.ops` before committing.
- Use `assembly_builder::place_with_clearance_plan` when placing a part near
  other authored geometry. Supply the exact moving frame and Cube shape plus
  every exact blocker frame and Cube shape. The frames must share a
  translation-only authored parent; the helper rejects rotated frames,
  non-Cube geometry, duplicate blockers, overlap, and less than the requested
  gap before proposal. The returned transform remains a normal reviewed
  change set.
- Use `assembly_builder::place_with_collision_clearance_plan` for referenced,
  Cylinder, Mesh, or compound bodies. Supply exact moving/blocker body paths
  and a minimum gap; it reads `QueryUsdPrim { collision_bounds: true }` from
  the shared composed collision owner, requires a translation-only common
  parent chain, and rejects missing, malformed, unsupported, overlapping, or
  duplicated collision envelopes before proposal. This is the preferred
  dynamic Rhai path for general assembly placement; it does not duplicate
  primitive dimensions or write USD directly.
- For one selected prim's complete read-only visual/physics explanation, use
  `QueryUsdPrim { topology: true }`. The result scopes to the nearest
  `PhysicsRigidBodyAPI` ancestor (or the selected prim), then returns one
  `topology.parts` list with visual/collider flags, inherited purpose,
  collision state, body owner, per-shape canonical bounds, local/world frames,
  source-layer head, and render/physics material plus resolved shader paths.
  `topology.joints` reports standard joint body targets in the same scope;
  `topology.diagnostics` is authoritative for malformed or unavailable facts.
  `topology.projection` and `topology.binding` expose composed generation and
  the existing USD projection markers. This query is opt-in, read-only, and
  does not replace selection or create a second scene graph.
- For general-body snap editing, use
  `assembly_builder::align_collision_centers_plan` or
  `assembly_builder::align_collision_edges_plan`. They align aggregate
  composed bounds for exact sibling paths on an explicit axis, edge, and gap,
  preserve unrelated translation components, and return reviewed
  `SetTranslate` plans. The same translation-only parent-chain and
  malformed/missing collision-data checks apply.
- `batch` or a proposal is one journal/change-set unit when an intent changes
  multiple facts. Supply the inspected `parent_gen` so a stale edit fails
  atomically.
- `propose` → `review_session` / `review_proposal` → `commit_proposal` is the
  interactive review flow for a multi-operation change. Proposal creation and
  review do not mutate the document; commit enters the ordinary USD journal and
  undo path. A conflict requires a fresh inspection and proposal.

For a new assembly, use `NewDocument` or `ForkDocument`, add existing assets by
USD reference through the typed operation/attachment surface, and author
standard `UsdGeom`, `UsdPhysics`, `UsdShade`, and `UsdLux` fields. Do not make
a lander-specific command, a second resolver, a direct USDA writer, or a
runtime-only ECS setter.

Every actual asset edit must happen while the headful session is visible and
through the command/API or Editor panel. Do not use `sed`, a generated USDA
replacement, a direct filesystem write, or direct ECS mutation to get around a
rejected command. A rejected target or invalid topology is feedback from the
authoritative owner and must be fixed there.

## Concrete assembly workflow

Open the target Twin, inspect the composed assembly, and decide whether the
requested change belongs in the Twin wrapper or in the reusable source asset.
For a new payload, inspect the host's composed mount sockets and the
component's plug frame first, then submit one validated `AttachComponent`
intent. The lowering authors the reference, placement, joint, relationships,
and socket occupancy together. For a change to an existing mounted part,
submit its exact component, joint, and optional socket paths to
`DetachComponent` or use the typed transform/attribute operation at the
resolved layer; do not re-create the component merely to change its pose.

After each change, visually check the lander's geometry, material, pose, joint
attachment, and collision relationship in the focused preview. A visually
plausible result is not enough: query the composed paths and verify that the
expected `UsdPhysics` bodies/joints and authored relationships exist.

For a repeatable lander workflow, use the returned document identity and
generation rather than a path-derived guess:

```rhai
let before = assembly_edit::describe(lander_doc);
let target = assembly_edit::resolve_target(lander_doc, lander_path, "@root@");
assembly_ui::open_session(lander_preview, lander_doc, "@root@");
let proposal = assembly_edit::propose(
    lander_doc,
    "Assembly",
    "Adjust lander pose",
    inspected_typed_ops,
    before.generation,
);
```

Here `inspected_typed_ops` is the exact `UsdOp` plan built from composed
inspection. Review the proposal in the existing Inspector, commit it after the
visible checkpoint, then inspect the affected paths with the new generation.
For a mount, submit the exact reflected `AttachSpec` obtained from
`DiscoverSchema` to `assembly_edit::attach_component`; it must carry the
component, socket, plug, joint, frame, and ownership paths from the composed
inspection.

For a multi-asset workflow, keep the documents and preview leases independent:

```rhai
let lander_before = assembly_edit::describe(lander_doc);
let payload_before = assembly_edit::describe(payload_doc);
assembly_ui::open_session(lander_preview, lander_doc, "@root@");
assembly_ui::open_session(payload_preview, payload_doc, "@root@");
let ack = assembly_edit::batch(
    lander_doc,
    "Place inspected payload components",
    [
        #{ SetTranslate: #{ edit_target: "@root@", path: lander_path, value: lander_translation } },
        #{ SetTranslate: #{ edit_target: "@root@", path: second_lander_path, value: second_lander_translation } },
    ],
    lander_before.generation,
);
let payload_unchanged = assembly_edit::sync_document(payload_doc, payload_before.generation);
```

Use the reflected `AttachSpec` with `assembly_edit::attach_component` when the
assets must become one mounted assembly; do not merge source documents by
copying layers. The translated paths are inspected paths in `lander_doc`,
while `payload_before` remains the payload document's independent revision
cursor. A stale generation is a rejected edit and requires a fresh inspection
before retrying.

## Save, verify, and close

Do not save automatically as part of proposal commit. After the user approves
the visible result, call `assembly_edit::save_document(doc)` for a file-backed
document or `assembly_edit::save_as_document(doc, path)` for a fork, then
confirm the document is no longer dirty with
`ListOpenDocuments`/`InspectUsdDocument`. Use
`assembly_edit::discard_document(doc)` to restore the file through the owner,
or `assembly_edit::close_document(doc)` after the final checkpoint. Keep
undo/redo available through `UndoDocument` and `RedoDocument` during feedback.

For runtime behavior, use the production scene/scenario gate and inspect its
real verdict. An Editor preview must not synthesize or execute Modelica merely
because it contains a component collection; use `preview_domain_isolation`
when changing that runtime admission boundary. Preview queries remain valid
even when the authored network is incomplete.

For physical acceptance, inspect the production scene/scenario's
real verdict. `--validate` is parse/preflight only. For code or authored asset
changes, run the narrowest relevant checks after the interactive session; do
not claim a screenshot proves physics, persistence, or reload. Before closing,
capture the final screenshot and typed state, then use `CloseUsdPreview` and
the API `Exit` only for sessions owned by this agent. Verify the process and
port are gone.

## Non-negotiable boundaries

- USD owns identity, topology, references, frames, parameters, materials, and
  physics facts; ECS is only its projection.
- A hierarchy is not an attachment. A movable mounted rigid body needs its
  authored `UsdPhysics` joint and frames.
- `UsdPreviewId` owns transient session selection and panel state while each
  `UsdPreviewViewId` owns one presentation camera/render target; neither is
  authored into USD.
- Use standard USD schemas whenever they own the concept, and existing
  `lunco-usd-compose`, journal, mount, and transform-frame owners whenever they
  already implement it.
- No name-based discovery, compatibility alias, legacy path, fallback layer,
  direct file mutation, or second history/state mechanism.
