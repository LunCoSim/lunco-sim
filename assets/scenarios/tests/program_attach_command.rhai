// Production acceptance gate for the AttachProgram command.
//
// This is intentionally authored Rhai: the test exercises the same command,
// USD document, live projection, port registration, and Modelica worker path
// that a user reaches from the Models palette. It is not a Rust copy of the
// command's lowering logic.

fn on_start(me) {
    this.elapsed = 0.0;
    this.sent = false;
    this.done = false;
    this.source_only_sent = false;
    this.source_only_error = false;
    this.doc = -1;
    this.plant = -1;
    this.host = "/ProgramAttachTest/Ball";
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "PROGRAM ATTACH COMMAND", "PROGRAM_ATTACH_COMMAND");
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

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
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "no open USD document was discoverable"); }
            return;
        }

        let result = attach_program(
            this.doc,
            "@runtime@",
            this.host,
            "Plant",
            "lunco://models/Balloon.mo",
            [
                program_input_connection("height", this.host + ".outputs:position_y"),
                program_input_connection("velocity", this.host + ".outputs:velocity_y"),
                program_input_default("rho0", 1.225),
                program_input_default("gravity", 1.62),
            ],
            [program_output("netForce", [this.host + ".inputs:force_y"])],
            true,
        );
        if !result.ok {
            fail(this, "AttachProgram was rejected: " + result.error);
            return;
        }
        this.sent = true;
        return;
    }

    if this.sent && this.plant < 0 {
        this.plant = find("/ProgramAttachTest/Ball/Plant");
    }

    if this.sent && this.plant >= 0 {
        let ports = query("ListPorts", #{ entity: this.plant });
        let status = query("CosimStatus");
        let has_height = false;
        let has_net_force = false;
        for p in ports.ports {
            if p.name == "height" { has_height = true; }
            if p.name == "netForce" { has_net_force = true; }
        }
        let running = false;
        for row in status.entities {
            if row.name == "/ProgramAttachTest/Ball/Plant" && row.has_simcomponent {
                running = true;
            }
        }
        if has_height && has_net_force && running && !this.source_only_sent {
            let source_only = attach_program(
                this.doc,
                "@runtime@",
                this.host,
                "SourceOnly",
                "lunco://models/Balloon.mo",
                [],
                [],
                false,
            );
            if !source_only.ok {
                fail(this, "source-only AttachProgram was rejected: " + source_only.error);
                return;
            }
            this.source_only_sent = true;
            return;
        }
        if this.source_only_sent {
            for row in status.entities {
                if row.name == "/ProgramAttachTest/Ball/SourceOnly"
                    && row.status.starts_with("Error:")
                {
                    this.source_only_error = true;
                }
            }
        }
        if has_height && has_net_force && running && this.source_only_error {
            this.done = true;
            report_verdict([
                t_true(this.plant >= 0, "attached program prim was not projected"),
                t_true(has_height, "Modelica input port `height` was not exposed"),
                t_true(has_net_force, "Modelica output port `netForce` was not exposed"),
                t_true(running, "attached program did not enter CosimStatus"),
                t_true(this.source_only_error, "source-only attachment was not reported as an error"),
            ], "PROGRAM ATTACH COMMAND", "PROGRAM_ATTACH_COMMAND");
            return;
        }
    }

    if this.elapsed > 25.0 {
        fail(this, "AttachProgram did not produce a projected running program within 25 seconds");
    }
}
