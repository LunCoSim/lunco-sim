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
program attached to an entity. Its `task(me)` tree is advanced on every fixed
simulation tick by the native behavior kernel; it is not a one-shot snippet.

> **The host (Rust) is mechanism; the script is policy.** Navigation, objectives,
> behaviour trees, sequencing — all live in hot-reloadable `.rhai`, never compiled
> into the engine.

A script touches the world through exactly the same **command/query API** the HTTP
API, MCP, and UI use — so it inherits [every command](./commands-reference.md) for
free and stays decoupled from physics. Scripts are **host-authoritative**
([Part II §L](#l-networking--determinism)).

You'll need a running app with its API on, e.g. the luncosim:

```sh
target/debug/luncosim --api 4101
```

## 1. Mental model

| You write | The engine does |
|---|---|
| `fn task(me)` | builds one native task tree; the kernel advances it each fixed step |
| `fn mission(me)` | declares objective state and completion conditions |
| `fn on_start(me)` | optional setup after (re)compile |
| `fn on_event(me, evt)` | optional reaction to a `TelemetryEvent` |
| `fn on_stop(me)` | optional teardown on hot-reload / detach / despawn |

`me` is the host entity's id. Task progress, dwell timing, and event waits are
owned by the native task kernel. A Rhai function called indirectly by the task
driver must receive its configuration as arguments; do not rely on top-level
variables being visible there. You sense with queries/`get` and act with
`cmd`/`set`.

## 2. Your first script

Create `assets/scenarios/my_rover_mission.rhai`:

```rhai
fn task(me) {
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

The rover drives the waypoints. Re-issue `RunScenario` on the same entity to
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
./scripts/run_scene_tests.sh --no-build autopilot
```

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
fn task(me)          { seq([wait(1.0)]); }               // canonical progression
fn mission(me)       { [objective("landing", #{})]; }   // optional objectives
fn on_start(me)      { /* setup */ }                     // once, after (re)compile
fn on_event(me, evt) { if evt.name == "GO" { /* … */ } } // a TelemetryEvent arrived
fn on_stop(me)       { brake(me); }                      // teardown: hot-reload / detach / despawn
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
| `query(name, #{params})` | **READ** — call a read-only query provider (Raycast, Nearest, GroundHeight…). Successful data is returned directly; no-data is `()`; failures return `#{ok:false,error}`. |
| `query("ListSpawnCatalog", #{})` | **READ** — discover the authoritative `entry_id`, name, category, default transform, and source for assets accepted by `cmd("SpawnEntity", ...)`. |
| `get(id, "Comp.field")` / `set(id, "Comp.field", v)` | reflected component read/write (vectors → `[x,y,z]`); scalar co-simulation names use the canonical `PortRegistry` surface. |
| `find(name)` / `world_pos(id)` | locate an entity; read its f64 active-frame position (site-local on a surface). |
| `emit(name, value?)` | fire a `TelemetryEvent` (delivered to `on_event` next tick). |
| `notify(msg)` / `notify_kind(msg, kind)` | HUD notification (`kind`: `"info"`/`"warn"`/`"error"`). |
| `list_entities()` | every entity (`#{id,name,type,catalog_id,usd_prim_path,pos}`) — identity comes from USD/catalog data; filter/select in-script. |

> **`set` vs `cmd`.** Use `set` to tune a reflected value. When `set`
> falls through to a scalar port it is a raw write and has no persistent hold;
> use `cmd("SetPorts", #{target: id, writes: [[name, value]]})` when wiring must
> be overridden until the shared hold expires; use `cmd("ReleasePort", ... )` to
> end that hold early. Direct
> writes are host-authoritative and unavailable to client-scoped scripts. Use `cmd`
> for an *operation* with side effects
> beyond a field write (spawning, swapping a material, anything an observer reacts to).

## 5. Making it move: navigation & sensing

The prelude turns raw verbs into rover behaviour (read the topic files for the
authoritative list; highlights in [Part II §B](#b-prelude-helpers)):

- **Drive:** `drive(rover, fwd, steer)`, `brake(rover)`, `nav_to(entity, target, speed, radius)`.
- **Sense:** `velocity`/`speed`, `raycast`, `obstacle_ahead`, `ground_height`, `nearest`, `entities_in_radius`.
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
| `query(name, #{params})` | value \| `()` \| error map | **READ** — call any query provider (Raycast, Nearest, GroundHeight, …); successful data is direct, successful no-data is `()`, and failures are `#{ok:false,error}` |
| `query("ListSpawnCatalog", #{})` | map | discover the spawn catalog used to validate `SpawnEntity.entry_id` |
| `get(id, "Comp.field")` | value \| `()` | reflected component **read** (vectors → `[x,y,z]`, quats → `[x,y,z,w]`, structs → maps) |
| `set(id, "Comp.field", value)` | bool | host-side **tuning write** — reflected component field or canonical scalar co-simulation port; supported field types only; not replicated, undoable, or a persistent port hold; `false` on bad path/type |
| `get_setting("Res.field")` | value \| `()` | reflected **resource read** — global settings/config live in resources, not components |
| `set_setting("Res.field", value)` | bool | host-side **tuning write** to a supported reflected resource field; not replicated or undoable; `false` on bad path/type |
| `get_twin_setting("namespace.key")` | value \| `()` | read a scalar from the active Twin manifest's generic `[settings]` table |
| `set_twin_setting("namespace.key", value)` | bool | persist a scalar in the active Twin manifest through the generic `SetTwinSetting` command |
| `get_exposure("namespace", "property")` | value \| `()` | read one raw engine capability value; Rhai owns selection and presentation policy |
| `world_pos(id)` | `[x,y,z]` \| `()` | f64 active-frame position; independent of camera recentering and celestial ancestors |
| `world_forward(id)` | `[x,y,z]` \| `()` | active-frame heading |
| `find(name)` | id (`-1` if none) | entity id by `Name` |
| `name(id)` | string \| `()` | reverse of `find` |
| `parent(id)` / `children(id)` | id \| `()` / `[id,…]` | hierarchy traversal |
| `owner_of(id)` | session id \| `()` | who controls the vessel (`0` = local human, autopilot band = an AI); `()` if unowned |
| `controller(id)` | string \| `()` | driver's role — `"AiAgent"` (autopilot) vs `"Owner"`/`"Operator"` (human) — the human-vs-AI test |
| `is_controlled(id)` | bool | is any session (human or autopilot) driving it |
| `list_entities()` | `[#{id,name,type,catalog_id,usd_prim_path,pos}]` | every registered entity; `type` is the projected USD kind, not a control-component heuristic |
| `add(id, "Comp", #{fields})` | bool | **structural** — insert/replace a reflected component (built from default + fields); needs `#[reflect(Default)]` |
| `remove(id, "Comp")` | bool | **structural** — strip a reflected component |
| `despawn(id)` | bool | **structural** — despawn an entity (+children); replicates on a host. *Spawn:* use `cmd("SpawnEntity", #{entry_id, position})` (no generic spawn — clients reconstruct from the catalog) |
| `emit(name, value?)` | bool | fire a `TelemetryEvent` (delivered to `on_event` next tick) |
| `sim_tick()` / `dt()` / `elapsed_seconds()` | i64 / f64 / f64 | the fixed simulation clock |
| `rand()` / `rand_range(lo,hi)` / `rand_int(lo,hi)` | f64 / f64 / i64 | **deterministic** RNG — seeded per hook from `(entity, tick, hook)`, identical on every peer and replay |
| `param(id, key, default)` | any | read a `lunco:param:<key>` attribute from a prim (`custom float lunco:param:wmax = 1.05`); returns `default` if it is absent |
| `detach_joint(id)` | bool | despawn a joint entity (releases the rigid link between two bodies, e.g. lander→rover) |
| `notify(msg)` / `notify_kind(msg, kind)` | () | send a HUD notification; `kind` is `"info"` / `"warn"` / `"error"` |

JSON appears **only** at the `cmd`/`query` params seam (that's the API's own
contract). Both directions are native: `get`/`get_setting` build rhai values
straight from reflect, and `set`/`set_setting` write rhai values straight back —
no JSON round-trip on the read or write path.

> **`set` vs `cmd`.** Use `set`/`set_setting` for host-side tuning through the
> reflected field surface or the canonical scalar co-simulation port surface.
> This is a raw write, not a persistent hold; use
> `cmd("SetPorts", #{target: id, writes: [[name, value]]})` for a hold. Direct
> writes are host-authoritative and unavailable to client-scoped scripts. Use `cmd` for
> an *operation* with side effects beyond a field write (spawning, swapping a
> material, anything an observer must react to). Settings are only reachable if
> their type is `register_type`'d with `#[reflect(Component)]` / `#[reflect(Resource)]`.

## B. Prelude helpers

The [`prelude/`](../assets/scripting/prelude) directory (one `.rhai` per topic —
`nav`, `sensing`, `control`, `tasks`, `mission`, `patrol`, `science`, `links`,
`math`, `select`, `hud`, …) is the hot-reloadable helper library on top of the
verbs — read the topic files for the full, authoritative list. Highlights:

- **Vector math:** `vsub`/`vadd`/`vlen`/`vdot`/`vcross`/`vnorm`/`vscale`/`clamp`, `distance`, `arrived`.
- **Navigation:** `drive(rover, fwd, steer)`, `brake(rover)`, `steer_to`, `nav_to(entity, target, speed, radius)`.
- **Sensing:** `velocity`/`speed`, `raycast`, `obstacle_ahead`, `ground_height`, `nearest`, `entities_in_radius`.
- **Connectivity / routing** ([`links.rhai`](../assets/scripting/prelude/links.rhai)): `links()` (the live link graph — `#{nodes, adj, edges, groups}` from `query("Links")`), `reachable(from, to)`, `link_path(from, to)`, `link_path_names(from, to)`, `can_reach(rover, station)`. The Rust kernel computes only link GEOMETRY at a tunable cadence and publishes the graph; **routing is pure rhai policy** — call it at decision time (e.g. in `on_event` on `link.los`), not every tick. Nodes are identified by **GID** — the same id `find()` returns — and every helper takes either a GID (that node) or a `lunco:link:class` string (the GROUP with that role), so `can_reach(find("…/Comms"), "earth")` means "any Earth station" while each station stays separately addressable. A class is a shared role, never an identity: three DSN complexes all author `class = "earth"`. See [doc 49](./architecture/49-connectivity-link-kernel.md).
- **Collision events:** `collision_pair`/`collision_other`/`entered`/`exited` (parse `COLLISION_START`/`COLLISION_END`).
- **Task trees (`task(me)`):** every constructor emits a node with an explicit `kind`; there is no field-presence inference. Leaves are `step`/`once`/`act_for`/`act_until_event`/`wait`/`wait_until`/`wait_for`/`wait_for_from`/`check`, composites are `seq`/`par_all`/`par_race`/`sel`/`reactive_seq`/`reactive_sel`, and decorators are `repeat`/`forever`/`retry`/`invert`/`force_ok`/`force_fail`. The adapter rejects missing/unknown kinds and cross-kind fields, then compiles once onto the existing `lunco-behavior` kernel. See [`rhai-task-tree.md`](architecture/rhai-task-tree.md). The kernel emits `TASK_COMPLETE` or `TASK_FAILED`.
- **Timeline (Layer 2):** `compile_timeline`, `timeline_step`. A timeline step
  must contain exactly one explicit operation word (`move_to`,
  `move_to_entity`, `possess`, `brake`, `cmd`, `emit`, `wait`, or `wait_event`);
  common fields such as `subject`, `speed`, `radius`, `secs`, `params`, and
  `value` are validated against that operation at the command boundary.
- **Script-first authoring:** explicit-document `usd_apply` / `usd_apply_ops` /
  `usd_add_prim`, `attach_fixed` / `attach_revolute`, `attach_program`, and
  `modelica_apply`, plus typed Modelica op constructors in
  [`prelude/authoring.rhai`](../assets/scripting/prelude/authoring.rhai).
  These are policy wrappers over the existing journaled command surfaces; USD
  remains the scene/topology authority and Modelica remains the equation/graph
  authority. Obtain document ids from `ListOpenDocuments` before authoring.
- **Mission durability:** `mission_checkpoint` and
  `mission_checkpoint_read` author phase state on the host prim as USD string
  attributes through the explicit document command path. Define
  `fn mission_document(me) { <usd-document-id> }` and use the returned document
  id at objective/phase boundaries so a task can resume after a hot reload or
  restart without a second persistence mechanism. `ListOpenDocuments` supplies
  the id in editor sessions.
- **Selection toolkit:** `all_of_type`, `min_by`/`max_by`, `count_where`, `nearest_where`/`farthest_where`, `has_component`, `kind`.
- **View / cutscenes:** `set_camera(name)` — cut the scene viewport to a `def Camera` by name (leaf or full USD path); pairs with a timeline for cutscene camera changes. `possess(vessel)`, `notify(msg)`, `photo()` (capture from the active camera).
- **Patrol / checkpoints** ([`patrol.rhai`](../assets/scripting/prelude/patrol.rhai)): `engage_patrol(vessel, points, speed?, radius?, dwell?)`, `patrol(vessel, points, …)` (hot-swap an engaged vessel's route), `add_checkpoint(vessel, x, y, z)`, `clear_patrol(vessel)`. Each waypoint may be a bare `[x,y,z]` or a `#{pos, dwell?, on_arrival?}` map carrying arrival actions — the declarative way to "fire a tool at a waypoint" (no tree composition). `clear_patrol` fires the `ClearPatrol` typed command (the canonical stop-&-clear verb).
- **Science instruments** ([`science.rhai`](../assets/scripting/prelude/science.rhai)): `photo_from(vessel)` (capture from a vessel's mounted camera — fires `CaptureFromCamera`), `take_photo()` / `take_photo(args)` (a `run_tool` action value for a waypoint's `on_arrival` list, naming the registered `science::take_photo` tool). The Rust core owns firing & cleaning via the `lunco-tools` registry + `lunco-tools-bevy` dispatch; these helpers just NAME the tool from data.
- **Tutorial HUD** ([`hud.rhai`](../assets/scripting/prelude/hud.rhai)): `hint(msg)`/`clear_hint()` (sticky instruction), `spotlight(anchor, caption)`/`clear_spotlight()` (dim + ring a workbench widget by `HelpAnchors` key), `focus_panel(id)` (open a singleton workbench panel on interactive hosts; unattended gates omit this presentation command), `objectives_hud(list)` (or just declare a `mission(me)` — it auto-publishes), `coach_step(steps, i)` (a guided coach-mark tour step; advance the cursor in `on_event`). This is how tutorials are authored — a tutorial is just a scenario. See [`tutorials/README.md`](../assets/tutorials/README.md).

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

Reuse one source across entities/missions by passing a JSON object string; the
script reads it as the read-only `params` constant:

```jsonc
RunScenario { target: <gid>, source: "...", params: "{\"speed\":1.5}" }
```
```rhai
fn task(me) {
    forever(once(|m| drive(m, params.speed, 0.0)))
}
```

## D. Sequencing (missions)

Two script-first layers, both pure rhai (no engine rebuild):

- **Layer 1 — task tree** ([`sequence.rhai`](../assets/scripting/examples/sequence.rhai)): build a step tree with `step`/`once`/`wait`/`wait_until`/`wait_for`; return it from `task(me)`. The native kernel feeds events and owns the cursor.
- **Layer 2 — declarative timeline** ([`timeline.rhai`](../assets/scripting/examples/timeline.rhai)): a mission as **pure data**. Each step has exactly one operation word (`move_to`, `move_to_entity`, `possess`, `brake`, `cmd`, `emit`, `wait`, or `wait_event`) and only that operation's fields; `compile_timeline` lowers it onto a task tree. Because it's data, a timeline is serialisable — run one inline with `RunTimeline`, or store it (see [§I](#i-persistence)).

Progress is observable on the telemetry bus: `TASK_COMPLETE` or `TASK_FAILED`
for the native task root, plus the mission/objective events emitted by your task
leaves and `mission(me)` declaration.

## E. Tools (shared libraries)

A **tool library** is a named bundle of reusable policy, callable as
`libname::fn(...)` from any hook (no `import` — they bind as static modules).

- Author one: drop a `.rhai` in [`rhai/tools/`](../assets/scripting/tools), or `RegisterToolLibrary { name, source }` at runtime (hot-reloadable).
- Examples: [`formation.rhai`](../assets/scripting/tools/formation.rhai) (formation flying), [`survey.rhai`](../assets/scripting/tools/survey.rhai) (lawnmower survey pattern).
- Discover: `ListToolLibraries`, `GetToolLibrary { name }`.
- **Persistence:** registered libraries are mirrored to `<twin>/tools/*.rhai` and reloaded when the Twin opens.

## F. Policy hooks (decision functions)

Distinct from scenarios: a **policy hook** is a small *pure* rhai function —
`ctx` in → a value out — that a Rust seam consults **by id** at a decision point.
Authored under [`policy/`](../assets/scripting/policy), registered under a
`HookId`, and **hot-rewritable** (replace the file, or `SetScriptedPolicy` the
same id) — so behavior that used to be hardcoded is data, no rebuild.

- [`control_authority.rhai`](../assets/scripting/policy/control_authority.rhai)
  (`control.authority.take`) — may `taker` take a vessel from its current owner?
  (spec 034). Returns `bool`.
- [`boot.rhai`](../assets/scripting/policy/boot.rhai) (`boot.entry`) — what does an
  app do at **startup**? `ctx = #{ onboarded, first_start_id, has_scene_arg,
  automated }` → `#{ command, params }` (the seam dispatches it — e.g.
  `StartTutorial` to onboard) or `()` (the app loads its default). This is where
  "first run → show the tutorial, not the default scene" lives.

The seam supplies context Rust alone can see (argv, roles, first-run flag); the
*decision* is entirely the policy's. Consulted via `lunco_hooks::invoke(id, &[ctx])`.

## G. Vessel controllers & control authority

A vessel that drives itself (a GNC / autopilot) is built in **three layers**: the
control **LAW in Modelica** (`.mo`), high-level **logic/events in rhai** (no per-tick
loops), and **structure/authority in USD**. Full recipe + gotchas:
[`skills/authoring-vessel-controllers`](../skills/authoring-vessel-controllers/SKILL.md).

**Control authority is the wired `piloted` signal.** The GNC is *internal* to the
vessel model; a user and an autopilot are both *external sessions* that **possess**
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
> fn on_start(me) { cmd("PossessVessel", #{ target: me }); }
> ```
> The claim keys on `target`, **not on an avatar**, so this works headless — an unattended
> or server-side run needs no avatar to hold authority.

## H. Autopilot & Behavior Tree Integration

While Layer-1 Sequences and Layer-2 Timelines are useful for linear scripts, complex, reactive, and resilient AI behaviors (like obstacle avoidance and path interception) are best authored using the **Autopilot Behavior Tree System**.

The autopilot accepts a JSON tree specification (`BehaviorSpec`) containing composite nodes, decorators, and actions/conditions, compiling them into a high-performance native behavior tree (see [behaviour-trees.md](./behaviour-trees.md)).

You can trigger a behavior tree on a vessel from Rhai by issuing the `SetAutopilotBehavior` command:

```rhai
fn on_start(me) {
    // Drive to a goal point, but halt if an obstacle is detected in a forward 50-degree cone
    let bt_spec = "{\"kind\":\"reactive_selector\",\"children\":[" +
        "{\"kind\":\"sequence\",\"children\":[" +
            "{\"kind\":\"obstacle_ahead\",\"distance\":8.0,\"cone\":50.0}," +
            "{\"kind\":\"hold\"}]}," +
        "{\"kind\":\"drive_to\",\"target\":[120.0, 0.0, 50.0],\"speed\":0.7,\"radius\":3.0}]}";

    cmd("SetAutopilotBehavior", #{ vessel: me, spec_json: bt_spec });
}
```

Available nodes include:
- **Composites:** `sequence`, `selector`, `parallel`, `reactive_sequence`, `reactive_selector`.
- **Decorators:** `invert`, `force_success`, `force_failure`, `timeout`, `cooldown`.
- **Actions:** `drive_to`, `follow`, `intercept`, `patrol`, `face`, `cruise`, `brake`, `hold`, `steer_clear`, `wait`, `run_tool`. `patrol` waypoints may each carry an `on_arrival` list of actions (e.g. `run_tool`) — see [`patrol.rhai`](../assets/scripting/prelude/patrol.rhai); `run_tool` fires a registered tool once (latched, re-armed by `repeat`/`cooldown`) and is dispatched by `lunco-tools-bevy`.
- **Conditions:** `arrived`, `facing`, `obstacle_ahead`, `path_blocked`.

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
fn task(me) {
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
    "current_waypoint": [10.0, 0.0, 50.0]
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
| Control | `SetScenarioPaused { target, paused }`, `StopScenario { target }` |

## N. Examples index

| File | Shows |
|---|---|
| [`patrol.rhai`](../assets/scripting/examples/patrol.rhai) | a looping waypoint patrol |
| [`mission.rhai`](../assets/scripting/examples/mission.rhai) | event-channel coordination between scripts |
| [`mission_plan.rhai`](../assets/scripting/examples/mission_plan.rhai) | a declarative waypoint plan via the task kernel |
| [`sequence.rhai`](../assets/scripting/examples/sequence.rhai) | a linear task-tree sequence |
| [`timeline.rhai`](../assets/scripting/examples/timeline.rhai) | a Layer-2 mission as data |
| [`robot_mission.rhai`](../assets/scripting/examples/robot_mission.rhai) | task-tree mission with durable phase checkpoints and the default no-`on_tick` style |
| [`script_first_robot.rhai`](../assets/scripting/examples/script_first_robot.rhai) | USD component assembly plus a Modelica control graph batch |
| [`multi_robot_mission_coordinator.rhai`](../assets/scripting/examples/multi_robot_mission_coordinator.rhai) | single-authority event-driven assignment coordinator |
| [`multi_robot_mission_worker.rhai`](../assets/scripting/examples/multi_robot_mission_worker.rhai) | identity-scoped worker that installs a native task tree |
| [`avoid.rhai`](../assets/scripting/examples/avoid.rhai) | sensing + obstacle avoidance |
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
