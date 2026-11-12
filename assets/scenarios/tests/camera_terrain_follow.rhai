// @timing presentation
// A long follow boom must remain above terrain beyond the physical collider ring.
import "lunco://scripting/tests/lib/terrain_camera.rhai" as terrain_camera;

const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn set_direction(yaw) {
    let result = cmd("SetCameraFollow", #{ distance_m: 100.0, height_m: 8.0,
        yaw_rad: yaw, pitch_rad: 0.18 });
    if result == () || result.ok != true { throw "follow direction was rejected"; }
}

fn on_start(me, params) {
    if !(params.clearance_tolerance_m > 0.0 && params.maximum_preparation_wait_s >= 2.0) { throw "camera gate requires a declared clearance tolerance"; }
    let status = query("TerrainLodStatus", #{});
    this.camera = status.viewport_camera;
    let camera = terrain_camera::main_cover(status, this.camera, ());
    if camera == () { throw "follow gate needs a main viewport terrain cover"; }
    this.source = camera.source;
    this.phase_start_position = camera.position_local_m;
    this.yaws = [0.0, 0.8, 1.6, 2.4, 3.2, 4.0, 4.8, 5.6];
    this.index = 0;
    this.checks = [];
    this.clear = true;
    set_direction(this.yaws[0]);
    this.started = clock_snapshot().wall_elapsed_s;
}

fn on_tick(me, params) {
    if this.index >= this.yaws.len() { return; }
    let status = query("TerrainLodStatus", #{});
    let camera = terrain_camera::main_cover(status, this.camera, this.source);
    this.clear = this.clear && terrain_camera::above_ground(camera, params.clearance_tolerance_m);
    let elapsed = clock_snapshot().wall_elapsed_s - this.started;
    if elapsed < 2.0 { return; }
    let moved = false;
    if camera != () {
        let p = camera.position_local_m;
        let start = this.phase_start_position;
        let dx = p[0] - start[0]; let dy = p[1] - start[1]; let dz = p[2] - start[2];
        moved = dx * dx + dy * dy + dz * dz > 1.0;
        if !moved && status.stream.pending > 0 && elapsed < params.maximum_preparation_wait_s { return; }
        print("follow yaw=" + this.yaws[this.index] + " moved_m="
            + sqrt(dx * dx + dy * dy + dz * dz) + " clearance_m=" + camera.clearance_m
            + " clear=" + this.clear + " eye=" + p + " elapsed_s=" + elapsed
            + " pending=" + status.stream.pending + " refused=" + status.stream.preparation_refused);
        this.phase_start_position = p;
    }
    this.checks.push(t_true(this.clear && moved,
        "follow camera moves and clears terrain at yaw " + this.yaws[this.index]));
    this.index += 1;
    if this.index == this.yaws.len() {
        report_verdict(this.checks, "CAMERA TERRAIN FOLLOW", "CAMERA_TERRAIN_FOLLOW");
        return;
    }
    set_direction(this.yaws[this.index]);
    this.clear = true;
    this.started = clock_snapshot().wall_elapsed_s;
}
