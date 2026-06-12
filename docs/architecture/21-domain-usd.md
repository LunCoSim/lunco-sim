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

| Open entry point | Result |
|---|---|
| **Open Twin…** (folder) | real Twin (`root_path`, `twin.toml`, scenarios, runs) → designated stage active → Grid |
| **Open Scene…** (loose `.usda`) | **ephemeral Twin** (`root_path = None`, anchoring uses the file's own parent dir) → that file's document active → Grid |
| **New scene** | ephemeral Twin → untitled stage document active → Grid |

The ephemeral Twin has no `twin.toml` / scenarios / runs on disk; its active
stage is the loose file's `UsdDocument` (already opened by `on_open_file` as
`DocumentOrigin::File`). It is still runnable (implicit Scenario/Run, spec 14)
and saveable (`SaveDocument` writes the `.usda`). **`SaveAsTwin`** promotes it
to a real folder Twin.

## Which stage opens — scene resolution

A Twin may contain **many** `.usda` files. Exactly one is the **active stage**
that projects into the Grid; the rest are an **asset library** — referenceable
into the active stage, never auto-loaded. This section is the canonical rule
for *which* stage opens.

### Why a declared entry point

Core USD has **no project-level entry point**. The organizing unit is a single
**root layer** (`.usd` / `.usda` / `.usdc`): you open *one* file and
**composition** (sublayers, references, payloads, variants) pulls in everything
else, producing the **stage**. The only entry-point mechanisms USD itself
provides are **within a file** (`defaultPrim` layer metadata) or **by naming
convention** — neither resolves "which file in a folder is the scene."

`twin.toml` fills that gap by **declaring** the entry point. The Twin layer
earns its keep precisely by naming the starting scene — we never *infer* it
from a folder of files.

### Resolution rule

The Twin adds exactly **one** thing over a plain folder: it **auto-loads the
declared starting scene**. Nothing else is inferred.

| Open entry point | Browser | Active stage on open |
|---|---|---|
| **Open Folder** (no manifest) | lists all files (USD, Modelica, …) | **none** — user double-clicks a `.usda` to load it |
| **Open Twin** (`twin.toml`) | same folder browser | **auto-loads `[usd] default_scene`** |
| **Twin** with no `default_scene` | same folder browser | none — behaves like a folder; warn "no starting scene declared" |
| **Loose `.usda`** (orphan) | just that file | that file (ephemeral Twin, above) |

Opening a Twin **is** opening its folder — same browser, same file list — with
the single addition that `default_scene` is loaded automatically. A folder
loads nothing until the user picks a file; the Twin's manifest *is* that pick,
pre-declared.

Whether loaded automatically (Twin) or by the user's double-click (folder), a
scene loads as a **single root** (the `LoadScene` / `SetActiveStage` path —
clear-and-replace, one `UsdPrimPath` root under the Grid). Loading another
scene re-points that single active stage; it never stacks.

> **This supersedes today's "import every scene" behavior.** On `TwinAdded`,
> the current code (`open_usd_docs_on_twin_added`, `lunco-usd/src/commands.rs`)
> loops *every* `.usda` in the Twin and fires `OpenFile` on each — and for a
> USD path `OpenFile` not only registers the document, it **additively mounts**
> it into the Grid (`spawn_scene_root_world`, Blender-style append). So opening
> a Twin with three scenes stacks all three into one viewport. That fights
> composition and is **not** the intended model.
>
> Intended behavior: on Twin open, resolve **one** active stage per the table
> above and mount only that. The other `.usda` files are still *indexed* and
> shown in the browser (so the user can see and open them), but are **not**
> mounted — they are a referenceable asset library, composed into the active
> stage on demand via `AddReference` (see Verbs). Switching scenes re-points
> the single active stage; it never stacks.

### `default_scene` is a path, the scene owns composition

`[usd] default_scene` names a path **relative to the Twin root**. Keep the
manifest thin: it points *at* a USD root; the USD root owns scene composition
(sublayers/references/payloads). Don't grow the manifest into a scene
description — that's USD's job. See
[`13-twin-and-workflow.md`](13-twin-and-workflow.md) § 3 for the `[usd]`
section.

## Verbs — they all reuse existing surfaces

| User intent | Operation | Surface |
|---|---|---|
| **Open a Twin** | Open a folder → designated stage becomes active → Grid renders it | existing `OpenFolder`/`OpenTwin` + folder picker |
| **Open a loose scene** | Open a `.usda` → ephemeral Twin → that file's document becomes the active stage → Grid | `OpenFile` (registers the doc) + `OpenScene`/`SetActiveStage` (makes it the world) |
| **Built-in demo** | implicit Twin opened at startup | startup |
| **Add object / import** | author into the current stage: `ApplyUsdOp { active_stage, AddReference{…} }` (primitives: `AddPrim`); recompose into Grid; saved into the Twin by `SaveDocument` | existing `ApplyUsdOp` + one new `UsdOp` |
| **Promote loose → Twin** | `SaveAsTwin` | existing |
| **Run / server** | `TwinCommand`s | existing `--api` surface (spec 14 "Headless + remote") |

---

## Technical Reference — Implementation Details

### Pipeline Phases

1. **UsdBevyPlugin** — Spawns child entities for USD prims and attaches meshes + transforms.
2. **UsdAvianPlugin** — Maps USD physics attributes (`physics:rigidBodyEnabled`, `physics:mass`, `physics:collisionEnabled`) to Avian3D components.
3. **UsdSimPlugin** — Detects simulation schemas (`PhysxVehicleContextAPI`, `PhysxVehicleWheelAPI`, `PhysxVehicleDriveSkidAPI`) and creates `WheelRaycast`, `FlightSoftware`, `DifferentialDrive`, etc.

