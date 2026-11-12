// @timing presentation
// Free camera descent reaches declared clearance; near-flat lateral travel has bounded drift.
import "lunco://scripting/tests/lib/terrain_camera.rhai" as terrain_camera;

const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [params.avatar_id, params.support_id], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn on_start(me, params) {
    if !(params.clearance_m >= 0.0 && params.clearance_tolerance_m > 0.0
        && params.maximum_lateral_altitude_drift_m >= 0.0) {
        throw "free-flight gate requires clearance and lateral-drift tolerances";
    }
    this.minimum_clearance = max(0.0, params.clearance_m - params.clearance_tolerance_m);
    this.maximum_clearance = params.clearance_m + params.clearance_tolerance_m;
    this.avatar = params.avatar_id;
    let status = query("TerrainLodStatus", #{});
    this.camera = status.viewport_camera;
    let camera = terrain_camera::main_cover(status, this.camera, ());
    if this.avatar == () || camera == () {
        throw "free-flight terrain test needs a main viewport terrain cover";
    }
    this.source = camera.source;
    if !(params.source_half_extent_m > 0.0 && params.minimum_support_distance_m > 0.0)
        || params.support_id == () { throw "free-flight gate requires its physical-support fixture"; }
    let eye = world_pos(this.avatar);
    let support = world_pos(params.support_id);
    let dx = eye[0] - support[0]; let dz = eye[2] - support[2];
    let support_under_camera = false;
    let support_present = false;
    for tile in status.contacts {
        if tile.source != this.source { continue; }
        support_present = support_present || tile.ready;
        let side = 2.0 * params.source_half_extent_m / (2.0 ** tile.depth.to_float());
        let x0 = -params.source_half_extent_m + tile.x.to_float() * side;
        let z0 = -params.source_half_extent_m + tile.z.to_float() * side;
        let p = camera.position_local_m;
        support_under_camera = support_under_camera
            || (p[0] >= x0 && p[0] <= x0 + side && p[2] >= z0 && p[2] <= z0 + side);
    }
    this.fixture_checks = [
        t_true(dx * dx + dz * dz >= params.minimum_support_distance_m * params.minimum_support_distance_m,
            "free camera starts far from the physical support body"),
        t_true(support_present && !support_under_camera,
            "native terrain support exists only away from the camera")
    ];
    this.start = camera.clearance_m;
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
    let camera = terrain_camera::main_cover(status, this.camera, this.source);
    if camera == () { throw "free-flight main viewport or terrain source changed"; }
    let clearance = camera.clearance_m;
    this.minimum = min(this.minimum, clearance);
    let elapsed = clock_snapshot().wall_elapsed_s - this.started;
    if this.phase == 1 {
        let p = camera.position_local_m;
        this.maximum_lateral_drift = max(this.maximum_lateral_drift,
            abs(p[1] - this.floor_position[1]));
        if elapsed < 1.0 { return; }
        cmd("SimulateIntent", #{intent: "right", held: false, target: this.avatar});
        let dx = p[0] - this.floor_position[0];
        let dz = p[2] - this.floor_position[2];
        this.checks.push(t_true(dx * dx + dz * dz > 0.25, "free camera can move laterally near terrain"));
        this.checks.push(t_true(this.minimum >= this.minimum_clearance, "lateral camera motion stays above terrain"));
        this.checks.push(t_true(this.maximum_lateral_drift <= params.maximum_lateral_altitude_drift_m,
            "lateral input stays within the declared altitude-drift bound"));
        this.checks.push(t_true(clearance <= this.maximum_clearance,
            "lateral travel retains camera-scale ground clearance"));
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
    this.checks = this.fixture_checks;
    this.checks.push(t_true(this.start > 10.0, "free camera starts above the terrain"));
    for check in [
        t_true(this.start - clearance > 5.0, "downward movement approaches the ground"),
        t_true(clearance <= this.maximum_clearance, "descent reaches the declared camera clearance"),
        t_true(this.minimum >= this.minimum_clearance, "free camera stays above the terrain while moving")
    ] { this.checks.push(check); }
    this.floor = clearance;
    this.floor_position = camera.position_local_m;
    this.maximum_lateral_drift = 0.0;
    this.phase = 1;
    this.started = clock_snapshot().wall_elapsed_s;
    cmd("SimulateIntent", #{intent: "right", held: true, target: this.avatar});
}

fn on_stop(me, params) {
    if this.avatar == () { return; }
    for intent in ["yaw_left", "right", "yaw_right"] {
        cmd("SimulateIntent", #{intent: intent, held: false, target: this.avatar});
    }
}
