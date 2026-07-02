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
        d = ev.some(|e| e.name == s.event && (want == () || e.source == want));
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
    if evt.name == () { return; }
    let e = #{ name: evt.name, source: evt.source };
    // Buffer name + emitter source so `wait_for` (name) and `wait_for_from`
    // (name + source) can both match against this tick's events.
    if this.task != () {
        if this.__events == () { this.__events = []; }
        this.__events.push(e);
    }
    // Independent buffer for a declarative `mission(me)`'s `requires_event`
    // objectives — so event-gated steps work whether or not a task is running.
    if this.mission != () {
        if this.__mevents == () { this.__mevents = []; }
        this.__mevents.push(e);
    }
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

