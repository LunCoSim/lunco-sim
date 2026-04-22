# 21 — USD Domain

> **Stub.** Pending content migration from legacy `docs/USD_SYSTEM.md` and
> consolidation with the Document System design.

## Scope

USD (Pixar Universal Scene Description) is the scene-graph and asset format
LunCoSim uses for the 3D world. Bases, rovers, habitats, terrain — everything
physical — lives as USD prims in USD stages. See
[`../../crates/lunco-usd/`](../../crates/lunco-usd/) and companion crates
`lunco-usd-avian`, `lunco-usd-bevy`, `lunco-usd-composer`, `lunco-usd-sim`.

## Relationship to the Document System

A USD stage is a `UsdDocument` in the Document System model
([see 10-document-system.md](10-document-system.md)). Planned `UsdOp` set:

```rust
enum UsdOp {
    AddPrim        { path, type_name },
    RemovePrim     { path },
    SetAttribute   { path, attr, value },
    SetTransform   { path, xform },
    SetRelationship{ path, rel, targets },
    // ...
}
```

Views observing a `UsdDocument`:

- **3D viewport** — renders the stage via Bevy + avian3d
- **Scene tree panel** — shows the prim hierarchy
- **USDA text editor** — text view of the stage (authoritative or preview)
- **Property inspector** — shows attributes of the selected prim

Per the Document System pattern, editing in any view produces a `UsdOp` that
applies to the `UsdDocument`; all other views update immediately.

## Current state

Today (pre-Document-System migration) USD is loaded into the ECS world at
spawn time and lives there as components (USD prims become Bevy entities
via `UsdPrimPath`). This works for view-only scenarios. Editing support will
land when the `UsdDocument` is implemented.

## Relationship to the simulation layers

USD is the **3D scene**, not a simulation artefact. Scenarios (the
simulation graph — participants, connections, clocks) live as RON files
under `<twin>/scenarios/` and are declared in `twin.toml`. The link
between a 3D prim and a simulation participant is a single stable id:

```usda
def Xform "Engine" (
    customData = {
        string "luncosim:participant_id" = "engine"
    }
) {
    # transform, mesh, mass, thrust vector, ...
}
```

The scenario names `engine` as a participant with a Modelica source +
connections; USD places it in 3D and optionally attaches renderable
geometry. Edit the pose in the viewport → USD changes, scenario
unaffected. Edit wiring in the diagram → scenario changes, pose
unaffected. They round-trip through different authoring surfaces but
meet at `participant_id`.

A future `LuncoSimParticipant` USD schema can replace the `customData`
stub with typed attributes (source path, fidelity-id override,
connection endpoints), but for MVP the customData key is enough.

See [`14-simulation-layers.md`](14-simulation-layers.md) for the
Twin / Scenario / Run / Model shape.

## Open content to migrate

- `docs/USD_SYSTEM.md` (legacy) — overview of USD-Bevy integration
- Per-crate READMEs in `lunco-usd-*` (when written)

## See also

- [`10-document-system.md`](10-document-system.md) — the pattern this domain fits into
- [`00-overview.md`](00-overview.md) — big picture, three-tier architecture
- [`14-simulation-layers.md`](14-simulation-layers.md) — simulation layers + participant_id link
- `specs/030-usd-scene-integration` — detailed spec
