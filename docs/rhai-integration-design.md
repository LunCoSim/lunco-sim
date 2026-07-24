# Rhai Integration Design — scripting & scenarios

Rhai drives scenarios — *"rover moves along a path via checkpoints, loads next
goals"* — and, more broadly, **manipulates every object in the sim (Twin, USD,
Modelica, cosim, scene, vehicles) from script.** The engine builds on native
(default), `--no-default-features` (script-free), `python`, and
`wasm32-unknown-unknown`.

> **Authoring a scenario?** Read the **[Scripting Guide](./scripting-guide.md)** —
> a task-oriented how-to. This document is the architecture + design rationale.

### Capabilities

- **Scenario parameters** — `RunScenario { …, params }` (JSON object string) →
  read in-script as the `params` constant; one source serves many entities.
- **Lifecycle** — `on_stop` teardown hook (hot-reload / detach / despawn) +
  `SetScenarioPaused` / `StopScenario`. The lifecycle lives in a **language-neutral
  driver** (`scenario.rs`, `ScenarioRuntime` trait) over a **native world bridge**
  (`bridge_core.rs`, `ValueBuilder` — no JSON on the read path); rhai is one
  backend, Python can implement the same traits.
- **Introspection** — `ScriptStatus` (compile/runtime health) + `ScriptInspect`
  (live `this` state, defined hooks, generation, running/paused).
- **Authoring catalog** — `ScriptingCatalog` aggregates the full callable surface
  (verbs/hooks/prelude/tools/commands/queries).
- **Timeline storage** — `RegisterTimeline` / `RunStoredTimeline` +
  `ListTimelines` / `GetTimeline`, persisted to `<twin>/timelines/*.json`.
- **USD-embedded scenarios (load)** — a `LunCoProgramAPI` child prim naming a `.rhai`
  (`info:sourceAsset`, or `info:sourceCode` authored in place)
  auto-attaches + runs on spawn.
- **Host-authoritative gate** — script systems run on Host / Standalone, never on
  a networked Client (which receives behaviour via replication).

Python scenarios run via `PythonScenarioRuntime` implementing the same
`ScenarioRuntime` trait — not the legacy `inputs`/`outputs` dict path.

---

## Running scenarios

**Principle:** core = mechanism, rhai = ALL policy (objectives, navigation,
behavior trees, sequencing live in hot-reloadable `.rhai`, never compiled in).

### How to load & run a scenario

A scenario is a `.rhai` program with lifecycle hooks. Attach it to any entity:

- **API / MCP / scripts:** the `RunScenario { target, source }` command
  (`crates/lunco-scripting/src/commands.rs`). MCP tool: **`run_scenario`**
  (`mcp/src/index.js`). HTTP: `{"command":"RunScenario","params":{"target":<gid>,"source":"<rhai>"}}`.
  Idempotent + **hot-reload**: re-running on the same entity recompiles in place
  (bumps `ScriptDocument.generation`).
- **One-shot eval (no attach):** the `RunRhai { code }` command — runs once with
  full World access; stdout returned via `QueryCommandResult`.
- **Direct (code/tests):** insert a `ScriptDocument` into `ScriptRegistry` +
  attach `ScriptedModel { language: Rhai, document_id }`.

### Lifecycle hooks (per-entity runtime, `world_bridge.rs` `tick_rhai_models`)

```rhai
fn on_start(me) { ... }        // once after (re)compile; `me` = host entity gid
fn on_tick(me)  { ... }        // every FixedUpdate
fn on_event(me, evt) { ... }   // evt = #{name, value, severity, timestamp}; frame-delayed
```

State rule (rhai-specific, important): script `fn`s are **pure** — they cannot
see top-level `let`s; only `const` globals are visible. Persistent per-tick state
lives on **`this`** (a per-entity object map: `this.idx = 0`). `this` is bound
ONLY in the hook the engine calls directly — **NOT** in helper functions it
calls, so prelude/library helpers must be stateless (take+return state).

### Host verbs (the entire Rust-exposed vocabulary — `world_bridge.rs`)

