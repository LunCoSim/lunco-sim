// @timing presentation
// A long follow boom must remain above terrain beyond the physical collider ring.
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
    let cameras = status.camera_cover;
    this.clear = this.clear && cameras.len() == 1 && cameras[0].clearance_m > 0.1;
    if clock_snapshot().wall_elapsed_s - this.started < 2.0 { return; }
    this.checks.push(t_true(this.clear, "follow camera clears terrain at yaw " + this.yaws[this.index]));
    this.index += 1;
    if this.index == this.yaws.len() {
        report_verdict(this.checks, "CAMERA TERRAIN FOLLOW", "CAMERA_TERRAIN_FOLLOW");
        return;
    }
    set_direction(this.yaws[this.index]);
    this.clear = true;
    this.started = clock_snapshot().wall_elapsed_s;
}
