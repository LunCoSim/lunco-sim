// Generic SysML v2 -> USD requirement evaluator.
//
// SysML remains the normative source: a check names a requirement usage and
// reads expected values through `expected_attr` from AnalyzeSysml. The check
// table is authored by a Twin, while this evaluator is reusable by any Twin.
// It deliberately supports only typed, read-only observations; it never edits
// USD and never embeds a vehicle name or domain assumption. Constraint
// predicates execute from the source-pinned SysML model through the neutral
// constraint IR; this module only binds observations and records the verdict.

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

fn _numeric_sysml_type(declared_type) {
    if declared_type == () || declared_type.primitive == () { return false; }
    let primitive = _text(declared_type.primitive);
    primitive == "Integer" || primitive == "Rational" || primitive == "Real"
}

fn _string_sysml_type(declared_type) {
    declared_type != () && declared_type.primitive != ()
        && _text(declared_type.primitive) == "String"
}

// Compare structured USD values without flattening them into strings.  USD
// transform orders, colour arrays, and topology lists are all authored values;
// silently coercing them would let a malformed order pass a requirement gate.
fn _equal_value(actual, expected) {
    if actual == () || expected == () { return actual == expected; }
    if type_of(actual) == "array" || type_of(expected) == "array" {
        if type_of(actual) != "array" || type_of(expected) != "array"
            || actual.len() != expected.len() { return false; }
        for index in 0..actual.len() {
            if !_equal_value(actual[index], expected[index]) { return false; }
        }
        return true;
    }
    if type_of(actual) == "string" || type_of(expected) == "string" {
        return _text(actual) == _text(expected);
    }
    actual == expected
}

fn _query(path, attrs, doc_id) {
    let request = #{path: path};
    if doc_id != () { request.doc_id = doc_id; }
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

// Resolve an authored requirement usage to its canonical qualified identity.
// Twin observers may use a short local ID (for example `wheel001`) to keep a
// repeated check table compact, but the source report is the only authority
// for the package-qualified name.  Never choose between collisions: a short
// ID that occurs more than once is an authored error and must be reported.
fn requirement_name(report, expected) {
    if report == () || report.requirement_records == ()
        || expected == () || expected == "" {
        return #{ok: false, error: "SysML requirement identity is missing"};
    }
    let found = ();
    for record in report.requirement_records {
        let name = _record_name(record);
        if name == expected || _short_name(name) == expected {
            if found != () {
                return #{ok: false,
                    error: "ambiguous SysML requirement identity: " + expected};
            }
            found = name;
        }
    }
    if found == () {
        return #{ok: false,
            error: "SysML requirement usage is missing: " + expected};
    }
    #{ok: true, name: found}
}

// Resolve a verification case using the same source-backed identity rule as
// requirements.  Registry names and evidence names therefore converge on the
// exact qualified SysML case instead of each observer carrying a package
// prefix that can drift from its source.
fn verification_name(report, expected) {
    if report == () || report.verification_records == ()
        || expected == () || expected == "" {
        return #{ok: false, error: "SysML verification identity is missing"};
    }
    let found = ();
    for record in report.verification_records {
        let name = _record_name(record);
        if name == expected || _short_name(name) == expected {
            if found != () {
                return #{ok: false,
                    error: "ambiguous SysML verification identity: " + expected};
            }
            found = name;
        }
    }
    if found == () {
        return #{ok: false,
            error: "SysML verification case is missing: " + expected};
    }
    #{ok: true, name: found}
}

// Validate every check identity once before evaluation.  Keep the authored
// records compact: component observers may repeat one requirement hundreds of
// times, and copying a qualified name into every record can exceed Rhai's
// string/value budget.  Evaluation stamps the canonical names on emitted
// evidence instead.
fn _resolve_indexed_identity(index, expected, kind) {
    if expected == () || expected == "" {
        return #{ok: false, error: "SysML " + kind + " identity is missing"};
    }
    let found = index[expected];
    if found == () {
        return #{ok: false,
            error: "SysML " + kind + " identity is missing: " + expected};
    }
    if found == "<ambiguous>" {
        return #{ok: false,
            error: "ambiguous SysML " + kind + " identity: " + expected};
    }
    #{ok: true, name: found}
}

fn _report_error(report, fallback) {
    if type_of(report) != "map" { return fallback; }
    if report.error != () && report.error != "" { return report.error; }
    if report.errors != () && report.errors.len() > 0 {
        return report.errors[0];
    }
    fallback
}

fn _resolve_check_identities(checks, indexes) {
    for check in checks {
        if check == () || check.requirement == () || check.verification == () {
            return #{ok: false, check_id: if check == () { "<invalid>" } else { check.id },
                error: "check requires requirement and verification identities"};
        }
        let requirement = _resolve_indexed_identity(indexes.requirement_identities,
            check.requirement, "requirement");
        if requirement.ok != true {
            return #{ok: false, check_id: check.id, error: requirement.error};
        }
        let verification = _resolve_indexed_identity(indexes.verification_identities,
            check.verification, "verification");
        if verification.ok != true {
            return #{ok: false, check_id: check.id, error: verification.error};
        }
    }
    #{ok: true}
}

// Resolve a Twin component and its verification case from the composed Rhai
// policy report. Ownership and mapping are manifest data, so this
// helper deliberately rejects registry errors, missing names, and duplicate
// matches instead of guessing a scene or script.  Authored tests can then use
// `binding.component`/`binding.verification` while keeping identity in
// twin.toml and SysML.
fn component_binding(report, component_name) {
    if type_of(report) != "map" || report.ok != true {
        return #{ok: false,
            error: _report_error(report, "SysML/Twin source report is unavailable")};
    }
    if component_name == () || component_name == "" {
        return #{ok: false, error: "Twin component name is missing"};
    }
    if report.component_registry_errors != () && report.component_registry_errors.len() > 0 {
        return #{ok: false, error: "Twin component registry is invalid",
            details: report.component_registry_errors};
    }
    if report.verification_registry_errors != () && report.verification_registry_errors.len() > 0 {
        return #{ok: false, error: "Twin verification registry is invalid",
            details: report.verification_registry_errors};
    }
    if report.components == () {
        return #{ok: false, error: "Twin component registry is unavailable"};
    }
    let found = ();
    for component in report.components {
        if component.name == component_name {
            if found != () {
                return #{ok: false, error: "duplicate Twin component: " + component_name};
            }
            found = component;
        }
    }
    if found == () {
        return #{ok: false, error: "Twin component is not registered: " + component_name};
    }
    if found.verification == () || found.verification == "" {
        return #{ok: false, error: "Twin component has no verification: " + component_name};
    }
    if report.verification_registry == () {
        return #{ok: false, error: "Twin verification registry is unavailable"};
    }
    let verification = ();
    for candidate in report.verification_registry {
        if candidate.name == found.verification {
            if verification != () {
                return #{ok: false, error: "duplicate Twin verification: " + found.verification};
            }
            verification = candidate;
        }
    }
    if verification == () {
        return #{ok: false, error: "Twin verification is not registered: " + found.verification};
    }
    #{ok: true, component: found, verification: verification}
}

fn _lazy_attribute(report, name) {
    if report == () || report.path == () || report.path == "" {
        return #{ok: false, error: "SysML source report has no resolvable path"};
    }
    let selected = query("AnalyzeSysml", #{path: report.path,
        tables: ["attributes"], attribute_names: [name]});
    if selected == () || selected.analysis == () {
        return #{ok: false, error: _report_error(selected,
            "AnalyzeSysml returned no selected attribute facts")};
    }
    let found = ();
    for entry in selected.analysis.attributes {
        if entry.qualified_name == name || entry.name == name {
            if found != () {
                return #{ok: false,
                    error: "ambiguous SysML attribute `" + name + "`; use a qualified name"};
            }
            found = entry;
        }
    }
    #{ok: true, record: found}
}

