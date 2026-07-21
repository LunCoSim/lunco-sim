// ── An autopilot with no waypoints holds ────────────────────────────────────
//
// TEST-ONLY. Runs against `scenes/sandbox/autopilot_hold_test.usda`.
//
// WHAT IT GUARDS. That engaging an autopilot on a vessel that has NO route
// claims the vessel and stops there. It used to drive: `EngageAutopilot` with an
// unset `throttle` substituted 0.5, and both UI paths (Command Deck "Engage
// autopilot", Toggle Autopilot) sent a constant setpoint of their own. A rover
// with no waypoints therefore left in a straight line the moment it was engaged
// — from outside, indistinguishable from an autopilot that ignores its route.
//
// Holding means BRAKING, not merely releasing the throttle: a rover parked on a
// slope with the drive released rolls. The ground here is flat so the assertion
// is about the setpoint and nothing else; the brake is what makes the same
// answer hold on real terrain.
//
// ── WHY TWO ROVERS ──────────────────────────────────────────────────────────
//
// "It didn't move" is exactly what a dead drive path, an unloaded rover or a
// frozen physics tick also produce. `Routed` is the CONTROL: the same rover,
// the same engage verb, one waypoint 12 m ahead. It must arrive. Without it the
// headline check passes on a broken build.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("AUTOPILOT_HOLD", "PASS"|"FAIL")` — the telemetry verdict `scene_test`
// turns into its exit code.

// How far a held rover may drift. Settling, suspension sag and solver noise are
// centimetres; the defect drove the rover the length of the run (tens of
// metres). One metre sits far clear of both.
const HOLD_TOL_M = 1.0;

// The control's waypoint, 12 m along +X from where `Routed` starts. Far enough
// that arriving cannot be confused with drift, close enough to reach in the
// drive window.
const LEG_M = 12.0;

/// Distance between two samples, or `()` if either is missing. NOT `seg` from
/// the prelude: that returns 0.0 on a missing sample, which is exactly the value
/// that makes "it held still" pass when the rover has actually despawned.
fn dist(a, b) {
    if a == () || b == () { return (); }
    vlen(vsub(a, b))
}

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 3.0;    // land + let the kinematic → dynamic handover finish
    this.drive_s = 20.0;    // long enough for the control to cover 12 m
    this.phase = "settle";
    this.engaged = false;

    this.idle = find_or_none("/AutopilotHoldTest/Idle");
    this.routed = find_or_none("/AutopilotHoldTest/Routed");

    this.idle_p0 = ();
    this.routed_p0 = ();
    this.idle_max = 0.0;    // worst drift seen at ANY sample, not just the end —
                            // a rover that drives off and is stopped by something
                            // must not be able to come back and pass.

    print("[hold] ── an autopilot with no waypoints holds ──────────────────");
    print("[hold] idle=" + this.idle + " routed=" + this.routed);
    print("[hold] tolerance " + HOLD_TOL_M + " m over a " + this.drive_s + "s run");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.idle == () || this.routed == () {
        fail_fast("rover prims not found — wrong scene? expected " +
                  "scenes/sandbox/autopilot_hold_test.usda",
                  "AUTOPILOT HOLD", "AUTOPILOT_HOLD");
        this.phase = "done";
        return;
    }

    this.t += dt();

    if this.phase == "settle" {
        brake(this.idle);
        brake(this.routed);
        if this.t >= this.settle_s {
            this.idle_p0 = world_pos(this.idle);
            this.routed_p0 = world_pos(this.routed);

            // THE SUBJECT: engage with no `spec_json` and no `throttle`. This is
            // what the Engage button sends, and what a bare
            // `cmd("EngageAutopilot", #{ vessel })` sends.
            cmd("EngageAutopilot", #{ vessel: this.idle });

            // THE CONTROL: same verb, one waypoint. Authored relative to where
            // the rover actually settled, so the leg is 12 m whatever the drop
            // did.
            let wp = [this.routed_p0[0] + LEG_M, this.routed_p0[1], this.routed_p0[2]];
            engage_patrol(this.routed, [wp]);

            this.engaged = true;
            this.phase = "drive";
            print("[hold] engaged: idle with NO route, routed toward " +
                  r2(wp[0]) + "," + r2(wp[2]));
        }
        return;
    }

    // No `drive()` calls from here on — whatever moves these rovers is the
    // autopilot, which is the whole question.
    let d = dist(world_pos(this.idle), this.idle_p0);
    if d != () && d > this.idle_max { this.idle_max = d; }

    if this.t >= this.settle_s + this.drive_s {
        this.phase = "done";
        verdict(#{
            idle: this.idle,
            routed: this.routed,
            idle_p0: this.idle_p0,
            routed_p0: this.routed_p0,
            idle_max: this.idle_max,
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   held        the headline: the routeless rover never wandered further than
//               HOLD_TOL_M from where it was engaged, at any sample.
//   control     the routed rover covered most of its leg — proof the engage
//               verb, the drive path and physics all work in this run, so
//               "held" means held rather than "nothing was driving anything".
fn verdict(s) {
    let f = [];

    let idle_end = dist(world_pos(s.idle), s.idle_p0);
    let routed_end = dist(world_pos(s.routed), s.routed_p0);

    f.push(t_present(idle_end, "idle rover has no world position at the end of the run"));
    f.push(t_present(routed_end, "routed rover has no world position at the end of the run"));

    f.push(t_max(s.idle_max, HOLD_TOL_M,
        "idle rover DROVE OFF with no waypoints: peak displacement (m)"));

    // The control must genuinely traverse, not merely twitch: most of the leg.
    f.push(t_moved(routed_end, LEG_M * 0.6,
        "routed rover did not drive — the engage/drive path is dead, so the " +
        "hold check above proves nothing (m)"));

    print("[hold] ── results ──────────────────────────────────────────────");
    print("[hold]   idle    peak=" + r2(s.idle_max) + " m  end=" + r2(idle_end) +
          " m  (limit " + HOLD_TOL_M + ")");
    print("[hold]   routed  end=" + r2(routed_end) + " m  (leg " + LEG_M + ")");

    report_verdict(f, "AUTOPILOT HOLD", "AUTOPILOT_HOLD");
}
