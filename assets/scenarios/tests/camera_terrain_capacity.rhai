// @timing presentation
// A coarse view cannot admit descent into an unresolved authored dig.
import "lunco://scripting/tests/lib/terrain_camera.rhai" as terrain_camera;
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [params.avatar_id], entity_writes: [],
       query_reads: ["TerrainLodStatus", "RuntimeDiagnostics"], required_inputs: [] }
}

fn pose_within_tolerance(actual, expected, tolerance) {
    let dx = actual[0] - expected[0];
    let dy = actual[1] - expected[1];
    let dz = actual[2] - expected[2];
    dx * dx + dy * dy + dz * dz <= tolerance * tolerance
}

fn on_start(me, params) {
    if !(params.clearance_m > 0.0 && params.clearance_tolerance_m > 0.0
        && params.pose_tolerance_m > 0.0) {
        throw "capacity gate requires its declared camera clearance";
    }
    let status = query("TerrainLodStatus", #{});
    this.camera = status.viewport_camera;
    let view = terrain_camera::main_cover(status, this.camera, ());
    if view == () { throw "capacity gate requires its authored-dig output/source"; }
    this.source = view.source;
    this.budget = status.config.tile_budget;
    this.avatar = params.avatar_id;
    this.started = clock_snapshot().wall_elapsed_s;
    this.phase = -1;
    this.done = false;
    this.checks = [];
}

fn on_tick(me, params) {
    if this.done { return; }
    let status = query("TerrainLodStatus", #{});
    let view = terrain_camera::main_cover(status, this.camera, this.source);
    if view == () { throw "capacity gate lost its exact output/source"; }
    if clock_snapshot().wall_elapsed_s - this.started > 45.0 {
        this.done = true;
        report_verdict([t_true(false, "capacity refusal and recovery did not finish")],
            "CAMERA TERRAIN CAPACITY", "CAMERA_TERRAIN_CAPACITY");
        return;
    }
    if this.phase == -1 {
        if !view.ready || status.stream.pending != 0 { return; }
        let quality = cmd("SetTerrainRenderingQuality", #{tile_budget: 1});
        this.checks.push(t_true(quality != () && quality.ok, "small visual capacity is admitted"));
        this.checks.push(t_true(abs(view.clearance_m - 35.0) < 0.01,
            "the source includes the authored five-metre dig"));
        this.phase = 0;
        return;
    }
    if this.phase == 0 {
        if status.config.tile_budget != 1 || !view.selection_current
            || status.stream.pending != 0 || view.wanted != 1 || view.drawn != 1 { return; }
        this.checks.push(t_true(view.depth == 0 && view.budget_refused > 0,
            "the constrained cover retains its coarse root"));
        cmd("SimulateIntent", #{intent: "yaw_left", held: true, target: this.avatar});
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        if status.stream.preparation_refused == 0 { return; }
        this.refused_position = view.position_local_m;
        this.refused_native_position = world_pos(this.avatar);
        this.stable_samples = 0;
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        if status.stream.preparation_refused == 0 {
            this.stable_samples = 0;
            return;
        }
        if status.stream.pending != status.stream.selection_pending { return; }
        this.stable_samples += 1;
        if this.stable_samples < 3 { return; }
        let native_position = world_pos(this.avatar);
        print("capacity refusal source_snapshot=" + this.refused_position + " latest_snapshot=" + view.position_local_m
            + " native_start=" + this.refused_native_position + " native_end=" + native_position);
        this.checks.push(t_true(pose_within_tolerance(native_position, this.refused_native_position, params.pose_tolerance_m),
            "refused descent retains its admitted pose within the declared sweep tolerance"));
        this.checks.push(t_true(status.stream.pending == status.stream.selection_pending,
            "the exact refused request does not restart worker preparation"));
        let quality = cmd("SetTerrainRenderingQuality", #{tile_budget: this.budget});
        this.checks.push(t_true(quality != () && quality.ok, "capacity restoration is admitted"));
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        if view.clearance_m > 0.1 || !view.ready || status.stream.pending != 0 { return; }
        cmd("SimulateIntent", #{intent: "yaw_left", held: false, target: this.avatar});
        this.checks.push(t_true(this.refused_position[1] - view.position_local_m[1] > 4.0,
            "the camera descends into the resolved dig after capacity changes"));
        let diagnostics = query("RuntimeDiagnostics", #{});
        this.checks.push(t_true(status.stream.preparation_refused == 0
            && diagnostics.errors == 0 && diagnostics.warnings == 0,
            "the retired refusal leaves no stale count or camera error"));
        this.stationary_position = world_pos(this.avatar);
        this.stopped_samples = 0;
        this.phase = 6;
        return;
    }
    if this.phase == 6 {
        let position = world_pos(this.avatar);
        if !pose_within_tolerance(position, this.stationary_position, params.pose_tolerance_m) || !view.ready || status.stream.pending != 0 {
            this.stationary_position = position;
            this.stopped_samples = 0;
            return;
        }
        this.stopped_samples += 1;
        if this.stopped_samples < 2 { return; }
        let quality = cmd("SetTerrainRenderingQuality", #{tile_budget: 1});
        this.checks.push(t_true(quality != () && quality.ok,
            "stationary coarsening is submitted through the quality owner"));
        this.phase = 4;
        return;
    }
    if this.phase == 4 {
        if status.config.tile_budget != 1 || !view.presentation_pending
            || status.stream.presentation_pending == 0 || status.stream.pending == 0 { return; }
        this.checks.push(t_true(pose_within_tolerance(world_pos(this.avatar), this.stationary_position, params.pose_tolerance_m)
            && view.drawn > 1 && !view.ready && status.stream.pending > 0,
            "unsafe coarsening retains the fine cover and pose within the declared sweep tolerance"));
        print("stationary coarsening held at " + view.position_local_m
            + " clearance_m=" + view.clearance_m + " drawn=" + view.drawn
            + " native_start=" + this.stationary_position + " native_end=" + world_pos(this.avatar)
            + " ready=" + view.ready + " pending=" + status.stream.pending
            + " presentation_pending=" + status.stream.presentation_pending);
        let quality = cmd("SetTerrainRenderingQuality", #{tile_budget: this.budget});
        this.checks.push(t_true(quality != () && quality.ok,
            "restoring quality retires the blocked publication request"));
        this.phase = 5;
        return;
    }
    if !view.ready || status.stream.pending != 0 || status.stream.presentation_pending != 0 { return; }
    let diagnostics = query("RuntimeDiagnostics", #{});
    this.checks.push(t_true(pose_within_tolerance(world_pos(this.avatar), this.stationary_position, params.pose_tolerance_m)
        && view.clearance_m >= params.clearance_m - params.clearance_tolerance_m
        && diagnostics.errors == 0 && diagnostics.warnings == 0,
        "safe publication recovers within the pose tolerance without penetrating the camera"));
    this.done = true;
    report_verdict(this.checks, "CAMERA TERRAIN CAPACITY", "CAMERA_TERRAIN_CAPACITY");
}

fn on_stop(me, params) {
    if this.avatar != () {
        cmd("SimulateIntent", #{intent: "yaw_left", held: false, target: this.avatar});
    }
    if this.budget != () { cmd("SetTerrainRenderingQuality", #{tile_budget: this.budget}); }
}
