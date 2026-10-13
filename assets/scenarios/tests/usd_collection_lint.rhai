// Native membership failures must reach the authored lint policy. The fixture
// also carries a valid explicit collection, which must remain free of this error.

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["GetDiagnostics"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.phase = "start";
    this.elapsed = 0.0;
}

fn on_tick(me, ctx) {
    if this.phase == "done" { return; }
    this.elapsed += dt();
    if this.phase == "start" {
        cmd("RunLint", #{});
        this.phase = "wait";
        return;
    }
    let report = query("GetDiagnostics", #{ scope: "loaded_stages" });
    if report == () || report.available != true || report.complete != true {
        if this.elapsed < 10.0 { return; }
        this.phase = "done";
        report_verdict(["loaded-stage lint did not publish a terminal report"],
            "USD COLLECTION LINT", "USD_COLLECTION_LINT");
        return;
    }
    let errors = [];
    let valid_reported = false;
    for diagnostic in report.diagnostics {
        if diagnostic.code != "invalid-collection" { continue; }
        errors.push(diagnostic);
        if diagnostic.subject == "/World/Valid" { valid_reported = true; }
    }
    this.phase = "done";
    report_verdict([
        t_true(errors.len() == 1, "expected exactly one native collection diagnostic"),
        t_true(errors.len() == 1 && errors[0].subject == "/World/Malformed"
            && errors[0].severity == "error", "the malformed collection must report an error"),
        t_true(!valid_reported, "the valid explicit collection must remain clean"),
    ], "USD COLLECTION LINT", "USD_COLLECTION_LINT");
}
