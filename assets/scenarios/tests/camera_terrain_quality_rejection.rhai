// @timing presentation
// Invalid visual lattices reject atomically through the public command owner.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn on_start(me, params) {
    let before = query("TerrainLodStatus", #{}).config;
    let checks = [];
    for request in [#{tile_resolution: 4}, #{cinematic_resolution: 4},
                    #{tile_resolution: 65, max_depth: 0}] {
        let result = cmd("SetTerrainRenderingQuality", request);
        checks.push(t_true(result != () && result.ok == false,
            "invalid terrain quality produces a terminal rejection"));
        let after = query("TerrainLodStatus", #{}).config;
        checks.push(t_true(after.tile_resolution == before.tile_resolution
            && after.cinematic_resolution == before.cinematic_resolution
            && after.max_depth == before.max_depth,
            "rejected quality leaves the admitted visual contract unchanged"));
    }
    report_verdict(checks, "CAMERA TERRAIN QUALITY REJECTION", "CAMERA_TERRAIN_QUALITY_REJECTION");
}
