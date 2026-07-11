# 48 — Object Builder

> Renumbered 45 → 48 (2026-07-11): the big_space series (44–47) already owned 45
> (`45-big-space-correct-usage.md`). No content change.

Design analysis for an in-app tool that builds and edits simulation objects: a canvas
for wiring connections, an editor for the Rhai behaviour attached to a prim, a USD prim
tree with derived parameter editors, and assembly-from-components — reconfiguring a
rover's bogie, or building a new rover or robotic arm out of parts.

This is analysis, not a plan of record. It exists to answer one question before any
code is written: **what actually has to be built, versus what already exists and merely
has to be connected?**

The answer is that roughly four-fifths of the substrate is already there, one of the
hard problems has no solution at all today, and there is a performance cliff sitting
directly in the path of the most-requested feature.

---

## 1. What already exists

Each of these is load-bearing for the tool and needs no new foundation.

**A generic node-graph canvas.** `crates/lunco-canvas` is already substrate, not a
Modelica editor. `Node { kind: SmolStr, data: Box<dyn Any>, ports }` (`scene.rs:295`)
with a `VisualRegistry` that rebuilds visuals from `(kind, data)` on load. The Modelica
diagram is one *projector* on top of it (`kind = "modelica.icon"` / `"modelica.connection"`).
The crate doc names future non-Modelica hosts explicitly. A USD connection canvas is a
second projector, not a second canvas.

**Typed, invertible USD edit ops.** `UsdOp` (`crates/lunco-usd/src/document.rs:209`) has
eleven variants — `AddPrim` (with a `reference` arc), `RemovePrim`, `MovePrim`,
`SetAttribute`, `SetTranslate`, `SetRotate`, `SetRelationship`, `SetConnection`,
`SetTimeSample`, `RemoveTimeSample`, `ReplaceSource`. Every one carries an
`edit_target: LayerId` and every one returns its inverse when applied.

**A two-layer document.** `base ⊕ runtime` (`document.rs:424`). The base is the authored
`.usda`; the runtime layer is an overlay that never touches the base file. Composition is
memoized by generation.

**Incremental reprojection into the live world.** `sync_twin_overlays`
(`crates/lunco-usd/src/twin_projection.rs:290`) replays the document's typed op log onto
the live composed stage; openusd's change sink drains into ECS spawn/despawn/transform
reconciliation. Only four ops force a full rebuild (below).

**A canonical journal.** `crates/lunco-twin-journal` stores `op + inverse` per entry in a
causal DAG with `(lamport, author)` tie-break, persists to disk, replays deterministically,
and merges across peers. USD ops already journal automatically — any `DocumentHost` with a
`JournalOpRecorder` attached records on `apply`. **Nothing needs to be done to make Object
Builder edits journal.** They will journal because they are `UsdOp`s.

**A derivation-based Inspector.** `crates/lunco-sandbox-edit/src/ui/inspector.rs` already
discovers what is editable from which components an entity carries, and writes back through
the correct layer per domain — Modelica params via `ModelicaOp` + recompile, USD attrs into
the runtime layer, joint setpoints via port writes.

**Real USD variant sets.** Not aspirational: `skid_rover.usda:47-51` declares
`variantSets = "drivetrain"` with `raycast` / `physical` selections that swap the entire
drivetrain component. `ackermann_rover.usda` does the same. The composition engine resolves
them; Rust reads the flattened result.

---

## 2. The edit-and-reboot loop already works — for twin-backed scenes

This was worth checking, because it looked like the weak point and is not.

`RestartScene` (`crates/lunco-usd-sim/src/cosim.rs:1186`) calls `asset_server.reload(ap)`
with the comment *"Force a fresh disk read so on-disk edits actually apply."* Read alone,
that says in-memory edits are lost on reboot.

They are not, because it deliberately reuses the stage **handle**, preserving the
`twin://` scheme (`cosim.rs:1198-1207`). A `twin://` path does not resolve to the raw file —
it resolves through `TwinRoots`, which serves the *composed* `base ⊕ runtime` bytes that
`drain_pending_twin_docs` published (`twin_projection.rs:255`) and `sync_twin_overlays`
keeps current. The runtime layer additionally persists to `<twin-root>/.lunco/runtime/…`
(`runtime_persistence.rs:39`), so edits survive not just reboot but process exit.

