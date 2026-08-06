// Drive acceptance for the two rocker-bogie control fixtures and the minimal
// flat-ground fixture. The scene owns the composed rover and ground; this
// scenario owns the explicit control writes and asserts real displacement.

const SETTLE_S = 3.0;
const DRIVE_S = 8.0;
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

    drive(this.rover, 1.0, 0.0);
    this.travel += seg(this.prev, p);
    this.prev = p;
    this.samples += 1;

    if this.t >= SETTLE_S + DRIVE_S {
        brake(this.rover);
        this.phase = "done";
        let net = seg(this.start, p);
        let fails = [];
        fails.push(t_bounded(p, 300.0, "rocker-bogie chassis"));
        fails.push(t_true(this.samples > 100,
            "drive phase produced only " + this.samples + " samples"));
        fails.push(t_moved(net, MIN_TRAVEL_M, "rocker-bogie net travel"));
        print("[rb-drive] net=" + r2(net) + " m path=" + r2(this.travel) + " m");
        report_verdict(fails, "ROCKER BOGIE DRIVE", "ROCKER_BOGIE_DRIVE");
    }
}
