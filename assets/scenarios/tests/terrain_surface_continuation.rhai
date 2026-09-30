// Run on an admitted celestial DEM scene. Mission heights are the independent
// reference for the generated exterior's entire native posting perimeter.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus", "TerrainHeights"], required_inputs: [] }
}

fn boundary_matches(error_m) { error_m < 0.005 }

fn on_start(me, ctx) {
    let failures = [];
    let status = query("TerrainLodStatus", #{ include_geometry: true, vertex_limit: 1, index_limit: 1 });
    if status == () || status.continuations == () {
        print(status);
        report_verdict(["TerrainLodStatus did not expose the continuation"], "DEM SURFACE CONTINUATION", "DEM_SURFACE_CONTINUATION");
        return;
    }
    failures.push(t_true(status.continuations.len() == 1,
        "the fixture must admit exactly one DEM continuation"));
    if status.continuations.len() == 1 {
        let collar = status.continuations[0];
        failures.push(t_true(collar.ready && !collar.hidden,
            "the composed continuation is not admitted"));
        failures.push(t_true(collar.source_valid && collar.source_reflected,
            "the DEM source shader is not reflected and valid"));
        failures.push(t_true(!boundary_matches(80.0),
            "the boundary validator must reject a displaced corner"));
        for width in collar.widths_m {
            failures.push(t_true(abs(width - collar.widths_m[0]) < 0.000001,
                "DEM exterior must use one shared transition distance"));
        }
        let spokes = collar.corner_spokes;
        let maximum_corner_grade = 0.0;
        for i in 4..spokes.len() {
            let a = spokes[i - 4].local;
            let b = spokes[i].local;
            let dx = b[0] - a[0];
            let dz = b[2] - a[2];
            let distance = sqrt(dx * dx + dz * dz);
            maximum_corner_grade = max(maximum_corner_grade, abs(b[1] - a[1]) / distance);
        }
        failures.push(t_true(maximum_corner_grade < 0.3,
            "the exterior corner forms a cliff: grade " + maximum_corner_grade));
        let count = 4 * (collar.source_resolution.to_int() - 1);
        let offset = 0;
        let checked = 0;
        let maximum_error = 0.0;
        let missing = false;
        while offset < count {
            let limit = min(128, count - offset);
            let page = query("TerrainLodStatus", #{ include_geometry: true,
                vertex_offset: offset, vertex_limit: limit, index_limit: 1 });
            let vertices = page.continuations[0].geometry.physical_positions;
            let points = [];
            for vertex in vertices {
                points.push([clamp(vertex[0], -collar.half_extent_m, collar.half_extent_m),
                    clamp(vertex[2], -collar.half_extent_m, collar.half_extent_m)]);
            }
            let heights = query("TerrainHeights", #{ points: points });
            for i in 0..vertices.len() {
                let sample = heights[i];
                if !sample.found { missing = true; }
                else { maximum_error = max(maximum_error, abs(sample.height - vertices[i][1])); }
                checked += 1;
            }
            offset += limit;
        }
        failures.push(t_true(!missing && boundary_matches(maximum_error),
            "native perimeter differs from mission DEM by " + maximum_error + " metres"));
        failures.push(t_true(checked == count, "native perimeter coverage is incomplete"));
        let dem_checked = 0;
        let dem_maximum_error = 0.0;
        let dem_morph_error = 0.0;
        for tile in status.tiles {
            if tile.source != collar.source || !tile.boundary_inspectable { continue; }
            let offset = 0;
            loop {
                let page = query("TerrainLodStatus", #{ include_geometry: true,
                    mesh_entity: tile.entity, boundary_only: true,
                    vertex_offset: offset, vertex_limit: 128, index_limit: 1 });
                let geometry = page.tiles[0].geometry;
                let vertices = geometry.physical_positions;
                let morphs = geometry.physical_morph_positions;
                let points = [];
                let chosen = [];
                for i in 0..vertices.len() {
                    let vertex = vertices[i];
                    let along = if abs(abs(vertex[0]) - collar.half_extent_m) < 0.001 { vertex[2] } else { vertex[0] };
                    let posting = (along + collar.half_extent_m) / (2.0 * collar.half_extent_m)
                        * (collar.source_resolution.to_int() - 1);
                    if abs(posting - round(posting)) < 0.0001 {
                        points.push([clamp(vertex[0], -collar.half_extent_m, collar.half_extent_m),
                            clamp(vertex[2], -collar.half_extent_m, collar.half_extent_m)]);
                        chosen.push(i);
                    }
                    dem_morph_error = max(dem_morph_error, abs(morphs[i][1] - vertex[1]));
                }
                if points.len() > 0 {
                    let heights = query("TerrainHeights", #{ points: points });
                    for i in 0..points.len() {
                        if !heights[i].found { missing = true; }
                        else { dem_maximum_error = max(dem_maximum_error,
                            abs(heights[i].height - vertices[chosen[i]][1])); }
                        dem_checked += 1;
                    }
                }
                offset += vertices.len();
                if offset >= geometry.vertex_count.to_int() { break; }
            }
        }
        print(#{ native_collar_vertices: checked, collar_error_m: maximum_error,
            native_dem_vertices: dem_checked, dem_error_m: dem_maximum_error, morph_error_m: dem_morph_error });
        failures.push(t_true(dem_checked >= count, "rendered DEM native boundary coverage is incomplete"));
        failures.push(t_true(!missing && boundary_matches(dem_maximum_error),
            "rendered DEM boundary differs from mission heights by " + dem_maximum_error + " metres"));
        failures.push(t_true(boundary_matches(dem_morph_error),
            "rendered DEM morph targets move the shared boundary"));

    }
    report_verdict(failures, "DEM SURFACE CONTINUATION", "DEM_SURFACE_CONTINUATION");
}
