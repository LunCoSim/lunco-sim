# LunCo Scripting Guide

How to write **scenarios** — persistent per-entity programs that sense and drive
the simulation — in LunCoSim.

- **Crate:** [`lunco-scripting`](../crates/lunco-scripting) · **Design rationale:** [rhai-integration.md](./architecture/rhai-integration.md)
- **Examples:** [`assets/scripting/examples/`](../assets/scripting/examples) · **Helper library:** [`assets/scripting/prelude/`](../assets/scripting/prelude)
- **Every command you can call:** [`commands-reference.md`](./commands-reference.md) (auto-generated)

This guide has two parts:

- **Part I — Tutorial** (below): write, run, debug, and persist a scenario from
  zero. Start here if you're new.
- **Part II — Reference** (below): the full verb surface, prelude helpers,
  sequencing, persistence, determinism, and the rest. Jump here once you know the basics.

---

# Part I — Tutorial: your first scenario

The language is **rhai** — a small, sandboxed, pure-Rust language that runs
everywhere the sim does, including the browser (wasm). A **scenario** is a rhai
program attached to an entity. Its `task(me, ctx)` tree is advanced on every fixed
simulation tick by the native behavior kernel; it is not a one-shot snippet.

> **The host (Rust) is mechanism; the script is policy.** Navigation, objectives,
> behaviour trees, sequencing — all live in hot-reloadable `.rhai`, never compiled
> into the engine.

For discrete controls, use `intent_pulse(target, intent)` or
`intent_edge(target, intent, "pressed"|"released"|"pulse")` from the control
prelude. This emits one target-scoped semantic edge and the `intent.edge` event;
the Twin's Rhai/Modelica policy decides whether to latch, release, toggle, or
actuate it. Keep `SimulateIntent`/`SetPorts` for held and continuous values.

For UI automation, use the native input helpers from `prelude/input.rhai`.
They compose typed Bevy window events rather than calling an editor or scene
tool directly:

```rhai
input_key_press("AltLeft");
input_click("primary", 960.0, 540.0);
input_key_release("AltLeft");
```

The same path drives egui, picking, focus, key bindings, and authored tools.
Use `input_pointer_move`, `input_pointer_press`/`release`, and `input_scroll`
for drag and viewport workflows. Coordinates are logical primary-window pixels;
Rhai owns the gesture sequence and `InjectWindowInput` owns only typed event
validation and delivery.

The command result's `id` is also the edge's causal correlation id. Inspect the
current downstream path with:

```rhai
let edge = intent_pulse(target, "release");
let trace = query("CausalTrace", #{
    target: target,
    correlation_id: edge.id,
});
```

`trace.control_binding` shows the authored intent-to-port mapping,
`trace.port_surface` shows the `PortRegistry`-selected owner and current input,
`trace.connection_edges` and `trace.joint_admission` show topology/admission,
and `trace.measured_channels` shows current retained signal samples. An absent
or pending stage is an incomplete path; it is not treated as successful
actuation. Omitting `correlation_id` selects the newest trace for `target`.

