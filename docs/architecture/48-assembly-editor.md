# 48 — Assembly editor

> Status: Implemented substrate; production program-attachment gate included · Audience:
> contributors on the Assembly editor and reusable components

The Assembly editor is the authoring surface for assembling and editing a
separate USD document. It projects that document's prims and connections into
the existing canvas, prim-tree, inspector, and isolated USD viewport surfaces.
It does not create a second scene model or a vehicle-specific assembly API.

The workbench exposes this surface through the registered `editor`
perspective. The registered `terrain_sculpt` perspective exposes the existing
terrain tools and keeps sculpting separate from object assembly. Editor is
available in the default title-bar switcher; terrain sculpting remains an
explicit authored mode available through the typed `ActivatePerspective`
command. They are composed from the panels listed below; they do not
introduce a second authoring model.

## Assembly Editor and Builder boundaries

Editor is the focused authoring workspace for one explicit USD document at a
time. Open a rover, lander, or other assembly from the Twin Browser, then use
its isolated `UsdPreviewId` and USD prim tree to inspect and edit the authored
parts. The Editor layout does not expose the live Entity list or spawn palette,
so a mounted scene entity cannot be mistaken for the document being authored.

Build is the general live-Twin composition workspace. It owns the scene entity
tree, catalog, placement, and base-level composition of multiple assets. A
compound assembly is one selectable element there because the existing USD
`PhysicsRigidBodyAPI`/`SelectableRoot` projection gives the authored assembly
root ownership of its descendant colliders and selection. The existing
`MobilityRoot` takes precedence for a vehicle. Nested parts remain available by
drilling into the assembly's USD document in Editor; no second group registry,
name convention, or ECS-only grouping state is introduced.

## Ownership and persistence

- USD owns prim identity, hierarchy, references, variants, transforms, ports,
  connections, mount frames, and authored parameters.
- `lunco-canvas` is a reusable view/projector. Modelica and USD connections use
  different projectors over the same canvas substrate.
- `DocumentId` is the identity of an open assembly/USD file. The existing
  `DocumentRegistry<UsdDocument>` owns its source, generation, origin, undo
  stack, and lifecycle; the Assembly editor never creates a parallel registry.
- `DocumentRegistry::fork` creates a new untitled assembly snapshot with a
  fresh `DocumentId`. `UsdDocument::fork` copies the base/runtime authored
  layers and edit history while creating a document-owned composition cache;
  the host copies undo/redo state by value and the registry attaches a new
  recorder to the same Twin journal. Save-As is the first operation that can
  bind the snapshot to a filesystem path.
- `UsdOp` plus `ApplyUsdOp`/`ApplyUsdOps` is the only write path. It supplies journaling,
  inverse operations, save, undo, and live projection.
- The editor operates on an explicitly opened USD document. The existing
  `OpenFile`/`NewDocument`/`SaveAsDocument` commands provide the file lifecycle;
  the Editor preview is opened with `OpenUsdPreview`, which carries an
  explicit `UsdPreviewId`, `DocumentId`, and `LayerId`. `FocusUsdPreview` selects
  the session shown by the dock and `CloseUsdPreview` releases only that session.
  All preview sessions are isolated from the simulation scene.
- A `UsdPreviewSession` owns one projected composed stage, scene root, and
  render layer. A `UsdPreviewView` owns only one camera, light, render target,
  projection mode, orbit pose, and navigation scale over that session.
  `OpenUsdPreviewView` therefore provides
  split or tabbed 3D inspection without duplicate USD projection work.
  Hidden view tabs have inactive cameras; visible tabs publish their own dock
  geometry and are resized independently. `UsdPreviewRenderBudget` bounds each
  target to 2048 px per axis and 4,194,304 pixels, with an 8,388,608-pixel
  frame-wide cap by default. A view is activated only after its panel publishes
  geometry inside that budget; zero-valued limits are invalid and produce no
  render target rather than an unbounded allocation.
  A click on a `.usda`, `.usd`, or `.usdc` file in any open Twin resolves that file at the emitting
  Twin's path, admits it through the existing async `OpenFile` pipeline, then
  opens/focuses the stable editor session; it never becomes `LoadScene`.
- `SelectUsdPrim` is the editor's path-selection command. It requires the
  focused `UsdPreviewId` and resolves the path only against that session's
  stage handle and preview-root hierarchy, so identical paths in the live
  scene or another open document cannot cross the editor boundary.
