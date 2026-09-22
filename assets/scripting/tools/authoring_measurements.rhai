// Generic authored measurement policy.
//
// USD owns geometry, transforms, collision representation, units and
// provenance. This library only consumes the typed QueryUsdPrim surface and
// evaluates caller-supplied requirements. It is deliberately independent of
// vehicles, routes, Twin names, and a custom measurement schema.

import "lunco://scripting/tools/engineering_units" as engineering_units;
import "lunco://scripting/tools/mechanical_relations" as mechanical_relations;

fn query_ok(record) {
    record != () && record.ok != false
}

fn unit_scale(units) {
    engineering_units::linear_scale(units)
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
    if !vec3_is_finite(a) || !vec3_is_finite(b) { return (); }
    vlen(vsub(vec3_from(a), vec3_from(b)))
}

fn verdict(actual, expected, tolerance) {
    let relation = mechanical_relations::equal(actual, expected, tolerance);
    if relation == () { return "unavailable"; }
    relation.status
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

fn _axis_value(value, axis) {
    if !vec3_is_finite(value) { return (); }
    if axis == "x" { return vcomponent(value, 0); }
    if axis == "y" { return vcomponent(value, 1); }
    if axis == "z" { return vcomponent(value, 2); }
    ()
}

/// Check a point against a plane using only native typed vectors. The plane
/// normal and expected distance are authored by the caller; Rust does not
/// choose an axis or a tolerance.
fn plane_distance_requirement(doc, spec) {
    let point_path = spec.point;
    let plane_path = spec.plane;
    let units = if spec.units != () { spec.units } else { "m" };
    let scale = unit_scale(units);
    let subject = point_path + " ↔ plane " + plane_path;
    if scale == () {
        return unavailable(subject, "plane_distance", "unsupported units: " + units, "Rhai");
    }
    let records = query("QueryUsdPrims", #{doc_id: doc, paths: [point_path, plane_path]});
    if !query_ok(records) || records.records == () || records.records.len() != 2 {
        return unavailable(subject, "plane_distance", "point or plane subject is unavailable", "QueryUsdPrims");
    }
    let normal = if spec.normal != () { vec3_from(spec.normal) } else { vec3(0.0, 1.0, 0.0) };
    if !vec3_is_valid(normal) {
        return unavailable(subject, "plane_distance", "plane normal is invalid", "Rhai");
    }
    let point = records.records[0].world_position;
    let plane = records.records[1].world_position;
    if !vec3_is_finite(point) || !vec3_is_finite(plane) {
        return unavailable(subject, "plane_distance", "point or plane position is invalid", "QueryUsdPrims");
    }
    let actual = mechanical_relations::plane_signed_distance(point, plane, normal) / scale;
    let expected = if spec.expected != () { spec.expected } else { 0.0 };
    let tolerance = if spec.tolerance != () { spec.tolerance } else { 0.0 };
    let status = verdict(actual, expected, tolerance);
    #{ok: status == "passed", status: status, kind: "plane_distance",
      subject: subject, point: point_path, plane: plane_path, frame: "canonical_stage",
      units: units, actual: actual, expected: expected, tolerance: tolerance,
      normal: normal, method: "QueryUsdPrims.world_position", source: "USD+Rhai"}
}

/// Check that one authored subject is below another along an explicitly
/// selected axis. This is a reusable relation policy, not a Griffin rule.
fn under_requirement(doc, spec) {
    let child_path = spec.child;
    let parent_path = spec.parent;
    let axis = if spec.axis != () { spec.axis } else { "y" };
    let units = if spec.units != () { spec.units } else { "m" };
    let scale = unit_scale(units);
    let subject = child_path + " under " + parent_path + " along " + axis;
    if scale == () {
        return unavailable(subject, "under", "unsupported units: " + units, "Rhai");
    }
    let records = query("QueryUsdPrims", #{doc_id: doc, paths: [child_path, parent_path]});
    if !query_ok(records) || records.records == () || records.records.len() != 2 {
        return unavailable(subject, "under", "child or parent subject is unavailable", "QueryUsdPrims");
    }
    let child = _axis_value(records.records[0].world_position, axis);
    let parent = _axis_value(records.records[1].world_position, axis);
    if child == () || parent == () {
        return unavailable(subject, "under", "axis is invalid or position is unavailable", "QueryUsdPrims");
    }
    let minimum = if spec.minimum != () { spec.minimum } else { 0.0 };
    let tolerance = if spec.tolerance != () { spec.tolerance } else { 0.0 };
    let actual = mechanical_relations::axis_delta(
        records.records[1].world_position, records.records[0].world_position, axis) / scale;
    let relation = mechanical_relations::at_least(actual, minimum, tolerance);
    let status = if relation == () { "unavailable" } else { relation.status };
    #{ok: status == "passed", status: status, kind: "under", subject: subject,
      child: child_path, parent: parent_path, axis: axis, frame: "canonical_stage",
      units: units, actual: actual, minimum: minimum, tolerance: tolerance,
      method: "QueryUsdPrims.world_position", source: "USD+Rhai"}
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
    if spec.kind == "plane_distance" { return plane_distance_requirement(doc, spec); }
    if spec.kind == "under" { return under_requirement(doc, spec); }
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
