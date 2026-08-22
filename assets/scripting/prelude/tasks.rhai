// ── Tasks: the canonical auto-advanced mission policy ───────────────────────
//
// A task is pure Rhai data. The native `lunco-behavior` kernel owns the cursor,
// dwell timing, event delivery, and fixed-step progression. Return the tree
// from `task(me)`; do not implement a second cursor in `on_tick`.

// The constructors below are the only script-side spellings of the task
// schema discriminators. They are data tags at the Rhai-to-Rust boundary, not
// user-facing mission text; `lunco-scripting/src/task_tree.rs` parses them
// immediately into its typed runtime enum. Keep the tags in these constructors
// rather than top-level Rhai constants: indirect prelude calls do not resolve
// top-level constants in the runtime's separate function-call scope.

/// Step constructors (sugar over the map literals; all optional to use).
fn step(act, done) { #{ kind: "step", act: act, done: done } }
fn once(act)       { #{ kind: "once", act: act } }
fn act_for(act, secs) { #{ kind: "act_for", act: act, secs: secs } }
fn act_until_event(act, event, src) {
    #{ kind: "act_until_event", act: act, event: event, src: src }
}
fn wait(secs)      { #{ kind: "wait", secs: secs } }
fn wait_until(done){ #{ kind: "wait_until", done: done } }
fn wait_for(event) { #{ kind: "wait_for", event: event } }
fn wait_for_from(event, src) {
    #{ kind: "wait_for_from", event: event, src: src }
}
// This is the built-in "do X, wait until Y, then Z" capability available to
// every script:
//
//   fn task(me) {
//       seq([
//           once(|m| drive_to(m, CP1)),         // run an action, advance
//           wait_until(|m| arrived(m, CP1)),    // suspend until true
//           wait(3.0),                          // suspend 3 sim-seconds
//           wait_for("GO"),                     // suspend until event "GO"
//           once(|m| drive_to(m, CP2)),
//       ]);
//   }
//   // no fixed-tick hook or event plumbing — the task drives itself. Step
//   // closures take the host gid `m`.
//
// rhai has no real coroutines (and wasm can't stack-switch), so a task is a
// COOPERATIVE cursor advanced once per tick, not a suspended call stack — the
// right model for a deterministic, replicated sim. Steps reuse the Layer-1
// vocabulary (once/wait/wait_until/wait_for/step). `on_event` remains available
// for external event policy; task event waits are delivered by the kernel.

// A task is a TREE of nodes. Leaves are the Layer-1 step maps (once/step/
// act_for/act_until_event/wait/wait_until/wait_for); composites nest other nodes:
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
// Failure-aware nodes (native kernel vocabulary):
//   check(|m| pred)     Success/Failure immediately — the leaf that can fail
//   sel([a, b, c])      fallback: first child to SUCCEED wins; fails if all fail
//   retry(n, node)      re-attempt `node` on Failure, up to n times
//   invert(node)        swap Success ↔ Failure
//   force_ok(node)      any terminal → Success        force_fail(node) → Failure
//   reactive_seq([g,a]) re-tick from the FIRST child every tick (live guards):
//                       "do a WHILE g holds"; reactive_sel = priority arbiter

fn __task_composite(kind, items) { #{ kind: kind, items: items } }
fn __task_decorator(kind, body) { #{ kind: kind, body: body } }

/// Node constructors (return the root from `task(me)`, or assign it once in
/// `on_start` when setup needs to happen first).
fn seq(items)      { __task_composite("seq", items) }
fn par_all(items)  { __task_composite("all", items) }
fn par_race(items) { __task_composite("race", items) }
fn repeat(n, body) { let out = __task_decorator("repeat", body); out.n = n; out }
fn forever(body)   { __task_decorator("forever", body) }
fn sel(items)      { __task_composite("sel", items) }
fn retry(n, body)  { let out = __task_decorator("retry", body); out.n = n; out }
fn invert(body)    { __task_decorator("invert", body) }
fn force_ok(body)  { __task_decorator("force_ok", body) }
fn force_fail(body){ __task_decorator("force_fail", body) }
fn reactive_seq(items) { __task_composite("reactive_seq", items) }
fn reactive_sel(items) { __task_composite("reactive_sel", items) }
/// Predicate leaf that can FAIL (for sel/retry/invert): Success when the
/// predicate is true, Failure when false — unlike wait_until, which waits.
fn check(pred)     { #{ kind: "check", check: pred } }

/// Engine driver — buffer an event for a declarative `mission(me)`'s
/// `requires_event` objectives. (The task tree's `wait_for` leaves are fed
/// natively by the kernel driver; this rhai buffer only serves the mission.)
fn __note_task_event(me, evt) {
    if evt.name == () { return; }
    if this.mission != () {
        if this.__mevents == () { this.__mevents = []; }
        let source = if evt.source == () { 0 } else { evt.source };
        this.__mevents.push(#{ name: evt.name, source: source });
    }
}

/// SUGAR — define a `task(me)` function returning a node and the engine runs it
/// automatically: no on_start, no `this.task = …`. The whole scenario can be:
///
///   fn task(me) {
///       seq([ once(|m| drive_to(m, CP1)), wait_until(|m| arrived(m, CP1)), wait(3.0) ])
///   }
///
/// Engine driver — called once after `on_start`; seeds `this.task` from a user
/// `task(me)` if defined and not already set (an explicit assignment wins).
fn __init_task(me) {
    if this.task == () && is_def_fn("task", 1) { this.task = task(me); }
}
