// Attach to an owned production host through RunScenarioAsset after readiness.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "GetDocumentSource", "GetDiagnostics"], required_inputs: []}
}
fn ack(result) {
    if result != () && result.status == "pending" && result.id != () {
        return command_result(result.id);
    }
    result
}
fn parser_report(doc, generation) {
    let diagnostics = query("GetDiagnostics", #{doc_id: doc});
    if diagnostics == () || diagnostics.channels == () { return (); }
    for channel in diagnostics.channels {
        if channel.id == "modelica.parser" && channel.state == "ready"
            && channel.generation == generation { return diagnostics; }
    }
    ()
}
fn parser_error_count(report) {
    let count = 0;
    for diagnostic in report.diagnostics {
        if diagnostic.source == "rumoca-parser" { count += 1; }
    }
    count
}
fn finish(state, failures) {
    if state.done { return state; }
    state.done = true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures, "MODELICA PARSE COMPLETION", "MODELICA_PARSE_COMPLETION");
    state
}
fn on_start(me, params) {
    this = #{done: false, ticks: 0, phase: "initial", doc: (), failures: [], edit: ()};
    let created = editor::new_scratch("model ParseInitial Real x; equation x=1; end ParseInitial;", "Parse completion probe");
    if created == () || created.ok != true {
        this = finish(this, ["scratch admission failed"]); return;
    }
    this.doc = created.doc_id;
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 4000 {
        this = finish(this, ["parse completion timed out at " + this.phase]); return;
    }
    if this.edit != () {
        let result = ack(this.edit);
        if result == () || result.status == "pending" { return; }
        if result.ok != true { this = finish(this, ["source edit rejected: " + result.to_string()]); return; }
        this.edit = ();
    }
    let source = query("GetDocumentSource", #{doc_id: this.doc});
    let status = query("CompileStatus", #{doc_id: this.doc});
    if source == () || source.generation == () || status == () || status.candidates == () { return; }
    let report = parser_report(this.doc, source.generation);
    if report == () { return; }
    if this.phase == "initial" {
        if status.ast_parsed != true || !status.candidates.contains("ParseInitial") { return; }
        this.failures.push(t_true(parser_error_count(report) == 0, "initial syntax has no parse diagnostics"));
        this.edit = editor::replace_source(this.doc, "model ParseBroken Real x = ; end ParseBroken;", source.generation);
        this.phase = "invalid"; return;
    }
    if this.phase == "invalid" {
        let located = false;
        for diagnostic in report.diagnostics {
            if diagnostic.source == "rumoca-parser" && diagnostic.code == "syntax"
                && diagnostic.line != () && diagnostic.column != ()
                && to_int(diagnostic.line) > 0 && to_int(diagnostic.column) > 0 { located = true; }
        }
        if !located { return; }
        this.failures.push(t_true(status.ast_parsed == false, "invalid syntax is visibly rejected"));
        this.failures.push(t_true(status.candidates.contains("ParseInitial")
            && !status.candidates.contains("ParseBroken"), "invalid edit retains the healthy index"));
        this.failures.push(t_true(report.generation == source.generation,
            "located diagnostics identify the invalid source generation"));
        this.edit = editor::replace_source(this.doc, "model ParseRepaired Real y; equation y=2; end ParseRepaired;", source.generation);
        this.phase = "repaired"; return;
    }
    if this.phase == "repaired" {
        if status.ast_parsed != true || !status.candidates.contains("ParseRepaired") { return; }
        this.failures.push(t_true(!status.candidates.contains("ParseInitial")
            && !status.candidates.contains("ParseBroken"), "repair replaces the class index"));
        this.failures.push(t_true(parser_error_count(report) == 0,
            "repair clears prior parse diagnostics"));
        this.failures.push(t_true(report.generation == source.generation,
            "repair diagnostics match the repaired source generation"));
        this = finish(this, this.failures);
    }
}
