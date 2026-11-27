// @timing presentation
// Observe the published near-field cover throughout two fixed-eye yaw sweeps.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [params.camera_id],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn output_cover(status, params) {
    if status.viewport_camera != params.viewport_camera { return (); }
    let selected = ();
    for view in status.camera_cover {
        if view.camera == params.output_camera && view.source == params.source {
            if selected != () { throw "rotation output cover is ambiguous"; }
            selected = view;
        }
    }
    selected
}

fn near_cover(view, eye, radius) {
    let nodes = #{};
    let spacing = 0.0;
    let current = true;
    if view == () || view.draw_cover == () { return #{nodes: nodes, spacing: spacing, current: false}; }
    for tile in view.draw_cover {
        let dx = max(0.0, abs(tile.center_m[0] - eye[0]) - tile.half_extent_m);
        let dz = max(0.0, abs(tile.center_m[1] - eye[2]) - tile.half_extent_m);
        if dx * dx + dz * dz <= radius * radius {
            let key = tile.depth.to_string() + ":" + tile.x.to_string() + ":" + tile.z.to_string();
            nodes[key] = [tile.entity, tile.spacing_m, tile.morph_band_m, tile.stitch_edges];
            // A fully morphed posting can be twice as coarse as its own lattice.
            let stitched = false;
            for edge in tile.stitch_edges { stitched = stitched || edge > 0.0; }
            let worst_spacing = if tile.morph_band_m[0] >= 1.0e19 && !stitched { tile.spacing_m } else { tile.spacing_m * 2.0 };
            spacing = max(spacing, worst_spacing);
            current = current && tile.current_lattice;
        }
    }
    #{nodes: nodes, spacing: spacing, current: current && nodes.len() > 0}
}

// Every tile intersecting a ring contributes its actual posting spacing.
// A single level of difference is permitted for the 2:1 neighbour closure,
// but a distant island must not be multiple levels finer than inner terrain.
fn radial_quality(view, eye, radii) {
    let previous_max = 0.0;
    let bands = [];
    let good = true;
    for radius in radii {
        let min_spacing = 1.0e30;
        let max_spacing = 0.0;
        let radius2 = radius * radius;
        for tile in view.draw_cover {
            let dx = abs(tile.center_m[0] - eye[0]);
            let dz = abs(tile.center_m[1] - eye[2]);
            let near_x = max(0.0, dx - tile.half_extent_m);
            let near_z = max(0.0, dz - tile.half_extent_m);
            let far_x = dx + tile.half_extent_m;
            let far_z = dz + tile.half_extent_m;
            if near_x * near_x + near_z * near_z <= radius2
                && far_x * far_x + far_z * far_z >= radius2 {
                min_spacing = min(min_spacing, tile.spacing_m);
                max_spacing = max(max_spacing, tile.spacing_m);
            }
        }
        good = good && max_spacing > 0.0 && min_spacing >= previous_max * 0.5;
        previous_max = max(previous_max, max_spacing);
        bands.push([radius, min_spacing, max_spacing]);
    }
    #{ok: good, bands: bands}
}

fn issue_heading(me) {
    if this.done { return; }
    let params = this.parameters;
    let result = cmd("SetCameraLookAt", #{camera: params.camera_id, eye: params.eye,
        target: params.targets[this.heading]});
    this.commands_ok = this.commands_ok && result.ok;
    this.started = clock_snapshot().wall_elapsed_s;
    this.heading_active = true;
    this.heading_ready = false;
    if !result.ok { this.finish(me); }
}

fn heading_finished(me) { this.done || this.heading_ready }

