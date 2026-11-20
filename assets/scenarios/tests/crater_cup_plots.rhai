// @timing presentation
// Run on the mounted Crater Cup Twin through RunScenarioAsset.
const OXYGEN = "/World/ProductTank1/Insulation";
const SOLAR = "/World/SolarField/Wing0_3";
const SAMPLE = "/World/Sample1";
const SCOUT = "/World/Scout/RoofPanel";

fn simulation_dependencies(me, ctx) {
    #{entity_reads: [find_path(OXYGEN), find_path(SOLAR), find_path(SAMPLE), find_path(SCOUT)],
      entity_writes: [], modelica_entities: [],
      query_reads: ["GetReadiness", "InspectSignalGrid", "InspectWorkbench"], required_inputs: []}
}

fn aim_at(path) {
    this.point = viewport_position(find_path(path));
    if this.point == () { throw "Cannot project " + path; }
    input_pointer_move(this.point[0], this.point[1]);
}

fn click(me) {
    input_click("primary", this.point[0], this.point[1]);
}

fn plots_rendered() {
    query("InspectWorkbench", #{}).rendered_panels.some(|panel| panel.id == "signal_grid")
}

fn shows_plots(title, count) {
    plots_rendered() && query("InspectSignalGrid", #{}).grids.some(|grid|
        grid.grid == 1 && grid.title == title && grid.plots.len() == count
        && grid.time_span_s != () && grid.time_span_s[1] > grid.time_span_s[0])
}

fn task(me, ctx) {
    seq([
        wait_until(|m| query("GetReadiness", #{}).ready
            && current_camera_name() == "/World/Avatar"
            && viewport_position(find_path(OXYGEN)) != ()
            && cmd_data("InspectGuidedHud", #{}).cards.len() > 0),
        wait(0.5),
        once(|m| {
            this.checks = [t_true(shows_plots("overview", 2), "live plant plots appear before any click")];
            this.aim_at(OXYGEN);
        }),
        wait(0.25), once(Fn("click")), wait(0.6),
        once(|m| {
            this.checks.push(t_true(shows_plots("Oxygen receiver", 3),
                "one receiver click renders three live gas traces"));
            this.aim_at(SOLAR);
        }),
        wait(0.25), once(Fn("click")), wait(0.6),
        once(|m| {
            this.checks.push(t_true(shows_plots("Photovoltaic field", 2),
                "clicking the array switches to its live power and temperature"));
            this.aim_at(SAMPLE);
        }),
        wait(0.25), once(Fn("click")), wait(0.6),
        once(|m| {
            let hint = cmd_data("InspectGuidedHud", #{}).cards[0].hint;
            this.checks.push(t_true(hint.contains("Push the sample") && !plots_rendered(),
                "an information-only object replaces the readout and hides the plots"));
            this.aim_at(SCOUT);
        }),
        wait(0.25), once(Fn("click")), wait(0.9),
        once(|m| {
            this.checks.push(t_true(current_camera_name().ends_with("/MastCam")
                && shows_plots("scout", 2), "clicking Scout opens its mounted camera and live plots"));
            report_verdict(this.checks, "CRATER CUP PLOTS", "CRATER_CUP_PLOTS");
        })
    ])
}
