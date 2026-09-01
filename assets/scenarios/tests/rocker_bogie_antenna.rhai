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

fn child(parent, wanted) {
    if parent == () || parent < 0 { return -1; }
    for id in children(parent) {
        if name(id) == wanted { return id; }
    }
    -1
}

fn top_of(entity, half_height) {
    if entity == () || entity < 0 { return (); }
    let p = world_pos(entity);
    let q = world_rotation(entity);
    if p == () || q == () { return (); }
    vadd(p, qrot(q, [0.0, half_height, 0.0]))
}

fn bottom_of(entity, half_height) {
    if entity == () || entity < 0 { return (); }
    let p = world_pos(entity);
    let q = world_rotation(entity);
    if p == () || q == () { return (); }
    vadd(p, qrot(q, [0.0, -half_height, 0.0]))
}

fn roof_gap(deck, deck_top, part_base) {
    if deck == () || deck < 0 || deck_top == () || part_base == () {
        return -1.0;
    }
    let deck_pos = world_pos(deck);
    let deck_q = world_rotation(deck);
    if deck_pos == () || deck_q == () { return -1.0; }
    let deck_inv = [-deck_q[0], -deck_q[1], -deck_q[2], deck_q[3]];
    let deck_top_local = qrot(deck_inv, vsub(deck_top, deck_pos))[1];
    let part_base_local = qrot(deck_inv, vsub(part_base, deck_pos))[1];
    let gap = deck_top_local - part_base_local;
    if gap < 0.0 { 0.0 } else { gap }
}

fn support_measurements(antenna, rover) {
    if antenna == () || antenna < 0 || rover == () || rover < 0 { return #{}; }
    let yaw = child(antenna, "Yaw Head");
    let gimbal = child(yaw, "Dish Gimbal");
    let mast = child(antenna, "Mast");
    let yoke = child(gimbal, "Dish Yoke");
    let dish = child(gimbal, "Dish Head");
    let deck = child(rover, "Solar Deck");
    let wifi = child(rover, "Wifi Radio");
    let wifi_housing = child(wifi, "Radio Housing");
    let wifi_whip = child(wifi, "Whip");
    let science_mast = child(rover, "Mast");
    let camera_head = child(rover, "Camera Head");

    let mast_top = top_of(mast, 0.25);
    let antenna_mast_base = bottom_of(mast, 0.25);
    let yaw_pos = world_pos(yaw);
    let yoke_top = top_of(yoke, 0.05);
    let dish_pos = world_pos(dish);
    let deck_top = top_of(deck, 0.02);
    let wifi_housing_bottom = bottom_of(wifi_housing, 0.03);
    let wifi_housing_top = top_of(wifi_housing, 0.03);
    let wifi_whip_bottom = bottom_of(wifi_whip, 0.12);
    let science_mast_top = top_of(science_mast, 0.5);
    let camera_head_bottom = bottom_of(camera_head, 0.15);

    #{
        mast: mast,
        yaw: yaw,
        yoke: yoke,
        dish: dish,
        deck: deck,
        wifi_housing: wifi_housing,
        wifi_whip: wifi_whip,
        science_mast: science_mast,
        camera_head: camera_head,
        mast_to_yaw: point_gap(mast_top, yaw_pos),
        yoke_to_dish: point_gap(yoke_top, dish_pos),
        deck_to_antenna_mast: roof_gap(deck, deck_top, antenna_mast_base),
        deck_to_wifi_housing: roof_gap(deck, deck_top, wifi_housing_bottom),
        wifi_housing_to_whip: point_gap(wifi_housing_top, wifi_whip_bottom),
        science_mast_to_camera: point_gap(science_mast_top, camera_head_bottom),
    }
}

fn rover_specs() {
    [
        #{
            id: "infinite",
            root: "/RockerBogieAntennaTest/Rover",
        },
        #{
            id: "battery",
            root: "/RockerBogieAntennaTest/RoverBattery",
        },
    ]
}

fn on_start(me) {
    this.t = 0.0;
    this.phase = "settle";
    this.specs = rover_specs();
    this.rovers = [];
    this.antennas = [];
    this.baselines = [];
    this.rover_start = [];
    this.antenna_start = [];
    this.travel = [];
    this.prev = [];
    for spec in this.specs {
        this.rovers.push(find(spec.root));
        this.antennas.push(find(spec.root + "/Antenna"));
        this.baselines.push(());
        this.rover_start.push(());
        this.antenna_start.push(());
        this.travel.push(0.0);
        this.prev.push(());
    }
    print("[rb-antenna] ── shared hardware across power variants ────────");
    for i in 0..this.specs.len() {
        print("[rb-antenna] " + this.specs[i].id + " rover=" + this.rovers[i] +
              " antenna=" + this.antennas[i]);
    }
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();
    for i in 0..this.specs.len() {
        if this.rovers[i] == () || this.rovers[i] < 0 ||
           this.antennas[i] == () || this.antennas[i] < 0 {
            fail_fast("" + this.specs[i].id +
                      " rocker-bogie rover or antenna was not composed",
                      "ROCKER BOGIE ANTENNA", "ROCKER_BOGIE_ANTENNA");
            this.phase = "done";
            return;
        }
    }

    if this.phase == "settle" {
        for rover in this.rovers { brake(rover); }
        if this.t >= SETTLE_S {
            this.phase = "drive";
            for i in 0..this.specs.len() {
                let p = world_pos(this.rovers[i]);
                this.baselines[i] = support_measurements(this.antennas[i], this.rovers[i]);
                this.rover_start[i] = p;
                this.antenna_start[i] = world_pos(this.antennas[i]);
                this.prev[i] = p;
                print("[rb-antenna] " + this.specs[i].id +
                      " baseline mast_to_yaw=" + r4(this.baselines[i].mast_to_yaw) +
                      " m yoke_to_dish=" + r4(this.baselines[i].yoke_to_dish) + " m");
            }
        }
        return;
    }

    for i in 0..this.specs.len() {
        let p = world_pos(this.rovers[i]);
        drive(this.rovers[i], 1.0, 0.0);
        this.travel[i] += vlen(vsub(p, this.prev[i]));
        this.prev[i] = p;
    }

    if this.t >= SETTLE_S + DRIVE_S {
        this.phase = "done";
        let snapshots = [];
        for i in 0..this.specs.len() {
            brake(this.rovers[i]);
            snapshots.push(#{
                id: this.specs[i].id,
                baseline: this.baselines[i],
                end: support_measurements(this.antennas[i], this.rovers[i]),
                rover_start: this.rover_start[i],
                rover_end: world_pos(this.rovers[i]),
                antenna_start: this.antenna_start[i],
                antenna_end: world_pos(this.antennas[i]),
                travel: this.travel[i],
            });
        }
        verdict(snapshots);
    }
}