So: **edit → journal → reboot → edits still there** is a property the system has today, and
it holds precisely for scenes opened as documents under a Twin. It does *not* hold for a
scene loaded as a plain file path, which reloads raw base bytes and drops the runtime layer.

The design consequence is a rule, not a feature: **the Object Builder must only ever operate
on doc-backed twin scenes.** If it can open a raw file, it silently eats the user's work on
the first reboot. Enforce it at open time rather than discovering it in a bug report.

(Minor inconsistency to resolve while there: `journal_persistence.rs:40` writes
`history/journal.json`, while `runtime_persistence.rs:38` describes it as `.lunco/journal/`.
The comment is wrong or the path is; they should agree.)

---

## 3. The real gaps

### 3.1 There is no spatial component interface — this is the hard one

The user's ask is "build from components… change rover bogie configuration… build new
rovers/robotic hands." Today that is not a UI problem. It is a data-model problem.

A component is attached by hand-authoring **two independent sets of coordinates that
nothing checks against each other**:

1. the referencing prim's own `xformOp:translate` — where the part sits, and
2. a separate joint prim's `physics:localPos0` / `localPos1` — where the constraint anchors.

`imu.usda:10-13` states this outright. `rocker_bogie.usda:649-763` pays the price: ten
explicit revolute joints, each with hand-written anchor numbers. Moving a wheel means
editing its transform and then, in a different part of the file, the joint's anchor, and
getting both right. Nothing validates them. Nothing derives one from the other.

Grep confirms there is no `lunco:mount`, no attachment frame, no socket, nothing spatial:

```
rg 'lunco:mount|mount_frame|attach_frame|socket'   → 0 hits in assets, 0 relevant in rust
```

You cannot build a "snap a wheel onto a bogie" tool on top of that, because there is
nothing on either part that says *where a wheel goes* or *what kind of joint belongs there*.
Every assembly UI would have to invent the numbers, which is exactly what the human is
doing now.

**Proposal — declare mounts in USD.** A host advertises sockets; a component advertises
the frame by which it attaches.

```usda
def Xform "Mounts"
{
    def Xform "wheel_fl" (kind = "mount")
    {
        uniform token  lunco:mount:socket = "wheel"      # what may attach
        uniform token  lunco:mount:joint  = "revolute"   # the constraint it implies
        vector3f       lunco:mount:axis   = (1, 0, 0)
        double3 xformOp:translate = (1.2, -0.3, 0.9)     # the frame itself
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
}
```

and on the component's `defaultPrim`:

```usda
uniform token lunco:mount:plug = "wheel"   # what socket this fits
rel lunco:mount:frame = </Wheel/Mounts/hub>
```

Then a new op — call it `AttachComponent { edit_target, socket_path, asset, prim, name }` —
is a **macro that lowers to existing ops**:

- `AddPrim { reference }` for the part,
- `SetTranslate` / `SetRotate` computed so the plug frame coincides with the socket frame,
- `AddPrim` for the joint, with `localPos0`/`localPos1` and `physics:axis` *derived from the
  two frames* rather than typed by a human.

Its inverse is two `RemovePrim`s, so it journals and undoes like everything else. Bogie
reconfiguration becomes "move the socket"; the joint anchor follows, because it was never an
independent number.

This is the single highest-leverage change in this document, and the only one that requires
inventing schema rather than wiring existing parts.

**Status — the lowering landed; the mount-frame layer is deferred by design.**
`AttachComponent` exists as a command (`crates/lunco-usd/src/commands.rs`) over a pure
op-lowering (`crates/lunco-usd/src/attach.rs`, `attach_component_ops`). It removes the
duplicate-number problem *today*: given a placement, it references the part in, places it, and
authors the joint with `localPos0 = placement`, `localPos1 = origin` — the exact convention
every joint in `rocker_bogie.usda` already follows (`localPos1 = (0,0,0)` throughout). The
anchor is derived, never retyped. It is unit-tested at the op level (five tests): anchor
derivation, body-relating, axis-per-joint-kind, joint-type mapping, and apply-order safety.

