# lunco-usd

The **high-level orchestrator** for LunCoSim's USD (Universal Scene Description)
system. It loads rover/scene definitions from USD files and maps them to Bevy
entities with Avian3D physics and LunCoSim simulation components.

## Assembly boundary

`lunco-usd-compose` is the render-free composition leaf. It asks
`lunco-assets` for canonical asset identities and bytes, then lets OpenUSD
assemble sublayers, references, payloads, and variants into an inert stage.
`lunco-usd` re-exports that entrypoint for top-level applications.

Composition does **not** start Modelica, Rhai, behavior trees, physics, or
rendering. Those are independent projections after a stage is available.

## `UsdPlugins`

A convenience bundle (`app.add_plugins(UsdPlugins)`) that wires the real,
existing subsystems:

- **`UsdBevyPlugin`** (from `lunco-usd-bevy`) — visual sync: spawns child
  entities for USD prims, attaches meshes + transforms + hierarchy.
- **`UsdAvianPlugin`** (from `lunco-usd-avian`) — physics mapping: USD physics
  attributes → Avian3D `RigidBody` / `Collider` / `Mass` / `Damping`.
- **`UsdSimPlugin`** (from `lunco-usd-sim`) — simulation mapping: detects sim
  schemas and creates `WheelRaycast`, FSW, and generic authored port bindings.
- **`UsdCommandsPlugin`** (this crate, `commands` module) — the **headless-safe**
  document/file verb layer: `ApplyUsdOp`, `OpenFile` / `NewDocument` /
  `SaveDocument` observers, the async load pipeline, and the twin-scene
  resolver. Added unconditionally so server / sandbox / networking bins get the
  full USD document surface (egui-free).

Assembly agents and editor view models use the registered read queries
`InspectUsdDocument`, `InspectUsdEditSession`, `ResolveUsdTarget`, and
`SyncUsdDocument`. They operate on the same document registry, typed op ring,
journal, and mounted canonical OpenUSD stage as the command path; they do not
maintain a second resolver, document store, or history log.

The UI-only `InspectUsdViewport` query belongs to `UsdViewportPlugin`, not the
headless document layer. It reports the focused `UsdPreviewId`/view pair and
all open preview and view handles, including each session's explicit document,
edit target, projection generation, and `projection_ready` state. Agents use
it with `CaptureScreenshot` to identify exactly what is visible and wait for
the explicit ready state before submitting a typed edit; tab labels and
filesystem names are presentation text, not identity.

`CreateUsdProposal` validates an explicit typed `UsdOp` plan against a cloned
document and keeps it outside authored USD for review. Its
`SourceAsset`/`Assembly`/`InstanceOverride` scope and required parent
generation are part of the proposal contract. `ReviewUsdProposal` can mute,
unmute, or reject the plan without changing the document. `CommitUsdProposal`
rechecks the document generation, authored-layer revision, origin, file
watermark, scope, and operation validation before entering the existing
grouped journal/undo path. A stale plan becomes an explicit conflict; it is
never rebased or silently overwritten. Save/Save-As remains a separate
explicit document operation.

## UI plugins (`ui` feature only)

Behind the `ui` feature the `ui` module adds the egui browser/viewport panels,
added separately by app composition (not by `UsdPlugins`):

- **`UsdUiPlugin`** — Twin browser / loaded-stages / dispatch panels.
- **`UsdViewportPlugin`** — `UsdViewportPanel` plus the instance-backed
  `UsdPreviewViewPanel`. `OpenUsdPreview` owns one projected USD session;
  `OpenUsdPreviewView` adds an independent camera/render target over that
  session for a dock tab or split. `FocusUsdPreviewView` and
  `CloseUsdPreviewView` address the exact presentation view; hidden views do
  not render. Visible targets use `UsdPreviewRenderBudget`: 2048 px per axis,
  4,194,304 pixels per view, and 8,388,608 visible pixels per frame by default.

Each `UsdPreviewView` owns CAD-style presentation navigation: primary/left-drag
pan, secondary/right-drag orbit, middle-drag pan, wheel zoom,
perspective/orthographic mode, fit-to-bounds, and reset. The typed commands
`SetUsdPreviewProjection`, `PanUsdPreviewView`, `ZoomUsdPreviewView`,
`FrameUsdPreviewView`, and `ResetUsdPreviewView` are shared by the toolbar and
agent/Rhai automation. They operate on projected Bevy bounds and camera state;
they never author USD camera or transform opinions.

`ExplodeUsdPreview` is the shared preview-only assembly inspection command.
It requires explicit preview/document/assembly/part identities and supports
`Enable`, `Update`, and `Reset`. It captures original local transforms in the
session, computes stable hierarchy-aware offsets in the assembly frame, and
returns structured offset data without changing authored USD, journal state,
save state, physics, or simulation. Reprojection restores the captured
baseline; all views over one session see the same transient pose.

Clicking a USD file in any open Twin's Files section resolves that Twin's
absolute path, opens the file through `OpenFile` and the async document
registry, then binds the admitted `DocumentId` to `EDITOR_PREVIEW_ID`. This is
document inspection only; it never replaces the running scene. Repeated clicks
reuse the existing file identity and preview lease, while pending reads owned
by a closed Twin are cancelled.

Native Assembly Editor panels key their derived state by `UsdPreviewId`. Each
open session retains its own prim tree, connection canvas, parameter/variant/
mount view, joint and animation view, authored layer, and projection
generation; the dock paints the session selected by the focused view. The
shared ECS selection is the focused-session projection and is restored from
the editor session's canonical prim-path selection when focus changes. Panel edits resolve the
session's explicit `DocumentId`, `LayerId`, and generation before dispatching
typed USD commands.

The editor selection is exposed separately by the UI-owned
`InspectUsdSelection` query. It reports canonical `DocumentId`/`UsdPreviewId`/
prim-path identity, composed type and kind, parent/assembly paths, the primary
and Inspector-target paths, explicit multi/no-selection state, and the existing
typed operation families. Its session cache stores paths rather than Bevy
entities, so a USD reprojection can resolve fresh runtime projections and drop
deleted paths. Use `assembly_edit::selection_context()` for the focused preview
or `selection_context_for(preview)` for a hidden open preview.

## Document model

The egui-free USD document model lives in `document` (`UsdDocument`, `UsdOp`,
`UsdChange`, `LayerId`) + the shared `DocumentRegistry<UsdDocument>`. Edits
author through OpenUSD's `Stage` by SDF path (`lunco_usd_bevy::author`).

## Engineering metadata

LunCoSim enriches standard-compliant USD with simulation-only metadata in the
`lunco:*` namespace (Ephemeris IDs, sensor hit radii, telemetry port mappings)
that standard OpenUSD schemas don't define — without polluting standard visual
or physics schemas. Concrete `lunco:` attribute → Bevy component mappings:

*   `lunco:ephemeris_id` -> `Spacecraft::ephemeris_id`
*   `lunco:hit_radius_m` -> `Spacecraft::hit_radius_m`

Display names use the standard USD prim `displayName` metadata, the field
usdview and Omniverse read for friendly prim labels. `doc` is for documentation
prose, not short labels.

See [docs/architecture/21-domain-usd.md](../../docs/architecture/21-domain-usd.md)
for the full architecture.
