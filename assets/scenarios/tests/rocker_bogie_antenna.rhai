// ── Rocker-bogie antenna support ────────────────────────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/rocker_bogie_antenna.usda`.
//
// The rover owns one shared antenna component. Its mast reaches the YawHead,
// the DishYoke reaches the DishHead, and the complete assembly rides the rover
// during motion. The check reads the composed world poses rather than copying
// the USD values into a second implementation.

const SETTLE_S = 2.0;
const DRIVE_S = 6.0;
const MAX_SUPPORT_GAP_M = 0.08;
const MAX_DRIVE_DRIFT_M = 0.08;

fn point_gap(a, b) {
    if a == () || b == () { return -1.0; }
    vlen(vsub(a, b))
}

fn top_of(entity, half_height) {
    let p = world_pos(entity);
    let q = world_rotation(entity);
    if p == () || q == () { return (); }
    vadd(p, qrot(q, [0.0, half_height, 0.0]))
}

fn support_measurements(antenna) {
    let path = usd_path(antenna);
    if path == () { return #{}; }
    let mast = find(path + "/Mast");
    let yaw = find(path + "/YawHead");
    let yoke = find(path + "/YawHead/DishGimbal/DishYoke");
    let dish = find(path + "/YawHead/DishGimbal/DishHead");

    let mast_top = top_of(mast, 0.25);
    let yaw_pos = world_pos(yaw);
    let yoke_top = top_of(yoke, 0.05);
    let dish_pos = world_pos(dish);

    #{
        mast: mast,
        yaw: yaw,
        yoke: yoke,
        dish: dish,
        mast_to_yaw: point_gap(mast_top, yaw_pos),
        yoke_to_dish: point_gap(yoke_top, dish_pos),
    }
}

fn on_start(me) {
    this.t = 0.0;
    this.phase = "settle";
    this.rover = find("/RockerBogieAntennaTest/Rover");
    this.antenna = find("/RockerBogieAntennaTest/Rover/Antenna");
    this.baseline = ();
    this.rover_start = ();
    this.antenna_start = ();
    this.travel = 0.0;
    this.prev = ();
    print("[rb-antenna] ── mast/yoke/dish support check ────────────────");
    print("[rb-antenna] rover=" + this.rover + " antenna=" + this.antenna);
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    if this.rover == () || this.rover < 0 || this.antenna == () || this.antenna < 0 {
        fail_fast("rocker-bogie rover or antenna was not composed",
                  "ROCKER BOGIE ANTENNA", "ROCKER_BOGIE_ANTENNA");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let p = world_pos(this.rover);
    if p == () {
        fail_fast("rocker-bogie rover has no world pose",
                  "ROCKER BOGIE ANTENNA", "ROCKER_BOGIE_ANTENNA");
        this.phase = "done";
        return;
    }

    if this.phase == "settle" {
        brake(this.rover);
        if this.t >= SETTLE_S {
            this.phase = "drive";
            this.baseline = support_measurements(this.antenna);
            this.rover_start = p;
            this.antenna_start = world_pos(this.antenna);
            this.prev = p;
            print("[rb-antenna] baseline mast_to_yaw=" + r4(this.baseline.mast_to_yaw) +
                  " m yoke_to_dish=" + r4(this.baseline.yoke_to_dish) + " m");
        }
        return;
    }

    drive(this.rover, 1.0, 0.0);
    this.travel += vlen(vsub(p, this.prev));
    this.prev = p;

    if this.t >= SETTLE_S + DRIVE_S {
        this.phase = "done";
        brake(this.rover);
        let end = support_measurements(this.antenna);
        verdict(#{
            baseline: this.baseline,
            end: end,
            rover_start: this.rover_start,
            rover_end: p,
            antenna_start: this.antenna_start,
            antenna_end: world_pos(this.antenna),
            travel: this.travel,
        });
    }
}

fn verdict(s) {
    let f = [];
    f.push(t_present(s.baseline.mast, "antenna mast"));
    f.push(t_present(s.baseline.yaw, "antenna yaw head"));
    f.push(t_present(s.baseline.yoke, "antenna dish yoke"));
    f.push(t_present(s.baseline.dish, "antenna dish head"));
    f.push(t_range(s.baseline.mast_to_yaw, 0.0, MAX_SUPPORT_GAP_M,
        "mast top is disconnected from the YawHead"));
    f.push(t_range(s.baseline.yoke_to_dish, 0.0, MAX_SUPPORT_GAP_M,
        "DishYoke is disconnected from the DishHead"));
    f.push(t_moved(vlen(vsub(s.rover_end, s.rover_start)), 1.0,
        "rocker-bogie rover travel"));

    let rover_offset_start = vsub(s.antenna_start, s.rover_start);
    let rover_offset_end = vsub(s.antenna_end, s.rover_end);
    f.push(t_range(point_gap(rover_offset_start, rover_offset_end),
        0.0, MAX_DRIVE_DRIFT_M,
        "antenna mount drifted away from the moving rover"));
    f.push(t_range(s.end.mast_to_yaw, 0.0, MAX_SUPPORT_GAP_M,
        "mast top separated from the YawHead during motion"));
    f.push(t_range(s.end.yoke_to_dish, 0.0, MAX_SUPPORT_GAP_M,
        "DishYoke separated from the DishHead during motion"));

    print("[rb-antenna] ── results ───────────────────────────────────");
    print("[rb-antenna]   rover travel     " + r2(s.travel) + " m");
    print("[rb-antenna]   mast→yaw gap     " + r4(s.end.mast_to_yaw) + " m");
    print("[rb-antenna]   yoke→dish gap    " + r4(s.end.yoke_to_dish) + " m");
    report_verdict(f, "ROCKER BOGIE ANTENNA", "ROCKER_BOGIE_ANTENNA");
}
