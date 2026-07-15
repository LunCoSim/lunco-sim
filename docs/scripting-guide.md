# LunCo Scripting Guide

How to write **scenarios** — persistent per-entity programs that sense and drive
the simulation — in LunCoSim.

- **Crate:** [`lunco-scripting`](../crates/lunco-scripting) · **Design rationale:** [rhai-integration-design.md](./rhai-integration-design.md)
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
program attached to an entity that runs every fixed simulation tick. It is *not*
a one-shot snippet.

> **The host (Rust) is mechanism; the script is policy.** Navigation, objectives,
> behaviour trees, sequencing — all live in hot-reloadable `.rhai`, never compiled
> into the engine.

A script touches the world through exactly the same **command/query API** the HTTP
API, MCP, and UI use — so it inherits [every command](./commands-reference.md) for
free and stays decoupled from physics. Scripts are **host-authoritative**
([Part II §L](#l-networking--determinism)).

You'll need a running app with its API on, e.g. the sandbox:

```sh
cargo run -p lunco-sandbox --bin sandbox -- --api 3000
```

## 1. Mental model

| You write | The engine does |
|---|---|
| `fn on_start(me)` | runs once after (re)compile |
| `fn on_tick(me)`  | runs every `FixedUpdate` step |
| `fn on_event(me, evt)` | runs when a `TelemetryEvent` arrives |
| `fn on_stop(me)`  | runs on hot-reload / detach / despawn |

`me` is the host entity's id. Per-tick mutable state lives on the implicit `this`
object map (rhai functions are otherwise pure — they can't see top-level `let`s,
so thread state through `this`). You sense with queries/`get` and act with
`cmd`/`set`.

## 2. Your first script

Create `assets/scenarios/my_rover_mission.rhai`:

```rhai
fn on_start(me) {
    notify("Rover mission initiated!");
    this.wp_index = 0;
    this.waypoints = [
        [10.0, 0.0, 0.0],
        [20.0, 0.0, 10.0],
        [0.0, 0.0, 20.0],
    ];
}

fn on_tick(me) {
    if this.wp_index >= this.waypoints.len() {
        notify("Mission complete! Parking.");
        brake(me);
        return;
    }

    let target = this.waypoints[this.wp_index];
    if nav_to(me, target, 0.8, 2.0) {
        notify("Reached waypoint " + this.wp_index);
        this.wp_index += 1;
    }
}
```

`nav_to` and `brake` are [prelude helpers](#b-prelude-helpers) — high-level
verbs built on the raw `cmd`/`get` bridge. No control loops to hand-code.

### Run it

Attach it to a rover. Get the rover's id (`list_entities()` or the UI), then fire
`RunScenario` over the API (the same path MCP and in-app launchers use):

```json
{
  "command": "RunScenario",
  "params": {
    "target": 4869542932533563,
    "source": "assets/scenarios/my_rover_mission.rhai"
  }
}
```

The rover drives the waypoints. Re-issue `RunScenario` on the same entity to
**hot-reload** after you edit the file — no rebuild, no restart (state resets,
the outgoing program's `on_stop` runs first).

### Inspect & debug

- `print(...)` lands in the console.
- `ScriptStatus { target }` reports compile/runtime health (state, errors with
  file/line/column).
- `ScriptInspect { target }` shows the live `this` map, defined hooks, generation,
  paused/running.

```json
{ "command": "ScriptInspect", "params": { "target": 4869542932533563 } }
```

### Persist it in the scene

So it runs automatically on load, give the prim a program child in your `.usda`. A
program is a prim, not an attribute — delete the prim and the behaviour goes with it:

```usda
def Xform "Rover_01" {
    def LunCoProgram "Mission" {
        uniform asset lunco:program:sourceAsset = @scenarios/my_rover_mission.rhai@
        # …or author the source in place:
        # uniform string lunco:program:sourceCode = '''<rhai source>'''
    }
}
```

That's the whole loop: **write → run → inspect → persist.** The rest of Part I
fills in the everyday verbs; Part II is the complete reference.

## 3. Lifecycle hooks (the full set)

```rhai
fn on_start(me)      { this.count = 0; }                 // once, after (re)compile
fn on_tick(me)       { this.count += 1; }                // every FixedUpdate tick
fn on_event(me, evt) { if evt.name == "GO" { /* … */ } } // a TelemetryEvent arrived
fn on_stop(me)       { brake(me); }                      // teardown: hot-reload / detach / despawn
```

- Define any subset. `on_stop` is where you stop actuators / release claims.
- `this` persists across ticks for one entity.

## 4. The everyday verbs

You'll use these constantly (the complete table is in
[Part II §A](#a-full-verb-surface); every `#[Command]` is in the
[command reference](./commands-reference.md)):

| Verb | Purpose |
|---|---|
| `cmd(name, #{params})` | **WRITE** — fire any command by name (spawn, possess, set input…). Returns `#{ id, ok, data, error }`. |
| `query(name, #{params})` | **READ** — call a query provider (Raycast, Nearest, GroundHeight…). |
| `get(id, "Comp.field")` / `set(id, "Comp.field", v)` | reflected component read/write (vectors → `[x,y,z]`). |
| `find(name)` / `world_pos(id)` | locate an entity; read its float-origin-correct position. |
| `emit(name, value?)` | fire a `TelemetryEvent` (delivered to `on_event` next tick). |
| `notify(msg)` / `notify_kind(msg, kind)` | HUD notification (`kind`: `"info"`/`"warn"`/`"error"`). |
| `list_entities()` | every entity (`#{id,name,type,pos}`) — filter/select in-script. |

> **`set` vs `cmd`.** Use `set` to tune a *value* (a field, a config knob) — a direct
> reflected write, host-authoritative. Use `cmd` for an *operation* with side effects
> beyond a field write (spawning, swapping a material, anything an observer reacts to).

## 5. Making it move: navigation & sensing

The prelude turns raw verbs into rover behaviour (read the topic files for the
authoritative list; highlights in [Part II §B](#b-prelude-helpers)):

- **Drive:** `drive(rover, fwd, steer)`, `brake(rover)`, `nav_to(entity, target, speed, radius)`, `run_plan`.
- **Sense:** `velocity`/`speed`, `raycast`, `obstacle_ahead`, `ground_height`, `nearest`, `entities_in_radius`.
- **Math:** `distance`, `arrived`, `vsub`/`vlen`/`vnorm`/`vcross`, `clamp`.
- **Collisions:** `collision_pair`/`entered`/`exited` (parse `COLLISION_START`/`COLLISION_END`).

A reactive mission (avoid obstacles, run a waypoint plan, coordinate between
scripts) is all rhai — see the [examples index](#n-examples-index).

## 6. Where to go next

- **Every command** `cmd()` can fire: [`commands-reference.md`](./commands-reference.md).
- **Deeper topics** (sequencing, tools, policy hooks, vessel controllers, behavior
  trees, determinism): [Part II](#part-ii--reference).
- **Design rationale**: [`rhai-integration-design.md`](./rhai-integration-design.md).
- **rhai language**: <https://rhai.rs/book/>.

---

# Part II — Reference

## A. Full verb surface

The host exposes a minimal, generic bridge. Everything else is prelude policy.

| Verb | Returns | Purpose |
|---|---|---|
| `cmd(name, #{params})` | `#{ id, ok, data, error }` | **WRITE** — fire any `#[Command]` by name (synchronous; `data` carries assigned values like a spawned gid). The full list is the [command reference](./commands-reference.md). |
| `query(name, #{params})` | value \| `()` | **READ** — call any query provider (Raycast, Nearest, GroundHeight, …) |
| `get(id, "Comp.field")` | value \| `()` | reflected component **read** (vectors → `[x,y,z]`, quats → `[x,y,z,w]`, structs → maps) |
| `set(id, "Comp.field", value)` | bool | reflected component **write** — the mirror of `get`; coerces by field type (int→float, `[x,y,z]`→Vec3); `false` on bad path/type |
| `get_setting("Res.field")` | value \| `()` | reflected **resource read** — global settings/config live in resources, not components |
| `set_setting("Res.field", value)` | bool | reflected **resource write** — tune any registered setting; `false` on bad path/type |
| `world_pos(id)` | `[x,y,z]` \| `()` | float-origin-correct world position |
| `world_forward(id)` | `[x,y,z]` \| `()` | world heading |
| `find(name)` | id (`-1` if none) | entity id by `Name` |
| `name(id)` | string \| `()` | reverse of `find` |
| `parent(id)` / `children(id)` | id \| `()` / `[id,…]` | hierarchy traversal |
| `owner_of(id)` | session id \| `()` | who controls the vessel (`0` = local human, autopilot band = an AI); `()` if unowned |
| `controller(id)` | string \| `()` | driver's role — `"AiAgent"` (autopilot) vs `"Owner"`/`"Operator"` (human) — the human-vs-AI test |
| `is_controlled(id)` | bool | is any session (human or autopilot) driving it |
| `list_entities()` | `[#{id,name,type,pos}]` | every registered entity (filter/select in-script) |
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

> **`set` vs `cmd`.** Use `set`/`set_setting` to tune a *value* (a field, a config
> knob) — it's a direct reflected write, host-authoritative because scenarios run
> host-only, and the change replicates through normal component sync. Use `cmd` for
> an *operation* with side effects beyond a field write (spawning, swapping a
> material, anything an observer must react to). Settings are only reachable if
> their type is `register_type`'d with `#[reflect(Component)]` / `#[reflect(Resource)]`.

## B. Prelude helpers

The [`prelude/`](../assets/scripting/prelude) directory (one `.rhai` per topic —
`nav`, `sensing`, `control`, `tasks`, `mission`, `patrol`, `science`, `links`,
`math`, `select`, `hud`, …) is the hot-reloadable helper library on top of the
verbs — read the topic files for the full, authoritative list. Highlights:

- **Vector math:** `vsub`/`vadd`/`vlen`/`vdot`/`vcross`/`vnorm`/`vscale`/`clamp`, `distance`, `arrived`.
- **Navigation:** `drive(rover, fwd, steer)`, `brake(rover)`, `steer_to`, `nav_to(entity, target, speed, radius)`, `run_plan`.
- **Sensing:** `velocity`/`speed`, `raycast`, `obstacle_ahead`, `ground_height`, `nearest`, `entities_in_radius`.
- **Connectivity / routing** ([`links.rhai`](../assets/scripting/prelude/links.rhai)): `links()` (the live link graph — `#{nodes, adj, edges}` from `query("Links")`), `reachable(from, to)`, `link_path(from, to)`, `can_reach(rover, station)`. The Rust kernel computes only link GEOMETRY at a tunable cadence and publishes the graph; **routing is pure rhai policy** — call it at decision time (e.g. in `on_event` on `link.los`), not every tick. Node keys are the authored `lunco:link:class` (else the prim name). See [doc 49](./architecture/49-connectivity-link-kernel.md).
- **Collision events:** `collision_pair`/`collision_other`/`entered`/`exited` (parse `COLLISION_START`/`COLLISION_END`).
- **Sequencer (Layer 1):** `seq_init`, `run_steps`, `seq_note_event`, step ctors `step`/`once`/`wait`/`wait_until`/`wait_for`/`wait_for_from(event, source_id)`; `seq([steps])` shorthand to build and run immediately.
- **Task trees (`this.task`):** composites `seq`/`par_all`/`par_race`/`repeat`/`forever` plus the failure-aware kernel vocabulary `check(pred)`/`sel`/`retry`/`invert`/`force_ok`/`force_fail`/`reactive_seq`/`reactive_sel`. The constructors build pure data; the tree is compiled once and TICKED NATIVELY on the `lunco-behavior` kernel (the same engine the rover autopilot uses) — a `seq` advances through instantly-done steps within one tick, so use `wait`/`wait_until`/`wait_for` as the suspension points. Emits `TASK_COMPLETE` on root success, `TASK_FAILED` on root failure.
- **Timeline (Layer 2):** `compile_timeline`, `timeline_step`.
- **Selection toolkit:** `all_of_type`, `min_by`/`max_by`, `count_where`, `nearest_where`/`farthest_where`, `has_component`, `kind`.
- **View / cutscenes:** `set_camera(name)` — cut the scene viewport to a `def Camera` by name (leaf or full USD path); pairs with a timeline for cutscene camera changes. `possess(vessel)`, `notify(msg)`, `photo()` (capture from the active camera).
- **Patrol / checkpoints** ([`patrol.rhai`](../assets/scripting/prelude/patrol.rhai)): `engage_patrol(vessel, points, speed?, radius?, dwell?)`, `patrol(vessel, points, …)` (hot-swap an engaged vessel's route), `add_checkpoint(vessel, x, y, z)`, `clear_patrol(vessel)`. Each waypoint may be a bare `[x,y,z]` or a `#{pos, dwell?, on_arrival?}` map carrying arrival actions — the declarative way to "fire a tool at a waypoint" (no tree composition). `clear_patrol` fires the `ClearPatrol` typed command (the canonical stop-&-clear verb).
- **Science instruments** ([`science.rhai`](../assets/scripting/prelude/science.rhai)): `photo_from(vessel)` (capture from a vessel's mounted camera — fires `CaptureFromCamera`), `take_photo()` / `take_photo(args)` (a `run_tool` action value for a waypoint's `on_arrival` list, naming the registered `science::take_photo` tool). The Rust core owns firing & cleaning via the `lunco-tools` registry + `lunco-tools-bevy` dispatch; these helpers just NAME the tool from data.
- **Tutorial HUD** ([`hud.rhai`](../assets/scripting/prelude/hud.rhai)): `hint(msg)`/`clear_hint()` (sticky instruction), `spotlight(anchor, caption)`/`clear_spotlight()` (dim + ring a workbench widget by `HelpAnchors` key), `objectives_hud(list)` (or just declare a `mission(me)` — it auto-publishes), `coach_step(steps, i)` (a guided coach-mark tour step; advance the cursor in `on_event`). This is how tutorials are authored — a tutorial is just a scenario. See [`tutorials/README.md`](../assets/tutorials/README.md).

Add helpers freely — the prelude is loaded **from disk at startup** on native
(`assets/scripting/prelude/*.rhai`): edit a helper, restart the app, no rebuild.
The compiled-in copy is the fallback (missing directory, or a disk file that
fails to parse — the app logs the error and boots on the embedded prelude
rather than bricking) and the source of truth on wasm, so a rebuild still
refreshes it for installed/web builds.

## C. Scenario parameters

Reuse one source across entities/missions by passing a JSON object string; the
script reads it as the read-only `params` constant:

```jsonc
RunScenario { target: <gid>, source: "...", params: "{\"speed\":1.5}" }
```
```rhai
fn on_tick(me) { drive(me, params.speed, 0.0); }
```

## D. Sequencing (missions)

Two layers, both pure rhai (no engine rebuild):

- **Layer 1 — imperative steps** ([`sequence.rhai`](../assets/scripting/examples/sequence.rhai)): build a step array with `step`/`once`/`wait`/`wait_until`/`wait_for` and run it with `run_steps`; feed events via `seq_note_event` in `on_event`.
- **Layer 2 — declarative timeline** ([`timeline.rhai`](../assets/scripting/examples/timeline.rhai)): a mission as **pure data** (an array of `{move_to|cmd|emit|wait|wait_event}` steps), lowered onto Layer 1 by `compile_timeline`. Because it's data, a timeline is serialisable — run one inline with `RunTimeline`, or store it (see [§I](#i-persistence)).

Progress is observable on the telemetry bus: `STEP_COMPLETE(idx)` per step,
`SEQUENCE_COMPLETE(len)` at the end (and `OBJECTIVE_COMPLETE`/`PLAN_COMPLETE` for `run_plan`).

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

- **Per-entity scenarios → USD (load):** a script is a `LunCoProgram` child prim, and it
  auto-attaches and runs when the prim is spawned:
  - `uniform asset lunco:program:sourceAsset = @scenarios/foo.rhai@` — the file, resolved
    like every other asset the scene depends on.
  - `uniform string lunco:program:sourceCode = '''<rhai>'''` — the source authored in place
    in the USD layer. An edit to it is an ordinary attribute edit, so it journals, undoes
    and replicates like any other.
  - `custom float lunco:param:<key> = <v>` — one typed attribute per per-instance setting,
    read in-script by `param(me, "<key>", default)`.
- **Tool libraries → files:** `<twin>/tools/*.rhai` (see [§E](#e-tools-shared-libraries)).
- **Timelines → files:** `RegisterTimeline { name, timeline }` stores to `<twin>/timelines/<name>.json`; reloaded on Twin open. Discover with `ListTimelines`/`GetTimeline`; run a stored one with `RunStoredTimeline { target, name }`.
- **Port-threshold events → USD:** author one `def LunCoPortEvent` child prim per rule on
  the program whose port it watches, and a telemetry event fires automatically when the
  port crosses the threshold. Each rule is four typed properties: `lunco:event:port`
  (token), `lunco:event:op` (token — `lt` \| `le` \| `gt` \| `ge`), `lunco:event:threshold`
  (double), `lunco:event:emit` (token). Example — the lander's `LowFuel` prim watches
  `m_prop` with `lt` 200 and emits `lander_low_fuel`; its `Depleted` prim watches the same
  port with `le` 0.5 and emits `lander_depleted`. Scripts receive these via `on_event`.

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
fn on_tick(me) {
    print("Rover " + name(me) + " position: " + world_pos(me));
}
```

### Inspecting Script Status
When a script fails to compile or crashes at runtime, the engine exposes detailed error logs (including file origin, line, and column numbers). You can retrieve this diagnostic information via the `ScriptStatus` API query:
```json
// Query
{"command": "ScriptStatus", "params": {"target": 1234}}

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
{"command": "ScriptInspect", "params": {"target": 1234}}

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
| HTTP API | `{"command":"RunScenario","params":{"target":<gid>,"source":"<rhai>"}}` |
| MCP | the `run_scenario` tool (`mcp/src/index.js`) |
| One-shot eval | `RunRhai { code }` — runs once with full world access; stdout via `QueryCommandResult` |
| Control | `SetScenarioPaused { target, paused }`, `StopScenario { target }` |

## N. Examples index

| File | Shows |
|---|---|
| [`patrol.rhai`](../assets/scripting/examples/patrol.rhai) | a looping waypoint patrol |
| [`mission.rhai`](../assets/scripting/examples/mission.rhai) | event-channel coordination between scripts |
| [`mission_plan.rhai`](../assets/scripting/examples/mission_plan.rhai) | a declarative waypoint plan via `run_plan` |
| [`sequence.rhai`](../assets/scripting/examples/sequence.rhai) | the Layer-1 step sequencer |
| [`timeline.rhai`](../assets/scripting/examples/timeline.rhai) | a Layer-2 mission as data |
| [`avoid.rhai`](../assets/scripting/examples/avoid.rhai) | sensing + obstacle avoidance |
| [`tools/formation.rhai`](../assets/scripting/tools/formation.rhai) | a tool library (formation flying) |
| [`tools/survey.rhai`](../assets/scripting/tools/survey.rhai) | a custom tool library (survey pattern) |

## Links

- [Command reference](./commands-reference.md) — every `#[Command]`, auto-generated
- [lunco-scripting crate README](../crates/lunco-scripting/README.md)
- [Rhai integration design & as-built reference](./rhai-integration-design.md)
- [prelude/](../assets/scripting/prelude) — the helper library (one file per topic)
- [Examples directory](../assets/scripting/examples)
- [Crate index](./crates-index.md)
- [rhai language reference](https://rhai.rs/book/)
