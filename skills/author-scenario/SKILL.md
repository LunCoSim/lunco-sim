---
name: author-scenario
description: >
  How to write a scenario in LunCoSim — a rhai program attached to an entity
  that senses the world and drives it every tick. USE THIS SKILL whenever the
  user asks, in plain words, things like: "make the rover patrol these
  waypoints", "drive it to X then Y", "have it react when it reaches / enters /
  sees something", "coordinate these two vehicles", "run this mission /
  sequence / timeline", "make it do X after N seconds", "spawn some rovers and
  have them survey the area", or "why isn't my script doing anything / holding
  its state?". Any request to orchestrate behaviour, missions, waypoints,
  reactions, or multi-entity coordination belongs here — the user will NOT say
  "scenario" or "rhai". (For the agent mid-code, it also covers: an `on_tick` /
  `on_event` / `on_start` hook, `RunScenario`, `nav_to` / `run_plan` / a
  sequencer step, `emit` / a `TelemetryEvent`, `this`-state that resets or
  reads empty, a `find`/`cmd`/`query` verb, or a `LunCoProgram` prim.) These
  rules are project-specific: rhai `fn`s are pure (they can't see top-level
  `let`, so naive state silently vanishes), `this` binds ONLY in the hook the
  engine calls, `goto` is reserved, events arrive one tick late, scripts are
  host-authoritative (never run on a client), and control MATH does not belong
  here (that's Modelica — see authoring-vessel-controllers). Reference impls:
  assets/scripting/examples/ (patrol, mission, sequence, timeline, avoid).
---

# Authoring scenarios

A **scenario** is a rhai program attached to an entity that runs **every fixed
simulation tick** via lifecycle hooks — not a one-shot snippet. It is the
**policy** layer: navigation, missions, reactions, coordination.

> **Host = mechanism, script = policy.** A scenario touches the world only
> through the same command/query API the HTTP API, MCP, and UI use — so it
> inherits every command for free and stays decoupled from physics.

**Scope boundary — do not blur these:**
- **Control MATH** (PID, mixing, force/torque) → Modelica, NOT rhai. If you're
  writing a per-tick control loop here, stop — see
  [`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md).
- **Scene structure / spawning geometry / wiring** → USD.
- A scenario **senses and decides**; it drives via high-level verbs
  (`nav_to`, `drive`, `cmd`), reacts to events, and sequences phases.

Full reference: [`docs/scripting-guide.md`](../../docs/scripting-guide.md). The
authoritative callable surface in one place: the `ScriptingCatalog` query.

## 1. Lifecycle hooks — the shape of every scenario

Define any subset. First param (`me`) is the host entity id; per-tick mutable
state lives on the implicit `this` map.

```rhai
fn on_start(me)      { this.i = 0; }                      // once, after (re)compile
fn on_tick(me)       { this.i += 1; }                     // every FixedUpdate tick
fn on_event(me, evt) { if evt.name == "GO" { /* … */ } }  // a TelemetryEvent arrived
fn on_stop(me)       { brake(me); }                       // teardown: hot-reload / detach / despawn
```

**The state rule that trips up everyone (get this right first):**
- rhai `fn`s are **pure** — they CANNOT see top-level `let`s. Thread all
  persistent state through **`this`**.
- `this` is bound **ONLY** inside the hook the engine calls directly — **NOT**
  in prelude/helper functions it calls. So helpers must be stateless: take state
  in, return it out. Never read `this` from a helper.
- `this` resets on hot-reload (re-`RunScenario` recompiles in place; the old
  program's `on_stop` runs first).

## 2. The verb surface (host bridge — everything else is prelude)

| Verb | Purpose |
|---|---|
| `cmd(name, #{params})` | **WRITE** — fire any `#[Command]` by name; returns `#{id,ok,data,error}` (`data` carries e.g. a spawned gid) |
| `query(name, #{params})` | **READ** — any query provider (Raycast, Nearest, GroundHeight, …) |
| `get(id,"Comp.field")` / `set(id,"Comp.field",v)` | reflected component read / write |
| `world_pos(id)` / `world_forward(id)` | float-origin-correct pose (use these, never raw `Transform`) |
| `find(name)` / `name(id)` / `parent`/`children` | entity lookup + hierarchy |
| `owner_of(id)` / `controller(id)` / `is_controlled(id)` | who's driving (human vs AI vs unowned) |
| `emit(name, value?)` | fire a `TelemetryEvent` (delivered to `on_event` **next** tick) |
| `sim_tick()` / `dt()` / `elapsed_seconds()` | the fixed clock |
| `rand()` / `rand_range(lo,hi)` | **deterministic** RNG (seeded per `(entity,tick,hook)`) |
| `despawn(id)` / `add`/`remove`(id,"Comp",…) | structural. **Spawn:** `cmd("SpawnEntity", #{entry_id, position})` — no generic spawn |
| `notify(msg)` / `notify_kind(msg,kind)` | HUD notification |

JSON appears **only** at the `cmd`/`query` params seam. `get`/`set` are native
reflect — no JSON round-trip.

## 3. Prelude helpers (hot-reloadable policy — no Rust rebuild)

`assets/scripting/prelude/*.rhai`, one file per topic. Read them for the full
list. Highlights:
- **Nav:** `drive(rover,fwd,steer)`, `brake(rover)`, `nav_to(entity,target,speed,radius)` (returns true on arrival), `run_plan`. **`goto` is a reserved word — use `nav_to`.**
- **Sensing:** `distance`, `arrived`, `velocity`/`speed`, `raycast`, `obstacle_ahead`, `ground_height`, `nearest`, `entities_in_radius`.
- **Selection:** `all_of_type`, `nearest_where`, `count_where`, `min_by`/`max_by`.
- **Sequencer:** `seq([steps])`, `step`/`once`/`wait`/`wait_until`/`wait_for`; feed events with `seq_note_event` in `on_event`.

Add helpers freely — edit the prelude, no rebuild.

## 4. Missions & sequencing (two layers, both pure rhai)

- **Layer 1 — imperative steps** (`examples/sequence.rhai`): build a step array
  with `step`/`wait`/`wait_for` and run with `run_steps`.
- **Layer 2 — declarative timeline** (`examples/timeline.rhai`): a mission as
  **pure data** (`{move_to|cmd|emit|wait|wait_event}` steps) lowered by
  `compile_timeline`. Serialisable — run inline with `RunTimeline`, or persist.

Progress is observable on the bus: `STEP_COMPLETE(idx)`, `SEQUENCE_COMPLETE(len)`,
`OBJECTIVE_COMPLETE`/`PLAN_COMPLETE`.

For **complex reactive AI** (obstacle avoidance, interception) prefer the
Autopilot Behavior Tree (`cmd("SetAutopilotBehavior", #{vessel, spec_json})`,
see `docs/behaviour-trees.md`) over hand-rolled `on_tick` state machines.

## 5. Events — the reactive spine

`emit(name, value)` fires a `TelemetryEvent`; the target's `on_event` receives it
**one tick later** (deterministic actor model — "A emits, B reacts" is
order-independent). Scripts interact ONLY through events + shared ECS state,
never by calling each other's functions (isolated VMs). Producers also include
physics (`COLLISION_START`), lifecycle (`SCENE_LOADED`), and model-port thresholds
authored as `LunCoPortEvent` child prims of a program (one prim per rule:
`lunco:event:port`, `lunco:event:op`, `lunco:event:threshold`, `lunco:event:emit`).

## 6. Running & debugging

Prefer the HTTP API (curl-first; canonical port **4101** — launch per the
[`test-via-api`](../test-via-api/SKILL.md) / [`run-modelica`](../run-modelica/SKILL.md) skills):

```jsonc
// attach + run (idempotent hot-reload); source is inline rhai OR an asset path
{"command":"RunScenario","params":{"target":<gid>,"source":"<rhai or path>","params":"{\"speed\":1.5}"}}
{"command":"SetScenarioPaused","params":{"target":<gid>,"paused":true}}
{"command":"StopScenario","params":{"target":<gid>}}
```
- `params` is a JSON-object string; the script reads it as the read-only `params` constant.
- **Debug:** `ScriptStatus {target}` → compile/runtime health + located errors; `ScriptInspect {target}` → live `this`, hooks, generation, running/paused. `print(...)` goes to the process log.
- One-shot (no attach): `RunRhai {code}` — full world access, stdout via `QueryCommandResult`.

## 7. Persistence — bake into the scene (USD)

A script is a PRIM — give the entity a `LunCoProgram` child and it auto-runs on spawn.
Delete the prim and the behaviour is gone:
```usda
def Xform "Rover_01"
{
    def LunCoProgram "Patrol"
    {
        uniform asset info:sourceAsset = @scenarios/patrol.rhai@
        # or author the source in place:
        # uniform string lunco:program:sourceCode = '''fn on_tick(me){ ... }'''

        # per-instance config: one typed attribute per key, read by param(me, "speed", 1.0)
        custom float lunco:param:speed = 2.0
    }
}
```
Timelines persist via `RegisterTimeline` → `<twin>/timelines/*.json`; tool
libraries → `<twin>/tools/*.rhai`.

## The recipe (checklist)

1. Decide the shape: reactive (`on_event`) vs sequenced (timeline/sequencer) vs continuous (`on_tick` + `nav_to`). Reach for a Behavior Tree if it's a reactive AI.
2. Write hooks; keep ALL state on `this`; keep helpers stateless.
3. Drive with prelude verbs (`nav_to`/`drive`/`cmd`) — never a control loop (that's Modelica).
4. Wire reactions through `emit`/`on_event` (remember the one-tick delay).
5. `RunScenario` on the target gid; verify with `ScriptInspect`; iterate by re-running (hot-reload).
6. Persist it as a `LunCoProgram` child prim on the target once it works.

## Anti-patterns (each has cost real time)

- ❌ Persistent state in top-level `let` or read from a helper — invisible/unbound. Use `this`, in hooks only.
- ❌ A per-tick control law (PID, force mixing) in rhai — belongs in Modelica.
- ❌ `goto(...)` — reserved word; use `nav_to`.
- ❌ Expecting an `emit` to be seen the same tick — it arrives next tick.
- ❌ Assuming a scenario runs on clients — it's host-authoritative; clients get replicated state, not the script.
- ❌ A generic `spawn(...)` — use `cmd("SpawnEntity", #{entry_id, position})` so clients reconstruct from the catalog.
- ❌ Reading raw `Transform` for position — use `world_pos` (float-origin correct).

## Drivetrain parity test

A scenario can also be a **regression test**. `assets/scenarios/drivetrain_parity.rhai`
+ `assets/scenes/sandbox/drivetrain_parity.usda` are the worked example — copy
their shape when you need a scenario that ASSERTS rather than merely acts.

**What it guards.** Raycast and joint wheels are two realizations of ONE
parameter set. They once diverged: the no-load axle speed was authored under two
names (60 vs 12 rad/s) AND the raycast drive force had no torque–speed term, so
raycast rovers ran ~5× faster than joint rovers built from the same asset. Both
now read the single authored `physxVehicleEngine:maxRotationSpeed` (12 rad/s, in
`components/mobility/wheel.usda`), so both cap at `ω_max·r = 12 × 0.4 = 4.8 m/s`.
The scene instances `skid_rover.usda` **twice**, differing in exactly one
opinion — `variants = { string drivetrain = "raycast" | "physical" }` — and the
scenario drives BOTH from ONE tick loop, so they see identical commands on
identical frames. Tolerances: **±15 %** terminal/peak speed, **±20 %** distance
(it integrates the acceleration transient, where the solvers legitimately differ
most), **±35 %** yaw magnitude with an **intolerant sign check**, plus an
absolute `[0.5, 1.25] × ω_max·r` band — parity alone is satisfiable by both
rovers being wrong together.

**How to run.**
```bash
cargo run -j2 --bin sandbox -- --scene scenes/sandbox/drivetrain_parity.usda 2>&1 | tee /tmp/parity.log
```
The `LunCoProgram` prim in the scene auto-runs the script on load; the run takes
~21 s of sim time (3 s settle → 12 s straight → 6 s steer). Then:
```bash
grep -E 'DRIVETRAIN PARITY|PARITY FAIL|TESTS_' /tmp/parity.log
```

**How to read the verdict.** There is no exit code — a scenario is a tick hook,
not a process — so it prints the harness verdict contract
(`assets/scripting/tests/lib/test_assert.rhai`) and one unmistakable last line:
```
TESTS_OK 8
DRIVETRAIN PARITY: PASS
```
or
```
  PARITY FAIL: terminal speed (m/s): raycast=23.9 physical=4.71 ratio=5.07x diff=80.29% (tol 15%)
TESTS_FAIL 1/8
DRIVETRAIN PARITY: FAIL
```
It also `emit`s `DRIVETRAIN_PARITY` = `"PASS"`/`"FAIL"` and raises a toast.

**The part worth copying: make silence impossible.** Scenarios fail *silently* —
a hook that never fires, a `find` that returned `-1`, a phase that never
advances all look like a clean run. So: print the resolved gids in `on_start`;
fail loudly on the first tick if a prim is missing instead of ticking forever;
log a `[parity] …` sample row with real numbers every 0.5 s (**the log is the
evidence — a run with no sample table proves nothing**); and treat *both values
≈ 0* as a FAILURE, never a match. A test that cannot fail is the bug one level
up.