What is **deliberately not** in v1: rotated placement and socket/plug frame matching. Those
compute a *placement* from two `lunco:mount:*` frames and then call this same lowering — but a
wrong frame conversion is a physics bug visible only with the renderer running, so the frame
math is held until it can be validated in-app rather than shipped blind. The schema above is
the spec for that layer; the lowering beneath it is done and tested.

### 3.2 Four missing UsdOps — DONE

All four now exist on `UsdOp` (`crates/lunco-usd/src/document.rs`), each with a typed or
snapshot inverse, so all four journal and undo like every other op:

- **`SetVariantSelection { path, variant_set, variant }`** — read-modify-writes the
  `variantSelection` map so a sibling set's selection survives. "Change the rover bogie
  configuration" is now one op. Recomposes a subtree → projector rebuilds.
- **`SetApiSchemas { path, schemas }`** — explicit-list-op author of `apiSchemas`, so a
  runtime-built prim can be made a rigid body / collider. Projector **rebuilds** (see §3.3).
- **`SetPayload { path, asset_paths }`** — explicit-list-op author of `payload`. Recomposes
  a subtree → projector rebuilds.
- **`SetActive { path, active }`** — non-destructive "disable this part"; snapshot inverse
  (NOT `!active`, which would mis-undo a no-op deactivation). Projector **rebuilds** (§3.3).

### 3.3 The performance cliff — RESOLVED

`op_needs_rebuild` (`twin_projection.rs`) used to force a full scene recompose for
`SetRelationship`. A physics joint authors `rel physics:body0` and `rel physics:body1` — two
`SetRelationship`s — so **every component attach rebuilt the whole world**, twice per joint.

Both `SetRelationship` and `SetConnection` now have incremental live-stage authors
(`CanonicalStage::author_relationship` / `author_connection`) and were removed from the
rebuild set. `SetRelationship` refreshes only the owning prim's subtree (or, for
`material:binding`, fans out to scene visuals since a binding reaches meshes anywhere).

**A second, silent bug surfaced while fixing this:** `SetConnection` was *classified* as
incremental but had **no arm** in `apply_incremental_op_to_stage` — it fell through `_ => {}`.
So every cosim wire authored at runtime reached the document and never the live stage: a
dropped edit that only appeared after the next unrelated full rebuild. Now authored and
refreshed. This is exactly the wire-drawing path the connection canvas (§4) depends on.