// Resolve one typed SysML attribute without silently choosing a colliding
// short name. Qualified names are the stable contract; a local name is only
// accepted when the validator proved it unique.
fn attribute(report, name) {
    if type_of(report) != "map" || name == () || name == "" {
        return #{ok: false, error: "SysML attribute name is missing"};
    }
    if name.contains("::") {
        if report.attributes_qualified != () && report.attributes_qualified[name] != () {
            return #{ok: true, record: report.attributes_qualified[name]};
        }
        let lazy = _lazy_attribute(report, name);
        if lazy.ok != true { return lazy; }
        if lazy.record != () { return #{ok: true, record: lazy.record}; }
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
        // The source summary leaves large attribute tables unloaded. Fetch a
        // narrow typed record from the same generic analysis API; ambiguity
        // remains visible and is rejected here in the requirements policy.
        let lazy = _lazy_attribute(report, name);
        if lazy.ok != true { return lazy; }
        if lazy.record != () { return #{ok: true, record: lazy.record}; }
        return #{ok: false, error: "SysML attribute is missing: " + name};
    }
    #{ok: true, record: report.attributes[name]}
}

// Lower a selected source literal through the shared Rust SysML/Rhai bridge.
// Callers should include `name` in source_with_attributes() when requesting a
// batch; that preserves native Vec3, enum, quantity and recursive collection
// values without reparsing source text or converting through JSON strings.
fn native_value(report, name) {
    let resolved = attribute(report, name);
    if resolved.ok != true { return resolved; }
    if resolved.record == () {
        return #{ok: false, error: "SysML attribute is missing: " + name};
    }
    let qualified = if resolved.record.qualified_name != () {
        resolved.record.qualified_name
    } else {
        name
    };
    // Compact policy reports may carry selected `attributes` without the
    // original analysis table. Only attempt the no-query fast path when that
    // immutable table is actually present; otherwise resolve this one typed
    // attribute through the same generic API used by unloaded summaries.
    if report.analysis != () {
        let value = sysml_value_from_report(report, qualified);
        if value.ok != true { return value; }
        if value.found == true { return value.value; }
    }
    if report.path == () || report.path == "" {
        return #{ok: false, error: "SysML source report has no resolvable path"};
    }
    let selected = sysml_value(report.path, qualified);
    if selected.ok != true { return selected; }
    selected.value
}

fn number(report, name) {
    let resolved = attribute(report, name);
    if resolved.ok != true { return resolved; }
    if resolved.record.value == () { return (); }
    if resolved.record.value.number_value != () { return resolved.record.value.number_value; }
    ()
}

fn boolean(report, name) {
    let resolved = attribute(report, name);
    if resolved.ok != true { return resolved; }
    if resolved.record.value == ()
        || resolved.record.value.boolean_value == () { return (); }
    resolved.record.value.boolean_value
}

// Return a numeric SysML vector as a native Rhai array. The provider parses
// the authored collection once; callers never split CSV strings or round-trip
// through JSON. A non-numeric vector is rejected as a typed-source error.
fn vector(report, name) {
    let resolved = attribute(report, name);
    if resolved.ok != true { return resolved; }
    if resolved.record.value == () || resolved.record.value.kind != "vector"
        || resolved.record.value.elements == () {
        return ();
    }
    if resolved.record.declared_type == ()
        || resolved.record.declared_type.dimensions == ()
        || resolved.record.declared_type.dimensions.len() != 1
        || !_numeric_sysml_type(resolved.record.declared_type) {
        return ();
    }
    if resolved.record.declared_type.dimensions[0] != resolved.record.value.elements.len() {
        return ();
    }
    let result = [];
    for element in resolved.record.value.elements {
        if element.number_value == () { return (); }
        result.push(element.number_value);
    }
    result
}

// Return declared String[N] textual data as a native Rhai array. Engineering
// identities should be SysML enumerations and are projected by native_value;
// this helper is retained for actual prose/text collections only.
fn string_vector(report, name) {
    let resolved = attribute(report, name);
    if resolved.ok != true { return resolved; }
    if resolved.record.value == ()
        || resolved.record.value.kind != "vector"
        || resolved.record.value.elements == ()
        || resolved.record.declared_type == ()
        || !_string_sysml_type(resolved.record.declared_type)
        || resolved.record.declared_type.dimensions == ()
        || resolved.record.declared_type.dimensions.len() != 1
        || resolved.record.declared_type.dimensions[0] != resolved.record.value.elements.len() {
        return ();
    }
    let result = [];
    for element in resolved.record.value.elements {
        if element.kind != "string" || element.literal == () { return (); }
        let value = element.literal;
        if value.len() < 2 || !value.starts_with("\"") || !value.ends_with("\"") {
            return ();
        }
        result.push(_text(value));
    }
    result
}

fn text(report, name) {
    let resolved = attribute(report, name);
    if resolved.ok != true { return resolved; }
    if resolved.record.value == () { return (); }
    _text(resolved.record.value.literal)
}

// Build lookup tables once per evaluation.  Component checks can be numerous
// (one transform observation per station), so repeatedly walking the complete
// SysML requirement and verification arrays makes an otherwise linear test
// quadratic and can exhaust Rhai's per-tick operation budget.
fn _indexes(report) {
    let requirements = #{};
    let requirement_identities = #{};
    let requirement_handles = #{};
    let requirement_handle_ambiguities = #{};
    if report.requirement_records != () {
        for record in report.requirement_records {
            let name = _record_name(record);
            if name != "" {
                let handle = record.element.handle;
                requirements[name] = true;
                requirements[_short_name(name)] = true;
                if requirement_handles[name] == () {
                    requirement_handles[name] = handle;
                } else if requirement_handles[name] != handle {
                    requirement_handle_ambiguities[name] = true;
                }
                let handle_short = _short_name(name);
                if requirement_handles[handle_short] == () {
                    requirement_handles[handle_short] = handle;
                } else if requirement_handles[handle_short] != handle {
                    requirement_handle_ambiguities[handle_short] = true;
                }
                if requirement_identities[name] == () {
                    requirement_identities[name] = name;
                }
                let short = _short_name(name);
                if requirement_identities[short] == () {
                    requirement_identities[short] = name;
                } else if requirement_identities[short] != name {
                    requirement_identities[short] = "<ambiguous>";
                }
            }
        }
    }
    let verification_identities = #{};
    let verification_handles = #{};
    let verification_handle_ambiguities = #{};
    if report.verification_records != () {
        for verification in report.verification_records {
            let name = _record_name(verification);
            if name == "" { continue; }
            let handle = verification.element.handle;
            if verification_handles[name] == () {
                verification_handles[name] = handle;
            } else if verification_handles[name] != handle {
                verification_handle_ambiguities[name] = true;
            }
            if verification_identities[name] == () {
                verification_identities[name] = name;
            }
            let short = _short_name(name);
            if verification_handles[short] == () {
                verification_handles[short] = handle;
            } else if verification_handles[short] != handle {
                verification_handle_ambiguities[short] = true;
            }
            if verification_identities[short] == () {
                verification_identities[short] = name;
            } else if verification_identities[short] != name {
                verification_identities[short] = "<ambiguous>";
            }
        }
    }
    #{requirements: requirements,
      coverage_graph: report.requirement_coverage,
      requirement_identities: requirement_identities,
      verification_identities: verification_identities,
      requirement_handles: requirement_handles,
      verification_handles: verification_handles,
      requirement_handle_ambiguities: requirement_handle_ambiguities,
      verification_handle_ambiguities: verification_handle_ambiguities}
}

fn _indexed_requirement(indexes, expected) {
    expected != () && expected != ""
        && (indexes.requirements[expected] == true
            || indexes.requirements[_short_name(expected)] == true)
}

fn _indexed_coverage(indexes, requirement, verification_name) {
    if requirement == () || verification_name == () { return false; }
    if indexes.requirement_handle_ambiguities[requirement] == true
        || indexes.requirement_handle_ambiguities[_short_name(requirement)] == true
        || indexes.verification_handle_ambiguities[verification_name] == true
        || indexes.verification_handle_ambiguities[_short_name(verification_name)] == true {
        return false;
    }
    let requirement_handle = indexes.requirement_handles[requirement];
    if requirement_handle == () {
        requirement_handle = indexes.requirement_handles[_short_name(requirement)];
    }
    let verification_handle = indexes.verification_handles[verification_name];
    if verification_handle == () {
        verification_handle = indexes.verification_handles[_short_name(verification_name)];
    }
    if requirement_handle == () || verification_handle == () { return false; }
    indexes.coverage_graph.covers(verification_handle, requirement_handle)
}

fn _expected(report, check) {
    if check.expected_attr == () { return #{ok: true, value: check.expected}; }
    let resolved = attribute(report, check.expected_attr);
    if resolved.ok != true { return resolved; }
    if resolved.record.value == () {
        return #{ok: false,
            error: "SysML attribute has no literal value: " + check.expected_attr};
    }
    if resolved.record.value.number_value != () {
        return #{ok: true, value: resolved.record.value.number_value};
    }
    if resolved.record.value.boolean_value != () {
        return #{ok: true, value: resolved.record.value.boolean_value};
    }
    if check.expected_index != () {
        let value = native_value(report, check.expected_attr);
        let indices = if type_of(check.expected_index) == "array" {
            check.expected_index
        } else { [check.expected_index] };
        for index in indices {
            if type_of(value) == "array" && value.len() > index {
                value = value[index];
            } else if type_of(value) == "Vec3" {
                if index == 0 { value = value.x; }
                else if index == 1 { value = value.y; }
                else if index == 2 { value = value.z; }
                else { return #{ok: false, error: "SysML expected vector index is out of range: " + check.expected_attr}; }
            } else {
                return #{ok: false, error: "SysML expected vector component is unavailable: " + check.expected_attr};
            }
        }
        return #{ok: true, value: value};
    }
    #{ok: true, value: _text(resolved.record.value.literal)}
}

/// Create a generic requirement-linked SysML constraint check record.
/// `bindings` is keyed by declared constraint parameter name or qualified
/// name. Each value may be a native typed value or an explicit provider
/// observation map (`provider`, `state`, `value`, and optional `detail`).
/// Constraint logic remains authored in SysML.
fn constraint_check(id, component, requirement, verification,
                    constraint_name, bindings, provider, tolerance) {
    #{id: id, component: component, requirement: requirement,
      verification: verification, kind: "constraint",
      constraint: constraint_name,
      bindings: if bindings == () { #{} } else { bindings },
      provider: if provider == () || provider == "" { "derived" } else { provider },
      tolerance: if tolerance == () { 0.0 } else { tolerance }}
}

fn _constraint_observations(model, check) {
    if check.constraint == () || check.constraint == "" {
        return #{ok: false, error: "SysML constraint identity is missing"};
    }
    let ir = model.constraint_ir(check.constraint);
    if ir == () || ir.valid != true || ir.constraint == () {
        let message = "SysML constraint is unavailable or invalid: " + check.constraint;
        if ir != () && ir.diagnostics != () && ir.diagnostics.len() > 0 {
            message += "; " + ir.diagnostics[0].message;
        }
        return #{ok: false, error: message, ir: ir};
    }
    let bindings = if check.bindings == () { #{} } else { check.bindings };
    let observations = #{};
    let known_names = #{};
    for parameter in ir.constraint.parameters {
        known_names[parameter.name] = parameter.qualified_name;
        known_names[parameter.qualified_name] = parameter.qualified_name;
    }
    // Unknown authored keys are a binding error. Never silently ignore a typo
    // and turn an absent observation into an apparent constraint pass.
    for name in bindings.keys() {
        if known_names[name] == () {
            return #{ok: false,
                error: "binding names no SysML constraint parameter: " + name,
                ir: ir};
        }
    }
    for parameter in ir.constraint.parameters {
        let supplied = bindings[parameter.name];
        if supplied == () { supplied = bindings[parameter.qualified_name]; }
        if supplied == () {
            observations[parameter.qualified_name] = #{
                provider: check.provider, state: "unavailable",
                detail: "No observation was supplied for this SysML constraint parameter"};
            continue;
        }
        if type_of(supplied) == "map" {
            let observation = supplied;
            if observation.provider == () { observation.provider = check.provider; }
            if observation.state == () { observation.state = "value"; }
            observations[parameter.qualified_name] = observation;
        } else {
            observations[parameter.qualified_name] = #{
                provider: check.provider, state: "value", value: supplied};
        }
    }
    #{ok: true, observations: observations, ir: ir}
}

