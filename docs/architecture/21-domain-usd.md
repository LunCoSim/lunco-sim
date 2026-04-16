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

## Open content to migrate

- `docs/USD_SYSTEM.md` (legacy) — overview of USD-Bevy integration
- Per-crate READMEs in `lunco-usd-*` (when written)

## See also

- [`10-document-system.md`](10-document-system.md) — the pattern this domain fits into
- [`00-overview.md`](00-overview.md) — big picture, three-tier architecture
- `specs/030-usd-scene-integration` — detailed spec
