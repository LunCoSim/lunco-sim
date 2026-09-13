// Generic SysML v2 -> USD requirement evaluator.
//
// SysML remains the normative source: a check names a requirement usage and
// reads expected values through `expected_attr` from ValidateSysml.  The check
// table is authored by a Twin, while this evaluator is reusable by any Twin.
// It deliberately supports only typed, read-only observations; it never edits
// USD and never embeds a vehicle name or domain assumption.

fn _text(value) {
    if value == () { return ""; }
    let result = value;
    if result.len() > 0 && result.starts_with("\"") {
        result = result.sub_string(1, result.len() - 1);
    }
    if result.len() > 0 && result.ends_with("\"") {
        result = result.sub_string(0, result.len() - 1);
    }
    result
}

fn _close(actual, expected, tolerance) {
    actual != () && expected != () && (actual - expected).abs() <= tolerance
}

fn _numeric(value) {
    let kind = type_of(value);
    kind == "float" || kind == "int" || kind == "f32" || kind == "f64"
        || kind == "decimal"
}

fn _query(path, attrs) {
    let request = #{path: path};
    if attrs != () { request.attrs = attrs; }
    query("QueryUsdPrim", request)
}

fn _ok(record) {
    record != () && record.ok != false && record.path != ()
}

fn _record_name(record) {
    if record == () { return ""; }
    if record.element != () && record.element.qualified_name != () {
        return record.element.qualified_name;
    }
    if record.qualified_name != () { return record.qualified_name; }
    ""
}

fn _short_name(name) {
    let parts = name.split("::");
    if parts.len() == 0 { return name; }
    parts[parts.len() - 1]
}

fn _find_requirement(report, expected) {
    if report == () || report.requirement_records == () { return (); }
    for record in report.requirement_records {
        let name = _record_name(record);
        if name == expected || _short_name(name) == expected { return record; }
    }
    ()
}

fn _verification_covers(report, requirement, verification_name) {
    if report == () || report.verification_records == () { return false; }
    let wanted = _short_name(requirement);
    for verification in report.verification_records {
        let name = _record_name(verification);
        if verification_name != () && verification_name != ""
            && name != verification_name && _short_name(name) != verification_name {
            continue;
        }
        if verification.verifies == () { continue; }
        for covered in verification.verifies {
            if covered == requirement || _short_name(covered) == wanted { return true; }
        }
    }
    false
}

fn _expected(report, check) {
    if check.expected_attr == () { return check.expected; }
    if report == () || report.attributes == () { return (); }
    let attribute = report.attributes[check.expected_attr];
    if attribute == () || attribute.value == () { return (); }
    if attribute.value.number != () { return attribute.value.number.to_float(); }
    _text(attribute.value.literal)
}

fn _failure(check, message) {
    #{ ok: false, id: check.id, component: check.component,
       requirement: check.requirement, path: check.path, error: message }
}

