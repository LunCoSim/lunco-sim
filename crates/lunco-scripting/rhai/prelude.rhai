// LunCo scripting prelude — ergonomic wrappers over the three host verbs.
//
// The host (Rust) exposes only the minimal, generic bridge:
//   cmd(name, #{params})          write: fire any #[Command] by name; returns
//                                        #{ id, ok, data, error } — `data` is the
//                                        handler's assigned bag (spawned gid, …),
//                                        so you can create-then-manipulate
//   query(name, #{params})        read:  call any ApiQueryProvider (Raycast,
//                                        Nearest, GroundHeight, …) — the read
//                                        twin of cmd(); physics stays in its
//                                        owning crate, not in scripting
//   world_pos(id) -> [x,y,z]      read:  float-origin-correct world position
//   world_forward(id) -> [x,y,z]  read:  world heading
//   get(id, "Comp.field")         read:  reflected field; vectors/quats come
//                                        back as arrays ([x,y,z]/[x,y,z,w]),
//                                        structs as maps
//   list_entities() -> [#{id,name,type,pos}]  (type+pos enable in-rhai selection)
//   find(name) -> id              lookup: id of the entity named `name` (-1 if none)
//   name(id) -> string            lookup: reverse of find — id back to its Name, or ()
//   parent(id) -> id              tree:   parent's id, or () if none/unregistered
//   children(id) -> [id, ...]     tree:   ids of direct registered children
//   sim_tick() -> i64             time:  current fixed tick (integer)
//   dt() -> f64                   time:  fixed-step delta in seconds (1/60)
//   elapsed_seconds() -> f64      time:  monotonic sim seconds since startup
//   emit(name, value)             event: fire a TelemetryEvent
//   on_event(self, evt)           event: receives emit()s AND physics
//                                        COLLISION_START / COLLISION_END pulses
//
// Everything below is POLICY, authored here (hot-reloadable), never compiled
// into core. Add helpers freely — no Rust rebuild required.

// ── Math on the [x,y,z] arrays world_pos returns ───────────────────────────

