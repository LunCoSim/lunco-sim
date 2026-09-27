// Complete runtime Modelica lint and diagnostics API acceptance.
//
// The engine exposes source identity through ListBundled, parse/lint facts
// through ValidateAsset, and generation-scoped editor findings through
// GetModelDiagnostics. This script owns the assertions and verdict at the
// production API boundary.

import "lunco://scripting/tools/modelica_editor" as modelica_editor;

const DIAGNOSTIC_MODEL = "model api_diagnostic_probe\n  Real missing_value;\n  Real determined_value;\nequation\n  determined_value = 0;\nend api_diagnostic_probe;\n";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["FindModel", "ListBundled", "ValidateAsset",
                     "GetDocumentSource", "GetModelDiagnostics", "CompileStatus"],
       required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
    this.phase = "assets";
    this.failures = [];
    this.diagnostics_doc = ();
    this.lint_checked = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    if this.phase == "await_lint" {
        if this.elapsed > 15.0 {
            this.failures.push("GetModelDiagnostics did not finish linting the scratch model generation");
            this.done = true;
            report_verdict(this.failures, "MODELICA LINT", "MODELICA_LINT");
            return;
        }
        let source = query("GetDocumentSource", #{ doc_id: this.diagnostics_doc });
        if source == () || source.generation == () { return; }
        let diagnostics = query("GetModelDiagnostics", #{ doc_id: this.diagnostics_doc });
        if diagnostics == () || diagnostics.lint_state == "pending" { return; }

        if !this.lint_checked {
            this.failures.push(t_true(
                diagnostics.doc_id == this.diagnostics_doc
                    && diagnostics.generation == source.generation,
                "GetModelDiagnostics did not identify the current scratch document generation"));
            this.failures.push(t_true(
                diagnostics.lint_state == "ready" && diagnostics.lint_revision > 0,
                "GetModelDiagnostics did not publish a completed Rumoca lint snapshot"));
            let naming_finding = ();
            if diagnostics.diagnostics != () {
                for finding in diagnostics.diagnostics {
                    if finding.source == "rumoca-linter" && finding.rule == "naming-convention" {
                        naming_finding = finding;
                    }
                }
            }
            this.failures.push(t_true(
                naming_finding != ()
                    && naming_finding.severity == "warning"
                    && naming_finding.line > 0
                    && naming_finding.column > 0
                    && naming_finding.suggestion != (),
                "GetModelDiagnostics omitted Rumoca's source-located naming finding or fix suggestion"));
            this.lint_checked = true;
        }

        let compile_status = query("CompileStatus", #{ doc_id: this.diagnostics_doc });
        if compile_status == () || compile_status.ast_parsed != true
            || compile_status.state != "idle" {
            return;
        }
        let compile = cmd("CompileModel", #{
            doc_id: this.diagnostics_doc,
            class: "api_diagnostic_probe",
            force: true,
        });
        if compile == () || compile.ok == false {
            let detail = if compile == () || compile.error == () {
                "missing command diagnostic"
            } else {
                compile.error
            };
            this.failures.push("CompileModel rejected the underdetermined diagnostic probe: " + detail);
            this.done = true;
            report_verdict(this.failures, "MODELICA LINT", "MODELICA_LINT");
            return;
        }
        this.phase = "await_compile_failure";
        this.elapsed = 0.0;
        return;
    }

    if this.phase == "await_compile_failure" {
        if this.elapsed > 30.0 {
            this.failures.push("the underdetermined Modelica probe did not reach a compile error");
            this.done = true;
            report_verdict(this.failures, "MODELICA LINT", "MODELICA_LINT");
            return;
        }
        let status = query("CompileStatus", #{ doc_id: this.diagnostics_doc });
        if status == () || status.state == "idle" || status.state == "compiling" { return; }
        this.failures.push(t_true(
            status.state == "error",
            "the underdetermined Modelica probe did not fail compilation"));
        this.failures.push(t_true(
            status.error_message != ()
                && status.error_message.contains("Why simulation did not start")
                && status.error_message.contains("missing_value"),
            "CompileStatus omitted the unmatched Modelica unknown explanation"));
        let diagnostics = query("GetModelDiagnostics", #{ doc_id: this.diagnostics_doc });
        let explanation_found = false;
        if diagnostics != () && diagnostics.diagnostics != () {
            for finding in diagnostics.diagnostics {
                if finding.source == "rumoca-compiler"
                    && finding.message.contains("Why simulation did not start")
                    && finding.message.contains("missing_value") {
                    explanation_found = true;
                }
            }
        }
        this.failures.push(t_true(
            explanation_found,
            "GetModelDiagnostics omitted the structural compile explanation"));
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
