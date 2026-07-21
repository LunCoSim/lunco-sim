# 21 — USD Domain

> Status: Active · Audience: contributors on scene-graph, geometry, and the 3D world
>
> USD (Pixar Universal Scene Description) is the scene-graph and asset format
> LunCoSim uses for the 3D world. Bases, rovers, habitats, terrain — everything
> physical — lives as USD prims in USD stages. See
> [`../../crates/lunco-usd/`](../../crates/lunco-usd/) and companion crates
> `lunco-usd-avian`, `lunco-usd-bevy` (which also owns composition/flattening), `lunco-usd-sim`.

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
view produces a **typed `UsdOp`** that applies to the document; every other view
updates. The op is the single description of the delta — never a diff re-derived
by reading state back (the *author-once coherence* invariant, below).

Current `UsdOp` set (`lunco-usd/src/document.rs`), each carrying an
`edit_target: LayerId` naming which layer receives the opinion:

```rust
enum UsdOp {
    ReplaceSource   { edit_target, source },           // whole-layer text replace
    AddPrim         { edit_target, parent_path, name, type_name, reference },
    RemovePrim      { edit_target, path },
    MovePrim        { edit_target, from_path, to_path }, // rename / reparent (NamespaceEditor)
    SetTranslate    { edit_target, path, value },
    SetRotate       { edit_target, path, value },
    SetAttribute    { edit_target, path, name, type_name, value },
    SetRelationship { edit_target, path, name, targets },
    SetConnection   { edit_target, path, name, type_name, sources }, // dataflow edges (W1)
    SetTimeSample   { edit_target, path, name, time, value },
    RemoveTimeSample{ edit_target, path, name, time },
}
```

`AddReference`/`AddPayload` are folded into `AddPrim { reference }` +
`author_reference`. Programmatic and UI edits go through the
**`ApplyUsdOp { doc, op }`** command (`commands.rs`, observer `on_apply_usd_op`),
which returns a generation-ack; direct source mutation is out. `UsdOp` implements
both `DocumentOp` and `lunco_twin_journal::OpPayload` — so **authoring an edit *is*
journaling it *is* syncing it** (see [`31-networking-and-state-sync.md`](31-networking-and-state-sync.md) § Journal plane).

Views observing a `UsdDocument`:

- **3D viewport / Grid** — renders the stage via Bevy + avian3d (the *live*
  world; see "Active stage" below)
- **Scene tree panel** — the prim hierarchy
- **USDA text editor** — text view of the stage
- **Property inspector** — attributes of the selected prim

### Two representations: authored layers ⊕ the composed stage

A running scene is held in **two** forms, and neither absorbs the other — this is
USD's own `SdfLayer` (authored opinions you save) vs `UsdStage` (the composition) split:

- **`UsdDocument`** (`lunco-usd/src/document.rs`) — the authored `sdf::Data` **layers**:
  `base` (persisted root layer, written on Save) **⊕** `runtime` (ephemeral overlay —
  spawns, moves, obstacle fields — *not* saved). `LayerId::root()` vs `LayerId::runtime()`
  route each op. Plain, `Send`, serializable: this is what Save writes, the journal
  records, and the network ships. Reads are cheap and off-main-thread.
- **`CanonicalStage`** (`lunco-usd-bevy/src/canonical.rs`) — the live, *composed* openusd
  `Stage` with references / sublayers / variants resolved. `Rc`-backed, therefore `!Send`:
  a main-thread `NonSend` resource (`CanonicalStages`). It is the projection engine —
  authoring onto it fires openusd's change sink, which reconciles the ECS.

The `Send`/`!Send` boundary falls on this same seam by nature, so the two stay even if
openusd ever makes `Stage` `Send`. Save / journal / net-sync touch the cheap serializable
layers; composition (the expensive resolver work) is isolated to the one stage owner.

### Op-driven projection (author-once coherence)