fn _constraint_is_required(report, requirement, constraint_name) {
    let identity = requirement_name(report, requirement);
    if identity.ok != true {
        return #{ok: false, error: identity.error};
    }
    let record = ();
    for candidate in report.requirement_records {
        if _record_name(candidate) == identity.name {
            record = candidate;
            break;
        }
    }
    if record == () {
        return #{ok: false,
            error: "SysML requirement record is unavailable: " + identity.name};
    }
    if record.constraints == () {
        return #{ok: false,
            error: "SysML requirement has no projected require/assume memberships: " + identity.name};
    }
    for membership in record.constraints {
        if membership.kind != "requirement" { continue; }
        if membership.qualified_name == constraint_name
            || membership.definition_name == constraint_name {
            return #{ok: true, requirement: identity.name,
                usage: membership.qualified_name,
                definition: membership.definition_name};
        }
    }
    #{ok: false, error: "constraint is not a required constraint of "
        + identity.name + ": " + constraint_name}
}

fn _check_constraint(report, check, model) {
    if !sysml_model_is(model) {
        return #{ok: false, id: check.id, component: check.component,
            requirement: check.requirement, verification: check.verification,
            kind: check.kind, constraint: check.constraint, verdict: "error",
            error: "source-pinned SysML model is unavailable"};
    }
    if model.source_revision != report.source_revision {
        return #{ok: false, id: check.id, component: check.component,
            requirement: check.requirement, verification: check.verification,
            kind: check.kind, constraint: check.constraint, verdict: "error",
            error: "SysML source revision changed between requirement analysis and constraint binding",
            analyzed_source_revision: report.source_revision,
            constraint_source_revision: model.source_revision};
    }
    let membership = _constraint_is_required(report, check.requirement,
        check.constraint);
    if membership.ok != true {
        return #{ok: false, id: check.id, component: check.component,
            requirement: check.requirement, verification: check.verification,
            kind: check.kind, constraint: check.constraint, verdict: "error",
            error: membership.error,
            source_revision: report.source_revision};
    }
    let bound = _constraint_observations(model, check);
    if bound.ok != true {
        return #{ok: false, id: check.id, component: check.component,
            requirement: check.requirement, verification: check.verification,
            kind: check.kind, constraint: check.constraint, verdict: "error",
            error: bound.error,
            diagnostics: if bound.ir == () { [] } else { bound.ir.diagnostics }};
    }
    let tolerance = if check.tolerance == () { 0.0 } else { check.tolerance };
    if !_numeric(tolerance) || tolerance < 0.0 {
        return #{ok: false, id: check.id, component: check.component,
            requirement: check.requirement, verification: check.verification,
            kind: check.kind, constraint: check.constraint, verdict: "error",
            error: "constraint comparison tolerance must be a finite non-negative number"};
    }
    let evaluation = model.evaluate_constraint(check.constraint,
        bound.observations, tolerance, tolerance);
    if evaluation == () || evaluation.verdict == () {
        return #{ok: false, id: check.id, component: check.component,
            requirement: check.requirement, verification: check.verification,
            kind: check.kind, constraint: check.constraint, verdict: "error",
            error: "SysML constraint evaluator returned no verdict",
            observations: bound.observations};
    }
    #{ok: evaluation.verdict == "pass", id: check.id,
      component: check.component, requirement: check.requirement,
      verification: check.verification, kind: check.kind,
      constraint: check.constraint, verdict: evaluation.verdict,
      error: if evaluation.verdict == "pass" { "" }
        else { "SysML constraint verdict: " + evaluation.verdict },
      constraint_fingerprint: bound.ir.constraint.fingerprint,
      constraint_source: bound.ir.constraint.source,
      required_constraint_usage: membership.usage,
      required_constraint_definition: membership.definition,
      expression_results: evaluation.expression_results,
      diagnostics: evaluation.diagnostics,
      observations: bound.observations,
      source_revision: report.source_revision,
      tolerance: tolerance}
}

