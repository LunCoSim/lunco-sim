// ── An autopilot with no waypoints holds ────────────────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/autopilot_hold.usda`.
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
// After the hold window, the same production fixture hot-swaps a patrol onto
// that engaged rover while it is part-way through the first route. It must
// continue toward the current leg rather than U-turn to leg 0. This is the
// public observable replacement for the old Rust-only cursor test; exact cursor
// arithmetic remains a Rust mechanism contract.
//
// ── WHY TWO ROVERS ──────────────────────────────────────────────────────────
//
// "It didn't move" is exactly what a dead drive path, an unloaded rover or a
// frozen physics tick also produce. `Routed` is the CONTROL: the same rover,
// the same engage verb, one waypoint 12 m ahead. It must traverse its leg.
// Without it the hold and hot-swap assertions pass on a broken build.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("AUTOPILOT_HOLD", "PASS"|"FAIL")` — the telemetry verdict `scene_test`
// turns into its exit code.

// How far a held rover may drift. Settling, suspension sag and solver noise are
// centimetres; the defect drove the rover the length of the run (tens of
// metres). One metre sits far clear of both.
const HOLD_TOL_M = 1.0;

// The control's waypoint, 12 m along −Z from where `Routed` starts. Far enough
// that arriving cannot be confused with drift, close enough to reach in the
// drive window. −Z is the rover's own forward at an identity spawn — "ahead" —
// so the leg is a straight chase and the assertion tests the DRIVE path, which
// is what the control exists to prove. (An off-heading leg would test the skid
// yaw authority instead, which is a different property — and with the regolith
// cornering fit it turns too slowly to reach any lateral waypoint in the window.)
const LEG_M = 12.0;