| verb | channel | purpose |
|------|---------|---------|
| `cmd(name, #{params})` | write | fire ANY registered `#[Command]` by name (reflect dispatch via `ApiCommandEvent`); behind networking RBAC; host-authoritative |
| `world_pos(id)` → `[x,y,z]` | read | float-origin-correct world position |
| `world_forward(id)` → `[x,y,z]` | read | world heading (only read rhai can't derive itself) |
| `get(id, "Comp.field")` | read | generic reflected component-field read |
| `find(name)` / `list_entities()` | read | entity lookup by `Name` / enumerate |
| `sim_tick()` | read | current FixedUpdate tick |
| `emit(name, value)` | event | fire a `TelemetryEvent` on the shared bus |

Everything else is **policy in rhai** — see the prelude
`assets/scripting/prelude/` (one file per topic): vector math, `distance`/`arrived`,
`steer_to`/`nav_to` (closed-loop steering), `run_plan` (declarative waypoint/
objective executor), `drive`/`brake`/`load_scene` wrappers. The prelude is loaded
FROM DISK at startup on native (edit → restart, no rebuild), with the
`include_dir!`-embedded copy as the fallback and the wasm source of truth
(wasm-safe, no IO). A disk file that fails to parse logs and falls back to the
embedded prelude, so a broken edit can't brick startup.
NB: `goto` is a reserved word in rhai — the nav helper is `nav_to`.

### Events / pub-sub

`emit()` reuses the **`TelemetryEvent`** bus (observer-dispatched; YAMCS
mnemonic in `name`) — no new event type. External clients receive script events
via `SubscribeTelemetry` (`lunco-api` `executor.rs` + `subscription.rs`). Scripts
receive events via `on_event` (frame-delayed: emit on
tick N → deliver tick N+1 → deterministic actor model). Inter-script interaction
is bus-only (isolated VMs); see §7f.

### Examples

`assets/scripting/examples/`: `patrol.rhai` (waypoint loop, emits
checkpoints), `mission.rhai` (coordinator reacting via `on_event`),
`mission_plan.rhai` (declarative `run_plan` mission).

### Build notes / gotchas

- rhai is a **default-on optional feature** (`default = ["rhai"]`); removable for
  a script-free build.
- `lunco-api` dep MUST be `default-features = false` (its default `transport-http`
  pulls tokio→mio and breaks wasm).
- wasm needs `--cfg getrandom_backend="wasm_js"` (set by `build_web.sh`).
- A `Result`-returning `#[on_command]` records to `CommandResults` — that resource
  must exist.

### Deferred (design-only / separate scope)

- ROS2 bridge (needs an `rclrs` transport crate) — seam ready, see §7d.
- Inspector/editor params UI (exposing `ScriptedModel` + doc source).
- Avian sensor-volume checkpoint auto-emit (rhai `arrived()` polling already covers it).

---

## 0. The key realization — the command bus is already the universal surface

The system already has a single, uniform manipulation API: the typed
`#[Command]` bus. ~90 commands span every subsystem, all are `#[reflect(Event)]`
(auto-discoverable), and **dispatch-by-name already exists** — `api_command_dispatcher`
(`crates/lunco-api/src/executor.rs:90-162`) deserializes JSON params into a
reflected struct and fires it with `ReflectEvent::trigger(world, &dyn Reflect, &type_reg)`.
HTTP and MCP are just two callers of this path.

**Therefore "manipulate everything from rhai" ≠ 90 bindings. It = ONE generic
bridge** (`cmd()` / `query()`) that reuses the reflect-dispatch path. rhai becomes
a *third transport*. Every existing command — and every future one — is reachable
for free, with the same RBAC/authz gate the API already enforces.

Representative commands already covering the user's surface:

| Subsystem | Commands (file:line) |
|---|---|
| Rover/vehicle | `SetPorts` — writes named input ports (`throttle`/`steer`/`brake`); `DriveMix` allocates them to actuators (`lunco-cosim/src/lib.rs`, `lunco-mobility::apply_drive_mix`) |
| Camera/control | `PossessVessel`, `ReleaseVessel`, `FocusTarget`, `FollowTarget` (`lunco-avatar/src/commands.rs`) |
| Scene/USD | `LoadScene`, `ClearScene` (`lunco-usd-sim/src/cosim.rs:814,884`) |
| Scene editing | `SpawnEntity`, `MoveEntity`, `SetObjectProperty`, `SelectEntity` (`lunco-sandbox-edit/src/commands.rs`) |
| Modelica/cosim | `CompileActiveModel`, `SetModelInput`, run/step commands (`lunco-modelica/...`) |
| Celestial | `TeleportToSurface`, `LeaveSurface` (`lunco-celestial/src/commands.rs`) |
| Scripting | `RunRhai`, `RunPython` (`lunco-scripting/src/commands.rs`) |
| Queries (return data) | `QueryEntity`, `ListEntities`, `DiscoverSchema`, `QueryCommandResult` (`lunco-api/src/executor.rs` — `ApiRequest` variants, not commands) |

---

## 1. The capability surface (grounded)

The pieces that make "manipulate everything from rhai" work, and where each lives:

| Capability | Evidence |
|---|---|
| Universal command bus | ~90 `#[Command]`, `lunco-command-macro` |
| Dispatch-by-name (reflect) | `executor.rs:90-162` (`ReflectEvent::trigger`) |
| RBAC/authz on commands | `#[authz_target]`, `SessionRegistry::may_possess`, sender-identity binding |
| Stable entity ids | `GlobalEntityId(u64)` (`lunco-core/src/lib.rs:121`), `ApiEntityRegistry::resolve` |
| Scene/Twin/Modelica/cosim verbs | LoadScene/Spawn/SetObjectProperty/Compile/... |
| Sandboxed rhai engine | `RhaiBackend` op/depth/size caps (`backend.rs:40-77`) |
| rhai → World access | `ScenarioRuntime` exposes host functions to rhai engine |
| Persistent script state across ticks | `this` map persisted on scenario entity across ticks |
| Temporal sequencing (wait/over-time) | Task-tree constructors in `prelude/tasks.rhai` (pure data), ticked NATIVELY on the `lunco-behavior` kernel (`lunco-scripting/src/task_tree.rs`) |
| Navigation: waypoints/goals/arrival/path-follow | `nav_to`, `drive`, `run_plan` in `prelude/nav.rhai` |
| By-name entity lookup | `find(name)` verb |
| Timer "after N seconds" | `wait(secs)` / `wait_until(cond)` in sequencer |
| Telemetry subscribe (events to script) | `on_event` hook, `seq_note_event` delivery |

---

## 2. Architecture — two layers

```
┌─────────────────────────────────────────────────────────────┐
│ Layer B — Scenario Runtime (temporal: checkpoints, goals)    │
│   persistent per-scenario rhai (AST+Scope), host lifecycle   │
│   hooks: on_start / on_tick / on_event                       │
├─────────────────────────────────────────────────────────────┤
│ Layer A — Universal Bridge (manipulate everything, one-shot) │
│   cmd(name, #{params})  query(name, #{params})  find(name)   │
│   → ReflectEvent::trigger / ApiRequest, behind RBAC          │
├─────────────────────────────────────────────────────────────┤
│ rhai::Engine (sandboxed) + World access + native primitives  │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. Layer A — the World bridge (manipulate everything)

### 3.1 Giving rhai access to the World
`ScriptBackend::eval(&self, code)` has no World. Run scenario/command scripts in
an **exclusive system** (`&mut World`) and expose a scoped World pointer to host
functions for the eval duration — the standard bevy-scripting pattern
(`bevy_mod_scripting`). Reads run synchronously; writes mirror
`executor.rs:134-161` (build reflected event, `ReflectEvent::trigger`).

> Keep the existing pure `RhaiBackend` for the trivial `RunRhai{code}` stdout
> case. Add a *new* world-bound execution context for scenarios/commands. Don't
> overload `eval`.

### 3.2 Exposed verbs (the entire vocabulary, ~6 functions)
```rust
cmd(name: &str, params: Map) -> Dynamic   // dispatch ANY command by name (reflect)
query(name: &str, params: Map) -> Dynamic // ApiRequest queries (QueryEntity, ...)
find(name: &str) -> i64                    // Name -> GlobalEntityId (sugar over ListEntities)
entity(id) -> EntityHandle                 // position/rotation/components accessor
sim_time() -> f64                          // SimTick * SECS_PER_TICK
log(msg)                                    // already have print()
```
That is the *whole* Rust-side surface. `cmd()` reaches all ~90 commands +
every future command with no new glue. Twin/USD/Modelica/cosim are all just
command names.

### 3.3 Ergonomics live in a rhai *prelude*, not Rust
Ship a standard `prelude.rhai` (script, not Rust) wrapping raw `cmd()` into
friendly verbs — so authoring stays nice without per-command Rust code:
```rhai
fn drive(r, fwd, steer) { cmd("SetPorts", #{ target: r, writes: [["throttle", fwd], ["steer", steer]] }); }
fn possess(r)           { cmd("PossessVessel", #{ target: r }); }
fn load(path)           { cmd("LoadScene", #{ path: path, root_prim: "" }); }
fn set_prop(id, k, v)   { cmd("SetObjectProperty", #{ target: id, key: k, value: v }); }
```

### 3.4 Security (must-have)
`cmd()` MUST pass through the same authz/RBAC gate as the API
(`#[authz_target]`, `SessionRegistry`, sender identity). A shared/untrusted
scenario script then can't exceed its owner's authority. The sandbox caps
(ops/depth/size) already bound runaway scripts. The exposed verb set = the
entire capability surface — nothing reachable that isn't a vetted command.

---

## 4. Layer B — scenario runtime (the checkpoints/goals problem)

### 4.1 Why a plain script won't work
rhai is synchronous with **no async/await** (Rune has it; rhai doesn't), there is
**no coroutine/yield/wait**, and `SetPorts` carries no persistent setpoint — it
must be re-emitted every tick. So *"drive to checkpoint, wait until arrived, then
next goal"* cannot be a blocking script. Two valid models:

**(A) Declarative plan** — script runs ONCE, returns a mission (data); a native
runtime executes it over time. Same op-graph/recipe shape used elsewhere.
```rhai
fn mission() {
  [ goto(WP1), wait_arrive(2.0), goto(WP2), dwell(10.0), goto(BASE) ]
}
```
Fast, deterministic, trivially serializable/replicated. Best for fixed routes.

**(B) Event/tick callbacks** — a **persistent per-scenario rhai instance**
(compiled `AST` + `Scope` that survive across ticks) stored on a
`ScenarioRuntime` component. The host calls rhai functions when things happen:
```rhai
let goals = [WP1, WP2, BASE];
let i = 0;
fn on_tick(ctx) {
  let r = ctx.rover;
  if arrived(r, goals[i], 2.0) { i += 1; if i >= goals.len() { return done(); } }
  steer_toward(r, goals[i]);           // emits SetPorts(throttle/steer) this tick
}
fn on_event(name, data) { if name == "obstacle" { /* replan */ } }
```
One cheap `call_fn` per scenario per tick (sparse — not per-vertex), so interpreter
cost is negligible. Best for conditional/reactive logic. **This is the KSP-grade path.**

Recommend supporting **both**: declarative for routes, callbacks for logic. They
compose — a declarative plan can contain script-condition steps.

### 4.2 Required upgrade over today's runtime
Both current paths are one-shot/recompile-every-tick. Layer B needs
**compile-once + persistent Scope**:
- `Engine::compile(src) -> AST` once (on doc load / hot-reload).
- `ScenarioRuntime { ast, scope }` component; `engine.call_fn(&mut scope, &ast, "on_tick", (ctx,))` each FixedUpdate.
- Hot-reload = recompile AST on `ScriptOp::SetSource` (reuse `ScriptDocument` +
  `DocumentHost` versioning already in `doc.rs`).

---

## 5. Navigation primitives

`SetPorts` is the only actuator (writes `throttle`/`steer` inputs → `DriveMix` →
port propagation → wheel physics); everything goal-shaped builds on it. The native set
(registered as rhai verbs), all deterministic, emitting `SetPorts` each tick:

```rust
distance(a, b) -> f64                 // world_vector(a,b).length()  (coords.rs:109)
heading_error(rover, target) -> f64   // chassis forward vs vector-to-target
arrived(rover, pos, tol) -> bool      // distance < tol
steer_toward(rover, target)           // P-controller: heading->steer, dist->throttle, emit SetPorts
```
`world_position`/`world_vector` already exist (`lunco-core/src/coords.rs:63,109`)
and handle the floating-origin (big_space) correctly — use them, don't read raw
`Transform`.

A native `PathFollower { waypoints, index, tol }` component can execute the
declarative plan (model A) entirely in Rust at native speed; the script just
authors the waypoint list.

---

## 6. Determinism & networking

Run scenarios **host-authoritative** (server/owner): the scenario emits
`SetPorts`/etc., which already replicate via the `CommandBus` `SyncChannel` and
client prediction (`AppliedInputSeq`, `OwnedInputLog`). This avoids divergence —
clients don't run scenario logic, they receive its command stream. `rand()` uses
deterministic per-hook seeding (`(entity, tick, hook)` triple) — a re-run at the
same tick produces the same sequence. This matches the existing determinism
discipline (port propagation, steering, cosim).

---

## 7. Implementation structure

The system is organized into four layers, each building on the one below:

1. **World bridge + `cmd()`/`query()`/`find()`** — exclusive-system context,
   reflect-dispatch, RBAC gate; `prelude.rhai` provides the core verb table.
2. **Persistent scenario runtime** — `ScenarioRuntime` AST+Scope,
   `on_start`/`on_tick`/`on_event`, hot-reload via `ScriptDocument`.
3. **Navigation primitives** — `distance`/`arrived`/`steer_toward` +
   `PathFollower`; the checkpoint/goal scenario runs end to end.
4. **Authoring polish** — declarative-plan executor, scenario examples,
   editor/Inspector params, telemetry→`on_event` wiring.

---

## 7b. Is the command bus enough? — No: two-channel model

The command bus is the right channel for **writes that must be authoritative,
replicated, RBAC-gated, undoable, and audited**. It is the wrong channel for
**reads** and **fine-grained state** — which a per-tick `on_tick` callback needs
constantly. Evidence: commands return via async `QueryCommandResult` polling
(`executor.rs:587`); `QueryEntity` returns only a fixed blob
(pos/rot/name/type, `executor.rs:535`) — no arbitrary component fields, no
cosim/Modelica values; reflect-dispatch JSON-(de)serializes per call. And the
intended read bridge (`python/reflect.rs` `EntityProxy`) is a **stub** — it
touches no ECS (`reflect.rs:28-37`).

So tighter integration IS needed, as a **second, complementary channel**:

| | Channel 1 — Commands (write/action) | Channel 2 — Reflection bridge (data plane) |
|---|---|---|
| Direction | writes | reads (+ scoped local writes) |
| Mechanism | `cmd()` → `ReflectEvent::trigger` | `AppTypeRegistry` + `ReflectComponent` get/set |
| Use for | SetPorts, LoadScene, Spawn, SetObjectProperty — anything authoritative/replicated/undoable | position, heading, sensors, cosim/Modelica vars, arbitrary `#[reflect]` fields, entity iteration |
| Latency | async (poll result) | **synchronous** during eval |
| Replicated? | yes (CommandBus SyncChannel) | no (local read) |
| Cost | JSON+reflect+observer per call | direct reflected field access (no JSON) |

**Both run inside the same World-bound exclusive-system context** (§3.1). The
reflection bridge is exactly the unfinished `EntityProxy` — finish it properly
against `AppTypeRegistry`/`ReflectComponent` (well-trodden; this is what
`bevy_mod_scripting` does).

**Default rule — reads direct, writes through commands:**
- READ arbitrary state → reflection bridge (fast, synchronous, local). `pos(r)`,
  `entity(r).Battery.level`, `cosim_var(m, "height")`.
- WRITE that must replicate / be authoritative / undoable → `cmd()` (bus). Keeps
  determinism + networking intact: clients receive the authoritative command
  stream, they don't run scenario logic (§6).
- Direct reflected *writes* allowed ONLY for explicitly local/non-replicated
  scratch state (scenario-private vars, editor-only tweaks) — clearly flagged, to
  avoid silently bypassing replication/authz.

This makes "manipulate *everything*" real: Channel 1 = every action verb;
Channel 2 = every readable field. Hot per-tick paths can later get typed
accessors generated from reflection if profiling demands it.

## 7c. Critical review — is this standard, and what are we missing?

**Plumbing: correct, for the right reason.** The two-channel split (reflection
reads + command writes) is unusual vs Unity/Godot/Unreal (which read+write objects
directly), but correct for our *category*: a **deterministic networked sim**
(Factorio / RTS lockstep), where mutations must flow through a replicated ordered
command stream and reads are local. Reads-via-reflection and a lifecycle callback
(`on_tick`) are universally standard.

**Scenario layer: this is where we were under-built.** Leading with "scenario =
imperative rhai + hand-rolled state machine (`i += 1`)" is the low-level version
of what every engine gives authors. Missing, in priority order:

1. **Coroutines / sequencing (#1 gap).** "Do X, wait until Y, then Z" is *the*
   scenario primitive — Unity `yield return WaitUntil`, Godot `await`, Luau
   `task.wait`, Unreal latent actions. rhai has **no async** — this is the real
   cost of picking rhai over Luau. Must be paid back with a Sequencer/BT layer.
2. **Events/signals + trigger volumes.** Checkpoints in real engines are **Avian
   sensor volumes firing on_enter**, not `arrived(pos,tol)` distance polling.
   Need a script-facing event/signal bus (telemetry-subscribe is a stub,
   `executor.rs:584`). Scripting is event-driven first, tick-driven second.
3. **Behavior Trees** for reactive multi-goal behavior (patrol → react → resume) —
   the game-AI standard (Unreal), more composable than an imperative loop.
4. **Declarative objectives (the real "KSP-grade" part).** KSP contracts are a
   declarative objective/condition system (reach X, dwell, plant flag) with
   completion + branching — evaluated, not imperatively scripted. rhai = glue for
   custom conditions. We under-weighted this.
5. **Time-warp coupling.** Scenarios tick in sim-time, respecting `TimeTransport`
   (pause / speed). Ties into the timer/coroutine layer.
6. **Observability/debugging** — inspect/step scenario state, visualize active
   goal/BT node.

**Corrected layering — everything above the core line is rhai, not Rust:**
```
┌─ rhai stdlib (hot-reloadable .rhai, moddable, NOT compiled) ──────────┐
│ Objectives / contracts / missions    conditions, completion, branching│
│ Behavior Trees / Sequencer           coroutine substitute (state in Scope)
│ Navigation (goto/arrived/steer)      pure rhai over world_pos + cmd    │
│ Prelude command wrappers                                               │
└───────────────────────────────────────────────────────────────────────┘
══════════════ CORE BOUNDARY (mechanism only) ══════════════════════════
  Scenario VM (AST+Scope, hot-reload) · on_start/on_tick/on_event
  Ch.1 cmd() → reflect+RBAC · Ch.2 reflection reads · world_pos()
  Event bus (emit + deliver; ROS2 bridge seam) · sim_time() · log
  Events/Triggers from Avian sensors (volumes, not distance polling)
  USD scene/prefab (static authoring)
```
The Sequencer/BT "coroutine substitute" is **rhai stdlib, not core** — advanced
one step per `on_tick`, state in the persistent `Scope`. The core has no
`Objective`/`BehaviorTree`/`Goal` *logic* type; it only ticks hooks and moves
messages. This honors "lean core, policy as data" and keeps Luau's missing
coroutines a non-issue (the substitute lives in script-space).

**Honest caveat:** rhai has no native coroutines (Luau does); we pay it back with
the rhai-stdlib Sequencer/BT — independently the more standard tool for game AI.
Fair trade, but the sequencing layer is **core to the product**, just not core to
the *engine* (it's shipped rhai, hot-reloadable).

## 7d. Core/script boundary (mechanism vs policy) + ROS2

**Directive:** objectives are authored in rhai; behavior trees and
all higher-level constructs are REMOVED from the Rust core; ROS2 integration is
planned. Resulting split:

**Core exposes only (irreducible mechanism):**
- Persistent scenario VM — `rhai::AST` + `Scope` per scenario, recompiled on
  `ScriptDocument` source change (hot-reload).
- Host→script hooks: `on_start()`, `on_tick(ctx)` (sim-time, transport-gated via `TimeTransport`),
  `on_event(evt)`.
- Ch.1 write — `cmd(name, #{…})` → `ReflectEvent::trigger`, behind RBAC.
- Ch.2 read — reflection bridge (`get(entity,"Comp.field")`, `query()`, `list`,
  `find`) + `world_pos(entity)` (float-origin/big_space correct — the ONE nav read
  that must be native).
- Event bus — `emit(name, data)` + delivery of physics/sensor/timer/**external**
  events to `on_event`. This bus is the ROS2 bridge seam.
- `sim_time()`, seeded-RNG-off, `log`.
- A serializable **goal/action envelope** `{id, params, status, feedback, result,
  cancel}` mirroring ROS2 action semantics (the only concession for interop).

**rhai stdlib owns (all policy, shipped as hot-reloadable `.rhai`):** sequencer,
behavior trees, objectives/contracts/missions, navigation helpers, command-wrapper
prelude. The "scenario language" lives here.

**ROS2 alignment — the message model already matches.** `SyncChannel {Local |
CommandBus | ControlStream}` (`core/commands.rs:125`) is explicitly the ROS
Service/Topic trichotomy. Mapping:

| ROS2 | lunco |
|---|---|
| Topic (pub/sub) | event bus / `ControlStream` / telemetry |
| Service (req/resp) | command + `Ack` (poll `QueryCommandResult`) |
| **Action (goal/feedback/result/cancel)** | **scenario objective/goal** |

Constraints to honor NOW so we don't repaint later:
1. Events & commands stay serializable messages — already `reflect + serde`.
2. Goal/objective = serializable action-shaped envelope (above) → a rhai objective
   can be driven by an external ROS2 action client OR exposed as an action server.
3. The event bus is the bridge seam (names ↔ topics); no script-only event model
   that can't bridge.
4. rhai stays ROS-agnostic — a ROS2 goal arrives as `on_event`, rhai pursues it,
   feedback `emit()`ed, the core bridge translates. rhai never imports ROS.

Payoff: a rhai-authored mission is automatically a ROS2 action server — external
robotics nodes can task the sim, and sim scenarios can task real robots — because
the seam is the message bus, not the scenario logic.

## 7e. Simulation events — REUSE TelemetryEvent (do not invent SimEvent)

Directive: introduce a first-class sim event that "fires" and that
scripts react to — but **reuse existing infrastructure, don't reinvent.** It
already exists in `crates/lunco-core/src/telemetry.rs` (XTCE/YAMCS-aligned — bonus
ground-station/ROS interop):

- `TelemetryEvent { name, severity: Severity, data: TelemetryValue, timestamp }`
  (`:57`) — "discrete notification of a system state change." THIS is the sim event.
- `TelemetryValue` (F64/I64/Bool/String, serde) (`:41`) — the payload value.
- `Severity` (YAMCS 5-tier) (`:25`); `SampledParameter` (`:101`) — continuous data;
  `Parameter { name, unit, path }` (`:87`) — reflection-path monitor source for the
  lunco-telemetry sampling engine.
- timestamp = `WorldTime.epoch_jd` TDB epoch (Julian Date) — already the standard (`:14`).
- The docstring even names *"Command Ack"* as an example `TelemetryEvent` — it was
  designed for exactly this notification role.

This gives the third verb with zero new types:

| Verb | Direction | Reused mechanism | ROS2 |
|---|---|---|---|
| read (Ch.2) | pull state | reflection bridge | params/state |
| `cmd()` (Ch.1) | imperative "do this" | reflect command → RBAC, replicated | service / action-request |
| `emit()`/`on_event()` (Ch.3) | "this happened" | **`TelemetryEvent` / `SampledParameter`** | topic / action-feedback |

**Subscription/delivery — finish the existing path, don't add one:**
`ApiRequest::SubscribeTelemetry { filter }` (`schema.rs:35`) + `TelemetryResponse`
(`schema.rs:74`) are already the designed pub/sub; the executor handler is a STUB
returning "Subscription created" (`executor.rs:584`). Implement it to stream
filtered `TelemetryEvent`/`SampledParameter`. That ONE path then feeds *all*
consumers: rhai scenarios (`on_event`), external API/MCP subscribers, the ROS2
topic bridge, and the UI. No second event model.

**rhai verbs are just produce/consume of TelemetryEvent:**
- `emit(name, severity, value)` → fires a `TelemetryEvent` (e.g.
  `commands.trigger`/writer).
- `on_event(e)` ← scenario VM delivers filtered `TelemetryEvent`s (reuses the
  subscribe filter). `subscribe(pattern, handler)` is rhai-stdlib sugar over it.

**Producers fire `TelemetryEvent` (reuse, no new bus):** Avian sensor/collision
bridge → `"TRIGGER_ENTER"` (the checkpoint mechanism — volume, not polling);
lifecycle → `"SCENE_LOADED"` etc.; timers → `"TIMER_FIRED"`. **Threshold events
reuse `Parameter` + the lunco-telemetry sampling engine** (compare
`SampledParameter` to bounds → `TelemetryEvent`) rather than new code; or a rhai
objective polls Ch.2 reads and `emit()`s.

**Subject identity — RESOLVED: YAMCS mnemonic, zero schema change.** Encode the
subject in `name` (`"ROVER.ZHURONG.TRIGGER_ENTER"`, `data = I64(zone_id)`).
`TelemetryEvent` is reused unchanged — matches the mission-control convention
already adopted, and dotted mnemonics map straight to ROS2 topic names. Scripts
filter by mnemonic prefix (`"ROVER.ZHURONG.*"`). Entity-id resolution
(mnemonic ↔ `GlobalEntityId`) happens at the bridge edge, not in the event type.

**Checkpoint loop (reusing TelemetryEvent):**
```rhai
fn on_event(e) {                       // e is a TelemetryEvent
  if e.name == "TRIGGER_ENTER" && e.data == goals[i] {   // (a) mnemonic + zone id
    i += 1;
    if i >= goals.len() { emit("OBJECTIVE_COMPLETE", Severity::Info, rover_id); }
    else { cmd("SetPorts", #{ target: rover_id, writes: [["throttle", 1.0]] }); }
  }
}
```

**Determinism:** TelemetryEvents fire host-authoritative → scenarios react on host
→ emit commands → replicate. Clients get the command stream + a replicated event
subset for UI (`SyncChannel::Local` vs `ControlStream`). No client-side divergence.

## 7f. Script topology — attaching to entities & inter-script interaction

**Attach a script to an entity:** reuse `ScriptedModel` (`doc.rs:100`) — already
the per-entity hook (`document_id`, `language`, `paused`, `inputs`/`outputs`). Set
`language: Rhai` and add a rhai branch to `run_scripted_models` (Python-only
today, `lib.rs:81`). The script's `on_tick(self)` identity IS the host entity.

**Execution model:** ONE shared `rhai::Engine` resource (all host fns registered),
**per-entity `AST` + persistent `Scope`** (compiled once, hot-reloaded on source
change). Fixes today's "fresh Engine per eval" cost. The same `ScriptDocument`
reused on many entities = **prefab scripts** — 10 rovers run `patrol.rhai`, each
with its own `Scope` (independent goal index/state).

**Two roles, both just `ScriptedModel`s:** entity-script (autonomy, ~Unity
MonoBehaviour) and scenario-script (orchestration, on a scenario/singleton entity).

**Inter-script interaction — through the World, never directly.** Each script is an
**isolated VM (own AST+Scope); scripts never call each other's closures or share
rhai memory.** They interact only via the three verbs — which is what preserves
determinism, networking, hot-reload, and the ROS2 boundary:

| Channel | A → B | Analogue |
|---|---|---|
| Events (`TelemetryEvent`) — primary | A `emit(...)` → B `on_event` | Godot signals / ROS2 topics |
| Shared ECS state (reflection) | A writes component → B reads it (World = blackboard) | ECS/BT blackboard |
| Cosim ports (`inputs`/`outputs`) | A output wired to B input | Modelica `SimConnection` |

No direct cross-VM calls are offered — by design.

**Orchestration patterns (same verbs):** *distributed* (each entity runs its own
behavior) vs *centralized* (one scenario `cmd()`s many entities).

**Determinism — frame-delayed actor model:**
1. Iterate `ScriptedModel`s in deterministic order (by `GlobalEntityId`).
2. Events emitted in tick N delivered at start of tick N+1 (queued, drained
   deterministically) → "A emits, B reacts" is order-independent. One-tick latency.
   Same-tick delivery only for explicitly local/non-replicated events.

## 8. Design decisions

### Resolved
- Sequencing model → **callbacks first** (persistent rhai, `on_tick`/`on_event`),
  declarative plans added later.
- Bridge scope → **all commands, behind RBAC** (generic `cmd()`).
- Integration depth → **two-channel** (commands for writes + reflection bridge for
  reads); finish the `EntityProxy` stub as the read plane.
- Higher-level constructs (objectives, behavior trees, sequencer, navigation) →
  **rhai stdlib, NOT core**. Core ships mechanism only.
- Events → **reuse `TelemetryEvent`/`SampledParameter`** (no new type); finish the
  `SubscribeTelemetry` stub as the single pub/sub path.
- Event subject identity → **YAMCS mnemonic** in `name` (zero schema change).
- ROS2 → events↔topics, commands↔services/actions, objectives↔actions; keep the
  message bus as the bridge seam; rhai stays ROS-agnostic.

Still open:

- **Scenario as a Document?** Store scenario scripts as `ScriptDocument`
  (`language: Rhai`) for hot-reload/versioning/undo — reuse existing substrate. (Rec: yes.)
- **Where scenarios live in USD** — a `lunco:scenario` prim attr (script id +
  params) so a `.usd`/Twin carries its scenario, like terrain recipes? (Rec: yes.)
- **Action-envelope shape** — exact serializable goal/feedback/result/cancel struct
  for ROS2 action interop (defer until ROS2 bridge work starts).
