// @timing presentation
// Near-ground movement crosses a native render-origin cell in both directions.
import "lunco://scripting/tests/lib/terrain_camera.rhai" as terrain_camera;
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [params.avatar_id], entity_writes: [],
       query_reads: ["TerrainLodStatus", "SceneCameraAudit"], required_inputs: [] }
}

fn render_position(camera) {
    let result = ();
    for row in query("SceneCameraAudit", #{}).candidates {
        if row.entity == camera {
            if result != () { throw "rebase camera identity is ambiguous"; }
            result = row.world_translation;
        }
    }
    if result == () { throw "rebase camera has no native transform"; }
    result
}

fn on_start(me, params) {
    let status = query("TerrainLodStatus", #{});
    this.camera = status.viewport_camera;
    let view = terrain_camera::main_cover(status, this.camera, ());
    if view == () || !view.ready { throw "rebase gate requires its settled camera cover"; }
    this.source = view.source;
    this.avatar = params.avatar_id;
    this.start = view.position_local_m;
    this.previous = this.start;
    this.previous_render = render_position(this.camera);
    this.minimum_clearance = view.clearance_m;
    this.maximum_drift = 0.0;
    this.jumped = false;
    this.phase = 0;
    this.done = false;
    this.started = clock_snapshot().wall_elapsed_s;
    this.checks = [t_true(abs(view.clearance_m - params.clearance_m)
        <= params.clearance_tolerance_m, "rebase starts at camera-scale clearance")];
    cmd("SimulateIntent", #{intent: "right", held: true, target: this.avatar});
}

fn on_tick(me, params) {
    if this.done { return; }
    let status = query("TerrainLodStatus", #{});
    let view = terrain_camera::main_cover(status, this.camera, this.source);
    if view == () { throw "rebase gate lost its exact output/source"; }
    let p = view.position_local_m;
    let rendered = render_position(this.camera);
    this.jumped = this.jumped
        || abs((rendered[0] - this.previous_render[0]) - (p[0] - this.previous[0])) > 1000.0;
    this.previous = p;
    this.previous_render = rendered;
    this.minimum_clearance = min(this.minimum_clearance, view.clearance_m);
    this.maximum_drift = max(this.maximum_drift, abs(p[1] - this.start[1]));
    let elapsed = clock_snapshot().wall_elapsed_s - this.started;
    if this.phase == 0 {
        if elapsed < 2.0 { return; }
        cmd("SimulateIntent", #{intent: "right", held: false, target: this.avatar});
        this.checks.push(t_true(p[0] - this.start[0] > 10.0,
            "near-ground camera moves across the positive cell boundary"));
        this.checks.push(t_true(this.jumped, "native render origin rebases during positive travel"));
        this.turn = p;
        this.jumped = false;
        this.phase = 1;
        this.started = clock_snapshot().wall_elapsed_s;
        cmd("SimulateIntent", #{intent: "left", held: true, target: this.avatar});
        return;
    }
    if this.phase == 1 {
        if elapsed < 2.0 { return; }
        cmd("SimulateIntent", #{intent: "left", held: false, target: this.avatar});
        this.checks.push(t_true(this.turn[0] - p[0] > 10.0,
            "near-ground camera moves across the negative cell boundary"));
        this.checks.push(t_true(this.jumped, "native render origin rebases during negative travel"));
        this.phase = 2;
        this.started = clock_snapshot().wall_elapsed_s;
        return;
    }
    if !view.ready || status.stream.pending != 0 {
        if elapsed <= 15.0 { return; }
    }
    this.checks.push(t_true(this.minimum_clearance >= params.clearance_m - params.clearance_tolerance_m,
        "both rebases preserve source clearance"));
    this.checks.push(t_true(this.maximum_drift <= params.maximum_lateral_altitude_drift_m,
        "both rebases preserve the declared lateral altitude bound"));
    this.checks.push(t_true(view.ready && status.stream.pending == 0,
        "the latest camera cover settles after both rebases"));
    this.done = true;
    print("rebase start=" + this.start + " turn=" + this.turn + " end=" + p
        + " minimum_clearance_m=" + this.minimum_clearance + " maximum_drift_m=" + this.maximum_drift);
    report_verdict(this.checks, "CAMERA TERRAIN REBASE", "CAMERA_TERRAIN_REBASE");
}

fn on_stop(me, params) {
    if this.avatar == () { return; }
    for intent in ["right", "left"] {
        cmd("SimulateIntent", #{intent: intent, held: false, target: this.avatar});
    }
}