- Preview navigation is owned by `UsdPreviewView`. Primary-drag orbit,
  middle/secondary-drag pan, and wheel zoom update that view's camera state
  from per-frame pointer deltas. `SetUsdPreviewProjection`,
  `PanUsdPreviewView`, `ZoomUsdPreviewView`, `FrameUsdPreviewView`, and
  `ResetUsdPreviewView` are the same typed boundary for agents and UI
  controls. Perspective and orthographic are presentation modes only; no
  authored USD camera or stage transform is rewritten by navigation. `Frame`
  uses the projected Bevy `Aabb` hierarchy, so it does not duplicate USD
  traversal or invent asset-specific camera poses. `InspectUsdViewport`
  reports the active projection, orbit target, distance, and orthographic
  scale so an agent can correlate a screenshot with the exact view state.
- Native editor view-models are keyed by `UsdPreviewId`. The prim tree,
  connection canvas, parameter/variant/mount views, joint editor, and
  animation editor derive one entry per open session and paint the session
  selected by the focused view. View cameras and render targets are separate
  presentation state, so pan/zoom and image resources cannot leak between
  views or between documents with identical prim paths. The shared ECS
  selection is only the focused-session projection; the editor-owned session
  selection stores canonical prim paths and restores fresh ECS projections when
  focus changes or a document is reprojected.
- ECS entities and view-models are projections. They must not become a second
  source of component topology or authored values.

## Current surfaces

The existing implementation provides the substrate the perspective composes:

| Surface | Responsibility |
|---|---|
| USD prim tree | Browse composed hierarchy and select entity-backed prims |
| Connection canvas | Project typed `inputs:`/`outputs:` and physics joint relationships; write `SetConnection` through `ApplyUsdOp`/`ApplyUsdOps` |
| Inspector | Derive editable fields from the selected prim/components and dispatch typed operations |
| Mount section | Read socket/plug contracts and offer snap or attach actions |
| Behaviour editor | Edit a program's authored source through the scripting command path |
| Component attach | Reference an asset, place it, and author the joint as one typed command |
| Program attach | Discover `.mo`/`.py` sources and lower source, ports, defaults, and wires through `AttachProgram` |

The perspective must remain a composition of these surfaces. The Twin Browser
opens an explicit preview session for the selected document; the USD preview, prim
tree, Connections graph, Inspector, and command handlers consume that same
explicit document binding. The Connections graph and native USD panels are
empty until their focused session has a projected stage; they never infer an
editable stage from entity counts or the live simulation.
Do not add a second graph library, a special rover assembly implementation, or
direct ECS mutation for convenience.

### Snapshot and edit contract

An Assembly editor may start from a reusable USD component or an existing
assembly by forking its current document. The fork is a full authored
base-plus-runtime snapshot with a new identity; equal generations do not imply
shared content. Its derived composition memo is private to the fork and keyed
by the document identity plus both layer revisions. The composed USD stage and
resolver remain owned by the existing USD composition path, so the editor does
not introduce a second scene cache or dependency traversal.

The fork has no file identity until Save-As. Its subsequent operations are
journalled under the new document id, while the existing Twin journal remains
the one cross-document history stream. Per-document undo/redo remains on that
document's `DocumentHost`.

### Command and inspection contract

`ForkDocument`, `CloseDocument`, and `DiscardDocument` are the lifecycle
verbs for an Assembly document. Forking returns the new `DocumentId`; closing
removes it from the registry; discarding an untitled document closes it and
discarding a writable file document reads the file through storage off-thread,
then resets the resident document and its undo history. A file identity or
document revision change during the read is rejected rather than applied to a
different document.

`ApplyUsdOp` and `ApplyUsdOps` accept an optional `parent_gen`. When supplied,
the registry rejects a stale request before authoring or journaling. Successful
acks identify the document, target layer, affected paths, new generation,
canonical journal cursor, and change-set id for compound edits; the API response
is sent after the document mutation has committed. Callers that do not have a
causal predecessor pass `null`; they do not use a second edit path.

`InspectUsdDocument` is the read-only agent/human inspection query. It requires
an explicit `doc` and can accept a USD `path`; it reports the document's
authored layers and revisions, local composed prim topology, dependency arcs
from `lunco-usd-compose`, diagnostics, and journal cursor. It never infers an
active document or constructs another resolver/cache.