Edits reach the live world by **replaying the typed op onto the `CanonicalStage`**, not
by re-flattening the scene per edit:

```
UsdOp ─apply→ UsdDocument (base⊕runtime, op_log, generation++)
        │
        ├─ journal records op + inverse (undo / sync)
        └─ sync_twin_overlays replays op → CanonicalStage.author_*  (twin_projection.rs)
                    │  fires openusd change sink
                    └─ project_stage_changes drains sink → reconcile ECS  (live_consume.rs)
                         · InfoOnly xformOp:translate → cheap pose update
                         · Resync → spawn added / despawn removed subtree
```

Invariant: **every generation bump records exactly one op-log entry**. `ops_since`
returns `None` when the op ring is shorter than the generation delta, degrading safely
to a full rebuild (`rebuild_scene_from_composed`) rather than a silent projection lie.
Coarse ops (`ReplaceSource`, `MovePrim`, `RemoveTimeSample`, `SetRelationship`) rebuild;
the common interactive ops replay incrementally (`apply_incremental_op_to_stage`).

The **read** surface is the `UsdRead` trait (`lunco-usd-bevy/src/read.rs`): `children`,
`scalar::<T>`, `attr_value`, `rel_target`, `scalar_at` (time-sampled), etc. It is
implemented for both `StageView` (the live composed stage, `view.rs`) and `sdf::Data`
(the flattened layer), so one generic reader works against live and flattened alike.
The `UsdStageAsset` now carries only a `Send` `StageRecipe` (`recipe`) — the live stage
is built on the main thread from it; there is no stored `reader` object.

## Scene ownership — Twin → active stage → Grid

### The chain

