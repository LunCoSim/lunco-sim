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
  schemas and creates `WheelRaycast`, FSW, and data-selected `DriveMix` control
  surfaces.
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
- **`UsdViewportPlugin`** — `UsdViewportPanel`, the focused isolated USD
  preview lease rendered into the dock. `OpenUsdPreview`, `FocusUsdPreview`,
  and `CloseUsdPreview` own preview lifecycle; other leases remain projected
  while the dock displays one focus.

Native Assembly Editor panels key their derived state by `UsdPreviewId`. Each
open lease retains its own prim tree, connection canvas, parameter/variant/
mount view, joint and animation view, authored layer, and projection
generation; the dock paints only the focused entry. The shared ECS selection is
the focused-lease projection and is restored from the editor session selection
when focus changes. Panel edits resolve the lease's explicit `DocumentId`,
`LayerId`, and generation before dispatching typed USD commands.

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
