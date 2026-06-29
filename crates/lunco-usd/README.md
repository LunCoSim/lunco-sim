# lunco-usd

The **high-level orchestrator** for LunCoSim's USD (Universal Scene Description)
system. It loads rover/scene definitions from USD files and maps them to Bevy
entities with Avian3D physics and LunCoSim simulation components.

## `UsdPlugins`

A convenience bundle (`app.add_plugins(UsdPlugins)`) that wires the real,
existing subsystems:

- **`UsdBevyPlugin`** (from `lunco-usd-bevy`) — visual sync: spawns child
  entities for USD prims, attaches meshes + transforms + hierarchy.
- **`UsdAvianPlugin`** (from `lunco-usd-avian`) — physics mapping: USD physics
  attributes → Avian3D `RigidBody` / `Collider` / `Mass` / `Damping`.
- **`UsdSimPlugin`** (from `lunco-usd-sim`) — simulation mapping: detects sim
  schemas and creates `WheelRaycast` / FSW / `DifferentialDrive` components.
- **`UsdCommandsPlugin`** (this crate, `commands` module) — the **headless-safe**
  document/file verb layer: `ApplyUsdOp`, `OpenFile` / `NewDocument` /
  `SaveDocument` observers, the async load pipeline, and the twin-scene
  resolver. Added unconditionally so server / sandbox / networking bins get the
  full USD document surface (egui-free).

> There is **no `UsdLunCoPlugin`** — that was an old doc artifact.

## UI plugins (`ui` feature only)

Behind the `ui` feature the `ui` module adds the egui browser/viewport panels,
added separately by app composition (not by `UsdPlugins`):

- **`UsdUiPlugin`** — Twin browser / loaded-stages / dispatch panels.
- **`UsdViewportPlugin`** — `UsdViewportPanel`, the 3D scene of the active USD
  document rendered into the dock.

## Document model

The egui-free USD document model lives in `document` (`UsdDocument`, `UsdOp`,
`UsdChange`, `LayerId`) + `registry` (`UsdDocumentRegistry`). Edits author
through openusd's `Stage` by SDF path (`lunco_usd_bevy::author`) — the old
byte-splicing text editor is gone.

## Engineering metadata

LunCoSim enriches standard-compliant USD with simulation-only metadata in the
`lunco:*` namespace (Ephemeris IDs, sensor hit radii, telemetry port mappings)
that standard OpenUSD schemas don't define — without polluting standard visual
or physics schemas.

See [docs/architecture/21-domain-usd.md](../../docs/architecture/21-domain-usd.md)
for the full architecture.
