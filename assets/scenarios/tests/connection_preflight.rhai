// Connection preflight is intentionally a negative Rhai scene test. It proves
// that standard USD connections are inspected before simulation and that each
// broken source remains visible by exact rule and consuming-property subject.

fn by_rule(findings, rule) {
    let out = [];
    for finding in findings {
        if finding.rule == rule { out.push(finding); }
    }
    out
}

fn has_subject(findings, subject) {
    for finding in findings {
        if finding.subject == subject { return true; }
    }
    false
}

fn find_document() {
    let documents = query("ListOpenDocuments");
    if documents == () || documents.open_documents == () { return -1; }
    for item in documents.open_documents {
        if item.kind != "usd" { continue; }
        let root = query("QueryUsdPrim", #{
            doc_id: item.doc_id,
            path: "/ConnectionPreflight",
            children: false,
        });
        if root != () && root.ok != false { return item.doc_id; }
    }
    -1
}

fn on_start(me) {
    this.done = false;
    this.elapsed = 0.0;
    this.phase = 0;
    this.command_id = -1;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.phase == 0 {
        let doc = find_document();
        if doc < 0 {
            if this.elapsed > 8.0 {
                fail_fast("connection_preflight USD document was not discoverable",
                    "CONNECTION PREFLIGHT", "CONNECTION_PREFLIGHT");
            }
            return;
        }
        let before = assembly_edit::describe(doc);
        let accepted = assembly_edit::connection(
            doc,
            "@root@",
            "/ConnectionPreflight/Sink",
            "inputs:command_missing",
            "float",
            ["/ConnectionPreflight/Ghost.outputs:signal"],
            before.generation,
        );
        if accepted.id == () {
            fail_fast("ApplyUsdOp did not return a command id for the invalid connection",
                "CONNECTION PREFLIGHT", "CONNECTION_PREFLIGHT");
            return;
        }
        this.command_id = accepted.id;
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        let rejected = command_result(this.command_id);
        if rejected.status == "pending" {
            if this.elapsed > 8.0 {
                fail_fast("invalid connection command stayed pending instead of producing a terminal result",
                    "CONNECTION PREFLIGHT", "CONNECTION_PREFLIGHT");
            }
            return;
        }
        if rejected.status != "failed" || rejected.error == ()
            || !rejected.error.contains("missing prim") {
            fail_fast("invalid connection command did not fail loudly: " + rejected.error,
                "CONNECTION PREFLIGHT", "CONNECTION_PREFLIGHT");
            return;
        }
        this.phase = 2;
    }

    if this.phase == 2 {
        // RunLint also folds the live PortRegistry check for runtime-only
        // provider ports. Read it on the next scenario tick so the queued
        // live evidence is terminal before assertions inspect the report.
        cmd("RunLint", #{});
        this.phase = 3;
        return;
    }

    this.done = true;
    let report = query("LintReport");
    if report == () {
        fail_fast("LintReport query returned (), so connection preflight was not exercised",
            "CONNECTION PREFLIGHT", "CONNECTION_PREFLIGHT");
        return;
    }

    let findings = report.findings;
    let failures = [];
    let missing_property = by_rule(findings, "connection-source-property-missing");
    let missing_prim = by_rule(findings, "connection-source-prim-missing");
    let mismatch = by_rule(findings, "connection-source-type-mismatch");

    if missing_property.len() != 1 {
        failures.push("connection-source-property-missing count=" + missing_property.len() + ", expected 1");
    } else if !has_subject(missing_property, "/ConnectionPreflight/Sink.inputs:missing_property") {
        failures.push("missing-property finding did not name the consuming property");
    }
    if missing_prim.len() != 1 {
        failures.push("connection-source-prim-missing count=" + missing_prim.len() + ", expected 1");
    } else if !has_subject(missing_prim, "/ConnectionPreflight/Sink.inputs:missing_prim") {
        failures.push("missing-prim finding did not name the consuming property");
    }
    if mismatch.len() != 1 {
        failures.push("connection-source-type-mismatch count=" + mismatch.len() + ", expected 1");
    } else if !has_subject(mismatch, "/ConnectionPreflight/Sink.inputs:type_mismatch") {
        failures.push("type-mismatch finding did not name the consuming property");
    }
    let runtime_missing = by_rule(findings, "connection-source-runtime-port-missing");
    if runtime_missing.len() != 1 {
        failures.push("connection-source-runtime-port-missing count=" + runtime_missing.len() + ", expected 1");
    } else if !has_subject(runtime_missing, "/ConnectionPreflight/Sink.inputs:runtime_missing") {
        failures.push("runtime-port finding did not name the consuming property");
    }
    let runtime_invalid = by_rule(findings, "connection-source-runtime-direction-invalid");
    if runtime_invalid.len() != 1 {
        failures.push("connection-source-runtime-direction-invalid count=" + runtime_invalid.len() + ", expected 1");
    } else if !has_subject(runtime_invalid, "/ConnectionPreflight/Sink.inputs:runtime_invalid") {
        failures.push("runtime-direction finding did not name the consuming property");
    }
    if has_subject(findings, "/ConnectionPreflight/Sink.inputs:valid") {
        failures.push("valid float connection was reported as broken");
    }
    if has_subject(findings, "/ConnectionPreflight/Sink.inputs:runtime_valid") {
        failures.push("valid runtime connection was reported as broken");
    }
    if has_subject(findings, "/ConnectionPreflight/Sink.inputs:revolute_valid") {
        failures.push("standard PhysicsRevoluteJoint angle connection was reported as broken");
    }
    if has_subject(findings, "/ConnectionPreflight/Sink.inputs:prismatic_valid") {
        failures.push("standard PhysicsPrismaticJoint displacement connection was reported as broken");
    }
    if report.errors != 5 {
        failures.push("total lint errors=" + report.errors + ", expected exactly 5 connection errors");
    }

    if failures.len() != 0 {
        for failure in failures { print("  FAIL: " + failure); }
        print("CONNECTION PREFLIGHT: FAIL");
        emit("CONNECTION_PREFLIGHT", "FAIL");
        notify_kind("CONNECTION PREFLIGHT: FAIL", "error");
        return;
    }
    print("CONNECTION PREFLIGHT: PASS");
    emit("CONNECTION_PREFLIGHT", "PASS");
}
