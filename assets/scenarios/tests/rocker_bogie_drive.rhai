// Drive acceptance for the two rocker-bogie control fixtures and the minimal
// flat-ground fixture. The scene owns the composed rover and ground; this
// scenario owns the explicit control writes and asserts real displacement.

const SETTLE_S = 3.0;
const DRIVE_S = 8.0;
const STEER_S = 6.0;
const MIN_TRAVEL_M = 2.0;

fn on_start(me) {
    this.t = 0.0;
    this.phase = "settle";
    this.rover = find("/RockerBogieDrive/Rover");
    if this.rover == () || this.rover < 0 {
        this.rover = find("/RockerBogieFlat/Rover");
    }
    this.start = ();
    this.prev = ();
    this.travel = 0.0;
    this.samples = 0;
    this.fprev = ();
    this.yaw_steer = 0.0;
    this.yaw_samples = 0;
    print("[rb-drive] rover gid=" + this.rover);
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    if this.rover == () || this.rover < 0 {
        fail_fast("rocker-bogie rover was not composed", "ROCKER BOGIE DRIVE", "ROCKER_BOGIE_DRIVE");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let p = world_pos(this.rover);
    if p == () {
        fail_fast("rocker-bogie world pose disappeared", "ROCKER BOGIE DRIVE", "ROCKER_BOGIE_DRIVE");
        this.phase = "done";
        return;
    }

    if this.phase == "settle" {
        brake(this.rover);
        if this.t >= SETTLE_S {
            this.phase = "drive";
            this.start = p;
            this.prev = p;
            print("[rb-drive] settled; full-throttle drive started");
        }
        return;
    }

    if this.phase == "drive" {
        drive(this.rover, 1.0, 0.0);
        this.travel += seg(this.prev, p);
        this.prev = p;
        this.samples += 1;
        if this.t >= SETTLE_S + DRIVE_S {
            this.phase = "steer";
            this.phase_start = this.t;
            this.fprev = world_forward(this.rover);
            print("[rb-drive] steering leg started; positive steer must change heading");
        }
        return;
    }

    drive(this.rover, 1.0, 0.6);
    let fwd = world_forward(this.rover);
    if this.fprev != () && fwd != () {
        this.yaw_steer += yaw_delta_deg(this.fprev, fwd);
        this.yaw_samples += 1;
    }
    if fwd != () { this.fprev = fwd; }

    if this.t >= SETTLE_S + DRIVE_S + STEER_S {
        brake(this.rover);
        this.phase = "done";
        let net = seg(this.start, p);
        let fails = [];
        fails.push(t_bounded(p, 300.0, "rocker-bogie chassis"));
        fails.push(t_true(this.samples > 100,
            "drive phase produced only " + this.samples + " samples"));
        fails.push(t_moved(net, MIN_TRAVEL_M, "rocker-bogie net travel"));
        fails.push(t_true(this.yaw_samples > 20,
            "steering phase produced only " + this.yaw_samples + " heading samples"));
        fails.push(t_true(this.yaw_steer.abs() > 5.0,
            "positive steer changed heading by only " + r2(this.yaw_steer) + " deg"));
        print("[rb-drive] net=" + r2(net) + " m path=" + r2(this.travel) +
              " m steer_yaw=" + r2(this.yaw_steer) + " deg");
        report_verdict(fails, "ROCKER BOGIE DRIVE", "ROCKER_BOGIE_DRIVE");
    }
}
