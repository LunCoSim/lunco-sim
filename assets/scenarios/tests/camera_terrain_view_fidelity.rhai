// @timing presentation
// Snapshot gate: the driver establishes distinct main/feed views, moves the
// physical body, and waits for cover publication before attaching this scenario.
// Raw camera/source identities come from TerrainLodStatus and are equality-only;
// they are never used as command targets or converted through floating point.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn view_of(rows, camera, source) {
    for row in rows {
        if row.camera == camera && row.source == source { return row; }
    }
    ()
}

fn spacing_matches(row, minimum, maximum) {
    row != () && row.spacing_m != () && row.spacing_m >= minimum && row.spacing_m <= maximum
}

fn view_published(row) {
    row != () && row.ready && row.wanted > 0 && row.resident == row.wanted
        && row.drawn > 0 && row.depth != () && row.depth <= row.effective_max_depth
}

fn on_start(me, params) {
    if params.camera_raw == () || params.other_camera_raw == () || params.source_raw == ()
        || params.camera_raw == params.other_camera_raw || params.require_distinct_depths == ()
        || !(params.min_spacing_m > 0.0 && params.max_spacing_m >= params.min_spacing_m
        && params.other_min_spacing_m > 0.0
        && params.other_max_spacing_m >= params.other_min_spacing_m
        && params.maximum_budget_refused >= 0) {
        throw "view-fidelity gate requires two exact camera identities and metric bounds";
    }
    let status = query("TerrainLodStatus", #{});
    let main = view_of(status.camera_cover, params.camera_raw, params.source_raw);
    let other = view_of(status.camera_cover, params.other_camera_raw, params.source_raw);
    let physical_count = 0;
    let physical_ready = true;
    for contact in status.contacts {
        if contact.source == params.source_raw {
            physical_count += 1;
            physical_ready = physical_ready && contact.ready && contact.resolution > 1
                && contact.min_wavelength_m > 0.0;
        }
    }
    let checks = [
        t_true(main != (), "main output has its exact camera/source demand"),
        t_true(other != (), "secondary output has its exact camera/source demand"),
        t_true(view_published(main), "main view has an admitted drawn cover"),
        t_true(view_published(other), "secondary view has an admitted drawn cover"),
        t_true(spacing_matches(main, params.min_spacing_m, params.max_spacing_m),
            "main view meets its own metric spacing bounds"),
        t_true(spacing_matches(other, params.other_min_spacing_m, params.other_max_spacing_m),
            "secondary view meets its own metric spacing bounds"),
        t_true(main != () && main.budget_refused <= params.maximum_budget_refused,
            "main view stays within the declared refinement-refusal bound"),
        t_true(other != () && other.budget_refused <= params.maximum_budget_refused,
            "secondary view stays within the declared refinement-refusal bound"),
        t_true(main != () && other != () && (!params.require_distinct_depths
            || main.depth != other.depth), "separated views retain distinct requested detail"),
        t_true(physical_count > 0, "the moved physical assembly retains terrain support"),
        t_true(physical_count > 0 && physical_ready, "physical support remains ready independently"),
        t_true(main != () && other != () && main.effective_max_depth == other.effective_max_depth,
            "both views share the source's metric depth bound")
    ];
    if params.retired_camera_raw != () {
        checks.push(t_true(view_of(status.camera_cover, params.retired_camera_raw,
            params.source_raw) == (), "retired output contributes no terrain demand"));
    }
    if params.expected_main_wanted != () {
        checks.push(t_true(main != () && main.wanted == params.expected_main_wanted,
            "a remote body and secondary output preserve the stationary main cover"));
    }
    report_verdict(checks, "CAMERA TERRAIN VIEW FIDELITY", "CAMERA_TERRAIN_VIEW_FIDELITY");
}
