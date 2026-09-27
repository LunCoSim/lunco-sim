// Complete runtime Modelica lint and diagnostics API acceptance.
//
// The engine exposes source identity through ListBundled, parse/lint facts
// through ValidateAsset, and generation-scoped editor findings through
// GetDiagnostics. This script owns the assertions and verdict at the
// production Rhai query boundary.

import "lunco://scripting/tools/modelica_editor" as modelica_editor;

const DIAGNOSTIC_MODEL = "model api_diagnostic_probe\n  Real missing_value;\n  Real determined_value;\nequation\n  determined_value = 0;\nend api_diagnostic_probe;\n";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["FindModel", "ListBundled", "ValidateAsset",
                     "GetDocumentSource", "GetDiagnostics"],
       required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
    this.phase = "assets";
    this.failures = [];
    this.diagnostics_doc = ();
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    if this.phase == "await_lint" {
        if this.elapsed > 15.0 {
            this.failures.push("GetDiagnostics did not finish linting the scratch model generation");
            this.done = true;
            report_verdict(this.failures, "MODELICA LINT", "MODELICA_LINT");
            return;
        }
        let source = query("GetDocumentSource", #{ doc_id: this.diagnostics_doc });
        if source == () || source.generation == () { return; }
        let diagnostics = query("GetDiagnostics", #{ doc_id: this.diagnostics_doc });
        if diagnostics == () || diagnostics.available != true || diagnostics.state == "pending" { return; }

        this.failures.push(t_true(
            diagnostics.doc_id == this.diagnostics_doc
                && diagnostics.generation == source.generation,
            "GetDiagnostics did not identify the current scratch document generation"));
        let lint_channel = ();
        for channel in diagnostics.channels {
            if channel.id == "modelica.rumoca-lint" { lint_channel = channel; }
        }
        this.failures.push(t_true(
            diagnostics.complete == true
                && lint_channel != ()
                && lint_channel.state == "ready"
                && lint_channel.generation == source.generation
                && to_int(lint_channel.revision) > 0,
            "GetDiagnostics did not publish a completed Rumoca lint snapshot"));
        let naming_finding = ();
        if diagnostics.diagnostics != () {
            for finding in diagnostics.diagnostics {
                if finding.source == "rumoca-linter" && finding.code == "naming-convention" {
                    naming_finding = finding;
                }
            }
        }
        this.failures.push(t_true(
            naming_finding != ()
                && naming_finding.severity == "warning"
                && to_int(naming_finding.line) > 0
                && to_int(naming_finding.column) > 0
                && naming_finding.suggestion != (),
            "GetDiagnostics omitted Rumoca's source-located naming finding or fix suggestion"));
        this.done = true;
        report_verdict(this.failures, "MODELICA LINT", "MODELICA_LINT");
        return;
    }

    if this.elapsed < 0.1 { return; }

    let inventory = query("ListBundled", #{});
    let failures = [];
    let search = query("FindModel", #{ query: "rocket", limit: 20 });
    let bundled_example_found = false;
    if search != () && search.items != () {
        for item in search.items {
            if item.uri == "bundled://AnnotatedRocketStage.mo" && item.source == "bundled" {
                bundled_example_found = true;
            }
        }
    }
    failures.push(t_true(bundled_example_found,
        "FindModel did not return the bundled rocket example without an open document"));
    if inventory == () || inventory.sources == () {
        failures.push(t_true(false,
            "ListBundled did not publish the complete Modelica source inventory"));
    } else {
        failures.push(t_true(inventory.sources.len() > inventory.bundled.len(),
            "Modelica source inventory is unexpectedly incomplete"));
        for source in inventory.sources {
            let report = query("ValidateAsset", #{ path: source.uri });
            if report == () {
                failures.push(t_true(false,
                    "ValidateAsset returned no report for " + source.path));
                continue;
            }
            failures.push(t_true(report.ok == true,
                source.path + " failed Modelica parse or lint validation"));
        }
    }
    print("[modelica-lint] checked " +
          if inventory == () || inventory.sources == () { "0" } else { inventory.sources.len() } +
          " runtime sources");
    this.failures = failures;
    let scratch = modelica_editor::new_scratch(DIAGNOSTIC_MODEL, "ModelicaDiagnosticsApiProbe");
    if scratch == () || scratch.ok != true {
        let detail = if scratch == () || scratch.error == () {
            "missing document diagnostic"
        } else {
            scratch.error
        };
        this.failures.push("Modelica Editor did not create the diagnostics API probe: " + detail);
        this.done = true;
        report_verdict(this.failures, "MODELICA LINT", "MODELICA_LINT");
        return;
    }
    this.diagnostics_doc = scratch.doc_id;
    this.phase = "await_lint";
    this.elapsed = 0.0;
}
