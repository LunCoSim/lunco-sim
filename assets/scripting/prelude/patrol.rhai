// ── Patrol / checkpoints (DATA authored in rhai, visualised by the Rust core) ─
//
// A checkpoint list is a [`BehaviorSpec::Patrol`] — the SAME data shape
// `SetAutopilotBehavior` takes. These helpers build that data as a native
// `#{...}` map, serialise it with `to_json`, and feed it to the EXISTING typed
// command — no new Rust verb, the spec persists when the scenario is journaled
// (`DomainKind::Script`) and the live path-line gizmo reads the mirrored
// `AutopilotBehaviorSpec` component the command installs on the vessel.
//
// `points` is an array of waypoints. Each may be a bare `[x, y, z]` world
// position OR a map `#{pos:[x,y,z], dwell:f64, on_arrival:[...]}` carrying
// per-waypoint dwell and arrival actions (e.g. `[take_photo()]` from
// `science.rhai` — the declarative home for "fire a tool at a waypoint", no
// tree composition). `speed`/`radius`/`dwell` default to 0.6 / 3.0 / 0.0.
//
//   engage_patrol(rover, [[10,0,0], [10,0,10], [0,0,10]], 0.5, 2.0, 1.0)
//   patrol(rover, points)               // set behaviour on an engaged vessel
//   patrol(rover, [#{pos:[5,0,5], on_arrival:[take_photo()]}])  // armed waypoint
//   add_checkpoint(rover, 5.0, 0.0, 5.0) // append to this scenario's list + reset
//   clear_patrol(rover)                 // ClearPatrol (brake + drop the mirror)

/// Default patrol tuning (kept here, not in Rust — no magic numbers in core).
const PATROL_SPEED  = 0.6;
const PATROL_RADIUS = 3.0;
const PATROL_DWELL  = 0.0;

/// Build a `BehaviorSpec::Patrol` map from `points`.
fn __patrol_spec(points, speed, radius, dwell) {
    #{
        kind: "patrol",
        waypoints: points,
        speed: speed,
        radius: radius,
        dwell: dwell,
    }
}

/// Engage an autopilot on `vessel` AND set its behaviour to a patrol over
/// `points` in one step. The single canonical way to start a rover driving a
/// checkpoint loop from a scenario; the spec mirrors to `AutopilotBehaviorSpec`
/// on the vessel, so the path-line gizmo + Command Deck render it.
fn engage_patrol(vessel, points, speed, radius, dwell) {
    let s   = if speed  == () { PATROL_SPEED  } else { speed  };
    let r   = if radius == () { PATROL_RADIUS } else { radius };
    let d   = if dwell  == () { PATROL_DWELL  } else { dwell  };
    let pts = if points == () { [] } else { points };
    cmd("EngageAutopilot", #{ vessel: vessel, spec_json: to_json(__patrol_spec(pts, s, r, d)) });
}

/// `(vessel, points)` — engage with default tuning.
fn engage_patrol(vessel, points) { engage_patrol(vessel, points, (), (), ()); }

/// Set (or hot-swap) the behaviour of an already-engaged vessel to a patrol over
/// `points`. Use after `engage_patrol`/`EngageAutopilot` to redefine the
/// checkpoint loop without re-claiming the vessel.
fn patrol(vessel, points, speed, radius, dwell) {
    let s = if speed  == () { PATROL_SPEED  } else { speed  };
    let r = if radius == () { PATROL_RADIUS } else { radius };
    let d = if dwell  == () { PATROL_DWELL  } else { dwell  };
    let pts = if points == () { [] } else { points };
    cmd("SetAutopilotBehavior", #{ vessel: vessel, spec_json: to_json(__patrol_spec(pts, s, r, d)) });
}

/// `(vessel, points)` — set patrol with default tuning.
fn patrol(vessel, points) { patrol(vessel, points, (), (), ()); }

/// Append a checkpoint to THIS scenario's per-vessel list (kept on `this` so a
/// long mission can accumulate), then re-emit a patrol covering the full list.
///
/// Interactive (Ctrl+LMB) additions go through the Rust UI and don't call this —
/// that path mutates the live spec and re-emits `SetAutopilotBehavior` directly.
/// Scenario scripts use this so their checkpoint list survives for later steps.
fn add_checkpoint(vessel, x, y, z) {
    if this.__checkpoints == () { this.__checkpoints = #{}; }
    let list = this.__checkpoints[vessel];
    if list == () { list = []; }
    list.push([x, y, z]);
    this.__checkpoints[vessel] = list;
    patrol(vessel, list, (), (), ());
}

/// Clear the patrol on `vessel` and stop it, and forget this scenario's
/// checkpoint list for it. Fires the canonical `ClearPatrol` typed command —
/// the same verb the Command Deck "Clear patrol" button and the
/// delete-last-waypoint path use (§4.2 — one input shape, every surface). No
/// hand-built `Brake` JSON.
fn clear_patrol(vessel) {
    if this.__checkpoints != () { this.__checkpoints.remove(vessel); }
    cmd("ClearPatrol", #{ vessel: vessel });
}