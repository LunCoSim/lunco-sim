---
name: build-usd-scene
description: >
  How to author and edit the 3D world in LunCoSim — load scenes, spawn objects,
  place/move/rotate them, and tune their properties, over the API. USE THIS
  SKILL whenever the user asks, in plain words, things like: "put a lander near
  that crater", "spawn a few rovers here", "load the Moon scene", "add some
  rocks / obstacles", "move / rotate / scale this", "set its colour / mass /
  material", "build a scene with X and Y", or "clear the scene and start over".
  Any request to assemble or edit what's IN the 3D world belongs here — the user
  won't say "USD" or "prim". (For the agent mid-code: `LoadScene` / `SpawnEntity`
  / `MoveEntity` / `SetObjectProperty`, an `entry_id` from the spawn catalog, a
  `.usda` file, coordinate placement, or "why did the gizmo grab the wrong
  thing?".) Project-specific and non-obvious: USD is the SOURCE OF TRUTH
  (projected to ECS — you edit the world by authoring it), the engine frame is
  fixed (Y-up, right-handed, −Z-forward, metres), `LoadScene` paths are relative
  to the assets root, spawnable things come from a catalog (`list_bundled`), and
  live edits must NOT go through `SetDocumentSource`. For the vehicle's BEHAVIOUR
  use author-scenario; for its GNC use authoring-vessel-controllers.
---

# Build & edit USD scenes

The 3D world is **OpenUSD, projected to Bevy ECS** — USD is the source of truth,
the ECS scene is its projection. You build the world by **authoring USD** (via
commands that apply reversible ops), not by mutating ECS directly. Drive it over
the API (`--api`, port **4101**; launch per [`test-via-api`](../test-via-api/SKILL.md)).

Design background: [`21-domain-usd.md`](../../docs/architecture/21-domain-usd.md),
[`usd-source-of-truth-ecs-projection-design.md`](../../docs/usd-source-of-truth-ecs-projection-design.md).

## The one coordinate frame (spec 009)

The engine runs in **one fixed canonical frame: Y-up, right-handed, −Z-forward,
SI metres, f64.** Any external asset (USD `upAxis`/`metersPerUnit`, glTF, Blender)
is converted **once, at the importer** — never branch on convention in your own
placement math. A `position` you pass to `SpawnEntity` is Y-up metres.

## The command surface

| Command | Params | Does |
|---|---|---|
| `LoadScene` | `{path, root_prim}` | Load a USD scene. `path` is **relative to the `assets/` root** (do NOT prefix `assets/`). `root_prim` empty = the stage's `defaultPrim`. |
| `ClearScene` | `{}` | Tear down the current scene. |
| `RestartScene` | `{}` | Reload/reset the current scene. |
| `SpawnEntity` | `{target, entry_id, position:[x,y,z], rotation?}` | Instance a catalogued prefab. `entry_id` comes from the **spawn catalog** (`list_bundled` / `ListBundled`). |
| `MoveEntity` | `{…}` | Reposition an existing entity. |
| `SetObjectProperty` | `{entity_id:u64, property, value}` | Set a named property (both strings; value is coerced by property type). |
| `SelectEntity` | `{…}` | Select (drives the gizmo/inspector). |
| `SetPorts` | `{target, writes:[[name,val]]}` | Poke an input port (e.g. drive a spawned rover) — see [`author-scenario`](../author-scenario/SKILL.md) for behaviour. |

Discover the live set with `DiscoverSchema`; discover spawnables with `list_bundled`.

## Recipe

1. **Base:** `LoadScene {path:"scenes/…/foo.usda", root_prim:""}` for an existing
   scene, or start from the loaded default and add to it. `ClearScene` first if
   replacing.
2. **What can I spawn?** `list_bundled` → pick an `entry_id`.
3. **Place it:** `SpawnEntity {entry_id, position:[x,y,z], rotation?}` (Y-up metres).
   The response `data` carries the new entity id.
4. **Adjust:** `MoveEntity` / `SetObjectProperty` (colour, mass, material, scale) /
   `SelectEntity` to inspect.
5. **Confirm:** `CaptureScreenshot` → `/tmp/x.png` → Read it (see
   [`inspect-simulation`](../inspect-simulation/SKILL.md) for reading state back).
6. **Persist:** to make it permanent, author it into the `.usda` scene file under
   `assets/scenes/` (the runtime edits are USD ops; save them into the layer).

## Gotchas

- **`LoadScene` path is relative to `assets/`** — `"scenes/sandbox/lander_test.usda"`, never `"assets/scenes/…"`.
- **Spawn `entry_id` must be in the catalog** — an unknown id logs `unknown entry '…'` and no-ops. List first with `list_bundled`.
- **Empty spawn path / root_prim → the `defaultPrim` sentinel**: an empty path means "the stage's default prim", not an error.
- **Gizmo / selection frame:** on a static-USD select, the selectable root is tagged `SelectableRoot` in the **world frame** — not `GridAnchor`. If the gizmo grabs the wrong thing or the wrong frame, that tag is why.
- **Never `SetDocumentSource` for live scene building** — it replaces the whole source and cancels in-flight work. Apply edits as **individual ops** (`SpawnEntity`/`MoveEntity`/`SetObjectProperty`), one at a time.
- **USD → ECS is a projection**, so authored changes flow one way — edit the USD (via ops), and the ECS scene reconciles. Don't hand-mutate ECS transforms expecting them to persist.
- **Behaviour ≠ scene.** Making a spawned rover *do* something (drive, patrol) is a scenario — see [`author-scenario`](../author-scenario/SKILL.md); its self-driving GNC is [`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md).

## Anti-patterns

- ❌ Prefixing `LoadScene` paths with `assets/`.
- ❌ Guessing an `entry_id` instead of `list_bundled`.
- ❌ `SetDocumentSource` to build a scene incrementally — use per-object ops.
- ❌ Branching placement math on up-axis/units — the frame is fixed; convert only at the importer.
- ❌ Mutating ECS `Transform` directly and expecting USD to remember it — author the USD.
