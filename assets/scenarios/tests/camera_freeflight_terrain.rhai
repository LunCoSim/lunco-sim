// @timing presentation
// The free avatar remains above DEM terrain outside the rover's collider ring.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn on_start(me, params) {
    this.avatar = find(params.avatar_path);
    let status = query("TerrainLodStatus", #{});
    if this.avatar < 0 || status.camera_cover.len() != 1 {
        throw "free-flight terrain test needs one active avatar camera";
    }
    this.start = status.camera_cover[0].clearance_m;
    this.minimum = this.start;
    this.started = clock_snapshot().wall_elapsed_s;
    this.phase = 0;
    this.done = false;
    let result = cmd("SimulateIntent", #{intent: "yaw_left", held: true, target: this.avatar});
    if result == () || result.ok != true { throw "downward intent was rejected"; }
}

fn on_tick(me, params) {
    if this.done { return; }
    let status = query("TerrainLodStatus", #{});
    if status.camera_cover.len() != 1 { return; }
    let clearance = status.camera_cover[0].clearance_m;
    this.minimum = min(this.minimum, clearance);
    let elapsed = clock_snapshot().wall_elapsed_s - this.started;
    if this.phase == 1 {
        if elapsed < 1.0 { return; }
        cmd("SimulateIntent", #{intent: "right", held: false, target: this.avatar});
        let p = status.camera_cover[0].position_local_m;
        let dx = p[0] - this.floor_position[0];
        let dz = p[2] - this.floor_position[2];
        this.checks.push(t_true(dx * dx + dz * dz > 0.25, "free camera can move laterally near terrain"));
        this.checks.push(t_true(this.minimum >= 0.3, "lateral camera motion stays above terrain"));
        this.floor = clearance;
        this.phase = 2;
        this.started = clock_snapshot().wall_elapsed_s;
        cmd("SimulateIntent", #{intent: "yaw_right", held: true, target: this.avatar});
        return;
    }
    if this.phase == 2 {
        if elapsed < 1.0 { return; }
        cmd("SimulateIntent", #{intent: "yaw_right", held: false, target: this.avatar});
        this.checks.push(t_true(clearance - this.floor > 5.0, "free camera can move upward away from terrain"));
        this.done = true;
        report_verdict(this.checks, "CAMERA FREEFLIGHT TERRAIN", "CAMERA_FREEFLIGHT_TERRAIN");
        return;
    }
    if elapsed < 3.0 { return; }
    cmd("SimulateIntent", #{intent: "yaw_left", held: false, target: this.avatar});
    this.checks = [
        t_true(this.start > 10.0, "free camera starts above the terrain"),
        t_true(this.start - clearance > 5.0, "downward movement reaches the ground"),
        t_true(this.minimum >= 0.3, "free camera stays above the terrain while moving")
    ];
    this.floor = clearance;
    this.floor_position = status.camera_cover[0].position_local_m;
    this.phase = 1;
    this.started = clock_snapshot().wall_elapsed_s;
    cmd("SimulateIntent", #{intent: "right", held: true, target: this.avatar});
}