fn _failure(check, message) {
    #{ ok: false, id: check.id, component: check.component,
       requirement: check.requirement, verification: check.verification,
       kind: check.kind, path: check.path, doc_id: check.doc_id,
       error: message }
}

fn _check_one(report, check, indexes, model) {
    if check == () || check.id == () || check.requirement == ()
        || check.verification == () || check.kind == () {
        return #{ok: false, id: "<invalid>", error: "check requires id, requirement, verification, and kind"};
    }
    if !_indexed_requirement(indexes, check.requirement) {
        return _failure(check, "SysML requirement usage is missing");
    }
    if !_indexed_coverage(indexes, check.requirement, check.verification) {
        return _failure(check, "no SysML verification case covers this requirement");
    }
    if check.kind == "constraint" {
        return _check_constraint(report, check, model);
    }

    let expected_result = _expected(report, check);
    if expected_result.ok != true {
        return _failure(check, expected_result.error);
    }
    if check.kind == "coverage" { return #{ok: true, id: check.id, component: check.component, requirement: check.requirement}; }

    if check.kind == "source_reference_available" {
        let resolved = referenced_attribute(report, check.source_owner,
            check.reference_feature);
        if resolved.ok != true {
            return _failure(check, resolved.error);
        }
        if resolved.record.value == () {
            return _failure(check, "referenced SysML source value is missing");
        }
        return #{ok: true, id: check.id, component: check.component,
            requirement: check.requirement,
            source_feature: resolved.record.handle,
            source_span: #{file: resolved.record.file,
                start: resolved.record.start, end: resolved.record.end}};
    }

    if check.kind == "source_collection_count" {
        if check.actual_count == () || check.expected_count == ()
            || !_numeric(check.actual_count) || !_numeric(check.expected_count) {
            return _failure(check, "typed source collection/count values are unavailable");
        }
        if check.actual_count != check.expected_count {
            return #{ok: false, id: check.id, component: check.component,
                requirement: check.requirement,
                error: "typed source collection length disagrees with its declared count",
                actual: check.actual_count, expected: check.expected_count};
        }
        return #{ok: true, id: check.id, component: check.component,
            requirement: check.requirement,
            actual: check.actual_count, expected: check.expected_count};
    }

    // Compare a typed SysML collection with its declared count. This is a
    // generic source-side check: Twin tests still verify every composed USD
    // path separately, while this catches a stale list/count contract before
    // a loop can silently omit a component. No authored literal is split.
    if check.kind == "source_list_count" {
        if check.list_attr == () || check.list_attr == "" {
            return _failure(check, "SysML identity list attribute is missing");
        }
        let resolved = attribute(report, check.list_attr);
        if resolved.ok != true {
            return _failure(check, resolved.error);
        }
        if resolved.record.value == () {
            return _failure(check, "SysML identity list is missing");
        }
        // Use the native typed projection for every collection kind. It
        // preserves the source-declared member type instead of forcing the
        // values through a String/numeric-vector compatibility decoder.
        let values = native_value(report, check.list_attr);
        if type_of(values) == "map" && values.ok == false {
            return _failure(check, values.error);
        }
        if values == () || type_of(values) != "array" {
            return _failure(check, "SysML collection is not a typed array value");
        }
        let actual = values.len();
        let expected = expected_result.value;
        if expected == () { return _failure(check, "SysML expected count is missing"); }
        if actual.to_float() != expected.to_float() {
            return #{ok: false, id: check.id, component: check.component,
                requirement: check.requirement, path: check.path,
                error: "SysML collection length disagrees with declared count",
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
        let record = _query(check.path, ["visibility"], check.doc_id);
        if !_ok(record) { return _failure(check, "USD prim is missing"); }
        if check.expected_type != () && record.type_name != check.expected_type {
            return _failure(check, "USD type is " + record.type_name + ", expected " + check.expected_type);
        }
        if check.visible == true && record.attrs["visibility"] == "invisible" {
            return _failure(check, "USD prim is invisible");
        }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path, doc_id: check.doc_id};
    }
    if check.kind == "not_exists" {
        let record = _query(check.path, (), check.doc_id);
        if _ok(record) {
            return #{ok: false, id: check.id, component: check.component,
                requirement: check.requirement, verification: check.verification,
                kind: check.kind, path: check.path, doc_id: check.doc_id,
                error: "USD prim that must be absent is present",
                actual_type: record.type_name};
        }
        return #{ok: true, id: check.id, component: check.component,
            requirement: check.requirement, verification: check.verification,
            kind: check.kind, path: check.path, doc_id: check.doc_id};
    }
    if check.kind == "children" {
        let missing = [];
        for child_path in check.paths {
            let child = _query(child_path, ["visibility"], check.doc_id);
            if !_ok(child) || check.visible == true && child.attrs["visibility"] == "invisible" {
                missing.push(child_path);
            }
        }
        if missing.len() > 0 { return _failure(check, "missing/invisible USD children: " + missing); }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path, doc_id: check.doc_id};
    }
    if check.kind == "attribute" {
        let actual_record = _query(check.path, [check.attr], check.doc_id);
        if !_ok(actual_record) || actual_record.attrs[check.attr] == () {
            return _failure(check, "USD attribute " + check.attr + " is missing");
        }
        let actual = actual_record.attrs[check.attr];
        let expected = expected_result.value;
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
                requirement: check.requirement, path: check.path, doc_id: check.doc_id,
                error: "USD attribute disagrees with SysML expected value",
                actual: actual, expected: expected};
        }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path, doc_id: check.doc_id, actual: actual, expected: expected};
    }
    if check.kind == "attribute_component" {
        let actual_record = _query(check.path, [check.attr], check.doc_id);
        if !_ok(actual_record) || actual_record.attrs[check.attr] == () {
            return _failure(check, "USD array attribute " + check.attr + " is missing");
        }
        let actual = actual_record.attrs[check.attr];
        let indices = if check.index_path != () { check.index_path }
            else if check.index != () { [check.index] } else { [] };
        if indices.len() == 0 { return _failure(check, "USD array component path is missing"); }
        for index in indices {
            if type_of(actual) != "array" || index < 0 || actual.len() <= index {
                return _failure(check, "USD array component is missing");
            }
            actual = actual[index];
        }
        let expected = expected_result.value;
        let tolerance = if check.tolerance == () { 0.001 } else { check.tolerance };
        if !_close(actual, expected, tolerance) { return _failure(check, "USD array component disagrees with expected value"); }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path, doc_id: check.doc_id, actual: actual, expected: expected};
    }
    if check.kind == "extent_component" {
        let actual_record = _query(check.path, ["size", "xformOp:scale"], check.doc_id);
        if !_ok(actual_record) || actual_record.attrs["size"] == ()
            || actual_record.attrs["xformOp:scale"] == () {
            return _failure(check, "USD extent source attributes are missing");
        }
        let scale = actual_record.attrs["xformOp:scale"];
        if check.index == () || scale.len() <= check.index { return _failure(check, "USD extent vector component is missing"); }
        let multiplier = if check.multiplier == () { 1.0 } else { check.multiplier };
        let actual = actual_record.attrs["size"] * scale[check.index] * multiplier;
        let expected = expected_result.value;
        let tolerance = if check.tolerance == () { 0.001 } else { check.tolerance };
        if !_close(actual, expected, tolerance) {
            return #{ok: false, id: check.id, component: check.component,
                requirement: check.requirement, path: check.path, doc_id: check.doc_id,
                error: "derived USD extent disagrees with SysML expected value",
                actual: actual, expected: expected, tolerance: tolerance};
        }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path, doc_id: check.doc_id, actual: actual, expected: expected};
    }
    if check.kind == "bounds_component" {
        let is_collision = check.representation == "collision";
        let representation = if is_collision { "collision_bounds" }
            else { "geometry_bounds" };
        let representation_label = if is_collision { "collision" } else { "geometry" };
        let request = #{path: check.path};
        if representation == "collision_bounds" { request.collision_bounds = true; }
        else { request.geometry_bounds = true; }
        if check.doc_id != () { request.doc_id = check.doc_id; }
        let actual_record = query("QueryUsdPrim", request);
        if !_ok(actual_record) || actual_record[representation] == () {
            return _failure(check, "composed USD " + representation_label + " bounds are missing");
        }
        let bounds = actual_record[representation];
        if bounds == () || bounds.half_extents == ()
            || check.index == () || bounds.half_extents.len() <= check.index {
            return _failure(check, "composed USD bounds component is missing");
        }
        let multiplier = if check.multiplier == () { 1.0 } else { check.multiplier };
        let actual = bounds.half_extents[check.index] * multiplier;
        let expected = expected_result.value;
        if expected == () { return _failure(check, "SysML expected attribute is missing"); }
        let tolerance = if check.tolerance == () { 0.001 } else { check.tolerance };
        if !_close(actual, expected, tolerance) {
            return #{ok: false, id: check.id, component: check.component,
                requirement: check.requirement, path: check.path, doc_id: check.doc_id,
                error: "composed USD bound disagrees with SysML expected value",
                actual: actual, expected: expected, tolerance: tolerance};
        }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement,
            path: check.path, doc_id: check.doc_id, representation: representation,
            actual: actual, expected: expected};
    }
    if check.kind == "attribute_equals" {
        let actual_record = _query(check.path, [check.attr], check.doc_id);
        if !_ok(actual_record) || actual_record.attrs[check.attr] == () { return _failure(check, "USD attribute is missing"); }
        let actual = actual_record.attrs[check.attr];
        let expected = expected_result.value;
        // QueryUsdPrim preserves the canonical quoted literal for some
        // authored string opinions.  Compare string content, not the literal
        // delimiters, while keeping numeric/bool equality exact.
        if !_equal_value(actual, expected) { return _failure(check, "USD attribute does not equal expected value"); }
        return #{ok: true, id: check.id, component: check.component, requirement: check.requirement, path: check.path, doc_id: check.doc_id};
    }
    if check.kind == "relationship" {
        if check.rel == () || check.rel == "" {
            return _failure(check, "USD relationship name is missing");
        }
        let request = #{path: check.path, rels: [check.rel]};
        if check.doc_id != () { request.doc_id = check.doc_id; }
        let actual_record = query("QueryUsdPrim", request);
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
            requirement: check.requirement, path: check.path, doc_id: check.doc_id, actual: actual,
            expected: expected};
    }
    _failure(check, "unknown generic SysML/USD check kind " + check.kind)
}

