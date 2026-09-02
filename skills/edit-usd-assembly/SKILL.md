---
name: edit-usd-assembly
description: >
  Create or modify a reusable LunCoSim USD assembly such as a rover, lander,
  payload, or sensor mount through the live headful Assembly Editor. Use when
  the user must see each change in the running window, give feedback between
  edits, and the agent must inspect the result with screenshots. Use the
  existing `assembly_edit` Rhai tools and typed USD commands; do not edit USDA
  text or ECS state directly.
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
  -d '{"type":"ExecuteCommand","command":"OpenTwin","params":{"path":"/home/rod/Documents/models/lunar-base-model"}}'
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
3. Query `assembly_edit::viewport()` and capture a screenshot when the user
   asks what is visible. Correlate the focused preview/view and visible tabs
   with `ListOpenDocuments`; use the returned explicit handles, never a title.
4. Use `assembly_edit::describe(doc)`, `inspect(doc, path)`, and
   `resolve_target(doc, path, edit_target)` to read composed topology, layer
   ownership, generation, and the legal target. Use `sync_document` when an
   already-known generation can be advanced by a typed delta.
5. Select the `Editor` perspective from the title-bar switcher, or activate it
   through the typed `ActivatePerspective { id: "editor" }` command, then open
   an isolated preview with `OpenUsdPreview { preview, doc, edit_target }`; use
   `FocusUsdPreview { preview }` when changing the visible document. Every
   panel and selection must remain bound to that `UsdPreviewId`.
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

   Navigate the focused view with primary-drag orbit, middle/secondary-drag
   pan, and wheel zoom. The toolbar and agents use the same typed commands:
   `SetUsdPreviewProjection { view, projection: "perspective"|"orthographic" }`,
   `PanUsdPreviewView { view, delta: [x, y] }`,
   `ZoomUsdPreviewView { view, factor }`, `FrameUsdPreviewView { view }`, and
   `ResetUsdPreviewView { view }`. These are view presentation operations; they
   do not author USD camera or transform values. `Frame` uses the projected
   visual bounds, and `InspectUsdViewport` reports projection, target, distance,
   and orthographic scale for screenshot correlation.

The built-in wrappers are in
[`assembly_edit.rhai`](../../assets/scripting/tools/assembly_edit.rhai). The
preview helpers are `preview_open`, `preview_view_open`,
`preview_view_focus`, `preview_view_projection`, `preview_view_frame`,
`preview_view_reset`, `preview_view_pan`, `preview_view_zoom`,
`preview_view_close`, `preview_focus`, and `preview_close`. Use
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
For numeric part transforms, use the focused preview's Inspector Transform
section or call the existing `assembly_edit::transform(doc, edit_target, path,
translation, rotation, parent_gen)` helper with the exact selection context.
Preview values are local canonical metres and Euler XYZ degrees; the Inspector
commits changed translation and/or rotation fields as one journaled
`ApplyUsdOps` edit.
The standard transform gizmo is also available for the focused preview: select
the exact prim, drag its unparented proxy, and release to commit the changed
local translation and/or Euler XYZ rotation as one generation-checked
`ApplyUsdOps` change set. Preview gizmo drags use Bevy's parent-local
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
The presentation helpers are in
[`assembly_ui.rhai`](../../assets/scripting/tools/assembly_ui.rhai): use
`panel_templates(preview, doc, edit_target)` to discover the nine existing
Editor surfaces/workflows and their explicit handles. `open_session(preview,
doc, edit_target)` activates the Editor, opens/focuses the explicit preview,
and foregrounds its viewport. Use `focus`, `open_structure`, `open_inspector`,
`open_connections`, `open_animation`, `open_mount`, and `open_review` to focus
the owning registered panels. Animation is an Environment section and mount
and review are Inspector sections; persistence is the existing document
lifecycle command group and has no fabricated panel. These helpers only
dispatch existing `ActivatePerspective`/`FocusPanel` commands; they do not own
layout or create parallel document/view state.
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
- `add_prim`, `remove_prim`, `move_prim`, `payload`, and `active` expose the
  existing typed structural USD operations. They require an explicit target,
  exact paths, and the inspected generation; they never replace a layer's raw
  source.
- `assembly_edit::attach_component` and `assembly_edit::detach_component` use
  the existing mount, socket, joint, frame, ownership, and occupancy
  validators. Supply exact paths in the
  reflected `AttachSpec`/`DetachSpec`; never identify a part by a name prefix
  such as `Wheel_`.
- `assembly_edit::attach_program(doc, spec)` dispatches the existing typed
  `AttachProgram` contract. Build its `inputs` and `outputs` with the
  namespaced helpers `assembly_edit::program_input_connection`,
  `assembly_edit::program_input_default`, and `assembly_edit::program_output`;
  the source asset, host path, program name, edit target, and port paths remain
  explicit in `spec`.
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

## Concrete lander workflow

For the local lunar-base Twin:

```text
Twin root: /home/rod/Documents/models/lunar-base-model
Twin:      astrobotic-griffin-1
Scene:     twin://astrobotic-griffin-1/scenes/griffin_1_surface_ops.usda
Wrapper:   twin://astrobotic-griffin-1/vehicles/griffin_1.usda
Source:    lunco://vessels/landers/descent_lander.usda
```

Open the Twin, inspect `Lander` in the mission composition, and decide whether
the requested change belongs in the Twin wrapper or in the reusable source
asset. For a new payload, inspect the host's composed mount sockets and the
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