### Rover Definitions

#### Consolidated Base Files
| File | Steering | Default Wheel Type |
|------|----------|-------------------|
| `skid_rover.usda` | `PhysxVehicleDriveSkidAPI` | `raycast` |
| `ackermann_rover.usda` | `PhysxVehicleDrive4WAPI` | `raycast` |

#### Wheel Type Declaration
The `lunco:wheelType` attribute on the **chassis prim** determines wheel behavior:
- `raycast` (default): `WheelRaycast`, `RayCaster`, entity splitting.
- `physical`: `RigidBody`, `Collider`, `MotorActuator`.

#### Entity Layout (Raycast Rover)
Raycast wheels need identity rotation so `RayCaster` casts straight down. The system splits the USD wheel into:
1. **Physics entity**: identity rotation, NO mesh.
2. **Visual child entity**: correct orientation + mesh.

### glTF Payloads & Placeholders

For glTFs that ship via `Assets.toml` (e.g. Perseverance), we pair a `lunco-lib://` payload with a **`def Cube` placeholder**. 
- Third-party tools (Blender, usdview) fall back to the Cube.
- Our pipeline overlays the photoreal glTF and hides the Cube.

#### Why a `.glb` payload isn't composable, and interop

A `.glb`/`.gltf` is **not a USD layer** — USD composition only composes formats
a registered `SdfFileFormat` plugin can parse (`.usda`/`.usdc`/`.usd`/`.usdz`).
Core USD ships no glTF plugin, so a `payload = @terrain.glb@` resolves to an
empty layer in stock USD. Our engine sidesteps this: it detects the binary
extension, stubs the arc out of composition, and routes the file to Bevy's glTF
loader via a synthesized `lunco:resolvedAsset` (so the terrain renders for us,
native + web).

**To make the glb compose in external tools (Blender/usdview):** install
Adobe's open-source [`USD-Fileformat-plugins`](https://github.com/adobe/USD-Fileformat-plugins)
(glTF/FBX/OBJ/STL/PLY `SdfFileFormat` plugins) and point `PXR_PLUGINPATH_NAME`
at them. The `@terrain.glb@` payload then composes natively as `Mesh` geometry —
config only, no conversion, no engine code. This is the proper interop path.

**TODO (proper internal handling):** mirror that plugin inside our pure-Rust
pipeline with a small glTF→USD-layer shim in `lunco-usd-bevy/compose.rs` (emit
`Mesh` specs instead of stubbing), removing the `lunco:resolvedAsset`
side-channel so terrain is ordinary composed USD everywhere. See the
`TODO(glb-composability)` marker in `lunco-usd-bevy/src/resolver.rs`.

### Reference Resolution
USD references (e.g., `@/components/mobility/wheel.usda@`) are resolved relative to the **USD asset root** (`assets/`). The `UsdComposer` resolves:
- `/`-prefixed paths anchor at the asset root.
- Plain relative paths anchor at the layer's directory.
- URI schemes (`lunco-lib://`) pass through to the `AssetSource`.

### Sandbox Editing Tools (UX Bridge)
The `lunco-sandbox-edit` crate provides the interactive layer (palette, gizmo, inspector).
- **Spawning**: `SpawnEntity` command is wired to `UsdOp::AddReference` against the active stage.
- **Manipulation**: The transform gizmo authors `UsdOp::SetTranslate` against the document.
- **Undo**: Reverting a `UsdOp` in the document system automatically updates the 3D world.

| Scheme | Purpose | Resolves to |
|---|---|---|
| (none) | In-tree authored content / scene-relative | `assets/...` or the layer's source |
| `lunco-lib://` | Workspace-shipped fixtures (downloaded models) | `<cache>/...` |
| `lunco://` | **Engine asset library** (rovers, parts, vessels) — location-independent ref usable from external Twins | `assets/...` |
| `twin://<name>/...` | **Internal, runtime-only.** The currently-open Twin's root, keyed by Twin name. Reads an external Twin scene + its co-located assets (fs on native, http on web). Never authored into a file. | the opened Twin folder |

> `lunco://` was previously *reserved* for a future collaborative protocol; it's
> now the engine library scheme. A collaborative/Nucleus-like protocol, if added,
> should pick a distinct scheme (e.g. `lunco-net://`).
>
> **External Twins:** a scene living outside the project (its own repo) is opened
> via File → Open Folder. The Twin-open flow registers the folder under
> `twin://<name>` (name from `twin.toml`) and loads `twin://<name>/<default_scene>`.
> The scene authors only **relative** paths (co-located terrain glb) and
> `lunco://` library refs — so the `.usda` is portable and identity
> (`Provenance`) is the stable `twin://<name>/<rel>`, not a machine path.

### Coordinate Systems

| System | Up Axis | Forward Axis | Notes |
|--------|---------|--------------|-------|
| USD    | Y       | +Z           | Standard USD convention |
| Bevy   | Y       | -Z           | Right-handed, Z-backward |
| Avian3D| Y       | -Z           | Matches Bevy |

### Testing
All tests load **real USD files** through the same pipeline as runtime:
- `integration_asset_loading.rs` — verifies full pipeline (composition → Bevy → Avian → Sim)
- `rover_structure.rs` — verifies wheel entity structure (identity rotation + visual child)

---

## See also

- [`41-axes-and-units.md`](41-axes-and-units.md) — coordinate/unit conversion boundary
- [`10-document-system.md`](10-document-system.md) — the document pattern
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — Twin container + layout
- [`14-simulation-layers.md`](14-simulation-layers.md) — Twin/Scenario/Run/Model + `participant_id`
- [`00-overview.md`](00-overview.md) — three-tier architecture
- `specs/030-usd-scene-integration` — detailed spec
