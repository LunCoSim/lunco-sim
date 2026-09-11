// Production Rhai acceptance for the live duplicate-port linter bridge.
//
// The USD fixture owns the topology. This scenario owns the observable
// contract: RunLint must expose one structured collision for the duplicate
// boundary and remain silent for the clean assembly.

fn by_rule(findings, rule) {
    let out = [];
    for finding in findings {
        if finding.rule == rule { out.push(finding); }
    }
    out
}

fn find_document() {
    let documents = query("ListOpenDocuments");
    if documents == () || documents.open_documents == () { return -1; }
    for item in documents.open_documents {
        if item.kind != "usd" { continue; }
        let root = query("QueryUsdPrim", #{
            doc_id: item.doc_id,
            path: "/PortOwnerCollision",
            children: false,
        });
        if root != () && root.ok != false { return item.doc_id; }
    }
    -1
}

fn has_subject(findings, subject) {
    for finding in findings {
        if finding.subject == subject { return true; }
    }
    false
}

fn on_start(me) {
    this.done = false;
    this.elapsed = 0.0;
    this.doc = -1;
    this.phase = 0;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    if this.phase == 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 10.0 {
                fail_fast("port-owner collision fixture was not discoverable",
                    "PORT OWNER COLLISION", "PORT_OWNER_COLLISION");
            }
            return;
        }
        cmd("RunLint", #{});
        this.phase = 1;
        return;
    }

    let report = query("LintReport");
    if report == () {
        if this.elapsed > 10.0 {
            fail_fast("LintReport query returned () after RunLint",
                "PORT OWNER COLLISION", "PORT_OWNER_COLLISION");
        }
        return;
    }

    let findings = report.findings;
    let collisions = by_rule(findings, "port-owner-collision");
    let failures = [];
    failures.push(t_true(collisions.len() == 1,
        "expected one port-owner-collision finding, got " + collisions.len()));
    if collisions.len() == 1 {
        let finding = collisions[0];
        failures.push(t_true(finding.subject == "/PortOwnerCollision/CollisionCase",
            "collision subject was " + finding.subject));
        failures.push(t_true(finding.severity == "warn",
            "collision severity was " + finding.severity + ", expected warn"));
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
    failures.push(t_true(!has_subject(collisions, "/PortOwnerCollision/CleanCase"),
        "clean assembly was reported as a duplicate owner"));

    this.done = true;
    report_verdict(failures, "PORT OWNER COLLISION", "PORT_OWNER_COLLISION");
}