fn vsub(a, b) { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn vadd(a, b) { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
fn vlen(a)    { (a[0]*a[0] + a[1]*a[1] + a[2]*a[2]).sqrt() }
fn vdot(a, b) { a[0]*b[0] + a[1]*b[1] + a[2]*b[2] }
fn vcross(a, b) {
    [ a[1]*b[2] - a[2]*b[1],
      a[2]*b[0] - a[0]*b[2],
      a[0]*b[1] - a[1]*b[0] ]
}
fn vnorm(a) {
    let l = vlen(a);
    if l <= 0.0 { a } else { [a[0]/l, a[1]/l, a[2]/l] }
}
fn clamp(x, lo, hi) { if x < lo { lo } else if x > hi { hi } else { x } }

/// Distance between two entities in metres, or -1.0 if either lacks a position.
fn distance(a, b) {
    let pa = world_pos(a);
    let pb = world_pos(b);
    if pa == () || pb == () { return -1.0; }
    vlen(vsub(pa, pb))
}

/// True once `entity` is within `radius` metres of world point `target` ([x,y,z]).
fn arrived(entity, target, radius) {
    let p = world_pos(entity);
    if p == () { return false; }
    vlen(vsub(p, target)) <= radius
}

// ── Navigation (PURE rhai policy over the world_pos/world_forward reads) ─────
//
// All steering math lives here — Rust only exposes the raw position/heading
// reads + the drive() SetPorts command. Tune or replace freely; no Rust rebuild.

/// Signed steer command in [-1, 1] that turns `entity` toward world point
/// `target`. Sign from the yaw-plane (up = +Y) cross product of heading × goal.
fn steer_to(entity, target) {
    let p = world_pos(entity);
    let f = world_forward(entity);
    if p == () || f == () { return 0.0; }
    let to  = vnorm(vsub(target, p));
    let fwd = vnorm(f);
    let cross = vcross(fwd, to);
    clamp(cross[1], -1.0, 1.0)   // +Y component = left/right yaw error
}

/// Drive `entity` toward world point `target` at `speed`. Returns true once
/// within `radius` (and brakes). The core per-tick navigation primitive.
/// (Named `nav_to`, not `goto` — `goto` is a reserved word in rhai.)
fn nav_to(entity, target, speed, radius) {
    if arrived(entity, target, radius) {
        brake(entity);
        return true;
    }
    drive(entity, speed, steer_to(entity, target));
    false
}

// ── Declarative plan / objectives (PURE rhai — objectives live here, not core) ─
//
// A "plan" is just data: an array of world waypoints `[[x,y,z], ...]`. run_plan
// advances one objective at a time, emitting OBJECTIVE_COMPLETE per waypoint and
// PLAN_COMPLETE at the end. The step index IS the resumable state (no coroutines
// needed). IMPORTANT rhai rule: `this` is bound ONLY in the hook that the engine
// calls (on_tick), NOT in functions the hook calls. So run_plan is STATELESS —
// it takes the current index and returns the next; the hook holds the index on
// `this`. Richer objectives (sample, dwell, wait-for-event) compose the same way.

/// Advance a waypoint plan by one tick. `idx` is the current objective index;
/// returns the next index (incremented when the current waypoint is reached).
/// When `idx >= plan.len()` the plan is complete. Typical use in on_tick:
///   `this.idx = run_plan(me, PLAN, this.idx, SPEED, RADIUS);`
fn run_plan(entity, plan, idx, speed, radius) {
    if idx >= plan.len() { return idx; }
    let wp = plan[idx];
    if nav_to(entity, wp, speed, radius) {
        emit("OBJECTIVE_COMPLETE", idx);
        idx += 1;
        if idx >= plan.len() { emit("PLAN_COMPLETE", true); }
    }
    idx
}

// ── Sequencer (Layer 1: time/sequence primitives — PURE rhai policy) ─────────
//
// A general step sequencer over the same "thread state through `this`" rule as
// run_plan (the engine binds `this` ONLY inside the hook, not in called fns), so
// every primitive is STATELESS: it takes a cursor and returns the next cursor.
// The hook owns the cursor on `this`.
//
// A STEP is a map; a SEQUENCE is an array of steps. Step shapes (use the
// constructors below, or write the map literal directly):
//   #{ act: |m| ..., done: |m| ... }   run `act` every tick; advance when `done`
//   #{ act: |m| ... }                  one-shot: run `act` once, then advance
//   #{ secs: 5.0 }                     dwell N seconds, then advance
//   #{ done: |m| ... }                 wait until predicate true (no action)
//   #{ event: "NAME" }                 wait until a matching event arrives
// (`act`/`done`/`secs`/`event` avoid rhai's reserved `do`/`until`.)
//
// Events arrive in on_event, not on_tick, so wait-for-event needs the hook to
// feed them in. The 3-hook shell (`me` is the host gid; state lives on `this`):
//   fn on_start(me)      { this.cur = seq_init(); }
//   fn on_tick(me)       { this.cur = run_steps(me, STEPS, this.cur); }
//   fn on_event(me, evt) { this.cur = seq_note_event(this.cur, evt); }
//
// Progress is observable on the telemetry bus: STEP_COMPLETE(idx) per step and
// SEQUENCE_COMPLETE(len) at the end (mirrors run_plan's OBJECTIVE/PLAN emits).

/// Fresh sequencer cursor. `entered_at` is () until the first tick stamps it
/// (so dwell timing works regardless of when the sequence starts).
fn seq_init() { #{ step: 0, entered_at: (), events: [] } }

/// Step constructors (sugar over the map literals; all optional to use).
fn step(act, done) { #{ act: act, done: done } }   // closed-loop: act until done
fn once(act)       { #{ act: act } }               // run once, then advance
fn wait(secs)      { #{ secs: secs } }             // dwell `secs` seconds
fn wait_until(done){ #{ done: done } }             // hold until predicate true
fn wait_for(event) { #{ event: event } }           // hold until event `name` fires (any source)
fn wait_for_from(event, src) { #{ event: event, src: src } } // ...from a SPECIFIC emitter gid

/// True when the current step `s` is complete. Precedence: explicit `done`
/// predicate, else `secs` dwell, else `event` match, else one-shot (advance now).
fn step_done(me, s, cur, now) {
    if s.done != ()  { return s.done.call(me); }
    if s.secs != ()  { return (now - cur.entered_at) >= s.secs; }
    if s.event != () { return cur.events.some(|n| n == s.event); }
    true
}

/// Advance a step sequence by one tick. Runs the current step's `act` (if any),
/// then advances when the step is done. Returns the next cursor; when
/// `cur.step >= steps.len()` the sequence is complete (subsequent calls no-op).
/// Typical use in on_tick: `self.cur = run_steps(me, STEPS, self.cur);`
fn run_steps(me, steps, cur) {
    if cur.step >= steps.len() { return cur; }
    let now = elapsed_seconds();
    if cur.entered_at == () { cur.entered_at = now; }   // lazy stamp on entry
    let s = steps[cur.step];
    if s.act != () { s.act.call(me); }
    if step_done(me, s, cur, now) {
        emit("STEP_COMPLETE", cur.step);
        cur.step += 1;
        cur.entered_at = now;     // stamp the next step's entry time
        cur.events = [];          // events are per-step; clear on advance
        if cur.step >= steps.len() { emit("SEQUENCE_COMPLETE", cur.step); }
    }
    cur
}

/// Feed an on_event payload into the cursor's per-step event buffer so that
/// `wait_for(name)` steps can complete. Call from on_event. Returns the cursor.
fn seq_note_event(cur, evt) {
    if evt.name != () { cur.events.push(evt.name); }
    cur
}

// ── Tasks: a BUILT-IN auto-advanced sequence (no on_tick/on_event needed) ─────
//
// The sequencer above needs three hooks wired by hand (on_start seeds the cursor,
// on_tick calls run_steps, on_event calls seq_note_event). A *task* removes all
// of that: declare it ONCE by assigning `this.task` in on_start, and the engine
// advances it every tick and feeds it events automatically — the runtime calls
// the `__run_task` / `__note_task_event` drivers below for you. This is the
// built-in "do X, wait until Y, then Z" capability available to every script:
//
//   fn on_start(me) {
//       this.task = seq([
//           once(|m| drive_to(m, CP1)),         // run an action, advance
//           wait_until(|m| arrived(m, CP1)),    // suspend until true
//           wait(3.0),                          // suspend 3 sim-seconds
//           wait_for("GO"),                     // suspend until event "GO"
//           once(|m| drive_to(m, CP2)),
//       ]);
//   }
//   // no on_tick, no on_event — the task drives itself. Step closures take the
//   // host gid `m` (the cursor calls them as `act.call(me)`).
//
// rhai has no real coroutines (and wasm can't stack-switch), so a task is a
// COOPERATIVE cursor advanced once per tick, not a suspended call stack — the
// right model for a deterministic, replicated sim. Steps reuse the Layer-1
// vocabulary (once/wait/wait_until/wait_for/step). A user on_tick/on_event still
// runs if defined (for logic beyond the task).

// A task is a TREE of nodes. Leaves are the Layer-1 step maps (once/wait/
// wait_until/wait_for/step); composites nest other nodes:
//   seq([a, b, c])      run in order, advance when each is done
//   par_all([a, b])     run concurrently, done when ALL are done
//   par_race([a, b])    run concurrently, done when ANY is done
//   repeat(n, node)     run `node` to completion `n` times
//   forever(node)       run `node` to completion, over and over (never done)
// e.g. patrol two legs forever, braking 2s at each:
//   this.task = forever(seq([
//       once(|m| drive_to(m, A)), wait_until(|m| arrived(m, A)), wait(2.0),
//       once(|m| drive_to(m, B)), wait_until(|m| arrived(m, B)), wait(2.0),
//   ]));
// The engine ticks the tree each frame (__tick) and feeds events into it.

/// Composite node constructors (assign the root to `this.task` in on_start).
fn seq(items)      { #{ k: "seq",  items: items, i: 0 } }
fn par_all(items)  { #{ k: "all",  items: items, fin: items.map(|x| false) } }
fn par_race(items) { #{ k: "race", items: items } }
fn repeat(n, body) { #{ k: "repeat", n: n, body: body, cur: body, done_n: 0 } }
fn forever(body)   { #{ k: "forever", body: body, cur: body } }

/// True if any event in `ev` matches `evt_name` and (when `want != ()`) was
/// emitted by source `want`. Hoisted to its own top-level function so the closure
/// doesn't nest past rhai's stock expr-depth cap inside `__tick_leaf`'s if/else
/// chain (same reason the composite kinds below are each their own helper).
fn __event_matches(ev, evt_name, want) {
    ev.some(|e| {
        let name_ok = e.name == evt_name;
        let src_ok = want == () || e.source == want;
        name_ok && src_ok
    })
}

/// Tick ONE leaf step `s` (a once/wait/wait_until/wait_for/step map), tracking its
/// own dwell start on `s._t0`. `ev` is this tick's event-name buffer. Returns
/// `#{ n: <updated leaf>, d: <done?> }`. (Mirrors run_steps' step_done precedence.)
fn __tick_leaf(me, s, now, ev) {
    if s.act != () { s.act.call(me); }            // closed-loop action (once/step)
    let d = true;                                  // bare/once → advance immediately
    if s.done != () { d = s.done.call(me); }       // wait_until / step predicate
    else if s.secs != () { if s._t0 == () { s._t0 = now; } d = (now - s._t0) >= s.secs; }
    // wait_for: match event name, and (if wait_for_from gave a `src`) the EMITTER
    // too — so one name from many sensors is distinguished by source. `src` may be
    // a gid OR a path string resolved LAZILY here (entities may not exist yet when
    // the task tree is built at on_start).
    else if s.event != () {
        let want = s.src;
        if want != () && type_of(want) == "string" { want = find(want); }
        d = __event_matches(ev, s.event, want);
    }
    #{ n: s, d: d }
}

// Tick one task node (recursive). Each composite kind is a separate helper so no
// single function nests past rhai's default expr-depth cap — the prelude parses
// on a stock `Engine` (catalog / one-shot backend), not just the scenario engine.
// Each returns `#{ n: <updated node>, d: <done?> }`.

/// Dispatch by node kind; a node with no `k` is a leaf step.
fn __tick(me, node, now, ev) {
    let k = node.k;
    if k == "seq"     { return __tick_seq(me, node, now, ev); }
    if k == "all"     { return __tick_all(me, node, now, ev); }
    if k == "race"    { return __tick_race(me, node, now, ev); }
    if k == "repeat"  { return __tick_repeat(me, node, now, ev); }
    if k == "forever" { return __tick_forever(me, node, now, ev); }
    __tick_leaf(me, node, now, ev)
}

fn __tick_seq(me, node, now, ev) {
    let n = node.items.len();
    if node.i >= n { return #{ n: node, d: true }; }
    let item = node.items[node.i];
    let r = __tick(me, item, now, ev);
    node.items[node.i] = r.n;
    if r.d { node.i += 1; }
    #{ n: node, d: node.i >= n }
}

fn __tick_all(me, node, now, ev) {
    let all_done = true;
    let n = node.items.len();
    for j in 0..n {
        if !node.fin[j] {
            let item = node.items[j];
            let r = __tick(me, item, now, ev);
            node.items[j] = r.n;
            node.fin[j] = r.d;
        }
        if !node.fin[j] { all_done = false; }
    }
    #{ n: node, d: all_done }
}

fn __tick_race(me, node, now, ev) {
    let any_done = false;
    let n = node.items.len();
    for j in 0..n {
        let item = node.items[j];
        let r = __tick(me, item, now, ev);
        node.items[j] = r.n;
        if r.d { any_done = true; }
    }
    #{ n: node, d: any_done }
}

fn __tick_repeat(me, node, now, ev) {
    if node.done_n >= node.n { return #{ n: node, d: true }; }
    let r = __tick(me, node.cur, now, ev);
    node.cur = r.n;
    if r.d { node.done_n += 1; node.cur = node.body; }   // reset to a fresh body
    #{ n: node, d: node.done_n >= node.n }
}

fn __tick_forever(me, node, now, ev) {
    let r = __tick(me, node.cur, now, ev);
    node.cur = r.n;
    if r.d { node.cur = node.body; }
    #{ n: node, d: false }
}

/// Engine driver — advance `this.task` one tick against this frame's event
/// buffer, then clear it (events live one tick, the actor-model cadence). Called
/// by the runtime after on_tick; no-op when no task was declared. Emits
/// TASK_COMPLETE once when the root finishes.
fn __run_task(me) {
    if this.task == () { return; }
    let ev = if this.__events == () { [] } else { this.__events };
    let r = __tick(me, this.task, elapsed_seconds(), ev);
    this.task = r.n;
    this.__events = [];
    if r.d && this.__task_done != true { this.__task_done = true; emit("TASK_COMPLETE", 0); }
}

/// Engine driver — buffer an event name for `this.task`'s `wait_for` leaves to
/// consume on the next advance. Called by the runtime per delivered event.
fn __note_task_event(me, evt) {
    if this.task == () { return; }
    if this.__events == () { this.__events = []; }
    // Buffer name + emitter source so `wait_for` (name) and `wait_for_from`
    // (name + source) can both match against this tick's events.
    if evt.name != () { this.__events.push(#{ name: evt.name, source: evt.source }); }
}

/// SUGAR — define a `task(me)` function returning a node and the engine runs it
/// automatically: no on_start, no `this.task = …`. The whole scenario can be:
///
///   fn task(me) {
///       seq([ once(|m| drive_to(m, CP1)), wait_until(|m| arrived(m, CP1)), wait(3.0) ])
///   }
///
/// Engine driver — called once after on_start; seeds `this.task` from a user
/// `task(me)` if defined and not already set (an explicit `this.task =` wins).
fn __init_task(me) {
    if this.task == () && is_def_fn("task", 1) { this.task = task(me); }
}

// ── Objectives: a DECLARATIVE mission (KSP-contract style) ────────────────────
//
// A task DOES things; a mission WATCHES — a set of objectives, each a completion
// condition (not imperative steps), evaluated every tick. Objectives can require
// others (branching/unlock), dwell (hold the condition N seconds), fail on a
// condition, and fire on_complete. The engine emits OBJECTIVE_COMPLETE(id) /
// OBJECTIVE_FAILED(id) and MISSION_COMPLETE / MISSION_FAILED. Declare by defining
// `fn mission(me)` (sugar, auto-run) or assigning `this.mission` in on_start:
//
//   fn mission(me) {
//       [
//           objective("reach_base", #{ done: |m| arrived(m, BASE), dwell: 3.0,
//                                      on_complete: |m| emit("AT_BASE", 1) }),
//           objective("sample", #{ requires: ["reach_base"],
//                                  done: |m| has_sample(m),
//                                  fail: |m| get(m, "Battery.level") < 0.1 }),
//       ]
//   }
// A scenario may run a `task` (behaviour) AND a `mission` (success tracking).

/// One objective. `spec` keys (all optional but usually `done`): `done` (|m| bool
/// completion), `dwell` (secs the condition must hold), `fail` (|m| bool failure),
/// `requires` (array of objective ids that must complete first), `on_complete`
/// (|m| side-effect when it completes).
fn objective(id, spec) {
    #{
        id: id,
        done: spec.done,
        dwell: spec.dwell,
        fail: spec.fail,
        requires: if spec.requires != () { spec.requires } else { [] },
        on_complete: spec.on_complete,
        state: "pending",   // pending → active → complete | failed
        t0: (),             // dwell entry time
    }
}

/// State of objective `id` within mission list `m`, or () if absent.
fn __obj_state(m, id) {
    for o in m { if o.id == id { return o.state; } }
    ()
}

/// True once every id in `requires` is complete.
fn __obj_ready(m, requires) {
    for rid in requires { if __obj_state(m, rid) != "complete" { return false; } }
    true
}

/// Advance one ACTIVE objective: fail check → completion (with optional dwell).
fn __advance_objective(me, o, now) {
    if o.fail != () && o.fail.call(me) {
        o.state = "failed";
        emit("OBJECTIVE_FAILED", o.id);
        return o;
    }
    let met = o.done == () || o.done.call(me);
    if !met { o.t0 = (); return o; }          // condition broke → reset dwell
    if o.dwell != () {
        if o.t0 == () { o.t0 = now; }
        if (now - o.t0) < o.dwell { return o; }
    }
    o.state = "complete";
    emit("OBJECTIVE_COMPLETE", o.id);
    if o.on_complete != () { o.on_complete.call(me); }
    o
}

/// Tick one objective: unlock from pending when its requires are met, then
/// advance if active. Returns the updated objective.
fn __tick_objective(me, o, m, now) {
    if o.state == "complete" || o.state == "failed" { return o; }
    if o.state == "pending" && __obj_ready(m, o.requires) { o.state = "active"; }
    if o.state == "active" { o = __advance_objective(me, o, now); }
    o
}

/// Mission roll-up as `#{ all_done, any_failed }`. PURE (no `this`): the runtime
/// binds `this` only in the directly-called driver, not in functions it calls, so
/// the MISSION_COMPLETE/FAILED emit + one-shot guard live in `__run_mission`.
fn __mission_status(m) {
    let all_done = true;
    let any_failed = false;
    for o in m {
        if o.state == "failed" { any_failed = true; }
        if o.state != "complete" { all_done = false; }
    }
    #{ all_done: all_done, any_failed: any_failed }
}

/// Engine driver — evaluate `this.mission` one tick. Called by the runtime after
/// on_tick; no-op when no mission was declared. Emits MISSION_COMPLETE /
/// MISSION_FAILED once when the mission resolves.
fn __run_mission(me) {
    if this.mission == () { return; }
    let now = elapsed_seconds();
    let m = this.mission;
    for i in 0..m.len() {
        m[i] = __tick_objective(me, m[i], m, now);
    }
    this.mission = m;
    let s = __mission_status(m);
    if this.__mission_done != true {
        if s.any_failed { this.__mission_done = true; emit("MISSION_FAILED", 0); }
        else if s.all_done { this.__mission_done = true; emit("MISSION_COMPLETE", 0); }
    }
}

/// Engine driver — called once after on_start; seeds `this.mission` from a user
/// `mission(me)` if defined and not already set.
fn __init_mission(me) {
    if this.mission == () && is_def_fn("mission", 1) { this.mission = mission(me); }
}

// ── Timeline (Layer 2: a mission as DATA, interpreted onto Layer 1) ──────────
//
// Layer 1 steps carry rhai CLOSURES; a TIMELINE step is pure DATA (a map of
// fields), so a whole mission is serialisable — authorable as a `const`, stored
// in the Twin, or driven over the API by `RunTimeline` (which generates the
// generic executor below). `compile_timeline` lowers data steps to Layer-1
// steps ONCE (in on_start); on_tick then runs them with the same `run_steps`.
//
// Data step vocabulary (first matching field wins):
//   #{ move_to: [x,y,z], speed: 1.0, radius: 2.0 }  drive until arrived (speed/radius optional)
//   #{ cmd: "Name", params: #{...} }              fire any command once, then advance
//   #{ emit: "NAME", value: v }                   emit a telemetry marker once
//   #{ wait: 5.0 }                                dwell N seconds
//   #{ wait_event: "NAME" }                       hold until an event arrives
// Unknown steps lower to a no-op (skipped) — forward-compatible with new fields.
//
// Generic executor (RunTimeline injects exactly this around a `const TIMELINE`):
//   fn on_start(me)      { this.cur = seq_init(); this.steps = compile_timeline(TIMELINE.steps); }
//   fn on_tick(me)       { this.cur = run_steps(me, this.steps, this.cur); }
//   fn on_event(me, evt) { this.cur = seq_note_event(this.cur, evt); }

/// Lower ONE data step to a Layer-1 step (closures capturing the step's fields).
fn timeline_step(s) {
    if s.move_to != () {
        let wp  = s.move_to;
        let spd = if s.speed  != () { s.speed  } else { 1.0 };
        let rad = if s.radius != () { s.radius } else { 2.0 };
        return step(|m| nav_to(m, wp, spd, rad), |m| arrived(m, wp, rad));
    }
    if s.cmd != () {
        let name = s.cmd;
        let params = if s.params != () { s.params } else { #{} };
        return once(|m| cmd(name, params));
    }
    if s.emit != () {
        let name = s.emit;
        let val = s.value;
        return once(|m| emit(name, val));
    }
    if s.wait != () { return wait(s.wait); }
    if s.wait_event != () { return wait_for(s.wait_event); }
    once(|m| {})   // unknown → no-op, advance next tick
}

/// Lower a whole data timeline (array of data steps) to Layer-1 steps. Call ONCE
/// in on_start and hold the result on `this`; feed it to run_steps each tick.
fn compile_timeline(steps) { steps.map(|s| timeline_step(s)) }

// ── Rover control (re-emit every tick: the command has no persistent setpoint) ──
// Control = write the vessel's named input ports via the ONE generic `SetPorts`
// command (`writes` = [[port, value], …]). A wheeled rover exposes
// `throttle`/`steer`/`brake`; other vessels expose their own input ports.

/// Drive a rover this tick. `seq`/`tick` default to 0 — fine for host-local
/// scenarios; networked control should thread real sequence/tick numbers.
fn drive(rover, forward, steer) {
    cmd("SetPorts", #{ target: rover, writes: [["throttle", forward], ["steer", steer]], seq: 0, tick: 0 })
}

/// Stop a rover this tick (full brake).
fn brake(rover) {
    cmd("SetPorts", #{ target: rover, writes: [["brake", 1.0]], seq: 0, tick: 0 })
}

// ── Scene / world manipulation shortcuts (all generic cmd() under the hood) ──

fn load_scene(path)            { cmd("LoadScene", #{ path: path }) }
fn clear_scene()               { cmd("ClearScene") }
fn set_property(id, key, val)  { cmd("SetObjectProperty", #{ target: id, key: key, value: val }) }
fn detach_joint(joint)         { cmd("DetachJoint", #{ target: joint }) }
fn possess(vessel)             { cmd("PossessVessel", #{ target: vessel }) }

// Switch the active window camera by name (full USD prim path or its leaf) —
// the switch surface for cutscenes: `set_camera("ChaseCam")` cuts to the rover
// cam, `set_camera("WideShot")` to a scene camera. "Which camera renders" is
// Bevy's own is_active; scene cameras are standard USD `def Camera` prims.
fn set_camera(name)            { cmd("SetActiveCamera", #{ name: name }) }

// On-screen player notifications (toasts). `notify` is info-styled; `notify_kind`
// takes a style: "info" | "success" | "warn" | "error". Lets a scenario announce
// each phase without any Rust — drawn by the avatar UI overlay, logged headless.
fn notify(msg)                 { cmd("ShowNotification", #{ text: msg }) }
fn notify_kind(msg, kind)      { cmd("ShowNotification", #{ text: msg, kind: kind }) }

// Event-source helpers. `evt.source` is the EMITTER's gid — which sensor/script
// fired — so ONE `on_event` can branch per source (e.g. many checkpoints all
// named `enter:checkpoint`, different behaviour each). `0` = global (raw input).
fn emitter(evt)                { evt.source }
fn from_entity(evt, id)        { evt.source == id }

/// Fire a command and return JUST its assigned data (the `.data` bag), or () if
/// it failed or reported none. The create-then-manipulate shortcut:
///   let id = cmd_data("SpawnThing", #{...}); if id != () { set_property(id, ...) }
fn cmd_data(name, params) {
    let r = cmd(name, params);
    if r.ok { r.data } else { () }
}

// ── Sensing (reads over get()/query() — gives scripts eyes, not just hands) ──
//
// get() now returns vectors as [x,y,z], so component reads feed the vector math
// above directly. query() reaches the spatial providers (avian-backed Raycast /
// GroundHeight live in lunco-mobility; Nearest / EntitiesInRadius in lunco-api).

/// Linear velocity [vx,vy,vz] of an entity (Avian `LinearVelocity`), or () if
/// the entity has no velocity component.
fn velocity(id) { get(id, "LinearVelocity") }

/// Speed in m/s (0.0 if no velocity).
fn speed(id) { let v = velocity(id); if v == () { 0.0 } else { vlen(v) } }

/// Cast a world-space ray. Returns #{hit, entity, distance, point, normal};
/// `hit` is false when nothing is struck.
fn raycast(origin, dir, max) { query("Raycast", #{ origin: origin, dir: dir, max: max }) }

/// True if a collider is within `dist` straight ahead of `entity` — the basic
/// obstacle-avoidance probe.
fn obstacle_ahead(entity, dist) {
    let p = world_pos(entity);
    let f = world_forward(entity);
    if p == () || f == () { return false; }
    let r = raycast(p, f, dist);
    if r == () { return false; }
    r.hit == true
}

/// Ground/terrain height (world Y) under a world (x,z), or () if nothing below.
fn ground_height(x, z) {
    let r = query("GroundHeight", #{ x: x, z: z });
    if r == () { return (); }
    r.height
}

/// Nearest registered entity id to a world point, or () if none. Pass `me` as
/// `self_id` to exclude yourself.
fn nearest(point, self_id) {
    let r = query("Nearest", #{ point: point, exclude: self_id });
    if r == () || r.id == () { return (); }
    r.id
}

/// Ids of every registered entity within `radius` of a world point.
fn entities_in_radius(point, radius) {
    let r = query("EntitiesInRadius", #{ point: point, radius: radius });
    if r == () { return []; }
    r.ids
}

// ── Collision / trigger events (react in on_event, no polling) ───────────────
//
// The mobility crate mirrors avian `CollisionStart`/`CollisionEnd` onto the
// telemetry bus as `COLLISION_START` / `COLLISION_END` events whose value is the
// gid pair "a:b" (a = avian event target, 0 = unregistered collider). They land
// in `on_event(self, evt)` one tick after contact (same actor-model delay as
// emit). avian only fires for entities with collision events enabled — trigger
// volumes (Sensors) are auto-armed; solid bodies opt in via CollisionEventsEnabled.
//
//   fn on_event(self, evt) {
//       let other = collision_other(evt, self);
//       if other != () { /* touched `other` — react */ }
//   }

/// `[gidA, gidB]` of a collision `evt` (0 = unregistered side), or () if `evt`
/// isn't a COLLISION_START/COLLISION_END pulse.
fn collision_pair(evt) {
    if evt.name != "COLLISION_START" && evt.name != "COLLISION_END" { return (); }
    let parts = evt.value.split(":");
    if parts.len() < 2 { return (); }
    [parse_int(parts[0]), parse_int(parts[1])]
}

/// The OTHER entity in a collision `evt` involving `self_id`, or () if `evt`
/// isn't a collision touching `self_id`. The basic contact probe. A return of
/// `0` means you touched an unregistered collider (e.g. raw terrain).
fn collision_other(evt, self_id) {
    let p = collision_pair(evt);
    if p == () { return (); }
    if p[0] == self_id { p[1] }
    else if p[1] == self_id { p[0] }
    else { () }
}

/// True if `evt` is a contact START touching `self_id` (entered a zone / hit
/// something). Use inside on_event.
fn entered(evt, self_id) {
    evt.name == "COLLISION_START" && collision_other(evt, self_id) != ()
}

/// True if `evt` is a contact END touching `self_id` (left a zone / separated).
fn exited(evt, self_id) {
    evt.name == "COLLISION_END" && collision_other(evt, self_id) != ()
}

// Named trigger volumes — a Sensor authored with a Name fires, IN ADDITION to
// the generic COLLISION_START/END, an `enter:<name>` / `exit:<name>` pulse whose
// value is the ENTERING entity's gid. So a scenario reacts to a zone by name,
// not by reverse-mapping gids:
//
//   fn on_event(self, evt) {
//       if entered_zone(evt, "pad_2") { possess(evt.value); }   // evt.value = who entered
//       let z = zone_of(evt);                                   // or branch on the name
//   }

/// The trigger-zone name an `enter:`/`exit:` pulse carries, or () if `evt` isn't
/// a zone pulse. `enter:pad_2` → "pad_2".
fn zone_of(evt) {
    if evt.name.starts_with("enter:") { evt.name.sub_string(6) }
    else if evt.name.starts_with("exit:") { evt.name.sub_string(5) }
    else { () }
}

/// True if `evt` is an ENTER pulse for trigger zone `name` (something entered it;
/// `evt.value` is the entrant's gid).
fn entered_zone(evt, name) { evt.name == "enter:" + name }

/// True if `evt` is an EXIT pulse for trigger zone `name` (`evt.value` = the gid
/// that left).
fn exited_zone(evt, name) { evt.name == "exit:" + name }

// ── Selection toolkit (write targeting/selection algorithms in rhai) ─────────
//
// list_entities() returns #{ id, name, type, pos } per entity, so selection is
// idiomatic functional rhai over the built-in filter/map/reduce/sort_by. These
// are the reusable PRIMITIVES — compose them or write your own; all are policy,
// hot-reloadable, no Rust rebuild. Deeper per-entity reads use get(id, "Comp.f").
// Pass closures as predicates/metrics, e.g.
//   let weakest = min_by(all_of_type("rover"), |e| get(e.id, "Battery.charge"));

/// Vector × scalar.
fn vscale(a, s) { [a[0]*s, a[1]*s, a[2]*s] }

/// True if entity `id` currently has component `name` (e.g. "RoverVessel",
/// "LinearVelocity"). Built on get(), which returns () for an absent component.
fn has_component(id, name) { get(id, name) != () }

/// Kind tag for one entity ("spacecraft"/"rover"/"planet"/"unknown").
/// NOTE: vessel "kind" is now topology-derived — the old RoverVessel marker is
/// gone and FlightSoftware (the possessable-vessel signal) does NOT derive
/// Reflect, so has_component() can't see it. We approximate by testing the
/// reflect-registered topology components: SimComponent (Modelica-flown →
/// spacecraft) and the drive components (→ rover). A Rust bridge fn that reads
/// FlightSoftware directly would be more precise.
/// TODO: expose a native vessel_kind(id) bridge fn once FlightSoftware topology
/// is queryable from script (kind is now derived, not stored on a marker).
fn kind(id) {
    if has_component(id, "SimComponent") { "spacecraft" }
    else if has_component(id, "DriveMix") { "rover" }
    else if has_component(id, "CelestialBody") { "planet" }
    else { "unknown" }
}

/// All entities of a kind, as list_entities() records (#{id,name,type,pos}).
fn all_of_type(t) { list_entities().filter(|e| e.type == t) }

/// The list element with the smallest `metric.call(e)`, or () if empty.
fn min_by(list, metric) {
    let best = ();
    let best_v = ();
    for e in list {
        let v = metric.call(e);
        if best == () || v < best_v { best = e; best_v = v; }
    }
    best
}

/// The list element with the largest `metric.call(e)`, or () if empty.
fn max_by(list, metric) {
    let best = ();
    let best_v = ();
    for e in list {
        let v = metric.call(e);
        if best == () || v > best_v { best = e; best_v = v; }
    }
    best
}

/// How many list elements satisfy `pred.call(e)`.
fn count_where(list, pred) { list.filter(|e| pred.call(e)).len() }

/// The entity record nearest to world point `point` matching `pred`, or () if
/// none. Operates on list_entities() records (uses e.pos).
fn nearest_where(point, pred) {
    let cands = list_entities().filter(|e| e.pos != () && pred.call(e));
    min_by(cands, |e| vlen(vsub(e.pos, point)))
}

/// The entity record farthest from `point` matching `pred`, or ().
fn farthest_where(point, pred) {
    let cands = list_entities().filter(|e| e.pos != () && pred.call(e));
    max_by(cands, |e| vlen(vsub(e.pos, point)))
}