fn _check_one(report, check) {
    if check == () || check.id == () || check.requirement == () || check.kind == () {
        return #{ok: false, id: "<invalid>", error: "check requires id, requirement, and kind"};
    }
    if _find_requirement(report, check.requirement) == () {
        return _failure(check, "SysML requirement usage is missing");
    }
    if !_verification_covers(report, check.requirement, check.verification) {
        return _failure(check, "no SysML verification case covers this requirement");
    }
    if check.kind == "coverage" { return #{ok: true, id: check.id, component: check.component, requirement: check.requirement}; }

    if check.path == () || check.path == "" {
        return _failure(check, "USD observation path is missing");
    }
    if check.kind == "exists" {
        let record = _query(check.path, ["visibility"]);
        if !_ok(record) { return _failure(check, "USD prim is missing"); }
        if check.expected_type != () && record.type_name != check.expected_type {
            return _failure(check, "USD type is " + record.type_name + ", expected " + check.expected_type);
        }
        if check.visible == true && record.attrs["visibility"] == "invisible" {
            return _failure(check, "USD prim is invisible");
        }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path};
    }
    if check.kind == "children" {
        let missing = [];
        for child_path in check.paths {
            let child = _query(child_path, ["visibility"]);
            if !_ok(child) || check.visible == true && child.attrs["visibility"] == "invisible" {
                missing.push(child_path);
            }
        }
        if missing.len() > 0 { return _failure(check, "missing/invisible USD children: " + missing); }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path};
    }
    if check.kind == "attribute" {
        let actual_record = _query(check.path, [check.attr]);
        if !_ok(actual_record) || actual_record.attrs[check.attr] == () {
            return _failure(check, "USD attribute " + check.attr + " is missing");
        }
        let actual = actual_record.attrs[check.attr];
        let expected = _expected(report, check);
        if expected == () { return _failure(check, "SysML expected attribute is missing"); }
        let tolerance = if check.tolerance == () { 0.001 } else { check.tolerance };
        if _numeric(actual) && _numeric(expected) {
            if !_close(actual, expected, tolerance) {
                return #{ok: false, id: check.id, component: check.component,
                    requirement: check.requirement, path: check.path,
                    error: "USD attribute disagrees with SysML expected value",
                    actual: actual, expected: expected, tolerance: tolerance};
            }
        } else if actual != expected {
            return #{ok: false, id: check.id, component: check.component,
                requirement: check.requirement, path: check.path,
                error: "USD attribute disagrees with SysML expected value",
                actual: actual, expected: expected};
        }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path, actual: actual, expected: expected};
    }
    if check.kind == "attribute_component" {
        let actual_record = _query(check.path, [check.attr]);
        if !_ok(actual_record) || actual_record.attrs[check.attr] == () {
            return _failure(check, "USD vector attribute " + check.attr + " is missing");
        }
        let vector = actual_record.attrs[check.attr];
        if check.index == () || vector.len() <= check.index { return _failure(check, "USD vector component is missing"); }
        let expected = _expected(report, check);
        let tolerance = if check.tolerance == () { 0.001 } else { check.tolerance };
        if !_close(vector[check.index], expected, tolerance) { return _failure(check, "USD vector component disagrees with SysML expected value"); }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path, actual: vector[check.index], expected: expected};
    }
    if check.kind == "extent_component" {
        let actual_record = _query(check.path, ["size", "xformOp:scale"]);
        if !_ok(actual_record) || actual_record.attrs["size"] == ()
            || actual_record.attrs["xformOp:scale"] == () {
            return _failure(check, "USD extent source attributes are missing");
        }
        let scale = actual_record.attrs["xformOp:scale"];
        if check.index == () || scale.len() <= check.index { return _failure(check, "USD extent vector component is missing"); }
        let multiplier = if check.multiplier == () { 1.0 } else { check.multiplier };
        let actual = actual_record.attrs["size"] * scale[check.index] * multiplier;
        let expected = _expected(report, check);
        let tolerance = if check.tolerance == () { 0.001 } else { check.tolerance };
        if !_close(actual, expected, tolerance) {
            return #{ok: false, id: check.id, component: check.component,
                requirement: check.requirement, path: check.path,
                error: "derived USD extent disagrees with SysML expected value",
                actual: actual, expected: expected, tolerance: tolerance};
        }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path, actual: actual, expected: expected};
    }
    if check.kind == "bounds_component" {
        let actual_record = query("QueryUsdPrim", #{path: check.path, geometry_bounds: true});
        if !_ok(actual_record) || actual_record.geometry_bounds == () {
            return _failure(check, "composed USD geometry bounds are missing");
        }
        let bounds = actual_record.geometry_bounds;
        if bounds == () || bounds.half_extents == ()
            || check.index == () || bounds.half_extents.len() <= check.index {
            return _failure(check, "composed USD geometry bounds component is missing");
        }
        let multiplier = if check.multiplier == () { 1.0 } else { check.multiplier };
        let actual = bounds.half_extents[check.index] * multiplier;
        let expected = _expected(report, check);
        if expected == () { return _failure(check, "SysML expected attribute is missing"); }
        let tolerance = if check.tolerance == () { 0.001 } else { check.tolerance };
        if !_close(actual, expected, tolerance) {
            return #{ok: false, id: check.id, component: check.component,
                requirement: check.requirement, path: check.path,
                error: "composed USD geometry bound disagrees with SysML expected value",
                actual: actual, expected: expected, tolerance: tolerance};
        }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement,
            path: check.path, actual: actual, expected: expected};
    }
    if check.kind == "attribute_equals" {
        let actual_record = _query(check.path, [check.attr]);
        if !_ok(actual_record) || actual_record.attrs[check.attr] == () { return _failure(check, "USD attribute is missing"); }
        if actual_record.attrs[check.attr] != check.expected { return _failure(check, "USD attribute does not equal expected value"); }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path};
    }
    _failure(check, "unknown generic SysML/USD check kind " + check.kind)
}

/// Evaluate a Twin-authored check table. Returns one result per component
/// check, preserving IDs and paths for a test report or UI evidence panel.
fn evaluate(source_report, checks) {
    let results = [];
    let failures = [];
    if source_report == () || source_report.ok != true {
        return #{ok: false, results: [], failures: ["SysML source report is unavailable"]};
    }
    for check in checks {
        let result = _check_one(source_report, check);
        results.push(result);
        if result.ok != true { failures.push(result); }
    }
    #{ok: failures.len() == 0, results: results, failures: failures,
      requirement_count: checks.len(), failure_count: failures.len(),
      source_revision: source_report.source_revision_hex}
}

/// Build the canonical source report for the active Twin. A test can pass the
/// result directly to `evaluate`, avoiding duplicated ValidateSysml calls.
fn source() {
    let name = twin_name();
    if name == "" { return #{ok: false, error: "active Twin is unavailable"}; }
    query("ValidateSysml", #{path: "twin://" + name})
}