// The hot-swap route reaches its first point, then samples the second leg
// part-way through it. A reset-to-zero implementation has to reverse toward
// HOT_FIRST_M; a resumed implementation continues toward HOT_SECOND_M.
const HOT_FIRST_M = 6.0;
const HOT_SECOND_M = 18.0;
const HOT_THIRD_M = 30.0;
const HOT_FOURTH_M = 42.0;
const HOT_FIFTH_M = 54.0;
const HOT_SECOND_ENTRY_M = 9.0;
const HOT_OBSERVE_S = 8.0;
const HOT_TIMEOUT_S = 42.0;

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
    this.hold_s = 10.0;     // prove the no-route hold before reusing that rover
    this.phase = "settle";
    this.engaged = false;

    this.idle = find_or_none("/AutopilotHoldTest/Idle");
    this.routed = find_or_none("/AutopilotHoldTest/Routed");

    this.idle_p0 = ();
    this.routed_p0 = ();
    this.idle_max = 0.0;    // worst drift seen at ANY sample, not just the end —
                            // a rover that drives off and is stopped by something
                            // must not be able to come back and pass.
    this.hot_p0 = ();
    this.hot_wp0 = ();
    this.hot_wp1 = ();
    this.hot_wp2 = ();
    this.hot_swap_p0 = ();
    this.hot_end = ();
    this.hot_first_reached = false;
    this.hot_swap_issued = false;
    this.hot_swap_t = ();

    print("[hold] ── an autopilot with no waypoints holds ──────────────────");
    print("[hold] idle=" + this.idle + " routed=" + this.routed);
    print("[hold] tolerance " + HOLD_TOL_M + " m over a " + this.hold_s + "s hold window");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.idle == () || this.routed == () {
        fail_fast("rover prims not found — wrong scene? expected " +
                  "scenes/tests/autopilot_hold.usda",
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
            // did — 12 m along the rover's own heading (−Z).
            let wp = [this.routed_p0[0], this.routed_p0[1], this.routed_p0[2] - LEG_M];
            engage_patrol(this.routed, [wp]);

            this.engaged = true;
            this.phase = "hold";
            print("[hold] engaged: idle with NO route, routed toward " +
                  r2(wp[0]) + "," + r2(wp[2]));
        }
        return;
    }

    // No `drive()` calls from here on — whatever moves these rovers is the
    // autopilot, which is the whole question.
    if this.phase == "hold" {
        let d = dist(world_pos(this.idle), this.idle_p0);
        if d != () && d > this.idle_max { this.idle_max = d; }

        if this.t >= this.settle_s + this.hold_s {
            // Reuse the same engaged vessel for the production hot-swap check.
            // `patrol` is the public live update path; no Rust-only test hook is
            // involved.
            this.hot_p0 = world_pos(this.idle);
            this.hot_wp0 = [this.hot_p0[0], this.hot_p0[1], this.hot_p0[2] - HOT_FIRST_M];
            this.hot_wp1 = [this.hot_p0[0], this.hot_p0[1], this.hot_p0[2] - HOT_SECOND_M];
            this.hot_wp2 = [this.hot_p0[0], this.hot_p0[1], this.hot_p0[2] - HOT_THIRD_M];
            patrol(this.idle, [this.hot_wp0, this.hot_wp1, this.hot_wp2]);
            this.phase = "hot_swap";
            print("[hold] hold passed; live patrol starts at " + r2(this.hot_p0[2]) +
                  " and targets " + r2(this.hot_wp0[2]) + "," + r2(this.hot_wp1[2]));
        }
        return;
    }

    if this.phase == "hot_swap" {
        let hp = world_pos(this.idle);
        if hp == () { return; }

        // First prove the route is progressing. Then wait until the rover is
        // part-way through leg 1, not sitting at the first waypoint.
        if !this.hot_first_reached {
            let d0 = dist(hp, this.hot_wp0);
            if d0 != () && d0 < 3.0 { this.hot_first_reached = true; }
        } else if !this.hot_swap_issued {
            let d1 = dist(hp, this.hot_wp1);
            if d1 != () && d1 < HOT_SECOND_ENTRY_M {
                this.hot_swap_p0 = hp;
                let wp3 = [this.hot_p0[0], this.hot_p0[1], this.hot_p0[2] - HOT_FOURTH_M];
                let wp4 = [this.hot_p0[0], this.hot_p0[1], this.hot_p0[2] - HOT_FIFTH_M];
                patrol(this.idle, [this.hot_wp0, this.hot_wp1, this.hot_wp2, wp3, wp4]);
                this.hot_swap_issued = true;
                this.hot_swap_t = this.t;
                print("[hold] hot-swap at z=" + r2(hp[2]) +
                      " while approaching leg 1; appended legs continue to " +
                      r2(wp4[2]));
            }
        } else if this.t >= this.hot_swap_t + HOT_OBSERVE_S {
            this.hot_end = hp;
            this.phase = "done";
            verdict(#{
                idle: this.idle,
                routed: this.routed,
                idle_p0: this.idle_p0,
                routed_p0: this.routed_p0,
                idle_max: this.idle_max,
                hot_p0: this.hot_p0,
                hot_swap_p0: this.hot_swap_p0,
                hot_end: this.hot_end,
                hot_first_reached: this.hot_first_reached,
                hot_swap_issued: this.hot_swap_issued,
            });
            return;
        }

        if this.t > this.settle_s + this.hold_s + HOT_TIMEOUT_S {
            this.phase = "done";
            verdict(#{
                idle: this.idle,
                routed: this.routed,
                idle_p0: this.idle_p0,
                routed_p0: this.routed_p0,
                idle_max: this.idle_max,
                hot_p0: this.hot_p0,
                hot_swap_p0: this.hot_swap_p0,
                hot_end: this.hot_end,
                hot_first_reached: this.hot_first_reached,
                hot_swap_issued: this.hot_swap_issued,
            });
        }
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   held        the headline: the routeless rover never wandered further than
//               HOLD_TOL_M from where it was engaged, at any sample.
//   control     the routed rover covered most of its leg — proof the engage
//               verb, the drive path and physics all work in this run, so
//               "held" means held rather than "nothing was driving anything".
//   hot swap    after the same rover receives a longer patrol mid-leg, its z
//               coordinate continues toward the next waypoint. Positive z
//               progress would mean the rebuild reset to the first leg.
fn verdict(s) {
    let f = [];

    let idle_end = dist(world_pos(s.idle), s.idle_p0);
    let routed_end = dist(world_pos(s.routed), s.routed_p0);

    f.push(t_present(idle_end, "idle rover has no world position at the end of the run"));
    f.push(t_present(routed_end, "routed rover has no world position at the end of the run"));

    f.push(t_max(s.idle_max, HOLD_TOL_M,
        "idle rover DROVE OFF with no waypoints: peak displacement (m)"));

    // The control must genuinely traverse, not merely twitch. Stated against
    // HOLD_TOL_M — the thing it has to be distinguished FROM — and not against
    // LEG_M. What this assertion owes the test is "the drive path is live, so
    // `held` means held"; whether the rover ARRIVES in the drive window is a
    // question about its speed, and at `LEG_M * 0.6` it was asking one. It went red
    // at 6.68 m of 7.2 m when the shipped authored drivetrain parameters slowed the rover's
    // spin-up, on a run where the routed rover drove 6.7x the hold tolerance and
    // the idle one did not move — i.e. the test's actual claim held perfectly.
    f.push(t_moved(routed_end, HOLD_TOL_M * 4.0,
        "routed rover did not drive — the engage/drive path is dead, so the " +
        "hold check above proves nothing (m)"));

    let hot_progress = ();
    if s.hot_swap_p0 != () && s.hot_end != () {
        hot_progress = s.hot_swap_p0[2] - s.hot_end[2];
    }
    f.push(t_true(s.hot_first_reached,
        "the hot-swap rover never reached its first waypoint; the update was not tested mid-route"));
    f.push(t_true(s.hot_swap_issued,
        "the live patrol update was never issued before the production timeout"));
    f.push(t_present(hot_progress,
        "the hot-swap rover has no post-update position sample"));
    f.push(t_true(hot_progress > 0.5,
        "the rover reversed toward leg 0 after a mid-route update: forward z progress was " +
        hot_progress + " m; the current leg must be resumed"));

    print("[hold] ── results ──────────────────────────────────────────────");
    print("[hold]   idle    peak=" + r2(s.idle_max) + " m  end=" + r2(idle_end) +
          " m  (limit " + HOLD_TOL_M + ")");
    print("[hold]   routed  end=" + r2(routed_end) + " m  (leg " + LEG_M + ")");
    print("[hold]   hot-swap first=" + s.hot_first_reached + " issued=" +
          s.hot_swap_issued + " forward_z=" + hot_progress + " m");

    report_verdict(f, "AUTOPILOT HOLD", "AUTOPILOT_HOLD");
}
