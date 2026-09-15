<!-- AUTO-GENERATED. Do not edit by hand.
     Source of truth: the running app's `DiscoverSchema` (GET /api/commands/schema),
     decorated with the `///` docs on each `#[Command]` struct.
     Regenerate: cargo run -p gen-command-docs -- --schema <schema.json> -->

# Command Reference

Every externally callable mutation in LunCoSim is a reflected typed command — an event dispatched through one
bus, reachable from the **HTTP API** (`POST /api/commands`, `{"type":"ExecuteCommand","command":"…","params":{…}}`),
**MCP**, and **rhai** (`cmd("CommandName", #{ … })`). This page is generated from the
**runtime schema** the app itself advertises, so every command below is one you can
actually call, with the fields the deserializer actually accepts. See the
[Scripting Guide](scripting-guide.md) §3 for the rhai `cmd()`/`query()` bridge and the
[API doc](architecture/12-api.md) for the HTTP contract.

**217 commands** across **40** crates. All documented.

> **Regenerate:** dump the schema from a running app, then
> `cargo run -p gen-command-docs -- --schema <schema.json>` (see the tool's `--help`).

## Index


**Scene editing & authoring**

- [`lunco-luncosim-edit-core`](#lunco-luncosim-edit-core) (1 command)
- [`lunco-luncosim-edit-ui`](#lunco-luncosim-edit-ui) (7 commands)
- [`lunco-scene-commands`](#lunco-scene-commands) (8 commands)

**USD / scenes**

- [`lunco-usd`](#lunco-usd) (1 command)

**Modelica modeling & simulation**

- [`lunco-modelica-core`](#lunco-modelica-core) (8 commands)
- [`lunco-modelica-ui`](#lunco-modelica-ui) (33 commands)

**Vessels, mobility & control**

- [`lunco-controller`](#lunco-controller) (4 commands)

**Avatar & possession**

- [`lunco-avatar`](#lunco-avatar) (2 commands)

**Workbench UI & panels**

- [`lunco-ui`](#lunco-ui) (1 command)
- [`lunco-workbench`](#lunco-workbench) (11 commands)

**Scripting & scenarios**

- [`lunco-scripting`](#lunco-scripting) (11 commands)

**Documents & twins**

- [`lunco-doc-bevy`](#lunco-doc-bevy) (10 commands)

**Time & clock**

- [`lunco-time`](#lunco-time) (5 commands)

**Celestial, environment & comms**

- [`lunco-environment`](#lunco-environment) (1 command)

**Terrain**

- [`lunco-terrain-surface`](#lunco-terrain-surface) (8 commands)

**Obstacle fields**

- [`lunco-obstacle-field`](#lunco-obstacle-field) (1 command)

**API & schema**

- [`lunco-api`](#lunco-api) (2 commands)

**Core**

- [`lunco-core`](#lunco-core) (2 commands)

**Other (source location unknown)**

- [`lunco-assets-datasets`](#lunco-assets-datasets) (2 commands)
- [`lunco-avatar-core`](#lunco-avatar-core) (6 commands)
- [`lunco-capture`](#lunco-capture) (4 commands)
- [`lunco-celestial-spatial`](#lunco-celestial-spatial) (3 commands)
- [`lunco-core-session`](#lunco-core-session) (3 commands)
- [`lunco-cosim-core`](#lunco-cosim-core) (3 commands)
- [`lunco-luncosim-core`](#lunco-luncosim-core) (1 command)
- [`lunco-luncosim-ui`](#lunco-luncosim-ui) (2 commands)
- [`lunco-modelica-ui-core`](#lunco-modelica-ui-core) (2 commands)
- [`lunco-scene-authoring`](#lunco-scene-authoring) (6 commands)
- [`lunco-scene-camera`](#lunco-scene-camera) (3 commands)
- [`lunco-scene-catalog`](#lunco-scene-catalog) (2 commands)
- [`lunco-scene-validation`](#lunco-scene-validation) (1 command)
- [`lunco-telemetry`](#lunco-telemetry) (1 command)
- [`lunco-usd-bevy-camera`](#lunco-usd-bevy-camera) (5 commands)
- [`lunco-usd-core`](#lunco-usd-core) (9 commands)
- [`lunco-usd-sim-cosim`](#lunco-usd-sim-cosim) (3 commands)
- [`lunco-usd-viewport-ui`](#lunco-usd-viewport-ui) (18 commands)
- [`lunco-viz`](#lunco-viz) (1 command)
- [`lunco-workbench-core`](#lunco-workbench-core) (9 commands)
- [`lunco-workbench-guided-ui`](#lunco-workbench-guided-ui) (9 commands)
- [`lunco-workspace`](#lunco-workspace) (8 commands)

---

## Scene editing & authoring

### `lunco-luncosim-edit-core` <a id="lunco-luncosim-edit-core"></a>

#### `SetSpawnDiagnostics`

 Enable or disable the Spawn Ghost pipeline trace.

- *defined in:* `crates/lunco-luncosim-edit-core/src/spawn.rs`

| Field | Type | Description |
|---|---|---|
| `enabled` | `bool` |   |

### `lunco-luncosim-edit-ui` <a id="lunco-luncosim-edit-ui"></a>

#### `AcquireDiagnosticVisual`

 Acquire one explicit camera or collider diagnostic.

- *defined in:* `crates/lunco-luncosim-edit-ui/src/diagnostic_visuals.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  Stable entity address.  The API bridge accepts the entity's `api_id`. |
| `kind` | `String` |  `camera` or `collider`. |
| `policy` | `String` |  Presentation policy.  The initial implementation accepts `default`. |

#### `AddCameraHere`

 Capture the active viewport camera's pose as a new `def Camera` prim.

 Authored into [`LayerId::root`] — the authored scene, serialized on Save. A
 captured shot is a durable edit to the twin, unlike the gizmo/waypoint
 interactions that write the ephemeral `runtime` overlay and vanish.

- *defined in:* `crates/lunco-luncosim-edit-ui/src/ui/cinematic.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `Option < String >` |  Prim name for the new camera. `None` picks the first free `View_N`. |

#### `ReleaseDiagnosticVisual`

 Release one opaque diagnostic lease.

- *defined in:* `crates/lunco-luncosim-edit-ui/src/diagnostic_visuals.rs`

| Field | Type | Description |
|---|---|---|
| `lease` | `u64` |  Opaque handle returned by `AcquireDiagnosticVisual`. |

#### `SelectEntity`

 Select an entity by API id — the headless/scriptable equivalent of a
 viewport selection gesture. Drives the same [`SelectedEntities`]
 resource and [`Selected`] highlight the mouse path uses, so the Inspector
 immediately shows that entity's components (Transform, Physics, Shader
 Parameters, …). Pass `entity_id == 0` to clear the selection.

 Selection is an editor concept (it targets the Inspector/gizmo), so this
 command lives in the `ui`-gated selection module — a headless server exposes
 no selection.

- *defined in:* `crates/lunco-luncosim-edit-ui/src/selection.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API-stable global entity ID from `ListEntities`, resolved to the live  Bevy entity by `ApiEntityRegistry`. `0` clears the selection. |
| `extend` | `bool` |  If true, maintains the previous selection and adds this entity to it (like Shift-click) |
| `toggle` | `bool` |  If true, toggles the selection state of the entity (like Cmd/Ctrl-click) |
| `remove_only` | `bool` |  If true, removes this entity without adding it when it is not selected  (the Ctrl+Left-click viewport intent). |

#### `SelectUsdPrim`

 Select a composed USD prim in one explicit open and focused preview.

 The preview lease is part of the identity. A path is not globally unique:
 the running scene and every open editor document can contain the same
 authored path. Resolving only by path can therefore select an entity from
 the wrong document. The handler first validates the lease and then scopes
 the projection lookup to its stage handle and preview root.

- *defined in:* `crates/lunco-luncosim-edit-ui/src/selection.rs`

| Field | Type | Description |
|---|---|---|
| `preview` | `UsdPreviewId` |  The isolated USD preview that owns the selection. |
| `path` | `String` |  Absolute composed USD prim path within that preview's stage. |
| `extend` | `bool` |   |
| `toggle` | `bool` |   |

#### `SetDiagnosticLayers`

 Set a batch of reusable scene diagnostic layers. The layer names are
 generic presentation capabilities; authored Twin policy chooses when to
 request them through Rhai.

- *defined in:* `crates/lunco-luncosim-edit-ui/src/diagnostic_visuals.rs`

| Field | Type | Description |
|---|---|---|
| `enabled` | `bool` |   |
| `layers` | `Vec < String >` |   |

#### `UpdateDiagnosticVisual`

 Replace the target/kind/policy of one lease without creating a second one.

- *defined in:* `crates/lunco-luncosim-edit-ui/src/diagnostic_visuals.rs`

| Field | Type | Description |
|---|---|---|
| `lease` | `u64` |  Opaque handle returned by `AcquireDiagnosticVisual`. |
| `target` | `Option < GlobalEntityId >` |  Optional replacement target. |
| `kind` | `Option < String >` |  Optional replacement kind. |
| `policy` | `Option < String >` |  Optional replacement policy. |

### `lunco-scene-commands` <a id="lunco-scene-commands"></a>

#### `DeleteEntity`

 Delete an entity from the scene.

 The typed verb for "remove this" authors a journaled, replicated, undoable
 runtime-layer edit. Runtime-only prims use `RemovePrim`; base-authored and
 referenced prims use a stronger `active = false` override so the base scene
 and referenced asset remain intact.

 This despawns AND (via [`persist_delete_to_runtime_layer`]) authors the
 corresponding USD edit, which is what makes deletion journaled and undoable.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  Entity to remove. |
| `intent` | `lunco_core :: EditIntent` |  `Persistent` (the default) authors the removal into the document; an  `Interactive` delete is live-only and does not journal. |

#### `DetachJoint`

 Detach a joint by despawning it.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The joint entity to despawn. |
| `intent` | `lunco_core :: EditIntent` |  Persistent (default) authors the joint's removal into the scene's runtime  layer — removing runtime-only prims and deactivating base/composed prims  — so it journals, syncs, and survives reload, before despawning.  Interactive just pops the live joint (a throwaway test), no journal. See  [`lunco_core::EditIntent`]. Omitted by API callers → `Persistent`. |

#### `MoveEntity`

 Move an existing entity to a position in the active physics frame.

 Programmatic equivalent of grabbing the entity with the gizmo and
 dragging it. The handler:
 1. Switches the body to `RigidBody::Kinematic` (if it has a
    `RigidBody`) so Avian treats the new pose as authoritative
    rather than fighting back via integration.
 2. Converts the active-frame target once into the entity's actual parent
    and BigSpace cell/local storage.
 3. Lets the BigSpace physics bridge derive Avian's pose from that one
    authoritative storage write.
 4. Sets a one-tick `LinearVelocity` consistent with the move so
    any joint coupled to a dynamic body propagates the motion.

 Designed for automated tests / MCP tool clients that need to
 drive the world without a mouse. Single-shot — body type stays
 Kinematic until another command (or a gizmo drag-end) restores it.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API-stable global entity ID from `ListEntities`, resolved to the live  Bevy entity by `ApiEntityRegistry`. |
| `translation` | `[f64 ; 3]` |  Target translation in the semantic [`lunco_spatial::ActivePhysicsFrame`].  The concrete BigSpace grid, the entity's actual parent, and the cell/local  split are internal storage details resolved by the observer. The wire  representation is f64 so positions retain precision across API/network  round trips. |

#### `RotateEntity`

 Set an entity's world ORIENTATION — the rotational twin of [`MoveEntity`].

 Reachable as `cmd("RotateEntity", #{entity_id, rotation: [x, y, z, w]})`, or
 `set_world_rotation(id, q)` from the rhai prelude. The quaternion is the same
 `[x, y, z, w]` form `world_rotation(id)` returns and `qrot` consumes, so a
 script can read an orientation, transform it, and write it back without ever
 converting representation.

 The public quaternion is expressed in [`lunco_spatial::ActivePhysicsFrame`], the
 same semantic frame as `MoveEntity`. Rotation is not frame-invariant: a
 rotating body Grid and a rotated assembly parent both change the local
 quaternion that must be stored on the entity. The observer performs that
 hierarchy conversion once.

 Written through `Transform`, never through avian's `Rotation`, for exactly
 the reason `MoveEntity` never hand-writes `Position`:
 `lunco-usd_avian_core::PhysicsBridgeSystems::Read` detects the external
 `Transform` write and derives the physics pose from it (carrying it to
 jointed descendants); a hand-written `Rotation` is a second, wronger opinion
 that the bridge's writeback then undoes. The body is pinned Kinematic for the
 move, as `MoveEntity` does, so the solver treats the new pose as
 authoritative rather than fighting it. When `AngularVelocity` is present,
 the live handler also publishes a bounded one-tick angular pulse so jointed
 bodies receive the rotation; cleanup clears it after the physics step.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API-stable global entity ID from `ListEntities`, resolved to the live  Bevy entity by `ApiEntityRegistry`. |
| `rotation` | `[f64 ; 4]` |  Target world orientation as `[x, y, z, w]`. Normalised on arrival — a  quaternion that has been interpolated or sampled is unit only to float  tolerance, and refusing it would make this fail for poses that are  perfectly usable. A degenerate (near-zero) quaternion IS refused: it  names no orientation, and silently substituting identity would spin the  body to an attitude the caller never asked for. |

#### `SelectSceneEntity`

 Select one live scene entity through the render-free shared selection
 resource. Editor packages may add highlights or gizmos, but authored tools
 only need this canonical entity selection so a later pointer context can
 carry the selected USD path in both interactive and headless runs.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API-stable global entity ID from the live entity registry. `0` clears. |
| `extend` | `bool` |  Retain the current selection and add this entity. |
| `toggle` | `bool` |  Toggle this entity in the current selection. |
| `remove_only` | `bool` |  Remove this entity without adding it. |

#### `SetUsdConnection`

 Author a native USD attribute connection (`connectionPaths`) onto a prim.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  Target entity or prim root. |
| `name` | `String` |  Attribute name (e.g. `inputs:angle` or `inputs:earth_azimuth`). |
| `type_name` | `String` |  Attribute type name (e.g. `float`). Defaults to `float`. |
| `sources` | `Vec < String >` |  Absolute property paths this attribute connects to (e.g. `["/SandboxScene/Skid_Raycast_1/Comms/EarthTrackerController.outputs:az"]`). |

#### `StepPhysics`

 Freeze physics and advance it deliberately, one frame at a time.

 The verb a cutscene or an offline recording wants, and the reason it is NOT
 `SetTimeTransport`: pausing the world clock also stops `FixedUpdate`, so the
 scenario script that paused it never runs again to unpause itself — the shot
 hangs and a recording spools frames forever. A physics hold freezes
 `Time<Physics>` while `Time<Virtual>` (and so the script) keeps running.

 * `{"hold": true}` — freeze the world; the script keeps ticking.
 * `{"steps": 1}` — let exactly one frame of physics through, then re-freeze.
 * `{"hold": false}` — hand the world back to normal simulation.

 Steps only apply while held; queued with nothing holding they are dropped rather
 than banked against an unrelated hold (a terrain bake, say).

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `hold` | `Option < bool >` |  Raise (`Some(true)`) / release (`Some(false)`) the cinematic hold; `None`  leaves it as-is so a step can be sent on its own. |
| `steps` | `Option < u32 >` |  Frames of physics to let through the hold. `None` = 0. |

#### `TransformEntity`

 Set an entity's complete active-frame pose as one scene edit.

 This is the compound counterpart to [`MoveEntity`] and [`RotateEntity`].
 Interactive editors use it when translation and rotation are produced by
 one gesture, so live seating and document persistence share one semantic
 command and one undo/change-set boundary.
 For physics bodies, the live handler publishes bounded one-tick linear and
 angular pulses when the corresponding Avian components are present, allowing
 joint constraints to consume the complete pose edit before cleanup.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API-stable global entity ID from `ListEntities`, resolved to the live  Bevy entity by `ApiEntityRegistry`. |
| `translation` | `[f64 ; 3]` |  Target translation in the explicit active physics frame. |
| `rotation` | `[f64 ; 4]` |  Target orientation in the explicit active physics frame, `[x,y,z,w]`. |

## USD / scenes

### `lunco-usd` <a id="lunco-usd"></a>

#### `SetDomeLight`

 Author the scene's HDRI environment: a `UsdLuxDomeLight` carrying
 `inputs:texture:file`. Projected by `lunco_usd_bevy_light::dome` into a skybox +
 image-based lighting.

 **This is the only way to change the environment at runtime.** It lowers to
 [`UsdOp`]s and goes through [`apply_ops_as_change_set`], so the edit saves,
 journals, undoes as ONE unit, and replicates — exactly like any other USD
 edit. Writing to the `Skybox`/`GeneratedEnvironmentMapLight` components
 directly would light the local viewport and be invisible to all four of
 those, which is the failure mode this command exists to prevent.

 Idempotent: `AddPrim` is a `define_prim`, so re-issuing hot-replaces the
 dome rather than stacking duplicates. Every field is `Option` — `None`
 leaves the authored value alone, so a lighting tweak need not restate the
 texture.

- *defined in:* `crates/lunco-usd/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `Option < DocumentId >` |  Document to author into. `None` = the workspace's active document. |
| `path` | `Option < String >` |  Prim path of the dome. `None` = `/World/Sky`.   It must live **under the stage's `defaultPrim` subtree** (`/World` in  every scene here) — a prim authored outside it composes into the layer  but is never mounted, so the sky would silently not appear. |
| `texture` | `Option < String >` |  `inputs:texture:file` — the HDRI, resolved relative to the stage layer  (e.g. `../hdri/lunar_horizon_2k.hdr`). Equirectangular (`.hdr`, `.png`)  or a `.ktx2` cubemap. |
| `intensity` | `Option < f32 >` |  `inputs:intensity` — multiplier on the image (1.0 = as authored). |
| `exposure` | `Option < f32 >` |  `inputs:exposure` — stops, applied as intensity × 2^exposure. |
| `color` | `Option < [f32 ; 3] >` |  `inputs:color` — linear RGB tint multiplied into the image. |
| `rotation` | `Option < [f32 ; 3] >` |  `xformOp:rotateXYZ`, **degrees** — spins the environment. The usual case  is yaw only (`[0, heading, 0]`). |
| `skybox` | `Option < bool >` |  `lunco:dome:skybox` — `false` lights the scene from the HDRI but leaves  the sky black. The lunar case: real bounce light, no visible sky. |

## Modelica modeling & simulation

### `lunco-modelica-core` <a id="lunco-modelica-core"></a>

#### `AddModelicaComponent`

 Add a sub-component to a class.

- *defined in:* `crates/lunco-modelica-core/src/api/component.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |
| `class` | `String` |   |
| `type_name` | `String` |   |
| `name` | `String` |   |
| `x` | `f32` |   |
| `y` | `f32` |   |
| `width` | `f32` |   |
| `height` | `f32` |   |
| `animation_ms` | `u32` |  Pulse-glow duration in ms. `0` = no animation (instant). |

#### `ApplyModelicaOps`

 Apply a batch of Modelica document operations in one shot.

 Use this command instead of a stream of single-op commands when several
 edits belong together: they land as one undo group, and the document is
 only re-parsed once at the end. This is the structural authoring surface
 for the Modelica document; it does not attach a simulation program to USD.

- *defined in:* `crates/lunco-modelica-core/src/api/mod.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document to edit; unassigned (`0` over the API) = active. |
| `ops` | `Vec < ApiOp >` |  Ops to apply, in order. |

#### `ConnectComponents`

 Add a `connect(a.p, b.q)` equation to a class.

- *defined in:* `crates/lunco-modelica-core/src/api/diagram.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |
| `class` | `String` |   |
| `from` | `String` |   |
| `to` | `String` |   |
| `animation_ms` | `u32` |  Edge-flash duration in ms. `0` = no animation. |

#### `DisconnectComponents`

 Delete the `connect(from, to)` equation joining two component ports. The
 inverse of `ConnectComponents`; a connection that isn't there is a logged
 no-op.

- *defined in:* `crates/lunco-modelica-core/src/api/diagram.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document to edit; unassigned (`0` over the API) = active. |
| `class` | `String` |  Class within the document that owns the connection. |
| `from` | `String` |  Source port, `"<component>.<port>"`. |
| `to` | `String` |  Target port, `"<component>.<port>"`. |

#### `RemoveModelicaComponent`

 Remove a component instance from a class.

 Removes ONLY the declaration. Any `connect(...)` equation still naming the
 component is left behind and will fail to compile, so issue the matching
 `DisconnectComponents` calls FIRST — that is the order the canvas uses
 (orphan-edge removals precede the node removal, so rumoca can still resolve
 the connect spans). Batch both through `ApplyModelicaOps` to keep them in
 one undo group.

- *defined in:* `crates/lunco-modelica-core/src/api/component.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document to edit; unassigned (`0` over the API) = active. |
| `class` | `String` |  Class within the document that declares the component. |
| `name` | `String` |  Component instance name to remove. |

#### `RenameModelicaClass`

 Rename a top-level class within an open Modelica document.

- *defined in:* `crates/lunco-modelica-core/src/api/class.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |
| `old_name` | `String` |   |
| `new_name` | `String` |   |

#### `SetDocumentSource`

 Replace an open document's entire source text.

- *defined in:* `crates/lunco-modelica-core/src/api/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |
| `source` | `String` |   |

#### `SetModelInput`

 Push a runtime input value into a compiled model's stepper.

 This command is owned by the UI-free Modelica core, so the same reflected
 command is available to headless API hosts, the workbench, Rhai, and any
 future transport. Its observer queues the exclusive port/model write using
 the same helper as the canvas path and reports the actual apply result.

- *defined in:* `crates/lunco-modelica-core/src/model_commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document id; zero selects the documented active-document default. |
| `name` | `String` |  Declared Modelica input name. |
| `value` | `f64` |  Runtime input value. |

### `lunco-modelica-ui` <a id="lunco-modelica-ui"></a>

#### `AddCanvasPlot`

 Drop a "Scope" plot onto the active canvas.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/diagram.rs`

| Field | Type | Description |
|---|---|---|
| `x` | `f32` |   |
| `y` | `f32` |   |
| `width` | `f32` |   |
| `height` | `f32` |   |
| `signal` | `String` |   |

#### `AddSignalToPlot`

 Add one signal to an existing plot panel.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/plot.rs`

| Field | Type | Description |
|---|---|---|
| `plot` | `u64` |  `VizId` of the target plot panel. |
| `signal` | `String` |  Signal name to add. |

#### `AutoArrangeDiagram`

 Lay the class's components out on a deterministic grid and persist the
 positions as one undo-able batch of `SetPlacement` ops — Dymola's
 **Edit → Auto Arrange**. The passive open-time fallback stacks components at
 the origin, so this is how an imported model gets a readable diagram.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document to arrange; unassigned (`0` over the API) = active. |

#### `CancelExperiment`

 Cancel in-flight batch run(s). Signals the runner's cancel flag, which is
 honored at compile boundaries and on every solver step; the run then ends
 `Cancelled`. Target a specific run by `experiment_id`, or set `all`.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `experiment_id` | `Option < String >` |  Cancel one run by id (uuid string). Ignored when `all` is set. |
| `all` | `bool` |  Cancel every in-flight run. |

#### `CompileModel`

 Compile a document: rumoca front-end → DAE → simulator setup. Idempotent —
 an already-compiled, unmodified model skips the worker dispatch unless
 `force`. Never changes `paused`; type/parse/DAE errors land in
 `WorkbenchState.compilation_error` and surface in the Diagnostics panel.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  The document to compile. Unassigned (`0` over the API) means the  **active** document, which is what a toolbar click and a headless  `cmd("CompileModel", #{})` both want. |
| `class` | `Option < String >` |  Optional explicit target class. When `Some`, bypass both the  drilled-in pin and the picker — compile this exact class.  Used by API callers that need deterministic behaviour without  a GUI (cf. spec 033 User Story 1.5). |
| `force` | `bool` |  Force a recompile even if the model is already compiled and  clean (same document generation). Defaults to `false` so a  Compile on an up-to-date model is an idempotent no-op. |
| `resume_after_compile` | `bool` |  When `true`, the post-compile success handler unpauses the model  so it starts live-stepping the instant the stepper is installed.  Set by `RunActiveModel` ("Run live") so a single click compiles  *and* plays — crucially including the first-ever compile, where  no model entity yet exists to carry the resume intent. Defaults  to `false`: a plain Compile leaves the model paused/ready. |

#### `ConfirmClassPicker`

 Confirm (or dismiss) the "Which class should Compile/Fast Run …?" picker
 modal that appears when a package has more than one runnable model. This is
 the headless/API equivalent of clicking the dialog's button: it mirrors the
 confirm path in [`render_compile_class_picker`] exactly — pin the chosen
 class as the doc's drilled-in class (so resolution skips the picker), close
 the dialog, and re-dispatch the original Compile / Fast Run for the pick.

 - `qualified` `None` → use the dialog's pre-selected candidate.
 - `qualified` set    → pick that class (must be one of the candidates).
 - `cancel` `true`    → just close the dialog without running.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `qualified` | `Option < String >` |  Class to pick. `None` = the dialog's pre-selected candidate. |
| `cancel` | `bool` |  Dismiss the picker without running (same as the Cancel button). |

#### `CreateNewScratchModel`

 Request to create a new untitled Modelica model and open its tab.

 Both fields default to `None` for the plain "New model" entry points
 (File ▸ New, the package browser, the welcome screen). The URL-share
 loader (`crate::model_share`) fires this with `source`/`name`
 populated so a shared model reuses this exact creation + tab-open
 path instead of duplicating it.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `source` | `Option < String >` |  Initial source. `None` → a minimal `model <name> end <name>;` stub. |
| `name` | `Option < String >` |  Display name, deduplicated against existing in-memory models.  `None` → the model name parsed from `source`, else an  auto-incremented "Untitled". |

#### `DeleteExperiment`

 Remove experiment record(s) from the registry. Terminal runs only —
 in-flight runs (via id / `all`) are skipped; cancel them first. Scope by
 `experiment_id`, `doc` (every run for that doc's twin), or `all`.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `experiment_id` | `Option < String >` |   |
| `doc_id` | `Option < DocumentId >` |   |
| `all` | `bool` |   |

#### `DuplicateModelFromReadOnly`

 Duplicate a read-only (library) model into a new editable Untitled
 document. Unassigned `source_doc_id` (`0` over the API) means the active
 document.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `source_doc_id` | `DocumentId` |   |

#### `FastRunActiveModel`

 Fast Run — compile + simulate end-to-end off-thread (Web Worker on
 wasm, std::thread on native). The result is stored as an Experiment
 in [`lunco_experiments::ExperimentRegistry`]. See
 `docs/architecture/25-experiments.md`.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |
| `class` | `Option < String >` |  Target class. When `None`, resolves via drilled-in class or picker. |
| `t_end` | `Option < f64 >` |  Override experiment StopTime (seconds). `None` = use annotation or fallback. |
| `dt` | `Option < f64 >` |  Override output interval / step (seconds, Modelica `Interval`). `None`  = use annotation or fallback. Mutually exclusive with `n_intervals`. |
| `n_intervals` | `Option < u32 >` |  Override output point count as a number of intervals (Modelica  `NumberOfIntervals`): emits `n + 1` evenly-spaced samples. The count  alternative to `dt`; when set it takes precedence and clears `dt`. |
| `tolerance` | `Option < f64 >` |  Override solver tolerance. `None` = use annotation or fallback. |
| `solver` | `Option < String >` |  Pin the solver to a registered id — `ListSolvers` enumerates them and is  the only vocabulary accepted. An unregistered id fails the run rather  than falling back, so a typo cannot silently produce numbers from a  different backend. `None`/`"auto"` lets the resolver pick from where the  run executes. |
| `h0` | `Option < f64 >` |  Override the solver's initial step (seconds). `None` = the backend's  span-based default. A diagnostic for long-horizon runs that fail at a  stiff transient near `t₀`. |

#### `FitCanvas`

 Zoom and pan the canvas so the whole diagram fits the viewport.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document to fit; unassigned (`0` over the API) = active. |

#### `FocusComponent`

 Centre the canvas on one named component — how a screenshot or a review
 walkthrough targets a specific part of a large diagram.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document to focus in; unassigned (`0` over the API) = active. |
| `name` | `String` |  Component instance name as it appears in the diagram. |
| `padding` | `f32` |  Margin in canvas units to leave around the component. |

#### `FormatDocument`

 Run rumoca-tool-fmt on the active document.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |

#### `GetFile`

 Read a file's text and echo it to the log between `-- BEGIN --` /
 `-- END --` markers. A diagnostic for API callers that cannot see the host
 filesystem — it does NOT open a document (use `Open` for that). Goes through
 `lunco-storage`, so it works in the browser build too.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Path to read, resolved the same way document sources are. |

#### `InspectActiveDoc`

 Dump the active document's registry state to the log — id, source length,
 parse status, linked entities. A debugging verb for "what does the app
 actually think is open?", taking no parameters because it always targets
 whatever the user is looking at.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/inspect.rs`
- *fields:* none — call with `InspectActiveDoc` (no params)

#### `MoveComponent`

 Move a component instance in the diagram.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/diagram.rs`

| Field | Type | Description |
|---|---|---|
| `class` | `String` |   |
| `name` | `String` |   |
| `x` | `f32` |   |
| `y` | `f32` |   |
| `width` | `f32` |   |
| `height` | `f32` |   |

#### `NewPlotPanel`

 Open a new plot panel. With `source` set it duplicates that plot's signal
 bindings and picked series — "open another view of this, then diverge" —
 otherwise it starts from `signals`.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/plot.rs`

| Field | Type | Description |
|---|---|---|
| `title` | `String` |  Panel title. Empty = derived from `source` (`"<title> (copy)"`), or a  default when there is no source. |
| `signals` | `Vec < String >` |  Signal names to plot initially. |
| `source` | `u64` |  `VizId` of a plot to clone bindings from. `0` = start empty. |

#### `Open`

 Unified open command — dispatches on the URI scheme.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `uri` | `String` |   |

#### `OpenInNewView`

 Open the same document in a new tab (split / sibling view).

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |

#### `PanCanvas`

 Pan the diagram canvas by an offset, leaving zoom alone.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document to pan; unassigned (`0` over the API) = active. |
| `x` | `f32` |  Horizontal offset in canvas units. |
| `y` | `f32` |  Vertical offset in canvas units. |

#### `PauseActiveModel`

 Run-control events — fire against `doc_id=0` to target the active
 document, or a specific `DocumentId.raw()` for automation.

 Simulation already ticks automatically once a model is compiled
 (see `spawn_modelica_requests` — steps every `FixedUpdate` unless
 `ModelicaModel.paused`). These commands are the user-facing
 handles on that loop:

  * [`PauseActiveModel`]  — freeze stepping without tearing down
    worker state. `paused = true`.
  * [`ResumeActiveModel`] — thaw from paused. `paused = false`.
  * [`ResetActiveModel`]  — send `ModelicaCommand::Reset` to the
    worker so it rebuilds the stepper from the cached DAE and
    zeroes `current_time`. Cheap — no recompile.

 A separate Step-one-frame command is intentionally deferred until
 #59 (named experiments / Runs panel) lands — the infrastructure
 for a "force one step" flag is better designed alongside that.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |

#### `Redo`

 Redo the most recently undone edit.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |

#### `RenameExperiment`

 Rename an experiment run in the [`ExperimentRegistry`]. Mirrors
 `DeleteExperiment`'s id-as-string addressing so the same value the UI
 holds (and API callers pass) resolves the run.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `experiment_id` | `String` |  Target run id (the `ExperimentId`'s inner value as a string). |
| `name` | `String` |  New display name. |

#### `ResetActiveModel`

 See [`PauseActiveModel`].

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |

#### `RestartActiveModel`

 Reset to `t=0` and run again. Composition of [`ResetActiveModel`]
 followed by [`RunActiveModel`].

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |

#### `ResumeActiveModel`

 See [`PauseActiveModel`].

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |

#### `RunActiveModel`

 Start a live realtime simulation: compile-if-stale, then play.

 This is the user-facing "Run" verb. If the model is already
 compiled and clean (same document generation), it simply unpauses —
 no recompile. Otherwise it sets [`ModelicaModel::resume_after_compile`]
 and triggers a [`CompileModel`]; the post-compile success handler in
 the worker then unpauses, so play begins as soon as the stepper is
 installed. Contrast with [`CompileModel`] (compile only, never auto-
 starts) and [`ResumeActiveModel`] (unpause only, never compiles).

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |
| `class` | `Option < String >` |  Optional explicit target class, forwarded to the compile. |

#### `RunExperiment`

 Define + dispatch a batch experiment with explicit parameter overrides,
 inputs, and bounds — the programmatic counterpart to the Experiments
 panel. Unlike `FastRunActiveModel`, overrides come from the command (not
 the UI draft), so an agent can sweep parameters without touching source.
 Discover the resulting `experiment_id` via `ListRuns` (newest, or by
 `label`); read the trajectory with `GetExperimentResult`.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Target document. Unassigned → the active document. |
| `class` | `Option < String >` |  Target class. `None` → drilled-in class or sole non-package class. |
| `overrides` | `Vec < crate :: api :: ApiModification >` |  Parameter overrides `[{name, value}]` (e.g. `{name:"Isp", value:"300"}`). |
| `inputs` | `Vec < crate :: api :: ApiModification >` |  Runtime input overrides `[{name, value}]`. |
| `t_start` | `Option < f64 >` |   |
| `t_end` | `Option < f64 >` |   |
| `dt` | `Option < f64 >` |  Output step in seconds (Modelica `Interval`). Mutually exclusive with  `n_intervals`. |
| `n_intervals` | `Option < u32 >` |  Output point count as a number of intervals (Modelica  `NumberOfIntervals`); takes precedence over `dt` when set. |
| `tolerance` | `Option < f64 >` |   |
| `solver` | `Option < String >` |  Pin the solver to a registered id — `ListSolvers` enumerates them and is  the only vocabulary accepted. An unregistered id fails the run rather  than falling back, so a typo cannot silently produce numbers from a  different backend. `None`/`"auto"` lets the resolver pick from where the  run executes. |
| `h0` | `Option < f64 >` |  Override the solver's initial step (seconds). `None` = the backend's  span-based default. A diagnostic for long-horizon runs that fail at a  stiff transient near `t₀`. |
| `label` | `Option < String >` |  Optional run name (shown in ListRuns). Defaults to auto "Run N". |

#### `SaveActiveDocument`

 Save the document — the one save verb, in-process and over the API alike.
 Unassigned `doc_id` (`0` over the API) means the active document.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |

#### `SaveActiveDocumentAs`

 Save the document to `path`. Unassigned `doc_id` means the active document.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |
| `path` | `String` |   |

#### `SetViewMode`

 Switch how a document is rendered — source text, diagram canvas, icon, or
 documentation.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document to switch; unassigned (`0` over the API) = active. |
| `mode` | `String` |  One of `"text"`, `"diagram"`, `"icon"`, or `"docs"`. Anything else  leaves the mode unchanged. |

#### `SetZoom`

 Set the diagram canvas zoom factor directly, bypassing scroll-wheel steps —
 for scripted captures that need a repeatable framing.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document whose canvas to zoom; unassigned (`0` over the API) = active. |
| `zoom` | `f32` |  Zoom factor, `1.0` = 100%. |

#### `Undo`

 Undo the most recent edit on the active document.

- *defined in:* `crates/lunco-modelica-ui/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |   |

## Vessels, mobility & control

### `lunco-controller` <a id="lunco-controller"></a>

#### `InjectWindowInput`

 Inject one native-style input event into the local application window.

 This is the generic automation boundary for Rhai, API clients, playback,
 and accessibility tooling. It does not invoke a scene tool or semantic
 command directly. Instead, the next input phase receives the same Bevy
 `WindowEvent` plus typed keyboard/mouse messages that the winit backend
 normally emits, so every existing consumer follows its ordinary path.

- *defined in:* `crates/lunco-controller/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `event` | `WindowInputEvent` |   |

#### `SetControlPath`

 Declare that commands can (or cannot) currently reach `target`.

 The generic verb behind [`lunco_core_session::ControlPathRegistry`]. A mission
 script computes the DOMAIN fact and states the CONSEQUENCE here; an authored
 policy ([`lunco_core_session::AUTHORIZE_HOOK`]) then decides what to refuse.
 Space School does exactly that — `ss3_radio_shadow.rhai` reads real link geometry
 with `can_reach(radio, "earth")` and calls this — which keeps doc 49's split one
 layer up: the kernel computes geometry, the script decides what it means, and
 nothing in Rust ever concludes "no link ⇒ no control" (a store-and-forward
 mission would disagree).

 It lives here rather than in `lunco-core` for a mechanical reason: `#[Command]`
 expands to `lunco_core::…` paths, so a command cannot be declared inside that
 crate. Beside `drive_from_bindings` is the right second choice — this is the path
 it gates.

- *defined in:* `crates/lunco-controller/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The vessel commands cannot reach. |
| `down` | `bool` |  `true` ⇒ commands do not reach `target`. |

#### `SimulateIntent`

 Force an intent held or released, as if a key were pressed — the headless way to
 drive a possessed vessel over the API or from rhai.

 `held = true` is "stuck" (the key is down and stays down); `held = false` is
 "unstuck" (released). This command remains the level-triggered surface for
 driving a held control value. Use [`SimulateIntentEdge`] for an atomic
 momentary press/release or pulse. The named intent is the USD control
 vocabulary (`forward`, `action`, `yaw_left`, …), parsed by
 [`lunco_core::parse_user_intent`], so it matches whatever a vessel's
 `Controls` profile binds.

- *defined in:* `crates/lunco-controller/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `intent` | `String` |  Intent name (`forward`, `backward`, `left`, `right`, `yaw_left`, `yaw_right`,  `action`, `release`, …). |
| `held` | `bool` |  `true` = hold it down, `false` = release it. |
| `target` | `Entity` |  The **entity this intent drives** (normally a vessel or avatar command  surface). An intent is meaningless without its target: two spawns of one  asset are two distinct entities, and a targetless intent is rejected. Over  the API this takes the target's `api_id` — the `GlobalEntityId` reported by  `ListEntities` — and is resolved to the live entity. |

#### `SimulateIntentEdge`

 Deliver one atomic target-scoped semantic edge without requiring callers to
 emulate a pulse with ordered `held: true` / `held: false` commands.

 This is the API/Rhai/network entry point. The handler validates the shared
 intent vocabulary and emits [`lunco_core::SemanticIntentEdge`]; it does not
 decide which port or mechanism the consuming Twin should actuate.

- *defined in:* `crates/lunco-controller/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The entity whose semantic control surface receives the edge. |
| `intent` | `String` |  Intent name (`action`, `release`, `forward`, …). |
| `edge` | `String` |  `pressed`, `released`, or `pulse`. |

## Avatar & possession

### `lunco-avatar` <a id="lunco-avatar"></a>

#### `InspectVessels`

 Diagnostic read-out of every **commandable** vessel's *control authority* state —
 the chain that decides whether the stick actually flies it:
 `GlobalEntityId` (needed for ownership + the model's `piloted` sensor),
 `ControlBinding` (intent→port map from the USD `Controls` scope), and whether
 the `SessionRegistry` currently records an owner (⇒ `piloted = 1`). Logs one
 `[inspect]` line per vessel at INFO. API-driven: `{"type":"ExecuteCommand","command":"InspectVessels"}`.

- *defined in:* `crates/lunco-avatar/src/lib.rs`
- *fields:* none — call with `InspectVessels` (no params)

#### `ShowNotification`

 Show a transient on-screen notification (toast) to the player.

 Pushes onto the [`crate::ScreenNotifications`] resource; the optional
 `lunco-avatar-ui` adapter renders active toasts top-center and fades them
 out. Headless hosts accept the command (and log it) but draw nothing. Fired
 from rhai via `notify(msg)` / `notify_kind(msg, kind)` (see the prelude) so a
 scenario can announce each phase without touching Rust.

- *defined in:* `crates/lunco-avatar/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `text` | `String` |  The message text. |
| `kind` | `String` |  Visual style: "info" (default), "success", "warn", or "error". |
| `secs` | `f32` |  Seconds to display; `0` uses the default (~4.5s). |

## Workbench UI & panels

### `lunco-ui` <a id="lunco-ui"></a>

#### `CloseModal`

 Dismiss the currently displayed modal without closing the application.

 This is intentionally separate from CloseWindow: external API/Rhai
 callers must be able to release a UI consent dialog while the simulation,
 network API, and download tasks continue running.

- *defined in:* `crates/lunco-ui/src/modal/mod.rs`
- *fields:* none — call with `CloseModal` (no params)

### `lunco-workbench` <a id="lunco-workbench"></a>

#### `CloseWindow`

 Close the primary window (sends `AppExit::Success`).

- *defined in:* `crates/lunco-workbench/src/window_command.rs`
- *fields:* none — call with `CloseWindow` (no params)

#### `CopyShareLink`

 Produce a shareable link for the active document and copy it to the
 clipboard.

 Like [`OpenFile`], this is a typed shell command whose behaviour is
 domain-specific and lives in the domain crate
 (`lunco-modelica-core` encodes the active model's source into a URL
 fragment). The headless HTTP API exposes the read-only `GetShareLink`
 query separately; it returns the URL in its `data` payload instead of
 touching a clipboard.

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`
- *fields:* none — call with `CopyShareLink` (no params)

#### `MaximizeWindow`

 Maximize / restore the primary OS window. `maximized = None`
 toggles based on [`WindowMaximized`].

- *defined in:* `crates/lunco-workbench/src/window_command.rs`

| Field | Type | Description |
|---|---|---|
| `maximized` | `Option < bool >` |   |

#### `MinimizeWindow`

 Minimize the primary OS window.

- *defined in:* `crates/lunco-workbench/src/window_command.rs`
- *fields:* none — call with `MinimizeWindow` (no params)

#### `SaveAll`

 Save every open document in the current session.

 Documents with a writable canonical path are written via their
 owning domain's [`SaveDocument`](lunco_doc_bevy::SaveDocument)
 observer. Untitled documents are written into the active Twin using
 their workspace title; with no active Twin their domain's normal Save-As
 picker is used.

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`
- *fields:* none — call with `SaveAll` (no params)

#### `SaveAsTwin`

 Promote the current session into a Twin at `folder`.

 Writes `twin.toml`, saves every open document into the new root, and
 declares the first open USD document as the default scene. Empty
 `folder` triggers a folder picker.

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`

| Field | Type | Description |
|---|---|---|
| `folder` | `String` |  Target folder for the new Twin's `twin.toml`. Empty triggers  the picker. |

#### `SetTheme`

 Set or toggle the active theme mode. Omit `mode` to toggle.

- *defined in:* `crates/lunco-workbench/src/theme_command.rs`

| Field | Type | Description |
|---|---|---|
| `mode` | `Option < String >` |  `"dark"` / `"light"` (case-insensitive). When `None`, toggles. |
| `persist` | `Option < bool >` |  `false` = apply for this session only, leave `settings.json` alone.  Default `true` (the historical behavior). |

#### `ShowOpenFilePicker`

 Request a system "Open File" dialog.

 Dispatches [`ShowOpenFilePicker`] which triggers the picker via
 [`lunco_workbench_file_dialog::PickHandle`]. On success, the file dialog resolves to
 [`OpenFile`] with the chosen path.

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`
- *fields:* none — call with `ShowOpenFilePicker` (no params)

#### `ShowOpenFolderPicker`

 Request a system "Open Folder" dialog.

 Dispatches [`ShowOpenFolderPicker`] which triggers the picker via
 [`lunco_workbench_file_dialog::PickHandle`]. On success, the file dialog resolves to
 [`OpenFolder`] with the chosen path.

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`
- *fields:* none — call with `ShowOpenFolderPicker` (no params)

#### `ToggleInputOverlay`

 Command to toggle the input overlay visibility.

- *defined in:* `crates/lunco-workbench/src/input_overlay.rs`

| Field | Type | Description |
|---|---|---|
| `enabled` | `bool` |  `true` to show the overlay, `false` to hide it. |

#### `TogglePerfHud`

 Flip the perf HUD on/off. Persisted via `lunco-settings`.

- *defined in:* `crates/lunco-workbench/src/perf_hud.rs`

| Field | Type | Description |
|---|---|---|
| `enabled` | `bool` |  `true` enables the HUD; `false` hides it. |

## Scripting & scenarios

### `lunco-scripting` <a id="lunco-scripting"></a>

#### `RegisterTimeline`

 Save a named mission **timeline** to the Twin — the storage counterpart of
 `RunTimeline` (which runs an inline one). Validates the JSON parses as a
 timeline, stores it in the [`crate::timelines::TimelineStore`], and mirrors it
 to `<twin>/timelines/<name>.json` so it survives a restart (reloaded by the
 `TwinAdded` observer). Discover with `ListTimelines`/`GetTimeline`, run with
 `RunStoredTimeline`. Idempotent (re-registering a name replaces it).

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |   |
| `timeline` | `String` |  JSON: a steps array, or an object with a `steps` array (and optional `name`). |

#### `RegisterToolLibrary`

 Register (or hot-replace) a named rhai **tool library** — a reusable bundle
 of selection / behaviour policy callable from any scenario as
 `name::fn(...)` (see [`crate::tool_libs`]). The scenario-authoring counterpart
 to RunScenario: RunScenario attaches a program to ONE entity; this publishes
 shared library code every scenario can call, with no Rust rebuild. Idempotent
 + hot-reload — re-registering a name replaces it and the runtime picks it up
 on the next tick.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |   |
| `source` | `String` |   |

#### `RunRhai`

 Run a rhai snippet against the live world — the scripting escape hatch when
 no typed command covers what you need.

 The result arrives on the next `Update`: rhai needs full `World` access,
 which an observer cannot hold, so the handler enqueues the snippet and the
 exclusive `drain_world_scripts` system runs it before answering the
 deferred API request with the real stdout. `Update` is intentional because
 kinematic celestial warp freezes `FixedUpdate`.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `code` | `String` |  rhai source to evaluate. The scripting prelude is in scope. |

#### `RunRhaiTool`

 Invoke a registered Rhai tool with a typed value.

 This is the structured counterpart to [`RunRhai`]. It is intended for
 engine adapters such as scene click tools: the payload crosses the Bevy
 command queue as the shared [`TelemetryValue`] model and becomes a native
 Rhai value inside the scripting backend. No source snippet or JSON literal
 is used to carry the payload.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `tool` | `String` |  Registered tool namespace, for example `recover` or `waypoint_editor`. |
| `args` | `TelemetryValue` |  Structured argument passed as the single `on_click(context)` argument. |

#### `RunRhaiToolHook`

 Invoke any one-argument hook exposed by a registered Rhai tool.

 This is the generic interaction seam used by authored pointer policies and
 menus. The hook name is validated against the tool registry before it is
 queued; the payload remains a typed [`TelemetryValue`] until the Rhai
 adapter creates its native value.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `tool` | `String` |  Registered tool namespace, for example `waypoint_editor`. |
| `hook` | `String` |  One-argument function in that namespace, without `/1`. |
| `args` | `TelemetryValue` |  Structured argument passed to the hook. |

#### `RunScenario`

 Attach a persistent rhai scenario to an entity — the scenario-loading entry
 point for the API / MCP / UI / ROS2. Registers the source as a
 `ScriptDocument` and attaches a `ScriptedModel { Rhai }` to `target`, so the
 per-entity runtime can build a native `task(me)` tree and run optional
 lifecycle/event hooks.

 Idempotent + HOT-RELOAD: re-running on an entity that already has a scenario
 reuses its document id and bumps the generation, so `tick_rhai_models`
 recompiles in place (state reset) instead of leaking documents.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |   |
| `source` | `String` |   |
| `params` | `String` |  Optional scenario parameters as a JSON object string (e.g.  `{"speed":1.5,"target":"rover_b"}`), readable in the script as the  `params` constant. Omitted → none. |
| `reload_policy` | `ScenarioReloadPolicy` |  Behavior of this scenario when the active scene is replaced. `retain`  keeps a stable orchestration host alive; `restart` runs `on_start` again  after the replacement is ready. |

#### `RunScenarioAsset`

 Attach a file-backed Rhai scenario to an entity. The asset is loaded through
 the normal Bevy asset graph, so imports, Twin ownership, wasm, and hot reload
 use the same path as USD-authored scenarios. This is the generic launch seam
 for authored flows; no domain-specific catalog or host is required.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |   |
| `source_asset` | `String` |  Root-qualified script asset (`lunco://...` or `twin://...`). |
| `params` | `String` |  Optional scenario parameters as a JSON object string. |
| `scene_asset` | `String` |  Optional scene asset to request before the scenario starts. The scene  transition remains owned by the USD scene command layer; this field  only composes the generic scenario-launch request with that lifecycle. |
| `reload_policy` | `ScenarioReloadPolicy` |  Lifecycle behavior when the active scene is replaced. |

#### `RunStoredTimeline`

 Run a stored mission timeline on an entity by name (resolved from the
 [`crate::timelines::TimelineStore`]) — the one-step "fetch + run" for a
 `RegisterTimeline`d / file-authored mission, sparing callers a
 `GetTimeline`→`RunTimeline` round-trip. Same execution path as `RunTimeline`.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |   |
| `name` | `String` |   |

#### `RunTimeline`

 Run a declarative **mission timeline** on an entity — Layer 2 of the
 sequencer. The timeline is pure DATA (`timeline` is a JSON string: either a
 `[ ...steps ]` array or `{ "name": ..., "steps": [ ... ] }`), so a mission is
 authorable/storable/shippable without writing rhai. The handler lowers it to
 a generated `task(me)` source that calls the prelude's `compile_timeline`
 and hands the resulting tree to the native behavior kernel. It attaches via
 the same path as `RunScenario` — so hot-reload, per-entity state, and
 `TASK_COMPLETE`/`TASK_FAILED` telemetry all come from the native task driver.

 Step vocabulary (see prelude `timeline_step`): `{move_to,speed,radius}`,
 `{move_to_entity,speed,radius}`, `{possess}`, `{brake,secs}`,
 `{cmd,params}`, `{emit,value}`, `{wait}`, `{wait_event}`. Each step must
 contain exactly one operation field; the operation word is the timeline
 discriminator and common fields are validated separately below.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |   |
| `timeline` | `String` |  JSON: a steps array, or an object with a `steps` array (and optional `name`). |

#### `SetScenarioPaused`

 Pause or resume the scenario attached to `target` (sets `ScriptedModel.paused`).
 Paused scenarios skip fixed-step task/lifecycle execution (rhai) or backend
 execution (python) but keep their state — resume continues where they left
 off. The clean API form of toggling the `paused` field; language-agnostic.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |   |
| `paused` | `bool` |   |

#### `StopScenario`

 Stop & detach the scenario from `target` — removes its `ScriptedModel` so it
 stops ticking. A rhai scenario runs its `on_stop` teardown hook on the next
 runtime tick (the prune in `tick_rhai_models`). The `ScriptDocument` stays in
 the registry, so the scenario can be re-attached / re-run later.

- *defined in:* `crates/lunco-scripting/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |   |

## Documents & twins

### `lunco-doc-bevy` <a id="lunco-doc-bevy"></a>

#### `CloseDocument`

 Request to remove the document from its registry (and any linked
 runtime state — entities, caches).

 Handled per-domain: the owning registry calls its remove-document
 path, which fires [`DocumentClosed`]. Foreign domains ignore the
 trigger. Idempotent — closing a non-existent or already-closed
 document is a no-op.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  The document to close. |

#### `DiscardDocument`

 Explicitly discard the current document state and restore its file source.

 The owning domain performs the file read asynchronously, then resets the
 document and its undo history through its registry. Untitled documents are
 closed because they have no file source to restore; read-only origins are
 rejected by the owner.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  The document whose local edits should be discarded. |

#### `ForkDocument`

 Fork an open file-backed document into an independently editable untitled
 document.

 The owning domain copies its typed authored state and history through its
 [`lunco_doc::ForkableDocument`] implementation. The fork has no file
 identity until [`SaveAsDocument`] assigns one, so two open documents cannot
 save over the same path.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `source_doc_id` | `DocumentId` |  Existing document to snapshot. |
| `name` | `String` |  Untitled name shown until Save-As. |

#### `NewDocument`

 Create a new untitled document of the given kind.

 `kind` is the registered `DocumentKindId` string (`"modelica"`,
 `"julia"`, `"usd"`, …). An **empty** `kind` is the "use the
 default" signal — the workbench-side observer looks up the registry,
 picks the first kind whose `can_create_new` is true, and re-fires
 this command with the resolved kind. That's how Ctrl+N reaches a
 sensible default without the keybind owner having to know which
 domain crates are loaded.

 Domain crates add observers that gate on `cmd.kind == "<their_id>"`
 and create the actual document. The workbench's default observer only
 handles the empty-kind resolution.

 Lives here (not in the egui workbench) so headless / sandbox / server
 binaries can dispatch document creation by `kind` without the UI
 shell — the picker-driven path is a workbench concern, the typed verb
 is a document-lifecycle concern.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `kind` | `String` |  Registered document kind id, or empty for "default". |

#### `OpenFile`

 Open a file at `path` into a new tab.

 Empty `path` triggers a native Open-File picker (a workbench concern)
 and re-fires this command with the chosen path on success. A
 **non-empty** `path` skips the dialog — that's how HTTP automation,
 recents, drag-drop, and headless / server callers reach the same code
 path without any UI.

 The actual loading is domain-specific: `lunco-modelica-core` observes this
 and reads `.mo` files; `lunco-usd` observes it for `.usd*`. Each
 domain's observer ignores paths it doesn't own, so they coexist.

 Lives here (not in the egui workbench) so headless / sandbox / server
 binaries can open files by path; only the empty-path picker dispatch
 stays in the workbench.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Filesystem path or URI (`bundled://`, `mem://`). Empty triggers  the picker (workbench only). |

#### `RedoDocument`

 Request to redo the last undone history group on the document.

 Counterpart of [`UndoDocument`]. Same per-domain dispatch rules.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  The document whose most recent undone history group should be re-applied. |

#### `RenameOpenDocument`

 Rename an open document identified by its workspace document id.

 The document layer owns this payload because it addresses a document even
 when that document is an untitled draft. Windowed hosts may observe it and
 route filesystem-backed documents into their workspace rename flow.

- *defined in:* `crates/lunco-doc-bevy/src/rename.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `lunco_doc :: DocumentId` |  The document to rename. |
| `new_name` | `String` |  New filename or class identifier; path separators are not accepted. |

#### `SaveAsDocument`

 Request the owning domain persist the document **to a new location**.

 `path` semantics mirror [`OpenFile`]:

 - **Empty** → the observer fires
   [`lunco_workbench_file_dialog::PickHandle`](../lunco_workbench_file_dialog/struct.PickHandle.html)
   with `PickFollowUp::SaveAs(doc)` and returns. The workbench's
   `on_pick_resolved` re-fires this command with the chosen path
   filled in. Cancellation is silent.
 - **Non-empty** → the observer writes directly, rebinds the
   document's [`lunco_doc::DocumentOrigin`] to the new writable `File` variant,
   updates `last_saved_generation`, and fires [`DocumentSaved`].

 This single shape covers UI dialogs, recents, drag-drop, HTTP
 automation, and the Untitled-promotion path (Ctrl+S on a draft
 routes to `SaveAsDocument { doc, path: "" }`).

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  The document to persist. |
| `path` | `String` |  Target path. Empty triggers the picker. |

#### `SaveDocument`

 Request to persist the document's current source to disk.

 Handled per-domain: the owning registry resolves the document's
 canonical path, writes the source, and fires [`DocumentSaved`] on
 success. No-ops if the document has no canonical path (Save-As
 needed — separate command, not defined yet) or if the backing
 library is read-only (MSL, Bundled in Modelica's case).

 Dirty state (generation vs. last-saved generation) is a per-document
 concern; the owning domain updates its internal tracker in the
 observer.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  The document to persist. |

#### `UndoDocument`

 Request to undo the most recent history group on the document, syncing any dependent UI
 state (editor buffer, diagram canvas) to match the reverted source.

 Handled per-domain: the registry that owns `doc` runs its
 [`DocumentHost`](lunco_doc::DocumentHost)`::undo()`, fires
 [`crate::DocumentChanged`], and performs whatever view-state sync the
 domain requires (e.g. for Modelica, update the text buffer). Domains
 that don't own `doc` ignore the trigger.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  The document whose most recent history group should be undone. |

## Time & clock

### `lunco-time` <a id="lunco-time"></a>

#### `ControlAnimation`

 Drive the [`AnimationPreview`] transport. Each field is optional so one verb
 covers run / pause / scroll(seek) / rate / loop — `{"type":"ExecuteCommand","command":"ControlAnimation",
 "params":{"playing":false}}` pauses, `{"seek_secs":3.0}` scrubs to 3 s,
 `{"rate":2.0}` doubles speed, `{"looping":true}` loops. Headless-safe: it only
 writes the preview domain's [`Playback`], never any UI or render resource.

 Fields are orthogonal, so a **restart** is one trigger:
 `{"playing":true,"seek_secs":0.0}` — seek to the range start and run. Seek to
 [`Playback::start`] rather than a literal `0.0`: [`step_playhead`] clamps to
 `[start, end]`, so on a clip whose range starts late a hardcoded 0 lands
 outside the range and snaps forward on the next step.

- *defined in:* `crates/lunco-time/src/domain.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Option < Entity >` |  Which driven domain to control. `None` = the shared [`AnimationPreview`].   A per-object driven clock (a camera path's, say) is otherwise unreachable:  it owns its own [`Playback`], so the preview transport does not touch it and  the shot cannot be paused, scrubbed or replayed at all. Point this at the  domain entity to drive it with the same one verb. |
| `playing` | `Option < bool >` |  Play (`Some(true)`) / pause (`Some(false)`) the animation; `None` leaves it. |
| `seek_secs` | `Option < f64 >` |  Seek the playhead to this time in **seconds**; `None` leaves it. |
| `rate` | `Option < f64 >` |  Playback rate (1.0 = realtime); `None` leaves it. |
| `looping` | `Option < bool >` |  Wrap at the range end instead of clamping (`None` leaves it). Honoured by  [`step_playhead`], and only meaningful once the range is bounded — an  unbounded `Playback` ignores it, so a looping cutscene needs authored  clip spans (grown by `bind_animated_to_preview`). |

#### `ResetTime`

 Reset the **entire clock tree** to defaults — fired on every scene load.

 This command restores the standing clock shape across scene reloads (doc 19
 §11b):

 * **celestial** → back on the `Epoch` root, affine identity;
 * **interaction** → wall-rooted identity (its default);
 * **animation preview** → playhead 0, playing, 1×;
 * **transport** → Playing at 1×, except for an explicit pause requested while
   the scene transition was pending, which is applied once to the replacement;
 * **mission calendar** → the authored mission epoch at tick zero. The epoch
   itself is preserved so a scene load can apply its `SetMissionEpoch` afterward.

- *defined in:* `crates/lunco-time/src/domain.rs`
- *fields:* none — call with `ResetTime` (no params)

#### `SetClock`

 Re-point, rate-scale or seek one clock —
 `{"type":"ExecuteCommand","command":"SetClock","params":{"clock":"Celestial","parent":"Real","scale":1000}}`
 runs the sky 1000× **while the simulation stays paused**.

 One verb covers every case, because in an affine tree they are the same case:
 * **detach / re-attach** — `parent` (the pause story: a clock freezes because of
   *where it hangs*, so unfreezing one clock is a re-parent, not a flag),
 * **time-dilate** — `scale` (`1000` = the sky at 1000×; the sim is untouched),
 * **seek** — `epoch_jd` on the celestial clock, or `offset` in seconds.

 World state, not a view preference: it goes through the command/journal path, so
 every client sees the same sky and a replay reproduces it.

- *defined in:* `crates/lunco-time/src/domain.rs`

| Field | Type | Description |
|---|---|---|
| `clock` | `ClockId` |  Which clock to edit. |
| `parent` | `Option < ClockParent >` |  Re-parent it (`"sim"` = freezes with the sim; `"real"` = free-running). |
| `scale` | `Option < f64 >` |  Rate relative to the parent (1.0 = follow, 1000.0 = 1000×). |
| `offset` | `Option < f64 >` |  Affine offset over the parent, seconds. |
| `epoch_jd` | `Option < f64 >` |  Seek the CELESTIAL clock to an absolute date (Julian Date, TDB). Ignored on  other clocks — they have no epoch mapping. |

#### `SetMissionEpoch`

 Re-anchor the world clock at an absolute epoch (Julian Date, TDB) —
 `{"type":"ExecuteCommand","command":"SetMissionEpoch","params":{"epoch_jd":2461253.0}}`. Sets both
 the mission origin and the calendar anchor at the CURRENT tick, so the sim
 jumps to that date without a tick discontinuity. This is how a scene picks
 its date: a site-anchored USD stage authors `double lunco:time:epochJd` on
 its root prim (e.g. an epoch where the Shackleton site is sunlit) and the
 USD bridge fires this command on load.

- *defined in:* `crates/lunco-time/src/domain.rs`

| Field | Type | Description |
|---|---|---|
| `epoch_jd` | `f64` |  Absolute epoch, Julian Date (TDB). |

#### `SetTimeTransport`

 Drive the LIVE-WORLD transport (physics/tick clock), distinct from
 [`ControlAnimation`] which drives the keyframe preview. Each field optional so
 one verb covers pause / play / rate — `{"type":"ExecuteCommand","command":"SetTimeTransport",
 "params":{"playing":false}}` PAUSES the whole simulation (tick + physics),
 `{"rate":4.0}` runs it 4× realtime, and the bounded causal ladder ends at
 64×. Rates below 0.1× or above 64× are rejected. Use `SetClock` for a
 presentation-only celestial rate when a detached clock is explicitly needed.
 This is THE pause command:
 exposed on the API/MCP and wrapped by the rhai prelude verbs
 `pause()`/`play()`/`set_rate()`, so a cutscene or a "reload-then-pause"
 one-liner can freeze the world.

- *defined in:* `crates/lunco-time/src/domain.rs`

| Field | Type | Description |
|---|---|---|
| `playing` | `Option < bool >` |  Play (`Some(true)`) / pause (`Some(false)`); `None` leaves it. |
| `rate` | `Option < f64 >` |  Speed multiplier vs realtime (1.0 = realtime, bounded to 0.1–64.0 for  the causal live transport); `None` leaves it. |

## Celestial, environment & comms

### `lunco-environment` <a id="lunco-environment"></a>

#### `SetEnvironmentLight`

 Sets scene environment lighting at runtime: the sun's direction and the
 global ambient level.

 All three fields are optional — only the ones provided change, the rest
 keep their current value. So a curl that just lowers the sun looks like:

 ```jsonc
 {"type":"ExecuteCommand","command":"SetEnvironmentLight","params":{"sun_pitch":-0.15}}
 ```

 - **`sun_yaw` / `sun_pitch`** — direction of the single `DirectionalLight`
   in radians, using the same `EulerRot::YXZ` (yaw-then-pitch) convention as
   the sandbox settings panel. A small negative `sun_pitch` (e.g. `-0.15`,
   ~8.5° above the horizon) gives long, raking lunar shadows; `-0.8` is a
   high ~46° sun with short shadows.
 - **`ambient_brightness`** — the [`GlobalAmbientLight`] level (the *real*
   scene-wide fill; the per-camera `AmbientLight` component is only an
   override). Lower it (~30–60) for deep, high-contrast lunar shadow cores;
   the airless Moon has near-black shadows.

- *defined in:* `crates/lunco-environment/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `sun_yaw` | `Option < f32 >` |  Sun azimuth in radians (`EulerRot::YXZ` yaw). `None` keeps current. |
| `sun_pitch` | `Option < f32 >` |  Sun elevation in radians (`EulerRot::YXZ` pitch); negative tilts the  light down. `None` keeps current. |
| `illuminance` | `Option < f32 >` |  Sun illuminance in lux. `None` keeps current. |
| `sun_color` | `Option < [f32 ; 3] >` |  Sun color as linear RGB. `None` keeps current. |
| `shadow_maps_enabled` | `Option < bool >` |  Whether the sun casts shadows. `None` keeps current. |
| `shadow_first_cascade_bound` | `Option < f32 >` |  Far bound of the first (sharpest) shadow cascade, metres.  `None` keeps current. |
| `shadow_max_distance` | `Option < f32 >` |  Total shadow-casting range, metres. Smaller ⇒ denser shadow-map  texels ⇒ crisper shadows. `None` keeps current. |
| `ambient_brightness` | `Option < f32 >` |  Global ambient brightness (cd/m²-scaled). `None` keeps current. |
| `exposure_ev100` | `Option < f32 >` |  Camera physical exposure, EV100 (≈15 = sunlight, 9.7 = Blender default).  Moves with `illuminance`: brighter sun ⇒ higher EV. `None` keeps current. |
| `earthshine_color` | `Option < [f32 ; 3] >` |  [`Earthshine`] fill color, linear RGB (cool blue ≈ 0.6,0.75,1.0).  `None` keeps current. |
| `bloom_intensity` | `Option < f32 >` |  Bloom intensity on the scene cameras. `None` keeps current; zero disables  bloom and a non-zero value enables the HDR target required by the effect.   **Applied render-side** (`lunco_render_bevy::env_light`) — bloom is  `bevy_post_process`, and this crate must not name it. That observer  writes the render intent, whose binder owns the concrete post-process  component. |

## Terrain

### `lunco-terrain-surface` <a id="lunco-terrain-surface"></a>

#### `BrushTerrain`

 Raise or lower terrain with a radial brush, recorded as a named edit layer
 so it can be removed later. Document-free terrains only — a doc-backed
 terrain's edits are authored to its USD layer instead.

- *defined in:* `crates/lunco-terrain-surface/src/terrain.rs`

| Field | Type | Description |
|---|---|---|
| `x` | `f32` |  Brush centre, world X. |
| `z` | `f32` |  Brush centre, world Z. |
| `radius` | `f32` |   |
| `amplitude` | `f32` |   |
| `id` | `String` |  Optional stable id for the edit (so it can be removed later). Empty = auto. |

#### `FlattenTerrain`

 Flatten the terrain toward `target_y` within `radius`, blending back to the
 existing surface at the edge — the "level a landing pad" tool. `(x, z)` are
 terrain-local metres. Reachable as `cmd("FlattenTerrain", #{x, z, radius, target_y})`.

- *defined in:* `crates/lunco-terrain-surface/src/terrain.rs`

| Field | Type | Description |
|---|---|---|
| `x` | `f32` |   |
| `z` | `f32` |   |
| `radius` | `f32` |   |
| `target_y` | `f32` |   |
| `id` | `String` |  Optional stable id for the edit (so it can be removed later). Empty = auto. |

#### `PlaceCrater`

 Place ONE hand-authored impact crater: rim radius `radius` m centred at
 terrain-local `(x, z)`, bowl `depth` m (0 = realistic default `0.4·radius`,
 the fresh d/D ≈ 0.2 morphology). Same analytic profile as the procedural
 field, so it lands in mesh + collider + derived maps alike, and it is an
 addressable edit — remove it later via `RemoveTerrainLayer{id}`. Reachable
 as `cmd("PlaceCrater", #{x, z, radius})`.

- *defined in:* `crates/lunco-terrain-surface/src/terrain.rs`

| Field | Type | Description |
|---|---|---|
| `x` | `f32` |   |
| `z` | `f32` |   |
| `radius` | `f32` |   |
| `depth` | `f32` |  Bowl depth in metres; 0/absent = realistic default (0.4·radius). |
| `id` | `String` |  Optional stable id for the edit (so it can be removed later). Empty = auto. |

#### `PlaceRock`

 Place ONE hand-authored boulder at terrain-local `(x, z)`, radius `size` m —
 its own addressable layer (removable via `RemoveTerrainLayer{id}`). Same
 mesh/collider derivation as the procedural rock field, so it looks and
 drives identically. Reachable as `cmd("PlaceRock", #{x, z, size})`.

- *defined in:* `crates/lunco-terrain-surface/src/terrain.rs`

| Field | Type | Description |
|---|---|---|
| `x` | `f32` |   |
| `z` | `f32` |   |
| `size` | `f32` |  Boulder radius in metres; 0/absent = 0.6 m. |
| `seed` | `u64` |  Shape/orientation seed; 0 = derived from position (stable, varied). |
| `id` | `String` |  Optional stable id for the layer (so it can be removed later). Empty = auto. |

#### `RemoveTerrainLayer`

 Remove a terrain layer by its [`LayerId`] — undo a specific dig/flatten (or any
 addressable layer). Re-bakes via `Changed<TerrainLayerStack>`. Reachable as
 `cmd("RemoveTerrainLayer", #{id})`.

- *defined in:* `crates/lunco-terrain-surface/src/terrain.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |   |

#### `SetTerrainOverlay`

 Arm / re-tune the terrain analysis overlay at runtime (MCP / scripting / UI).

 **Every field is optional: an OMITTED field keeps its current value.** So
 `{ "enabled": true }` arms the overlay with the existing angles/opacity, and
 `{ "cliff_deg": 25 }` re-tunes the critical angle without touching `enabled`.

 The fields are `Option<T>` rather than zero-sentinels because the sentinel form
 could not represent "omitted" for `enabled` — `#[Command(default)]` gave it
 `false`, so a re-tune like `{"cliff_deg":25}` silently turned the overlay OFF —
 and it made `opacity: 0` unsettable.

- *defined in:* `crates/lunco-terrain-surface/src/overlay.rs`

| Field | Type | Description |
|---|---|---|
| `enabled` | `Option < bool >` |   |
| `safe_deg` | `Option < f32 >` |   |
| `cliff_deg` | `Option < f32 >` |   |
| `opacity` | `Option < f32 >` |   |
| `lod_depth` | `Option < bool >` |  Switch the overlay to the LOD-depth view (still needs `enabled`). |
| `shader` | `Option < String >` |  Replace the diagnostic fragment shader asset. Omitted keeps the current  diagnostic material. |
| `vertex_shader` | `Option < String >` |  Replace the diagnostic vertex shader asset. Omitted keeps the current  diagnostic vertex stage. |

#### `SetTerrainRenderingQuality`

 Edit the persisted terrain rendering-quality fields through the same typed
 command bus used by the Graphics settings menu. Omitted values remain
 unchanged; an invalid candidate is rejected as a whole instead of being
 clamped into an undocumented quality level.

- *defined in:* `crates/lunco-terrain-surface/src/stream_viz.rs`

| Field | Type | Description |
|---|---|---|
| `tile_resolution` | `Option < usize >` |   |
| `cinematic_resolution` | `Option < usize >` |   |
| `pixel_error` | `Option < f64 >` |   |
| `max_depth` | `Option < u8 >` |   |
| `probe_resolution` | `Option < usize >` |   |
| `bakes_per_frame` | `Option < usize >` |   |
| `max_inflight_bakes` | `Option < usize >` |   |
| `tile_budget` | `Option < usize >` |   |
| `cover_edits_per_frame` | `Option < usize >` |   |
| `hysteresis_ratio` | `Option < f64 >` |   |
| `morph_start_ratio` | `Option < f64 >` |   |

#### `SpawnDemTerrain`

 Build a DEM terrain from a site directory at **native resolution**. `uri`
 points at a `lunar_terrain_exporter` output dir; the one file read is
 `materials/textures/heightmap.tif`, a georeferenced GeoTIFF that states its own
 extent and projection.

 `window_m` is the side length (metres) of the centred region realized as one
 full-5 m-resolution tile (mesh + collider). `0` = the whole DEM (heavy — a
 16 km map is ~10 M verts; prefer tiled streaming). Detail is **never**
 decimated.

- *defined in:* `crates/lunco-terrain-surface/src/terrain.rs`

| Field | Type | Description |
|---|---|---|
| `uri` | `String` |   |
| `shader` | `String` |  Fragment shader source for the terrain material. The path is resolved by  the normal asset-source rules; it is not selected by the terrain engine. |
| `vertex_shader` | `Option < String >` |  CDLOD vertex shader source. Required when `lod_viz` is enabled; omitted  for a static mesh that uses the shader's standard Bevy vertex stage. |
| `window_m` | `f32` |   |
| `target_res` | `u32` |  Visual-quality downsample target (samples per side). `0` = native (no  decimation). Re-issue the command with a different value to rebuild the  same site at another quality and compare. |
| `lod_viz` | `bool` |  Stream camera-driven CDLOD tiles using the authored terrain material  instead of one static mesh; collider/physics unchanged. Production visual path. |
| `collider_ring` | `bool` |  Stream a canonical-res collider ring around runtime physical support  footprints instead of one static full-DEM collider (replaces it — physics  rides the streamed tiles). |
| `collider` | `crate :: collider_ring :: TerrainColliderSettings` |  Physics-only collider-ring lattice. Omitted command fields use the  documented terrain-physics defaults and never read graphics quality. |
| `crater_density` | `f32` |  Convenience: add a crater layer at this density (craters per hectare). `0`  (default) = no craters. The USD path instead composes layers as child prims  (see [`crate::terrain_layers`]); this is for the quick command path. |

## Obstacle fields

### `lunco-obstacle-field` <a id="lunco-obstacle-field"></a>

#### `UpdateObstacleFieldSpec`

 Replace the obstacle-field spec and regenerate the field. The whole spec is
 sent, not a delta, so a caller that means to change one knob must send the
 others back unchanged. Journaled as a `DomainKind::ObstacleField` op.

- *defined in:* `crates/lunco-obstacle-field/src/plugin.rs`

| Field | Type | Description |
|---|---|---|
| `spec` | `ObstacleFieldSpec` |  The complete new spec — density, seed, extent, size distribution. |

## API & schema

### `lunco-api` <a id="lunco-api"></a>

#### `Exit`

 Shut down the application.

 `force = true`: exit immediately. The reliable path for automation.

 `force = false`: close the way a user would — route through the interactive
 dirty-document save prompt, which a windowed host installs an observer for
 (`lunco_modelica_ui::ui::commands::util`). **On a host with no window there is
 nobody to answer that prompt**, so this exits directly rather than waiting
 forever for a modal that will never be drawn.

 Shutting down is a session concern, so it lives with the session and exists
 on every binary — a windowless host cannot fall back to closing a window.
 Hosts with extra work to do on the way out (cancel in-flight compiles, prompt
 to save) observe the same command and do their part.

- *defined in:* `crates/lunco-api/src/session.rs`

| Field | Type | Description |
|---|---|---|
| `force` | `bool` |  Skip the interactive save prompt and exit immediately. |

#### `Ping`

 API readiness probe. Answers as soon as the command core is up, on every
 build — windowed, headless, or wasm.

- *defined in:* `crates/lunco-api/src/session.rs`
- *fields:* none — call with `Ping` (no params)

## Core

### `lunco-core` <a id="lunco-core"></a>

#### `SetSubsystemEnabled`

 Enable or disable a runtime subsystem registered by its owning plugin.

 The command is intentionally generic: authored scenarios may use it for a
 progressive-fidelity flow, while the core only validates the registered
 subsystem name and publishes the resulting state.

- *defined in:* `crates/lunco-core/src/subsystems.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Registered subsystem key. |
| `on` | `bool` |  `true` enables, `false` disables. |

#### `SpawnEntity`

 Spawn an independent entity from the catalog at a given world position.

 **Why the type lives in `lunco-core` and the handler does not.** `SpawnEntity`
 is a *wire* command: `lunco-networking` declares its channel
 (`declare_channel::<SpawnEntity>`), which needs nothing but the type. The
 handler (`on_spawn_entity_command`) lives with the catalog it spawns from, in
 `lunco-scene-commands`. Keeping the *definition* here is what lets the networking crate
 drop its dependency on the 13.4k-LOC editor — an edge that used to drag the
 whole editor closure (→ modelica → workspace → doc-bevy) into every networking
 build for exactly two symbols (review A6).

 `reflect_default` semantics: API/rhai callers may omit optional fields — a
 missing `rotation` defaults to `None` (→ identity). Position is always
 expressed in the current semantic physics frame; callers
 never pass a Bevy grid entity or perform BigSpace hierarchy conversion
 themselves.

- *defined in:* `crates/lunco-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `entry_id` | `String` |  The independent catalog entry ID (e.g. "ball_dynamic", "skid_rover"). |
| `position` | `[f64 ; 3]` |  Position in the active physics frame, in metres. Kept as f64 through  command transport and frame conversion; narrowing occurs only at the  final scene-root-local Bevy `Transform` boundary. |
| `rotation` | `Option < [f64 ; 4] >` |  Rotation in the active physics frame as an `(x, y, z, w)` unit  quaternion (optional; omitted → identity). Kept as f64 across the  command boundary for the same reason as `position`; Bevy's f32  [`bevy::prelude::Quat`] is a render/local-transform representation, not a  simulation-frame interchange type. |

## Other (source location unknown)

### `lunco-assets-datasets` <a id="lunco-assets-datasets"></a>

#### `CancelDataset`

 User intent to cancel one dataset operation.

- *defined in:* `crates/lunco-assets-datasets/src/registry.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |  Globally unique dataset id. |

#### `RequestDataset`

 User intent to start one dataset operation.

- *defined in:* `crates/lunco-assets-datasets/src/registry.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |  Globally unique dataset id. |

### `lunco-avatar-core` <a id="lunco-avatar-core"></a>

#### `FocusTarget`

 Focus on a target without taking control.

- *defined in:* `crates/lunco-avatar-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `avatar` | `Option < Entity >` |  The avatar entity that is focusing, when a local camera exists. |
| `target` | `Entity` |  The entity to focus on. |

#### `FollowTarget`

 Follow a target with the chase camera without taking control.

- *defined in:* `crates/lunco-avatar-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `avatar` | `Option < Entity >` |  The avatar entity that will follow, when a local camera exists. |
| `target` | `Entity` |  The entity to follow. |

#### `PossessVessel`

 Possess a vessel, taking direct control of it.

- *defined in:* `crates/lunco-avatar-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `avatar` | `Option < Entity >` |  The avatar entity taking possession, when a camera should be bound. |
| `target` | `Entity` |  The entity exposing the writable `InputPorts` surface to possess. |
| `bind_camera` | `bool` |  Whether possession also binds the avatar's camera to the vessel. |

#### `ReleaseVessel`

 Release possession of the currently controlled vessel.

- *defined in:* `crates/lunco-avatar-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The avatar entity releasing possession. |

#### `ReturnFromOrbit`

 Return the local camera from celestial orbit to its saved camera state.

- *defined in:* `crates/lunco-avatar-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The local avatar camera returning from orbit view. |

#### `SetCameraInput`

 Tune pointer-to-camera response while the application is running.

- *defined in:* `crates/lunco-avatar-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `look_radians_per_pointer_unit` | `Option < f32 >` |  Camera radians per pointer-motion unit. |
| `orbit_surface_min_scale` | `Option < f64 >` |  Lower bound for orbital rotation at the body's surface, in `[0, 1]`. |
| `orbit_distance_curve_exponent` | `Option < f64 >` |  Positive exponent shaping the apparent-horizon distance response. |

### `lunco-capture` <a id="lunco-capture"></a>

#### `CaptureFromCamera`

 **Capture from a specific vessel's mounted camera** — the typed command behind the
 `science::take_photo` instrument.

 Lives HERE rather than in `lunco-avatar` (its domain home) for the same reason
 [`CaptureScreenshot`] does: resolving a `Camera3d` and spawning a `Screenshot` is a
 render-world readback, and `lunco-avatar` is render-free by construction. A binary with
 no renderer therefore does not register this command *and* does not advertise the tool —
 rather than advertising a `take_photo` that captures nothing.

 `default`: `target` must have a reflect default or the executor's constructibility guard
 drops a no-param call — `photo()` in `control.rhai` sends `{}`. The default (`None`) means
 capture the explicitly resolved active scene camera.

- *defined in:* `crates/lunco-capture/src/screenshot.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Option < Entity >` |  Vessel whose unique mounted camera to capture from. `None` → the explicitly resolved  active scene camera. |

#### `CaptureScreenshot`

 **The one screenshot command.**

 Declared HERE, next to the only implementation, so a binary with no render backend does
 not advertise a command it cannot execute — `DiscoverSchema` (and hence the MCP tool list
 and the generated command reference) only sees it when this plugin is added.

 The reflected fields are the executable API contract used by the handler and generated
 command schema.

- *defined in:* `crates/lunco-capture/src/screenshot.rs`

| Field | Type | Description |
|---|---|---|
| `save_to_file` | `bool` |  Write the PNG to `path` instead of returning the bytes to the caller. |
| `path` | `String` |  Destination when `save_to_file`. Empty ⇒ a timestamped name in the cwd. |
| `region` | `Vec < u32 >` |  Optional crop `[x, y, w, h]` in physical pixels, applied before save/encode. Empty ⇒  the full frame. Cropping server-side lets a caller zoom into a panel without an  external image tool. |

#### `StartOfflineRecording`

 Command to start frame-by-frame recording.

- *defined in:* `crates/lunco-capture/src/screenshot.rs`

| Field | Type | Description |
|---|---|---|
| `output_dir` | `String` |  Target folder. Empty => 'recorded_frames' in the current working dir. |
| `fps` | `u32` |  Video target FPS (default: 60). |

#### `StopOfflineRecording`

 Command to stop frame-by-frame recording.

- *defined in:* `crates/lunco-capture/src/screenshot.rs`
- *fields:* none — call with `StopOfflineRecording` (no params)

### `lunco-celestial-spatial` <a id="lunco-celestial-spatial"></a>

#### `LeaveSurface`

 Leave the current body's surface and return to orbit view.

 Opens a transactional `OrbitCamera` view in the body's explicit star-fixed
 orbit grid. Returning restores the avatar's exact prior surface frame.

- *defined in:* `crates/lunco-celestial-spatial/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The avatar entity leaving the surface. |

#### `SetLinkCadence`

 Set the connectivity recompute cadence at runtime (any client / language).

- *defined in:* `crates/lunco-celestial-spatial/src/link.rs`

| Field | Type | Description |
|---|---|---|
| `interval_s` | `f64` |   |

#### `TeleportToSurface`

 Teleport the avatar to a celestial body's surface.

 Places the camera on the body's Grid in surface-relative mode.

- *defined in:* `crates/lunco-celestial-spatial/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The avatar entity to teleport. |
| `body_entity` | `Entity` |  The celestial body whose surface should receive the avatar. |

### `lunco-core-session` <a id="lunco-core-session"></a>

#### `ClaimControl`

 Claim a stable control endpoint for the originating session.

 The command only changes session authority. Avatar camera binding and
 controller-specific composition remain with the higher-level command that
 needs them.

- *defined in:* `crates/lunco-core-session/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  Endpoint whose global identity becomes controlled. |

#### `ReleaseControlClaim`

 Release one stable control endpoint owned by the originating session.

- *defined in:* `crates/lunco-core-session/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  Endpoint whose global identity is released. |

#### `UpdateProfile`

 Update the display name associated with the active user session.

- *defined in:* `crates/lunco-core-session/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  New session display name. |

### `lunco-cosim-core` <a id="lunco-cosim-core"></a>

#### `ReleaseControl`

 Release the complete control intent for an endpoint and apply its safe state.

- *defined in:* `crates/lunco-cosim-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The endpoint whose complete control intent is released. |

#### `ReleasePort`

 Release one manual input-port intent and hand that port back to its wiring.

- *defined in:* `crates/lunco-cosim-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The entity whose hold is released. |
| `name` | `String` |  Input-port name. |

#### `SetPorts`

 Write a batch of named input ports on `target`.

 This is the generic control command: a wheeled rover, a Modelica-flown
 lander, or any other endpoint is controlled by writing the input names it
 declares. The receiver applies writes through its authoritative port
 backend. `seq` and `tick` carry prediction bookkeeping for networked input.

- *defined in:* `crates/lunco-cosim-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The entity whose input ports are written. |
| `writes` | `Vec < (String, f64) >` |  `(port_name, value)` writes to apply this tick. |
| `seq` | `u32` |  Client prediction sequence number, when the command came from a client. |
| `tick` | `u64` |  Simulation tick associated with this command. |

### `lunco-luncosim-core` <a id="lunco-luncosim-core"></a>

#### `SetRhaiPolicy`

 Convenience command: author (or hot-replace) a rhai policy as a `LunCoPolicy`
 USD prim under `<mounted-root>/Policies/<name>` in ONE call, instead of
 hand-issuing the underlying `ApplyUsdOp`s. Because it authors USD doc ops, the policy **journals →
 syncs to every peer → the projector activates it** (registers the rhai hook; at
 `MERGE_SEAM` flips the merge strategy). Re-issuing with the same `name` (or later
 editing `info:sourceCode`) **hot-replaces the hook live** — dynamic rhai
 editing with no file system, converging across the network.

 This command authors the INLINE source (`info:sourceCode`, journal plane) —
 the live-edit form. A file-backed policy is authored instead by pointing
 `info:sourceAsset` at an `@…rhai@` file (content plane, CID-synced); the
 projector resolves it via the asset server, and inline wins when both are set.

 This is the ergonomic surface over the canonical form (a `LunCoPolicy` prim); the
 raw `ApplyUsdOp` path still works. Single active scene doc for now (mirrors the
 journal drivers).

- *defined in:* `crates/lunco-luncosim-core/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Prim name under the mounted scene's `Policies` scope (the identity for  hot-replace); defaults to a sanitized `seam` when empty. |
| `seam` | `String` |  The hook seam (id): e.g. `"journal.merge.order"`, `"rbac.authorize"`, or  `"synth.<name>"` for a generated Modelica source/unit/layout policy. |
| `entry` | `String` |  The rhai entry function name. |
| `source` | `String` |  The rhai source defining `entry` (+ helpers). |
| `deterministic` | `bool` |  Deterministic (fresh rhai scope per invoke). Convergent seams (merge, drive)  must be `true`; the host-only authorize gate may be `false`. |

### `lunco-luncosim-ui` <a id="lunco-luncosim-ui"></a>

#### `SaveScenario`

 Save a live-edited Rhai scenario's current source back onto the
 `LunCoProgramAPI` prim it came from — the other half of scenario authoring.

 The shared USD lowering selects `info:sourceCode` and clears the old
 `info:id` and `info:sourceAsset` arms. The `string` value is authored RAW,
 so the whole Rhai source round-trips verbatim, journals like any edit, and
 reaches the `.usda` on `SaveDocument`.

 It authors onto the PROGRAM, not onto the vessel running it
 ([`lunco_core::ScenarioProgramPrim`] carries the path): a vessel can run
 several programs, and a source written onto the vessel would sit on a prim
 that runs nothing.

 Only doc-backed Twin scenes have an editable document; a raw-file scene is
 refused (logged, not silently dropped), matching the rule that the builder
 must only edit doc-backed scenes or it eats work on the next reload.

- *defined in:* `crates/lunco-luncosim-ui/src/save_scenario.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The scripted entity whose live scenario source to persist onto its prim.  Ownership-gated (same as `RunScenario`): saving a scenario is editing it. |

#### `SetScenarioRegistryFixture`

 Toggle the explicit production-harness failure fixture.

 This command is intentionally narrow: it changes only the menu's
 presentation fixture and leaves every Twin/scene resource untouched. The
 default `false` state has no effect on ordinary production runs.

- *defined in:* `crates/lunco-luncosim-ui/src/ui/scenario_fixture.rs`

| Field | Type | Description |
|---|---|---|
| `unavailable` | `bool` |  `true` injects the unavailable state; `false` restores normal discovery. |

### `lunco-modelica-ui-core` <a id="lunco-modelica-ui-core"></a>

#### `FocusDocumentByName`

 Focus the first open Modelica document whose title contains `pattern`.

 The Modelica UI owns the observer and tab-resolution policy. Keeping only
 the payload here lets other UI packages request focus without depending on
 the complete Modelica workbench.

- *defined in:* `crates/lunco-modelica-ui-core/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `pattern` | `String` |  Case-insensitive substring of the document title. Empty is a no-op. |

#### `OpenClass`

 Open a Modelica class by its fully-qualified name.

 The Modelica UI owns class lookup, duplication, and tab creation. This
 payload is shared so URI handlers, application boot routing, and panels all
 use one command contract.

- *defined in:* `crates/lunco-modelica-ui-core/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `qualified` | `String` |  Fully-qualified class path, for example  `Modelica.Blocks.Examples.PID_Controller`. |
| `action` | `ClassAction` |  Whether to view or duplicate the class. |

### `lunco-scene-authoring` <a id="lunco-scene-authoring"></a>

#### `CreateShader`

 Create a new dynamic shader from a built-in template (or supplied WGSL),
 persist it into the open Twin (`<twin>/shaders/<name>.wgsl`, or
 `assets/shaders/` when no Twin is open), register it in the picker, and
 optionally bind it to a target entity — all live, no restart.

 ```json
 {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"my_panel","template":"checker","target":42}}
 {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"custom","source":"<wgsl...>"}}
 ```

- *defined in:* `crates/lunco-scene-authoring/src/properties.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Display name / file stem, e.g. `"my_panel"` (sanitised to `[a-z0-9_]`). |
| `template` | `String` |  Template id when `source` is empty: `"solid"` (default) or `"checker"`. |
| `source` | `String` |  Full WGSL source. Empty → generate from `template`. |
| `target` | `u64` |  API id of an entity to apply the new shader to. `0` = create only. |

#### `DeleteShader`

 Delete a shader: unregister it from the picker [`ShaderCatalog`] and remove
 its `.wgsl` from disk (the twin's `shaders/` folder, or `assets/shaders`).
 Entities currently using it keep their in-memory material for the session.

 ```json
 {"type":"ExecuteCommand","command":"DeleteShader","params":{"path":"twin://moonbase/shaders/old.wgsl"}}
 ```

- *defined in:* `crates/lunco-scene-authoring/src/properties.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Asset path to remove (`twin://name/shaders/x.wgsl` or `shaders/x.wgsl`). |

#### `ImportShader`

 Import an existing `.wgsl` file from anywhere on disk INTO the open Twin
 (copies it to `<twin>/shaders/<name>.wgsl`), registers it in the picker, and
 optionally binds it to a target entity. The file must be a prop-pickable
 dynamic shader: a `Material` struct, and every `//!@engine` field it declares
 must be prop-fillable per the engine-param registry.

 ```json
 {"type":"ExecuteCommand","command":"ImportShader","params":{"source_path":"/home/me/cool.wgsl","name":"cool","target":42}}
 ```

- *defined in:* `crates/lunco-scene-authoring/src/properties.rs`

| Field | Type | Description |
|---|---|---|
| `source_path` | `String` |  Filesystem path of the `.wgsl` to import (absolute or cwd-relative). |
| `name` | `String` |  Optional new stem; empty → keep the source file's own stem. |
| `target` | `u64` |  API id of an entity to apply the imported shader to. `0` = import only. |

#### `ReloadShader`

 Force-reload live WGSL assets from disk so edits apply without restarting
 the app. Calls [`AssetServer::reload`], which re-runs the loader and lets
 dependent material pipelines rebuild.

 A supplied bare path such as `"shaders/wheel.wgsl"` resolves against the
 active engine-library identity, including its explicit `lunco://` spelling.
 An explicit `lunco://…` or `twin://…` path is matched exactly. An empty path
 reloads every currently loaded WGSL asset. The command fails visibly when
 the requested asset is not active instead of reporting a successful no-op.

- *defined in:* `crates/lunco-scene-authoring/src/properties.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |   |

#### `SetObjectProperty`

 Set a property on a scene object at runtime (live override — not persisted
 to USD). One general command instead of many narrow ones; new properties
 just add a `match` arm. Drive it from curl after a screenshot to iterate:

 ```jsonc
 {"type":"ExecuteCommand","command":"SetObjectProperty",
  "params":{"entity_id":42,"property":"shader","value":"shaders/balloon.wgsl"}}
 {"type":"ExecuteCommand","command":"SetObjectProperty",
  "params":{"entity_id":42,"property":"wedge_count","value":"12"}}
 {"type":"ExecuteCommand","command":"SetObjectProperty",
  "params":{"entity_id":42,"property":"cell_a","value":"0.1,0.8,0.2"}}
 ```

 Recognised `property` values:
 - `shader` → author a [`ShaderLook`] for that `.wgsl` (asset path); the render
   binder turns it into a material.
 - any parameter named by the shader's `Material` struct (e.g. `albedo`,
   `wedge_count`, `cell_a`) → set that named value on the entity's `ShaderLook`
   (requires `shader` set first, or a USD shader material). The shader's
   reflected schema resolves the type; colours are `r,g,b`.
 - `visible` → `true`/`false` toggles `Visibility`.
 - Per-wheel tire-spin dynamics (target a single wheel entity by its `api_id`):
   `brake_torque`, `slip_stiffness`, `bearing_damping`, `friction_mu`, `mass`,
   `moi`, `wheel_radius`, `rest_length`, `spring_k`, `damping_c` → set that
   `f64` field on the wheel's `WheelRaycast` live. Each wheel is its own entity,
   so this gives independent per-wheel control. Motor torque and no-load speed
   are owned by the composed Modelica motor prim; edit its authored
   `inputs:stall_torque` / `inputs:no_load_speed` attributes instead of
   addressing a wheel-local drive parameter.

- *defined in:* `crates/lunco-scene-authoring/src/properties.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API-stable global entity ID from `ListEntities`, resolved to the live  Bevy entity by `ApiEntityRegistry`. |
| `property` | `String` |  Property name (see struct docs). |
| `value` | `String` |  Value; comma-separated `r,g,b` for colors, a single float for params,  an asset path for `shader`, `true`/`false` for `visible`. |

#### `SetShaderSource`

 Replace a shader asset's WGSL **source in place** from text sent over the
 API, recompiling it live without touching disk or restarting. Overwrites the
 active `Shader` asset(s) at `path` (e.g. `"shaders/wheel.wgsl"`), so every
 material using them re-specializes its pipeline next frame. Bare engine
 paths resolve the same `lunco://`/default-source aliases as [`ReloadShader`].
 Compile/validation outcome surfaces in the render log (naga errors on a bad
 shader). Pairs with [`ReloadShader`] (disk) — this one is for pushing edits
 directly.

- *defined in:* `crates/lunco-scene-authoring/src/properties.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Asset path of the shader to overwrite, e.g. `"shaders/wheel.wgsl"`. |
| `source` | `String` |  New WGSL source text. |

### `lunco-scene-camera` <a id="lunco-scene-camera"></a>

#### `FocusEntityById`

 Point the free-flight avatar camera at an entity (by API id), from a fixed
 side-on-and-above angle at `distance` metres. Lets API clients (MCP tools,
 automated screenshots) frame a subject — e.g. a wheel — without hand-driving
 the camera. `entity_id` is the API id from `ListEntities` (a `u64`), same as
 the scene's entity-mutation commands.

- *defined in:* `crates/lunco-scene-camera/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API-stable global entity ID from `ListEntities`, resolved to the live  Bevy entity by `ApiEntityRegistry`. |
| `distance` | `f32` |  Camera distance from the target, metres. `<= 0` → default 6. |

#### `FocusEntityByPath`

 Set the render-free runtime focus to the composed USD prim at `path`.

 This is separate from the editor's `SelectUsdPrim`: a headless
 recorder has no Inspector, gizmo, or picking state to maintain, but
 runtime-authored surfaces still need a stable subject for scoped telemetry.
 The authored USD path remains stable across entity ids and scene reloads.

- *defined in:* `crates/lunco-scene-camera/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Absolute composed USD prim path (for example `/World/Lander`). |

#### `SetCameraLookAt`

 Aim the free-flight avatar camera: place it at `eye` and look at `target`
 (both absolute world-space). The flexible primitive — the client computes the
 angle (e.g. approach a wheel from its outboard side) and distance.

 Authoritative: whatever camera mode the avatar is in (orbit focus on a
 planet, spring-arm follow, surface mode), this strips it and reinstates a
 `FreeFlightCamera` at the requested pose — an API client asking for a
 specific view must always get it. `eye` and `target` speak the semantic
 [`lunco_spatial::ActivePhysicsFrame`]; the concrete grid is resolved from that
 resource so a previous orbit focus or a canonical render-only grid cannot
 put the camera in a different frame.

- *defined in:* `crates/lunco-scene-camera/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `eye` | `Vec3` |   |
| `target` | `Vec3` |   |

### `lunco-scene-catalog` <a id="lunco-scene-catalog"></a>

#### `RescanShaders`

 Re-read shader file names from the open Twins and engine asset library.

- *defined in:* `crates/lunco-scene-catalog/src/catalog.rs`
- *fields:* none — call with `RescanShaders` (no params)

#### `RescanSpawnCatalog`

 Force a re-scan of project USD files into the spawn catalog.

- *defined in:* `crates/lunco-scene-catalog/src/catalog.rs`
- *fields:* none — call with `RescanSpawnCatalog` (no params)

### `lunco-scene-validation` <a id="lunco-scene-validation"></a>

#### `RunLint`

 Lint what is loaded now.

 Findings land in [`lunco_lint::LintReport`] (readable via the `LintReport`
 query) and are logged — errors at `error!`, warnings at `warn!`.

- *defined in:* `crates/lunco-scene-validation/src/lint_command.rs`

| Field | Type | Description |
|---|---|---|
| `domain` | `String` |  Restrict to one lint domain (`"usd"`). Empty = every domain this scene  can produce facts for. Named rather than enumerated so a domain added  later needs no change to this verb. |
| `scope` | `String` |  Inspection scope. Empty or `"loaded_stages"` keeps the existing live  stage behavior; `"twin"` inspects the active Twin's resolver namespaces. |
| `policy` | `String` |  Twin namespace severity policy: `"warn"` (default) or `"error"`.  The policy is passed to authored Rhai; facts and collision ownership stay  in the generic Rust inspection path. |
| `doc_id` | `Option < u64 >` |  When present, lint exactly this open Editor document after its projected  stage reaches the document generation. Omitted keeps the loaded-scene  behavior for live simulation callers. |

### `lunco-telemetry` <a id="lunco-telemetry"></a>

#### `ControlTelemetry`

 Control the telemetry subsystem at runtime.

 **One verb, all-`Option` fields** — the [`ControlAnimation`](lunco_time::ControlAnimation)
 idiom. `None` means "leave unchanged". Five separate `StartTelemetry` /
 `SetTelemetryRate` / `SetRetention` / … commands would be five things to discover,
 document, journal, and keep in sync; this is one.

 `channel: None` addresses the **subsystem** (the master switch). `channel: Some(name)`
 addresses every channel with that name — names are not unique across entities, and
 "turn off `motor_current` everywhere" is the useful operation. To address exactly one
 entity's channel, edit its `Parameter` component directly (the Inspector, a script).

- *defined in:* `crates/lunco-telemetry/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `channel` | `Option < String >` |  Channel name, or `None` for the whole subsystem. |
| `entity` | `Option < Entity >` |  **Create** the channel on this entity if it does not exist.   Without this there was NO way to author a telemetry channel through the API at all —  only from rhai or USD. That left an external client (an agent, OpenMCT, a dashboard)  able to *read* channels but never to *ask for* one, so the only way to watch an  arbitrary port was to poll it from the client. `port` (or `reflect`) names what to  sample; both absent ⇒ this is a retune of an existing channel, not a create. |
| `port` | `Option < String >` |  Source for a created channel: a port name on `entity` (the fast path — this is what  makes any Modelica variable, Avian body value, joint, FSW signal, or USD sensor  watchable without authoring anything in the scene). |
| `reflect` | `Option < String >` |  Source for a created channel: a reflection path (`"Port.value"`). The escape  hatch, for a field no port exposes. |
| `unit` | `Option < String >` |  Engineering unit for a created channel. |
| `enabled` | `Option < bool >` |   |
| `rate_hz` | `Option < f64 >` |   |
| `retention` | `Option < usize >` |   |
| `atol` | `Option < f64 >` |  Absolute tolerance for the subsystem default numeric deadband. Applies  only when `channel` is `None`; a named channel uses `deadband` as its  explicit absolute override. |
| `rtol` | `Option < f64 >` |  Relative tolerance for the subsystem default numeric deadband. Applies  only when `channel` is `None`. |
| `deadband` | `Option < f64 >` |   |

### `lunco-usd-bevy-camera` <a id="lunco-usd-bevy-camera"></a>

#### `CameraPathTransport`

 **Transport verb for an authored camera path** — play / pause / rewind, addressed
 by the path prim's USD path (full path or its leaf, like [`SetActiveCamera`]).

 Exists because path release is otherwise owned entirely by the offline recorder
 (`start_camera_paths_when_recording_starts` in `lunco-luncosim`), and in an
 ordinary interactive session no recorder ever runs — so an authored path would
 sit held at its first frame forever. This is the deliberate, *explicit* answer to
 that: one verb the user (or a script, or the HTTP API) invokes. It is NOT a
 second automatic release. Two things racing to start the same shot on their own
 initiative is exactly the non-determinism the recorder-owned release was
 introduced to kill; adding a fallback here would reintroduce it.

 Typed [`Command`], so it is reachable everywhere with no per-language binding:
 rhai `cmd("CameraPathTransport", #{ path: "/World/Shot01", action: "Play" })`,
 the HTTP API, and MCP.

 # Per-shot camera paths are now viable

 The campaign is authored as ONE continuous 58 s curve spanning six shots. That
 was forced by the *previous* design, where every gate released simultaneously on
 a single global terrain-ready event — several short per-shot paths would all have
 started at once, so only a curve that was already continuous could survive it.

 That constraint is gone. Release is now per-path and demand-driven: the recorder
 releases on its own start edge, and this command addresses ONE path by prim path.
 A scene can therefore author a separate short `BasisCurves` path per shot and
 drive each independently. Nothing in the campaign does that yet — noted so
 whoever authors shots next knows they are no longer stuck with one long curve.

- *defined in:* `crates/lunco-usd-bevy-camera/src/camera_path.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  The path prim's USD path (e.g. `/World/Shots/Shot01`), or just its leaf  (`Shot01`). |
| `action` | `CameraPathAction` |  Play, pause, or rewind. |

#### `ObserveAvatar`

 Explicitly show the local avatar camera.

- *defined in:* `crates/lunco-usd-bevy-camera/src/camera_switch.rs`
- *fields:* none — call with `ObserveAvatar` (no params)

#### `ResumeCameraDirector`

 Return presentation ownership to the authored camera director.

- *defined in:* `crates/lunco-usd-bevy-camera/src/camera_switch.rs`
- *fields:* none — call with `ResumeCameraDirector` (no params)

#### `SetActiveCamera`

 Switch the viewport's active camera to the `SceneCamera` whose `Name` matches.

 Works with no avatar present. `name` matches the full USD prim path *or*
 its leaf, so a cutscene can `set_camera("ChaseCam")` to reach
 `/World/Rover/ChaseCam`, or `set_camera("WideShot")` for a scene camera.

- *defined in:* `crates/lunco-usd-bevy-camera/src/camera_switch.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Camera name (full USD prim path or its leaf). |

#### `SetUserCamera`

 Explicit operator selection of a named authored camera.

 Unlike [`SetActiveCamera`], this takes ownership from the authored director
 until [`ResumeCameraDirector`] is requested.

- *defined in:* `crates/lunco-usd-bevy-camera/src/camera_switch.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Camera name (full USD prim path or its leaf). |

### `lunco-usd-core` <a id="lunco-usd-core"></a>

#### `ApplyUsdOp`

 Apply one [`UsdOp`] to a document through the typed command bus.

 The `lunco-usd` runtime observes this command and routes it through the
 document registry so undo/redo, change notification, and read-only
 enforcement remain centralized there.

- *defined in:* `crates/lunco-usd-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Target document. |
| `parent_gen` | `Option < u64 >` |  Generation the caller edited from. When present, the operation is  rejected if the document advanced before it arrived. |
| `op` | `UsdOp` |  Operation to apply. |

#### `ApplyUsdOps`

 Apply one authored intent consisting of several USD operations.

 The `lunco-usd` runtime journals this list as one undo unit and observes it
 only after the document reaches its complete shape.

- *defined in:* `crates/lunco-usd-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Target document. |
| `parent_gen` | `Option < u64 >` |  Generation the caller edited from. When present, the complete compound  edit is rejected if the document advanced before it arrived. |
| `label` | `String` |  Human-readable undo/journal label. |
| `ops` | `Vec < UsdOp >` |  Ordered primitive USD operations comprising the one intent. |

#### `ApplyUsdTransientOps`

 Apply a compound USD edit that belongs to a disposable view rather than to
 user-authored history. The document and typed operation log still advance,
 so the live canonical stage receives the same ordered delta; only the
 undo/redo and external-journal entries are omitted.

- *defined in:* `crates/lunco-usd-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Target document. |
| `parent_gen` | `Option < u64 >` |  Generation the view was derived from. |
| `label` | `String` |  Human-readable diagnostic label. |
| `ops` | `Vec < UsdOp >` |  Ordered view operations. |

#### `AttachComponent`

 Attach one component asset to a host body as one journalled USD change set.

- *defined in:* `crates/lunco-usd-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Target document. |
| `spec` | `crate :: attach :: AttachSpec` |  The attachment to perform. |

#### `AttachProgram`

 Attach one source-backed simulation program to an existing USD prim.

- *defined in:* `crates/lunco-usd-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Target USD document. |
| `spec` | `crate :: program :: ProgramAttachSpec` |  Complete program attachment intent. |

#### `CommitUsdProposal`

 Merge one accepted proposal through the ordinary grouped USD edit path.

- *defined in:* `crates/lunco-usd-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `proposal` | `UsdProposalId` |  Proposal to accept and merge into its explicit document target. |

#### `CreateUsdProposal`

 Prepare a typed USD edit plan for review without mutating the document.

- *defined in:* `crates/lunco-usd-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Document that owns the authored target. |
| `scope` | `UsdEditScope` |  Explicit source-asset, assembly, or instance-override scope. |
| `label` | `String` |  Human-readable intent and eventual journal change-set label. |
| `parent_gen` | `u64` |  Generation read by the proposal author. |
| `ops` | `Vec < UsdOp >` |  Complete typed plan, kept out of the document until commit. |

#### `DetachComponent`

 Remove one attached component as one atomic authored intent.

- *defined in:* `crates/lunco-usd-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc_id` | `DocumentId` |  Target document. |
| `spec` | `crate :: attach :: DetachSpec` |  Exact component attachment to remove. |

#### `ReviewUsdProposal`

 Change review state without applying any USD operation.

- *defined in:* `crates/lunco-usd-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `proposal` | `UsdProposalId` |  Proposal allocated by [`CreateUsdProposal`]. |
| `action` | `UsdProposalReviewAction` |  Review decision. |

### `lunco-usd-sim-cosim` <a id="lunco-usd-sim-cosim"></a>

#### `ClearScene`

 Clear the active scene — despawn every USD prim entity + cosim wire
 and free the worker-side Modelica steppers / Python script docs they
 referenced, leaving an empty viewport.

 Fired when a Twin / folder opens with nothing to show — no
 `[usd] default_scene`, or a plain folder with no USD content — so the
 viewport reflects the newly opened folder instead of keeping the
 previously loaded scene. (`LoadScene` does this same clear *before*
 loading its new scene.) Also useful standalone over the API / MCP as
 a "clear the world" verb.

- *defined in:* `crates/lunco-usd-sim-cosim/src/lib.rs`
- *fields:* none — call with `ClearScene` (no params)

#### `LoadScene`

 Reload (or load) a USD scene at runtime via the API.

 `curl … {"type":"ExecuteCommand","command":"LoadScene","params":{"path":"lunco://scenes/luncosim/sandbox_scene.usda"}}`

 - `path`: root-qualified USD address (`lunco://…` or `twin://…`).
 - `root_prim`: optional override for the SDF path of the prim to
   spawn. Empty (default) reads the stage's `defaultPrim` metadata;
   if absent, the scene load fails visibly; a whole-stage `/` mount is not a
   valid scene root.

 Despawns every existing entity carrying `UsdPrimPath` plus every
 `SimConnection` (cosim wires are scene-derived in current code), then
 reloads the asset from disk and spawns a fresh root entity. Existing
 pipelines (`sync_usd_visuals`, `process_usd_cosim_prims`, the
 avian/sim translators) take it from there. The canonical `WorldGrid`
 is used as the parent — i.e. the `BigSpace` host stays put across
 reloads. Invalid world-shell topology is reported rather than repaired
 or resolved by entity order.

 Cleans up worker-side state too: sends `ModelicaCommand::Despawn`
 for every entity carrying a `ModelicaModel` (the Modelica worker
 drops its `steppers` / `cached_models` / `sim_streams` entries). Scene-owned
 Rhai documents are stopped and closed by the shared `SceneTeardown` owner;
 independent API/editor documents remain open until their explicit close.
 Without these ownership boundaries, repeated reloads accumulate stale
 workers or make an unrelated interactive document disappear.

- *defined in:* `crates/lunco-usd-sim-cosim/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Root-qualified USD address (`lunco://…` or `twin://…`). Filesystem paths  are opened through `OpenFile`, not this scene-mount command. |
| `root_prim` | `String` |  Optional override for the prim to spawn. Empty (default) reads  `defaultPrim` from the stage's metadata header. A missing `defaultPrim`  is a visible scene-load error; the runtime never mounts `/`. |

#### `RestartScene`

 Reload the CURRENTLY-ACTIVE scene from disk — the "restart" verb.

 [`LoadScene`] deliberately no-ops when asked to load the scene that is already
 active (same path + root), so it cannot pick up on-disk edits to the LIVE
 scene. `RestartScene` always clears the current scene's entities, force-reloads
 its stage asset from disk (busting the asset cache), and respawns a single
 fresh root — so editing a `.usda` then `restart_scene()` shows the change with
 no duplicate instances. `reset_document` is interpreted by the document layer:
 it is false for the normal preserve-edits restart and true only after the UI
 has confirmed a full reset. The lifecycle mechanic still targets whichever
 scene is loaded.
 Paired with `pause()` this is the "reload-then-freeze" one-liner the workflow
 wanted (`restart_scene(); pause();`).

- *defined in:* `crates/lunco-usd-sim-cosim/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `reset_document` | `bool` |  Discard the active file document's authored and runtime layers before  remounting. Callers must obtain explicit user consent first. |

### `lunco-usd-viewport-ui` <a id="lunco-usd-viewport-ui"></a>

#### `ApplyUsdInspectionPreset`

 Apply one persisted presentation preset to an explicit preview view.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |
| `name` | `String` |   |

#### `CloseUsdPreview`

 Close one preview session and release all of its presentation resources.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `preview` | `UsdPreviewId` |   |

#### `CloseUsdPreviewView`

 Close one presentation view. Closing the final view also closes its parent
 preview session because a session without a presentation view cannot be
 reached from the editor.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |

#### `DeleteUsdInspectionPreset`

 Delete one persisted presentation preset.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |   |

#### `ExplodeUsdPreview`

 Apply a transient, session-scoped explode pose to an explicit USD preview.
 This command changes only projected Bevy transforms; it never enters the
 USD document, journal, save state, or simulation projection.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `preview` | `UsdPreviewId` |   |
| `doc_id` | `DocumentId` |   |
| `assembly` | `String` |  Exact composed `kind = "assembly"` prim path. |
| `parts` | `Vec < String >` |  Exact composed prim paths below `assembly`. Rust sorts these paths for  stable offsets, so repeated calls do not depend on caller ordering. |
| `action` | `UsdPreviewExplodeAction` |   |
| `axis` | `Option < UsdPreviewExplodeAxis >` |  Required for `enable` and `update`; `null` is accepted for `reset`. |
| `spacing` | `Option < f32 >` |  Required for `enable` and `update`; `null` is accepted for `reset`. |

#### `FocusUsdPreview`

 Focus an already-open preview session in the USD dock.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `preview` | `UsdPreviewId` |   |

#### `FocusUsdPreviewView`

 Focus one presentation view and its parent USD preview session.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |

#### `FrameUsdPreviewSelection`

 Fit one preview view to the visual bounds of an exact composed prim
 subtree. Selection/reveal remains owned by the Editor selection surface;
 this command only changes presentation camera state.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `preview` | `UsdPreviewId` |   |
| `view` | `UsdPreviewViewId` |   |
| `path` | `String` |   |

#### `FrameUsdPreviewView`

 Fit one preview view to the projected visual bounds of its USD stage.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |

#### `OpenUsdPreview`

 Open one explicit document and authored edit target in an isolated preview
 session. Reopening the same `preview` id for its current document focuses
 and updates that lease in place; another document replaces only that
 explicit lease. Other sessions keep their roots, cameras, and stages
 untouched.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `preview` | `UsdPreviewId` |  Stable caller-owned identity of the preview session. |
| `doc_id` | `DocumentId` |  The USD document to render. |
| `edit_target` | `LayerId` |  The authored layer to use for editor mutations made from this preview. |

#### `OpenUsdPreviewView`

 Open an additional presentation view over an existing USD preview session.
 The view id is explicit so persisted layouts and agents can address the
 exact camera without relying on tab order or display names.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `preview` | `UsdPreviewId` |   |
| `view` | `UsdPreviewViewId` |   |

#### `PanUsdPreviewView`

 Pan one preview view in egui logical screen points. The view converts the
 delta to its camera plane using the current projection and render-target
 viewport.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |
| `delta` | `[f32 ; 2]` |   |

#### `ResetUsdPreviewView`

 Restore one preview view's default orbit pose and fit it to its stage.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |

#### `SaveUsdInspectionPreset`

 Save the current presentation pose under one explicit settings name.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |
| `name` | `String` |   |

#### `SetUsdPreviewProjection`

 Change the projection of one isolated USD preview view. This changes only
 the editor camera; authored USD camera opinions stay read-only presentation
 input and are never rewritten by a navigation gesture.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |
| `projection` | `UsdPreviewProjection` |   |

#### `SetUsdPreviewTextLayer`

 Change which authored/composed snapshot the Text mode displays.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |
| `layer` | `UsdPreviewTextLayer` |   |

#### `SetUsdPreviewViewMode`

 Change only the presentation mode of one existing USD preview view.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |
| `mode` | `UsdPreviewViewMode` |   |

#### `ZoomUsdPreviewView`

 Zoom one preview view by a positive multiplicative factor. Perspective
 views change orbit distance; orthographic views change projection scale.

- *defined in:* `crates/lunco-usd-viewport-ui/src/viewport.rs`

| Field | Type | Description |
|---|---|---|
| `view` | `UsdPreviewViewId` |   |
| `factor` | `f32` |   |

### `lunco-viz` <a id="lunco-viz"></a>

#### `SetTelemetryBrowserView`

 Select the telemetry browser's signal filter and focused signal.

- *defined in:* `crates/lunco-viz/src/telemetry_browser.rs`

| Field | Type | Description |
|---|---|---|
| `filter` | `String` |   |
| `signal` | `String` |   |

### `lunco-workbench-core` <a id="lunco-workbench-core"></a>

#### `ActivatePerspective`

 Activate a registered perspective by its stable identifier.

- *defined in:* `crates/lunco-workbench-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |  The identifier of the perspective to activate. |

#### `FocusPanel`

 Bring a registered singleton panel forward in the concrete shell.

- *defined in:* `crates/lunco-workbench-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |  The singleton panel's stable id. |

#### `OpenEphemeralSource`

 Open an ephemeral generated document in the read-only source viewer.

- *defined in:* `crates/lunco-workbench-core/src/source.rs`

| Field | Type | Description |
|---|---|---|
| `uri` | `String` |  URI shown as the document identity. |
| `text` | `String` |  Complete generated source text. |

#### `OpenSourceView`

 Open a registered asset as read-only text in the source viewer.

- *defined in:* `crates/lunco-workbench-core/src/source.rs`

| Field | Type | Description |
|---|---|---|
| `asset_path` | `String` |  Registered asset path. |

#### `OpenTwinSource`

 Open one file belonging to an open Twin in the editable source panel.

- *defined in:* `crates/lunco-workbench-core/src/source.rs`

| Field | Type | Description |
|---|---|---|
| `twin_root` | `String` |  Absolute root of the already-open Twin. |
| `relative_path` | `String` |  File path relative to that root. |
| `pinned` | `bool` |  Keep the file open when another preview is selected. |
| `focus` | `Option < bool >` |  Whether opening the source should focus its tab. |

#### `ResetToDefaultPerspective`

 Reset the shell to its required or first registered perspective.

- *defined in:* `crates/lunco-workbench-core/src/commands.rs`
- *fields:* none — call with `ResetToDefaultPerspective` (no params)

#### `ResetWorkspaceLayout`

 Reset the concrete workbench layout to the active perspective preset.

- *defined in:* `crates/lunco-workbench-core/src/commands.rs`
- *fields:* none — call with `ResetWorkspaceLayout` (no params)

#### `SaveSourceText`

 Persist an editable source buffer, optionally refreshing its owning domain.

- *defined in:* `crates/lunco-workbench-core/src/source.rs`

| Field | Type | Description |
|---|---|---|
| `twin_root` | `String` |  Absolute root of the already-open Twin. |
| `relative_path` | `String` |  File path relative to that root. |
| `text` | `String` |  Complete UTF-8 source text. |
| `update` | `bool` |  Re-dispatch the owning document open operation after writing. |

#### `SetRequiredPerspective`

 Constrain the shell to an authored perspective, or release the constraint.

- *defined in:* `crates/lunco-workbench-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `Option < String >` |  Raw perspective identifier, or `None` to release the constraint. |

### `lunco-workbench-guided-ui` <a id="lunco-workbench-guided-ui"></a>

#### `ClearSpotlight`

 Clear any active spotlight. Rhai: `clear_spotlight()`.

- *defined in:* `crates/lunco-workbench-guided-ui/src/lib.rs`
- *fields:* none — call with `ClearSpotlight` (no params)

#### `ClearTour`

 End the guided tour (hide the coach card + scrim). Rhai: `end_tour()`.

- *defined in:* `crates/lunco-workbench-guided-ui/src/lib.rs`
- *fields:* none — call with `ClearTour` (no params)

#### `GuidedBack`

 Return to the previous guided guided step.

- *defined in:* `crates/lunco-workbench-guided-ui/src/lib.rs`
- *fields:* none — call with `GuidedBack` (no params)

#### `GuidedNext`

 Advance a guided guided step through the shared typed-command bus.
 The command projector supplies the established `cmd:GuidedNext` event
 consumed by authored Rhai tours.

- *defined in:* `crates/lunco-workbench-guided-ui/src/lib.rs`
- *fields:* none — call with `GuidedNext` (no params)

#### `GuidedSkip`

 Stop the current guided guided tour.

- *defined in:* `crates/lunco-workbench-guided-ui/src/lib.rs`
- *fields:* none — call with `GuidedSkip` (no params)

#### `SetHint`

 Set the persistent one-line hint. Empty `text` clears it. Rhai: `hint(msg)`
 / `clear_hint()`.

- *defined in:* `crates/lunco-workbench-guided-ui/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `text` | `String` |  Instruction text; empty hides the hint line. |

#### `SetObjectives`

 Set the persistent objectives checklist. `text` is a pre-formatted block
 (one objective per line). Empty clears it. Rhai: `objectives_hud(list)` —
 the prelude formats the list into this block and also auto-publishes it from
 declarative `mission(me)` state.

- *defined in:* `crates/lunco-workbench-guided-ui/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `text` | `String` |  Pre-formatted checklist block; empty hides the objectives card. |

#### `SetTourStep`

 Show a guided-tour coach step: spotlight `anchor`, and draw a coach card with
 `title`/`body`, progress dots (`index`/`total`), and Back/Next/Skip controls.
 Rhai: `coach(index, total, anchor, title, body)`. The controls emit
 `cmd:GuidedNext` / `cmd:GuidedBack` / `cmd:GuidedSkip` on the event bus,
 which the tour script advances on (a script can simulate a click with
 `emit("cmd:GuidedNext", 0)`).

- *defined in:* `crates/lunco-workbench-guided-ui/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `index` | `i64` |  0-based step index (progress dots). |
| `total` | `i64` |  Total step count. |
| `anchor` | `String` |  `HelpAnchors` key to spotlight; empty = centred card. |
| `title` | `String` |  Coach-card banner title. |
| `body` | `String` |  Coach-card body text. |

#### `Spotlight`

 Spotlight a workbench widget by its [`HelpAnchors`](lunco_workbench_core::presentation::HelpAnchors) key,
 dimming everything else. Rhai: `spotlight(anchor, caption)`.

- *defined in:* `crates/lunco-workbench-guided-ui/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `anchor` | `String` |  The `HelpAnchors` key of the widget to highlight (e.g. `"twin_browser"`). |
| `text` | `String` |  Optional caption shown in the callout. Empty = no caption text. |

### `lunco-workspace` <a id="lunco-workspace"></a>

#### `AddFolderToWorkspace`

 Add a folder to the workspace **without** closing the open ones —
 VS Code's "Add Folder to Workspace…". A folder with a `twin.toml` routes to
 [`AddTwin`].

 Empty `path` asks a windowed host for a picker (see the module docs).

- *defined in:* `crates/lunco-workspace/src/open.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Filesystem path of the folder to add. Empty asks for a picker. |

#### `AddTwin`

 Strict variant of [`AddFolderToWorkspace`] — requires a `twin.toml`.

 Empty `path` asks a windowed host for a picker (see the module docs).

- *defined in:* `crates/lunco-workspace/src/open.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Filesystem path of the Twin root (must contain `twin.toml`).  Empty asks for a picker. |

#### `CreateTwin`

 Create a new Twin folder and asynchronously add it to the workspace.
 Empty `path` means "ask the windowed workbench for a folder".

- *defined in:* `crates/lunco-workspace/src/open.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Target Twin folder. The manifest is created here; missing ancestors are  created by the storage-backed manifest writer. |
| `name` | `String` |  Human-readable name. Empty uses the target folder name. |
| `default_scene` | `String` |  Optional Twin-relative USD stage opened when the Twin is admitted. |

#### `OpenFolder`

 Open a folder as the workspace root — a Twin if it has a `twin.toml`,
 otherwise a plain folder Twin (a first-class mode, no manifest required).

 VS Code semantics: this **replaces** the current workspace folders. Use
 [`AddFolderToWorkspace`] to keep them.

 Unlike [`OpenTwin`], an empty `path` is an ERROR rather than a picker
 request — a windowed host dispatches `ShowOpenFolderPicker` for that.

- *defined in:* `crates/lunco-workspace/src/open.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Filesystem path of the folder to open. |

#### `OpenTwin`

 Open a Twin folder — strict: the folder must contain a `twin.toml`.

 VS Code semantics: this **replaces** the currently open folders. Use
 [`AddTwin`] to keep them.

 Empty `path` means "ask the user", which only a windowed host can honour —
 see the module docs.

- *defined in:* `crates/lunco-workspace/src/open.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Filesystem path of the Twin root (must contain `twin.toml`).  Empty asks a windowed host to show a folder picker. |

#### `RenameTwinEntry`

 Rename a file or folder inside an open Twin.

 The workspace owns this payload because it identifies an entry by its Twin
 root and relative path, independently of any window, dock, or renderer.

- *defined in:* `crates/lunco-workspace/src/rename.rs`

| Field | Type | Description |
|---|---|---|
| `twin_root` | `String` |  Absolute path of the Twin root containing the entry. |
| `relative_path` | `String` |  Path of the entry relative to the Twin root. |
| `new_name` | `String` |  New filename; path separators are not accepted. |

#### `ResetTwinSetting`

 Remove one generic project-owned setting from the active Twin.

- *defined in:* `crates/lunco-workspace/src/open.rs`

| Field | Type | Description |
|---|---|---|
| `key` | `String` |  Namespaced setting key to remove. |

#### `SetTwinSetting`

 Persist one generic project-owned setting on the active Twin.

- *defined in:* `crates/lunco-workspace/src/open.rs`

| Field | Type | Description |
|---|---|---|
| `key` | `String` |  Namespaced setting key, for example `ui.camera_status`. |
| `value` | `TwinSettingInput` |  Scalar value to persist in the Twin manifest. |

---

<!-- 217 commands from the runtime schema; scanned 802 .rs files for docs (0 parse failure(s) skipped).
     `#[Command]` in source but NOT in the runtime schema — test fixtures, hidden
     (`ApiVisibility::hide`), or never registered; deliberately not documented: Collision, HiddenCommand, InternalEvent, JoinServer, LeaveServer, PluginCommand, PromoteScenario, RecoverVessel, ReflectedEvent, RunPython, ScriptOpenCommand, ScriptOwnedCommand, SetAllowFreeMovement, SetFollowMode, SetFollowOptIn, SetObserveMode, SetTargetClient, SetTeachMode, SetVisualLead, SharePerspective, TestEcho
-->
