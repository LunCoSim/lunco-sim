# Rhai Integration Design — scripting & scenarios

> Status: Active · Audience: contributors on `lunco-scripting` and the scenario runtime
>
> The *how-to* is [`../scripting-guide.md`](../scripting-guide.md); this is the why.

Rhai drives scenarios — *"rover moves along a path via checkpoints, loads next
goals"* — and, more broadly, **manipulates every object in the sim (Twin, USD,
Modelica, cosim, scene, vehicles) from script.** The engine builds on native
(default), `--no-default-features` (script-free), `python`, and
`wasm32-unknown-unknown`.

> **Authoring a scenario?** Read the **[Scripting Guide](../scripting-guide.md)** —
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
- **Browser tool surface** — `lunco-web` exposes `mountRhaiTool`, which mounts a
  trusted HTML fragment and stylesheet and routes explicit `data-rhai` actions
  or `application/rhai` script blocks through the existing `lunco_rhai` bridge.
  HTML/CSS is presentation; Rhai remains policy and the typed command bus remains
  the only engine mutation boundary. Native editor panels remain egui panels.
- **Timeline storage** — `RegisterTimeline` / `RunStoredTimeline` +
  `ListTimelines` / `GetTimeline`, persisted to `<twin>/timelines/*.json`.
- **USD-embedded scenarios (load)** — a `LunCoProgramAPI` child prim naming a `.rhai`
  (`info:implementationSource = "sourceAsset"` or `"sourceCode"` authored in place)
  auto-attaches + runs on spawn.
- **Execution scope** — host/standalone scenarios run authoritatively. A
  client-scoped scenario may run for local presentation and prediction, but its
  `cmd()` calls are limited to registered client-local commands or
  ownership-gated predictive controls; direct reflected writes, structural
  edits, and policy changes are rejected.

Python currently supports only the optional one-shot `RunPython` command. It
does not implement `ScenarioRuntime` or execute `ScriptedModel` lifecycle
hooks; Python scenario lifecycle support remains explicitly planned in
`lunco-scripting/src/scenario.rs`.
The native shared-library probe is lazy: the scripting plugin keeps Python
`Uninitialized` until a Python command, participant, or REPL request needs the
runtime. A Python participant then resolves availability at the USD bind seam;
an unavailable interpreter is reported as a terminal participant error.

---

## Running scenarios

**Principle:** core = mechanism, rhai = ALL policy (objectives, navigation,
behavior trees, sequencing live in hot-reloadable `.rhai`, never compiled in).

### How to load & run a scenario

A scenario is a `.rhai` program with lifecycle hooks. Attach it to any entity:

- **API / MCP / scripts:** the `RunScenario { target, source }` command
  (`crates/lunco-scripting/src/commands.rs`). MCP tool: **`run_scenario`**
  (`mcp/src/index.js`). HTTP: `{"type":"ExecuteCommand","command":"RunScenario","params":{"target":<gid>,"source":"<rhai>"}}`.
  Idempotent + **hot-reload**: re-running on the same entity recompiles in place
  (bumps `ScriptDocument.generation`).
- **One-shot eval (no attach):** the `RunRhai { code }` command — runs once with
  full World access; stdout is returned in the original deferred response.
- **Direct (code/tests):** insert a `ScriptDocument` into `ScriptRegistry` +
  attach `ScriptedModel { language: Rhai, document_id }`.

### Lifecycle hooks (per-entity runtime, `world_bridge.rs` `tick_rhai_models`)

```rhai
fn task(me) { ... }            // builds the native task tree once
fn mission(me) { ... }         // optional objective declaration
fn on_start(me) { ... }        // optional setup after (re)compile
fn on_event(me, evt) { ... }   // optional next-scenario-pass event reaction
fn on_stop(me) { ... }         // optional teardown
```

State rule (rhai-specific, important): script `fn`s are **pure** — an indirect
task helper must receive its configuration as arguments. The native task kernel
owns the cursor, dwell timing, and event waits; user policy does not maintain a
second fixed-tick loop or cursor map. The `on_tick` hook is reserved for
authored test scenarios that sample state and publish a bounded verdict; it is
not part of the production mission contract.

### Host verbs (the entire Rust-exposed vocabulary — `world_bridge.rs`)

