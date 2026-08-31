// Production regression for a goal initially behind a rover.
// The autopilot must pivot to recover heading, never command reverse throttle,
// and then make measurable forward progress while pointing in the travel direction.

const SETTLE_S = 3.0;
const TIMEOUT_S = 60.0;
const GOAL_M = 12.0;
const ARRIVAL_RADIUS_M = 2.0;

fn fail_fast(reason) {
    report_verdict([reason], "AUTOPILOT FORWARD APPROACH", "AUTOPILOT_FORWARD_APPROACH");
}

fn on_start(me) {
    this.t = 0.0;
    this.phase = "settle";
    this.done = false;
    this.rover = find("/AutopilotForwardApproachTest/Rover");
    this.start = ();
    this.goal = ();
    this.initial_forward = ();
    this.min_throttle = ();
    this.max_steer = 0.0;
    this.max_goal_alignment = -1.0;
    this.max_goal_progress = 0.0;
    this.forward_drive_samples = 0;
    this.samples = 0;
    this.moved_samples = 0;
    print("[autopilot-forward] rover=" + this.rover);
}

fn on_tick(me) {
    if this.done { return; }
    if this.rover == () || this.rover < 0 {
        this.done = true;
        fail_fast("rear-approach rover was not composed");
        return;
    }

    this.t += dt();
    let p = world_pos(this.rover);
    if p == () {
        this.done = true;
        fail_fast("rear-approach rover lost its authoritative world pose");
        return;
    }

    if this.phase == "settle" {
        brake(this.rover);
        if this.t >= SETTLE_S {
            this.start = p;
            this.goal = [p[0], p[1], p[2] + GOAL_M];
            this.initial_forward = world_forward(this.rover);
            if this.initial_forward == () {
                this.done = true;
                fail_fast("rear-approach rover has no authoritative heading");
                return;
            }
            engage_patrol(this.rover, [this.goal], 0.6, ARRIVAL_RADIUS_M, 0.0);
            this.phase = "drive";
            print("[autopilot-forward] goal=" + this.goal +
                  " initial_forward=" + this.initial_forward);
        }
        return;
    }

    let throttle = get(this.rover, "throttle");
    let steer = get(this.rover, "steer");
    let fwd = world_forward(this.rover);
    if throttle != () {
        if this.min_throttle == () || throttle < this.min_throttle {
            this.min_throttle = throttle;
        }
    }
    if steer != () && steer.abs() > this.max_steer {
        this.max_steer = steer.abs();
    }
    if fwd != () {
        let alignment = vdot(vnorm(fwd), [0.0, 0.0, 1.0]);
        if alignment > this.max_goal_alignment {
            this.max_goal_alignment = alignment;
        }
        if throttle != () && throttle > 0.05 && alignment > 0.7 {
            this.forward_drive_samples += 1;
        }
    }
    this.samples += 1;
    let goal_progress = vdot(vsub(p, this.start), [0.0, 0.0, 1.0]);
    if goal_progress > this.max_goal_progress {
        this.max_goal_progress = goal_progress;
    }
    if vlen(vsub(p, this.start)) > 0.25 { this.moved_samples += 1; }

    // Stop as soon as the observable forward-approach contract is established.
    // The timeout below is only a dead-run guard; it is not a qualification time.
    if (this.forward_drive_samples > 20 && this.max_goal_progress > 3.0) ||
       this.t >= SETTLE_S + TIMEOUT_S {
        brake(this.rover);
        let final_forward = world_forward(this.rover);
        let final_alignment = if final_forward == () { () } else { vdot(vnorm(final_forward), [0.0, 0.0, 1.0]) };
        let checks = [
            t_present(this.initial_forward, "initial heading"),
            t_true(this.initial_forward != () &&
                   vdot(vnorm(this.initial_forward), [0.0, 0.0, 1.0]) < -0.8,
                "fixture goal was not initially behind the rover"),
            t_present(this.min_throttle, "autopilot throttle samples"),
            t_true(this.min_throttle >= -0.001,
                "autopilot commanded reverse throttle while approaching a rear goal"),
            t_true(this.max_steer > 0.8,
                "autopilot never issued the hard turn needed to recover heading"),
            t_true(this.samples > 100, "rear approach produced too few control samples"),
            t_true(this.moved_samples > 20, "rear approach rover did not move"),
            t_true(this.forward_drive_samples > 20,
                "autopilot never resumed positive-throttle driving after turning"),
            t_true(this.max_goal_progress > 3.0,
                "rover did not make measurable forward progress toward the rear waypoint"),
            t_present(final_alignment, "final travel heading"),
            t_true(final_alignment != () && final_alignment > 0.7,
                "rover did not turn to face forward; alignment=" + final_alignment),
        ];
        print("[autopilot-forward] samples=" + this.samples +
              " min_throttle=" + r2(this.min_throttle) +
              " max_steer=" + r2(this.max_steer) +
              " forward_drive_samples=" + this.forward_drive_samples +
              " goal_progress=" + r2(this.max_goal_progress) +
              " max_goal_alignment=" + r2(this.max_goal_alignment) +
              " final_alignment=" + r2(final_alignment));
        this.done = true;
        report_verdict(checks, "AUTOPILOT FORWARD APPROACH", "AUTOPILOT_FORWARD_APPROACH");
    }
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        fail_fast("rear approach stopped before its verdict");
    }
}
