// @timing presentation
// A distant perspective output must not retain the close-ground normal page.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn on_start(me, params) {
    let status = query("TerrainLodStatus", #{include_draw_cover: true});
    let view = ();
    for candidate in status.camera_cover {
        if candidate.camera == params.camera && candidate.source == params.source { view = candidate; }
    }
    if view == () || params.low_spacing_m <= 0.0 || params.low_contacts.len() == 0 {
        throw "range gate requires the actual ground cover and physical baseline";
    }
    let contacts = [];
    let physical_ready = true;
    for row in status.contacts {
        contacts.push([row.source, row.depth, row.x, row.z, row.resolution, row.min_wavelength_m]);
        physical_ready = physical_ready && row.ready;
    }
    let same_contacts = contacts.len() == params.low_contacts.len();
    for contact in contacts { same_contacts = same_contacts && params.low_contacts.contains(contact); }
    print("range clearance_m=" + view.clearance_m + " ground_spacing_m=" + params.low_spacing_m
        + " elevated_spacing_m=" + view.spacing_m);
    report_verdict([
        t_true(view.clearance_m >= params.min_clearance_m, "the output is genuinely distant"),
        t_true(view.ready && view.presented_ready && view.draw_cover.len() == view.drawn, "a complete settled draw cover remains available"),
        t_true(if params.expect_relief {
            view.relief != () && view.relief.current && view.relief.texel_m >= 2.0 * params.low_texel_m
        } else { view.relief == () }, "the relief level follows the distant view footprint"),
        t_true(view.spacing_m >= 2.0 * params.low_spacing_m && view.wanted <= status.config.tile_budget,
            "distance permits coarser geometric sampling within budget"),
        t_true(same_contacts && physical_ready, "visual distance and relief do not change physical contacts")
    ], "TERRAIN RELIEF RANGE", "TERRAIN_RELIEF_RANGE");
}
