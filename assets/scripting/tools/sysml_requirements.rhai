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

// Resolve one typed SysML attribute without silently choosing a colliding
// short name. Qualified names are the stable contract; a local name is only
// accepted when the validator proved it unique.
fn attribute(report, name) {
    if report == () || name == () || name == "" {
        return #{ok: false, error: "SysML attribute name is missing"};
    }
    if name.contains("::") {
        if report.attributes_qualified != () && report.attributes_qualified[name] != () {
            return #{ok: true, record: report.attributes_qualified[name]};
        }
        return #{ok: false, error: "qualified SysML attribute is missing: " + name};
    }
    if report.attribute_collisions != () {
        for collision in report.attribute_collisions {
            if collision.name == name {
                return #{ok: false, error: "ambiguous SysML attribute `" + name + "`; use a qualified name"};
            }
        }
    }
    if report.attributes == () || report.attributes[name] == () {
        return #{ok: false, error: "SysML attribute is missing: " + name};
    }
    #{ok: true, record: report.attributes[name]}
}

fn number(report, name) {
    let resolved = attribute(report, name);
    if resolved.ok != true || resolved.record.value == () {
        return ();
    }
    if resolved.record.value.number != () { return resolved.record.value.number.to_float(); }
    ()
}

fn text(report, name) {
    let resolved = attribute(report, name);
    if resolved.ok != true || resolved.record.value == () { return (); }
    _text(resolved.record.value.literal)
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
    let resolved = attribute(report, check.expected_attr);
    if resolved.ok != true || resolved.record.value == () { return (); }
    if resolved.record.value.number != () { return resolved.record.value.number.to_float(); }
    _text(resolved.record.value.literal)
}

fn _expected_error(report, check) {
    if check.expected_attr == () { return ""; }
    let resolved = attribute(report, check.expected_attr);
    if resolved.ok != true { return resolved.error; }
    if resolved.record.value == () { return "SysML attribute has no literal value: " + check.expected_attr; }
    ""
}

fn _failure(check, message) {
    #{ ok: false, id: check.id, component: check.component,
       requirement: check.requirement, verification: check.verification,
       kind: check.kind, path: check.path, error: message }
}

fn _check_one(report, check) {
    if check == () || check.id == () || check.requirement == ()
        || check.verification == () || check.kind == () {
        return #{ok: false, id: "<invalid>", error: "check requires id, requirement, verification, and kind"};
    }
    if _find_requirement(report, check.requirement) == () {
        return _failure(check, "SysML requirement usage is missing");
    }
    if !_verification_covers(report, check.requirement, check.verification) {
        return _failure(check, "no SysML verification case covers this requirement");
    }
    let expected_error = _expected_error(report, check);
    if expected_error != "" { return _failure(check, expected_error); }
    if check.kind == "coverage" { return #{ok: true, id: check.id, component: check.component, requirement: check.requirement}; }

    // Compare a typed comma-separated SysML identity list with its declared
    // count.  This is intentionally a generic source-side check: Twin tests
    // still verify every composed USD path separately, while this catches a
    // stale list/count contract before a loop can silently omit a component.
    if check.kind == "source_list_count" {
        if check.list_attr == () || check.list_attr == "" {
            return _failure(check, "SysML identity list attribute is missing");
        }
        let resolved = attribute(report, check.list_attr);
        if resolved.ok != true || resolved.record.value == () {
            return _failure(check, "SysML identity list is missing");
        }
        let literal = _text(resolved.record.value.literal);
        let actual = 0;
        if literal != "" {
            for item in literal.split(",") {
                item.trim();
                if item != "" { actual += 1; }
            }
        }
        let expected = _expected(report, check);
        if expected == () { return _failure(check, "SysML expected count is missing"); }
        if actual.to_float() != expected.to_float() {
            return #{ok: false, id: check.id, component: check.component,
                requirement: check.requirement, path: check.path,
                error: "SysML identity list length disagrees with declared count",
                actual: actual, expected: expected};
        }
        return #{ok: true, id: check.id, component: check.component,
            requirement: check.requirement, kind: check.kind,
            actual: actual, expected: expected};
    }

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
        let matches = false;
        if _numeric(actual) && _numeric(expected) {
            matches = _close(actual, expected, tolerance);
        } else {
            // QueryUsdPrim preserves canonical quotes for authored string
            // literals.  Normalize both sides before comparing, exactly as
            // the attribute_equals observation does below.
            if type_of(actual) == "string" { actual = _text(actual); }
            if type_of(expected) == "string" { expected = _text(expected); }
            matches = actual == expected;
        }
        if !matches {
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
        let actual = actual_record.attrs[check.attr];
        let expected = if check.expected_attr == () { check.expected } else { _expected(report, check) };
        // QueryUsdPrim preserves the canonical quoted literal for some
        // authored string opinions.  Compare string content, not the literal
        // delimiters, while keeping numeric/bool equality exact.
        if type_of(actual) == "string" {
            actual = _text(actual);
            expected = _text(expected);
        }
        if actual != expected { return _failure(check, "USD attribute does not equal expected value"); }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path};
    }
    if check.kind == "relationship" {
        if check.rel == () || check.rel == "" {
            return _failure(check, "USD relationship name is missing");
        }
        let actual_record = query("QueryUsdPrim", #{path: check.path, rels: [check.rel]});
        if !_ok(actual_record) || actual_record.relationships == ()
            || actual_record.relationships[check.rel] == () {
            return _failure(check, "USD relationship is missing");
        }
        let actual = actual_record.relationships[check.rel];
        let expected = if check.targets == () { [] } else { check.targets };
        if actual.len() != expected.len() {
            return _failure(check, "USD relationship target count disagrees with expected value");
        }
        for target in expected {
            if !actual.contains(target) {
                return _failure(check, "USD relationship targets disagree with expected value");
            }
        }
        return #{ok: true, id: check.id, component: check.component,
            requirement: check.requirement, path: check.path, actual: actual,
            expected: expected};
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
        // Stamp the authored identity on every result, including helper
        // failures.  This keeps the evidence table self-describing and makes
        // it possible to aggregate reports without retaining the check list.
        if result.verification == () { result.verification = check.verification; }
        if result.requirement == () { result.requirement = check.requirement; }
        if result.component == () && check.component != () { result.component = check.component; }
        if result.kind == () { result.kind = check.kind; }
        results.push(result);
        if result.ok != true { failures.push(result); }
    }
    let verification = "";
    if checks.len() > 0 && checks[0].verification != () {
        verification = checks[0].verification;
    }
    #{ok: failures.len() == 0, results: results, failures: failures,
      requirement_count: checks.len(), failure_count: failures.len(),
      verification: verification,
      source_revision: source_report.source_revision_hex,
      source_files: source_report.source_files}
}

/// Build the canonical source report for the active Twin. A test can pass the
/// result directly to `evaluate`, avoiding duplicated ValidateSysml calls.
fn source() {
    let name = twin_name();
    if name == "" { return #{ok: false, error: "active Twin is unavailable"}; }
    query("ValidateSysml", #{path: "twin://" + name, compact: true})
}
