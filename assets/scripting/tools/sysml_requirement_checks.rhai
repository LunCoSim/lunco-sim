// Generic typed requirement observers and evidence formatting.
// Acceptance predicates belong in source SysML constraints. Twin Rhai binds
// composed observations and chooses provider-specific checks.

import "lunco://scripting/tools/sysml_requirements" as sysml_requirements;

fn source_text_list(source, qualified_name) {
    let values = sysml_requirements::native_value(source, qualified_name);
    if values == () || type_of(values) != "array" { return []; }
    let identifiers = [];
    for value in values {
        if type_of(value) == "EnumValue" { identifiers.push(value.literal); }
        else if type_of(value) == "string" { identifiers.push(value); }
        else { return []; }
    }
    identifiers
}

fn source_enum_identifiers(source, qualified_name) {
    let values = sysml_requirements::native_value(source, qualified_name);
    if values == () || type_of(values) != "array" { return []; }
    let identifiers = [];
    for value in values {
        if type_of(value) != "EnumValue" { return []; }
        identifiers.push(value.literal);
    }
    identifiers
}

fn source_position_list(source, qualified_name) {
    let values = sysml_requirements::native_value(source, qualified_name);
    if values == () || type_of(values) != "array" { return []; }
    for value in values { if type_of(value) != "Vec3" { return []; } }
    values
}

fn source_number_list(source, qualified_name) {
    let typed = sysml_requirements::vector(source, qualified_name);
    if typed == () { return []; }
    typed
}

fn _short_identity(name) {
    if name == () { return (); }
    let parts = name.split("::");
    if parts.len() == 0 { return name; }
    parts[parts.len() - 1]
}

fn exists(id, component, requirement, verification, path, expected_type) {
    #{id: id, component: component, requirement: _short_identity(requirement),
      verification: _short_identity(verification), kind: "exists", path: path,
      expected_type: expected_type, visible: true}
}

fn not_exists(id, component, requirement, verification, path) {
    #{id: id, component: component, requirement: _short_identity(requirement),
      verification: _short_identity(verification), kind: "not_exists", path: path}
}

fn children(id, component, requirement, verification, path, paths) {
    #{id: id, component: component, requirement: _short_identity(requirement),
      verification: _short_identity(verification), kind: "children", path: path,
      paths: paths, visible: true}
}

fn attribute_check(id, component, requirement, verification, path, attr,
                   expected_attr, tolerance) {
    #{id: id, component: component, requirement: _short_identity(requirement),
      verification: _short_identity(verification), kind: "attribute", path: path,
      attr: attr, expected_attr: expected_attr, tolerance: tolerance}
}

fn attribute_equals(id, component, requirement, verification, path, attr, expected) {
    #{id: id, component: component, requirement: _short_identity(requirement),
      verification: _short_identity(verification), kind: "attribute_equals", path: path,
      attr: attr, expected: expected}
}

fn extent_component(id, component, requirement, verification, path, index,
                    expected_attr, multiplier, tolerance) {
    #{id: id, component: component, requirement: _short_identity(requirement),
      verification: _short_identity(verification), kind: "extent_component", path: path,
      index: index, multiplier: multiplier, expected_attr: expected_attr,
      tolerance: tolerance}
}

fn bounds_component(id, component, requirement, verification, path, index,
                    expected_attr, multiplier, tolerance) {
    #{id: id, component: component, requirement: _short_identity(requirement),
      verification: _short_identity(verification), kind: "bounds_component", path: path,
      index: index, multiplier: multiplier, expected_attr: expected_attr,
      tolerance: tolerance}
}

fn source_list_count(id, component, requirement, verification, list_attr, count_attr) {
    #{id: id, component: component, requirement: _short_identity(requirement),
      verification: _short_identity(verification), kind: "source_list_count",
      path: list_attr, list_attr: list_attr, expected_attr: count_attr}
}

fn coverage(id, component, requirement, verification) {
    #{id: id, component: component, requirement: _short_identity(requirement),
      verification: _short_identity(verification), kind: "coverage"}
}

// Adapter/source-shape checks are diagnostics about the generic typed bridge,
// not acceptance evidence for a system requirement. Keep them out of the
// requirement evaluator and its coverage/provenance tables.
fn preflight(id, ok, message, actual, expected) {
    #{id: id, kind: "source_preflight", ok: ok == true,
      error: if ok == true { "" } else { message },
      message: message, actual: actual, expected: expected}
}

fn finish_preflight(checks, title, channel, root, package_name) {
    let failures = [];
    for check in checks {
        if check.ok != true { failures.push(check); }
    }
    let result = #{ok: failures.len() == 0, results: checks,
        failures: failures, check_count: checks.len(),
        requirement_count: 0, failure_count: failures.len(),
        metrics: #{root: root, component_package: package_name,
            source_preflight: true}};
    report_structured_verdict(result, title, channel);
    result
}

fn finish(source, checks, title, channel, root, package_name, verification) {
    let result = if checks.len() > 64 {
        sysml_requirements::evaluate_stream(source, checks, channel, title)
    } else {
        sysml_requirements::evaluate(source, checks)
    };
    let seen = #{};
    let evidence = [];
    let evidence_failures = [];
    if result.results != () {
        for check in result.results {
            let requirement = check.requirement;
            if requirement == () || requirement == "" || seen[requirement] == true { continue; }
            seen[requirement] = true;
            let selected = sysml_requirements::provenance_for_requirement(source, requirement);
            if selected.ok == true { evidence.push(selected.evidence); }
            else { evidence_failures.push(#{requirement: requirement, error: selected.error}); }
        }
    }
    result.source_evidence = evidence;
    result.source_evidence_failures = evidence_failures;
    if evidence_failures.len() > 0 {
        result.ok = false;
        if result.failures == () { result.failures = []; }
        for failure in evidence_failures { result.failures.push(failure); }
        result.failure_count = if result.failure_count == () { 0 } else { result.failure_count };
        result.failure_count += evidence_failures.len();
    }
    result.metrics = #{clock: clock_snapshot(), root: root,
        component_package: package_name, verification: result.verification,
        separate_component_document: true};
    report_structured_verdict(result, title, channel);
    result
}
