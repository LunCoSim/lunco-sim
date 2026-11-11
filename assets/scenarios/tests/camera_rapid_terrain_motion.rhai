// @timing presentation
// Rapid view changes preserve clearance and settle the latest camera cover.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn set_pose(index) {
    let yaws = [0.0, PI(), PI() / 2.0, -PI() / 2.0];
    let distances = [80.0, 150.0, 100.0, 130.0];
    let heights = [32.0, 72.0, 48.0, 64.0];
    let yaw = yaws[index % yaws.len()] + (index / 4).to_float() * 0.13;
    let slot = index % distances.len();
    let result = cmd("SetCameraFollow", #{
        distance_m: distances[slot], height_m: heights[slot], yaw_rad: yaw, pitch_rad: -0.05
    });
    if result == () || result.ok != true { throw "rapid camera pose was rejected"; }
}

fn on_start(me, params) {
    this.index = 0;
    this.samples = 0;
    this.all_clear = true;
    this.all_cover_ready = true;
    this.last_position = ();
    this.maximum_motion_m = 0.0;
    this.final_sent = false;
    this.settled_started = 0.0;
    this.done = false;
    set_pose(0);
}

fn on_tick(me, params) {
    if this.done { return; }
    let status = query("TerrainLodStatus", #{});
    let cameras = status.camera_cover;
    if cameras.len() == 1 {
        let camera = cameras[0];
        let position = camera.position_local_m;
        this.samples += 1;
        this.all_clear = this.all_clear && camera.clearance_m > 0.1;
        this.all_cover_ready = this.all_cover_ready && camera.ready;
        if this.last_position != () {
            let dx = position[0] - this.last_position[0];
            let dy = position[1] - this.last_position[1];
            let dz = position[2] - this.last_position[2];
            this.maximum_motion_m = max(this.maximum_motion_m, sqrt(dx * dx + dy * dy + dz * dz));
        }
        this.last_position = position;
    }

    if this.index < 31 {
        this.index += 1;
        set_pose(this.index);
        return;
    }

    if !this.final_sent {
        let result = cmd("SetCameraFollow", #{
            distance_m: 120.0, height_m: 55.0, yaw_rad: 2.35, pitch_rad: -0.05
        });
        if result == () || result.ok != true { throw "final camera pose was rejected"; }
        this.final_sent = true;
        this.settled_started = clock_snapshot().wall_elapsed_s;
        return;
    }

    let settled = cameras.len() == 1 && cameras[0].ready
        && status.stream.pending.to_int() == 0
        && status.stream.focus_resident.to_int() >= status.stream.focus_wanted.to_int();
    let far_lod = cameras.len() == 1
        && cameras[0].depth.to_int() < status.config.max_depth.to_int();
    if settled {
        print("camera rapid motion samples=" + this.samples
            + " moved_m=" + this.maximum_motion_m
            + " all_clear=" + this.all_clear
            + " all_cover_ready=" + this.all_cover_ready);
        this.done = true;
        report_verdict([
            t_true(this.samples >= 16 && this.maximum_motion_m > 5.0,
                "the burst moved the active camera across distinct terrain views"),
            t_true(this.all_clear, "camera terrain clearance held through the burst"),
            t_true(status.stream.pending.to_int() == 0
                && status.stream.focus_resident.to_int() >= status.stream.focus_wanted.to_int(),
                "latest camera detail settled with no pending terrain work"),
            t_true(cameras.len() == 1 && cameras[0].ready,
                "latest camera cover is ready after the burst"),
            t_true(far_lod, "the distant camera uses a coarser terrain LOD")
        ], "CAMERA RAPID TERRAIN MOTION", "CAMERA_RAPID_TERRAIN_MOTION");
    } else if clock_snapshot().wall_elapsed_s - this.settled_started > 15.0 {
        print("camera rapid motion timeout samples=" + this.samples
            + " moved_m=" + this.maximum_motion_m
            + " all_clear=" + this.all_clear
            + " all_cover_ready=" + this.all_cover_ready);
        this.done = true;
        report_verdict([
            t_true(this.samples >= 16 && this.maximum_motion_m > 5.0,
                "the burst moved the active camera across distinct terrain views"),
            t_true(this.all_clear, "camera terrain clearance held through the burst"),
            t_true(false, "latest camera terrain work did not settle within 15 seconds"),
            t_true(cameras.len() == 1 && cameras[0].ready,
                "latest camera cover is ready after the burst"),
            t_true(far_lod, "the distant camera uses a coarser terrain LOD")
        ], "CAMERA RAPID TERRAIN MOTION", "CAMERA_RAPID_TERRAIN_MOTION");
    }
}