| verb | channel | purpose |
|------|---------|---------|
| `cmd(name, #{params})` | write | fire ANY registered `#[Command]` by name (reflect dispatch via `ApiCommandEvent`); behind networking RBAC; host-authoritative |
| `query(name, #{params})` | read | invoke a read-only structured provider; data is direct, no-data is `()`, errors are `#{ok:false,error}` |
| `world_pos(id)` → `[x,y,z]` | read | float-origin-correct world position |
| `world_forward(id)` → `[x,y,z]` | read | world heading (only read rhai can't derive itself) |
| `get(id, "Comp.field")` | read | generic reflected component-field read |
| `set(id, "Comp.field", value)` | local write | host-side tuning of a supported reflected field or canonical scalar co-simulation port; not a command-bus hold |
| `get_setting("Res.field")` / `set_setting("Res.field", value)` | read / local write | reflected resource access for host settings/configuration |
| `get_twin_setting("namespace.key")` / `set_twin_setting("namespace.key", value)` | read / persistent write | generic scalar policy in the active Twin manifest; no per-setting Rust binding |
| `get_exposure("namespace", "property")` | read | one raw value from the generic engine exposure registry; Rhai owns presentation policy |
| `find(name)` / `list_entities()` | read | entity lookup by canonical `Name` / enumerate with human-readable presentation labels |
| `sim_tick()` | read | current FixedUpdate tick |
| `emit(name, value)` | event | fire a `TelemetryEvent` on the shared bus |

Everything else is **policy in rhai** — see the prelude
`assets/scripting/prelude/` (one file per topic): vector math, `distance`/`arrived`,
`steer_to`/`nav_to` (closed-loop steering), task-tree mission constructors,
`drive`/`brake`/`load_scene` wrappers. The prelude is loaded
FROM DISK at startup on native (edit → restart, no rebuild), with the
`include_dir!`-embedded copy when no editable asset directory exists and the
wasm source of truth (wasm-safe, no IO). Once a native disk source set is
selected, a parse error is reported and startup does not silently switch to
stale embedded policy.
NB: `goto` is a reserved word in rhai — the nav helper is `nav_to`.

### Events / pub-sub

`emit()` reuses the **`TelemetryEvent`** bus (observer-dispatched; YAMCS
mnemonic in `name`) — no new event type. External clients receive script events
via `SubscribeTelemetry` (`lunco-api` `executor.rs` + `subscription.rs`). Scripts
receive events via `on_event` on the next scenario pass: a running simulation
delivers at the next fixed pass, while a paused simulation uses the next
`Update` pass without running fixed-step behavior. Inter-script interaction is
bus-only (isolated VMs); see §7f.

### Examples

`assets/scripting/examples/`: `patrol.rhai` (waypoint loop, emits
checkpoints), `mission.rhai` (coordinator reacting via `on_event`),
`mission_plan.rhai` (declarative task-tree mission), `robot_mission.rhai`
(durable phase checkpoints), and `script_first_robot.rhai` (USD + Modelica
authoring through explicit document ids).

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

---

## 0. The key realization — the command bus is already the universal surface

The system already has a single, uniform manipulation API: the typed
`#[Command]` bus. ~90 commands span every subsystem, all are `#[reflect(Event)]`
(auto-discoverable), and **dispatch-by-name already exists** — `api_command_dispatcher`
(`crates/lunco-api/src/executor.rs:90-162`) deserializes JSON params into a
reflected struct and fires it with `ReflectEvent::trigger(world, &dyn Reflect, &type_reg)`.
HTTP and MCP are just two callers of this path.

**Therefore "manipulate everything from rhai" ≠ 90 bindings. It = ONE generic
bridge** (`cmd()` / `query()`) that reuses the reflect-dispatch and read-provider
paths. rhai becomes
a *third transport*. Every existing command — and every future one — is reachable
for free, with the same RBAC/authz gate the API already enforces.

Representative commands already covering the user's surface:

| Subsystem | Commands (file:line) |
|---|---|
| Rover/vehicle | `SetPorts` — writes named input ports (`throttle`/`steer`/`brake`); `DriveMix` allocates them to actuators (`lunco-cosim/src/lib.rs`, `lunco-mobility::apply_drive_mix`) |
| Camera/control | `PossessVessel`, `ReleaseVessel`, `FocusTarget`, `FollowTarget` (`lunco-avatar/src/commands.rs`) |
| Scene/USD | `LoadScene`, `ClearScene` (`lunco-usd-sim/src/cosim.rs:814,884`) |
| Scene editing | `SpawnEntity`, `MoveEntity`, `RotateEntity`, `TransformEntity`, `SetObjectProperty`, `SelectEntity` (`lunco-scene-commands/src/commands.rs`); `SelectUsdPrim` (`lunco-luncosim-edit/src/selection.rs`) |
| USD geometry editing | `SetUsdAttribute` (`lunco-scene-commands`) — standard USD attributes such as `point3f[] points`; the `gizmo` and `nurbs` Rhai tools are policy libraries over this command |
| Modelica/cosim | `CompileModel`, `SetModelInput`, run/step commands (`lunco-modelica/...`) |
| Celestial | `TeleportToSurface`, `LeaveSurface` (`lunco-celestial/src/commands.rs`) |
| Scripting | `RunRhai`, `RunPython` (`lunco-scripting/src/commands.rs`) |
| Reads | `ListEntities`, `DiscoverSchema`, `ReadPorts`, `ReadExposures`, `GetReadiness`, and domain query providers (all use the tagged `ExecuteCommand` envelope where applicable) |

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
| Sandboxed Rhai scenario runtime | `ScenarioDriver` owns the persistent per-entity engine and lifecycle caps |
| rhai → World access | `ScenarioRuntime` exposes host functions to rhai engine |
| Persistent script state across ticks | `this` map persisted on scenario entity across ticks |
| Temporal sequencing (wait/over-time) | Task-tree constructors in `prelude/tasks.rhai` (pure data), ticked NATIVELY on the `lunco-behavior` kernel (`lunco-scripting/src/task_tree.rs`) |
| Navigation: waypoints/goals/arrival/path-follow | `nav_to`, `drive`, task trees in `prelude/nav.rhai` and `prelude/tasks.rhai` |
| By-name entity lookup | `find(name)` verb; `name(id)` returns the presentation label and `QueryEntity` supplies the full USD path |
| Timer "after N seconds" | `wait(secs)` / `wait_until(cond)` in the native task tree |
| Telemetry subscribe (events to script) | `on_event` hook and task `wait_for` delivery |

---

## 2. Architecture — two layers

```
┌─────────────────────────────────────────────────────────────┐
│ Layer B — Scenario Runtime (temporal: checkpoints, goals)    │
│   persistent per-scenario rhai (AST+Scope), host lifecycle   │
│   task(me) / mission(me) + lifecycle and event hooks           │
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
Scenario and command scripts run in an **exclusive system** (`&mut World`) and
expose the scoped world bridge for the evaluation duration. Reads run
synchronously; command writes use the shared reflected command dispatcher, while
host-local tuning writes use the reflected component/resource and port seams.
The scenario driver is the authoritative Rhai lifecycle runtime. `RunRhai` uses
the same engine and bridge for one-shot evaluation without attaching a
persistent `ScriptedModel`; it is a separate execution mode, not a second
lifecycle or compatibility implementation.

### 3.2 Exposed verbs (the generic runtime surface)
```rust
cmd(name: &str, params: Map) -> Dynamic       // any registered #[Command]
query(name: &str, params: Map) -> Dynamic     // registered read-only provider
get(id, path) / set(id, path, value)           // reflected fields / scalar ports
get_setting(path) / set_setting(path, value)  // reflected resource fields
find(name) / name(id) / parent(id) / children(id)
list_entities() / world_pos(id) / world_forward(id) / world_rotation(id)
sim_tick() / dt() / elapsed_seconds()
emit(name, value?) / subscribe(name) / subscribe_prefix(prefix)
```
These are the generic Rust-side operations. `cmd()` reaches every registered
public command and `query()` reaches every registered provider; future domain
commands and queries become available through their owning registrations without
new script bindings. `ScriptingCatalog` is the live authoring description of
this surface.
Twin/USD/Modelica/cosim are all just command or provider names.

### 3.3 Ergonomics live in a rhai *prelude*, not Rust
Ship a standard `prelude.rhai` (script, not Rust) wrapping raw `cmd()` into
friendly verbs — so authoring stays nice without per-command Rust code:
```rhai
fn drive(r, fwd, steer) { cmd("SetPorts", #{ target: r, writes: [["throttle", fwd], ["steer", steer]] }); }
fn possess(r)           { cmd("PossessVessel", #{ target: r }); }
// `path` is a root-qualified scene address (`lunco://…` or `twin://…`).
// Use `OpenFile` for a filesystem path so the owning Twin is discovered first.
fn load(path)           { cmd("LoadScene", #{ path: path, root_prim: "" }); }
fn set_prop(id, k, v)   { cmd("SetObjectProperty", #{ target: id, key: k, value: v }); }
```

### 3.4 Security (must-have)
`cmd()` and direct reflected writes MUST pass through the same
authz/RBAC gate as the API
(`#[authz_target]`, `SessionRegistry`, sender identity). A shared/untrusted
scenario script then can't exceed its owner's authority. The luncosim caps
(ops/depth/size) already bound runaway scripts. The exposed verb set = the
entire capability surface — query providers are read-only, while commands carry
the mutation contract and policy. Client-scoped scripts cannot use direct
mutation paths.

---

## 4. Layer B — scenario runtime (the checkpoints/goals problem)

### 4.1 The task-tree contract
rhai is synchronous and `SetPorts` carries no persistent setpoint, so a mission
must re-emit actuator commands while it is active. The canonical script surface
is a cooperative task tree: `task(me)` returns pure data once, and the native
behavior kernel advances one deterministic step per fixed tick.

```rhai
fn task(me) {
    let goals = [[12.0, 0.0, 0.0], [12.0, 0.0, 25.0]];
    let steps = [];
    for i in 0..goals.len() {
        let goal = goals[i];
        steps.push(step(|m| nav_to(m, goal, 1.0, 2.0),
                        |m| arrived(m, goal, 2.0)));
    }
    steps.push(wait_for("MISSION_RELEASE"));
    seq(steps)
}
```

Task leaves provide drive-until, one-shot, dwell, predicate, and event waits;
composites provide sequence, parallel, repeat, race, retry, and reactive policy.
`mission(me)` separately declares objective state and completion conditions.
Both are hot-reloadable Rhai policy; Rust supplies only the generic runtime,
command/query bridge, and behavior-kernel mechanism.

The exact node contract is documented in
[`rhai-task-tree.md`](rhai-task-tree.md): every node has an explicit `kind`,
including leaves, and the adapter rejects missing, unknown, or cross-kind
fields. The task map is an authoring boundary; runtime execution uses the
typed task-kind parser and the existing `lunco-behavior` composites.

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
   native task/mission drivers plus `on_start`/`on_event`, hot-reload via
   `ScriptDocument`.
3. **Navigation primitives** — `distance`/`arrived`/`steer_toward` +
   `PathFollower`; the checkpoint/goal scenario runs end to end.
4. **Authoring polish** — declarative-plan executor, scenario examples,
   editor/Inspector params, telemetry→`on_event` wiring.

---

## 7b. Is the command bus enough? — No: two-channel model

The command bus is the right channel for **writes that must be authoritative,
replicated, RBAC-gated, undoable, and audited**. It is the wrong channel for
**reads** and **fine-grained state** — which task closures need synchronously.
Commands that need a result use the original deferred response
(`executor.rs`); read providers own their result shape and coordinate-frame
semantics — no generic component dump, no guessed render-frame position, and no
implicit compatibility blob. Reflect-dispatch JSON-(de)serializes per call. The
Rhai read bridge is implemented against the live ECS; the Python
`EntityProxy` remains a separate one-shot binding concern.

So tighter integration IS needed, as a **second, complementary channel**:

| | Channel 1 — Commands (write/action) | Channel 2 — Reflection bridge (data plane) |
|---|---|---|
| Direction | writes | reads (+ scoped local writes) |
| Mechanism | `cmd()` → `ReflectEvent::trigger` | `AppTypeRegistry` + `ReflectComponent`/`ReflectResource` plus `PortRegistry` |
| Use for | SetPorts, LoadScene, Spawn, SetObjectProperty — anything authoritative/replicated/undoable | position, heading, sensors, cosim/Modelica vars, reflected fields/resources, entity iteration |
| Latency | request/response or deferred result | **synchronous** during eval |
| Replicated? | yes (CommandBus SyncChannel) | no (local read) |
| Cost | JSON+reflect+observer per call | direct reflected field access (no JSON) |

**Both run inside the same World-bound exclusive-system context** (§3.1). The
Rhai reflection bridge is implemented against `AppTypeRegistry`,
`ReflectComponent`, and `ReflectResource`; its authoring surface is also
reported by `ScriptingCatalog`, including which reflected fields the native
converter can write. The Python `EntityProxy` remains a separate one-shot
binding concern and is not part of the Rhai runtime contract.

**Default rule — reads direct, mutations explicit:**
- READ arbitrary state → reflection bridge (fast, synchronous, local). `pos(r)`,
  `entity(r).Battery.level`, `cosim_var(m, "height")`.
- WRITE that must replicate / be authoritative / undoable → `cmd()` (bus). Keeps
  determinism + networking intact: clients receive the authoritative command
  stream, they don't run scenario logic (§6).
- Direct reflected writes and canonical port writes are host-side tuning
  surfaces and are authority-gated; a client-scoped script cannot use them
  because they have no prediction/forwarding path. `SetPorts` is the
  persistent-hold control command; `set()` is a raw write.

This makes "manipulate *everything*" real: Channel 1 = every typed command;
Channel 2 = every readable field. Hot per-tick paths can later get typed
accessors generated from reflection if profiling demands it.

## 7c. Critical review — is this standard, and what are we missing?

**Plumbing: correct, for the right reason.** The two-channel split (reflection
reads + command writes) is unusual vs Unity/Godot/Unreal (which read+write objects
directly), but correct for our *category*: a **deterministic networked sim**
(Factorio / RTS lockstep), where mutations must flow through a replicated ordered
command stream and reads are local. Reads-via-reflection and a lifecycle callback
(`task(me)` plus event hooks) are the production mission surface.

**Scenario layer: implemented.** The current runtime is event-first and keeps
policy out of the Rust engine core:

1. **Sequencing** — `seq`/`par_*`/`repeat`/`wait_*` are data nodes executed by the
   behavior kernel; Rhai authors the policy and callbacks.
2. **Events and Sensors** — `TelemetryEvent` reaches `on_event`; Avian overlap
   Sensors publish waypoint arrivals. A waypoint authors a visible dome and a
   ground-anchored invisible trigger, both using the same standard USD `radius`;
   mission scripts do not scale markers or poll a duplicate arrival tolerance.
3. **Behavior Trees** — BT.CPP v4 XML owns route topology and the native behavior
   kernel executes it; USD owns waypoint identity and geometry.
4. **Objectives** — declarative Rhai objectives consume real event/state predicates
   and publish completion to the tutorial HUD.
5. **Simulation time** — waits use simulation time and respect pause/transport rate.
6. **Observability** — `ScriptStatus`, `ScriptInspect`, route cursor state, and
   `ReachedWaypoints` expose execution and arrival state.

**Corrected layering — everything above the core line is rhai, not Rust:**
```
┌─ rhai stdlib (hot-reloadable .rhai, moddable, NOT compiled) ──────────┐
│ Objectives / contracts / missions    conditions, completion, branching│
│ Behavior Trees / Sequencer           coroutine substitute (state in Scope)
│ Navigation (goto/arrived/steer)      pure rhai over world_pos + cmd    │
│ Prelude command wrappers                                               │
└───────────────────────────────────────────────────────────────────────┘
══════════════ CORE BOUNDARY (mechanism only) ══════════════════════════
  Scenario VM (AST+Scope, hot-reload) · task/mission/on_start/on_event
  Ch.1 cmd() → reflect+RBAC · Ch.2 reflection reads · world_pos()
  Event bus (emit + deliver; ROS2 bridge seam) · sim_tick()/dt()/elapsed_seconds()
  Events/Triggers from Avian sensors (volumes, not distance polling)
  USD scene/prefab (static authoring)
```
The Sequencer is **Rhai policy data, not core logic**. BT.CPP route topology is
decoded by the autopilot domain and executed by its generic behavior kernel. The
engine core only provides lifecycle, command, observation, and event mechanisms.

Rhai has no native coroutines; the cooperative task tree is the deterministic,
hot-reloadable replacement and does not create a second engine loop.

## 7d. Core/script boundary (mechanism vs policy) + ROS2

**Directive:** objectives are authored in rhai; behavior trees and
all higher-level constructs are REMOVED from the Rust core; ROS2 integration is
planned. Resulting split:

**Core exposes only (irreducible mechanism):**
- Persistent scenario VM — `rhai::AST` + `Scope` per scenario, recompiled on
  `ScriptDocument` source change (hot-reload).
- Host→script hooks: native `task(me)`/`mission(me)` drivers, `on_start()`,
  `on_event(evt)` for production scenarios; authored test scenarios may also
  use `on_tick(me)` for bounded verdict observation (all sim-time,
  transport-gated via `TimeTransport`).
- Ch.1 write — `cmd(name, #{…})` → `ReflectEvent::trigger`, behind RBAC.
- Ch.2 read/local tuning — reflection and the canonical port bridge
  (`get`/`set`, `get_setting`/`set_setting`, `query()`, `list`, `find`) +
  `world_pos(entity)` (float-origin/big_space correct — the ONE nav read that
  must be native).
- Event bus — `emit(name, data)` + delivery of physics/sensor/timer/**external**
  events to `on_event`. This bus is the ROS2 bridge seam.
- `sim_tick()`, `dt()`, `elapsed_seconds()`, and deterministic seeded `rand()` /
  `rand_range()` / `rand_int()`.
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
| Service (req/resp) | command + `Ack` on the original request |
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
- `TelemetryValue` (F64/I64/Bool/String/Array/Map, serde) (`:41`) — the typed payload value; structured event parameters do not need string parsing.
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

**Subscription/delivery — one existing path:**
`ApiRequest::SubscribeTelemetry { filter }` (`schema.rs:35`) + `TelemetryResponse`
(`schema.rs:74`) are the pub/sub surface. The executor registers a filtered
subscription, and the same `TelemetryEvent`/`SampledParameter` stream feeds Rhai
scenarios (`on_event`), external API/MCP subscribers, the ROS2 topic bridge, and
the UI. No second event model.

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

**Attach a script to an entity:** reuse `ScriptedModel` (`doc.rs:100`) — the
per-entity hook (`document_id`, `language`, `paused`, `inputs`/`outputs`). Rhai
scenarios use `RhaiScenarioRuntime`; Python has no `ScriptedModel` lifecycle
runtime yet and is currently limited to one-shot `RunPython` evaluation. The
script's `task(me)` identity is the host entity id; `this` is the persistent
scenario-state map supplied to lifecycle hooks and native task closures.

**Execution model:** ONE shared `rhai::Engine` resource (all host fns registered),
**per-entity `AST` + persistent `Scope`** (compiled once, hot-reloaded on source
change). Fixes today's "fresh Engine per eval" cost. The same `ScriptDocument`
reused on many entities = **prefab scripts** — 10 rovers run `patrol.rhai`, each
with its own `Scope` (independent goal index/state).

Task leaves use anonymous closures with one positional host id: `|me| ...`.
The task driver invokes them with the persistent scenario state map as `this`.
Named `Fn("...")` pointers are ordinary script callbacks, not task leaves; a
named helper can be called explicitly from an anonymous task closure. This is
the single callback contract at the Rhai-to-kernel boundary.

**Two roles, both just `ScriptedModel`s:** entity-script (autonomy, ~Unity
MonoBehaviour) and scenario-script (orchestration, on a scenario/singleton entity).

**Inter-script interaction — through the World, never directly.** Each script is an
**isolated VM (own AST+Scope); scripts never call each other's closures or share
rhai memory.** They interact through the generic world channels — which preserves
determinism, networking, hot-reload, and the ROS2 boundary:

| Channel | A → B | Analogue |
|---|---|---|
| Events (`TelemetryEvent`) — primary | A `emit(...)` → B `on_event` | Godot signals / ROS2 topics |
| Shared ECS state (reflection) | A writes component → B reads it (World = blackboard) | ECS/BT blackboard |
| Cosim ports (`inputs`/`outputs`) | A output wired to B input | Modelica `SimConnection` |

No direct cross-VM calls are offered — by design.

**Orchestration patterns (same verbs):** *distributed* (each entity runs its own
behavior) vs *centralized* (one scenario `cmd()`s many entities).

**Determinism — pass-delayed actor model:**
1. Iterate `ScriptedModel`s in deterministic order (by `GlobalEntityId`).
2. Events emitted in one driver pass are delivered at the start of the next
   driver pass (queued, drained deterministically) → "A emits, B reacts" is
   order-independent. Paused simulations continue discrete delivery from
   `Update`, while fixed-step behavior remains stopped.

## 8. Design decisions

### Resolved
- Sequencing model → **task trees first** (persistent Rhai policy with native
  progression); `on_event` remains the event reaction seam.
- Bridge scope → **all commands, behind RBAC** (generic `cmd()`).
- Integration depth → **two-channel** (commands for writes + reflection bridge for
  reads); finish the `EntityProxy` stub as the read plane.
- Higher-level constructs (objectives, behavior trees, sequencer, navigation) →
  **rhai stdlib, NOT core**. Core ships mechanism only.
- Events → **reuse `TelemetryEvent`/`SampledParameter`** (no new type); the typed
  `SubscribeTelemetry` path is the single pub/sub surface.
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
