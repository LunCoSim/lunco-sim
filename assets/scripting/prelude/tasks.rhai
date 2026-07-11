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
// of that: declare it ONCE by assigning `this.task` in on_start, and the NATIVE
// kernel driver advances it every tick and feeds it events automatically. This
// is the built-in "do X, wait until Y, then Z" capability available to every
// script:
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
//
// These constructors build PURE DATA — the tree is ticked NATIVELY on the
// lunco-behavior kernel (the same engine the rover autopilot runs on), which
// compiles the map once per assignment and feeds it events. There is no rhai
// tick code: mechanism lives in Rust (`lunco-scripting/src/task_tree.rs`),
// policy stays here. Emits TASK_COMPLETE once when the root finishes (and
// TASK_FAILED if a failing node — see below — ends the tree).
//
// Failure-aware nodes (kernel vocabulary the old rhai engine didn't have):
//   check(|m| pred)     Success/Failure immediately — the leaf that can fail
//   sel([a, b, c])      fallback: first child to SUCCEED wins; fails if all fail
//   retry(n, node)      re-attempt `node` on Failure, up to n times
//   invert(node)        swap Success ↔ Failure
//   force_ok(node)      any terminal → Success        force_fail(node) → Failure
//   reactive_seq([g,a]) re-tick from the FIRST child every tick (live guards):
//                       "do a WHILE g holds"; reactive_sel = priority arbiter

/// Node constructors (assign the root to `this.task` in on_start).
fn seq(items)      { #{ k: "seq",  items: items } }
fn par_all(items)  { #{ k: "all",  items: items } }
fn par_race(items) { #{ k: "race", items: items } }
fn repeat(n, body) { #{ k: "repeat", n: n, body: body } }
fn forever(body)   { #{ k: "forever", body: body } }
fn sel(items)      { #{ k: "sel",  items: items } }
fn retry(n, body)  { #{ k: "retry", n: n, body: body } }
fn invert(body)    { #{ k: "invert", body: body } }
fn force_ok(body)  { #{ k: "force_ok", body: body } }
fn force_fail(body){ #{ k: "force_fail", body: body } }
fn reactive_seq(items) { #{ k: "reactive_seq", items: items } }
fn reactive_sel(items) { #{ k: "reactive_sel", items: items } }
/// Predicate leaf that can FAIL (for sel/retry/invert): Success when the
/// predicate is true, Failure when false — unlike wait_until, which waits.
fn check(pred)     { #{ check: pred } }

/// Engine driver — buffer an event for a declarative `mission(me)`'s
/// `requires_event` objectives. (The task tree's `wait_for` leaves are fed
/// natively by the kernel driver; this rhai buffer only serves the mission.)
fn __note_task_event(me, evt) {
    if evt.name == () { return; }
    if this.mission != () {
        if this.__mevents == () { this.__mevents = []; }
        this.__mevents.push(#{ name: evt.name, source: evt.source });
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