/// Evaluate a Twin-authored check table. Returns one result per component
/// check, preserving IDs and paths for a test report or UI evidence panel.
fn evaluate(source_report, checks) {
    if checks != () && checks.len() > 64 {
        // Large tables must use the streaming path so direct callers cannot
        // accidentally construct an oversized aggregate.  The stream has no
        // channel here; callers that need live evidence should call it with a
        // channel explicitly when streaming a large result set.
        return evaluate_stream(source_report, checks, "", "");
    }
    // Keep each value crossing the Rhai boundary bounded. A component may
    // legitimately emit hundreds of observations; retaining one large array
    // can trip Rhai's 64 KiB string/value limit. Chunks preserve every typed
    // observation without raising the global engine limit.
    let results = [];
    let result_chunks = [];
    let result_chunk = [];
    let failures = [];
    let failure_chunks = [];
    let failure_chunk = [];
    if type_of(source_report) != "map" || source_report.ok != true {
        let message = _report_error(source_report, "SysML source report is unavailable");
        return #{ok: false, results: [], failures: [message], error: message};
    }
    let indexes = _indexes(source_report);
    let identities = _resolve_check_identities(checks, indexes);
    if identities.ok != true {
        let failure = #{ok: false, id: identities.check_id,
            error: identities.error};
        return #{ok: false, results: [failure], failures: [failure],
            check_count: 0, requirement_count: 0, failure_count: 1};
    }
    let needs_model = false;
    for check in checks {
        if check.kind == "constraint" { needs_model = true; }
    }
    let model = ();
    if needs_model {
        if source_report.path == () || source_report.path == "" {
            let failure = #{ok: false,
                error: "SysML source report has no resolvable model path"};
            return #{ok: false, results: [failure], failures: [failure],
                check_count: 0, requirement_count: 0, failure_count: 1};
        }
        model = sysml_model(source_report.path);
    }
    for check in checks {
        let result = _check_one(source_report, check, indexes, model);
        // Stamp canonical source identities only on emitted evidence.  The
        // authored check stays short and is never duplicated for every leaf
        // observation, while reports remain self-describing.
        result.requirement = indexes.requirement_identities[check.requirement];
        result.verification = indexes.verification_identities[check.verification];
        if result.component == () && check.component != () { result.component = check.component; }
        if result.kind == () { result.kind = check.kind; }
        result_chunk.push(result);
        if result_chunk.len() >= 64 {
            result_chunks.push(result_chunk);
            result_chunk = [];
        }
        if result.ok != true {
            // Keep the compatibility array bounded while preserving every
            // failure in chunked evidence.
            if failures.len() < 64 { failures.push(result); }
            failure_chunk.push(result);
            if failure_chunk.len() >= 64 {
                failure_chunks.push(failure_chunk);
                failure_chunk = [];
            }
        }
    }
    if result_chunk.len() > 0 { result_chunks.push(result_chunk); }
    if failure_chunk.len() > 0 { failure_chunks.push(failure_chunk); }
    // Keep the flattened compatibility field only for small reports. Large
    // failure sets use bounded chunks as well, avoiding a second oversized
    // aggregate while retaining every failure record.
    let total_failures = 0;
    for chunk in failure_chunks { total_failures += chunk.len(); }
    if total_failures <= 64 {
        for chunk in result_chunks {
            for result in chunk { results.push(result); }
        }
    } else { failures = []; }
    let verification = "";
    if checks.len() > 0 && checks[0].verification != () {
        verification = indexes.verification_identities[checks[0].verification];
    }
    // A component observer normally expands one SysML requirement over many
    // repeated instances and leaf observations.  Keep those checks as the
    // durable evidence table, but do not call the observation count a
    // requirement count: doing so made a four-tank component appear to have
    // hundreds of requirements.  The short requirement identity is stable in
    // the check records and is sufficient for this bounded aggregate.
    let requirement_summary = #{};
    let requirement_names = [];
    for chunk in result_chunks {
        for result in chunk {
            let requirement = result.requirement;
            if requirement == () || requirement == "" { continue; }
            if requirement_summary[requirement] == () {
                requirement_summary[requirement] = #{checks: 0, failures: 0};
                requirement_names.push(requirement);
            }
            requirement_summary[requirement].checks += 1;
            if result.ok != true {
                requirement_summary[requirement].failures += 1;
            }
        }
    }
    #{ok: total_failures == 0, results: results, failures: failures,
      result_chunks: result_chunks, failure_chunks: failure_chunks,
      // `check_count` is the number of concrete USD/source observations.
      check_count: checks.len(),
      // `requirement_count` is the number of unique SysML requirement usages
      // represented by those observations.
      requirement_count: requirement_names.len(),
      requirement_names: requirement_names,
      requirement_summary: requirement_summary,
      failure_count: total_failures, verification: verification,
      source_revision: source_report.source_revision,
      source_files: source_report.source_files}
}