`InspectUsdViewport` is the read-only presentation query for the same
headful session. It reports the focused preview/view pair and every explicit
preview lease with its document, edit target, projected generation, and
independent view ids. An agent correlates this typed state with
`CaptureScreenshot` and `view_image` before editing what the user has open;
the tab label is never treated as document identity.

`InspectUsdSelection` is the read-only authoring-context query for one explicit
open preview (or the focused preview when `preview` is omitted). It reports
document/preview/edit-layer identity, selected composed paths, the primary
selection, the Inspector drill target, composed `type_name`/`kind`, parent and
USD assembly paths, and the existing typed command/Rhai operation families.
Multi-selection and no-selection are explicit states; duplicate projected paths
and stale selection entries are reported instead of being resolved by a name or
an arbitrary entity. `assembly_edit::selection_context()` reads the focused
context and `selection_context_for(preview)` reads a hidden open preview
without changing user-visible focus.

The Inspector uses that same lease boundary for numeric transform edits. A
selected entity under the focused preview root is edited as its composed USD
prim's local canonical transform: the controls display metres and Euler XYZ
degrees and submit one `ApplyUsdOps` change set containing the changed existing
`SetTranslate` and/or `SetRotate` operation after a value is committed. The command
uses the preview's explicit document, edit target, and projected generation, so
stale edits are rejected and the USD projector remains the only preview update
owner. Live simulation entities continue to use `MoveEntity`, which owns
BigSpace/physics seating; a preview entity is never sent through that live
identity path.

The standard transform gizmo uses the same unparented proxy frontend for both
owners. At drag start, a focused preview target is identified by its exact
preview root, stage handle, document, edit target, generation, and `UsdPrimPath`;
its local composed Bevy `Transform` becomes the transaction snapshot. During the
drag, the proxy's render pose is converted to parent-local space with Bevy's
`GlobalTransform::reparented_to`. Release authors only the changed local
translation and/or Euler XYZ rotation as one generation-checked `ApplyUsdOps`
change set. Escape restores the local snapshot, while a changed preview
generation or closed session discards the transaction without restoring stale
state. Preview drags never create `RigidBody`, `KinematicDrive`, or a physics
hold. Live targets keep the existing BigSpace/Avian capture, rebranch, restore,
and `TransformEntity` path.

The presentation binding is shared with the standard workbench contracts. A
visible focused preview camera is the sole `GizmoCamera`, and the workbench's
measured `PanelRects` is converted once into the gizmo frontend's logical
`viewport_rect`. The maintained gizmo picking backend consumes that same
rectangle before testing handles, keeping the rendered and interactive
coordinates identical. The singleton and separate preview-tab renderers both
record `SceneTarget::Offscreen` ownership in `ScenePickGate`; this preserves
the global live-scene egui guard while allowing only a handle drag inside the
focused preview surface. No second USD gizmo, cursor transform, or
panel-local input gate is introduced.

`InspectUsdEditSession` is the read-only proposal review query. It requires an
explicit `doc` and returns each typed proposal, its explicit scope, generation
and layer-revision preconditions, affected paths, diagnostics, review state,
and external-file staleness. `CreateUsdProposal`, `ReviewUsdProposal`, and
`CommitUsdProposal` are the corresponding typed plan, review, and commit
commands. The browser exposes the same review actions for humans; agents use
the query and commands through the shared Rhai library.

`ResolveUsdTarget` requires `doc`, a prim `path`, and an explicit `edit_target`
(`@root@` or `@runtime@`). Local authored and runtime opinions are resolved by
`UsdDocument`; referenced or payloaded paths are resolved by the already-mounted
`CanonicalStage`, and the returned OpenUSD prim stack identifies the actual
composed opinions. If an arc has no mounted canonical stage, the query rejects
the request instead of guessing from the flat authored layer. `SyncUsdDocument`
uses the document's bounded typed-op ring: a covered generation returns an
ordered delta, while an expired cursor returns the complete base/runtime layer
snapshot needed to resync. A future cursor is rejected. Neither query creates a
second composition cache, resolver, or history log.

The target response reports `edit_scope` (`authored_layer`, `local_override`,
`composed_read_only`, or `missing`) instead of claiming that every composed
prim supports every operation. Namespace edits still follow the operation's
typed validation; a composed read is not permission to remove or move a
referenced or variant-contained prim.

### Agent and automation surface

