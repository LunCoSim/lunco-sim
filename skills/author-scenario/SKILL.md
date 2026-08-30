---
name: author-scenario
description: >
  Author event-driven LunCoSim scenarios for missions, waypoints, reactions, or
  multi-entity coordination. Use when working with `task`, `mission`, `on_event`,
  `RunScenario`, `nav_to`, `emit`, or persistent `this` state. Scenarios own
  sequencing and policy; Modelica owns continuous control math, USD owns scene
  structure and wiring, and authoring-vessel-controllers owns vessel GNC.
---

# Authoring scenarios

A **scenario** is a rhai program attached to an entity. Production scenarios
are task/event-driven policy. They must not define `on_tick`; that hook is
reserved for authored tests under `assets/scenarios/tests/` to sample live
telemetry and publish a bounded verdict. Continuous rover dynamics remain in
fixed-step physics/Modelica.

> **Host = mechanism, script = policy.** A scenario touches the world only
> through the same command/query API the HTTP API, MCP, and UI use — so it
> inherits every command for free and stays decoupled from physics.

**Scope boundary — do not blur these:**
- **Control MATH** (PID, mixing, force/torque) → Modelica, NOT rhai. If you're
  writing a per-tick control loop here, stop — see
  [`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md).
- **Scene structure / spawning geometry / wiring** → USD.
- **Vector and angle math is already NATIVE — never write it in a script.**
  `vadd` `vsub` `vscale` `vlen` `vdot` `vcross` `vnorm` `qrot` `clamp`
  `angle_deg` `yaw_delta_deg` are Rust (`lunco_scripting::rhai_math`, on glam),
  operating on the same `[x,y,z]` float arrays `world_pos` / `world_forward`
  return. Reimplementing one in rhai is how four scripts ended up with four
  copies of the same broken `acos` guard.
- A scenario **senses and decides**; it drives via high-level verbs
  (`nav_to`, `drive`, `cmd`), reacts to events, and sequences phases.

**The two rules that make the math surface safe:**

1. **Every math verb is TOTAL and returns `()` when there is nothing to
   measure** — a `()` input, a wrong-length array, a degenerate orientation.
   Check with `== ()`; never accumulate an unchecked result. There is no NaN to
   guard against, because a partial function's domain is enforced in Rust:
   ```rhai
   let d = yaw_delta_deg(this.fprev, world_forward(me));
   if d != () { this.yaw += d; }        // skip the tick, don't poison the sum
   ```
2. **Angles are PER-TICK DELTAS.** `yaw_delta_deg` saturates at 180°, so a total
   swept angle is accumulated from deltas — never measured start-to-end. Past
   half a revolution a direct measure folds back and reads as a turn the other
   way.

Full reference: [`docs/scripting-guide.md`](../../docs/scripting-guide.md). The
authoritative callable surface in one place: the `ScriptingCatalog` query.

## 1. Lifecycle hooks — the shape of every scenario

Define any subset. First param (`me`) is the host entity id. Production
progression is returned by `task(me)` and advanced by the native behavior
kernel; `mission(me)` supplies durable objective tracking. Lifecycle/event hooks
remain available for setup, reactions, and teardown.

```rhai
fn task(me)           { seq([wait_until(|m| arrived(m, GOAL, 2.0))]); }
fn mission(me)        { [objective("survey", #{})]; }       // optional
fn on_start(me)       { this.i = 0; }                       // once, after (re)compile
fn on_event(me, evt)  { if evt.name == "GO" { /* … */ } } // event-driven policy
fn on_stop(me)        { brake(me); }                       // hot-reload / detach / despawn
// Bounded sampled observer (tests only):
// fn on_tick(me) { this.samples.push(query("rover_status", #{id: me})); }
```

**The state rule that trips up everyone (get this right first):**
- rhai `fn`s are **pure** — they CANNOT see top-level `let`s. Thread all
  persistent state through **`this`**.
- `this` is the persistent scenario-state map. Direct lifecycle/mission drivers
  receive the host entity id as `me`; task leaves receive that same id as their
  one positional argument and are authored as anonymous closures (`|me| ...`).
  The native task driver binds `this` while invoking those closures, and owns
  the task cursor/dwell/event state. Named `Fn("...")` pointers are not task
  leaves; call a named helper explicitly from an anonymous closure when useful.
- Hot-reload runs `on_stop` before installing the new program state; initialize
  all required `this` fields in the new run.

## 2. The verb surface (host bridge — everything else is prelude)

| Verb | Purpose |
|---|---|
| `cmd(name, #{params})` | **WRITE** — fire any `#[Command]` by name; returns `#{id,ok,data,error}` (`data` carries e.g. a spawned gid) |
| `query(name, #{params})` | **READ** — any read-only query provider (Raycast, Nearest, GroundHeight, …) |
| `get(id,"Comp.field")` / `set(id,"Comp.field",v)` | reflected component read / write |
| `world_pos(id)` / `world_forward(id)` | float-origin-correct pose (use these, never raw `Transform`) |
| `find(name)` / `name(id)` / `usd_path(id)` / `parent`/`children` | entity lookup + hierarchy; `name` is presentation, `usd_path` is canonical USD topology |
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
- **Nav:** `drive(rover,fwd,steer)`, `brake(rover)`, `nav_to(entity,target,speed,radius)` (returns true on arrival). New missions return task trees. **`goto` is a reserved word — use `nav_to`.**
- **Sensing:** `distance`, `arrived`, `velocity`/`speed`, `raycast`, `obstacle_ahead`, `ground_height`, `nearest`, `entities_in_radius`.
- **Selection:** `all_of_type`, `nearest_where`, `count_where`, `min_by`/`max_by`.
- **Task tree:** `seq`/`par_all`/`par_race`/`repeat`/`forever`, leaves `step`/`once`/`act_for`/`wait`/`wait_until`/`wait_for`/`wait_for_from`, and failure nodes `check`/`sel`/`retry`/`invert`/`force_ok`/`force_fail`/`reactive_seq`/`reactive_sel`. Return the tree from `task(me)`; the kernel owns event delivery and there is one task progression path.
- **Testing** (`prelude/auto_tests.rhai`): `t_range` `t_max` `t_true` `t_rel` `t_present` `t_bounded` `t_moved` `report_verdict` `fail_fast` `seg` `find_or_none` `r2`/`r4`.

Add helpers freely — edit the prelude, no rebuild.

## 3a. Writing a scene TEST

A test scenario is an ordinary scenario whose last act is a verdict. Take the
assertions from `prelude/auto_tests.rhai` — do not paste private copies of
`r2`/`t_range`/`t_report` into a new test.

**Where it goes is what makes it a test**, and there is no name convention to
remember:

| | |
|---|---|
| `assets/scenes/tests/<name>.usda` | the rig |
| `assets/scenarios/tests/<name>.rhai` | its scenario |

`scripts/run_scene_tests.sh` runs everything in `scenes/tests/`, the Scene menu
hides it (`AssetVisibilitySettings`, one checkbox in Settings), and
`every_test_scene_carries_a_scenario` fails on any of them that asserts nothing.
A rig written into `scenes/luncosim/` instead gates nothing however carefully it
asserts — `no_test_scene_hides_outside_the_tests_directory` is the check that
says so. Do NOT suffix the file `_test`: the folder already said it.

A check returns `""` on pass and a MESSAGE on failure; collect them so every
check runs and the report names all of them:

```rhai
fn verdict(s) {
    let f = [];
    f.push(t_bounded(s.hull_pos, 100.0, "hull"));      // still a vehicle
    f.push(t_range(s.tilt, 0.0, 5.0, "tilt at rest (deg)"));
    f.push(t_moved(s.distance, 1.0, "rover travel"));  // and it actually drove
    report_verdict(f, "LANDING LEGS", "LANDING_LEGS"); // prints, emits, toasts
}
```

`report_verdict(fails, title, channel)` prints the greppable `<title>: PASS|FAIL`
line, emits the verdict on `channel` — which is what sets `luncosim test`'s exit
code — and raises a toast. Call it once, last. Use `fail_fast` for setup
failures (a `find` that returned -1, the wrong scene) so a broken run stops on
tick one instead of ticking silently to the limit.

For a tutorial, this scenario is an **observer**, not a second lesson. Attach it
to the same production scene fixture as the tutorial and observe its public
`cmd:*` events, mission verdict, and live state. Count the mechanism that
matters (`cmd:PossessVessel` plus a real port write, for example), then verify
the resulting movement or value. Never make the observer send the same control
commands as the lesson, and never accept `MISSION_COMPLETE` by itself.

This keeps tutorial regression tests in Rhai, where they can be edited and run
without rebuilding the Rust core:

```bash
target/debug/luncosim test \
  --scene scenes/tests/tutorial_first_drive.usda --max-ticks 6000
```

Scene loading and asynchronous Modelica participant readiness are bounded by
wall time, not update count. Use `--readiness-timeout SECS` when a machine needs
a different compile budget; the shell gate uses its separate `READINESS_TIMEOUT`
startup budget. The shell's larger `SCENE_TIMEOUT` wall-clock backstop allows
valid long-running missions to keep advancing, while `--max-ticks` remains the
simulated-time liveness bound after readiness. A readiness timeout is a
no-verdict failure and must be diagnosed at the worker/readiness owner, not
hidden by increasing an update-count constant.

The generic Rust contract may still compile every embedded script and exercise
the shared hook seam. Keep it content-agnostic; a lesson's steps, required
events, and expected command sequence belong in an authored Rhai observer.

**A silent pass is not a pass.** A scenario fails silently in every direction
that matters: a hook that never fires, a phase that never advances, a `find`
that missed. So assert that something was MEASURED (`t_present`) and that
something MOVED (`t_moved`, or `t_rel`'s both-near-zero rejection), and print a
per-sample table — a run with no sample rows proves nothing.

Run it headlessly:

```
target/debug/luncosim test \
    --scene scenes/tests/landing_legs.usda --max-ticks 500
```

Build `target/debug/luncosim` in the current worktree before the first gate.
Test commands consume that exact production build; they do not use `cargo run`.
For USD/Rhai-only iteration, reuse it without rebuilding:

```bash
./scripts/run_scene_tests.sh --no-build --exact <scene-name>
./scripts/run_scene_tests.sh --no-build -j 4 <scene-substring>
```

The runner defaults to four independent headless production processes.
`-j/--jobs N` changes only that process bound; each gate process still uses
`--threads 1 --jitter 0`, and graphics assertions run in their separate serial
offscreen pass. Use `-j 1` when diagnosing ordering or resource interactions.

For a standalone assertion against a running scene, use the live no-restart
wrapper:

```bash
./scripts/api/run_rhai_test.sh 4101 assets/scripting/tests/test_usd_query.rhai /SandboxScene/Box
```

The helper assembles the prelude and delegates to the native `luncosim rhai
--stdout` client, which sends the test through `RunRhai`; edit the `.rhai` file
and run it again in the same API session. Keep generic command/lifecycle/cache tests in
Rust, and move authored mission or vehicle outcomes into a discovered
`scenes/tests` + `scenarios/tests` pair.

### Keep test hooks below Rhai's expression-complexity ceiling

An authored test `on_tick` is a bounded fixed-step verdict hook, not a
replacement for the task/event machinery:

- one helper per phase;
- one sampler and one accumulator;
- a final verdict/report helper;
- short structured log rows instead of long concatenation expressions.

Hook-bound `this` is not available inside helpers. In the production host,
ordinary map arguments are passed by value and script-defined map helpers do not
resolve as mutable methods. Therefore test phase helpers should be reducers:
accept an explicit state map, return the updated map, and let the test observer
copy the returned keys into `this`. This both controls parser complexity and
makes phase logic independently testable.

### Choose the test runner from Rhai

The test observer owns its execution domain. Existing observers are headless by
default; a test whose assertion is rendered pixels, UI state, or a graphics
diagnostic declares the GPU-backed path with a top-level literal:

```rhai
const TEST_KIND = "graphics";
```

The scene still binds the observer through `LunCoProgramAPI` and
`info:sourceAsset`. The runner discovers that composed binding and reads the
literal without executing the script, so USD does not carry a second test-mode
field and the shell gate does not maintain an exception list. Valid values are
`"headless"` and `"graphics"`; omit the declaration for the deterministic
headless default.

## 3b. A rig test needs a CONTROL, and an anti-trivial guard

A comparative assertion is only as good as its ability to fail. Two traps, both
of which produce a confidently green test that measures nothing.

**The anti-trivial guard.** "The two sides mirror" is satisfied perfectly by a rig
that never moved: `0 ≈ -0` passes. So assert the driven side ACTUALLY MOVED before
asserting anything about the other one.

```rhai
f.push(t_true(s.peak_l > 0.02,
    "the driven rocker never moved — a rig at rest mirrors trivially, so " +
    "nothing below would mean anything"));
```

**The control case.** Ship a second scene with the mechanism DISABLED, and assert
it fails the same check. Without it, "coupled mirrors" might be measuring gravity,
symmetry, or nothing at all. `differential_rig{,_nodiff}` and
`rocker_bogie{,_nodiff}` are the worked pair.

**The control invariant must match the fixture.** A disabled or uncoupled control
case must have an explicit expected response; do not infer it from a simplified
stand or from a symmetric result. Derive the assertion from the mechanism's
purpose and declare the case through a parameter.

**Declare which case a stage is; never sniff it.** Both stages reference one rig
and differ only in whether the drive is live, so one scenario serves both — but it
must be TOLD which:

```usda
def Scope "Test" (prepend apiSchemas = ["LunCoProgramAPI"]) {
    uniform asset info:sourceAsset = @lunco://scenarios/tests/rocker_bogie.rhai@
    float lunco:param:coupled = 1.0        # read: param(me, "coupled", -1.0)
}
```

Reading it off the coupling's own stiffness would make the expectation depend on
the very authoring the test exists to check.

### Avoid weak test assertions

- **`t_rel(a, b, tol_pct, what)` takes a PERCENTAGE.** `0.2` means 0.2%, not 20%.
  Write the numeric tolerance and its percent meaning together.
- **Helper functions never see `this`.** `on_tick` has it; anything it calls does
  not. Pass every measurement through the verdict map — which is also what keeps
  the verdict a pure function of what was measured.
- **A guard that cannot run is not a guard.** Wrapping a check in
  `if s.x != () { ... }` makes it disappear when the port is absent. Wait for the
  port to exist, then measure; count the assertions in the final verdict.
- **Never do arithmetic on a possibly-absent reading.** `()` divided by 1000
  THROWS, the scenario dies between its last print and `report_verdict`, and
  `luncosim test` reports NO-VERDICT — the failure looks like a hang, not like the
  assertion that was about to fail. Route every logged number through a formatter
  that answers `"(none)"`.
- **A control must vary the quantity that actually gates.** Confirm the relevant
  live variable, peer set, or connection output changes before diagnosing the
  geometry or downstream behavior.
- **Keep the producer in the connection path.** A battery's `soc_out` belongs to
  the Battery prim. When another solver consumes it, author
  `float inputs:engine_enable.connect = </Rover/Battery.outputs:soc_out>` on the
  consumer. The generated network projection resolves that member output to its
  solver wrapper; do not invent a vessel-level SOC port or use a fallback value.
  A script may read an intentionally published network boundary, but that
  boundary must be an authored connection to the battery, never a second state.
- **A cut is not a camera loan.** `set_camera("RoverCam")` rebinds the viewport
  until another action rebinds it. Give every `wait_until` a bound and provide a
  return-to-avatar beat when the mission uses a cinematic camera.

## 4. Missions & sequencing (task policy, both pure rhai)

- **Layer 1 — task tree** (`examples/sequence.rhai`): build a tree with
  `step`/`wait`/`wait_for` and return it from `task(me)`. Action and predicate
  leaves are anonymous `|me| ...` closures; the native kernel owns progression,
  state binding, and event delivery.
- **Layer 2 — declarative timeline** (`examples/timeline.rhai`): a mission as
  **pure data**. Each step has exactly one operation word (`move_to`,
  `move_to_entity`, `possess`, `brake`, `cmd`, `emit`, `wait`, or `wait_event`)
  and only that operation's fields; `compile_timeline` lowers it inside
  `task(me)`. It is serialisable and can also be run through
  `RunTimeline`/`RunStoredTimeline`.

Progress is observable on the bus: `TASK_COMPLETE`/`TASK_FAILED` for the native
task root and `OBJECTIVE_COMPLETE`/`PLAN_COMPLETE` when mission policy emits
those application events.

For **complex reactive AI** (obstacle avoidance, interception) prefer the
Autopilot Behavior Tree (`cmd("SetAutopilotBehavior", #{vessel, spec_json})`,
see `docs/behaviour-trees.md`) over hand-rolled `on_tick` state machines.

## 5. Events — the reactive spine

`emit(name, value)` fires a `TelemetryEvent`; the target's `on_event` receives it
**one tick later** (deterministic actor model — "A emits, B reacts" is
order-independent). Scripts interact ONLY through events + shared ECS state,
never by calling each other's functions (isolated VMs). Producers also include
physics (`COLLISION_START`), lifecycle (`SCENE_LOADED`), and Modelica condition outputs
connected to `LunCoEvent.inputs:trigger`. The event prim adds the bus-facing name and
severity; threshold and hysteresis equations remain in Modelica.

## 6. Running & debugging

Prefer the HTTP API (curl-first; canonical port **4101** — launch per the
[`test-via-api`](../test-via-api/SKILL.md) / [`run-modelica`](../run-modelica/SKILL.md) skills):

```jsonc
// attach + run (idempotent hot-reload); source is inline rhai OR an asset path
{"type":"ExecuteCommand","command":"RunScenario","params":{"target":<gid>,"source":"<rhai or path>","params":"{\"speed\":1.5}"}}
{"type":"ExecuteCommand","command":"SetScenarioPaused","params":{"target":<gid>,"paused":true}}
{"type":"ExecuteCommand","command":"StopScenario","params":{"target":<gid>}}
```
- `params` is a JSON-object string; the script reads it as the read-only `params` constant.
- **Debug:** `ScriptStatus {target}` → compile/runtime health + located errors; `ScriptInspect {target}` → live `this`, hooks, generation, running/paused. `print(...)` goes to the process log.
- One-shot (no attach): `RunRhai {code}` — full world access, stdout in the original deferred response.

## 7. Persistence — bake into the scene (USD)

A script is a PRIM — give the entity a `LunCoProgramAPI` child and it auto-runs on spawn.
Delete the prim and the behaviour is gone:
```usda
def Xform "Rover_01"
{
    def Scope "Patrol" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @scenarios/patrol.rhai@
        # File-backed source is canonical for production programs.

        # per-instance config: one typed attribute per key, read by param(me, "speed", 1.0)
        custom float lunco:param:speed = 2.0
    }
}
```
Timelines persist via `RegisterTimeline` → `<twin>/timelines/*.json`; tool
libraries → `<twin>/tools/*.rhai`.

## The recipe (checklist)

1. Decide the shape: sequenced (`task`), objective-tracked (`mission`), or
   reactive (`on_event`). Use a Behavior Tree for reactive AI. Use `on_tick`
   only in authored tests for bounded state sampling and verdicts; rover
   continuous control/dynamics belong to native fixed-step systems or Modelica.
2. Return a task tree; pass task configuration into anonymous `|me| ...`
   closures. Keep persistent lifecycle state on `this` only where a hook or
   task closure genuinely needs it; named `Fn("...")` callbacks are not leaves.
3. Drive with prelude verbs (`nav_to`/`drive`/`cmd`) — never a control loop (that's Modelica).
4. Wire reactions through `emit`/`on_event` (remember the one-tick delay).
5. `RunScenario` on the target gid through the live API; verify with `ScriptInspect`; iterate by re-running (in-place hot-reload, no app restart).
6. Persist it as a `LunCoProgramAPI` child prim on the target once it works.

## Anti-patterns (each has cost real time)

- ❌ Persistent state in top-level `let` or read from a helper — invisible/unbound. Use `this`, in hooks only.
- ❌ A per-tick control law (PID, force mixing) in rhai — belongs in Modelica.
- ❌ `goto(...)` — reserved word; use `nav_to`.
- ❌ Expecting an `emit` to be seen the same tick — it arrives next tick.
- ❌ Assuming a scenario runs on clients — it's host-authoritative; clients get replicated state, not the script.
- ❌ A generic `spawn(...)` — use `cmd("SpawnEntity", #{entry_id, position})` so clients reconstruct from the catalog.
- ❌ Reading raw `Transform` for position — use `world_pos` (float-origin correct).
- ❌ Passing `Fn("named_helper")` to `once`/`step`/`wait_until` — task leaves
  require anonymous `|me| ...` closures so the native driver can bind `this`.

## The gate set — what the shipped scene tests guard

`./scripts/run_scene_tests.sh` builds `luncosim` once and runs every gate scene through `luncosim test`
headless and deterministically (`--threads 1 --jitter 0`) using four production
processes by default. `-j/--jobs N` changes the process bound; it does not
change the gate's deterministic flags. Exit 0=PASS / 1=FAIL / 2=no verdict.
The set, and what each one is FOR:

| Scene | Guards |
|---|---|
| `drivetrain_parity` · `ackermann_parity` · `six_independent_parity` | raycast ≡ physical for one authored parameter set (below) |
| `parts_attached` | **nothing falls off the vehicle.** Drive the assembled rig and require every descendant to remain within the authored relative-distance tolerance. |
| `lint_selftest` | **the linter itself.** A scene authored wrong on purpose, so `RunLint` → rules → `LintReport` can be shown to FIND the faults by rule id — and to stay silent on the correctly jointed wheel beside them |

Two lessons those last two encode, worth copying into any new gate:

- **Measure something rotation-invariant.** `parts_attached` compares
  `|p_part − p_vessel|` before and after a drive: a spinning wheel, a steering
  knuckle and a stroking suspension all leave it alone, while a part left on the
  ground changes it by the length of the drive. It walks `children()`, so it
  needs no list of part names and covers parts added later.
- **Prove the measurement can fail.** Each of these asserts its subject actually
  MOVED (or that a deliberate fault was actually FOUND). A vessel that never
  simulates, a hook that never fires and a clean scene are indistinguishable
  otherwise — `parts_attached` excludes rucheyok for exactly that reason rather
  than counting a frozen rover as a pass.

## Comparative mechanics tests

When two authored realizations implement one contract, place them in one scene
test and drive them with identical commands. Assert that both realizations move,
compare physical outputs with tolerances appropriate to the contract, and check
direction as well as magnitude. Add an independent bound so two equally wrong
implementations cannot satisfy parity together.

The shipped drivetrain, attachment, and linter fixtures under
`assets/scenes/tests/` and `assets/scenarios/tests/` demonstrate this shape. Keep
the test scenario responsible for measured samples and the verdict; keep the
mechanism and its parameters in USD, Modelica, or the owning engine subsystem.
