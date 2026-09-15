// The assertion is the owning render-boundary diagnostic. An authored shader contract
// must fail visibly; this scenario does not accept a renderer-selected material.
const TEST_KIND = "graphics";

fn query_ok(record) {
    record != () && record.ok != false
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.5 { return; }

    let diagnostics = query("RuntimeDiagnostics", #{});
    let rejected = false;
    if query_ok(diagnostics) {
        for finding in diagnostics.findings {
            if finding.producer == "shader-render"
                && finding.code == "render-shader-stage"
                && finding.severity == "error" {
                rejected = true;
            }
        }
    }
    this.done = true;
    report_verdict([
        t_true(rejected,
               "invalid authored shader was not rejected by the render shader owner"),
    ], "SHADER STAGE CONTRACT", "SHADER_STAGE_CONTRACT");
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["shader stage contract stopped before its rejection observation"],
            "SHADER STAGE CONTRACT", "SHADER_STAGE_CONTRACT");
    }
}