The built-in `assembly_edit` Rhai tool library is the shared agent/editor
policy surface for these APIs. It calls `OpenFile`, `NewDocument`,
`ForkDocument`, the document save/close/discard lifecycle verbs, the six
read-only USD queries (including `InspectUsdViewport` and
`InspectUsdSelection`),
`ApplyUsdOp`/`ApplyUsdOps`, `AttachComponent`, `DetachComponent`,
`AttachProgram`, and the generic document undo/redo commands through
`cmd`/`query`; it adds no
second registry, resolver, cache, parser, operation log, or persistence format.
`open(path)`, `new_document()`, and `fork_document(source, name)` acknowledge
the existing asynchronous document lifecycle, so callers obtain the resulting
`DocumentId` from `ListOpenDocuments` rather than guessing one. The
`save_document`, `save_as_document`, `close_document`, and
`discard_document` helpers route to the same explicit lifecycle verbs used by
the human UI. `describe`, `inspect`, `resolve_target`, and `sync_document`
always receive an explicit document. Every authored helper receives an
explicit `@root@` or `@runtime@` target, and `batch` requires each reflected
`UsdOp` to carry its own target.

Structural helpers `add_prim`, `remove_prim`, `move_prim`, `payload`, and
`active` expose the existing typed USD operations for composition changes,
namespace moves, payload arcs, and non-destructive activation. They do not
expose raw source replacement; a caller must inspect the exact path and pass
the returned generation.

The proposal helpers are described in the proposal review contract below;
they are the only review path exposed by this library. The optional
`parent_gen` on `add_prim`, `remove_prim`, `move_prim`, `transform`,
`attribute`, `keyframe`, `remove_keyframe`, `relationship`, `connection`,
`schema`, `variant`, `payload`, `active`, and
`batch` is the existing
revision precondition. A supplied cursor makes stale edits fail atomically
before authoring or journaling; `()` is reserved for an operation with no
causal predecessor. `transform` uses one `ApplyUsdOps` change set for its
translation/rotation pair. Attach and detach helpers pass the complete typed
specification to the existing mount/socket/joint validators. `assembly_edit::attach_program`
passes the complete `ProgramAttachSpec` contract to the existing
`AttachProgram` lowering. The library is
listed by `ListToolLibraries` and completion, and its source is hot-reloadable
through the standard tool-library loader. It intentionally exposes no direct
USDA writer, runtime-only setter, guessed target, or unowned preview operation.

The companion `assembly_ui` tool library is presentation policy over the same
typed boundary. Its templates describe the registered `editor` perspective,
Twin Browser, USD prim tree, USD viewport, Connections canvas, Inspector, and
Environment panel ids, plus the existing animation, mount, review, and
persistence sections/workflows. `open_session` takes an explicit
`UsdPreviewId`/`DocumentId`/edit target, then uses `ActivatePerspective`,
`OpenUsdPreview`, and `FocusPanel`; opening the preview already selects its
session lease. It does not create a second layout, document binding, selection
store, or editor state. Mount and review remain Inspector sections, animation
remains an Environment section, and persistence remains the existing lifecycle
commands and File menu rather than a fabricated panel. Agent workflows
therefore use the same presentation and authoring surfaces as a human without
introducing another UI runtime.

`assembly_ui::panel_templates(preview, doc, edit_target)` returns nine
descriptors. Each contains the explicit session handles and a capability list;
section descriptors identify their owning panel, while the persistence
descriptor contains no panel id because save, Save-As, discard, and close are
document commands. `open_mount`, `open_review`, and `open_animation` focus the
owning existing panel. `assembly_ui::open_session` returns these descriptors as
`templates` so an agent can choose the next human-visible surface without
guessing panel ownership.

For a lander edit, open the lunar-base Twin, take the USD `DocumentId` from
`ListOpenDocuments`, inspect the composed lander and resolve its legal authored
target, then open one explicit preview:

```rhai
let before = assembly_edit::describe(doc);
let target = assembly_edit::resolve_target(doc, "/Lander", "@root@");
let ui = assembly_ui::open_session(preview, doc, "@root@");
let plan = assembly_edit::propose(
    doc,
    "Assembly",
    "Adjust lander pose",
    inspected_typed_ops,
    before.generation,
);
```

Here `inspected_typed_ops` is the exact `UsdOp` plan built from composed
inspection. Review it in the Inspector, commit it only after the visible
checkpoint, then inspect the affected composed paths using the returned
generation. If the edit is a mount, submit the reflected `AttachSpec` to
`assembly_edit::attach_component` instead; it must contain the inspected
component, socket, plug, joint, frames, and explicit ownership paths. The
preview handle and document handle stay explicit throughout; the example does
not infer a lander from a name or entity count.

