// Production acceptance gate for the AttachProgram command.
//
// This is intentionally authored Rhai: the test exercises the same command,
// USD document, live projection, port registration, and Modelica worker path
// that a user reaches from the Models palette. It is not a Rust copy of the
// command's lowering logic.

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListOpenDocuments", "QueryUsdPrim", "InspectUsdDocument", "CosimStatus"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.sent = false;
    this.done = false;
    this.source_only_sent = false;
    this.source_only_error = false;
    this.source_only_diagnostic = false;
    this.progress = #{};
    this.rejection_verified = false;
    this.doc = ();
    this.plant = -1;
    this.host = "/ProgramAttachTest/Ball";
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "PROGRAM ATTACH COMMAND", "PROGRAM_ATTACH_COMMAND");
    }
}

fn authored_prim_exists(doc, path) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path });
    result != () && result.ok != false && result.active == true
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 25.0 {
        fail(this, "AttachProgram did not reach its running/error boundary within 25 seconds: " + this.progress.to_string());
        return;
    }

    if !this.sent && this.elapsed >= 1.0 {
        let docs = query("ListOpenDocuments");
        if docs == () {
            fail(this, "ListOpenDocuments did not return its documented result");
            return;
        }
        for item in docs.open_documents {
            if item.kind == "usd" {
                this.doc = item.doc_id;
                break;
            }
        }
        if this.doc == () {
            if this.elapsed > 8.0 { fail(this, "no open USD document was discoverable"); }
            return;
        }

        if !this.rejection_verified {
            let before = query("InspectUsdDocument", #{doc_id: this.doc}).generation;
            let rejected = cmd("AttachProgram", #{doc_id: this.doc, spec: #{
                edit_target: "@runtime@", host_path: this.host, name: "RejectedProgram",
                source_asset: "lunco://models/Balloon.mo",
                inputs: [#{name: "height", type_name: "double", default_value: 1.0,
                    connection: this.host + ".outputs:position_y"}],
                outputs: [], realtime_safe: false,
            }});
            this.rejection_verified = rejected != () && rejected.ok == false
                && rejected.status == "rejected" && rejected.error.contains("exactly one")
                && query("InspectUsdDocument", #{doc_id: this.doc}).generation == before
                && !authored_prim_exists(this.doc, this.host + "/RejectedProgram");
            if !this.rejection_verified {
                fail(this, "invalid AttachProgram did not reject atomically with its terminal diagnostic");
                return;
            }
        }
        let plant_path = this.host + "/Plant";
        if authored_prim_exists(this.doc, plant_path) {
            this.sent = true;
        } else {
            let result = assembly_edit::attach_program(this.doc, #{
                edit_target: "@runtime@",
                host_path: this.host,
                name: "Plant",
                source_asset: "lunco://models/Balloon.mo",
                inputs: [
                    assembly_edit::program_input_connection("height", this.host + ".outputs:position_y"),
                    assembly_edit::program_input_connection("velocity", this.host + ".outputs:velocity_y"),
                    assembly_edit::program_input_default("rho0", 1.225),
                    assembly_edit::program_input_default("gravity", 1.62),
                ],
                outputs: [assembly_edit::program_output("netForce", [this.host + ".inputs:force_y"])],
                realtime_safe: true,
            });
            if !result.ok {
                fail(this, "AttachProgram was rejected: " + result.error);
                return;
            }
            this.sent = true;
            return;
        }
    }

    if this.sent {
        this.plant = find_path("/ProgramAttachTest/Ball/Plant");
    }

    if this.sent && this.plant >= 0 {
        // Projection can replace the committed access plan while preserving
        // the authored API id. Declare this dynamic read at its use boundary.
        track_entity_read(this.plant);
        let ports = query("ListPorts", #{ api_id: this.plant });
        let status = query("CosimStatus", #{include_values: true});
        // Entity registration and port projection are separate lifecycle edges.
        // A missing provider payload is a wait, not a pass and not an iterable
        // value; do not let a transient unit result turn the authored verdict
        // into evidence after an on_tick error.
        if ports == () || status == () { return; }
        if ports.ok == false || status.ok == false {
            fail(this, "program inspection rejected: " + #{ports: ports.error, status: status.error}.to_string());
            return;
        }
        let port_rows = ports.ports;
        let status_rows = status.entities;
        if port_rows == () || status_rows == () { return; }
        let has_height = false;
        let has_net_force = false;
        for p in port_rows {
            if p.name == "height" { has_height = true; }
            if p.name == "netForce" { has_net_force = true; }
        }
        let running = false;
        for row in status_rows {
            if row.name == "/ProgramAttachTest/Ball/Plant" && row.has_simcomponent
                && row.status == "Running" {
                running = true;
            }
        }
        if has_height && has_net_force && running && !this.source_only_sent {
            let source_only_path = this.host + "/SourceOnly";
            if authored_prim_exists(this.doc, source_only_path) {
                this.source_only_sent = true;
            } else {
                let source_only = assembly_edit::attach_program(this.doc, #{
                    edit_target: "@runtime@",
                    host_path: this.host,
                    name: "SourceOnly",
                    source_asset: "lunco://models/Balloon.mo",
                    inputs: [],
                    outputs: [],
                    realtime_safe: false,
                });
                if !source_only.ok {
                    fail(this, "source-only AttachProgram was rejected: " + source_only.error);
                    return;
                }
                this.source_only_sent = true;
                return;
            }
        }
        if this.source_only_sent {
            for row in status_rows {
                if row.name == "/ProgramAttachTest/Ball/SourceOnly"
                    && row.status.starts_with("Error:")
                {
                    this.source_only_error = true;
                    this.source_only_diagnostic = row.status.contains("/ProgramAttachTest/Ball/SourceOnly")
                        && row.status.contains("no declared inputs or outputs");
                }
            }
        }
        this.progress = #{height: has_height, net_force: has_net_force, running: running,
            source_only_sent: this.source_only_sent, source_only_error: this.source_only_error,
            source_only_diagnostic: this.source_only_diagnostic};
        if has_height && has_net_force && running && this.source_only_error
            && this.source_only_diagnostic {
            this.done = true;
            report_verdict([
                t_true(this.rejection_verified, "invalid attachment is rejected without changing the document"),
                t_true(this.plant >= 0, "attached program prim was not projected"),
                t_true(has_height, "Modelica input port `height` was not exposed"),
                t_true(has_net_force, "Modelica output port `netForce` was not exposed"),
                t_true(running, "attached program did not enter CosimStatus"),
                t_true(this.source_only_error, "source-only attachment was not reported as an error"),
                t_true(this.source_only_diagnostic, "the interface owner reports the source-only program and its missing scalar interface"),
            ], "PROGRAM ATTACH COMMAND", "PROGRAM_ATTACH_COMMAND");
            return;
        }
    }

}