fn record_heading(me) {
    if this.done { return; }
    this.forward = this.latest_forward;
    let view = output_cover(query("TerrainLodStatus", #{include_draw_cover: true}), this.parameters);
    let radial = radial_quality(view, this.eye, this.parameters.radial_radii);
    this.radial_ok = this.radial_ok && radial.ok;
    print("rotation radial=" + radial.bands.to_string());
    print("rotation heading=" + this.heading + " near_tiles=" + this.latest_near.nodes.len()
        + " max_spacing=" + this.latest_near.spacing);
    this.heading += 1;
    this.heading_active = false;
}

fn on_start(me, params) {
    if params.targets.len() < 16 || params.radius_m <= 0.0 || params.max_spacing_m <= 0.0 {
        throw "rotation gate requires two complete sweeps and explicit near-field quality bounds";
    }
    let status = query("TerrainLodStatus", #{include_draw_cover: true});
    let view = output_cover(status, params);
    if view == () || !view.ready || view.draw_cover.len() == 0 { throw "rotation gate requires a settled drawn cover"; }
    this.parameters = params;
    this.eye = view.position_local_m;
    this.forward = view.forward_local;
    this.baseline = near_cover(view, this.eye, params.radius_m);
    this.relief = view.relief;
    this.relief_ok = !params.require_relief || (view.relief != () && view.relief.current
        && view.relief.texel_m > 0.0 && view.relief.texel_m <= params.max_relief_texel_m);
    let radial = radial_quality(view, this.eye, params.radial_radii);
    this.radial_ok = radial.ok;
    print("rotation radial baseline=" + radial.bands.to_string());
    this.heading = 0;
    this.samples = 0;
    this.commands_ok = true;
    this.pose_ok = true;
    this.current_ok = this.baseline.current;
    this.identity_ok = true;
    this.spacing_ok = this.baseline.spacing <= params.max_spacing_m;
    this.headings_ok = true;
    this.budget_ok = view.wanted <= status.config.tile_budget;
    this.done = false;
    this.heading_active = false;
    this.heading_ready = false;
}

fn on_tick(me, params) {
    if this.done == () || this.done { return; }
    this.samples += 1;
    let status = query("TerrainLodStatus", #{include_draw_cover: true});
    let view = output_cover(status, params);
    if view == () {
        this.pose_ok = false;
        this.current_ok = false;
        this.finish(me);
        return;
    } else {
        for axis in 0..3 {
            this.pose_ok = this.pose_ok && abs(this.eye[axis] - view.position_local_m[axis]) <= params.position_tolerance_m;
        }
        let near = near_cover(view, this.eye, params.radius_m);
        this.current_ok = this.current_ok && near.current && view.presented_ready && view.draw_cover.len() == view.drawn;
        this.identity_ok = this.identity_ok && near.nodes == this.baseline.nodes;
        this.relief_ok = this.relief_ok && (!params.require_relief || view.relief == this.relief);
        this.spacing_ok = this.spacing_ok && near.spacing <= params.max_spacing_m;
        this.budget_ok = this.budget_ok && view.wanted <= status.config.tile_budget;
        if this.heading_active {
            let elapsed = clock_snapshot().wall_elapsed_s - this.started;
            let delta = 0.0;
            for axis in 0..3 {
                let difference = this.forward[axis] - view.forward_local[axis];
                delta += difference * difference;
            }
            this.latest_forward = view.forward_local;
            this.latest_near = near;
            this.heading_ready = elapsed >= params.minimum_heading_s && view.ready
                && view.selection_current && delta > 0.05;
            if elapsed > params.heading_timeout_s && !this.heading_ready {
                this.headings_ok = false;
                this.finish(me);
            }
        }
    }
}

fn finish(me) {
    if this.done { return; }
    this.done = true;
    let params = this.parameters;
    report_verdict([
        t_true(this.commands_ok && this.headings_ok && this.heading == params.targets.len(), "all requested headings actually publish"),
        t_true(this.samples >= params.targets.len() && this.pose_ok, "camera rotates at a fixed source-relative position"),
        t_true(this.current_ok, "near-field drawn cover is nonempty and current throughout rotation"),
        t_true(this.identity_ok, "near-field mesh identities, stitches and morph bands do not churn"),
        t_true(this.spacing_ok, "drawn near-field postings satisfy the declared quality bound"),
        t_true(this.radial_ok, "outer detail does not exceed inner posting quality by multiple LOD levels"),
        t_true(this.budget_ok, "the selected cover respects the configured tile budget"),
        t_true(this.relief_ok, "the admitted fine normal page stays current and identical throughout yaw")
    ], "TERRAIN ROTATION COVER", "TERRAIN_ROTATION_COVER");
}

fn task(me, params) {
    let steps = [];
    for target in params.targets {
        steps.push(once(Fn("issue_heading")));
        steps.push(wait_until(Fn("heading_finished")));
        steps.push(once(Fn("record_heading")));
    }
    steps.push(once(Fn("finish")));
    seq(steps)
}