// Evaluate a large table while streaming each bounded evidence record.  This
// is the production path for component observers with more than 64 checks:
// no aggregate result array is constructed, yet every typed observation is
// available to the same evidence channel as a small evaluation.
fn evaluate_stream(source_report, checks, channel, title) {
    if type_of(source_report) != "map" || source_report.ok != true {
        let message = _report_error(source_report, "SysML source report is unavailable");
        return #{ok: false, results: [], failures: [message], error: message,
            check_count: 0, requirement_count: 0, failure_count: 1,
            streamed: true};
    }
    let indexes = _indexes(source_report);
    let identities = _resolve_check_identities(checks, indexes);
    if identities.ok != true {
        let failure = #{ok: false, id: identities.check_id,
            error: identities.error};
        return #{ok: false, results: [failure], failures: [failure],
            check_count: 0, requirement_count: 0, failure_count: 1,
            streamed: true};
    }
    let needs_model = false;
    for check in checks {
        if check.kind == "constraint" { needs_model = true; }
    }
    let model = ();
    if needs_model {
        if source_report.path == () || source_report.path == "" {
            let failure = #{ok: false,
                error: "SysML source report has no resolvable model path"};
            return #{ok: false, results: [failure], failures: [failure],
                check_count: 0, requirement_count: 0, failure_count: 1,
                streamed: true};
        }
        model = sysml_model(source_report.path);
    }
    let verification = "";
    if checks.len() > 0 && checks[0].verification != () {
        verification = indexes.verification_identities[checks[0].verification];
    }
    let failure_count = 0;
    let failures = [];
    let requirement_summary = #{};
    let requirement_names = [];
    let result_index = 0;
    for check in checks {
        let result = _check_one(source_report, check, indexes, model);
        result.requirement = indexes.requirement_identities[check.requirement];
        result.verification = indexes.verification_identities[check.verification];
        if result.component == () && check.component != () { result.component = check.component; }
        if result.kind == () { result.kind = check.kind; }
        if channel != () && channel != "" {
            emit(channel + "_EVIDENCE_RESULT", #{schema_version: 1,
                title: title, channel: channel, verification: verification,
                source_revision: source_report.source_revision,
                result_count: checks.len(), failure_count: (),
                result_index: result_index, result: result});
        }
        result_index += 1;
        if result.ok != true {
            failure_count += 1;
            if failures.len() < 64 { failures.push(result); }
        }
        let requirement = result.requirement;
        if requirement != () && requirement != "" {
            if requirement_summary[requirement] == () {
                requirement_summary[requirement] = #{checks: 0, failures: 0};
                requirement_names.push(requirement);
            }
            requirement_summary[requirement].checks += 1;
            if result.ok != true { requirement_summary[requirement].failures += 1; }
        }
    }
    if channel != () && channel != "" {
        emit(channel + "_EVIDENCE_STREAM_SUMMARY", #{schema_version: 1,
            title: title, channel: channel, verification: verification,
            source_revision: source_report.source_revision,
            result_count: checks.len(), failure_count: failure_count,
            requirement_count: requirement_names.len(),
            requirement_names: requirement_names,
            requirement_summary: requirement_summary});
    }
    #{ok: failure_count == 0, results: [], failures: failures,
      result_chunks: [], failure_chunks: [], streamed: true,
      check_count: checks.len(), requirement_count: requirement_names.len(),
      requirement_names: requirement_names, requirement_summary: requirement_summary,
      failure_count: failure_count, verification: verification,
      source_revision: source_report.source_revision,
      source_files: source_report.source_files}
}

fn _document_checks(doc_id, checks) {
    let scoped = [];
    for check in checks {
        if type_of(check) != "map" {
            scoped.push(check);
            continue;
        }
        let copy = #{};
        for name in check.keys() { copy[name] = check[name]; }
        copy.doc_id = doc_id;
        scoped.push(copy);
    }
    scoped
}

/// Evaluate generic requirement checks against an explicit open Editor
/// document. The document identifier is attached to each USD observation, so
/// no observer can accidentally read a different active stage.
fn evaluate_document(source_report, doc_id, checks) {
    if doc_id == () {
        return #{ok: false, results: [], failures: [
            "document-scoped SysML evaluation requires an explicit doc_id"],
            check_count: 0, requirement_count: 0, failure_count: 1};
    }
    evaluate(source_report, _document_checks(doc_id, checks))
}

/// Streaming counterpart of `evaluate_document` for bounded Editor evidence.
fn evaluate_document_stream(source_report, doc_id, checks, channel, title) {
    if doc_id == () {
        let failure = #{ok: false,
            error: "document-scoped SysML evaluation requires an explicit doc_id"};
        return #{ok: false, results: [failure], failures: [failure],
            check_count: 0, requirement_count: 0, failure_count: 1,
            streamed: true};
    }
    evaluate_stream(source_report, _document_checks(doc_id, checks), channel, title)
}

/// Build the canonical source report for the active Twin. Parsing and typed
/// facts come from Rust; this Rhai policy joins manifest ownership with
/// source-backed requirement and verification identities.
fn source() {
    _source_projection([], [], [], false, true)
}

/// Open the active Twin's immutable, revision-pinned Rust SysML session.
///
/// Use this for repeated typed lookups inside one builder or verifier. The
/// session is source-backed and read-only; Rhai must not manufacture a second
/// requirement-value table or read `.sysml` files directly.
fn model() {
    let name = twin_name();
    if name == "" { return (); }
    sysml_model("twin://" + name)
}

/// Select only typed values for a value-only policy pass.
///
/// This projection intentionally has no requirement/verification identity
/// tables. It is safe for numeric/model-building reads, but must not be used
/// to emit requirement-linked evidence; use source_with_selection() for that.
fn source_with_attributes(names) {
    // Numeric/typed consumers do not need the Twin-wide requirement and
    // verification registries. Keep this projection proportional to the
    // caller's selected attributes; callers that need identities use
    // source_with_selection, and full contract audits use source().
    _source_projection(names, [], [], false, false)
}

/// Select only the attributes, requirement usages and verification cases
/// needed by one policy pass. The Rust SysML projection performs the table
/// filtering against a single immutable source snapshot before Rhai receives
/// it, keeping large Twins within Rhai's bounded value budget. This narrowly
/// selected report intentionally omits the Twin-wide component/verification
/// registry; the selected SysML identities are evaluated directly below.
fn source_with_selection(attributes, requirements, verifications) {
    _source_projection(attributes, requirements, verifications, false, false)
}

/// Select the constraint graph for the geometry/Modelica policy.
fn source_with_constraints(names) {
    _source_projection(names, [], [], true, false)
}

// Full Twin reports use a small bounded query window and retain only the
// source identities and verification links needed by this policy. Rich
// requirement details remain available through source_with_selection().
fn _compact_requirement_identity(record) {
    let name = _record_name(record);
    #{element: #{qualified_name: name, handle: record.element.handle}, qualified_name: name,
      kind: record.kind}
}

fn _compact_verification_identity(record) {
    let name = _record_name(record);
    #{element: #{qualified_name: name, handle: record.element.handle},
      qualified_name: name, verifies: record.verifies,
      verified_requirements: record.verified_requirements}
}

