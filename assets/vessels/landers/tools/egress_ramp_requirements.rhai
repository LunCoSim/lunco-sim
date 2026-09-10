// Standalone egress-ramp component contract.
// Read-only: it queries composed USD and never repairs, hides, or rewrites it.

const PUBLIC_EGRESS_SOURCE = "https://www.astrobotic.com/astrobotic-griffin-lander-and-nasas-viper-moon-rover-complete-complex-test-drives/";

fn _query(doc_id, path, attrs, children) {
    let request = #{path: path};
    if doc_id != () { request.doc_id = doc_id; }
    if attrs != () { request.attrs = attrs; }
    if children != () { request.children = children; }
    query("QueryUsdPrim", request)
}

fn _ok(record) {
    record != () && record.ok != false && record.path != ()
}

fn _finding(path, message) {
    #{id: "RAMP-001", severity: "error", path: path, message: message}
}

fn _has(values, expected) {
    if values == () { return false; }
    for value in values { if value == expected { return true; } }
    false
}

fn deployment_allowed(angle_rad, limit_rad) {
    angle_rad != () && limit_rad != () && limit_rad > 0.0
        && angle_rad.abs() <= limit_rad
}

fn deployment_check(angle_rad, limit_rad) {
    if angle_rad == () || limit_rad == () {
        return #{ok: false, error: "ramp angle and limit are required"};
    }
    if deployment_allowed(angle_rad, limit_rad) {
        return #{ok: true, angle_rad: angle_rad, limit_rad: limit_rad};
    }
    #{ok: false, angle_rad: angle_rad, limit_rad: limit_rad,
      error: "ramp deployment exceeds its authored component limit"}
}

fn lint(doc_id, root) {
    let findings = [];
    let record = _query(doc_id, root,
        ["physics:mass", "physics:diagonalInertia", "lunco:component_role",
         "lunco:mechanism", "lunco:deployment_angle_limit_rad",
         "lunco:provenance:source", "visibility"], true);
    if !_ok(record) {
        findings.push(_finding(root, "egress ramp component root is missing"));
        return #{ok: false, root: root, findings: findings, errors: ["missing root"]};
    }
    if record.type_name != "Xform" {
        findings.push(_finding(root, "component root must be an Xform"));
    }
    if record.attrs["visibility"] == "invisible" {
        findings.push(_finding(root, "component root must not be hidden"));
    }
    if record.attrs["physics:mass"] == () || record.attrs["physics:mass"] <= 0.0 {
        findings.push(_finding(root, "component needs positive physics:mass"));
    }
    let inertia = record.attrs["physics:diagonalInertia"];
    if inertia == () || inertia.len() != 3
        || inertia[0] <= 0.0 || inertia[1] <= 0.0 || inertia[2] <= 0.0 {
        findings.push(_finding(root, "component needs positive diagonal inertia"));
    }
    for attr_name in ["lunco:component_role", "lunco:mechanism", "lunco:provenance:source"] {
        if record.attrs[attr_name] == () || record.attrs[attr_name] == "" {
            findings.push(_finding(root, "missing " + attr_name));
        }
    }
    let source = record.attrs["lunco:provenance:source"];
    if source != () && source != PUBLIC_EGRESS_SOURCE {
        findings.push(_finding(root, "component provenance must use the public egress source"));
    }
    let limit = record.attrs["lunco:deployment_angle_limit_rad"];
    if limit == () || limit <= 0.0 || limit > 1.57079632679 {
        findings.push(_finding(root, "deployment limit must be positive and no more than 90 degrees"));
    }

    for child_name in ["Surface", "EdgeRail", "EdgeRailInner", "HingeBlock", "MountSocket"] {
        let path = root + "/" + child_name;
        let child = _query(doc_id, path, ["physics:collisionEnabled", "visibility"], false);
        if !_ok(child) {
            findings.push(_finding(path, "required component child is missing"));
        } else if child.attrs["visibility"] == "invisible" {
            findings.push(_finding(path, "required ramp child is hidden"));
        }
        if (child_name == "Surface" || child_name == "EdgeRail"
            || child_name == "EdgeRailInner") {
            if !_ok(child) || child.attrs["physics:collisionEnabled"] != true {
                findings.push(_finding(path, "ramp contact geometry needs enabled collision"));
            }
        }
    }
    let errors = [];
    for finding in findings {
        errors.push(finding.path + ": " + finding.message);
    }
    #{ok: errors.len() == 0, root: root, findings: findings,
      errors: errors, public_source: PUBLIC_EGRESS_SOURCE}
}

fn lint_live(doc_id, root) {
    if doc_id == () {
        return #{ok: false, error: "lint_live requires an explicit component doc_id"};
    }
    let report = lint(doc_id, root);
    print("[egress-ramp] root=" + root + " result="
        + (if report.ok { "PASS" } else { "FAIL" })
        + " findings=" + report.findings.len());
    for finding in report.findings {
        print("[egress-ramp] ERROR " + finding.path + ": " + finding.message);
    }
    report
}
