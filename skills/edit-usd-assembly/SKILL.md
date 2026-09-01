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

## Open the exact document and preview

The document registry and OpenUSD composition system are authoritative. Never
guess a document id, preview id, prim path, or edit layer from a name or from
the active simulation viewport.

1. Open a Twin folder with `OpenTwin` (its path contains `twin.toml`) or open a
   source with `assembly_edit::open(path)`. `OpenFile` is the normal async
   document lifecycle; `LoadScene` is only for mounting a scene.
2. Query `ListOpenDocuments` and select the returned `DocumentId`.
3. Use `assembly_edit::describe(doc)`, `inspect(doc, path)`, and
   `resolve_target(doc, path, edit_target)` to read composed topology, layer
   ownership, generation, and the legal target. Use `sync_document` when an
   already-known generation can be advanced by a typed delta.
4. Activate the `editor` perspective and open an isolated preview with
   `OpenUsdPreview { preview, doc, edit_target }`; use
   `FocusUsdPreview { preview }` when changing the visible document. Every
   panel and selection must remain bound to that `UsdPreviewId`.

The built-in wrappers are in
[`assembly_edit.rhai`](../../assets/scripting/tools/assembly_edit.rhai). The
preview helpers are `preview_open`, `preview_focus`, and `preview_close`.
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
- `attach_component` and `detach_component` use the existing mount, socket,
  joint, frame, ownership, and occupancy validators. Supply exact paths in the
  reflected `AttachSpec`/`DetachSpec`; never identify a part by a name prefix
  such as `Wheel_`.
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

## Save, verify, and close

Do not save automatically as part of proposal commit. After the user approves
the visible result, dispatch `SaveDocument` for a file-backed document or
`SaveAsDocument` for a fork, then confirm the document is no longer dirty with
`ListOpenDocuments`/`InspectUsdDocument`. Keep undo/redo available through
`UndoDocument` and `RedoDocument` during feedback.

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
- The focused `UsdPreviewId` owns transient view, selection, and panel state;
  it is not authored into USD.
- Use standard USD schemas whenever they own the concept, and existing
  `lunco-usd-compose`, journal, mount, and transform-frame owners whenever they
  already implement it.
- No name-based discovery, compatibility alias, legacy path, fallback layer,
  direct file mutation, or second history/state mechanism.
