// Production Rhai acceptance for duplicate runtime port ownership in the loaded-stage lint.

fn by_code(findings, code) {
    let out = [];
    for finding in findings {
        if finding.code == code { out.push(finding); }
    }
    out
}

fn simulation_dependencies(me, ctx) {
    let collision = find_path("/PortOwnerCollision/CollisionCase");
    #{ modelica_entities: [],
       entity_reads: [collision],
       entity_writes: [],
       query_reads: ["GetDiagnostics", "QueryPhysicsState", "ReadPorts"],
       required_inputs: [] }
}

fn input_port_value(target, name) {
    let ports = query("ReadPorts", #{ api_id: target });
    if ports == () || ports.ports == () { return (); }
    for port in ports.ports {
        if port.name == name && port.direction == "in" { return port.value; }
    }
    ()
}

fn finish_test(me, failures) {
    if me.done { return; }
    me.done = true;
    report_verdict(failures, "PORT OWNER COLLISION", "PORT_OWNER_COLLISION");
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
    this.phase = "owners";
    this.collision = find_path("/PortOwnerCollision/CollisionCase");
    this.lint_revision_before = ();
    this.waiting_for = "Modelica and Avian port owners";
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 15.0 {
        finish_test(this, ["timed out waiting for " + this.waiting_for]);
        return;
    }

    if this.phase == "owners" {
        if this.collision < 0 { return; }
        let state = query("QueryPhysicsState", #{ id: this.collision });
        let mass = input_port_value(this.collision, "mass");
        if state == () || state.ok == false
            || state.authored_mass_kg != 10.0 || mass != 10.0 {
            return;
        }

        let previous = query("GetDiagnostics", #{ scope: "loaded_stages" });
        this.lint_revision_before = if previous == () || previous.revision == () {
            ()
        } else {
            previous.revision
        };
        let requested = cmd("RunLint", #{ scope: "loaded_stages" });
        if requested == () || requested.ok != true {
            finish_test(this, ["RunLint rejected the loaded-stage collision check"]);
            return;
        }
        this.phase = "lint";
        this.waiting_for = "the completed loaded-stage lint revision requested by this test";
        return;
    }

    if this.phase == "lint" {
        let report = query("GetDiagnostics", #{ scope: "loaded_stages" });
        if report == () || report.available != true || report.complete != true {
            return;
        }
        if this.lint_revision_before != () && report.revision == this.lint_revision_before {
            return;
        }

        let collisions = by_code(report.diagnostics, "port-owner-collision");
        let failures = [];
        failures.push(t_true(collisions.len() == 1,
            "expected one port-owner-collision finding, got " + collisions.len()));
        failures.push(t_true(report.ok == false && to_int(report.errors) > 0,
            "duplicate runtime port ownership did not fail the loaded-stage lint report"));
        if collisions.len() == 1 {
            let finding = collisions[0];
            failures.push(t_true(finding.subject == "/PortOwnerCollision/CollisionCase",
                "collision subject was " + finding.subject));
            failures.push(t_true(finding.severity == "error",
                "collision severity was " + finding.severity + ", expected error"));
            failures.push(t_true(finding.message.contains("PORT_OWNER_COLLISION: `mass` has 2 input owners"),
                "collision message did not describe the duplicate input owners"));
            failures.push(t_true(finding.message.contains("winner: ")
                && finding.message.contains("System at"),
                "collision message did not identify the generated Modelica owner"));
            failures.push(t_true(finding.message.contains("Avian rigid body"),
                "collision message did not identify the rigid-body owner"));
            failures.push(t_true(finding.message.contains("/PortOwnerCollision/CollisionCase.inputs:mass"),
                "collision message did not include the Modelica property path"));
            failures.push(t_true(finding.message.contains("/PortOwnerCollision/CollisionCase.inputs/outputs:mass"),
                "collision message did not include the Avian property path"));
            failures.push(t_true(finding.message.contains("registry precedence 1")
                && finding.message.contains("registry precedence 2"),
                "collision message did not expose registry precedence"));
            failures.push(t_true(finding.message.contains("writes may be routed to the winner"),
                "collision message did not explain the input routing consequence"));
        }

        finish_test(this, failures);
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        finish_test(this, ["scene stopped while waiting for " + this.waiting_for]);
    }
}
