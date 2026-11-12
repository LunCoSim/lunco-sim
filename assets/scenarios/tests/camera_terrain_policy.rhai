// @timing presentation
// Run only in the driver-created scratch Twin; policy writes are persistent.
import "lunco://scripting/tests/lib/terrain_camera.rhai" as terrain_camera;
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [params.avatar_id], entity_writes: [],
       query_reads: ["TerrainLodStatus", "RuntimeDiagnostics"], required_inputs: [] }
}

fn on_start(me, params) {
    if twin_source_identity() != params.twin_source_identity || get_twin_setting("avatar.allow_through_soil") != () {
        throw "camera policy gate requires its fresh scratch Twin";
    }
    this.avatar = params.avatar_id;
    this.twin = params.twin_source_identity;
    let status = query("TerrainLodStatus", #{});
    this.camera = status.viewport_camera;
    let view = terrain_camera::main_cover(status, this.camera, ());
    if view == () || !view.ready { throw "camera policy gate requires a settled flat view"; }
    this.source = view.source;
    this.checks = [t_true(get(this.avatar, "CameraCollisionEnvelope.radius_m") != (),
        "the default policy publishes its enforced camera envelope")];
    this.changed = set_twin_setting("avatar.allow_through_soil", true);
    this.checks.push(t_true(this.changed, "the explicit traversal policy is admitted"));
    this.phase = 0;
    this.started = clock_snapshot().wall_elapsed_s;
    this.done = false;
}

fn on_tick(me, params) {
    if this.done { return; }
    if twin_source_identity() != this.twin { throw "camera policy gate lost its scratch Twin"; }
    let status = query("TerrainLodStatus", #{});
    let view = terrain_camera::main_cover(status, this.camera, this.source);
    if view == () { throw "camera policy gate lost its output/source"; }
    let elapsed = clock_snapshot().wall_elapsed_s - this.started;
    if elapsed > 30.0 { throw "camera policy participation did not converge"; }
    if this.phase == 0 {
        if get(this.avatar, "CameraCollisionEnvelope.radius_m") != () { return; }
        this.checks.push(t_true(get_twin_setting("avatar.allow_through_soil") == true,
            "navigation and publication withdraw the same enforced envelope"));
        cmd("SimulateIntent", #{intent: "yaw_left", held: true, target: this.avatar});
        this.started = clock_snapshot().wall_elapsed_s;
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        if elapsed < 2.0 { return; }
        cmd("SimulateIntent", #{intent: "yaw_left", held: false, target: this.avatar});
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        if !view.ready || status.stream.pending != 0 { return; }
        this.checks.push(t_true(view.clearance_m < -5.0 && status.stream.presentation_pending == 0,
            "explicit through-soil traversal also permits terrain publication below ground"));
        let reset = cmd("ResetTwinSetting", #{key: "avatar.allow_through_soil"});
        this.reset_admitted = reset != () && reset.ok;
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        if get(this.avatar, "CameraCollisionEnvelope.radius_m") == () { return; }
        this.checks.push(t_true(this.reset_admitted && get_twin_setting("avatar.allow_through_soil") == (),
            "restoring the omitted-value policy republishes collision participation"));
        cmd("SimulateIntent", #{intent: "yaw_right", held: true, target: this.avatar});
        this.started = clock_snapshot().wall_elapsed_s;
        this.phase = 4;
        return;
    }
    if this.phase == 4 {
        if elapsed < 2.0 { return; }
        cmd("SimulateIntent", #{intent: "yaw_right", held: false, target: this.avatar});
        this.phase = 5;
        return;
    }
    if !view.ready || status.stream.pending != 0 { return; }
    let diagnostics = query("RuntimeDiagnostics", #{});
    this.checks.push(t_true(view.clearance_m > 10.0 && status.stream.presentation_pending == 0
        && diagnostics.errors == 0 && diagnostics.warnings == 0,
        "explicit upward recovery clears the terrain and its publication wait"));
    this.done = true;
    report_verdict(this.checks, "CAMERA TERRAIN POLICY", "CAMERA_TERRAIN_POLICY");
}

fn on_stop(me, params) {
    if this.avatar != () {
        for intent in ["yaw_left", "yaw_right"] {
            cmd("SimulateIntent", #{intent: intent, held: false, target: this.avatar});
        }
    }
    if this.changed == true && twin_source_identity() == this.twin {
        cmd("ResetTwinSetting", #{key: "avatar.allow_through_soil"});
    }
}
