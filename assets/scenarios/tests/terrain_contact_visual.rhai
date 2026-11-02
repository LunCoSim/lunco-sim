// Run on a settled rendered DEM with a physical body. Probe the actual CPU
// mesh postings and the material's deformation contract against native rays.
const TEST_KIND = "graphics";
fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [find_path(params.body_path)], entity_writes: [],
      query_reads: ["TerrainLodStatus", "QueryEntity", "Raycast", "InspectUsdCurveView", "InspectVehicleTrail"], required_inputs: []}
}
fn matches_surface(error_m) { error_m < 0.001 }
fn on_start(me, params) {
    this.body = find_path(params.body_path);
    let status = query("TerrainLodStatus", #{});
    this.tiles = status.contacts;
    this.camera_detail = status.camera_cover.len() > 0;
    for cover in status.camera_cover {
        if abs(cover.clearance_m) < 10.0 {
            this.camera_detail = this.camera_detail && cover.ready && cover.pinned
                && cover.depth.to_int() >= status.config.max_depth.to_int();
        }
    }
    this.index = 0;
    this.samples = 0;
    this.finer_samples = 0;
    this.worst = 0.0;
    this.displaced_rejected = true;
    this.all_ready = true;
    this.all_pinned = true;
    this.all_meshes = true;
    this.done = false;
    this.doc = params.doc_id;
    this.curve = params.curve_id;
    this.require_trails = params.require_trails;
    this.require_finer = params.require_finer;
}
fn on_tick(me, params) {
    if this.done { return; }
    if this.index < this.tiles.len() {
        let tile = this.tiles[this.index];
        this.index += 1;
        this.all_ready = this.all_ready && tile.ready && tile.drawn;
        this.all_pinned = this.all_pinned && tile.pinned;
        if tile.entity == () { this.all_meshes = false; return; }
        let res = tile.resolution.to_int();
        let status = query("TerrainLodStatus", #{include_geometry: true,
            mesh_entity: tile.entity, vertex_offset: (res / 2) * res + res / 2,
            vertex_limit: res + 2, index_limit: 1});
        if status.tiles.len() != 1 || status.tiles[0].geometry == () {
            this.all_meshes = false; return;
        }
        let geometry = status.tiles[0].geometry;
        this.all_meshes = this.all_meshes && geometry.vertex_count.to_int() == res * res;
        let p = geometry.physical_positions;
        // Interior triangle: no edge stitching; pinning must disable distance morph.
        let a = p[0]; let b = p[res]; let c = p[1];
        let x = (a[0] + b[0] + c[0]) / 3.0;
        let y = (a[1] + b[1] + c[1]) / 3.0;
        let z = (a[2] + b[2] + c[2]) / 3.0;
        let body = query("QueryEntity", #{id: this.body}).position;
        // Keep the native terrain ray clear of the articulated body.
        if (x-body[0])*(x-body[0]) + (z-body[2])*(z-body[2]) > 9.0 {
            let hit = query("Raycast", #{origin:[x,y+0.1,z], dir:[0.0,-1.0,0.0],
                max:0.2, solid:false, exclude_sensors:true});
            if hit.hit {
                this.samples += 1;
                if tile.depth.to_int() > tile.canonical_depth.to_int() { this.finer_samples += 1; }
                this.worst = max(this.worst, abs(hit.point[1]-y));
                this.displaced_rejected = this.displaced_rejected
                    && !matches_surface(abs(hit.point[1] - (y + 0.02)));
            } else { this.all_meshes = false; }
        }
        return;
    }
    this.done = true;
    print("CONTACT VISUAL: tiles=" + this.tiles.len() + ", rays=" + this.samples
        + ", finer rays=" + this.finer_samples + ", maximum triangle/contact error (m)=" + this.worst);
    let route = query("InspectUsdCurveView", #{doc_id: this.doc, entity_id: this.curve});
    print("CONTACT ROUTE: " + route.to_string());
    if route.projection == () {
        report_verdict(["route inspection failed: " + route.to_string()],
            "TERRAIN CONTACT VISUAL", "TERRAIN_CONTACT_VISUAL");
        return;
    }
    let trail = query("InspectVehicleTrail", #{id: this.body});
    let tracks = 0;
    let tracks_valid = true;
    for lane in trail.lanes {
        if lane.sample_count.to_int() > 1 {
            tracks += 1;
            tracks_valid = tracks_valid && lane.projection == "terrain"
                && lane.published_revision != () && lane.error == ()
                && abs(lane.render_width_m-lane.wheel_width_m) < 0.000001;
        }
    }
    report_verdict([
        t_true(this.camera_detail, "nearby active cameras retain their finest geometry"),
        t_true(!this.require_finer || this.finer_samples > 0, "a finer camera mesh subdivides actual physical contact triangles"),
        t_true(this.tiles.len() > 0, "physics publishes an explicit support cover"),
        t_true(this.all_ready, "the complete physical support cover is visibly resident"),
        t_true(this.all_pinned, "contact tiles cannot morph onto a coarser surface"),
        t_true(this.all_meshes, "rendered contact meshes use the physical lattice"),
        t_true(this.samples > 0 && matches_surface(this.worst),
            "rendered triangles match native contacts within 1 mm"),
        t_true(this.samples > 0 && this.displaced_rejected, "2 cm displaced triangles fail the same native-ray gate"),
        t_true(route.projection == "terrain_surface" && route.error == "",
            "the route ribbon is a ground-material annotation"),
        t_true(route.displayed_surface_binding_count != () && route.displayed_surface_binding_count.to_int() >= this.tiles.len(),
            "new contact tiles retain the shared route and wheel-mark texture"),
        t_true(!this.require_trails || (tracks > 0 && tracks_valid),
            "physical wheel marks remain published at tire width after driving")
    ], "TERRAIN CONTACT VISUAL", "TERRAIN_CONTACT_VISUAL");
}