fn _paged_twin_source(path, tables) {
    // Keep each returned Dynamic comfortably below Rhai's aggregate string
    // budget. The limit is transport batching, not model or requirement policy.
    let page_limit = 16;
    let offset = 0;
    let page_count = 0;
    let revision = ();
    let requirement_total = ();
    let verification_total = ();
    let assembled = ();
    let requirements = [];
    let verifications = [];
    loop {
        let page = query("AnalyzeSysml", #{path: path, tables: tables,
            offset: offset, limit: page_limit});
        if page == () || page.analysis == () {
            return #{ok: false, path: path, error: _report_error(page,
                "AnalyzeSysml page omitted its semantic facts")};
        }
        let page_revision = page.analysis.source_revision;
        if page_revision == () {
            return #{ok: false, path: path,
                error: "AnalyzeSysml page omitted its source revision"};
        }
        if revision == () {
            revision = page_revision;
            assembled = page;
        } else if revision != page_revision {
            return #{ok: false, path: path,
                error: "SysML source revision changed during paged analysis"};
        }
        page_count += 1;
        for record in page.analysis.requirements {
            requirements.push(_compact_requirement_identity(record));
        }
        for record in page.analysis.verifications {
            verifications.push(_compact_verification_identity(record));
        }
        let page_tables = page.analysis.page.tables;
        if page_tables == () || page_tables.requirements == ()
            || page_tables.verifications == () {
            return #{ok: false, path: path,
                error: "AnalyzeSysml page omitted table pagination metadata"};
        }
        if requirement_total == () {
            requirement_total = page_tables.requirements.total;
            verification_total = page_tables.verifications.total;
        } else if requirement_total != page_tables.requirements.total
            || verification_total != page_tables.verifications.total {
            return #{ok: false, path: path,
                error: "SysML fact-table totals changed during paged analysis"};
        }
        if page_tables.requirements.has_more != true
            && page_tables.verifications.has_more != true {
            break;
        }
        offset += page_limit;
    }
    assembled.analysis = #{source_files: assembled.analysis.source_files,
        source_revision: revision,
        requirements: requirements, verifications: verifications};
    assembled.source_page_count = page_count;
    assembled.source_fact_totals = #{requirements: requirement_total,
        verifications: verification_total};
    assembled
}

fn _source_projection(names, requirement_names, verification_names,
                      include_constraints, include_twin_contract) {
    let name = twin_name();
    if name == "" { return #{ok: false, error: "active Twin is unavailable"}; }
    if type_of(names) != "array" {
        return #{ok: false, error: "SysML attribute selector must be an array"};
    }
    let selected = [];
    for literal in names {
        if type_of(literal) != "string" || literal == "" {
            return #{ok: false, error: "SysML attribute selector contains an invalid name"};
        }
        if !selected.contains(literal) { selected.push(literal); }
    }
    let tables = [];
    if include_twin_contract || requirement_names.len() > 0 {
        tables.push("requirements");
    }
    if include_twin_contract || verification_names.len() > 0 {
        tables.push("verifications");
    }
    if selected.len() > 0 { tables.push("attributes"); }
    if include_constraints {
        tables.push("constraints");
        tables.push("relationships");
        tables.push("references");
    }
    let request = #{path: "twin://" + name, tables: tables};
    if selected.len() > 0 { request.attribute_names = selected; }
    if requirement_names.len() > 0 { request.requirement_names = requirement_names; }
    if verification_names.len() > 0 { request.verification_names = verification_names; }
    let analyzed = if include_twin_contract {
        _paged_twin_source(request.path, tables)
    } else {
        query("AnalyzeSysml", request)
    };
    if analyzed == () || analyzed.analysis == () {
        return #{ok: false, error: _report_error(analyzed,
            "AnalyzeSysml could not resolve the Twin SysML source set")};
    }

    let report = #{path: analyzed.path, kind: analyzed.kind,
        ok: analyzed.ok, errors: analyzed.errors, warnings: analyzed.warnings,
        source_files: analyzed.analysis.source_files,
        source_revision: analyzed.analysis.source_revision,
        source_page_count: if analyzed.source_page_count == () { 1 } else { analyzed.source_page_count },
        source_fact_totals: if analyzed.source_fact_totals == () { () } else { analyzed.source_fact_totals },
        requirement_records: analyzed.analysis.requirements,
        verification_records: analyzed.analysis.verifications,
        requirement_coverage: sysml_requirement_coverage(analyzed.analysis.verifications),
        attributes: #{}, attributes_qualified: #{}, attribute_collisions: [],
        constraints: if analyzed.analysis.constraints == () { [] } else { analyzed.analysis.constraints },
        relationships: if analyzed.analysis.relationships == () { [] } else { analyzed.analysis.relationships },
        references: if analyzed.analysis.references == () { [] } else { analyzed.analysis.references }};

    if analyzed.analysis.attributes != () {
        let short_names = #{};
        for attribute in analyzed.analysis.attributes {
            let qualified = attribute.qualified_name;
            let short = attribute.name;
            report.attributes_qualified[qualified] = attribute;
            if short_names[short] == () {
                short_names[short] = [qualified];
            } else {
                short_names[short].push(qualified);
            }
        }
        for short in short_names.keys() {
            if short_names[short].len() > 1 {
                report.attribute_collisions.push(#{name: short,
                    qualified_names: short_names[short]});
            }
        }
    }

    if !include_twin_contract { return report; }
    let contract = query("ReadActiveTwinContract", #{});
    if contract == () || contract.open != true {
        report.ok = false;
        report.errors.push(_report_error(contract,
            "active Twin contract is unavailable"));
        report.components = [];
        report.component_registry_errors = ["active Twin contract is unavailable"];
        report.verification_registry = [];
        report.verification_registry_errors = ["active Twin contract is unavailable"];
        return report;
    }
    report.components = contract.components;
    report.component_registry_errors = contract.component_errors;
    report.verification_registry = contract.verification_cases;
    report.verification_registry_errors = contract.verification_errors;
    for error in contract.sysml_source_errors {
        let message = "Twin SysML source set: " + error;
        report.errors.push(message);
    }
    for error in report.component_registry_errors {
        report.errors.push("Twin component registry: " + error);
    }
    for candidate_case in report.verification_registry {
        let resolved = verification_name(report, candidate_case.name);
        if resolved.ok != true {
            report.verification_registry_errors.push(resolved.error);
        }
    }
    for error in report.verification_registry_errors {
        report.errors.push("Twin verification registry: " + error);
    }
    report.ok = report.ok == true && report.errors.len() == 0
        && report.component_registry_errors.len() == 0
        && report.verification_registry_errors.len() == 0;
    report
}

fn _literal_text(attribute) {
    if attribute == () || attribute.value == () { return (); }
    if attribute.value.string_value != () { return attribute.value.string_value; }
    if attribute.value.literal_kind == "string" { return attribute.value.literal; }
    ()
}

// Resolve a standard SysML short name to a source-revision-scoped element
// handle. The short name is only a lookup key; subsequent queries follow
// model references by typed handle.
fn requirement_handle_by_short_name(report, short_name) {
    if type_of(report) != "map" || short_name == () || short_name == "" {
        return #{ok: false, error: "SysML requirement short name is missing"};
    }
    let records = report.requirement_records;
    if records == () && report.analysis != () {
        records = report.analysis.requirements;
    }
    if records == () {
        return #{ok: false, error: "SysML requirement records are unavailable"};
    }
    let found = ();
    for record in records {
        if record.element == () || record.element.handle == ()
            || record.element.short_name != short_name { continue; }
        if found != () {
            return #{ok: false,
                error: "ambiguous SysML requirement short name: " + short_name};
        }
        found = record.element.handle;
    }
    if found == () {
        return #{ok: false,
            error: "SysML requirement short name is missing: " + short_name};
    }
    #{ok: true, handle: found}
}

// Follow one authored reference feature to its resolved target attribute.
// This preserves element identity through the generic API instead of storing
// a qualified feature name in another String-valued model attribute.
fn referenced_attribute(report, owner_handle, reference_feature) {
    if type_of(report) != "map" || report.path == () || report.path == ""
        || owner_handle == () || reference_feature == () || reference_feature == "" {
        return #{ok: false, error: "SysML attribute reference is incomplete"};
    }
    let links = query("AnalyzeSysml", #{
        path: report.path,
        tables: ["references"],
        reference_from_owner_handles: [owner_handle],
        reference_from_features: [reference_feature]
    });
    if links == () || links.analysis == () || links.analysis.references == () {
        return #{ok: false,
            error: _report_error(links, "SysML attribute reference is unavailable")};
    }
    let targets = [];
    for link in links.analysis.references {
        if link.target_feature != () { targets.push(link.target_feature); }
    }
    if targets.len() != 1 {
        return #{ok: false, error: if targets.len() == 0 {
            "SysML attribute reference does not resolve to a feature"
        } else { "SysML attribute reference resolves ambiguously" }};
    }
    let selected = query("AnalyzeSysml", #{
        path: report.path,
        tables: ["attributes"],
        attribute_handles: [targets[0]]
    });
    if selected == () || selected.analysis == () || selected.analysis.attributes == ()
        || selected.analysis.attributes.len() != 1 {
        return #{ok: false,
            error: _report_error(selected, "referenced SysML attribute is unavailable")};
    }
    #{ok: true, record: selected.analysis.attributes[0]}
}