fn variant_verdict(s) {
    let f = [];
    let label = "[" + s.id + "] ";
    let b = s.baseline;
    let e = s.end;
    f.push(t_present(b.mast, label + "antenna mast"));
    f.push(t_present(b.yaw, label + "antenna yaw head"));
    f.push(t_present(b.yoke, label + "antenna dish yoke"));
    f.push(t_present(b.dish, label + "antenna dish head"));
    f.push(t_present(b.deck, label + "rover solar roof"));
    f.push(t_present(b.wifi_housing, label + "Wi-Fi radio housing"));
    f.push(t_present(b.wifi_whip, label + "Wi-Fi radio whip"));
    f.push(t_present(b.science_mast, label + "science mast"));
    f.push(t_present(b.camera_head, label + "science camera head"));
    f.push(t_range(b.mast_to_yaw, 0.0, MAX_SUPPORT_GAP_M,
        label + "mast top is disconnected from the YawHead"));
    f.push(t_range(b.yoke_to_dish, 0.0, MAX_SUPPORT_GAP_M,
        label + "DishYoke is disconnected from the DishHead"));
    f.push(t_range(b.deck_to_antenna_mast, 0.0, MAX_SUPPORT_GAP_M,
        label + "antenna mast is floating above the rover roof"));
    f.push(t_range(b.deck_to_wifi_housing, 0.0, MAX_SUPPORT_GAP_M,
        label + "Wi-Fi radio housing is floating above the rover roof"));
    f.push(t_range(b.wifi_housing_to_whip, 0.0, MAX_SUPPORT_GAP_M,
        label + "Wi-Fi radio whip is disconnected from its housing"));
    f.push(t_range(b.science_mast_to_camera, 0.0, MAX_SUPPORT_GAP_M,
        label + "science camera head is disconnected from its mast"));
    f.push(t_moved(vlen(vsub(s.rover_end, s.rover_start)), 1.0,
        label + "rocker-bogie rover travel"));

    let rover_offset_start = vsub(s.antenna_start, s.rover_start);
    let rover_offset_end = vsub(s.antenna_end, s.rover_end);
    f.push(t_range(point_gap(rover_offset_start, rover_offset_end),
        0.0, MAX_DRIVE_DRIFT_M,
        label + "antenna mount drifted away from the moving rover"));
    f.push(t_range(e.mast_to_yaw, 0.0, MAX_SUPPORT_GAP_M,
        label + "mast top separated from the YawHead during motion"));
    f.push(t_range(e.yoke_to_dish, 0.0, MAX_SUPPORT_GAP_M,
        label + "DishYoke separated from the DishHead during motion"));
    f.push(t_range(e.deck_to_antenna_mast, 0.0, MAX_SUPPORT_GAP_M,
        label + "antenna mast separated from the rover roof during motion"));
    f.push(t_range(e.deck_to_wifi_housing, 0.0, MAX_SUPPORT_GAP_M,
        label + "Wi-Fi radio housing separated from the rover roof during motion"));
    f.push(t_range(e.wifi_housing_to_whip, 0.0, MAX_SUPPORT_GAP_M,
        label + "Wi-Fi radio whip separated from its housing during motion"));
    f.push(t_range(e.science_mast_to_camera, 0.0, MAX_SUPPORT_GAP_M,
        label + "science camera head separated from its mast during motion"));

    print("[rb-antenna] " + s.id + " travel=" + r2(s.travel) +
          " m mast→yaw=" + r4(e.mast_to_yaw) +
          " m yoke→dish=" + r4(e.yoke_to_dish) +
          " m roof→antenna=" + r4(e.deck_to_antenna_mast) +
          " m roof→radio=" + r4(e.deck_to_wifi_housing) + " m");
    f
}

fn verdict(snapshots) {
    let f = [];
    for s in snapshots {
        let checks = variant_verdict(s);
        for check in checks { f.push(check); }
    }
    report_verdict(f, "ROCKER BOGIE ANTENNA", "ROCKER_BOGIE_ANTENNA");
}