Four ops still rebuild, correctly: `SetVariantSelection` and `SetPayload` (value resolution
re-composes the affected subtree wholesale, which the incremental sink can't express), and —
found during verification — `SetApiSchemas` and `SetActive`. The incremental subtree refresh
(`reinstantiate_entity`) only re-derives an entity's *visual*; it does not re-run physics
extraction or despawn the entity. So an apiSchema change wouldn't actually make a prim a rigid
body at runtime, and `SetActive(false)` wouldn't remove its entity — the visual-only refresh is
the wrong tool. A rebuild re-derives the physics component set and the active-prim set correctly.
This does **not** touch the attach hot-path: `AttachComponent` emits neither op, so building a
vehicle from parts stays rebuild-free (its `AddPrim`/`SetTranslate`/`SetRelationship`/
`SetAttribute` ops all replay incrementally).

Historical note — the two ways out that were weighed:

- Give `SetRelationship` an incremental live-stage author (it is absent, not impossible —
  the stage API has `author_reference`, so relationship authoring is the same shape), or
- Batch a macro op into one rebuild by suppressing reprojection until the macro completes,
  which is cheaper to implement and strictly worse for interactivity.

The first is correct. The second is a fallback.

### 3.4 Undo is built and unwired

`UndoManager` (`lunco-twin-journal/src/lib.rs:1473`) has per-author stacks, `UndoScope::{Document, Twin}`,
`take_undo`/`take_redo`. Grep finds it instantiated **only in its own crate's tests**. Meanwhile
`DocumentHost` keeps a separate live inverse-op stack that *is* wired.

Two undo systems, unreconciled — this is gap #3 in `docs/architecture/18-unified-journal-and-history.md`.
No snapshots are needed to close it: every journal entry already stores its inverse. The work is
deciding which stack is authoritative and honouring author-scope so undo in a networked session
doesn't revert a peer's edit.

An object builder without undo is a toy. This must land.

### 3.5 Parameters are an untyped string

`lunco:params = "rest_altitude=1.5, kv=1.2"` is split on `,`, then `=`, then
`parse::<f64>()` — **non-numeric values are silently dropped** (`lunco-usd-bevy/src/lib.rs:910-925`).
It feeds `param(me, key, default)` in Rhai and nothing else. It is not a USD-attr or
Modelica-param override channel.

Typed per-component config already exists the right way, as real USD attributes
(`wheel.usda:42-51`: `double lunco:springStiffness`, `lunco:motorPower`, …). The gap is that
nothing declares their bounds, units, or documentation, so an editor cannot derive a control.

USD gives this for free via per-attribute `customData`:

```usda
double lunco:springStiffness = 1200 (
    customData = { double min = 0; double max = 5000; string unit = "N/m" }
)
```

The Inspector already iterates discovered parameters; extending it to read `customData` for
range and unit turns every existing component into an editable one with no per-type UI code.
That preserves the project's standing rule that the Inspector *derives* and never hardcodes.

### 3.6 No prim tree, no script editor, no script diagnostics UI

- The Twin Browser shows **files, not prims** (`ListTwinProvider` returns `twin.files()`).
  There is no USD prim tree anywhere in the UI.
- There is a Modelica code editor with a hand-rolled syntax layouter
  (`code_editor.rs:673`) and **no editor for `.rhai` or `.usda`** — only a plain-TextEdit
  REPL panel.
- Script diagnostics carry line and column (`ScriptStatus` → `{severity, message, line, col}`)
  and **nothing consumes them**. Modelica has a diagnostics panel with click-to-source; scripting
  has no analogue. The data exists; the consumer doesn't.
- Live-edited scenarios **can now be saved back to their USD prim** — DONE (was the
  `lunco-scripting/src/commands.rs:198` TODO; see §3.7). The remaining gap is UI to invoke it.

Also worth stating plainly, because it will otherwise arrive as a bug report: a scenario's
per-entity `this` state is wiped on hot-reload and on scene restart, by design
(`scenario.rs:320`, `world_bridge.rs:924-957`). Reboot means behaviour restarts from scratch.

### 3.7 Save a live-edited scenario back to its prim — DONE

The TODO said this was "BLOCKED on a USD bridge that must be built." The bridge was already
built by the twin-projection work: `DocBackedTwinScenes` maps a running scene's
`twin://<name>/<rel>` stage to its editable `UsdDocument`. So the save is now three pieces:

- **`lunco_usd::twin_projection::scene_document_for(backed, asset_server, scene_asset)`** — the
  asset↔document bridge. A runtime entity carries a `UsdPrimPath { stage_handle, path }`; this
  maps that stage handle to the editable `DocumentId` (or `None` for a raw-file scene, which has
  no savable document — so it is refused, never silently dropped).
- **`SaveScenario { target }`** (in `lunco-sandbox`, the only crate that depends on both
  `lunco-usd` and `lunco-scripting` — `lunco-usd-sim` can't, it would be circular). It resolves
  the entity's live source (`ScriptRegistry`), its prim path, and the backing document, then
  authors `lunco:script` onto the root layer via `ApplyUsdOp` — so it journals, and
  `SaveDocument` writes it through to the `.usda`.
- **String authoring is one architectural rule, not per-call-site escaping.** `SetAttribute`
  with `type_name == "string"` authors the value **raw** (`Value::String`): the USDA writer
  picks a delimiter the content can't close, and the lexer keeps raw bytes between delimiters
  (it does *not* unescape), so backslashes, quotes and newlines round-trip verbatim. The one
  thing USDA cannot delimit — a value containing both `"""` and `'''` — is rejected at apply,
  not at save (a stranded unsavable document is worse). This is the *single* place attribute
  strings are handled, so no call site ever hand-escapes a literal. It replaced a separate
  `SetStringAttribute` op (itself a DRY violation) and the fragile `format!("{:?}")` that
  `SetRhaiPolicy` used, which produced Rust-debug quoting, not USDA delimiting, and silently
  corrupted any multi-line rhai source. A `string` edit also skips the projector's visual
  refresh — a string attribute is non-visual metadata/behaviour, and refreshing would hot-reload
  a running scenario (resetting its `this`) on a mere save.

Not yet verified: the full loop in a live twin (edit → `SaveScenario` → reload → source stuck).
The entity→document resolution runs through bevy's `AssetServer`, so that last inch wants an
in-app check rather than a unit test that would mostly exercise bevy. Everything it is built
from — the raw-string round-trip, the rejection, undo, the bridge idiom — is tested.

---

## 4. Shape of the tool

Nothing in `lunco-workbench` needs to change. A perspective is a layout preset; panels register
into slots; per-perspective dock isolation and per-Twin layout persistence are free.

```
┌ Object Builder ────────────────────────────────────────────────┐
│ Prim Tree    │  Canvas (connections)  /  Viewport   │ Inspector │
│  (new)       │       (new projector)                │ (extend)  │
│ Palette      │                                      │ Params    │
│  (exists)    ├──────────────────────────────────────┤ (extend)  │
│              │ Script editor + diagnostics (new)    │           │
└────────────────────────────────────────────────────────────────┘
```

**Connection canvas** = a second `lunco-canvas` projector. Nodes are prims carrying
`inputs:*` / `outputs:*` (`kind = "usd.prim"`); edges are `connectionPaths`
(`kind = "usd.connection"`). Wire-drag emits `UsdOp::SetConnection`; that is the entire
write path, and it journals for free.

Node layout needs somewhere to live. Modelica stores placement in `.mo` annotations. The USD
equivalent is an authored attribute — `float2 lunco:canvas:pos` on the prim — which journals
like any other edit. **Debounce it.** A drag advances the document generation every frame, and
`sync_twin_overlays` already learned this lesson the expensive way (`twin_projection.rs:305-310`,
~212µs/frame of wasted recomposition before it was fixed).

---

## 5. Sequencing & status

Ordered by what unblocks what, not by visibility. Status as of the current pass.

**Phase 0 — make the loop safe.** ✅ mostly done. `SetVariantSelection` added; `SetRelationship`
given an incremental live-stage author so the attach path doesn't rebuild the world (§3.3);
`scene_document_for` provides the doc-back check a builder needs (§3.7). The `UndoManager` architectural
question is now **decided** (2026-07-11 — see below): `DocumentHost` stays authoritative for per-document
`Ctrl+Z`, `UndoManager` is reserved for future twin-wide undo, no double-wiring. No code change; blocker
cleared. Refuse-to-open a non-doc-backed scene lands with the builder's open path.

**Phase 1 — see and tune.** ⏳ mostly done. The **Object Builder perspective**
(`lunco-sandbox-edit/src/ui/mod.rs`, `ObjectBuilderPerspective`) composes tree + palette + viewport +
Inspector + the two new projectors. The **USD prim tree** landed ✅ (2026-07-11):
`lunco-sandbox-edit/src/ui/usd_prim_tree.rs` (`UsdPrimTreePanel` + `UsdPrimTreeView` +
`produce_usd_prim_tree`). It reconstructs the full USD hierarchy from every spawned prim's
`UsdPrimPath` (synthesizing intermediate xforms that carry no entity), reading each prim's type + body
flag off the composed stage via the same main-thread `CanonicalStages` producer the connection canvas
uses (panels can't touch the `!Send` stage). Docked as the first `🌲 Prims` side-browser tab; clicking
an entity-backed node selects it through the shared `apply_selection`; top two levels open by default.
**Bounded parameter sliders — landed + renderer-verified (2026-07-11):** the Inspector reads per-attribute
`customData {min,max,unit}` for data-driven sliders. Reader (`lunco-usd-bevy`: `AttrUiHint` +
`StageView::attr_ui_hint`, parsing the `customData` `Dictionary` behind a plain-Rust struct so consumers
need no `openusd`), view-model (`usd_params::produce_usd_param_view`), section
(`inspector::usd_parameters_section`), authored ranges on `CosimTarget`
(`primvars:wedge_count`/`band_count`). Write-back verified live: a `SetAttribute` on the primvar (the exact
op the slider defers) re-runs the checker shader in the renderer (24×16 fine ↔ 2×3 coarse). Preserves the
Inspector-*derives*-never-hardcodes rule (§3.5).

**Phase 2 — wire.** ✅ **DONE** (2026-07-11). Connection canvas landed as a second `lunco-canvas`
projector: `crates/lunco-sandbox-edit/src/ui/connection_canvas/` (`projection.rs` pure `collect_graph`
+ `build_scene`, unit-tested; `visuals.rs`; `mod.rs` panel + main-thread producer + write-back).
Registered in `SandboxEditUiPlugin`, docked as a `🔗 Connections` centre tab in the Object Builder
perspective. Reads every wiring-relevant prim (connectors or `PhysicsRigidBodyAPI` body) as a node,
`inputs:*.connect` as dataflow edges and `physics:body0/1` as joint edges; wire-drag →
`SetConnection`, delete → clear / `RemovePrim`, all via the journaled `ApplyUsdOp`. Boot-verified in
the sandbox (`142 prim entities → 25 nodes, 10 edges`). **Note:** enumerate prims from the ECS
`UsdPrimPath` entities, *not* `StageView::prim_paths()` — the live traversal misses composed children
(fixed in `93a2bfed`). **Open (polish):** initial fit uses a nominal rect (press `F` to fit precisely);
node positions are session-only (persisting `lunco:canvasPos` is a follow-up).

**Phase 3 — assemble.** ⏳ primitive done + API-drivable; mount ergonomics open. `AttachComponent` and
its op-lowering are landed and tested (§3.1); `resolve_mount_placement` / `AttachSpec::from_mount`
(`attach.rs`) compute a part's placement + rotation so a plug frame aligns to a socket frame,
unit-tested against hand-computed matrices. **Verified 2026-07-11:** `AttachComponent` is reachable over
the HTTP/MCP API as-is — its nested `AttachSpec` (incl. the `AttachJoint` enum) deserializes through the
executor's `TypedReflectDeserializer`. **JSON contract** (single-field tuple structs unwrap to their
inner value): `{ "doc": 1, "spec": { "edit_target": "@root@", "host_path": "/…", "name": "…", "asset":
"<raw path>", "placement": [x,y,z], "rotate_deg": [x,y,z], "joint": "Fixed" | {"Revolute": {"axis":
"X"}} } }`. So placement-based attach works today with one `execute_command`.

**Retrofit snap — landed + renderer-verified (2026-07-11).** The mount-frame *reading* the design held
back (a wrong frame conversion is a physics bug only the renderer shows) is now built for the **retrofit**
case, where both frames are on the live composed stage — no asset-stage open needed:

- **`lunco:mount:*` schema** authored on a demo assembly (`sandbox_scene.usda` → `Base`): a host advertises
  sockets under a `Mounts` group (`lunco:mount:socket` / `:joint` / `:axis`, `rel :part`), an attached
  child part advertises its plug (`lunco:mount:plug`, `rel :frame`).
- **Reader** (`lunco-usd-bevy/src/mount.rs`): `read_sockets` / `read_plug_frame` compose each mount prim's
  frame **body-local** through the `Mounts` group via `local_transform_at` (`frame_in_body`). Tested against
  a real composed stage (`mount_reader_tests`, 3 tests): socket reads body-local `(0,2.5,0)` not world
  `(5,8.5,5)`; plug reads part-local; metadata + `part` rel compose.
- **Op-lowering** `realign_component_ops` (`attach.rs`): re-authors an existing part's `xformOp:translate`/
  `rotateXYZ` + the joint's `localPos0` from the resolved placement — *no* `AddPrim`/`SetRelationship`, so
  it touches no topology and never rebuilds the world. Tested (2 tests): derives-anchor-from-placement,
  authors-rotation-unconditionally (so a retrofit can clear a stale rotation to zero).
- **UI**: Inspector `🔩 Mount` section (`inspector::mount_section`) over a NonSend view-model
  (`usd_mount::produce_usd_mount_view`) — one row per socket, a `⟳ Snap` button that defers the realign ops
  through `apply_usd_ops` (resolves the doc once, dispatches each journaled `ApplyUsdOp`), disabled when the
  part is already aligned.
- **Renderer-verified (2026-07-11):** dispatching the exact realign ops the button emits (`ApplyUsdOp` on
  the twin doc) snapped the demo `Arm` from beside `Base` to directly on the socket (`(2,0,0)→(0,2.5,0)`),
  **held stably** because `localPos0` moved with it — the joint didn't yank it back. This is the in-renderer
  validation the frame math was held for. (The literal egui click isn't MCP-automatable, but the button's op
  output is verified live; the section + view-model compile clean.)

**New-attach snap — landed + tested (2026-07-11).** The other half: reference a component in and snap its
plug to a socket, where the plug lives inside the *not-yet-loaded asset*, not the composed scene.

- **`read_asset_plug_frame(fs_path)`** (`mount.rs`) composes the asset's full closure off disk
  (`compose_file_to_stage`, resolving its references) and reads the plug frame off its `defaultPrim` — the
  part every `AttachSpec` references in. Tested against the shipped demo component (`mount_probe.usda`,
  plug 0.4 m above the part origin). Native-only (composition does file I/O).
- **Socket schema** gained `lunco:mount:asset` (the raw component path a socket is designed to hold), read
  into `MountSocket.asset`. An **empty** socket (no `rel :part`) that names an asset offers `⊕ Attach` in
  the Inspector; the handler resolves the asset path against the asset root (`<cwd>/assets`),
  `read_asset_plug_frame`s it, `AttachSpec::from_mount`s it onto the socket frame, and dispatches the
  journaled `AttachComponent` (references + places + joints the part). Socket joint token → typed
  `AttachJoint` via `attach_joint_from`.
- **Demo:** `components/mount_probe.usda` (a magenta part with a `probe` plug) + an empty `probe` socket on
  `Base` (`sandbox_scene.usda`) naming it. Select `Base` → `🔩 Mount` → `⊕ Attach mount_probe.usda`.
- **Verified:** the reader/compose piece by the disk-compose test above; `from_mount` by its matrix tests;
  `AttachComponent` was already API-verified (JSON contract above) with 5 op tests. Not run: the literal
  egui click-through (no egui MCP automation; a live GUI instance was unavailable — the user's own
  host+client session held the ports, and a competing GPU instance would risk it).

**Still open — the shipped-rover retrofit (`physical_drivetrain.usda` / `rocker_bogie.usda`).** These author
the duplication the schema removes literally — `over Wheel_FL.translate == Wheel_FL_Hinge.localPos0 ==
(-0.9,-0.65,-1.225)`, one number typed twice per wheel. The retrofit *mechanism* is complete and proven
(the verified `Base`/`Arm` snap is mechanically identical). What a literal "drop the hand-written anchors"
needs beyond authoring mount frames is a **load-time mount-resolution pass** that derives `localPos0` from
the frames at load — otherwise removing the anchors leaves the joints un-anchored. That is a separate
feature, and authoring mounts onto a shipped, path-translated, cross-file drivetrain wants per-rover
in-renderer physics verification (all wheels articulate) before it ships — so it is deliberately deferred
rather than edited blind. The substrate (reader + `realign`/`from_mount` + snap UI) is what it rests on.

> **Merge check (2026-07-11), physics bridge `cc87d39f`:** the plan survives intact. The bridge
> (`lunco-usd-avian/src/big_space_bridge.rs`) only replaces avian's f32↔f64 *body world-pose* sync;
> it does **not** change joint-anchor interpretation — `localPos0/1` are still read body-local
> (`lunco-usd-avian/src/lib.rs`, `with_local_anchor1/2`), so `attach.rs`'s `localPos0=placement,
> localPos1=0` convention and `resolve_mount_placement` (pure local-frame `Affine3A`, `EulerRot::XYZ`)
> are correct as-is — the bridge makes jointed bodies *more* precise for free. `coords.rs::world_pose`
> is a hierarchy/world-space, ECS-bound helper solving a different problem; do **not** switch the mount
> math to it. **One guardrail** from `45-big-space-correct-usage.md` §3.3: when the UI reads socket/plug
> frames off the stage, read each prim's **local** `xformOp:*`; do **not** adopt `world_position_seeded`
> (it's flagged for retirement). Author assembled physics in the grid's local/site frame, never in
> celestial space.

**Phase 4 — behaviour.** ✅ **DONE** (2026-07-11). Save-back-to-prim was already closed (§3.7); the
rhai **editor panel** now landed: `crates/lunco-sandbox/src/ui/rhai_editor_panel.rs` (`RhaiEditorPanel`
+ `RhaiEditorVm` view-model + `produce_rhai_editor_vm` producer). It lives in **lunco-sandbox** (not
sandbox-edit — it needs `ScriptRegistry`/`RunScenario` from lunco-scripting and `SaveScenario` from
lunco-sandbox), registered in `crates/lunco-sandbox/src/ui/mod.rs`, docked as a `📜 Behaviour` centre
tab in the Object Builder perspective (referenced by string id, filtered in apps that don't register
it). Follows `SelectedEntities`, resolves `ScriptedModel → ScriptRegistry` source into an editable
buffer (re-synced only on a doc/generation change with no unsaved edits, so typing is never clobbered),
and surfaces `DocumentDiagnostics` (the line/col the rhai compile path emits) as a click-to-jump list
plus a compile-status chip. **Save & Run** = `RunScenario{source}` (sets live source + hot-reloads the
scenario) → `SaveScenario` (persists onto `lunco:script`), both journaled; **Revert** reloads the saved
source. Boot-verified: attaching a rhai script with a deliberate `let boost = ;` showed the source and
the diagnostic `✗ 9:17 Unexpected ';' (line 9, position 17)`. **Follow-up:** a painted line-number
gutter (the codebase has none; the Modelica editor deliberately deferred it — the diagnostics list is
the interim surface).

### The `UndoManager` decision (Phase 0's open item)

`UndoManager` (`lunco-twin-journal`) is built but unused, and wiring it is genuinely decision-blocked,
not mechanical. Two undo stacks exist: the live per-document `DocumentHost` inverse stack (wired, what
`Ctrl+Z` uses today) and the journal `UndoManager` (per-author, twin-wide scope). They record the same
edits and **cannot both drive one `Undo` command** — running both double-undoes. Worse, the journal
recorder fires identically for a fresh edit, an undo, and a redo, so it cannot feed `UndoManager`'s
`record_local` / `record_redo` split correctly without new plumbing. The networked-author isolation is
*safe* (local edits are the only ones a recorder-fed manager ever sees — peer edits bypass the recorder),
so the block is purely: **make the journal authoritative and retire the `DocumentHost` stack, or demote
`UndoManager` to a separate twin-wide/cross-document undo the per-document command doesn't touch.**

**Decision (2026-07-11): keep the `DocumentHost` per-document inverse stack authoritative for `Ctrl+Z`;
reserve `UndoManager` for a future twin-wide / cross-author undo, and do NOT feed it from the
per-document recorder.** Rationale: the two costs of the alternatives are asymmetric. Wiring both onto
one `Undo` command is a *correctness* regression (double-undo, plus the recorder can't split
fresh-edit/undo/redo) — a live bug in the hot editing path. Keeping `DocumentHost` authoritative is the
*status quo that already works*: `Ctrl+Z` is correct today, per-document scope is what a builder wants
(undo my last edit to *this* prim, not some other author's), and it composes cleanly with the journal
(every `ApplyUsdOp` still records to the journal for replay/history — that path is untouched). The only
capability we defer is *cross-document / twin-wide* undo, which has no user demand yet and, when it does,
belongs on its own command (`UndoTwin`?) reading `UndoManager` — never on the per-document `Undo`. So the
"unused `UndoManager`" is not dead-by-mistake; it is **reserved infrastructure** for that later scope.
This needs no code change now — it removes the blocker by making the call, and prevents the tempting-but-
wrong double-wiring. Revisit only when a concrete cross-author-undo requirement lands.

---

## 6. What could go wrong

- **Attach = rebuild.** §3.3. Decide before building the canvas.
- **Canvas layout churn.** Node positions are journaled document edits. Debounce or the journal
  fills with drag frames.
- **Op-ring overflow.** More edits than ring capacity between syncs returns `None` from
  `ops_since` and forces a rebuild. A fast interactive tool can hit this; the fallback is correct
  but slow.
- **Raw-file scenes.** §2. A silent data-loss path if the builder is allowed to open one.
- **`this` is not persistent.** §3.6. Reboot restarts behaviour. Surface it in the UI rather
  than letting it read as a bug.
- **No reduced-coordinate articulation.** `PhysicsArticulationRootAPI` is a tag and an authority
  hint, not a solver. Assemblies are pairwise Avian joints plus one soft differential constraint.
  A ten-joint robotic arm built by snapping parts will inherit whatever accuracy and stability
  pairwise joints give — a mount schema makes such an arm *authorable*, it does not make it
  *well-conditioned*. That is a separate problem and should not be discovered during Phase 3.
