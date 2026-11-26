// @timing presentation
// The driver pauses transport and settles its camera outputs before launching.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn tile_keys(status) {
    let keys = #{};
    for tile in status.tiles {
        keys[tile.camera.to_string() + ":" + tile.entity.to_string()] = true;
    }
    keys
}

fn on_start(me, params) {
    if params.duration_s <= 0.0 { throw "stationary cover gate requires a positive duration"; }
    let status = query("TerrainLodStatus", #{include_geometry: true, vertex_limit: 1, index_limit: 1});
    this.views = status.camera_cover;
    this.tiles = tile_keys(status);
    this.started = clock_snapshot().wall_elapsed_s;
    this.samples = 0;
    this.complete = this.views.len() > 0 && this.tiles.len() > 0;
    this.stable_pose = true;
    this.stable_tiles = true;
    this.done = false;
}

fn on_tick(me, params) {
    if this.done == () || this.done { return; }
    let now = clock_snapshot().wall_elapsed_s;
    this.samples += 1;
    let status = query("TerrainLodStatus", #{});
    this.complete = this.complete && status.stream.pending == 0
        && status.camera_cover.len() == this.views.len();
    for view in status.camera_cover {
        this.complete = this.complete && view.ready && view.wanted > 0
            && view.wanted == view.resident && view.wanted == view.drawn;
        let matched = false;
        for original in this.views {
            if original.camera == view.camera && original.source == view.source {
                matched = true;
                this.stable_pose = this.stable_pose
                    && original.position_local_m == view.position_local_m
                    && original.forward_local == view.forward_local;
                this.stable_tiles = this.stable_tiles && original.draw_revision == view.draw_revision;
            }
        }
        this.stable_pose = this.stable_pose && matched;
    }
    if now - this.started < params.duration_s { return; }
    this.done = true;
    let final_status = query("TerrainLodStatus", #{include_geometry: true, vertex_limit: 1, index_limit: 1});
    this.stable_tiles = this.stable_tiles && tile_keys(final_status) == this.tiles;
    report_verdict([
        t_true(this.samples > 1 && this.complete, "stationary camera covers remain fully published"),
        t_true(this.stable_pose, "the tested camera poses and source identities remain stationary"),
        t_true(this.stable_tiles, "drawn tile identities do not regress or churn")
    ], "TERRAIN STATIONARY COVER", "TERRAIN_STATIONARY_COVER");
}
