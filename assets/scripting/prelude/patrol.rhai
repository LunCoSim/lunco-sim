// ── Patrol / waypoints (DATA authored in rhai, visualised by the Rust core) ─
//
// A waypoint list is a [`BehaviorSpec::Patrol`] — the SAME data shape
// `SetAutopilotBehavior` takes. These helpers build that data as a native
// `#{...}` map, serialise it with `to_json`, and feed it to the EXISTING typed
// command — no new Rust verb, the spec persists when the scenario is journaled
// (`DomainKind::Script`) and the live route projection reads the mirrored
// `AutopilotBehaviorSpec` component the command installs on the vessel.
//
// `points` is an array of waypoints. Each may be a bare `[x, y, z]` world
// position OR a map `#{pos:[x,y,z], dwell:f64, on_arrival:[...]}` carrying
// per-waypoint dwell and arrival actions (e.g. `[take_photo()]` from
// `science.rhai` — the declarative home for "fire a tool at a waypoint", no
// tree composition). The helper canonicalizes bare positions to the current
// `PatrolWaypoint` map shape before crossing the typed command boundary.
// `speed`/`radius`/`dwell` default to 0.6 / 3.0 / 0.0.
//
//   engage_patrol(rover, [[10,0,0], [10,0,10], [0,0,10]], 0.5, 2.0, 1.0)
//   patrol(rover, points)               // set behaviour on an engaged vessel
//   patrol(rover, [#{pos:[5,0,5], on_arrival:[take_photo()]}])  // armed waypoint
//   clear_patrol(rover)                 // ClearPatrol (brake + drop the mirror)

/// Default patrol tuning (kept here, not in Rust — no magic numbers in core).
const PATROL_SPEED  = 0.6;
const PATROL_RADIUS = 3.0;
const PATROL_DWELL  = 0.0;

/// Build a `BehaviorSpec::Patrol` map from `points`.
fn __patrol_spec(points, speed, radius, dwell) {
    let waypoints = [];
    for point in points {
        if type_of(point) == "array" {
            waypoints.push(#{ pos: point });
        } else {
            waypoints.push(point);
        }
    }
    #{
        kind: "patrol",
        waypoints: waypoints,
        speed: speed,
        radius: radius,
        dwell: dwell,
    }
}

/// Engage an autopilot on `vessel` AND set its behaviour to a patrol over
/// `points` in one step. The single canonical way to start a rover driving a
/// waypoint loop from a scenario; the spec mirrors to `AutopilotBehaviorSpec`
/// on the vessel, so the route projection + Command Deck render it.
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
/// waypoint loop without re-claiming the vessel.
fn patrol(vessel, points, speed, radius, dwell) {
    let s = if speed  == () { PATROL_SPEED  } else { speed  };
    let r = if radius == () { PATROL_RADIUS } else { radius };
    let d = if dwell  == () { PATROL_DWELL  } else { dwell  };
    let pts = if points == () { [] } else { points };
    cmd("SetAutopilotBehavior", #{ vessel: vessel, spec_json: to_json(__patrol_spec(pts, s, r, d)) });
}

/// `(vessel, points)` — set patrol with default tuning.
fn patrol(vessel, points) { patrol(vessel, points, (), (), ()); }

/// Clear the patrol on `vessel` and stop it. Fires the canonical
/// `ClearPatrol` typed command —
/// the same verb the Command Deck "Clear patrol" button and waypoint context
/// actions use (§4.2 — one input shape, every surface). No
/// hand-built `Brake` JSON.
///
fn clear_patrol(vessel) {
    cmd("ClearPatrol", #{ vessel: vessel });
}
