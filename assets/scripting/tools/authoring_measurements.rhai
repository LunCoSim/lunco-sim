// Generic authored measurement policy.
//
// USD owns geometry, transforms, collision representation, units and
// provenance. This library only consumes the typed QueryUsdPrim surface and
// evaluates caller-supplied requirements. It is deliberately independent of
// vehicles, routes, Twin names, and a custom measurement schema.

fn query_ok(record) {
    record != () && record.ok != false
}

fn unit_scale(units) {
    if units == "m" { return 1.0; }
    if units == "cm" { return 0.01; }
    if units == "mm" { return 0.001; }
    ()
}

fn unavailable(subject, kind, message, source) {
    #{
        ok: false,
        status: "unavailable",
        subject: subject,
        kind: kind,
        message: message,
        source: source,
    }
}

fn distance3(a, b) {
    if a == () || b == () { return (); }
    if type_of(a) != "Vec3" && (type_of(a) != "array" || a.len() != 3) { return (); }
    if type_of(b) != "Vec3" && (type_of(b) != "array" || b.len() != 3) { return (); }
    vlen(vsub(vec3_from(a), vec3_from(b)))
}

fn verdict(actual, expected, tolerance) {
    if actual == () || expected == () || tolerance == () {
        return "unavailable";
    }
    if (actual - expected).abs() <= tolerance { "passed" } else { "failed" }
}

fn distance_requirement(doc, spec) {
    let from = spec.from;
    let to = spec.to;
    let units = if spec.units != () { spec.units } else { "m" };
    let scale = unit_scale(units);
    let subject = from + " ↔ " + to;
    if scale == () {
        return unavailable(subject, "distance", "unsupported units: " + units, "Rhai");
    }
    // Read both endpoints from one composed-stage snapshot. This avoids
    // reopening the stage and rechecking the document generation for every
    // endpoint when a component has many dimensional rules.
    let records = query("QueryUsdPrims", #{ doc_id: doc, paths: [from, to] });
    if !query_ok(records) || records.records == () || records.records.len() != 2 {
        return unavailable(subject, "distance", "one or both USD subjects are unavailable", "QueryUsdPrims");
    }
    let a = records.records[0];
    let b = records.records[1];
    let meters = distance3(a.world_position, b.world_position);
    let actual = if meters != () { meters / scale } else { () };
    let expected = if spec.expected != () { spec.expected } else { () };
    let tolerance = if spec.tolerance != () { spec.tolerance } else { () };
    let status = verdict(actual, expected, tolerance);
    #{
        ok: status == "passed",
        status: status,
        kind: "distance",
        subject: subject,
        from: from,
        to: to,
        frame: "canonical_stage",
        units: units,
        actual: actual,
        expected: expected,
        tolerance: tolerance,
        method: "QueryUsdPrims.world_position",
        source: "USD",
    }
}

fn bounds_requirement(doc, spec) {
    let path = spec.path;
    let units = if spec.units != () { spec.units } else { "m" };
    let scale = unit_scale(units);
    if scale == () {
        return unavailable(path, "extent", "unsupported units: " + units, "Rhai");
    }
    let record = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        collision_bounds: true,
    });
    if !query_ok(record) || record.collision_bounds == () {
        return unavailable(path, "extent", "collision representation has no composed bounds", "QueryUsdPrim");
    }
    let bounds = record.collision_bounds;
    let extents = bounds.half_extents;
    let axis = if spec.axis != () { spec.axis } else { 0 };
    if extents == () || axis < 0 || axis > 2 {
        return unavailable(path, "extent", "collision bounds have no requested axis", "QueryUsdPrim");
    }
    let actual = extents[axis] * 2.0 / scale;
    let expected = if spec.expected != () { spec.expected } else { () };
    let tolerance = if spec.tolerance != () { spec.tolerance } else { () };
    let status = verdict(actual, expected, tolerance);
    #{
        ok: status == "passed",
        status: status,
        kind: "extent",
        subject: path,
        axis: axis,
        representation: "collision",
        frame: bounds.frame,
        units: units,
        actual: actual,
        expected: expected,
        tolerance: tolerance,
        bounds: bounds,
        method: "QueryUsdPrim.collision_bounds",
        source: "USD",
    }
}

fn evaluate_requirement(doc, spec) {
    if spec == () || spec.kind == () {
        return unavailable("<missing requirement>", "unknown", "requirement kind is required", "Rhai");
    }
    if spec.kind == "distance" { return distance_requirement(doc, spec); }
    if spec.kind == "extent" { return bounds_requirement(doc, spec); }
    unavailable("<unknown requirement>", spec.kind, "unsupported requirement kind", "Rhai")
}

/// Evaluate every rule and retain every result. Findings contain only failed
/// or unavailable rules, while `checks` proves that no rule was skipped.
fn requirement_report(doc, requirements) {
    if doc == () {
        return #{ ok: false, status: "unavailable", error: "explicit document id is required" };
    }
    if requirements == () || requirements.len() == 0 {
        return #{ ok: false, status: "unavailable", error: "at least one requirement is required" };
    }
    let checks = [];
    let findings = [];
    for spec in requirements {
        let check = evaluate_requirement(doc, spec);
        checks.push(check);
        if check.status != "passed" { findings.push(check); }
    }
    #{
        ok: findings.len() == 0,
        status: if findings.len() == 0 { "passed" } else { "failed" },
        document: doc,
        checks: checks,
        findings: findings,
        evaluated: checks.len(),
        failed: findings.len(),
    }
}

/// Compact evidence contract for a UI/agent consumer. It keeps the actual
/// method, frame, units, tolerance, and generation owner-visible instead of
/// reducing a measurement to a bare boolean.
fn evidence_summary(report) {
    if report == () { return (); }
    #{
        ok: report.ok,
        status: report.status,
        evaluated: report.evaluated,
        failed: report.failed,
        checks: report.checks,
        findings: report.findings,
    }
}
