// Negative fixture: selected only if the Twin-root copy wrongly wins over the
// existing copy beside the authoring layer.
fn on_start(me, ctx) {
    report_verdict([t_true(false, "Twin-root copy overrode the layer-anchored search path")],
        "Twin search paths", "TWIN_SEARCH_PATHS");
}