For a multi-asset assembly, open each source document independently and use
one preview lease per document. A single document change that moves several
known prims is one typed change set with the generation read immediately
before authoring:

```rhai
let first = assembly_edit::describe(lander_doc);
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
    first.generation,
);
let payload_unchanged = assembly_edit::sync_document(payload_doc, payload_before.generation);
```

The two translated paths are inspected paths owned by `lander_doc`;
`payload_doc` remains an independent preview/document session and is not
silently merged into the lander. For a real mount, use `AttachComponent` with
the reflected explicit spec so USD authors the reference, placement, joint,
frames, and occupancy as one validated operation. If either document changes
between inspection and the edit, the generation precondition rejects the
stale request and the workflow must inspect again.

`keyframe` and `remove_keyframe` expose the existing reversible
`SetTimeSample`/`RemoveTimeSample` primitives to agents. The Editor Inspector
uses those same primitives for a selected prim's current pose, while the
Environment panel's existing `ControlAnimation` transport plays and scrubs the
result. The document authoring owner adds a missing xform channel to
`xformOpOrder` as part of the first keyframe, so a keyed pose is a valid USD
transform rather than an unattached attribute.

### Proposal review and conflict contract

`CreateUsdProposal` is the safe plan boundary for human and agent tooling. It
requires an explicit `UsdEditScope` (`SourceAsset`, `Assembly`, or
`InstanceOverride`) and the document generation the author inspected. The USD
owner validates every typed operation against a cloned `UsdDocument`, checks
the scope against authored composition arcs, and stores only the typed plan in
the session review resource. No authored layer, preview stage, journal entry,
or undo group changes during proposal creation or review.

`InspectUsdEditSession` exposes the proposal's typed operations, affected paths,
layer revision, origin identity, validation diagnostics, and review state.
`ReviewUsdProposal` mutates only that state: mute, unmute, or reject. Commit is
the explicit merge boundary. `CommitUsdProposal` rechecks the parent
generation, root-layer revision, document origin, external file watermark,
scope, and typed validation, then calls the existing grouped USD operation
path. The accepted plan therefore becomes one ordinary journal change set and
one document undo unit. A mismatch leaves the plan in `Conflict`; there is no
automatic rebase, source overwrite, or second history stream. Closing or
discarding the document clears its pending review plans.

The browser row reflects modified, review, muted, and conflict state from this
document-scoped resource. `SaveDocument`/`SaveAsDocument` remain explicit
persistence commands; proposal commit never writes a file implicitly.

### Interactive headful authoring

The agent-and-human authoring workflow is defined by the
[`edit-usd-assembly` skill](../../skills/edit-usd-assembly/SKILL.md). Assembly
edits start in a windowed production `target/debug/luncosim --api PORT`
session; `--offscreen` and `--no-ui` are not substitutes for this workflow.
The user sees the focused `UsdPreviewViewId` and its parent `UsdPreviewId`
while the agent uses the same typed
commands or the built-in `assembly_edit` Rhai library. After each coherent
change, the agent checks the command acknowledgement and generation, reads the
affected composed prim, captures and inspects a screenshot, and presents the
result for user feedback before another material edit or final save. A missing
display is an explicit workflow blocker, not a reason to bypass the visible
preview.

Interactive authoring does not permit direct USDA writes, ECS mutation, guessed
document/layer/prim identities, or an assembly-specific state path. The existing
document registry, OpenUSD composition, typed `UsdOp` journal, preview session,
mount validators, and explicit `SaveDocument`/`SaveAsDocument` commands remain
the only owners.

The production acceptance fixture is
`assets/scenes/tests/assembly_editor_proposal.usda`; its Rhai observer drives
the same proposal/query/commit surface and verifies non-mutating review,
journal undo/redo, and stale-conflict rejection. The existing
`assembly_workflow` fixture remains the catalog attach/save/reload regression.

## Mount and attach contract

A reusable component declares a plug frame. A host applies
`LunCoMountHostAPI` and explicitly lists its socket prims in
`lunco:mount:sockets`; socket paths and attachment-joint paths are relationship
data, not naming conventions. Detection is by the applied LunCo mount schemas,
not by loose attribute names or a required grouping prim.

