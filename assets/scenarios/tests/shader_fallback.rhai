// The assertion is the owning render-boundary diagnostic. An authored shader contract
// must fail visibly; this scenario does not accept a renderer-selected material.
const TEST_KIND = "render-contract";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["RuntimeDiagnostics"], required_inputs: [] }
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me, ctx) {
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

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report_verdict(["shader stage contract stopped before its rejection observation"],
            "SHADER STAGE CONTRACT", "SHADER_STAGE_CONTRACT");
    }
}