```
Twin (workspace folder, owns documents)         spec 14
  └─ active USD stage = a UsdDocument            spec 10 / 21
        └─ composed (flatten_stage)               lunco-usd-bevy/compose.rs
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

On `TwinAdded` (`open_usd_docs_on_twin_added`, `lunco-usd/src/commands.rs`),
exactly **one** stage resolves per the table above, and the mount is
**doc-first**: the scene's document opens first (its base read through the
`twin://` source, web-ready), the persisted `.lunco/runtime` overlay is
restored into it, its composed (`base ⊕ runtime`) source is published as the
twin byte-overlay — and only then does `LoadScene` fire, so the **single**
projection already carries restored runtime spawns/moves (see the E1b flow in
[18-unified-journal-and-history](18-unified-journal-and-history.md)). The
Twin's other `.usda` files are *indexed* and shown in the browser but **not**
mounted — a referenceable asset library, composed into the active stage on
demand via `AddReference` (see Verbs). Switching scenes re-points the single
active stage; it never stacks.

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
2. **UsdAvianPlugin** — Maps USD physics to Avian3D: rigid bodies (`PhysicsRigidBodyAPI`, with its `physics:rigidBodyEnabled`), mass-properties (`physics:mass`, `physics:diagonalInertia`, `physics:centerOfMass`), colliders (`physics:collisionEnabled`, all `UsdGeom` shapes), and **all joints** (see [Physics joints](#physics-joints)). The single home for Avian joint construction.
3. **UsdSimPlugin** — Detects simulation schemas (`PhysxVehicleContextAPI`, `PhysxVehicleWheelAPI`, and the Omniverse differential/steering APIs `PhysxVehicleTankDifferentialAPI` (skid) / `PhysxVehicleAckermannSteeringAPI`) and creates `WheelRaycast`, `ActuatorPorts`, a data-driven `DriveMix` (allocated by a named kernel in `lunco-mobility`'s `ControlKernelRegistry` — `skid`/`linear`), `DifferentialCoupling`, sensors, etc. Also wires actuator topology + drive mix and cosim models/wires (see [`22-domain-cosim.md`](22-domain-cosim.md)).

### Rover Definitions

#### Consolidated Base Files
| File | Steering | Default Wheel Type |
|------|----------|-------------------|
| `skid_rover.usda` | `PhysxVehicleTankDifferentialAPI` (skid) | `raycast` |
| `ackermann_rover.usda` | `PhysxVehicleAckermannSteeringAPI` | `raycast` |

#### Wheel Type Declaration
The `lunco:wheelType` attribute on the **chassis prim** determines wheel behavior:
- `raycast` (default): `WheelRaycast`, `RayCaster`, entity splitting.
- `physical`: `RigidBody`, `Collider`, `MotorActuator`.

#### Entity Layout (Raycast Rover)
Raycast wheels need identity rotation so `RayCaster` casts straight down. The system splits the USD wheel into:
1. **Physics entity**: identity rotation, NO mesh.
2. **Visual child entity**: correct orientation + mesh.

A raycast wheel decomposes traction in the **actual contact plane** (the ray-hit
normal), so a leaning single-track vehicle (bike/motorcycle) gets correct lateral
grip; for an upright wheel this is identical to the flat basis. The steer axis is
`lunco:steerAxis` (float3, wheel-local; default `+Y`) — a raked motorcycle fork
authors e.g. `(0, 0.91, 0.42)`.

### Physics Joints

All Avian joints are built by **`lunco-usd-avian`** from standard `UsdPhysics`
joint prims (`physics:body0/1` rels, `physics:axis` token, `physics:localPos0/1`
anchors, `physics:limitLower/Upper` or `physics:min/maxDistance`):

| USD prim | Avian joint | Notes |
|---|---|---|
| `PhysicsRevoluteJoint` | `RevoluteJoint` | 1-DOF hinge; exposes `angle` port |
| `PhysicsPrismaticJoint` | `PrismaticJoint` | 1-DOF slider; exposes `displacement` port |
| `PhysicsFixedJoint` | `FixedJoint` | rigid weld |
| `PhysicsSphericalJoint` | `SphericalJoint` | ball; `physics:coneAngle0/1Limit` → swing, limits → twist |
| `PhysicsDistanceJoint` | `DistanceJoint` | tether within `[minDistance, maxDistance]` |
| `PhysicsD6Joint` / `PhysicsJoint` | reduced | per-DOF `PhysicsLimitAPI` (`low>high`=locked) → the matching primitive; genuinely multi-DOF warns |

**Joint drive (`UsdPhysicsDriveAPI`):** `drive:angular:*` on a revolute or
`drive:linear:*` on a prismatic joint — `physics:targetPosition` (enables the
motor at load, so an Omniverse-authored mechanism seeks its setpoint with no
wire), `physics:targetVelocity`, `physics:maxForce` (motor saturation). A cosim
wire on the joint's `angle`/`displacement` port overrides the target per tick. The
programmatic wheel hinge also lives here (`wheel_revolute_joint`).

### Collision filtering — which pairs never touch

Two mechanisms, and only one of them is automatic.

**Jointed pairs** are filtered by avian: every joint this loader builds carries
`JointCollisionDisabled` (via `joint_bundle`), so a body never collides with the
body it is jointed to. That covers parent and child, and stops there.

**Everything else is authored**, through `UsdPhysicsFilteredPairsAPI`:

```usda
def Cube "Pad" ( prepend apiSchemas = [..., "PhysicsFilteredPairsAPI"] )
{
    rel physics:filteredPairs = </Lander/Hull>
}
```

Read by `lunco-usd-avian::filtered_pairs`, which resolves each end to the entity
that actually owns the collider — a collider under a body folds into that body's
compound shape, so naming either resolves to the body — and hands the pair to
avian's one `CollisionHooks` slot (`UsdCollisionFilter`, installed by
`PhysicsPlugins::with_collision_hooks` in `lunco-sandbox`). Filtering is
symmetric: one opinion is the whole pair.

Two properties worth knowing:

- **Armed before first contact.** Resolution runs in `PhysicsSystems::Prepare`,
  ahead of the narrow phase, because avian's broad phase skips any pair already
  in the contact graph. A filter applied later does not remove an existing
  contact.
- **Nothing is inferred.** There is no "a vehicle does not collide with itself"
  rule, because *vehicle* is not a thing the physics knows — a rover on a
  lander's deck is one or two depending on the minute, and an arm should collide
  with its own base. MuJoCo, PhysX and URDF/MoveIt each landed in the same
  place: automatic for adjacency, authored beyond it.

`PhysicsCollisionGroup` (the group-level `physics:filteredGroups` /
`invertFilteredGroups` / `mergeGroup` form, which maps to avian `CollisionLayers`)
is **not** read yet. Pairwise authoring is O(n²) in a subassembly, so this is the
next thing to add if a vehicle ever needs more than a handful of pairs.

### Sensors

`lunco:sensor:*` markers on a rigid-body prim attach `lunco-cosim` sensors that
expose telemetry ports (see [`22-domain-cosim.md`](22-domain-cosim.md)):

| Attribute | Sensor | Ports |
|---|---|---|
| `bool lunco:sensor:imu` | IMU | `accel_{x,y,z}` (world lin. accel), `spec_force_{x,y,z}` (body-frame `a−g`) |
| `bool lunco:sensor:range` (+ `token :rangeAxis`, `float :rangeMax`) | range finder | `range` (raycast distance along a body-local axis, default `-Y`) |
| `bool lunco:sensor:contact` | contact | `contact` (0/1), `contact_force` (N) |
| `float3 lunco:sensor:offset` | (shared) | body-local mount point — IMU lever-arm + range origin |

### Cameras

Scene cameras are **standard `def Camera` (`UsdGeomCamera`) prims** — `lunco-usd-bevy`
projects each to an *inactive* Bevy `Camera3d` (see [`17-view-and-intent.md §6`](17-view-and-intent.md)).

| Attribute | Meaning |
|---|---|
| `float focalLength`, `float verticalAperture` | vertical FOV = `2·atan(verticalAperture / (2·focalLength))` |
| `float2 clippingRange` | near / far planes |
| `token projection` | `perspective` (default) or `orthographic` |
| `custom double3 lunco:cameraLookAt` | aim the camera at this point (parent-local); overrides authored rotation |

- **Placement:** a **top-level** `def Camera` is a static scene camera (a wide
  shot); it can host the big_space `FloatingOrigin` directly. A `def Camera`
  **nested under a moving prim** (e.g. under a rover Xform) becomes an *onboard*
  camera that rides the mount — realised as a grid-direct follower so it stays
  jitter-free at any distance (no follow-code in the USD). Aim it forward with
  `lunco:cameraLookAt`.
- **Switching:** cameras spawn inactive; make one the active view with
  `set_camera("Name")` (rhai / API `SetActiveCamera`, matches the prim's leaf or
  full path) or the `KeyC` hotkey. Exactly one window camera renders at a time.

### glTF Payloads & Placeholders

For glTFs that ship via `Assets.toml` (e.g. Perseverance), we pair a `lunco://` payload with a **`def Cube` placeholder**. 
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

**Composition-stack anchoring.** The binary arc is discovered from the
*composed layer stack*, not just the root layer: `compose.rs` walks every loaded
layer (`discover_binary_sites`) keyed by authoring site `(layer id, spec path)`,
then matches each composed prim's `prim_stack` against those sites to anchor
`lunco:resolvedAsset` on the **composed prim**. So a `payload = @model.glb@`
authored *inside a referenced `.usda` wrapper* (the `scene → wrapper.usda → .glb`
shape the `structures/*.usda` model wrappers use) surfaces on the composed prim —
e.g. `/Scene/Bldg/Visual` — exactly like a glb referenced directly in the scene.
This keeps USD the source of truth for a placed model while still rendering the
glTF (and firing the failure placeholder). Covered by
`glb_payload_in_referenced_wrapper_anchors_on_composed_prim`.

**To make the glb compose in external tools (Blender/usdview):** install
Adobe's open-source [`USD-Fileformat-plugins`](https://github.com/adobe/USD-Fileformat-plugins)
(glTF/FBX/OBJ/STL/PLY `SdfFileFormat` plugins) and point `PXR_PLUGINPATH_NAME`
at them. The `@terrain.glb@` payload then composes natively as `Mesh` geometry —
config only, no conversion, no engine code. This is the proper interop path.

*Future Enhancement (Proper Internal Handling):* A small glTF→USD-layer shim in `lunco-usd-bevy/compose.rs` can be added to emit `Mesh` specs instead of stubbing. This would remove the `lunco:resolvedAsset` side-channel so that terrain is ordinary composed USD everywhere.

### Reference Resolution
USD references (e.g., `@/components/mobility/wheel.usda@`) are resolved relative to the **USD asset root** (`assets/`). The `UsdComposer` resolves:
- `/`-prefixed paths anchor at the asset root.
- Plain relative paths anchor at the layer's directory.
- URI schemes (`lunco://`, `twin://`) pass through to the `AssetSource`.

See [`56-asset-resolution-and-cache.md`](56-asset-resolution-and-cache.md) for which form to
author, and why a relative `../` escape fails (silently, for `LunCoProgram` source assets).

> [!WARNING]
> **Never try to remove a referenced arc by re-authoring `references =` in an `over`.**
> References **compose**; they do not overwrite. Re-authoring the arc adds a **second** copy
> of the asset onto the same prim — duplicate rigid body, collider and sensors — which
> yields a non-finite raycast origin and panics `avian3d` at load.
>
> To drop a referenced child, deactivate it instead:
> ```usd
> over "GNC" ( active = false ) { }
> ```
> Deactivating a prim drops its whole subtree, which is the intended way to subtract from a
> composed asset.

### Sandbox Editing Tools (UX Bridge)
The `lunco-sandbox-edit` crate provides the interactive layer (palette, gizmo, inspector).
- **Spawning**: `SpawnEntity` command is wired to `UsdOp::AddReference` against the active stage.
  A palette spawn mounts the stage's `defaultPrim` via the **empty-path sentinel**
  (`UsdPrimPath { path: "" }`) — the loader resolves and writes back the concrete
  prim path. USD stays the source of truth for the root prim; deriving
  `/PascalCase(stem)` from the filename silently mounts a non-existent prim (→
  invisible spawn) whenever the stem and its `defaultPrim` disagree (e.g. a
  `*_glb.usda` wrapper whose prim has no `Glb` suffix).
- **Selection root**: a prim that declares `lunco:spawnable = true` — authored or
  *composed from a referenced wrapper* — is tagged `SelectableRoot`, so a click on
  a deep glTF sub-mesh resolves *up* (via `find_selectable`, depth cap 32) to the
  placed model root rather than the clicked leaf. This keeps the transform gizmo on
  the object's authored placement transform instead of dropping it at the world
  origin (a glb leaf carries a ~identity parent-local transform).
- **Manipulation**: The transform gizmo authors `UsdOp::SetTranslate` against the document.
  The default `transform-gizmo-bevy` `mouse_interaction` driver is disabled (Cargo
  `default-features = false`, only `gizmo_picking_backend` kept); drags are driven by
  `drive_gizmo_drag_no_shift`, gated to plain (non-Shift) presses so Shift+click stays
  *select-only* and never arms a grab on the gizmo rendered over the object.
- **Undo**: Reverting a `UsdOp` in the document system automatically updates the 3D world.

| Scheme | Purpose | Resolves to |
|---|---|---|
| (none) | Layer-relative refs **inside a Twin** (co-located terrain, textures) | the layer's own source |
| `lunco://` | **Engine asset library** (rovers, parts, vessels, downloaded binaries) — location-independent ref usable from external Twins | `assets/...`, then `<cache>/...` |
| `twin://<name>/...` | **Internal, runtime-only.** The currently-open Twin's root, keyed by Twin name. Reads an external Twin scene + its co-located assets (fs on native, http on web). Never authored into a file. | the opened Twin folder |

> `lunco://` was previously *reserved* for a future collaborative protocol; it's
> now the engine library scheme. A collaborative/Nucleus-like protocol, if added,
> should pick a distinct scheme (e.g. `lunco-net://`).
>
> The cache is **not addressable**: a scheme pointing at it would bake a
> machine-local location into authored content, and the file would resolve only
> inside our pipeline. Downloaded binaries live at their logical `lunco://`
> address; the `lunco://` reader resolves `assets/` first,
> then the cache. Large binaries still stay out of git — they are *resolved*
> into the library, not *addressed* in the cache. See
> [`56-asset-resolution-and-cache.md`](56-asset-resolution-and-cache.md).
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

### Transform decode

One shared stack (`lunco-usd-bevy`, `local_transform_at`) decodes a prim's local
`Transform`, used by **both** the static load decoder (`read_transform_from_usd` + the
instantiate path) and the per-frame animation sampler, so a static pose and its animated
pose always agree. Precedence:

1. **`xformOpOrder`** (when authored) — honored exactly by `compose_xform_order_at`,
   including op order and `!invert!`. USD is row-vector (`M = S·R·T`, openusd's
   `Matrix4d::from_trs`): the **last** listed op is applied first to the geometry. Op
   matrices are built in glam's column form and right-multiplied, so the standard
   `["translate","rotateXYZ","scale"]` decodes to exactly `Transform{t,r,s}`.
2. **`xformOp:transform`** — a full `matrix4d` decomposed via `read_matrix_transform_at`.
3. **Piecewise fallback** — `xformOp:translate` + rotation + `xformOp:scale`.

Rotation (`local_rotation_at`) covers every USD channel: the six Euler orders
`rotateXYZ`…`rotateZYX`, the quaternion `xformOp:orient` (`quatf`/`quatd`/`quath`), and
single-axis `rotateX/Y/Z`.

### Animation

Authored `timeSamples` drive entities at the current sim time (architecture doc 19 — the
unified time spine). At composition (`flatten_stage`) each attribute's composed
`timeSamples` and the stage `timeCodesPerSecond` are carried onto the flattened scene
(sublayer/reference `LayerOffset`s are baked in by PCP), so animation works on referenced
assets, not just single-layer files. A prim with any animated channel is tagged
`UsdAnimated`; the per-frame samplers then drive:

- **Transform** — the full transform decode above, evaluated at the entity's resolved time.
- **Visibility** — animated `visibility` token (held).
- **Material** — animated `inputs:diffuseColor` / `inputs:opacity` (and geom
  `primvars:displayColor`) into the entity's **`PbrLook`** — the render-free appearance
  intent ([`render-decoupling.md`](render-decoupling.md)). This crate names no material
  type; `lunco-render-bevy` binds `PbrLook` to a real material.

  > An animated prim carries the **`unshared`** opt-out. `PbrLook` materials are cached
  > by content, so an animated `displayColor` re-keys the cache every frame — minting a
  > material per frame and freeing none. That is an unbounded leak that presents as a
  > slow memory climb, not a crash.

An animated rigid body is demoted to `RigidBody::Kinematic` (`lunco-usd-avian`) so the
sampler's writes don't fight the physics solver. Playback is independent of the physics
clock: animated entities bind to a singleton **animation-preview** `TimeDomain`, driven by
the `ControlAnimation` command (API/MCP) and the Inspector **Animation** section
(play / pause / scrub / rate). See [`19-unified-time-and-clock.md`](19-unified-time-and-clock.md)
(T5/T7) for the clock model.

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
- [`19-unified-time-and-clock.md`](19-unified-time-and-clock.md) — time spine + USD animation sampler/transport
- [`00-overview.md`](00-overview.md) — three-tier architecture
- `specs/030-usd-scene-integration` — detailed spec