```usda
def Xform "Base" (
    prepend apiSchemas = ["LunCoMountHostAPI"]
)
{
    rel lunco:mount:sockets = [</Base/Interfaces/wheel_fl>]
    def Xform "Interfaces" {
        def Xform "wheel_fl" (
            prepend apiSchemas = ["LunCoMountSocketAPI"]
        )
        {
            uniform token lunco:mount:socket = "wheel"
            uniform token lunco:mount:joint  = "revolute"
            token lunco:mount:axis = "X"
            double3 xformOp:translate = (1.2, -0.3, 0.9)
            uniform token[] xformOpOrder = ["xformOp:translate"]
        }
    }
}

def Xform "Wheel" (
    prepend apiSchemas = ["LunCoMountPlugAPI"]
)
{
    uniform token lunco:mount:plug = "wheel"
    rel lunco:mount:frame = </Wheel/Interfaces/hub>
}
```

The attach flow is:

1. Read the composed socket or the component asset's `defaultPrim` plug.
2. Resolve the plug-to-socket placement and rotation in the host's local frame.
3. Lower `AttachComponent` to `AddPrim`/reference, transforms, the typed joint,
   and authored joint relationships. The author supplies an explicit joint leaf;
   the lowering records it on the child through
   `lunco:mount:attachmentJoint`.
4. Apply the complete operation set through the USD command's one journal change
   set (`apply_ops_as_change_set`).

Program attachment follows the same author-once rule. `AttachProgram` validates a
complete `ProgramAttachSpec` and lowers it to one USD change set. An empty port
contract is source-only; it is not silently treated as a running cosim model.
The author must add explicit scalar ports and connections before expecting live
value exchange. The palette, `assembly_edit` tool, HTTP API, and Assembly editor all call
this same command.

`AttachSpec::from_mount` and `resolve_mount_placement` own the frame math;
`attach_component_ops` owns the USD lowering. `AttachSpec` has no independent
joint-anchor input: the lowering always derives `physics:localPos0` from the
single placement and authors `physics:localPos1` at the part origin. Direct and
socket attaches therefore share one author-once rule; the editor must never
copy a transform number into a second joint field.

The same frame contract supports retrofit snap for an already referenced part:
the inspector re-authors its transform and joint anchor without re-referencing
the asset. The command owner validates the host body, socket schema, accepted
kind, joint/axis contract, relationship, asset plug, and occupancy; invalid
frames fail closed in the frame resolver and are reported to the author. Socket
occupancy is authored as `lunco:mount:part` in the same attach change set, and a
stale request is rejected before any child is lowered.

`DetachComponent` is the inverse assembly action. Rhai or the editor submits an
explicit `component_path`, `joint_path`, and, for socket attachments,
`socket_path`. Rust verifies the recorded relationships, layer ownership, and
incoming relationship blockers, then lowers socket clearing plus joint and
component-subtree removal through the same one-change-set boundary. It never
guesses a joint from a component name and never silently deletes external
Modelica/electrical/data links. Undo restores the complete topology.

## Physics invariants

- A movable mounted part needs a rigid body and a joint. A nested visual or
  collider must not silently become an unconnected body.
- `UsdPhysics` owns bodies, joints, frames, limits, and drives. The Assembly editor only
  authors those facts; Avian is the runtime projection.
- A component may be reusable only when its default prim, units, mount contract,
  and physical ownership are explicit.
- A parser pass is not an acceptance test. New attach or mount behavior needs
  composition tests, op-lowering tests, and a production scene run.

## Remaining design work

1. Keep canvas layout as an authored decision only if layout persistence is
   required; otherwise keep it UI-local. Do not journal every drag frame by
   default.
2. Keep per-document `DocumentHost` undo authoritative for `Ctrl+Z`. Reserve
   the twin journal's broader undo manager for a separate future twin-wide or
   cross-author command; never wire both to one undo verb.
3. Reduced-coordinate articulation and large assemblies need their own physics
   contract. Mount frames make an assembly authorable; they do not replace a
   solver model for a coupled articulated system.
4. Validate the complete user path against the production binary: open a Twin,
   attach or snap a component, attach a source-backed program, inspect the
   composed joint and ports, save, reload, and confirm the body remains stable.

See [`clean-architecture-and-usd-standards.md`](clean-architecture-and-usd-standards.md)
for the standard-schema gate and
[`50-usd-driven-visuals.md`](50-usd-driven-visuals.md) for reusable visual
components.
