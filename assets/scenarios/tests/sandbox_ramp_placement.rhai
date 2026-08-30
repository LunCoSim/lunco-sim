// ── Sandbox ramp placement regression ───────────────────────────────────────
//
// The interactive sandbox owns the ramp geometry and the rover spawn.  This
// focused test composes that real scene, drives one raycast rover from its flat
// approach across the first ramp, and checks the authored grade is physically
// present in the observed world pose.

const SETTLE_S = 2.0;
const DRIVE_S = 28.0;

fn verdict(pass, reason) {
    if pass {
        emit("SANDBOX_RAMP_PLACEMENT", "PASS");
    } else {
        print("SANDBOX_RAMP_PLACEMENT FAIL: " + reason);
        emit("SANDBOX_RAMP_PLACEMENT", "FAIL");
    }
}

fn on_start(me) {
    this.t = 0.0;
    this.done = false;
    this.rover = find("/SandboxRampPlacement/Skid_Raycast_1");
    this.ramp = find("/SandboxRampPlacement/Ramp");
    this.start = ();
    this.min_y = 0.0;
    this.max_x = 0.0;
    if this.rover >= 0 && this.ramp >= 0 {
        this.start = world_pos(this.rover);
        if this.start != () {
            this.min_y = this.start[1];
            this.max_x = this.start[0];
        }
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();

    if this.rover < 0 || this.ramp < 0 || this.start == () {
        this.done = true;
        verdict(false, "focused sandbox ramp fixture is missing its rover, ramp, or start pose");
        return;
    }

    if this.t < SETTLE_S {
        return;
    }

    drive(this.rover, 0.55, 0.0);
    let p = world_pos(this.rover);
    if p != () {
        if p[1] < this.min_y { this.min_y = p[1]; }
        if p[0] > this.max_x { this.max_x = p[0]; }
    }

    if this.t >= SETTLE_S + DRIVE_S {
        this.done = true;
        drive(this.rover, 0.0, 0.0);
        if p == () {
            verdict(false, "rover lost its world pose during the ramp traverse");
            return;
        }
        let rise = p[1] - this.start[1];
        let progress = p[0] - this.start[0];
        print("[sandbox-ramp] start=" + this.start + " final=" + p +
              " progress_x=" + progress + " rise_y=" + rise +
              " min_y=" + this.min_y + " max_x=" + this.max_x);
        verdict(progress > 20.0 && rise > 1.0,
            "rover did not climb the authored ramp (progress_x=" + progress +
            ", rise_y=" + rise + ")");
    }
}
