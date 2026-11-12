// @timing presentation
// The production driver loads identical composed fixtures differing only in focal length.
import "lunco://scripting/tests/lib/terrain_camera.rhai" as terrain_camera;
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn on_start(me, params) {
    let status = query("TerrainLodStatus", #{});
    let view = terrain_camera::main_cover(status, status.viewport_camera, ());
    if view == () || !(params.wide_spacing_m > 0.0) || params.wide_contacts.len == 0 {
        throw "zoom gate requires a real wide-angle baseline and its physical support";
    }
    let same_position = true;
    for axis in 0..3 {
        same_position = same_position
            && abs(view.position_local_m[axis] - params.wide_position_m[axis]) < 0.000001;
    }
    let contacts = [];
    let physical_ready = true;
    for row in status.contacts {
        contacts.push([row.depth, row.x, row.z, row.resolution, row.min_wavelength_m]);
        physical_ready = physical_ready && row.ready;
    }
    let same_contacts = contacts.len == params.wide_contacts.len;
    for contact in contacts {
        same_contacts = same_contacts && params.wide_contacts.contains(contact);
    }
    print("zoom wide_spacing_m=" + params.wide_spacing_m + " narrow_spacing_m=" + view.spacing_m
        + " wide_depth=" + params.wide_depth + " narrow_depth=" + view.depth);
    report_verdict([
        t_true(view.ready && status.stream.pending == 0, "the telephoto output has a settled real cover"),
        t_true(same_position, "zoom changes detail at an unchanged source-relative camera position"),
        t_true(view.clearance_m > 900.0, "zoom discovery works far beyond the near-field radius"),
        t_true(view.spacing_m < params.wide_spacing_m * 0.5, "narrower FOV requests finer visible postings"),
        t_true(view.depth > params.wide_depth, "narrower FOV refines the visible small-crater cover"),
        t_true(view.budget_refused == 0, "the clipped telephoto footprint fits its declared budget"),
        t_true(same_contacts && physical_ready, "zoom preserves the independent physical collider lattice"),
        t_true(!view.presentation_pending && status.stream.presentation_pending == 0,
            "the telephoto cover publishes without an unsafe geometry replacement")
    ], "CAMERA TERRAIN ZOOM", "CAMERA_TERRAIN_ZOOM");
}
