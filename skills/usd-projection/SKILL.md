---
name: usd-projection
description: >
  Extend or diagnose LunCoSim's USD-to-ECS projection: add a supported prim or
  attribute, trace an ignored field, or fix edits that fail to persist, undo,
  replicate, or render. Use for `lunco-usd*` machinery and document-owned ECS
  state. Prefer this skill for projection internals; use build-usd-scene for
  scene authoring and luncosim-architecture for cross-domain ownership.
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
          project_stage_changes            (lunco-usd/src/live_consume.rs)
              ├── resynced   → structural: spawn / despawn prims
              └── info_only  → attribute-only: translate, rotate, domes …
              │
              ▼
          UsdVisualProjectionQueued  (bounded ECS binding queue)
              │
              ▼
          instantiate_usd_prim_from_reader (lunco-usd-bevy/src/lib.rs)
              └── match reader.type_name(path) → components
```

The asset loader composes the fetched layer closure and snapshots the complete
default-time `UsdRead` surface into `UsdStageProjectionPlan` on its worker. The
`Add, UsdPrimPath` observer and `sync_usd_visuals` both feed the same
`UsdVisualProjectionQueued` marker. `process_queued_usd_visuals` drains that
queue with its configured per-frame budget, but the extractor reads only the
owned plan during initial materialisation; it does not parse USD, walk a live
stage, or resolve composed bindings on the UI thread. CPU geometry uses the
existing async compute path and only Bevy asset insertion remains on the main
thread. After the initial asset generation, explicit live edits use the
canonical `StageView` and the same extractor contract.

Doc-backed Twin admission is also asset-event driven. `UsdSourceText` is loaded
through the registered source scheme; `AssetEvent` and
`AssetLoadFailedEvent` advance or fail the pending document transaction. The
composed document overlay is published before `LoadScene` is submitted.
Referenced stage closures follow the same event boundary before a reference is
authored onto the live stage. The instance carries a path-remapped copy of the
prepared source plan and its root identity; descendants reuse both through the
same queue. The entity reader invalidates that prepared source when the
canonical stage generation changes, so authored overrides are always read from
the live composed stage. Do not add frame-count timeouts, per-frame load polls,
or direct filesystem reads to this path. After admission,
`DocumentChanged` and stage-asset lifecycle events wake the single
`sync_twin_overlays` owner; do not add a per-frame generation scan or a
viewport-specific edit/reload path.

The Editor is document-scoped. `DocumentId` from the existing
`DocumentRegistry<UsdDocument>` identifies the file being edited; the Twin
Browser opens the explicit `OpenUsdPreview { preview, doc, edit_target }`
lease and can later use `FocusUsdPreview` or `CloseUsdPreview`. The isolated
preview, prim tree, Inspector, and USD commands consume that same document
binding.
Never choose an editor stage by entity count, insertion order, or the current
simulation viewport, and never use an active-viewport fallback for an entity
that lacks an explicit document binding.

For agent or editor synchronization, call `SyncUsdDocument` with the explicit
document generation. Use its typed delta while the cursor is covered; consume
the returned base/runtime layer snapshot when it reports an expired history
window. Reject future cursors. To edit a composed path, call
`ResolveUsdTarget` with the explicit document, prim path, and `@root@` or
`@runtime@` target. Referenced and payloaded paths are valid only when the
existing `CanonicalStage` is mounted; use its `edit_scope` and typed-operation
validation rather than treating a composed read as permission to move or
remove a referenced/variant prim. Do not inspect flat layer data as a
replacement for OpenUSD PCP resolution.

Agents and editor automation should use the built-in `assembly_edit` Rhai
library. It is a thin wrapper over `OpenFile`, `InspectUsdDocument`,
`ResolveUsdTarget`, `SyncUsdDocument`, `ApplyUsdOp`/`ApplyUsdOps` (including
`SetTimeSample` and `RemoveTimeSample` for keyframes),
`AttachComponent`, `DetachComponent`, `UndoDocument`/`RedoDocument`,
`CreateUsdProposal`, `InspectUsdEditSession`, `ReviewUsdProposal`, and
`CommitUsdProposal`; it does not create another document registry, resolver,
USDA writer, or operation log. `open`
returns the normal asynchronous command acknowledgement and callers discover
the resulting id through `ListOpenDocuments`. Read helpers require an explicit
`doc`; authored helpers require `doc`, an edit target, and a USD path. Their
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
a synthesizer, then use Bevy identity change detection to revisit only changed
prim entities. Reserve the full root pass for a USD wiring or member-source
invalidation. Do not add a second stage scan or a name-based candidate list.
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

An edit that does not lower to a `UsdOp` is absent from **save, journal, undo,
and network replication**. Route every editor and runtime mutation through the
same projection boundary.

```rust
commands.trigger(ApplyUsdOp { doc, parent_gen: None, op });      // one op
apply_ops_as_change_set(world, doc, "Edit material", ops);       // N ops, ONE undo unit
```

Prefer `apply_ops_as_change_set` whenever an intent lowers to more than one op —
a loop of `ApplyUsdOp` journals N independent entries, and undo then peels off
one and leaves the object half-edited.

Writing an ECS component directly is legitimate **only** for state that is
genuinely not part of the document (a camera's current yaw, a hover highlight).
If a user would expect it to survive save-and-reload, it belongs in USD.

`AttachProgram { doc, spec }` is the canonical multi-op authoring intent for a
source-backed Modelica, Python, Rhai, or behaviour-tree program. It lowers the
complete `LunCoProgramAPI` child, source asset, scalar ports, defaults, and
connections through this same change-set path. A palette or script must call
that command; it must not create an ECS marker or write a parallel registry.
An empty contract is source-only and remains visibly distinct from a running
cosim participant.

## Law 2 — ask the scene root, never guess

To author a new top-level prim you need the target document *and* the parent
path. Both come from the scene root:

```rust
roots: Query<&UsdPrimPath, With<lunco_usd_bevy::UsdSceneRoot>>
let doc = scene_document_for(&backed, &asset_server, root.stage_handle.id())?;
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
  `lunco_usd::material::ensure_preview_surface_ops()` — it builds the
  Material+Shader+binding for you, and it is deliberately in `lunco-usd` so
  every crate authors materials the same way.
