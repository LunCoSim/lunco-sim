// @timing presentation
//! Catalog navigation must restart the guide and retain its exact Twin owner.
fn on_start(me, ctx) {
    this.phase = 0; this.elapsed = 0.0; this.at = 0.0; this.done = false;
    this.checks = []; this.cursor = -1;
    let application_path = tutorials::application_path("lunco://tutorials/sandbox/rover_lab.rhai");
    this.checks.push(t_true(application_path[0].title == "Lunar Rover Lab"
        && application_path[1].title == "Moon Driving Basics" && application_path.len() == 5,
        "installed application menu supplies the complete classroom continuation"));
    let rejected = false;
    try { tutorials::application_path("lunco://tutorials/missing.rhai"); }
    catch (error) { rejected = error.to_string().contains("expected one installed application lesson"); }
    this.checks.push(t_true(rejected, "an unknown installed lesson is rejected rather than guessed"));
    this.twin = query("ReadActiveTwinContract", #{}).twin_id;
    let contribution = invoke_hook("application.asset.lifecycle", ["json_scope_changed", #{
        scope: "twin", twin_id: this.twin.to_string(), twin_name: "Navigation test",
        asset_root_uri: "lunco://", payload: [#{asset_uri: "lunco://tutorials/catalog.json",
            persisted_result: false, text: ctx.catalog, error: ()}]
    }]);
    if !contribution.ok { throw contribution.error; }
    for track in contribution.value.menus[0].items[0].children {
        for lesson in track.children {
            if lesson.args.source_asset == "lunco://tutorials/perspectives/overview.rhai" {
                this.launch = lesson.args;
            }
        }
    }
    this.first = this.launch.source_asset;
    this.second = this.launch.params.lesson_path[1].source_asset;
    this.checks.push(t_true(this.launch.owner_twin_id == this.twin
        && this.launch.params.lesson_path.len() > 1, "the catalog supplies a scoped continuation"));
    tutorials::start(this.launch);
}

fn finish_test(checks) {
    report_verdict(checks, "Tutorial navigation", "TUTORIAL_NAVIGATION");
}

fn completion_card(cards, source) {
    for card in cards {
        for action in card.actions {
            if action.id == source + ":repeat" { return card; }
        }
    }
    ()
}

fn complete_tour(tour, cursor) {
    if tour != () && tour.index != cursor {
        cmd("GuidedNext", #{});
        return tour.index;
    }
    cursor
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += execution_context().delta_seconds;
    if this.elapsed - this.at > 10.0 {
        this.checks.push(t_true(false, "navigation stalled in phase " + this.phase));
        this.done = true; finish_test(this.checks); return;
    }
    let hud = cmd_data("InspectGuidedHud", #{});
    let tour = hud.tour;
    if this.phase == 0 {
        if tour == () { return; }
        this.checks.push(t_true(tour.twin == this.twin && tour.index == 0,
            "the selected guide starts under its exact Twin without a scene"));
        cmd("GuidedNext", #{}); this.phase = 1; this.at = this.elapsed; return;
    }
    if this.phase == 1 {
        if tour == () || tour.index != 1 { return; }
        cmd("GuidedBack", #{}); this.phase = 2; this.at = this.elapsed; return;
    }
    if this.phase == 2 {
        if tour == () || tour.index != 0 { return; }
        this.checks.push(t_true(tour.twin == this.twin, "Next and Back preserve the tour owner"));
        this.phase = 3; this.at = this.elapsed; return;
    }
    if this.phase == 3 || this.phase == 5 {
        this.cursor = complete_tour(tour, this.cursor);
        let card = completion_card(hud.cards, this.first);
        if card == () { return; }
        if this.phase == 3 {
            this.checks.push(t_true(tour == () && card.hint.contains("not assessed")
                && card.visible && card.interactive,
                "completion offers visible, usable navigation in the current perspective without an assessment claim"));
            tutorials::action(this.first + ":repeat");
            this.phase = 4;
        } else {
            tutorials::action(this.first + ":next");
            this.phase = 6;
        }
        this.at = this.elapsed; this.cursor = -1; return;
    }
    if this.phase == 4 {
        if tour == () { return; }
        this.checks.push(t_true(tour.index == 0 && tour.twin == this.twin
            && tour.title == "Choose your workbench", "Restart lesson reopens that guide without requiring a scene"));
        this.phase = 5; this.at = this.elapsed; return;
    }
    if this.phase == 6 {
        if tour == () || tour.title != "Simulation time" { return; }
        this.checks.push(t_true(tour.index == 0 && tour.twin == this.twin,
            "Next starts the catalog-selected lesson under the same exact Twin"));
        tutorials::action(this.first + ":next");
        this.phase = 61; this.at = this.elapsed; return;
    }
    if this.phase == 61 {
        if this.elapsed - this.at < 0.25 { return; }
        this.checks.push(t_true(tour != () && tour.index == 0 && tour.twin == this.twin,
            "a navigation action from the previous lesson cannot advance the current guide"));
        cmd("GuidedSkip", #{}); this.phase = 7; this.at = this.elapsed; return;
    }
    if this.phase == 7 {
        if tour != () { return; }
        this.checks.push(t_true(completion_card(hud.cards, this.second) == (),
            "skipping does not expose completion navigation"));
        cmd("GuidedNext", #{}); this.phase = 8; this.at = this.elapsed; return;
    }
    if this.elapsed - this.at < 0.25 { return; }
    this.checks.push(t_true(tour == (), "a skipped tour cannot be revived by Next"));
    this.done = true; finish_test(this.checks);
}
