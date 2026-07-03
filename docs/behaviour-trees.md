# Behaviour Trees

How autonomous behaviour is structured in LunCoSim — the reusable **mechanism**
(a small, engine-agnostic tree kernel) and the **policy** authored as data (a
JSON/rhai `BehaviorSpec` an autopilot compiles and can hot-swap at runtime).

- **Kernel crate:** [`lunco-behavior`](../crates/lunco-behavior) — `Status`, the
  `Node` trait, composites, reactive composites, decorators, the `Action` leaf.
  No dependency on bevy, avian, rhai, or python.
- **Consumer:** [`lunco-autopilot`](../crates/lunco-autopilot) — supplies a
  `DriveCtx` (vessel pose in, `throttle`/`steer`/`brake` out, mission clock) and a
  data schema (`BehaviorSpec`) that names Rust nav-math leaves. Trees are authored
  as data so rhai/JSON can define them and swap them live (`SetAutopilotBehavior`).
- **Related:** [scripting-guide.md](./scripting-guide.md) (the rhai task sequencer,
  a sibling cooperative model), spec
  [034-control-authority-arbiter](../specs/034-control-authority-arbiter).

---

## 1. Why a behaviour tree

Projects like ours drive **autonomous rovers** (skid/ackermann), **spacecraft GNC**
(landers, sun-trackers), and **mission/ConOps sequencing**, and teach them through
**tutorials**. Those all want the same thing: reactive, composable, inspectable
autonomy that a designer can author and change without a rebuild. A behaviour tree
gives that — small nodes with one `tick → {Running, Success, Failure}` contract,
composed into larger behaviour, re-evaluated every fixed tick.

Two design rules the codebase holds to:

1. **Math in Rust, structure as data.** The steering/nav computation lives in Rust
   leaves (`nav_setpoint`); the *shape* of the tree (which waypoints, when to brake,
   what to fall back to) is JSON/rhai data. So behaviour is dynamic and
   hot-swappable, but the numerics stay fast and testable.
2. **The kernel is clock-free.** `lunco-behavior` has no notion of time, world, or
   engine. Anything that needs a clock (a timeout), a pose, or sensors lives in the
   consuming layer that owns a context — e.g. the autopilot's `Timeout` reads
   `DriveCtx.now` (the mission clock, so it freezes under pause/warp).

---

## 2. The tick contract

Every node implements:

```rust
fn tick(&mut self, ctx: &mut Ctx) -> Status;   // Running | Success | Failure
fn reset(&mut self) {}                          // parent restarts me → go fresh
```

Composites reset themselves on a terminal result, so a finished subtree is fresh
when re-entered (e.g. under a loop). Reactive composites additionally reset the
children they *skipped* this tick, so a preempted branch never carries stale state.

---

## 3. Node catalogue

Authored as JSON internally tagged by `kind` (snake_case). This is the full set the
autopilot compiles today.

### Composites

| `kind` | Params | Semantics |
|---|---|---|
| `sequence` | `children` | Run in order; fail on first failure; succeed when all succeed. Latches the running child. |
| `selector` | `children` | Fallback: first child that doesn't fail; fail only if all fail. Latches the running child. |
| `parallel` | `require` (`all`\|`one`), `children` | Tick every child each tick; `all` = succeed when all do (fail on any); `one` = succeed as soon as any does. "Do X while monitoring Y." |
| `reactive_sequence` | `children` | Like `sequence` but re-ticks **from the first child every tick** — guards stay live. "Do B **while** A holds." |
| `reactive_selector` | `children` | Like `selector` but re-ticks from the highest-priority child every tick — a higher option preempts a lower one mid-run. The priority arbiter. |

> **Reactive vs. plain is the key distinction.** A plain `selector([at_goal?→brake,
> drive])` latches `drive` as Running and never re-checks `at_goal?`. The
> `reactive_selector` re-checks the guard every frame and switches to `brake` the
> instant it trips. Use reactive when a guard must keep holding for the action to
> continue; use plain when a step, once started, should run to completion.

### Loops