- `primvars:displayColor` / `displayOpacity` are the *only* Gprim display
  attributes. There is no "display emissive" — **emission requires a material**.
- Genuinely new concepts get the `lunco:` vendor namespace (`lunco:dome:skybox`,
  `lunco:surface:skybox`, `lunco:terrain:*`). That is the correct, spec-sanctioned way to extend USD. What
  is *not* correct is inventing a second spelling for something USD already has.

The procedural camera-background contract is an `Xform` with
`lunco:surface:skybox = true` and a standard `UsdShade` material binding. Read
that intent once in `lunco-usd-bevy` and stamp the existing render-free
`ProceduralSkybox` component. Do not project a `UsdGeomGprim` for the
background, carry `info:wgsl:vertexAsset`, or read the USD flag again in a
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
   (`lunco-usd-bevy/src/read.rs`), implemented by both `StageView` (the live
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
   as `info_only`, and `project_stage_changes` only handles the cases it knows —
   translate, rotate, dome lights. If you add an editable attribute and skip
   this, `SetFoo` will journal and save correctly and **nothing will move on
   screen** until reload. Add a handler in `live_consume.rs`.
5. **Author it.** Add a command that lowers to `UsdOp`s (Law 1) and register it
   with `register_commands!` — a command is only reachable from the HTTP API /
   MCP / rhai if its *type* is in the reflect registry.
6. **Test it.** Because extractors use the shared composed-reader contract,
   unit-test the prepared `UsdStageProjectionPlan` for initial-load behavior
   and use a live `StageView` only for explicit authored-edit behavior — no App,
   no renderer.

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
