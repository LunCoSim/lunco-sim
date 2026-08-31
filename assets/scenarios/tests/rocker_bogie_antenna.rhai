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
            this.baseline = support_measurements(this.antenna, this.rover);
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
        let end = support_measurements(this.antenna, this.rover);
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
    f.push(t_present(s.baseline.deck, "rover solar roof"));
    f.push(t_present(s.baseline.wifi_housing, "Wi-Fi radio housing"));
    f.push(t_present(s.baseline.wifi_whip, "Wi-Fi radio whip"));
    f.push(t_present(s.baseline.science_mast, "science mast"));
    f.push(t_present(s.baseline.camera_head, "science camera head"));
    f.push(t_range(s.baseline.mast_to_yaw, 0.0, MAX_SUPPORT_GAP_M,
        "mast top is disconnected from the YawHead"));
    f.push(t_range(s.baseline.yoke_to_dish, 0.0, MAX_SUPPORT_GAP_M,
        "DishYoke is disconnected from the DishHead"));
    f.push(t_range(s.baseline.deck_to_antenna_mast, 0.0, MAX_SUPPORT_GAP_M,
        "antenna mast is floating above the rover roof"));
    f.push(t_range(s.baseline.deck_to_wifi_housing, 0.0, MAX_SUPPORT_GAP_M,
        "Wi-Fi radio housing is floating above the rover roof"));
    f.push(t_range(s.baseline.wifi_housing_to_whip, 0.0, MAX_SUPPORT_GAP_M,
        "Wi-Fi radio whip is disconnected from its housing"));
    f.push(t_range(s.baseline.science_mast_to_camera, 0.0, MAX_SUPPORT_GAP_M,
        "science camera head is disconnected from its mast"));
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
    f.push(t_range(s.end.deck_to_antenna_mast, 0.0, MAX_SUPPORT_GAP_M,
        "antenna mast separated from the rover roof during motion"));
    f.push(t_range(s.end.deck_to_wifi_housing, 0.0, MAX_SUPPORT_GAP_M,
        "Wi-Fi radio housing separated from the rover roof during motion"));
    f.push(t_range(s.end.wifi_housing_to_whip, 0.0, MAX_SUPPORT_GAP_M,
        "Wi-Fi radio whip separated from its housing during motion"));
    f.push(t_range(s.end.science_mast_to_camera, 0.0, MAX_SUPPORT_GAP_M,
        "science camera head separated from its mast during motion"));

    print("[rb-antenna] ── results ───────────────────────────────────");
    print("[rb-antenna]   rover travel     " + r2(s.travel) + " m");
    print("[rb-antenna]   mast→yaw gap     " + r4(s.end.mast_to_yaw) + " m");
    print("[rb-antenna]   yoke→dish gap    " + r4(s.end.yoke_to_dish) + " m");
    print("[rb-antenna]   roof→antenna gap  " + r4(s.end.deck_to_antenna_mast) + " m");
    print("[rb-antenna]   roof→radio gap    " + r4(s.end.deck_to_wifi_housing) + " m");
    print("[rb-antenna]   radio→whip gap    " + r4(s.end.wifi_housing_to_whip) + " m");
    print("[rb-antenna]   mast→camera gap   " + r4(s.end.science_mast_to_camera) + " m");
    report_verdict(f, "ROCKER BOGIE ANTENNA", "ROCKER_BOGIE_ANTENNA");
}
