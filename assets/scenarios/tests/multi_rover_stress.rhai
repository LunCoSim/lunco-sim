// Production stress-matrix correctness check for the authored 4/8/20 rover
// scenes. The scene owns the roster and lane positions; this script discovers
// that roster and exercises the same drive command for every vehicle.

const SETTLE_SECONDS = 3.0;
const DRIVE_SECONDS = 10.0;
const DRIVE_DISTANCE_M = 20.0;
const DRIVE_THROTTLE = 0.20;

fn on_start(me) {
    this.t = 0.0;
    this.phase = "settle";
    this.rovers = [];
    this.previous = [];
    this.distance = [];

    let group = find("/MultiRoverStress/Rovers");
    if group == () || group < 0 {
        print("MULTI-ROVER STRESS: FAIL — /MultiRoverStress/Rovers is absent");
        emit("MULTI_ROVER_STRESS", "FAIL");
        this.phase = "done";
        return;
    }
    for rover in children(group) {
        if world_pos(rover) != () {
            this.rovers.push(rover);
            this.previous.push(());
            this.distance.push(0.0);
        }
    }

    // A one-rover run is not a stress matrix. The exact 4/8/20 cardinality is
    // authored by each USD fixture; discovery keeps this reader reusable.
    if this.rovers.len() < 4 {
        print("MULTI-ROVER STRESS: FAIL — discovered only " + this.rovers.len() +
              " rover(s); minimum matrix cardinality is four");
        emit("MULTI_ROVER_STRESS", "FAIL");
        this.phase = "done";
        return;
    }

    print("[multi-rover-stress] discovered " + this.rovers.len() + " rover(s)");
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();

    if this.phase == "settle" {
        for rover in this.rovers { brake(rover); }
        if this.t >= SETTLE_SECONDS {
            for i in 0..this.rovers.len() {
                let at = world_pos(this.rovers[i]);
                this.previous[i] = at;
                if at != () {
                    engage_patrol(
                        this.rovers[i],
                        [[at[0], at[1], at[2] - DRIVE_DISTANCE_M]],
                        DRIVE_THROTTLE,
                        0.75,
                        ()
                    );
                }
            }
            this.phase = "drive";
            print("[multi-rover-stress] drive started for " + this.rovers.len() + " rover(s)");
        }
        return;
    }

    for i in 0..this.rovers.len() {
        let at = world_pos(this.rovers[i]);
        if at != () && this.previous[i] != () {
            this.distance[i] += seg(this.previous[i], at);
            this.previous[i] = at;
        }
    }

    if this.t < SETTLE_SECONDS + DRIVE_SECONDS { return; }
    for rover in this.rovers { this.clear_patrol(rover); }

    let checks = [];
    for i in 0..this.rovers.len() {
        let at = world_pos(this.rovers[i]);
        checks.push(t_true(at != (), "rover " + i + " was removed during the stress run"));
        if at != () {
            checks.push(t_bounded(at, 500.0, "rover " + i + " position"));
        }
        checks.push(t_true(this.distance[i] > 0.25,
            "rover " + i + " did not move under the shared patrol command"));
    }
    report_verdict(checks, "MULTI-ROVER STRESS", "MULTI_ROVER_STRESS");
    this.phase = "done";
}

fn on_stop(me) {
    for rover in this.rovers {
        if rover != () && rover >= 0 { this.clear_patrol(rover); }
    }
}
