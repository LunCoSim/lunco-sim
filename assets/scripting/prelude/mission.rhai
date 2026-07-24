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

/// One objective. `spec` keys (all optional but usually `done` or
/// `requires_event`): `text` (human label for the HUD, defaults to `id`), `done`
/// (|m| bool completion), `requires_event` (event NAME whose arrival completes
/// the objective — e.g. `"cmd:PossessVessel"` to advance when the user possesses
/// a vessel; latched, so it may combine with `done`), `dwell` (secs the
/// condition must hold), `fail` (|m| bool failure), `requires` (array of
/// objective ids that must complete first), `on_complete` (|m| side-effect when
/// it completes).
fn objective(id, spec) {
    #{
        id: id,
        text: if spec.text != () { spec.text } else { id },
        done: spec.done,
        requires_event: spec.requires_event,
        dwell: spec.dwell,
        fail: spec.fail,
        requires: if spec.requires != () { spec.requires } else { [] },
        on_complete: spec.on_complete,
        state: "pending",   // pending → active → complete | failed
        t0: (),             // dwell entry time
        __evfired: false,   // requires_event latch (event seen at least once)
    }
}

/// True if any buffered event this tick has name `want`.
fn __obj_saw_event(evs, want) {
    for e in evs { if e.name == want { return true; } }
    false
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
/// Completion requires the `done` closure (if any) AND the `requires_event`
/// latch (if any); an objective with neither completes immediately (narration).
fn __advance_objective(me, o, now) {
    if o.fail != () && o.fail.call(me) {
        o.state = "failed";
        emit("OBJECTIVE_FAILED", o.id);
        return o;
    }
    let met = o.done == () || o.done.call(me);
    if o.requires_event != () { met = met && (o.__evfired == true); }
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

/// Tick one objective: latch any matching event, unlock from pending when its
/// requires are met, then advance if active. Returns the updated objective.
fn __tick_objective(me, o, m, now, evs) {
    if o.state == "complete" || o.state == "failed" { return o; }
    if o.requires_event != () && __obj_saw_event(evs, o.requires_event) { o.__evfired = true; }
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
    if this.mission == () { this.__mevents = []; return; }
    let now = elapsed_seconds();
    let evs = if this.__mevents == () { [] } else { this.__mevents };
    let m = this.mission;
    for i in 0..m.len() {
        m[i] = __tick_objective(me, m[i], m, now, evs);
    }
    this.mission = m;
    this.__mevents = [];   // events live one tick (actor-model cadence)
    // Auto-publish the objectives checklist to the tutorial HUD when it changes,
    // so a declarative `mission(me)` renders on screen with no extra scripting.
    let hud = __mission_hud(m);
    if this.__hud_obj != hud { this.__hud_obj = hud; cmd("SetObjectives", #{ text: hud }); }
    let s = __mission_status(m);
    if this.__mission_done != true {
        if s.any_failed { this.__mission_done = true; emit("MISSION_FAILED", 0); }
        else if s.all_done { this.__mission_done = true; emit("MISSION_COMPLETE", 0); }
    }
}

/// Format a mission's objectives into the HUD checklist block (glyph per state).
fn __mission_hud(m) {
    let out = "";
    for o in m {
        if out != "" { out += "\n"; }
        out += __obj_glyph(o.state) + " " + o.text;
    }
    out
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
//   #{ move_to_entity: "/path", speed, radius }   drive to another entity, tracked live
//   #{ possess: "/path" }                         take control of a vessel (PossessVessel)
//   #{ brake: true, secs: 1.0 }                   hold the brake for N seconds
//   #{ cmd: "Name", params: #{...} }              fire any command once, then advance
//   #{ emit: "NAME", value: v }                   emit a telemetry marker once
//   #{ wait: 5.0 }                                dwell N seconds
//   #{ wait_event: "NAME" }                       hold until an event arrives
// Any drive/brake step may add `subject: "/path"` to act on that entity instead
// of the timeline's host (so one scenario can orchestrate a separate vessel).
// Unknown steps lower to a no-op (skipped) — forward-compatible with new fields.
//
// `RunTimeline` owns progression internally. Scenario rhai supplies declarative
// steps and reacts to events; it does not install a per-tick hook.

/// Resolve a step's subject entity: an explicit `subject` path, else host `m`.
fn timeline_subject(m, s) {
    if s.subject == () { return m; }
    let g = find(s.subject);
    if g != () { g } else { m }
}

/// Lower ONE data step to a Layer-1 step (closures capturing the step's fields).
fn timeline_step(s) {
    if s.move_to != () {
        let wp  = s.move_to;
        let spd = if s.speed  != () { s.speed  } else { 1.0 };
        let rad = if s.radius != () { s.radius } else { 2.0 };
        return step(|m| nav_to(timeline_subject(m, s), wp, spd, rad),
                    |m| arrived(timeline_subject(m, s), wp, rad));
    }
    if s.move_to_entity != () {
        let tgt = s.move_to_entity;
        let spd = if s.speed  != () { s.speed  } else { 1.0 };
        let rad = if s.radius != () { s.radius } else { 2.0 };
        return step(
            |m| { let g = find(tgt); if g != () { nav_to(timeline_subject(m, s), world_pos(g), spd, rad); } },
            |m| { let g = find(tgt); g != () && arrived(timeline_subject(m, s), world_pos(g), rad) });
    }
    if s.possess != () {
        let tgt = s.possess;
        return once(|m| cmd("PossessVessel", #{ target: find(tgt) }));
    }
    if s.brake != () {
        let secs = if s.secs != () { s.secs } else { 1.0 };
        return #{ act: |m| brake(timeline_subject(m, s)), secs: secs };
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