fn number_from_reference(report, owner_handle, reference_feature) {
    let resolved = referenced_attribute(report, owner_handle, reference_feature);
    if resolved.ok != true { return resolved; }
    let record = resolved.record;
    if record.value == () || record.value.number_value == ()
        || !_numeric_sysml_type(record.declared_type) {
        return #{ok: false, error: "referenced SysML attribute is not numeric"};
    }
    #{ok: true, value: record.value.number_value, record: record}
}

fn vector_from_reference(report, owner_handle, reference_feature) {
    let resolved = referenced_attribute(report, owner_handle, reference_feature);
    if resolved.ok != true { return resolved; }
    let record = resolved.record;
    if record.value == () || record.value.elements == ()
        || record.declared_type == () || record.declared_type.dimensions == ()
        || record.declared_type.dimensions.len() != 1
        || !_numeric_sysml_type(record.declared_type)
        || record.declared_type.dimensions[0] != record.value.elements.len() {
        return #{ok: false, error: "referenced SysML attribute is not a numeric vector"};
    }
    let result = [];
    for element in record.value.elements {
        if element.number_value == () {
            return #{ok: false, error: "referenced SysML vector contains a non-numeric value"};
        }
        result.push(element.number_value);
    }
    #{ok: true, value: result, record: record}
}

fn string_vector_from_reference(report, owner_handle, reference_feature) {
    let resolved = referenced_attribute(report, owner_handle, reference_feature);
    if resolved.ok != true { return resolved; }
    let record = resolved.record;
    if record.value == () || record.value.elements == ()
        || record.declared_type == () || record.declared_type.dimensions == ()
        || record.declared_type.dimensions.len() != 1
        || !_string_sysml_type(record.declared_type)
        || record.declared_type.dimensions[0] != record.value.elements.len() {
        return #{ok: false, error: "referenced SysML attribute is not a String vector"};
    }
    let result = [];
    for element in record.value.elements {
        if element.string_value == () {
            return #{ok: false, error: "referenced SysML String vector contains a non-text value"};
        }
        result.push(element.string_value);
    }
    #{ok: true, value: result, record: record}
}

fn text_from_reference(report, owner_handle, reference_feature) {
    let resolved = referenced_attribute(report, owner_handle, reference_feature);
    if resolved.ok != true { return resolved; }
    if !_string_sysml_type(resolved.record.declared_type)
        || resolved.record.value == ()
        || resolved.record.value.string_value == () {
        return #{ok: false, error: "referenced SysML attribute is not textual"};
    }
    #{ok: true, value: resolved.record.value.string_value, record: resolved.record}
}

// Follow resolved SysML references from a requirement element handle to its
// evidence item and then to atomic SourceLocator values. No model identity or
// collection of sources is flattened into String data.
fn provenance(report, requirement_handle) {
    provenance_for_requirement(report, requirement_handle)
}

fn provenance_for_requirement(report, requirement_handle) {
    if type_of(report) != "map" || report.path == () || report.path == "" {
        return #{ok: false,
            error: _report_error(report, "SysML source report is unavailable")};
    }
    if type_of(requirement_handle) != "map"
        || requirement_handle.element_id == ()
        || requirement_handle.source_revision == ()
        || requirement_handle.source_fingerprint == () {
        return #{ok: false,
            error: "SysML provenance requires a source-scoped requirement element handle"};
    }

    let requirement_links = query("AnalyzeSysml", #{
        path: report.path,
        tables: ["references"],
        reference_target_handles: [requirement_handle],
        reference_from_features: ["targetRequirement"]
    });
    if requirement_links == () || requirement_links.analysis == ()
        || requirement_links.analysis.references == () {
        return #{ok: false,
            error: _report_error(requirement_links, "requirement evidence link is unavailable")};
    }
    if requirement_links.analysis.references.len() != 1 {
        return #{ok: false, error: if requirement_links.analysis.references.len() == 0 {
            "SysML requirement evidence link is missing for the selected element"
        } else { "ambiguous SysML requirement evidence links for the selected element" }};
    }
    let requirement_link = requirement_links.analysis.references[0];
    let evidence_owner = requirement_link.from_owner;
    if evidence_owner == () {
        return #{ok: false, error: "requirement evidence reference has no owning element"};
    }

    let source_links = query("AnalyzeSysml", #{
        path: report.path,
        tables: ["references"],
        reference_from_owner_handles: [evidence_owner]
    });
    if source_links == () || source_links.analysis == ()
        || source_links.analysis.references == () {
        return #{ok: false,
            error: _report_error(source_links, "evidence source links are unavailable")};
    }

    let sources = [];
    for link in source_links.analysis.references {
        if link.from_feature_name != "sources" { continue; }
        let source_handle = link.target;
        if source_handle == () {
            return #{ok: false, error: "evidence source reference has no resolved target"};
        }
        let source_attributes = query("AnalyzeSysml", #{
            path: report.path,
            tables: ["attributes"],
            attribute_names: ["locator", "role"],
            attribute_owner_handles: [source_handle]
        });
        if source_attributes == () || source_attributes.analysis == ()
            || source_attributes.analysis.attributes == () {
            return #{ok: false,
                error: _report_error(source_attributes, "source metadata is unavailable")};
        }
        let locator_attribute = ();
        let role_attribute = ();
        for attribute in source_attributes.analysis.attributes {
            if attribute.name == "locator" {
                if locator_attribute != () {
                    return #{ok: false, error: "source locator is ambiguous on a linked source element"};
                }
                locator_attribute = attribute;
            } else if attribute.name == "role" {
                if role_attribute != () {
                    return #{ok: false, error: "source role is ambiguous on a linked source element"};
                }
                role_attribute = attribute;
            }
        }
        if locator_attribute == () || role_attribute == () {
            return #{ok: false,
                error: "linked evidence source requires one locator and one role"};
        }
        let locator = _literal_text(locator_attribute);
        let locator_type = if locator_attribute.declared_type != ()
            && locator_attribute.declared_type.resolved_type != () {
            locator_attribute.declared_type.resolved_type.qualified_name
        } else { () };
        let role_type = if role_attribute.declared_type != ()
            && role_attribute.declared_type.category == "Enumeration"
            && role_attribute.declared_type.resolved_type != () {
            role_attribute.declared_type.resolved_type.qualified_name
        } else { () };
        let role_literal = if role_attribute.value != ()
            && role_attribute.value.string_value != () {
            role_attribute.value.string_value
        } else { () };
        if locator == () || locator == "" || locator_type == ()
            || role_type == () || role_literal == () || role_literal == "" {
            return #{ok: false,
                error: "source locator or enumeration role is not a typed value"};
        }
        sources.push(#{
            element: source_handle,
            role: #{type: role_type, literal: role_literal},
            locator_type: locator_type,
            locator: locator,
            source_span: #{file: locator_attribute.file,
                start: locator_attribute.start, end: locator_attribute.end}
        });
    }

    let evidence = #{
        element: evidence_owner,
        requirement: requirement_handle,
        sources: sources,
        source_span: #{file: requirement_link.file,
            start: requirement_link.start, end: requirement_link.end}
    };
    let revision = report.source_revision;
    let files = report.source_files;
    if report.analysis != () {
        if revision == () { revision = report.analysis.source_revision; }
        if files == () { files = report.analysis.source_files; }
    }
    #{ok: true, evidence: evidence,
      source_revision: revision,
      source_files: files}
}
