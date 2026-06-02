# 21 — USD Domain

> USD (Pixar Universal Scene Description) is the scene-graph and asset format
> LunCoSim uses for the 3D world. Bases, rovers, habitats, terrain — everything
> physical — lives as USD prims in USD stages. See
> [`../../crates/lunco-usd/`](../../crates/lunco-usd/) and companion crates
> `lunco-usd-avian`, `lunco-usd-bevy`, `lunco-usd-composer`, `lunco-usd-sim`.

## Scope

A USD **stage** is the 3D scene. This doc is the canonical reference for how a
scene is **owned, loaded, rendered, and edited**. The short version:

> **The Twin owns the scene. The live 3D world (the `Grid` / `BigSpace` root)
> is the *rendered result of the active Twin's current state* — its active USD
> stage *document* plus its active Run state. You don't load files into the
> world; the world is a projection of the Twin.**
>
> A **loose** `.usda` is not an exception: opening one materialises an
> *ephemeral Twin* around it (VS Code's open-file-vs-open-folder model; spec
> 14's *"implicit Twin materialised on workspace open"*). Same pipeline, same
> invariant — a loose file is the degenerate Twin, promotable to a real folder
> Twin with `SaveAsTwin`.

This aligns with the canonical layer model in
[`14-simulation-layers.md`](14-simulation-layers.md) (*"Twin is the control
surface… owns documents + scenarios + runs"*) and the Document System in
[`10-document-system.md`](10-document-system.md).

## Relationship to the Document System

A USD stage is a `UsdDocument` in the Document System model. Editing in any
view produces a `UsdOp` that applies to the document; every other view updates.
Current `UsdOp` set (`lunco-usd/src/document.rs`) and the planned additions:

```rust
enum UsdOp {
    AddPrim     { edit_target, parent_path, name, type_name },
    RemovePrim  { path },
    SetTranslate{ path, value },
    ReplaceSource { .. },           // whole-document text replace
    // planned — authoring an external asset *into* the current stage:
    AddReference{ edit_target, parent_path, name, asset_uri },   // def "X" (references = @uri@)
    AddPayload  { edit_target, parent_path, name, asset_uri },   // deferred-load variant
}
```

Views observing a `UsdDocument`:

- **3D viewport / Grid** — renders the stage via Bevy + avian3d (the *live*
  world; see "Active stage" below)
- **Scene tree panel** — the prim hierarchy
- **USDA text editor** — text view of the stage
- **Property inspector** — attributes of the selected prim

## Scene ownership — Twin → active stage → Grid

### The chain

```
Twin (workspace folder, owns documents)         spec 14
  └─ active USD stage = a UsdDocument            spec 10 / 21
        └─ composed (UsdComposer::flatten)        lunco-usd-composer
              └─ UsdStageAsset (baked stage)       lunco-usd-bevy
                    └─ UsdPrimPath root under Grid  → sync_usd_visuals spawns entities
                          └─ the live 3D world      (avian + cosim translators key off prims)
```

The Grid is **downstream** of the Twin's stage document. Opening a different
Twin, or switching its active stage, re-points the Grid at a different stage
document. The built-in demo scene is just the **implicit Twin** opened at
startup (spec 14: *"one implicit Twin materialised on workspace open"*).

### Folder Twins vs loose files vs new — one pipeline, three doors

Mirrors VS Code (open-folder = workspace, open-file = implicit workspace):

| Open entry point | Result |
|---|---|
| **Open Twin…** (folder) | real Twin (`root_path`, `twin.toml`, scenarios, runs) → designated stage active → Grid |
| **Open Scene…** (loose `.usda`) | **ephemeral Twin** (`root_path = None`, anchoring uses the file's own parent dir) → that file's document active → Grid |
| **New scene** | ephemeral Twin → untitled stage document active → Grid |

The ephemeral Twin has no `twin.toml` / scenarios / runs on disk; its active
stage is the loose file's `UsdDocument` (already opened by `on_open_file` as
`DocumentOrigin::File`). It is still runnable (implicit Scenario/Run, spec 14)
and saveable (`SaveDocument` writes the `.usda`). **`SaveAsTwin`** promotes it
to a real folder Twin — the loose door becomes the folder door. No parallel
loading path exists: loose is the degenerate Twin flowing through the same
*active stage → Grid* projection.

### How a Twin designates its root stage

Resolved in priority order:

1. **Explicit manifest key** — `twin.toml`:
   ```toml
   [usd]
   root = "scenes/main.usda"
   ```
   Wins whenever present. Lets a Twin carry several `.usda` files and name
   which one is the scene.
2. **Convention + fallback** — when no key is set:
   - exactly one `.usda` in the Twin → that is the stage;
   - several → prefer `scene.usda` / `main.usda`, else the stage whose
     `defaultPrim` resolves.

`open_usd_docs_on_twin_added` already opens a Twin's `.usda` files into
`UsdDocumentRegistry`; designation only picks *which* opened document is marked
active.

### The active-stage bridge

`viewport.rs::install_active_doc` already implements *document → live 3D*: parse
+ flatten the document's source (anchored to its own directory — USD's
layer-relative rule), `Assets::<UsdStageAsset>::add`, attach `UsdPrimPath`,
rebuild on change. Today it targets the **preview viewport's** `scene_root`.
The proper architecture **retargets this onto the `Grid`**, owned per-Twin via
an `ActiveStage` marker, so the simulated world is the active stage document.

This replaces the interim baked path (`setup_sandbox` + a hardcoded
`ScenePath` + `LoadScene` via `asset_server`), which spec 21 previously flagged
as view-only and awaiting the `UsdDocument`.

## Verbs — they all reuse existing surfaces

| User intent | Operation | Surface |
|---|---|---|
| **Open a Twin** | Open a folder → designated stage becomes active → Grid renders it | existing `OpenFolder`/`OpenTwin` + folder picker |
| **Open a loose scene** | Open a `.usda` → ephemeral Twin → that file's document becomes the active stage → Grid | `OpenFile` (registers the doc) + `OpenScene`/`SetActiveStage` (makes it the world) |
| **Built-in demo** | implicit Twin opened at startup | startup |
| **Add object / import** | author into the current stage: `ApplyUsdOp { active_stage, AddReference{…} }` (primitives: `AddPrim`); recompose into Grid; saved into the Twin by `SaveDocument` | existing `ApplyUsdOp` + one new `UsdOp` |
| **Promote loose → Twin** | `SaveAsTwin` | existing |
| **Run / server** | `TwinCommand`s | existing `--api` surface (spec 14 "Headless + remote") |

**Import is not a verb of its own** — it is *editing the active stage* (USD
reference / Blender Link-Append / Godot child instance), not a second scene.

**The loose-file open verb is *"set the active stage"*, not a baked-asset
loader.** `OpenFile` registers a `.usda` document to inspect; `OpenScene` /
`SetActiveStage` makes a document the active stage of the (ephemeral or real)
Twin so the Grid projects it. The old `LoadScene` (which did
`asset_server.load` of a baked `UsdStageAsset`) is retired in favour of this —
its job becomes "set active stage", routed through the Twin/`ActiveStage`
model, never a parallel loading path.

### Live update must be incremental

The preview viewport can afford a full `rebuild_active_asset` (despawn
children, re-sync). The **live simulated world cannot** — a full recompose on
every edit would discard avian physics state, cosim steppers, and big_space
cells. `sync_usd_visuals` is already additive (`Without<UsdVisualSynced>`), so:

- **Add object** → author the prim into the active stage doc, re-flatten, and
  spawn **only the new prim's subtree** under the existing root. Physics/cosim
  untouched.
- **Open / replace stage** → the only operation that does a full despawn +
  respawn (it is replacing the whole stage).

## Relationship to the simulation layers

USD is the **3D scene**, not a simulation artefact. Scenarios (participants,
connections, clocks) live as RON under `<twin>/scenarios/` and are declared in
`twin.toml` (spec 14). The link between a 3D prim and a simulation participant
is one stable id carried on the prim:

```usda
def Xform "Engine" (
    customData = { string "luncosim:participant_id" = "engine" }
) { # transform, mesh, mass, thrust vector, ... }
```

Edit the pose in the viewport → USD changes, scenario unaffected. Edit wiring
in the diagram → scenario changes, pose unaffected. They round-trip through
different authoring surfaces but meet at `participant_id`. A future
`LuncoSimParticipant` USD schema can replace the `customData` stub with typed
attributes; for MVP the customData key is enough.

## Staged implementation plan

Each step is additive and independently verifiable.

1. **Grid renders the active stage document.** Retarget the `install_active_doc`
   bridge from the viewport `scene_root` onto the `Grid`; add an `ActiveStage`
   notion. Demo scene becomes the implicit Twin's stage. *Foundation; removes
   the baked `ScenePath` spawn.*
2. **Open Twin → stage active on Grid.** Resolve the Twin's root stage
   (manifest `[usd] root` priority, else convention/fallback) and mark it
   active when the Twin opens.
3. **Add object.** `UsdOp::AddReference` (+ `AddPayload`) + incremental subtree
   spawn + Save into the Twin. Menu "Add → Object/Reference…".
4. **Menu/picker.** Surface "Open…" as Open Twin; retire the loose-file scene
   loader.

## Relationship to existing specs / built features

- **spec 030 (usd-scene-integration)** — the USD parser / adapter / Avian /
  composer pipeline and the **reference** mechanism (`rover.usda` referenced
  into a scene, edits propagate). Largely **built** (`lunco-usd-*`). "Import an
  object" = authoring such a reference — no new verb.
- **spec 031 (sandbox-editing-tools)** — **built** in `lunco-sandbox-edit`:
  spawn palette + catalog (`SpawnSource::UsdFile`), click-to-place, ghost
  preview, selection, translate/rotate gizmo, force tool, parameter inspector,
  undo. *This is the entire in-scene "add object" UX — reuse it, do not
  rebuild.* **But 031 §Out-of-Scope explicitly defers "saving/loading edited
  scenes back to USD"**: spawns/gizmo edits mutate loose ECS entities, not a
  document. **This design supersedes that deferral** — the missing capability
  is *persistence*: wire `SpawnEntity` → `UsdOp::AddReference` and the gizmo →
  `UsdOp::SetTranslate` against the active stage document so edits round-trip
  and save into the Twin.
- The runtime **Twin** control resource (spec 14: scenarios/runs/`TwinCommand`)
  is **not built yet** — today only the folder model (`lunco_twin::Twin`) +
  `Workspace` exist. So near-term, *"world = active stage document"* is the
  achievable invariant; full *"world = Twin current state"* (with Runs) follows
  spec 14.

## See also

- [`41-axes-and-units.md`](41-axes-and-units.md) — coordinate/unit conversion boundary (the USD spoke runs at the document↔world edge; needed for *external* USD)
- [`10-document-system.md`](10-document-system.md) — the document pattern
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — Twin container + layout
- [`14-simulation-layers.md`](14-simulation-layers.md) — Twin/Scenario/Run/Model + `participant_id`
- [`00-overview.md`](00-overview.md) — three-tier architecture
- `specs/030-usd-scene-integration` — detailed spec
- Legacy to migrate: `docs/USD_SYSTEM.md`, `lunco-usd-*` per-crate READMEs
