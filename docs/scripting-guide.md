# LunCo Scripting Guide

How to write **scenarios** — persistent per-entity programs that sense and drive
the simulation — in LunCoSim.

- **Crate:** [`lunco-scripting`](../crates/lunco-scripting) · **Design rationale:** [rhai-integration-design.md](./rhai-integration-design.md)
- **Examples:** [`assets/scripting/examples/`](../assets/scripting/examples) · **Helper library:** [`assets/scripting/prelude/`](../assets/scripting/prelude)

---

## 1. Mental model

The default scripting language is **rhai** — a small, sandboxed, pure-Rust
language that runs everywhere the sim does, including the browser (wasm). (Python
exists for one-shot eval via `RunPython`; a full Python scenario lifecycle is
planned but not yet implemented. Lua is a reserved language id, not implemented.)

A **scenario** is a rhai program attached to an entity. It runs every fixed
simulation tick with lifecycle hooks — it is *not* a one-shot snippet. The split:

> **The host (Rust) is mechanism; the script is policy.** Navigation, objectives,
> behaviour trees, sequencing — all live in hot-reloadable `.rhai`, never compiled
> into the engine.

A script touches the world through exactly the same **command/query API** the
HTTP API, MCP, and UI use — so it inherits every command for free and stays
decoupled from physics. Scripts are **host-authoritative** (see [§10](#10-networking--determinism)).

## 2. Lifecycle hooks

Define any subset. The first parameter (`me` by convention) is the host entity's
id; per-tick mutable state lives on the implicit `this` object map.

```rhai
fn on_start(me)      { this.count = 0; }                // once, after (re)compile
fn on_tick(me)       { this.count += 1; }               // every FixedUpdate tick
fn on_event(me, evt) { if evt.name == "GO" { /* … */ } }// a TelemetryEvent arrived
fn on_stop(me)       { brake(me); }                     // teardown: hot-reload / detach / despawn
```

- **Hot-reload:** re-running `RunScenario` on the same entity recompiles in place
  (state resets, `on_stop` of the outgoing program runs first).
- **`on_stop`** fires on hot-reload swap, `StopScenario`, or despawn — stop
  actuators / release claims here.
- **`this`** persists across ticks for one entity; rhai functions are otherwise
  pure (they can't see top-level `let`s), so thread state through `this`.

## 3. The verb surface

The host exposes a minimal, generic bridge. Everything else is prelude policy.

| Verb | Returns | Purpose |
|---|---|---|
| `cmd(name, #{params})` | `#{ id, ok, data, error }` | **WRITE** — fire any `#[Command]` by name (synchronous; `data` carries assigned values like a spawned gid) |
| `query(name, #{params})` | value \| `()` | **READ** — call any query provider (Raycast, Nearest, GroundHeight, …) |
| `get(id, "Comp.field")` | value \| `()` | reflected component **read** (vectors → `[x,y,z]`, quats → `[x,y,z,w]`, structs → maps) |
| `set(id, "Comp.field", value)` | bool | reflected component **write** — the mirror of `get`; coerces by field type (int→float, `[x,y,z]`→`Vec3`); `false` on bad path/type |
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

JSON appears **only** at the `cmd`/`query` params seam (that's the API's own
contract). Both directions are native: `get`/`get_setting` build rhai values
straight from reflect, and `set`/`set_setting` write rhai values straight back —
no JSON round-trip on the read or write path.

> **`set` vs `cmd`.** Use `set`/`set_setting` to tune a *value* (a field, a
> config knob) — it's a direct reflected write, host-authoritative because
> scenarios run host-only, and the change replicates through normal component
> sync. Use `cmd` for an *operation* with side effects beyond a field write
> (spawning, swapping a material, anything an observer must react to). Settings
> are only reachable if their type is `register_type`'d with
> `#[reflect(Component)]` / `#[reflect(Resource)]`.

## 4. Prelude helpers

The [`prelude/`](../assets/scripting/prelude) directory (one `.rhai` per topic —
`nav`, `sensing`, `control`, `tasks`, `mission`, `hud`, …) is the hot-reloadable
helper library on top of the verbs — read the topic files for the full,
authoritative list. Highlights:

- **Vector math:** `vsub`/`vadd`/`vlen`/`vdot`/`vcross`/`vnorm`/`vscale`/`clamp`, `distance`, `arrived`.
- **Navigation:** `drive(rover, fwd, steer)`, `brake(rover)`, `steer_to`, `nav_to(entity, target, speed, radius)`, `run_plan`.
- **Sensing:** `velocity`/`speed`, `raycast`, `obstacle_ahead`, `ground_height`, `nearest`, `entities_in_radius`.
- **Collision events:** `collision_pair`/`collision_other`/`entered`/`exited` (parse `COLLISION_START`/`COLLISION_END`).
- **Sequencer (Layer 1):** `seq_init`, `run_steps`, `seq_note_event`, step ctors `step`/`once`/`wait`/`wait_until`/`wait_for`.
- **Timeline (Layer 2):** `compile_timeline`, `timeline_step`.
- **Selection toolkit:** `all_of_type`, `min_by`/`max_by`, `count_where`, `nearest_where`/`farthest_where`, `has_component`, `kind`.
- **View / cutscenes:** `set_camera(name)` — cut the scene viewport to a `def Camera` by name (leaf or full USD path); pairs with a timeline for cutscene camera changes. `possess(vessel)`, `notify(msg)`.
- **Tutorial HUD** ([`hud.rhai`](../assets/scripting/prelude/hud.rhai)): `hint(msg)`/`clear_hint()` (sticky instruction), `spotlight(anchor, caption)`/`clear_spotlight()` (dim + ring a workbench widget by `HelpAnchors` key), `objectives_hud(list)` (or just declare a `mission(me)` — it auto-publishes), `coach_step(steps, i)` (a guided coach-mark tour step; advance the cursor in `on_event`). This is how tutorials are authored — a tutorial is just a scenario. See [`tutorials/README.md`](../assets/tutorials/README.md).

Add helpers freely — editing the prelude needs no Rust rebuild.

## 5. Scenario parameters

Reuse one source across entities/missions by passing a JSON object string; the
script reads it as the read-only `params` constant:

```jsonc
RunScenario { target: <gid>, source: "...", params: "{\"speed\":1.5}" }
```
```rhai
fn on_tick(me) { drive(me, params.speed, 0.0); }
```

## 6. Sequencing (missions)

Two layers, both pure rhai (no engine rebuild):

- **Layer 1 — imperative steps** ([`sequence.rhai`](../assets/scripting/examples/sequence.rhai)): build a step array with `step`/`once`/`wait`/`wait_until`/`wait_for` and run it with `run_steps`; feed events via `seq_note_event` in `on_event`.
- **Layer 2 — declarative timeline** ([`timeline.rhai`](../assets/scripting/examples/timeline.rhai)): a mission as **pure data** (an array of `{move_to|cmd|emit|wait|wait_event}` steps), lowered onto Layer 1 by `compile_timeline`. Because it's data, a timeline is serialisable — run one inline with `RunTimeline`, or store it (next section).

Progress is observable on the telemetry bus: `STEP_COMPLETE(idx)` per step,
`SEQUENCE_COMPLETE(len)` at the end (and `OBJECTIVE_COMPLETE`/`PLAN_COMPLETE` for `run_plan`).

## 7. Tools (shared libraries)

A **tool library** is a named bundle of reusable policy, callable as
`libname::fn(...)` from any hook (no `import` — they bind as static modules).

- Author one: drop a `.rhai` in [`rhai/tools/`](../assets/scripting/tools), or `RegisterToolLibrary { name, source }` at runtime (hot-reloadable).
- Examples: [`formation.rhai`](../assets/scripting/tools/formation.rhai) (formation flying), [`survey.rhai`](../assets/scripting/tools/survey.rhai) (lawnmower survey pattern).
- Discover: `ListToolLibraries`, `GetToolLibrary { name }`.
- **Persistence:** registered libraries are mirrored to `<twin>/tools/*.rhai` and reloaded when the Twin opens.

## 7a. Policy hooks (decision functions)

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

## 7b. Vessel controllers & control authority

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

## 8. Persistence

- **Per-entity scenarios → USD (load):** author `custom string lunco:script = '''<rhai>'''` on a prim; on spawn it auto-attaches and runs. *(Writing a live-edited scenario back onto its prim is not yet supported — it needs a USD asset↔document bridge.)*
- **Tool libraries → files:** `<twin>/tools/*.rhai` (see [§7](#7-tools-shared-libraries)).
- **Timelines → files:** `RegisterTimeline { name, timeline }` stores to `<twin>/timelines/<name>.json`; reloaded on Twin open. Discover with `ListTimelines`/`GetTimeline`; run a stored one with `RunStoredTimeline { target, name }`.

## 9. Introspection & discovery

| Query | Answers |
|---|---|
| `ScriptStatus { target }` | *Is it healthy?* — compile/runtime diagnostics (state, ok, located errors) |
| `ScriptInspect { target }` | *What is it doing?* — live `this` state, defined hooks, generation, paused/running, plus the status block |
| `ScriptingCatalog` | the full callable surface in one doc: `verbs`, `hooks`, `prelude`, `tools`, `commands`, `queries` — the authoring/discovery source of truth |

## 10. Networking & determinism

Scenarios are **host-authoritative**: they run on the `Host` and in single-player
(`Standalone`), but **not** on a networked `Client`. A client receives scripted
behaviour via replication of the resulting entity state — it does not re-run the
script (which would double-fire `cmd()`/`emit()` and diverge the per-entity
`this`). For deterministic behaviour scripts read the fixed clock (`dt`,
`sim_tick`, `elapsed_seconds`) and `rand()` is deliberately not exposed.

## 11. Running a scenario

| Transport | How |
|---|---|
| HTTP API | `{"command":"RunScenario","params":{"target":<gid>,"source":"<rhai>"}}` |
| MCP | the `run_scenario` tool (`mcp/src/index.js`) |
| One-shot eval | `RunRhai { code }` — runs once with full world access; stdout via `QueryCommandResult` |
| Control | `SetScenarioPaused { target, paused }`, `StopScenario { target }` |

## 12. Examples index

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

- [lunco-scripting crate README](../crates/lunco-scripting/README.md)
- [Rhai integration design & as-built reference](./rhai-integration-design.md)
- [prelude/](../assets/scripting/prelude) — the helper library (one file per topic)
- [Examples directory](../assets/scripting/examples)
- [Crate index](./crates-index.md)
- [rhai language reference](https://rhai.rs/book/)
