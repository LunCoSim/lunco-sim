// Source-backed Modelica editor facade.
//
// AST/diagram lowering, parse scheduling, journaling and undo are Rust-owned;
// this facade keeps document identity, generation cursors and checkpoint UX
// consistent with USD and SysML.

fn _error(stage, message) {
    #{ ok: false, domain: "modelica", stage: stage, retryable: false,
       error: message, errors: [message] }
}

fn _doc_required(doc, operation) {
    if doc == () || doc == 0 {
        return _error(operation, "an explicit Modelica document id is required");
    }
    #{ ok: true }
}

fn open(path) { cmd("OpenFile", #{ path: path }) }
fn new_document() { cmd("NewDocument", #{ kind: "modelica" }) }
fn new_scratch(source, name) {
    if source == () || type_of(source) != "string" || source == ""
        || name == () || type_of(name) != "string" || name == "" {
        return _error("new_scratch", "non-empty Modelica source and scratch name are required");
    }
    let created = cmd("CreateNewScratchModel", #{ source: source, name: name });
    if created == () || created.ok != true || created.data == ()
        || created.data.doc_id == () || created.data.doc_id <= 0 {
        return _error("new_scratch", "Modelica Editor did not return the created document identity");
    }
    #{ ok: true, doc_id: created.data.doc_id, command: created }
}

fn inspect(doc) {
    let valid = _doc_required(doc, "inspect");
    if !valid.ok { return valid; }
    let source = query("GetDocumentSource", #{ doc_id: doc });
    if source == () || source.ok != true { return source; }
    let compile = query("CompileStatus", #{ doc_id: doc });
    if compile == () || compile.ok != true {
        return _error("compile_status", "CompileStatus failed");
    }
    #{ ok: true, domain: "modelica", source: source, compile: compile,
       generation: source.generation, dirty: source.dirty }
}

/// Apply the existing prelude ApiOp constructors as one generation-checked
/// command. The Modelica owner reparses/defer-schedules structural operations
/// without blocking the Editor thread.
fn apply(doc, ops, parent_generation) {
    let valid = _doc_required(doc, "apply");
    if !valid.ok { return valid; }
    if ops == () || type_of(ops) != "array" || ops.is_empty() {
        return _error("apply", "a non-empty Modelica operation array is required");
    }
    modelica_apply_against(doc, ops, parent_generation)
}

/// Source replacement is explicit and remains undoable. Prefer structured
/// ApiOps for component/connection/diagram edits.
fn replace_source(doc, source, parent_generation) {
    apply(doc, [modelica_replace_source(source)], parent_generation)
}

fn verify(doc) {
    let state = inspect(doc);
    if state == () || state.ok != true { return state; }
    let compile = state.compile;
    let clean = compile.state == "ok" && compile.ast_parsed == true
        && compile.error_message == ();
    if !clean {
        return #{ ok: false, domain: "modelica", stage: "compile",
            retryable: compile.state == "compiling", inspection: state,
            error: "Modelica source is not compiled cleanly" };
    }
    #{ ok: true, domain: "modelica", stage: "verified", retryable: false,
       inspection: state, generation: state.generation }
}

fn save(doc) { cmd("SaveDocument", #{ doc_id: doc }) }
fn save_as(doc, path) { cmd("SaveAsDocument", #{ doc_id: doc, path: path }) }
fn undo(doc) { cmd("UndoDocument", #{ doc_id: doc }) }
fn redo(doc) { cmd("RedoDocument", #{ doc_id: doc }) }
fn close(doc) { cmd("CloseDocument", #{ doc_id: doc }) }

fn _query_ok(value) {
    value != () && value.ok != false
}

fn _source_generation_detail(source, expected) {
    if source == () {
        return "GetDocumentSource returned no document snapshot";
    }
    if source.generation == () {
        return "GetDocumentSource omitted the document generation";
    }
    "expected generation " + expected.to_string()
        + ", observed " + source.generation.to_string()
}

fn _experiment_error(stage, message, ticket, retryable) {
    #{ ok: false, pending: false, complete: false, domain: "modelica",
       stage: stage, retryable: retryable, error: message, ticket: ticket }
}

fn _pending_experiment(ticket, stage, detail) {
    ticket.poll_wait_ticks = ticket.poll_delay_ticks;
    if ticket.poll_delay_ticks < 16 {
        ticket.poll_delay_ticks *= 2;
        if ticket.poll_delay_ticks > 16 { ticket.poll_delay_ticks = 16; }
    }
    let result = #{ ok: true, pending: true, complete: false,
        stage: stage, ticket: ticket };
    if detail != () { result.detail = detail; }
    result
}

fn _accept_experiment_ack(ticket, acknowledgement) {
    if acknowledgement == () || acknowledgement.ok != true
        || acknowledgement.data == () || acknowledgement.data.experiment_id == ()
        || type_of(acknowledgement.data.experiment_id) != "string"
        || acknowledgement.data.experiment_id == "" {
        return _experiment_error("dispatch",
            "RunExperiment acknowledgement omitted its Modelica experiment identity",
            ticket, false);
    }
    ticket.experiment_id = acknowledgement.data.experiment_id;
    ticket.stage = "result";
    ticket.poll_delay_ticks = 1;
    ticket.poll_wait_ticks = 0;
    #{ ok: true, pending: true, complete: false,
       stage: "result", ticket: ticket,
       dispatch: acknowledgement, experiment_id: ticket.experiment_id }
}

/// Build a caller-owned polling ticket for one fixed-bounds batch solve.
/// Polling is explicit: each call observes one runtime snapshot and never
/// blocks the Editor/Rhai thread waiting for Rumoca's worker or frame drain.
fn experiment_ticket(doc, class, source_generation, label,
    output_names, t_start, t_end, dt, poll_limit) {
    if doc == () || doc == 0 || class == () || type_of(class) != "string"
        || class == "" || source_generation == () || label == ()
        || type_of(label) != "string" || label == "" {
        return _error("experiment_ticket", "explicit document, class, source generation, and run label are required");
    }
    if type_of(output_names) != "array" || output_names.is_empty() {
        return _error("experiment_ticket", "at least one Modelica output name is required");
    }
    if poll_limit == () || poll_limit <= 0 {
        return _error("experiment_ticket", "a positive Modelica status-poll limit is required");
    }
    if t_start == () || t_end == () || dt == () || !t_start.is_finite()
        || !t_end.is_finite() || !dt.is_finite() || t_end <= t_start || dt <= 0.0 {
        return _error("experiment_ticket", "finite, increasing solve bounds and a positive output interval are required");
    }
    let names = [];
    for name in output_names {
        if type_of(name) != "string" || name == "" || names.contains(name) {
            return _error("experiment_ticket", "output names must be non-empty and unique");
        }
        names.push(name);
    }
    #{ ok: true, pending: true, complete: false,
       ticket: #{
           stage: "prepare",
           doc_id: doc,
           class: class,
           source_generation: source_generation,
           label: label,
           output_names: names,
           t_start: t_start,
           t_end: t_end,
           dt: dt,
           poll_limit: poll_limit,
           poll_count: 0,
           poll_delay_ticks: 1,
           poll_wait_ticks: 0,
           command_id: (),
           experiment_id: (),
       }}
}

fn _poll_experiment_dispatch(ticket) {
    let command = command_result(ticket.command_id);
    if command == () || command.status == "pending" {
        return _pending_experiment(ticket, "dispatch", command);
    }
    if command.ok != true {
        let message = if command.error == () {
            "RunExperiment command did not complete: " + command.status
        } else {
            command.error
        };
        return _experiment_error("dispatch", message, ticket, false);
    }
    _accept_experiment_ack(ticket, command)
}

/// Advance an experiment ticket once. The caller stores the returned ticket
/// and calls again on a later simulation/editor tick while `pending` is true.
/// Completed series remain the native f64 arrays returned by GetExperimentResult.
fn poll_experiment_ticket(ticket) {
    if type_of(ticket) != "map" || ticket.stage == () || ticket.doc_id == ()
        || ticket.source_generation == () || ticket.class == ()
        || ticket.label == () || ticket.output_names == ()
        || ticket.poll_limit == () || ticket.poll_count == () {
        return _experiment_error("ticket", "a complete Modelica experiment ticket is required", ticket, false);
    }
    if ticket.stage == "complete" {
        return _experiment_error("ticket", "completed experiment tickets cannot be polled twice", ticket, false);
    }
    if ticket.stage == "timed_out" {
        return _experiment_error("timeout", "Modelica experiment ticket exceeded its bounded poll budget", ticket, true);
    }
    if ticket.stage != "prepare" && ticket.stage != "dispatch" && ticket.stage != "result" {
        return _experiment_error("ticket", "unknown Modelica experiment ticket stage", ticket, false);
    }

    if ticket.poll_wait_ticks > 0 {
        ticket.poll_wait_ticks -= 1;
        return #{ ok: true, pending: true, complete: false,
            stage: ticket.stage, ticket: ticket };
    }
    ticket.poll_count += 1;
    if ticket.poll_count > ticket.poll_limit {
        ticket.stage = "timed_out";
        return _experiment_error("timeout", "Modelica experiment ticket exceeded its bounded poll budget", ticket, true);
    }

    if ticket.stage == "prepare" {
        let source = query("GetDocumentSource", #{ doc_id: ticket.doc_id });
        if !_query_ok(source) || source.generation == () {
            return _experiment_error("prepare", "GetDocumentSource could not inspect the explicit document", ticket, false);
        }
        if source.generation != ticket.source_generation {
            return _experiment_error("prepare", "Modelica source generation changed before dispatch", ticket, false);
        }
        let compile = query("CompileStatus", #{ doc_id: ticket.doc_id });
        if !_query_ok(compile) {
            return _experiment_error("prepare", "CompileStatus could not inspect the generated class", ticket, false);
        }
        if compile.state == "error" {
            let message = if compile.error_message == () {
                "generated Modelica source has parse errors"
            } else {
                compile.error_message
            };
            return _experiment_error("prepare",
                message, ticket, false);
        }
        let class_found = false;
        for candidate in compile.candidates {
            if candidate == ticket.class || candidate.ends_with("." + ticket.class) {
                class_found = true;
            }
        }
        if compile.ast_parsed != true || !class_found {
            return _pending_experiment(ticket, "prepare", #{
                compile_state: compile.state,
                ast_parsed: compile.ast_parsed,
                candidates: compile.candidates,
            });
        }

        let dispatch = cmd("RunExperiment", #{
            doc_id: ticket.doc_id,
            class: ticket.class,
            overrides: [],
            inputs: [],
            t_start: ticket.t_start,
            t_end: ticket.t_end,
            dt: ticket.dt,
            label: ticket.label,
        });
        if dispatch == () || dispatch.ok != true {
            let message = if dispatch == () || dispatch.error == () {
                "RunExperiment was rejected"
            } else {
                dispatch.error
            };
            return _experiment_error("dispatch", message, ticket, false);
        }
        if dispatch.status == "pending" && dispatch.id != () {
            ticket.command_id = dispatch.id;
            ticket.stage = "dispatch";
            ticket.poll_delay_ticks = 1;
            return _poll_experiment_dispatch(ticket);
        }
        return _accept_experiment_ack(ticket, dispatch);
    }

    if ticket.stage == "dispatch" {
        return _poll_experiment_dispatch(ticket);
    }

    if ticket.stage == "result" {
        let source = query("GetDocumentSource", #{ doc_id: ticket.doc_id });
        if !_query_ok(source) || source.generation != ticket.source_generation {
            return _experiment_error("result",
                "Modelica source generation changed while its experiment was running ("
                    + _source_generation_detail(source, ticket.source_generation) + ")",
                ticket, false);
        }
        let status = query("RunStatus", #{ experiment_id: ticket.experiment_id });
        if !_query_ok(status) {
            return _experiment_error("result", "RunStatus could not inspect the dispatched experiment", ticket, false);
        }
        if status.state == "failed" || status.state == "cancelled" {
            let message = if status.error == () {
                "Modelica experiment did not complete successfully"
            } else {
                status.error
            };
            return _experiment_error("result",
                message, ticket, false);
        }
        if status.state != "done" {
            return _pending_experiment(ticket, "result", status);
        }
        let result = query("GetExperimentResult", #{
            experiment_id: ticket.experiment_id,
            variables: ticket.output_names,
        });
        if !_query_ok(result) || result.state != "done" {
            return _experiment_error("result", "GetExperimentResult did not return the completed numeric trajectory", ticket, false);
        }
        if result.missing_variables != () && !result.missing_variables.is_empty() {
            return _experiment_error("result", "Modelica result omitted requested output series", ticket, false);
        }
        if result.times == () || result.times.is_empty() {
            return _experiment_error("result", "Modelica experiment returned no time samples", ticket, false);
        }
        for sample_time in result.times {
            if type_of(sample_time) != "f64" || !sample_time.is_finite() {
                return _experiment_error("result", "Modelica result contains a non-finite time sample", ticket, false);
            }
        }
        for name in ticket.output_names {
            let samples = result.series[name];
            if type_of(samples) != "array" || samples.is_empty()
                || samples.len() != result.times.len() {
                return _experiment_error("result", "Modelica result has incomplete numeric samples for " + name, ticket, false);
            }
            for sample in samples {
                if type_of(sample) != "f64" || !sample.is_finite() {
                    return _experiment_error("result", "Modelica output series is not finite f64 data: " + name, ticket, false);
                }
            }
        }
        ticket.stage = "complete";
        return #{ ok: true, pending: false, complete: true,
            stage: "complete", ticket: ticket, run: status, result: result };
    }

    _experiment_error("ticket", "unknown Modelica experiment ticket stage", ticket, false)
}

/// Signal cancellation for a dispatched solve. The returned ticket remains
/// pollable until RunStatus reports its terminal cancelled state.
fn cancel_experiment_ticket(ticket) {
    if type_of(ticket) != "map" || ticket.experiment_id == ()
        || ticket.experiment_id == "" {
        return _experiment_error("cancel", "a dispatched Modelica experiment identity is required", ticket, false);
    }
    let command = cmd("CancelExperiment", #{
        experiment_id: ticket.experiment_id,
        all: false,
    });
    if command == () || command.ok != true {
        return _experiment_error("cancel", "Modelica experiment cancellation was not accepted", ticket, true);
    }
    #{ ok: true, pending: true, complete: false,
       stage: ticket.stage, ticket: ticket, command: command }
}
