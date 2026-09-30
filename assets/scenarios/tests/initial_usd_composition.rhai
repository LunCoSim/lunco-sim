// Initial-stage acceptance for an incomplete USD composition closure, both
// with the shipped application policy and with a Twin policy override.

fn simulation_dependencies(me, ctx) {
    #{
        modelica_entities: [],
        entity_reads: [],
        entity_writes: [],
        query_reads: ["RuntimeDiagnostics", "QueryUsdPrim", "SceneTestParameters"],
        required_inputs: [],
    }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    let parameters = query("SceneTestParameters");
    let report = query("RuntimeDiagnostics", #{});
    if parameters == () || parameters.scene == ()
        || report == () || report.findings == () {
        if this.elapsed < 10.0 { return; }
        this.done = true;
        let detail = if report != () && report.ok == false {
            report.error
        } else {
            ""
        };
        report_verdict([
            t_true(false,
                "initial-stage runtime diagnostics did not become available; " + detail),
        ], "INITIAL USD COMPOSITION", "INITIAL_USD_COMPOSITION");
        return;
    }

    let missing_dependency = false;
    for diagnostic in report.findings {
        if diagnostic.code == "USD_COMPOSITION_MISSING_DEPENDENCY"
            && diagnostic.subject.index_of("missing.usda") >= 0 {
            missing_dependency = true;
        }
    }

    let root = find_path("/World");
    let partial = find_path("/World/Partial");
    let unresolved_child = find_path("/World/Partial/Missing");
    let inspected = if partial >= 0 {
        query("QueryUsdPrim", #{ path: "/World/Partial" })
    } else {
        ()
    };

    this.done = true;
    let expected_scene = parameters.scene.index_of("initial_allow.usda") >= 0
        || parameters.scene.index_of(
            "initial_usd_composition_default/initial_default.usda") >= 0;
    report_verdict([
        t_true(expected_scene,
            "an initial-stage composition-policy fixture was loaded"),
        t_true(root >= 0 && partial >= 0
                && inspected != () && inspected.ok != false,
            "the available root and referenced prim were projected"),
        t_true(unresolved_child < 0,
            "the unresolved nested arc did not fabricate child prims"),
        t_true(missing_dependency,
            "the loaded-stage diagnostics retain the exact unresolved composition dependency"),
    ], "INITIAL USD COMPOSITION", "INITIAL_USD_COMPOSITION");
}
