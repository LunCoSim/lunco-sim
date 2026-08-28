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

**192 commands** across **27** crates. All documented.

> **Regenerate:** dump the schema from a running app, then
> `cargo run -p gen-command-docs -- --schema <schema.json>` (see the tool's `--help`).

## Index


**Scene editing & authoring**

- [`lunco-luncosim-edit`](#lunco-luncosim-edit) (10 commands)
- [`lunco-scene-commands`](#lunco-scene-commands) (20 commands)

**USD / scenes**

- [`lunco-usd`](#lunco-usd) (5 commands)
- [`lunco-usd-bevy`](#lunco-usd-bevy) (5 commands)
- [`lunco-usd-sim`](#lunco-usd-sim) (3 commands)

**Modelica modeling & simulation**

- [`lunco-modelica`](#lunco-modelica) (43 commands)

**Co-simulation**

- [`lunco-cosim`](#lunco-cosim) (2 commands)

**Vessels, mobility & control**

- [`lunco-autopilot`](#lunco-autopilot) (6 commands)
- [`lunco-controller`](#lunco-controller) (2 commands)

**Avatar & possession**

- [`lunco-avatar`](#lunco-avatar) (9 commands)

**Workbench UI & panels**

- [`lunco-ui`](#lunco-ui) (1 command)
- [`lunco-workbench`](#lunco-workbench) (35 commands)

**Tutorials & HUD**

- [`lunco-tutorial`](#lunco-tutorial) (4 commands)

**Scripting & scenarios**

- [`lunco-scripting`](#lunco-scripting) (8 commands)

**Documents & twins**

- [`lunco-doc-bevy`](#lunco-doc-bevy) (7 commands)

**Time & clock**

- [`lunco-time`](#lunco-time) (5 commands)

**Celestial, environment & comms**

- [`lunco-celestial`](#lunco-celestial) (3 commands)
- [`lunco-environment`](#lunco-environment) (1 command)

**Terrain**

- [`lunco-terrain-surface`](#lunco-terrain-surface) (8 commands)

**Obstacle fields**

- [`lunco-obstacle-field`](#lunco-obstacle-field) (1 command)

**API & schema**

- [`lunco-api`](#lunco-api) (2 commands)

**Core**

- [`lunco-core`](#lunco-core) (1 command)

**Other (source location unknown)**

- [`lunco-assets`](#lunco-assets) (2 commands)
- [`lunco-luncosim`](#lunco-luncosim) (2 commands)
- [`lunco-telemetry`](#lunco-telemetry) (1 command)
- [`lunco-viz`](#lunco-viz) (1 command)
- [`lunco-workspace`](#lunco-workspace) (5 commands)

---

## Scene editing & authoring

### `lunco-luncosim-edit` <a id="lunco-luncosim-edit"></a>

#### `AddCameraHere`

 Capture the active viewport camera's pose as a new `def Camera` prim.

 Authored into [`LayerId::root`] — the authored scene, serialized on Save. A
 captured shot is a durable edit to the twin, unlike the gizmo/waypoint
 interactions that write the ephemeral `runtime` overlay and vanish.

- *defined in:* `crates/lunco-luncosim-edit/src/ui/cinematic.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `Option < String >` |  Prim name for the new camera. `None` picks the first free `View_N`. |

#### `CancelWaypointEdit`

 Back out of ANY in-flight waypoint edit: an armed Move/Insert placement, or the
 open context menu.

 A real command, so cancelling is one verb for every waypoint mode (not a special
 case bolted onto Move) and is reachable from rhai/the API like anything else —
 rather than each mode sniffing a raw key for itself.

- *defined in:* `crates/lunco-luncosim-edit/src/ui/waypoint_click.rs`
- *fields:* none — call with `CancelWaypointEdit` (no params)

#### `SelectEntity`

 Select an entity by API id — the headless/scriptable equivalent of a
 Shift+Left-click in the viewport. Drives the same [`SelectedEntities`]
 resource and [`Selected`] highlight the mouse path uses, so the Inspector
 immediately shows that entity's components (Transform, Physics, Shader
 Parameters, …). Pass `entity_id == 0` to clear the selection.

 Selection is an editor concept (it targets the Inspector/gizmo), so this
 command lives in the `ui`-gated selection module — a headless server exposes
 no selection.

- *defined in:* `crates/lunco-luncosim-edit/src/selection.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API id from `ListEntities` — `u64` "Pattern B", resolved in the  observer via `ApiEntityRegistry` (same as `FocusEntityById`). `0`  clears the selection. |
| `extend` | `bool` |  If true, maintains the previous selection and adds this entity to it (like Shift-click) |
| `toggle` | `bool` |  If true, toggles the selection state of the entity (like Cmd/Ctrl-click) |

#### `SelectEntityByPath`

 Select a composed USD prim by its authored path.

 The path is resolved against the live `UsdPrimPath` projection rather than
 an episode-specific entity id. This keeps scripted presentation commands
 stable across scene reloads and across duplicated asset instances.

- *defined in:* `crates/lunco-luncosim-edit/src/selection.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |   |
| `extend` | `bool` |   |
| `toggle` | `bool` |   |

#### `SetSpawnDiagnostics`

 Enable or disable the Spawn Ghost pipeline trace.

- *defined in:* `crates/lunco-luncosim-edit/src/spawn.rs`

| Field | Type | Description |
|---|---|---|
| `enabled` | `bool` |   |

#### `StartAutopilot`

 Command to engage autopilot on a vessel.

- *defined in:* `crates/lunco-luncosim-edit/src/ui/waypoint_click.rs`

| Field | Type | Description |
|---|---|---|
| `vessel` | `Entity` |  The vessel entity to start autopilot on. |

#### `ToggleAutopilot`

 Command to toggle autopilot on/off on a vessel.

- *defined in:* `crates/lunco-luncosim-edit/src/ui/waypoint_click.rs`

| Field | Type | Description |
|---|---|---|
| `vessel` | `Entity` |  The vessel entity to toggle autopilot on/off. |

#### `ToggleJointViz`

 Toggle joint / wheel-force visualization.

 `#[Command(default)]` → all-false. Pass only the flags you want on.
 Rhai: `cmd("ToggleJointViz", #{show_joints: true, show_wheel_forces: true})`.

- *defined in:* `crates/lunco-luncosim-edit/src/joint_viz.rs`

| Field | Type | Description |
|---|---|---|
| `show_joints` | `bool` |  Show joint anchors + axes. |
| `show_wheel_forces` | `bool` |  Show wheel force boxes + arrows. |

#### `TogglePhysicsArrows`

 Typed command to flip the global physics-arrows toggle from the
 API / scripts / UI buttons.

 Empty / default fields mean "don't change that flag" — but
 `#[Command(default)]` produces a struct of all-false, so callers
 who want "only velocity" pass `{"velocity": true}` and the rest
 stays as supplied (or defaults to false). Idempotent.

- *defined in:* `crates/lunco-luncosim-edit/src/physics_viz.rs`

| Field | Type | Description |
|---|---|---|
| `enabled` | `bool` |  Master enable. |
| `velocity` | `bool` |  Velocity arrows on every dynamic body when `enabled`. |
| `force` | `bool` |  Force arrows on every dynamic body when `enabled`. Ignored  for bodies without a `ConstantForce`. |

#### `TogglePhysicsGizmo`

 Toggle the selected-body dynamics / frames gizmo.

 `#[Command(default)]` → all-false; pass only the flags you want on.
 Rhai: `cmd("TogglePhysicsGizmo", #{show_mass: true, show_forces: true})`.
 Leaves `newtons_per_meter` untouched.

- *defined in:* `crates/lunco-luncosim-edit/src/physics_gizmo.rs`

| Field | Type | Description |
|---|---|---|
| `show_mass` | `bool` |  CoM + inertia layer. |
| `show_forces` | `bool` |  Force-arrows layer. |
| `show_frames` | `bool` |  Body-frame triads layer. |

### `lunco-scene-commands` <a id="lunco-scene-commands"></a>

#### `AddRuntimeWaypoint`

 A runtime waypoint appended to a spawned vessel's patrol. A new runtime patrol
 uses a 2 m geometric arrival radius; the shared USD marker has a 2.5 m overlap
 sensor, so the behavior-tree leg completes inside the collision-backed
 `waypoint.reached` volume.

 The target is an [`Entity`] deliberately: the API/Rhai command dispatcher
 resolves the stable `GlobalEntityId` supplied by callers before this handler
 runs, just like the other scene commands.

- *defined in:* `crates/lunco-scene-commands/src/runtime_waypoint.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  Spawned rover root receiving the waypoint. |
| `position` | `[f64 ; 3]` |  Waypoint origin in the semantic active physics frame. Cell and render  hierarchy details are resolved by this command boundary; the shared  marker's overlap Sensor is positioned at the same physical point. |

#### `CreateShader`

 Create a new dynamic shader from a built-in template (or supplied WGSL),
 persist it into the open Twin (`<twin>/shaders/<name>.wgsl`, or
 `assets/shaders/` when no Twin is open), register it in the picker, and
 optionally bind it to a target entity — all live, no restart.

 ```json
 {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"my_panel","template":"checker","target":42}}
 {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"custom","source":"<wgsl...>"}}
 ```

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Display name / file stem, e.g. `"my_panel"` (sanitised to `[a-z0-9_]`). |
| `template` | `String` |  Template id when `source` is empty: `"solid"` (default) or `"checker"`. |
| `source` | `String` |  Full WGSL source. Empty → generate from `template`. |
| `target` | `u64` |  API id of an entity to apply the new shader to. `0` = create only. |

#### `DeleteEntity`

 Delete an entity from the scene.

 The typed verb for "remove this", replacing the ad-hoc `world.despawn(entity)` the
 Inspector used to do in two places. A bare despawn is invisible to the document:
 the prim survives in the layer, so the deletion never journals, never replicates,
 never persists, and the next projection can bring the entity straight back.

 This despawns AND (via [`persist_delete_to_runtime_layer`]) authors a `RemovePrim`
 — which is what makes deletion undoable, because the document hands back an
 `AddPrim` inverse for free.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  Entity to remove. |
| `intent` | `lunco_core :: EditIntent` |  `Persistent` (the default) authors the removal into the document; an  `Interactive` delete is live-only and does not journal. |

#### `DeleteShader`

 Delete a shader: unregister it from the picker [`ShaderCatalog`] and remove
 its `.wgsl` from disk (the twin's `shaders/` folder, or `assets/shaders`).
 Entities currently using it keep their in-memory material for the session.

 ```json
 {"type":"ExecuteCommand","command":"DeleteShader","params":{"path":"twin://moonbase/shaders/old.wgsl"}}
 ```

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Asset path to remove (`twin://name/shaders/x.wgsl` or `shaders/x.wgsl`). |

#### `DetachJoint`

 Detach a joint by despawning it.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The joint entity to despawn. |
| `intent` | `lunco_core :: EditIntent` |  Persistent (default) authors the joint's removal into the scene's runtime  layer — so it journals, syncs, and survives reload — before despawning.  Interactive just pops the live joint (a throwaway test), no journal. See  [`lunco_core::EditIntent`]. Omitted by API callers → `Persistent`. |

#### `FocusEntityById`

 Point the free-flight avatar camera at an entity (by API id), from a fixed
 side-on-and-above angle at `distance` metres. Lets API clients (MCP tools,
 automated screenshots) frame a subject — e.g. a wheel — without hand-driving
 the camera. `entity_id` is the API id from `ListEntities` (a `u64`), same as
 [`MoveEntity`]/[`SetObjectProperty`].

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API id from `ListEntities` — `u64` "Pattern B", resolved in the observer  via `ApiEntityRegistry`; see [`MoveEntity`]'s `entity_id` for why it  stays `u64` and isn't auto-converted by the id codec. |
| `distance` | `f32` |  Camera distance from the target, metres. `<= 0` → default 6. |

#### `FocusEntityByPath`

 Set the render-free runtime focus to the composed USD prim at `path`.

 This is separate from the editor's `SelectEntityByPath`: a headless
 recorder has no Inspector, gizmo, or picking state to maintain, but
 runtime-authored surfaces still need a stable subject for scoped telemetry.
 The authored USD path remains stable across entity ids and scene reloads.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Absolute composed USD prim path (for example `/World/Lander`). |

#### `ImportShader`

 Import an existing `.wgsl` file from anywhere on disk INTO the open Twin
 (copies it to `<twin>/shaders/<name>.wgsl`), registers it in the picker, and
 optionally binds it to a target entity. The file must be a prop-pickable
 dynamic shader: a `Material` struct, and every `//!@engine` field it declares
 must be prop-fillable per the engine-param registry.

 ```json
 {"type":"ExecuteCommand","command":"ImportShader","params":{"source_path":"/home/me/cool.wgsl","name":"cool","target":42}}
 ```

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `source_path` | `String` |  Filesystem path of the `.wgsl` to import (absolute or cwd-relative). |
| `name` | `String` |  Optional new stem; empty → keep the source file's own stem. |
| `target` | `u64` |  API id of an entity to apply the imported shader to. `0` = import only. |

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
| `entity_id` | `u64` |  API-stable global entity ID (the `api_id` from `ListEntities`),  resolved to a Bevy `Entity` in the observer via `ApiEntityRegistry`.   Deliberately `u64`, not `Entity` — this is "**Pattern B**". The  type-driven id codec (`crates/lunco-networking/PH2_ID_CODEC.md`)  auto-converts only `Entity`-typed fields, so a `u64` field opts out and  is resolved here instead. NOT migrated to `Entity` because this command  is `#[Command(default)]`, which derives `Default`, and `Entity` has no  `Default`. Leaving it `u64` is a cleanliness leftover, not a  names/correctness issue — the codec no longer keys off field names at  all, so this `u64` is simply ignored by it. (An earlier comment here  blamed the resolver "dropping the generation"; that was stale — the  codec preserves index+generation via `Entity::to_bits()`.) |
| `translation` | `[f64 ; 3]` |  Target translation in the semantic [`lunco_core::ActivePhysicsFrame`].  The concrete BigSpace grid, the entity's actual parent, and the cell/local  split are internal storage details resolved by the observer. The wire  representation is f64 so positions retain precision across API/network  round trips. |

#### `ReloadShader`

 Force-reload shader assets from disk so live WGSL edits apply without
 restarting the app. Bypasses the file watcher (unreliable in this build):
 calls [`AssetServer::reload`], which re-runs the loader and triggers
 dependent material pipelines to rebuild. Empty `path` → reload the standard
 `assets/shaders/*` set; otherwise reload just that path (e.g.
 `"shaders/wheel.wgsl"`).

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |   |

#### `RescanShaders`

 Rescan the open Twins' `shaders/` folders (and `assets/shaders`) and register
 any prop-pickable `.wgsl` into the picker [`ShaderCatalog`]. Lets you drop a
 shader file into a Twin and pick it up without restarting.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`
- *fields:* none — call with `RescanShaders` (no params)

#### `RescanSpawnCatalog`

 Force a re-scan of project USD files into the spawn catalog. Picks up
 `*.usda` dropped into an already-open Twin mid-session (twin-open is
 auto-scanned; this covers new files after that). Idempotent.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`
- *fields:* none — call with `RescanSpawnCatalog` (no params)

#### `RotateEntity`

 Set an entity's world ORIENTATION — the rotational twin of [`MoveEntity`].

 Reachable as `cmd("RotateEntity", #{entity_id, rotation: [x, y, z, w]})`, or
 `set_world_rotation(id, q)` from the rhai prelude. The quaternion is the same
 `[x, y, z, w]` form `world_rotation(id)` returns and `qrot` consumes, so a
 script can read an orientation, transform it, and write it back without ever
 converting representation.

 The public quaternion is expressed in [`lunco_core::ActivePhysicsFrame`], the
 same semantic frame as `MoveEntity`. Rotation is not frame-invariant: a
 rotating body Grid and a rotated assembly parent both change the local
 quaternion that must be stored on the entity. The observer performs that
 hierarchy conversion once.

 Written through `Transform`, never through avian's `Rotation`, for exactly
 the reason `MoveEntity` never hand-writes `Position`:
 `BigSpacePhysicsBridgePlugin::pose_to_position` fires on the external
 `Transform` write and derives the physics pose from it (carrying it to
 jointed descendants); a hand-written `Rotation` is a second, wronger opinion
 that the bridge's writeback then undoes. The body is pinned Kinematic for the
 move, as `MoveEntity` does, so the solver treats the new pose as
 authoritative rather than fighting it.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API-stable global entity ID (the `api_id` from `ListEntities`). `u64`  rather than `Entity` for the same reason `MoveEntity` uses one:  `#[Command(default)]` derives `Default`, and `Entity` has none. |
| `rotation` | `[f64 ; 4]` |  Target world orientation as `[x, y, z, w]`. Normalised on arrival — a  quaternion that has been interpolated or sampled is unit only to float  tolerance, and refusing it would make this fail for poses that are  perfectly usable. A degenerate (near-zero) quaternion IS refused: it  names no orientation, and silently substituting identity would spin the  body to an attitude the caller never asked for. |

#### `RunLint`

 Lint what is loaded now.

 Findings land in [`lunco_lint::LintReport`] (readable via the `LintReport`
 query) and are logged — errors at `error!`, warnings at `warn!`.

- *defined in:* `crates/lunco-scene-commands/src/lint_command.rs`

| Field | Type | Description |
|---|---|---|
| `domain` | `String` |  Restrict to one lint domain (`"usd"`). Empty = every domain this scene  can produce facts for. Named rather than enumerated so a domain added  later needs no change to this verb. |

#### `SetCameraLookAt`

 Aim the free-flight avatar camera: place it at `eye` and look at `target`
 (both absolute world-space). The flexible primitive — the client computes the
 angle (e.g. approach a wheel from its outboard side) and distance.

 Authoritative: whatever camera mode the avatar is in (orbit focus on a
 planet, spring-arm follow, surface mode), this strips it and reinstates a
 `FreeFlightCamera` at the requested pose — an API client asking for a
 specific view must always get it. `eye` and `target` speak the semantic
 [`lunco_core::ActivePhysicsFrame`]; the concrete grid is resolved from that
 resource so a previous orbit focus or a canonical render-only grid cannot
 put the camera in a different frame.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `eye` | `Vec3` |   |
| `target` | `Vec3` |   |

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

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `entity_id` | `u64` |  API-stable global entity ID (the `api_id` from `ListEntities`), same  resolution path as [`MoveEntity`] — `u64` "Pattern B", resolved in the  observer; see [`MoveEntity`]'s `entity_id` for why it stays `u64`. |
| `property` | `String` |  Property name (see struct docs). |
| `value` | `String` |  Value; comma-separated `r,g,b` for colors, a single float for params,  an asset path for `shader`, `true`/`false` for `visible`. |

#### `SetShaderSource`

 Replace a shader asset's WGSL **source in place** from text sent over the
 API, recompiling it live without touching disk or restarting. Overwrites the
 `Shader` asset currently at `path` (e.g. `"shaders/wheel.wgsl"`), so every
 material using it re-specializes its pipeline next frame. Compile/validation
 outcome surfaces in the render log (naga errors on a bad shader). Pairs with
 [`ReloadShader`] (disk) — this one is for pushing edits directly.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Asset path of the shader to overwrite, e.g. `"shaders/wheel.wgsl"`. |
| `source` | `String` |  New WGSL source text. |

#### `SetUsdAttribute`

 Author one standard USD attribute in the active document's runtime layer.

 This is the generic authoring verb for data-driven editor tools. It does not
 add a LunCo schema or mutate an ECS component: the USD type and literal are
 passed to the document's typed `UsdOp::SetAttribute` path, so composed USD
 remains the source of truth. A tool such as `nurbs.rhai` can therefore edit
 `point3f[] points` without a Rust handler for every geometry type.

- *defined in:* `crates/lunco-scene-commands/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Absolute USD prim path owned by the active document. |
| `name` | `String` |  Attribute name, for example `points` or `inputs:radius`. |
| `type_name` | `String` |  USD type name, for example `point3f[]`, `float`, or `token`. |
| `value` | `String` |  USD literal, exactly as it would appear in USDA (except `string`, which  is raw content according to `UsdOp::SetAttribute`'s contract). |

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

## USD / scenes

### `lunco-usd` <a id="lunco-usd"></a>

#### `ApplyUsdOp`

 Apply a [`UsdOp`] to the named document via the typed-command bus.

 Same shape as `lunco-modelica`'s op-dispatch commands: UI clicks,
 HTTP API calls, and scripts all dispatch this; the observer
 routes it through [`DocumentRegistry::<UsdDocument>::apply`] so undo/redo,
 change notification, and read-only enforcement stay in one place.

- *defined in:* `crates/lunco-usd/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Target document. |
| `op` | `UsdOp` |  Operation to apply. |

#### `ApplyUsdOps`

 Apply one authored intent that lowers to several USD operations.

 This is the command boundary for program construction, component assembly,
 and other compound edits: UI, Rhai and API callers all submit the same typed
 operation list, which is journalled as one undo unit and observed by the live
 projector only after the document reaches its complete shape.

- *defined in:* `crates/lunco-usd/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Target document. |
| `label` | `String` |  Human-readable undo/journal label. |
| `ops` | `Vec < UsdOp >` |  Ordered primitive USD operations comprising the one intent. |

#### `AttachComponent`

 Attach a component asset to a host body as a jointed child, deriving the
 joint anchor from the placement so it is authored once, not twice. Lowers to
 the primitive [`UsdOp`]s in [`crate::attach::attach_component_ops`].

 The whole lowering is applied inside **one journal change set**
 ([`apply_ops_as_change_set`]), so the attach is **one undo unit** — undo removes
 the part, its placement, its joint and the joint's anchors together. (It used to
 journal one entry per op: an undo peeled off a single op and left the object
 half-attached.)

 The complete lowered sequence is validated against a document clone before
 the live document is touched. If any op is rejected (for example because the
 host prim does not exist), no op is authored and the command logs the
 rejection. A valid sequence is then committed as one journal change set and
 undone as one unit. Socket-specific validation also rejects stale or
 incompatible requests before lowering.

- *defined in:* `crates/lunco-usd/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Target document. |
| `spec` | `crate :: attach :: AttachSpec` |  The attachment to perform. |

#### `AttachProgram`

 Attach one source-backed simulation program to an existing USD prim.

 The command lowers the complete `LunCoProgramAPI` contract — source asset,
 declared scalar ports, constants, connections, and realtime-safety promise —
 to one journaled USD change set. The Models palette, Rhai, HTTP, and future
 editor surfaces all use this command; none inserts ECS marker components.

 An empty `inputs`/`outputs` contract is valid for an effects-only program,
 but it is not a running scalar co-simulation participant. Add explicit ports
 and connections when the source must exchange values with Rust or Modelica.

- *defined in:* `crates/lunco-usd/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Target USD document. |
| `spec` | `crate :: program :: ProgramAttachSpec` |  Complete program attachment intent. |

#### `SetDomeLight`

 Author the scene's HDRI environment: a `UsdLuxDomeLight` carrying
 `inputs:texture:file`. Projected by `lunco_usd_bevy::dome` into a skybox +
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
| `doc` | `Option < DocumentId >` |  Document to author into. `None` = the workspace's active document. |
| `path` | `Option < String >` |  Prim path of the dome. `None` = `/World/Sky`.   It must live **under the stage's `defaultPrim` subtree** (`/World` in  every scene here) — a prim authored outside it composes into the layer  but is never mounted, so the sky would silently not appear. |
| `texture` | `Option < String >` |  `inputs:texture:file` — the HDRI, resolved relative to the stage layer  (e.g. `../hdri/lunar_horizon_2k.hdr`). Equirectangular (`.hdr`, `.png`)  or a `.ktx2` cubemap. |
| `intensity` | `Option < f32 >` |  `inputs:intensity` — multiplier on the image (1.0 = as authored). |
| `exposure` | `Option < f32 >` |  `inputs:exposure` — stops, applied as intensity × 2^exposure. |
| `color` | `Option < [f32 ; 3] >` |  `inputs:color` — linear RGB tint multiplied into the image. |
| `rotation` | `Option < [f32 ; 3] >` |  `xformOp:rotateXYZ`, **degrees** — spins the environment. The usual case  is yaw only (`[0, heading, 0]`). |
| `skybox` | `Option < bool >` |  `lunco:dome:skybox` — `false` lights the scene from the HDRI but leaves  the sky black. The lunar case: real bounce light, no visible sky. |

### `lunco-usd-bevy` <a id="lunco-usd-bevy"></a>

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

- *defined in:* `crates/lunco-usd-bevy/src/camera_path.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  The path prim's USD path (e.g. `/World/Shots/Shot01`), or just its leaf  (`Shot01`). |
| `action` | `CameraPathAction` |  Play, pause, or rewind. |

#### `ObserveAvatar`

 Explicitly show the local avatar camera.

- *defined in:* `crates/lunco-usd-bevy/src/camera_switch.rs`
- *fields:* none — call with `ObserveAvatar` (no params)

#### `ResumeCameraDirector`

 Return presentation ownership to the authored camera director.

- *defined in:* `crates/lunco-usd-bevy/src/camera_switch.rs`
- *fields:* none — call with `ResumeCameraDirector` (no params)

#### `SetActiveCamera`

 Switch the viewport's active camera to the `SceneCamera` whose `Name` matches.

 Works with no avatar present. `name` matches the full USD prim path *or*
 its leaf, so a cutscene can `set_camera("ChaseCam")` to reach
 `/World/Rover/ChaseCam`, or `set_camera("WideShot")` for a scene camera.

- *defined in:* `crates/lunco-usd-bevy/src/camera_switch.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Camera name (full USD prim path or its leaf). |

#### `SetUserCamera`

 Explicit operator selection of a named authored camera.

 Unlike [`SetActiveCamera`], this takes ownership from the authored director
 until [`ResumeCameraDirector`] is requested.

- *defined in:* `crates/lunco-usd-bevy/src/camera_switch.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Camera name (full USD prim path or its leaf). |

### `lunco-usd-sim` <a id="lunco-usd-sim"></a>

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

- *defined in:* `crates/lunco-usd-sim/src/cosim.rs`
- *fields:* none — call with `ClearScene` (no params)

#### `LoadScene`

 Reload (or load) a USD scene at runtime via the API.

 `curl … {"type":"ExecuteCommand","command":"LoadScene","params":{"path":"lunco://scenes/luncosim/sandbox_scene.usda"}}`

 - `path`: root-qualified USD address (`lunco://…` or `twin://…`).
 - `root_prim`: optional override for the SDF path of the prim to
   spawn. Empty (default) reads the stage's `defaultPrim` metadata;
   if absent, the scene load fails visibly; the runtime never mounts the whole
   stage at `/`.

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

- *defined in:* `crates/lunco-usd-sim/src/cosim.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Root-qualified USD address (`lunco://…` or `twin://…`). Filesystem paths  are opened through `OpenFile`, not this scene-mount command. |
| `root_prim` | `String` |  Optional override for the prim to spawn. Empty (default) reads `defaultPrim` from the stage's metadata header; a missing `defaultPrim` is a visible scene-load error. |

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

- *defined in:* `crates/lunco-usd-sim/src/cosim.rs`

| Field | Type | Description |
|---|---|---|
| `reset_document` | `bool` |  Discard the active file document's authored and runtime layers before  remounting. Callers must obtain explicit user consent first. |

## Modelica modeling & simulation

### `lunco-modelica` <a id="lunco-modelica"></a>

#### `AddCanvasPlot`

 Drop a "Scope" plot onto the active canvas.

- *defined in:* `crates/lunco-modelica/src/ui/commands/diagram.rs`

| Field | Type | Description |
|---|---|---|
| `x` | `f32` |   |
| `y` | `f32` |   |
| `width` | `f32` |   |
| `height` | `f32` |   |
| `signal` | `String` |   |

#### `AddModelicaComponent`

 Add a sub-component to a class.

- *defined in:* `crates/lunco-modelica/src/api/component.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |
| `class` | `String` |   |
| `type_name` | `String` |   |
| `name` | `String` |   |
| `x` | `f32` |   |
| `y` | `f32` |   |
| `width` | `f32` |   |
| `height` | `f32` |   |
| `animation_ms` | `u32` |  Pulse-glow duration in ms. `0` = no animation (instant). |

#### `AddSignalToPlot`

 Add one signal to an existing plot panel.

- *defined in:* `crates/lunco-modelica/src/ui/commands/plot.rs`

| Field | Type | Description |
|---|---|---|
| `plot` | `u64` |  `VizId` of the target plot panel. |
| `signal` | `String` |  Signal name to add. |

#### `ApplyModelicaOps`

 Apply a batch of Modelica document operations in one shot.

 Use this command instead of a stream of single-op commands when several
 edits belong together: they land as one undo group, and the document is
 only re-parsed once at the end. This is the structural authoring surface
 for the Modelica document; it does not attach a simulation program to USD.

- *defined in:* `crates/lunco-modelica/src/api/mod.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document to edit; unassigned (`0` over the API) = active. |
| `ops` | `Vec < ApiOp >` |  Ops to apply, in order. |

#### `AutoArrangeDiagram`

 Lay the class's components out on a deterministic grid and persist the
 positions as one undo-able batch of `SetPlacement` ops — Dymola's
 **Edit → Auto Arrange**. The passive open-time fallback stacks components at
 the origin, so this is how an imported model gets a readable diagram.

- *defined in:* `crates/lunco-modelica/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document to arrange; unassigned (`0` over the API) = active. |

#### `CancelExperiment`

 Cancel in-flight batch run(s). Signals the runner's cancel flag, which is
 honored at compile boundaries and on every solver step; the run then ends
 `Cancelled`. Target a specific run by `experiment_id`, or set `all`.

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `experiment_id` | `Option < String >` |  Cancel one run by id (uuid string). Ignored when `all` is set. |
| `all` | `bool` |  Cancel every in-flight run. |

#### `CompileModel`

 Compile a document: rumoca front-end → DAE → simulator setup. Idempotent —
 an already-compiled, unmodified model skips the worker dispatch unless
 `force`. Never changes `paused`; type/parse/DAE errors land in
 `WorkbenchState.compilation_error` and surface in the Diagnostics panel.

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  The document to compile. Unassigned (`0` over the API) means the  **active** document, which is what a toolbar click and a headless  `cmd("CompileModel", #{})` both want. |
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

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `qualified` | `Option < String >` |  Class to pick. `None` = the dialog's pre-selected candidate. |
| `cancel` | `bool` |  Dismiss the picker without running (same as the Cancel button). |

#### `ConnectComponents`

 Add a `connect(a.p, b.q)` equation to a class.

- *defined in:* `crates/lunco-modelica/src/api/diagram.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |
| `class` | `String` |   |
| `from` | `String` |   |
| `to` | `String` |   |
| `animation_ms` | `u32` |  Edge-flash duration in ms. `0` = no animation. |

#### `CreateNewScratchModel`

 Request to create a new untitled Modelica model and open its tab.

 Both fields default to `None` for the plain "New model" entry points
 (File ▸ New, the package browser, the welcome screen). The URL-share
 loader (`crate::model_share`) fires this with `source`/`name`
 populated so a shared model reuses this exact creation + tab-open
 path instead of duplicating it.

- *defined in:* `crates/lunco-modelica/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `source` | `Option < String >` |  Initial source. `None` → a minimal `model <name> end <name>;` stub. |
| `name` | `Option < String >` |  Display name, deduplicated against existing in-memory models.  `None` → the model name parsed from `source`, else an  auto-incremented "Untitled". |

#### `DeleteExperiment`

 Remove experiment record(s) from the registry. Terminal runs only —
 in-flight runs (via id / `all`) are skipped; cancel them first. Scope by
 `experiment_id`, `doc` (every run for that doc's twin), or `all`.

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `experiment_id` | `Option < String >` |   |
| `doc` | `Option < DocumentId >` |   |
| `all` | `bool` |   |

#### `DisconnectComponents`

 Delete the `connect(from, to)` equation joining two component ports. The
 inverse of `ConnectComponents`; a connection that isn't there is a logged
 no-op.

- *defined in:* `crates/lunco-modelica/src/api/diagram.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document to edit; unassigned (`0` over the API) = active. |
| `class` | `String` |  Class within the document that owns the connection. |
| `from` | `String` |  Source port, `"<component>.<port>"`. |
| `to` | `String` |  Target port, `"<component>.<port>"`. |

#### `DuplicateModelFromReadOnly`

 Duplicate a read-only (library) model into a new editable Untitled
 document. Unassigned `source_doc` (`0` over the API) means the active
 document.

- *defined in:* `crates/lunco-modelica/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `source_doc` | `DocumentId` |   |

#### `FastRunActiveModel`

 Fast Run — compile + simulate end-to-end off-thread (Web Worker on
 wasm, std::thread on native). The result is stored as an Experiment
 in [`lunco_experiments::ExperimentRegistry`]. See
 `docs/architecture/25-experiments.md`.

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |
| `class` | `Option < String >` |  Target class. When `None`, resolves via drilled-in class or picker. |
| `t_end` | `Option < f64 >` |  Override experiment StopTime (seconds). `None` = use annotation or fallback. |
| `dt` | `Option < f64 >` |  Override output interval / step (seconds, Modelica `Interval`). `None`  = use annotation or fallback. Mutually exclusive with `n_intervals`. |
| `n_intervals` | `Option < u32 >` |  Override output point count as a number of intervals (Modelica  `NumberOfIntervals`): emits `n + 1` evenly-spaced samples. The count  alternative to `dt`; when set it takes precedence and clears `dt`. |
| `tolerance` | `Option < f64 >` |  Override solver tolerance. `None` = use annotation or fallback. |
| `solver` | `Option < String >` |  Pin the solver to a registered id — `ListSolvers` enumerates them and is  the only vocabulary accepted. An unregistered id fails the run rather  than falling back, so a typo cannot silently produce numbers from a  different backend. `None`/`"auto"` lets the resolver pick from where the  run executes. |
| `h0` | `Option < f64 >` |  Override the solver's initial step (seconds). `None` = the backend's  span-based default. A diagnostic for long-horizon runs that fail at a  stiff transient near `t₀`. |

#### `FitCanvas`

 Zoom and pan the canvas so the whole diagram fits the viewport.

- *defined in:* `crates/lunco-modelica/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document to fit; unassigned (`0` over the API) = active. |

#### `FocusComponent`

 Centre the canvas on one named component — how a screenshot or a review
 walkthrough targets a specific part of a large diagram.

- *defined in:* `crates/lunco-modelica/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document to focus in; unassigned (`0` over the API) = active. |
| `name` | `String` |  Component instance name as it appears in the diagram. |
| `padding` | `f32` |  Margin in canvas units to leave around the component. |

#### `FocusDocumentByName`

 Focus the first open tab whose title contains `pattern` — the way an agent
 says "switch to the rocket model" without knowing document ids. No match is
 a logged no-op, not an error.

- *defined in:* `crates/lunco-modelica/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `pattern` | `String` |  Case-insensitive substring of the tab title. Empty = no-op. |

#### `FormatDocument`

 Run rumoca-tool-fmt on the active document.

- *defined in:* `crates/lunco-modelica/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |

#### `GetFile`

 Read a file's text and echo it to the log between `-- BEGIN --` /
 `-- END --` markers. A diagnostic for API callers that cannot see the host
 filesystem — it does NOT open a document (use `Open` for that). Goes through
 `lunco-storage`, so it works in the browser build too.

- *defined in:* `crates/lunco-modelica/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `path` | `String` |  Path to read, resolved the same way document sources are. |

#### `InspectActiveDoc`

 Dump the active document's registry state to the log — id, source length,
 parse status, linked entities. A debugging verb for "what does the app
 actually think is open?", taking no parameters because it always targets
 whatever the user is looking at.

- *defined in:* `crates/lunco-modelica/src/ui/commands/inspect.rs`
- *fields:* none — call with `InspectActiveDoc` (no params)

#### `MoveComponent`

 Move a component instance in the diagram.

- *defined in:* `crates/lunco-modelica/src/ui/commands/diagram.rs`

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

- *defined in:* `crates/lunco-modelica/src/ui/commands/plot.rs`

| Field | Type | Description |
|---|---|---|
| `title` | `String` |  Panel title. Empty = derived from `source` (`"<title> (copy)"`), or a  default when there is no source. |
| `signals` | `Vec < String >` |  Signal names to plot initially. |
| `source` | `u64` |  `VizId` of a plot to clone bindings from. `0` = start empty. |

#### `Open`

 Unified open command — dispatches on the URI scheme.

- *defined in:* `crates/lunco-modelica/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `uri` | `String` |   |

#### `OpenClass`

 Act on a class by its fully-qualified name — how the Package Browser opens
 a library model without the caller knowing document ids. `action` decides
 whether it is drilled into for viewing or copied into an editable document.

- *defined in:* `crates/lunco-modelica/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `qualified` | `String` |  Fully-qualified class path, e.g. `"Modelica.Electrical.Analog.Basic.Resistor"`. |
| `action` | `ClassAction` |  `View` (default) drills into the class; `Duplicate { name }` copies it  into a new editable document — the route for editing a read-only  library model. |

#### `OpenInNewView`

 Open the same document in a new tab (split / sibling view).

- *defined in:* `crates/lunco-modelica/src/ui/commands/lifecycle.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |

#### `PanCanvas`

 Pan the diagram canvas by an offset, leaving zoom alone.

- *defined in:* `crates/lunco-modelica/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document to pan; unassigned (`0` over the API) = active. |
| `x` | `f32` |  Horizontal offset in canvas units. |
| `y` | `f32` |  Vertical offset in canvas units. |

#### `PauseActiveModel`

 Run-control events — fire against `doc=0` to target the active
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

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |

#### `Redo`

 Redo the most recently undone edit.

- *defined in:* `crates/lunco-modelica/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |

#### `RemoveModelicaComponent`

 Remove a component instance from a class.

 Removes ONLY the declaration. Any `connect(...)` equation still naming the
 component is left behind and will fail to compile, so issue the matching
 `DisconnectComponents` calls FIRST — that is the order the canvas uses
 (orphan-edge removals precede the node removal, so rumoca can still resolve
 the connect spans). Batch both through `ApplyModelicaOps` to keep them in
 one undo group.

- *defined in:* `crates/lunco-modelica/src/api/component.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document to edit; unassigned (`0` over the API) = active. |
| `class` | `String` |  Class within the document that declares the component. |
| `name` | `String` |  Component instance name to remove. |

#### `RenameExperiment`

 Rename an experiment run in the [`ExperimentRegistry`]. Mirrors
 `DeleteExperiment`'s id-as-string addressing so the same value the UI
 holds (and API callers pass) resolves the run.

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `experiment_id` | `String` |  Target run id (the `ExperimentId`'s inner value as a string). |
| `name` | `String` |  New display name. |

#### `RenameModelicaClass`

 Rename a top-level class within an open Modelica document.

- *defined in:* `crates/lunco-modelica/src/api/class.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |
| `old_name` | `String` |   |
| `new_name` | `String` |   |

#### `ResetActiveModel`

 See [`PauseActiveModel`].

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |

#### `RestartActiveModel`

 Reset to `t=0` and run again. Composition of [`ResetActiveModel`]
 followed by [`RunActiveModel`].

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |

#### `ResumeActiveModel`

 See [`PauseActiveModel`].

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |

#### `RunActiveModel`

 Start a live realtime simulation: compile-if-stale, then play.

 This is the user-facing "Run" verb. If the model is already
 compiled and clean (same document generation), it simply unpauses —
 no recompile. Otherwise it sets [`ModelicaModel::resume_after_compile`]
 and triggers a [`CompileModel`]; the post-compile success handler in
 the worker then unpauses, so play begins as soon as the stepper is
 installed. Contrast with [`CompileModel`] (compile only, never auto-
 starts) and [`ResumeActiveModel`] (unpause only, never compiles).

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |
| `class` | `Option < String >` |  Optional explicit target class, forwarded to the compile. |

#### `RunExperiment`

 Define + dispatch a batch experiment with explicit parameter overrides,
 inputs, and bounds — the programmatic counterpart to the Experiments
 panel. Unlike `FastRunActiveModel`, overrides come from the command (not
 the UI draft), so an agent can sweep parameters without touching source.
 Discover the resulting `experiment_id` via `ListRuns` (newest, or by
 `label`); read the trajectory with `GetExperimentResult`.

- *defined in:* `crates/lunco-modelica/src/ui/commands/compile.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Target document. Unassigned → the active document. |
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
 Unassigned `doc` (`0` over the API) means the active document.

- *defined in:* `crates/lunco-modelica/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |

#### `SaveActiveDocumentAs`

 Save the document to `path`. Unassigned `doc` means the active document.

- *defined in:* `crates/lunco-modelica/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |
| `path` | `String` |   |

#### `SetDocumentSource`

 Replace an open document's entire source text.

- *defined in:* `crates/lunco-modelica/src/api/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |
| `source` | `String` |   |

#### `SetModelInput`

 Push a runtime input value into a compiled model's stepper.

 This command is owned by the UI-free Modelica core, so the same reflected
 command is available to headless API hosts, the workbench, Rhai, and any
 future transport. Its observer queues the exclusive port/model write using
 the same helper as the canvas path and reports the actual apply result.

- *defined in:* `crates/lunco-modelica/src/model_commands.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document id; zero selects the documented active-document default. |
| `name` | `String` |  Declared Modelica input name. |
| `value` | `f64` |  Runtime input value. |

#### `SetViewMode`

 Switch how a document is rendered — source text, diagram canvas, icon, or
 documentation.

- *defined in:* `crates/lunco-modelica/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document to switch; unassigned (`0` over the API) = active. |
| `mode` | `String` |  One of `"text"`, `"diagram"`, `"icon"`, or `"docs"`. Anything else  leaves the mode unchanged. |

#### `SetZoom`

 Set the diagram canvas zoom factor directly, bypassing scroll-wheel steps —
 for scripted captures that need a repeatable framing.

- *defined in:* `crates/lunco-modelica/src/ui/commands/nav.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  Document whose canvas to zoom; unassigned (`0` over the API) = active. |
| `zoom` | `f32` |  Zoom factor, `1.0` = 100%. |

#### `Undo`

 Undo the most recent edit on the active document.

- *defined in:* `crates/lunco-modelica/src/ui/commands/doc.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |   |

## Co-simulation

### `lunco-cosim` <a id="lunco-cosim"></a>

#### `ReleasePort`

 Release one manual input-port hold before its normal timeout.

 This is a discrete command beside the high-frequency [`SetPorts`] control
 stream. The reflected `Entity` field keeps API, Rhai, UI, and network
 callers on the same entity-resolution and authority path.

- *defined in:* `crates/lunco-cosim/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The entity whose hold is released. |
| `name` | `String` |  Input-port name. |

#### `SetPorts`

 The ONE generic control command: write a batch of named input ports on
 `target`, applied through [`PortRegistry::write_port`]. This is the whole of
 vessel control — there are no dedicated rover/lander command verbs and no
 axis/`VesselIntent` vocabulary. "Controlling" anything means writing its
 command input ports:
 - a wheeled rover exposes `throttle`/`steer`/`brake` (its `InputPorts`
   input surface, via the core input-port backend); a mix system projects them
   onto its actuator ports,
 - a cosim-flown lander exposes its Modelica command inputs (`throttle`/`pitch`/
   `roll`/`yaw`) via the [`SimComponent`] backend,
 - a crane/door/factory arm exposes whatever input ports it declares.

 The same command is emitted by the keyboard input path
 (`lunco-controller`), the HTTP/MCP API, scripts, and replayed remote peers —
 so every surface drives every controllable thing identically. `seq`/`tick`
 carry the prediction bookkeeping (host ack + client input log); it rides
 `SyncChannel::ControlStream` over the network.

- *defined in:* `crates/lunco-cosim/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The entity whose input ports are written. |
| `writes` | `Vec < (String, f64) >` |  `(port_name, value)` writes to apply this tick. Undeclared names are  dropped by `PortRegistry` (strict per-backend) — the write stays a no-op,  but when the target exposes a port surface WITHOUT that name the drop is  recorded once per `(entity, port)` in [`CosimDiagnostics::faults`] (M12),  so a typo'd port from the API/script/autopilot surfaces instead of  vanishing. A binding may still name ports a given vessel doesn't have. |
| `seq` | `u32` |   |
| `tick` | `u64` |   |

## Vessels, mobility & control

### `lunco-autopilot` <a id="lunco-autopilot"></a>

#### `ClearPatrol`

 Clear the patrol (or any behaviour) on `vessel` and stop it: sets the
 autopilot's behaviour to [`BehaviorSpec::Brake`] AND removes the
 [`AutopilotBehaviorSpec`] mirror from the vessel, so the route projection /
 Command Deck stop showing waypoints. The single canonical "stop & clear"
 verb — replaces the hand-built `SetAutopilotBehavior` + `Brake`-JSON dance
 that was duplicated in the Command Deck and the waypoint context actions
 (§4.2 — one input shape, every surface).

- *defined in:* `crates/lunco-autopilot/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `vessel` | `Entity` |  Vessel whose patrol to clear. |

#### `DisengageAutopilot`

 Disengage the autopilot on `vessel` WITHOUT clearing its patrol: replaces
 the live behaviour with [`BehaviorSpec::Brake`] (the vessel stops) but
 LEAVES the [`AutopilotBehaviorSpec`] mirror intact so the patrol survives a
 later re-engage. Distinct from [`ClearPatrol`] (which wipes the patrol data
 too) — the Command Deck "Disengage" button wants this one (pause driving,
 keep the route).

- *defined in:* `crates/lunco-autopilot/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `vessel` | `Entity` |  Vessel whose autopilot to disengage (brake, keep patrol data). |

#### `EngageAutopilot`

 Engage an autopilot on `vessel`: spawn an [`Autopilot`] actor (its `AiAgent`
 session claims the vessel next tick) and, if `spec_json` is non-empty, attach a
 behaviour tree. The create-an-autopilot seam for the API / rhai:
 `cmd("EngageAutopilot", #{ vessel: v, throttle: 0.5 })` or with a
 `spec_json` [`BehaviorSpec`] to navigate.

- *defined in:* `crates/lunco-autopilot/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `vessel` | `Entity` |  The vessel to put under autopilot. |
| `index` | `u64` |  Actor index within the reserved session band (distinct autopilots differ). |
| `throttle` | `f64` |  Constant forward setpoint, honoured ONLY when the vessel has no route —  i.e. explicit opt-in cruise. Omit it (0) and a routeless autopilot HOLDS  rather than driving off; with a route the behaviour tree owns the setpoint  and this is ignored either way. |
| `spec_json` | `String` |  Optional JSON [`BehaviorSpec`]; when present the autopilot navigates via the  behaviour tree instead of the constant `throttle`. |

#### `ExportBehaviorXml`

 Export a behaviour tree (JSON [`BehaviorSpec`]) to BehaviorTree.CPP v4 XML —
 the format Groot2 edits and ROS/Nav2 runs. The result is returned in the Ack
 (`xml`), so a rhai scenario or the API can convert a tree to a portable,
 editable file. Round-trips with [`ImportBehaviorXml`].

- *defined in:* `crates/lunco-autopilot/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `spec_json` | `String` |  A JSON [`BehaviorSpec`] (the same shape [`SetAutopilotBehavior`] takes). |

#### `ImportBehaviorXml`

 Import a BehaviorTree.CPP v4 XML tree back to a JSON [`BehaviorSpec`] — the
 inverse of [`ExportBehaviorXml`]. The JSON is returned in the Ack (`spec_json`)
 ready to feed [`SetAutopilotBehavior`] / [`EngageAutopilot`].

- *defined in:* `crates/lunco-autopilot/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `xml` | `String` |  A BehaviorTree.CPP v4 XML document. |

#### `SetAutopilotBehavior`

 Set (or hot-swap) an autopilot's behaviour tree from a JSON [`BehaviorSpec`].
 The dynamic authoring seam: a rhai scenario `cmd("SetAutopilotBehavior", #{
 vessel: v, spec_json: "{...}" })` defines or replaces a vessel's behaviour at
 runtime — different autopilots, updated on the fly, no rebuild. The tree is
 data, so authoring is glue; the leaves it names are the Rust primitives.

- *defined in:* `crates/lunco-autopilot/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `vessel` | `Entity` |  The vessel whose autopilot should adopt the behaviour. |
| `spec_json` | `String` |  A JSON [`BehaviorSpec`] (see its docs for the shape). |

### `lunco-controller` <a id="lunco-controller"></a>

#### `SetControlPath`

 Declare that commands can (or cannot) currently reach `target`.

 The generic verb behind [`lunco_core::session::ControlPathRegistry`]. A mission
 script computes the DOMAIN fact and states the CONSEQUENCE here; an authored
 policy ([`lunco_core::session::AUTHORIZE_HOOK`]) then decides what to refuse.
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
 "unstuck" (released). A momentary "one" press is `held:true` then `held:false`.
 The named intent is the USD control vocabulary (`forward`, `action`, `yaw_left`,
 `speed_boost`, …), parsed by [`lunco_core::parse_user_intent`]. `speed_boost` is
 consumed by the local free-flight avatar; movement intents still match whatever
 a vessel's `Controls` profile binds.

- *defined in:* `crates/lunco-controller/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `intent` | `String` |  Intent name (`forward`, `backward`, `left`, `right`, `yaw_left`, `yaw_right`, `speed_boost`, `action`, `release`, …). |
| `held` | `bool` |  `true` = hold it down, `false` = release it. |
| `target` | `Entity` |  The **entity this intent drives** (normally a vessel or avatar command  surface). An intent is meaningless without its target: two spawns of one  asset are two distinct entities, and a targetless intent is rejected. Over  the API this takes the target's `api_id` — the `GlobalEntityId` reported by  `ListEntities` — and is resolved to the live entity. |

## Avatar & possession

### `lunco-avatar` <a id="lunco-avatar"></a>

#### `FocusTarget`

 Focus on a target without taking control.

 Switches the avatar to `OrbitCamera` mode centered on the target.

- *defined in:* `crates/lunco-avatar/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `avatar` | `Option < Entity >` |  The avatar entity that is focusing (local camera representation). `None`  for headless/direct control with no avatar. |
| `target` | `Entity` |  The entity to focus on. |

#### `FollowTarget`

 Follow a target with the chase camera, without taking control.

 Inserts `SpringArmCamera` so the camera tracks the target's heading,
 but omits `ControllerLink` and vessel input bindings — keyboard input
 stays inert toward the target. Use this for non-vessel objects (balloons,
 props, observation targets) where the player wants to ride along but
 not drive. `PossessVessel` is conceptually `FollowTarget` plus a
 controller binding.

- *defined in:* `crates/lunco-avatar/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `avatar` | `Option < Entity >` |  The avatar entity that will follow (local camera representation). `None`  for headless/direct control with no avatar. |
| `target` | `Entity` |  The entity to follow. |

#### `InspectVessels`

 Diagnostic read-out of every **commandable** vessel's *control authority* state —
 the chain that decides whether the stick actually flies it:
 `GlobalEntityId` (needed for ownership + the model's `piloted` sensor),
 `ControlBinding` (intent→port map from the USD `Controls` scope), and whether
 the `SessionRegistry` currently records an owner (⇒ `piloted = 1`). Logs one
 `[inspect]` line per vessel at INFO. API-driven: `{"type":"ExecuteCommand","command":"InspectVessels"}`.

- *defined in:* `crates/lunco-avatar/src/lib.rs`
- *fields:* none — call with `InspectVessels` (no params)

#### `PossessVessel`

 Possess a vessel, taking direct control of it.

 Switches the avatar to a vessel-locked camera mode and inserts a
 `ControllerLink` so that input events are forwarded to the vessel.

- *defined in:* `crates/lunco-avatar/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `avatar` | `Option < Entity >` |  The avatar entity taking possession — this process's *local* embodiment  in the world, used only to bind the chase camera. `None` for headless or  direct API control with no camera binding. |
| `target` | `Entity` |  The non-`Avatar` entity exposing the writable `InputPorts` surface to possess (becomes the controlled vessel). |
| `bind_camera` | `bool` |  Whether possession also rebinds the avatar's camera to the chase rig —  the default, interactive behaviour. `false` claims control authority  only: what a recording scenario wants, where the script drives the  vessel through ports while an authored camera path owns the view.  A camera bind with no explicit avatar resolves only the authoritative  `TheLocalAvatar` slot. It fails visibly when that slot is empty; it never  selects an arbitrary `Avatar` entity. |

#### `ReleaseVessel`

 Release possession of the currently controlled vessel.

 Removes the `ControllerLink` and returns the avatar to free-flight mode.
 Keeps the camera at its current position — no jarring teleport.

- *defined in:* `crates/lunco-avatar/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The avatar entity releasing possession. |

#### `ReturnFromOrbit`

 Return the local camera from a celestial orbit view to the exact camera
 mode and BigSpace frame from which that view was entered.

 Unlike [`ReleaseVessel`], this is a presentation transition: it does not
 release control authority or remove a `ControllerLink`.

- *defined in:* `crates/lunco-avatar/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The local avatar camera returning from orbit view. |

#### `SetCameraInput`

 Tune pointer-to-camera response while the application is running.

 Omitted fields retain their current persisted value. The same typed
 [`crate::CameraInputSettings`] resource drives free, surface, chase, and
 body-orbit cameras; this command is the API/script boundary for changing it
 without introducing a second transient set of camera constants.

- *defined in:* `crates/lunco-avatar/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `look_radians_per_pointer_unit` | `Option < f32 >` |  Camera radians per pointer-motion unit before behavior-specific scaling. |
| `orbit_surface_min_scale` | `Option < f64 >` |  Lower bound for orbital rotation at the body's surface, in `[0, 1]`. |
| `orbit_distance_curve_exponent` | `Option < f64 >` |  Positive exponent shaping the apparent-horizon distance response. |

#### `ShowNotification`

 Show a transient on-screen notification (toast) to the player.

 Pushes onto the [`crate::ScreenNotifications`] resource; the ui-gated
 `draw_notifications` overlay renders active toasts top-center and fades them
 out. Headless hosts accept the command (and log it) but draw nothing. Fired
 from rhai via `notify(msg)` / `notify_kind(msg, kind)` (see the prelude) so a
 scenario can announce each phase without touching Rust.

- *defined in:* `crates/lunco-avatar/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `text` | `String` |  The message text. |
| `kind` | `String` |  Visual style: "info" (default), "success", "warn", or "error". |
| `secs` | `f32` |  Seconds to display; `0` uses the default (~4.5s). |

#### `UpdateProfile`

 Update the profile name for the active user session.

- *defined in:* `crates/lunco-avatar/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |   |

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

#### `ActivatePerspective`

 Activate a registered [`Perspective`](crate::Perspective) by its
 `PerspectiveId` string. The luncosim registers `sandbox_view`,
 `rover_build`, `terrain_sculpt`, and `object_builder`; the last two are
 explicit authoring modes and are hidden from the default title-bar
 switcher. Unknown ids produce a user-visible status error.

- *defined in:* `crates/lunco-workbench/src/perspective_command.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |  The id string of a registered perspective (e.g. `"rover_build"`). |

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

- *defined in:* `crates/lunco-workbench/src/screenshot.rs`

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

- *defined in:* `crates/lunco-workbench/src/screenshot.rs`

| Field | Type | Description |
|---|---|---|
| `save_to_file` | `bool` |  Write the PNG to `path` instead of returning the bytes to the caller. |
| `path` | `String` |  Destination when `save_to_file`. Empty ⇒ a timestamped name in the cwd. |
| `region` | `Vec < u32 >` |  Optional crop `[x, y, w, h]` in physical pixels, applied before save/encode. Empty ⇒  the full frame. Cropping server-side lets a caller zoom into a panel without an  external image tool. |

#### `ClearSpotlight`

 Clear any active spotlight. Rhai: `clear_spotlight()`.

- *defined in:* `crates/lunco-workbench/src/tutorial_overlay.rs`
- *fields:* none — call with `ClearSpotlight` (no params)

#### `ClearTour`

 End the guided tour (hide the coach card + scrim). Rhai: `end_tour()`.

- *defined in:* `crates/lunco-workbench/src/tutorial_overlay.rs`
- *fields:* none — call with `ClearTour` (no params)

#### `CloseWindow`

 Close the primary window (sends `AppExit::Success`).

- *defined in:* `crates/lunco-workbench/src/window_command.rs`
- *fields:* none — call with `CloseWindow` (no params)

#### `CopyShareLink`

 Produce a shareable link for the active document and copy it to the
 clipboard.

 Like [`OpenFile`], this is a typed shell command whose behaviour is
 domain-specific and lives in the domain crate
 (`lunco-modelica` encodes the active model's source into a URL
 fragment). The headless HTTP API exposes the read-only `GetShareLink`
 query separately; it returns the URL in its `data` payload instead of
 touching a clipboard.

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`
- *fields:* none — call with `CopyShareLink` (no params)

#### `FocusPanel`

 Bring a registered singleton panel forward in the dock, mounting it in its
 authored default slot when it is currently closed.

 `id` is matched against [`Panel::id`]'s static string (e.g.
 `"modelica_experiments"`, `"modelica_telemetry"`). An unregistered panel
 is a no-op; a registered closed panel is opened in its authored default
 slot.

 Exposed as a typed command so HTTP automation can deterministically
 reach a tab before screenshotting / driving it.

- *defined in:* `crates/lunco-workbench/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |  The singleton panel's [`PanelId`] string (e.g.  `"modelica_experiments"`). |

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

#### `OpenEphemeralSource`

 Open an ephemeral generated document in the read-only source viewer.

- *defined in:* `crates/lunco-workbench/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `uri` | `String` |  URI shown as the document identity. |
| `text` | `String` |  Complete generated source text. |

#### `OpenSourceView`

 Open a registered asset as read-only text in the source viewer panel.

 Fired by the LunCo Library browser section when any file is clicked —
 uniformly for every source extension (`.usda`, `.rhai`, `.mo`, `.btxml`,
 `.wgsl`), because the library is a *browse + read* surface, not a load
 surface. Distinct from [`OpenFile`](lunco_doc_bevy::OpenFile) on purpose:
 `OpenFile` is extension-routed (USD and Modelica each claim their own types
 and open their native editors), so routing the library through it would
 double-open `.usda`/`.mo` (their observers fire too). `OpenSourceView` has
 exactly one observer — the workbench source viewer — so there is no conflict.

 The command and its viewer live here because the LunCo Library is a
 workbench built-in and must behave consistently in every workbench host.

- *defined in:* `crates/lunco-workbench/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `asset_path` | `String` |  Registered `AssetFile::asset_path`; arbitrary filesystem paths are not  accepted by this library-only command. |

#### `OpenTwinSource`

 Open one file belonging to an open Twin in the editable source panel.

- *defined in:* `crates/lunco-workbench/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `twin_root` | `String` |  Absolute root of the already-open Twin. |
| `relative_path` | `String` |  File path relative to that root. |
| `pinned` | `bool` |  Keep the file open when another preview is selected. |

#### `RenameOpenDocument`

 Rename an open document (a tab in the workspace).

 Differs from [`RenameTwinEntry`]: identifies the target by
 [`DocumentId`] rather than `(twin_root, relative_path)`, so it works
 for Untitled drafts that have no on-disk path, as well as for saved
 files that belong to no open Twin.

 The observer routes by [`DocumentOrigin`]:

 - `File { writable: true }` *under an open Twin*: forwards to
   [`RenameTwinEntry`] — same on-disk path, same `FileRenamed` chain,
   same Modelica class-name rewrite.
 - `Untitled { name }`: domain crates observe this command directly
   (Modelica chains to [`RenameModelicaClass`]) — workbench has no
   semantic handle on what an Untitled draft means.
 - `File { writable: false }` or `Bundled`: read-only, rejected.

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `lunco_doc :: DocumentId` |  The document to rename. |
| `new_name` | `String` |  New filename / class identifier — no path separators allowed. |

#### `RenameTwinEntry`

 Rename a file or folder *inside* an open Twin.

 Identifies the entry by `(twin_root, relative_path)` so the
 command body is self-contained (no Bevy resource handles) — HTTP
 callers, scripts, and the inline browser editor all dispatch the
 same shape. The observer:

 1. Validates inputs (new_name non-empty, no path separators, source
    exists, target doesn't already exist).
 2. Asks [`lunco_storage`] to rename backend handles for the absolute paths.
 3. Re-scans the affected Twin via [`Twin::reload`] so the file
    index reflects disk.
 4. Patches every open Document whose `DocumentOrigin::File { path }`
    lay under the old path — paths are rewritten so live edits don't
    detach from disk.
 5. Fires [`FileRenamed`] for domain plugins to chain follow-ups
    (Modelica class-declaration rename, USD reference rewrites, …).

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`

| Field | Type | Description |
|---|---|---|
| `twin_root` | `String` |  Absolute path of the Twin root the entry belongs to. The  observer resolves this back to a `TwinId` via  [`WorkspaceResource::twins`]. |
| `relative_path` | `String` |  Path of the entry relative to `twin_root` (e.g. `Rover.mo` or  `subdir/Other.mo`). |
| `new_name` | `String` |  New filename — no path separators allowed (rename only; move  across directories is a separate concern). |

#### `ResetWorkspaceLayout`

 Reset the dock layout to the active perspective's clean preset — the
 recovery hatch when a stale persisted layout drops a panel (e.g. the 3D
 Viewport, which leaves the centre blank and the camera inactive). Exposed on
 the API bus and as the **View ▸ Reset Layout** menu item.

- *defined in:* `crates/lunco-workbench/src/perspective_command.rs`
- *fields:* none — call with `ResetWorkspaceLayout` (no params)

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

#### `SaveSourceText`

 Persist the editable source buffer, optionally refreshing its owning domain.

- *defined in:* `crates/lunco-workbench/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `twin_root` | `String` |  Absolute root of the already-open Twin. |
| `relative_path` | `String` |  File path relative to that root. |
| `text` | `String` |  Complete UTF-8 source text. |
| `update` | `bool` |  Re-dispatch `OpenFile` after writing so the owning domain updates. |

#### `SetHint`

 Set the persistent one-line hint. Empty `text` clears it. Rhai: `hint(msg)`
 / `clear_hint()`.

- *defined in:* `crates/lunco-workbench/src/tutorial_overlay.rs`

| Field | Type | Description |
|---|---|---|
| `text` | `String` |  Instruction text; empty hides the hint line. |

#### `SetObjectives`

 Set the persistent objectives checklist. `text` is a pre-formatted block
 (one objective per line). Empty clears it. Rhai: `objectives_hud(list)` —
 the prelude formats the list into this block and also auto-publishes it from
 declarative `mission(me)` state.

- *defined in:* `crates/lunco-workbench/src/tutorial_overlay.rs`

| Field | Type | Description |
|---|---|---|
| `text` | `String` |  Pre-formatted checklist block; empty hides the objectives card. |

#### `SetTheme`

 Set or toggle the active theme mode. Omit `mode` to toggle.

- *defined in:* `crates/lunco-workbench/src/theme_command.rs`

| Field | Type | Description |
|---|---|---|
| `mode` | `Option < String >` |  `"dark"` / `"light"` (case-insensitive). When `None`, toggles. |
| `persist` | `Option < bool >` |  `false` = apply for this session only, leave `settings.json` alone.  Default `true` (the historical behavior). |

#### `SetTourStep`

 Show a guided-tour coach step: spotlight `anchor`, and draw a coach card with
 `title`/`body`, progress dots (`index`/`total`), and Back/Next/Skip controls.
 Rhai: `coach(index, total, anchor, title, body)`. The controls emit
 `cmd:TutorialNext` / `cmd:TutorialBack` / `cmd:TutorialSkip` on the event bus,
 which the tour script advances on (a script can simulate a click with
 `emit("cmd:TutorialNext", 0)`).

- *defined in:* `crates/lunco-workbench/src/tutorial_overlay.rs`

| Field | Type | Description |
|---|---|---|
| `index` | `i64` |  0-based step index (progress dots). |
| `total` | `i64` |  Total step count. |
| `anchor` | `String` |  `HelpAnchors` key to spotlight; empty = centred card. |
| `title` | `String` |  Coach-card banner title. |
| `body` | `String` |  Coach-card body text. |

#### `ShowOpenFilePicker`

 Request a system "Open File" dialog.

 Dispatches [`ShowOpenFilePicker`] which triggers the picker via
 [`crate::picker::PickHandle`]. On success, the picker resolves to
 [`OpenFile`] with the chosen path.

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`
- *fields:* none — call with `ShowOpenFilePicker` (no params)

#### `ShowOpenFolderPicker`

 Request a system "Open Folder" dialog.

 Dispatches [`ShowOpenFolderPicker`] which triggers the picker via
 [`crate::picker::PickHandle`]. On success, the picker resolves to
 [`OpenFolder`] with the chosen path.

- *defined in:* `crates/lunco-workbench/src/file_ops.rs`
- *fields:* none — call with `ShowOpenFolderPicker` (no params)

#### `SimulateInput`

 Command to simulate a keyboard input for the overlay.

- *defined in:* `crates/lunco-workbench/src/input_overlay.rs`

| Field | Type | Description |
|---|---|---|
| `key` | `String` |   |
| `pressed` | `bool` |   |

#### `SimulatePointer`

 Drive the existing window/picking cursor path from a typed command.

 This writes the same complete input message fan-out that the native backend
 emits. Consequently every normal consumer sees it: egui palette buttons,
 hover/picking, Spawn Ghost, Spawn placement, waypoint placement, selection,
 and any future cursor-driven tool. It is not a parallel "spawn at
 coordinates" API.

- *defined in:* `crates/lunco-workbench/src/input_overlay.rs`

| Field | Type | Description |
|---|---|---|
| `x` | `f32` |  Logical-pixel position in the primary window. |
| `y` | `f32` |  Logical-pixel position in the primary window. |
| `click` | `bool` |  Emit a primary-button press and release after moving to `(x, y)`. |

#### `Spotlight`

 Spotlight a workbench widget by its [`HelpAnchors`](crate::HelpAnchors) key,
 dimming everything else. Rhai: `spotlight(anchor, caption)`.

- *defined in:* `crates/lunco-workbench/src/tutorial_overlay.rs`

| Field | Type | Description |
|---|---|---|
| `anchor` | `String` |  The `HelpAnchors` key of the widget to highlight (e.g. `"twin_browser"`). |
| `text` | `String` |  Optional caption shown in the callout. Empty = no caption text. |

#### `StartOfflineRecording`

 Command to start frame-by-frame recording.

- *defined in:* `crates/lunco-workbench/src/screenshot.rs`

| Field | Type | Description |
|---|---|---|
| `output_dir` | `String` |  Target folder. Empty => 'recorded_frames' in the current working dir. |
| `fps` | `u32` |  Video target FPS (default: 60). |

#### `StopOfflineRecording`

 Command to stop frame-by-frame recording.

- *defined in:* `crates/lunco-workbench/src/screenshot.rs`
- *fields:* none — call with `StopOfflineRecording` (no params)

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

#### `TutorialBack`

 Return to the previous guided tutorial step.

- *defined in:* `crates/lunco-workbench/src/tutorial_overlay.rs`
- *fields:* none — call with `TutorialBack` (no params)

#### `TutorialNext`

 Advance a guided tutorial step through the shared typed-command bus.
 The command projector supplies the established `cmd:TutorialNext` event
 consumed by authored Rhai tours.

- *defined in:* `crates/lunco-workbench/src/tutorial_overlay.rs`
- *fields:* none — call with `TutorialNext` (no params)

#### `TutorialSkip`

 Stop the current guided tutorial tour.

- *defined in:* `crates/lunco-workbench/src/tutorial_overlay.rs`
- *fields:* none — call with `TutorialSkip` (no params)

## Tutorials & HUD

### `lunco-tutorial` <a id="lunco-tutorial"></a>

#### `ResetTutorialProgress`

 Clear persisted completion and first-run state without changing the loaded
 scene. This is the explicit recovery path for a shared settings file whose
 tutorial history no longer matches the user's current installation.

- *defined in:* `crates/lunco-tutorial/src/lib.rs`
- *fields:* none — call with `ResetTutorialProgress` (no params)

#### `SetSubsystemEnabled`

 Enable/disable a simulation subsystem at runtime (progressive fidelity).
 `name` must be registered by the owning subsystem plugin. Rhai:
 `set_subsystem(name, on)`.

- *defined in:* `crates/lunco-tutorial/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Registered subsystem key. |
| `on` | `bool` |  `true` enables, `false` disables. |

#### `SkipTutorial`

 Stop the current tutorial: clear the HUD, synchronously stop its host, and
 clear a world declared by that lesson through the normal scene lifecycle.
 A UI-only lesson has no scene of its own to clear.
 `cmd("SkipTutorial")`.

- *defined in:* `crates/lunco-tutorial/src/lib.rs`
- *fields:* none — call with `SkipTutorial` (no params)

#### `StartTutorial`

 Start a tutorial by id: resolve its authored scenario, mount its declared
 world if any, and run it on the host after the scene transaction completes.
 The single launch path — menu, F1, HTTP API, MCP, and other scripts
 (`cmd("StartTutorial", #{ id })`) all route here.

- *defined in:* `crates/lunco-tutorial/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |  The [`TutorialMeta::id`] to start. |

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
| `doc` | `DocumentId` |  The document to close. |

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

 The actual loading is domain-specific: `lunco-modelica` observes this
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

 Request to redo the last undone op on the document.

 Counterpart of [`UndoDocument`]. Same per-domain dispatch rules.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  The document whose most recent undone op should be re-applied. |

#### `SaveAsDocument`

 Request the owning domain persist the document **to a new location**.

 `path` semantics mirror [`OpenFile`]:

 - **Empty** → the observer fires
   [`lunco_workbench::picker::PickHandle`](../lunco_workbench/picker/struct.PickHandle.html)
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
| `doc` | `DocumentId` |  The document to persist. |
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
| `doc` | `DocumentId` |  The document to persist. |

#### `UndoDocument`

 Request to undo one op on the document, syncing any dependent UI
 state (editor buffer, diagram canvas) to match the reverted source.

 Handled per-domain: the registry that owns `doc` runs its
 [`DocumentHost`](lunco_doc::DocumentHost)`::undo()`, fires
 [`crate::DocumentChanged`], and performs whatever view-state sync the
 domain requires (e.g. for Modelica, update the text buffer). Domains
 that don't own `doc` ignore the trigger.

- *defined in:* `crates/lunco-doc-bevy/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `doc` | `DocumentId` |  The document whose most recent op should be undone. |

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

 This is the architecture that keeps time correct across scene reloads: a scene may
 have detached the celestial clock, run it at 100 000×, scrubbed the animation
 preview or paused the transport, and none of that may bleed into the next scene.
 Rather than have each subsystem remember to undo its own edits, one command
 restores the standing shape (doc 19 §11b):

 * **celestial** → back on the `Epoch` root, affine identity (re-coupled to the sim,
   so a sky left running at 100 000× stops the instant the scene reloads);
 * **interaction** → wall-rooted identity (its default);
 * **animation preview** → playhead 0, playing, 1×;
 * **transport** → Playing at 1×;
 * **mission calendar** → the authored mission origin, with any kinematic warp
   preview cleared. The mission origin itself is preserved so a scene load can
   apply its `SetMissionEpoch` afterward.

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
 `{"rate":4.0}` runs it 4× realtime. This is THE pause command: exposed on the
 API/MCP and wrapped by the rhai prelude verbs `pause()`/`play()`/`set_rate()`,
 so a cutscene or a "reload-then-pause" one-liner can freeze the world.

- *defined in:* `crates/lunco-time/src/domain.rs`

| Field | Type | Description |
|---|---|---|
| `playing` | `Option < bool >` |  Play (`Some(true)`) / pause (`Some(false)`); `None` leaves it. |
| `rate` | `Option < f64 >` |  Speed multiplier vs realtime (1.0 = realtime); `None` leaves it. |

## Celestial, environment & comms

### `lunco-celestial` <a id="lunco-celestial"></a>

#### `LeaveSurface`

 Leave the current body's surface and return to orbit view.

 Opens a transactional `OrbitCamera` view in the body's explicit star-fixed
 orbit grid. Returning restores the avatar's exact prior surface frame.

- *defined in:* `crates/lunco-celestial/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The avatar entity leaving the surface. |

#### `SetLinkCadence`

 Set the connectivity recompute cadence at runtime (any client / language).

- *defined in:* `crates/lunco-celestial/src/link.rs`

| Field | Type | Description |
|---|---|---|
| `interval_s` | `f64` |   |

#### `TeleportToSurface`

 Teleport the avatar to a celestial body's surface.

 Places the camera on the body's Grid in surface-relative mode.

- *defined in:* `crates/lunco-celestial/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The avatar entity to teleport. (`Entity` → the id codec converts this  gid↔local automatically; see `crates/lunco-networking/PH2_ID_CODEC.md`.) |
| `body_entity` | `u64` |  The body to teleport to, carried as raw local `Entity::to_bits()` and  reconstructed in the observer.   `u64`, not `Entity` — a variant of "**Pattern B**": the type-driven id  codec converts only `Entity`-typed fields, so this `u64` opts out and is  handled by hand. Unlike `MoveEntity::entity_id` (an `api_id` resolved via  `ApiEntityRegistry`) this is a *local* entity bit-pattern, so it is only  meaningful in-process. Left as-is by choice; the codec ignores it. |

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
| `window_m` | `f32` |   |
| `target_res` | `u32` |  Visual-quality downsample target (samples per side). `0` = native (no  decimation). Re-issue the command with a different value to rebuild the  same site at another quality and compare. |
| `lod_viz` | `bool` |  Stream camera-driven CDLOD tiles (procedural-regolith geomorph) instead of  one static mesh; collider/physics unchanged. Production visual path. |
| `collider_ring` | `bool` |  Stream a canonical-res collider ring around runtime physical support  footprints instead of one static full-DEM collider (replaces it — physics  rides the streamed tiles). |
| `collider` | `TerrainColliderSettings` |  Physics-only collider-ring lattice (`max_depth` and `tile_resolution`); independent of graphics quality and headless/rendered execution. |
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
 (`lunco_modelica::ui::commands::util`). **On a host with no window there is
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

#### `SpawnEntity`

 Spawn an independent entity from the catalog at a given world position. Route
 markers are not independent entities and are rejected here; use
 `AddRuntimeWaypoint` with an explicit vessel for a runtime route member.

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
 expressed in the current semantic [`crate::ActivePhysicsFrame`]; callers
 never pass a Bevy grid entity or perform BigSpace hierarchy conversion
 themselves.

- *defined in:* `crates/lunco-core/src/commands.rs`

| Field | Type | Description |
|---|---|---|
| `entry_id` | `String` |  The independent catalog entry ID (e.g. "ball_dynamic", "skid_rover"). |
| `position` | `[f64 ; 3]` |  Position in the active physics frame, in metres. Kept as f64 through  command transport and frame conversion; narrowing occurs only at the  final scene-root-local Bevy `Transform` boundary. |
| `rotation` | `Option < [f64 ; 4] >` |  Rotation in the active physics frame as an `(x, y, z, w)` unit  quaternion (optional; omitted → identity). Kept as f64 across the  command boundary for the same reason as `position`; Bevy's f32  [`bevy::prelude::Quat`] is a render/local-transform representation, not a  simulation-frame interchange type. |

## Other (source location unknown)

### `lunco-assets` <a id="lunco-assets"></a>

#### `CancelDataset`

 Cancel a declared dataset download. The operation remains owned until its
 worker returns, after which the row becomes requestable again.

- *defined in:* `crates/lunco-assets/src/datasets.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |  Globally unique dataset id from [`DatasetEntry::id`]. |

#### `RequestDataset`

 User intent to start a declared dataset download.

 The UI emits this event instead of mutating [`DatasetRegistry`] directly;
 the registry remains the only owner of download authorisation and task
 lifecycle.

- *defined in:* `crates/lunco-assets/src/datasets.rs`

| Field | Type | Description |
|---|---|---|
| `id` | `String` |  Globally unique dataset id from [`DatasetEntry::id`]. |

### `lunco-luncosim` <a id="lunco-luncosim"></a>

#### `SaveScenario`

 Save a live-edited rhai scenario's current source back onto the `LunCoProgramAPI`
 prim it came from — the other half of scenario authoring.

 The write converts that prim to the selected inline `info:sourceCode` arm and clears
 the old `info:id`/`info:sourceAsset` arms. The shared lowering authors the `string`
 value RAW, so the whole rhai source round-trips verbatim, journals like any edit, and
 reaches the `.usda` on `SaveDocument`.

 It authors onto the PROGRAM, not onto the vessel running it
 ([`ScenarioProgramPrim`](lunco_core::ScenarioProgramPrim) carries the path): a
 vessel can run several programs, and a source written onto the vessel would sit on
 a prim that runs nothing.

 Only doc-backed twin scenes have an editable document; a raw-file scene is
 **refused** (logged, not silently dropped) — matching the rule that the builder
 must only edit doc-backed scenes or it eats work on the next reload.

- *defined in:* `crates/lunco-luncosim/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `target` | `Entity` |  The scripted entity whose live scenario source to persist onto its prim.  Ownership-gated (same as `RunScenario`): saving a scenario is editing it. |

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

- *defined in:* `crates/lunco-luncosim/src/lib.rs`

| Field | Type | Description |
|---|---|---|
| `name` | `String` |  Prim name under the mounted scene's `Policies` scope (the identity for  hot-replace); defaults to a sanitized `seam` when empty. |
| `seam` | `String` |  The hook seam (id): e.g. `"journal.merge.order"`, `"rbac.authorize"`, a  `lunco:driveKernel` id a rover points at, or `"synth.<name>"` for a  generated Modelica source/unit/layout policy. |
| `entry` | `String` |  The rhai entry function name. |
| `source` | `String` |  The rhai source defining `entry` (+ helpers). |
| `deterministic` | `bool` |  Deterministic (fresh rhai scope per invoke). Convergent seams (merge, drive)  must be `true`; the host-only authorize gate may be `false`. |

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

### `lunco-viz` <a id="lunco-viz"></a>

#### `SetTelemetryBrowserView`

 Select the telemetry browser's signal filter and focused signal.

- *defined in:* `crates/lunco-viz/src/telemetry_browser.rs`

| Field | Type | Description |
|---|---|---|
| `filter` | `String` |   |
| `signal` | `String` |   |

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

---

<!-- 192 commands from the runtime schema; scanned 674 .rs files for docs (0 parse failure(s) skipped).
     `#[Command]` in source but NOT in the runtime schema — test fixtures, hidden
     (`ApiVisibility::hide`), or never registered; deliberately not documented: Collision, HiddenCommand, InternalEvent, JoinServer, LeaveServer, PluginCommand, PromoteScenario, RecoverVessel, ReflectedEvent, RunPython, ScriptOpenCommand, ScriptOwnedCommand, SetActiveUsdViewport, SetAllowFreeMovement, SetFollowMode, SetFollowOptIn, SetObserveMode, SetTargetClient, SetTeachMode, SetVisualLead, SharePerspective, TestEcho
-->