| `kind` | Params | Semantics |
|---|---|---|
| `forever` | `child` | Repeat the child forever; only a child failure ends it. |
| `repeat` | `times`, `child` | Repeat until the child has **succeeded** `times` times. |
| `retry` | `times`, `child` | Re-attempt on **failure** up to `times` times, then give up (`Failure`); a child success ends it early. The failure-side mirror of `repeat` — re-try a flaky maneuver. |

### Decorators (single-child wrappers)

| `kind` | Params | Semantics |
|---|---|---|
| `invert` | `child` | Swap `Success` ↔ `Failure` (`Running` passes through). Turn a condition into its negation. |
| `force_success` | `child` | Map any terminal to `Success` — a best-effort step that must never fail its parent. |
| `force_failure` | `child` | Map any terminal to `Failure` — force an abort. |
| `timeout` | `seconds`, `child` | Abort with `Failure` (and brake) if the child stays `Running` past `seconds` of **mission time**. The watchdog. Lives in the autopilot because it needs the clock. |
| `cooldown` | `seconds`, `child` | After the child `Success`es, block re-entry (`Failure`) for `seconds` of mission time — rate-limit a one-shot action so it can't re-fire every tick. |

### Navigation & action leaves (write the `throttle`/`steer`/`brake` setpoint)

| `kind` | Params | Semantics |
|---|---|---|
| `drive_to` | `target`, `speed`, `radius` | Steer toward a world point; `Success` (and brake) within `radius`. |
| `follow` | `target` (GlobalEntityId), `speed`, `radius` | Track a **moving** entity: steer toward its *live* pose each tick, hold station within `radius`. Never finishes — stays `Running` while the target resolves, `Failure` (braking) if it vanishes so a fallback takes over. |
| `intercept` | `target` (GlobalEntityId), `speed`, `radius`, `lead` | Lead-pursuit: aim `lead` seconds *ahead* of the target along its velocity (cut it off, don't tail it); `Success` on contact (within `radius` of its actual pose), `Failure` (braking) if it vanishes. A catch-it pursuit that **finishes**, unlike `follow`. |
| `patrol` | `waypoints`, `speed`, `radius`, `dwell` | Loop waypoints forever; optionally dwell (braked) `dwell` s at each. Sugar for `forever(sequence([drive_to, wait?]…))`. |
| `face` | `target`, `tolerance` (deg) | Pivot in place (steer only, no throttle) to face the target; `Success` when within `tolerance`. Aim before driving, point an instrument. |
| `cruise` | `throttle`, `steer` | Hold a constant setpoint; always `Running`. |
| `brake` | — | Full brake; `Success`. |
| `hold` | — | Full brake but **never finishes** (`Running`) — a "stay put" action, e.g. under a `parallel` while a guard holds. |
| `wait` | `seconds` | Hold (braked) for `seconds` of mission time, then `Success`. Re-arms each lap under a loop (frozen clock ⇒ frozen wait). |

### Condition & scaffolding leaves (read-only / constant)

| `kind` | Params | Semantics |
|---|---|---|
| `arrived` | `target`, `radius` | `Success` within `radius` of the point, else `Failure`. Writes no setpoint — the guard that makes selectors meaningful. |
| `facing` | `target`, `tolerance` (deg) | `Success` if the heading is within `tolerance` of the target, else `Failure`. The read-only guard counterpart to `face`. |
| `obstacle_ahead` | `distance`, `cone` (deg) | `Success` if another known **vessel** is within `distance` in a forward cone of `cone` degrees (self excluded), else `Failure`. Vessel-vs-vessel proximity — no terrain/collider raycast (see roadmap). |
| `succeed` | — | Always `Success` — no-op / placeholder. |
| `fail` | — | Always `Failure` — placeholder / forced-failure branch. |

---

## 4. Worked examples

**Patrol a square, pausing at each corner** (one node):

```json
{"kind":"patrol","speed":0.7,"radius":3.0,"dwell":1.0,
 "waypoints":[[10,0,0],[10,0,10],[0,0,10],[0,0,0]]}
```

**Drive to a goal, but keep braking the instant you arrive** — reactive fallback:

```json
{"kind":"reactive_selector","children":[
  {"kind":"sequence","children":[
    {"kind":"arrived","target":[14,0,8],"radius":2.0},
    {"kind":"brake"}]},
  {"kind":"drive_to","target":[14,0,8],"speed":0.5}]}
```

**Attempt a drive for 30 s, else fall back to a safe pose** — watchdog + fallback:

```json
{"kind":"selector","children":[
  {"kind":"timeout","seconds":30,
   "child":{"kind":"drive_to","target":[100,0,0]}},
  {"kind":"brake"}]}
```

**Aim, then go** — sequence a pivot with a drive:

```json
{"kind":"sequence","children":[
  {"kind":"face","target":[50,0,50],"tolerance":5},
  {"kind":"drive_to","target":[50,0,50],"speed":0.6}]}
```

**Escort a moving vehicle, else patrol** — track a mover, fall back if it's gone:

```json
{"kind":"reactive_selector","children":[
  {"kind":"follow","target":4869542932533563,"speed":0.7,"radius":6.0},
  {"kind":"patrol","waypoints":[[0,0,0],[20,0,0]],"speed":0.5}]}
```

`target` is the leader's GlobalEntityId (api_id). From rhai, interpolate it:
`"{\"kind\":\"follow\",\"target\":" + find("/Convoy/Leader") + "}"`.

**Chase down a fleeing rover** — lead-pursuit that finishes on contact:

```json
{"kind":"intercept","target":4869542932533563,"speed":0.9,"radius":3.0,"lead":1.5}
```

**Drive to a goal, but stop for traffic** — reactive obstacle guard:

```json
{"kind":"reactive_selector","children":[
  {"kind":"sequence","children":[
    {"kind":"obstacle_ahead","distance":6,"cone":50},
    {"kind":"hold"}]},
  {"kind":"drive_to","target":[80,0,0],"speed":0.7}]}
```

Re-checked every tick: it holds while a vessel is in the way and resumes driving the
instant the path clears.

From rhai a scenario emits the same data and hot-swaps it live:

```rhai
cmd("SetAutopilotBehavior", #{ vessel: rover, spec_json: "{...}" });
```

---

## 5. Roadmap — nodes deferred until `DriveCtx` grows

The leaf vocabulary is bounded by what the context exposes. `DriveCtx` today gives
each leaf the **own** vessel pose + id (`self_gid`), the mission clock, the setpoint
out, and a snapshot of **other entities' live kinematic state** (`targets`: position
+ a finite-difference velocity, keyed by GlobalEntityId — what `follow`, `intercept`
and `obstacle_ahead` read). The following need still more, and each is deferred
because it crosses an **architectural boundary**, not because it's hard — doing it
half-way would be worse than not yet:

- **Terrain / collider raycast** (obstacle avoidance vs. the ground and static
  geometry, beyond the vessel-vs-vessel `obstacle_ahead`) — needs a spatial-query
  provider (avian `SpatialQuery` / `lunco-mobility`). The headless autopilot
  deliberately pulls in **no physics dep**; adding one is the boundary to cross.
- **Resource guards** (`battery_above`, `fuel_above`, thermal limits) — the
  `PortRegistry::read_port` API needs `&World`, which would force `drive_autopilots`
  to become an **exclusive system**. Wants a per-tick port-value snapshot resource
  first.
- **`wait_for_event` / signal conditions** — the telemetry bus mixes buffered
  messages and observer triggers; feeding it deterministically into a fixed-tick
  tree wants a dedicated per-tick event snapshot. (The rhai task sequencer already
  consumes events via its own path; the tree does not yet.)
- **Utility / priority-scoring selector** — a selector that *scores* children and
  runs the best needs a **score-returning node model**, i.e. a change to the tick
  contract (nodes return `Status`, not a number) — a design change, not an additive
  `Node`.
- **Named / reusable subtrees** — `{"kind":"ref","name":…}` + a `defs` block is a
  change to the **top-level spec schema**; worth its own revision.

When adding a leaf that reads new world state, extend `DriveCtx` (not the kernel),
keep the computation in Rust, and expose only the *structure* as a `BehaviorSpec`
variant — the same split that keeps behaviour dynamic and the numerics fast.