A script touches the world through exactly the same **command/query API** the HTTP
API, MCP, and UI use — so it inherits [every command](./commands-reference.md) for
free and stays decoupled from physics. Scripts are **host-authoritative**
([Part II §L](#l-networking--determinism)).

## Model and assembly authoring (human and AI)

For building or reviewing a USD assembly, use the reusable Rhai
`model_authoring` tool library. It is an authoring facade, not a second scene
graph or USD writer. Keep the exact identity tuple from the document read:

```text
document -> root path -> edit target -> generation
```

The normal sequence is:

```rhai
let context = model_authoring::model_context(doc, root, "@root@");
let ready = model_authoring::readiness_report(doc, root, "@root@", policy);
let recipe = model_authoring::scene_recipe(doc, "@root@", scene, context.generation);
let graph = model_authoring::port_graph(doc, root, "@root@");
let wires = model_authoring::wiring_plan(
    doc, "@root@", root, connections, context.generation);
```

`model_context` is one composed, path-addressed read of an assembly: children,
references, variants, component/frame/mount facts, bodies, joints, colliders,
ports, generation, and available actions. `readiness_report` accepts an
explicit Twin policy and returns stable checks for topology, physicality,
mounts, connections, controls, and runtime evidence. An omitted check is
`not_requested`; it is not an implicit pass.

`scene_recipe` returns dry typed USD operations for referenced assemblies,
terrain, cameras, and initial state. It returns routes and program attachments
as explicit hand-offs to `waypoint_editor` and `assembly_edit::attach_program`.
After review, apply USD operations through `assembly_edit::batch` or the normal
proposal/review/commit flow, then re-read the new generation. Do not write
USDA text directly.

Camera creation is authored through the camera entry in that recipe: Rhai
validates the requested role, pose, projection, look-at, and standard
`UsdGeomCamera` intrinsics, then emits a standard `def Camera` with
`LunCoCameraAPI`. Omitted intrinsics use only the defaults defined by the USD
camera schema. The window host's convenience choice is a separate
`camera.default_presentation` policy over derived facts; return exactly
`avatar`, `generated`, or `none`. A missing, faulting, or invalid policy or
camera contract remains a visible no-camera/diagnostic result. The runtime
does not choose a first camera, infer an avatar camera from an entity, or
repair malformed authored values.

Use `port_graph` to discover standard USD `inputs:`, `outputs:`, and
`connectors:` endpoints. `wiring_plan` validates exact source/sink paths,
direction, and USD type before returning `SetConnection` operations. Modelica
and Rhai are classified from their authored source declarations; no vehicle
specific port vocabulary is required.

Use `publish_component(doc, root, edit_target, output, provenance)` only after
the component contract passes. It validates a top-level `kind = "component"`
root, `defaultPrim`, applied schemas, references, and caller-supplied
provenance, then returns ordinary metadata operations and an explicit Save-As
command. Review/apply the operations and call
`assembly_edit::save_as_document` explicitly; publication never autosaves.

These tools are generic and hot-reloadable. Put vehicle recipes, limits,
dimensions, and mission policy in the owning Twin's Rhai package. Put
continuous equations in Modelica and keep Rust limited to shared typed engine
mechanisms.

Behavioral and linter regressions use the same authored boundary: keep the
fixture in `assets/scenes/tests/*.usda`, attach an observer in
`assets/scenarios/tests/*.rhai`, and assert the public command/query result.
Do not embed a large USDA string in a Rust test when the behavior can be
observed through the production scene runner. Rust tests remain for low-level
parsing, serialization, lowering, and generic engine seams that Rhai cannot
observe.

You'll need a running app with its API on, e.g. the luncosim:

```sh
target/debug/luncosim --api 4101
```

## 1. Mental model

| You write | The engine does |
|---|---|
| `fn task(me, ctx)` | builds one native task tree; the kernel advances it each fixed step |
| `fn mission(me, ctx)` | declares objective state and completion conditions |
| `fn on_start(me, ctx)` | optional setup after (re)compile |
| `fn on_event(me, evt, ctx)` | optional reaction to a `TelemetryEvent` |
| `fn on_stop(me, ctx)` | optional teardown on hot-reload / detach / despawn |

`me` is the host entity's id. Task action and predicate leaves accept anonymous
closures with that one positional argument (`|me| ...`) or named script
functions (`Fn("name")`, declared as `fn name(me)`). Both forms access
persistent task state as the driver-bound `this`. The native task driver owns
task progress, dwell timing, and event waits. You sense with queries/`get` and
act with `cmd`/`set`.

## 2. Your first script

Create `assets/scenarios/my_rover_mission.rhai`:

```rhai
fn task(me, ctx) {
    let waypoints = [
        [10.0, 0.0, 0.0],
        [20.0, 0.0, 10.0],
        [0.0, 0.0, 20.0],
    ];
    let steps = [];
    for i in 0..waypoints.len() {
        let target = waypoints[i];
        steps.push(step(
            |m| nav_to(m, target, 0.8, 2.0),
            |m| arrived(m, target, 2.0),
        ));
    }
    steps.push(once(|m| {
        notify("Mission complete! Parking.");
        brake(m);
    }));
    seq(steps)
}
```

`nav_to` and `brake` are [prelude helpers](#b-prelude-helpers) — high-level
verbs built on the raw `cmd`/`get` bridge. No control loops to hand-code.

### Run it

Attach it to a rover. Get the rover's id (`list_entities()` or the UI), then fire
`RunScenario` over the API (the same path MCP and in-app launchers use):

`RunScenario.source` is the Rhai source text. It is not a filesystem path. For a
file-backed script, read the file into the request body; this keeps the command
contract identical for HTTP, MCP, and the in-app editor:

```bash
./scripts/api/run_scenario.sh 4869542932533563 \
  assets/scenarios/my_rover_mission.rhai 4101 '{}'
```

The wrapper is optional; it is equivalent to reading the file with `jq -Rs`
and posting the tagged request directly.

```json
{
  "type": "ExecuteCommand",
  "command": "RunScenario",
  "params": {
    "target": 4869542932533563,
    "source": "<rhai source text>"
  }
}
```

The subject follows the route points. Re-issue `RunScenario` on the same entity to
**hot-reload** after you edit the file by sending the updated contents again —
no rebuild, no restart (the outgoing program's `on_stop` runs first). For a
scene-authored file-backed program, use
`uniform asset info:sourceAsset = @lunco://scenarios/my_rover_mission.rhai@` instead;
the asset pipeline owns loading and hot replacement.

### Run authored tests without rebuilding

Behavior and mission outcomes belong in the production scene gate, not in a
Rust test that supplies a fake rover or spy command. After the first Rust build,
rerun authored scene tests with the existing binary:

```bash
./scripts/run_scene_tests.sh --no-build -j 1 scripting_task_contract
```

The scene runner defaults to four independent headless production processes.
`-j/--jobs N` changes that bound without changing each gate process's
deterministic `--threads 1 --jitter 0`; use `-j 1` when diagnosing ordering or
resource interactions. Graphics assertions remain a separate serial offscreen
pass.

For a standalone Rhai assertion that needs a live USD world, keep one API
session open and run the source through `RunRhai`:

```bash
./scripts/api/run_rhai_test.sh 4101 \
  assets/scripting/tests/test_usd_query.rhai /SandboxScene/Box
```

Both paths avoid a Rust rebuild. The live helper delegates to the native
`luncosim rhai --stdout` client and can be invoked repeatedly after editing the
`.rhai` file; it does not restart the simulator. Use
`RunScenario`/`run_scenario.sh` for a persistent per-entity observer and
`run_rhai_test.sh` for a one-shot verdict.

The terminal frontend and the evaluator are deliberately separate. The
running host's `lunco-scripting` package owns the Rhai engine and the reflected
`RunRhai` command. `lunco-rhai-repl` only reads terminal input and formats the
result; `lunco-api-client` only sends the generic API envelope. The in-process
stdin REPL in `lunco-scripting` is a host-local debug seam and is not reused as
a second remote client implementation.

### Inspect & debug

- `print(...)` lands in the console.
- `ScriptStatus { target }` reports compile/runtime health (state, errors with
  file/line/column).
- `ScriptInspect { target }` shows the live `this` map, defined hooks, generation,
  paused/running.

```json
{"type":"ExecuteCommand", "command": "ScriptInspect", "params": { "target": 4869542932533563 } }
```

### Persist it in the scene

So it runs automatically on load, give the prim a program child in your `.usda`. A
program is a prim, not an attribute — delete the prim and the behaviour goes with it:

```usda
def Xform "Rover_01" {
    def Scope "Mission" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @lunco://scenarios/my_rover_mission.rhai@
        # …or author the source in place:
        # uniform token info:implementationSource = "sourceCode"
        # uniform string info:sourceCode = '''<rhai source>'''
    }
}
```

That's the whole loop: **write → run → inspect → persist.** The rest of Part I
fills in the everyday verbs; Part II is the complete reference.

## 3. Lifecycle hooks

```rhai
fn task(me, ctx)          { seq([wait(1.0)]); }               // canonical progression
fn mission(me, ctx)       { [objective("landing", #{})]; }   // optional objectives
fn on_start(me, ctx)      { /* setup */ }                     // once, after (re)compile
fn on_event(me, evt, ctx) { if evt.name == "GO" { /* … */ } } // a TelemetryEvent arrived
fn on_stop(me, ctx)       { brake(me); }                      // teardown: hot-reload / detach / despawn
```

- Define any subset. `on_stop` is where you stop actuators / release claims.
- Use `task` for ordinary fixed-tick behavior and `on_event` for external
  events. Production scenarios must not define `on_tick`; authored test
  scenarios may use it to sample state and publish a bounded verdict.

## 4. The everyday verbs

You'll use these constantly (the complete table is in
[Part II §A](#a-full-verb-surface); every `#[Command]` is in the
[command reference](./commands-reference.md)):

| Verb | Purpose |
|---|---|
| `cmd(name, #{params})` | **WRITE** — fire any command by name (spawn, possess, set input…). Returns `#{ id, ok, data, error }`. |
| `query(name, #{params})` | **READ** — call a read-only query provider (Raycast, Nearest, GroundHeight, `CausalTrace`…). Successful data is returned directly; no-data is `()`; failures return `#{ok:false,error}`. |
| `query("ListSpawnCatalog", #{})` | **READ** — discover the authoritative `entry_id`, name, category, default transform, source, and `origin` (`builtin` or named `twin`) for assets accepted by `cmd("SpawnEntity", ...)`. |
| `query("ValidateTwin", #{path: "/work/rover-twin", policy: "warn"})` | **READ** — inspect Twin-wide Modelica, USD, Rhai tool, shader, and asset resolver namespaces; collisions are warnings by default and become errors with `policy: "error"`. |
| `get(id, "Comp.field")` / `set(id, "Comp.field", v)` | reflected component read/write (vectors → `[x,y,z]`); scalar co-simulation names use the canonical `PortRegistry` surface. |
| `find(name)` / `world_pos(id)` | locate an entity; read its f64 active-frame array vector. Use `world_point(id)` when the value crosses a position or authoring boundary. |
| `world_pos3(id)` / `world_forward3(id)` / `world_rotation_quat(id)` | native glam `Vec3`/`Quat` pose for hot loops; use `vec3_array`/`quat_array` when producing a wire/report value. |
| `world_point(id)` / `point3(values, frame)` | read or construct an explicit frame-tagged `point3`; use `point_values`, `point_frame`, `point_delta`, `point_distance`, and `point_offset` for frame-safe position work. |
| `emit(name, value?)` | fire a `TelemetryEvent` (delivered to `on_event` on the next scenario pass); scalar, array, and map payloads keep their typed structure. |
| `notify(msg)` / `notify_kind(msg, kind)` | HUD notification (`kind`: `"info"`/`"warn"`/`"error"`). |
| `list_entities()` | every entity (`#{id,name,type,catalog_id,input_surface,control_bound,celestial_body,pos}`) — identity comes from USD/catalog data; filter/select in-script. |

> **`set` vs `cmd`.** Use `set` to tune a reflected value. When `set`
> falls through to a scalar port it is a raw write and has no persistent hold;
> use `cmd("SetPorts", #{target: id, writes: [[name, value]]})` when wiring must
> be overridden until an explicit release; use `cmd("ReleasePort", ... )` for
> one port or `cmd("ReleaseControl", #{target: id})` for the complete vehicle
> command surface. Direct
> writes are host-authoritative and unavailable to client-scoped scripts. Use `cmd`
> for an *operation* with side effects
> beyond a field write (spawning, swapping a material, anything an observer reacts to).

## 5. Making it move: navigation & sensing

The prelude turns raw verbs into rover behaviour (read the topic files for the
authoritative list; highlights in [Part II §B](#b-prelude-helpers)):

- **Drive:** `drive(rover, fwd, steer)`, `brake(rover)`, `nav_to(entity, target, speed, radius)`.
- **Sense:** `velocity3`/`velocity`/`speed`, `raycast`, `obstacle_ahead`, `ground_height`, `nearest`, `entities_in_radius`.
- **Math:** `distance`, `arrived`, `vsub`/`vlen`/`vnorm`/`vcross`, `clamp`.
- **Collisions:** `collision_pair`/`entered`/`exited` (parse `COLLISION_START`/`COLLISION_END`).

A reactive mission (avoid obstacles, run a waypoint plan, coordinate between
scripts) is all rhai — see the [examples index](#n-examples-index).

## 6. Where to go next

- **Every command** `cmd()` can fire: [`commands-reference.md`](./commands-reference.md).
- **Deeper topics** (sequencing, tools, policy hooks, vessel controllers, behavior
  trees, determinism): [Part II](#part-ii--reference).
- **Recording deterministic frames** (the offline clock, `shot_*` verbs, `frozen`,
  the `shots.rhai` sequencer): [`offline-recording.md`](./offline-recording.md).
- **Design rationale**: [`architecture/rhai-integration.md`](./architecture/rhai-integration.md).
- **rhai language**: <https://rhai.rs/book/>.

---

# Part II — Reference

## A. Full verb surface

The host exposes a minimal, generic bridge. Everything else is prelude policy.

| Verb | Returns | Purpose |
|---|---|---|
| `cmd(name, #{params})` | `#{ id, ok, data, error }` | **WRITE** — fire any `#[Command]` by name (synchronous; `data` carries command-specific result data such as a spawned gid). The full list is the [command reference](./commands-reference.md). |
| `query(name, #{params})` | value \| `()` \| error map | **READ** — call any query provider (Raycast, Nearest, GroundHeight, `CausalTrace`, …); successful data is direct, successful no-data is `()`, and failures are `#{ok:false,error}` |
| `query("ListSpawnCatalog", #{})` | map | discover the spawn catalog used to validate `SpawnEntity.entry_id`, including each asset's source and authored `origin` |
| `query("ValidateTwin", #{path: "/work/rover-twin", policy: "error"})` | map | pre-flight one explicit Twin folder with the same `lint.twin` policy used by `RunLint { scope: "twin" }` |
| `get(id, "Comp.field")` | value \| `()` | reflected component **read** (vectors → `[x,y,z]`, quats → `[x,y,z,w]`, structs → maps) |
| `set(id, "Comp.field", value)` | bool | host-side **tuning write** — reflected component field or canonical scalar co-simulation port; supported field types only; not replicated, undoable, or a persistent port hold; `false` on bad path/type |
| `get_setting("Res.field")` | value \| `()` | reflected **resource read** — global settings/config live in resources, not components |
| `set_setting("Res.field", value)` | bool | host-side **tuning write** to a supported reflected resource field; not replicated or undoable; `false` on bad path/type |
| `get_twin_setting("namespace.key")` | value \| `()` | read a scalar from the active Twin manifest's generic `[settings]` table |
| `set_twin_setting("namespace.key", value)` | bool | persist a scalar in the active Twin manifest through the generic `SetTwinSetting` command |
| `get_exposure("namespace", "property")` | value \| `()` | read one raw engine capability value; Rhai owns selection and presentation policy |
| `world_pos(id)` | `[x,y,z]` \| `()` | f64 active-frame position; independent of camera recentering and celestial ancestors |
| `world_pos3(id)` | `Vec3` \| `()` | native glam active-frame position for hot-loop calculations |
| `world_point(id)` | `point3` \| `()` | active-frame position with an explicit `frame: "active_physics"` tag |
| `point3(values, frame)` | `point3` \| `()` | construct a frame-tagged position; rejects missing or non-3D values |
| `world_forward(id)` | `[x,y,z]` \| `()` | active-frame heading |
| `world_forward3(id)` | `Vec3` \| `()` | native glam active-frame heading |
| `world_rotation_quat(id)` | `Quat` \| `()` | native glam active-frame orientation |
| `find(name)` | id (`-1` if none) | entity id by canonical `Name` |
| `name(id)` | string \| `()` | human-readable presentation label; use `QueryEntity` for the canonical USD path |
| `usd_path(id)` | string \| `()` | prelude helper resolving `QueryEntity.usd_prim_path` for topology addressing |
| `parent(id)` / `children(id)` | id \| `()` / `[id,…]` | hierarchy traversal |
| `owner_of(id)` | session id \| `()` | which control session owns the entity; `()` if unowned |
| `controller(id)` | string \| `()` | controlling session role, or `()` if unowned |
| `is_controlled(id)` | bool | whether any session currently owns it |
| `list_entities()` | `[#{id,name,type,catalog_id,input_surface,control_bound,celestial_body,pos}]` | every registered entity; `name` is the human-readable presentation label, `type` is the projected USD kind, not a control-component heuristic; `input_surface` is the authoritative `InputPorts` readiness bit |
| `add(id, "Comp", #{fields})` | bool | **structural** — insert/replace a reflected component (built from default + fields); needs `#[reflect(Default)]` |
| `remove(id, "Comp")` | bool | **structural** — strip a reflected component |
| `despawn(id)` | bool | **structural** — despawn an entity (+children); replicates on a host. *Spawn:* use `cmd("SpawnEntity", #{entry_id, position})` (no generic spawn — clients reconstruct from the catalog) |
| `emit(name, value?)` | bool | fire a `TelemetryEvent` (delivered to `on_event` on the next scenario pass) |
| `intent_edge(target, intent, edge)` / `intent_pulse(target, intent)` | command result | emit one target-scoped semantic edge; the runtime publishes it as `intent.edge` with a correlation id usable by `CausalTrace` |
| `sim_tick()` / `dt()` / `elapsed_seconds()` | i64 / f64 / f64 | the fixed simulation clock |
| `rand()` / `rand_range(lo,hi)` / `rand_int(lo,hi)` | f64 / f64 / i64 | **deterministic** RNG — seeded per hook from `(entity, tick, hook)`, identical on every peer and replay |
| `param(id, key, default)` | any | read a `lunco:param:<key>` attribute from a prim (`custom float lunco:param:wmax = 1.05`); returns `default` if it is absent |
| `detach_joint(id)` | bool | detach an entity through the generic `DetachJoint` command; ordinary entities use normal removal, while joint entities release their rigid link through the solver lifecycle |
| `notify(msg)` / `notify_kind(msg, kind)` | () | send a HUD notification; `kind` is `"info"` / `"warn"` / `"error"` |

JSON appears **only** at the `cmd`/`query` params seam (that's the API's own
contract). Both directions are native: `get`/`get_setting` build rhai values
straight from reflect, and `set`/`set_setting` write rhai values straight back —
no JSON round-trip on the read or write path.

Rhai's standard scalar math (`sin`, `cos`, `exp`, `sqrt`, `atan(x, y)`, and related
functions) is already implemented with Rust `f64` operations; do not shadow it
in a prelude/tool. Native `Vec3`/`Quat` operations are registered by
`lunco-scripting` and reject non-finite values loudly at their owner.

> **`set` vs `cmd`.** Use `set`/`set_setting` for host-side tuning through the
> reflected field surface or the canonical scalar co-simulation port surface.
> This is a raw write, not a persistent hold; use
> `cmd("SetPorts", #{target: id, writes: [[name, value]]})` for a persistent
> command intent, and `cmd("ReleaseControl", #{target: id})` to apply the safe
> state immediately. Direct
> writes are host-authoritative and unavailable to client-scoped scripts. Use `cmd` for
> an *operation* with side effects beyond a field write (spawning, swapping a
> material, anything an observer must react to). Settings are only reachable if
> their type is `register_type`'d with `#[reflect(Component)]` / `#[reflect(Resource)]`.

## B. Prelude helpers

The [`prelude/`](../assets/scripting/prelude) directory (one `.rhai` per topic —
`nav`, `sensing`, `control`, `tasks`, `mission`, `patrol`, `science`, `links`,
`math`, `select`, `hud`, …) is the hot-reloadable helper library on top of the
verbs — read the topic files for the full, authoritative list. Highlights:

- **Vector math:** `vsub`/`vadd`/`vlen`/`vdot`/`vcross`/`vnorm`/`vscale`/`clamp`, `distance`, `arrived`. Use native `Vec3`/`Quat` (`world_pos3`, `world_forward3`, `world_rotation_quat`) in hot loops; arrays are the explicit USD/JSON/telemetry interchange form and are lowered with `vec3_array`/`quat_array`.
- **Navigation:** `drive(rover, fwd, steer)`, `brake(rover)`, `steer_to`, `nav_to(entity, target, speed, radius)`.
- **Discrete controls:** `intent_edge(target, intent, edge)` and `intent_pulse(target, intent)` emit one atomic `pressed`, `released`, or `pulse` edge; handle `intent.edge` in `on_event`.
- **Causal control inspection:** `query("CausalTrace", #{target: id, correlation_id: edge.id})` joins one semantic edge to its binding, selected port owner, USD/Avian admission state, and current measurements.
- **Sensing:** `velocity`/`speed`, `raycast`, `obstacle_ahead`, `ground_height`, `nearest`, `entities_in_radius`.
- **Connectivity / routing** ([`links.rhai`](../assets/scripting/prelude/links.rhai)): `links()` (the live link graph — `#{nodes, adj, edges, groups}` from `query("Links")`), `reachable(from, to)`, `link_path(from, to)`, `link_path_names(from, to)`, `can_reach(rover, station)`. The Rust kernel computes only link GEOMETRY at a tunable cadence and publishes the graph; **routing is pure rhai policy** — call it at decision time (e.g. in `on_event` on `link.los`), not every tick. Nodes are identified by **GID** — the same id `find()` returns — and every helper takes either a GID (that node) or a `lunco:link:class` string (the GROUP with that role), so `can_reach(find("…/Comms"), "earth")` means "any Earth station" while each station stays separately addressable. A class is a shared role, never an identity: three DSN complexes all author `class = "earth"`. See [doc 49](./architecture/49-connectivity-link-kernel.md).
- **Collision events:** `collision_pair`/`collision_other`/`entered`/`exited` (parse `COLLISION_START`/`COLLISION_END`).
- **Task trees (`task(me, ctx)`):** every constructor emits a node with an explicit `kind`; there is no field-presence inference. Leaves are `step`/`once`/`act_for`/`act_until_event`/`wait`/`wait_until`/`wait_for`/`wait_for_from`/`check`, and action/predicate leaves accept anonymous `|me| ...` closures or named `Fn("name")` callbacks declared as `fn name(me)`. Composites are `seq`/`par_all`/`par_race`/`sel`/`reactive_seq`/`reactive_sel`, and decorators are `repeat`/`forever`/`retry`/`invert`/`force_ok`/`force_fail`. The adapter rejects missing/unknown kinds and cross-kind fields, then compiles once onto the existing `lunco-behavior` kernel. See [`rhai-task-tree.md`](architecture/rhai-task-tree.md). The kernel emits `TASK_COMPLETE` or `TASK_FAILED`.
- **Timeline (Layer 2):** `compile_timeline`, `timeline_step`. A timeline step
  must contain exactly one explicit operation word (`move_to`,
  `move_to_entity`, `possess`, `brake`, `cmd`, `emit`, `wait`, or `wait_event`);
  common fields such as `subject`, `speed`, `radius`, `secs`, `params`, and
  `value` are validated against that operation at the command boundary.
- **Script-first authoring:** the dynamically reloadable `assembly_builder`
  tool provides semantic frame/shape construction, placement, geometry,
  retrofit-body, reference-target, and alignment plans above the
  namespaced `assembly_edit` tool, which owns
  explicit-document USD editing (`add_prim`, `transform`, `attribute`,
  `schema`, `variant`, `relationship`, `connection`, `batch`,
  `assembly_edit::attach_component`, `assembly_edit::detach_component`, and
  `assembly_edit::attach_program`) plus its `rigid_body_plan`,
  `revolute_joint_plan`, `program_input_*`, and `program_output`
  constructors. `modelica_apply` and its
  typed operation constructors remain in
  [`prelude/authoring.rhai`](../assets/scripting/prelude/authoring.rhai).
  These are policy wrappers over the existing journaled command surfaces. The
  attachment and detach helpers require an explicit edit target and exact
  component/joint/socket paths; USD remains the scene/topology authority and
  Modelica remains the equation/graph authority. Obtain document ids from
  `ListOpenDocuments` before authoring.
- **Mission durability:** `mission_checkpoint` and
  `mission_checkpoint_read` author phase state on the host prim as USD string
  attributes through the explicit document command path. Define
  `fn mission_document(me) { <usd-document-id> }` and use the returned document
  id at objective/phase boundaries so a task can resume after a hot reload or
  restart without a second persistence mechanism. `ListOpenDocuments` supplies
  the id in editor sessions.
- **Selection toolkit:** `all_of_type`, `min_by`/`max_by`, `count_where`, `nearest_where`/`farthest_where`, `has_component`, `kind`.
- **View / cutscenes:** `set_camera(name)` — cut the scene viewport to a `def Camera` by name (leaf or full USD path); pairs with a timeline for cutscene camera changes. `possess(vessel)`, `notify(msg)`, `photo()` (capture from the active camera).
- **Route programs** ([`route_follow.rhai`](../assets/scenarios/route_follow.rhai)): the scene owns an ordered USD route and a sibling `LunCoProgramAPI` program. The program resolves its `inputs:subject` relationship, reads route-point poses, and advances only on generic sensor enter events. Route points are not stored on or discovered through a vessel-owned list.
- **Route presentation**: the reusable [`route_point.usda`](../assets/markers/route_point.usda) asset owns the standard translucent, unlit, shadowless visual/material and trigger geometry. Its unvisited point is green in standard `primvars:displayColor`/`primvars:displayOpacity`; the generic `route_follow` policy uses the `waypoint_editor` transient USD view tool to turn a visited point gray. A scenario consumes `route_point_reached` for mission policy without recreating distance checks or adding a second marker implementation.
- **Science instruments** ([`science.rhai`](../assets/scripting/prelude/science.rhai)): `photo_from(vessel)` captures from a vessel's mounted camera through the typed `CaptureFromCamera` command. Tool actions are generic task/program data; the engine dispatches only registered executable tools.
- **Tutorial HUD** ([`hud.rhai`](../assets/scripting/prelude/hud.rhai)): `hint(msg)`/`clear_hint()` (sticky instruction), `spotlight(anchor, caption)`/`clear_spotlight()` (dim + ring a workbench widget by `HelpAnchors` key), `focus_panel(id)` (open a singleton workbench panel on interactive hosts; unattended gates omit this presentation command), `objectives_hud(list)` (or just declare a `mission(me, ctx)` — it auto-publishes), `coach_step(steps, i)` (a guided coach-mark tour step; advance the cursor in `on_event`). This is how tutorials are authored — a tutorial is just a scenario. See [`tutorials/README.md`](../assets/tutorials/README.md).

`coach` only presents a step. Tutorial progression is authored in the lesson's
`on_event`, where it matches raw public event names (`cmd:<Name>`, `key:<Name>`,
and authored simulation events). Keep lesson-specific runtime checks in Rhai
observers under `assets/scenarios/tests/`; run them through the production
`luncosim test` command so changing a tutorial script does not require
rebuilding Rust.

Add helpers freely — the prelude is loaded **from disk at startup** on native
(`assets/scripting/prelude/*.rhai`): edit a helper, restart the app, no rebuild.
The compiled-in copy is used when the editable directory is absent and is the
source of truth on wasm, so a rebuild still refreshes it for installed/web
builds. Once a native disk source set is selected, a parse error is reported
and the app does not silently run stale embedded helpers.

## C. Scenario parameters

Reuse one source across entities/missions by passing one natural typed object.
The object belongs to the attached program instance and arrives as the explicit
`ctx` argument; there is no global `params` variable:

```jsonc
RunScenario { target: <gid>, source: "...", params: #{ speed: 1.5 } }
```
```rhai
fn task(me, ctx) {
    let speed = if ctx.speed == () { 0.6 } else { ctx.speed };
    forever(once(|m| drive(m, speed, 0.0)))
}
```

Omitting `params` means `{}`. Rhai owns defaults for individual keys, so a
scenario can choose the right default for its policy. The command boundary
rejects a scalar, array, `null`, non-finite number, or value outside the shared
telemetry range; Rust only validates and transfers the typed map once per
program instance. `param(id, key, default)` remains the separate USD-authored
attribute helper and does not read launch parameters.

## D. Sequencing (missions)

Two script-first layers, both pure rhai (no engine rebuild):

- **Layer 1 — task tree** ([`sequence.rhai`](../assets/scripting/examples/sequence.rhai)): build a step tree with `step`/`once`/`wait`/`wait_until`/`wait_for`; return it from `task(me, ctx)`. The native kernel feeds events and owns the cursor.
- **Layer 2 — declarative timeline** ([`timeline.rhai`](../assets/scripting/examples/timeline.rhai)): a mission as **pure data**. Each step has exactly one operation word (`move_to`, `move_to_entity`, `possess`, `brake`, `cmd`, `emit`, `wait`, or `wait_event`) and only that operation's fields; `compile_timeline` lowers it onto a task tree. Because it's data, a timeline is serialisable — run one inline with `RunTimeline`, or store it (see [§I](#i-persistence)).

Progress is observable on the telemetry bus: `TASK_COMPLETE` or `TASK_FAILED`
for the native task root, plus the mission/objective events emitted by your task
leaves and `mission(me, ctx)` declaration.

## E. Tools (shared libraries)

A **tool library** is a named bundle of reusable policy, callable as
`libname::fn(...)` from any hook. A library may also lazily import another
registered tool with ordinary Rhai syntax (`import "other_tool" as other_tool`);
the tool resolver uses the registry and does not read dependency files.

- Author one: drop a `.rhai` in [`assets/scripting/tools/`](../assets/scripting/tools), or `RegisterToolLibrary { name, source }` at runtime (hot-reloadable).
- Examples: [`assembly_builder.rhai`](../assets/scripting/tools/assembly_builder.rhai) (generic frame/shape construction, placement, alignment, composed collision-clearance, geometry, parameter, retrofit, and socket mating plans), [`assembly_edit.rhai`](../assets/scripting/tools/assembly_edit.rhai) (explicit USD assembly sessions), [`assembly_ui.rhai`](../assets/scripting/tools/assembly_ui.rhai) (Editor presentation workflows), [`editor_workflow.rhai`](../assets/scripting/tools/editor_workflow.rhai) (explicit inspect/projection/lint checkpoints and opt-in authored autosave), [`modelica_editor.rhai`](../assets/scripting/tools/modelica_editor.rhai) (generation-checked AST/diagram/text batches and compile checkpoints), [`sysml_editor.rhai`](../assets/scripting/tools/sysml_editor.rhai) (source-range edits and semantic checkpoints), [`rhai_editor.rhai`](../assets/scripting/tools/rhai_editor.rhai) (generation-checked script source edits and compile checkpoints), [`authoring_session.rhai`](../assets/scripting/tools/authoring_session.rhai) (cross-domain capability discovery, dry plan, grouped apply, checkpoint, undo, and redo), [`physics_acceptance.rhai`](../assets/scripting/tools/physics_acceptance.rhai) (generic contact, motion, settling, joint, and runtime-evidence checks), [`formation.rhai`](../assets/scripting/tools/formation.rhai) (formation flying), [`survey.rhai`](../assets/scripting/tools/survey.rhai) (lawnmower survey pattern).
- Discover: `ListToolLibraries`, `GetToolLibrary { name }`.
- **Persistence:** registered libraries are mirrored to `<twin>/tools/*.rhai` and reloaded when the Twin opens.

### One editing contract across formats

Use `authoring_session::capabilities(domain)` to discover the common editing
verbs, then `inspect` → `plan` → `apply` → `checkpoint`. The `plan` call is
pure and returns the affected operation kinds, count, label and parent
generation. `apply` sends one reviewed group to the format owner; it never
chooses an active tab or writes a source file directly. `checkpoint` reads the
owner's current generation and diagnostics, and `undo`/`redo` use the same
explicit document id.

```rhai
let state = authoring_session::inspect("sysml", sysml_doc);
let dry = authoring_session::plan("sysml", ops, "update contract", state.generation);
let applied = authoring_session::apply(
    "sysml", sysml_doc, ops, dry.label, dry.parent_generation);
let checked = authoring_session::checkpoint("sysml", sysml_doc);
```

The domain adapters intentionally differ only in their format semantics:
`assembly_edit` sends OpenUSD typed operations/proposals and waits for
projection readiness; `modelica_editor` uses AST/diagram/text operations and
the compiler state; `sysml_editor` edits UTF-8 source ranges and reads the
semantic requirement snapshot; `rhai_editor` uses the same contract for
source-backed Rhai tools and scenarios. Shader source remains renderer-owned
until it receives a document adapter. See the [unified authoring tooling review](reviews/authoring-tooling-review.md)
for the Editor UX and the remaining infrastructure gaps.

### Explicit USD assembly editing

Use `assembly_edit` for agent and editor automation over an open USD document.
It is a thin policy library over the existing typed USD command/query surface;
it does not parse USDA, maintain a second document/session, or write ECS state.
For a collaborative human-and-agent asset edit, follow the
[interactive headful Assembly Editor runbook](../skills/edit-usd-assembly/SKILL.md):
keep the production window visible, apply one coherent typed change at a time,
inspect a screenshot, and get user feedback before the next material edit or
save. The commands below remain the same; the runbook defines the required
interactive operating mode.
Create a new assembly with `assembly_edit::new_document()`, fork an existing
document with `assembly_edit::fork_document(source, name)`, or open a source
with `assembly_edit::open(path)`. These commands acknowledge the normal
asynchronous document lifecycle; discover the resulting explicit `DocumentId`
through `ListOpenDocuments`. Persist or end the session with
`save_document`, `save_as_document`, `close_document`, and `discard_document`;
they route through the same lifecycle commands as the human Editor. All
authored edits require the document, `@root@` or `@runtime@` layer, and the USD
path explicitly:

For repeated edit automation, `editor_workflow::after_edit(doc)` is an explicit
checkpoint helper. Call it after one reviewed apply/commit; it inspects the
document, waits for the current projection, runs document-scoped USD lint, and
returns a structured retryable result while projection is pending. It does not
run in `on_tick` in production and it never saves a stale projection. Authored
autosave is disabled when the Twin omits `usd.editor_autosave` and remains
disabled when the setting is `false`; only an explicit Twin value of `true`
allows the helper to call `SaveDocument`. Runtime overlay persistence is a
separate setting and file path. With the default policy, call
`assembly_edit::save_document(doc)` only after the human or agent approves the
visible result.

Component edits are live by default. The document that owns a component is
updated first; loaded assembly stages that reference its `twin://` layer are
then refreshed automatically in place, with their existing camera and view
state retained. This is dependency-scoped and does not flatten or duplicate
the component. A Twin can override that local presentation policy with the
generic hook registry:

For a deterministic audit, query `InspectUsdViewport` before and after the
edit. Its `stage_asset_path` and sorted `recipe_layers` show the exact layer
closure used by each preview. A file opened from a registered Twin is always
addressed with that Twin's assigned `twin://` authority; only a file outside a
registered Twin receives a synthetic viewport authority. This prevents a
component preview from silently becoming a different asset identity than the
assembly it is meant to update.

```rhai
bind_policy("usd.component_refresh", "decide_refresh", #"
    fn decide_refresh(facts) {
        // Return #{ action: "propagate" }, "defer", or "reject".
        #{ action: "propagate" }
    }
"#);
```

The hook runs once per dependent stage. A malformed or failing hook is reported
and that dependent remains on its previous projection; unregistering it returns
to automatic propagation. This hook does not change save policy: persistence is
still explicit unless the Twin opts into authored autosave.

```rhai
let checked = editor_workflow::after_edit(doc);
if checked.stage == "projection" {
    // Wait for the projection event, then call the helper again.
} else if checked.ok == true && checked.save_required == true {
    assembly_edit::save_document(doc); // explicit save, default policy
}
```

### Generic physics acceptance evidence

Use `physics_acceptance` in authored scene tests when a result needs more than
one scalar telemetry value. `sample(entity)` captures the existing world pose,
velocity, contact, and joint-drive surfaces; `contact_acceptance`,
`joint_distance_acceptance`, `motion_acceptance`, and `settling_acceptance`
apply thresholds supplied by the fixture. `system_evidence()` and
`system_acceptance()` preserve readiness, binding, and runtime-diagnostic
evidence in the same verdict. These helpers read the production surfaces and
do not change solver policy, add per-tick control, or encode a vehicle name.

Before editing what is visible, call `assembly_edit::viewport()` and use
`CaptureScreenshot` with the image viewer. Correlate the returned preview/view
handles with `ListOpenDocuments`; tab titles and filesystem names are not
document identity.

```rhai
let doc = 3;
let before = assembly_edit::describe(doc);
let generation = before.generation;
let changed = assembly_edit::transform(
    doc, "@root@", "/Rover",
    [1.0, 0.0, 0.0], (), (), generation,
);
let metadata = assembly_edit::prim_kind(
    doc, "@root@", "/Rover", "assembly", changed.data.generation,
);
let defaulted = assembly_edit::default_prim(
    doc, "@root@", "/Rover", metadata.data.generation,
);
```

Use `assembly_edit::references` to edit an existing prim's USD reference list
without flattening it. Entries are `#{ asset_path: "lunco://…" or
"twin://…", prim_path: () }`;
`Prepend`, `Append`, `Add`, and `Delete` retain weaker-layer arcs, while
`Explicit` replaces the selected layer's list and an empty list clears it.
`InspectUsdDocument` exposes `prim.references.authored` and
`prim.references.composed`; resolve the target before submitting so a
composed-only prim fails as read-only.

Use `assembly_edit::default_prim(doc, edit_target, path, parent_gen)` to set
the stage root's `defaultPrim`; it accepts `/Rover` or `Rover` and takes `()` as
`path` to clear only the selected layer. Use
`assembly_edit::prim_kind(doc, edit_target, path, kind, parent_gen)` for the
standard USD prim `kind` token, such as `component`, `assembly`, or `group`;
pass `()` as `kind` to clear that layer's opinion. Both helpers use the typed
`SetDefaultPrim`/`SetPrimKind` operations and trigger a composed projection
resync. `InspectUsdDocument` returns stage metadata at `metadata.defaultPrim`
and prim metadata at `prim.metadata.kind`; each contains `authored.root`,
`authored.runtime`, `composed`, and, when mounted, `canonical_stage` entries
with `present`, `value`, and `source` fields.

`propose` submits a complete typed plan for review without changing the
document. Use `review_session` to inspect its state, `review_proposal` to mute,
unmute, or reject it, and `commit_proposal` to enter the accepted plan as one
ordinary journal/undo change set. The commit rechecks the generation, layer
revision, document origin, external-file watermark, scope, and typed
operations; stale work is reported as a conflict and is not rebased.

Pass `()` for a missing causal predecessor. A generation from `InspectUsdDocument`,
`SyncUsdDocument`, or a command acknowledgement rejects stale writes before any
operation or journal entry is applied. `transform` batches translation and
rotation into one undo unit; `batch` accepts the existing reflected `UsdOp`
variants. `assembly_edit::attach_component` and
`assembly_edit::detach_component` use the existing mount, socket, joint, and
topology validators. `assembly_edit::attach_program(doc,
spec)` passes the complete source, port, connection, and realtime-safety
contract to the typed `AttachProgram` command; build its port maps with the
namespaced `assembly_edit::program_input_connection`,
`assembly_edit::program_input_default`, and `assembly_edit::program_output`
helpers. For a new moving part, `assembly_edit::rigid_body_plan` returns the
explicit body schema, mass, centre-of-mass, and diagonal-inertia operations;
`assembly_edit::revolute_joint_plan` returns a fully framed, bounded joint
with explicit body relationships, axis, degree limits, and collision policy;
`assembly_edit::fixed_joint_plan` returns the equivalent two-frame plan for a
standard fixed joint. Append each plan's `.ops` to one reviewed
`propose`/`batch` change set, then
add the part's shape, collision API, and transform explicitly. Use
`undo`/`redo` on the same explicit document. `keyframe` and `remove_keyframe`
author or remove one USD time sample
through the same journaled operations used by the Editor Inspector; `time` is
a USD time code and `value` is the literal for its explicit `type_name`.
Playback and scrubbing remain the shared `ControlAnimation` transport in the
Environment panel. There is no assembly-specific runtime setter or second
animation clock.

### Semantic assembly construction

Use the dynamically loaded `assembly_builder` library when authoring a model
from parts. It expresses intent above individual USD operations and returns a
typed plan for the same reviewed `assembly_edit::propose`/`batch` boundary:

```rhai
let part = assembly_builder::movable_cube_plan(
    "@root@", "/Assembly", "Panel", 2.0,
    [0.0, 0.0, 0.0], [0.5, 0.75, 0.5],
    1.0, [1.0, 0.0, 0.0], (), [1.0, 0.2, 0.8], true,
);
let proposal = assembly_edit::propose(
    doc, #{ Assembly: () }, "Create panel", part.ops, generation,
);
```

For a reusable parametric component, bundle its geometry and interfaces before
building the reviewed plan. Geometry roles are explicit: `render` controls
visual realization, `collision` controls the standard collision API, and
`material` names an already-authored/composed USD material target.

```rhai
let bundle = #{
    units: "m",
    dimensions: [1.2, 0.8, 0.05],
    geometry: [
        #{ name: "Panel", kind: "Cube", size: 1.0,
           render: true, collision: true,
           material: "/Assembly/Looks/Panel" },
        #{ name: "Drive", kind: "Cylinder", radius: 0.08,
           height: 0.3, axis: "Z", render: true, collision: false },
    ],
    mass: #{ value: 2.0, center_of_mass: [0.0, 0.0, 0.0],
             diagonal_inertia: [0.2, 0.3, 0.4] },
    frames: [
        #{ name: "Mount", role: "attachment", mount_kind: "panel" },
        #{ name: "DriveEnd", role: "actuator" },
    ],
    actuators: [
        #{ name: "deployment", direction: "input", unit: "rad",
           frame: "DriveEnd", default_value: 0.0, limits: [0.0, 1.57] },
    ],
    deployment: #{ state: "stowed", units: "rad", limits: [0.0, 1.57] },
};
let facts = assembly_builder::component_bundle_facts(bundle);
let plan = assembly_builder::component_bundle_plan(
    "@root@", "/Assembly", "Panel", bundle,
);
let proposal = assembly_edit::propose(
    doc, #{ Assembly: () }, "Create panel", plan.ops, generation,
);
```

`component_bundle_facts` is the validation boundary; use its normalized
`contract` for requirement reports. `component_bundle_plan` emits the root,
standard geometry, collision/material bindings, frames, mass facts, and
namespaced actuator properties as one typed plan. Units are currently SI
metres, geometry dimensions must be positive, and attachment/actuator frame
names must be unique. The plan does not guess a body or joint and does not
create a material, so add an explicit reviewed joint/mount plan and author the
material target separately when those contracts are required.

`place_plan` handles local translation, Euler XYZ rotation, and scale;
`frame_plan` handles a validated empty Xform frame;
`cube_plan` and `cylinder_shape_plan` handle standard geometry and explicit
collision APIs;
`hinge_plan` handles a fully framed revolute joint; and the alignment plans
use explicit queried prim/shape paths. Center alignment requires one authored
parent. Cube-edge alignment additionally requires axis-aligned cube extents,
explicit edge signs, and a non-negative gap. Unsupported or ambiguous input is
returned as a failed plan before any document mutation. A completed body
contract or standard joint identity is not promoted twice; use an explicit
attribute/transform update plan for an already-authored identity. The
implementation is
ordinary `.rhai` under `assets/scripting/tools/`, so it can be replaced or
registered at runtime without adding a Rust command or a second USD writer.

For reusable referenced models, `referenced_instance_plan` authors one
explicit identity, asset URI, parent, and local placement using the source
layer's `defaultPrim`. `referenced_instance_targeted_plan` accepts an explicit
absolute source prim when the composition asset requires that identity.
Package (`lunco://`) and Twin-local (`twin://`) asset identities use the same
typed path.
First-use reference loading and variant reconfiguration are separate reviewed
plans: wait until the composed instance children are queryable, then use
`select_variants_plan`. Parameter edits use `parameter_plan`, which returns
typed `SetAttribute` operations for the same `ApplyUsdOps`/proposal boundary
used by the Editor. The core tool has no model-specific paths; a Twin-local
recipe supplies those paths and facts.

For a generic Inspector or AI editing loop, first call
`editable_property_catalog(doc, path, edit_target, requested)`. Give it an
explicit field array for a focused view, or `()` to discover supported
standard `UsdGeom`, `UsdPhysics`, `UsdShade`, `kind`, variant, and
`inputs:`/`outputs:` properties. The result includes each field's owner,
type, units, composed value, USDA literal, edit scope, source path, and
`editable`/read-only status. Unknown names, including guessed `lunco:` fields,
are rejected. Structural `xformOpOrder` and derived `extent` are reported but
not editable.

Turn an edit list into a dry plan with
`editable_property_patch_plan(doc, edit_target, path, edits, parent_gen)`:

```rhai
let catalog = assembly_builder::editable_property_catalog(
    doc, "/Rover/Panel", "@root@", (),
);
let plan = assembly_builder::editable_property_patch_plan(
    doc, "@root@", "/Rover/Panel",
    [#{ name: "xformOp:translate", value: [2.0, 0.0, 0.0] },
     #{ name: "inputs:surface_area", type_name: "float", value: 3.5 }],
    catalog.generation,
);
let proposal = assembly_edit::propose(
    doc, #{ Rover: () }, "Edit panel properties", plan.ops, plan.parent_generation,
);
```

The patch planner preserves true no-op edits, validates exact types and target
scope, and returns typed `SetTranslate`/`SetAttribute`/relationship/kind/
variant operations without writing source. Review the proposal before commit;
do not bypass the document journal with a raw USDA rewrite.

For a whole reusable component, use the generic `component_editor` facade. It
is a Rhai tool library, so its dependencies are loaded through ordinary Rhai
imports rather than a Rust registry. `selected_update_context(preview, ())`
reads the exact selected component, generation, topology, and schema-owned
property catalog. A Twin or model package supplies the explicit component
bundle recipe; USD has no standard parametric-recipe schema and the editor
must not infer one from child names or materials:

```rhai
let context = component_editor::selected_update_context(preview, ());
let plan = component_editor::update_plan(
    context.doc_id, context.edit_target, context.path, bundle, bindings,
);
let proposal = assembly_edit::propose(
    context.doc_id, #{ Assembly: () }, "Update component", plan.ops,
    context.generation,
);
```

`update_plan` delegates to `assembly_builder::component_bundle_update_plan`.
It updates only existing standard geometry, transforms, mass facts, and
explicit bindings; it rejects topology, kind, collision-role, visibility, and
material drift. Review the proposal and commit it through
`review_session`/`commit_proposal`. An unchanged recipe is a true no-op, and a
failed or stale context produces no journal entry.

`place_with_clearance_plan` is the conservative placement path for parts that
must stay clear of authored geometry. It takes exact moving/blocker frame and
Cube-shape paths under one translation-only parent, checks the composed Cube
envelopes with `assembly_audit::aabb_clearance`, and rejects overlap,
insufficient gap, duplicate blockers, rotated frames, or non-Cube geometry
before the plan reaches proposal. For referenced, Cylinder, Mesh, or compound
bodies, use `place_with_collision_clearance_plan(doc, edit_target, moving_path,
translation, blockers, minimum_gap)`. It reads each aggregate composed
collision envelope through `QueryUsdPrim { collision_bounds: true }`, requires
a shared translation-only parent chain, and rejects missing or malformed
collision data before returning the same reviewable transform plan. The
geometry and transform rules remain in the shared USD owner; Rhai supplies only
the placement policy and exact paths. Use
`align_collision_centers_plan` or `align_collision_edges_plan` for
center/edge snap of those same general bodies; both preserve non-target
translation components and return the same reviewed `SetTranslate` plan.
The resulting operation is still
reviewed and committed through `assembly_edit`, so replacing this Rhai policy
does not create a second USD writer.

For Editor selection diagnostics, add `topology: true` to `QueryUsdPrim`:

```rhai
let selected = query("QueryUsdPrim", #{
    doc_id: doc,
    path: selected_path,
    topology: true,
});
```

When a check needs several existing prims, use `QueryUsdPrims` so the runtime
validates the document/projection once and reads one composed-stage snapshot:

```rhai
let records = query("QueryUsdPrims", #{
    doc_id: doc,
    paths: ["/Assembly/FrameA", "/Assembly/FrameB"],
    attrs: ["xformOp:translate"],
});
```

The provider is strict: an invalid or missing path fails the whole request;
`records` remains in the requested deterministic path order. Use
`QueryUsdPrim` when a rule intentionally probes one path that may be absent and
needs its individual error. Do not recreate a Rhai cache around these reads;
the native provider owns the snapshot boundary and generation check.

For numeric authoring evidence, use the built-in `authoring_measurements`
Rhai library. It evaluates explicit requirements over the same composed USD
query and never guesses geometry or selects a document implicitly:

```rhai
let report = authoring_measurements::requirement_report(doc, [
    #{ kind: "distance", from: "/Assembly/FrameA", to: "/Assembly/FrameB",
       units: "mm", expected: 250.0, tolerance: 0.5 },
    #{ kind: "extent", path: "/Assembly/Body", axis: 1,
       units: "m", expected: 1.2, tolerance: 0.01 },
]);
```

Every requirement is evaluated and retained in `report.checks`. The compact
`report.findings` list contains only `failed` or `unavailable` checks, so a
missing prim, missing collision representation, unsupported unit, or stale
document cannot appear as a passing measurement. Results carry the exact
paths, frame, units, tolerance, method, and source for a review panel or
agent to act on.

`selected.topology` is one read-only record. Its `selection.scope` is the
nearest rigid-body ancestor, or the selected prim when no body owns it.
`parts` combines visual and collision facts instead of making callers join two
inventories: each row has `visual`, `collider`, `collision_enabled`,
`body_owner`, inherited `purpose`, canonical local/world `transform`, per-shape
`bounds`, source-layer information, and render/physics material plus resolved
shader paths. `joints` contains standard physics joint body targets in the same
scope. Read `diagnostics` before treating a null bounds/material value as
meaningful; unsupported or malformed authored data remains visible there.
`projection` reports composed document/live-stage generation and `binding`
reports the selected prim's existing visual/physics projection markers. The
topology walk is opt-in and does not mutate selection, USD, or ECS state.

`find_compatible_socket` and `mount_component` are the referenced-part path:
they select a unique authored socket, derive its typed fixed/revolute/prismatic
joint from mount metadata, and delegate plug-frame placement, reference
lowering, occupancy, joint creation, and journalling to the existing
`AttachComponent` owner. They reject missing, occupied, incompatible, and
ambiguous sockets before submission. This lets any assembly script build from
reusable USD components without reproducing USD syntax or frame math in every
scenario.

For an already attached part, use
`assembly_builder::mount_frame_realignment_plan(doc, edit_target, host_path,
socket_path, part_path, joint_path)`. It follows the explicit host socket,
part plug frame, occupancy, attachment-joint, and body relationships, composes
the nested rigid frame chains, and returns one four-operation plan that updates
the part transform and the exact joint anchors without rebuilding topology.
The dynamic policy supports canonical `translate`/`rotateXYZ` frame stacks with
unit scale and rejects missing/ambiguous relationships, unsupported operations,
malformed values, non-rigid scale, and body mismatches before proposal. Capture
the inspected generation, then send the returned `.ops` through the normal
`propose`/`review_session`/`commit_proposal` flow. The
`assembly_mount_frame_realign` production fixture keeps the complete observable
test in Rhai, including nested rotation, topology preservation, and negative
plans; changing this policy does not require a Rust-core rebuild.

For AI-friendly discovery before any edit, use
`assembly_builder::authoring_context(doc, path, edit_target)`. The result keeps
the exact path, parent, document generation, resolved target, composed prim
record, topology/collision facts, authored sockets and occupancy, plug-frame
relationships, and an explicit `actions` affordance list together. It is
read-only and never guesses a target from a name.

Use `assembly_builder::place_or_attach_plan(doc, edit_target, request)` for a
reviewable placement/attachment intent. An `attach_component` request returns
an explicit `spec` for `assembly_edit::attach_component` after selecting one
compatible empty socket; a `realign_existing_mount` request returns the typed
`.ops` from the existing frame planner with `moved` and `fixed` paths. Apply
the reviewed result through the existing typed owner: proposal/review/commit
for `.ops`, or `assembly_edit::attach_component(doc, plan.spec)` for a new
component. This keeps generated authoring and human editing on the same USD
paths and validation boundary without adding a Rust policy layer.

When authoring from the open Editor, use
`assembly_builder::selected_authoring_context(preview)` to turn the current
single selection into that same exact authoring record. Pass `()` for the
focused preview or an explicit preview id for a hidden session. It rejects
no-selection, multiple selection, stale entries, and ambiguous projected paths;
it never guesses a document or prim from a tab/name. Treat its selection
identity and generation as a checkpoint before proposing an edit.

Use `assembly_builder::functional_frame_catalog(doc, edit_target, root_path)`
to discover authored datum, attachment, and actuator frames. It follows only
the root's explicit frame relationships and returns exact paths, roles,
mount/actuator facts, local transforms, root-relative transforms, and socket
paths. Use `assembly_builder::align_frames_plan(doc, edit_target,
moving_path, moving_frame_path, target_path, target_frame_path)` for a dry
visual placement plan. The roots must be sibling Xforms and the frame chains
must be rigid `translate`/`rotateXYZ` with unit scale; review the returned two
typed transform operations before sending them to `assembly_edit::propose`.
Use the existing attach/realignment planner when physical joint topology must
change. These helpers are generic Rhai policy over the existing USD query and
journal owners, so a vehicle recipe does not need a new Rust builder.

Mission-specific builders stay in the owning Twin. They should compose the
generic `assembly_builder` plans, keep all paths and study facts explicit, and
submit only typed operations through `assembly_edit`; they do not belong in the
core tool library or require a model-specific Rust writer.

Structural authoring uses the same typed operation surface: `add_prim`,
`remove_prim`, `move_prim`, `payload`, and `active` expose the existing
reversible USD operations. They require an explicit edit target, exact paths,
and the generation returned by inspection; raw layer replacement is not part
of the agent workflow.

### Authored assembly diagnostics

Use the [`assembly_audit`](../assets/scripting/tools/assembly_audit.rhai)
library with an explicit document and composed assembly manifest, for example
`assembly_audit::topology_report(doc, manifest)`. Every stage-reading helper
takes `doc` first; pass `()` only to audit the mounted live scene. A document
query fails until its canonical projection matches the document generation;
it never reads another preview or falls through to the live scene.
`topology_report` checks
prim existence, type, direct children, and caller-supplied relationship targets;
`mount_contract_report` checks the host socket, socket occupancy, component
attachment joint, asset, and joint-kind reciprocity. `joint_frame_report`
checks standard joint types, explicit bodies, schema-specific primary axes,
limits, and optional local frame opinions. See the
[joint diagnostic contract](architecture/48-assembly-editor.md#agent-and-automation-surface)
for axis applicability and defaults. `body_joint_coverage_report` and `collider_mass_report` check that
actual movable rigid bodies have mass/inertia and explicit joint/collider
coverage. Raycast wheels remain outside rigid-body/joint coverage because they
are not jointed bodies.

For one asset-level decision, use
`assembly_audit::physicality_report(doc, manifest)`. Each manifest entry must
declare `path` and `role: "physical"` or `role: "visual-only"`. Physical entries
reuse the existing `mass`, `inertia`, `joints`, and `colliders` fields and
delegate their checks to the two reports above. Visual-only entries require a
non-empty `reason` and are rejected if the composed collision query finds an
envelope. Duplicate paths, unknown roles, missing prims, and incomplete
physical coverage fail closed. The result contains one structured `parts`
record per decision plus exact `errors`, so generated authoring tools can show
the failing part without parsing a diagnostic string.

`explode_plan(parts, axis, spacing)` produces a structured, non-mutating set of
preview deltas. Apply any reviewed edit through `assembly_edit` and its typed
USD journal boundary; the audit library never writes transforms, chooses a
target by name, or creates a parallel topology registry. Tests should include
negative manifests for missing paths or reciprocal relationships when the
asset contract warrants them.

For an open headful Editor preview, use `assembly_edit::preview_explode_enable`
with the explicit `preview`, `doc`, `assembly`, and non-empty `parts` paths.
Use `preview_explode_update` to change axis/spacing and
`preview_explode_reset` to restore the captured local transforms. These
wrappers call the typed `ExplodeUsdPreview` command; they require
`InspectUsdViewport` to report `projection_ready: true` and never author USD.

Use `assembly_ui` for the presentation step after the document and preview
identities are known. `assembly_ui::panel_templates(preview, doc,
edit_target)` returns nine existing Editor surfaces/workflows with explicit
session handles. `assembly_ui::open_session(preview, doc, edit_target)`
activates the existing `editor` perspective, admits/focuses the explicit USD
preview, and focuses its viewport; `focus`, `open_structure`,
`open_inspector`, `open_connections`, `open_animation`, `open_mount`, and
`open_review` foreground existing panels. Animation is an Environment section,
mount and review are Inspector sections, and persistence is the existing
document lifecycle command group rather than a fabricated panel. These
functions do not create a layout, duplicate view-model state, or infer a
document from a name. The returned preview admission is not a substitute for
`InspectUsdDocument`/`InspectUsdViewport` or a runtime screenshot; continue with the headful
checkpoint in the assembly-editor runbook.

Use `assembly_ui` for a focused assembly in Editor and use the `rover_build`
perspective for general live-Twin base composition. Build treats each authored
USD compound root as one selectable element through the existing
`PhysicsRigidBodyAPI`/`SelectableRoot`/`MobilityRoot` projection; there is no
name-based or script-owned group registry. Internal rover or lander parts are
edited by opening that assembly's explicit USD document and preview in Editor.

## F. Policy hooks (decision functions)

See the complete contract in
[`architecture/hook-policies.md`](architecture/hook-policies.md). A policy
hook is a function-shaped seam: the owner declares a typed positional
signature with `lunco_hooks::declare_hook!`, Rust supplies a small fact map,
and an authored Rhai function returns the declared type. The declaration is
collected automatically from the owner; there is no central hook list.

Application policy selection is authored in
[`assets/scripting/policy/index.toml`](../assets/scripting/policy/index.toml)
and loaded at simulation startup. Its single `[startup]` function receives the
resolved policy records and installs every `[[policies]]` entry through the
typed bootstrap surface. A Twin may add its own `policies/index.toml` with a
separate `[startup]` function; the Twin function receives its authored entries,
which replace matching application policies when the Twin is active. Use
`list_hooks()` to inspect the reflected
`parameters: [{name, type}]` and `output` contract, `policy_status()` to read
load diagnostics, and `invoke_hook(id, [args])` to call an installed function.
With native-provider support enabled, `policy_status().native_plugins` reports
loaded provider ids and admission failures through the same status object.

`bind_policy(id, entry, source)` can install a local non-deterministic policy
for an installable seam. `unbind_policy(id)` removes exactly that
implementation; it does not silently restore another implementation. A
deterministic seam must be selected through an authored manifest that explicitly
marks it deterministic. Internal calls use typed `HookValue`, not JSON.

An explicitly approved Twin may also provide a native implementation for an
existing installable hook with `[[native_plugins]]` in `twin.toml`. The native
provider uses the same reflected contract and invocation path as a Rhai policy;
it is suitable for trusted, expensive kernels and returns typed data for the
Rust owner to validate and apply. Native code is not a sandbox and cannot load
from a USD or script-discovered path. See
[`architecture/native-hook-providers.md`](architecture/native-hook-providers.md)
for the provider lifecycle, ABI, diagnostics, and terrain extension boundary.

For example, [`control_authority.rhai`](../assets/scripting/policy/control_authority.rhai)
implements `control.authority.take`: it receives the owner-defined `ctx` map
and returns `bool`. Rust still owns authorization floors, validation, and the
effect of the result; Rhai owns the changeable decision.

When adding behavior, first check whether it is a hook candidate: a
changeable lifecycle rule, routing/selection decision, presentation choice,
permission, or Twin/scenario policy belongs at a typed Rust hook boundary with
its implementation in Rhai. Hooks can return structured nested maps and arrays
when the owner needs a decision or action plan, not only a scalar. The Rust
owner must validate and consume that result through a generic mechanism; do not
add a hook whose result is ignored. Keep continuous math, kinematics,
dynamics, invariants, and other hot-path mechanisms in Rust or Modelica, and
keep USD facts/topology authored in USD. Record the hook's owner, signature,
startup scope, lifecycle, failure semantics, and a production Rhai test.

The lifecycle hook's returned map is retained as the current lifecycle record
and is visible in `policy_status().lifecycle` (and the API status view), so a
structured policy result is observable rather than silently discarded. If a
seam only emits a notification, declare and return `Unit` instead.

## G. Authored controllers & control authority

A vessel that drives itself through an authored program is built in **three layers**: the
control **LAW in Modelica** (`.mo`), high-level **logic/events in rhai** (no per-tick
loops), and **structure/authority in USD**. Full recipe + gotchas:
[`skills/authoring-vessel-controllers`](../skills/authoring-vessel-controllers/SKILL.md).

**Control authority is the wired `piloted` signal.** The GNC is *internal* to the
vessel model; a user and an authored program are both *external sessions* that **possess**
the vessel (arbitrated by possession + RBAC). The internal controller yields to
whoever possesses by reading the read-only **`piloted`** cosim port (`1.0` when any
session owns the vessel — `SessionRegistry::owner_of(...).is_some()`), wired into the
model (`piloted:piloted`) and gating `cmd = piloted ? stick : gnc`. No in-model flag,
no rhai toggle, no per-tick check — possession is the single source of truth. Ride the
camera along without taking control via `follow(entity)`.

> [!IMPORTANT]
> **`piloted` selects the setpoint SOURCE — it is not a permission gate on attitude.**
> An unpossessed vessel still has full attitude authority via its `guidance_*` wires; what
> it loses is the external stick. So an unpossessed lander does not refuse to fly, it flies
> *itself* — and every `external_*` port write is **silently discarded** while the vehicle
> continues on GNC. A vehicle that "ignores your throttle and just falls" is almost always
> an unclaimed vessel, not a broken command.
>
> Claim it before commanding it:
> ```rhai
> fn on_start(me, ctx) { cmd("AcquireControl", #{ target: me }); }
> ```
> The claim keys on `target`, **not on an avatar**, so this works headless — an unattended
> or server-side run needs no avatar to hold authority.

For a controller that does not need avatar presentation, use the generic
authority commands directly:

```rhai
claim_control(me);
drive(me, 0.5, 0.0);
release_control_claim(me);
```

`ClaimControl` and `ReleaseControlClaim` update the session authority table;
`AcquireControl` composes the same transition with an avatar `ControlLink` and
optional camera binding.

## H. Task programs and the reusable kernel

Layer-1 tasks and Layer-2 timelines are authored in Rhai. Complex reactive
policy is composed from the same generic task tree: selectors, parallel/race
branches, waits, event leaves, and anonymous action/predicate closures.

The `lunco-behavior` crate owns only the reusable cursor, reset, and composite
mechanics. Rhai owns route choice and mission policy; there is no separate
vessel-specific behavior command or JSON behavior specification.

Return a task tree from `task(me, ctx)` and attach it through a scene-level
`LunCoProgramAPI` source:

```rhai
fn task(me, ctx) {
    reactive_sel([
        seq([check(|m| obstacle_ahead(m, 8.0, 50.0)), once(|m| brake(m))]),
        forever(step(|m| nav_to(m, [120.0, 0.0, 50.0], 0.7, 3.0), |m| false)),
    ])
}
```

Available constructors include `seq`, `sel`, `par_all`, `par_race`,
`reactive_seq`, `reactive_sel`, `repeat`, `forever`, `retry`, `check`,
`wait`, `wait_until`, `wait_for`, and `once`/`step` action leaves. The
production contract is exercised by
[`scripting_task_contract`](../assets/scenarios/tests/scripting_task_contract.rhai).

## I. Persistence

- **Per-entity scenarios → USD (load):** a script is a `LunCoProgramAPI` child prim, and it
  auto-attaches and runs when the prim is spawned:
  - `uniform asset info:sourceAsset = @lunco://scenarios/foo.rhai@` — the shipped
    file, resolved through the asset boundary.
  - `uniform string info:sourceCode = '''<rhai>'''` — the source authored in place
    in the USD layer when `info:implementationSource = "sourceCode"`. An edit to it is
    an ordinary attribute edit, so it journals, undoes and replicates like any other.
  - `custom float lunco:param:<key> = <v>` — one typed attribute per per-instance setting,
    read in-script by `param(me, "<key>", default)`.
  The program child is the authored attachment point, but the runtime binds `me` to
  its immediate owning prim. Use `name(me)` for the vessel or mission host that owns
  the program; `parent(me)` refers to the scene hierarchy above that owner and is not
  a replacement for the program host.
- **Tool libraries → files:** `<twin>/tools/*.rhai` (see [§E](#e-tools-shared-libraries)).
- **Timelines → files:** `RegisterTimeline { name, timeline }` stores to `<twin>/timelines/<name>.json`; reloaded on Twin open. Discover with `ListTimelines`/`GetTimeline`; run a stored one with `RunStoredTimeline { target, name }`.
- **Model events → USD:** express the condition in Modelica as a 0/1 output, then connect
  it to a `def LunCoEvent` prim through `inputs:trigger.connect`. The prim supplies only
  the bus-facing `lunco:event:name` and `lunco:event:severity`; scripts receive its rising
  edges through `on_event`. Physical thresholds and hysteresis stay in the model.

## J. Introspection & discovery

| Query | Answers |
|---|---|
| `ScriptStatus { target }` | *Is it healthy?* — compile/runtime diagnostics (state, ok, located errors) |
| `ScriptInspect { target }` | *What is it doing?* — live `this` state, defined hooks, generation, paused/running, plus the status block |
| `ScriptingCatalog` | the full callable surface in one doc: `verbs`, `hooks`, `prelude`, `tools`, `commands`, `queries` — the authoring/discovery source of truth |

## K. Debugging, Diagnostics & Error Handling

Developing scenarios requires quick feedback on compilation and runtime health. The scripting runtime provides several built-in mechanisms for debugging:

### Standard Output & Logging
You can print variables and state information directly to standard output/console using the standard print statement:
```rhai
fn task(me, ctx) {
    seq([once(|m| print("Rover " + name(m) + " position: " + world_pos(m)))])
}
```

### Inspecting Script Status
When a script fails to compile or crashes at runtime, the engine exposes detailed error logs (including file origin, line, and column numbers). You can retrieve this diagnostic information via the `ScriptStatus` API query:
```json
// Query
{"type":"ExecuteCommand","command": "ScriptStatus", "params": {"target": 1234}}

// Response
{
  "ok": false,
  "state": "CompileError",
  "error": "Syntax error: expected ';' (line 12, position 45)"
}
```

### Live Variable Monitoring
You can inspect the live keys and values of the `this` state map attached to any running scenario using `ScriptInspect`:
```json
// Query
{"type":"ExecuteCommand","command": "ScriptInspect", "params": {"target": 1234}}

// Response
{
  "generation": 3,
  "paused": false,
  "state": {
    "count": 142,
    "current_route_point": [10.0, 0.0, 50.0]
  }
}
```

## L. Networking & determinism

Scenarios are **host-authoritative**: they run on the `Host` and in single-player
(`Standalone`), but **not** on a networked `Client`. A client receives scripted
behaviour via replication of the resulting entity state — it does not re-run the
script (which would double-fire `cmd()`/`emit()` and diverge the per-entity
`this`). For deterministic behaviour scripts read the fixed clock (`dt`,
`sim_tick`, `elapsed_seconds`); `rand()` is available but uses **deterministic
per-hook seeding** (`(entity, tick, hook)` triple) so a re-run at the same tick
produces the same sequence — no explicit seeding needed.

## M. Running a scenario

| Transport | How |
|---|---|
| HTTP API | `{"type":"ExecuteCommand","command":"RunScenario","params":{"target":<gid>,"source":"<rhai>"}}` |
| MCP | the `run_scenario` tool (`mcp/src/index.js`) |
| One-shot eval | `RunRhai { code }` — runs once with full world access; stdout in the original deferred response |
| Structured click tool | `RunRhaiTool { tool, args }` — invokes `on_click(context)` with typed values; scene contexts include diagnostic `button` (`primary`, `secondary`, or `middle`) and settings-derived semantic `pointer_intents`; authored tools should use `pointer_intent(context, name)` |
| Control | `SetScenarioPaused { target, paused }`, `StopScenario { target }` |

## N. Examples index

| File | Shows |
|---|---|
| [`patrol.rhai`](../assets/scripting/examples/patrol.rhai) | a looping task-tree route policy |
| [`mission.rhai`](../assets/scripting/examples/mission.rhai) | event-channel coordination between scripts |
| [`mission_plan.rhai`](../assets/scripting/examples/mission_plan.rhai) | a declarative route plan via the task kernel |
| [`sequence.rhai`](../assets/scripting/examples/sequence.rhai) | a linear task-tree sequence |
| [`timeline.rhai`](../assets/scripting/examples/timeline.rhai) | a Layer-2 mission as data |
| [`robot_mission.rhai`](../assets/scripting/examples/robot_mission.rhai) | task-tree mission with durable phase checkpoints and the default no-`on_tick` style |
| [`script_first_robot.rhai`](../assets/scripting/examples/script_first_robot.rhai) | USD component assembly plus a Modelica control graph batch |
| [`multi_robot_mission_coordinator.rhai`](../assets/scripting/examples/multi_robot_mission_coordinator.rhai) | single-authority event-driven assignment coordinator |
| [`multi_robot_mission_worker.rhai`](../assets/scripting/examples/multi_robot_mission_worker.rhai) | identity-scoped worker that installs a native task tree |
| [`avoid.rhai`](../assets/scripting/examples/avoid.rhai) | sensing + obstacle avoidance |
| [`tools/assembly_builder.rhai`](../assets/scripting/tools/assembly_builder.rhai) | selected-prim authoring context, functional-frame catalog, dry frame alignment, semantic placement, generic component bundles, Cube and composed collision alignment/clearance, referenced component instances, staged variant selection, geometry, socket mating, retrofit, and body/joint plans |
| [`tools/formation.rhai`](../assets/scripting/tools/formation.rhai) | a tool library (formation flying) |
| [`tools/survey.rhai`](../assets/scripting/tools/survey.rhai) | a custom tool library (survey pattern) |

## Links

- [Command reference](./commands-reference.md) — every `#[Command]`, auto-generated
- [lunco-scripting crate README](../crates/lunco-scripting/README.md)
- [Rhai integration design & as-built reference](./architecture/rhai-integration.md)
- [prelude/](../assets/scripting/prelude) — the helper library (one file per topic)
- [Examples directory](../assets/scripting/examples)
- [Crate index](./crates-index.md)
- [rhai language reference](https://rhai.rs/book/)
